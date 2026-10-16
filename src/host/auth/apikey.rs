//! API keys for server-to-server clients (`{"mode": "api_keys", "file": …}`),
//! sent as `Authorization: Bearer afx_<id>_<secret>`.
//!
//! A key is 256 random bits, shown once when it is created
//! ([`new_key`], `serve api-key new`); the key file stores only its SHA-256
//! (a high-entropy key needs no slow hash) next to the identity it stands for
//! (user, groups, claims), the dashboards it may read, an optional expiry and
//! a `revoked` flag. The public id selects the entry, the hash is compared in
//! constant time. The file is re-read when it changes, so a revocation takes
//! effect within [`RELOAD_EVERY`] without a restart; a file that no longer
//! parses, or that others may write, disables every key (fail closed). Keys
//! never appear in logs: the audit log names the key id only.

use super::session::ct_eq;
use super::Scope;
use crate::host::access::{clean_claim, valid_claim_name, Claims, MAX_ATTRS, MAX_GROUPS};
use crate::host::serving::valid_dashboard_id;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Every key starts with this.
pub const PREFIX: &str = "afx_";
/// The key file is checked for changes at most this often.
pub const RELOAD_EVERY: Duration = Duration::from_secs(2);
const ID_HEX: usize = 12;
const SECRET_HEX: usize = 64;

/// One key of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEntry {
    pub id: String,
    /// Hex SHA-256 of the whole key string.
    pub sha256: String,
    pub claims: Claims,
    /// `None` = every dashboard.
    pub dashboards: Option<Vec<String>>,
    /// Unix seconds.
    pub expires: Option<u64>,
    pub revoked: bool,
}

/// Hex SHA-256 of `s`.
pub fn sha256_hex(s: &str) -> String {
    let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, s.as_bytes());
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n.div_ceil(2)];
    aws_lc_rs::rand::fill(&mut b).expect("system randomness");
    b.iter().map(|b| format!("{b:02x}")).collect::<String>()[..n].to_string()
}

/// A new key: `(the key — show it once, its id, its hash)`.
pub fn new_key() -> (String, String, String) {
    let id = random_hex(ID_HEX);
    let key = format!("{PREFIX}{id}_{}", random_hex(SECRET_HEX));
    let hash = sha256_hex(&key);
    (key, id, hash)
}

/// The id of a well-formed key.
fn key_id(key: &str) -> Option<&str> {
    let rest = key.strip_prefix(PREFIX)?;
    let (id, secret) = rest.split_once('_')?;
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    (hex(id, ID_HEX) && hex(secret, SECRET_HEX)).then_some(id)
}

/// Parse a key file: `{"keys": [{"id", "sha256", "user", "groups"?,
/// "claims"?, "dashboards"?, "expires"?, "revoked"?, "note"?, "created"?}]}`.
pub fn parse_file(text: &str) -> Result<Vec<KeyEntry>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let keys = v
        .get("keys")
        .and_then(Value::as_array)
        .ok_or("expected {\"keys\": [...]}")?;
    let mut out: Vec<KeyEntry> = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        let bad = |what: &str| format!("keys[{i}]: {what}");
        let o = k.as_object().ok_or_else(|| bad("not an object"))?;
        for f in o.keys() {
            if !matches!(
                f.as_str(),
                "id" | "sha256"
                    | "user"
                    | "groups"
                    | "claims"
                    | "dashboards"
                    | "expires"
                    | "revoked"
                    | "note"
                    | "created"
            ) {
                return Err(bad(&format!("unknown field '{f}'")));
            }
        }
        let id = o
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| s.len() == ID_HEX && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| bad("id must be 12 hex digits"))?;
        if out.iter().any(|e| e.id == id) {
            return Err(bad("duplicate id"));
        }
        let sha256 = o
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| bad("sha256 must be 64 hex digits"))?
            .to_ascii_lowercase();
        let user = clean_claim(o.get("user").and_then(Value::as_str).unwrap_or(""))
            .ok()
            .filter(|u| !u.is_empty())
            .ok_or_else(|| bad("user is required"))?;
        let mut groups = Vec::new();
        if let Some(g) = o.get("groups") {
            let g = g
                .as_array()
                .filter(|g| g.len() <= MAX_GROUPS)
                .ok_or_else(|| bad("groups must be a list"))?;
            for x in g {
                let x = x
                    .as_str()
                    .and_then(|s| clean_claim(s).ok())
                    .ok_or_else(|| bad("bad group"))?;
                if !x.is_empty() {
                    groups.push(x);
                }
            }
        }
        groups.sort();
        groups.dedup();
        let mut attrs = Vec::new();
        if let Some(c) = o.get("claims") {
            let c = c
                .as_object()
                .filter(|c| c.len() <= MAX_ATTRS)
                .ok_or_else(|| bad("claims must be an object"))?;
            for (name, v) in c {
                if !valid_claim_name(name) {
                    return Err(bad(&format!("invalid claim name '{name}'")));
                }
                let v = match v {
                    Value::Null => None,
                    Value::String(s) => Some(clean_claim(s).map_err(|_| bad("bad claim value"))?),
                    _ => return Err(bad("claim values must be strings")),
                };
                attrs.push((name.to_ascii_lowercase(), v.filter(|v| !v.is_empty())));
            }
        }
        let dashboards = match o.get("dashboards") {
            None | Some(Value::Null) => None,
            Some(d) => Some(
                d.as_array()
                    .ok_or_else(|| bad("dashboards must be a list of ids"))?
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .filter(|s| valid_dashboard_id(s))
                            .map(str::to_string)
                            .ok_or_else(|| bad("bad dashboard id"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };
        let expires = match o.get("expires") {
            None | Some(Value::Null) => None,
            Some(e) => Some(
                e.as_u64()
                    .ok_or_else(|| bad("expires must be unix seconds"))?,
            ),
        };
        let revoked = match o.get("revoked") {
            None => false,
            Some(r) => r
                .as_bool()
                .ok_or_else(|| bad("revoked must be a boolean"))?,
        };
        out.push(KeyEntry {
            id: id.to_ascii_lowercase(),
            sha256,
            claims: Claims {
                user,
                groups,
                attrs,
            },
            dashboards,
            expires,
            revoked,
        });
    }
    Ok(out)
}

/// The JSON of one key-file entry.
pub fn entry_json(e: &KeyEntry, note: Option<&str>, created: Option<&str>) -> Value {
    let mut o = json!({"id": e.id, "sha256": e.sha256, "user": e.claims.user});
    if !e.claims.groups.is_empty() {
        o["groups"] = json!(e.claims.groups);
    }
    if !e.claims.attrs.is_empty() {
        o["claims"] = Value::Object(
            e.claims
                .attrs
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect(),
        );
    }
    if let Some(d) = &e.dashboards {
        o["dashboards"] = json!(d);
    }
    if let Some(x) = e.expires {
        o["expires"] = json!(x);
    }
    if e.revoked {
        o["revoked"] = json!(true);
    }
    if let Some(n) = note {
        o["note"] = json!(n);
    }
    if let Some(c) = created {
        o["created"] = json!(c);
    }
    o
}

/// Read a key file, refusing one that others may write.
pub fn read_file(p: &Path) -> Result<(String, Option<SystemTime>), String> {
    let meta = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", p.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o022 != 0 {
            return Err(format!(
                "{}: writable by group or others — chmod 600 it",
                p.display()
            ));
        }
    }
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok((text, meta.modified().ok()))
}

struct State {
    /// `Err`: the file is broken — every key is refused until it is fixed.
    keys: Result<Vec<KeyEntry>, String>,
    stamp: Option<(SystemTime, u64)>,
    checked: Instant,
}

/// The configured key file, reloaded when it changes.
pub struct ApiKeys {
    pub file: PathBuf,
    state: Mutex<State>,
}

impl std::fmt::Debug for ApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeys")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

/// What happened when the file was checked (for the audit log).
#[derive(Debug, Clone, PartialEq)]
pub enum Reload {
    Unchanged,
    Loaded(usize),
    Failed(String),
}

impl ApiKeys {
    pub fn from_json(o: &serde_json::Map<String, Value>) -> Result<ApiKeys, String> {
        for k in o.keys() {
            if !matches!(k.as_str(), "mode" | "file") {
                return Err(format!("auth (api_keys): unknown key '{k}' (file)"));
            }
        }
        let file = o
            .get("file")
            .and_then(Value::as_str)
            .filter(|f| !f.trim().is_empty())
            .ok_or("auth.file (api_keys) must be the path of the key file")?;
        let file = PathBuf::from(file);
        // Broken at startup: refuse to start.
        let (text, mtime) = read_file(&file).map_err(|e| format!("auth.file: {e}"))?;
        let keys = parse_file(&text).map_err(|e| format!("auth.file {}: {e}", file.display()))?;
        let len = text.len() as u64;
        Ok(ApiKeys {
            file,
            state: Mutex::new(State {
                keys: Ok(keys),
                stamp: mtime.map(|m| (m, len)),
                checked: Instant::now(),
            }),
        })
    }

    /// Re-read the file if it changed (at most every [`RELOAD_EVERY`]).
    fn reload(&self, st: &mut State, force: bool) -> Reload {
        if !force && st.checked.elapsed() < RELOAD_EVERY {
            return Reload::Unchanged;
        }
        st.checked = Instant::now();
        let stamp = std::fs::metadata(&self.file)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        if !force && stamp.is_some() && stamp == st.stamp && st.keys.is_ok() {
            return Reload::Unchanged;
        }
        st.stamp = stamp;
        match read_file(&self.file).and_then(|(t, _)| parse_file(&t)) {
            Ok(k) => {
                let n = k.len();
                st.keys = Ok(k);
                Reload::Loaded(n)
            }
            Err(e) => {
                st.keys = Err(e.clone());
                Reload::Failed(e)
            }
        }
    }

    /// Authenticate `key`: its id (for the log, also on failure) and the
    /// claims + scope, or the reason it was refused.
    pub fn authenticate(
        &self,
        key: &str,
        on_reload: &dyn Fn(&Reload),
    ) -> (Option<String>, Result<(Claims, Scope), &'static str>) {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let r = self.reload(&mut st, false);
        if r != Reload::Unchanged {
            on_reload(&r);
        }
        let Some(id) = key_id(key) else {
            return (None, Err("malformed_key"));
        };
        let hash = sha256_hex(key);
        let keys = match &st.keys {
            Ok(k) => k,
            Err(_) => return (Some(id.to_string()), Err("key_file_broken")),
        };
        let entry = keys.iter().find(|e| e.id == id.to_ascii_lowercase());
        // Compare against something either way (no early exit on an unknown id).
        let expected = entry.map_or("0".repeat(64), |e| e.sha256.clone());
        let matches = ct_eq(&hash, &expected);
        let id = Some(id.to_ascii_lowercase());
        match entry {
            None => (id, Err("unknown_key")),
            Some(_) if !matches => (id, Err("unknown_key")),
            Some(e) if e.revoked => (id, Err("revoked")),
            Some(e) if e.expires.is_some_and(|x| x <= super::jwt::now_s()) => (id, Err("expired")),
            Some(e) => (
                id,
                Ok((
                    e.claims.clone(),
                    Scope {
                        dashboards: e.dashboards.clone(),
                        vars: Vec::new(),
                    },
                )),
            ),
        }
    }
}

/// Rewrite `file` with `entries` (atomically: a `0600` temp file renamed over
/// it).
pub fn write_file(file: &Path, entries: &[Value]) -> Result<(), String> {
    use std::io::Write;
    let text = serde_json::to_string_pretty(&json!({"keys": entries})).unwrap_or_default();
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        file.file_name().and_then(|n| n.to_str()).unwrap_or("keys"),
        crate::host::serving::random_token()
    ));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(text.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", file.display())
    })
}

/// The entries of `file` as JSON (an absent file is empty).
pub fn file_entries(file: &Path) -> Result<Vec<Value>, String> {
    if !file.exists() {
        return Ok(Vec::new());
    }
    let (text, _) = read_file(file)?;
    parse_file(&text)?; // validate
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(v["keys"].as_array().cloned().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "anofox-keys-{}",
            crate::host::serving::random_token()
        ));
        std::fs::create_dir(&d).unwrap();
        d
    }

    fn entry(id: &str, hash: &str) -> KeyEntry {
        KeyEntry {
            id: id.into(),
            sha256: hash.into(),
            claims: Claims {
                user: "svc-etl".into(),
                groups: vec!["etl".into()],
                attrs: vec![("tenant".into(), Some("acme".into()))],
            },
            dashboards: Some(vec!["sales".into()]),
            expires: None,
            revoked: false,
        }
    }

    #[test]
    fn keys_are_hashed_scoped_revocable() {
        let d = tmpdir();
        let f = d.join("keys.json");
        let (key, id, hash) = new_key();
        assert!(key.starts_with("afx_") && key.len() == 4 + 12 + 1 + 64);
        assert_eq!(key_id(&key), Some(id.as_str()));
        write_file(&f, &[entry_json(&entry(&id, &hash), Some("etl"), None)]).unwrap();
        // the file holds the hash, never the key
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(!text.contains(&key[17..]) && text.contains(&hash));
        let k = ApiKeys::from_json(json!({"mode": "api_keys", "file": f}).as_object().unwrap())
            .unwrap();
        let none = |_: &Reload| {};
        let (kid, r) = k.authenticate(&key, &none);
        assert_eq!(kid.as_deref(), Some(id.as_str()));
        let (c, s) = r.unwrap();
        assert_eq!(c.user, "svc-etl");
        assert!(s.allows("sales") && !s.allows("payroll"));
        // a wrong secret with a right id, an unknown id, garbage
        let wrong = format!(
            "{}{}",
            &key[..key.len() - 1],
            if key.ends_with('0') { '1' } else { '0' }
        );
        assert_eq!(k.authenticate(&wrong, &none).1, Err("unknown_key"));
        let (other, _, _) = new_key();
        assert_eq!(k.authenticate(&other, &none).1, Err("unknown_key"));
        assert_eq!(k.authenticate("afx_nope", &none).1, Err("malformed_key"));
        // revoke: the file changes, the next check (forced here) refuses it
        let mut e = entry(&id, &hash);
        e.revoked = true;
        write_file(&f, &[entry_json(&e, None, None)]).unwrap();
        {
            let mut st = k.state.lock().unwrap();
            assert_eq!(k.reload(&mut st, true), Reload::Loaded(1));
        }
        assert_eq!(k.authenticate(&key, &none).1, Err("revoked"));
        // expired
        let mut e = entry(&id, &hash);
        e.expires = Some(1);
        write_file(&f, &[entry_json(&e, None, None)]).unwrap();
        k.reload(&mut k.state.lock().unwrap(), true);
        assert_eq!(k.authenticate(&key, &none).1, Err("expired"));
        // a broken file disables every key (fail closed)
        std::fs::write(&f, "{not json").unwrap();
        let r = k.reload(&mut k.state.lock().unwrap(), true);
        assert!(matches!(r, Reload::Failed(_)), "{r:?}");
        assert_eq!(k.authenticate(&key, &none).1, Err("key_file_broken"));
        // a group/world-writable file is refused
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            write_file(&f, &[entry_json(&entry(&id, &hash), None, None)]).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o666)).unwrap();
            assert!(ApiKeys::from_json(json!({"file": f}).as_object().unwrap()).is_err());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn key_files_are_validated() {
        let h = "a".repeat(64);
        for bad in [
            json!({"keys": [{"id": "xyz", "sha256": h, "user": "u"}]}),
            json!({"keys": [{"id": "0123456789ab", "sha256": "abc", "user": "u"}]}),
            json!({"keys": [{"id": "0123456789ab", "sha256": h}]}),
            json!({"keys": [{"id": "0123456789ab", "sha256": h, "user": "u", "claims": {"__x-y": "1"}}]}),
            json!({"keys": [{"id": "0123456789ab", "sha256": h, "user": "u", "dashboards": ["../x"]}]}),
            json!({"keys": [{"id": "0123456789ab", "sha256": h, "user": "u", "key": "afx_…"}]}),
            json!({"keys": [{"id": "0123456789ab", "sha256": h, "user": "u"}, {"id": "0123456789ab", "sha256": h, "user": "v"}]}),
            json!({"nope": []}),
        ] {
            assert!(parse_file(&bad.to_string()).is_err(), "{bad}");
        }
        let ok = json!({"keys": [{"id": "0123456789ab", "sha256": h, "user": "u",
            "claims": {"tenant": "t"}, "expires": 1900000000, "note": "n"}]});
        let e = parse_file(&ok.to_string()).unwrap();
        assert_eq!(e[0].claims.attrs, vec![("tenant".into(), Some("t".into()))]);
        assert_eq!(e[0].dashboards, None);
    }
}

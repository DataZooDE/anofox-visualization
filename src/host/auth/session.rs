//! Server-signed sessions: the OIDC login cookie and the page session an
//! embed link is exchanged for. A session is an HS256 JWT signed with a key
//! only this server (or its replicas, with `session_secret_env`) holds, so it
//! carries the claims and scope without server-side state: replicas stay
//! stateless. Expiry is the server's (`exp` in the signed payload, checked on
//! every request), not the browser's. Logout revokes the session id on the
//! replica that handled it (see `docs/secure-serving.md`).

use super::Scope;
use crate::host::access::Claims;
use crate::host::serving::valid_ident;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// Minimum length of a configured secret (bytes).
pub const MIN_SECRET_LEN: usize = 32;
const ISSUER: &str = "anofox-visualization";
/// Longest session token we hand out (a cookie must stay below ~4 KB).
pub const MAX_SESSION_LEN: usize = 3800;

/// `len` random bytes, base64url (unpadded).
pub fn random_b64(len: usize) -> String {
    use base64::Engine;
    let mut b = vec![0u8; len];
    aws_lc_rs::rand::fill(&mut b).expect("system randomness");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// base64url(SHA-256(s)) — the PKCE S256 challenge.
pub fn sha256_b64(s: &str) -> String {
    use base64::Engine;
    let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, s.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(d.as_ref())
}

/// Constant-time equality of two strings (lengths are not secret).
pub fn ct_eq(a: &str, b: &str) -> bool {
    aws_lc_rs::constant_time::verify_slices_are_equal(a.as_bytes(), b.as_bytes()).is_ok()
}

/// Read a secret from `<key>` or `<key>_env` of a method's configuration.
pub fn secret(
    o: &serde_json::Map<String, Value>,
    key: &str,
    env: &dyn Fn(&str) -> Option<String>,
    required: bool,
) -> Result<Option<String>, String> {
    let env_key = format!("{key}_env");
    let s = match (o.get(key), o.get(&env_key)) {
        (Some(s), None) => s
            .as_str()
            .ok_or_else(|| format!("auth.{key} must be a string"))?
            .to_string(),
        (None, Some(e)) => {
            let name = e
                .as_str()
                .ok_or_else(|| format!("auth.{env_key} must be a string"))?;
            env(name).ok_or_else(|| {
                format!("auth.{env_key}: environment variable '{name}' is not set")
            })?
        }
        (Some(_), Some(_)) => return Err(format!("auth: give {key} or {env_key}, not both")),
        (None, None) if required => return Err(format!("auth: {key} (or {env_key}) is required")),
        (None, None) => return Ok(None),
    };
    let s = s.trim().to_string();
    if s.len() < MIN_SECRET_LEN || !s.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(format!(
            "auth.{key}: at least {MIN_SECRET_LEN} printable ASCII characters (e.g. `openssl rand -hex 32`)"
        ));
    }
    Ok(Some(s))
}

/// A session as it travels in a cookie or header.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub sid: String,
    pub exp: u64,
    pub claims: Claims,
    pub scope: Scope,
}

/// The key sessions of one method are signed with, and its revocations.
pub struct Sessions {
    enc: EncodingKey,
    dec: DecodingKey,
    /// `session_secret_env` given: sessions survive restarts and work on
    /// every replica with the same secret.
    pub persistent: bool,
    /// The longest session this key issues: a token claiming a longer life
    /// (`exp - iat`) is refused, whoever signed it.
    max_ttl: u64,
    revocations: Option<Revocations>,
    revoked: Mutex<HashMap<String, u64>>,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("persistent", &self.persistent)
            .finish_non_exhaustive()
    }
}

impl Sessions {
    /// `label` separates the keys of different uses of one secret; `max_ttl`
    /// is the longest session [`Sessions::issue`] is asked for.
    pub fn new(secret: Option<&str>, label: &str, max_ttl: u64) -> Sessions {
        use aws_lc_rs::hmac;
        let (base, persistent) = match secret {
            Some(s) => (s.as_bytes().to_vec(), true),
            None => {
                let mut b = vec![0u8; 32];
                aws_lc_rs::rand::fill(&mut b).expect("system randomness");
                (b, false)
            }
        };
        let k = hmac::Key::new(hmac::HMAC_SHA256, &base);
        let key = hmac::sign(&k, format!("anofox-session/{label}").as_bytes());
        Sessions {
            enc: EncodingKey::from_secret(key.as_ref()),
            dec: DecodingKey::from_secret(key.as_ref()),
            persistent,
            max_ttl,
            revocations: None,
            revoked: Mutex::new(HashMap::new()),
        }
    }

    /// Sign `payload` (any JSON object) for `aud`, valid until `exp`.
    pub fn sign_raw(&self, aud: &str, exp: u64, mut payload: Value) -> String {
        payload["iss"] = json!(ISSUER);
        payload["aud"] = json!(aud);
        payload["exp"] = json!(exp);
        payload["iat"] = json!(super::jwt::now_s());
        jsonwebtoken::encode(&Header::new(Algorithm::HS256), &payload, &self.enc)
            .expect("HS256 signing")
    }

    /// Verify a token this server signed for `aud`.
    pub fn verify_raw(&self, aud: &str, token: &str) -> Result<Value, &'static str> {
        if !super::jwt::looks_like_jwt(token) {
            return Err("malformed_token");
        }
        let mut v = Validation::new(Algorithm::HS256);
        v.algorithms = vec![Algorithm::HS256];
        v.leeway = 0;
        v.set_required_spec_claims(&["exp", "iss", "aud"]);
        v.set_issuer(&[ISSUER]);
        v.set_audience(&[aud]);
        jsonwebtoken::decode::<Value>(token, &self.dec, &v)
            .map(|d| d.claims)
            .map_err(|e| super::jwt::reason(&e))
    }

    /// A new session token for `claims` + `scope`, valid for `ttl_s`.
    pub fn issue(
        &self,
        aud: &str,
        ttl_s: u64,
        claims: &Claims,
        scope: &Scope,
    ) -> (String, Session) {
        let sid = random_b64(16);
        let exp = super::jwt::now_s() + ttl_s;
        let payload = json!({
            "sid": sid,
            "u": claims.user,
            "g": claims.groups,
            "a": claims.attrs,
            "d": scope.dashboards,
            "v": scope.vars,
        });
        let token = self.sign_raw(aud, exp, payload);
        (
            token,
            Session {
                sid,
                exp,
                claims: claims.clone(),
                scope: scope.clone(),
            },
        )
    }

    /// The session in `token` (signed by us for `aud`, not expired, not
    /// revoked).
    pub fn verify(&self, aud: &str, token: &str) -> Result<Session, &'static str> {
        let p = self.verify_raw(aud, token)?;
        let bad = "malformed_token";
        let (exp, iat) = (p["exp"].as_u64().ok_or(bad)?, p["iat"].as_u64().ok_or(bad)?);
        if exp.saturating_sub(iat) > self.max_ttl {
            return Err("session_ttl");
        }
        let s = |v: &Value| v.as_str().map(str::to_string).ok_or(bad);
        let strs = |v: &Value| -> Result<Vec<String>, &'static str> {
            v.as_array().ok_or(bad)?.iter().map(s).collect()
        };
        let sid = s(&p["sid"])?;
        self.check_revocations(&sid)?;
        let attrs = p["a"]
            .as_array()
            .ok_or(bad)?
            .iter()
            .map(|kv| {
                let name = s(&kv[0])?;
                let v = match &kv[1] {
                    Value::Null => None,
                    v => Some(s(v)?),
                };
                Ok((name, v))
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        let dashboards = match &p["d"] {
            Value::Null => None,
            v => Some(strs(v)?),
        };
        let vars = p["v"]
            .as_array()
            .ok_or(bad)?
            .iter()
            .map(|kv| Ok((s(&kv[0])?, s(&kv[1])?)))
            .collect::<Result<Vec<_>, &'static str>>()?;
        // Signed by us, but names still go into SQL: check them again.
        if vars.iter().any(|(n, _)| !super::lockable_var(n))
            || attrs.iter().any(|(n, _)| !valid_ident(n))
        {
            return Err(bad);
        }
        Ok(Session {
            sid,
            exp: p["exp"].as_u64().ok_or(bad)?,
            claims: Claims {
                user: s(&p["u"])?,
                groups: strs(&p["g"])?,
                attrs,
            },
            scope: Scope { dashboards, vars },
        })
    }

    /// Revoke `sid` until `exp` (logout).
    pub fn revoke(&self, sid: &str, exp: u64) {
        let now = super::jwt::now_s();
        {
            let mut r = self.revoked.lock().unwrap_or_else(|p| p.into_inner());
            r.retain(|_, e| *e >= now);
            r.insert(sid.to_string(), exp);
        }
        if let Some(f) = &self.revocations {
            f.append(sid, exp);
        }
    }

    /// Report a broken revocation file to the audit log too.
    pub fn set_audit(&self, log: std::sync::Arc<super::audit::AuditLog>) {
        if let Some(f) = &self.revocations {
            let _ = f.audit.set(log);
        }
    }

    /// `revoked`, or `revocations_unavailable` while the revocation file
    /// can't be trusted (a 503 for the server, not a reason to log in again).
    fn check_revocations(&self, sid: &str) -> Result<(), &'static str> {
        if let Some(f) = &self.revocations {
            f.reload(&self.revoked);
            if f.is_broken() {
                return Err("revocations_unavailable");
            }
        }
        let r = self.revoked.lock().unwrap_or_else(|p| p.into_inner());
        if r.contains_key(sid) {
            return Err("revoked");
        }
        Ok(())
    }

    /// Keep revocations in `path` (JSON lines, `0600`, no symlinks): a logout
    /// survives a restart, and replicas sharing the file see each other's
    /// within [`Revocations::RELOAD_EVERY`].
    pub fn with_revocation_file(mut self, path: std::path::PathBuf) -> Result<Sessions, String> {
        let bad =
            |e: &dyn std::fmt::Display| format!("auth.revocation_file: {}: {e}", path.display());
        // Created 0600 (an existing file is tightened), never via a symlink.
        crate::host::access::open_log(&path).map_err(|e| bad(&e))?;
        compact_revocations(&path, || {}).map_err(|e| bad(&e))?;
        let f = Revocations {
            path: path.clone(),
            stamp: Mutex::new((None, None)),
            broken: std::sync::atomic::AtomicBool::new(false),
            audit: std::sync::OnceLock::new(),
        };
        f.reload(&self.revoked);
        if f.is_broken() {
            return Err(bad(&"cannot be read safely"));
        }
        self.revocations = Some(f);
        Ok(self)
    }
}

/// The file stamp read last, and when the file was last checked.
type ReloadState = (
    Option<(std::time::SystemTime, u64)>,
    Option<std::time::Instant>,
);

/// A revocation file ([`Sessions::with_revocation_file`]).
pub struct Revocations {
    path: std::path::PathBuf,
    /// (file stamp read last, when it was last checked)
    stamp: Mutex<ReloadState>,
    /// The file can't be trusted right now: every session is refused.
    broken: std::sync::atomic::AtomicBool,
    /// Where a broken file is also reported.
    audit: std::sync::OnceLock<std::sync::Arc<super::audit::AuditLog>>,
}

impl Revocations {
    pub const RELOAD_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

    fn append(&self, sid: &str, exp: u64) {
        use std::io::Write;
        let line = format!("{}\n", json!({"sid": sid, "exp": exp}));
        let r = crate::host::access::open_log(&self.path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = r {
            eprintln!(
                "[anofox-serve] revocation file {}: {e}",
                self.path.display()
            );
        }
    }

    /// Merge the file's unexpired revocations into `into` when it changed.
    /// Merge the file's unexpired revocations into `into` when it changed.
    /// A file that is gone, unreadable or writable by group or others marks
    /// the revocations broken: every session is refused until it is fixed
    /// (fail closed), and the error is reported.
    fn reload(&self, into: &Mutex<HashMap<String, u64>>) {
        let mut st = self.stamp.lock().unwrap_or_else(|p| p.into_inner());
        if st.1.is_some_and(|t| t.elapsed() < Self::RELOAD_EVERY) {
            return;
        }
        st.1 = Some(std::time::Instant::now());
        match self.read(&mut st.0) {
            Ok(None) => {}
            Ok(Some(text)) => {
                let now = super::jwt::now_s();
                let mut r = into.lock().unwrap_or_else(|p| p.into_inner());
                for (sid, exp) in parse_revocations(&text, now) {
                    r.insert(sid, exp);
                }
                self.set_broken(None);
            }
            Err(e) => self.set_broken(Some(e)),
        }
    }

    /// The file's text if it changed since `stamp` (`None`: unchanged).
    fn read(
        &self,
        stamp: &mut Option<(std::time::SystemTime, u64)>,
    ) -> Result<Option<String>, String> {
        let meta = std::fs::symlink_metadata(&self.path).map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("not a regular file".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o022 != 0 {
                return Err("writable by group or others".into());
            }
        }
        let now = meta.modified().ok().map(|m| (m, meta.len()));
        if now.is_some() && now == *stamp && !self.is_broken() {
            return Ok(None);
        }
        *stamp = now;
        std::fs::read_to_string(&self.path)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    fn set_broken(&self, e: Option<String>) {
        use std::sync::atomic::Ordering;
        if let Some(e) = &e {
            if !self.broken.swap(true, Ordering::SeqCst) {
                if let Some(a) = self.audit.get() {
                    a.write(&super::audit::Event {
                        event: "revocation_file",
                        outcome: "error",
                        reason: Some("unusable"),
                        detail: Some(e),
                        ..Default::default()
                    });
                }
                eprintln!(
                    "[anofox-serve] revocation file {}: {} — refusing every login session until it is fixed",
                    self.path.display(),
                    crate::host::access::diag(e)
                );
            }
        } else {
            self.broken.store(false, Ordering::SeqCst);
        }
    }

    fn is_broken(&self) -> bool {
        self.broken.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Drop the expired lines of a revocation file: write the rest to a new
/// `0600` file and rename it into place — unless the file changed meanwhile
/// (another replica appended a logout): then it is left as it is, so no
/// revocation is lost. `before_rename` is a test hook.
fn compact_revocations(
    path: &std::path::Path,
    before_rename: impl FnOnce(),
) -> std::io::Result<bool> {
    use std::io::Write;
    let stamp = |p: &std::path::Path| -> std::io::Result<(Option<std::time::SystemTime>, u64)> {
        let m = std::fs::metadata(p)?;
        Ok((m.modified().ok(), m.len()))
    };
    let before = stamp(path)?;
    let text = std::fs::read_to_string(path)?;
    if stamp(path)? != before {
        return Ok(false);
    }
    let keep = parse_revocations(&text, super::jwt::now_s());
    let tmp = path.with_extension(format!("tmp-{}", crate::host::serving::random_token()));
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        for (sid, exp) in &keep {
            writeln!(f, "{}", json!({"sid": sid, "exp": exp}))?;
        }
        f.sync_all()?;
    }
    before_rename();
    if stamp(path)? != before {
        let _ = std::fs::remove_file(&tmp);
        return Ok(false);
    }
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// The unexpired `(sid, exp)` lines of a revocation file.
fn parse_revocations(text: &str, now: u64) -> Vec<(String, u64)> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Some((v["sid"].as_str()?.to_string(), v["exp"].as_u64()?)))
        .filter(|(_, exp)| *exp >= now)
        .collect()
}

/// One-time ids (`jti`) seen until their expiry — refuses a replayed link.
#[derive(Default)]
pub struct Seen(Mutex<HashMap<String, u64>>);

impl Seen {
    /// `true` the first time `id` is seen (before `exp`).
    pub fn first(&self, id: &str, exp: u64) -> bool {
        let now = super::jwt::now_s();
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        m.retain(|_, e| *e >= now);
        if m.len() >= 100_000 || m.contains_key(id) {
            return false;
        }
        m.insert(id.to_string(), exp);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> Claims {
        Claims {
            user: "alice".into(),
            groups: vec!["eu".into()],
            attrs: vec![("tenant".into(), Some("acme".into())), ("x".into(), None)],
        }
    }

    #[test]
    fn sessions_round_trip_and_are_bound_to_key_and_audience() {
        let s = Sessions::new(None, "login", 3600);
        let scope = Scope {
            dashboards: Some(vec!["sales".into()]),
            vars: vec![("region".into(), "'EU'".into())],
        };
        let (tok, sess) = s.issue("login", 60, &claims(), &scope);
        let back = s.verify("login", &tok).unwrap();
        assert_eq!(back, sess);
        assert_eq!(back.claims, claims());
        // another audience, another server's key, a tampered payload
        assert_eq!(s.verify("embed", &tok), Err("bad_audience"));
        let other = Sessions::new(None, "login", 3600);
        assert_eq!(other.verify("login", &tok), Err("bad_signature"));
        let parts: Vec<&str> = tok.split('.').collect();
        let mut p: Value = {
            use base64::Engine;
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .unwrap();
            serde_json::from_slice(&raw).unwrap()
        };
        p["d"] = Value::Null; // widen the scope to every dashboard
        let forged = {
            use base64::Engine;
            let b = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(p.to_string());
            format!("{}.{b}.{}", parts[0], parts[2])
        };
        assert_eq!(s.verify("login", &forged), Err("bad_signature"));
        // the same secret gives the same key (replicas); labels separate uses
        let a = Sessions::new(Some("0123456789abcdef0123456789abcdef"), "login", 3600);
        let b = Sessions::new(Some("0123456789abcdef0123456789abcdef"), "login", 3600);
        let c = Sessions::new(Some("0123456789abcdef0123456789abcdef"), "embed", 3600);
        let (t, _) = a.issue("login", 60, &claims(), &Scope::default());
        assert!(b.verify("login", &t).is_ok());
        assert_eq!(c.verify("login", &t), Err("bad_signature"));
    }

    #[test]
    fn expiry_and_revocation() {
        let s = Sessions::new(None, "login", 3600);
        let t = s.sign_raw("login", super::super::jwt::now_s() - 1, json!({"sid": "x"}));
        assert_eq!(s.verify("login", &t), Err("expired"));
        let (t, sess) = s.issue("login", 60, &claims(), &Scope::default());
        s.revoke(&sess.sid, sess.exp);
        assert_eq!(s.verify("login", &t), Err("revoked"));
        // a token signed with the key but claiming a longer life (#114)
        let long = s.sign_raw(
            "login",
            super::super::jwt::now_s() + 7200,
            json!({"sid": "y"}),
        );
        assert_eq!(s.verify("login", &long), Err("session_ttl"));
    }

    #[test]
    fn logout_survives_a_restart_with_a_revocation_file() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-rev-{}",
            crate::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("revoked.jsonl");
        let secret = Some("0123456789abcdef0123456789abcdef");
        let a = Sessions::new(secret, "login", 3600)
            .with_revocation_file(file.clone())
            .unwrap();
        let (t, sess) = a.issue("login", 60, &claims(), &Scope::default());
        a.revoke(&sess.sid, sess.exp);
        // a restarted server (or another replica on the same file)
        let b = Sessions::new(secret, "login", 3600)
            .with_revocation_file(file.clone())
            .unwrap();
        assert_eq!(b.verify("login", &t), Err("revoked"));
        // compaction on start: expired lines are dropped
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
            writeln!(f, "{}", json!({"sid": "old", "exp": 1})).unwrap();
        }
        let _c = Sessions::new(secret, "login", 3600)
            .with_revocation_file(file.clone())
            .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("\"old\"") && text.contains(&sess.sid),
            "{text}"
        );
        // a file that disappears or becomes writable by others: fail closed
        let (t2, _) = b.issue("login", 60, &claims(), &Scope::default());
        std::fs::remove_file(&file).unwrap();
        b.revocations.as_ref().unwrap().stamp.lock().unwrap().1 = None;
        let audit = dir.join("audit.jsonl");
        b.set_audit(std::sync::Arc::new(
            super::super::audit::AuditLog::new(audit.to_str().unwrap()).unwrap(),
        ));
        assert_eq!(b.verify("login", &t2), Err("revocations_unavailable"));
        let a = std::fs::read_to_string(&audit).unwrap();
        assert!(
            a.contains("\"audit\":\"revocation_file\"") && a.contains("\"outcome\":\"error\""),
            "{a}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&file, "").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
            b.revocations.as_ref().unwrap().stamp.lock().unwrap().1 = None;
            assert_eq!(b.verify("login", &t2), Err("revocations_unavailable"));
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            b.revocations.as_ref().unwrap().stamp.lock().unwrap().1 = None;
            assert!(b.verify("login", &t2).is_ok());
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let link = dir.join("link.jsonl");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(Sessions::new(secret, "login", 60)
                .with_revocation_file(link)
                .is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compaction_never_loses_a_concurrent_logout() {
        // Another replica appends between our read and our rename: the
        // compaction is abandoned, the new line stays.
        let dir = std::env::temp_dir().join(format!(
            "anofox-rev-c-{}",
            crate::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("revoked.jsonl");
        std::fs::write(&file, format!("{}\n", json!({"sid": "old", "exp": 1}))).unwrap();
        let exp = super::super::jwt::now_s() + 600;
        let appended = compact_revocations(&file, || {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
            writeln!(f, "{}", json!({"sid": "new", "exp": exp})).unwrap();
        })
        .unwrap();
        assert!(!appended);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"new\""), "{text}");
        // unchanged meanwhile: compacted
        assert!(compact_revocations(&file, || {}).unwrap());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("\"new\"") && !text.contains("\"old\""),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_time_ids() {
        let seen = Seen::default();
        let exp = super::super::jwt::now_s() + 60;
        assert!(seen.first("a", exp));
        assert!(!seen.first("a", exp));
        assert!(seen.first("b", exp));
    }

    #[test]
    fn constant_time_compare_and_randomness() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(!ct_eq("", "a"));
        let (a, b) = (random_b64(32), random_b64(32));
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        // RFC 7636 appendix B
        assert_eq!(
            sha256_b64("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}

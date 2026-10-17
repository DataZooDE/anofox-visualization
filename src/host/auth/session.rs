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
            f.record(sid, exp);
        }
    }

    /// Log out the session in `token`: revoke it and return its (sid, user).
    /// While the revocation file is unavailable no session verifies, but the
    /// logout still counts: a token we signed that has not expired is
    /// revoked anyway, and its line is queued until the file is back.
    pub fn logout(&self, aud: &str, token: &str) -> Option<(String, String)> {
        match self.verify(aud, token) {
            Ok(s) => {
                self.revoke(&s.sid, s.exp);
                Some((s.sid, s.claims.user))
            }
            Err("revocations_unavailable") => {
                let p = self.verify_raw(aud, token).ok()?;
                let (sid, exp, iat) = (p["sid"].as_str()?, p["exp"].as_u64()?, p["iat"].as_u64()?);
                if exp.saturating_sub(iat) > self.max_ttl {
                    return None;
                }
                // Logged out already: nothing to record (or to audit) again.
                if self
                    .revoked
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains_key(sid)
                {
                    return None;
                }
                self.revoke(sid, exp);
                Some((
                    sid.to_string(),
                    p["u"].as_str().unwrap_or_default().to_string(),
                ))
            }
            Err(_) => None,
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
        // Created on the very first start only: a lock file without the file
        // means it was deleted or moved, and a new empty one would forget
        // every logout in it.
        if std::fs::symlink_metadata(&path).is_err()
            && std::fs::symlink_metadata(lock_path(&path)).is_ok()
        {
            return Err(bad(
                &"missing, but its .lock file exists: restore the file (or remove the .lock file to start with no revocations)",
            ));
        }
        // Created 0600 (an existing file is tightened), never via a symlink.
        crate::host::access::open_log(&path).map_err(|e| bad(&e))?;
        compact_revocations(&path, || {}).map_err(|e| bad(&e))?;
        let f = Revocations {
            path: path.clone(),
            stamp: Mutex::new((None, None)),
            pending: Mutex::new(HashMap::new()),
            broken: std::sync::atomic::AtomicBool::new(false),
            failing: std::sync::atomic::AtomicBool::new(false),
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
    /// Logouts not written yet (the file was unavailable): retried on the
    /// next logout and reload.
    pending: Mutex<HashMap<String, u64>>,
    /// The last write failed (reported once, until a write succeeds).
    failing: std::sync::atomic::AtomicBool,
    /// The file can't be trusted right now: every session is refused.
    broken: std::sync::atomic::AtomicBool,
    /// Where a broken file is also reported.
    audit: std::sync::OnceLock<std::sync::Arc<super::audit::AuditLog>>,
}

impl Revocations {
    pub const RELOAD_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

    /// Write a logout to the file, or queue it while the file is unavailable.
    fn record(&self, sid: &str, exp: u64) {
        let mut q = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        if q.len() < 100_000 {
            q.insert(sid.to_string(), exp);
        }
        self.flush(&mut q);
    }

    /// Append the queued logouts, under the file's lock and to the file that
    /// is at `path` now (not one compaction just replaced). A failure keeps
    /// them queued and is reported on stderr and in the audit log.
    fn flush(&self, q: &mut HashMap<String, u64>) {
        use std::io::Write;
        let now = super::jwt::now_s();
        q.retain(|_, exp| *exp >= now);
        if q.is_empty() {
            return;
        }
        let r = lock_revocations(&self.path, false).and_then(|_lock| {
            for _ in 0..3 {
                let mut f = open_nofollow(&self.path, true)?;
                if same_file(&f, &self.path)? {
                    let text: String = q
                        .iter()
                        .map(|(sid, exp)| format!("{}\n", json!({"sid": sid, "exp": exp})))
                        .collect();
                    return f.write_all(text.as_bytes());
                }
            }
            Err(std::io::Error::other("the file keeps being replaced"))
        });
        use std::sync::atomic::Ordering;
        match r {
            Ok(()) => {
                q.clear();
                self.failing.store(false, Ordering::SeqCst);
            }
            Err(e) => {
                let e = format!("{e} — {} logout(s) queued", q.len());
                eprintln!(
                    "[anofox-serve] revocation file {}: {e}",
                    self.path.display()
                );
                if !self.failing.swap(true, Ordering::SeqCst) {
                    if let Some(a) = self.audit.get() {
                        a.write(&super::audit::Event {
                            event: "revocation_file",
                            outcome: "error",
                            reason: Some("write_failed"),
                            detail: Some(&e),
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

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
        {
            let mut q = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            self.flush(&mut q);
        }
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
        let mut f = open_nofollow(&self.path, false).map_err(|e| e.to_string())?;
        let meta = f.metadata().map_err(|e| e.to_string())?;
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
        let mut text = String::new();
        std::io::Read::read_to_string(&mut f, &mut text).map_err(|e| e.to_string())?;
        Ok(Some(text))
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

/// Open the existing file `path` to read it, or to append to it — never
/// through a symlink, a regular file only, and not one writable by group or
/// others. It is never created here: a revocation file that is gone stays
/// gone (sessions are refused) until it is restored.
fn open_nofollow(path: &std::path::Path, append: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(!append).append(append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK: a FIFO put in its place can't hang the server
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = o.open(path)?;
    let meta = f.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    #[cfg(unix)]
    if append {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o022 != 0 {
            return Err(std::io::Error::other("writable by group or others"));
        }
    }
    Ok(f)
}

/// `<path>.lock`.
fn lock_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    name.into()
}

/// The exclusive lock on `<path>.lock` that every writer of a revocation
/// file holds while it appends or compacts, so a logout never lands in a
/// file compaction is about to replace. Released when the returned handle is
/// dropped. The lock file is opened read + write (what `LockFileEx` needs on
/// Windows), `0600`, never through a symlink, a regular file only.
/// `wait`: block until the lock is free (compaction at start); otherwise (a
/// request) give up after about a second.
fn lock_revocations(path: &std::path::Path, wait: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = o.open(lock_path(path))?;
    let meta = f.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("lock file: not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
    }
    if wait {
        f.lock()?;
        return Ok(f);
    }
    for _ in 0..200 {
        match f.try_lock() {
            Ok(()) => return Ok(f),
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "lock file: held too long by another writer",
    ))
}

/// Is the open file `f` still the one at `path` (same inode, not replaced)?
fn same_file(f: &std::fs::File, path: &std::path::Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (a, b) = (f.metadata()?, std::fs::symlink_metadata(path)?);
        Ok(a.dev() == b.dev() && a.ino() == b.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (f, path);
        Ok(true)
    }
}

/// Drop the expired lines of a revocation file: write the rest to a new
/// `0600` file and rename it into place, holding the file's lock
/// ([`lock_revocations`]) from the read through the rename. The file is read
/// and stat'ed through one handle (never via a symlink); if `path` is no
/// longer that file, or it grew (a writer that doesn't take the lock), it is
/// left as it is, so no revocation is lost. `before_rename` is a test hook.
fn compact_revocations(
    path: &std::path::Path,
    before_rename: impl FnOnce(),
) -> std::io::Result<bool> {
    use std::io::Write;
    let _lock = lock_revocations(path, true)?;
    let mut f = open_nofollow(path, false)?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut f, &mut text)?;
    let len = text.len() as u64;
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
        let mut t = o.open(&tmp)?;
        for (sid, exp) in &keep {
            writeln!(t, "{}", json!({"sid": sid, "exp": exp}))?;
        }
        t.sync_all()?;
    }
    before_rename();
    if !same_file(&f, path).unwrap_or(false) || f.metadata()?.len() != len {
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

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anofox-rev-{tag}-{}",
            crate::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    /// Another replica's handle on the same file.
    fn replica(path: &std::path::Path) -> Revocations {
        Revocations {
            path: path.to_path_buf(),
            stamp: Mutex::new((None, None)),
            pending: Mutex::new(HashMap::new()),
            broken: std::sync::atomic::AtomicBool::new(false),
            failing: std::sync::atomic::AtomicBool::new(false),
            audit: std::sync::OnceLock::new(),
        }
    }

    #[test]
    fn an_append_waits_for_a_compaction_in_progress() {
        let dir = tmp("lock");
        let file = dir.join("revoked.jsonl");
        std::fs::write(&file, format!("{}\n", json!({"sid": "old", "exp": 1}))).unwrap();
        let exp = super::super::jwt::now_s() + 600;
        let mut other = None;
        let compacted = compact_revocations(&file, || {
            let r = replica(&file);
            let t = std::thread::spawn(move || r.record("new", exp));
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert!(!t.is_finished(), "an append ran during the compaction");
            other = Some(t);
        })
        .unwrap();
        assert!(compacted);
        other.unwrap().join().unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("\"new\"") && !text.contains("\"old\""),
            "{text}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let lock = dir.join("revoked.jsonl.lock");
            let mode = std::fs::metadata(&lock).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_logout_is_lost_to_concurrent_compactions() {
        // One replica logs sessions out while others start (and compact the
        // file) over and over: every logout must still be in the file.
        let rounds: usize = std::env::var("ANOFOX_REVOCATION_STRESS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(40);
        let dir = tmp("stress");
        let file = dir.join("revoked.jsonl");
        let exp = super::super::jwt::now_s() + 600;
        let mut missing = 0;
        for round in 0..rounds {
            let expired: String = (0..5)
                .map(|i| format!("{}\n", json!({"sid": format!("x{i}"), "exp": 1})))
                .collect();
            std::fs::write(&file, expired).unwrap();
            let r = replica(&file);
            let sids: Vec<String> = (0..20).map(|i| format!("r{round}-{i}")).collect();
            let todo = sids.clone();
            let t = std::thread::spawn(move || {
                for sid in &todo {
                    r.record(sid, exp);
                    std::thread::yield_now();
                }
                r
            });
            let mut compactions = 0;
            while !t.is_finished() {
                compact_revocations(&file, std::thread::yield_now).unwrap();
                compactions += 1;
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
            // what a reload does with logouts queued while the lock was busy
            let r = t.join().unwrap();
            r.flush(&mut r.pending.lock().unwrap());
            assert!(compactions > 0);
            let text = std::fs::read_to_string(&file).unwrap();
            missing += sids.iter().filter(|s| !text.contains(s.as_str())).count();
        }
        assert_eq!(missing, 0, "{missing} of {} logouts lost", rounds * 20);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other replica of [`no_logout_is_lost_to_another_process`]: run
    /// by that test (in a child process), a no-op otherwise.
    #[test]
    #[ignore]
    fn revocation_replica_process() {
        let Ok(path) = std::env::var("ANOFOX_REVOCATION_REPLICA") else {
            return;
        };
        let r = replica(std::path::Path::new(&path));
        let exp = super::super::jwt::now_s() + 600;
        for i in 0..50 {
            r.record(&format!("p{i}"), exp);
        }
        // what a reload does with logouts queued while the lock was busy
        let mut q = r.pending.lock().unwrap();
        for _ in 0..30 {
            r.flush(&mut q);
        }
        assert!(q.is_empty());
    }

    #[test]
    fn no_logout_is_lost_to_another_process() {
        let rounds: usize = std::env::var("ANOFOX_REVOCATION_STRESS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(10);
        let dir = tmp("proc");
        let file = dir.join("revoked.jsonl");
        for round in 0..rounds {
            std::fs::write(&file, format!("{}\n", json!({"sid": "x", "exp": 1}))).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "host::auth::session::tests::revocation_replica_process",
                ])
                .args(["--ignored", "--quiet", "--test-threads=1"])
                .env("ANOFOX_REVOCATION_REPLICA", &file)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let status = loop {
                compact_revocations(&file, || {}).unwrap();
                std::thread::sleep(std::time::Duration::from_micros(200));
                if let Some(s) = child.try_wait().unwrap() {
                    break s;
                }
            };
            assert!(status.success(), "round {round}: replica failed");
            let text = std::fs::read_to_string(&file).unwrap();
            for i in 0..50 {
                assert!(
                    text.contains(&format!("\"p{i}\"")),
                    "round {round}: p{i} lost"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compaction_reads_through_its_own_handle() {
        let dir = tmp("handle");
        let file = dir.join("revoked.jsonl");
        let line = format!("{}\n", json!({"sid": "old", "exp": 1}));
        std::fs::write(&file, &line).unwrap();
        // the file is swapped for another of the same length between the
        // read and the rename: the swapped-in file is left as it is
        let other = dir.join("other.jsonl");
        let swapped = line.replace("old", "new");
        std::fs::write(&other, &swapped).unwrap();
        let compacted = compact_revocations(&file, || {
            std::fs::rename(&other, &file).unwrap();
        })
        .unwrap();
        assert!(!compacted);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), swapped);
        // never through a symlink
        #[cfg(unix)]
        {
            let link = dir.join("link.jsonl");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(compact_revocations(&link, || {}).is_err());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), swapped);
            let r = replica(&link);
            assert!(r.read(&mut None).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_logout_during_an_outage_is_recorded_once_the_file_is_back() {
        let dir = tmp("outage");
        let file = dir.join("revoked.jsonl");
        let secret = Some("0123456789abcdef0123456789abcdef");
        let a = Sessions::new(secret, "login", 3600)
            .with_revocation_file(file.clone())
            .unwrap();
        let recheck = |s: &Sessions| s.revocations.as_ref().unwrap().stamp.lock().unwrap().1 = None;
        let (t1, s1) = a.issue("login", 60, &claims(), &Scope::default());
        let (t2, s2) = a.issue("login", 60, &claims(), &Scope::default());
        // the file is gone: no session verifies, but logout still counts —
        // and the file is not recreated (empty) by it
        std::fs::remove_file(&file).unwrap();
        recheck(&a);
        assert_eq!(a.verify("login", &t1), Err("revocations_unavailable"));
        assert_eq!(
            a.logout("login", &t1),
            Some((s1.sid.clone(), "alice".into()))
        );
        // logging out again records (and audits) nothing more
        assert_eq!(a.logout("login", &t1), None);
        a.revoke(&s1.sid, s1.exp);
        assert_eq!(
            a.revocations
                .as_ref()
                .unwrap()
                .pending
                .lock()
                .unwrap()
                .len(),
            1
        );
        assert!(!file.exists());
        // a symlink in its place: not written through either
        #[cfg(unix)]
        {
            let target = dir.join("target.jsonl");
            std::fs::write(&target, "").unwrap();
            std::os::unix::fs::symlink(&target, &file).unwrap();
            assert!(a.logout("login", &t2).is_some());
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
            std::fs::remove_file(&file).unwrap();
        }
        #[cfg(not(unix))]
        a.revoke(&s2.sid, s2.exp);
        // bad or foreign tokens log nothing out
        assert_eq!(a.logout("login", "x.y.z"), None);
        let other = Sessions::new(None, "login", 3600);
        let (t3, _) = other.issue("login", 60, &claims(), &Scope::default());
        assert_eq!(a.logout("login", &t3), None);
        // the file is restored: the queued logouts are written on reload
        crate::host::access::open_log(&file).unwrap();
        recheck(&a);
        assert_eq!(a.verify("login", &t1), Err("revoked"));
        assert_eq!(a.verify("login", &t2), Err("revoked"));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains(&s1.sid) && text.contains(&s2.sid), "{text}");
        // and other replicas (a restart) see them
        let b = Sessions::new(secret, "login", 3600)
            .with_revocation_file(file.clone())
            .unwrap();
        assert_eq!(b.verify("login", &t1), Err("revoked"));
        assert_eq!(b.verify("login", &t2), Err("revoked"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_file_without_its_revocation_file_refuses_to_start() {
        let dir = tmp("l4");
        let file = dir.join("revoked.jsonl");
        let start = || Sessions::new(None, "login", 60).with_revocation_file(file.clone());
        // the very first start creates both
        assert!(start().is_ok());
        assert!(file.exists() && lock_path(&file).exists());
        // the file is deleted (or moved away): refuse, don't start empty
        std::fs::remove_file(&file).unwrap();
        let e = start().unwrap_err();
        assert!(e.contains("missing"), "{e}");
        assert!(!file.exists());
        // restored: starts again
        std::fs::write(&file, "").unwrap();
        assert!(start().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_never_hangs_the_server() {
        let dir = tmp("l2");
        let fifo = |p: &std::path::Path| {
            let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        };
        let file = dir.join("revoked.jsonl");
        fifo(&file);
        let lock_file = dir.join("other.jsonl");
        std::fs::write(&lock_file, "").unwrap();
        fifo(&lock_path(&lock_file));
        // each open of a FIFO (no writer, no reader) fails at once
        let (tx, rx) = std::sync::mpsc::channel();
        let (f, l) = (file.clone(), lock_file.clone());
        std::thread::spawn(move || {
            let r = [
                open_nofollow(&f, false).is_err(),
                open_nofollow(&f, true).is_err(),
                crate::host::access::open_log(&f).is_err(),
                lock_revocations(&l, false).is_err(),
                lock_revocations(&l, true).is_err(),
                Sessions::new(None, "login", 60)
                    .with_revocation_file(f.clone())
                    .is_err(),
            ];
            tx.send(r).unwrap();
        });
        let r = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("hung on a FIFO");
        assert_eq!(r, [true; 6]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_file_is_private_read_write_and_no_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("l3");
        let file = dir.join("revoked.jsonl");
        std::fs::write(&file, "").unwrap();
        // created 0600; an existing one is tightened
        drop(lock_revocations(&file, true).unwrap());
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&lock_path(&file)), 0o600);
        std::fs::set_permissions(lock_path(&file), std::fs::Permissions::from_mode(0o644)).unwrap();
        let l = lock_revocations(&file, true).unwrap();
        assert_eq!(mode(&lock_path(&file)), 0o600);
        // opened for reading and writing (LockFileEx on Windows needs it)
        let fl = unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&l), libc::F_GETFL) };
        assert_eq!(fl & libc::O_ACCMODE, libc::O_RDWR);
        assert_eq!(fl & libc::O_APPEND, 0);
        drop(l);
        // never through a symlink
        let other = dir.join("other.jsonl");
        std::fs::write(&other, "").unwrap();
        std::os::unix::fs::symlink(dir.join("target"), lock_path(&other)).unwrap();
        assert!(lock_revocations(&other, true).is_err());
        assert!(!dir.join("target").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_held_too_long_queues_the_logout_and_is_audited_once() {
        let dir = tmp("l5");
        let file = dir.join("revoked.jsonl");
        let a = Sessions::new(None, "login", 3600)
            .with_revocation_file(file.clone())
            .unwrap();
        let audit = dir.join("audit.jsonl");
        a.set_audit(std::sync::Arc::new(
            super::super::audit::AuditLog::new(audit.to_str().unwrap()).unwrap(),
        ));
        // another writer holds the lock: a logout waits about a second, no more
        let held = lock_revocations(&file, true).unwrap();
        let (t1, s1) = a.issue("login", 60, &claims(), &Scope::default());
        let (t2, s2) = a.issue("login", 60, &claims(), &Scope::default());
        let started = std::time::Instant::now();
        assert!(a.logout("login", &t1).is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(a.logout("login", &t2).is_some());
        // refused in memory at once, queued for the file, reported once
        assert_eq!(a.verify("login", &t1), Err("revoked"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
        let log = std::fs::read_to_string(&audit).unwrap();
        assert_eq!(
            log.matches("\"reason\":\"write_failed\"").count(),
            1,
            "{log}"
        );
        assert!(log.contains("\"audit\":\"revocation_file\""), "{log}");
        // released: the next write takes both
        drop(held);
        let (_, s3) = a.issue("login", 60, &claims(), &Scope::default());
        a.revoke(&s3.sid, s3.exp);
        let text = std::fs::read_to_string(&file).unwrap();
        for s in [&s1, &s2, &s3] {
            assert!(text.contains(&s.sid), "{text}");
        }
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

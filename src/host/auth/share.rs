//! Share links (`{"mode": "share"}`, #763): a dashboard of the locked server
//! that anyone with its link may open (`"access": "public"`), or anyone who
//! knows its password (`"access": "password"`) — without a host application
//! signing embed links.
//!
//! ```json
//! {"mode": "share", "session_secret_env": "ANOFOX_SHARE_SESSION",
//!  "dashboards": {
//!    "sales": {"access": "public", "claims": {"tenant": "acme"}},
//!    "ops":   {"access": "password", "password_hash": "pbkdf2-sha256$600000$…$…",
//!              "groups": ["partner"], "vars": {"region": "EU"}}}}
//! ```
//!
//! * A share viewer is the server-made user `share:<dashboard>` (reserved: no
//!   other login method may claim it, and it never submits or saves) with the
//!   configured groups and claims, so the row and column policies apply as to
//!   any viewer. `vars` are **locked**, as an embed link's.
//! * Only the dashboards listed here are shared; nothing else of the server
//!   (the list page, other dashboards, `/healthz` details) is reachable.
//! * A password is never stored: `serve share-password` prints a
//!   PBKDF2-HMAC-SHA256 hash for the options (and, when it makes one up, the
//!   password once). A correct password sets a login cookie for that one
//!   dashboard (`Path=/d/<id>`), signed by this server.
//! * Wrong passwords are limited per client address and per dashboard;
//!   checking one takes a few hundred milliseconds, and at most
//!   [`MAX_CONCURRENT`] are checked at once.
//! * The page itself then gets a page session (as an embed link's page does)
//!   that its requests carry in `X-Anofox-Session`.

use super::session::{random_b64, secret, Sessions};
use super::{Reply, Scope};
use crate::host::access::{clean_claim, Claims, MAX_ATTRS, MAX_GROUPS};
use crate::host::serving::{html_escape, sql_literal, valid_ident};
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The users a share link signs in as: `share:<dashboard>`.
pub const USER_PREFIX: &str = crate::host::access::SHARE_USER_PREFIX;
/// The login cookie (one per dashboard: `Path=/d/<id>`).
pub const COOKIE: &str = "__Secure-anofox_share";
/// The audience of the login cookie and of the page session.
const LOGIN_AUD: &str = "share-login";
pub const PAGE_AUD: &str = "share";
/// PBKDF2 iterations of a new hash (OWASP 2023 for PBKDF2-HMAC-SHA256).
pub const ITERATIONS: u32 = 600_000;
/// Fewer iterations are refused in a configured hash.
const MIN_ITERATIONS: u32 = 100_000;
/// More are refused too (a hash must not be a CPU bomb).
const MAX_ITERATIONS: u32 = 10_000_000;
/// Wrong passwords per client address, and per dashboard, in [`WINDOW`].
pub const MAX_FAILS_PER_PEER: u32 = 10;
pub const MAX_FAILS_PER_DASHBOARD: u32 = 100;
pub const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Passwords checked at the same time (each takes a CPU for a while).
pub const MAX_CONCURRENT: usize = 4;
/// Longest password accepted (bytes).
const MAX_PASSWORD: usize = 256;
/// Shortest password `serve share-password` accepts.
pub const MIN_PASSWORD: usize = 12;
/// Most shared dashboards in one configuration.
const MAX_DASHBOARDS: usize = 256;
/// Most variables one dashboard may lock.
const MAX_VARS: usize = 64;

/// Who may open a shared dashboard.
#[derive(Clone, PartialEq, Eq)]
pub enum Access {
    Public,
    /// A PBKDF2-HMAC-SHA256 hash: iterations, salt, derived key.
    Password {
        iterations: NonZeroU32,
        salt: Vec<u8>,
        key: Vec<u8>,
    },
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Access::Public => "public",
            Access::Password { .. } => "password",
        })
    }
}

/// One shared dashboard.
#[derive(Debug, Clone)]
pub struct Shared {
    pub access: Access,
    pub claims: Claims,
    pub scope: Scope,
}

pub struct Share {
    dashboards: Vec<(String, Shared)>,
    pub session_ttl: u64,
    pub sessions: Sessions,
    limiter: Limiter,
    checking: AtomicUsize,
}

impl std::fmt::Debug for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Share")
            .field("dashboards", &self.dashboards.len())
            .field("session_ttl", &self.session_ttl)
            .finish_non_exhaustive()
    }
}

/// Why a password login did not succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginError {
    /// Not a password-protected dashboard of this server.
    NotShared,
    WrongPassword,
    /// Too many wrong passwords (from this address, or for this dashboard).
    Locked,
    /// Too many passwords are being checked right now.
    Busy,
}

impl LoginError {
    pub fn reason(self) -> &'static str {
        match self {
            LoginError::NotShared => "not_shared",
            LoginError::WrongPassword => "wrong_password",
            LoginError::Locked => "locked_out",
            LoginError::Busy => "busy",
        }
    }
}

/// A hash for `password` in the format `password_hash` takes:
/// `pbkdf2-sha256$<iterations>$<salt>$<key>` (base64url, no padding).
pub fn hash_password(password: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut salt = [0u8; 16];
    aws_lc_rs::rand::fill(&mut salt).expect("system randomness");
    let mut key = [0u8; 32];
    let it = NonZeroU32::new(ITERATIONS).expect("nonzero");
    aws_lc_rs::pbkdf2::derive(
        aws_lc_rs::pbkdf2::PBKDF2_HMAC_SHA256,
        it,
        &salt,
        password.as_bytes(),
        &mut key,
    );
    format!(
        "pbkdf2-sha256${ITERATIONS}${}${}",
        b64.encode(salt),
        b64.encode(key)
    )
}

/// A random password (base64url, 20 characters, ~120 bits).
pub fn generate_password() -> String {
    random_b64(15)
}

fn parse_hash(s: &str) -> Result<Access, String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bad = || {
        "password_hash: expected pbkdf2-sha256$<iterations>$<salt>$<key> (from `serve share-password`)"
            .to_string()
    };
    let p: Vec<&str> = s.trim().split('$').collect();
    let [alg, it, salt, key] = p.as_slice() else {
        return Err(bad());
    };
    if *alg != "pbkdf2-sha256" {
        return Err(bad());
    }
    let it: u32 = it.parse().map_err(|_| bad())?;
    if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&it) {
        return Err(format!(
            "password_hash: {MIN_ITERATIONS}..={MAX_ITERATIONS} iterations"
        ));
    }
    let salt = b64.decode(salt).map_err(|_| bad())?;
    let key = b64.decode(key).map_err(|_| bad())?;
    if salt.len() < 16 || key.len() != 32 {
        return Err(bad());
    }
    Ok(Access::Password {
        iterations: NonZeroU32::new(it).ok_or_else(bad)?,
        salt,
        key,
    })
}

/// Does `password` match? (Constant time in the derived key.)
fn check(access: &Access, password: &str) -> bool {
    match access {
        Access::Public => false,
        Access::Password {
            iterations,
            salt,
            key,
        } => aws_lc_rs::pbkdf2::verify(
            aws_lc_rs::pbkdf2::PBKDF2_HMAC_SHA256,
            *iterations,
            salt,
            password.as_bytes(),
            key,
        )
        .is_ok(),
    }
}

/// The dashboard id a path is about: `/d/<id>` or `/d/<id>/…`.
pub fn dashboard_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/d/")?;
    let id = rest.split('/').next().unwrap_or("");
    (!id.is_empty()).then_some(id)
}

/// Is `path` the page of dashboard `id` (not one of its assets)?
pub fn is_page(path: &str, id: &str) -> bool {
    path.strip_prefix("/d/")
        .and_then(|r| r.strip_prefix(id))
        .is_some_and(|r| r.is_empty() || r == "/")
}

impl Share {
    pub fn from_json(
        o: &serde_json::Map<String, Value>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Share, String> {
        for k in o.keys() {
            if !matches!(
                k.as_str(),
                "mode" | "dashboards" | "session_ttl_s" | "session_secret" | "session_secret_env"
            ) {
                return Err(format!(
                    "auth (share): unknown key '{k}' (dashboards, session_ttl_s, session_secret_env)"
                ));
            }
        }
        let session_secret = secret(o, "session_secret", env, false)?;
        let session_ttl = match o.get("session_ttl_s") {
            None => 8 * 3600,
            Some(v) => v
                .as_u64()
                .filter(|n| (60..=7 * 86_400).contains(n))
                .ok_or("auth.session_ttl_s (share) must be an integer 60..=604800")?,
        };
        let ds = o
            .get("dashboards")
            .and_then(Value::as_object)
            .filter(|d| !d.is_empty() && d.len() <= MAX_DASHBOARDS)
            .ok_or("auth.dashboards (share): an object of 1..=256 dashboards, {\"<id>\": {\"access\": \"public\" | \"password\", …}}")?;
        let mut dashboards = Vec::new();
        for (id, d) in ds {
            if !valid_ident(id) {
                return Err(format!(
                    "auth.dashboards (share): '{id}' is not a dashboard id"
                ));
            }
            dashboards.push((id.clone(), shared(id, d)?));
        }
        Ok(Share {
            dashboards,
            session_ttl,
            sessions: Sessions::new(session_secret.as_deref(), "share", session_ttl),
            limiter: Limiter::default(),
            checking: AtomicUsize::new(0),
        })
    }

    /// The shared dashboard `id`.
    pub fn get(&self, id: &str) -> Option<&Shared> {
        self.dashboards
            .iter()
            .find(|(d, _)| d == id)
            .map(|(_, s)| s)
    }

    /// The ids shared, and how.
    pub fn list(&self) -> impl Iterator<Item = (&str, &Access)> {
        self.dashboards.iter().map(|(d, s)| (d.as_str(), &s.access))
    }

    /// A login cookie (any of `cookies`) valid for dashboard `id`.
    pub fn logged_in(&self, id: &str, cookies: &[&str]) -> Result<(), &'static str> {
        let mut last = "malformed_token";
        for c in cookies {
            match self.sessions.verify(LOGIN_AUD, c.trim()) {
                Ok(s) if s.scope.dashboards.as_deref() == Some(&[id.to_string()][..]) => {
                    return Ok(())
                }
                Ok(_) => last = "out_of_scope",
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Check a password for dashboard `id`: the login cookie's value.
    pub fn login(
        &self,
        id: &str,
        password: &str,
        peer: Option<IpAddr>,
    ) -> Result<String, LoginError> {
        let Some(d) = self.get(id).filter(|d| d.access != Access::Public) else {
            return Err(LoginError::NotShared);
        };
        if self.limiter.locked(id, peer) {
            return Err(LoginError::Locked);
        }
        if self.checking.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
            self.checking.fetch_sub(1, Ordering::SeqCst);
            return Err(LoginError::Busy);
        }
        let ok = password.len() <= MAX_PASSWORD && check(&d.access, password);
        self.checking.fetch_sub(1, Ordering::SeqCst);
        if !ok {
            self.limiter.fail(id, peer);
            return Err(LoginError::WrongPassword);
        }
        let (token, _) = self
            .sessions
            .issue(LOGIN_AUD, self.session_ttl, &d.claims, &d.scope);
        Ok(token)
    }

    /// The `Set-Cookie` value of a login for `id`.
    pub fn cookie(&self, id: &str, token: &str) -> String {
        format!(
            "{COOKIE}={token}; Path=/d/{id}; Max-Age={}; HttpOnly; Secure; SameSite=Lax",
            self.session_ttl
        )
    }

    /// The password page of dashboard `id` (`next`: where to go after it).
    pub fn password_page(&self, id: &str, next: &str, error: Option<&str>) -> Reply {
        let err = error
            .map(|e| {
                format!(
                    "<p role=\"alert\" style=\"color:#b42318\">{}</p>",
                    html_escape(e)
                )
            })
            .unwrap_or_default();
        let mut r = super::oidc::page(
            "Password required",
            &format!(
                "{err}<form method=\"post\" action=\"/share/{id}\">\
<label for=\"pw\">Password for <b>{name}</b></label><br>\
<input id=\"pw\" type=\"password\" name=\"password\" autocomplete=\"current-password\" required autofocus \
style=\"font:inherit;padding:.4rem;margin:.5rem 0;width:100%;max-width:22rem\"><br>\
<input type=\"hidden\" name=\"next\" value=\"{next}\">\
<button type=\"submit\" style=\"font:inherit;padding:.4rem 1rem\">Open</button></form>",
                id = html_escape(id),
                name = html_escape(id),
                next = html_escape(next),
            ),
        );
        if error.is_some() {
            r.status = 401;
        }
        r.header("Cache-Control", "no-store")
            .header("Referrer-Policy", "same-origin")
    }

    /// Where a login for `id` goes next: `next` when it is that dashboard's
    /// page (with a query), else the page itself — never another site.
    pub fn next_url(id: &str, next: Option<&str>) -> String {
        let page = format!("/d/{id}");
        match next {
            Some(n)
                if (n == page || n.starts_with(&format!("{page}?")))
                    && !n.contains(['\\', '\r', '\n']) =>
            {
                n.to_string()
            }
            _ => page,
        }
    }
}

fn shared(id: &str, d: &Value) -> Result<Shared, String> {
    let o = d
        .as_object()
        .ok_or_else(|| format!("auth.dashboards.{id} (share) must be an object"))?;
    for k in o.keys() {
        if !matches!(
            k.as_str(),
            "access" | "password_hash" | "groups" | "claims" | "vars"
        ) {
            return Err(format!(
                "auth.dashboards.{id} (share): unknown key '{k}' (access, password_hash, groups, claims, vars)"
            ));
        }
    }
    let access = match (
        o.get("access").and_then(Value::as_str),
        o.get("password_hash"),
    ) {
        (Some("public"), None) => Access::Public,
        (Some("public"), Some(_)) => {
            return Err(format!(
                "auth.dashboards.{id} (share): a public dashboard has no password_hash"
            ))
        }
        (Some("password"), Some(h)) => parse_hash(h.as_str().unwrap_or(""))
            .map_err(|e| format!("auth.dashboards.{id} (share): {e}"))?,
        (Some("password"), None) => {
            return Err(format!(
                "auth.dashboards.{id} (share): password_hash is required (`serve share-password`)"
            ))
        }
        _ => {
            return Err(format!(
                "auth.dashboards.{id} (share): access must be \"public\" or \"password\""
            ))
        }
    };
    let mut groups = Vec::new();
    if let Some(g) = o.get("groups") {
        let g = g
            .as_array()
            .filter(|a| a.len() <= MAX_GROUPS)
            .ok_or_else(|| format!("auth.dashboards.{id}.groups must be a list of names"))?;
        for x in g {
            let x = x
                .as_str()
                .and_then(|s| clean_claim(s).ok())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("auth.dashboards.{id}.groups: invalid group"))?;
            groups.push(x);
        }
    }
    groups.sort();
    groups.dedup();
    let mut attrs = Vec::new();
    if let Some(c) = o.get("claims") {
        let c = c
            .as_object()
            .filter(|c| c.len() <= MAX_ATTRS)
            .ok_or_else(|| {
                format!("auth.dashboards.{id}.claims: an object of at most {MAX_ATTRS} claims")
            })?;
        for (name, v) in c {
            if !crate::host::access::valid_claim_name(name) {
                return Err(format!(
                    "auth.dashboards.{id}.claims: invalid claim name '{name}'"
                ));
            }
            let v =
                match v {
                    Value::Null => None,
                    Value::String(s) => Some(clean_claim(s).map_err(|_| {
                        format!("auth.dashboards.{id}.claims.{name}: invalid value")
                    })?),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => {
                        return Err(format!(
                            "auth.dashboards.{id}.claims.{name}: a string, number or boolean"
                        ))
                    }
                }
                .filter(|v| !v.is_empty());
            attrs.push((name.to_ascii_lowercase(), v));
        }
    }
    let mut vars = Vec::new();
    if let Some(vs) = o.get("vars") {
        let vs = vs
            .as_object()
            .filter(|v| v.len() <= MAX_VARS)
            .ok_or_else(|| {
                format!("auth.dashboards.{id}.vars: an object of at most {MAX_VARS} variables")
            })?;
        for (name, val) in vs {
            if !super::lockable_var(name) {
                return Err(format!(
                    "auth.dashboards.{id}.vars: '{name}' can't be locked"
                ));
            }
            vars.push((
                name.clone(),
                sql_literal(val).map_err(|e| format!("auth.dashboards.{id}.vars.{name}: {e}"))?,
            ));
        }
    }
    Ok(Shared {
        access,
        claims: Claims {
            user: format!("{USER_PREFIX}{id}"),
            groups,
            attrs,
        },
        scope: Scope {
            dashboards: Some(vec![id.to_string()]),
            vars,
        },
    })
}

/// Wrong passwords in the current window, per client address and per
/// dashboard (this replica only).
#[derive(Default)]
struct Limiter(Mutex<HashMap<String, (u32, Instant)>>);

impl Limiter {
    fn keys(id: &str, peer: Option<IpAddr>) -> [(String, u32); 2] {
        let peer = peer.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
        [
            (format!("p\u{0}{peer}"), MAX_FAILS_PER_PEER),
            (format!("d\u{0}{id}"), MAX_FAILS_PER_DASHBOARD),
        ]
    }

    fn locked(&self, id: &str, peer: Option<IpAddr>) -> bool {
        let m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        Self::keys(id, peer).iter().any(|(k, max)| {
            m.get(k)
                .is_some_and(|(n, t)| now.duration_since(*t) < WINDOW && n >= max)
        })
    }

    fn fail(&self, id: &str, peer: Option<IpAddr>) {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        if m.len() >= 10_000 {
            m.retain(|_, (_, t)| now.duration_since(*t) < WINDOW);
        }
        for (k, _) in Self::keys(id, peer) {
            // A full map (10 000 live entries) still counts the dashboard.
            if m.len() >= 10_000 && !m.contains_key(&k) && k.starts_with('p') {
                continue;
            }
            let e = m.entry(k).or_insert((0, now));
            if now.duration_since(e.1) >= WINDOW {
                *e = (0, now);
            }
            e.0 += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn share(v: Value) -> Result<Share, String> {
        Share::from_json(v.as_object().unwrap(), &|_| None)
    }

    #[test]
    fn hashes_verify_and_parse() {
        let h = hash_password("correct horse battery");
        let a = parse_hash(&h).unwrap();
        assert!(check(&a, "correct horse battery"));
        assert!(!check(&a, "correct horse batterY"));
        assert!(!check(&a, ""));
        assert_ne!(h, hash_password("correct horse battery"), "salted");
        for bad in [
            "",
            "pbkdf2-sha256$600000$abc",
            "bcrypt$600000$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "pbkdf2-sha256$1000$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "pbkdf2-sha256$99999999$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            assert!(parse_hash(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn config() {
        let h = hash_password("a long enough password");
        let s = share(json!({"mode": "share", "dashboards": {
            "sales": {"access": "public", "claims": {"tenant": "acme"}, "groups": ["b", "a", "a"]},
            "ops": {"access": "password", "password_hash": h, "vars": {"region": "EU"}}}}))
        .unwrap();
        let sales = s.get("sales").unwrap();
        assert_eq!(sales.access, Access::Public);
        assert_eq!(sales.claims.user, "share:sales");
        assert_eq!(sales.claims.groups, ["a", "b"]);
        assert_eq!(
            sales.claims.attrs,
            [("tenant".to_string(), Some("acme".to_string()))]
        );
        assert_eq!(
            sales.scope.dashboards.as_deref(),
            Some(&["sales".to_string()][..])
        );
        let ops = s.get("ops").unwrap();
        assert_eq!(ops.scope.vars, [("region".to_string(), "'EU'".to_string())]);
        assert!(s.get("other").is_none());
        for (bad, why) in [
            (json!({"mode": "share"}), "dashboards"),
            (json!({"mode": "share", "dashboards": {}}), "dashboards"),
            (
                json!({"mode": "share", "dashboards": {"a b": {"access": "public"}}}),
                "id",
            ),
            (
                json!({"mode": "share", "dashboards": {"x": {"access": "open"}}}),
                "access",
            ),
            (
                json!({"mode": "share", "dashboards": {"x": {"access": "password"}}}),
                "password_hash",
            ),
            (
                json!({"mode": "share", "dashboards": {"x": {"access": "public", "password_hash": h}}}),
                "public",
            ),
            (
                json!({"mode": "share", "dashboards": {"x": {"access": "public", "vars": {"__claim_user": "x"}}}}),
                "locked",
            ),
            (
                json!({"mode": "share", "dashboards": {"x": {"access": "public", "user": "bob"}}}),
                "unknown key",
            ),
            (
                json!({"mode": "share", "dashboards": {"x": {"access": "public"}}, "secret": "x"}),
                "unknown key",
            ),
            (
                json!({"mode": "share", "dashboards": {"x": {"access": "public"}}, "session_ttl_s": 1}),
                "session_ttl_s",
            ),
        ] {
            let e = share(bad.clone()).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
    }

    #[test]
    fn login_cookie_and_limits() {
        let h = hash_password("a long enough password");
        let s = share(json!({"mode": "share", "dashboards": {
            "ops": {"access": "password", "password_hash": h},
            "ops2": {"access": "password", "password_hash": hash_password("another long password")},
            "pub": {"access": "public"}}}))
        .unwrap();
        let peer: Option<IpAddr> = Some("203.0.113.9".parse().unwrap());
        assert_eq!(s.login("pub", "x", peer), Err(LoginError::NotShared));
        assert_eq!(s.login("nope", "x", peer), Err(LoginError::NotShared));
        let tok = s.login("ops", "a long enough password", peer).unwrap();
        assert!(s.logged_in("ops", &[&tok]).is_ok());
        // a login for one dashboard is not a login for another
        assert_eq!(s.logged_in("ops2", &[&tok]), Err("out_of_scope"));
        assert!(s.logged_in("ops", &["garbage"]).is_err());
        // a page session is not a login cookie
        let (page, _) = s.sessions.issue(
            PAGE_AUD,
            60,
            &s.get("ops").unwrap().claims,
            &s.get("ops").unwrap().scope,
        );
        assert!(s.logged_in("ops", &[&page]).is_err());
        assert!(s.cookie("ops", &tok).contains("Path=/d/ops;"));
        // wrong passwords: locked out after MAX_FAILS_PER_PEER from one address
        for _ in 0..MAX_FAILS_PER_PEER {
            assert_eq!(
                s.login("ops", "wrong", peer),
                Err(LoginError::WrongPassword)
            );
        }
        assert_eq!(
            s.login("ops", "a long enough password", peer),
            Err(LoginError::Locked)
        );
        // …another address still gets in
        let other: Option<IpAddr> = Some("198.51.100.1".parse().unwrap());
        assert!(s.login("ops", "a long enough password", other).is_ok());
    }

    #[test]
    fn next_and_paths() {
        assert_eq!(Share::next_url("ops", Some("/d/ops?v.x=1")), "/d/ops?v.x=1");
        assert_eq!(Share::next_url("ops", Some("/d/other")), "/d/ops");
        assert_eq!(
            Share::next_url("ops", Some("https://evil.example/d/ops")),
            "/d/ops"
        );
        assert_eq!(Share::next_url("ops", Some("/d/opsx")), "/d/ops");
        assert_eq!(Share::next_url("ops", None), "/d/ops");
        assert_eq!(dashboard_of("/d/ops"), Some("ops"));
        assert_eq!(dashboard_of("/d/ops/assets/x.png"), Some("ops"));
        assert_eq!(dashboard_of("/d/"), None);
        assert_eq!(dashboard_of("/api/panel"), None);
        assert!(is_page("/d/ops", "ops") && is_page("/d/ops/", "ops"));
        assert!(!is_page("/d/ops/assets/x", "ops") && !is_page("/d/opsx", "ops"));
    }

    #[test]
    fn password_page_escapes() {
        let s = share(
            json!({"mode": "share", "dashboards": {"ops": {"access": "password",
            "password_hash": hash_password("a long enough password")}}}),
        )
        .unwrap();
        let p = s.password_page("ops", "/d/ops?x=\"><script>", Some("Wrong <b>"));
        let body = String::from_utf8(p.body.clone()).unwrap();
        assert!(!body.contains("<script>") && !body.contains("Wrong <b>"));
        assert_eq!(p.status, 401);
        assert_eq!(p.header_value("Cache-Control"), Some("no-store"));
    }
}

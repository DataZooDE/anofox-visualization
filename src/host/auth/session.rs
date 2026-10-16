//! Server-signed sessions: the OIDC login cookie and the page session an
//! embed link is exchanged for. A session is an HS256 JWT signed with a key
//! only this server (or its replicas, with `session_secret_env`) holds, so it
//! carries the claims and scope without server-side state: replicas stay
//! stateless. Expiry is the server's (`exp` in the signed payload, checked on
//! every request), not the browser's. Logout revokes the session id on the
//! replica that handled it (see `docs/secure-serving.md`).

use super::Scope;
use crate::host::access::{is_reserved_var, Claims};
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
    /// `label` separates the keys of different uses of one secret.
    pub fn new(secret: Option<&str>, label: &str) -> Sessions {
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
        let s = |v: &Value| v.as_str().map(str::to_string).ok_or(bad);
        let strs = |v: &Value| -> Result<Vec<String>, &'static str> {
            v.as_array().ok_or(bad)?.iter().map(s).collect()
        };
        let sid = s(&p["sid"])?;
        if self.is_revoked(&sid) {
            return Err("revoked");
        }
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
        if vars
            .iter()
            .any(|(n, _)| !valid_ident(n) || is_reserved_var(n))
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
        let mut r = self.revoked.lock().unwrap_or_else(|p| p.into_inner());
        r.retain(|_, e| *e >= now);
        r.insert(sid.to_string(), exp);
    }

    fn is_revoked(&self, sid: &str) -> bool {
        let r = self.revoked.lock().unwrap_or_else(|p| p.into_inner());
        r.contains_key(sid)
    }
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
        let s = Sessions::new(None, "login");
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
        let other = Sessions::new(None, "login");
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
        let a = Sessions::new(Some("0123456789abcdef0123456789abcdef"), "login");
        let b = Sessions::new(Some("0123456789abcdef0123456789abcdef"), "login");
        let c = Sessions::new(Some("0123456789abcdef0123456789abcdef"), "embed");
        let (t, _) = a.issue("login", 60, &claims(), &Scope::default());
        assert!(b.verify("login", &t).is_ok());
        assert_eq!(c.verify("login", &t), Err("bad_signature"));
    }

    #[test]
    fn expiry_and_revocation() {
        let s = Sessions::new(None, "login");
        let t = s.sign_raw("login", super::super::jwt::now_s() - 1, json!({"sid": "x"}));
        assert_eq!(s.verify("login", &t), Err("expired"));
        let (t, sess) = s.issue("login", 60, &claims(), &Scope::default());
        s.revoke(&sess.sid, sess.exp);
        assert_eq!(s.verify("login", &t), Err("revoked"));
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

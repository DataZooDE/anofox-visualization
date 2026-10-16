//! Signed embed links (`{"mode": "embed"}`): the host application signs
//! `{dashboard, user, groups, claims, vars, exp}` as an HS256 JWT with a
//! secret it shares with this server, and links to `/d/<id>?embed=<token>`.
//! The server checks the signature (constant time), the expiry (required, at
//! most `max_link_ttl_s` ahead), that the link names *this* dashboard, that
//! every claim name is allowed (`claims`) and every variable name is an
//! identifier that is not reserved. The link is then exchanged for a page
//! session ([`super::session`]) scoped to that one dashboard, with the link's
//! variables **locked**: set by the server after the viewer's, whatever the
//! URL or the page asks for. Nothing in the URL besides the signed token can
//! change the scope.
//!
//! Replay: a link works for anyone who has it until it expires — keep
//! `max_link_ttl_s` short (minutes) and mint links per page view; with
//! `one_time: true` a link carrying a `jti` is accepted once per server
//! process.

use super::jwt::{looks_like_jwt, now_s, reason};
use super::session::{secret, Seen, Sessions};
use super::Scope;
use crate::host::access::{clean_claim, AuthError, Claims, MAX_ATTRS, MAX_GROUPS};
use crate::host::serving::sql_literal;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::Value;

/// Max variables a link may lock.
const MAX_VARS: usize = 64;
/// Fields a link may carry.
const FIELDS: [&str; 12] = [
    "dashboard",
    "user",
    "groups",
    "claims",
    "vars",
    "exp",
    "iat",
    "nbf",
    "jti",
    "iss",
    "aud",
    "sub",
];

pub struct Embed {
    key: DecodingKey,
    pub max_link_ttl: u64,
    pub session_ttl: u64,
    leeway: u64,
    /// Claim names a link may set (`__claim_<name>`).
    pub claims: Vec<String>,
    one_time: bool,
    seen: Seen,
    pub sessions: Sessions,
}

impl std::fmt::Debug for Embed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embed")
            .field("max_link_ttl", &self.max_link_ttl)
            .field("session_ttl", &self.session_ttl)
            .field("claims", &self.claims)
            .field("one_time", &self.one_time)
            .finish_non_exhaustive()
    }
}

impl Embed {
    pub fn from_json(
        o: &serde_json::Map<String, Value>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Embed, String> {
        for k in o.keys() {
            if !matches!(
                k.as_str(),
                "mode"
                    | "secret"
                    | "secret_env"
                    | "max_link_ttl_s"
                    | "session_ttl_s"
                    | "leeway_s"
                    | "claims"
                    | "one_time"
                    | "session_secret"
                    | "session_secret_env"
            ) {
                return Err(format!("auth (embed): unknown key '{k}' (secret_env, max_link_ttl_s, session_ttl_s, leeway_s, claims, one_time, session_secret_env)"));
            }
        }
        let s = secret(o, "secret", env, true)?.unwrap_or_default();
        let session_secret = secret(o, "session_secret", env, false)?;
        if session_secret.as_deref() == Some(s.as_str()) {
            return Err("auth (embed): session_secret must differ from secret (the host application must not be able to mint sessions)".into());
        }
        let secs = |k: &str, default: u64, max: u64| -> Result<u64, String> {
            match o.get(k) {
                None => Ok(default),
                Some(v) => v
                    .as_u64()
                    .filter(|n| (1..=max).contains(n) || (k == "leeway_s" && *n == 0))
                    .ok_or_else(|| format!("auth.{k} must be an integer 1..={max}")),
            }
        };
        let mut claims = Vec::new();
        if let Some(c) = o.get("claims") {
            let c = c
                .as_array()
                .ok_or("auth.claims (embed) must list the claim names a link may set")?;
            if c.len() > MAX_ATTRS {
                return Err(format!("auth.claims: at most {MAX_ATTRS} claims"));
            }
            for n in c {
                let n = n
                    .as_str()
                    .filter(|n| crate::host::access::valid_claim_name(n))
                    .ok_or("auth.claims: invalid claim name")?;
                claims.push(n.to_ascii_lowercase());
            }
        }
        Ok(Embed {
            key: DecodingKey::from_secret(s.as_bytes()),
            max_link_ttl: secs("max_link_ttl_s", 300, 86_400)?,
            session_ttl: secs("session_ttl_s", 3600, 86_400)?,
            leeway: secs("leeway_s", 30, super::jwt::MAX_LEEWAY_S)?,
            claims,
            one_time: match o.get("one_time") {
                None => false,
                Some(v) => v.as_bool().ok_or("auth.one_time must be a boolean")?,
            },
            seen: Seen::default(),
            sessions: Sessions::new(session_secret.as_deref(), "embed"),
        })
    }

    /// Verify an embed link for dashboard `id`: the claims and the scope (this
    /// dashboard, the locked variables), or the reason it was refused.
    pub fn verify_link(&self, token: &str, id: &str) -> Result<(Claims, Scope), &'static str> {
        if !looks_like_jwt(token) {
            return Err("malformed_token");
        }
        let header = jsonwebtoken::decode_header(token).map_err(|e| reason(&e))?;
        if header.alg != Algorithm::HS256 || header.crit.is_some() {
            return Err("bad_alg");
        }
        let mut v = Validation::new(Algorithm::HS256);
        v.algorithms = vec![Algorithm::HS256];
        v.leeway = self.leeway;
        v.validate_exp = true;
        v.validate_nbf = true;
        v.validate_aud = false;
        v.set_required_spec_claims(&["exp"]);
        // HMAC verification in constant time (aws-lc-rs).
        let p = jsonwebtoken::decode::<Value>(token, &self.key, &v)
            .map_err(|e| reason(&e))?
            .claims;
        let o = p.as_object().ok_or("malformed_token")?;
        if o.keys().any(|k| !FIELDS.contains(&k.as_str())) {
            return Err("unknown_field");
        }
        let now = now_s();
        let exp = p["exp"].as_u64().ok_or("missing_claim")?;
        if exp > now + self.max_link_ttl + self.leeway {
            return Err("link_ttl");
        }
        super::jwt::check_iat(&p, self.leeway, None)?;
        if p.get("dashboard").and_then(Value::as_str) != Some(id) {
            return Err("out_of_scope");
        }
        let claims = self.claims_of(&p).map_err(|e| e.reason())?;
        let mut vars = Vec::new();
        if let Some(vs) = p.get("vars") {
            let vs = vs.as_object().ok_or("bad_vars")?;
            if vs.len() > MAX_VARS {
                return Err("bad_vars");
            }
            for (name, val) in vs {
                if !super::lockable_var(name) {
                    return Err("bad_vars");
                }
                vars.push((name.clone(), sql_literal(val).map_err(|_| "bad_vars")?));
            }
        }
        if self.one_time {
            let jti = p
                .get("jti")
                .and_then(Value::as_str)
                .ok_or("missing_claim")?;
            if jti.is_empty() || jti.len() > 128 || !self.seen.first(jti, exp + self.leeway) {
                return Err("replayed");
            }
        }
        Ok((
            claims,
            Scope {
                dashboards: Some(vec![id.to_string()]),
                vars,
            },
        ))
    }

    fn claims_of(&self, p: &Value) -> Result<Claims, AuthError> {
        let user = match p.get("user") {
            None | Some(Value::Null) => "embed".to_string(),
            Some(Value::String(s)) => clean_claim(s)?,
            Some(_) => return Err(AuthError::Malformed),
        };
        if user.is_empty() {
            return Err(AuthError::NoUser);
        }
        let mut groups = Vec::new();
        match p.get("groups") {
            None | Some(Value::Null) => {}
            Some(Value::Array(a)) if a.len() <= MAX_GROUPS => {
                for g in a {
                    let g = clean_claim(g.as_str().ok_or(AuthError::Malformed)?)?;
                    if !g.is_empty() {
                        groups.push(g);
                    }
                }
            }
            Some(_) => return Err(AuthError::Malformed),
        }
        groups.sort();
        groups.dedup();
        let given = match p.get("claims") {
            None | Some(Value::Null) => serde_json::Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => return Err(AuthError::Malformed),
        };
        // Only configured claim names: a link can't invent `__claim_*` names.
        if given
            .keys()
            .any(|k| !self.claims.contains(&k.to_ascii_lowercase()))
        {
            return Err(AuthError::Malformed);
        }
        let mut attrs = Vec::new();
        for name in &self.claims {
            let v = match given.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
                None | Some((_, Value::Null)) => None,
                Some((_, Value::String(s))) => Some(clean_claim(s)?),
                Some((_, Value::Number(n))) => Some(n.to_string()),
                Some((_, Value::Bool(b))) => Some(b.to_string()),
                Some(_) => return Err(AuthError::Malformed),
            }
            .filter(|v| !v.is_empty());
            attrs.push((name.clone(), v));
        }
        Ok(Claims {
            user,
            groups,
            attrs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header};
    use serde_json::json;

    const SECRET: &str = "embed-secret-0123456789abcdef0123456789";

    fn embed(extra: Value) -> Embed {
        let mut o = json!({"mode": "embed", "secret": SECRET, "claims": ["tenant"]});
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        Embed::from_json(o.as_object().unwrap(), &|_| None).unwrap()
    }

    fn link(p: Value) -> String {
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &p,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    fn good() -> Value {
        json!({"dashboard": "sales", "user": "acme-viewer", "groups": ["viewers"],
               "claims": {"tenant": "acme"}, "vars": {"region": "EU", "year": 2026},
               "exp": now_s() + 120})
    }

    #[test]
    fn a_link_scopes_the_session_to_its_dashboard_and_vars() {
        let e = embed(json!({}));
        let (c, s) = e.verify_link(&link(good()), "sales").unwrap();
        assert_eq!(c.user, "acme-viewer");
        assert_eq!(c.groups, vec!["viewers".to_string()]);
        assert_eq!(
            c.attrs,
            vec![("tenant".to_string(), Some("acme".to_string()))]
        );
        assert_eq!(s.dashboards, Some(vec!["sales".to_string()]));
        assert_eq!(
            s.vars,
            vec![
                ("region".into(), "'EU'".into()),
                ("year".into(), "2026".into())
            ]
        );
        assert!(s.allows("sales") && !s.allows("payroll"));
        // the URL can't widen the scope: the same link on another dashboard
        assert_eq!(e.verify_link(&link(good()), "payroll"), Err("out_of_scope"));
        // viewer variables are overridden by the locked ones
        let mut vars = vec![
            ("region".to_string(), "'US'".to_string()),
            ("q".into(), "1".into()),
        ];
        s.lock_vars(&mut vars);
        assert_eq!(
            vars,
            vec![
                ("q".to_string(), "1".to_string()),
                ("region".into(), "'EU'".into()),
                ("year".into(), "2026".into())
            ]
        );
    }

    #[test]
    fn tampered_expired_and_overlong_links_are_refused() {
        let e = embed(json!({"leeway_s": 0, "max_link_ttl_s": 600}));
        let t = link(good());
        // tamper: swap the payload for another tenant, keep the signature
        let parts: Vec<&str> = t.split('.').collect();
        let mut p = good();
        p["claims"]["tenant"] = json!("other");
        let forged = {
            use base64::Engine;
            let b = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(p.to_string());
            format!("{}.{b}.{}", parts[0], parts[2])
        };
        assert_eq!(e.verify_link(&forged, "sales"), Err("bad_signature"));
        // signed with another secret
        let other = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &good(),
            &EncodingKey::from_secret(b"another-secret-0123456789abcdef0123456"),
        )
        .unwrap();
        assert_eq!(e.verify_link(&other, "sales"), Err("bad_signature"));
        // expired, no exp, an exp too far ahead
        let mut p = good();
        p["exp"] = json!(now_s() - 1);
        assert_eq!(e.verify_link(&link(p), "sales"), Err("expired"));
        let mut p = good();
        p.as_object_mut().unwrap().remove("exp");
        assert_eq!(e.verify_link(&link(p), "sales"), Err("missing_claim"));
        let mut p = good();
        p["exp"] = json!(now_s() + 86_400);
        assert_eq!(e.verify_link(&link(p), "sales"), Err("link_ttl"));
        // other algorithms (alg none, RS256 header) are refused
        let h = {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#)
        };
        assert!(e
            .verify_link(&format!("{h}.{}.", parts[1]), "sales")
            .is_err());
        assert_eq!(
            e.verify_link("not a token", "sales"),
            Err("malformed_token")
        );
    }

    #[test]
    fn links_cannot_name_reserved_or_unknown_things() {
        let e = embed(json!({}));
        for (k, v, why) in [
            ("vars", json!({"__claim_user": "admin"}), "bad_vars"),
            ("vars", json!({"__CLAIM_groups": ["admins"]}), "bad_vars"),
            ("vars", json!({"anofox_data": "/etc"}), "bad_vars"),
            ("vars", json!({"__ANOFOX_rls_sales": 1}), "bad_vars"),
            ("vars", json!({"a; DROP": 1}), "bad_vars"),
            ("vars", json!({"x": {"nested": 1}}), "bad_vars"),
            ("claims", json!({"role": "admin"}), "malformed_identity"),
            ("claims", json!({"user": "admin"}), "malformed_identity"),
            ("user", json!("a\nb"), "malformed_identity"),
            ("dashboards", json!(["payroll"]), "unknown_field"),
        ] {
            let mut p = good();
            p[k] = v;
            assert_eq!(e.verify_link(&link(p), "sales"), Err(why), "{k}");
        }
    }

    #[test]
    fn one_time_links_are_not_replayable() {
        let e = embed(json!({"one_time": true}));
        let mut p = good();
        assert_eq!(
            e.verify_link(&link(p.clone()), "sales"),
            Err("missing_claim")
        );
        p["jti"] = json!("link-1");
        let t = link(p);
        assert!(e.verify_link(&t, "sales").is_ok());
        assert_eq!(e.verify_link(&t, "sales"), Err("replayed"));
    }

    #[test]
    fn config_is_validated() {
        let p = |v: Value| Embed::from_json(v.as_object().unwrap(), &|_| None);
        assert!(p(json!({"mode": "embed"})).is_err());
        assert!(p(json!({"mode": "embed", "secret": "short"})).is_err());
        assert!(p(json!({"mode": "embed", "secret": SECRET, "nope": 1})).is_err());
        assert!(p(json!({"mode": "embed", "secret": SECRET, "claims": ["a-b"]})).is_err());
        assert!(p(json!({"mode": "embed", "secret": SECRET, "max_link_ttl_s": 0})).is_err());
        assert!(p(json!({"mode": "embed", "secret": SECRET, "session_secret": SECRET})).is_err());
        assert!(!format!("{:?}", embed(json!({}))).contains(SECRET));
    }
}

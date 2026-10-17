//! OpenID Connect login (`{"mode": "oidc"}`): the authorization code flow with
//! PKCE (S256), `state` and `nonce`, ending in a server-signed session cookie.
//!
//! * `GET /auth/login?next=/d/sales` — random `state`, `nonce` and PKCE
//!   verifier go into a short-lived, signed, `HttpOnly` cookie
//!   (`__Host-anofox_oidc`, 10 minutes); the browser is sent to the IdP.
//! * `GET /auth/callback?code&state` — the state must match the cookie's
//!   (constant time); the code is exchanged (with the PKCE verifier and the
//!   client secret, over HTTPS); the ID token is verified like a bearer token
//!   (signature by the IdP's JWKS, `iss`, `aud` = client id, `azp`, `exp`,
//!   `iat`) and must carry the cookie's `nonce`. Its claims become the
//!   session (`__Host-anofox_session`: `HttpOnly; Secure; SameSite=Lax`),
//!   valid for `session_ttl_s` — enforced by the server, not the browser.
//! * `POST /auth/logout` — revokes the session id and clears the cookie
//!   (`GET` shows a button: a cross-site link can't log anyone out).

use super::http::{query_encode, require_https, Fetch};
use super::jwt::{now_s, verifier_from_json, ClaimMap, Verifier};
use super::session::{ct_eq, random_b64, secret, sha256_b64, Session, Sessions};
use super::{Reply, Scope};
use crate::host::serving::html_escape;
use serde_json::{json, Value};
use std::sync::Arc;

pub const SESSION_COOKIE: &str = "__Host-anofox_session";
pub const STATE_COOKIE: &str = "__Host-anofox_oidc";
/// How long a login may take at the IdP.
const STATE_TTL: u64 = 600;

pub struct Oidc {
    pub verifier: Verifier,
    pub map: ClaimMap,
    client_id: String,
    client_secret: Option<String>,
    redirect_url: String,
    scopes: String,
    pub session_ttl: u64,
    pub sessions: Sessions,
    post_logout_url: Option<String>,
}

impl std::fmt::Debug for Oidc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Oidc")
            .field("issuer", &self.verifier.provider.issuer)
            .field("client_id", &self.client_id)
            .field("redirect_url", &self.redirect_url)
            .finish_non_exhaustive()
    }
}

/// `redirect_url`: HTTPS, or plain HTTP on a loopback host (local use); its
/// path must be `/auth/callback`.
fn check_redirect(u: &str) -> Result<(), String> {
    let ok_scheme = require_https(u, "auth.redirect_url").is_ok()
        || ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
            .iter()
            .any(|p| u.starts_with(p));
    let path_ok = u
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .is_some_and(|(_, p)| p == "auth/callback");
    if !ok_scheme || !path_ok || !u.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(format!(
            "auth.redirect_url must be https://<this server>/auth/callback (http only on a loopback host); got '{u}'"
        ));
    }
    Ok(())
}

impl Oidc {
    pub fn from_json(
        o: &serde_json::Map<String, Value>,
        env: &dyn Fn(&str) -> Option<String>,
        fetch: Arc<dyn Fetch>,
    ) -> Result<Oidc, String> {
        for k in o.keys() {
            if !matches!(
                k.as_str(),
                "mode"
                    | "issuer"
                    | "client_id"
                    | "client_secret"
                    | "client_secret_env"
                    | "redirect_url"
                    | "scopes"
                    | "user_claim"
                    | "groups_claim"
                    | "claims"
                    | "algorithms"
                    | "leeway_s"
                    | "max_age_s"
                    | "jwks_url"
                    | "jwks_ttl_s"
                    | "session_ttl_s"
                    | "session_secret"
                    | "session_secret_env"
                    | "post_logout_url"
                    | "revocation_file"
                    | "ca_file"
            ) {
                return Err(format!("auth (oidc): unknown key '{k}'"));
            }
        }
        let client_id = o
            .get("client_id")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty() && c.len() <= 256 && c.bytes().all(|b| b.is_ascii_graphic()))
            .ok_or("auth.client_id is required")?
            .to_string();
        // A client secret may be shorter than our own secrets: it is the IdP's.
        let client_secret = match (o.get("client_secret"), o.get("client_secret_env")) {
            (None, None) => None,
            (Some(s), None) => Some(
                s.as_str()
                    .ok_or("auth.client_secret must be a string")?
                    .to_string(),
            ),
            (None, Some(e)) => {
                let n = e
                    .as_str()
                    .ok_or("auth.client_secret_env must be a string")?;
                Some(env(n).ok_or_else(|| {
                    format!("auth.client_secret_env: environment variable '{n}' is not set")
                })?)
            }
            _ => return Err("auth: give client_secret or client_secret_env, not both".into()),
        };
        let redirect_url = o
            .get("redirect_url")
            .and_then(Value::as_str)
            .ok_or("auth.redirect_url is required (https://<this server>/auth/callback)")?
            .to_string();
        check_redirect(&redirect_url)?;
        let scopes = match o.get("scopes") {
            None => "openid email profile".to_string(),
            Some(v) => {
                let s: Vec<&str> = v
                    .as_array()
                    .ok_or("auth.scopes must be a list")?
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic()))
                            .ok_or("auth.scopes: bad scope")
                    })
                    .collect::<Result<_, _>>()?;
                if !s.contains(&"openid") {
                    return Err("auth.scopes must include openid".into());
                }
                s.join(" ")
            }
        };
        let session_ttl = match o.get("session_ttl_s") {
            None => 8 * 3600,
            Some(v) => v
                .as_u64()
                .filter(|n| (60..=7 * 86400).contains(n))
                .ok_or("auth.session_ttl_s must be 60..=604800")?,
        };
        let post_logout_url = match o.get("post_logout_url") {
            None => None,
            Some(v) => {
                let u = v.as_str().ok_or("auth.post_logout_url must be a URL")?;
                if !(u.starts_with('/') && !u.starts_with("//")) {
                    require_https(u, "auth.post_logout_url")?;
                }
                Some(u.to_string())
            }
        };
        let session_secret = secret(o, "session_secret", env, false)?;
        let mut sessions = Sessions::new(session_secret.as_deref(), "login", session_ttl);
        if let Some(f) = o.get("revocation_file") {
            let f = f
                .as_str()
                .filter(|f| !f.trim().is_empty())
                .ok_or("auth.revocation_file must be a file path")?;
            sessions = sessions.with_revocation_file(f.into())?;
        }
        Ok(Oidc {
            verifier: verifier_from_json(o, vec![client_id.clone()], fetch)?,
            map: ClaimMap::from_json(o, "email")?,
            client_id,
            client_secret,
            redirect_url,
            scopes,
            session_ttl,
            sessions,
            post_logout_url,
        })
    }

    /// `GET /auth/login`: to the IdP, with state/nonce/PKCE in a signed cookie.
    pub fn login(&self, next: &str) -> Reply {
        let disc = match self.verifier.provider.discover() {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "[anofox-serve] OIDC discovery: {}",
                    crate::host::access::diag(&e)
                );
                return Reply::text(
                    503,
                    "the identity provider is not reachable; try again later",
                );
            }
        };
        let Some(auth_ep) = disc.authorization_endpoint else {
            return Reply::text(503, "the identity provider has no authorization endpoint");
        };
        let (state, nonce, verifier) = (random_b64(24), random_b64(24), random_b64(32));
        let cookie = self.sessions.sign_raw(
            "oidc-state",
            now_s() + STATE_TTL,
            json!({"s": state, "n": nonce, "v": verifier, "next": safe_next(next)}),
        );
        let sep = if auth_ep.contains('?') { '&' } else { '?' };
        let url = format!(
            "{auth_ep}{sep}response_type=code&client_id={}&redirect_uri={}&scope={}&state={state}&nonce={nonce}&code_challenge={}&code_challenge_method=S256",
            query_encode(&self.client_id),
            query_encode(&self.redirect_url),
            query_encode(&self.scopes),
            sha256_b64(&verifier),
        );
        Reply::redirect(302, &url).cookie(set_cookie(STATE_COOKIE, &cookie, STATE_TTL))
    }

    /// `GET /auth/callback`: the session (and where to go), or the reason the
    /// login failed.
    pub fn callback(
        &self,
        code: Option<&str>,
        state: Option<&str>,
        state_cookie: Option<&str>,
    ) -> Result<(String, Session, String), &'static str> {
        let st = self
            .sessions
            .verify_raw("oidc-state", state_cookie.ok_or("no_state")?)
            .map_err(|_| "bad_state")?;
        let want = st["s"].as_str().ok_or("bad_state")?;
        if !ct_eq(state.ok_or("bad_state")?, want) {
            return Err("bad_state");
        }
        let code = code
            .filter(|c| !c.is_empty() && c.len() <= 4096)
            .ok_or("no_code")?;
        let verifier = st["v"].as_str().ok_or("bad_state")?;
        let disc = self
            .verifier
            .provider
            .discover()
            .map_err(|_| "idp_unreachable")?;
        let token_ep = disc.token_endpoint.ok_or("idp_unreachable")?;
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.redirect_url.as_str()),
            ("client_id", self.client_id.as_str()),
            ("code_verifier", verifier),
        ];
        let basic = self
            .client_secret
            .as_deref()
            .map(|s| (self.client_id.as_str(), s));
        let resp = self
            .verifier
            .provider
            .fetch
            .post_form(&token_ep, &form, basic)
            .map_err(|e| {
                eprintln!(
                    "[anofox-serve] OIDC token exchange: {}",
                    crate::host::access::diag(&e)
                );
                "token_exchange"
            })?;
        let id_token = resp
            .get("id_token")
            .and_then(Value::as_str)
            .ok_or("no_id_token")?;
        let p = self.verifier.verify(id_token)?;
        // Several audiences: the token must have been issued to us (azp).
        if p["aud"].as_array().is_some_and(|a| a.len() > 1)
            && p.get("azp").and_then(Value::as_str) != Some(self.client_id.as_str())
        {
            return Err("bad_audience");
        }
        let nonce = st["n"].as_str().ok_or("bad_state")?;
        if !p
            .get("nonce")
            .and_then(Value::as_str)
            .is_some_and(|n| ct_eq(n, nonce))
        {
            return Err("bad_nonce");
        }
        let claims = self.map.claims(&p).map_err(|e| e.reason())?;
        let (token, sess) =
            self.sessions
                .issue("login", self.session_ttl, &claims, &Scope::default());
        if token.len() > super::session::MAX_SESSION_LEN {
            return Err("session_too_large");
        }
        let next = safe_next(st["next"].as_str().unwrap_or("/"));
        Ok((token, sess, next))
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// After a logout: the configured page, or our own.
    pub fn after_logout(&self) -> String {
        self.post_logout_url
            .clone()
            .unwrap_or_else(|| "/auth/logged-out".into())
    }
}

/// A local path to return to after the login: `/…` only (never `//host`,
/// `/\host` or a scheme), else `/`.
pub fn safe_next(next: &str) -> String {
    let ok = next.starts_with('/')
        && !next.starts_with("//")
        && !next.starts_with("/\\")
        && next.len() <= 512
        && next.bytes().all(|b| b.is_ascii_graphic());
    if ok {
        next.to_string()
    } else {
        "/".into()
    }
}

/// A `__Host-` cookie: `Secure`, `HttpOnly`, `Path=/`, `SameSite=Lax`.
pub fn set_cookie(name: &str, value: &str, max_age: u64) -> String {
    format!("{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; Secure; SameSite=Lax")
}

/// Clear a cookie set by [`set_cookie`].
pub fn clear_cookie(name: &str) -> String {
    format!("{name}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax")
}

/// The small pages of the login flow.
pub fn page(title: &str, body: &str) -> Reply {
    Reply::html(
        200,
        format!(
            "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{t}</title>\
<body style=\"font:15px system-ui;padding:2rem;max-width:40rem\"><h1>{t}</h1>{body}",
            t = html_escape(title)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::super::jwt::testkit::*;
    use super::*;

    fn oidc(f: Arc<FakeIdp>) -> Oidc {
        let o = json!({"mode": "oidc", "issuer": ISS, "client_id": "dash",
            "client_secret": "s3cret", "redirect_url": "https://dash.test/auth/callback",
            "groups_claim": "groups"});
        Oidc::from_json(o.as_object().unwrap(), &|_| None, f).unwrap()
    }

    /// Run /auth/login and pull state, nonce, challenge and the cookie out.
    fn start(o: &Oidc, next: &str) -> (String, String, String, String) {
        let r = o.login(next);
        assert_eq!(r.status, 302);
        let loc = r.header_value("Location").unwrap().to_string();
        assert!(loc.starts_with(&format!(
            "{ISS}/authorize?response_type=code&client_id=dash"
        )));
        let q = crate::host::serving::parse_query(&loc);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "https://dash.test/auth/callback");
        let c = r.header_value("Set-Cookie").unwrap();
        assert!(c.starts_with(STATE_COOKIE) && c.contains("HttpOnly") && c.contains("Secure"));
        let cookie = c
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1
            .to_string();
        (
            q["state"].clone(),
            q["nonce"].clone(),
            q["code_challenge"].clone(),
            cookie,
        )
    }

    fn id_token(k: &TestKey, nonce: &str, extra: Value) -> String {
        let now = now_s();
        let mut p = json!({"iss": ISS, "aud": "dash", "sub": "u1", "email": "alice@example.com", "email_verified": true,
            "groups": ["sales"], "nonce": nonce, "iat": now, "exp": now + 300});
        for (a, b) in extra.as_object().unwrap() {
            p[a] = b.clone();
        }
        sign(k, Some("k1"), &p)
    }

    #[test]
    fn code_flow_with_pkce_state_and_nonce() {
        let k = es256("k1");
        let f = idp(&[&k]);
        let o = oidc(f.clone());
        let (state, nonce, challenge, cookie) = start(&o, "/d/sales?v.x=1");
        f.set(
            &format!("{ISS}/token"),
            json!({"id_token": id_token(&k, &nonce, json!({}))}),
        );
        let (tok, sess, next) = o
            .callback(Some("c0de"), Some(&state), Some(&cookie))
            .unwrap();
        assert_eq!(next, "/d/sales?v.x=1");
        assert_eq!(sess.claims.user, "alice@example.com");
        assert_eq!(sess.claims.groups, vec!["sales".to_string()]);
        assert_eq!(o.sessions.verify("login", &tok).unwrap().sid, sess.sid);
        // the token request carried the PKCE verifier matching the challenge
        let posts = f.posts.lock().unwrap();
        let form: std::collections::HashMap<_, _> = posts[0].1.iter().cloned().collect();
        assert_eq!(sha256_b64(&form["code_verifier"]), challenge);
        assert_eq!(form["code"], "c0de");
        assert_eq!(form["grant_type"], "authorization_code");
    }

    #[test]
    fn callback_attacks_are_refused() {
        let k = es256("k1");
        let f = idp(&[&k]);
        let o = oidc(f.clone());
        let (state, nonce, _, cookie) = start(&o, "/");
        f.set(
            &format!("{ISS}/token"),
            json!({"id_token": id_token(&k, &nonce, json!({}))}),
        );
        // CSRF / login fixation: wrong or missing state, no state cookie, a
        // state cookie from another server
        assert_eq!(
            o.callback(Some("c"), Some("x"), Some(&cookie)).err(),
            Some("bad_state")
        );
        assert_eq!(
            o.callback(Some("c"), None, Some(&cookie)).err(),
            Some("bad_state")
        );
        assert_eq!(
            o.callback(Some("c"), Some(&state), None).err(),
            Some("no_state")
        );
        let other = oidc(f.clone());
        let (s2, _, _, c2) = start(&other, "/");
        assert_eq!(
            o.callback(Some("c"), Some(&s2), Some(&c2)).err(),
            Some("bad_state")
        );
        // a replayed ID token from another login (nonce mismatch)
        f.set(
            &format!("{ISS}/token"),
            json!({"id_token": id_token(&k, "other-nonce", json!({}))}),
        );
        assert_eq!(
            o.callback(Some("c"), Some(&state), Some(&cookie)).err(),
            Some("bad_nonce")
        );
        // an ID token for another client, or shared without azp
        f.set(
            &format!("{ISS}/token"),
            json!({"id_token": id_token(&k, &nonce, json!({"aud": "other"}))}),
        );
        assert_eq!(
            o.callback(Some("c"), Some(&state), Some(&cookie)).err(),
            Some("bad_audience")
        );
        f.set(
            &format!("{ISS}/token"),
            json!({"id_token": id_token(&k, &nonce, json!({"aud": ["dash", "other"]}))}),
        );
        assert_eq!(
            o.callback(Some("c"), Some(&state), Some(&cookie)).err(),
            Some("bad_audience")
        );
        // expired ID token; one signed by a key the IdP doesn't publish
        f.set(
            &format!("{ISS}/token"),
            json!({"id_token": id_token(&k, &nonce, json!({"exp": now_s() - 3600}))}),
        );
        assert_eq!(
            o.callback(Some("c"), Some(&state), Some(&cookie)).err(),
            Some("expired")
        );
        let rogue = es256("k1");
        f.set(
            &format!("{ISS}/token"),
            json!({"id_token": id_token(&rogue, &nonce, json!({}))}),
        );
        assert_eq!(
            o.callback(Some("c"), Some(&state), Some(&cookie)).err(),
            Some("bad_signature")
        );
        // #115: an unverified (or unstated) e-mail is not an identity
        for ev in [json!(false), json!(null)] {
            f.set(
                &format!("{ISS}/token"),
                json!({"id_token": id_token(&k, &nonce, json!({"email_verified": ev}))}),
            );
            assert_eq!(
                o.callback(Some("c"), Some(&state), Some(&cookie)).err(),
                Some("email_unverified")
            );
        }
        f.set(&format!("{ISS}/token"), json!({"access_token": "x"}));
        assert_eq!(
            o.callback(Some("c"), Some(&state), Some(&cookie)).err(),
            Some("no_id_token")
        );
    }

    #[test]
    fn open_redirects_and_bad_config_are_refused() {
        for (n, want) in [
            ("/d/x", "/d/x"),
            ("//evil.test/x", "/"),
            ("/\\evil.test", "/"),
            ("https://evil.test", "/"),
            ("javascript:alert(1)", "/"),
            ("/a\nb", "/"),
        ] {
            assert_eq!(safe_next(n), want, "{n}");
        }
        let f = idp(&[]);
        let base = json!({"mode": "oidc", "issuer": ISS, "client_id": "dash",
            "redirect_url": "https://dash.test/auth/callback"});
        assert!(Oidc::from_json(base.as_object().unwrap(), &|_| None, f.clone()).is_ok());
        for (k, v) in [
            ("redirect_url", json!("http://dash.test/auth/callback")),
            ("redirect_url", json!("https://dash.test/elsewhere")),
            ("issuer", json!("http://idp.test")),
            ("scopes", json!(["email"])),
            ("session_ttl_s", json!(10)),
            ("algorithms", json!(["HS256"])),
            ("nope", json!(1)),
        ] {
            let mut o = base.clone();
            o[k] = v;
            assert!(
                Oidc::from_json(o.as_object().unwrap(), &|_| None, f.clone()).is_err(),
                "{k}"
            );
        }
        let mut o = base.clone();
        o["redirect_url"] = json!("http://127.0.0.1:8950/auth/callback");
        assert!(Oidc::from_json(o.as_object().unwrap(), &|_| None, f).is_ok());
    }
}

//! Built-in authentication for the dashboard servers (access control phase 3,
//! issue #50), next to the auth proxy of phase 1 ([`crate::host::access`]).
//! `options.auth` (`--auth`) is one method object or a list of them:
//!
//! | `mode` | Credential | Module |
//! |---|---|---|
//! | `proxy` | identity headers + the proxy's shared secret | [`access::ProxyAuth`] |
//! | `jwt` | `Authorization: Bearer <JWT>` (or a cookie) from an IdP | [`jwt`] |
//! | `oidc` | a login at the IdP → a server-signed session cookie | [`oidc`] |
//! | `embed` | a signed link `/d/<id>?embed=<token>` → a page session | [`embed`] |
//! | `api_keys` | `Authorization: Bearer afx_…` | [`apikey`] |
//!
//! Every method ends in the same [`Claims`] as the proxy, so the reserved
//! `__claim_*` variables, the order they are set in, the cache keys, policies
//! and the request log work the same whichever way a viewer signed in. A
//! method may also narrow the [`Scope`] — the dashboards a credential may read
//! and variables it locks (embed links, API keys).
//!
//! **Which credential counts.** Exactly one is used per request, the first
//! present of: an embed link (on `GET /d/<id>`), the proxy secret, an
//! `Authorization` header, an embed page session (`X-Anofox-Session`), the
//! login cookie, a JWT cookie. A present but invalid credential is refused —
//! the server never falls back to the next one. With nothing present the
//! request is refused (a page view is sent to `/auth/login` when `oidc` is
//! configured). A configuration error fails the server start.
//!
//! [`audit`] records each authentication event and admin action.

pub mod apikey;
pub mod audit;
pub mod embed;
pub mod http;
pub mod jwt;
pub mod oidc;
pub mod session;

use crate::host::access::{self, Claims, ProxyAuth};
use audit::{AuditLog, Event};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::Arc;

/// The header an embedded page sends its page session in.
pub const SESSION_HEADER: &str = "X-Anofox-Session";

/// What a credential may reach besides the claims.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// `None`: every dashboard.
    pub dashboards: Option<Vec<String>>,
    /// Variables the credential fixes, as `(name, typed SQL literal)`: set
    /// after (and instead of) the viewer's own.
    pub vars: Vec<(String, String)>,
}

/// May a credential lock variable `name`? An identifier that is not a
/// server-set variable: no `__claim_*`, no `anofox_data`, and nothing in the
/// `__anofox` namespace (policy views, #49), in any case.
pub fn lockable_var(name: &str) -> bool {
    crate::host::serving::valid_ident(name)
        && !access::is_reserved_var(name)
        && !name
            .to_ascii_lowercase()
            .starts_with(crate::host::policy::RESERVED_PREFIX)
}

impl Scope {
    pub fn allows(&self, dashboard: &str) -> bool {
        self.dashboards
            .as_ref()
            .is_none_or(|d| d.iter().any(|x| x == dashboard))
    }

    /// Drop the viewer's values of the locked variables and append the locked
    /// ones (DuckDB names are case-insensitive).
    pub fn lock_vars(&self, vars: &mut Vec<(String, String)>) {
        if self.vars.is_empty() {
            return;
        }
        vars.retain(|(k, _)| !self.vars.iter().any(|(l, _)| l.eq_ignore_ascii_case(k)));
        vars.extend(self.vars.iter().cloned());
    }
}

/// A signed-in viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub claims: Claims,
    /// `proxy`, `jwt`, `oidc`, `embed` or `api_key`.
    pub method: &'static str,
    pub scope: Scope,
    /// An embed link was just exchanged: the page session for the page.
    pub page_session: Option<String>,
    /// The API key id or session id (public; for the audit log).
    pub ref_id: Option<String>,
}

/// Why a request was refused (`401`; the reason goes to the logs only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Denied {
    pub reason: &'static str,
    pub method: Option<&'static str>,
    /// Already written to the audit log (with more detail).
    pub audited: bool,
}

impl Denied {
    /// The refusal is the server's (its revocation file can't be trusted
    /// right now): `503` with `Retry-After`, no login redirect.
    pub fn unavailable(&self) -> Option<Reply> {
        (self.reason == "revocations_unavailable").then(|| {
            Reply::text(
                503,
                "service unavailable: sign-in state can't be checked right now",
            )
            .header("Retry-After", "10")
        })
    }
}

/// One request, as authentication sees it.
#[derive(Debug, Clone, Copy)]
pub struct Req<'a> {
    pub method: &'a str,
    /// Path and query string.
    pub url: &'a str,
    pub peer: Option<IpAddr>,
    /// Every header, in order, duplicates included.
    pub headers: &'a [(&'a str, &'a str)],
}

impl<'a> Req<'a> {
    pub fn path(&self) -> &'a str {
        self.url.split('?').next().unwrap_or("/")
    }

    fn all(&self, name: &str) -> Vec<&'a str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
            .collect()
    }

    /// Every value of cookie `name` across all `Cookie` headers.
    fn cookies(&self, name: &str) -> Vec<&'a str> {
        self.all("Cookie")
            .into_iter()
            .flat_map(|h| h.split(';'))
            .filter_map(|kv| {
                let (k, v) = kv.trim().split_once('=')?;
                (k == name).then_some(v.trim())
            })
            .collect()
    }

    /// Every value of query parameter `name`.
    fn query_all(&self, name: &str) -> Vec<String> {
        let Some((_, q)) = self.url.split_once('?') else {
            return Vec::new();
        };
        q.split('&')
            .filter_map(|p| p.split_once('=').or(Some((p, ""))))
            .filter(|(k, _)| crate::host::serving::url_decode(k) == name)
            .map(|(_, v)| crate::host::serving::url_decode(v))
            .collect()
    }
}

/// An HTTP answer produced by the login itself (redirects, login pages).
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn text(status: u16, msg: &str) -> Reply {
        Reply {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: msg.as_bytes().to_vec(),
        }
    }
    pub fn html(status: u16, body: String) -> Reply {
        Reply {
            status,
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            body: body.into_bytes(),
        }
    }
    pub fn redirect(status: u16, location: &str) -> Reply {
        Reply::text(status, "").header("Location", location)
    }
    pub fn header(mut self, k: &str, v: &str) -> Reply {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
    pub fn cookie(self, c: String) -> Reply {
        self.header("Set-Cookie", &c)
    }
    /// The first value of header `k`.
    pub fn header_value(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

/// `jwt` mode: bearer tokens from an IdP.
pub struct JwtAuth {
    pub verifier: jwt::Verifier,
    pub map: jwt::ClaimMap,
    /// Also read the token from this cookie.
    pub cookie: Option<String>,
    /// Accept OIDC ID tokens as bearer tokens (`accept_id_tokens`; off: an
    /// ID token is a login receipt for a client, not an API credential).
    pub accept_id_tokens: bool,
}

/// Does a verified token look like an OIDC ID token? Not when its header
/// says `at+jwt` (an RFC 9068 access token); otherwise yes with `nonce`,
/// `at_hash`, `c_hash`, Cognito's `token_use: "id"` or Keycloak's `typ: "ID"`.
fn is_id_token(header_typ: Option<&str>, p: &Value) -> bool {
    if header_typ.is_some_and(|t| {
        t.eq_ignore_ascii_case("at+jwt") || t.eq_ignore_ascii_case("application/at+jwt")
    }) {
        return false;
    }
    let field = |k: &str| p.get(k).and_then(Value::as_str);
    ["nonce", "at_hash", "c_hash"]
        .iter()
        .any(|k| p.get(*k).is_some())
        || field("token_use") == Some("id")
        || field("typ").is_some_and(|t| t.eq_ignore_ascii_case("id"))
}

/// Refuse a configuration in which one secret plays two roles (#114): the
/// host application holding the embed link secret must not be able to sign
/// login sessions, page sessions or proxy requests, and so on. Only the two
/// session secrets (OIDC and embed) may be the same value — their keys are
/// derived per use.
fn distinct_secrets(items: &[&Value], env: &dyn Fn(&str) -> Option<String>) -> Result<(), String> {
    let mut seen: Vec<(String, String)> = Vec::new();
    for it in items {
        let Some(o) = it.as_object() else { continue };
        let mode = o.get("mode").and_then(Value::as_str).unwrap_or("");
        let keys: &[&str] = match mode {
            "proxy" => &["shared_secret"],
            "embed" => &["secret", "session_secret"],
            "oidc" => &["session_secret"],
            _ => &[],
        };
        for k in keys {
            // Malformed values are reported by the method's own parser.
            if let Ok(Some(v)) = session::secret(o, k, env, false) {
                seen.push((format!("{mode}.{k}"), v));
            }
        }
    }
    for (i, (a, va)) in seen.iter().enumerate() {
        for (b, vb) in &seen[i + 1..] {
            let sessions = a.ends_with("session_secret") && b.ends_with("session_secret");
            if !sessions && session::ct_eq(va, vb) {
                return Err(format!(
                    "auth: {a} and {b} must be different secrets (one party could otherwise sign for the other)"
                ));
            }
        }
    }
    Ok(())
}

/// The configured login methods.
#[derive(Default)]
pub struct Auth {
    pub proxy: Option<ProxyAuth>,
    pub jwt: Option<JwtAuth>,
    pub oidc: Option<oidc::Oidc>,
    pub embed: Option<embed::Embed>,
    pub keys: Option<apikey::ApiKeys>,
    pub audit: Option<Arc<AuditLog>>,
    /// When each credential's last successful use was audited (sampling).
    audited_ok: std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

/// A credential's successful uses are audited at most this often.
pub const AUDIT_OK_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth")
            .field("methods", &self.methods())
            .finish_non_exhaustive()
    }
}

/// How a method's HTTP client is made (tests pass a fake IdP).
pub type MakeFetch<'a> =
    &'a dyn Fn(Option<&std::path::Path>) -> Result<Arc<dyn http::Fetch>, String>;

fn https_fetch(ca: Option<&std::path::Path>) -> Result<Arc<dyn http::Fetch>, String> {
    Ok(Arc::new(http::Https::new(ca)?))
}

impl Auth {
    /// Parse `options.auth` / `--auth`: one method object, or a list of up to
    /// five with different `mode`s. `env` resolves `*_env` settings.
    pub fn from_json(v: &Value, env: impl Fn(&str) -> Option<String>) -> Result<Auth, String> {
        Auth::from_json_with(v, &env, &https_fetch)
    }

    pub fn from_json_with(
        v: &Value,
        env: &dyn Fn(&str) -> Option<String>,
        make_fetch: MakeFetch,
    ) -> Result<Auth, String> {
        let items: Vec<&Value> = match v {
            Value::Array(a) if !a.is_empty() && a.len() <= 5 => a.iter().collect(),
            Value::Array(_) => return Err("auth: a list of 1 to 5 login methods".into()),
            o => vec![o],
        };
        distinct_secrets(&items, env)?;
        let mut a = Auth::default();
        for it in items {
            let o = it
                .as_object()
                .ok_or("auth must be a JSON object (or a list of them)")?;
            let mode = o.get("mode").and_then(Value::as_str).unwrap_or("");
            let dup = || format!("auth: mode \"{mode}\" is configured twice");
            let ca =
                |o: &serde_json::Map<String, Value>| -> Result<Option<std::path::PathBuf>, String> {
                    match o.get("ca_file") {
                        None => Ok(None),
                        Some(v) => Ok(Some(
                            v.as_str().ok_or("auth.ca_file must be a path")?.into(),
                        )),
                    }
                };
            match mode {
                "proxy" => {
                    if a.proxy.is_some() {
                        return Err(dup());
                    }
                    a.proxy = Some(ProxyAuth::from_json(it, env)?);
                }
                "jwt" => {
                    if a.jwt.is_some() {
                        return Err(dup());
                    }
                    for k in o.keys() {
                        if !matches!(
                            k.as_str(),
                            "mode"
                                | "issuer"
                                | "audience"
                                | "jwks_url"
                                | "jwks_ttl_s"
                                | "algorithms"
                                | "leeway_s"
                                | "max_age_s"
                                | "user_claim"
                                | "groups_claim"
                                | "claims"
                                | "cookie"
                                | "ca_file"
                                | "accept_id_tokens"
                        ) {
                            return Err(format!("auth (jwt): unknown key '{k}' (issuer, audience, jwks_url, jwks_ttl_s, algorithms, leeway_s, max_age_s, user_claim, groups_claim, claims, cookie, ca_file)"));
                        }
                    }
                    let audiences: Vec<String> = match o.get("audience") {
                        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
                        Some(Value::Array(l)) if !l.is_empty() => l
                            .iter()
                            .map(|x| x.as_str().filter(|s| !s.is_empty()).map(str::to_string))
                            .collect::<Option<_>>()
                            .ok_or("auth.audience must be strings")?,
                        _ => {
                            return Err(
                                "auth.audience is required (this server's audience at the IdP)"
                                    .into(),
                            )
                        }
                    };
                    let cookie = match o.get("cookie") {
                        None => None,
                        Some(c) => Some(
                            c.as_str()
                                .filter(|c| {
                                    !c.is_empty()
                                        && c.len() <= 64
                                        && c.bytes().all(|b| {
                                            b.is_ascii_alphanumeric() || b"_-".contains(&b)
                                        })
                                })
                                .ok_or("auth.cookie must be a cookie name")?
                                .to_string(),
                        ),
                    };
                    let fetch = make_fetch(ca(o)?.as_deref())?;
                    a.jwt = Some(JwtAuth {
                        verifier: jwt::verifier_from_json(o, audiences, fetch)?,
                        map: jwt::ClaimMap::from_json(o, "sub")?,
                        cookie,
                        accept_id_tokens: match o.get("accept_id_tokens") {
                            None => false,
                            Some(v) => v
                                .as_bool()
                                .ok_or("auth.accept_id_tokens must be a boolean")?,
                        },
                    });
                }
                "oidc" => {
                    if a.oidc.is_some() {
                        return Err(dup());
                    }
                    let fetch = make_fetch(ca(o)?.as_deref())?;
                    a.oidc = Some(oidc::Oidc::from_json(o, env, fetch)?);
                }
                "embed" => {
                    if a.embed.is_some() {
                        return Err(dup());
                    }
                    a.embed = Some(embed::Embed::from_json(o, env)?);
                }
                "api_keys" => {
                    if a.keys.is_some() {
                        return Err(dup());
                    }
                    a.keys = Some(apikey::ApiKeys::from_json(o)?);
                }
                _ => {
                    return Err(
                        "auth.mode must be \"proxy\", \"jwt\", \"oidc\", \"embed\" or \"api_keys\""
                            .into(),
                    )
                }
            }
        }
        // The OIDC client's ID tokens must not double as API bearer tokens.
        if let (Some(j), Some(o)) = (&a.jwt, &a.oidc) {
            if !j.accept_id_tokens && j.verifier.audiences.iter().any(|x| x == o.client_id()) {
                return Err("auth (jwt): audience is the oidc client_id — ID tokens of the login would be accepted as API tokens; use the API's own audience (or set accept_id_tokens)".into());
            }
        }
        Ok(a)
    }

    /// The configured methods, in a fixed order.
    pub fn methods(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if self.proxy.is_some() {
            m.push("proxy");
        }
        if self.jwt.is_some() {
            m.push("jwt");
        }
        if self.oidc.is_some() {
            m.push("oidc");
        }
        if self.embed.is_some() {
            m.push("embed");
        }
        if self.keys.is_some() {
            m.push("api_keys");
        }
        m
    }

    /// The start message's description of the login.
    pub fn describe(&self) -> String {
        let mut out = Vec::new();
        if let Some(p) = &self.proxy {
            out.push(format!(
                "auth proxy (trusted identity headers: {}; secret in {}) — the proxy must set or remove each of them on every request",
                p.trusted_headers(),
                p.secret_header
            ));
        }
        if let Some(j) = &self.jwt {
            out.push(format!(
                "JWT bearer tokens from {}",
                j.verifier.provider.issuer
            ));
        }
        if let Some(o) = &self.oidc {
            out.push(format!(
                "OIDC login at {}{}",
                o.verifier.provider.issuer,
                if o.sessions.persistent {
                    ""
                } else {
                    " (sessions end when the server restarts: set session_secret_env to keep them, and on every replica)"
                }
            ));
        }
        if let Some(e) = &self.embed {
            out.push(format!(
                "signed embed links (≤ {} s, page sessions {} s)",
                e.max_link_ttl, e.session_ttl
            ));
        }
        if let Some(k) = &self.keys {
            out.push(format!("API keys from {}", k.file.display()));
        }
        out.join("; ")
    }

    /// Set the audit log (and record the start).
    pub fn set_audit(&mut self, log: Arc<AuditLog>) {
        log.write(&Event {
            event: "start",
            outcome: "ok",
            detail: Some(&self.methods().join(",")),
            ..Default::default()
        });
        if let Some(o) = &self.oidc {
            o.sessions.set_audit(log.clone());
        }
        self.audit = Some(log);
    }

    fn audit(&self, e: Event) {
        if let Some(a) = &self.audit {
            a.write(&e);
        }
    }

    /// Fetch the identity providers' keys now, so a misconfiguration shows at
    /// startup (requests still fail closed if this fails; it is retried).
    pub fn prefetch(&self) -> Vec<String> {
        let mut warn = Vec::new();
        if let Some(j) = &self.jwt {
            if let Err(e) = j.verifier.jwks.prefetch() {
                warn.push(format!("JWKS: {e}"));
            }
        }
        if let Some(o) = &self.oidc {
            if let Err(e) = o.verifier.jwks.prefetch() {
                warn.push(format!("OIDC JWKS: {e}"));
            }
        }
        warn
    }

    /// The `/auth/*` endpoints of the OIDC login (`None`: not one of them).
    /// Runs before authentication — after the Host and Origin checks.
    pub fn route(&self, r: &Req) -> Option<Reply> {
        let path = r.path();
        if !path.starts_with("/auth/") {
            return None;
        }
        let o = self.oidc.as_ref()?;
        let one = |k: &str| -> Option<String> {
            let v = r.query_all(k);
            (v.len() == 1).then(|| v[0].clone())
        };
        Some(match (r.method, path) {
            ("GET", "/auth/login") => o.login(&one("next").unwrap_or_else(|| "/".into())),
            ("GET", "/auth/callback") => {
                let cookies = r.cookies(oidc::STATE_COOKIE);
                let state_cookie = (cookies.len() == 1).then(|| cookies[0]);
                let res = if one("error").is_some() {
                    Err("idp_error")
                } else {
                    o.callback(one("code").as_deref(), one("state").as_deref(), state_cookie)
                };
                match res {
                    Ok((token, sess, next)) => {
                        self.audit(Event {
                            event: "login",
                            outcome: "ok",
                            method: Some("oidc"),
                            user: Some(&sess.claims.user),
                            sid: Some(&sess.sid),
                            peer: r.peer,
                            ..Default::default()
                        });
                        Reply::redirect(303, &next)
                            .cookie(oidc::clear_cookie(oidc::STATE_COOKIE))
                            .cookie(oidc::set_cookie(oidc::SESSION_COOKIE, &token, o.session_ttl))
                    }
                    Err(reason) => {
                        self.audit(Event {
                            event: "login",
                            outcome: "deny",
                            method: Some("oidc"),
                            reason: Some(reason),
                            peer: r.peer,
                            ..Default::default()
                        });
                        let mut p = oidc::page(
                            "Sign-in failed",
                            "<p>The sign-in could not be completed. <a href=\"/auth/login\">Try again</a>.</p>",
                        );
                        p.status = 401;
                        p.cookie(oidc::clear_cookie(oidc::STATE_COOKIE))
                    }
                }
            }
            ("GET", "/auth/logout") => oidc::page(
                "Sign out",
                "<form method=\"post\" action=\"/auth/logout\"><button type=\"submit\">Sign out</button></form>",
            ),
            ("POST", "/auth/logout") => {
                for c in r.cookies(oidc::SESSION_COOKIE) {
                    if let Ok(s) = o.sessions.verify("login", c) {
                        o.sessions.revoke(&s.sid, s.exp);
                        self.audit(Event {
                            event: "logout",
                            outcome: "ok",
                            method: Some("oidc"),
                            user: Some(&s.claims.user),
                            sid: Some(&s.sid),
                            peer: r.peer,
                            ..Default::default()
                        });
                    }
                }
                Reply::redirect(303, &o.after_logout()).cookie(oidc::clear_cookie(oidc::SESSION_COOKIE))
            }
            ("GET", "/auth/logged-out") => oidc::page(
                "Signed out",
                "<p>You are signed out. <a href=\"/auth/login\">Sign in again</a>.</p>",
            ),
            _ => Reply::text(404, "not found"),
        })
    }

    /// A static file of the viewer UI (code, styles, fonts — never data) that
    /// an embedded page loads without its page session: with `embed`
    /// configured, these are public. They hold nothing but the open-source UI.
    pub fn public_asset(&self, method: &str, path: &str) -> bool {
        self.embed.is_some()
            && method == "GET"
            && !path.starts_with("/d/")
            && !path.starts_with("/api/")
            && !path.starts_with("/auth/")
            && !path.contains("..")
            && [".js", ".mjs", ".css", ".wasm", ".woff2", ".woff", ".ico"]
                .iter()
                .any(|e| path.ends_with(e))
    }

    /// Send a browser that has no credential at all to the login.
    pub fn login_redirect(&self, r: &Req, d: &Denied) -> Option<Reply> {
        let path = r.path();
        (self.oidc.is_some()
            && r.method == "GET"
            // no credential, or an ended login session (not a bad link or key)
            && (d.reason == "no_credentials"
                || (d.method == Some("oidc") && matches!(d.reason, "expired" | "revoked")))
            && (path == "/" || path.starts_with("/d/")))
        .then(|| {
            Reply::redirect(
                302,
                &format!("/auth/login?next={}", http::query_encode(r.url)),
            )
            .cookie(oidc::clear_cookie(oidc::SESSION_COOKIE))
        })
    }

    /// Authenticate one request (see the module docs for which credential
    /// counts). A refusal is audited.
    pub fn authenticate(&self, r: &Req) -> Result<Identity, Denied> {
        let res = self.credential(r);
        self.audited(r, res)
    }

    /// An embed page session posted in a form (`serve --dashboards`, whose
    /// pages have no script: `POST /d/<id>` with `embed_session` in an
    /// `application/x-www-form-urlencoded` body — never in a URL).
    pub fn page_session(&self, token: Option<&str>, r: &Req) -> Result<Identity, Denied> {
        let res = match (&self.embed, token) {
            (Some(e), Some(t)) => e
                .sessions
                .verify("embed", t.trim())
                .map(|sess| Identity {
                    claims: sess.claims,
                    method: "embed",
                    scope: sess.scope,
                    page_session: None,
                    ref_id: Some(sess.sid),
                })
                .map_err(|reason| Denied {
                    reason,
                    method: Some("embed"),
                    audited: false,
                }),
            _ => Err(Denied {
                reason: "no_credentials",
                method: None,
                audited: false,
            }),
        };
        self.audited(r, res)
    }

    /// Audit the outcome of an authentication and pass it on.
    fn audited(&self, r: &Req, res: Result<Identity, Denied>) -> Result<Identity, Denied> {
        if let Ok(id) = &res {
            // Logins and embed exchanges are audited where they happen; each
            // other credential's use once per AUDIT_OK_EVERY.
            if id.page_session.is_none() && self.sample_ok(id) {
                let api = id.method == "api_key";
                self.audit(Event {
                    event: "auth",
                    outcome: "ok",
                    method: Some(id.method),
                    user: Some(&id.claims.user),
                    key_id: id.ref_id.as_deref().filter(|_| api),
                    sid: id.ref_id.as_deref().filter(|_| !api),
                    peer: r.peer,
                    path: Some(r.path()),
                    ..Default::default()
                });
            }
        }
        if let Err(d) = &res {
            // "no credential" on a page view is the normal start of a login.
            if d.reason != "no_credentials" && !d.audited {
                self.audit(Event {
                    event: "auth",
                    outcome: "deny",
                    method: d.method,
                    reason: Some(d.reason),
                    peer: r.peer,
                    path: Some(r.path()),
                    ..Default::default()
                });
            }
        }
        res
    }

    fn credential(&self, r: &Req) -> Result<Identity, Denied> {
        let deny = |method: &'static str| {
            move |reason: &'static str| Denied {
                reason,
                method: Some(method),
                audited: false,
            }
        };
        let path = r.path();
        // 1. An embed link, exchanged for a page session.
        if let (Some(e), "GET", Some(id)) = (&self.embed, r.method, path.strip_prefix("/d/")) {
            // (`?embed=1` alone is the UI's chrome-less view, not a link.)
            let links: Vec<String> = r
                .query_all("embed")
                .into_iter()
                .filter(|l| l.contains('.'))
                .collect();
            if !links.is_empty() {
                let id = id.trim_end_matches('/');
                let [link] = links.as_slice() else {
                    return Err(deny("embed")("malformed_token"));
                };
                let (claims, scope) = e.verify_link(link, id).map_err(deny("embed"))?;
                let (page, sess) = e.sessions.issue("embed", e.session_ttl, &claims, &scope);
                self.audit(Event {
                    event: "embed",
                    outcome: "ok",
                    method: Some("embed"),
                    user: Some(&claims.user),
                    dashboard: Some(id),
                    sid: Some(&sess.sid),
                    peer: r.peer,
                    ..Default::default()
                });
                return Ok(Identity {
                    claims,
                    method: "embed",
                    scope,
                    page_session: Some(page),
                    ref_id: Some(sess.sid.clone()),
                });
            }
        }
        // 2. The auth proxy (its secret header is present).
        if let Some(p) = &self.proxy {
            let only_proxy = self.methods() == ["proxy"];
            if only_proxy || !r.all(&p.secret_header).is_empty() {
                return p
                    .authenticate(r.peer, r.headers)
                    .map(|claims| Identity {
                        claims,
                        method: "proxy",
                        scope: Scope::default(),
                        page_session: None,
                        ref_id: None,
                    })
                    .map_err(|e| deny("proxy")(e.reason()));
            }
        }
        // 3. Authorization: Bearer — an API key or a JWT.
        if self.jwt.is_some() || self.keys.is_some() {
            let auth = r.all("Authorization");
            if !auth.is_empty() {
                let [h] = auth.as_slice() else {
                    return Err(deny("jwt")("malformed_token"));
                };
                let token = match h.split_once(' ') {
                    Some((s, t)) if s.eq_ignore_ascii_case("bearer") => t.trim(),
                    _ => return Err(deny("jwt")("malformed_token")),
                };
                if let (Some(k), true) = (&self.keys, token.starts_with(apikey::PREFIX)) {
                    let on_reload = |rl: &apikey::Reload| self.audit_reload(rl);
                    let (id, res) = k.authenticate(token, &on_reload);
                    return match res {
                        Ok((claims, scope)) => Ok(Identity {
                            claims,
                            method: "api_key",
                            scope,
                            page_session: None,
                            ref_id: id,
                        }),
                        Err(reason) => {
                            // The key id (public) names the key; never the key.
                            self.audit(Event {
                                event: "auth",
                                outcome: "deny",
                                method: Some("api_key"),
                                reason: Some(reason),
                                key_id: id.as_deref(),
                                peer: r.peer,
                                path: Some(path),
                                ..Default::default()
                            });
                            Err(Denied {
                                reason,
                                method: Some("api_key"),
                                audited: true,
                            })
                        }
                    };
                }
                let Some(j) = &self.jwt else {
                    return Err(deny("api_key")("malformed_key"));
                };
                return self.jwt_identity(j, token);
            }
        }
        // 4. An embedded page's session (a header only, never the URL).
        if let Some(e) = &self.embed {
            let s = r.all(SESSION_HEADER);
            if !s.is_empty() {
                let [s] = s.as_slice() else {
                    return Err(deny("embed")("malformed_token"));
                };
                let sess = e
                    .sessions
                    .verify("embed", s.trim())
                    .map_err(deny("embed"))?;
                return Ok(Identity {
                    claims: sess.claims,
                    method: "embed",
                    scope: sess.scope,
                    page_session: None,
                    ref_id: Some(sess.sid),
                });
            }
        }
        // 5. The OIDC login cookie.
        if let Some(o) = &self.oidc {
            let c = r.cookies(oidc::SESSION_COOKIE);
            if !c.is_empty() {
                // Two session cookies (cookie tossing from a sibling domain):
                // refuse rather than pick one.
                let [c] = c.as_slice() else {
                    return Err(deny("oidc")("malformed_token"));
                };
                let sess = o.sessions.verify("login", c).map_err(deny("oidc"))?;
                return Ok(Identity {
                    claims: sess.claims,
                    method: "oidc",
                    scope: Scope::default(),
                    page_session: None,
                    ref_id: Some(sess.sid),
                });
            }
        }
        // 6. A JWT in a cookie.
        if let Some(j) = &self.jwt {
            if let Some(name) = &j.cookie {
                let c = r.cookies(name);
                if !c.is_empty() {
                    let [c] = c.as_slice() else {
                        return Err(deny("jwt")("malformed_token"));
                    };
                    return self.jwt_identity(j, c);
                }
            }
        }
        Err(Denied {
            reason: "no_credentials",
            method: None,
            audited: false,
        })
    }

    /// Is this credential's success due for the audit log?
    fn sample_ok(&self, id: &Identity) -> bool {
        if self.audit.is_none() {
            return false;
        }
        let key = format!("{:?}", (id.method, &id.claims.user, &id.ref_id));
        let now = std::time::Instant::now();
        let mut m = self.audited_ok.lock().unwrap_or_else(|p| p.into_inner());
        if m.len() >= 10_000 {
            m.retain(|_, t| now.duration_since(*t) < AUDIT_OK_EVERY);
            if m.len() >= 10_000 {
                m.clear();
            }
        }
        match m.get(&key) {
            Some(t) if now.duration_since(*t) < AUDIT_OK_EVERY => false,
            _ => {
                m.insert(key, now);
                true
            }
        }
    }

    fn jwt_identity(&self, j: &JwtAuth, token: &str) -> Result<Identity, Denied> {
        let d = |reason| Denied {
            reason,
            method: Some("jwt"),
            audited: false,
        };
        let (payload, typ) = j.verifier.verify_typed(token).map_err(d)?;
        if !j.accept_id_tokens && is_id_token(typ.as_deref(), &payload) {
            return Err(d("id_token"));
        }
        let claims = j.map.claims(&payload).map_err(|e| d(e.reason()))?;
        Ok(Identity {
            claims,
            method: "jwt",
            scope: Scope::default(),
            page_session: None,
            ref_id: None,
        })
    }

    fn audit_reload(&self, r: &apikey::Reload) {
        let n;
        let (outcome, detail) = match r {
            apikey::Reload::Unchanged => return,
            apikey::Reload::Loaded(k) => {
                n = format!("{k} key(s)");
                ("ok", n.as_str())
            }
            apikey::Reload::Failed(e) => ("error", e.as_str()),
        };
        self.audit(Event {
            event: "api_keys_reloaded",
            outcome,
            detail: Some(detail),
            ..Default::default()
        });
    }
}

/// The reason strings of [`access::AuthError`] are reused by every method.
pub use access::AuthError;

#[cfg(test)]
mod tests;

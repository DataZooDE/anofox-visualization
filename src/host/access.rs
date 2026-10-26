//! Access control for the dashboard servers (phase 1, issue #48): viewer
//! identity from a trusted auth proxy, the identity as **reserved** SQL
//! variables, and a JSON-lines request log. Shared by the extension's
//! `anofox_serve_dashboards` (`options.auth`, `options.log`) and the `serve
//! --dashboards` binary (`--auth`, `--log`). Pure apart from the log writer, so
//! it is unit-tested here once. See `docs/secure-serving.md` (threat model).
//!
//! * [`ProxyAuth`] trusts identity headers (`X-Forwarded-User`, …) only from a
//!   request that carries the proxy's **shared secret** (constant-time compare)
//!   and, when `trusted_proxies` is set, comes from one of those peer
//!   addresses. Anything else — a missing/wrong secret, an untrusted peer, a
//!   missing user (or `required` claim), a repeated identity header — is
//!   refused (`401`). The secret proves the *request* passed the proxy, not
//!   that the proxy wrote every header: the proxy must set or remove each
//!   configured identity header on every request, so only the user header is
//!   on by default (groups and extra claims are opt-in).
//! * [`Claims`] become the variables `__claim_user` (VARCHAR),
//!   `__claim_groups` (VARCHAR[]) and `__claim_<name>` (VARCHAR) — set by the
//!   server after the viewer's variables. A viewer can never set a
//!   `__claim_*` variable ([`is_reserved_var`]): a panel request naming one is
//!   rejected (`400`). Panels filter with `getvariable('__claim_user')`.
//! * [`RequestLog`] writes one JSON object per request: when, who, what
//!   (dashboard, panel), status, latency, cache outcome; variable *names* only
//!   unless configured otherwise; never a header, cookie or the secret.

use crate::host::serving::{ct_eq, sql_literal, valid_ident};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;

/// Prefix of the variables the server reserves for the viewer's claims.
pub const CLAIM_PREFIX: &str = "__claim_";

/// The variable a locked server sets to its data folder (`options.data`).
pub const DATA_VAR: &str = crate::sql::requires::DATA_VAR;

/// Is `name` a variable only the server sets — a claim (`__claim_*`) or
/// `anofox_data` — in any case (DuckDB variable names are case-insensitive)?
/// Reserved whether or not auth or a data folder is configured.
pub fn is_reserved_var(name: &str) -> bool {
    (name.len() >= CLAIM_PREFIX.len()
        && name.as_bytes()[..CLAIM_PREFIX.len()].eq_ignore_ascii_case(CLAIM_PREFIX.as_bytes()))
        || name.eq_ignore_ascii_case(DATA_VAR)
}

/// Default header names (oauth2-proxy / Caddy / nginx conventions).
pub const DEFAULT_USER_HEADER: &str = "X-Forwarded-User";
pub const DEFAULT_GROUPS_HEADER: &str = "X-Forwarded-Groups";
pub const DEFAULT_SECRET_HEADER: &str = "X-Anofox-Proxy-Secret";
/// A shared secret must be at least this long (e.g. `openssl rand -hex 32`).
pub const MIN_SECRET_LEN: usize = 32;
pub const MAX_CLAIM_LEN: usize = 1024;
pub const MAX_GROUPS: usize = 256;
pub const MAX_ATTRS: usize = 16;

/// A network range (`10.0.0.0/8`, `::1/128`, or a bare address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    bits: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Result<Cidr, String> {
        let s = s.trim();
        let (a, bits) = match s.split_once('/') {
            Some((a, b)) => (a, Some(b)),
            None => (s, None),
        };
        let addr = a
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .map_err(|_| format!("'{s}' is not an IP address or CIDR range"))?;
        // Peers are compared in their IPv4 form, so a mapped range would
        // silently never match (or need a /96+ prefix): ask for IPv4 instead.
        if let IpAddr::V6(v6) = addr {
            if v6.to_ipv4_mapped().is_some() {
                return Err(format!(
                    "'{s}': write IPv4-mapped addresses as IPv4 (e.g. 10.0.0.0/8); IPv4 peers are matched in their IPv4 form"
                ));
            }
        }
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let bits = match bits {
            None => max,
            Some(b) => b
                .parse::<u8>()
                .ok()
                .filter(|b| *b <= max)
                .ok_or_else(|| format!("'{s}': bad prefix length"))?,
        };
        Ok(Cidr { addr, bits })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = normalize(ip);
        let (a, b, width) = match (self.addr, ip) {
            (IpAddr::V4(a), IpAddr::V4(b)) => (u32::from(a) as u128, u32::from(b) as u128, 32),
            (IpAddr::V6(a), IpAddr::V6(b)) => (u128::from(a), u128::from(b), 128),
            _ => return false,
        };
        let shift = width - self.bits as u32;
        shift >= width || (a >> shift) == (b >> shift)
    }
}

/// `::ffff:a.b.c.d` → `a.b.c.d`, so one IPv4 rule covers both forms.
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

/// An optional identity header (groups or an extra claim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimHeader {
    pub header: String,
    /// Absent or empty → `401` (otherwise the claim is empty / NULL).
    pub required: bool,
}

/// `auth: {"mode": "proxy", …}` — identity from a trusted auth proxy.
#[derive(Clone)]
pub struct ProxyAuth {
    pub header_user: String,
    /// `None` (the default) = no groups (`__claim_groups` is an empty list).
    pub header_groups: Option<ClaimHeader>,
    pub groups_separator: String,
    /// Extra claims: `(claim name, header)` → `__claim_<name>`.
    pub claims: Vec<(String, ClaimHeader)>,
    pub secret_header: String,
    secret: String,
    /// Peer addresses allowed to talk to the server at all; `None` (key not
    /// given) = any peer (the shared secret is still required). An empty list
    /// is rejected.
    pub trusted_proxies: Option<Vec<Cidr>>,
}

impl std::fmt::Debug for ProxyAuth {
    // Never print the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyAuth")
            .field("header_user", &self.header_user)
            .field("header_groups", &self.header_groups)
            .field("claims", &self.claims)
            .field("secret_header", &self.secret_header)
            .field("trusted_proxies", &self.trusted_proxies)
            .finish_non_exhaustive()
    }
}

/// `"X-Header"` or `{"header": "X-Header", "required": true}`.
fn claim_header(key: &str, v: &serde_json::Value) -> Result<ClaimHeader, String> {
    let bad = || {
        format!(
            "auth.{key} must be an HTTP header name or {{\"header\": name, \"required\": bool}}"
        )
    };
    let (h, required) = match v {
        serde_json::Value::String(h) => (h.as_str(), false),
        serde_json::Value::Object(o) => {
            if o.keys().any(|k| k != "header" && k != "required") {
                return Err(bad());
            }
            let req = match o.get("required") {
                None => false,
                Some(r) => r.as_bool().ok_or_else(bad)?,
            };
            (
                o.get("header").and_then(|h| h.as_str()).ok_or_else(bad)?,
                req,
            )
        }
        _ => return Err(bad()),
    };
    let h = h.trim();
    if !valid_header_name(h) {
        return Err(bad());
    }
    Ok(ClaimHeader {
        header: h.to_string(),
        required,
    })
}

fn valid_header_name(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 64
        && h.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && !["host", "cookie", "origin", "content-type", "content-length"]
            .iter()
            .any(|x| h.eq_ignore_ascii_case(x))
}

impl ProxyAuth {
    /// Parse `options.auth` / `--auth`. `env` resolves `shared_secret_env`
    /// (injected for tests; callers pass `|k| std::env::var(k).ok()`).
    pub fn from_json(
        v: &serde_json::Value,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<ProxyAuth, String> {
        let o = v.as_object().ok_or("auth must be a JSON object")?;
        for k in o.keys() {
            if !matches!(
                k.as_str(),
                "mode"
                    | "header_user"
                    | "header_groups"
                    | "groups_separator"
                    | "claims"
                    | "shared_secret"
                    | "shared_secret_env"
                    | "secret_header"
                    | "trusted_proxies"
            ) {
                return Err(format!("auth: unknown key '{k}' (mode, header_user, header_groups, groups_separator, claims, shared_secret, shared_secret_env, secret_header, trusted_proxies)"));
            }
        }
        match o.get("mode").and_then(|m| m.as_str()) {
            Some("proxy") => {}
            _ => return Err("auth.mode must be \"proxy\"".into()),
        }
        let header = |key: &str, default: &str| -> Result<String, String> {
            match o.get(key) {
                None => Ok(default.to_string()),
                Some(v) => v
                    .as_str()
                    .map(str::trim)
                    .filter(|h| valid_header_name(h))
                    .map(str::to_string)
                    .ok_or_else(|| format!("auth.{key} must be an HTTP header name")),
            }
        };
        let header_user = header("header_user", DEFAULT_USER_HEADER)?;
        // Opt-in: a groups header the proxy doesn't set (or clear) on every
        // request would be taken from the client.
        let header_groups = match o.get("header_groups") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(claim_header("header_groups", v)?),
        };
        let groups_separator = match o.get("groups_separator") {
            None => ",".to_string(),
            Some(v) => v
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 4)
                .ok_or("auth.groups_separator must be a short non-empty string")?
                .to_string(),
        };
        let secret_header = header("secret_header", DEFAULT_SECRET_HEADER)?;
        let mut claims = Vec::new();
        if let Some(c) = o.get("claims") {
            let c = c
                .as_object()
                .ok_or("auth.claims must map claim names to header names")?;
            if c.len() > MAX_ATTRS {
                return Err(format!("auth.claims: at most {MAX_ATTRS} claims"));
            }
            for (name, h) in c {
                if !valid_claim_name(name) {
                    return Err(format!("auth.claims: invalid claim name '{name}'"));
                }
                let h = claim_header(&format!("claims.{name}"), h)?;
                claims.push((name.to_ascii_lowercase(), h));
            }
        }
        let secret = match (o.get("shared_secret"), o.get("shared_secret_env")) {
            (Some(s), None) => s
                .as_str()
                .ok_or("auth.shared_secret must be a string")?
                .to_string(),
            (None, Some(e)) => {
                let name = e.as_str().ok_or("auth.shared_secret_env must be a string")?;
                env(name).ok_or_else(|| {
                    format!("auth.shared_secret_env: environment variable '{name}' is not set")
                })?
            }
            (Some(_), Some(_)) => {
                return Err("auth: give shared_secret or shared_secret_env, not both".into())
            }
            (None, None) => {
                return Err("auth: a shared_secret (or shared_secret_env) is required — the proxy sends it in auth.secret_header so the server can tell it from a client".into())
            }
        };
        let secret = secret.trim().to_string();
        if secret.len() < MIN_SECRET_LEN || !secret.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(format!(
                "auth: the shared secret must be at least {MIN_SECRET_LEN} printable ASCII characters (e.g. `openssl rand -hex 32`)"
            ));
        }
        let mut trusted_proxies = None;
        if let Some(t) = o.get("trusted_proxies") {
            let t = t
                .as_array()
                .ok_or("auth.trusted_proxies must be a list of addresses / CIDR ranges")?;
            if t.is_empty() {
                // `[]` reads like "nobody" but would have meant "any peer".
                return Err("auth.trusted_proxies is empty: list the proxy's addresses (e.g. [\"127.0.0.1\"]), or leave the key out to accept any peer that has the secret".into());
            }
            let mut list = Vec::new();
            for c in t {
                let c = c
                    .as_str()
                    .ok_or("auth.trusted_proxies must be a list of strings")?;
                list.push(Cidr::parse(c).map_err(|e| format!("auth.trusted_proxies: {e}"))?);
            }
            trusted_proxies = Some(list);
        }
        let mut names: Vec<String> = [&header_user, &secret_header]
            .into_iter()
            .chain(header_groups.as_ref().map(|g| &g.header))
            .chain(claims.iter().map(|(_, h)| &h.header))
            .map(|h| h.to_ascii_lowercase())
            .collect();
        let n = names.len();
        names.sort();
        names.dedup();
        if names.len() != n {
            return Err(
                "auth: every header (user, groups, claims, secret) must be different".into(),
            );
        }
        Ok(ProxyAuth {
            header_user,
            header_groups,
            groups_separator,
            claims,
            secret_header,
            secret,
            trusted_proxies,
        })
    }

    /// The identity headers this server will believe (for the startup
    /// message), so operators can check each is set or cleared by the proxy.
    pub fn trusted_headers(&self) -> String {
        let mut out = vec![format!("user={}", self.header_user)];
        let req = |h: &ClaimHeader| if h.required { " (required)" } else { "" };
        if let Some(g) = &self.header_groups {
            out.push(format!("groups={}{}", g.header, req(g)));
        }
        for (name, h) in &self.claims {
            out.push(format!("{name}={}{}", h.header, req(h)));
        }
        out.join(", ")
    }

    /// Authenticate one request from `peer` with `headers` (every header, in
    /// order, duplicates included). Only after the peer and the secret check
    /// out are the identity headers read; a repeated identity header is an
    /// error (a proxy that appends instead of replacing could otherwise let the
    /// client's value through).
    pub fn authenticate(
        &self,
        peer: Option<IpAddr>,
        headers: &[(&str, &str)],
    ) -> Result<Claims, AuthError> {
        if let Some(list) = &self.trusted_proxies {
            match peer {
                Some(ip) if list.iter().any(|c| c.contains(ip)) => {}
                _ => return Err(AuthError::UntrustedPeer),
            }
        }
        let all = |name: &str| -> Vec<&str> {
            headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
                .collect()
        };
        let one = |name: &str| -> Result<Option<&str>, AuthError> {
            match all(name).as_slice() {
                [] => Ok(None),
                [v] => Ok(Some(v)),
                _ => Err(AuthError::Malformed),
            }
        };
        match one(&self.secret_header) {
            Ok(Some(s)) if ct_eq(s.trim(), &self.secret) => {}
            _ => return Err(AuthError::BadSecret),
        }
        let clean = clean_claim;
        let user = match one(&self.header_user)? {
            Some(u) => clean(u)?,
            None => return Err(AuthError::NoUser),
        };
        if user.is_empty() {
            return Err(AuthError::NoUser);
        }
        if is_reserved_user(&user) {
            return Err(AuthError::Malformed);
        }
        let mut groups = Vec::new();
        if let Some(h) = &self.header_groups {
            if let Some(g) = one(&h.header)? {
                for part in g.split(self.groups_separator.as_str()) {
                    let part = clean(part)?;
                    if !part.is_empty() {
                        groups.push(part);
                    }
                }
            }
            if h.required && groups.is_empty() {
                return Err(AuthError::MissingClaim);
            }
        }
        groups.sort();
        groups.dedup();
        if groups.len() > MAX_GROUPS {
            return Err(AuthError::Malformed);
        }
        let mut attrs = Vec::new();
        for (name, h) in &self.claims {
            let v = one(&h.header)?
                .map(clean)
                .transpose()?
                .filter(|v| !v.is_empty());
            if h.required && v.is_none() {
                return Err(AuthError::MissingClaim);
            }
            attrs.push((name.clone(), v));
        }
        Ok(Claims {
            user,
            groups,
            attrs,
        })
    }
}

/// Users of this form (`embed:<dashboard>`) are made by the server for embed
/// links without a `user`; no proxy header, token, or API key may claim one.
pub const RESERVED_USER_PREFIX: &str = "embed:";

/// Is `user` in the server's reserved form?
pub fn is_reserved_user(user: &str) -> bool {
    user.len() >= RESERVED_USER_PREFIX.len()
        && user.as_bytes()[..RESERVED_USER_PREFIX.len()]
            .eq_ignore_ascii_case(RESERVED_USER_PREFIX.as_bytes())
}

/// One claim value as the server accepts it from any login method: trimmed,
/// at most [`MAX_CLAIM_LEN`] bytes, no control characters.
pub fn clean_claim(v: &str) -> Result<String, AuthError> {
    let v = v.trim();
    if v.len() > MAX_CLAIM_LEN || v.chars().any(|c| c.is_control()) {
        return Err(AuthError::Malformed);
    }
    Ok(v.to_string())
}

/// Is `name` usable as a claim name (`__claim_<name>`)? An identifier of at
/// most 32 characters; `user` and `groups` are the built-in claims.
pub fn valid_claim_name(name: &str) -> bool {
    valid_ident(name)
        && name.len() <= 32
        && !["user", "groups"].contains(&name.to_ascii_lowercase().as_str())
}

/// Why [`ProxyAuth::authenticate`] refused a request. Every reason is a `401`
/// with the same body; the reason only goes to the request log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    UntrustedPeer,
    BadSecret,
    NoUser,
    /// A claim configured `required` is absent or empty.
    MissingClaim,
    Malformed,
    /// The user claim is an e-mail address the IdP has not verified (#115).
    Unverified,
}

impl AuthError {
    pub fn reason(&self) -> &'static str {
        match self {
            AuthError::UntrustedPeer => "untrusted_peer",
            AuthError::BadSecret => "bad_secret",
            AuthError::NoUser => "no_user",
            AuthError::MissingClaim => "missing_claim",
            AuthError::Malformed => "malformed_identity",
            AuthError::Unverified => "email_unverified",
        }
    }
}

/// The body of every `401`.
pub const UNAUTHORIZED: &str = "unauthorized: sign in through the dashboard's login proxy";

/// A viewer's identity, as established by the proxy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Claims {
    pub user: String,
    /// Sorted, deduplicated.
    pub groups: Vec<String>,
    /// `(claim name, value)` in configuration order; `None` = header absent.
    pub attrs: Vec<(String, Option<String>)>,
}

impl Claims {
    /// The claims as `(variable, typed SQL literal)` pairs: `__claim_user`,
    /// `__claim_groups` (VARCHAR[]), `__claim_<name>` (VARCHAR or NULL).
    pub fn variables(&self) -> Vec<(String, String)> {
        let lit = |s: &str| sql_literal(&serde_json::Value::String(s.to_string()));
        let mut out = vec![(
            format!("{CLAIM_PREFIX}user"),
            lit(&self.user).unwrap_or_else(|_| "NULL".into()),
        )];
        let groups = serde_json::Value::Array(
            self.groups
                .iter()
                .map(|g| serde_json::Value::String(g.clone()))
                .collect(),
        );
        out.push((
            format!("{CLAIM_PREFIX}groups"),
            sql_literal(&groups).unwrap_or_else(|_| "[]::VARCHAR[]".into()),
        ));
        for (name, v) in &self.attrs {
            let l = match v {
                Some(v) => lit(v).unwrap_or_else(|_| "NULL".into()),
                None => "NULL::VARCHAR".into(),
            };
            out.push((format!("{CLAIM_PREFIX}{name}"), l));
        }
        out
    }

    /// A canonical, unambiguous rendering for cache / single-flight keys:
    /// two viewers with different claims never share a key.
    pub fn key(&self) -> String {
        format!("{:?}", (&self.user, &self.groups, &self.attrs))
    }
}

/// What a request log line records about the variables of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogVars {
    None,
    /// The default: names only (values may be personal data).
    Names,
    Values,
}

/// Where the request log goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogSink {
    Stderr,
    /// Appended to (created `0600`); reopened per line, so a rename-based
    /// rotation (logrotate without `copytruncate`) needs no signal.
    File(PathBuf),
}

/// One request, as the log sees it.
#[derive(Debug, Default)]
pub struct LogEntry<'a> {
    pub method: &'a str,
    /// The URL path, without the query string.
    pub path: &'a str,
    pub status: u16,
    pub ms: u128,
    pub user: Option<&'a str>,
    pub peer: Option<IpAddr>,
    pub dashboard: Option<&'a str>,
    pub panel: Option<usize>,
    /// "hit", "shared" or "run" (`/api/panel`).
    pub cache: Option<&'static str>,
    pub vars: &'a [(String, String)],
    pub bytes: Option<usize>,
    /// An [`AuthError::reason`] for a refused request.
    pub deny: Option<&'static str>,
    /// The login method (`proxy`, `jwt`, `oidc`, `embed`, `api_key`).
    pub auth: Option<&'static str>,
}

/// JSON-lines request log (`options.log` / `--log`).
#[derive(Debug)]
pub struct RequestLog {
    pub sink: LogSink,
    pub vars: LogVars,
    lock: Mutex<()>,
}

impl RequestLog {
    /// `log`: `"stderr"` or a file path; `vars`: `"none" | "names" | "values"`.
    /// A file is opened once here, so a bad path fails at startup.
    pub fn new(log: &str, vars: Option<&str>) -> Result<RequestLog, String> {
        let vars = match vars.unwrap_or("names") {
            "none" => LogVars::None,
            "names" => LogVars::Names,
            "values" => LogVars::Values,
            v => {
                return Err(format!(
                    "log_vars must be none, names or values (got '{v}')"
                ))
            }
        };
        let sink = match log.trim() {
            "" => return Err("log must be \"stderr\" or a file path".into()),
            "stderr" | "-" => LogSink::Stderr,
            p => {
                let p = PathBuf::from(p);
                open_log(&p).map_err(|e| format!("log: cannot open {}: {e}", p.display()))?;
                LogSink::File(p)
            }
        };
        Ok(RequestLog {
            sink,
            vars,
            lock: Mutex::new(()),
        })
    }

    /// The JSON line for `e` at `now` (no trailing newline).
    pub fn line(&self, e: &LogEntry, now: std::time::SystemTime) -> String {
        use serde_json::{json, Map, Value};
        let mut o = Map::new();
        o.insert("ts".into(), json!(rfc3339(now)));
        o.insert("user".into(), json!(e.user));
        o.insert("method".into(), json!(e.method));
        let path: String = e.path.chars().take(256).collect();
        o.insert("path".into(), json!(path));
        o.insert("status".into(), json!(e.status));
        o.insert("ms".into(), json!(e.ms as u64));
        if let Some(d) = e.dashboard {
            o.insert("dashboard".into(), json!(d));
        }
        if let Some(p) = e.panel {
            o.insert("panel".into(), json!(p));
        }
        if let Some(c) = e.cache {
            o.insert("cache".into(), json!(c));
        }
        if let Some(b) = e.bytes {
            o.insert("bytes".into(), json!(b));
        }
        if let Some(ip) = e.peer {
            o.insert("peer".into(), json!(normalize(ip).to_string()));
        }
        if let Some(d) = e.deny {
            o.insert("deny".into(), json!(d));
        }
        if let Some(a) = e.auth {
            o.insert("auth".into(), json!(a));
        }
        match self.vars {
            LogVars::None => {}
            LogVars::Names => {
                let mut names: Vec<&str> = e.vars.iter().map(|(k, _)| k.as_str()).collect();
                names.sort();
                o.insert("vars".into(), json!(names));
            }
            LogVars::Values => {
                let m: Map<String, Value> =
                    e.vars.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                o.insert("vars".into(), Value::Object(m));
            }
        }
        // serde_json escapes control characters: one entry is always one line.
        Value::Object(o).to_string()
    }

    /// Write one line (errors are reported on stderr, never fatal).
    pub fn write(&self, e: &LogEntry) {
        let line = self.line(e, std::time::SystemTime::now());
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        match &self.sink {
            LogSink::Stderr => eprintln!("{line}"),
            LogSink::File(p) => {
                use std::io::Write;
                let r = open_log(p).and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
                if let Err(err) = r {
                    eprintln!("[anofox-serve] request log {}: {err}", p.display());
                }
            }
        }
    }
}

/// Open the log for appending: created `0600`; a symlink at the path is
/// refused (`O_NOFOLLOW`), and an existing file readable by others is
/// tightened to `0600`.
pub fn open_log(p: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // O_NONBLOCK: a FIFO put in its place can't hang the server
        o.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let f = o.open(p)?;
        let meta = f.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(f)
    }
    #[cfg(not(unix))]
    o.open(p)
}

/// Request data for a plain-text diagnostic line (`[anofox-serve] …`):
/// control characters (newlines!) escaped, so it can never start a line of
/// its own — e.g. a forged request-log entry on the same stderr — and capped.
pub fn diag(s: &str) -> String {
    let mut out: String = s.chars().take(512).flat_map(char::escape_debug).collect();
    if s.chars().count() > 512 {
        out.push('…');
    }
    out
}

/// Does a request target contain control characters (a bare `\n`, `\t`, …)?
/// Such requests are refused (`400`) before anything is logged or routed.
pub fn has_control_chars(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC).
pub fn rfc3339(t: std::time::SystemTime) -> String {
    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

/// The exact inverse of [`rfc3339`] — called on `Record.ts` (#600), which
/// is normally this function's own output (`Store::append` builds it from
/// `rfc3339(SystemTime::now())`), but review of #611 S1/S2: `Record.ts`
/// is read back from the store *file*, which something other than this
/// server's own `Store::append` could in principle have written or
/// edited (a restore, a hand edit, a different tool) — a parser that
/// isn't a true inverse could then silently accept the wrong instant
/// (`2026-02-31` as March 3; a signed fractional-seconds field such as
/// `.+12Z` treated as `012`) instead of refusing it. Returns microseconds
/// since the Unix epoch (UTC) — the DuckDB Appender's own
/// `duckdb_timestamp.micros`.
///
/// Made exact the cheap way: after the fixed-width parse below (still
/// `None` on anything that isn't even shaped like `YYYY-MM-DDTHH:MM:SS.
/// mmmZ`), the candidate `micros` is re-formatted with [`rfc3339`] itself
/// and compared byte-for-byte against `s`. Anything that doesn't survive
/// that round trip — an out-of-range day-of-month/leap day the day-count
/// arithmetic below doesn't itself validate, a signed digit field
/// `str::parse::<i64>` would otherwise accept — is `None`, never a wrong
/// answer. [`seed_viewer_state_row_via_sql`] (`duckext/src/serve.rs`,
/// #611 S2) is what keeps one such record from taking an otherwise-valid
/// generation's whole trainer read-back down.
pub fn parse_rfc3339_micros(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 24 || &b[4..5] != b"-" || &b[7..8] != b"-" || &b[10..11] != b"T" {
        return None;
    }
    if &b[13..14] != b":" || &b[16..17] != b":" || &b[19..20] != b"." || &b[23..24] != b"Z" {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| std::str::from_utf8(&b[r]).ok()?.parse::<i64>().ok();
    let year = digits(0..4)?;
    let month = digits(5..7)?;
    let day = digits(8..10)?;
    let hour = digits(11..13)?;
    let min = digits(14..16)?;
    let sec = digits(17..19)?;
    let millis = digits(20..23)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    // Howard Hinnant's `days_from_civil` — the exact inverse of the
    // `civil_from_days` arithmetic `rfc3339` itself already uses above.
    // Not itself validated against the month's real length (that's what
    // the round trip below is for): this can compute a `days` for, say,
    // 2026-02-31 that actually lands on March 3rd.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + min * 60 + sec)?;
    let micros = secs.checked_mul(1_000_000)?.checked_add(millis * 1000)?;
    if micros < 0 {
        return None;
    }
    let t = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_micros(micros as u64))?;
    (rfc3339(t) == s).then_some(micros)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn auth(extra: serde_json::Value) -> ProxyAuth {
        let mut v = json!({"mode": "proxy", "shared_secret": SECRET});
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        ProxyAuth::from_json(&v, |_| None).unwrap()
    }

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn reserved_names() {
        assert!(is_reserved_var("__claim_user"));
        assert!(is_reserved_var("__CLAIM_User"));
        assert!(is_reserved_var("__claim_"));
        assert!(!is_reserved_var("claim_user"));
        assert!(!is_reserved_var("__clai"));
        assert!(!is_reserved_var("region"));
        assert!(is_reserved_var("anofox_data") && is_reserved_var("ANOFOX_DATA"));
        assert!(!is_reserved_var("anofox_data2"));
    }

    #[test]
    fn config_is_validated() {
        let p = |v: serde_json::Value| {
            ProxyAuth::from_json(&v, |k| (k == "SEC").then(|| SECRET.to_string()))
        };
        assert!(p(json!({"mode": "proxy", "shared_secret": SECRET})).is_ok());
        assert!(p(json!({"mode": "proxy", "shared_secret_env": "SEC"})).is_ok());
        // no secret, a short one, an unset env var, both, unknown mode / key
        assert!(p(json!({"mode": "proxy"})).is_err());
        assert!(p(json!({"mode": "proxy", "shared_secret": "short"})).is_err());
        assert!(p(json!({"mode": "proxy", "shared_secret_env": "NOPE"})).is_err());
        assert!(
            p(json!({"mode": "proxy", "shared_secret": SECRET, "shared_secret_env": "SEC"}))
                .is_err()
        );
        assert!(p(json!({"mode": "jwt", "shared_secret": SECRET})).is_err());
        assert!(p(json!({"mode": "proxy", "shared_secret": SECRET, "nope": 1})).is_err());
        // headers must be header names, distinct, and not Host/Cookie
        assert!(
            p(json!({"mode": "proxy", "shared_secret": SECRET, "header_user": "a b"})).is_err()
        );
        assert!(
            p(json!({"mode": "proxy", "shared_secret": SECRET, "header_user": "Cookie"})).is_err()
        );
        assert!(p(json!({"mode": "proxy", "shared_secret": SECRET, "header_user": "X-Anofox-Proxy-Secret"})).is_err());
        assert!(
            p(json!({"mode": "proxy", "shared_secret": SECRET, "claims": {"user": "X-U"}}))
                .is_err()
        );
        assert!(
            p(json!({"mode": "proxy", "shared_secret": SECRET, "claims": {"a-b": "X-U"}})).is_err()
        );
        assert!(
            p(json!({"mode": "proxy", "shared_secret": SECRET, "trusted_proxies": ["nope"]}))
                .is_err()
        );
        // the secret never shows up in Debug output
        let a = auth(json!({}));
        assert!(!format!("{a:?}").contains(SECRET));
    }

    #[test]
    fn cidr() {
        let c = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains("10.1.2.3".parse().unwrap()));
        assert!(!c.contains("11.0.0.1".parse().unwrap()));
        assert!(c.contains("::ffff:10.9.9.9".parse().unwrap()));
        let one = Cidr::parse("127.0.0.1").unwrap();
        assert!(one.contains("127.0.0.1".parse().unwrap()));
        assert!(!one.contains("127.0.0.2".parse().unwrap()));
        assert!(Cidr::parse("::1").unwrap().contains("::1".parse().unwrap()));
        assert!(Cidr::parse("0.0.0.0/0")
            .unwrap()
            .contains("8.8.8.8".parse().unwrap()));
        assert!(!Cidr::parse("fd00::/8")
            .unwrap()
            .contains("10.0.0.1".parse().unwrap()));
        assert!(Cidr::parse("10.0.0.0/33").is_err());
        // IPv4-mapped ranges: a clear error instead of a rule that never matches
        for m in [
            "::ffff:10.0.0.0/104",
            "::ffff:127.0.0.1",
            "[::ffff:10.0.0.1]",
        ] {
            let e = Cidr::parse(m).unwrap_err();
            assert!(e.contains("IPv4"), "{m}: {e}");
        }
    }

    #[test]
    fn trusted_proxy_identity() {
        let a =
            auth(json!({"header_groups": "X-Forwarded-Groups", "claims": {"tenant": "X-Tenant"}}));
        let c = a
            .authenticate(
                ip("127.0.0.1"),
                &[
                    ("x-anofox-proxy-secret", SECRET),
                    ("X-Forwarded-User", " alice@example.com "),
                    ("X-Forwarded-Groups", "sales, eu,sales"),
                    ("X-Tenant", "acme"),
                ],
            )
            .unwrap();
        assert_eq!(c.user, "alice@example.com");
        assert_eq!(c.groups, vec!["eu".to_string(), "sales".to_string()]);
        assert_eq!(
            c.attrs,
            vec![("tenant".to_string(), Some("acme".to_string()))]
        );
        let vars = c.variables();
        assert_eq!(
            vars[0],
            ("__claim_user".into(), "'alice@example.com'".into())
        );
        assert_eq!(vars[1], ("__claim_groups".into(), "['eu', 'sales']".into()));
        assert_eq!(vars[2], ("__claim_tenant".into(), "'acme'".into()));
        // a quote in a claim stays a literal
        let c = a
            .authenticate(
                None,
                &[
                    ("X-Anofox-Proxy-Secret", SECRET),
                    ("X-Forwarded-User", "o'brien"),
                ],
            )
            .unwrap();
        assert_eq!(c.variables()[0].1, "'o''brien'");
        assert_eq!(c.variables()[1].1, "[]::VARCHAR[]");
        assert_eq!(c.variables()[2].1, "NULL::VARCHAR");
    }

    #[test]
    fn spoofed_identity_is_refused() {
        let a = auth(
            json!({"trusted_proxies": ["127.0.0.1/32", "10.0.0.0/8"], "header_groups": "X-Forwarded-Groups"}),
        );
        let spoof = [
            ("X-Forwarded-User", "admin"),
            ("X-Forwarded-Groups", "admins"),
        ];
        // a client that sends identity headers itself: no secret → refused
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &spoof),
            Err(AuthError::BadSecret)
        );
        // a wrong / truncated / padded-with-garbage secret
        for bad in ["wrong", &SECRET[1..], &format!("{SECRET}x"), ""] {
            let h = [
                ("X-Anofox-Proxy-Secret", bad),
                ("X-Forwarded-User", "admin"),
            ];
            assert_eq!(
                a.authenticate(ip("127.0.0.1"), &h),
                Err(AuthError::BadSecret),
                "{bad}"
            );
        }
        // the right secret from an untrusted peer (or an unknown peer)
        let h = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", "admin"),
        ];
        assert_eq!(
            a.authenticate(ip("192.168.1.5"), &h),
            Err(AuthError::UntrustedPeer)
        );
        assert_eq!(a.authenticate(None, &h), Err(AuthError::UntrustedPeer));
        assert!(a.authenticate(ip("10.2.3.4"), &h).is_ok());
        // the secret twice, the user twice (an appending proxy), no / empty user,
        // control characters, an oversize value
        let dup_secret = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", "a"),
        ];
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &dup_secret),
            Err(AuthError::BadSecret)
        );
        let dup_user = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", "admin"),
            ("x-forwarded-user", "alice"),
        ];
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &dup_user),
            Err(AuthError::Malformed)
        );
        let dup_groups = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", "a"),
            ("X-Forwarded-Groups", "x"),
            ("X-Forwarded-Groups", "admins"),
        ];
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &dup_groups),
            Err(AuthError::Malformed)
        );
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &[("X-Anofox-Proxy-Secret", SECRET)]),
            Err(AuthError::NoUser)
        );
        let empty = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", "  "),
        ];
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &empty),
            Err(AuthError::NoUser)
        );
        let ctl = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", "a\u{7}b"),
        ];
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &ctl),
            Err(AuthError::Malformed)
        );
        let long = "x".repeat(MAX_CLAIM_LEN + 1);
        let big = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", long.as_str()),
        ];
        assert_eq!(
            a.authenticate(ip("127.0.0.1"), &big),
            Err(AuthError::Malformed)
        );
    }

    #[test]
    fn headers_the_proxy_does_not_set_are_not_claims() {
        // #82: the secret proves the request passed the proxy, not that the
        // proxy wrote the other headers. Groups are off unless configured.
        let a = auth(json!({}));
        assert_eq!(a.header_groups, None);
        let h = [
            ("X-Anofox-Proxy-Secret", SECRET),
            ("X-Forwarded-User", "carol"),
            ("X-Forwarded-Groups", "admins"),
            ("X-Forwarded-Tenant", "acme"),
        ];
        let c = a.authenticate(None, &h).unwrap();
        assert!(c.groups.is_empty() && c.attrs.is_empty(), "{c:?}");
        assert_eq!(c.variables()[1].1, "[]::VARCHAR[]");
        assert_eq!(a.trusted_headers(), "user=X-Forwarded-User");
        // `required` claims: absent or empty → 401, present → the value
        let a = auth(json!({
            "header_groups": {"header": "X-Forwarded-Groups", "required": true},
            "claims": {"tenant": {"header": "X-Forwarded-Tenant", "required": true}, "email": "X-Email"}
        }));
        assert_eq!(
            a.trusted_headers(),
            "user=X-Forwarded-User, groups=X-Forwarded-Groups (required), tenant=X-Forwarded-Tenant (required), email=X-Email"
        );
        assert!(a.authenticate(None, &h).is_ok());
        let no_tenant = [h[0], h[1], h[2]];
        assert_eq!(
            a.authenticate(None, &no_tenant),
            Err(AuthError::MissingClaim)
        );
        let empty_tenant = [h[0], h[1], h[2], ("X-Forwarded-Tenant", " ")];
        assert_eq!(
            a.authenticate(None, &empty_tenant),
            Err(AuthError::MissingClaim)
        );
        let no_groups = [h[0], h[1], h[3]];
        assert_eq!(
            a.authenticate(None, &no_groups),
            Err(AuthError::MissingClaim)
        );
        let empty_groups = [h[0], h[1], ("X-Forwarded-Groups", " , "), h[3]];
        assert_eq!(
            a.authenticate(None, &empty_groups),
            Err(AuthError::MissingClaim)
        );
        // an optional claim that is empty reads as NULL
        let c = a
            .authenticate(None, &[h[0], h[1], h[2], h[3], ("X-Email", "")])
            .unwrap();
        assert_eq!(c.attrs[1], ("email".to_string(), None));
        // bad shapes
        let p = |v: serde_json::Value| ProxyAuth::from_json(&v, |_| None);
        for bad in [
            json!({"mode": "proxy", "shared_secret": SECRET, "header_groups": {"header": "X-G", "nope": 1}}),
            json!({"mode": "proxy", "shared_secret": SECRET, "header_groups": {"required": true}}),
            json!({"mode": "proxy", "shared_secret": SECRET, "claims": {"t": {"header": "X-T", "required": "yes"}}}),
            json!({"mode": "proxy", "shared_secret": SECRET, "header_groups": 1}),
        ] {
            assert!(p(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn empty_trusted_proxies_is_rejected() {
        let e = ProxyAuth::from_json(
            &json!({"mode": "proxy", "shared_secret": SECRET, "trusted_proxies": []}),
            |_| None,
        )
        .unwrap_err();
        assert!(e.contains("empty"), "{e}");
        // leaving the key out = any peer with the secret
        let a = auth(json!({}));
        assert!(a.trusted_proxies.is_none());
        let h = [("X-Anofox-Proxy-Secret", SECRET), ("X-Forwarded-User", "a")];
        assert!(a.authenticate(ip("203.0.113.7"), &h).is_ok());
    }

    #[test]
    fn diagnostics_cannot_forge_lines() {
        let forged = "/x\n{\"ts\":\"2026-10-16T20:00:00.000Z\",\"user\":\"bob\"}\r\n";
        let d = diag(forged);
        assert!(!d.contains('\n') && !d.contains('\r'), "{d}");
        assert!(d.starts_with("/x\\n{"), "{d}");
        assert!(has_control_chars(forged));
        assert!(has_control_chars("/d/a\tb"));
        assert!(!has_control_chars("/d/sales?v.region=EU%0A"));
        let long = "a".repeat(2000);
        assert_eq!(diag(&long).chars().count(), 513);
    }

    #[test]
    fn claims_keys_differ_per_viewer() {
        let a = Claims {
            user: "alice".into(),
            ..Default::default()
        };
        let b = Claims {
            user: "bob".into(),
            ..Default::default()
        };
        let a2 = Claims {
            user: "alice".into(),
            groups: vec!["hr".into()],
            ..Default::default()
        };
        assert_ne!(a.key(), b.key());
        assert_ne!(a.key(), a2.key());
        // no delimiter games: ("a,b", []) vs ("a", ["b"])
        let x = Claims {
            user: "a\", [\"b".into(),
            ..Default::default()
        };
        let y = Claims {
            user: "a".into(),
            groups: vec!["b".into()],
            ..Default::default()
        };
        assert_ne!(x.key(), y.key());
    }

    #[test]
    fn log_lines() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_792_152_000_123);
        assert_eq!(rfc3339(t), "2026-10-16T12:00:00.123Z");
        assert_eq!(rfc3339(std::time::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let vars = vec![
            ("region".to_string(), "'EU'".to_string()),
            ("q".to_string(), "'secret\nvalue'".to_string()),
        ];
        let e = LogEntry {
            method: "POST",
            path: "/api/panel",
            status: 200,
            ms: 12,
            user: Some("alice\n{\"forged\":1}"),
            peer: ip("::ffff:127.0.0.1"),
            dashboard: Some("sales"),
            panel: Some(3),
            cache: Some("hit"),
            vars: &vars,
            bytes: Some(512),
            deny: None,
            auth: Some("jwt"),
        };
        let log = RequestLog {
            sink: LogSink::Stderr,
            vars: LogVars::Names,
            lock: Mutex::new(()),
        };
        let line = log.line(&e, t);
        assert!(!line.contains('\n'), "{line}");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ts"], "2026-10-16T12:00:00.123Z");
        assert_eq!(v["user"], "alice\n{\"forged\":1}");
        assert_eq!(v["dashboard"], "sales");
        assert_eq!(v["panel"], 3);
        assert_eq!(v["status"], 200);
        assert_eq!(v["ms"], 12);
        assert_eq!(v["cache"], "hit");
        assert_eq!(v["peer"], "127.0.0.1");
        assert_eq!(v["auth"], "jwt");
        assert_eq!(v["vars"], json!(["q", "region"]));
        assert!(!line.contains("EU") && !line.contains("secret"), "{line}");
        let log = RequestLog {
            vars: LogVars::Values,
            ..log
        };
        let v: serde_json::Value = serde_json::from_str(&log.line(&e, t)).unwrap();
        assert_eq!(v["vars"]["region"], "'EU'");
        let log = RequestLog {
            vars: LogVars::None,
            ..log
        };
        let v: serde_json::Value = serde_json::from_str(&log.line(&e, t)).unwrap();
        assert!(v.get("vars").is_none());
        // a refused request
        let d = LogEntry {
            method: "GET",
            path: "/d/sales",
            status: 401,
            deny: Some(AuthError::BadSecret.reason()),
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::from_str(&log.line(&d, t)).unwrap();
        assert_eq!(v["deny"], "bad_secret");
        assert_eq!(v["user"], serde_json::Value::Null);
        assert!(RequestLog::new("stderr", Some("bogus")).is_err());
        assert!(RequestLog::new("/nonexistent-dir/x.jsonl", None).is_err());
    }

    /// #600: `parse_rfc3339_micros` is `rfc3339`'s exact inverse, over a
    /// round-trip through real `SystemTime`s (not just the string form),
    /// since that's what `Store::append` actually feeds it and what the
    /// Appender-based `seed_viewer_state` needs back out.
    #[test]
    fn parse_rfc3339_micros_round_trips_rfc3339() {
        for t in [
            std::time::UNIX_EPOCH,
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_792_152_000_123),
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(1),
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(60 * 60 * 24 * 365 * 54), // ~2024
        ] {
            let s = rfc3339(t);
            let micros = parse_rfc3339_micros(&s).unwrap_or_else(|| panic!("{s}"));
            let want_micros = t.duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
            // `rfc3339` truncates to milliseconds, so the parsed micros
            // only need to agree down to that precision, not sub-ms.
            assert_eq!(micros, want_micros / 1000 * 1000, "{s}");
        }
        assert_eq!(parse_rfc3339_micros(""), None);
        assert_eq!(parse_rfc3339_micros("not-a-date"), None);
        assert_eq!(parse_rfc3339_micros("2026-13-01T00:00:00.000Z"), None); // month 13
        assert_eq!(parse_rfc3339_micros("2026-10-16T25:00:00.000Z"), None); // hour 25
        assert_eq!(parse_rfc3339_micros("2026-10-16 12:00:00.000Z"), None); // no 'T'
        assert_eq!(parse_rfc3339_micros("2026-10-16T12:00:00.000"), None); // no 'Z'
                                                                           // #611 S1: these three all passed the fixed-width shape/range
                                                                           // checks above (day 1..=31, no out-of-range hour/min/sec) and
                                                                           // `str::parse::<i64>` happily parses a signed fractional-seconds
                                                                           // field — without the round trip, each would have silently
                                                                           // returned a *different*, wrong instant instead of `None`.
        assert_eq!(parse_rfc3339_micros("2026-02-31T00:00:00.000Z"), None); // no Feb 31st
        assert_eq!(parse_rfc3339_micros("2026-01-01T00:00:00.+12Z"), None); // signed millis
        assert_eq!(parse_rfc3339_micros("2026-01-01T00:00:00.-01Z"), None); // signed millis
    }

    #[test]
    fn log_file_is_private_and_appended() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-log-{}",
            crate::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        let p = dir.join("req.jsonl");
        let log = RequestLog::new(p.to_str().unwrap(), None).unwrap();
        let e = LogEntry {
            method: "GET",
            path: "/",
            status: 200,
            ..Default::default()
        };
        log.write(&e);
        log.write(&e);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // an existing file readable by others is tightened to 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            log.write(&e);
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
            // a symlink at the log path is refused (no write through it)
            let target = dir.join("elsewhere");
            std::fs::write(&target, "").unwrap();
            let link = dir.join("link.jsonl");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(RequestLog::new(link.to_str().unwrap(), None).is_err());
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
            std::fs::remove_file(&p).unwrap();
            std::fs::write(&p, "").unwrap();
        }
        // rotated away by rename: the next line goes to a new file
        std::fs::rename(&p, dir.join("req.jsonl.1")).unwrap();
        log.write(&e);
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

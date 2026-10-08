//! Helpers shared by both serving implementations — the `serve` binary
//! (`src/bin/serve.rs`) and the extension's `anofox_serve` /
//! `anofox_serve_dashboards` (`duckext/src/serve.rs`). Pure functions only (no
//! HTTP or DuckDB dependency) so they are unit-tested here once.
//!
//! Security model (see `docs/secure-serving.md`):
//! * **Authoring** servers run client SQL, so every request must come from the
//!   author: the Host header must be loopback (defeats DNS rebinding), a
//!   cross-origin `Origin` is refused (defeats CSRF from other sites *and* other
//!   localhost ports), and a random per-server token is required — handed over
//!   once via `?token=` in the URL the server prints/opens, then kept in an
//!   `HttpOnly; SameSite=Strict` cookie (or sent as `X-Anofox-Token`).
//! * **Locked** servers never run client SQL: the client names a dashboard +
//!   panel and supplies variable *values*, which become typed SQL literals
//!   ([`sql_literal`]); the SQL itself is owned by the server.

use std::collections::BTreeMap;

/// Header carrying the per-server token for non-browser clients.
pub const TOKEN_HEADER: &str = "X-Anofox-Token";

/// HTML-escape text for element content *and* attribute values.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Serialise JSON for embedding inside an inline `<script>`: escapes `<`, `>`,
/// `&` and the JS line separators so the payload can never close the script
/// element (`</script>`) or open a comment (`<!--`).
pub fn json_for_script(v: &serde_json::Value) -> String {
    let s = v.to_string();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// A random 128-bit hex token. Seeded from the OS (std's `RandomState` keys are
/// drawn from the platform CSPRNG), mixed with time + a counter, so no extra
/// dependency is needed. On unix `/dev/urandom` is preferred when readable.
pub fn random_token() -> String {
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut buf = [0u8; 16];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut buf).is_ok() {
                return buf.iter().map(|b| format!("{b:02x}")).collect();
            }
        }
    }
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(CTR.fetch_add(1, Ordering::Relaxed));
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            h.write_u128(d.as_nanos());
        }
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Constant-time string comparison (for tokens).
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Is `bind` a loopback address (`127.0.0.0/8`, `::1`, `localhost`)?
pub fn is_loopback_bind(bind: &str) -> bool {
    let b = bind.trim().trim_start_matches('[').trim_end_matches(']');
    if b.eq_ignore_ascii_case("localhost") {
        return true;
    }
    b.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Split `host[:port]` (incl. `[v6]:port`) into the host part and the port.
fn split_host_port(h: &str) -> (&str, Option<&str>) {
    let h = h.trim();
    if let Some(rest) = h.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let host = &rest[..end];
            let port = rest[end + 1..].strip_prefix(':');
            return (host, port);
        }
    }
    match h.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host, Some(port)),
        _ => (h, None),
    }
}

/// Is the request's `Host` header a loopback name for this server's port?
/// Rejects everything else — in particular an attacker-controlled DNS name that
/// was re-bound to 127.0.0.1 (DNS rebinding).
pub fn host_is_loopback(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else { return false };
    let (h, p) = split_host_port(host);
    let port_ok = match p {
        Some(p) => p == port.to_string(),
        None => port == 80,
    };
    port_ok && is_loopback_bind(h)
}

/// CSRF check: an `Origin` header (browsers send it on every cross-origin
/// request and on all POSTs) must name the same host:port the request was sent
/// to. A missing `Origin` (curl, same-origin GET) passes — authentication then
/// rests on the token. `null` origins (sandboxed iframes, file://) are refused.
pub fn origin_matches_host(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let Some(host) = host else { return false };
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    authority
        .trim_end_matches('/')
        .eq_ignore_ascii_case(host.trim())
}

/// Extract a cookie value from a `Cookie:` header.
pub fn cookie_value<'a>(cookie_header: Option<&'a str>, name: &str) -> Option<&'a str> {
    cookie_header?.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// The headers of one request that the access checks look at.
#[derive(Default, Clone, Copy)]
pub struct RequestMeta<'a> {
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub cookie: Option<&'a str>,
    pub token_header: Option<&'a str>,
}

/// Why a request was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    /// Host header is not loopback (403).
    BadHost,
    /// Cross-origin request (403).
    BadOrigin,
    /// Missing/wrong token (401).
    NoToken,
}

impl Denied {
    pub fn status(&self) -> u16 {
        match self {
            Denied::NoToken => 401,
            _ => 403,
        }
    }
    pub fn message(&self) -> &'static str {
        match self {
            Denied::BadHost => "forbidden: Host must be a loopback address (127.0.0.1 / localhost / [::1])",
            Denied::BadOrigin => "forbidden: cross-origin request",
            Denied::NoToken => "unauthorized: open the URL printed when the server started (it carries the session token)",
        }
    }
}

/// Access policy for an **authoring** server (client-SQL capable): loopback
/// Host, same-origin, and a per-server token.
pub struct AuthoringGuard {
    pub port: u16,
    pub token: String,
}

impl AuthoringGuard {
    pub fn new(port: u16) -> Self {
        AuthoringGuard {
            port,
            token: random_token(),
        }
    }
    /// Cookie name — includes the port because cookies are shared across ports.
    pub fn cookie_name(&self) -> String {
        format!("anofox_token_{}", self.port)
    }
    /// The URL to print/open: it carries the token once.
    pub fn login_url(&self, host: &str) -> String {
        format!("http://{host}:{}/?token={}", self.port, self.token)
    }
    /// Host + Origin checks (no token) — applied to every request.
    pub fn check_origin(&self, m: &RequestMeta) -> Result<(), Denied> {
        if !host_is_loopback(m.host, self.port) {
            return Err(Denied::BadHost);
        }
        if !origin_matches_host(m.origin, m.host) {
            return Err(Denied::BadOrigin);
        }
        Ok(())
    }
    /// Full check: Host + Origin + token (cookie or header).
    pub fn check(&self, m: &RequestMeta) -> Result<(), Denied> {
        self.check_origin(m)?;
        let name = self.cookie_name();
        let presented = m.token_header.or_else(|| cookie_value(m.cookie, &name));
        match presented {
            Some(t) if ct_eq(t, &self.token) => Ok(()),
            _ => Err(Denied::NoToken),
        }
    }
    /// If `url` carries `?token=<valid>`, return the `(Location, Set-Cookie)`
    /// for a redirect that stores the token in a cookie and drops it from the URL.
    pub fn bootstrap(&self, url: &str) -> Option<(String, String)> {
        let q = parse_query(url);
        let t = q.get("token")?;
        if !ct_eq(t, &self.token) {
            return None;
        }
        let path = url.split('?').next().unwrap_or("/");
        let cookie = format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict",
            self.cookie_name(),
            self.token
        );
        Some((path.to_string(), cookie))
    }
}

/// `[A-Za-z_][A-Za-z0-9_]{0,63}` — a variable / identifier we accept by name.
pub fn valid_ident(name: &str) -> bool {
    let mut cs = name.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && name.len() <= 64
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A dashboard id (file stem): `[A-Za-z0-9_.-]`, not starting with `.`.
pub fn valid_dashboard_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// A SQL string literal (`'…'` with `'` doubled). DuckDB string literals have no
/// backslash escapes, so this is a complete escaping.
pub fn sql_string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A double-quoted SQL identifier (`"` doubled).
pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Max length of one string value / number of list items in [`sql_literal`].
pub const MAX_VALUE_LEN: usize = 4096;
pub const MAX_LIST_LEN: usize = 1000;

/// Turn a client-supplied JSON value into a **typed SQL literal**: strings →
/// `'…'`, numbers → numeric literal, booleans/null → keywords, a flat array of
/// those → a list literal. Anything else (objects, nested arrays, NUL bytes,
/// oversize values, non-finite numbers) is rejected. The output can only ever be
/// a literal — there is no way to smuggle an expression or a subquery in.
pub fn sql_literal(v: &serde_json::Value) -> Result<String, String> {
    scalar_literal(v).or_else(|e| match v {
        serde_json::Value::Array(items) => {
            if items.len() > MAX_LIST_LEN {
                return Err(format!("list has more than {MAX_LIST_LEN} items"));
            }
            let parts: Result<Vec<String>, String> = items.iter().map(scalar_literal).collect();
            Ok(format!("[{}]", parts?.join(", ")))
        }
        _ => Err(e),
    })
}

fn scalar_literal(v: &serde_json::Value) -> Result<String, String> {
    match v {
        serde_json::Value::Null => Ok("NULL".into()),
        serde_json::Value::Bool(b) => Ok(if *b { "TRUE" } else { "FALSE" }.into()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.to_string())
            } else if let Some(u) = n.as_u64() {
                Ok(u.to_string())
            } else {
                match n.as_f64() {
                    Some(f) if f.is_finite() => Ok(format!("{f:?}")),
                    _ => Err("number is not finite".into()),
                }
            }
        }
        serde_json::Value::String(s) => {
            if s.len() > MAX_VALUE_LEN {
                return Err(format!("string value longer than {MAX_VALUE_LEN} bytes"));
            }
            if s.contains('\0') {
                return Err("string value contains NUL".into());
            }
            Ok(sql_string_literal(s))
        }
        _ => Err("value must be a string, number, boolean, null or a flat list of those".into()),
    }
}

/// Max number of variables one locked panel request may set.
pub const MAX_VARS: usize = 64;

/// A server-side paging request for a `::PAGED` table panel.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PageRequest {
    /// Return `[{"n": count}]` instead of a page of rows.
    pub count: bool,
    pub limit: u64,
    pub offset: u64,
    /// Column to sort by (any name; it is quoted, never interpolated raw).
    pub sort: Option<String>,
    pub desc: bool,
    /// Full-text filter (bound as a prepared-statement parameter).
    pub filter: Option<String>,
}

/// A locked-mode data request: `POST /api/panel` with
/// `{"dashboard": id, "panel": n, "vars": {name: value}, "page": {...}}`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PanelRequest {
    pub dashboard: String,
    pub panel: usize,
    /// `(name, typed SQL literal)` — already validated.
    pub vars: Vec<(String, String)>,
    pub page: Option<PageRequest>,
}

/// Parse + validate a locked-mode panel request body.
pub fn parse_panel_request(body: &str) -> Result<PanelRequest, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("bad request JSON: {e}"))?;
    let o = v.as_object().ok_or("request must be a JSON object")?;
    for k in o.keys() {
        if !matches!(k.as_str(), "dashboard" | "panel" | "vars" | "page") {
            return Err(format!("unknown request field '{k}'"));
        }
    }
    let dashboard = o
        .get("dashboard")
        .and_then(|d| d.as_str())
        .filter(|d| valid_dashboard_id(d))
        .ok_or("'dashboard' must be a dashboard id")?
        .to_string();
    let panel = o
        .get("panel")
        .and_then(|p| p.as_u64())
        .ok_or("'panel' must be a non-negative integer")? as usize;
    let mut vars = Vec::new();
    if let Some(vs) = o.get("vars") {
        let vs = vs.as_object().ok_or("'vars' must be an object")?;
        if vs.len() > MAX_VARS {
            return Err(format!("more than {MAX_VARS} variables"));
        }
        for (k, val) in vs {
            if !valid_ident(k) {
                return Err(format!("invalid variable name '{k}'"));
            }
            let lit = sql_literal(val).map_err(|e| format!("variable '{k}': {e}"))?;
            vars.push((k.clone(), lit));
        }
    }
    let page = match o.get("page") {
        None | Some(serde_json::Value::Null) => None,
        Some(p) => {
            let p = p.as_object().ok_or("'page' must be an object")?;
            let num = |k: &str, dflt: u64| p.get(k).and_then(|v| v.as_u64()).unwrap_or(dflt);
            let limit = num("limit", 10);
            if limit == 0 || limit > 10_000 {
                return Err("'page.limit' must be in 1..=10000".into());
            }
            let sort = p.get("sort").and_then(|s| s.as_str()).map(str::to_string);
            if sort.as_ref().is_some_and(|s| s.len() > 256) {
                return Err("'page.sort' too long".into());
            }
            let filter = p
                .get("filter")
                .and_then(|s| s.as_str())
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty());
            if filter
                .as_ref()
                .is_some_and(|s| s.len() > MAX_VALUE_LEN || s.contains('\0'))
            {
                return Err("'page.filter' too long or contains NUL".into());
            }
            Some(PageRequest {
                count: p.get("count").and_then(|v| v.as_bool()).unwrap_or(false),
                limit,
                offset: num("offset", 0),
                sort,
                desc: p.get("desc").and_then(|v| v.as_bool()).unwrap_or(false),
                filter,
            })
        }
    };
    Ok(PanelRequest {
        dashboard,
        panel,
        vars,
        page,
    })
}

/// Wrap a server-owned panel query for paging. Returns the SQL (with at most one
/// `?` placeholder) and the parameter to bind to it (the filter text).
pub fn page_sql(base: &str, p: &PageRequest) -> (String, Option<String>) {
    let base = base.trim().trim_end_matches(';');
    let filter = p.filter.as_ref().map(|f| f.trim().to_string());
    let where_ = if filter.is_some() {
        " WHERE CAST(_dp AS VARCHAR) ILIKE '%' || ? || '%'"
    } else {
        ""
    };
    if p.count {
        return (
            format!("SELECT count(*) AS n FROM ({base}) _dp{where_}"),
            filter,
        );
    }
    let order = match &p.sort {
        Some(c) => format!(
            " ORDER BY {} {}",
            quote_ident(c),
            if p.desc { "DESC" } else { "ASC" }
        ),
        None => String::new(),
    };
    (
        format!(
            "SELECT * FROM ({base}) _dp{where_}{order} LIMIT {} OFFSET {}",
            p.limit, p.offset
        ),
        filter,
    )
}

/// One whitelisted dashboard parameter (`-- @param name [a, b] = a`).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub allowed: Vec<String>,
    pub default: String,
}

/// Header-comment metadata of a dashboard `.sql` file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DashboardMeta {
    pub title: String,
    pub params: Vec<Param>,
    pub refresh: u32,
    /// Extensions to `LOAD` before serving (`-- @load name`), identifier-only.
    pub loads: Vec<String>,
}

/// Parse `-- @title …`, `-- @refresh <s>`, `-- @param name [a, b] = a`,
/// `-- @load <extension>` header comments.
pub fn parse_dashboard_meta(id: &str, script: &str) -> DashboardMeta {
    let mut m = DashboardMeta {
        title: id.to_string(),
        ..Default::default()
    };
    for line in script.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("-- @title ") {
            m.title = r.trim().to_string();
        } else if let Some(r) = l.strip_prefix("-- @refresh ") {
            m.refresh = r.trim().parse().unwrap_or(0);
        } else if let Some(r) = l.strip_prefix("-- @load ") {
            let ext = r.trim();
            if valid_ident(ext) {
                m.loads.push(ext.to_string());
            }
        } else if let Some(r) = l.strip_prefix("-- @param ") {
            if let (Some(o), Some(c)) = (r.find('['), r.find(']')) {
                if c < o {
                    continue;
                }
                let name = r[..o].trim().to_string();
                let allowed: Vec<String> = r[o + 1..c]
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let default = r[c + 1..]
                    .split('=')
                    .nth(1)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| allowed.first().cloned())
                    .unwrap_or_default();
                if valid_ident(&name) && !allowed.is_empty() {
                    m.params.push(Param {
                        name,
                        allowed,
                        default,
                    });
                }
            }
        }
    }
    m
}

/// Parse `?a=b&c=d` into a map (percent-decoded).
pub fn parse_query(url: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some((_, q)) = url.split_once('?') {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                out.insert(url_decode(k), url_decode(v));
            }
        }
    }
    out
}

/// Percent-decoding (`+` → space).
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = |b: u8| (b as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (h(bytes[i + 1]), h(bytes[i + 2])) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Content type by file extension.
pub fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("parquet") => "application/vnd.apache.parquet",
        _ => "application/octet-stream",
    }
}

/// Is this body free of CLI dot-commands? The DuckDB CLI interprets a `-c`
/// argument (and any input line) starting with `.` as a meta command (`.shell`,
/// `.output`, `.read`, …). SQL never needs a line that starts with `.`.
pub fn has_dot_command(sql: &str) -> bool {
    sql.lines().any(|l| l.trim_start().starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn escaping() {
        assert_eq!(
            html_escape("<a href=\"x\">'&"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;"
        );
        let s = json_for_script(&json!({"sql": "</script><script>alert(1)</script>"}));
        assert!(!s.contains("</script>") && !s.contains('<'), "{s}");
        // still valid JSON that decodes to the original
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["sql"], "</script><script>alert(1)</script>");
    }

    #[test]
    fn literals_are_only_literals() {
        assert_eq!(sql_literal(&json!("EU")).unwrap(), "'EU'");
        assert_eq!(
            sql_literal(&json!("' || (SELECT 1) || '")).unwrap(),
            "''' || (SELECT 1) || '''"
        );
        assert_eq!(sql_literal(&json!(5)).unwrap(), "5");
        assert_eq!(sql_literal(&json!(-2.5)).unwrap(), "-2.5");
        assert_eq!(sql_literal(&json!(true)).unwrap(), "TRUE");
        assert_eq!(sql_literal(&json!(null)).unwrap(), "NULL");
        assert_eq!(sql_literal(&json!(["a", "b'c"])).unwrap(), "['a', 'b''c']");
        assert!(sql_literal(&json!({"a": 1})).is_err());
        assert!(sql_literal(&json!([["nested"]])).is_err());
        assert!(sql_literal(&json!("a\u{0}b")).is_err());
        assert!(sql_literal(&json!("x".repeat(MAX_VALUE_LEN + 1))).is_err());
    }

    #[test]
    fn panel_request() {
        let r = parse_panel_request(
            r#"{"dashboard":"sales","panel":3,"vars":{"region":"EU","n":[1,2]},"page":{"limit":5,"offset":10,"sort":"c\"1","desc":true,"filter":"x"}}"#,
        )
        .unwrap();
        assert_eq!(r.dashboard, "sales");
        assert_eq!(r.panel, 3);
        assert!(r.vars.contains(&("region".into(), "'EU'".into())));
        let (sql, param) = page_sql("SELECT 1 AS c0;", r.page.as_ref().unwrap());
        assert_eq!(
            sql,
            "SELECT * FROM (SELECT 1 AS c0) _dp WHERE CAST(_dp AS VARCHAR) ILIKE '%' || ? || '%' ORDER BY \"c\"\"1\" DESC LIMIT 5 OFFSET 10"
        );
        assert_eq!(param.as_deref(), Some("x"));
        assert!(parse_panel_request(r#"{"dashboard":"../x","panel":0}"#).is_err());
        assert!(parse_panel_request(r#"{"dashboard":"a","panel":0,"sql":"SELECT 1"}"#).is_err());
        assert!(
            parse_panel_request(r#"{"dashboard":"a","panel":0,"vars":{"x; DROP":1}}"#).is_err()
        );
        assert!(
            parse_panel_request(r#"{"dashboard":"a","panel":0,"vars":{"x":{"a":1}}}"#).is_err()
        );
    }

    #[test]
    fn host_and_origin() {
        assert!(host_is_loopback(Some("127.0.0.1:8080"), 8080));
        assert!(host_is_loopback(Some("localhost:8080"), 8080));
        assert!(host_is_loopback(Some("[::1]:8080"), 8080));
        assert!(!host_is_loopback(Some("evil.example:8080"), 8080));
        assert!(!host_is_loopback(Some("127.0.0.1:9999"), 8080));
        assert!(!host_is_loopback(None, 8080));
        assert!(origin_matches_host(None, Some("127.0.0.1:8080")));
        assert!(origin_matches_host(
            Some("http://127.0.0.1:8080"),
            Some("127.0.0.1:8080")
        ));
        assert!(!origin_matches_host(
            Some("http://127.0.0.1:9999"),
            Some("127.0.0.1:8080")
        ));
        assert!(!origin_matches_host(Some("null"), Some("127.0.0.1:8080")));
        assert!(
            is_loopback_bind("127.0.0.1")
                && is_loopback_bind("::1")
                && !is_loopback_bind("0.0.0.0")
        );
    }

    #[test]
    fn authoring_guard() {
        let g = AuthoringGuard::new(8080);
        assert_eq!(g.token.len(), 32);
        let good = RequestMeta {
            host: Some("127.0.0.1:8080"),
            ..Default::default()
        };
        assert_eq!(g.check(&good), Err(Denied::NoToken));
        let cookie = format!("a=b; {}={}", g.cookie_name(), g.token);
        let with_cookie = RequestMeta {
            cookie: Some(&cookie),
            ..good
        };
        assert_eq!(g.check(&with_cookie), Ok(()));
        let rebound = RequestMeta {
            host: Some("attacker.example:8080"),
            ..with_cookie
        };
        assert_eq!(g.check(&rebound), Err(Denied::BadHost));
        let csrf = RequestMeta {
            origin: Some("http://localhost:3000"),
            ..with_cookie
        };
        assert_eq!(g.check(&csrf), Err(Denied::BadOrigin));
        let hdr = RequestMeta {
            token_header: Some(&g.token),
            ..good
        };
        assert_eq!(g.check(&hdr), Ok(()));
        let (loc, set) = g.bootstrap(&format!("/?token={}", g.token)).unwrap();
        assert_eq!(loc, "/");
        assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict"));
        assert!(g.bootstrap("/?token=nope").is_none());
    }

    #[test]
    fn dashboard_meta() {
        let m = parse_dashboard_meta(
            "s",
            "-- @title Sales\n-- @param region [EU, US] = US\n-- @load anofox_forecast\n-- @load x; DROP\nSELECT 1;",
        );
        assert_eq!(m.title, "Sales");
        assert_eq!(m.params[0].default, "US");
        assert_eq!(m.loads, vec!["anofox_forecast".to_string()]);
    }

    #[test]
    fn dot_commands() {
        assert!(has_dot_command(".shell id"));
        assert!(has_dot_command("SELECT 1;\n  .read x"));
        assert!(!has_dot_command("SELECT 1.5, t.x FROM t"));
    }
}

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

/// Constant-time string comparison (for tokens and secrets). No early return
/// on a length mismatch either: every byte of the longer input is visited.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let n = a.len().max(b.len());
    let mut diff = (a.len() ^ b.len()) as u64;
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
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

/// Host check for the dashboard servers (locked and static): the `Host` header
/// must be a loopback name on this server's port, or one of the `allowed`
/// names (from `options.hosts` / `--allow-host`, e.g. the public name a reverse
/// proxy forwards). This defeats DNS rebinding, where an attacker's domain is
/// re-pointed at a server that is only reachable from the victim's machine or
/// network. An allowed entry without a port matches that name on any port;
/// `"*"` disables the check (an explicit opt-out). A missing `Host` is refused.
pub fn host_allowed(host: Option<&str>, port: u16, allowed: &[String]) -> bool {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    if host_is_loopback(Some(host), port) {
        return true;
    }
    let (h, p) = split_host_port(host);
    allowed.iter().any(|a| {
        let a = a.trim();
        if a == "*" {
            return true;
        }
        let (ah, ap) = split_host_port(a);
        ah.eq_ignore_ascii_case(h) && (ap.is_none() || ap == p)
    })
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
    /// for a redirect that stores the token in a cookie and drops it from the
    /// URL — every other query parameter (`?theme=`, `?v.*=`) is kept as sent.
    pub fn bootstrap(&self, url: &str) -> Option<(String, String)> {
        let q = parse_query(url);
        let t = q.get("token")?;
        if !ct_eq(t, &self.token) {
            return None;
        }
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        // The Location comes from the request: keep it a same-site path. HTTP
        // rejects control characters in a header value (the header would
        // fail to build); `//host` would redirect off-site, and so would
        // `/\host` (browsers read a `\` as `/`).
        let path = if path.starts_with('/')
            && !path.starts_with("//")
            && !path.contains('\\')
            && !path.to_ascii_lowercase().contains("%5c")
            && !path.bytes().any(|b| b.is_ascii_control())
        {
            path
        } else {
            "/"
        };
        // The token goes under any spelling a server could read as `token`
        // (case, `[]`, trailing `+` / blanks / NUL, `;` as a separator).
        let is_token = |kv: &str| {
            let k = url_decode(kv.split('=').next().unwrap_or("")).to_ascii_lowercase();
            let k = k.trim_matches(|c: char| c.is_whitespace() || c == '\0' || c == '+');
            k.trim_end_matches("[]") == "token"
        };
        let rest: Vec<&str> = query
            .split('#')
            .next()
            .unwrap_or("")
            .split(['&', ';'])
            .filter(|kv| !kv.is_empty() && !is_token(kv))
            .collect();
        let query = rest.join("&");
        let location = if query.is_empty() || query.bytes().any(|b| b.is_ascii_control()) {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        let cookie = format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict",
            self.cookie_name(),
            self.token
        );
        Some((location, cookie))
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
            if items.is_empty() {
                // typed: a bare `[]` is an INTEGER list, which list_contains(…, 'x') rejects
                return Ok("[]::VARCHAR[]".into());
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

/// A server-side request for a `::PAGED` table panel: one page of rows, or the
/// row count, the column statistics or one column's distinct values — all over
/// the same filtered result. Every client-supplied value is a bound parameter
/// (or, for [`page_sql_inline`], an escaped string literal) and every column
/// name a quoted identifier.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PageRequest {
    /// Return `[{"n": count}]` instead of a page of rows.
    pub count: bool,
    pub limit: u64,
    pub offset: u64,
    /// Column to sort by (any name; it is quoted, never interpolated raw).
    pub sort: Option<String>,
    pub desc: bool,
    /// Full-text filter over the whole row.
    pub filter: Option<String>,
    /// Per-column filters, ANDed.
    pub filters: Vec<ColFilter>,
    /// Return one row of statistics for these columns instead of a page: per
    /// column `i`: `n{i}` non-null count, `d{i}` approximate distinct count and,
    /// for values that read as numbers, `s{i}` sum, `m{i}` mean, `md{i}` median,
    /// `lo{i}` min, `hi{i}` max; plus `n` = the row count.
    pub stats: Vec<String>,
    /// Return the column-stats strip's distributions for these columns: rows
    /// `(c, t, k, v, n)` — per column index `c`, `t = 'h'` rows of a 10-bin
    /// histogram of the values that read as numbers (`k` = bin 0..=9 between
    /// their min and max, `n` = count) and `t = 't'` rows of the 3 most
    /// frequent values (`v` as text, `n` = count).
    pub profile: Vec<String>,
    /// Return up to [`MAX_DISTINCT`] distinct values `v` of this column (for a
    /// filter dropdown), under every filter except those on the column itself.
    pub distinct: Option<String>,
    /// Columns that are not table data — only the `::PAGED` panel's `::INFO`
    /// definition ([`crate::sql::INFO_COLUMN`]), at most once: left out of
    /// every answer, the search over the whole row included.
    pub omit: Vec<String>,
}

/// One per-column filter of a [`PageRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct ColFilter {
    pub col: String,
    pub op: FilterOp,
    pub value: String,
}

/// How a [`ColFilter`] compares: text equality / containment, or a numeric
/// comparison (values that don't read as numbers never match).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Eq,
    Contains,
    Gt,
    Ge,
    Lt,
    Le,
}

/// Limits of a [`PageRequest`].
pub const MAX_FILTERS: usize = 32;
pub const MAX_STATS_COLS: usize = 64;
/// Most columns a page request may leave out (`omit`).
pub const MAX_OMIT_COLS: usize = 8;
pub const MAX_DISTINCT: usize = 50;
const MAX_COL_LEN: usize = 256;

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

/// DuckDB's variable names are case-insensitive: of two spellings of one name
/// the later `SET` wins. Keep only that one (dropping the earlier), so
/// whoever builds the cache key and whoever runs the `SET VARIABLE`s agree on
/// one value per name — used both on the request's own `vars` (here) and,
/// after a credential's locked variables are appended
/// ([`crate::host::auth::Scope::lock_vars`]), on the combined list, so two
/// spellings of a locked variable can't collide with each other either
/// (security review of #210/#222, M2).
pub fn dedup_last_spelling(vars: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut vars: Vec<(String, String)> = vars
        .into_iter()
        .rev()
        .filter(|(k, _)| seen.insert(k.to_ascii_lowercase()))
        .collect();
    vars.reverse();
    vars
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
            if crate::host::access::is_reserved_var(k) {
                return Err(format!(
                    "variable '{k}' is reserved: __claim_* (the viewer's login) and anofox_data are set by the server"
                ));
            }
            let lit = sql_literal(val).map_err(|e| format!("variable '{k}': {e}"))?;
            vars.push((k.clone(), lit));
        }
    }
    let vars = dedup_last_spelling(vars);
    let page = match o.get("page") {
        None | Some(serde_json::Value::Null) => None,
        Some(p) => Some(parse_page(p)?),
    };
    Ok(PanelRequest {
        dashboard,
        panel,
        vars,
        page,
    })
}

/// Parse + validate the `page` object of a panel request (also used by the
/// browser, through wasm, to build the same SQL in its own DuckDB).
pub fn parse_page(p: &serde_json::Value) -> Result<PageRequest, String> {
    let p = p.as_object().ok_or("'page' must be an object")?;
    for k in p.keys() {
        if !matches!(
            k.as_str(),
            "count"
                | "limit"
                | "offset"
                | "sort"
                | "desc"
                | "filter"
                | "filters"
                | "stats"
                | "profile"
                | "distinct"
                | "omit"
        ) {
            return Err(format!("unknown page field '{k}'"));
        }
    }
    let num = |k: &str, dflt: u64| p.get(k).and_then(|v| v.as_u64()).unwrap_or(dflt);
    let limit = num("limit", 10);
    if limit == 0 || limit > 10_000 {
        return Err("'page.limit' must be in 1..=10000".into());
    }
    let col = |v: &serde_json::Value, what: &str| -> Result<String, String> {
        let c = v.as_str().ok_or(format!("{what} must be a column name"))?;
        if c.is_empty() || c.len() > MAX_COL_LEN || c.contains('\0') {
            return Err(format!("{what}: invalid column name"));
        }
        Ok(c.to_string())
    };
    let text = |v: &str, what: &str| -> Result<String, String> {
        if v.len() > MAX_VALUE_LEN || v.contains('\0') {
            return Err(format!("{what} too long or contains NUL"));
        }
        Ok(v.to_string())
    };
    let sort = match p.get("sort") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(col(v, "'page.sort'")?),
    };
    let filter = match p.get("filter").and_then(|s| s.as_str()) {
        Some(f) if !f.trim().is_empty() => Some(text(f, "'page.filter'")?),
        _ => None,
    };
    let mut filters = Vec::new();
    if let Some(fs) = p.get("filters") {
        let fs = fs.as_array().ok_or("'page.filters' must be an array")?;
        if fs.len() > MAX_FILTERS {
            return Err(format!("more than {MAX_FILTERS} column filters"));
        }
        for f in fs {
            let f = f.as_object().ok_or("a column filter must be an object")?;
            let op = match f.get("op").and_then(|o| o.as_str()).unwrap_or("") {
                "eq" => FilterOp::Eq,
                "contains" => FilterOp::Contains,
                "gt" => FilterOp::Gt,
                "ge" => FilterOp::Ge,
                "lt" => FilterOp::Lt,
                "le" => FilterOp::Le,
                o => {
                    return Err(format!(
                        "unknown filter op '{o}' (eq, contains, gt, ge, lt, le)"
                    ))
                }
            };
            let value = match f.get("value") {
                Some(serde_json::Value::String(v)) => text(v, "a filter value")?,
                Some(serde_json::Value::Number(n)) => n.to_string(),
                _ => return Err("a filter value must be a string or a number".into()),
            };
            filters.push(ColFilter {
                col: col(
                    f.get("col").unwrap_or(&serde_json::Value::Null),
                    "a filter's 'col'",
                )?,
                op,
                value,
            });
        }
    }
    let mut stats = Vec::new();
    if let Some(cs) = p.get("stats") {
        let cs = cs
            .as_array()
            .ok_or("'page.stats' must be an array of column names")?;
        if cs.len() > MAX_STATS_COLS {
            return Err(format!("more than {MAX_STATS_COLS} statistics columns"));
        }
        for c in cs {
            stats.push(col(c, "'page.stats'")?);
        }
    }
    let mut profile = Vec::new();
    if let Some(cs) = p.get("profile") {
        let cs = cs
            .as_array()
            .ok_or("'page.profile' must be an array of column names")?;
        if cs.len() > MAX_STATS_COLS {
            return Err(format!("more than {MAX_STATS_COLS} profile columns"));
        }
        for c in cs {
            profile.push(col(c, "'page.profile'")?);
        }
    }
    let distinct = match p.get("distinct") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(col(v, "'page.distinct'")?),
    };
    // Only the panel's own ::INFO column can be left out (a page never skips
    // a data column); once, whatever the request repeats.
    let mut omit = Vec::new();
    if let Some(cs) = p.get("omit") {
        let cs = cs
            .as_array()
            .ok_or("'page.omit' must be an array of column names")?;
        if cs.len() > MAX_OMIT_COLS {
            return Err(format!("more than {MAX_OMIT_COLS} omitted columns"));
        }
        for c in cs {
            if c.as_str() != Some(crate::sql::INFO_COLUMN) {
                return Err("'page.omit' may only name the panel's ::INFO column".into());
            }
        }
        if !cs.is_empty() {
            omit.push(crate::sql::INFO_COLUMN.to_string());
        }
    }
    let count = p.get("count").and_then(|v| v.as_bool()).unwrap_or(false);
    if [
        count,
        !stats.is_empty(),
        !profile.is_empty(),
        distinct.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count()
        > 1
    {
        return Err("a page request asks for one of: rows, count, stats, profile, distinct".into());
    }
    Ok(PageRequest {
        count,
        limit,
        offset: num("offset", 0),
        sort,
        desc: p.get("desc").and_then(|v| v.as_bool()).unwrap_or(false),
        filter,
        filters,
        stats,
        profile,
        distinct,
        omit,
    })
}

/// Wrap a server-owned panel query for paging: returns the SQL (with `?`
/// placeholders) and the parameters to bind to them, in order.
pub fn page_sql(base: &str, p: &PageRequest) -> (String, Vec<String>) {
    let mut params = Vec::new();
    let sql = build_page_sql(base, p, &mut |v| {
        params.push(v.to_string());
        "?".to_string()
    });
    (sql, params)
}

/// [`page_sql`] with every parameter inlined as an escaped string literal, for
/// a client that runs its own SQL (the browser's DuckDB-Wasm, the authoring
/// server's `/query`): the same query, one builder.
pub fn page_sql_inline(base: &str, p: &PageRequest) -> String {
    build_page_sql(base, p, &mut |v| format!("'{}'", v.replace('\'', "''")))
}

fn build_page_sql(base: &str, p: &PageRequest, param: &mut dyn FnMut(&str) -> String) -> String {
    let base = base.trim().trim_end_matches(';');
    let base = if p.omit.is_empty() {
        base.to_string()
    } else {
        let cols: Vec<String> = p.omit.iter().map(|c| quote_ident(c)).collect();
        format!("SELECT * EXCLUDE ({}) FROM ({base}) _om", cols.join(", "))
    };
    // A value read as a number — through text, so any column type is fine.
    // Text like an md5 "1e99…" reads as ±inf: only finite values count.
    let num = |c: &str| {
        format!(
            "(CASE WHEN isfinite(TRY_CAST(CAST({c} AS VARCHAR) AS DOUBLE)) \
             THEN TRY_CAST(CAST({c} AS VARCHAR) AS DOUBLE) END)"
        )
    };
    let mut conds = Vec::new();
    if let Some(f) = p.filter.as_ref().map(|f| f.trim()) {
        conds.push(format!(
            "CAST(_dp AS VARCHAR) ILIKE '%' || {} || '%'",
            param(f)
        ));
    }
    for f in &p.filters {
        if p.distinct.as_deref() == Some(f.col.as_str()) {
            continue; // a dropdown lists the values its own filter would hide
        }
        let c = quote_ident(&f.col);
        let v = param(&f.value);
        conds.push(match f.op {
            FilterOp::Eq => format!("CAST({c} AS VARCHAR) = {v}"),
            FilterOp::Contains => format!("CAST({c} AS VARCHAR) ILIKE '%' || {v} || '%'"),
            op => {
                let o = match op {
                    FilterOp::Gt => ">",
                    FilterOp::Ge => ">=",
                    FilterOp::Lt => "<",
                    _ => "<=",
                };
                format!("{} {o} TRY_CAST({v} AS DOUBLE)", num(&c))
            }
        });
    }
    let where_ = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    let from = format!("FROM ({base}) _dp{where_}");
    if p.count {
        return format!("SELECT count(*) AS n {from}");
    }
    if let Some(d) = &p.distinct {
        let c = quote_ident(d);
        return format!(
            "SELECT DISTINCT CAST({c} AS VARCHAR) AS v {from} ORDER BY v NULLS LAST LIMIT {}",
            MAX_DISTINCT + 1
        );
    }
    if !p.stats.is_empty() {
        let mut sel = vec!["count(*) AS n".to_string()];
        for (i, col) in p.stats.iter().enumerate() {
            let c = quote_ident(col);
            let x = num(&c);
            sel.push(format!(
                "count({c}) AS n{i}, approx_count_distinct({c}) AS d{i}, sum({x}) AS s{i}, \
                 avg({x}) AS m{i}, median({x}) AS md{i}, min({x}) AS lo{i}, max({x}) AS hi{i}"
            ));
        }
        return format!("SELECT {} {from}", sel.join(", "));
    }
    // Each column's histogram (10 bins over its numeric values) and top 3
    // values. Every profiled column groups all of its values, so the browser
    // asks for one column per request (#118): asking for all at once runs
    // those aggregates together and multiplies the peak memory.
    if !p.profile.is_empty() {
        let parts: Vec<String> = p
            .profile
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let c = quote_ident(col);
                let x = num(&c);
                // GROUP / ORDER BY position: a table column named `k`, `v`
                // or `n` would bind before the alias of the same name.
                format!(
                    "SELECT {i} AS c, 'h' AS t, k, NULL::VARCHAR AS v, count(*) AS n FROM \
                     (SELECT CASE WHEN _hi > _lo THEN least(9, floor((_x - _lo) / (_hi - _lo) * 10))::INTEGER \
                     ELSE 0 END AS k FROM (SELECT {x} AS _x FROM _f) _v, \
                     (SELECT min({x}) AS _lo, max({x}) AS _hi FROM _f) _b WHERE _x IS NOT NULL) _h GROUP BY 3 \
                     UNION ALL SELECT {i}, 't', NULL::INTEGER, v, n FROM \
                     (SELECT CAST({c} AS VARCHAR) AS v, count(*) AS n FROM _f WHERE {c} IS NOT NULL \
                     GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 3) _t"
                )
            })
            .collect();
        // By position (c, t, k, n DESC, v): `v` breaks ties between top
        // values with the same count, so the rows come back in one order
        // (the client shows them in it).
        return format!(
            "WITH _f AS (SELECT * {from}) {} ORDER BY 1, 2, 3, 5 DESC, 4",
            parts.join(" UNION ALL ")
        );
    }
    let order = match &p.sort {
        Some(c) => format!(
            " ORDER BY {} {} NULLS LAST",
            quote_ident(c),
            if p.desc { "DESC" } else { "ASC" }
        ),
        None => String::new(),
    };
    format!(
        "SELECT * {from}{order} LIMIT {} OFFSET {}",
        p.limit, p.offset
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
    /// `-- @vars a, b`: the only variables a viewer may set on a locked server
    /// (others are dropped); `None` = any (not reserved) variable.
    pub vars: Option<Vec<String>>,
    /// `-- @group Sales / EMEA`: where the dashboard sits in a sidebar or nav,
    /// with ` / ` between the levels; `None` = ungrouped.
    pub group: Option<String>,
    /// `-- @tags a, b`: free labels, trimmed, lower-cased, without duplicates,
    /// at most [`MAX_TAGS`].
    pub tags: Vec<String>,
    /// `-- @description <text>`: one line on what it answers (several lines
    /// are joined); `None` when missing or empty.
    pub description: Option<String>,
    /// `-- @owner <text>`: who looks after the dashboard (a name, a team, an
    /// address); `None` when missing or empty.
    pub owner: Option<String>,
    /// `-- @certified <YYYY-MM-DD>`: the day it was last checked; `None` when
    /// missing or not a valid date.
    pub certified: Option<String>,
}

impl DashboardMeta {
    /// A served folder's nav entry: `{id, title, group, tags, description,
    /// owner, certified}` (`window.__served.nav`).
    pub fn nav_json(&self, id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "title": self.title, "group": self.group, "tags": self.tags,
            "description": self.description, "owner": self.owner, "certified": self.certified,
        })
    }
}

/// The day (UTC, `YYYY-MM-DD`) a file was last modified — a served
/// dashboard's "updated" on the landing page; `None` when unknown.
pub fn file_date(path: &std::path::Path) -> Option<String> {
    let secs = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(iso_date_of_unix(secs))
}

/// `YYYY-MM-DD` (UTC) of a Unix time in seconds (the proleptic Gregorian
/// calendar; Howard Hinnant's `civil_from_days`).
pub fn iso_date_of_unix(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A calendar date written `YYYY-MM-DD` (as `-- @certified` wants it).
pub fn valid_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = &s[r];
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    y >= 1900 && (1..=days).contains(&d)
}

/// The most `-- @tags` a dashboard keeps (the rest are dropped; the linter
/// warns).
pub const MAX_TAGS: usize = 10;
/// A `-- @group` longer than this (in characters) is linted.
pub const MAX_GROUP_CHARS: usize = 80;

/// The value of a `-- @<name>` header line (`None` when `line` is something
/// else). The value may be empty: `-- @group` alone gives `Some("")`.
pub fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let r = line.trim().strip_prefix("-- @")?.strip_prefix(name)?;
    (r.is_empty() || r.starts_with(char::is_whitespace)).then(|| r.trim())
}

/// A `-- @group` value with its levels trimmed and joined by ` / `
/// (`Sales/EMEA` → `Sales / EMEA`); `None` when nothing is left.
pub fn normalize_group(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" / "))
}

/// The labels of a `-- @tags` value: trimmed, lower-cased, non-empty.
pub fn split_tags(raw: &str) -> impl Iterator<Item = String> + '_ {
    raw.split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
}

/// Parse `-- @title …`, `-- @refresh <s>`, `-- @param name [a, b] = a`,
/// `-- @load <extension>`, `-- @vars a, b`, `-- @group a / b`, `-- @tags a, b`,
/// `-- @description <text>`, `-- @owner <text>`, `-- @certified <YYYY-MM-DD>`
/// header comments (a repeated `@group` wins over the earlier one; `@tags`
/// lines add up). A `@param` or
/// `@vars` entry naming a reserved `__claim_*` variable is ignored: claims are
/// never viewer-settable.
pub fn parse_dashboard_meta(id: &str, script: &str) -> DashboardMeta {
    let mut m = DashboardMeta {
        title: id.to_string(),
        ..Default::default()
    };
    for line in script.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("-- @title ") {
            m.title = r.trim().to_string();
        } else if let Some(r) = header_value(l, "group") {
            m.group = normalize_group(r);
        } else if let Some(r) = header_value(l, "description") {
            if !r.is_empty() {
                m.description = Some(match m.description.take() {
                    Some(d) => format!("{d} {r}"),
                    None => r.to_string(),
                });
            }
        } else if let Some(r) = header_value(l, "owner") {
            m.owner = Some(r.to_string()).filter(|o| !o.is_empty());
        } else if let Some(r) = header_value(l, "certified") {
            m.certified = Some(r.to_string()).filter(|d| valid_iso_date(d));
        } else if let Some(r) = header_value(l, "tags") {
            for t in split_tags(r) {
                if m.tags.len() < MAX_TAGS && !m.tags.contains(&t) {
                    m.tags.push(t);
                }
            }
        } else if let Some(r) = l.strip_prefix("-- @refresh ") {
            m.refresh = r.trim().parse().unwrap_or(0);
        } else if let Some(r) = l.strip_prefix("-- @load ") {
            let ext = r.trim();
            if valid_ident(ext) {
                m.loads.push(ext.to_string());
            }
        } else if let Some(r) = l.strip_prefix("-- @vars ") {
            let vars = m.vars.get_or_insert_with(Vec::new);
            for v in r.split(',').map(str::trim) {
                if valid_ident(v) && !crate::host::access::is_reserved_var(v) {
                    vars.push(v.to_ascii_lowercase());
                }
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
                if valid_ident(&name)
                    && !crate::host::access::is_reserved_var(&name)
                    && !allowed.is_empty()
                {
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
        let (sql, params) = page_sql("SELECT 1 AS c0;", r.page.as_ref().unwrap());
        assert_eq!(
            sql,
            "SELECT * FROM (SELECT 1 AS c0) _dp WHERE CAST(_dp AS VARCHAR) ILIKE '%' || ? || '%' ORDER BY \"c\"\"1\" DESC NULLS LAST LIMIT 5 OFFSET 10"
        );
        assert_eq!(params, vec!["x".to_string()]);
        assert!(parse_panel_request(r#"{"dashboard":"../x","panel":0}"#).is_err());
        assert!(parse_panel_request(r#"{"dashboard":"a","panel":0,"sql":"SELECT 1"}"#).is_err());
        assert!(
            parse_panel_request(r#"{"dashboard":"a","panel":0,"vars":{"x; DROP":1}}"#).is_err()
        );
        assert!(
            parse_panel_request(r#"{"dashboard":"a","panel":0,"vars":{"x":{"a":1}}}"#).is_err()
        );
    }

    /// #210: DuckDB's variable names are case-insensitive, and of two
    /// spellings of one name in a request the later `SET` wins — so the
    /// parsed request keeps only that spelling, matching what the server
    /// would actually run.
    #[test]
    fn panel_request_keeps_the_last_spelling_of_a_variable() {
        let r = parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{"A":1,"a":2}}"#).unwrap();
        assert_eq!(r.vars, vec![("a".to_string(), "2".to_string())]);

        let r = parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{"a":2,"A":1}}"#).unwrap();
        assert_eq!(r.vars, vec![("A".to_string(), "1".to_string())]);

        let r = parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{"a":1,"b":2}}"#).unwrap();
        assert_eq!(
            r.vars,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string())
            ]
        );
    }

    /// A `::PERIOD`'s eight variables (as the browser captures them from its
    /// `SET VARIABLE`s) are accepted and bound as literals, NULLs included.
    #[test]
    fn panel_request_with_period_vars() {
        let res = crate::period::resolve_json("default=mtd; compare=none; today=2026-03-31", "{}");
        let vars: serde_json::Map<String, serde_json::Value> = res["vars"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (format!("p_{k}"), v.clone()))
            .collect();
        assert_eq!(vars.len(), crate::period::SUFFIXES.len());
        let body = serde_json::json!({"dashboard": "sales", "panel": 4, "vars": vars});
        let r = parse_panel_request(&body.to_string()).unwrap();
        assert!(r.vars.contains(&("p_from".into(), "'2026-03-01'".into())));
        assert!(r.vars.contains(&("p_prev_from".into(), "NULL".into())));
        assert!(r.vars.contains(&("p_label".into(), "'Mar 2026'".into())));
        assert!(r.vars.contains(&("p_grain".into(), "'day'".into())));
    }

    #[test]
    fn page_column_filters_stats_distinct() {
        let page = |j: &str| parse_page(&serde_json::from_str(j).unwrap());
        // column filters: every value is a parameter, every name a quoted identifier
        let p = page(
            r#"{"filters":[{"col":"re\"gion","op":"eq","value":"EU"},{"col":"amt","op":"ge","value":10},
                {"col":"note","op":"contains","value":"it's"}],"sort":"amt","limit":20}"#,
        )
        .unwrap();
        let (sql, params) = page_sql("SELECT * FROM t", &p);
        assert_eq!(
            sql,
            "SELECT * FROM (SELECT * FROM t) _dp WHERE CAST(\"re\"\"gion\" AS VARCHAR) = ? \
             AND (CASE WHEN isfinite(TRY_CAST(CAST(\"amt\" AS VARCHAR) AS DOUBLE)) THEN TRY_CAST(CAST(\"amt\" AS VARCHAR) AS DOUBLE) END) >= TRY_CAST(? AS DOUBLE) \
             AND CAST(\"note\" AS VARCHAR) ILIKE '%' || ? || '%' ORDER BY \"amt\" ASC NULLS LAST LIMIT 20 OFFSET 0"
        );
        assert_eq!(params, vec!["EU", "10", "it's"]);
        // the inline form escapes quotes in the literal
        assert!(page_sql_inline("SELECT * FROM t", &p).contains("ILIKE '%' || 'it''s' || '%'"));
        // stats: one row of aggregates over the filtered result
        let p =
            page(r#"{"stats":["amt","region"],"filters":[{"col":"amt","op":"lt","value":"5"}]}"#)
                .unwrap();
        let (sql, params) = page_sql("SELECT * FROM t", &p);
        assert!(sql.starts_with(
            "SELECT count(*) AS n, count(\"amt\") AS n0, approx_count_distinct(\"amt\") AS d0"
        ));
        assert!(sql.contains("sum((CASE WHEN isfinite(TRY_CAST(CAST(\"region\" AS VARCHAR) AS DOUBLE)) THEN TRY_CAST(CAST(\"region\" AS VARCHAR) AS DOUBLE) END)) AS s1"));
        assert!(sql.contains("median((CASE WHEN isfinite(TRY_CAST(CAST(\"amt\" AS VARCHAR) AS DOUBLE)) THEN TRY_CAST(CAST(\"amt\" AS VARCHAR) AS DOUBLE) END)) AS md0"));
        assert_eq!(params, vec!["5"]);
        // distinct ignores the column's own filter, keeps the others
        let p = page(
            r#"{"distinct":"region","filters":[{"col":"region","op":"eq","value":"EU"},{"col":"amt","op":"gt","value":1}]}"#,
        )
        .unwrap();
        let (sql, params) = page_sql("SELECT * FROM t", &p);
        assert!(sql.starts_with("SELECT DISTINCT CAST(\"region\" AS VARCHAR) AS v FROM"));
        assert!(sql.ends_with("ORDER BY v NULLS LAST LIMIT 51"));
        assert_eq!(params, vec!["1"]);
        // profile: per column a histogram and the top values, over the filtered rows
        let p =
            page(r#"{"profile":["amt"],"filters":[{"col":"amt","op":"lt","value":"5"}]}"#).unwrap();
        let (sql, params) = page_sql("SELECT * FROM t", &p);
        assert!(sql.starts_with("WITH _f AS (SELECT * FROM (SELECT * FROM t) _dp WHERE"));
        assert!(sql.contains("SELECT 0 AS c, 'h' AS t"));
        assert!(sql.contains("GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 3"));
        assert_eq!(params, vec!["5"]);
        // rejected
        for bad in [
            r#"{"filters":[{"col":"a","op":"like","value":"x"}]}"#,
            r#"{"filters":[{"col":"a","op":"eq","value":{"x":1}}]}"#,
            r#"{"filters":[{"col":"","op":"eq","value":"x"}]}"#,
            r#"{"stats":["a"],"count":true}"#,
            r#"{"distinct":"a","stats":["a"]}"#,
            r#"{"profile":["a"],"stats":["a"]}"#,
            r#"{"profile":"a"}"#,
            r#"{"limit":0}"#,
            r#"{"sql":"SELECT 1"}"#,
        ] {
            assert!(page(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn host_allowlist() {
        let none: Vec<String> = vec![];
        let ok = |h: &str, allowed: &[String]| host_allowed(Some(h), 8095, allowed);
        // loopback names on the right port
        for h in [
            "127.0.0.1:8095",
            "localhost:8095",
            "[::1]:8095",
            "LOCALHOST:8095",
        ] {
            assert!(ok(h, &none), "{h}");
        }
        // DNS rebinding: an attacker's name pointed at 127.0.0.1 is refused
        assert!(!ok("evil.example:8095", &none));
        assert!(!ok("127.0.0.1:9999", &none));
        assert!(!host_allowed(None, 8095, &none));
        assert!(!ok("", &none));
        // allowlisted names: with any port, or with an exact port
        let allowed = vec!["dash.example.com".to_string(), "intranet:8443".to_string()];
        assert!(ok("dash.example.com", &allowed));
        assert!(ok("Dash.Example.com:443", &allowed));
        assert!(ok("intranet:8443", &allowed));
        assert!(!ok("intranet:8444", &allowed));
        assert!(!ok("evil.example", &allowed));
        assert!(ok("anything:1", &["*".to_string()]));
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
        for odd in [
            "//evil.example/",
            "/\\evil.example/",
            "/\\/evil.example/",
            "/%5Cevil.example/",
            "/%5cevil.example/",
            "/a\rb",
            "/a\x0bb",
            "x",
        ] {
            let (loc, _) = g.bootstrap(&format!("{odd}?token={}", g.token)).unwrap();
            assert_eq!(loc, "/", "{odd:?}");
        }
        let (loc, _) = g.bootstrap(&format!("/d/sales?token={}", g.token)).unwrap();
        assert_eq!(loc, "/d/sales");
        // #155: every parameter but the token survives, as sent.
        let (loc, _) = g
            .bootstrap(&format!(
                "/?theme=dark&token={}&v.run=a%20b&v.ch=x&v.ch=y",
                g.token
            ))
            .unwrap();
        assert_eq!(loc, "/?theme=dark&v.run=a%20b&v.ch=x&v.ch=y");
        let (loc, _) = g
            .bootstrap(&format!("/d/sales?%74oken={}&zoom=1", g.token))
            .unwrap();
        assert_eq!(loc, "/d/sales?zoom=1");
        let (loc, _) = g
            .bootstrap(&format!("/?token={}&v.a=\r\nX: y", g.token))
            .unwrap();
        assert_eq!(loc, "/");
        let (loc, _) = g
            .bootstrap(&format!("//evil.example/?token={}&a=1", g.token))
            .unwrap();
        assert_eq!(loc, "/?a=1");
        // other spellings of the key go too
        let t = &g.token;
        let (loc, _) = g
            .bootstrap(&format!(
                "/?token={t}&TOKEN={t}&token[]={t}&token+={t}&token%00={t}&a=1;token={t}&%20Token%20={t}&b=2"
            ))
            .unwrap();
        assert_eq!(loc, "/?a=1&b=2");
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
        assert_eq!(m.vars, None);
        // claims are never viewer-settable: not as a @param, not in @vars
        let m = parse_dashboard_meta(
            "s",
            "-- @param __claim_user [alice, bob]\n-- @param __CLAIM_x [a]\n-- @vars Region, selected, __claim_groups, bad name\nSELECT 1;",
        );
        assert!(m.params.is_empty());
        assert_eq!(
            m.vars,
            Some(vec!["region".to_string(), "selected".to_string()])
        );
    }

    #[test]
    fn dashboard_meta_group_and_tags() {
        let m = parse_dashboard_meta("s", "SELECT 1;");
        assert_eq!((m.group, m.tags.len()), (None, 0));
        let m = parse_dashboard_meta(
            "s",
            "-- @title  Weekly sales\n-- @group  Sales/ EMEA \n-- @tags weekly, KPI , ,emea\n\
             --  @tags kpi, Margin\nSELECT 1;",
        );
        assert_eq!(m.title, "Weekly sales");
        assert_eq!(m.group.as_deref(), Some("Sales / EMEA"));
        // `--  @tags` (two spaces) is not a header line
        assert_eq!(m.tags, ["weekly", "kpi", "emea"]);
        let m = parse_dashboard_meta(
            "s",
            "-- @group Ops\n-- @group  \n-- @groups x\n-- @tagsx y\n-- @tags k, kpi\nSELECT 1;",
        );
        assert_eq!(m.group, None, "an empty @group later ungroups");
        assert_eq!(m.tags, ["k", "kpi"]);
        let many: Vec<String> = (0..14).map(|i| format!("t{i}")).collect();
        let m = parse_dashboard_meta("s", &format!("-- @tags {}\n", many.join(",")));
        assert_eq!(m.tags.len(), MAX_TAGS);
        assert_eq!(m.tags[9], "t9");
        assert_eq!(
            normalize_group("a / b / c").as_deref(),
            Some("a / b / c"),
            "deeper paths are kept as written"
        );
        assert_eq!(header_value("-- @group", "group"), Some(""));
        // owner and certification
        let m = parse_dashboard_meta(
            "s",
            "-- @owner  Finance team <fin@example.com> \n-- @certified 2026-10-15\nSELECT 1;",
        );
        assert_eq!(m.owner.as_deref(), Some("Finance team <fin@example.com>"));
        assert_eq!(m.certified.as_deref(), Some("2026-10-15"));
        let j = m.nav_json("s");
        assert_eq!(j["owner"], "Finance team <fin@example.com>");
        assert_eq!(j["certified"], "2026-10-15");
        assert!(j["group"].is_null() && j["tags"].as_array().unwrap().is_empty());
        let m = parse_dashboard_meta(
            "s",
            "-- @description Which stores sell\n-- @description  the most?\n-- @description\nSELECT 1;",
        );
        assert_eq!(
            m.description.as_deref(),
            Some("Which stores sell the most?")
        );
        assert_eq!(
            m.nav_json("s")["description"],
            "Which stores sell the most?"
        );
        for (secs, day) in [
            (0, "1970-01-01"),
            (951_782_400, "2000-02-29"),
            (1_792_195_200, "2026-10-17"),
            (4_107_456_000, "2100-02-28"),
        ] {
            assert_eq!(iso_date_of_unix(secs), day, "{secs}");
        }
        let m = parse_dashboard_meta("s", "-- @owner\n-- @certified 15.10.2026\nSELECT 1;");
        assert_eq!((m.owner, m.certified), (None, None));
        for ok in ["2026-10-15", "2024-02-29", "2000-02-29", "1999-12-31"] {
            assert!(valid_iso_date(ok), "{ok}");
        }
        for bad in [
            "2026-13-01",
            "2026-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-10-1",
            "2026/10/15",
            "26-10-15",
            "2026-10-15x",
            "+026-10-15",
            "1800-01-01",
            "",
            "2026-00-10",
        ] {
            assert!(!valid_iso_date(bad), "{bad}");
        }
        assert_eq!(header_value("-- @title x", "group"), None);
    }

    #[test]
    fn reserved_claim_variables_are_rejected() {
        for name in [
            "__claim_user",
            "__CLAIM_USER",
            "__claim_groups",
            "__Claim_tenant",
        ] {
            let body = format!(r#"{{"dashboard":"d","panel":1,"vars":{{"{name}":"admin"}}}}"#);
            let e = parse_panel_request(&body).unwrap_err();
            assert!(e.contains("reserved"), "{name}: {e}");
        }
        assert!(
            parse_panel_request(r#"{"dashboard":"d","panel":1,"vars":{"claim_user":"x"}}"#).is_ok()
        );
        for name in ["anofox_data", "ANOFOX_Data"] {
            let body = format!(r#"{{"dashboard":"d","panel":1,"vars":{{"{name}":"/etc"}}}}"#);
            assert!(
                parse_panel_request(&body).unwrap_err().contains("reserved"),
                "{name}"
            );
        }
        // ct_eq: equal only when equal, whatever the lengths
        assert!(ct_eq("abc", "abc") && !ct_eq("abc", "abd") && !ct_eq("abc", "abcd"));
        assert!(!ct_eq("", "a") && ct_eq("", "") && !ct_eq("abc\0", "abc"));
    }

    #[test]
    fn dot_commands() {
        assert!(has_dot_command(".shell id"));
        assert!(has_dot_command("SELECT 1;\n  .read x"));
        assert!(!has_dot_command("SELECT 1.5, t.x FROM t"));
    }
}

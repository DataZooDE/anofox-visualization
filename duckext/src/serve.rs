//! In-process HTTP servers behind the extension's serving functions.
//!
//! * `anofox_serve(port)` — **authoring**: the embedded browser builder plus a
//!   `/query` bridge that runs SQL on the *live, read-write* session. Because it
//!   runs arbitrary SQL it is locked to the author: loopback bind only, the
//!   `Host` must be loopback (DNS rebinding), cross-origin requests are refused
//!   (CSRF — incl. pages on other localhost ports), and every request needs the
//!   random per-server token from the printed/opened URL (kept in an
//!   `HttpOnly; SameSite=Strict` cookie, or sent as `X-Anofox-Token`).
//! * `anofox_serve_dashboards(dir, port[, options])` — **locked**: serves the
//!   dashboards in `dir` to untrusted viewers. The client never sends SQL: it
//!   POSTs `{dashboard, panel, vars, page}` to `/api/panel`; the server runs its
//!   own panel SQL, with variable values bound as typed literals and the paging
//!   filter as a prepared-statement parameter. Queries run on a private
//!   read-only snapshot with external access, extension loading and config
//!   changes disabled, one fresh connection per request (no variable leakage
//!   between viewers), with parquet footers cached across requests, under a
//!   row cap and a query timeout. Results are cached
//!   per snapshot, identical concurrent requests run once, and a request
//!   superseded by the same viewer's newer one is interrupted
//!   ([`crate::panel_cache`]). The snapshot can be rebuilt while serving
//!   (`refresh_s`, `anofox_serve_refresh`) and is swapped atomically
//!   ([`crate::generation`]); `data` opens one folder of files read-only to
//!   the panels, for data too large or too live to snapshot. An access policy
//!   (`policy`, [`anofox_visualization::host::policy`]) rewrites every
//!   dashboard statement to read per-viewer views (row filters, column
//!   masks) and refuses to start on anything that could get around them.
//!   `options.source` (#352 prototype, [`crate::source`]) builds (part of)
//!   the snapshot from a live DuckDB 2.0 Quack server instead of (or
//!   alongside) the local session, through a dedicated staging instance —
//!   `quack` is never loaded into this serving instance.
//! * `anofox_serve_refresh(port)` — rebuild a locked server's snapshot now.
//! * `anofox_serve_stop(port)` — stop a server and delete its snapshot.
//!
//! Each server's state is owned by the server (no process-wide "last load
//! wins" globals besides the port registry), and startup is transactional:
//! nothing is registered until the snapshot is built and the port is bound.

use crate::db::{json_rows, Conn, Database};
use crate::generation::{Current, Generation};
use crate::limits::{Charge, Inflight, Queries};
use crate::panel_cache::{Served, SUPERSEDED};
use anofox_visualization::host::access::{self, Claims, LogEntry, RequestLog};
use anofox_visualization::host::app_brand::{self, AppBrand};
use anofox_visualization::host::auth::{self, audit::AuditLog, Auth, Scope};
use anofox_visualization::host::policy::{self, Enforcer, Policy, Script, Viewer};
use anofox_visualization::host::serving::{
    self as sv, content_type, json_for_script, quote_ident, sql_string_literal, AuthoringGuard,
    RequestMeta,
};
use anofox_visualization::host::telemetry;
use anofox_visualization::sql::deps::{self, Deps};
use anofox_visualization::sql::requires::{self as rq, Requirement};
use include_dir::{include_dir, Dir};
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

// The browser builder, embedded at compile time (needs web/pkg from wasm-pack —
// build-native.sh and CI build it first).
static WEB: Dir = include_dir!("$CARGO_MANIFEST_DIR/../web");

/// Per-database context handed to the serving functions as extra_info: the
/// connection opened at LOAD time on *that* database (so two databases that
/// load the extension each serve their own data).
pub struct DbCtx {
    pub conn: Mutex<Conn>,
}

/// A running server, keyed by port in [`SERVERS`].
struct Running {
    kind: &'static str,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    /// HTTP request-handling workers. Each request is already bounded on
    /// its own (a locked server's `timeout_ms` cancels a running panel
    /// query; authoring serialises on a single worker), so `stop()` fully
    /// joins these rather than racing the port's unbind against whatever
    /// time a legitimate, already-in-flight request still has left (#356
    /// C1 — a flat stop budget applied here left the port bound for up to
    /// `timeout_ms` after `anofox_serve_stop` had already reported success).
    workers: Vec<JoinHandle<()>>,
    /// The `refresh_s` thread for a locked server, if any. Unlike the HTTP
    /// workers, this has no bound of its own: an *abandoned* Quack build
    /// (past its own `timeout_s` + grace, #352 M1/R2) can still be running
    /// in the background with nothing else to wait on, so only this thread
    /// gets `stop()`'s short, fixed join budget (#356 I6).
    refresher: Option<JoinHandle<()>>,
    /// A locked server's state (for `anofox_serve_refresh`).
    locked: Option<Arc<Locked>>,
    /// Dropped after the workers have exited (closes the snapshot DB and
    /// deletes its directory for locked servers).
    _state: Box<dyn Send>,
}

static SERVERS: Mutex<BTreeMap<u16, Running>> = Mutex::new(BTreeMap::new());

fn lock_servers() -> std::sync::MutexGuard<'static, BTreeMap<u16, Running>> {
    SERVERS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Stop the server on `port` (unblocks and joins its workers, then releases
/// its state — for a locked server that closes and deletes the snapshot).
pub fn stop(port: u16) -> Result<String, String> {
    let running = lock_servers().remove(&port);
    let Some(mut r) = running else {
        return Err(format!(
            "anofox-visualization: nothing is serving on port {port}"
        ));
    };
    // #352 I2: `Locked.stop` (if this is a locked server) is the SAME Arc,
    // so a Quack build's watchdog sees this instantly as its cancel signal
    // too — no separate flag, no window where the request could be missed.
    r.stop.store(true, Ordering::SeqCst);
    for _ in 0..r.workers.len() {
        r.server.unblock();
    }
    // #356 C1: the HTTP workers are fully joined, not budget-capped — each
    // one handles at most one request at a time, and that request is
    // already bounded (a locked server's own `timeout_ms` cancels a running
    // panel query; authoring's single worker serialises on the live
    // connection the same way it always has). A flat cap here previously
    // let `anofox_serve_stop` report success while a legitimate, still
    // in-flight request kept the port bound underneath it.
    for h in std::mem::take(&mut r.workers) {
        let _ = h.join();
    }
    // #352 M1 fix 3 / I6: the refresh_s thread, in contrast, has no bound of
    // its own — it can be in the middle of an *abandoned* Quack build (past
    // its own `timeout_s` + grace) left running in the background (#352
    // M1/R2) with nothing else to wait on. Only this one thread gets a
    // short, fixed stop budget, regardless of the build's own `timeout_s`.
    const STOP_JOIN_BUDGET: Duration = Duration::from_secs(5);
    if let Some(h) = r.refresher.take() {
        join_bounded(vec![h], STOP_JOIN_BUDGET);
    }
    Ok(format!(
        "anofox-visualization: stopped the {} server on port {port}",
        r.kind
    ))
}

fn spawn_workers(
    server: &Arc<Server>,
    stop: &Arc<AtomicBool>,
    n: usize,
    handler: Arc<dyn Fn(Request) + Send + Sync>,
) -> Vec<JoinHandle<()>> {
    (0..n.max(1))
        .map(|_| {
            let (server, stop, handler) = (server.clone(), stop.clone(), handler.clone());
            std::thread::spawn(move || loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                match server.recv() {
                    Ok(req) => {
                        // A panic in one request must not take the worker down.
                        let h = handler.clone();
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(req)));
                    }
                    Err(_) => {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                }
            })
        })
        .collect()
}

// ---------------------------------------------------------------- helpers ---

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header")
}

fn req_header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn meta(req: &Request) -> RequestMeta<'_> {
    RequestMeta {
        host: req_header(req, "Host"),
        origin: req_header(req, "Origin"),
        cookie: req_header(req, "Cookie"),
        token_header: req_header(req, sv::TOKEN_HEADER),
    }
}

fn respond(req: Request, status: u16, ctype: &str, body: impl Into<Vec<u8>>) {
    let resp = Response::from_data(body.into())
        .with_status_code(status)
        .with_header(header("Content-Type", ctype))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"));
    let _ = req.respond(resp);
}

/// The pages' Content-Security-Policy (#252), as a response header: the
/// policy of `assistCsp()` (web/assist-csp.js) with `'self'` for the app's
/// own files, plus a per-response nonce for the server's own inline
/// `<script>`s and `frame-ancestors`. `base-uri 'self'`: the dashboard page
/// sets `<base href="/">`.
fn page_csp(nonce: &str, frame_ancestors: &str) -> String {
    let mut p = format!(
        "default-src 'none'; script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval' {CDN_DUCKDB_WASM} {CDN_KATEX}; \
         worker-src blob:; connect-src 'self' {CDN_DUCKDB_WASM} {DUCKDB_EXT} {USGS}; img-src 'self' data: blob:; \
         style-src 'self' 'unsafe-inline' https://fonts.googleapis.com {CDN_KATEX}; \
         font-src 'self' data: https://fonts.gstatic.com {CDN_KATEX}; manifest-src 'self'; media-src 'none'; \
         object-src 'none'; frame-src 'none'; base-uri 'self'; form-action 'none'"
    );
    if !frame_ancestors.is_empty() {
        p.push_str("; frame-ancestors ");
        p.push_str(frame_ancestors);
    }
    p
}

// The third-party origins of `assistCsp()` (web/assist-csp.js; a test keeps
// them equal).
const CDN_DUCKDB_WASM: &str = "https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.33.1-dev65.0/";
const CDN_KATEX: &str = "https://cdn.jsdelivr.net/npm/katex@0.19.0/";
const DUCKDB_EXT: &str = "https://extensions.duckdb.org/v1.5.6/";
const USGS: &str = "https://earthquake.usgs.gov/earthquakes/feed/";

/// `https://host[:port]`, `'self'` or `'none'`.
fn valid_frame_ancestor(o: &str) -> bool {
    o == "'self'"
        || o == "'none'"
        || o.strip_prefix("https://").is_some_and(|h| {
            !h.is_empty()
                && h.split_once(':')
                    .is_none_or(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b))
        })
}

/// The `frame-ancestors` value for these options ("" = no directive).
fn frame_ancestors(opts: &LockedOptions) -> String {
    match &opts.frame_ancestors {
        Some(list) if !list.is_empty() => list.join(" "),
        Some(_) => "'none'".into(),
        None if opts
            .auth
            .as_ref()
            .is_some_and(|a| a.methods().contains(&"embed")) =>
        {
            String::new()
        }
        None => "'none'".into(),
    }
}

/// An HTML page with its CSP: the server's own inline `<script>`s (only
/// they are a bare `<script>`: viewer and dashboard text is escaped,
/// external scripts carry `src`) get this response's nonce.
fn with_csp(html: &str, opts: &LockedOptions) -> (Vec<u8>, String) {
    let nonce = anofox_visualization::host::auth::session::random_b64(18);
    let body = html.replace("<script>", &format!("<script nonce=\"{nonce}\">"));
    (body.into_bytes(), page_csp(&nonce, &frame_ancestors(opts)))
}

/// An HTML page with its CSP header (see [`with_csp`]).
fn respond_page(req: Request, html: &str, opts: &LockedOptions, extra: &[(&str, &str)]) {
    let (body, csp) = with_csp(html, opts);
    let mut resp = Response::from_data(body)
        .with_status_code(200)
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Content-Security-Policy", &csp));
    for (k, v) in extra {
        resp = resp.with_header(header(k, v));
    }
    let _ = req.respond(resp);
}

/// Send a reply of the login (redirects, cookies, login pages).
fn send(req: Request, r: auth::Reply) -> u16 {
    let mut resp = Response::from_data(r.body)
        .with_status_code(r.status)
        .with_header(header("Content-Type", r.content_type))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Referrer-Policy", "same-origin"));
    for (k, v) in &r.headers {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
    r.status
}

/// A cached panel result as a response body (no copy of the JSON).
struct SharedBody(Arc<String>);

impl AsRef<[u8]> for SharedBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

fn respond_panel(req: Request, body: Arc<String>, etag: &str, deps: &str) {
    let len = body.len();
    let mut headers = vec![
        header("Content-Type", "application/json"),
        header("X-Content-Type-Options", "nosniff"),
        header("Cache-Control", "no-store"),
        header("ETag", etag),
    ];
    if !deps.is_empty() {
        headers.push(header("X-Anofox-Deps", deps));
    }
    let resp = Response::new(
        200.into(),
        headers,
        std::io::Cursor::new(SharedBody(body)),
        Some(len),
        None,
    );
    let _ = req.respond(resp);
}

fn text(req: Request, status: u16, msg: &str) {
    respond(
        req,
        status,
        "text/plain; charset=utf-8",
        msg.as_bytes().to_vec(),
    );
}

/// Read the request body, refusing anything larger than `limit` bytes.
fn read_body(req: &mut Request, limit: usize) -> Result<String, (u16, String)> {
    if req.body_length().is_some_and(|n| n > limit) {
        return Err((413, format!("request body exceeds {limit} bytes")));
    }
    let mut buf = Vec::new();
    req.as_reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| (400, format!("could not read body: {e}")))?;
    if buf.len() > limit {
        return Err((413, format!("request body exceeds {limit} bytes")));
    }
    String::from_utf8(buf).map_err(|_| (400, "request body is not UTF-8".to_string()))
}

/// `head`: markup put before `</head>` of index.html (the authoring app bar).
fn serve_asset(req: Request, path: &str, head: &str) {
    let asset = path.trim_start_matches('/');
    let asset = if asset.is_empty() {
        "index.html"
    } else {
        asset
    };
    // app-config.json is the static-hosting file: the extension takes options only.
    match WEB
        .get_file(asset)
        .filter(|_| asset != app_brand::STATIC_CONFIG)
    {
        Some(f) if asset == "index.html" && !head.is_empty() => {
            let html = String::from_utf8_lossy(f.contents());
            let html = html.replacen("</head>", &format!("{head}</head>"), 1);
            respond(req, 200, content_type(asset), html.into_bytes())
        }
        Some(f) => respond(req, 200, content_type(asset), f.contents().to_vec()),
        None => text(req, 404, "not found"),
    }
}

// -------------------------------------------------------- authoring mode ---

struct Authoring {
    ctx: Arc<DbCtx>,
    guard: AuthoringGuard,
    max_body: usize,
    /// `options` of `anofox_serve(port, options)`: the app bar's `<script>`.
    app: String,
}

/// `anofox_serve(port[, options])`: start the authoring server on
/// 127.0.0.1:`port`; `options` sets the builder's app bar (name, tagline,
/// logo, logo_dark, link, favicon — see `AppBrand`).
pub fn start_authoring(
    ctx: Arc<DbCtx>,
    port: u16,
    options: Option<&str>,
) -> Result<String, String> {
    let app = match options {
        Some(json) => AppBrand::from_json(json).map_err(|e| format!("options: {e}"))?,
        None => AppBrand::default(),
    };
    let mut servers = lock_servers();
    if servers.contains_key(&port) {
        return Err(format!(
            "anofox-visualization is already serving on port {port}"
        ));
    }
    // Authoring runs arbitrary SQL on the live session: loopback only, always.
    let addr = format!("127.0.0.1:{port}");
    let server = Arc::new(
        Server::http(&addr)
            .map_err(|e| format!("anofox-visualization: could not bind {addr}: {e}"))?,
    );
    let state = Arc::new(Authoring {
        ctx,
        guard: AuthoringGuard::new(port),
        max_body: 4 << 20,
        app: app.script(),
    });
    let url = state.guard.login_url("127.0.0.1");
    let stop = Arc::new(AtomicBool::new(false));
    let st = state.clone();
    // One worker: the live connection is used serially anyway.
    let workers = spawn_workers(
        &server,
        &stop,
        1,
        Arc::new(move |req| handle_authoring(&st, req)),
    );
    servers.insert(
        port,
        Running {
            kind: "authoring",
            server,
            stop,
            workers,
            refresher: None,
            locked: None,
            _state: Box::new(state),
        },
    );
    drop(servers);
    let _ = open::that(&url);
    Ok(format!(
        "anofox-visualization serving {url} (authoring: runs SQL on this session; loopback only; the URL carries the session token)"
    ))
}

fn handle_authoring(st: &Authoring, mut req: Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let m = meta(&req);
    // Token hand-over: /?token=… → cookie + redirect to the clean URL.
    if st.guard.check_origin(&m).is_ok() {
        if let Some((location, cookie)) = st.guard.bootstrap(&url) {
            let resp = Response::empty(303)
                .with_header(header("Location", &location))
                .with_header(header("Set-Cookie", &cookie))
                .with_header(header("Cache-Control", "no-store"));
            let _ = req.respond(resp);
            return;
        }
    }
    if let Err(d) = st.guard.check(&m) {
        return text(req, d.status(), d.message());
    }
    if path == "/query" {
        if req.method() != &Method::Post {
            return text(req, 405, "POST only");
        }
        let sql = match read_body(&mut req, st.max_body) {
            Ok(b) => b,
            Err((code, e)) => return text(req, code, &e),
        };
        let conn = st.ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        match authoring_query(&conn, &sql) {
            Ok(json) => respond(req, 200, "application/json", json.into_bytes()),
            Err(e) => text(req, 400, &e),
        }
        return;
    }
    if req.method() != &Method::Get {
        return text(req, 405, "method not allowed");
    }
    serve_asset(req, &path, &st.app);
}

/// Authoring `/query`: run the body on the live connection. Statements are
/// split by DuckDB's own parser; all but the last run for effect. The last one
/// is returned as JSON rows when it is a query (by DuckDB's statement type, so
/// `FROM t`, `VALUES`, `PIVOT`, `SUMMARIZE`, comment-led queries… all count),
/// otherwise it runs for effect and `[]` is returned.
fn authoring_query(conn: &Conn, sql: &str) -> Result<String, String> {
    let ex = conn.extract(sql)?;
    if ex.count == 0 {
        return Ok("[]".into());
    }
    for i in 0..ex.count - 1 {
        ex.prepare(conn, i)?.execute()?;
    }
    let last = ex.prepare(conn, ex.count - 1)?;
    let is_query =
        last.statement_type() == crate::ffi::duckdb_statement_type::DUCKDB_STATEMENT_TYPE_SELECT;
    if is_query {
        // We need the statement's text to wrap it; with one statement that is
        // the whole body. With several, recover it with the core splitter and
        // only trust it if it agrees with DuckDB's statement count.
        let text = if ex.count == 1 {
            Some(sql.to_string())
        } else {
            let clean = anofox_visualization::sql::strip_line_comments(sql);
            let parts: Vec<String> = anofox_visualization::sql::split_statements(&clean)
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .collect();
            (parts.len() == ex.count).then(|| parts[parts.len() - 1].clone())
        };
        if let Some(t) = text {
            // Try the text as-is, then without `--` comments (a trailing
            // `; -- note` would otherwise end up inside the wrapper).
            let stripped = anofox_visualization::sql::strip_line_comments(&t);
            for cand in [t.as_str(), stripped.as_str()] {
                match json_rows(
                    conn,
                    strip_trailing_semicolons(cand),
                    &[],
                    AUTHORING_MAX_ROWS,
                    &mut |_, want| Ok(want),
                ) {
                    Ok(json) => return Ok(json),
                    Err(e) if e.contains("row limit") => return Err(e),
                    Err(_) => {}
                }
            }
        }
    }
    last.execute()?;
    Ok("[]".into())
}

/// Row cap for authoring `/query` results (they all go to the browser).
const AUTHORING_MAX_ROWS: usize = 1_000_000;

fn strip_trailing_semicolons(s: &str) -> &str {
    s.trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
}

// ----------------------------------------------------------- locked mode ---

/// Tunables of a locked server (`options` JSON of anofox_serve_dashboards).
#[derive(Clone, Debug)]
pub struct LockedOptions {
    pub max_rows: usize,
    pub timeout: Duration,
    pub max_body: usize,
    pub threads: usize,
    /// Snapshot the session's other attached DuckDB databases too.
    pub attach: bool,
    /// Extra extensions to LOAD into the snapshot (before it is locked).
    pub load: Vec<String>,
    /// Default theme (a name, `a+b`, or a JSON object) for dashboards without
    /// their own `::THEME` — validated and frozen when the server starts.
    pub theme: Option<String>,
    /// Extra `Host` names accepted besides the loopback names (e.g. the public
    /// name a reverse proxy forwards); `"*"` disables the Host check.
    pub hosts: Vec<String>,
    /// Panel result cache size in bytes (`cache_mb`; 0 = off).
    pub cache_bytes: usize,
    /// Origins that may frame the pages (`frame_ancestors`, #252): the CSP
    /// `frame-ancestors`. Unset: `'none'` without an `embed` login, no
    /// restriction with one (embed links are meant to be framed).
    pub frame_ancestors: Option<Vec<String>>,
    /// Idle connections kept with a dashboard's setup already run (`warm`,
    /// #388; `None` = `threads`, 0 = off): a later panel request with the
    /// same setup inputs reuses one instead of re-running the setup.
    pub warm: Option<usize>,
    /// Rebuild the snapshot this often (`refresh_s`; `None` = never).
    pub refresh: Option<Duration>,
    /// A folder the panels may read (not write: see `check_read_only`) — the
    /// canonical path of `options.data`.
    pub data: Option<PathBuf>,
    /// Variables whose values name a file or folder inside `data`: each must
    /// be a plain name (`[A-Za-z0-9_.-]`, not starting with `.`).
    pub path_vars: Vec<String>,
    /// `auth`: the viewer login — an auth proxy, JWT/OIDC, embed links, API
    /// keys (every request but `/health` needs it; the claims become
    /// `__claim_*` variables).
    pub auth: Option<Arc<Auth>>,
    /// `log` (+ `log_vars`): JSON-lines request log.
    pub log: Option<Arc<RequestLog>>,
    /// Listen address (`bind`; default loopback). A specific address is also
    /// accepted as a `Host`.
    pub bind: std::net::IpAddr,
    /// DuckDB `memory_limit` of the snapshot instance (e.g. `"2GB"`).
    pub memory_limit: Option<String>,
    /// DuckDB `threads` of the snapshot instance, shared by all queries.
    pub duckdb_threads: Option<usize>,
    /// Row and column access policy (`policy`: a JSON file; `policy.json`
    /// next to the dashboards is used when the option is not given).
    pub policy: Option<Arc<Policy>>,
    /// `"policy": "none"`: serve without a policy although `auth` is set
    /// (otherwise that combination refuses to start).
    pub policy_none: bool,
    /// Key of the policy's `hash` mask (`policy_key_env`; random per server
    /// start when not given).
    pub policy_key: Option<PolicyKey>,
    /// Bytes of executed results held at once, until their responses are
    /// written (`max_inflight_mb`; [`crate::limits`]).
    pub max_inflight: usize,
    /// Bytes of one response (`max_response_mb`).
    pub max_response: usize,
    /// Panel queries running at once (`max_queries`); others wait.
    pub max_queries: usize,
    /// Where the panels' unqualified names resolve (`search_path`):
    /// `db[.schema]` entries, the first one `USE`d. Empty: the serving
    /// database's default database.
    pub search_path: Vec<(String, Option<String>)>,
    /// `app`: the installation's name, tagline, logo and favicon — the header
    /// of the `/` landing page (the app-bar settings of `anofox_serve`).
    pub app: Option<AppBrand>,
    /// `home`: a dashboard id `/` redirects to instead of the landing page
    /// (when the viewer may read it).
    pub home: Option<String>,
    /// `source`: build (part of) the snapshot from a live Quack server
    /// instead of (or alongside) the local session (#352 prototype) — see
    /// `duckext/src/source.rs` and docs/secure-serving.md, "Quack source".
    pub source: Option<Arc<crate::source::SourceOpts>>,
}

/// The `hash` mask's HMAC key. Never printed.
#[derive(Clone)]
pub struct PolicyKey(Arc<Vec<u8>>);

impl std::fmt::Debug for PolicyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PolicyKey(…)")
    }
}

impl Default for LockedOptions {
    fn default() -> Self {
        LockedOptions {
            max_rows: 100_000,
            timeout: Duration::from_secs(30),
            max_body: 64 << 10,
            threads: 4,
            attach: true,
            load: Vec::new(),
            theme: None,
            hosts: Vec::new(),
            cache_bytes: 64 << 20,
            warm: None,
            frame_ancestors: None,
            refresh: None,
            data: None,
            path_vars: Vec::new(),
            auth: None,
            log: None,
            bind: std::net::Ipv4Addr::LOCALHOST.into(),
            memory_limit: None,
            duckdb_threads: None,
            policy: None,
            policy_none: false,
            policy_key: None,
            max_inflight: 256 << 20,
            max_response: 32 << 20,
            max_queries: 4,
            search_path: Vec::new(),
            app: None,
            home: None,
            source: None,
        }
    }
}

impl LockedOptions {
    pub fn parse(json: Option<&str>) -> Result<LockedOptions, String> {
        let mut o = LockedOptions::default();
        let Some(json) = json.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(o);
        };
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("options: bad JSON: {e}"))?;
        let m = v.as_object().ok_or("options must be a JSON object")?;
        let (mut log, mut log_vars) = (None, None);
        let (mut auth_cfg, mut audit) = (None, None);
        for (k, val) in m {
            let num = || {
                val.as_u64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("options.{k} must be a positive integer"))
            };
            match k.as_str() {
                "max_rows" => o.max_rows = num()?.min(10_000_000) as usize,
                "timeout_ms" => o.timeout = Duration::from_millis(num()?),
                "max_body_bytes" => o.max_body = num()?.min(64 << 20) as usize,
                "threads" => o.threads = num()?.min(64) as usize,
                "frame_ancestors" => {
                    let list = val.as_array().ok_or("options.frame_ancestors must be an array of origins")?;
                    let mut out = Vec::new();
                    for o in list {
                        let o = o.as_str().ok_or("options.frame_ancestors must be an array of origins")?;
                        if !valid_frame_ancestor(o) {
                            return Err(format!("options.frame_ancestors: '{o}' is not an origin (https://host[:port], 'self' or 'none')"));
                        }
                        out.push(o.to_string());
                    }
                    o.frame_ancestors = Some(out);
                }
                "warm" => {
                    let n = val.as_u64().ok_or("options.warm must be a non-negative integer")?;
                    o.warm = Some(n.min(256) as usize);
                }
                "cache_mb" => {
                    let mb = val.as_u64().ok_or("options.cache_mb must be a non-negative integer")?;
                    o.cache_bytes = (mb.min(1 << 16) as usize) << 20;
                }
                "attach" => o.attach = val.as_bool().ok_or("options.attach must be a boolean")?,
                "load" => {
                    let arr = val.as_array().ok_or("options.load must be a list of extension names")?;
                    for e in arr {
                        let e = e.as_str().filter(|e| sv::valid_ident(e)).ok_or("options.load: invalid extension name")?;
                        o.load.push(e.to_string());
                    }
                }
                "theme" => {
                    let spec = match val {
                        serde_json::Value::String(t) => t.clone(),
                        serde_json::Value::Object(_) => val.to_string(),
                        _ => return Err("options.theme must be a theme name or a JSON object".into()),
                    };
                    anofox_visualization::theme::resolve(&spec).map_err(|e| format!("options.theme: {e}"))?;
                    o.theme = Some(spec);
                }
                "hosts" => {
                    let arr = val.as_array().ok_or("options.hosts must be a list of host names")?;
                    for h in arr {
                        let h = h
                            .as_str()
                            .map(str::trim)
                            .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || "-.:[]*".contains(c)))
                            .ok_or("options.hosts: invalid host name")?;
                        o.hosts.push(h.to_string());
                    }
                }
                "refresh_s" => o.refresh = Some(Duration::from_secs(num()?)),
                "data" => {
                    let d = val.as_str().filter(|d| !d.trim().is_empty()).ok_or("options.data must be a directory path")?;
                    // Canonical: DuckDB compares paths as text, and `..` or a
                    // symlink in the configured path would make that brittle.
                    let p = std::fs::canonicalize(d).map_err(|e| format!("options.data: '{d}': {e}"))?;
                    if !p.is_dir() {
                        return Err(format!("options.data: '{d}' is not a directory"));
                    }
                    if let Some(why) = too_broad(&p) {
                        return Err(format!("options.data: refusing '{}': {why} — point it at the folder that holds the data only", p.display()));
                    }
                    o.data = Some(p);
                }
                "path_vars" => {
                    let arr = val.as_array().ok_or("options.path_vars must be a list of variable names")?;
                    for v in arr {
                        let v = v.as_str().filter(|v| sv::valid_ident(v)).ok_or("options.path_vars: invalid variable name")?;
                        o.path_vars.push(v.to_string());
                    }
                }
                "auth" => auth_cfg = Some(val),
                "audit" => audit = Some(val.as_str().ok_or("options.audit must be \"stderr\" or a file path")?),
                "log" => log = Some(val.as_str().ok_or("options.log must be \"stderr\" or a file path")?),
                "log_vars" => log_vars = Some(val.as_str().ok_or("options.log_vars must be none, names or values")?),
                "bind" => {
                    o.bind = val
                        .as_str()
                        .and_then(|b| b.trim().trim_matches(['[', ']']).parse().ok())
                        .ok_or("options.bind must be an IP address (e.g. \"0.0.0.0\")")?;
                }
                "memory_limit" => {
                    let m = val
                        .as_str()
                        .map(str::trim)
                        .filter(|m| valid_memory_limit(m))
                        .ok_or("options.memory_limit must be a size like \"2GB\", \"512MiB\" or \"50%\"")?;
                    o.memory_limit = Some(m.to_string());
                }
                "duckdb_threads" => o.duckdb_threads = Some(num()?.min(1024) as usize),
                "policy" => {
                    let path = val.as_str().filter(|p| !p.trim().is_empty()).ok_or("options.policy must be the path of a policy file, or \"none\"")?;
                    if path == "none" {
                        o.policy_none = true;
                    } else {
                        o.policy = Some(Arc::new(read_policy(Path::new(path))?));
                    }
                }
                "policy_key_env" => {
                    let name = val.as_str().ok_or("options.policy_key_env must be the name of an environment variable")?;
                    let key = std::env::var(name).map_err(|_| format!("options.policy_key_env: environment variable '{name}' is not set"))?;
                    if key.len() < 32 {
                        return Err("options.policy_key_env: the key must be at least 32 characters (e.g. `openssl rand -hex 32`)".into());
                    }
                    o.policy_key = Some(PolicyKey(Arc::new(key.into_bytes())));
                }
                "max_inflight_mb" => o.max_inflight = (num()?.min(1 << 20) as usize) << 20,
                "max_response_mb" => o.max_response = (num()?.min(1 << 12) as usize) << 20,
                "max_queries" => o.max_queries = num()?.min(1024) as usize,
                "search_path" => {
                    let p = val.as_str().map(str::trim).ok_or("options.search_path must be a string like \"warehouse\" or \"warehouse.sales\"")?;
                    o.search_path = parse_search_path(p).map_err(|e| format!("options.search_path: {e}"))?;
                }
                "app" => {
                    if !val.is_object() {
                        return Err("options.app must be an object like {\"name\": \"…\", \"tagline\": \"…\", \"logo\": \"https://…\"}".into());
                    }
                    let a = AppBrand::from_json(&val.to_string()).map_err(|e| format!("options.app: {e}"))?;
                    a.check().map_err(|e| format!("options.app: {e}"))?;
                    o.app = Some(a);
                }
                "home" => {
                    let h = val.as_str().map(str::trim).filter(|h| sv::valid_dashboard_id(h)).ok_or("options.home must be a dashboard id (its file name without .sql)")?;
                    o.home = Some(h.to_string());
                }
                "source" => {
                    o.source = Some(Arc::new(crate::source::parse(val)?));
                }
                other => return Err(format!("options: unknown key '{other}' (max_rows, timeout_ms, max_body_bytes, threads, warm, frame_ancestors, cache_mb, attach, load, theme, hosts, refresh_s, data, path_vars, auth, audit, log, log_vars, bind, memory_limit, duckdb_threads, policy, policy_key_env, max_inflight_mb, max_response_mb, max_queries, search_path, app, home, source)")),
            }
        }
        // A response larger than the in-flight budget could never be sent.
        o.max_response = o
            .max_response
            .min(o.max_inflight.max(crate::limits::FREE_BYTES));
        if !o.path_vars.is_empty() && o.data.is_none() {
            return Err("options.path_vars needs options.data".into());
        }
        if let Some(val) = auth_cfg {
            let mut a = Auth::from_json(val, |k| std::env::var(k).ok())
                .map_err(|e| format!("options.{e}"))?;
            if let Some(path) = audit {
                a.set_audit(Arc::new(
                    AuditLog::new(path).map_err(|e| format!("options.{e}"))?,
                ));
            }
            o.auth = Some(Arc::new(a));
        } else if audit.is_some() {
            return Err("options.audit needs options.auth".into());
        }
        match (log, log_vars) {
            (Some(l), v) => {
                o.log = Some(Arc::new(
                    RequestLog::new(l, v).map_err(|e| format!("options.{e}"))?,
                ))
            }
            (None, Some(_)) => return Err("options.log_vars needs options.log".into()),
            (None, None) => {}
        }
        Ok(o)
    }
}

/// A DuckDB size: a positive number, then a unit (B, KB/KiB … TB/TiB, also
/// spelled out, any case) or `%` of RAM.
fn valid_memory_limit(m: &str) -> bool {
    let unit_at = m
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(m.len());
    let (num, unit) = m.split_at(unit_at);
    let unit = unit.trim().to_ascii_lowercase();
    num.parse::<f64>().is_ok_and(|n| n > 0.0)
        && matches!(
            unit.as_str(),
            "%" | "b"
                | "byte"
                | "bytes"
                | "kb"
                | "kib"
                | "kilobyte"
                | "kilobytes"
                | "mb"
                | "mib"
                | "megabyte"
                | "megabytes"
                | "gb"
                | "gib"
                | "gigabyte"
                | "gigabytes"
                | "tb"
                | "tib"
                | "terabyte"
                | "terabytes"
        )
        && (unit != "%" || num.parse::<f64>().is_ok_and(|n| n <= 100.0))
}

/// Read and parse an access policy file.
fn read_policy(path: &Path) -> Result<Policy, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("policy: cannot read '{}': {e}", path.display()))?;
    Policy::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `db[.schema][, db[.schema] …]` (plain identifiers) as (database, schema)
/// pairs.
fn parse_search_path(p: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let mut out = Vec::new();
    for e in p.split(',').map(str::trim) {
        let mut parts = e.split('.');
        let (db, schema) = (parts.next().unwrap_or(""), parts.next());
        if !sv::valid_ident(db)
            || schema.is_some_and(|s| !sv::valid_ident(s))
            || parts.next().is_some()
        {
            return Err(format!("'{e}' is not a database or database.schema name"));
        }
        out.push((db.to_string(), schema.map(str::to_string)));
    }
    if out.is_empty() || out.len() > 16 {
        return Err("expected 1 to 16 entries".into());
    }
    Ok(out)
}

struct Dash {
    id: String,
    sql: String,
    /// Planned statements: (is_setup, sql), indexed like the client's plan().
    stmts: Vec<(bool, String)>,
    /// Per statement: a `::PAGED` table with an `::INFO` column (the only
    /// column a page request may leave out).
    info: Vec<bool>,
    /// The header comments: title, `-- @vars` (the variables a viewer may
    /// set; others are dropped), group, tags, description, owner, certified.
    meta: sv::DashboardMeta,
    /// The day the file last changed (`YYYY-MM-DD`), for the landing page.
    updated: Option<String>,
    /// Each statement's own `-- @requires` (indexed like `stmts`).
    requires: Vec<Vec<Requirement>>,
    /// Per statement other than setup: the `::TAB` / `::SUBTAB` statements
    /// whose section it is in ([`anofox_visualization::sql::Panel::tabs`]).
    /// Their `-- @requires` gate it too, decided where each tab stands — as
    /// the page decides them — so a hidden tab's panels never run.
    tabs: Vec<Vec<usize>>,
}

/// A private, randomly named 0700 directory holding the snapshot files;
/// removed on Drop (i.e. when the server stops or startup fails).
struct SnapshotDir(PathBuf);

impl SnapshotDir {
    fn create() -> Result<SnapshotDir, String> {
        let base = std::env::temp_dir();
        for _ in 0..8 {
            let p = base.join(format!("anofox-snap-{}", sv::random_token()));
            let mut b = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            match b.create(&p) {
                Ok(()) => return Ok(SnapshotDir(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(format!(
                        "could not create snapshot dir {}: {e}",
                        p.display()
                    ))
                }
            }
        }
        Err("could not create a unique snapshot directory".into())
    }
}

impl Drop for SnapshotDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The read-only snapshot a locked server queries.
struct Snapshot {
    // Field order = drop order: disconnect the warm connections, close the
    // database, then delete its files.
    /// Idle connections with a dashboard's setup already run (#388).
    warm: WarmPool,
    db: Database,
    /// The database unqualified names resolve in (the source session's
    /// default database, or the first of `options.search_path`).
    default_db: String,
    /// What every request connection runs first: `USE` it, and set
    /// `options.search_path`.
    enter: String,
    /// The access policy, checked against this snapshot, and the dashboards
    /// rewritten for it (indexed like `Locked::dashboards`).
    policy: Option<Enforced>,
    /// Per dashboard (like `Locked::dashboards`), per statement: the
    /// request variables its answer can depend on (#210), for this
    /// snapshot's catalog (and, under a policy, its rewritten statements).
    deps: Vec<Vec<Deps>>,
    /// Per dashboard: a digest of `deps` (`window.__served.depsTag`, the
    /// `X-Anofox-Deps` response header).
    deps_tags: Vec<String>,
    /// Per dashboard, per statement: the request variables the setup before
    /// it reads ([`deps::setup_deps`]): what a warm connection is keyed on.
    warm_deps: Vec<Vec<Deps>>,
    _dir: SnapshotDir,
}

struct Enforced {
    enf: Enforcer,
    scripts: Vec<Script>,
}

/// Idle request connections on which a dashboard's setup already ran
/// (#388). The locked server runs a panel's setup for every panel request;
/// a materialising setup statement (`CREATE TEMP TABLE … AS`) made every
/// panel of a click rebuild it. A connection goes back here only after a
/// clean run, keyed by everything that shaped it ([`warm_key`]): the next
/// request with the same key gets a connection in the same state a fresh one
/// would reach, without the setup. Least recently used first out; the
/// snapshot owns the pool, so a refresh drops it.
struct WarmPool {
    idle: Mutex<VecDeque<Warm>>,
    cap: usize,
}

struct Warm {
    key: String,
    conn: Conn,
    /// The variables the last request `SET` on it (reset on reuse when the
    /// next request doesn't set them).
    set: Vec<String>,
}

impl WarmPool {
    fn new(cap: usize) -> WarmPool {
        WarmPool {
            idle: Mutex::new(VecDeque::new()),
            cap,
        }
    }
    fn take(&self, key: &str) -> Option<Warm> {
        let mut q = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let i = q.iter().rposition(|w| w.key == key)?;
        q.remove(i)
    }
    fn put(&self, w: Warm) {
        if self.cap == 0 {
            return;
        }
        let mut q = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        q.push_back(w);
        while q.len() > self.cap {
            q.pop_front();
        }
    }
}

/// Check the access policy against a new snapshot and rewrite every dashboard
/// for it. Fail closed: anything the policy refuses stops the start (or the
/// refresh — the previous snapshot then stays in service).
fn enforce(
    snap: &mut Snapshot,
    dashboards: &[Dash],
    policy: &Policy,
    key: &PolicyKey,
) -> Result<(), String> {
    let c = snap.db.connect()?;
    crate::register_policy_hash(&c, key.0.clone())?;
    let mut duck = |sql: &str| c.query(sql)?.string_rows(1_000_000);
    let enf = Enforcer::compile(policy, &mut duck, &snap.default_db).map_err(|e| {
        if e.contains("json_serialize_sql") {
            format!("{e} (the access policy needs the json extension: INSTALL json)")
        } else {
            e
        }
    })?;
    let mut scripts = Vec::new();
    for d in dashboards {
        scripts.push(enf.rewrite_script(&mut duck, &d.stmts).map_err(|(i, e)| {
            format!(
                "dashboard '{}' statement {i}: {e} (refused by the access policy)",
                d.id
            )
        })?);
    }
    snap.policy = Some(Enforced { enf, scripts });
    Ok(())
}

/// The catalog's macros and views with their definitions ([`deps::catalog_reads`]).
const CATALOG_SQL: &str =
    "SELECT function_name, coalesce(macro_definition, '') FROM duckdb_functions() \
     WHERE function_type IN ('macro', 'table_macro') \
     UNION ALL SELECT view_name, coalesce(sql, '') FROM duckdb_views()";

/// Work out every statement's variables ([`deps::script_deps`]) on a new
/// snapshot: which of its macros and views read variables comes from the
/// snapshot itself. Under a policy, the rewritten statements count too.
fn analyze_deps(snap: &mut Snapshot, dashboards: &[Dash]) -> Result<(), String> {
    let c = snap.db.connect()?;
    let objects: Vec<(String, String)> = c
        .query(CATALOG_SQL)?
        .string_rows(10_000_000)?
        .into_iter()
        .filter_map(|r| {
            let name = r.first()?.clone()?;
            Some((name, r.get(1).cloned().flatten().unwrap_or_default()))
        })
        .collect();
    let implicit = deps::catalog_reads(&objects);
    let (mut all, mut tags, mut warm) = (Vec::new(), Vec::new(), Vec::new());
    for (di, d) in dashboards.iter().enumerate() {
        let of = |stmts: &[(bool, String)], f: fn(&[deps::Stmt], &_) -> Vec<Deps>| {
            let view: Vec<deps::Stmt> = stmts
                .iter()
                .enumerate()
                .map(|(i, (setup, sql))| deps::Stmt {
                    setup: *setup,
                    sql,
                    requires: d.requires.get(i).map_or(&[][..], Vec::as_slice),
                    tabs: d.tabs.get(i).map_or(&[][..], Vec::as_slice),
                })
                .collect();
            f(&view, &implicit)
        };
        let mut ds = of(&d.stmts, deps::script_deps);
        let mut ws = of(&d.stmts, deps::setup_deps);
        if let Some(p) = &snap.policy {
            for (a, b) in ds
                .iter_mut()
                .zip(of(&p.scripts[di].stmts, deps::script_deps))
            {
                a.extend(&b);
            }
            for (a, b) in ws
                .iter_mut()
                .zip(of(&p.scripts[di].stmts, deps::setup_deps))
            {
                a.extend(&b);
            }
        }
        tags.push(deps_tag(&ds));
        all.push(ds);
        warm.push(ws);
    }
    snap.deps = all;
    snap.deps_tags = tags;
    snap.warm_deps = warm;
    Ok(())
}

fn deps_tag(ds: &[Deps]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{ds:?}").hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Extra time a Quack source build gets beyond `options.source.timeout_s`
/// before the caller gives up on it (security review M1): `timeout_s` is
/// the deadline the build's own internal watchdog uses to try a remote
/// `quack_cancel`; this grace gives that attempt a chance to land before
/// the caller stops waiting.
const SOURCE_BUILD_GRACE: Duration = Duration::from_secs(5);

/// Process-wide cap on Quack source build threads running at once, counting
/// both ones a caller is still waiting on and abandoned ones left running in
/// the background (security review R2): a repeatedly-retried failed startup
/// against a dead or hostile source otherwise leaks one staging instance's
/// threads (~100 of them, observed) per attempt, without limit. Shared by
/// every locked server in this process — `start_locked`'s *startup* build
/// has no persistent per-server state to track its own abandonment across
/// calls, so only a global cap bounds it.
const MAX_CONCURRENT_SOURCE_BUILDS: usize = 2;
static GLOBAL_SOURCE_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Per-server Quack-build bookkeeping for `/healthz`'s `build_overdue`
/// (security review R1): `in_flight` is true from just before the build
/// thread is spawned until that thread — including a drop guard that runs
/// even if it panics — clears it; `deadline` is the instant by which it
/// should have returned. "Overdue" is `in_flight` true and `deadline`
/// passed: no separate "abandoned" boolean that a panic could leave stuck
/// (the review's R1: the old design cleared it only via an explicit
/// `store(false, ..)` on the thread's *normal* path, so a panic — or losing
/// the race against the caller's own timeout — left it set forever).
struct SourceBuildState {
    in_flight: AtomicBool,
    deadline: Mutex<Option<Instant>>,
}

impl SourceBuildState {
    fn new() -> Arc<Self> {
        Arc::new(SourceBuildState {
            in_flight: AtomicBool::new(false),
            deadline: Mutex::new(None),
        })
    }

    fn overdue(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
            && self
                .deadline
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some_and(|d| Instant::now() > d)
    }
}

/// Releases this process's global build slot on drop — including when the
/// build thread panics (security review R1/R2).
struct ReleaseSlotOnDrop;
impl Drop for ReleaseSlotOnDrop {
    fn drop(&mut self) {
        GLOBAL_SOURCE_BUILDS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Run `src`'s Quack copy on a detached thread, never touching `ctx.conn`
/// (security review M1, fix 1) and never blocking the caller for longer than
/// `src.timeout + SOURCE_BUILD_GRACE` (fix 2) — covering `LOAD`, the remote
/// `ATTACH` and teardown too, not only the data copy the build's own
/// watchdog already bounds. `stop` is the *server's* stop flag, passed
/// straight through as the build's cancel signal: `anofox_serve_stop`
/// setting it cancels a build in flight immediately, with no separate
/// per-build flag to race against (security review I2 — a fresh flag
/// created only when a build starts has a window, right before it's
/// created, where an already-in-flight `stop` would be missed).
///
/// `state.in_flight` tolerates at most one build running past its deadline
/// per server: while it's set, a new attempt fails fast instead of piling
/// up threads, and a drop guard *inside* the thread clears it — on a normal
/// return or a panic alike (R1) — whenever it finally finishes (fix 2's
/// "left to finish, the slot stays busy"). `GLOBAL_SOURCE_BUILDS` additionally
/// caps how many such threads can exist across the whole process (R2).
fn run_source_build(
    src: &Arc<crate::source::SourceOpts>,
    dir: &Path,
    stop: &Arc<AtomicBool>,
    state: &Arc<SourceBuildState>,
) -> Result<(String, PathBuf, Vec<String>), String> {
    if state.in_flight.swap(true, Ordering::SeqCst) {
        return Err(
            "options.source: a previous build is still running past its timeout_s — refusing a \
             new attempt until it finishes"
                .into(),
        );
    }
    // The in_flight flag is this server's own; from here on a failure to
    // proceed must clear it again before returning.
    if GLOBAL_SOURCE_BUILDS.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT_SOURCE_BUILDS {
        GLOBAL_SOURCE_BUILDS.fetch_sub(1, Ordering::SeqCst);
        state.in_flight.store(false, Ordering::SeqCst);
        return Err(format!(
            "options.source: {MAX_CONCURRENT_SOURCE_BUILDS} Quack source builds are already \
             running or abandoned across this process — refusing a new attempt until one finishes"
        ));
    }
    let budget = src.timeout.saturating_add(SOURCE_BUILD_GRACE);
    *state.deadline.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now() + budget);
    let (tx, rx) = mpsc::channel();
    let (src, dir, stop, state_for_thread) =
        (src.clone(), dir.to_path_buf(), stop.clone(), state.clone());
    // #356 C2: both release guards are created HERE, before the thread
    // exists, and only then moved into its closure (not constructed inside
    // the closure body once it's already running). `Builder::spawn`
    // returns a `Result` instead of panicking on a failed OS thread
    // creation, so if it fails, the closure — and the guards captured in
    // it — are simply dropped back in THIS thread, still releasing the
    // in-flight marker and the global slot. With the old
    // `std::thread::spawn` plus guards constructed only once the new
    // thread's body started running, a failed spawn left both stuck
    // forever (nothing to ever drop them).
    let release_slot = ReleaseSlotOnDrop;
    let clear_in_flight = ClearInFlightOnDrop(state_for_thread);
    if let Err(e) = std::thread::Builder::new().spawn(move || {
        let _release_slot = release_slot;
        let _clear_in_flight = clear_in_flight;
        let r = crate::source::copy_from_quack(&dir, &src, &stop);
        let _ = tx.send(r);
    }) {
        return Err(format!(
            "options.source: could not start the build thread: {e}"
        ));
    }
    match rx.recv_timeout(budget) {
        // The thread is finishing up (or already has) — its own drop guard
        // clears `in_flight` and releases the global slot; nothing to do here.
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "options.source: build timed out after {:.0}s (timeout_s + a {:.0}s grace) — \
             it may still be running in the background and has been left to finish",
            budget.as_secs_f64(),
            SOURCE_BUILD_GRACE.as_secs_f64()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(
            "options.source: the build thread ended unexpectedly (likely a panic) before \
             returning a result"
                .into(),
        ),
    }
}

/// Clears `state.in_flight` on drop — including when the build thread
/// panics (security review R1).
struct ClearInFlightOnDrop(Arc<SourceBuildState>);
impl Drop for ClearInFlightOnDrop {
    fn drop(&mut self) {
        self.0.in_flight.store(false, Ordering::SeqCst);
    }
}

/// Join every handle, but never for longer than `budget` in total (security
/// review M1, fix 3): a worker — most likely the `refresh_s` thread, which
/// can be in the middle of a Quack build — that is still running when the
/// budget runs out is left to finish on its own (its `JoinHandle` is simply
/// dropped, not joined) instead of blocking `anofox_serve_stop`.
fn join_bounded(handles: Vec<JoinHandle<()>>, budget: Duration) {
    let (tx, rx) = mpsc::channel();
    let n = handles.len();
    for h in handles {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = h.join();
            let _ = tx.send(());
        });
    }
    let deadline = Instant::now() + budget;
    for _ in 0..n {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if rx.recv_timeout(remaining).is_err() {
            break;
        }
    }
}

struct Locked {
    port: u16,
    dashboards: Vec<Dash>,
    opts: LockedOptions,
    /// What a (re)build snapshots: the serving database's context and the
    /// extensions to load into the snapshot.
    ctx: Arc<DbCtx>,
    loads: Vec<String>,
    /// The snapshot (+ its result cache and epoch) new requests run on.
    current: Current<Snapshot>,
    /// Random per server; qualifies the ETags (with the epoch).
    server_id: String,
    /// Memory bounds: results in flight, queries at once.
    inflight: Inflight,
    queries: Queries,
    /// Recent `-- @requires` answers (see [`requires_met`]).
    requires_cache: rq::ExistsCache,
    /// The server's own stop flag (shared with `Running.stop`): also used as
    /// a Quack build's cancel signal (security review I2 — see
    /// [`run_source_build`]).
    stop: Arc<AtomicBool>,
    /// `options.source` (#352 M1/R1): this server's Quack-build bookkeeping
    /// for `/healthz`'s `build_overdue`.
    source_build: Arc<SourceBuildState>,
}

// `anofox_data`: the `SET VARIABLE` that tells panels where `options.data`
// is. Viewers can never set it (`access::is_reserved_var`, any request).
use anofox_visualization::host::access::DATA_VAR;

impl Locked {
    fn build(&self) -> Result<(Snapshot, Vec<String>), String> {
        // #352 M1: the Quack copy runs BEFORE ctx.conn is taken (it never
        // touches it) and is bounded, so a source that stops answering can
        // no longer stall every other locked server, the authoring
        // console, or a new `anofox_serve_dashboards` sharing this host
        // database's connection.
        let dir = SnapshotDir::create()?;
        let source_result = match &self.opts.source {
            Some(src) => Some(run_source_build(
                src,
                &dir.0,
                &self.stop,
                &self.source_build,
            )?),
            None => None,
        };
        let (mut snap, warnings) = {
            let conn = self.ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
            build_snapshot(&conn, &self.opts, &self.loads, dir, source_result)?
        };
        if let (Some(p), Some(k)) = (&self.opts.policy, &self.opts.policy_key) {
            enforce(&mut snap, &self.dashboards, p, k)?;
        }
        analyze_deps(&mut snap, &self.dashboards)?;
        Ok((snap, warnings))
    }

    /// Rebuild the snapshot and swap it in (`wait`: see [`Current::rebuild`]).
    fn refresh(&self, wait: bool) -> Result<Option<String>, String> {
        let t0 = Instant::now();
        let mut warnings = Vec::new();
        let gen = self
            .current
            .rebuild(wait, || {
                let (snap, w) = self.build()?;
                warnings = w;
                Ok(snap)
            })
            .map_err(|e| {
                eprintln!("[anofox-serve] snapshot refresh failed (still serving the previous one): {e}");
                format!("anofox-visualization: refresh failed, still serving the previous snapshot: {e}")
            })?;
        let Some(gen) = gen else {
            return Ok(None);
        };
        let warn = if warnings.is_empty() {
            String::new()
        } else {
            format!(" (warnings: {})", warnings.join("; "))
        };
        let msg = format!(
            "anofox-visualization: port {} now serves snapshot epoch {} (built in {} ms){warn}",
            self.port,
            gen.epoch,
            t0.elapsed().as_millis()
        );
        eprintln!("[anofox-serve] {msg}");
        Ok(Some(msg))
    }
}

/// `anofox_serve_dashboards(dir, port[, options])`.
pub fn start_locked(
    ctx: Arc<DbCtx>,
    dir: &str,
    port: u16,
    mut opts: LockedOptions,
) -> Result<String, String> {
    let mut servers = lock_servers();
    if servers.contains_key(&port) {
        return Err(format!(
            "anofox-visualization is already serving on port {port}"
        ));
    }
    let (dashboards, mut loads) = load_dashboards(Path::new(dir))?;
    if let Some(h) = &opts.home {
        if !dashboards.iter().any(|d| &d.id == h) {
            return Err(format!(
                "anofox-visualization: options.home: no dashboard '{h}' in '{dir}'"
            ));
        }
    }
    loads.extend(opts.load.iter().cloned());
    // A policy file next to the dashboards is enforced even without the
    // option: forgetting the option must not serve every row.
    if opts.policy.is_none() {
        let p = Path::new(dir).join("policy.json");
        if p.exists() {
            opts.policy = Some(Arc::new(read_policy(&p)?));
        }
    }
    if opts.policy.is_some() {
        // The policy resolves unqualified names in one database's `main`
        // schema: a search path must not send them anywhere else.
        if opts.search_path.len() > 1
            || opts
                .search_path
                .iter()
                .any(|(_, s)| s.as_ref().is_some_and(|s| !s.eq_ignore_ascii_case("main")))
        {
            return Err("anofox-visualization: with an access policy, options.search_path must name one database (its main schema)".into());
        }
        // json_serialize_sql: the policy rewrite parses with DuckDB.
        loads.push("json".into());
        if opts.policy_key.is_none() {
            let k = format!("{}{}", sv::random_token(), sv::random_token());
            opts.policy_key = Some(PolicyKey(Arc::new(k.into_bytes())));
        }
    } else if opts.auth.is_some() && !opts.policy_none {
        // A login without a policy relies on every panel filtering on the
        // claims; a policy.json deleted from the dashboard folder must not
        // silently get there.
        return Err("anofox-visualization: `auth` is set but no access policy was found (options.policy, or policy.json in the dashboard folder) — pass \"policy\": \"none\" to serve with per-panel claim filters only".into());
    }
    // The startup build must succeed (fail closed, #352) — and, like every
    // other build, is bounded (M1): the server isn't registered yet, so
    // nothing can call `anofox_serve_stop` on it; a throwaway stop flag
    // (never set) is enough here, the Quack copy's own `timeout_s` still
    // applies.
    let startup_dir = SnapshotDir::create()?;
    let startup_stop = Arc::new(AtomicBool::new(false));
    let startup_build_state = SourceBuildState::new();
    let source_result = match &opts.source {
        Some(src) => Some(run_source_build(
            src,
            &startup_dir.0,
            &startup_stop,
            &startup_build_state,
        )?),
        None => None,
    };
    let (mut snap, warnings) = {
        let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        build_snapshot(&conn, &opts, &loads, startup_dir, source_result)?
    };
    if let (Some(p), Some(k)) = (&opts.policy, &opts.policy_key) {
        enforce(&mut snap, &dashboards, p, k)?;
    }
    analyze_deps(&mut snap, &dashboards)?;
    let policy_note = match (&opts.policy, &opts.auth) {
        (Some(p), Some(_)) => format!(
            "; access policy on {} table(s) — every other table is refused",
            p.tables.len()
        ),
        (Some(p), None) => format!(
            "; access policy on {} table(s) — every other table is refused; WARNING: no auth, so every viewer is anonymous (no claims)",
            p.tables.len()
        ),
        (None, Some(_)) => "; no access policy (\"policy\": \"none\") — panels must filter on the claims themselves".into(),
        (None, None) => String::new(),
    };
    // Validate every statement with DuckDB's parser: exactly one each.
    {
        let c = snap.db.connect()?;
        for d in &dashboards {
            for (i, (_, sql)) in d.stmts.iter().enumerate() {
                match c.count_statements(sql) {
                    Ok(1) => {}
                    Ok(n) => {
                        return Err(format!(
                            "dashboard '{}' statement {i}: expected 1 statement, found {n}",
                            d.id
                        ))
                    }
                    Err(e) => return Err(format!("dashboard '{}' statement {i}: {e}", d.id)),
                }
            }
        }
        if opts.data.is_some() {
            check_read_only(&c, &dashboards)?;
        }
    }
    let addr = std::net::SocketAddr::new(opts.bind, port);
    if !opts.bind.is_unspecified() && !opts.bind.is_loopback() {
        // Requests addressed to the bound IP itself (health checks) pass the
        // Host check.
        opts.hosts.push(match opts.bind {
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
            ip => ip.to_string(),
        });
    }
    let server = Arc::new(
        Server::http(addr)
            .map_err(|e| format!("anofox-visualization: could not bind {addr}: {e}"))?,
    );
    let n = dashboards.len();
    let threads = opts.threads;
    let refresh = opts.refresh;
    let (inflight, queries) = (
        Inflight::new(opts.max_inflight),
        Queries::new(opts.max_queries),
    );
    // Created before `Locked` (security review I2): `Locked.stop` shares
    // this same flag, so a Quack build's cancel signal always exists and a
    // `stop()` racing a build's own startup can never find it missing.
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Locked {
        port,
        dashboards,
        current: Current::new(snap, opts.cache_bytes),
        opts,
        ctx,
        loads,
        server_id: sv::random_token()[..8].to_string(),
        inflight,
        queries,
        // A few seconds: files appear live, a rescan per request is avoided.
        requires_cache: rq::ExistsCache::new(Duration::from_secs(5), 4096),
        stop: stop.clone(),
        source_build: SourceBuildState::new(),
    });
    let auth_note = match &state.opts.auth {
        Some(a) => {
            let warn = a.prefetch();
            format!(
                "; login: {}{}",
                a.describe(),
                if warn.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (not reachable yet, requests fail closed until it is: {})",
                        warn.join("; ")
                    )
                }
            )
        }
        None if !state.opts.bind.is_loopback() => format!(
            "; WARNING: bound to {} without auth — every viewer who can reach this address sees every dashboard; put an auth proxy in front",
            state.opts.bind
        ),
        None => String::new(),
    };
    let st = state.clone();
    let workers = spawn_workers(
        &server,
        &stop,
        threads,
        Arc::new(move |req| handle_locked(&st, req)),
    );
    // #356 C1: the refresh_s thread is tracked separately from the HTTP
    // workers (see `Running::refresher`) so `stop()` can fully join the
    // latter while only budget-capping this one.
    let refresher_handle = refresh.map(|every| {
        let (st, stop) = (state.clone(), stop.clone());
        std::thread::spawn(move || refresher(&st, &stop, every))
    });
    servers.insert(
        port,
        Running {
            kind: "locked",
            server,
            stop,
            workers,
            refresher: refresher_handle,
            locked: Some(state.clone()),
            _state: Box::new(state),
        },
    );
    let warn = if warnings.is_empty() {
        String::new()
    } else {
        format!(" (warnings: {})", warnings.join("; "))
    };
    Ok(format!(
        "anofox-visualization serving {n} locked, read-only dashboard(s) at http://{addr}/ — server owns the SQL{auth_note}{policy_note}{warn}"
    ))
}

/// The `refresh_s` thread: rebuild every `every`, skipping a turn while an
/// on-demand refresh runs; stops with the server.
fn refresher(st: &Locked, stop: &AtomicBool, every: Duration) {
    let mut next = Instant::now() + every;
    loop {
        while Instant::now() < next {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100).min(every));
        }
        let _ = st.refresh(false);
        next = Instant::now() + every;
    }
}

/// `anofox_serve_refresh(port)`: rebuild the snapshot of the locked server on
/// `port` now (waiting for a refresh already running), and swap it in.
/// Requests already running finish on the old snapshot.
pub fn refresh(port: u16) -> Result<String, String> {
    let locked = match lock_servers().get(&port) {
        Some(r) => r.locked.clone().ok_or_else(|| {
            format!(
                "anofox-visualization: the server on port {port} is not a locked dashboard server"
            )
        })?,
        None => {
            return Err(format!(
                "anofox-visualization: nothing is serving on port {port}"
            ))
        }
    };
    Ok(locked.refresh(true)?.unwrap_or_default())
}

/// Why `dir` is too broad to open to every viewer as `options.data`: the
/// filesystem root, a system directory, or a folder that contains the home
/// directory or the temp directory (where the snapshots live).
fn too_broad(dir: &Path) -> Option<String> {
    let Some(parent) = dir.parent() else {
        return Some("the filesystem root".into());
    };
    const SYSTEM: [&str; 16] = [
        "bin", "boot", "dev", "etc", "home", "lib", "lib64", "opt", "proc", "root", "run", "sbin",
        "sys", "tmp", "usr", "var",
    ];
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if parent.parent().is_none() && SYSTEM.contains(&name) {
        return Some("a system directory".into());
    }
    let home = ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from);
    for (what, p) in home
        .map(|p| ("the home directory", p))
        .chain([("the temp directory", std::env::temp_dir())])
    {
        if p.canonicalize().is_ok_and(|p| p.starts_with(dir)) {
            return Some(format!("it contains {what}"));
        }
    }
    None
}

/// With a data folder open, every dashboard statement must be a query or one
/// of the setup forms the server needs — classified by DuckDB's own parser,
/// the same way the access policy does ([`policy::statement_query`],
/// [`policy::check_query`]) — because DuckDB can write inside
/// `allowed_directories` too (`COPY … TO`, and anything that runs a
/// statement: `EXPLAIN ANALYZE`, `PREPARE`/`EXECUTE`, `CALL`, `PRAGMA`, …).
/// An allowlist, so new statement kinds are refused.
fn check_read_only(c: &Conn, dashboards: &[Dash]) -> Result<(), String> {
    let mut duck = |sql: &str| {
        c.query(sql)?.string_rows(1).map_err(|e| {
            format!(
                "checking the dashboards for a data folder needs DuckDB's json extension: {}",
                e.lines().next().unwrap_or("")
            )
        })
    };
    for d in dashboards {
        for (i, (setup, sql)) in d.stmts.iter().enumerate() {
            read_only_statement(sql, *setup, &mut duck)
                .map_err(|e| format!("dashboard '{}' statement {i}: {e}", d.id))?;
        }
    }
    Ok(())
}

/// `Ok` if `sql` (a panel, or a setup statement if `setup`) may run with a data
/// folder open: a panel must be one query; a setup statement may also be
/// `SET VARIABLE <name> = <expr>` or `CREATE [OR REPLACE] TEMP TABLE|VIEW
/// <name> AS <query>`, whose expression / query must parse as one query.
fn read_only_statement(sql: &str, setup: bool, duck: &mut dyn policy::Duck) -> Result<(), String> {
    let query = if setup {
        policy::statement_query(sql).map_err(|e| format!("with a data folder: {e}"))?
    } else {
        sql.to_string()
    };
    policy::check_query(duck, &query)
        .map_err(|e| format!("with a data folder, only queries (SELECT …) are allowed: {e}"))
}

/// A DuckDB error as a viewer sees it: the data folder's absolute path is
/// replaced by `<data>` (the full text goes to the server log).
fn redact(e: String, data: Option<&Path>) -> String {
    match data.map(|d| d.to_string_lossy()) {
        Some(d) if e.contains(d.as_ref()) => {
            eprintln!("[anofox-serve] (unredacted) {e}");
            e.replace(d.as_ref(), "<data>")
        }
        _ => e,
    }
}

/// Is `lit` (a typed literal from `sv::sql_literal`) an acceptable value for a
/// `path_vars` variable: a string or integer that is one plain path segment —
/// `[A-Za-z0-9_.-]`, not starting with `.` (so never `.`, `..` or a hidden
/// file), no `/` — or NULL?
fn valid_path_value(lit: &str) -> bool {
    if lit == "NULL" || (!lit.is_empty() && lit.bytes().all(|b| b.is_ascii_digit())) {
        return true;
    }
    lit.strip_prefix('\'')
        .and_then(|l| l.strip_suffix('\''))
        .is_some_and(sv::valid_dashboard_id)
}

/// Load `*.sql` dashboards from `dir`. Returns them plus the union of their
/// `-- @load` extensions.
fn load_dashboards(dir: &Path) -> Result<(Vec<Dash>, Vec<String>), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read dashboard dir '{}': {e}", dir.display()))?;
    let mut out = Vec::new();
    let mut loads = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        if !sv::valid_dashboard_id(&id) {
            continue; // ids end up in URLs: only [A-Za-z0-9_.-]
        }
        let sql =
            std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
        let meta = sv::parse_dashboard_meta(&id, &sql);
        loads.extend(meta.loads.iter().cloned());
        let planned = anofox_visualization::sql::plan(&sql);
        let info = planned
            .iter()
            .map(|p| {
                use anofox_visualization::Role;
                let has = |want: Role| p.roles.iter().any(|(_, r)| *r == want);
                has(Role::PagedTable) && has(Role::Info)
            })
            .collect();
        // A statement in a ::TAB / ::SUBTAB section runs under the tab's
        // `-- @requires` too (setup only under its own): a hidden tab's panels
        // never run, also when a viewer asks for them directly.
        let tabs = planned
            .iter()
            .map(|p| if p.setup { Vec::new() } else { p.tabs.clone() })
            .collect();
        let (stmts, requires) = planned
            .into_iter()
            .map(|p| ((p.setup, p.sql.trim().to_string()), p.requires))
            .unzip();
        out.push(Dash {
            id,
            sql,
            stmts,
            info,
            meta,
            updated: sv::file_date(&p),
            requires,
            tabs,
        });
    }
    if out.is_empty() {
        return Err(format!("no .sql dashboards found in '{}'", dir.display()));
    }
    // #301: ascending `-- @order`, unordered dashboards after, then by id.
    out.sort_by(|a, b| a.meta.order_key(&a.id).cmp(&b.meta.order_key(&b.id)));
    loads.sort();
    loads.dedup();
    Ok((out, loads))
}

/// Copy the DuckDB database `src` into the empty, attached `dst` (#283).
/// Without foreign keys: `COPY FROM DATABASE`. With them, DuckDB inserts every
/// table in one plan and a child's FK check doesn't see its parent's rows
/// (duckdb/duckdb#16785), so: the schema (`… (SCHEMA)`), then one INSERT per
/// table, parents first. A table whose INSERT still fails (a self-referencing
/// FK: DuckDB doesn't see rows of the same statement) is re-created without
/// constraints (CREATE TABLE AS) if no other table references it.
/// Every write names `dst` (catalog-qualified); `src` is only read.
fn copy_database(
    live: &Conn,
    src: &str,
    dst: &str,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let lit = sql_string_literal(src);
    let edges: Vec<(String, String, String)> = live
        .query(&format!(
            "SELECT schema_name::VARCHAR, table_name::VARCHAR, referenced_table::VARCHAR \
             FROM duckdb_constraints() WHERE database_name = {lit} AND constraint_type = 'FOREIGN KEY'"
        ))?
        .string_rows(100_000)?
        .into_iter()
        .filter_map(|r| Some((r[0].clone()?, r[1].clone()?, r[2].clone()?)))
        .collect();
    if edges.is_empty() {
        return live.exec(&format!(
            "COPY FROM DATABASE {} TO {}",
            quote_ident(src),
            quote_ident(dst)
        ));
    }
    live.exec(&format!(
        "COPY FROM DATABASE {} TO {} (SCHEMA)",
        quote_ident(src),
        quote_ident(dst)
    ))?;
    let tables: Vec<(String, String)> = live
        .query(&format!(
            "SELECT schema_name::VARCHAR, table_name::VARCHAR FROM duckdb_tables() \
             WHERE database_name = {lit} AND NOT internal AND NOT temporary ORDER BY 1, 2"
        ))?
        .string_rows(100_000)?
        .into_iter()
        .filter_map(|r| Some((r[0].clone()?, r[1].clone()?)))
        .collect();
    // #299: one duckdb_columns() scan for the whole database, grouped here,
    // instead of one scan per table (while `live`'s lock is held) — with
    // 3,000 FK tables that turned the loop below into ~3,000 full scans of
    // the database's column catalog.
    let columns = columns_by_table(live, src)?;
    let empty = Vec::new();
    for i in fk_order(&tables, &edges) {
        let (schema, table) = &tables[i];
        let qual = |db: &str| {
            format!(
                "{}.{}.{}",
                quote_ident(db),
                quote_ident(schema),
                quote_ident(table)
            )
        };
        let (from, to) = (qual(src), qual(dst));
        let cols = insertable_columns(
            live,
            &to,
            columns
                .get(&(schema.clone(), table.clone()))
                .unwrap_or(&empty),
        )?
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
        let Err(e) = live.exec(&format!(
            "INSERT INTO {to} ({cols}) SELECT {cols} FROM {from}"
        )) else {
            continue;
        };
        let referenced = edges.iter().any(|(s, c, p)| {
            s.eq_ignore_ascii_case(schema)
                && p.eq_ignore_ascii_case(table)
                && !c.eq_ignore_ascii_case(table)
        });
        let first = e.lines().next().unwrap_or("").to_string();
        if referenced {
            return Err(format!("{schema}.{table}: {first}"));
        }
        live.exec(&format!("DROP TABLE {to}"))
            .and_then(|_| live.exec(&format!("CREATE TABLE {to} AS SELECT * FROM {from}")))
            .map_err(|e2| {
                format!("{schema}.{table}: {first} (and without its constraints: {e2})")
            })?;
        warnings.push(format!(
            "table {schema}.{table} of '{src}' copied without its constraints and indexes: {first}"
        ));
    }
    Ok(())
}

/// `(schema, table) -> [(column, has_default), …]`, a table's own columns in
/// `column_index` order — what [`columns_by_table`] groups a database's
/// `duckdb_columns()` scan into.
pub(crate) type ColumnsByTable = BTreeMap<(String, String), Vec<(String, bool)>>;

/// Every table's columns of database `src`, read in one `duckdb_columns()`
/// scan (#299) instead of one scan per table.
pub(crate) fn columns_by_table(live: &Conn, src: &str) -> Result<ColumnsByTable, String> {
    let rows = live
        .query(&format!(
            "SELECT schema_name::VARCHAR, table_name::VARCHAR, column_name::VARCHAR, \
             (column_default IS NOT NULL)::VARCHAR FROM duckdb_columns() \
             WHERE database_name = {} ORDER BY schema_name, table_name, column_index",
            sql_string_literal(src)
        ))?
        .string_rows(1_000_000)?;
    let mut out: ColumnsByTable = BTreeMap::new();
    for r in rows {
        let (Some(schema), Some(table), Some(col)) = (r[0].clone(), r[1].clone(), r[2].clone())
        else {
            continue;
        };
        let has_default = r[3].as_deref() == Some("true");
        out.entry((schema, table))
            .or_default()
            .push((col, has_default));
    }
    Ok(out)
}

/// The columns of `columns` (a table's own `(column, has_default)` list, from
/// [`columns_by_table`]) an INSERT into `to` can name: all but generated
/// ones. duckdb_columns() doesn't mark those, so a column with a default is
/// tried on its own: `INSERT … SELECT NULL WHERE false` binds (and fails for
/// a generated column) but writes nothing.
pub(crate) fn insertable_columns(
    live: &Conn,
    to: &str,
    columns: &[(String, bool)],
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (c, has_default) in columns {
        if *has_default
            && live
                .exec(&format!(
                    "INSERT INTO {to} ({}) SELECT NULL WHERE false",
                    quote_ident(c)
                ))
                .is_err()
        {
            continue;
        }
        out.push(c.clone());
    }
    Ok(out)
}

/// The order to fill `tables` in (#283): each after the tables its foreign
/// keys reference (`edges`: schema, child, parent; DuckDB keeps an FK within
/// one schema). Self-references are ignored; ties, and a cycle, go in the
/// given order. Names match case-insensitively, like DuckDB's.
pub(crate) fn fk_order(
    tables: &[(String, String)],
    edges: &[(String, String, String)],
) -> Vec<usize> {
    let key = |s: &str, t: &str| (s.to_lowercase(), t.to_lowercase());
    let idx: BTreeMap<(String, String), usize> = tables
        .iter()
        .enumerate()
        .map(|(i, (s, t))| (key(s, t), i))
        .collect();
    let mut parents = vec![Vec::new(); tables.len()];
    for (s, c, p) in edges {
        if let (Some(&ci), Some(&pi)) = (idx.get(&key(s, c)), idx.get(&key(s, p))) {
            if ci != pi {
                parents[ci].push(pi);
            }
        }
    }
    let mut done = vec![false; tables.len()];
    let mut out = Vec::with_capacity(tables.len());
    while out.len() < tables.len() {
        let next = (0..tables.len())
            .find(|&i| !done[i] && parents[i].iter().all(|&p| done[p]))
            .or_else(|| (0..tables.len()).find(|&i| !done[i]))
            .expect("a table is left");
        done[next] = true;
        out.push(next);
    }
    out
}

/// Snapshot the live session into a private directory and open it through a
/// fresh, locked-down DuckDB instance:
/// * every DuckDB database attached to the session (or just the default one
///   with `attach: false`) is copied (`COPY FROM DATABASE`; with foreign keys, schema first and then table by table, parents first, #283) and re-attached
///   READ_ONLY under its original name; other catalog types (postgres, …) are
///   skipped with a warning;
/// * extensions loaded in the session (and `-- @load`/`options.load`) are
///   loaded, best-effort, while that is still allowed;
/// * then `enable_external_access = false` (no file / network access: no
///   `COPY … TO`, `read_text`, `ATTACH`, `INSTALL`, `LOAD`) — except for the
///   `options.data` folder, which goes in `allowed_directories` — extension
///   auto-install/-load off, and `lock_configuration = true` so no request can
///   undo any of it.
fn build_snapshot(
    live: &Conn,
    opts: &LockedOptions,
    extra_loads: &[String],
    dir: SnapshotDir,
    source_result: Option<(String, PathBuf, Vec<String>)>,
) -> Result<(Snapshot, Vec<String>), String> {
    let attach_all = opts.attach;
    let mut warnings = Vec::new();
    let current = live
        .query("SELECT current_database()::VARCHAR")?
        .string_rows(1)?
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next().flatten())
        .ok_or("could not determine current_database()")?;
    let dbs = live
        .query(
            "SELECT database_name::VARCHAR, type::VARCHAR FROM duckdb_databases() \
             WHERE NOT internal AND database_name NOT IN ('system', 'temp') ORDER BY database_name",
        )?
        .string_rows(10_000)?;
    // The databases the panels must find: the default one, or the search path's.
    let mut needed: Vec<&str> = opts.search_path.iter().map(|(d, _)| d.as_str()).collect();
    if needed.is_empty() {
        needed.push(&current);
    }
    let mut copied: Vec<(String, PathBuf)> = Vec::new();
    for row in dbs {
        let (Some(name), ty) = (row[0].clone(), row[1].clone().unwrap_or_default()) else {
            continue;
        };
        if !attach_all && !needed.contains(&name.as_str()) {
            continue;
        }
        if ty != "duckdb" {
            warnings.push(format!("database '{name}' ({ty}) not snapshotted"));
            continue;
        }
        let file = dir.0.join(format!("db{}.duckdb", copied.len()));
        let alias = format!("__anofox_snap_{}", sv::random_token());
        let f = file.to_string_lossy().into_owned();
        // READ_WRITE explicitly: a read-only session (`duckdb -readonly`)
        // attaches new databases read-only by default.
        live.exec(&format!(
            "ATTACH {} AS {} (READ_WRITE)",
            sql_string_literal(&f),
            quote_ident(&alias)
        ))?;
        let r = copy_database(live, &name, &alias, &mut warnings);
        let d = live.exec(&format!("DETACH {}", quote_ident(&alias)));
        r.and(d)
            .map_err(|e| format!("snapshot of '{name}' failed: {e}"))?;
        copied.push((name, file));
    }
    for n in &needed {
        if !copied.iter().any(|(c, _)| c == n) {
            return Err(if opts.search_path.is_empty() {
                format!("could not snapshot the current database '{n}'")
            } else {
                format!("options.search_path: '{n}' is not an attached DuckDB database")
            });
        }
    }
    // `options.source` (#352): a Quack source build runs in its own staging
    // instance (never this one), and already ran before `live` was even
    // locked (M1 fix 1) — the result (or its error) just lands here as one
    // more attached database. `as` is the default unless
    // `options.search_path` names something else.
    let mut default_db = opts
        .search_path
        .first()
        .map_or_else(|| current.clone(), |(db, _)| db.clone());
    if let Some((name, file, w)) = source_result {
        if copied.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            return Err(format!(
                "options.source.as: '{name}' clashes with an existing attached database"
            ));
        }
        warnings.extend(w);
        if opts.search_path.is_empty() {
            default_db = name.clone();
        }
        copied.push((name, file));
    }
    let enter = match opts.search_path.split_first() {
        None => format!("USE {}", quote_ident(&default_db)),
        Some(((db, schema), rest)) => {
            let qualified = |db: &String, schema: &Option<String>| match schema {
                Some(s) => format!("{}.{}", quote_ident(db), quote_ident(s)),
                None => quote_ident(db),
            };
            let mut sql = format!("USE {}", qualified(db, schema));
            if !rest.is_empty() {
                let path: Vec<String> = opts
                    .search_path
                    .iter()
                    .map(|(d, s)| s.as_ref().map_or(d.clone(), |s| format!("{d}.{s}")))
                    .collect();
                sql += &format!(
                    "; SET search_path = {}",
                    sql_string_literal(&path.join(","))
                );
            }
            sql
        }
    };
    let loaded: Vec<String> = live
        .query(
            "SELECT extension_name::VARCHAR FROM duckdb_extensions() \
             WHERE loaded AND extension_name <> 'anofox_visualization' \
             AND install_mode IS DISTINCT FROM 'STATICALLY_LINKED'",
        )?
        .string_rows(10_000)?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    let allow_unsigned = live
        .query("SELECT current_setting('allow_unsigned_extensions')::VARCHAR")
        .and_then(|mut r| r.string_rows(1))
        .ok()
        .and_then(|r| r.into_iter().next())
        .and_then(|r| r.into_iter().next().flatten())
        .is_some_and(|v| v == "true");

    let tmp = dir.0.join("tmp");
    let tmp_s = tmp.to_string_lossy().into_owned();
    let mut cfg: Vec<(&str, &str)> = vec![
        ("autoinstall_known_extensions", "false"),
        ("autoload_known_extensions", "false"),
        ("allow_community_extensions", "false"),
        ("temp_directory", &tmp_s),
    ];
    if allow_unsigned {
        cfg.push(("allow_unsigned_extensions", "true"));
    }
    let threads = opts.duckdb_threads.map(|t| t.to_string());
    if let Some(t) = &threads {
        cfg.push(("threads", t));
    }
    if let Some(m) = &opts.memory_limit {
        cfg.push(("memory_limit", m));
    }
    // A host in-memory DB with the snapshots attached READ_ONLY under their own
    // names. (Opening ":memory:<name>" through the C API creates a FILE of that
    // name in the working directory, so open a plain ":memory:"; its default
    // catalog is called "memory", which only matters if a source has that name.)
    let db = Database::open(":memory:", &cfg)?;
    {
        let c = db.connect()?;
        if copied
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("memory"))
        {
            // Move the host's default catalog out of the way first.
            c.exec("ATTACH ':memory:' AS anofox_snapshot_host")?;
            c.exec("USE anofox_snapshot_host")?;
            c.exec("DETACH memory")?;
        }
        for (name, file) in &copied {
            let f = file.to_string_lossy().into_owned();
            c.exec(&format!(
                "ATTACH {} AS {} (READ_ONLY)",
                sql_string_literal(&f),
                quote_ident(name)
            ))?;
            // On unix the open file stays readable after unlinking: drop the
            // snapshot from the filesystem right away.
            #[cfg(unix)]
            let _ = std::fs::remove_file(file);
        }
        // With a data folder the statements are checked with json_serialize_sql
        // (`check_read_only`), which lives in the json extension.
        let json = "json".to_string();
        let mut exts: Vec<&String> = loaded.iter().chain(extra_loads.iter()).collect();
        if opts.data.is_some() {
            exts.push(&json);
        }
        exts.sort();
        exts.dedup();
        for e in exts {
            if !sv::valid_ident(e) {
                continue;
            }
            // #352: `quack` must never be loaded into the serving instance —
            // only the throwaway staging one `copy_from_quack` opens for
            // itself. This closes every path that could otherwise get it
            // loaded here: the live session already having it loaded, a
            // dashboard's own `-- @load quack`, or `options.load`.
            if e.eq_ignore_ascii_case("quack") {
                return Err(
                    "the 'quack' extension may not be loaded into a locked server (the live \
                     session has it loaded, a dashboard has `-- @load quack`, or options.load \
                     names it) — a live Quack source is configured with options.source instead"
                        .into(),
                );
            }
            if let Err(err) = c.exec(&format!("LOAD {}", quote_ident(e))) {
                let first = err.lines().next().unwrap_or("").to_string();
                warnings.push(format!("LOAD {e}: {first}"));
            }
        }
        if let Some(data) = &opts.data {
            let d = data.to_string_lossy();
            c.exec(&format!(
                "SET allowed_directories = [{}]",
                sql_string_literal(&d)
            ))?;
        }
        // #333: best effort, same as init() — one of the extensions just
        // LOADed above may have registered `anofox_telemetry_enabled` for
        // the first time in this fresh snapshot db (it didn't exist in the
        // host DB, or it did and this repeats the init-time attempt there
        // too). Must run before `lock_configuration = true` below, which
        // would otherwise refuse the SET for the rest of this snapshot's
        // life. Skipped on an explicit opt-in, same gate as init().
        if telemetry::is_disabled(std::env::var(telemetry::VAR).ok().as_deref()) {
            crate::best_effort_disable_existing_setting(&c);
        }
        // Keep parquet footers across requests: every request re-runs the
        // setup statements on a fresh connection, and their views would
        // otherwise read each file's footer again (#209). DuckDB checks a
        // cached footer's size and mtime (to the second) against the file, so
        // a file renamed into `data` is read fresh; an in-place rewrite at the
        // same size within the same second can still read a stale footer
        // (docs/secure-serving.md). A parquet extension setting, so not part
        // of `cfg`.
        if let Err(err) = c.exec("SET GLOBAL parquet_metadata_cache = true") {
            let first = err.lines().next().unwrap_or("").to_string();
            warnings.push(format!("parquet_metadata_cache: {first}"));
        }
        c.exec("SET enable_external_access = false")?;
        c.exec("SET lock_configuration = true")?;
        // Fail now, not on every request (e.g. a schema that doesn't exist).
        c.exec(&enter)
            .map_err(|e| format!("options.search_path: {}", e.lines().next().unwrap_or("")))?;
    }
    Ok((
        Snapshot {
            warm: WarmPool::new(opts.warm.unwrap_or(opts.threads)),
            db,
            default_db,
            enter,
            policy: None,
            deps: Vec::new(),
            deps_tags: Vec::new(),
            warm_deps: Vec::new(),
            _dir: dir,
        },
        warnings,
    ))
}

/// What the request log records about one request beyond method/path/status.
#[derive(Default)]
struct LogInfo {
    user: Option<String>,
    /// The login method that signed the viewer in.
    auth: Option<&'static str>,
    dashboard: Option<String>,
    panel: Option<usize>,
    cache: Option<&'static str>,
    vars: Vec<(String, String)>,
    bytes: Option<usize>,
    deny: Option<&'static str>,
}

/// `503` with `Retry-After` (memory or query slots exhausted: [`crate::limits`]).
fn busy(req: Request, msg: &str) {
    let resp = Response::from_data(msg.as_bytes().to_vec())
        .with_status_code(503)
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header(
            "Retry-After",
            &crate::limits::RETRY_AFTER_S.to_string(),
        ));
    let _ = req.respond(resp);
}

fn reply(req: Request, status: u16, msg: &str) -> u16 {
    text(req, status, msg);
    status
}

fn handle_locked(st: &Locked, req: Request) {
    let t0 = std::time::Instant::now();
    let method = req.method().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    let peer = req.remote_addr().map(|a| a.ip());
    let mut info = LogInfo::default();
    let status = route_locked(st, req, &path, &mut info);
    if let Some(log) = &st.opts.log {
        // Page views, data requests and refusals; not every static asset.
        if path == "/api/panel" || path == "/" || path.starts_with("/d/") || status >= 400 {
            log.write(&LogEntry {
                method: &method,
                path: &path,
                status,
                ms: t0.elapsed().as_millis(),
                user: info.user.as_deref(),
                peer,
                dashboard: info.dashboard.as_deref(),
                panel: info.panel,
                cache: info.cache,
                vars: &info.vars,
                bytes: info.bytes,
                deny: info.deny,
                auth: info.auth,
            });
        }
    }
}

fn route_locked(st: &Locked, mut req: Request, path: &str, info: &mut LogInfo) -> u16 {
    let m = meta(&req);
    // Request data in a diagnostic is escaped (access::diag): a `\n` in it
    // could otherwise forge a line of the request log on the same stderr.
    eprintln!(
        "[anofox-serve] {} {}",
        access::diag(&req.method().to_string()),
        access::diag(path)
    );
    // tiny_http ends the request line only at CRLF: a bare control character
    // in the target is never a legitimate URL.
    if access::has_control_chars(req.url()) {
        return reply(req, 400, "bad request: control character in the URL");
    }
    // DNS rebinding: only loopback names (or `options.hosts`) are served.
    if !sv::host_allowed(m.host, st.port, &st.opts.hosts) {
        return reply(
            req,
            421,
            "misdirected request: unknown Host — add it to options.hosts (e.g. {\"hosts\": [\"dash.example.com\"]})",
        );
    }
    // CSRF: a browser request from another origin is refused outright.
    if !sv::origin_matches_host(m.origin, m.host) {
        return reply(req, 403, "forbidden: cross-origin request");
    }
    let health = req.method() == &Method::Get && (path == "/health" || path == "/healthz");
    // Login: with `options.auth` every request but the health check (and,
    // with embed links, the UI's code files) carries a credential; without
    // one, nobody gets anything. The health check answers anyone, but only
    // signed-in callers see more than `status`.
    let ident = match &st.opts.auth {
        Some(auth) => {
            let headers: Vec<(&str, &str)> = req
                .headers()
                .iter()
                .map(|h| (h.field.as_str().as_str(), h.value.as_str()))
                .collect();
            let method = req.method().to_string();
            let r = auth::Req {
                method: &method,
                url: req.url(),
                peer: req.remote_addr().map(|a| a.ip()),
                headers: &headers,
            };
            // The OIDC login's own endpoints (/auth/*).
            if let Some(rep) = auth.route(&r) {
                return send(req, rep);
            }
            if auth.public_asset(&method, path) {
                serve_asset(req, path, "");
                return 200;
            }
            match auth.authenticate(&r) {
                Ok(id) => {
                    info.user = Some(id.claims.user.clone());
                    info.auth = Some(id.method);
                    Some(id)
                }
                Err(_) if health => None,
                Err(d) => {
                    info.deny = Some(d.reason);
                    info.auth = d.method;
                    if let Some(rep) = d.unavailable() {
                        return send(req, rep);
                    }
                    if let Some(rep) = auth.login_redirect(&r, &d) {
                        return send(req, rep);
                    }
                    return reply(req, 401, access::UNAUTHORIZED);
                }
            }
        }
        None => None,
    };
    let claims = ident.as_ref().map(|i| i.claims.clone());
    let scope = ident.as_ref().map(|i| &i.scope);
    match (req.method(), path) {
        (&Method::Get, "/health") | (&Method::Get, "/healthz") => {
            // Without a login — or with a credential scoped to some
            // dashboards (an embed page, an API key) — only the status.
            let full = ident.as_ref().is_some_and(|i| i.scope.dashboards.is_none());
            if st.opts.auth.is_some() && !full {
                respond(req, 200, "application/json", &b"{\"status\":\"ok\"}"[..]);
                return 200;
            }
            let gen = st.current.get();
            let mut body = serde_json::json!({
                "status": "ok",
                "version": anofox_visualization::BUILD_VERSION,
                "dashboards": st.dashboards.len(),
                "epoch": gen.epoch,
                "snapshot_age_s": gen.built.elapsed().as_secs(),
                "refresh_s": st.opts.refresh.map(|d| d.as_secs()),
                "refreshing": st.current.refreshing(),
                // The error itself is only logged / returned to the operator.
                "refresh_failed": st.current.last_failed(),
                "limits": limits_json(st),
            });
            // #352: only with a Quack source configured — numbers/booleans
            // only, never the source's URI, its token env/file name or any
            // error text (those stay in the server's own stderr).
            if st.opts.source.is_some() {
                // M1 fix 4: a build still running past its own deadline
                // (abandoned, left to finish in the background) also counts
                // as `source_ok: false`, on top of the last *completed*
                // attempt's own result — a stuck build must not look healthy
                // just because nothing has come back to report failure yet.
                let overdue = st.source_build.overdue();
                if let Some(map) = body.as_object_mut() {
                    map.insert(
                        "source_ok".into(),
                        serde_json::json!(!st.current.last_failed() && !overdue),
                    );
                    map.insert(
                        "consecutive_failures".into(),
                        serde_json::json!(st.current.consecutive_failures()),
                    );
                    map.insert("build_overdue".into(), serde_json::json!(overdue));
                }
            }
            respond(req, 200, "application/json", body.to_string().into_bytes());
            200
        }
        (&Method::Post, "/api/panel") => {
            let body = match read_body(&mut req, st.opts.max_body) {
                Ok(b) => b,
                Err((code, e)) => return reply(req, code, &e),
            };
            let supersede = req_header(&req, SUPERSEDE_HEADER).map(str::to_string);
            // Held until the response is written.
            let mut charge = st.inflight.charge();
            match locked_panel(
                st,
                &body,
                supersede.as_deref(),
                claims.as_ref(),
                scope,
                info,
                &mut charge,
            ) {
                Ok((json, etag, deps)) => {
                    info.bytes = Some(json.len());
                    respond_panel(req, json, &etag, &deps);
                    200
                }
                Err((code, e)) => {
                    if code != SUPERSEDED.0 {
                        eprintln!("[anofox-serve] {code}: {}", access::diag(&e));
                    }
                    if code == 503 {
                        busy(req, &e);
                        return 503;
                    }
                    reply(req, code, &e)
                }
            }
        }
        (_, "/query") => reply(
            req,
            410,
            "this server is locked: there is no SQL endpoint (use POST /api/panel)",
        ),
        (&Method::Get, "/") => {
            // options.home: straight to that dashboard (if this viewer may read it)
            if let Some(h) = st
                .opts
                .home
                .as_deref()
                .filter(|h| scope.is_none_or(|s| s.allows(h)))
            {
                let resp = Response::empty(302)
                    .with_header(header("Location", &format!("/d/{h}")))
                    .with_header(header("Cache-Control", "no-store"));
                let _ = req.respond(resp);
                return 302;
            }
            respond_page(req, &list_page(st, scope), &st.opts, &[]);
            200
        }
        (&Method::Get, p) if p.starts_with("/d/") => {
            let id = p["/d/".len()..].trim_end_matches('/');
            match st.dashboards.iter().find(|d| d.id == id) {
                // A scoped credential (embed link, API key) reads its own
                // dashboards only.
                Some(d) if !scope.is_none_or(|s| s.allows(&d.id)) => {
                    info.dashboard = Some(d.id.clone());
                    reply(req, 403, "forbidden: not in this credential's scope")
                }
                Some(d) => {
                    info.dashboard = Some(d.id.clone());
                    let page = ident.as_ref().and_then(|i| i.page_session.as_deref());
                    let html =
                        String::from_utf8_lossy(&served_index(st, d, scope, page)).into_owned();
                    // A page holding a session is never sent to another
                    // site as a Referer (the link is in the URL).
                    let extra: &[(&str, &str)] = if page.is_some() {
                        &[("Referrer-Policy", "same-origin")]
                    } else {
                        &[]
                    };
                    respond_page(req, &html, &st.opts, extra);
                    200
                }
                None => reply(req, 404, "no such dashboard"),
            }
        }
        (&Method::Get, p) => {
            let found = WEB.get_file(p.trim_start_matches('/')).is_some() || p == "/";
            serve_asset(req, p, "");
            if found {
                200
            } else {
                404
            }
        }
        _ => reply(req, 405, "method not allowed"),
    }
}

/// The effective limits and how much of them is in use (`/healthz`, signed-in
/// callers only).
fn limits_json(st: &Locked) -> serde_json::Value {
    let o = &st.opts;
    let (running, waiting, timed_out) = st.queries.stats();
    serde_json::json!({
        "threads": o.threads,
        "max_queries": o.max_queries,
        "max_rows": o.max_rows,
        "max_response_bytes": o.max_response,
        "max_inflight_bytes": o.max_inflight,
        "cache_bytes": o.cache_bytes,
        "timeout_ms": o.timeout.as_millis() as u64,
        "memory_limit": o.memory_limit,
        "duckdb_threads": o.duckdb_threads,
        "queries_running": running,
        "queries_waiting": waiting,
        "inflight_bytes": st.inflight.used(),
        "inflight_peak_bytes": st.inflight.peak(),
        // 503s: in-flight budget exceeded / no query slot within the timeout.
        "refused_inflight": st.inflight.refused(),
        "refused_queries": timed_out,
    })
}

/// Every variable of `vars` named in `path_vars` must hold a plain file or
/// folder name.
fn check_path_values(path_vars: &[String], vars: &[(String, String)]) -> Result<(), (u16, String)> {
    for (name, lit) in vars {
        if path_vars.iter().any(|v| v.eq_ignore_ascii_case(name)) && !valid_path_value(lit) {
            return Err((
                400,
                format!("variable '{name}' must name a file or folder: [A-Za-z0-9_.-], not starting with '.'"),
            ));
        }
    }
    Ok(())
}

/// `X-Anofox-Supersede: <viewer>/<panel>/<kind>` — a newer request with the
/// same value cancels the older one (`409`). The viewer id is 128 random bits
/// (32 hex digits) chosen by the page, so one viewer can't cancel another's.
const SUPERSEDE_HEADER: &str = "X-Anofox-Supersede";

fn valid_supersede(v: &str) -> bool {
    let Some((viewer, rest)) = v.split_once('/') else {
        return false;
    };
    viewer.len() == 32
        && viewer.bytes().all(|b| b.is_ascii_hexdigit())
        && (1..=64).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
}

/// The cache key of a panel request: everything its result depends on
/// besides the snapshot (each snapshot has its own cache), the `data` files
/// (a refresh drops the cache) and the dashboards — the parsed request, with
/// only the variables in `deps` (#210; lower case — DuckDB's variable names
/// are case-insensitive), in a canonical order. `deps` is the caller's own
/// variable-dependency analysis **extended with any credential-locked
/// variables** (security review of #210/#222, L1): those always count,
/// whatever the analysis says a panel reads, because they act like
/// authorization. `r.vars` must already hold exactly the variables `deps`
/// covers — `locked_panel` retains it before calling this, so the key and
/// the `SET VARIABLE`s that actually run agree by construction (M1). Debug
/// formatting quotes and escapes every string, so distinct requests never
/// collide.
/// With a login the viewer's claims are part of the key, always in full, so
/// one viewer's result (or running query) is never handed to a viewer with
/// other claims.
fn panel_key(r: &sv::PanelRequest, claims: Option<&Claims>, deps: &Deps) -> String {
    let mut vars: Vec<(String, &String)> = r
        .vars
        .iter()
        .filter(|(n, _)| deps.contains(n))
        .map(|(n, lit)| (n.to_ascii_lowercase(), lit))
        .collect();
    vars.sort();
    format!(
        "{:?}",
        (
            claims.map(Claims::key),
            &r.dashboard,
            r.panel,
            vars,
            &r.page
        )
    )
}

fn etag(server: &str, epoch: u64, key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    format!("\"{server}.{epoch}-{:016x}\"", h.finish())
}

/// Serve one panel of one dashboard for a viewer — from the cache, by joining
/// an identical running request, or by running it (`run_panel`). Returns the
/// JSON rows and their ETag.
fn locked_panel(
    st: &Locked,
    body: &str,
    supersede: Option<&str>,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    info: &mut LogInfo,
    charge: &mut Charge,
) -> Result<(Arc<String>, String, String), (u16, String)> {
    let mut r = sv::parse_panel_request(body).map_err(|e| (400, e))?;
    info.dashboard = Some(r.dashboard.clone());
    info.panel = Some(r.panel);
    for name in r.vars.iter().map(|(n, _)| n) {
        // (`__claim_*` and `anofox_data` are refused when the request is
        // parsed.) The policy's own names are reserved, too.
        if st.opts.policy.is_some()
            && name
                .to_ascii_lowercase()
                .starts_with(policy::RESERVED_PREFIX)
        {
            return Err((400, format!("variable '{name}' is set by the server")));
        }
    }
    check_path_values(&st.opts.path_vars, &r.vars)?;
    // The snapshot this request runs on, start to end (a refresh only changes
    // what later requests get).
    let gen = st.current.get();
    let di = st
        .dashboards
        .iter()
        .position(|d| d.id == r.dashboard)
        .ok_or((404, "no such dashboard".to_string()))?;
    let dash = &st.dashboards[di];
    let deps_tag = gen.snap.deps_tags.get(di).cloned().unwrap_or_default();
    // `-- @vars`: a variable the dashboard doesn't let viewers set is never set.
    if let Some(allowed) = &dash.meta.vars {
        r.vars
            .retain(|(k, _)| allowed.iter().any(|a| a.eq_ignore_ascii_case(k)));
    }
    // A scoped credential: its dashboards only, and its locked variables
    // replace the viewer's (set after them, part of the cache key).
    if let Some(s) = scope {
        if !s.allows(&dash.id) {
            return Err((403, "forbidden: not in this credential's scope".into()));
        }
        s.lock_vars(&mut r.vars);
        // A locked variable that names a path obeys the same rule as a
        // viewer's (checked above for those).
        check_path_values(&st.opts.path_vars, &s.vars)?;
        // Two spellings of one name can collide again here even though the
        // request's own vars were already deduped: the credential's locked
        // variables are appended afterwards, unconditionally (security
        // review of #210/#222, M2). The later SET (DuckDB's own rule) wins.
        r.vars = sv::dedup_last_spelling(r.vars);
    }
    info.vars = r.vars.clone();
    // The variables whose values the viewer chose (not a path variable, not
    // locked by the credential): a `${name}` in `-- @requires` may not name
    // one.
    let viewer_set: Vec<String> = r
        .vars
        .iter()
        .map(|(n, _)| n)
        .filter(|n| !st.opts.path_vars.iter().any(|p| p.eq_ignore_ascii_case(n)))
        .filter(|n| scope.is_none_or(|s| !s.vars.iter().any(|(l, _)| l.eq_ignore_ascii_case(n))))
        .cloned()
        .collect();
    // Under a policy, the statements rewritten for this snapshot.
    let script = gen.snap.policy.as_ref().map(|p| &p.scripts[di]);
    let stmts = script.map_or(&dash.stmts, |s| &s.stmts);
    let (is_setup, panel_sql) = stmts
        .get(r.panel)
        .ok_or((404, "no such panel".to_string()))?;
    // `omit` (parse_page allows only the ::INFO column's name): only for a
    // ::PAGED table that has one.
    if r.page.as_ref().is_some_and(|p| !p.omit.is_empty())
        && !dash.info.get(r.panel).copied().unwrap_or(false)
    {
        return Err((
            400,
            "'page.omit' may only name the panel's ::INFO column".into(),
        ));
    }
    if *is_setup {
        // setup runs server-side, per request (run_panel)
        return Ok((
            Arc::new("[]".into()),
            etag(&st.server_id, gen.epoch, "setup"),
            deps_tag,
        ));
    }
    let ticket = match supersede {
        None => None,
        // Namespaced by viewer login and dashboard: a viewer id is per page.
        Some(v) if valid_supersede(v) => {
            Some(format!("{:?}/{}/{v}", claims.map(|c| &c.user), r.dashboard))
        }
        Some(_) => {
            return Err((
                400,
                format!("{SUPERSEDE_HEADER} must be <32 hex digits>/<panel>/<kind>"),
            ))
        }
    };
    // Only the variables this statement can depend on (#210; All when
    // the analysis can't tell). Credential-locked variables are never
    // narrowed (security review of #210/#222, L1): they act like
    // authorization, so they always count, whatever the analysis says.
    let deps = gen
        .snap
        .deps
        .get(di)
        .and_then(|d| d.get(r.panel))
        .cloned()
        .unwrap_or(Deps::All);
    let mut key_deps = deps;
    if let Some(s) = scope {
        key_deps.extend(&Deps::Vars(
            s.vars.iter().map(|(n, _)| n.to_ascii_lowercase()).collect(),
        ));
    }
    // Execute exactly what the key records (security review of #210/#222,
    // M1): a variable outside `key_deps` is never `SET`, so it can't make a
    // shared single-flight fail for every joiner (a reserved word or a
    // mixed-type value a viewer chose for a variable the panel doesn't read
    // would otherwise poison requests that only share that panel's real
    // inputs), and a miss in the analysis gives NULL for that variable to
    // every viewer instead of leaking a value computed for another one.
    if !key_deps.is_all() {
        r.vars.retain(|(n, _)| key_deps.contains(n));
    }
    let key = panel_key(&r, claims, &key_deps);
    let t0 = std::time::Instant::now();
    let warm = warm_key(dash, gen.snap.warm_deps.get(di), stmts, &r, claims, scope);
    let warmed = AtomicBool::new(false);
    let (out, how) = gen.cache.run(&key, ticket, |abort| {
        run_panel(
            st,
            &gen,
            dash,
            stmts,
            script,
            panel_sql,
            &r,
            &viewer_set,
            claims,
            abort,
            charge,
            warm.as_deref(),
            &warmed,
        )
    });
    info.cache = Some(match how {
        Served::Hit => "hit",
        Served::Shared => "shared",
        Served::Executed => "run",
    });
    if how == Served::Executed {
        let cancelled = matches!(&out, Err((code, _)) if *code == SUPERSEDED.0);
        eprintln!(
            "[anofox-serve] ran {}#{} in {} ms{}{}",
            r.dashboard,
            r.panel,
            t0.elapsed().as_millis(),
            if warmed.load(Ordering::Relaxed) {
                " (warm)"
            } else {
                ""
            },
            if cancelled { " (superseded)" } else { "" }
        );
    }
    out.map(|json| (json, etag(&st.server_id, gen.epoch, &key), deps_tag))
}

/// The `SET VARIABLE`s that open every panel request, in order: the viewer's
/// variables, then the login's claims and `anofox_data` — set by the server
/// after the viewer's (who can't name them: both are reserved when the
/// request is parsed).
/// [`request_sets`] with each variable's name (to reset it on a warm
/// connection's next request).
fn request_sets_named(
    r: &sv::PanelRequest,
    claims: Option<&Claims>,
    data: Option<&Path>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = r
        .vars
        .iter()
        .map(|(name, lit)| (name.clone(), format!("SET VARIABLE {name} = {lit}")))
        .collect();
    for (name, lit) in claims.map(Claims::variables).unwrap_or_default() {
        out.push((name.to_string(), format!("SET VARIABLE {name} = {lit}")));
    }
    if let Some(data) = data {
        out.push((
            DATA_VAR.to_string(),
            format!(
                "SET VARIABLE {DATA_VAR} = {}",
                sql_string_literal(&data.to_string_lossy())
            ),
        ));
    }
    out
}

fn request_sets(r: &sv::PanelRequest, claims: Option<&Claims>, data: Option<&Path>) -> Vec<String> {
    let mut out: Vec<String> = r
        .vars
        .iter()
        .map(|(name, lit)| format!("SET VARIABLE {name} = {lit}"))
        .collect();
    for (name, lit) in claims.map(Claims::variables).unwrap_or_default() {
        out.push(format!("SET VARIABLE {name} = {lit}"));
    }
    if let Some(data) = data {
        out.push(format!(
            "SET VARIABLE {DATA_VAR} = {}",
            sql_string_literal(&data.to_string_lossy())
        ));
    }
    out
}

/// Run one panel of one dashboard. Fresh connection per request (variables
/// and temp objects never leak between viewers), variables set as typed
/// literals, the dashboard's preceding setup statements re-run (only TEMP
/// objects can be created — the snapshot is read-only), and the panel query
/// executed as a single prepared statement under the row cap + timeout. The
/// query is interrupted when every request waiting for it has been superseded.
/// It waits for a query slot first, and its JSON is charged to the in-flight
/// budget as it grows ([`crate::limits`]).
#[allow(clippy::too_many_arguments)]
fn run_panel(
    st: &Locked,
    gen: &Generation<Snapshot>,
    dash: &Dash,
    stmts: &[(bool, String)],
    script: Option<&Script>,
    panel_sql: &str,
    r: &sv::PanelRequest,
    viewer_set: &[String],
    claims: Option<&Claims>,
    abort: &crate::panel_cache::Abort,
    charge: &mut Charge,
    warm_key: Option<&str>,
    warmed: &AtomicBool,
) -> Result<String, (u16, String)> {
    let dash_id = dash.id.as_str();
    let cancelled = || (SUPERSEDED.0, SUPERSEDED.1.to_string());
    let Some(_slot) = st
        .queries
        .acquire(Instant::now() + st.opts.timeout, || abort.abandoned())
    else {
        return Err(if abort.abandoned() {
            cancelled()
        } else {
            (503, crate::limits::BUSY.to_string())
        });
    };
    let max_response = st.opts.max_response;
    let mut grow = |len: usize, want: usize| {
        if len > max_response {
            return Err(format!(
                "result exceeds the size limit ({} MB)",
                max_response >> 20
            ));
        }
        let cap = want.min(max_response);
        charge.hold(cap).map(|_| cap)
    };
    // A warm connection (#388): its setup already ran with the same inputs.
    let warm = warm_key.and_then(|k| gen.snap.warm.take(k));
    let reused = warm.is_some();
    warmed.store(reused, Ordering::Relaxed);
    let (conn, was_set) = match warm {
        Some(w) => (w.conn, w.set),
        None => (gen.snap.db.connect().map_err(|e| (500, e))?, Vec::new()),
    };
    let sets = request_sets_named(r, claims, st.opts.data.as_deref());
    let mut setup_failed = false;
    let intr = conn.interrupter();
    let Some(registered) = abort.register(move || intr.interrupt()) else {
        return Err(cancelled());
    };
    let (out, fired) = conn.with_deadline_fired(st.opts.timeout, |c| {
        if reused {
            // Back to what a fresh connection would hold: the previous
            // request's variables this one doesn't set are reset, this
            // request's are set, and the setup's own `SET`s run again after
            // them (they win over a viewer's, as below).
            for name in was_set
                .iter()
                .filter(|n| !sets.iter().any(|(m, _)| m.eq_ignore_ascii_case(n)))
            {
                c.exec(&format!("RESET VARIABLE {name}"))?;
            }
            for (_, set) in &sets {
                c.exec(set)?;
            }
            for (setup, sql) in &stmts[..r.panel] {
                // Best effort, as on a fresh connection (and then not kept).
                if *setup && sets_variables(sql) {
                    if let Err(e) = c.execute_prepared(sql, &[]) {
                        setup_failed = true;
                        eprintln!(
                            "[anofox-serve] setup statement failed in '{dash_id}': {}",
                            access::diag(&e)
                        );
                    }
                }
            }
            if abort.abandoned() {
                return Err(SUPERSEDED.1.to_string());
            }
            return run_panel_query(c, st, panel_sql, r, &mut grow);
        }
        c.exec(&gen.snap.enter)?;
        for (_, set) in &sets {
            c.exec(set)?;
        }
        // The policy's views, for this viewer (before any dashboard SQL). The
        // row filters read the claims set above; the column rules are decided
        // here from the same claims.
        if let (Some(p), Some(s)) = (&gen.snap.policy, script) {
            let viewer = claims.map_or_else(Viewer::default, |c| Viewer {
                user: Some(c.user.clone()),
                groups: c.groups.clone(),
            });
            for v in p.enf.view_statements(&s.tables, &viewer) {
                c.exec(&v)?;
            }
        }
        // With a data folder, a statement whose `-- @requires` input is
        // missing is skipped (setup) or reads "Not available" (the panel).
        let met = |c: &Conn, i: usize| -> bool {
            match (st.opts.data.as_deref(), dash.requires.get(i)) {
                (Some(data), Some(reqs)) if !reqs.is_empty() => {
                    requires_met(c, reqs, data, viewer_set, &st.requires_cache)
                }
                _ => true,
            }
        };
        let gates = dash.tabs.get(r.panel).map_or(&[][..], Vec::as_slice);
        for (i, (setup, sql)) in stmts[..r.panel].iter().enumerate() {
            // The panel's tab (and sub-tab), on the state at the tab.
            if gates.contains(&i) && !met(c, i) {
                return Err(REQUIRES_MISSING.to_string());
            }
            if *setup {
                if !met(c, i) {
                    continue;
                }
                // Best effort, like the browser: a failing setup statement is
                // reported in the log, the panel still runs.
                if let Err(e) = c.execute_prepared(sql, &[]) {
                    setup_failed = true;
                    eprintln!(
                        "[anofox-serve] setup statement failed in '{dash_id}': {}",
                        access::diag(&e)
                    );
                }
            }
        }
        // (An interrupt between two statements may not reach the next one.)
        if abort.abandoned() {
            return Err(SUPERSEDED.1.to_string());
        }
        if !met(c, r.panel) {
            return Err(REQUIRES_MISSING.to_string());
        }
        run_panel_query(c, st, panel_sql, r, &mut grow)
    });
    // No interrupt can reach the connection after this.
    drop(registered);
    // Pool it only after a clean run: no interrupt (a superseding viewer or
    // the deadline) may be pending on it, and every setup statement ran.
    if let Some(key) = warm_key {
        if out.is_ok() && !fired && !abort.abandoned() && !setup_failed {
            gen.snap.warm.put(Warm {
                key: key.to_string(),
                conn,
                set: sets.into_iter().map(|(n, _)| n).collect(),
            });
        }
    }
    out.map_err(|e| {
        if abort.abandoned() {
            cancelled()
        } else if e == REQUIRES_MISSING {
            // (no data in it: also the message under a policy)
            (400, e)
        } else if e == crate::limits::BUSY {
            (503, e)
        } else if e.contains("row limit") || e.contains("size limit") {
            (413, e)
        } else if gen.snap.policy.is_some() && !e.starts_with("query timed out") {
            // Under a policy, a DuckDB error could quote a value from a row or
            // column the viewer may not see: the viewer gets a generic
            // message, the log the details.
            eprintln!(
                "[anofox-serve] panel failed in '{dash_id}': {}",
                access::diag(&redact(e, st.opts.data.as_deref()))
            );
            (400, POLICY_PANEL_ERROR.to_string())
        } else {
            (400, redact(e, st.opts.data.as_deref()))
        }
    })
}

/// The panel query itself (a `::PAGED` page or the whole result), as JSON rows.
fn run_panel_query(
    c: &Conn,
    st: &Locked,
    panel_sql: &str,
    r: &sv::PanelRequest,
    grow: &mut dyn FnMut(usize, usize) -> Result<usize, String>,
) -> Result<String, String> {
    match &r.page {
        Some(p) => {
            let (q, params) = sv::page_sql(panel_sql, p);
            let params: Vec<&str> = params.iter().map(String::as_str).collect();
            json_rows(c, &q, &params, st.opts.max_rows, grow)
        }
        None => json_rows(c, panel_sql, &[], st.opts.max_rows, grow),
    }
}

/// Does a setup statement set or reset a variable (re-run on a warm
/// connection, after the request's own variables)? Conservative: any
/// statement starting with `SET` or `RESET`.
fn sets_variables(sql: &str) -> bool {
    let head: String = sql
        .lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with("--") && !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let w = head
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    w == "SET" || w == "RESET"
}

/// The key of a warm connection (#388) for this request, or `None` when the
/// panel can't use one: no setup worth keeping before it, or a `-- @requires`
/// on the way (whether a gated statement runs depends on files, per request).
/// Everything that shapes the connection is in it: the viewer's login (claims,
/// and through them a policy's per-viewer views), the dashboard and how far
/// into its setup the panel sits, and the request variables the setup can
/// read (`setup_deps`: per statement, [`deps::setup_deps`]; `All` when
/// unsure) plus every credential-locked
/// variable. Two requests with the same key get identical setup results.
fn warm_key(
    dash: &Dash,
    setup_deps: Option<&Vec<Deps>>,
    stmts: &[(bool, String)],
    r: &sv::PanelRequest,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
) -> Option<String> {
    let before = stmts.get(..r.panel)?;
    let last = before.iter().rposition(|(setup, _)| *setup)?;
    if !before
        .iter()
        .any(|(setup, sql)| *setup && !sets_variables(sql))
    {
        return None;
    }
    if dash
        .requires
        .iter()
        .take(r.panel + 1)
        .any(|q| !q.is_empty())
    {
        return None;
    }
    let read = setup_deps
        .and_then(|d| d.get(r.panel))
        .unwrap_or(&Deps::All);
    let locked =
        |n: &str| scope.is_some_and(|s| s.vars.iter().any(|(l, _)| l.eq_ignore_ascii_case(n)));
    let mut vars: Vec<(String, &String)> = r
        .vars
        .iter()
        .filter(|(n, _)| read.contains(n) || locked(n))
        .map(|(n, lit)| (n.to_ascii_lowercase(), lit))
        .collect();
    vars.sort();
    Some(format!(
        "{:?}",
        (claims.map(Claims::key), &r.dashboard, last, vars)
    ))
}

/// The error of a panel whose `-- @requires` input is missing: the client
/// reads it as "Not available" (it names a missing input: "no files found").
const REQUIRES_MISSING: &str =
    "optional input missing: no files found for this panel's -- @requires";

/// Is every `-- @requires` requirement of a statement met, on this request's
/// connection (its variables, the setup before it)? A requirement is met when
/// one of its alternatives matches a file inside the data folder. The rule:
/// * `${name}` takes the variable's value here, unless the viewer chose it
///   (`viewer_set`: not a `path_vars` variable, not locked by the credential)
///   or it is unset;
/// * a pattern (`*`, `?`, `[…]`) may come only from the author's literal
///   text: a value with one is refused (a dashboard variable may carry a
///   viewer's value: `SET VARIABLE rd = … || getvariable('q')`);
/// * the expanded path must stay inside the data folder ([`rq::confine`]: no
///   `..`, `~`, URL or absolute path outside it);
/// * a match counts only if its real path (symlinks resolved) is inside the
///   folder too: `glob` lists through a link pointing out of it.
///
/// Anything refused, unset or failing makes that alternative missing; the
/// reason (and any DuckDB error) goes to the log only. Definite answers are
/// remembered briefly per expanded path (`cache`); a failed or interrupted
/// check is not, so no viewer can make a file "missing" for others. The viewer learns only met / not met,
/// about files inside the folder, which every viewer may read anyway.
fn requires_met(
    c: &Conn,
    reqs: &[Requirement],
    data: &Path,
    viewer_set: &[String],
    cache: &rq::ExistsCache,
) -> bool {
    let log = |what: &str, alt: &str| {
        eprintln!(
            "[anofox-serve] -- @requires {}: {alt}",
            redact(what.to_string(), Some(data))
        )
    };
    let names = rq::all_vars(reqs);
    let values: Vec<Option<String>> = if names.is_empty() {
        Vec::new()
    } else {
        match c
            .query(&rq::vars_sql(&names))
            .and_then(|mut r| r.string_rows(1))
        {
            Ok(rows) => rows.into_iter().next().unwrap_or_default(),
            Err(e) => {
                log(&format!("variables unreadable ({e})"), "");
                Vec::new()
            }
        }
    };
    let lookup = |name: &str| -> Option<String> {
        if viewer_set.iter().any(|v| v.eq_ignore_ascii_case(name)) {
            return None;
        }
        let i = names.iter().position(|n| n.eq_ignore_ascii_case(name))?;
        values.get(i).cloned().flatten()
    };
    let root = std::fs::canonicalize(data).unwrap_or_else(|_| data.to_path_buf());
    let data_text = data.to_string_lossy();
    let alternative_met = |alt: &str| -> bool {
        let mut patterned = false;
        let expanded = rq::expand(alt, |name| {
            let v = lookup(name)?;
            patterned |= rq::has_glob(&v);
            Some(v)
        });
        let path = match expanded {
            Ok(_) if patterned => {
                log("refused (a pattern character in a variable's value)", alt);
                return false;
            }
            Ok(p) => p,
            Err(_) => return false,
        };
        let path = match rq::confine(&path, &data_text) {
            Ok(p) => p,
            Err(e) => {
                log(&format!("refused ({e})"), alt);
                return false;
            }
        };
        // (A failed or interrupted probe answers None: missing for this
        // request only, never cached for other viewers.)
        cache.exists(&path, || {
            match c
                .query(&rq::files_sql(&path))
                .and_then(|mut r| r.string_rows(rq::MAX_MATCHES))
            {
                Ok(rows) => {
                    let inside = rows
                        .iter()
                        .filter_map(|r| r.first().cloned().flatten())
                        .any(|f| rq::inside_canonical(&f, &root));
                    if !inside && rows.len() >= rq::MAX_MATCHES {
                        log("missing: the first matches all lie outside the folder", alt);
                    }
                    Some(inside)
                }
                Err(e) => {
                    log(&format!("check failed ({e})"), alt);
                    None
                }
            }
        })
    };
    reqs.iter()
        .all(|req| req.iter().any(|alt| alternative_met(alt)))
}

/// What a viewer sees when a panel fails under an access policy.
const POLICY_PANEL_ERROR: &str = "this panel could not be shown (the server log has the details)";

/// index.html with `window.__served = {id,title,sql,nav,locked,deps,depsTag}`
/// injected (script-safe JSON: `<` is emitted as `<`, so `</script>` in a
/// dashboard's SQL cannot break out).
/// The served page of `dash`. `scope` limits the navigation to the
/// dashboards the viewer may read; `session` is an embed link's page session
/// (the page sends it with each request, see `auth::SESSION_HEADER`).
fn served_index(st: &Locked, dash: &Dash, scope: Option<&Scope>, session: Option<&str>) -> Vec<u8> {
    let html = WEB
        .get_file("index.html")
        .map(|f| String::from_utf8_lossy(f.contents()).into_owned())
        .unwrap_or_default();
    let mut cfg = serde_json::json!({
        "id": dash.id, "title": dash.meta.title, "sql": dash.sql,
        "nav": nav_json(&st.dashboards, scope), "locked": true,
        "theme": st.opts.theme,
    });
    if let Some(s) = session {
        cfg["session"] = serde_json::json!(s);
    }
    // What each statement depends on (#210): the page re-requests a panel
    // only when one of its variables changed.
    let gen = st.current.get();
    if let Some(di) = st.dashboards.iter().position(|d| d.id == dash.id) {
        if let (Some(ds), Some(tag)) = (gen.snap.deps.get(di), gen.snap.deps_tags.get(di)) {
            cfg["deps"] = serde_json::json!(ds.iter().map(Deps::to_json).collect::<Vec<_>>());
            cfg["depsTag"] = serde_json::json!(tag);
        }
    }
    let cfg = json_for_script(&cfg);
    html.replacen("<head>", "<head><base href=\"/\">", 1)
        .replacen(
            "</head>",
            &format!("<script>window.__served={cfg};</script></head>"),
            1,
        )
        .into_bytes()
}

/// The served nav: the dashboards the viewer may read (`scope`), each
/// `{id, title, nav, group, tags, owner, certified}` — the client groups
/// them and shows `nav` (`-- @nav`, a short label) in place of `title` when
/// it is set (#355).
fn nav_json(dashboards: &[Dash], scope: Option<&Scope>) -> Vec<serde_json::Value> {
    dashboards
        .iter()
        .filter(|d| scope.is_none_or(|s| s.allows(&d.id)))
        .map(|d| d.meta.nav_json(&d.id))
        .collect()
}

/// The `/` landing page: the dashboards the viewer may read (the same
/// scope filter as the nav), as cards in groups, in the server's theme, with
/// `options.app` as its header (see `page::landing`).
fn list_page(st: &Locked, scope: Option<&Scope>) -> String {
    let set = anofox_visualization::theme::resolve(st.opts.theme.as_deref().unwrap_or(""))
        .unwrap_or_else(|_| anofox_visualization::theme::resolve("").expect("the default theme"));
    anofox_visualization::page::landing(&anofox_visualization::page::Landing {
        theme: &set,
        app: st.opts.app.as_ref(),
        items: st
            .dashboards
            .iter()
            .filter(|d| scope.is_none_or(|s| s.allows(&d.id)))
            .map(|d| anofox_visualization::page::LandingItem {
                href: format!("/d/{}", d.id),
                meta: &d.meta,
                updated: d.updated.clone(),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #352 R1: `in_flight` must clear even when the build thread panics —
    /// not just on a normal return — or a source build is refused forever
    /// after the first unrelated panic (the review's exact repro, via a
    /// non-ASCII token that made the old `redact` panic). `thread::spawn`'s
    /// child panicking does not crash this test process (the default panic
    /// strategy unwinds), so `ClearInFlightOnDrop`'s `Drop` runs during that
    /// unwind exactly as it would on a normal return.
    #[test]
    fn in_flight_clears_even_if_the_build_thread_panics() {
        let state = SourceBuildState::new();
        assert!(!state.in_flight.swap(true, Ordering::SeqCst));
        let s = state.clone();
        let h = std::thread::spawn(move || {
            let _guard = ClearInFlightOnDrop(s);
            panic!("simulated build-thread panic");
        });
        assert!(h.join().is_err(), "the thread should have panicked");
        assert!(
            !state.in_flight.load(Ordering::SeqCst),
            "in_flight must be cleared by the drop guard even on a panic"
        );
    }

    /// #352 R2: the process-wide cap's own arithmetic, pinned down directly.
    /// `GLOBAL_SOURCE_BUILDS` isn't touched by any other test, so this is
    /// safe to manipulate here; a real end-to-end repro needs two Quack
    /// builds genuinely overlapping in time, which — tried against both a
    /// bare black-hole listener (its ATTACH was observed to error out on
    /// its own after a while, plausibly a lower-level connect/keepalive
    /// timeout, closing the overlap window before a 3rd attempt could see
    /// it) and a SIGSTOP'd real source (reliable, used in
    /// test-quack-source.sh instead) — depends on timing this shell test
    /// doesn't need to reproduce.
    #[test]
    fn global_source_build_cap_refuses_once_reached() {
        let saved = GLOBAL_SOURCE_BUILDS.swap(0, Ordering::SeqCst);
        for _ in 0..MAX_CONCURRENT_SOURCE_BUILDS {
            assert!(
                GLOBAL_SOURCE_BUILDS.fetch_add(1, Ordering::SeqCst) < MAX_CONCURRENT_SOURCE_BUILDS
            );
        }
        // Mirrors run_source_build's own check-then-rollback exactly.
        let prior = GLOBAL_SOURCE_BUILDS.fetch_add(1, Ordering::SeqCst);
        assert!(
            prior >= MAX_CONCURRENT_SOURCE_BUILDS,
            "a build attempted once the cap is reached must see it"
        );
        GLOBAL_SOURCE_BUILDS.fetch_sub(1, Ordering::SeqCst);
        for _ in 0..MAX_CONCURRENT_SOURCE_BUILDS {
            GLOBAL_SOURCE_BUILDS.fetch_sub(1, Ordering::SeqCst);
        }
        GLOBAL_SOURCE_BUILDS.store(saved, Ordering::SeqCst);
    }

    /// #352 M1 fix 3: `join_bounded` returns once every handle has finished
    /// (fast case), and gives up — without panicking or hanging the test —
    /// once its budget runs out on a handle that never finishes, leaving it
    /// to run in the background.
    #[test]
    fn join_bounded_waits_for_fast_handles_and_gives_up_on_a_stuck_one() {
        let t0 = Instant::now();
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        let fast = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            d.store(true, Ordering::SeqCst);
        });
        join_bounded(vec![fast], Duration::from_secs(5));
        assert!(
            done.load(Ordering::SeqCst),
            "the fast handle should have joined"
        );
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "join_bounded should not have waited the full budget for a fast handle"
        );

        let t1 = Instant::now();
        let stuck = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(600));
        });
        join_bounded(vec![stuck], Duration::from_millis(100));
        assert!(
            t1.elapsed() < Duration::from_secs(2),
            "join_bounded must not block on a handle stuck past its budget"
        );
    }

    fn dash(id: &str, script: &str) -> Dash {
        Dash {
            id: id.into(),
            sql: script.into(),
            stmts: vec![],
            info: vec![],
            requires: vec![],
            tabs: vec![],
            meta: sv::parse_dashboard_meta(id, script),
            updated: None,
        }
    }

    #[test]
    fn fk_order_puts_parents_first() {
        let t = |s: &str, n: &str| (s.to_string(), n.to_string());
        let e = |s: &str, c: &str, p: &str| (s.to_string(), c.to_string(), p.to_string());
        let tables = vec![
            t("main", "actuals"),
            t("main", "materials"),
            t("main", "suppliers"),
            t("main", "tree"),
            t("s2", "materials"),
        ];
        let edges = vec![
            e("main", "actuals", "materials"),
            e("main", "materials", "Suppliers"),
            e("main", "tree", "tree"),
            e("main", "ghost", "materials"),
        ];
        assert_eq!(fk_order(&tables, &edges), vec![2, 1, 0, 3, 4]);
        // a cycle (not creatable in DuckDB, but never a hang): the given order
        let ab = vec![t("main", "a"), t("main", "b")];
        assert_eq!(
            fk_order(&ab, &[e("main", "a", "b"), e("main", "b", "a")]),
            vec![0, 1]
        );
    }

    /// #301: `load_dashboards` orders by `-- @order` ascending, not by file
    /// name — the issue's own repro (`zzz_first.sql` intended #1, etc.),
    /// plus an unordered file sorting after every explicitly ordered one.
    #[test]
    fn load_dashboards_follows_order_header_not_file_name() {
        let dir = std::env::temp_dir().join(format!("anofox-order-test-{}", sv::random_token()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("zzz_first.sql"), "-- @order 1\nSELECT 1;").unwrap();
        std::fs::write(dir.join("aaa_second.sql"), "-- @order 2\nSELECT 1;").unwrap();
        std::fs::write(dir.join("mmm_third.sql"), "-- @order 3\nSELECT 1;").unwrap();
        std::fs::write(dir.join("zzz_unordered.sql"), "SELECT 1;").unwrap();
        let (dashboards, _) = load_dashboards(&dir).unwrap();
        let ids: Vec<&str> = dashboards.iter().map(|d| d.id.as_str()).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            ids,
            ["zzz_first", "aaa_second", "mmm_third", "zzz_unordered"],
            "explicit @order wins over alphabetical file-name order; the \
             unordered file still sorts last, by id, as before #301"
        );
    }

    #[test]
    fn nav_keeps_groups_and_follows_the_scope() {
        let ds = vec![
            dash(
                "sales",
                "-- @title Weekly sales\n-- @nav Sales\n-- @group Sales / EMEA\n-- @tags kpi, weekly\n\
                 -- @owner Finance\n-- @certified 2026-10-15\n-- @description Per store.\nSELECT 1;",
            ),
            dash("ops", "-- @group Ops\nSELECT 1;"),
            dash("plain", "SELECT 1;"),
        ];
        let all = nav_json(&ds, None);
        assert_eq!(all.len(), 3);
        assert_eq!(
            all[0],
            // #355: `-- @nav` rides along as its own field, next to `title`
            // (a reader shows `nav || title`), not instead of it.
            serde_json::json!({"id": "sales", "title": "Weekly sales", "nav": "Sales", "group": "Sales / EMEA",
                "tags": ["kpi", "weekly"], "description": "Per store.", "owner": "Finance",
                "certified": "2026-10-15"})
        );
        assert_eq!(
            all[2],
            serde_json::json!({"id": "plain", "title": "plain", "nav": null, "group": null, "tags": [],
                "description": null, "owner": null, "certified": null})
        );
        // a scoped viewer sees only their dashboards — with their groups intact
        let scope = Scope {
            dashboards: Some(vec!["sales".into(), "plain".into()]),
            vars: vec![],
        };
        let some = nav_json(&ds, Some(&scope));
        let ids: Vec<&str> = some.iter().map(|d| d["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["sales", "plain"]);
        assert_eq!(some[0]["group"], "Sales / EMEA");
        assert_eq!(some[0]["certified"], "2026-10-15");
        assert_eq!(some[0]["nav"], "Sales");
        let none = Scope {
            dashboards: Some(vec![]),
            vars: vec![],
        };
        assert!(nav_json(&ds, Some(&none)).is_empty());
    }

    /// #214: a `::VIEW` alternative's own `-- @requires` must gate its
    /// statements through the exact same mechanism a `::TAB`/`::SUBTAB`
    /// already does: its marker's plan index lands in [`Dash::tabs`] (built
    /// here exactly the way [`load_dashboards`] builds it, from the core's
    /// `sql::plan` → `Panel::tabs`), so `run_panel`'s
    /// `gates.contains(&i) && !met(c, i)` walks it with no server code of
    /// its own — the whole point of #214's locked-server step: no logic
    /// change, only this test.
    #[test]
    fn a_view_alternatives_requires_lands_in_dash_tabs_like_a_tab() {
        let sql = "SELECT 'Lines' AS yoy ::VIEW;\n\
                    SELECT a::XAXIS, b::BARCHART FROM t;\n\
                    -- @requires ${rd}/forecasts.parquet\n\
                    SELECT 'Bars' AS yoy2 ::VIEW;\n\
                    SELECT a::XAXIS, c::BARCHART FROM t;";
        let planned = anofox_visualization::sql::plan(sql);
        assert_eq!(planned.len(), 4);
        // Exactly `load_dashboards`'s mapping from `planned` into `Dash`.
        let tabs: Vec<Vec<usize>> = planned
            .iter()
            .map(|p| if p.setup { Vec::new() } else { p.tabs.clone() })
            .collect();
        let requires: Vec<Vec<Requirement>> = planned.iter().map(|p| p.requires.clone()).collect();
        // Statement 2 is the hidden alternative's own `::VIEW` marker: its
        // own `-- @requires` is what `met(c, 2)` would check.
        assert!(!requires[2].is_empty(), "{requires:?}");
        assert!(requires[0].is_empty() && requires[1].is_empty() && requires[3].is_empty());
        // Statement 3 (the alternative's panel) is gated by statement 2 —
        // the same list shape `run_panel`'s
        // `dash.tabs.get(r.panel).map_or(&[][..], Vec::as_slice)` reads for
        // a ::SUBTAB's panel, gated by the ::SUBTAB's own index.
        assert_eq!(tabs[3], vec![2]);
        // The first (shown) alternative's marker (0) carries no requires of
        // its own, so a request for its panel (1) is gated on index 0 but
        // trivially met (`met()` is `true` when `reqs.is_empty()`).
        assert_eq!(tabs[1], vec![0]);
    }

    #[test]
    fn options() {
        let o = LockedOptions::parse(Some(
            r#"{"max_rows":5,"timeout_ms":100,"load":["spatial"]}"#,
        ))
        .unwrap();
        assert_eq!(o.max_rows, 5);
        assert_eq!(o.timeout, Duration::from_millis(100));
        assert_eq!(o.load, vec!["spatial".to_string()]);
        assert!(LockedOptions::parse(Some(r#"{"nope":1}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"load":["x; DROP"]}"#)).is_err());
        assert!(LockedOptions::parse(None).is_ok());
        let o = LockedOptions::parse(Some(r#"{"hosts":["dash.example.com","intranet:8443"]}"#))
            .unwrap();
        assert_eq!(
            o.hosts,
            vec!["dash.example.com".to_string(), "intranet:8443".to_string()]
        );
        assert!(LockedOptions::parse(Some(r#"{"hosts":["a b"]}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"hosts":"x"}"#)).is_err());
        let t = LockedOptions::parse(Some(r#"{"theme":"dark"}"#)).unwrap();
        assert_eq!(t.theme.as_deref(), Some("dark"));
        assert!(LockedOptions::parse(Some(r#"{"theme":"nope"}"#)).is_err());
        assert!(LockedOptions::parse(Some(
            r##"{"theme":{"extends":"light","palette":["#123456"]}}"##
        ))
        .is_ok());
    }

    #[test]
    fn limit_options() {
        let o = LockedOptions::parse(None).unwrap();
        assert_eq!(
            (o.max_inflight, o.max_response, o.max_queries),
            (256 << 20, 32 << 20, 4)
        );
        let o = LockedOptions::parse(Some(
            r#"{"max_inflight_mb":64,"max_response_mb":8,"max_queries":2}"#,
        ))
        .unwrap();
        assert_eq!(
            (o.max_inflight, o.max_response, o.max_queries),
            (64 << 20, 8 << 20, 2)
        );
        // A response can't be larger than the in-flight budget.
        let o =
            LockedOptions::parse(Some(r#"{"max_inflight_mb":4,"max_response_mb":100}"#)).unwrap();
        assert_eq!(o.max_response, 4 << 20);
        for bad in [
            r#"{"max_inflight_mb":0}"#,
            r#"{"max_response_mb":-1}"#,
            r#"{"max_queries":"4"}"#,
        ] {
            assert!(LockedOptions::parse(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn search_path_option() {
        assert!(LockedOptions::parse(None).unwrap().search_path.is_empty());
        let o = LockedOptions::parse(Some(r#"{"search_path":"warehouse"}"#)).unwrap();
        assert_eq!(o.search_path, vec![("warehouse".into(), None)]);
        let o = LockedOptions::parse(Some(r#"{"search_path":"warehouse.sales, memory.main"}"#))
            .unwrap();
        assert_eq!(
            o.search_path,
            vec![
                ("warehouse".into(), Some("sales".into())),
                ("memory".into(), Some("main".into()))
            ]
        );
        for bad in [
            r#"{"search_path":""}"#,
            r#"{"search_path":"a.b.c"}"#,
            r#"{"search_path":"a;DROP"}"#,
            r#"{"search_path":""a""}"#,
            r#"{"search_path":"a,,b"}"#,
            r#"{"search_path":["a"]}"#,
        ] {
            assert!(LockedOptions::parse(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn cache_options_and_keys() {
        assert_eq!(LockedOptions::parse(None).unwrap().cache_bytes, 64 << 20);
        let o = LockedOptions::parse(Some(r#"{"cache_mb":0}"#)).unwrap();
        assert_eq!(o.cache_bytes, 0);
        assert!(LockedOptions::parse(Some(r#"{"cache_mb":-1}"#)).is_err());
        let o = LockedOptions::parse(None).unwrap();
        assert!(o.bind.is_loopback() && o.memory_limit.is_none() && o.duckdb_threads.is_none());
        let o = LockedOptions::parse(Some(
            r#"{"bind":"0.0.0.0","memory_limit":"2GB","duckdb_threads":2}"#,
        ))
        .unwrap();
        assert!(o.bind.is_unspecified());
        assert_eq!(o.memory_limit.as_deref(), Some("2GB"));
        assert_eq!(o.duckdb_threads, Some(2));
        assert!(LockedOptions::parse(Some(r#"{"bind":"[::]"}"#))
            .unwrap()
            .bind
            .is_unspecified());
        assert!(LockedOptions::parse(Some(r#"{"bind":"example.com"}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"memory_limit":"1GB'; x"}"#)).is_err());
        for ok in ["2GB", "512 MiB", "1.5gb", "50%", "4096MB"] {
            assert!(valid_memory_limit(ok), "{ok}");
        }
        for bad in ["", "GB", "2", "2 parsecs", "0GB", "150%", "-1GB", "1e9GB"] {
            assert!(!valid_memory_limit(bad), "{bad}");
        }
        assert!(LockedOptions::parse(Some(r#"{"bind":"localhost"}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"bind":"1.2.3"}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"bind":1}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"duckdb_threads":"4"}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"duckdb_threads":0}"#)).is_err());
        // The variables' order doesn't matter; their values, the panel, the
        // dashboard and the page do.
        let key = |body: &str| panel_key(&sv::parse_panel_request(body).unwrap(), None, &Deps::All);
        let k = key(r#"{"dashboard":"d","panel":1,"vars":{"a":1,"b":"x"}}"#);
        assert_eq!(
            k,
            key(r#"{"dashboard":"d","panel":1,"vars":{"b":"x","a":1}}"#)
        );
        for other in [
            r#"{"dashboard":"d","panel":1,"vars":{"a":1,"b":"y"}}"#,
            r#"{"dashboard":"d","panel":1,"vars":{"a":"1","b":"x"}}"#,
            r#"{"dashboard":"d","panel":2,"vars":{"a":1,"b":"x"}}"#,
            r#"{"dashboard":"e","panel":1,"vars":{"a":1,"b":"x"}}"#,
            r#"{"dashboard":"d","panel":1,"vars":{"a":1,"b":"x"},"page":{"limit":10}}"#,
        ] {
            assert_ne!(k, key(other), "{other}");
        }
        assert_ne!(
            key(r#"{"dashboard":"d","panel":1,"page":{"limit":10,"offset":10}}"#),
            key(r#"{"dashboard":"d","panel":1,"page":{"limit":10,"offset":20}}"#)
        );
        assert!(etag("s1", 1, &k).starts_with("\"s1.1-"));
        assert_ne!(etag("s1", 1, &k), etag("s1", 2, &k));
        assert_ne!(etag("s1", 1, &k), etag("s2", 1, &k));
    }

    /// #210: the key holds only the variables `deps` says the panel can
    /// depend on; claims, panel and page still always distinguish it.
    #[test]
    fn keys_follow_the_variables_a_statement_reads() {
        let s = Deps::Vars(["series".to_string()].into());
        let key =
            |body: &str, d: &Deps| panel_key(&sv::parse_panel_request(body).unwrap(), None, d);

        let k1 = key(
            r#"{"dashboard":"d","panel":0,"vars":{"series":"a","region":"x"}}"#,
            &s,
        );
        assert_eq!(
            k1,
            key(
                r#"{"dashboard":"d","panel":0,"vars":{"series":"a","region":"y"}}"#,
                &s
            )
        );
        assert_eq!(
            k1,
            key(r#"{"dashboard":"d","panel":0,"vars":{"series":"a"}}"#, &s)
        );
        assert_ne!(
            k1,
            key(r#"{"dashboard":"d","panel":0,"vars":{"series":"b"}}"#, &s)
        );
        // Unset differs from set.
        assert_ne!(
            key(r#"{"dashboard":"d","panel":0,"vars":{}}"#, &s),
            key(r#"{"dashboard":"d","panel":0,"vars":{"series":"a"}}"#, &s)
        );
        // Case-insensitive.
        assert_eq!(
            key(r#"{"dashboard":"d","panel":0,"vars":{"Series":"a"}}"#, &s),
            key(r#"{"dashboard":"d","panel":0,"vars":{"series":"a"}}"#, &s)
        );

        // With Deps::default() (none): any variable value gives the same
        // key, but claims, panel and page still differ it.
        let none = Deps::default();
        let ka = key(r#"{"dashboard":"d","panel":0,"vars":{"a":1}}"#, &none);
        assert_eq!(
            ka,
            key(r#"{"dashboard":"d","panel":0,"vars":{"a":2}}"#, &none)
        );
        let r = sv::parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{"a":1}}"#).unwrap();
        let alice = Claims {
            user: "alice".into(),
            ..Default::default()
        };
        let bob = Claims {
            user: "bob".into(),
            ..Default::default()
        };
        assert_ne!(
            panel_key(&r, Some(&alice), &none),
            panel_key(&r, Some(&bob), &none)
        );
        assert_ne!(
            panel_key(&r, Some(&alice), &none),
            panel_key(&r, None, &none)
        );
        assert_ne!(
            ka,
            key(r#"{"dashboard":"d","panel":1,"vars":{"a":1}}"#, &none)
        );
        assert_ne!(
            ka,
            key(
                r#"{"dashboard":"d","panel":0,"vars":{"a":1},"page":{"limit":10}}"#,
                &none
            )
        );

        // With Deps::All (today's behaviour): every variable keys the result.
        assert_ne!(
            key(
                r#"{"dashboard":"d","panel":0,"vars":{"region":"x"}}"#,
                &Deps::All
            ),
            key(
                r#"{"dashboard":"d","panel":0,"vars":{"region":"y"}}"#,
                &Deps::All
            )
        );

        // Through a real PanelCache: a variable outside `deps` is a hit; one
        // inside it runs again. Claims never filter — bob still gets his own
        // run even with empty deps.
        let cache = crate::panel_cache::PanelCache::new(1 << 20);
        let k_a_x = key(
            r#"{"dashboard":"d","panel":0,"vars":{"series":"a","region":"x"}}"#,
            &s,
        );
        let (out, how) = cache.run(&k_a_x, None, |_| Ok("a@x".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("a@x", Served::Executed));
        let k_a_y = key(
            r#"{"dashboard":"d","panel":0,"vars":{"series":"a","region":"y"}}"#,
            &s,
        );
        let (out, how) = cache.run(&k_a_y, None, |_| Ok("rerun".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("a@x", Served::Hit));
        let k_b = key(r#"{"dashboard":"d","panel":0,"vars":{"series":"b"}}"#, &s);
        let (out, how) = cache.run(&k_b, None, |_| Ok("b".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("b", Served::Executed));

        let r = sv::parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{}}"#).unwrap();
        let alice_key = panel_key(&r, Some(&alice), &Deps::default());
        let (out, how) = cache.run(&alice_key, None, |_| Ok("alice".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("alice", Served::Executed));
        let bob_key = panel_key(&r, Some(&bob), &Deps::default());
        let (out, how) = cache.run(&bob_key, None, |_| Ok("bob".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("bob", Served::Executed));
    }

    /// Security review of #210/#222 (M1): `locked_panel` must execute exactly
    /// what the key records, so a variable outside the panel's deps (which a
    /// viewer could set to make its own `SET VARIABLE` fail: a reserved word,
    /// a mixed-type list, …) is dropped before either the key or the `SET`s
    /// are built — the same step `locked_panel` performs on `r.vars`.
    #[test]
    fn a_variable_outside_deps_is_dropped_before_the_key_and_the_sets() {
        let mut r = sv::parse_panel_request(
            r#"{"dashboard":"d","panel":1,"vars":{"region":"EU","z":[1,2]}}"#,
        )
        .unwrap();
        let deps = Deps::Vars(["region".to_string()].into());
        r.vars.retain(|(n, _)| deps.contains(n));
        assert_eq!(r.vars, vec![("region".to_string(), "'EU'".to_string())]);
        let sets = request_sets(&r, None, None);
        assert!(!sets.iter().any(|s| s.contains('z')), "{sets:?}");
        // The key no longer differs by `z`: two requests that agree on
        // `region` but not on `z` now share one key (the whole point — they
        // compute the same true answer), instead of two keys whose `SET`s
        // disagreed with what the key recorded.
        let mut r2 = sv::parse_panel_request(
            r#"{"dashboard":"d","panel":1,"vars":{"region":"EU","z":[9,9]}}"#,
        )
        .unwrap();
        r2.vars.retain(|(n, _)| deps.contains(n));
        assert_eq!(panel_key(&r, None, &deps), panel_key(&r2, None, &deps));
    }

    /// Security review of #210/#222 (L1): a credential's locked variable is
    /// never narrowed away, whatever the analysis says a panel reads —
    /// always in the key, always `SET`.
    #[test]
    fn credential_locked_variables_are_never_narrowed() {
        let mut r = sv::parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{}}"#).unwrap();
        let scope = Scope {
            dashboards: Some(vec!["d".into()]),
            vars: vec![("tenant".into(), "'acme'".into())],
        };
        scope.lock_vars(&mut r.vars);
        // The analysis says this panel reads nothing at all — not even
        // `tenant` — yet the locked variable must still count.
        let deps = Deps::default();
        let mut key_deps = deps.clone();
        key_deps.extend(&Deps::Vars(
            scope
                .vars
                .iter()
                .map(|(n, _)| n.to_ascii_lowercase())
                .collect(),
        ));
        assert!(key_deps.contains("tenant"));
        r.vars.retain(|(n, _)| key_deps.contains(n));
        assert_eq!(r.vars, vec![("tenant".to_string(), "'acme'".to_string())]);
        let sets = request_sets(&r, None, None);
        assert!(
            sets.iter()
                .any(|s| s.contains("tenant") || s.contains("TENANT")),
            "{sets:?}"
        );
        // A different tenant must give a different key even though `deps`
        // alone says nothing is read.
        let mut r2 = sv::parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{}}"#).unwrap();
        let scope2 = Scope {
            dashboards: Some(vec!["d".into()]),
            vars: vec![("tenant".into(), "'globex'".into())],
        };
        scope2.lock_vars(&mut r2.vars);
        assert_ne!(
            panel_key(&r, None, &key_deps),
            panel_key(&r2, None, &key_deps)
        );
    }

    #[test]
    fn refresh_and_data_options() {
        let o = LockedOptions::parse(Some(r#"{"refresh_s":60}"#)).unwrap();
        assert_eq!(o.refresh, Some(Duration::from_secs(60)));
        assert!(LockedOptions::parse(None).unwrap().refresh.is_none());
        assert!(LockedOptions::parse(Some(r#"{"refresh_s":0}"#)).is_err());
        // `data` is canonicalised (no `..` or symlink survives) and must be a
        // directory other than the root.
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let json = |d: &str| format!(r#"{{"data":{}}}"#, serde_json::json!(d));
        let sub = SnapshotDir::create().unwrap(); // a private folder under tmp
        let dir = sub.0.canonicalize().unwrap();
        std::fs::create_dir(dir.join("run")).unwrap();
        let o = LockedOptions::parse(Some(&json(&format!("{}/./run/..", dir.display())))).unwrap();
        assert_eq!(o.data.as_deref(), Some(dir.as_path()));
        // too broad: the temp directory itself (it holds the snapshots)
        assert!(LockedOptions::parse(Some(&json(&tmp.to_string_lossy()))).is_err());
        assert!(LockedOptions::parse(Some(&json("/no/such/dir/anofox"))).is_err());
        assert!(LockedOptions::parse(Some(&json("/"))).is_err());
        assert!(LockedOptions::parse(Some(&json(""))).is_err());
        let f = tmp.join(format!("anofox-opt-{}", sv::random_token()));
        std::fs::write(&f, "x").unwrap();
        assert!(LockedOptions::parse(Some(&json(&f.to_string_lossy()))).is_err());
        let _ = std::fs::remove_file(&f);
        // path_vars: identifiers, and only together with data.
        let ok = format!(
            r#"{{"data":{},"path_vars":["run"]}}"#,
            serde_json::json!(dir.to_string_lossy())
        );
        assert_eq!(
            LockedOptions::parse(Some(&ok)).unwrap().path_vars,
            vec!["run".to_string()]
        );
        assert!(LockedOptions::parse(Some(r#"{"path_vars":["run"]}"#)).is_err());
        let bad = ok.replace("\"run\"", "\"a-b\"");
        assert!(LockedOptions::parse(Some(&bad)).is_err());
    }

    #[test]
    fn policy_option() {
        let dir = SnapshotDir::create().unwrap();
        let f = dir.0.join("p.json");
        let opt = |p: &Path| format!(r#"{{"policy":{}}}"#, serde_json::json!(p.to_string_lossy()));
        std::fs::write(&f, r#"{"tables": {"sales": {"rows": true}}}"#).unwrap();
        let o = LockedOptions::parse(Some(&opt(&f))).unwrap();
        assert_eq!(o.policy.unwrap().tables[0].table, "sales");
        // A policy that can't be read or parsed stops the start.
        assert!(LockedOptions::parse(Some(&opt(&dir.0.join("nope.json")))).is_err());
        std::fs::write(&f, r#"{"tables": {"sales": {}}}"#).unwrap();
        assert!(LockedOptions::parse(Some(&opt(&f))).is_err());
        assert!(LockedOptions::parse(Some(r#"{"policy":""}"#)).is_err());
        let o = LockedOptions::parse(Some(r#"{"policy":"none"}"#)).unwrap();
        assert!(o.policy_none && o.policy.is_none());
        // The hash key: from the environment, long enough, never printed.
        std::env::set_var("ANOFOX_TEST_POLICY_KEY", "k".repeat(40));
        std::env::set_var("ANOFOX_TEST_POLICY_KEY_SHORT", "short");
        let o =
            LockedOptions::parse(Some(r#"{"policy_key_env":"ANOFOX_TEST_POLICY_KEY"}"#)).unwrap();
        assert!(!format!("{o:?}").contains(&"k".repeat(40)));
        for bad in [
            r#"{"policy_key_env":"ANOFOX_TEST_POLICY_KEY_SHORT"}"#,
            r#"{"policy_key_env":"ANOFOX_TEST_NO_SUCH_VAR"}"#,
        ] {
            let e = LockedOptions::parse(Some(bad)).unwrap_err();
            assert!(!e.contains("short"), "{e}");
        }
    }

    #[test]
    fn path_values_cannot_traverse() {
        let lit = |v: serde_json::Value| sv::sql_literal(&v).unwrap();
        for ok in ["run_2026-10-16", "r1.v2", "42"] {
            assert!(valid_path_value(&lit(serde_json::json!(ok))), "{ok}");
        }
        assert!(valid_path_value(&lit(serde_json::json!(42))));
        assert!(valid_path_value(&lit(serde_json::Value::Null)));
        for bad in [
            "..",
            ".",
            ".hidden",
            "../secret",
            "a/b",
            "/etc/passwd",
            "a\\b",
            "",
            "a'b",
            "run 1",
            "*",
            "r?n",
            "~",
            "%2e%2e",
        ] {
            assert!(
                !valid_path_value(&lit(serde_json::json!(bad))),
                "{bad:?} must be refused"
            );
        }
        for bad in [
            serde_json::json!(["a", "b"]),
            serde_json::json!(true),
            serde_json::json!(-1),
            serde_json::json!(1.5),
        ] {
            assert!(!valid_path_value(&lit(bad.clone())), "{bad}");
        }
    }

    /// DuckDB's `json_serialize_sql` through the `duckdb` CLI; `None` without
    /// one (the test is skipped locally, but fails on CI — `CI` set — where
    /// the workflow installs the pinned CLI).
    /// A `policy::Duck` over the `duckdb` CLI (one VARCHAR column).
    fn cli_rows(sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        let mut cmd = std::process::Command::new("duckdb");
        cmd.args(["-noheader", "-list", "-c", sql]);
        anofox_visualization::host::duckdb_cli::prepare("duckdb", &mut cmd);
        let out = cmd.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(vec![vec![Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )]])
    }

    fn cli_serialize(sql: &str) -> Option<Result<String, String>> {
        let q = format!(
            "SELECT json_serialize_sql({})::VARCHAR AS j",
            sql_string_literal(sql)
        );
        let mut cmd = std::process::Command::new("duckdb");
        cmd.args(["-noheader", "-list", "-c", &q]);
        anofox_visualization::host::duckdb_cli::prepare("duckdb", &mut cmd);
        let out = cmd.output().ok()?;
        Some(if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        })
    }

    #[test]
    fn only_queries_run_with_a_data_folder() {
        match cli_serialize("SELECT 1") {
            Some(Ok(out)) if out.contains("\"error\":false") => {}
            _ if std::env::var_os("CI").is_some() => {
                panic!("CI needs a working `duckdb` CLI on PATH (json_serialize_sql) for this test")
            }
            _ => {
                eprintln!("skipped: no working duckdb CLI");
                return;
            }
        }
        let ok = |sql: &str, setup: bool| read_only_statement(sql, setup, &mut cli_rows);
        for sql in [
            "SELECT * FROM read_parquet(getvariable('anofox_data') || '/' || getvariable('run') || '/x.parquet')",
            "FROM t",
            "WITH copy AS (SELECT 1) SELECT * FROM copy",
            "(SELECT 1) UNION ALL (SELECT 2)",
            "VALUES (1), (2)",
            // a PIVOT with its values listed (one without can't be checked)
            "SELECT * FROM (PIVOT (SELECT 'a' AS k, 1 AS v) ON k IN ('a') USING sum(v))",
        ] {
            assert!(ok(sql, false).is_ok(), "{sql}");
        }
        for sql in [
            "SET VARIABLE lo = (SELECT min(x) FROM t)",
            "set variable lo TO 5;",
            "CREATE TEMP TABLE copy_of AS SELECT 1",
            "CREATE OR REPLACE TEMPORARY VIEW v AS FROM t",
            "create temp table if not exists \"x y\" AS SELECT 1",
        ] {
            assert!(ok(sql, true).is_ok(), "{sql}");
        }
        // #85: every way of running a write (or any other statement) fails,
        // as a panel and as a setup statement.
        let data = "/srv/data";
        for sql in [
            format!("COPY (SELECT 1) TO '{data}/x.csv'"),
            format!("copy t TO '{data}/x'"),
            format!("EXPLAIN ANALYZE COPY (SELECT 'pwn') TO '{data}/w_explain.csv'"),
            format!("EXPLAIN COPY (SELECT 1) TO '{data}/x.csv'"),
            "EXPLAIN SELECT 1".to_string(),
            format!("PREPARE p AS COPY (SELECT 'pwn2') TO '{data}/w_prep.csv'"),
            "PREPARE p AS SELECT 1".to_string(),
            "EXECUTE p".to_string(),
            format!("EXPORT DATABASE '{data}/x'"),
            format!("IMPORT DATABASE '{data}/x'"),
            format!("ATTACH '{data}/x.db' AS x"),
            "DETACH x".to_string(),
            "INSTALL httpfs".to_string(),
            "LOAD httpfs".to_string(),
            "FORCE INSTALL httpfs".to_string(),
            "CHECKPOINT".to_string(),
            "VACUUM".to_string(),
            "PRAGMA database_size".to_string(),
            "CALL pragma_version()".to_string(),
            "SET threads = 1".to_string(),
            "RESET threads".to_string(),
            "USE memory".to_string(),
            "INSERT INTO t SELECT 1".to_string(),
            "CREATE TABLE t AS SELECT 1".to_string(),
            "CREATE MACRO m() AS 1".to_string(),
            format!("CREATE TEMP TABLE t AS EXPLAIN ANALYZE COPY (SELECT 1) TO '{data}/x.csv'"),
            format!("CREATE TEMP TABLE t AS COPY (SELECT 1) TO '{data}/x.csv'"),
            "CREATE TEMP TABLE t (a INT)".to_string(),
            "CREATE TEMP TABLE t(a) AS SELECT 1".to_string(),
            "SET VARIABLE x = 1; COPY (SELECT 1) TO 'x.csv'".to_string(),
            format!("SELECT 1; COPY (SELECT 1) TO '{data}/x.csv'"),
            "SELECT 1; SELECT 2".to_string(),
            "PIVOT (SELECT 'a' AS k, 1 AS v) ON k USING sum(v)".to_string(),
            "SELECT * FROM (PIVOT (SELECT 'a' AS k, 1 AS v) ON k USING sum(v))".to_string(),
            "BEGIN".to_string(),
            "".to_string(),
        ] {
            assert!(ok(&sql, false).is_err(), "panel: {sql}");
            assert!(ok(&sql, true).is_err(), "setup: {sql}");
        }
        // A missing json extension fails closed.
        let mut none =
            |_: &str| Err::<Vec<Vec<Option<String>>>, String>("no json_serialize_sql".into());
        assert!(read_only_statement("SELECT 1", false, &mut none).is_err());
        let mut garbage = |_: &str| Ok::<_, String>(vec![vec![Some("not json".to_string())]]);
        assert!(read_only_statement("SELECT 1", false, &mut garbage).is_err());
    }

    #[test]
    fn setup_forms() {
        assert_eq!(
            policy::statement_query("SET VARIABLE a = 1").as_deref(),
            Ok("SELECT  1")
        );
        assert_eq!(
            policy::statement_query("CREATE TEMP VIEW v AS SELECT 2;").as_deref(),
            Ok(" SELECT 2")
        );
        for sql in [
            "SET a = 1",
            "SET VARIABLEa = 1",
            "CREATE TABLE t AS SELECT 1",
            "CREATE TEMP SEQUENCE s",
            "CREATE TEMP TABLE t",
            "SET VARIABLE \"a = 1",
            "CREATE TEMP TABLE . AS SELECT 1",
            "CREATE TEMP TABLE a. AS SELECT 1",
        ] {
            assert!(policy::statement_query(sql).is_err(), "{sql}");
        }
    }

    #[test]
    fn data_errors_are_redacted() {
        let d = Path::new("/srv/runs");
        assert_eq!(
            redact(
                "Permission Error: Cannot access file \"/srv/runs/a/x.parquet\" and /srv/runs"
                    .into(),
                Some(d)
            ),
            "Permission Error: Cannot access file \"<data>/a/x.parquet\" and <data>"
        );
        assert_eq!(redact("other".into(), Some(d)), "other");
        assert_eq!(redact("/srv/runs".into(), None), "/srv/runs");
    }

    #[test]
    fn broad_data_folders_are_refused() {
        assert!(too_broad(Path::new("/")).is_some());
        for d in ["/etc", "/usr", "/home", "/tmp", "/var"] {
            assert!(too_broad(Path::new(d)).is_some(), "{d}");
        }
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        assert!(too_broad(&tmp).is_some());
        if let Some(home) =
            std::env::var_os("HOME").and_then(|h| PathBuf::from(h).canonicalize().ok())
        {
            assert!(too_broad(&home).is_some());
            assert!(too_broad(home.parent().unwrap()).is_some());
            assert!(too_broad(&home.join("runs")).is_none());
        }
        assert!(too_broad(&tmp.join("anofox-runs")).is_none());
        assert!(too_broad(Path::new("/srv/runs")).is_none());
        assert!(too_broad(Path::new("/data")).is_none());
    }

    #[test]
    fn auth_and_log_options() {
        let secret = "0123456789abcdef0123456789abcdef";
        let o = LockedOptions::parse(Some(&format!(
            r#"{{"auth":{{"mode":"proxy","shared_secret":"{secret}","trusted_proxies":["127.0.0.1"]}},"log":"stderr","log_vars":"none"}}"#
        )))
        .unwrap();
        assert_eq!(
            o.auth.as_ref().unwrap().proxy.as_ref().unwrap().header_user,
            "X-Forwarded-User"
        );
        assert!(o.log.is_some());
        // a broken auth config fails the start (fail closed), never serves open
        assert!(LockedOptions::parse(Some(r#"{"auth":{"mode":"proxy"}}"#)).is_err());
        assert!(
            LockedOptions::parse(Some(r#"{"auth":{"mode":"proxy","shared_secret":"short"}}"#))
                .is_err()
        );
        assert!(LockedOptions::parse(Some(r#"{"auth":true}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"log_vars":"names"}"#)).is_err());
        assert!(LockedOptions::parse(Some(r#"{"log":"stderr","log_vars":"all"}"#)).is_err());
        // the error never echoes the secret
        let e = LockedOptions::parse(Some(&format!(
            r#"{{"auth":{{"mode":"proxy","shared_secret":"{secret}","nope":1}}}}"#
        )))
        .unwrap_err();
        assert!(!e.contains(secret), "{e}");
        // P3: a list of methods and an audit log; audit needs auth
        let esec = "embed-secret-0123456789abcdef0123456789";
        let dir = SnapshotDir::create().unwrap();
        let audit = dir.0.join("audit.jsonl");
        let o = LockedOptions::parse(Some(&format!(
            r#"{{"auth":[{{"mode":"proxy","shared_secret":"{secret}"}},{{"mode":"embed","secret":"{esec}"}}],"audit":{}}}"#,
            serde_json::json!(audit.to_string_lossy())
        )))
        .unwrap();
        assert_eq!(o.auth.as_ref().unwrap().methods(), ["proxy", "embed"]);
        assert!(std::fs::read_to_string(&audit)
            .unwrap()
            .contains("\"audit\":\"start\""));
        assert!(LockedOptions::parse(Some(r#"{"audit":"stderr"}"#)).is_err());
        assert!(LockedOptions::parse(Some(&format!(
            r#"{{"auth":{{"mode":"embed","secret":"{esec}"}},"audit":"/nonexistent-dir/a.jsonl"}}"#
        )))
        .is_err());
    }

    #[test]
    fn locked_path_vars_are_checked_too() {
        // An embed link locking a path variable gets the viewer's rule.
        let pv = vec!["run".to_string()];
        let scope = Scope {
            dashboards: None,
            vars: vec![("RUN".into(), "'../../etc'".into())],
        };
        let mut vars = vec![("run".to_string(), "'run_a'".to_string())];
        assert!(check_path_values(&pv, &vars).is_ok());
        scope.lock_vars(&mut vars);
        assert_eq!(check_path_values(&pv, &scope.vars).unwrap_err().0, 400);
    }

    #[test]
    fn locked_vars_follow_the_viewers_and_precede_the_claims() {
        // An embed link's variables replace the viewer's and are set after
        // them; the claims still come last.
        let mut r = sv::parse_panel_request(
            r#"{"dashboard":"d","panel":0,"vars":{"region":"US","q":"x"}}"#,
        )
        .unwrap();
        let scope = Scope {
            dashboards: Some(vec!["d".into()]),
            vars: vec![("REGION".into(), "'EU'".into())],
        };
        scope.lock_vars(&mut r.vars);
        let claims = Claims {
            user: "acme".into(),
            ..Default::default()
        };
        let sets = request_sets(&r, Some(&claims), None);
        let pos = |needle: &str| sets.iter().position(|s| s.contains(needle)).unwrap();
        assert!(!sets.iter().any(|s| s.contains("'US'")), "{sets:?}");
        assert!(pos("q =") < pos("REGION = 'EU'"));
        assert!(pos("REGION = 'EU'") < pos("__claim_user"));
    }

    /// Security review of #210/#222 (M2): two embed links with the same
    /// claims (no `user`: both are `embed:d`) whose locked variables spell
    /// `tenant` twice, in opposite orders. Canonicalizing `r.vars` once,
    /// after `lock_vars`, keeps only the spelling DuckDB actually runs (the
    /// later `SET` wins), so the two links — which execute to *different*
    /// tenants — get different keys, instead of colliding on one.
    #[test]
    fn two_spellings_of_a_locked_variable_no_longer_collide() {
        let claims = Claims {
            user: "embed:d".into(),
            ..Default::default()
        };
        let s1 = Scope {
            dashboards: Some(vec!["d".into()]),
            vars: vec![
                ("Tenant".into(), "'acme'".into()),
                ("tenant".into(), "'globex'".into()),
            ],
        };
        let s2 = Scope {
            dashboards: Some(vec!["d".into()]),
            vars: vec![
                ("tenant".into(), "'globex'".into()),
                ("Tenant".into(), "'acme'".into()),
            ],
        };
        let deps = Deps::Vars(["tenant".to_string()].into());
        let canon = |s: &Scope| {
            let mut r =
                sv::parse_panel_request(r#"{"dashboard":"d","panel":0,"vars":{}}"#).unwrap();
            s.lock_vars(&mut r.vars);
            r.vars = sv::dedup_last_spelling(r.vars);
            r
        };
        let r1 = canon(&s1);
        let r2 = canon(&s2);
        let k1 = panel_key(&r1, Some(&claims), &deps);
        let k2 = panel_key(&r2, Some(&claims), &deps);
        let sets1 = request_sets(&r1, Some(&claims), None);
        let sets2 = request_sets(&r2, Some(&claims), None);
        // Each link now runs exactly one SET per name (the one DuckDB would
        // keep: the later of the two in that link's own declared order).
        assert_eq!(
            sets1
                .iter()
                .filter(|s| s.contains("TENANT") || s.contains("Tenant") || s.contains("tenant"))
                .count(),
            1,
            "{sets1:?}"
        );
        assert_eq!(
            sets2
                .iter()
                .filter(|s| s.contains("TENANT") || s.contains("Tenant") || s.contains("tenant"))
                .count(),
            1,
            "{sets2:?}"
        );
        assert_ne!(
            k1, k2,
            "different effective tenants must get different keys"
        );
        assert_ne!(sets1, sets2, "and indeed they run different SETs");
    }

    #[test]
    fn claims_isolate_cache_entries() {
        let claims = |user: &str, groups: &[&str]| Claims {
            user: user.into(),
            groups: groups.iter().map(|g| g.to_string()).collect(),
            attrs: vec![],
        };
        let (alice, bob) = (claims("alice", &[]), claims("bob", &[]));
        let alice_hr = claims("alice", &["hr"]);
        let r = sv::parse_panel_request(r#"{"dashboard":"d","panel":1,"vars":{"a":1}}"#).unwrap();
        let ka = panel_key(&r, Some(&alice), &Deps::All);
        assert_eq!(ka, panel_key(&r, Some(&alice.clone()), &Deps::All));
        assert_ne!(ka, panel_key(&r, Some(&bob), &Deps::All));
        assert_ne!(ka, panel_key(&r, Some(&alice_hr), &Deps::All));
        assert_ne!(ka, panel_key(&r, None, &Deps::All));
        // through the real cache: bob never gets alice's rows
        let cache = crate::panel_cache::PanelCache::new(1 << 20);
        let (out, how) = cache.run(&ka, None, |_| Ok("alice rows".to_string()));
        assert_eq!(
            (out.unwrap().as_str(), how),
            ("alice rows", Served::Executed)
        );
        let (out, how) = cache.run(&ka, None, |_| Ok("rerun".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("alice rows", Served::Hit));
        let kb = panel_key(&r, Some(&bob), &Deps::All);
        let (out, how) = cache.run(&kb, None, |_| Ok("bob rows".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("bob rows", Served::Executed));
    }

    #[test]
    fn every_request_sets_the_claims_after_the_viewer_vars() {
        // A run_panel that forgot the claims would still compile — and fail
        // open for filters such as `coalesce(getvariable('__claim_user'), …)`.
        let r = sv::parse_panel_request(r#"{"dashboard":"d","panel":1,"vars":{"region":"EU"}}"#)
            .unwrap();
        let alice = Claims {
            user: "alice".into(),
            groups: vec!["hr".into()],
            attrs: vec![("tenant".into(), None)],
        };
        let sets = request_sets(&r, Some(&alice), Some(Path::new("/srv/data")));
        assert_eq!(
            sets,
            vec![
                "SET VARIABLE region = 'EU'".to_string(),
                "SET VARIABLE __claim_user = 'alice'".to_string(),
                "SET VARIABLE __claim_groups = ['hr']".to_string(),
                "SET VARIABLE __claim_tenant = NULL::VARCHAR".to_string(),
                "SET VARIABLE anofox_data = '/srv/data'".to_string(),
            ]
        );
        // without a login: no claims (they read as NULL), never a default user
        let sets = request_sets(&r, None, None);
        assert_eq!(sets, vec!["SET VARIABLE region = 'EU'".to_string()]);
        // a viewer can't get a reserved name into the list at all
        for v in ["__claim_user", "anofox_data", "ANOFOX_DATA"] {
            let body = format!(r#"{{"dashboard":"d","panel":1,"vars":{{"{v}":"x"}}}}"#);
            assert!(sv::parse_panel_request(&body).is_err(), "{v}");
        }
    }

    #[test]
    fn auth_and_refresh_never_cross_users() {
        // Two users hammer the same panel while the snapshot is refreshed:
        // every answer must be the asking user's, from the snapshot the
        // request started on; the cache is per user (key) and per snapshot
        // (generation), and each (user, snapshot) runs exactly once.
        //
        // Deterministic progress (no sleeps as synchronisation): the refresher
        // rebuilds until every worker is done, and a worker is only done once
        // it has done its requests AND seen MIN_SNAPSHOTS snapshots.
        use crate::generation::Current;
        use std::collections::{BTreeSet, HashMap};
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        const WORKERS: usize = 8;
        const REQUESTS: usize = 400;
        const MIN_SNAPSHOTS: usize = 3;
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let current = Arc::new(Current::new(1u32, 1 << 20));
        let done = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let executions: Arc<Mutex<HashMap<(String, u32), usize>>> = Arc::default();
        let r = sv::parse_panel_request(r#"{"dashboard":"who","panel":0}"#).unwrap();
        let refresher = {
            let (current, stop) = (current.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut v = 1u32;
                while !stop.load(Ordering::SeqCst) {
                    v += 1;
                    current.rebuild(true, || Ok(v)).unwrap();
                    std::thread::yield_now();
                }
                v
            })
        };
        let workers: Vec<_> = (0..WORKERS)
            .map(|w| {
                let (current, done, executions, r) =
                    (current.clone(), done.clone(), executions.clone(), r.clone());
                std::thread::spawn(move || {
                    let user = if w % 2 == 0 { "alice" } else { "bob" };
                    let claims = Claims {
                        user: user.into(),
                        ..Default::default()
                    };
                    let mut seen = BTreeSet::new();
                    let mut i = 0;
                    while i < REQUESTS || seen.len() < MIN_SNAPSHOTS {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "no progress: {i} requests, snapshots seen {seen:?}"
                        );
                        let gen = current.get();
                        let key = panel_key(&r, Some(&claims), &Deps::All);
                        let (out, _) = gen.cache.run(&key, None, |_| {
                            // The "session": what the panel reads is what the
                            // request's SETs put there.
                            let sets = request_sets(&r, Some(&claims), None);
                            let who = sets
                                .iter()
                                .find_map(|s| s.strip_prefix("SET VARIABLE __claim_user = "))
                                .map(|l| l.trim_matches('\'').to_string())
                                .unwrap_or_default();
                            *executions
                                .lock()
                                .unwrap()
                                .entry((who.clone(), gen.snap))
                                .or_default() += 1;
                            Ok(format!("{who}@{}", gen.snap))
                        });
                        let out = out.unwrap();
                        let (who, snap) = out.split_once('@').unwrap();
                        assert_eq!(who, user, "cross-user answer {out}");
                        assert_eq!(
                            snap.parse::<u32>().unwrap(),
                            gen.snap,
                            "cross-snapshot answer {out}"
                        );
                        seen.insert(gen.snap);
                        i += 1;
                    }
                    done.fetch_add(1, Ordering::SeqCst);
                    seen.len()
                })
            })
            .collect();
        let snapshots_seen: Vec<usize> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        stop.store(true, Ordering::SeqCst);
        let last = refresher.join().unwrap();
        assert_eq!(done.load(Ordering::SeqCst), WORKERS);
        // snapshots are numbered 1..=last
        assert!(last as usize >= MIN_SNAPSHOTS, "{last}");
        assert!(snapshots_seen.iter().all(|n| *n >= MIN_SNAPSHOTS));
        // Single-flight + a cache per snapshot: each (user, snapshot) ran
        // exactly once, and only for the two users. (Exactly once holds here
        // because every run succeeds, the cache is far larger than the two
        // entries per snapshot and nothing is superseded; errors, evictions
        // and abandoned runs would legitimately run again.)
        let ex = executions.lock().unwrap();
        assert!(ex.values().all(|n| *n == 1), "{ex:?}");
        assert!(ex.keys().all(|(u, _)| u == "alice" || u == "bob"));
        assert!(ex.len() >= 2 * MIN_SNAPSHOTS);
    }

    #[test]
    fn a_refresh_during_a_request_keeps_its_snapshot() {
        // A request keeps the snapshot it started on, even if the refresh
        // lands while its query runs; its result is cached in that old
        // generation only, so the next request (new snapshot) runs again.
        use crate::generation::Current;
        let current = Current::new(1u32, 1 << 20);
        let r = sv::parse_panel_request(r#"{"dashboard":"who","panel":0}"#).unwrap();
        let alice = Claims {
            user: "alice".into(),
            ..Default::default()
        };
        let key = panel_key(&r, Some(&alice), &Deps::All);
        let gen = current.get();
        let (out, how) = gen.cache.run(&key, None, |_| {
            current.rebuild(true, || Ok(2)).unwrap(); // mid-request
            Ok(format!("alice@{}", gen.snap))
        });
        assert_eq!((out.unwrap().as_str(), how), ("alice@1", Served::Executed));
        let gen2 = current.get();
        assert_eq!(gen2.snap, 2);
        let (out, how) = gen2
            .cache
            .run(&key, None, |_| Ok(format!("alice@{}", gen2.snap)));
        assert_eq!((out.unwrap().as_str(), how), ("alice@2", Served::Executed));
        // and the old generation still answers its own result
        let (out, how) = gen.cache.run(&key, None, |_| unreachable!());
        assert_eq!((out.unwrap().as_str(), how), ("alice@1", Served::Hit));
    }

    #[test]
    fn supersede_header() {
        let id = "0123456789abcdef0123456789ABCDEF";
        assert!(valid_supersede(&format!("{id}/3/load7")));
        assert!(valid_supersede(&format!("{id}/12/rows:1")));
        assert!(!valid_supersede("short/3/load"));
        assert!(!valid_supersede(&format!("{id}/")));
        assert!(!valid_supersede(&format!("{id}/3/a b")));
        assert!(!valid_supersede(&format!("{id}/{}", "x".repeat(65))));
        assert!(!valid_supersede(&format!("{}z/3/load", &id[1..])));
    }

    #[test]
    fn snapshot_dir_is_private_and_removed() {
        let d = SnapshotDir::create().unwrap();
        let p = d.0.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        drop(d);
        assert!(!p.exists());
    }

    // #388: a warm connection's key.
    fn warm_req(panel: usize, vars: &str) -> sv::PanelRequest {
        sv::parse_panel_request(&format!(
            r#"{{"dashboard":"d","panel":{panel},"vars":{vars}}}"#
        ))
        .unwrap()
    }
    fn warm_dash(n: usize) -> Dash {
        let mut d = dash("d", "");
        d.requires = vec![vec![]; n];
        d
    }

    #[test]
    fn warm_key_tracks_what_shapes_the_setup() {
        let stmts = vec![
            (
                true,
                "SET VARIABLE a = coalesce(getvariable('a'), 1)".to_string(),
            ),
            (
                true,
                "CREATE TEMP TABLE w AS SELECT getvariable('a') AS v".to_string(),
            ),
            (false, "SELECT v FROM w".to_string()),
            (false, "SELECT getvariable('b')".to_string()),
        ];
        // Per statement: what the setup before it reads (deps::setup_deps).
        let read_a = Deps::Vars(["a".to_string()].into_iter().collect());
        let deps = vec![
            Deps::default(),
            read_a.clone(),
            read_a.clone(),
            read_a.clone(),
        ];
        let d = warm_dash(4);
        let k = |p: usize, vars: &str| {
            warm_key(&d, Some(&deps), &stmts, &warm_req(p, vars), None, None)
        };
        // Same setup inputs: the same key, whatever else the request sets.
        assert!(k(3, r#"{"a":1,"b":5}"#).is_some());
        assert_eq!(k(3, r#"{"a":1,"b":5}"#), k(3, r#"{"a":1,"b":6}"#));
        assert_eq!(k(3, r#"{"a":1}"#), k(2, r#"{"a":1}"#), "same setup prefix");
        // A different value of a variable the setup reads: another key.
        assert_ne!(k(3, r#"{"a":1}"#), k(3, r#"{"a":2}"#));
        // Unsure analysis (All): every variable counts.
        let all = vec![Deps::All; 4];
        let ka = |vars: &str| warm_key(&d, Some(&all), &stmts, &warm_req(3, vars), None, None);
        assert_ne!(ka(r#"{"b":5}"#), ka(r#"{"b":6}"#));
        // A viewer's login: never shared.
        let c = |u: &str| Claims {
            user: u.into(),
            groups: vec![],
            attrs: vec![],
        };
        let (ca, cb) = (c("ann"), c("bob"));
        let r = warm_req(3, "{}");
        assert_ne!(
            warm_key(&d, Some(&deps), &stmts, &r, Some(&ca), None),
            warm_key(&d, Some(&deps), &stmts, &r, Some(&cb), None)
        );
        // A credential-locked variable always counts, even unread.
        let sc = |v: &str| Scope {
            dashboards: None,
            vars: vec![("b".into(), v.into())],
        };
        let (s5, s6) = (sc("5"), sc("6"));
        let rb = |v: &str| warm_req(3, &format!(r#"{{"b":{v}}}"#));
        assert_ne!(
            warm_key(&d, Some(&deps), &stmts, &rb("5"), None, Some(&s5)),
            warm_key(&d, Some(&deps), &stmts, &rb("6"), None, Some(&s6))
        );
    }

    #[test]
    fn warm_key_none_without_setup_or_with_requires() {
        let only_set = vec![
            (true, "SET VARIABLE a = 1".to_string()),
            (false, "SELECT 1".to_string()),
        ];
        let d = warm_dash(2);
        assert_eq!(
            warm_key(&d, None, &only_set, &warm_req(1, "{}"), None, None),
            None
        );
        let stmts = vec![
            (true, "CREATE TEMP VIEW v AS SELECT 1".to_string()),
            (false, "SELECT * FROM v".to_string()),
        ];
        assert!(warm_key(&d, None, &stmts, &warm_req(1, "{}"), None, None).is_some());
        assert_eq!(
            warm_key(&d, None, &stmts, &warm_req(0, "{}"), None, None),
            None
        );
        let mut gated = warm_dash(2);
        gated.requires[0] =
            anofox_visualization::sql::requires::parse("-- @requires x.parquet\nSELECT 1");
        assert!(!gated.requires[0].is_empty());
        assert_eq!(
            warm_key(&gated, None, &stmts, &warm_req(1, "{}"), None, None),
            None
        );
    }

    #[test]
    fn sets_variables_is_a_set_or_reset() {
        assert!(sets_variables("SET VARIABLE a = 1"));
        assert!(sets_variables("-- default\n  reset variable a"));
        assert!(!sets_variables("CREATE TEMP TABLE t AS SELECT 1"));
        assert!(!sets_variables("SELECT 'SET'"));
        assert!(!sets_variables("SETTINGS_TABLE"));
    }

    // #252: the pages' CSP header.
    #[test]
    fn csp_matches_assist_csp_origins() {
        let js = include_str!("../../web/assist-csp.js");
        for (name, v) in [
            ("DUCKDB_WASM", CDN_DUCKDB_WASM),
            ("KATEX", CDN_KATEX),
            ("DUCKDB_EXT", DUCKDB_EXT),
            ("USGS", USGS),
        ] {
            assert!(
                js.contains(&format!("export const {name} = \"{v}\";")),
                "{name} differs from web/assist-csp.js"
            );
        }
    }

    #[test]
    fn csp_nonces_every_inline_script() {
        let opts = LockedOptions::default();
        let html = "<head><script>window.__served={};</script><script type=\"module\" src=\"./app.js\"></script></head><body><script>x()</script>";
        let (body, csp) = with_csp(html, &opts);
        let body = String::from_utf8(body).unwrap();
        let nonce = csp
            .split("'nonce-")
            .nth(1)
            .unwrap()
            .split('\'')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(
            body.matches(&format!("<script nonce=\"{nonce}\">")).count(),
            2
        );
        assert!(!body.contains("<script>"));
        assert!(body.contains("<script type=\"module\" src=\"./app.js\">"));
        assert!(csp.contains("frame-ancestors 'none'") && csp.contains("object-src 'none'"));
        let (_, csp2) = with_csp(html, &opts);
        assert_ne!(csp, csp2, "a fresh nonce per response");
    }

    #[test]
    fn frame_ancestors_defaults_and_validation() {
        let mut o = LockedOptions::default();
        assert_eq!(frame_ancestors(&o), "'none'");
        o.frame_ancestors = Some(vec!["https://app.example.com".into(), "'self'".into()]);
        assert_eq!(frame_ancestors(&o), "https://app.example.com 'self'");
        for ok in ["https://a.b", "https://a.b:8443", "'self'", "'none'"] {
            assert!(valid_frame_ancestor(ok), "{ok}");
        }
        for bad in [
            "http://a.b",
            "*",
            "https://",
            "https://a.b/x",
            "https://a b",
            "https://a.b:x",
            "data:",
            "https://a.b;script-src *",
        ] {
            assert!(!valid_frame_ancestor(bad), "{bad}");
        }
    }
}

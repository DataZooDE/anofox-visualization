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
//! * `anofox_serve_reload(port)` — read the dashboards folder again and swap
//!   it in with a new snapshot (#772); on an error the old set keeps serving.
//! * `anofox_serve_stop(port)` — stop a server and delete its snapshot.
//!
//! Each server's state is owned by the server (no process-wide "last load
//! wins" globals besides the port registry), and startup is transactional:
//! nothing is registered until the snapshot is built and the port is bound.

use crate::db::{json_rows, Conn, Database};
use crate::generation::{Current, Generation};
use crate::limits::{Charge, Inflight, Queries};
use crate::panel_cache::{Served, SUPERSEDED};
use anofox_visualization::dashboard;
use anofox_visualization::host::access::{self, Claims, LogEntry, RequestLog};
use anofox_visualization::host::app_brand::{self, AppBrand};
use anofox_visualization::host::assist;
use anofox_visualization::host::auth::{self, audit::AuditLog, Auth, Scope};
use anofox_visualization::host::ingest::{self, IngestOptions};
use anofox_visualization::host::metrics;
use anofox_visualization::host::notify;
use anofox_visualization::host::policy::{self, Enforcer, Policy, Script, Viewer};
use anofox_visualization::host::serving::{
    self as sv, content_type, json_for_script, quote_ident, sql_string_literal, AuthoringGuard,
    RequestMeta,
};
use anofox_visualization::host::site_css;
use anofox_visualization::host::state::{
    NewRecord, RateLimiter, Record, StartContext, StateOptions, Store,
};
use anofox_visualization::host::tasks;
use anofox_visualization::host::telemetry;
use anofox_visualization::host::writeback::{self as wb, WritebackOptions};
use anofox_visualization::sql::deps::{self, Deps};
use anofox_visualization::sql::materialize;
use anofox_visualization::sql::requires::{self as rq, Requirement};
use anofox_visualization::{InputKind, Role};
use include_dir::{include_dir, Dir};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, ListenAddr, Method, Request, Response, Server};

// The browser builder, embedded at compile time (needs web/pkg from wasm-pack —
// build-native.sh and CI build it first).
static WEB: Dir = include_dir!("$CARGO_MANIFEST_DIR/../web");

/// Per-database context handed to the serving functions as extra_info: the
/// connection opened at LOAD time on *that* database (so two databases that
/// load the extension each serve their own data).
pub struct DbCtx {
    pub conn: Mutex<Conn>,
    /// A second connection on the same database as `conn`, opened right
    /// next to it at LOAD time (same thread `conn`'s own `duckdb_connect`
    /// succeeded from) — *not* opened lazily from a server worker thread
    /// later: that crashed when tried (#199 M1; this C-API's `duckdb_connect`
    /// is apparently not safe to call from an arbitrary later thread, only
    /// cross-thread *use* of an already-open connection is, per `Conn`'s own
    /// doc). `/check` locks and reuses this one for its isolated scratch
    /// session (ATTACH/USE/DETACH per call, `scratch_check`) — the live
    /// `conn` above is never touched by a check.
    pub check: Mutex<Conn>,
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
    /// #767: the task runner (`options.tasks.run`), joined like the refresher.
    tasks: Option<JoinHandle<()>>,
    /// #768: the report runner (`options.notify.run`), joined like the refresher.
    reports: Option<JoinHandle<()>>,
    /// #769: the ingest refresher (`options.ingest.refresh`), joined likewise.
    ingest: Option<JoinHandle<()>>,
    /// #775: the durability snapshots (`options.durability.run`), joined
    /// likewise after a last snapshot.
    durable: Option<JoinHandle<()>>,
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
    if let Some(h) = r.tasks.take() {
        join_bounded(vec![h], STOP_JOIN_BUDGET);
    }
    if let Some(h) = r.reports.take() {
        join_bounded(vec![h], STOP_JOIN_BUDGET);
    }
    if let Some(h) = r.ingest.take() {
        join_bounded(vec![h], STOP_JOIN_BUDGET);
    }
    if let Some(h) = r.durable.take() {
        // A last snapshot of what was written up to the stop (an upload
        // can take longer than the others' budget).
        join_bounded(vec![h], Duration::from_secs(30));
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
    respond_csp(req, status, ctype, body, false)
}

/// [`respond`]; `duckdb_esm`: the page may import DuckDB-Wasm's `+esm`
/// build ([`sv::page_csp_with`]) — the authoring builder with the `/assist`
/// proxy on (#740).
fn respond_csp(req: Request, status: u16, ctype: &str, body: impl Into<Vec<u8>>, duckdb_esm: bool) {
    let mut body = body.into();
    // An HTML page (the authoring builder's index.html) gets its CSP header
    // and nonces (#252): authoring is loopback-only and never framed.
    let csp = ctype.starts_with("text/html").then(|| {
        let (html, csp) = sv::csp_page_with(&String::from_utf8_lossy(&body), "'none'", duckdb_esm);
        body = html.into_bytes();
        csp
    });
    let mut resp = Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", ctype))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"));
    if let Some(csp) = csp {
        resp = resp.with_header(header("Content-Security-Policy", &csp));
    }
    let _ = req.respond(resp);
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
    // #773: the operator's stylesheet, last in <head> so it wins over the
    // theme at equal specificity.
    let linked;
    let html = if opts.site_css.is_some() {
        linked = html.replacen(
            "</head>",
            &format!(
                "<link rel=\"stylesheet\" href=\"{}\"></head>",
                site_css::PATH
            ),
            1,
        );
        linked.as_str()
    } else {
        html
    };
    let (body, csp) = sv::csp_page(html, &frame_ancestors(opts));
    (body.into_bytes(), csp)
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
    // A login page gets its CSP header and nonces too (#252); never framed.
    let (body, csp) = if r.content_type.starts_with("text/html") {
        let (html, csp) = sv::csp_page(&String::from_utf8_lossy(&r.body), "'none'");
        (html.into_bytes(), Some(csp))
    } else {
        (r.body, None)
    };
    let mut resp = Response::from_data(body)
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
    if let Some(csp) = csp {
        resp = resp.with_header(header("Content-Security-Policy", &csp));
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
fn serve_asset(req: Request, path: &str, head: &str, duckdb_esm: bool) {
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
            respond_csp(req, 200, content_type(asset), html.into_bytes(), duckdb_esm)
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
    /// `options.assets` (#522), already [`anofox_visualization::image::
    /// validated_assets_root`]-checked at start — `None` (the default)
    /// means exactly today's behaviour: no `/assets/<rest>` route, every
    /// relative `::IMAGE`/markdown source draws the alt-text placeholder.
    /// Unlike the locked server's `assets/` subfolder convention, this is
    /// an explicit opt-in naming the root directly (authoring has no
    /// dashboards folder to derive one from — plan-514 decision 5, now
    /// overridden only when this option is given).
    assets_root: Option<PathBuf>,
    /// #740: the `/assist` proxy, when a key is configured ([`start_assist`]).
    proxy: Option<assist::Proxy>,
}

/// `options.assets`: pulled out of `options` before [`AppBrand::from_json`]
/// sees the rest, so an unrecognised key there still refuses loudly (#522)
/// — `AppBrand`'s own validation is otherwise untouched.
fn parse_assets_option(
    options: &str,
) -> Result<(Option<PathBuf>, assist::AssistOptions, bool, String), String> {
    let mut v: serde_json::Value =
        serde_json::from_str(options).map_err(|e| format!("bad JSON: {e}"))?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| "options must be a JSON object".to_string())?;
    // #740: the `assist` / `assist_*` keys, same as `assets` below.
    let asked = obj.keys().any(|k| k.starts_with("assist_"));
    let assist = assist::AssistOptions::take(obj)?;
    let assets = match obj.remove("assets") {
        None => None,
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(
            anofox_visualization::image::validated_assets_root(Path::new(&s))
                .map_err(|e| format!("options.assets: {e}"))?,
        ),
        Some(_) => return Err("options.assets must be a non-empty string path".into()),
    };
    Ok((assets, assist, asked, v.to_string()))
}

/// Review of #604 S1: `options.assets` reads a file with plain `std::fs`
/// ([`anofox_visualization::image::read_asset`]), never through the live
/// session's own DuckDB connection — so, unlike every *SQL* read on that
/// connection, it was not held to that connection's own
/// `enable_external_access`/`allowed_directories` lockdown at all. Someone
/// running untrusted SQL in a session deliberately locked down that way
/// (`enable_external_access = false`, the posture `docs/secure-serving.md`
/// recommends before loading this extension at all for that purpose) could
/// otherwise still get `anofox_serve(port, {"assets": "/anything"})` to
/// read any image-typed file the process can read, handed to whoever gets
/// the token back. Refuse the option outright instead:
/// - `enable_external_access` must be `true` — fails closed: a query
///   error, a missing setting or anything but the literal string `"true"`
///   refuses, the same as the setting being off.
/// - if `allowed_directories` is non-empty, `root` must canonicalize to a
///   path inside at least one of them.
///
/// Checked once at start, on `ctx`'s own live connection (the one `/query`
/// itself runs on) — never per asset request, same posture as
/// `validated_assets_root` itself.
fn assets_option_allowed(conn: &Conn, root: &Path) -> Result<(), String> {
    if !external_access_on(conn) {
        return Err(
            "options.assets needs enable_external_access (it reads files with std::fs, \
             bypassing that setting otherwise) — SET enable_external_access = true on this \
             session first, or leave options.assets out"
                .into(),
        );
    }
    inside_allowed_directories(conn, "options.assets", root)
}

/// `enable_external_access` is the literal `true` (fails closed).
fn external_access_on(conn: &Conn) -> bool {
    conn.query("SELECT current_setting('enable_external_access')::VARCHAR")
        .and_then(|mut r| r.string_rows(1))
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| row.into_iter().next().flatten())
        .as_deref()
        == Some("true")
}

/// With a non-empty `allowed_directories`, `path` (canonical) must be inside
/// one of them.
fn inside_allowed_directories(conn: &Conn, what: &str, path: &Path) -> Result<(), String> {
    let allowed: Vec<String> = conn
        .query("SELECT unnest(current_setting('allowed_directories')) AS d")
        .and_then(|mut r| r.string_rows(4096))
        .map(|rows| {
            rows.into_iter()
                .filter_map(|row| row.into_iter().next().flatten())
                .collect()
        })
        .unwrap_or_default();
    if allowed.is_empty() {
        return Ok(());
    }
    let inside = allowed.iter().any(|d| {
        Path::new(d)
            .canonicalize()
            .is_ok_and(|real| path.starts_with(&real))
    });
    if !inside {
        return Err(format!(
            "{what} '{}' is outside every directory in allowed_directories ({})",
            path.display(),
            allowed.join(", ")
        ));
    }
    Ok(())
}

/// #740: the `/assist` proxy of an authoring server, from `options`'
/// `assist_*` keys and the key (`ANTHROPIC_API_KEY` or
/// `options.assist_key_file`). The proxy calls out of the process and its
/// key file and log are plain `std::fs`, all past DuckDB's own lockdown, so
/// on a session with `enable_external_access = false` it stays off: an
/// error when an `assist_*` option asked for it, a note otherwise (a key in
/// the environment alone). The key file and the log must be inside
/// `allowed_directories` when that is set.
fn start_assist(
    conn: &Conn,
    a: assist::AssistOptions,
    asked: bool,
) -> Result<(Option<assist::Proxy>, String), String> {
    let has_key =
        a.key_file.is_some() || std::env::var(assist::KEY_ENV).is_ok_and(|k| !k.trim().is_empty());
    if a.off || !has_key {
        return Ok((None, String::new()));
    }
    if !external_access_on(conn) {
        if asked {
            return Err(
                "options.assist_* needs enable_external_access (the assistant's proxy calls the \
                 model API and reads its key file outside DuckDB) — SET enable_external_access = \
                 true on this session first, or leave the assist options out"
                    .into(),
            );
        }
        return Ok((
            None,
            "; assistant proxy off: enable_external_access is false on this session".into(),
        ));
    }
    if let Some(f) = &a.key_file {
        let real = f
            .canonicalize()
            .map_err(|e| format!("options.assist_key_file {}: {e}", f.display()))?;
        inside_allowed_directories(conn, "options.assist_key_file", &real)?;
    }
    if let Some(l) = a
        .log
        .as_deref()
        .filter(|l| !matches!(l.trim(), "stderr" | "-"))
    {
        let p = Path::new(l);
        let dir = p
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let real = dir
            .canonicalize()
            .map_err(|e| format!("options.assist_log {l}: {e}"))?;
        inside_allowed_directories(conn, "options.assist_log", &real)?;
    }
    match assist::build(false, a.cfg, a.key_file.as_deref(), a.log.as_deref())? {
        Some((p, note)) => Ok((Some(p), format!("; {note}"))),
        None => Ok((None, String::new())),
    }
}

/// `anofox_serve(port[, options])`: start the authoring server on
/// 127.0.0.1:`port`; `options` sets the builder's app bar (name, tagline,
/// logo, logo_dark, link, favicon — see `AppBrand`) and, opt-in,
/// `options.assets` (#522): a directory the builder also serves relative
/// `::IMAGE`/markdown image sources from at `GET/HEAD /assets/<rest>`,
/// under the same token/loopback/origin guard as every other authoring
/// route. Off (`None`) unless given — authoring otherwise has no
/// dashboards folder to serve `assets/` from at all (plan-514 decision 5).
pub fn start_authoring(
    ctx: Arc<DbCtx>,
    port: u16,
    options: Option<&str>,
) -> Result<String, String> {
    let (assets_root, assist_opts, assist_asked, app) = match options {
        Some(json) => {
            let (assets, assist_opts, asked, rest) = parse_assets_option(json)?;
            if let Some(root) = &assets {
                // #604 S1: before accepting the option at all, not after
                // the server is already listening.
                assets_option_allowed(&ctx.conn.lock().unwrap_or_else(|p| p.into_inner()), root)?;
            }
            // Review of #604 N3: `assets` is already gone from `rest` by
            // this point (pulled out above), so `AppBrand::from_json`'s own
            // "unknown key" hint never names it — append it here rather
            // than teach `AppBrand` about a key it otherwise has nothing
            // to do with.
            let app = AppBrand::from_json(&rest).map_err(|e| {
                format!(
                    "options: {e}{}",
                    if e.contains("unknown key") {
                        ", or assets, or assist / assist_*"
                    } else {
                        ""
                    }
                )
            })?;
            (assets, assist_opts, asked, app)
        }
        None => (
            None,
            assist::AssistOptions::default(),
            false,
            AppBrand::default(),
        ),
    };
    // #740: before binding, like the assets check.
    let (proxy, assist_note) = start_assist(
        &ctx.conn.lock().unwrap_or_else(|p| p.into_inner()),
        assist_opts,
        assist_asked,
    )?;
    let mut servers = lock_servers();
    // #604 S2: `port == 0` asks the OS for any free loopback port instead
    // of a caller guessing one and racing to bind it independently (the
    // race a port-reuse-based test helper can lose under heavy parallel
    // load — #607). The bind just below is the only atomic step; nothing
    // before it can collide, so the registry check against an explicit,
    // caller-chosen port is skipped for `0` (there is nothing yet to
    // collide with) and repeated below against the port the OS actually
    // handed back.
    if port != 0 && servers.contains_key(&port) {
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
    let port = match server.server_addr() {
        ListenAddr::IP(a) => a.port(),
        // tiny_http has this variant on Unix only (the Windows release
        // build broke on it, v2026.10.28).
        #[cfg(unix)]
        ListenAddr::Unix(_) => port,
    };
    if servers.contains_key(&port) {
        // Only reachable with `port == 0`, and only if the OS happened to
        // hand back a port this process already tracks under this exact
        // mechanism — refuse rather than clobber that entry.
        return Err(format!(
            "anofox-visualization is already serving on port {port}"
        ));
    }
    let state = Arc::new(Authoring {
        ctx,
        guard: AuthoringGuard::new(port),
        max_body: 4 << 20,
        // #740: the builder's `assistant` block after the app bar's script.
        app: app.script()
            + &proxy
                .as_ref()
                .map(assist::Proxy::page_script)
                .unwrap_or_default(),
        assets_root: assets_root.clone(),
        proxy,
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
            tasks: None,
            reports: None,
            ingest: None,
            durable: None,
            locked: None,
            _state: Box::new(state),
        },
    );
    drop(servers);
    let _ = open::that(&url);
    // #522: logged once, here — the one place a resolved assets root is
    // reported back to the caller.
    let assets_note = assets_root
        .map(|p| format!("; assets served from '{}'", p.display()))
        .unwrap_or_default();
    Ok(format!(
        "anofox-visualization serving {url} (authoring: runs SQL on this session; loopback only; the URL carries the session token){assets_note}{assist_note}"
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
    // #740: after the guard, like /query. 404 when the proxy is off.
    // Only `/assist/v1/…`: `/assist/<file>` is the builder's own code
    // (web/assist/).
    if assist::is_api_path(&path) {
        return assist_route(st.proxy.as_ref(), &path, req);
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
    if path == "/check" {
        // #199: the Check button on a live backend. Same guard as `/query`
        // above (token, loopback Host, same-origin) — no new unauthenticated
        // surface. Unlike `/query`, this never touches the live `conn`: see
        // `run_check`.
        if req.method() != &Method::Post {
            return text(req, 405, "POST only");
        }
        let body = match read_body(&mut req, st.max_body) {
            Ok(b) => b,
            Err((code, e)) => return text(req, code, &e),
        };
        match run_check(st, &body) {
            Ok(json) => respond(req, 200, "application/json", json.into_bytes()),
            // #199 follow-up: "a check is already running" is the one error
            // that isn't the request's fault — 503, like the locked
            // server's own busy answer, not 400.
            Err(e) if e.starts_with(CHECK_BUSY) => text(req, 503, &e),
            Err(e) => text(req, 400, &e),
        }
        return;
    }
    // #522: before the generic GET-only check below — this route also
    // answers `HEAD`. `st.guard.check` above already gated every route
    // this function handles, this one included; there is no further
    // per-dashboard scope to check (unlike the locked server's own
    // `/d/<id>/assets/<rest>`, authoring has no dashboards).
    if let Some(root) = &st.assets_root {
        if let Some(rest) = path.strip_prefix("/assets/") {
            if req.method() != &Method::Get && req.method() != &Method::Head {
                return text(req, 405, "GET/HEAD only");
            }
            return authoring_asset(root, rest, req);
        }
    }
    if req.method() != &Method::Get {
        return text(req, 405, "method not allowed");
    }
    serve_asset(req, &path, &st.app, st.proxy.is_some());
}

/// `POST /assist/v1/messages[/count_tokens]` (#740): on a thread of its own
/// (bounded by the proxy's pool), so a minute-long stream never holds the
/// one authoring worker `/query` runs on. The reply goes out on the raw
/// connection ([`assist::write_http`]: a stream is flushed per read).
fn assist_route(proxy: Option<&assist::Proxy>, path: &str, mut req: Request) {
    let (Some(p), Some(ep)) = (proxy, assist::Endpoint::from_path(path)) else {
        return text(req, 404, "not found");
    };
    if req.method() != &Method::Post {
        return text(req, 405, "POST only");
    }
    let Some(slot) = p.try_slot() else {
        let mut w = req.into_writer();
        let _ = assist::write_http(&mut *w, assist::Proxy::busy());
        return;
    };
    let p = p.clone();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = req
            .as_reader()
            .take(assist::MAX_REQUEST as u64 + 1)
            .read_to_end(&mut buf);
        let reply = p.handle(slot, ep, &buf);
        let mut w = req.into_writer();
        let _ = assist::write_http(&mut *w, reply);
    });
}

/// `GET`/`HEAD /assets/<rest>` (#522, opt-in): one image file from the
/// authoring builder's own `options.assets` root ([`start_authoring`]) —
/// reached only when that option was given (`handle_authoring` checks
/// `st.assets_root` first). Same containment/extension/magic-byte/size
/// rules and response headers as the locked server's own `/d/<id>/assets/
/// <rest>` ([`locked_asset`]) — both go through [`anofox_visualization::
/// image`]'s shared validator, nothing duplicated here. Every refusal is a
/// plain 404, same reasoning as `locked_asset`'s own doc comment: which
/// one it was (missing, traversal, wrong extension, not really an image,
/// too big) is not a caller's business.
fn authoring_asset(root: &Path, rest: &str, req: Request) {
    const MAX: u64 = anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES;
    // Authoring is loopback-only, single-token, no viewer logout to
    // outlive — still `private`, not the locked server's longer
    // `max-age=300`, since an asset here can change under an author's feet
    // mid-session just as easily as the dashboard script itself. Review of
    // #604 N5: the header list itself (CSP, nosniff, CORP) is `image::
    // asset_header_values`, shared with `locked_asset` and the CLI's own
    // `authoring_asset` — only this lifetime and the mime differ by caller.
    let headers = |mime: &str| {
        anofox_visualization::image::asset_header_values(mime, "private, max-age=60")
            .into_iter()
            .map(|(k, v)| header(k, &v))
            .collect::<Vec<_>>()
    };
    if req.method() == &Method::Head {
        return match anofox_visualization::image::stat_asset(root, rest, MAX) {
            Ok((mime, len)) => {
                let resp = Response::new(
                    200.into(),
                    headers(&mime),
                    std::io::empty(),
                    Some(len as usize),
                    None,
                );
                let _ = req.respond(resp);
            }
            Err(_) => text(req, 404, "not found"),
        };
    }
    match anofox_visualization::image::read_asset(root, rest, MAX) {
        Ok((mime, bytes)) => {
            let len = bytes.len();
            let resp = Response::new(
                200.into(),
                headers(&mime),
                std::io::Cursor::new(bytes),
                Some(len),
                None,
            );
            let _ = req.respond(resp);
        }
        Err(_) => text(req, 404, "not found"),
    }
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

/// Row cap for a `/check` query (#199): a dashboard's own setup / input
/// defaults, never a panel's full result — generous headroom, same idea as
/// `AUTHORING_MAX_ROWS` but its own constant since the two are answering
/// different questions.
const CHECK_MAX_ROWS: usize = 1_000_000;

/// Unique per `/check` call (across the whole process, like web/lint.js's
/// `scratchSeq`): the scratch database's name, so two checks — or a check
/// racing a future authoring feature — never collide.
static CHECK_SEQ: AtomicU64 = AtomicU64::new(0);

/// `/check`'s own timeout (#199 follow-up): a hung or very slow checked
/// query must not hold [`DbCtx::check`] forever — every later check (there
/// is only the one check connection) would queue behind it. An author's own
/// choice per request (`"timeout_ms"` in the body — the same trust level as
/// `script` itself, which already runs on this server), clamped to
/// `MAX_CHECK_TIMEOUT_MS`; `DEFAULT_CHECK_TIMEOUT_MS` without one, matching
/// the locked server's own default `timeout_ms`.
const DEFAULT_CHECK_TIMEOUT_MS: u64 = 30_000;
const MAX_CHECK_TIMEOUT_MS: u64 = 120_000;

/// Prefix of [`scratch_check`]'s error when [`DbCtx::check`] is already
/// locked by another check — matched by the `/check` dispatcher above to
/// answer 503 instead of 400.
const CHECK_BUSY: &str = "a check is already running on this server";

fn strip_trailing_semicolons(s: &str) -> &str {
    s.trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
}

/// Authoring `/check` (#199): `dashboard --check`'s diagnostics for the
/// builder's Check button on a live backend, which has no DuckDB-Wasm
/// scratch session to lint in. Body = `{"script", "strict", "design",
/// "timeout_ms"}` (`strict`/`design` optional, default `false`/`true` —
/// `--strict` / the inverse of `--no-design`; `timeout_ms` optional, default
/// [`DEFAULT_CHECK_TIMEOUT_MS`], clamped to [`MAX_CHECK_TIMEOUT_MS`]). The
/// lint's own queries run on a fresh, isolated connection
/// ([`scratch_check`]) — never on the live `conn`, which stays free the
/// whole time. Returns the same shape
/// [`anofox_visualization::lint::step_json`] does once `done`:
/// `{"diagnostics": […], "ok", "summary", "spans": [[start, end], …]}`
/// (`spans` in UTF-16 code units, for the editor to jump to a statement).
fn run_check(st: &Authoring, body: &str) -> Result<String, String> {
    use anofox_visualization::lint;
    let req: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON body: {e}"))?;
    let script = req
        .get("script")
        .and_then(Value::as_str)
        .ok_or_else(|| "\"script\" (a string) is required".to_string())?;
    let design = req.get("design").and_then(Value::as_bool).unwrap_or(true);
    let strict = req.get("strict").and_then(Value::as_bool).unwrap_or(false);
    let timeout_ms = req
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_CHECK_TIMEOUT_MS)
        .clamp(1, MAX_CHECK_TIMEOUT_MS);
    let diags = scratch_check(
        st,
        script,
        lint::LintOptions {
            design,
            stateful_session: true,
            // The authoring builder has no dashboards folder (#514 PR B,
            // decision 5), and `options.assets` (#522) is a plain serving
            // root, not a dashboards folder with its own sibling `.sql`
            // files to lint against — `image/missing-file` never fires
            // here, with or without that option set.
            image_root: None,
        },
        Duration::from_millis(timeout_ms),
    )?;
    let utf16 = |i: usize| script[..i].encode_utf16().count();
    Ok(serde_json::json!({
        "diagnostics": diags.iter().map(lint::Diagnostic::to_json).collect::<Vec<_>>(),
        "ok": lint::passes(&diags, strict),
        "summary": lint::summary(&diags),
        "spans": anofox_visualization::sql::lex::statement_spans(script)
            .iter()
            .map(|&(a, b)| [utf16(a), utf16(b)])
            .collect::<Vec<_>>(),
    })
    .to_string())
}

/// Runs `lint::check_opts` on [`DbCtx::check`] — a second connection to the
/// same database as the live session's `conn`, never `conn` itself, so a
/// check can never change what the next `/query` sees. Isolated the way
/// web/lint.js's `scratchRunner` isolates DuckDB-Wasm: an ATTACHed
/// `:memory:` scratch database, switched into with `USE` and put ahead of
/// the live database on `search_path`, so anything the script creates (a
/// plain or TEMP `CREATE TABLE`/`VIEW`/`MACRO`, `SET VARIABLE`) lands there,
/// not on the live database's own `main` — while the live database's tables
/// stay readable (an input's or `::SELECT_AS`'s default query needs real
/// rows). The scratch database is DETACHed before returning, success or
/// error alike (`ATTACH` is catalog-wide, not per connection, so leaving it
/// attached would leak into every later connection on this database —
/// including the live one's own `duckdb_databases()`); `check` itself is
/// kept open and reused by the next call, like `conn`.
///
/// `timeout` bounds the whole check (every query `check_opts` runs, not
/// just one): past it, a watchdog thread interrupts `check` the way the
/// locked server's own `timeout_ms` interrupts a panel connection
/// ([`Conn::with_deadline_fired`]) — there is only this one check
/// connection, so a hung query must not hold it forever and queue up every
/// later check behind it. `check_opts` absorbs an interrupted query's own
/// DuckDB error into an ordinary `sql-error` diagnostic and keeps going (the
/// same as any other failed statement), so on a fired timeout this also
/// appends one more, unambiguous `check/timeout` diagnostic — the result is
/// never silently "ok" just because `check_opts` itself never saw a hard
/// error. [`DbCtx::check`] is locked with `try_lock`, not a blocking wait: a
/// second check already running (there is at most one HTTP worker today,
/// but this holds regardless) gets a clear, immediate "busy" error
/// ([`CHECK_BUSY`]) instead of queuing — the mutex is never held across a
/// response, so this can't itself cause the busy answer it's guarding
/// against.
fn scratch_check(
    st: &Authoring,
    script: &str,
    opts: anofox_visualization::lint::LintOptions,
    timeout: Duration,
) -> Result<Vec<anofox_visualization::lint::Diagnostic>, String> {
    let scratch = st
        .ctx
        .check
        .try_lock()
        .map_err(|_| format!("{CHECK_BUSY}; try again"))?;
    let home_raw = scratch
        .query("SELECT current_database() AS d")
        .and_then(|mut r| r.string_rows(1))
        .map_err(|e| format!("check: {e}"))?
        .into_iter()
        .next()
        .and_then(|mut r| r.pop())
        .flatten()
        .ok_or_else(|| "check: could not read current_database()".to_string())?;
    let home = format!("\"{}\"", home_raw.replace('"', "\"\""));
    let n = CHECK_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!("anofox_check_{n}");
    scratch.exec(&format!("ATTACH ':memory:' AS {name}"))?;
    scratch.exec(&format!("USE {name}"))?;
    scratch.exec(&format!(
        "SET search_path = '{name}.main,{}.main'",
        home.replace('\'', "''")
    ))?;
    let (diags, fired) = scratch.with_deadline_fired(timeout, |c| {
        Ok(anofox_visualization::lint::check_opts(
            script,
            |sql| check_run_query(c, sql),
            opts,
        ))
    });
    // `with_deadline_fired` only ever sees `Ok` here (`check_opts` can't
    // itself return an error — see above), so this is always `Some`; the
    // fallback is defensive only.
    let mut diags = diags.unwrap_or_default();
    if fired {
        diags.push(anofox_visualization::lint::Diagnostic {
            severity: anofox_visualization::lint::Severity::Error,
            stmt: 0,
            code: "check/timeout",
            message: format!(
                "the check did not finish within {}ms and was interrupted; its diagnostics may be incomplete",
                timeout.as_millis()
            ),
            sql: String::new(),
        });
    }
    // Best-effort cleanup either way; `scratch` closes (and disconnects) on
    // drop regardless, same defence-in-depth web/lint.js's own
    // `detachAndClose` relies on. Run even after a fired interrupt: DETACH
    // is cheap (catalog metadata only, no query execution) and this is the
    // one connection every later check reuses, so leaving `name` attached —
    // or `search_path`/the current database pointed at it — would corrupt
    // every check after this one, not just this one's own cleanliness.
    let _ = scratch.exec(&format!("USE {home}"));
    let _ = scratch.exec(&format!("DETACH DATABASE IF EXISTS {name}"));
    // #517, review of #530 #5: `check_opts`'s `viewer_state`/
    // `viewer_state_latest` temps (if a `::SUBMIT` script needed them)
    // live in `temp.main`, not in the scratch database `DETACH` just
    // dropped — so they would otherwise outlive this one check and shadow
    // whatever any *later* check's own `viewer_state` reference resolves
    // to on this same, reused connection. Explicitly schema-qualified so
    // this can never touch a real (non-temp) `viewer_state`/
    // `viewer_state_latest` some other check's dashboard legitimately has.
    let _ = scratch.exec("DROP VIEW IF EXISTS temp.main.viewer_state_latest");
    let _ = scratch.exec("DROP TABLE IF EXISTS temp.main.viewer_state");
    Ok(diags)
}

/// One `lint::check_opts` query on `conn` (the [`scratch_check`] connection):
/// a query's rows, or `[]` for a statement run for effect (`SET VARIABLE`,
/// `CREATE TEMP …`) — [`authoring_query`]'s is-it-a-query logic, returning
/// typed rows instead of a response body. `stateful_session: true` means
/// `check_opts` sends exactly one statement per call (never a prelude
/// replay), so unlike `authoring_query` there is no multi-statement text to
/// recover.
fn check_run_query(conn: &Conn, sql: &str) -> Result<Vec<Map<String, Value>>, String> {
    let trimmed = strip_trailing_semicolons(sql);
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let ex = conn.extract(trimmed)?;
    if ex.count == 0 {
        return Ok(Vec::new());
    }
    for i in 0..ex.count - 1 {
        ex.prepare(conn, i)?.execute()?;
    }
    let last = ex.prepare(conn, ex.count - 1)?;
    let is_query =
        last.statement_type() == crate::ffi::duckdb_statement_type::DUCKDB_STATEMENT_TYPE_SELECT;
    if !is_query {
        last.execute()?;
        return Ok(Vec::new());
    }
    drop(last);
    let json = json_rows(conn, trimmed, &[], CHECK_MAX_ROWS, &mut |_, want| Ok(want))?;
    serde_json::from_str(&json).map_err(|e| format!("check: reading rows: {e}"))
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
    /// `nav_pickers` (#407 part 3, #556): controls in the served nav bar
    /// that each set one or two `path_vars` variables for every page —
    /// drawn left to right in list order. `options.nav_picker` (singular, a
    /// bare object) is still accepted as a one-element list; the two keys
    /// may not both be set. Every `sql` is author text, run once per
    /// snapshot (never from the viewer) — see [`NavPicker`] and
    /// `build_nav_picker`.
    pub nav_pickers: Vec<NavPicker>,
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
    /// `css_file` (#773): the operator's stylesheet, read and checked at
    /// start ([`site_css::check`]), served at [`site_css::PATH`].
    pub site_css: Option<Arc<str>>,
    /// `folders` (#772): serve the `*.sql` files one level down too, each
    /// folder a group of the nav and the `/` list.
    pub folders: bool,
    /// `ingest` (#769): `POST /api/data/<table>` for API keys with an
    /// `ingest:<table>` scope.
    pub ingest: Option<IngestOptions>,
    /// `preview` (#770): temporary dashboards from API keys with the
    /// `preview` scope (`POST /api/dashboards`).
    pub preview: Option<PreviewOptions>,
    /// `durability` (#775): snapshots of the state store and the write-back
    /// log to S3 (or another location DuckDB can write), one writer.
    pub durability: Option<DurabilityOptions>,
    /// `home`: a dashboard id `/` redirects to instead of the landing page
    /// (when the viewer may read it).
    pub home: Option<String>,
    /// `source`: build (part of) the snapshot from a live Quack server
    /// instead of (or alongside) the local session (#352 prototype) — see
    /// `duckext/src/source.rs` and docs/secure-serving.md, "Quack source".
    pub source: Option<Arc<crate::source::SourceOpts>>,
    /// `state` (#517 S2): per-viewer append-only state, structurally
    /// parsed here; the start-time refusals that need the loaded
    /// dashboards and snapshot (plan §2) run in [`start_locked`], which
    /// then opens the store and builds the live [`SubmitRuntime`] this
    /// only describes.
    pub state_opts: Option<StateOptions>,
    /// `writeback` (#684 slice 2): shared write-back into declared tables,
    /// structurally parsed here; [`start_locked`] runs its start checks and
    /// opens the change log ([`WritebackRuntime`]).
    pub writeback_opts: Option<WritebackOptions>,
    /// `max_asset_bytes` (#514 PR B): the cap on one file served from
    /// `<dashboards dir>/assets/` (`GET /d/<id>/assets/<rest>`, [`locked_asset`]).
    pub max_asset_bytes: usize,
    /// `metrics` (#765): the bearer token `GET /metrics` needs — `None`: no
    /// such route (404).
    pub metrics_token: Option<String>,
    /// `tasks` (#767): scheduled `tasks/*.task.sql`.
    pub tasks: Option<TasksOptions>,
    /// `notify` (#768): the reports' channels, and whether this replica sends.
    pub notify: Option<notify::NotifyOptions>,
    /// `reports` (#768): scheduled reports.
    pub reports: Vec<notify::Report>,
    /// `alerts` (#768 R2): a panel's rows, sent when they change.
    pub alerts: Vec<notify::Alert>,
}

/// `options.tasks` (#767, docs/plans/plan-tasks-reports.md §1).
#[derive(Debug, Clone)]
pub struct TasksOptions {
    /// Run the tasks on this replica (`run`; only one replica should).
    pub run: bool,
    /// The attached databases tasks may write; every other one must be
    /// attached `READ_ONLY` (checked at start).
    pub write: Vec<String>,
    /// One run's time limit (`timeout_s`, default 900).
    pub timeout: Duration,
    /// Where each run is recorded (`log`: a file or `"stderr"`; default stderr).
    pub log: Option<String>,
    /// #768 R2: told when a task starts failing and when it recovers
    /// (`on_failure: {"to": […]}`, channels from `options.notify`).
    pub on_failure: Vec<notify::Recipient>,
}

/// One entry of `options.nav_pickers` (or the single-element list
/// `options.nav_picker` unwraps to): `{"var": "model", "sql": "SELECT
/// model_id, title FROM …", "label": "Model"}` (`Dropdown`, today's only
/// shape — `type` defaults to it) or `{"type": "daterange", "vars":
/// ["view_from", "view_to"], "sql": "SELECT min(d)::DATE, max(d)::DATE FROM
/// …", "presets": ["last 7d", "all"]}` (#556).
///
/// A `Dropdown`'s `sql` must be exactly one query (checked like every
/// `data`-folder statement) and is run once per snapshot build (startup +
/// every refresh), with `anofox_data` set and no viewer context — never
/// per-request, never with the viewer's claims. Its rows become the
/// picker's options, each re-checked against the same plain-name rule as a
/// `path_vars` value ([`nav_picker_rows_to_options`]).
///
/// A `DateRange`'s `vars` are `[from, to]`, both required to be in
/// `path_vars` ([`LockedOptions::parse`]). Its `sql` is optional and, when
/// given, has the same trust level and run-once cadence as a dropdown's:
/// exactly one row of two columns, `(min, max)`, the full available date
/// range (`daterange_bounds_from_rows`). `presets` names entries of
/// `DATERANGE_PRESET_NAMES`, each computed from that `max` — never
/// wall-clock `today()` — and requires `sql` (there is no `max` to compute
/// them from otherwise); refused at parse time without it. Without `sql`
/// the picker is two plain, unbounded date inputs.
#[derive(Clone, Debug)]
pub enum NavPicker {
    Dropdown {
        var: String,
        sql: String,
        label: Option<String>,
    },
    DateRange {
        vars: [String; 2],
        sql: Option<String>,
        presets: Vec<String>,
        label: Option<String>,
    },
}

impl NavPicker {
    pub fn label(&self) -> Option<&str> {
        match self {
            NavPicker::Dropdown { label, .. } | NavPicker::DateRange { label, .. } => {
                label.as_deref()
            }
        }
    }
    /// Every `path_vars` name this picker sets (one for a dropdown, two for
    /// a date range) — shared by the startup `path_vars` check and the
    /// request-time daterange validation.
    pub fn vars(&self) -> &[String] {
        match self {
            NavPicker::Dropdown { var, .. } => std::slice::from_ref(var),
            NavPicker::DateRange { vars, .. } => vars,
        }
    }
}

/// `options.nav_pickers`/`options.nav_picker`: at most this many pickers —
/// each is inlined into every served page, so an unbounded list would be as
/// wasteful as an unbounded `nav_picker.sql` result.
const MAX_NAV_PICKERS: usize = 8;

/// One `options.nav_picker`/`options.nav_pickers[]` entry: `"type"` picks
/// `Dropdown` (default — today's only shape, so an untyped object parses
/// exactly as before) or `"daterange"` (#556).
fn parse_nav_picker(val: &serde_json::Value) -> Result<NavPicker, String> {
    let m = val.as_object().ok_or(
        "options.nav_picker(s): must be an object like {\"var\": \"model\", \"sql\": \"SELECT …\", \"label\": \"Model\"} or {\"type\": \"daterange\", \"vars\": [\"from\", \"to\"], …}",
    )?;
    match m.get("type").and_then(|v| v.as_str()) {
        None | Some("dropdown") => parse_dropdown_nav_picker(m),
        Some("daterange") => parse_daterange_nav_picker(m),
        Some(other) => Err(format!(
            "options.nav_picker(s).type: unknown '{other}' (dropdown, daterange)"
        )),
    }
}

fn parse_dropdown_nav_picker(
    m: &serde_json::Map<String, serde_json::Value>,
) -> Result<NavPicker, String> {
    if let Some(other) = m
        .keys()
        .find(|k| !matches!(k.as_str(), "type" | "var" | "sql" | "label"))
    {
        return Err(format!(
            "options.nav_picker(s): unknown key '{other}' (type, var, sql, label)"
        ));
    }
    let var = m
        .get("var")
        .and_then(|v| v.as_str())
        .filter(|v| sv::valid_ident(v))
        .ok_or("options.nav_picker(s).var must be a variable name")?;
    let sql = m
        .get("sql")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("options.nav_picker(s).sql must be a SELECT query")?;
    let label = parse_nav_picker_label(m)?;
    Ok(NavPicker::Dropdown {
        var: var.to_string(),
        sql: sql.to_string(),
        label,
    })
}

fn parse_daterange_nav_picker(
    m: &serde_json::Map<String, serde_json::Value>,
) -> Result<NavPicker, String> {
    if let Some(other) = m
        .keys()
        .find(|k| !matches!(k.as_str(), "type" | "vars" | "sql" | "label" | "presets"))
    {
        return Err(format!(
            "options.nav_picker(s) (daterange): unknown key '{other}' (type, vars, sql, label, presets)"
        ));
    }
    let vars_arr = m
        .get("vars")
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 2)
        .ok_or(
            "options.nav_picker(s) (daterange).vars must be an array of exactly two variable names [from, to]",
        )?;
    let mut vars = [String::new(), String::new()];
    for (i, v) in vars_arr.iter().enumerate() {
        let v = v
            .as_str()
            .filter(|v| sv::valid_ident(v))
            .ok_or("options.nav_picker(s) (daterange).vars must name variables")?;
        vars[i] = v.to_string();
    }
    if vars[0].eq_ignore_ascii_case(&vars[1]) {
        return Err(
            "options.nav_picker(s) (daterange).vars: from and to must be different variables"
                .into(),
        );
    }
    let sql = match m.get("sql") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("options.nav_picker(s) (daterange).sql must be a SELECT query")?
                .to_string(),
        ),
    };
    let mut presets = Vec::new();
    if let Some(p) = m.get("presets") {
        if sql.is_none() {
            return Err(
                "options.nav_picker(s) (daterange).presets needs .sql (presets are computed \
                 from its max date, which there is none of without it)"
                    .into(),
            );
        }
        let arr = p
            .as_array()
            .ok_or("options.nav_picker(s) (daterange).presets must be an array of preset names")?;
        for p in arr {
            let p = p.as_str().filter(|p| sv::valid_daterange_preset(p)).ok_or_else(|| {
                format!(
                    "options.nav_picker(s) (daterange).presets: unknown preset (expected one of {})",
                    sv::DATERANGE_PRESET_NAMES.join(", ")
                )
            })?;
            presets.push(p.to_string());
        }
    }
    let label = parse_nav_picker_label(m)?;
    Ok(NavPicker::DateRange {
        vars,
        sql,
        presets,
        label,
    })
}

/// `options.metrics` (#765): `{"token_env": "NAME"}` (or `{"token": "…"}`) —
/// the bearer token a Prometheus scraper sends to `GET /metrics`; at least
/// 32 printable ASCII characters.
fn parse_metrics(val: &serde_json::Value) -> Result<String, String> {
    let m = val.as_object().ok_or(
        "options.metrics must be an object like {\"token_env\": \"ANOFOX_METRICS_TOKEN\"}",
    )?;
    for k in m.keys() {
        if !matches!(k.as_str(), "token" | "token_env") {
            return Err(format!(
                "options.metrics: unknown key '{k}' (token_env, token)"
            ));
        }
    }
    let t = match (m.get("token"), m.get("token_env")) {
        (Some(t), None) => t
            .as_str()
            .ok_or("options.metrics.token must be a string")?
            .to_string(),
        (None, Some(e)) => {
            let name = e
                .as_str()
                .ok_or("options.metrics.token_env must be a string")?;
            std::env::var(name).map_err(|_| {
                format!("options.metrics.token_env: environment variable '{name}' is not set")
            })?
        }
        _ => return Err("options.metrics: give token_env (or token), one of them".into()),
    };
    let t = t.trim().to_string();
    if t.len() < 32 || !t.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(
            "options.metrics: the token must be at least 32 printable ASCII characters (e.g. `openssl rand -hex 32`)"
                .into(),
        );
    }
    Ok(t)
}

/// `options.tasks` (#767): `{"run": true, "write": ["stage"], "timeout_s":
/// 900, "log": "/var/log/anofox/tasks.jsonl"}`.
fn parse_tasks(
    val: &serde_json::Value,
    notify: Option<&notify::NotifyOptions>,
) -> Result<TasksOptions, String> {
    let m = val
        .as_object()
        .ok_or("options.tasks must be an object like {\"run\": true, \"write\": [\"stage\"]}")?;
    let mut t = TasksOptions {
        run: false,
        write: Vec::new(),
        timeout: Duration::from_secs(900),
        log: None,
        on_failure: Vec::new(),
    };
    for (k, v) in m {
        match k.as_str() {
            "run" => {
                t.run = v
                    .as_bool()
                    .ok_or("options.tasks.run must be true or false")?
            }
            "write" => {
                for w in v
                    .as_array()
                    .ok_or("options.tasks.write must be a list of database names")?
                {
                    let w = w
                        .as_str()
                        .filter(|w| sv::valid_ident(w))
                        .ok_or("options.tasks.write: a database name")?;
                    t.write.push(w.to_string());
                }
            }
            "timeout_s" => {
                t.timeout = Duration::from_secs(
                    v.as_u64()
                        .filter(|n| (1..=86_400).contains(n))
                        .ok_or("options.tasks.timeout_s must be 1..=86400")?,
                )
            }
            "on_failure" => {
                let m = v
                    .as_object()
                    .filter(|m| m.keys().all(|k| k == "to"))
                    .ok_or("options.tasks.on_failure must be {\"to\": [recipients]}")?;
                t.on_failure =
                    notify::parse_recipients(m.get("to"), notify, "options.tasks.on_failure")?;
            }
            "log" => {
                t.log = Some(
                    v.as_str()
                        .ok_or("options.tasks.log must be \"stderr\" or a file path")?
                        .to_string(),
                )
            }
            other => {
                return Err(format!(
                    "options.tasks: unknown key '{other}' (run, write, timeout_s, log, on_failure)"
                ))
            }
        }
    }
    Ok(t)
}

fn parse_nav_picker_label(
    m: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<String>, String> {
    match m.get("label") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() && s.chars().count() <= 100 => {
            Ok(Some(s.clone()))
        }
        _ => Err("options.nav_picker(s).label must be a short (<=100 character) string".into()),
    }
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
            nav_pickers: Vec::new(),
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
            site_css: None,
            folders: false,
            ingest: None,
            preview: None,
            durability: None,
            home: None,
            source: None,
            state_opts: None,
            writeback_opts: None,
            max_asset_bytes: anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES as usize,
            metrics_token: None,
            tasks: None,
            notify: None,
            reports: Vec::new(),
            alerts: Vec::new(),
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
        let (mut saw_nav_picker, mut saw_nav_pickers) = (false, false);
        let (mut notify_cfg, mut reports_cfg, mut alerts_cfg, mut tasks_cfg) =
            (None, None, None, None);
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
                    let list = val
                        .as_array()
                        .ok_or("options.frame_ancestors must be an array of origins")?;
                    let mut out = Vec::new();
                    for o in list {
                        let o = o
                            .as_str()
                            .ok_or("options.frame_ancestors must be an array of origins")?;
                        if !sv::valid_frame_ancestor(o) {
                            return Err(format!("options.frame_ancestors: '{o}' is not an origin (https://host[:port], http://127.0.0.1|localhost[:port], 'self' or 'none')"));
                        }
                        out.push(o.to_string());
                    }
                    o.frame_ancestors = Some(out);
                }
                "warm" => {
                    let n = val
                        .as_u64()
                        .ok_or("options.warm must be a non-negative integer")?;
                    o.warm = Some(n.min(256) as usize);
                }
                "cache_mb" => {
                    let mb = val
                        .as_u64()
                        .ok_or("options.cache_mb must be a non-negative integer")?;
                    o.cache_bytes = (mb.min(1 << 16) as usize) << 20;
                }
                "attach" => o.attach = val.as_bool().ok_or("options.attach must be a boolean")?,
                "load" => {
                    let arr = val
                        .as_array()
                        .ok_or("options.load must be a list of extension names")?;
                    for e in arr {
                        let e = e
                            .as_str()
                            .filter(|e| sv::valid_ident(e))
                            .ok_or("options.load: invalid extension name")?;
                        o.load.push(e.to_string());
                    }
                }
                "theme" => {
                    let spec = match val {
                        serde_json::Value::String(t) => t.clone(),
                        serde_json::Value::Object(_) => val.to_string(),
                        _ => {
                            return Err("options.theme must be a theme name or a JSON object".into())
                        }
                    };
                    anofox_visualization::theme::resolve(&spec)
                        .map_err(|e| format!("options.theme: {e}"))?;
                    o.theme = Some(spec);
                }
                "hosts" => {
                    let arr = val
                        .as_array()
                        .ok_or("options.hosts must be a list of host names")?;
                    for h in arr {
                        let h = h
                            .as_str()
                            .map(str::trim)
                            .filter(|h| {
                                !h.is_empty()
                                    && h.chars()
                                        .all(|c| c.is_ascii_alphanumeric() || "-.:[]*".contains(c))
                            })
                            .ok_or("options.hosts: invalid host name")?;
                        o.hosts.push(h.to_string());
                    }
                }
                "refresh_s" => o.refresh = Some(Duration::from_secs(num()?)),
                "data" => {
                    let d = val
                        .as_str()
                        .filter(|d| !d.trim().is_empty())
                        .ok_or("options.data must be a directory path")?;
                    // Canonical: DuckDB compares paths as text, and `..` or a
                    // symlink in the configured path would make that brittle.
                    let p = std::fs::canonicalize(d)
                        .map_err(|e| format!("options.data: '{d}': {e}"))?;
                    if !p.is_dir() {
                        return Err(format!("options.data: '{d}' is not a directory"));
                    }
                    if let Some(why) = too_broad(&p) {
                        return Err(format!("options.data: refusing '{}': {why} — point it at the folder that holds the data only", p.display()));
                    }
                    o.data = Some(p);
                }
                "path_vars" => {
                    let arr = val
                        .as_array()
                        .ok_or("options.path_vars must be a list of variable names")?;
                    for v in arr {
                        let v = v
                            .as_str()
                            .filter(|v| sv::valid_ident(v))
                            .ok_or("options.path_vars: invalid variable name")?;
                        o.path_vars.push(v.to_string());
                    }
                }
                "nav_picker" => {
                    if saw_nav_pickers {
                        return Err("options: nav_picker and nav_pickers cannot both be set".into());
                    }
                    saw_nav_picker = true;
                    o.nav_pickers = vec![parse_nav_picker(val)?];
                }
                "nav_pickers" => {
                    if saw_nav_picker {
                        return Err("options: nav_picker and nav_pickers cannot both be set".into());
                    }
                    saw_nav_pickers = true;
                    let arr = val
                        .as_array()
                        .ok_or("options.nav_pickers must be an array of pickers")?;
                    if arr.len() > MAX_NAV_PICKERS {
                        return Err(format!(
                            "options.nav_pickers: at most {MAX_NAV_PICKERS} pickers (each is \
                             inlined into every served page)"
                        ));
                    }
                    for item in arr {
                        o.nav_pickers.push(parse_nav_picker(item)?);
                    }
                }
                "auth" => auth_cfg = Some(val),
                "audit" => {
                    audit = Some(
                        val.as_str()
                            .ok_or("options.audit must be \"stderr\" or a file path")?,
                    )
                }
                "log" => {
                    log = Some(
                        val.as_str()
                            .ok_or("options.log must be \"stderr\" or a file path")?,
                    )
                }
                "log_vars" => {
                    log_vars = Some(
                        val.as_str()
                            .ok_or("options.log_vars must be none, names or values")?,
                    )
                }
                "metrics" => {
                    o.metrics_token = Some(parse_metrics(val)?);
                }
                "tasks" => tasks_cfg = Some(val),
                "notify" => notify_cfg = Some(val),
                "reports" => reports_cfg = Some(val),
                "alerts" => alerts_cfg = Some(val),
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
                    let path = val
                        .as_str()
                        .filter(|p| !p.trim().is_empty())
                        .ok_or("options.policy must be the path of a policy file, or \"none\"")?;
                    if path == "none" {
                        o.policy_none = true;
                    } else {
                        o.policy = Some(Arc::new(read_policy(Path::new(path))?));
                    }
                }
                "policy_key_env" => {
                    let name = val.as_str().ok_or(
                        "options.policy_key_env must be the name of an environment variable",
                    )?;
                    let key = std::env::var(name).map_err(|_| {
                        format!("options.policy_key_env: environment variable '{name}' is not set")
                    })?;
                    if key.len() < 32 {
                        return Err("options.policy_key_env: the key must be at least 32 characters (e.g. `openssl rand -hex 32`)".into());
                    }
                    o.policy_key = Some(PolicyKey(Arc::new(key.into_bytes())));
                }
                "max_inflight_mb" => o.max_inflight = (num()?.min(1 << 20) as usize) << 20,
                "max_response_mb" => o.max_response = (num()?.min(1 << 12) as usize) << 20,
                "max_queries" => o.max_queries = num()?.min(1024) as usize,
                // #514 PR B: a cap on one `assets/` file, in bytes (like
                // `max_body_bytes`, not MB-scaled — an image file is small).
                "max_asset_bytes" => o.max_asset_bytes = num()?.min(64 << 20) as usize,
                "search_path" => {
                    let p = val.as_str().map(str::trim).ok_or("options.search_path must be a string like \"warehouse\" or \"warehouse.sales\"")?;
                    o.search_path =
                        parse_search_path(p).map_err(|e| format!("options.search_path: {e}"))?;
                }
                "app" => {
                    if !val.is_object() {
                        return Err("options.app must be an object like {\"name\": \"…\", \"tagline\": \"…\", \"logo\": \"https://…\"}".into());
                    }
                    let a = AppBrand::from_json(&val.to_string())
                        .map_err(|e| format!("options.app: {e}"))?;
                    a.check().map_err(|e| format!("options.app: {e}"))?;
                    o.app = Some(a);
                }
                "folders" => {
                    o.folders = val
                        .as_bool()
                        .ok_or("options.folders must be true or false")?;
                }
                "ingest" => o.ingest = Some(IngestOptions::parse(val)?),
                "preview" => o.preview = Some(PreviewOptions::parse(val)?),
                "durability" => o.durability = Some(DurabilityOptions::parse(val)?),
                "css_file" => {
                    let f = val
                        .as_str()
                        .map(str::trim)
                        .filter(|f| !f.is_empty())
                        .ok_or("options.css_file must be the path of a .css file")?;
                    let meta =
                        std::fs::metadata(f).map_err(|e| format!("options.css_file {f}: {e}"))?;
                    if !meta.is_file() || meta.len() > site_css::MAX_BYTES as u64 {
                        return Err(format!(
                            "options.css_file {f}: must be a file of at most {} KiB",
                            site_css::MAX_BYTES / 1024
                        ));
                    }
                    let css = std::fs::read_to_string(f)
                        .map_err(|e| format!("options.css_file {f}: {e}"))?;
                    site_css::check(&css).map_err(|e| format!("options.css_file {f}: {e}"))?;
                    o.site_css = Some(Arc::from(css));
                }
                "home" => {
                    let h = val
                        .as_str()
                        .map(str::trim)
                        .filter(|h| sv::valid_dashboard_id(h))
                        .ok_or(
                            "options.home must be a dashboard id (its file name without .sql)",
                        )?;
                    o.home = Some(h.to_string());
                }
                "source" => {
                    o.source = Some(Arc::new(crate::source::parse(val)?));
                }
                "state" => {
                    // `StateOptions::parse`'s own errors already start with
                    // "options.state…" — not re-wrapped.
                    o.state_opts = Some(StateOptions::parse(val)?);
                }
                "writeback" => {
                    o.writeback_opts = Some(WritebackOptions::parse(val)?);
                }
                // The key list is generated, not copied by hand, so it can't
                // drift from `--site`'s own `SERVER_OPTION_KEYS` (#501).
                other => {
                    let keys = anofox_visualization::host::serve_options::all_keys_joined();
                    return Err(format!("options: unknown key '{other}' ({keys})"));
                }
            }
        }
        // A response larger than the in-flight budget could never be sent.
        o.max_response = o
            .max_response
            .min(o.max_inflight.max(crate::limits::FREE_BYTES));
        // Review of #554 S2: an asset request charges against this same
        // in-flight budget (`locked_asset`, below) — left unclamped,
        // `max_asset_bytes` above `max_inflight` would mean every asset
        // request unconditionally overruns the budget and 503s, with
        // nothing at startup saying why. Clamped the same way, just above.
        o.max_asset_bytes = o
            .max_asset_bytes
            .min(o.max_inflight.max(crate::limits::FREE_BYTES));
        if !o.path_vars.is_empty() && o.data.is_none() {
            return Err("options.path_vars needs options.data".into());
        }
        for np in &o.nav_pickers {
            for v in np.vars() {
                if !o.path_vars.iter().any(|p| p.eq_ignore_ascii_case(v)) {
                    return Err(format!(
                        "options.nav_picker(s): '{v}' must be one of options.path_vars"
                    ));
                }
            }
        }
        // A picker's `sql` runs once per snapshot, directly on the locked
        // connection — never through the access policy's per-viewer
        // row/column rewrite (`Enforcer::rewrite_script` only ever sees
        // `dashboards`). Running it anyway under a policy would silently
        // bypass that policy's row and column rules for whatever table it
        // reads, so the safer choice is to refuse the combination outright
        // rather than guess at a scope to run it under. A dropdown always
        // has `sql`; a daterange picker only when given one.
        let any_picker_sql = o.nav_pickers.iter().any(|np| match np {
            NavPicker::Dropdown { .. } => true,
            NavPicker::DateRange { sql, .. } => sql.is_some(),
        });
        if any_picker_sql && o.policy.is_some() {
            return Err(
                "options.nav_picker(s) cannot be combined with options.policy — a picker's sql \
                 runs once per snapshot outside the access policy's row/column rewrite, so it \
                 would bypass the policy for whatever table it reads; remove one of them"
                    .into(),
            );
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
        // #768: the channels first (a report's recipient names one); the
        // secrets come from the environment.
        if let Some(v) = notify_cfg {
            o.notify = Some(notify::parse_notify(v, &|k| std::env::var(k).ok())?);
        }
        if let Some(v) = reports_cfg {
            o.reports = notify::parse_reports(v, o.notify.as_ref())?;
        }
        if let Some(v) = alerts_cfg {
            o.alerts = notify::parse_alerts(v, o.notify.as_ref())?;
        }
        if let Some(v) = tasks_cfg {
            o.tasks = Some(parse_tasks(v, o.notify.as_ref())?);
        }
        // #769: only an API key may ingest, so there must be API keys.
        if o.durability.is_some() && o.state_opts.is_none() && o.writeback_opts.is_none() {
            return Err(
                "options.durability needs options.state or options.writeback (the files it keeps)"
                    .into(),
            );
        }
        if o.preview.is_some() && !o.auth.as_ref().is_some_and(|a| a.keys.is_some()) {
            return Err("options.preview needs auth with API keys ({\"mode\": \"api_keys\", …}): only an API key with the preview scope may create one".into());
        }
        if o.ingest.is_some() && !o.auth.as_ref().is_some_and(|a| a.keys.is_some()) {
            return Err("options.ingest needs auth with API keys ({\"mode\": \"api_keys\", …}): only an API key with an ingest:<table> scope may write".into());
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
    /// Per statement: a `-- @materialize` setup statement (#727), shared
    /// between requests through [`MatStore`].
    materialize: Vec<bool>,
    /// Per statement other than setup: the `::TAB` / `::SUBTAB` statements
    /// whose section it is in ([`anofox_visualization::sql::Panel::tabs`]).
    /// Their `-- @requires` gate it too, decided where each tab stands — as
    /// the page decides them — so a hidden tab's panels never run.
    tabs: Vec<Vec<usize>>,
    /// #517 S2, plan §4.2 step 8: every input-declared variable name
    /// (lower case) → the statement(s) that declare it, its kind, and its
    /// own output column index (`inputs::input_index`) — review of #538
    /// B2: option membership must be checked against *that* column, not a
    /// row's first one (an `::OPTION_LABEL` or `::HINT` column before it
    /// would otherwise be read instead). More than one entry for a name
    /// means two input statements declare it — ambiguous for
    /// `SUBMIT_VARS` (`/api/submit` refuses it, `400`), same as the
    /// static `submit/duplicate-input` lint.
    inputs: HashMap<String, Vec<(usize, InputKind, usize)>>,
    /// #517 S2: every `::SUBMIT` statement's own output columns, by plan
    /// index — `/api/submit`'s `panel` must name one of these.
    submits: HashMap<usize, SubmitSpec>,
    /// #684: every `::SAVE` statement's own output columns, by plan index —
    /// `/api/save`'s `panel` must name one of these.
    saves: HashMap<usize, SaveSpec>,
}

/// One `::SAVE` statement's shape (#684): the button's own column name (for
/// the audit log and the change log's record) and the columns naming what
/// it saves — read from the re-run statement, never from the request.
struct SaveSpec {
    name: String,
    graph_col: Option<usize>,
    form_col: Option<usize>,
    tables_col: usize,
}

/// One `::SUBMIT` statement's shape (#517 S2, plan §3/§4): the button's own
/// column (`name`, for the audit log's `detail`), `SUBMIT_VARS`'s output
/// column, and `SUBMIT_DONE`'s if the author gave one.
struct SubmitSpec {
    name: String,
    vars_col: usize,
    done_col: Option<usize>,
}

/// A private, randomly named 0700 directory holding the snapshot files;
/// removed on Drop (i.e. when the server stops or startup fails).
struct SnapshotDir(PathBuf);

impl SnapshotDir {
    fn create() -> Result<SnapshotDir, String> {
        let base = std::env::temp_dir();
        for _ in 0..8 {
            let p = base.join(format!("anofox-snap-{}", sv::random_token()));
            #[cfg_attr(not(unix), allow(unused_mut))]
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
/// The dashboards folder as last loaded (#772): what a snapshot is built
/// from. A snapshot keeps the set it was built from ([`Snapshot::dashes`]),
/// so a request always sees dashboards that match its snapshot's `deps`;
/// `anofox_serve_reload` loads a new set and swaps both together.
struct DashSet {
    list: Vec<Dash>,
    /// Extensions the snapshot loads: the dashboards' own `-- @load`s,
    /// `options.load`, and `json` under an access policy.
    loads: Vec<String>,
    /// `<dir>/assets`, canonicalized when the set was loaded (#514 PR B) —
    /// `None` when there is no such folder, so every
    /// `GET /d/<id>/assets/<rest>` request 404s ([`locked_asset`]).
    assets_root: Option<PathBuf>,
    /// #684 slice 3: per dashboard id, the declared tables its SQL names —
    /// those dashboards get them materialised (base ⊕ saves) and fold the
    /// change log's generation into their cache/warm keys (empty without
    /// `options.writeback`).
    dash_tables: HashMap<String, Vec<String>>,
}

impl DashSet {
    fn find(&self, id: &str) -> Option<&Dash> {
        self.list.iter().find(|d| d.id == id)
    }
}

/// Read the dashboards folder into a [`DashSet`]: the same reads and
/// refusals at start and on `anofox_serve_reload`.
fn load_dash_set(dir: &Path, opts: &LockedOptions) -> Result<DashSet, String> {
    let (list, mut loads) = load_dashboards(dir, opts.folders)?;
    // Review of #554 nit 8: a symlinked `assets` (e.g. `assets -> /`)
    // refuses to start, loudly — `canonical_assets_dir` itself already
    // treats it as `None` (no assets served), but silently starting with
    // assets "just off" would look identical to "there's no assets/ folder
    // here" to the operator; the mistake is worth surfacing.
    if anofox_visualization::image::assets_dir_is_symlink(dir) {
        return Err(format!(
            "anofox-visualization: '{}/assets' is a symlink — refusing to serve it (it may point outside the dashboards folder); replace it with a plain directory",
            dir.display()
        ));
    }
    let assets_root = anofox_visualization::image::canonical_assets_dir(dir);
    loads.extend(opts.load.iter().cloned());
    if opts.policy.is_some() {
        // json_serialize_sql: the policy rewrite parses with DuckDB.
        loads.push("json".into());
    }
    let dash_tables = match &opts.writeback_opts {
        None => HashMap::new(),
        Some(wb_opts) => list
            .iter()
            .map(|d| {
                let ts = wb_opts
                    .tables
                    .keys()
                    .filter(|t| wb::references(&d.sql, t))
                    .cloned()
                    .collect();
                (d.id.clone(), ts)
            })
            .collect(),
    };
    Ok(DashSet {
        list,
        loads,
        assets_root,
        dash_tables,
    })
}

struct Snapshot {
    // Field order = drop order: disconnect the warm connections, close the
    // database, then delete its files.
    /// Idle connections with a dashboard's setup already run (#388).
    warm: WarmPool,
    /// `-- @materialize` results (#727), in [`MAT_DB`].
    mats: MatStore,
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
    /// `options.nav_pickers`' computed state for this snapshot, aligned
    /// index-for-index with `opts.nav_pickers` — already checked read-only
    /// and value-validated ([`build_nav_picker`]). Recomputed on every
    /// refresh; empty without any pickers.
    nav_picker_snapshots: Vec<PickerSnapshot>,
    /// Which of the catalog's macros and views read variables (for a
    /// preview's deps, #770).
    implicit: HashMap<String, Deps>,
    /// #770: the previews' policy rewrite and deps on this snapshot, built
    /// on first use.
    previews: Mutex<HashMap<String, Arc<PreviewOn>>>,
    /// The dashboards this snapshot was built from (#772).
    dashes: Arc<DashSet>,
    _dir: SnapshotDir,
}

struct Enforced {
    enf: Enforcer,
    scripts: Vec<Script>,
    /// #736: saved tables the policy names → the TEMP table (base ⊕ saved
    /// rows) their policy view reads; `seed_writeback` builds them there.
    saved: BTreeMap<String, String>,
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

/// The server's own catalog for `-- @materialize` results (#727): an
/// in-memory database attached to the snapshot before its lockdown. Only the
/// server names it (reserved `__anofox` prefix).
const MAT_DB: &str = "__anofox_mat";

/// At most this many `-- @materialize` results per snapshot (least recently
/// used out; a refresh drops them all with the snapshot).
const MAT_CAP: usize = 16;

/// The `-- @materialize` results of one snapshot (#727): a heavy setup
/// statement's query runs once per [`mat_key`] into a table of [`MAT_DB`],
/// and every request connection that needs it gets its TEMP table as a copy
/// of that — instead of each cold panel request running the query again
/// (after a save, every panel of the page at once). Requests that need a
/// result being built wait for it.
/// One `-- @materialize` result: its table once built (locked while building).
type MatSlot = Arc<Mutex<Option<String>>>;

#[derive(Default)]
struct MatStore {
    /// Most recently used last.
    entries: Mutex<VecDeque<(String, MatSlot)>>,
    next: AtomicU64,
}

impl MatStore {
    /// The slot for `key` (created empty when new), and the table of an
    /// evicted entry to drop. An entry being built is never evicted.
    fn slot(&self, key: &str) -> (MatSlot, Option<String>) {
        let mut q = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = q.iter().position(|(k, _)| k == key) {
            let e = q.remove(i).expect("position is in range");
            let slot = e.1.clone();
            q.push_back(e);
            return (slot, None);
        }
        let slot = Arc::new(Mutex::new(None));
        q.push_back((key.to_string(), slot.clone()));
        let mut evicted = None;
        if q.len() > MAT_CAP {
            let free = q.iter().position(|(_, s)| s.try_lock().is_ok());
            if let Some(i) = free {
                let (_, s) = q.remove(i).expect("position is in range");
                evicted = s.lock().unwrap_or_else(|e| e.into_inner()).take();
            }
        }
        (slot, evicted)
    }

    /// Is `slot` still one of the entries (not evicted)?
    fn holds(&self, slot: &MatSlot) -> bool {
        let q = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        q.iter().any(|(_, s)| Arc::ptr_eq(s, slot))
    }
}

/// Run `-- @materialize` setup statement `sql` on `c` from the shared result
/// for `key` (#727), building it on `c` first when no request has. `Ok(false)`:
/// not done (not the `CREATE TEMP TABLE … AS <query>` form, or the build or
/// the copy failed) — the caller runs the statement as written. `Err` when
/// the request was abandoned or ran out of time waiting for another
/// request's build.
fn run_materialized(
    c: &Conn,
    mats: &MatStore,
    (dash_id, i): (&str, usize),
    key: &str,
    sql: &str,
    until: Instant,
    abort: &crate::panel_cache::Abort,
) -> Result<bool, String> {
    let Some((head, query)) = materialize::split(sql) else {
        return Ok(false);
    };
    let (slot, evicted) = mats.slot(key);
    if let Some(t) = evicted {
        // Best effort: a request still copying from it falls back to the
        // statement as written.
        let _ = c.exec(&format!("DROP TABLE IF EXISTS {t}"));
    }
    let mut built = loop {
        match slot.try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(e)) => break e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if abort.abandoned() {
                    return Err(SUPERSEDED.1.to_string());
                }
                if Instant::now() >= until {
                    return Err("query timed out waiting for a materialized setup statement".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    if built.is_none() {
        let t = format!(
            "{MAT_DB}.main.m{}",
            mats.next.fetch_add(1, Ordering::Relaxed)
        );
        let t0 = Instant::now();
        if let Err(e) = c.exec(&format!("CREATE TABLE {t} AS {query}")) {
            // Interrupted (superseded, or out of time): don't run it again.
            if abort.abandoned() {
                return Err(SUPERSEDED.1.to_string());
            }
            if Instant::now() >= until {
                return Err(e);
            }
            eprintln!(
                "[anofox-serve] materializing statement {i} of '{dash_id}' failed: {}",
                access::diag(&e)
            );
            return Ok(false);
        }
        eprintln!(
            "[anofox-serve] materialized statement {i} of '{dash_id}' in {} ms",
            t0.elapsed().as_millis()
        );
        *built = Some(t);
    }
    let t = built.clone().expect("built above");
    drop(built);
    let copied = c.exec(&format!("{head} FROM {t}")).is_ok();
    if !mats.holds(&slot) {
        // Evicted meanwhile: nobody else finds it any more.
        let _ = c.exec(&format!("DROP TABLE IF EXISTS {t}"));
    }
    Ok(copied)
}

/// Check the access policy against a new snapshot and rewrite every dashboard
/// for it. Fail closed: anything the policy refuses stops the start (or the
/// refresh — the previous snapshot then stays in service). `state_opts`
/// (#517 S3, plan §5): seeds the server-provided `viewer_state`/
/// `viewer_state_latest` temps into the rewrite for a state-enabled
/// dashboard, so `Mode::Verify` accepts references to them.
fn enforce(
    snap: &mut Snapshot,
    dashboards: &[Dash],
    policy: &Policy,
    key: &PolicyKey,
    state_opts: Option<&StateOptions>,
    wb: Option<(&WritebackOptions, &str)>,
) -> Result<(), String> {
    let c = snap.db.connect()?;
    crate::register_policy_hash(&c, key.0.clone())?;
    let mut duck = |sql: &str| c.query(sql)?.string_rows(1_000_000);
    // #736: the saved tables the policy names, and their TEMP stand-ins.
    let cover = match wb {
        Some((w, schema)) => writeback_policy_cover(w, policy, schema)?,
        None => BTreeMap::new(),
    };
    let redirect: HashMap<String, String> = match wb {
        Some((_, schema)) => cover
            .iter()
            .map(|(t, internal)| {
                (
                    format!("{schema}.{t}").to_ascii_lowercase(),
                    internal.clone(),
                )
            })
            .collect(),
        None => HashMap::new(),
    };
    let enf =
        Enforcer::compile_with(policy, &mut duck, &snap.default_db, &redirect).map_err(|e| {
            if e.contains("json_serialize_sql") {
                format!("{e} (the access policy needs the json extension: INSTALL json)")
            } else {
                e
            }
        })?;
    let mut scripts = Vec::new();
    for d in dashboards {
        let mut server_temps: Vec<&str> =
            if state_opts.is_some_and(|o| state_trainer_view(o, &d.id, None, None).is_some()) {
                vec!["viewer_state", "viewer_state_latest"]
            } else {
                Vec::new()
            };
        // #726: the saved tables this dashboard reads that the policy
        // doesn't name are TEMP tables the server builds per connection
        // (base ⊕ change log, `seed_writeback`); the ones it names are
        // policy tables like any other, whose view reads the saved rows
        // (#736).
        if let Some((w, _)) = wb {
            server_temps.extend(
                w.tables
                    .keys()
                    .filter(|t| !cover.contains_key(*t) && wb::references(&d.sql, t))
                    .map(String::as_str),
            );
        }
        scripts.push(
            enf.rewrite_script(&mut duck, &d.stmts, &server_temps)
                .map_err(|(i, e)| {
                    format!(
                        "dashboard '{}' statement {i}: {e} (refused by the access policy)",
                        d.id
                    )
                })?,
        );
    }
    snap.policy = Some(Enforced {
        enf,
        scripts,
        saved: cover,
    });
    Ok(())
}

/// The catalog's macros and views with their definitions ([`deps::catalog_reads`]).
const CATALOG_SQL: &str =
    "SELECT function_name, coalesce(macro_definition, '') FROM duckdb_functions() \
     WHERE function_type IN ('macro', 'table_macro') \
     UNION ALL SELECT view_name, coalesce(sql, '') FROM duckdb_views()";

/// One dashboard's variables on a snapshot: per statement what its answer
/// can depend on, their digest, and what its setup reads (warm keys). Under a
/// policy (`script`), the rewritten statements count too. Shared by
/// [`analyze_deps`] and the previews (#770).
fn dash_deps(
    d: &Dash,
    script: Option<&Script>,
    implicit: &HashMap<String, Deps>,
) -> (Vec<Deps>, String, Vec<Deps>) {
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
        f(&view, implicit)
    };
    let mut ds = of(&d.stmts, deps::script_deps);
    let mut ws = of(&d.stmts, deps::setup_deps);
    if let Some(sc) = script {
        for (a, b) in ds.iter_mut().zip(of(&sc.stmts, deps::script_deps)) {
            a.extend(&b);
        }
        for (a, b) in ws.iter_mut().zip(of(&sc.stmts, deps::setup_deps)) {
            a.extend(&b);
        }
    }
    let tag = deps_tag(&ds);
    (ds, tag, ws)
}

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
        let script = snap.policy.as_ref().map(|p| &p.scripts[di]);
        let (ds, tag, ws) = dash_deps(d, script, &implicit);
        tags.push(tag);
        all.push(ds);
        warm.push(ws);
    }
    snap.implicit = implicit;
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
    /// The dashboards folder (as given: a symlink swapped by a deploy is
    /// followed on the next `anofox_serve_reload`, #772).
    dir: PathBuf,
    /// The dashboards as last loaded: what the next (re)build snapshots.
    /// Requests read the set of the snapshot they run on
    /// (`gen.snap.dashes`), never this.
    dashes: RwLock<Arc<DashSet>>,
    opts: LockedOptions,
    /// What a (re)build snapshots: the serving database's context.
    ctx: Arc<DbCtx>,
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
    /// `options.state`'s live runtime (#517 S2) — `None` unless configured.
    submit: Option<Arc<SubmitRuntime>>,
    /// `options.writeback`'s live runtime (#684) — `None` unless configured.
    writeback: Option<Arc<WritebackRuntime>>,
    /// Request counts and latencies for `GET /metrics` (#765).
    metrics: anofox_visualization::host::metrics::Metrics,
    /// #767: per task, its last run (for `/metrics`).
    task_stats: Mutex<BTreeMap<String, TaskStat>>,
    /// #769: `options.ingest`'s live runtime (`None` unless configured).
    ingest: Option<Arc<IngestRuntime>>,
    /// #770: the stored previews (`None` without `options.preview`).
    previews: Option<Arc<PreviewStore>>,
    /// #775: the snapshots' bookkeeping (`/healthz`).
    durable: DurableState,
    /// #768: per report, its last run (for `/metrics`).
    report_stats: Mutex<BTreeMap<String, TaskStat>>,
    /// #768: the messages sent in the last hour (`notify.max_per_hour`).
    report_rate: Mutex<notify::Rate>,
    /// #768 R2: per alert, its checks (for `/metrics`).
    alert_stats: Mutex<BTreeMap<String, TaskStat>>,
    /// #768 R2: per alert, what it last sent (`notify.state` on disk).
    alert_states: Mutex<BTreeMap<String, notify::AlertState>>,
}

/// Live runtime for `options.state` (#517 S2, plan §1/§4): the open
/// [`Store`], the per-user [`RateLimiter`], a coarse global submit-rate
/// counter (plan §4.4: 20/s, all users), a per-viewer mutex (step 7: the
/// author's "only once" SQL gate and the append must be atomic for one
/// viewer, so two tabs can't race it) and the recent-nonce map (step 10: a
/// retry with the same `id` must not append twice).
struct SubmitRuntime {
    opts: StateOptions,
    store: Store,
    rate: RateLimiter,
    global: Mutex<GlobalBucket>,
    viewer_locks: Mutex<ViewerLocks>,
    recent: Mutex<RecentIds>,
    /// Set once `store.append` has ever refused with "full" (review of
    /// #538 should-fix 8: `/healthz`'s `state.full`). Append-only, so
    /// once full it never un-fulls itself in this process's lifetime.
    full: AtomicBool,
}

/// Live runtime for `options.writeback` (#684 slice 2): the change log (a
/// [`Store`] opened as one), and the same per-user rate limit, global cap,
/// per-writer mutex and recent-nonce map `/api/submit` uses.
struct WritebackRuntime {
    opts: WritebackOptions,
    store: Store,
    rate: RateLimiter,
    global: Mutex<GlobalBucket>,
    writer_locks: Mutex<ViewerLocks>,
    recent: Mutex<RecentIds>,
    full: AtomicBool,
    /// The change log folded at one generation, rebuilt after a save.
    folded: Mutex<Option<(u64, Arc<Folded>)>>,
    /// Held across a save's conflict check and its append (#684 slice 4),
    /// so two writers can't both pass the check against the same base.
    /// Never held across SQL or while a response is written: the check is
    /// in memory, the append one fdatasync.
    commit: Mutex<()>,
}

/// Each declared table as the change log leaves it ([`wb::fold`]).
type Folded = BTreeMap<String, wb::TableNow>;

impl WritebackRuntime {
    /// The change log's generation and its folded tables, read together
    /// (one `snapshot_all`, so the rows always match the generation a key
    /// folds in); folded once per generation.
    fn snapshot(&self) -> (u64, Arc<Folded>) {
        let (gen, records) = self.store.snapshot_all(usize::MAX);
        let mut cache = self.folded.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((g, f)) = cache.as_ref() {
            if *g == gen {
                return (gen, f.clone());
            }
        }
        let f = Arc::new(wb::fold(&self.opts, &records));
        if cache.as_ref().is_none_or(|(g, _)| *g <= gen) {
            *cache = Some((gen, f.clone()));
        }
        (gen, f)
    }
}

/// How an access policy covers the saved tables (#736): each saved table
/// the policy names in the schema saved tables resolve in (`schema`) →
/// the per-connection TEMP table its policy view reads instead of the base
/// (`__anofox_wb_<t>`: base ⊕ saved rows, so its row filter and column
/// rules apply to saved rows too). A saved table the policy names only in
/// another schema is refused: which one the policy means is ambiguous.
fn writeback_policy_cover(
    wb_opts: &WritebackOptions,
    policy: &Policy,
    schema: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for t in wb_opts.tables.keys() {
        let named: Vec<&str> = policy
            .tables
            .iter()
            .filter(|p| p.table.eq_ignore_ascii_case(t))
            .map(|p| p.schema.as_str())
            .collect();
        if named.iter().any(|s| s.eq_ignore_ascii_case(schema)) {
            out.insert(t.clone(), format!("__anofox_wb_{}", t.to_ascii_lowercase()));
        } else if !named.is_empty() {
            return Err(format!(
                "options.writeback.tables.{t}: the access policy names '{t}' in schema \
                 '{}', but saved tables live in '{schema}' — name it there or rename one",
                named.join("', '")
            ));
        }
    }
    Ok(out)
}

/// The schema unqualified (and so saved) table names resolve in: the
/// `search_path`'s first entry's, else `main`.
fn writeback_schema(opts: &LockedOptions) -> &str {
    opts.search_path
        .first()
        .and_then(|(_, s)| s.as_deref())
        .unwrap_or("main")
}

/// The columns of the table or view an unqualified `table` resolves to on
/// `c` (after the snapshot's `enter`: its current database and schema), or
/// `None` when there is no such base — then a saved table starts empty.
fn writeback_base_columns(c: &Conn, table: &str) -> Result<Option<Vec<String>>, String> {
    // `table` is a validated identifier (WritebackOptions::parse).
    let n: usize = c
        .query(&format!(
            "SELECT CAST(count(*) AS VARCHAR) FROM (SELECT table_name AS n, database_name AS d, \
             schema_name AS s \
             FROM duckdb_tables() WHERE NOT temporary UNION ALL SELECT view_name, database_name, \
             schema_name FROM duckdb_views() WHERE NOT temporary AND NOT internal) \
             WHERE lower(n) = lower('{table}') AND d = current_database() AND s = current_schema()"
        ))?
        .string_rows(1)?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if n == 0 {
        return Ok(None);
    }
    let cols = c
        .query(&format!("SELECT column_name FROM (DESCRIBE \"{table}\")"))?
        .string_rows(10_000)?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    Ok(Some(cols))
}

/// #684 slice 3: on a fresh connection, every declared table `tables` names
/// becomes a TEMP table with what the change log says it is now — the base
/// table cast to the declared types with the saved rows merged by key, or
/// the saved rows alone after a graph replaced it. The rows go through an
/// Appender into a staging table (never SQL text), and the statements are
/// [`wb::materialise_statements`]'s. Cold path only, like `viewer_state`.
fn seed_writeback(
    c: &Conn,
    opts: &WritebackOptions,
    folded: &Folded,
    tables: &[String],
    under_policy: &BTreeMap<String, String>,
) -> Result<(), String> {
    c.exec(&format!(
        "CREATE TEMP TABLE {} (t VARCHAR, \"row\" VARCHAR)",
        wb::STAGE
    ))?;
    {
        let app = crate::db::Appender::new_in(c, Some("temp"), Some("main"), wb::STAGE)?;
        for t in tables {
            for row in folded.get(t).map_or(&[][..], |n| &n.rows[..]) {
                app.begin_row()?;
                app.append_varchar(t)?;
                app.append_varchar(&Value::Object(row.clone()).to_string())?;
                app.end_row()?;
            }
        }
        app.close()?;
    }
    let empty = wb::TableNow::default();
    for t in tables {
        let decl = &opts.tables[t];
        let now = folded.get(t).unwrap_or(&empty);
        let base = if now.replaced {
            None
        } else {
            writeback_base_columns(c, t)?
        };
        for sql in wb::materialise_statements(t, decl, now, base.as_deref()) {
            c.exec(&sql)?;
        }
        // #736: a table the policy names is read only through the policy's
        // view, which reads this name (never the declared one).
        if let Some(internal) = under_policy.get(t) {
            c.exec(&format!(
                "ALTER TABLE temp.main.\"{t}\" RENAME TO \"{internal}\""
            ))?;
        }
    }
    c.exec(&format!("DROP TABLE temp.main.{}", wb::STAGE))
}

/// Why [`check_save_under_policy`] refused a save.
enum PolicyRefusal {
    /// The access policy forbids it (403, this text).
    Denied(String),
    /// The check itself failed (500; the details go to the log only — a
    /// DuckDB error could quote a row the writer may not see).
    Failed(String),
}

/// #736: may this writer save these changes into saved tables the access
/// policy names? A column the policy restricts for the writer may not be
/// written; a graph save (replacing every row) of a row-restricted table is
/// refused; and an upsert into one may only touch rows the writer can see,
/// before and after the save — checked as the writer, on a fresh
/// connection: the table built as panels see it (base ⊕ saved rows, then
/// with this save), counted directly and through the writer's own policy
/// view. Tables the policy doesn't name are not checked here (#726: read in
/// full).
fn check_save_under_policy(
    gen: &Generation<Snapshot>,
    w: &WritebackRuntime,
    claims: &Claims,
    changes: &Map<String, Value>,
    schema: &str,
) -> Result<(), PolicyRefusal> {
    let Some(p) = &gen.snap.policy else {
        return Ok(());
    };
    let viewer = Viewer {
        user: Some(claims.user.clone()),
        groups: claims.groups.clone(),
    };
    let mut row_checks = Vec::new();
    for ch in changes
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(t), Some(op)) = (
            ch.get("table").and_then(Value::as_str),
            ch.get("op").and_then(Value::as_str),
        ) else {
            continue;
        };
        if !p.saved.contains_key(t) {
            continue;
        }
        let Some(i) = p.enf.policy.find(schema, t) else {
            return Err(PolicyRefusal::Failed(format!(
                "policy table '{t}' not found"
            )));
        };
        let tp = &p.enf.policy.tables[i];
        let decl = &w.opts.tables[t];
        let rows: Vec<Map<String, Value>> = ch
            .get("rows")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
            .unwrap_or_default();
        let written: std::collections::BTreeSet<&str> = if op == "replace" {
            decl.columns.iter().map(|(c, _)| c.as_str()).collect()
        } else {
            rows.iter()
                .flat_map(|r| r.keys().map(String::as_str))
                .collect()
        };
        for c in written {
            let restricted = tp
                .columns
                .iter()
                .any(|r| r.column.eq_ignore_ascii_case(c) && !r.allows(&viewer));
            if restricted {
                return Err(PolicyRefusal::Denied(format!(
                    "you may not save '{t}.{c}': the access policy restricts that column for you"
                )));
            }
        }
        if tp.rows.is_some() {
            if op == "replace" {
                return Err(PolicyRefusal::Denied(format!(
                    "a graph save replaces every row of '{t}', whose rows the access policy \
                     restricts — that isn't allowed"
                )));
            }
            row_checks.push((t.to_string(), i, op.to_string(), rows));
        }
    }
    if row_checks.is_empty() {
        return Ok(());
    }
    let failed = PolicyRefusal::Failed;
    let c = gen.snap.db.connect().map_err(failed)?;
    c.exec(&gen.snap.enter).map_err(failed)?;
    for (name, lit) in claims.variables() {
        c.exec(&format!("SET VARIABLE {name} = {lit}"))
            .map_err(failed)?;
    }
    let (_, folded) = w.snapshot();
    for (t, i, op, rows) in row_checks {
        let decl = &w.opts.tables[&t];
        let internal = &p.saved[&t];
        // The keys this save touches, staged through an Appender (never SQL text).
        c.exec("CREATE OR REPLACE TEMP TABLE __anofox_wb_keys (\"row\" VARCHAR)")
            .map_err(failed)?;
        {
            let app =
                crate::db::Appender::new_in(&c, Some("temp"), Some("main"), "__anofox_wb_keys")
                    .map_err(failed)?;
            for r in &rows {
                app.begin_row().map_err(failed)?;
                app.append_varchar(&Value::Object(r.clone()).to_string())
                    .map_err(failed)?;
                app.end_row().map_err(failed)?;
            }
            app.close().map_err(failed)?;
        }
        let key_match = decl
            .key
            .iter()
            .map(|k| {
                let ty = decl.col(k).expect("a key is a declared column");
                format!(
                    "d.\"{k}\" = CAST(json_extract_string(CAST(k.\"row\" AS JSON), '$.{k}') AS {})",
                    ty.sql()
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let touched =
            format!("EXISTS (SELECT 1 FROM temp.main.__anofox_wb_keys k WHERE {key_match})");
        let before = folded.get(&t).cloned().unwrap_or_default();
        let mut after = before.clone();
        wb::apply_change(&mut after, decl, &op, rows.into_iter());
        for now in [before, after] {
            c.exec(&format!("DROP TABLE IF EXISTS temp.main.\"{internal}\""))
                .map_err(failed)?;
            let mut one = Folded::new();
            one.insert(t.clone(), now);
            seed_writeback(&c, &w.opts, &one, std::slice::from_ref(&t), &p.saved)
                .map_err(failed)?;
            for v in p.enf.view_statements(&[i], &viewer) {
                c.exec(&v).map_err(failed)?;
            }
            let count = |from: &str| -> Result<u64, PolicyRefusal> {
                c.query(&format!(
                    "SELECT CAST(count(*) AS VARCHAR) FROM {from} AS d WHERE {touched}"
                ))
                .and_then(|mut r| r.string_rows(1))
                .map_err(failed)?
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| failed("no count".into()))
            };
            let total = count(&format!("temp.main.\"{internal}\""))?;
            let seen = count(&format!("temp.main.{}{i}", policy::VIEW_PREFIX))?;
            if seen < total {
                return Err(PolicyRefusal::Denied(format!(
                    "you may only save rows of '{t}' you can see: this save touches a row \
                     the access policy hides from you, or makes one"
                )));
            }
        }
    }
    Ok(())
}

/// Plan §4.4's bound for the rate limiter's, the mutex map's and the
/// nonce map's distinct-user count — "bounded (LRU, 100k users)".
const SUBMIT_BOUNDED_USERS: usize = 100_000;
/// Plan §4.4: "global submits (all users): 20/s".
const GLOBAL_SUBMITS_PER_SEC: u32 = 20;
/// Plan §4.2 step 10: "the last 10 min".
const RECENT_ID_TTL: Duration = Duration::from_secs(600);

/// A one-second fixed window counter — coarser than the per-user token
/// bucket on purpose: it only needs to catch a flood across every viewer,
/// not account for any one of them fairly.
struct GlobalBucket {
    window_start: Instant,
    count: u32,
}

impl GlobalBucket {
    fn new() -> Self {
        GlobalBucket {
            window_start: Instant::now(),
            count: 0,
        }
    }

    /// `true`: under the cap this second, counted. `false`: at the cap,
    /// not counted (a backed-off client doesn't dig itself in deeper).
    fn take(&mut self, cap: u32, now: Instant) -> bool {
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.count = 0;
        }
        if self.count < cap {
            self.count += 1;
            true
        } else {
            false
        }
    }
}

/// Bounded `user -> Arc<Mutex<()>>`, oldest evicted (plan §4.4).
struct ViewerLocks {
    map: HashMap<String, Arc<Mutex<()>>>,
    order: VecDeque<String>,
    capacity: usize,
}

impl ViewerLocks {
    fn new() -> Self {
        Self::with_capacity(SUBMIT_BOUNDED_USERS)
    }

    /// Like [`Self::new`], but with an explicit `capacity` — the real
    /// code always uses [`SUBMIT_BOUNDED_USERS`]; a test uses a small one
    /// to exercise eviction without 100k inserts.
    fn with_capacity(capacity: usize) -> Self {
        ViewerLocks {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn get(&mut self, user: &str) -> Arc<Mutex<()>> {
        if let Some(m) = self.map.get(user) {
            return m.clone();
        }
        if self.map.len() >= self.capacity {
            // Review of #538, nit: skip an entry some other thread is
            // currently holding (`strong_count > 1`: the map's own clone,
            // plus at least one in a request's `_guard` scope) — evicting
            // it would hand `user` a brand-new, un-contended mutex while
            // the real holder still thinks it has exclusive access,
            // reopening the "only once" race for that one moment. Falls
            // through (the map grows past the bound by one) only in the
            // pathological case where every entry is held at once.
            let mut requeued = Vec::new();
            while let Some(oldest) = self.order.pop_front() {
                let held = self
                    .map
                    .get(&oldest)
                    .is_some_and(|m| Arc::strong_count(m) > 1);
                if held {
                    requeued.push(oldest);
                    continue;
                }
                self.map.remove(&oldest);
                break;
            }
            // Entries skipped over go back to the front, oldest first, so
            // the next eviction tries them again in the same order.
            for u in requeued.into_iter().rev() {
                self.order.push_front(u);
            }
        }
        let m = Arc::new(Mutex::new(()));
        self.map.insert(user.to_string(), m.clone());
        self.order.push_back(user.to_string());
        m
    }
}

/// One idempotent reply (plan §4.2 step 10), kept for [`RECENT_ID_TTL`].
#[derive(Clone)]
struct RecentReply {
    body: String,
    at: Instant,
}

// (user, dashboard, id): review of #538, nit — keying on `(user, id)`
// alone means the same client nonce sent for a *different* dashboard
// replays the first dashboard's reply. Clients mint one nonce per click,
// so this was harmless in practice, but the stricter key costs nothing.
type RecentKey = (String, String, String);

struct RecentIds {
    map: HashMap<RecentKey, RecentReply>,
    order: VecDeque<(RecentKey, Instant)>,
}

impl RecentIds {
    fn new() -> Self {
        RecentIds {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn evict_old(&mut self, now: Instant) {
        while let Some((_, at)) = self.order.front() {
            if now.duration_since(*at) > RECENT_ID_TTL {
                if let Some((k, _)) = self.order.pop_front() {
                    self.map.remove(&k);
                }
            } else {
                break;
            }
        }
    }

    fn get(&mut self, user: &str, dashboard: &str, id: &str, now: Instant) -> Option<String> {
        self.evict_old(now);
        self.map
            .get(&(user.to_string(), dashboard.to_string(), id.to_string()))
            .map(|r| r.body.clone())
    }

    fn insert(&mut self, user: &str, dashboard: &str, id: &str, body: String, now: Instant) {
        self.evict_old(now);
        if self.map.len() >= SUBMIT_BOUNDED_USERS {
            if let Some((k, _)) = self.order.pop_front() {
                self.map.remove(&k);
            }
        }
        let key = (user.to_string(), dashboard.to_string(), id.to_string());
        self.map.insert(key.clone(), RecentReply { body, at: now });
        self.order.push_back((key, now));
    }
}

// `anofox_data`: the `SET VARIABLE` that tells panels where `options.data`
// is. Viewers can never set it (`access::is_reserved_var`, any request).
use anofox_visualization::host::access::DATA_VAR;

impl Locked {
    /// The dashboards the next build snapshots.
    fn latest(&self) -> Arc<DashSet> {
        self.dashes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn build(&self, dashes: &Arc<DashSet>) -> Result<(Snapshot, Vec<String>), String> {
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
            build_snapshot(&conn, &self.opts, dashes, dir, source_result)?
        };
        if let (Some(p), Some(k)) = (&self.opts.policy, &self.opts.policy_key) {
            enforce(
                &mut snap,
                &dashes.list,
                p,
                k,
                self.submit.as_deref().map(|s| &s.opts),
                self.opts
                    .writeback_opts
                    .as_ref()
                    .map(|w| (w, writeback_schema(&self.opts))),
            )?;
        }
        analyze_deps(&mut snap, &dashes.list)?;
        Ok((snap, warnings))
    }

    /// Rebuild the snapshot and swap it in (`wait`: see [`Current::rebuild`]).
    fn refresh(&self, wait: bool) -> Result<Option<String>, String> {
        let t0 = Instant::now();
        let mut warnings = Vec::new();
        let gen = self
            .current
            .rebuild(wait, || {
                // Read inside the rebuild lock: a reload's new set is in
                // place before any later refresh builds.
                let (snap, w) = self.build(&self.latest())?;
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

    /// `anofox_serve_reload(port)` (#772): read the dashboards folder again,
    /// check it like a start, build a snapshot from it and swap both in. On
    /// any error the running set and snapshot stay in service. Task files and
    /// the server options are not re-read (that takes a restart).
    fn reload(&self) -> Result<String, String> {
        let t0 = Instant::now();
        let fail = |e: String| {
            eprintln!("[anofox-serve] reload failed (still serving the previous dashboards): {e}");
            format!(
                "anofox-visualization: reload failed, still serving the previous dashboards: {e}"
            )
        };
        let mut warnings = Vec::new();
        let mut count = 0;
        let gen = self
            .current
            .rebuild(true, || {
                let set = Arc::new(load_dash_set(&self.dir, &self.opts)?);
                let (snap, w) = self.build(&set)?;
                check_dash_set(&set, &snap, &self.opts, &self.dir)?;
                warnings = w;
                count = set.list.len();
                *self.dashes.write().unwrap_or_else(|p| p.into_inner()) = set;
                Ok(snap)
            })
            .map_err(fail)?
            .ok_or_else(|| fail("a rebuild was skipped".into()))?;
        let warn = if warnings.is_empty() {
            String::new()
        } else {
            format!(" (warnings: {})", warnings.join("; "))
        };
        let msg = format!(
            "anofox-visualization: port {} reloaded {count} dashboard(s), snapshot epoch {} (built in {} ms){warn}",
            self.port,
            gen.epoch,
            t0.elapsed().as_millis()
        );
        eprintln!("[anofox-serve] {msg}");
        Ok(msg)
    }
}

/// What a dashboard set must pass against its snapshot, at start and on
/// `anofox_serve_reload` (#772): the dashboards and panels the options name,
/// one statement each, the data folder's read-only rule, and
/// `options.state` / `options.writeback`'s start checks.
fn check_dash_set(
    set: &DashSet,
    snap: &Snapshot,
    opts: &LockedOptions,
    dir: &Path,
) -> Result<(), String> {
    // #768: a report names a dashboard here (and its csv/json formats a
    // `-- @name`d panel of it).
    for r in &opts.reports {
        let d = set
            .find(&r.dashboard)
            .ok_or_else(|| format!("report '{}': no dashboard '{}'", r.name, r.dashboard))?;
        let plan = anofox_visualization::sql::plan(&d.sql);
        for f in &r.formats {
            if let notify::Format::Csv(p) | notify::Format::Json(p) = f {
                if !plan
                    .iter()
                    .any(|x| !x.setup && x.name.as_deref() == Some(p.as_str()))
                {
                    return Err(format!(
                        "report '{}': dashboard '{}' has no panel named '{p}' (`-- @name {p}`)",
                        r.name, r.dashboard
                    ));
                }
            }
        }
    }
    // #768 R2: an alert names a dashboard here and a `-- @name`d panel of it.
    for a in &opts.alerts {
        let d = set
            .find(&a.dashboard)
            .ok_or_else(|| format!("alert '{}': no dashboard '{}'", a.name, a.dashboard))?;
        if !anofox_visualization::sql::plan(&d.sql)
            .iter()
            .any(|x| !x.setup && x.name.as_deref() == Some(a.panel.as_str()))
        {
            return Err(format!(
                "alert '{}': dashboard '{}' has no panel named '{}' (`-- @name {}`)",
                a.name, a.dashboard, a.panel, a.panel
            ));
        }
    }
    if let Some(h) = &opts.home {
        if set.find(h).is_none() {
            return Err(format!(
                "anofox-visualization: options.home: no dashboard '{h}' in '{}'",
                dir.display()
            ));
        }
    }
    // Validate every statement with DuckDB's parser: exactly one each.
    {
        let c = snap.db.connect()?;
        for d in &set.list {
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
            check_read_only(&c, &set.list)?;
        }
    }
    // #517 S2: `options.state`'s start-time refusals (plan §2) need the
    // dashboards and the snapshot.
    if let Some(state_opts) = &opts.state_opts {
        let known_dashboards: Vec<String> = set.list.iter().map(|d| d.id.clone()).collect();
        let reserved_names: Vec<String> = {
            let c = snap.db.connect()?;
            c.query(
                "SELECT table_name FROM duckdb_tables() UNION ALL \
                     SELECT view_name FROM duckdb_views()",
            )
            .map_err(|e| format!("options.state: {e}"))?
            .string_rows(1_000_000)
            .map_err(|e| format!("options.state: {e}"))?
            .into_iter()
            .flatten()
            .flatten()
            .collect()
        };
        let ctx = StartContext {
            has_auth: opts.auth.is_some(),
            embed_without_frame_ancestors: opts.auth.as_ref().is_some_and(|a| a.embed.is_some())
                && opts.frame_ancestors.is_none(),
            data_dir: opts.data.as_deref(),
            dashboard_dir: dir,
            known_dashboards: &known_dashboards,
            reserved_names: &reserved_names,
            search_path_entries: opts.search_path.len(),
        };
        state_opts.validate_start(&ctx)?;
    }
    // #684 slice 2: `options.writeback`'s (and `options.state`'s path, which
    // it must not share).
    if let Some(wb_opts) = &opts.writeback_opts {
        let known_dashboards: Vec<String> = set.list.iter().map(|d| d.id.clone()).collect();
        wb_opts.validate_start(&wb::StartContext {
            has_auth: opts.auth.is_some(),
            embed_without_frame_ancestors: opts.auth.as_ref().is_some_and(|a| a.embed.is_some())
                && opts.frame_ancestors.is_none(),
            data_dir: opts.data.as_deref(),
            dashboard_dir: dir,
            known_dashboards: &known_dashboards,
            state_path: opts.state_opts.as_ref().map(|s| s.path.as_path()),
        })?;
    }
    Ok(())
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
    // #767: tasks/*.task.sql — read (and checked) whenever `options.tasks` is
    // set; run only on the replica with `tasks.run`, after its attachments
    // passed the READ_ONLY check (D2).
    let task_list = match &opts.tasks {
        None => Vec::new(),
        Some(t) => {
            let list = load_tasks(Path::new(dir))?;
            if t.run {
                let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
                check_task_attachments(&conn, &t.write)?;
                list
            } else {
                Vec::new()
            }
        }
    };
    // #768: on the sending replica a report's schedule must evaluate now,
    // so a typo fails the start, not the first send (the dashboard and panel
    // it names: `check_dash_set`).
    for r in &opts.reports {
        if opts.notify.as_ref().is_some_and(|n| n.run) {
            let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
            eval_schedule(&conn, &r.schedule)
                .map_err(|e| format!("report '{}': schedule: {e}", r.name))?;
        }
    }
    // #768 R2: an alert's check must evaluate on the sending replica. Its
    // state is read now (a broken file stops the start rather than send
    // everything again).
    for a in &opts.alerts {
        if opts.notify.as_ref().is_some_and(|n| n.run) {
            let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
            eval_schedule(&conn, &a.schedule)
                .map_err(|e| format!("alert '{}': check: {e}", a.name))?;
        }
    }
    let alert_states = match opts.notify.as_ref().and_then(|n| n.state.as_deref()) {
        Some(p) if !opts.alerts.is_empty() => notify::load_states(p)?,
        _ => BTreeMap::new(),
    };
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
    let dashes = Arc::new(load_dash_set(Path::new(dir), &opts)?);
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
        build_snapshot(&conn, &opts, &dashes, startup_dir, source_result)?
    };
    if let (Some(p), Some(k)) = (&opts.policy, &opts.policy_key) {
        enforce(
            &mut snap,
            &dashes.list,
            p,
            k,
            opts.state_opts.as_ref(),
            opts.writeback_opts
                .as_ref()
                .map(|w| (w, writeback_schema(&opts))),
        )?;
    }
    analyze_deps(&mut snap, &dashes.list)?;
    check_dash_set(&dashes, &snap, &opts, Path::new(dir))?;
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
    // #726: saved tables the policy doesn't name are outside it — say so
    // at start (#736: the ones it names are read through its views).
    let unpoliced: Vec<&str> = match (&opts.policy, &opts.writeback_opts) {
        (Some(p), Some(w)) => {
            let cover = writeback_policy_cover(w, p, writeback_schema(&opts)).unwrap_or_default();
            let mut ts: Vec<&str> = w
                .tables
                .keys()
                .filter(|t| !cover.contains_key(*t))
                .map(String::as_str)
                .collect();
            ts.sort();
            ts
        }
        _ => Vec::new(),
    };
    let policy_note = match (&opts.policy, &opts.writeback_opts) {
        (Some(_), Some(_)) if !unpoliced.is_empty() => {
            let ts = &unpoliced;
            format!(
                "{policy_note}; saved table(s) {} are not under the policy — every viewer \
                 of a dashboard that reads one sees all of its rows",
                ts.join(", ")
            )
        }
        _ => policy_note,
    };
    // #775: a new node (no local file) starts from the newest snapshot; a
    // local file always wins, so a restore never overwrites newer rows.
    if let Some(d) = opts.durability.as_ref().filter(|d| d.run) {
        let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        if d.is_local() {
            std::fs::create_dir_all(&d.location)
                .map_err(|e| format!("options.durability.location {}: {e}", d.location))?;
        }
        for (name, path) in durable_files(&opts) {
            if !path.exists() {
                if let Some(n) = durable_restore(&conn, d, name, &path)? {
                    eprintln!(
                        "[anofox-serve] restored {} ({n} bytes) from {}",
                        path.display(),
                        d.object(name)
                    );
                }
            }
        }
    }
    // #517 S2: `options.state`'s start-time refusals (plan §2) need the
    // dashboards and the snapshot, both only available from here on — not
    // inside `LockedOptions::parse`, which runs before either exists.
    let submit = match &opts.state_opts {
        None => None,
        Some(state_opts) => {
            let store = Store::open(&state_opts.path, state_opts.max_mb)?;
            let rate = RateLimiter::new(
                state_opts.rate_per_minute,
                state_opts.rate_burst,
                SUBMIT_BOUNDED_USERS,
            );
            Some(Arc::new(SubmitRuntime {
                opts: state_opts.clone(),
                store,
                rate,
                global: Mutex::new(GlobalBucket::new()),
                viewer_locks: Mutex::new(ViewerLocks::new()),
                recent: Mutex::new(RecentIds::new()),
                full: AtomicBool::new(false),
            }))
        }
    };
    // #684 slice 2: `options.writeback`'s start checks need the dashboards
    // (and `options.state`'s path, which it must not share).
    let writeback = match &opts.writeback_opts {
        None => None,
        Some(wb_opts) => {
            // #726/#736: with an access policy, a saved table the policy
            // names is read through the policy's view of base ⊕ saved rows
            // (and saves into it are checked as the writer, `handle_save`);
            // one it doesn't name is read in full (said at start).
            if let Some(p) = &opts.policy {
                writeback_policy_cover(wb_opts, p, writeback_schema(&opts))?;
            }
            if opts.search_path.len() > 1 {
                return Err(
                    "options.writeback needs options.search_path to name at most one database"
                        .into(),
                );
            }
            {
                // Every column of a base table must be declared, or a panel
                // reading the saved table would silently lose it.
                let c = snap.db.connect()?;
                c.exec(&snap.enter)?;
                for (t, decl) in &wb_opts.tables {
                    if let Some(cols) = writeback_base_columns(&c, t)
                        .map_err(|e| format!("options.writeback.tables.{t}: {e}"))?
                    {
                        let missing = wb::undeclared_base_columns(decl, &cols);
                        if !missing.is_empty() {
                            return Err(format!(
                                "options.writeback.tables.{t}: the table has columns the \
                                 declaration doesn't name ({}) — declare every column, or \
                                 a saved {t} would hide them",
                                missing.join(", ")
                            ));
                        }
                    }
                }
            }
            let store = Store::open_writeback(&wb_opts.path, wb_opts.max_mb)?;
            Some(Arc::new(WritebackRuntime {
                folded: Mutex::new(None),
                commit: Mutex::new(()),
                opts: wb_opts.clone(),
                store,
                rate: RateLimiter::new(
                    wb_opts.rate_per_minute,
                    wb_opts.rate_burst,
                    SUBMIT_BOUNDED_USERS,
                ),
                global: Mutex::new(GlobalBucket::new()),
                writer_locks: Mutex::new(ViewerLocks::new()),
                recent: Mutex::new(RecentIds::new()),
                full: AtomicBool::new(false),
            }))
        }
    };
    // #770: previews kept on disk come back (expired ones are dropped).
    let preview_store = match &opts.preview {
        None => None,
        Some(po) => Some(Arc::new(PreviewStore::open(po.clone())?)),
    };
    // #769: the writing replica's ingest database is attached read-write.
    let ingest_rt = match &opts.ingest {
        None => None,
        Some(ing) => {
            if ing.run {
                let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
                check_ingest_database(&conn, &ing.database)?;
            }
            Some(Arc::new(IngestRuntime {
                rate: RateLimiter::new(ing.rate_per_minute, ing.rate_per_minute.min(60), 4096),
                write: Mutex::new(()),
                want_refresh: (Mutex::new(false), Condvar::new()),
            }))
        }
    };
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
    let n = dashes.list.len();
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
        dir: PathBuf::from(dir),
        dashes: RwLock::new(dashes),
        current: Current::new(snap, opts.cache_bytes),
        opts,
        ctx,
        server_id: sv::random_token()[..8].to_string(),
        inflight,
        queries,
        // A few seconds: files appear live, a rescan per request is avoided.
        requires_cache: rq::ExistsCache::new(Duration::from_secs(5), 4096),
        stop: stop.clone(),
        source_build: SourceBuildState::new(),
        submit,
        writeback,
        metrics: Default::default(),
        task_stats: Mutex::new(BTreeMap::new()),
        ingest: ingest_rt,
        previews: preview_store,
        durable: DurableState::default(),
        report_stats: Mutex::new(BTreeMap::new()),
        report_rate: Mutex::new(notify::Rate::default()),
        alert_stats: Mutex::new(BTreeMap::new()),
        alert_states: Mutex::new(alert_states),
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
    // #767: the task runner, on this replica only when `tasks.run`.
    let tasks_handle = (!task_list.is_empty()).then(|| {
        let (st, stop) = (state.clone(), stop.clone());
        std::thread::spawn(move || task_runner(&st, &stop, task_list))
    });
    // #768: the report runner, on this replica only when `notify.run`.
    let reports_running = !(state.opts.reports.is_empty() && state.opts.alerts.is_empty())
        && state.opts.notify.as_ref().is_some_and(|n| n.run);
    // #768 D4: each report's principal, for the operator to attest.
    let report_note: String = state
        .opts
        .reports
        .iter()
        .map(|r| {
            let p = &r.principal;
            let attrs: Vec<String> = p
                .attrs
                .iter()
                .map(|(k, v)| format!("{k}={}", v.as_deref().unwrap_or("NULL")))
                .collect();
            format!(
                "; report '{}' runs as {} (groups: [{}], claims: [{}]) to {} recipient(s){}",
                r.name,
                p.user,
                p.groups.join(", "),
                attrs.join(", "),
                r.to.len(),
                if reports_running {
                    ""
                } else {
                    ", not sent from this replica"
                }
            )
        })
        .chain(state.opts.alerts.iter().map(|a| {
            format!(
                "; alert '{}' runs as {} to {} recipient(s){}",
                a.name,
                a.principal.user,
                a.to.len(),
                if reports_running {
                    ""
                } else {
                    ", not sent from this replica"
                }
            )
        }))
        .collect();
    let reports_handle = reports_running.then(|| {
        let (st, stop) = (state.clone(), stop.clone());
        std::thread::spawn(move || report_runner(&st, &stop))
    });
    // #775: the durability snapshots, on the writer only.
    let durable_handle = state
        .opts
        .durability
        .as_ref()
        .is_some_and(|d| d.run)
        .then(|| {
            let (st, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || durability_runner(&st, &stop))
        });
    // #769: rebuilds the snapshot after writes, at most every 30 s.
    let ingest_handle = state
        .opts
        .ingest
        .as_ref()
        .is_some_and(|i| i.run && i.refresh)
        .then(|| {
            let (st, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || ingest_refresher(&st, &stop))
        });
    servers.insert(
        port,
        Running {
            kind: "locked",
            server,
            stop,
            workers,
            refresher: refresher_handle,
            tasks: tasks_handle,
            reports: reports_handle,
            ingest: ingest_handle,
            durable: durable_handle,
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
        "anofox-visualization serving {n} locked, read-only dashboard(s) at http://{addr}/ — server owns the SQL{auth_note}{policy_note}{report_note}{warn}"
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

// ---- #767: tasks ---------------------------------------------------------

/// A task's last runs, for `/metrics`.
#[derive(Debug, Default, Clone)]
struct TaskStat {
    last_ok: Option<u64>,
    runs: u64,
    failures: u64,
    failing_in_row: u64,
}

/// `<dir>/tasks/*.task.sql`, by file name; a task that doesn't parse stops
/// the start (naming it).
fn load_tasks(dir: &Path) -> Result<Vec<tasks::Task>, String> {
    let td = dir.join("tasks");
    let Ok(rd) = std::fs::read_dir(&td) else {
        return Ok(Vec::new());
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".task.sql"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| {
            let name = p
                .file_name()
                .map(|n| {
                    n.to_string_lossy()
                        .trim_end_matches(".task.sql")
                        .to_string()
                })
                .unwrap_or_default();
            let script = std::fs::read_to_string(p).map_err(|e| format!("task '{name}': {e}"))?;
            tasks::parse(&name, &script)
        })
        .collect()
}

/// One query's rows as JSON objects (small results only).
fn small_rows(c: &Conn, sql: &str) -> Result<Vec<Map<String, Value>>, String> {
    let json = crate::db::json_rows(c, sql, &[], 10_000, &mut |_, want| Ok(want))?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

/// D2: every database not in `write` must be attached READ_ONLY.
fn check_task_attachments(c: &Conn, write: &[String]) -> Result<(), String> {
    let rows = small_rows(
        c,
        "SELECT database_name AS n, readonly AS r FROM duckdb_databases() WHERE NOT internal",
    )?;
    let dbs: Vec<(String, bool)> = rows
        .iter()
        .map(|r| {
            (
                r.get("n").and_then(Value::as_str).unwrap_or("").to_string(),
                r.get("r").and_then(Value::as_bool).unwrap_or(false),
            )
        })
        .collect();
    tasks::check_attachments(&dbs, write)
}

/// When a task runs next.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Next {
    /// `'init'`: once, at the start.
    Init,
    /// An `INTERVAL`: now, then every so many seconds.
    Every(f64),
    /// A `TIMESTAMP` (Unix seconds), evaluated again after each run.
    At(f64),
}

fn eval_schedule(c: &Conn, expr: &str) -> Result<Next, String> {
    let rows = small_rows(
        c,
        &format!("SELECT typeof(e) AS t, e::VARCHAR AS v FROM (SELECT ({expr}) AS e)"),
    )?;
    let r = rows.first().ok_or("the schedule returned no row")?;
    let t = r.get("t").and_then(Value::as_str).unwrap_or("");
    let num = |sql: String| -> Result<f64, String> {
        small_rows(c, &sql)?
            .first()
            .and_then(|r| r.get("s"))
            .and_then(Value::as_f64)
            .ok_or_else(|| "the schedule is NULL".to_string())
    };
    match t {
        "INTERVAL" => {
            let s = num(format!("SELECT epoch(({expr})::INTERVAL) AS s"))?;
            if s < 1.0 {
                return Err("the schedule's interval must be at least 1 second".into());
            }
            Ok(Next::Every(s))
        }
        t if t.starts_with("TIMESTAMP") || t == "DATE" => Ok(Next::At(num(format!(
            "SELECT epoch(({expr})::TIMESTAMPTZ) AS s"
        ))?)),
        "VARCHAR" if r.get("v").and_then(Value::as_str) == Some("init") => Ok(Next::Init),
        _ => Err(format!(
            "::SCHEDULE must be an INTERVAL, a TIMESTAMP or 'init', not {t}"
        )),
    }
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The task thread: runs each task when it is due, one at a time, until the
/// server stops.
fn task_runner(st: &Locked, stop: &AtomicBool, list: Vec<tasks::Task>) {
    let Some(opts) = st.opts.tasks.clone() else {
        return;
    };
    let jobs: Vec<(&str, &str)> = list
        .iter()
        .map(|t| (t.name.as_str(), t.schedule.as_str()))
        .collect();
    run_on_schedule(st, stop, "task", &jobs, true, |i| {
        let t = &list[i];
        if run_task(st, &opts, t) && t.refresh {
            if let Err(e) = st.refresh(false) {
                eprintln!(
                    "[anofox-serve] task '{}': refresh failed: {}",
                    t.name,
                    access::diag(&e)
                );
            }
        }
    });
}

/// Run each job (`(name, schedule SQL)`) when it is due, one at a time,
/// until the server stops (#767 tasks, #768 reports). An INTERVAL runs
/// first at the start when `interval_now` (tasks), else one interval later
/// (reports: a restart doesn't send again).
fn run_on_schedule(
    st: &Locked,
    stop: &AtomicBool,
    what: &str,
    jobs: &[(&str, &str)],
    interval_now: bool,
    mut run: impl FnMut(usize),
) {
    let schedule = |sql: &str| -> Result<Next, String> {
        let c = st.ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        eval_schedule(&c, sql)
    };
    let warn = |name: &str, e: &str| {
        eprintln!("[anofox-serve] {what} '{name}': {}", access::diag(e));
    };
    // When each job is due next (`None`: never again).
    let mut due: Vec<Option<f64>> = jobs
        .iter()
        .map(|(name, sql)| match schedule(sql) {
            Ok(Next::Init) => Some(unix_now()),
            Ok(Next::Every(s)) => Some(unix_now() + if interval_now { 0.0 } else { s }),
            Ok(Next::At(at)) => Some(at),
            Err(e) => {
                warn(name, &e);
                Some(unix_now() + 60.0)
            }
        })
        .collect();
    while !stop.load(Ordering::SeqCst) {
        for (i, (name, sql)) in jobs.iter().enumerate() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if !due[i].is_some_and(|d| unix_now() >= d) {
                continue;
            }
            run(i);
            due[i] = match schedule(sql) {
                Ok(Next::Init) => None,
                Ok(Next::Every(s)) => Some(unix_now() + s),
                Ok(Next::At(at)) if at > unix_now() + 1.0 => Some(at),
                Ok(Next::At(_)) => {
                    warn(
                        name,
                        "its schedule gave a time that has passed; tried again in 60 s",
                    );
                    Some(unix_now() + 60.0)
                }
                Err(e) => {
                    warn(name, &e);
                    Some(unix_now() + 60.0)
                }
            };
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Run one task on the serving session's connection (D1), with its time
/// limit; record it. `true` when every statement succeeded.
fn run_task(st: &Locked, opts: &TasksOptions, t: &tasks::Task) -> bool {
    let started = unix_now();
    let t0 = Instant::now();
    let res = {
        let c = st.ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        let (r, fired) = c.with_deadline_fired(opts.timeout, |c| {
            for s in &t.statements {
                c.exec(s)?;
            }
            Ok(())
        });
        let r = if fired {
            Err(format!("timed out after {} s", opts.timeout.as_secs()))
        } else {
            r
        };
        // A task that stopped (or ended) inside BEGIN … COMMIT would leave
        // the shared connection in a transaction, and every later task and
        // snapshot build would fail on it: roll it back. ROLLBACK succeeds
        // only when one was open.
        match (c.exec("ROLLBACK"), r) {
            (Ok(_), Ok(())) => {
                Err("it left a transaction open (BEGIN without COMMIT); rolled back".into())
            }
            (_, r) => r,
        }
    };
    let ok = res.is_ok();
    let res = res.map_err(|e| redact(e, st.opts.data.as_deref()));
    let was_failing = {
        let mut m = st.task_stats.lock().unwrap_or_else(|p| p.into_inner());
        let s = m.entry(t.name.clone()).or_default();
        let was = s.failing_in_row > 0;
        s.runs += 1;
        if ok {
            s.last_ok = Some(started as u64);
            s.failing_in_row = 0;
        } else {
            s.failures += 1;
            s.failing_in_row += 1;
        }
        was
    };
    let line = serde_json::json!({
        "task": t.name,
        "start": started as u64,
        "ms": t0.elapsed().as_millis() as u64,
        "ok": ok,
        "error": res.as_ref().err(),
    })
    .to_string();
    run_log(opts.log.as_deref(), "task", &line);
    // #768 R2: the first failure in a row, and the recovery after one.
    if !opts.on_failure.is_empty() && was_failing != !ok {
        task_notify(
            st,
            &opts.on_failure,
            t,
            res.as_ref().err().map(String::as_str),
        );
    }
    ok
}

/// One JSON line of a task's or report's run log (a file, or stderr).
fn run_log(dest: Option<&str>, what: &str, line: &str) {
    match dest {
        None | Some("stderr") => eprintln!("[anofox-serve] {what} {line}"),
        Some(path) => {
            use std::io::Write as _;
            let w = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| writeln!(f, "{line}"));
            if let Err(e) = w {
                eprintln!("[anofox-serve] {what} log {}: {e}", access::diag(path));
            }
        }
    }
}

/// #767: the tasks' gauges for `/metrics`.
fn task_gauges(st: &Locked) -> Vec<metrics::Gauge> {
    let m = st.task_stats.lock().unwrap_or_else(|p| p.into_inner());
    if m.is_empty() {
        return Vec::new();
    }
    type Values = Vec<(Vec<(&'static str, String)>, f64)>;
    let per = |f: &dyn Fn(&TaskStat) -> Option<f64>| -> Values {
        m.iter()
            .filter_map(|(k, s)| f(s).map(|v| (vec![("task", k.clone())], v)))
            .collect()
    };
    let g = |name, help, values| metrics::Gauge { name, help, values };
    vec![
        g(
            "anofox_task_last_success_timestamp_seconds",
            "When each task last ran without an error (Unix seconds).",
            per(&|s| s.last_ok.map(|v| v as f64)),
        ),
        g(
            "anofox_task_runs",
            "Runs of each task since the start.",
            per(&|s| Some(s.runs as f64)),
        ),
        g(
            "anofox_task_failures",
            "Failed runs of each task since the start.",
            per(&|s| Some(s.failures as f64)),
        ),
        g(
            "anofox_task_consecutive_failures",
            "Failed runs of each task in a row (0 after a success).",
            per(&|s| Some(s.failing_in_row as f64)),
        ),
    ]
}

// ---- #768: reports ---------------------------------------------------------

/// The notify thread: sends each report and checks each alert when it is due
/// (`options.notify.run`).
fn report_runner(st: &Locked, stop: &AtomicBool) {
    let (reports, alerts) = (&st.opts.reports, &st.opts.alerts);
    let jobs: Vec<(&str, &str)> = reports
        .iter()
        .map(|r| (r.name.as_str(), r.schedule.as_str()))
        .chain(
            alerts
                .iter()
                .map(|a| (a.name.as_str(), a.schedule.as_str())),
        )
        .collect();
    run_on_schedule(st, stop, "report/alert", &jobs, false, |i| {
        if i < reports.len() {
            let _ = record_report(st, &reports[i], false);
        } else {
            let _ = record_alert(st, &alerts[i - reports.len()], false);
        }
    });
}

/// Run one report and record it (run log, `/metrics`): the number of
/// recipients it reached, or why it failed.
fn record_report(st: &Locked, r: &notify::Report, manual: bool) -> Result<usize, String> {
    let started = unix_now();
    let t0 = Instant::now();
    let res = run_report(st, r).map_err(|e| redact(e, st.opts.data.as_deref()));
    {
        let mut m = st.report_stats.lock().unwrap_or_else(|p| p.into_inner());
        let s = m.entry(r.name.clone()).or_default();
        s.runs += 1;
        if res.is_ok() {
            s.last_ok = Some(started as u64);
            s.failing_in_row = 0;
        } else {
            s.failures += 1;
            s.failing_in_row += 1;
        }
    }
    let line = serde_json::json!({
        "report": r.name,
        "start": started as u64,
        "ms": t0.elapsed().as_millis() as u64,
        "ok": res.is_ok(),
        "sent": res.as_ref().ok(),
        "manual": manual,
        "error": res.as_ref().err(),
    })
    .to_string();
    run_log(
        st.opts.notify.as_ref().and_then(|n| n.log.as_deref()),
        "report",
        &line,
    );
    res
}

/// Render a report's files through the export path as its principal (D4),
/// then send them to every recipient. Over a limit nothing is sent.
fn run_report(st: &Locked, r: &notify::Report) -> Result<usize, String> {
    use notify::Format;
    let n = st.opts.notify.as_ref().ok_or("options.notify is not set")?;
    let gen = st.current.get();
    let dash = gen
        .snap
        .dashes
        .find(&r.dashboard)
        .ok_or("no such dashboard")?;
    // Its own dashboard only, and its principal's claims.
    let scope = Scope {
        dashboards: Some(vec![dash.id.clone()]),
        vars: Vec::new(),
    };
    let vars = sv::parse_vars_object(Some(&Value::Object(r.vars.clone())))?;
    let who = ExportAs {
        claims: Some(&r.principal),
        scope: Some(&scope),
        method: Some("report"),
        vars,
    };
    let look = ExportLook {
        tab: r.tab.as_deref(),
        panel: None,
        width: 1200,
        dark: false,
        scale: 2.0,
    };
    let mut files = Vec::new();
    let mut as_of = None;
    let mut info = LogInfo::default();
    let failed = |what: &str, (_, e): (u16, String)| format!("{what}: {e}");
    for f in &r.formats {
        let (name, ctype, bytes) = match f {
            Format::Png | Format::Pdf => {
                let (ext, ctype) = match f {
                    Format::Png => ("png", "image/png"),
                    _ => ("pdf", "application/pdf"),
                };
                let (b, a) =
                    export_dashboard(st, dash, &who, &look, ext).map_err(|e| failed(ext, e))?;
                as_of = as_of.or(a);
                (format!("{}.{ext}", dash.id), ctype, b)
            }
            Format::Csv(p) | Format::Json(p) => {
                let (ext, ctype) = match f {
                    Format::Csv(_) => ("csv", "text/csv; charset=utf-8"),
                    _ => ("json", "application/json"),
                };
                let b = export_panel(st, dash, &who, p, ext, &mut info)
                    .map_err(|e| failed(&format!("{ext}:{p}"), e))?;
                (format!("{}-{p}.{ext}", dash.id), ctype, b)
            }
        };
        files.push(notify::File {
            name,
            content_type: ctype,
            bytes,
        });
    }
    let size: usize = files.iter().map(|f| f.bytes.len()).sum();
    if size > notify::MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "its files are {size} bytes, over the limit of {} — not sent",
            notify::MAX_ATTACHMENT_BYTES
        ));
    }
    let msg = notify::Message {
        kind: "report",
        name: r.name.clone(),
        title: dash.meta.title.clone(),
        status: None,
        detail: None,
        as_of,
        link: n.link.as_ref().map(|l| format!("{l}/d/{}", dash.id)),
        time: std::time::SystemTime::now(),
    };
    deliver_all(st, &r.to, &msg, &files)
}

/// Send `msg` with `files` to `to` (#768): one e-mail (every address in
/// Bcc), one POST per webhook and Teams webhook, one message (with its
/// files) per Slack channel, within `notify.max_per_hour` — over it,
/// nothing is sent. The number of recipients reached, or every failure.
fn deliver_all(
    st: &Locked,
    to: &[notify::Recipient],
    msg: &notify::Message,
    files: &[notify::File],
) -> Result<usize, String> {
    use notify::Recipient;
    let n = st.opts.notify.as_ref().ok_or("options.notify is not set")?;
    let emails: Vec<String> = to
        .iter()
        .filter_map(|t| match t {
            Recipient::Email(a) => Some(a.clone()),
            _ => None,
        })
        .collect();
    let hooks: Vec<(&str, &notify::Webhook)> = to
        .iter()
        .filter_map(|t| match t {
            Recipient::Webhook(w) => n.webhooks.get(w).map(|h| (w.as_str(), h)),
            _ => None,
        })
        .collect();
    let channels: Vec<&str> = to
        .iter()
        .filter_map(|t| match t {
            Recipient::Slack(c) => Some(c.as_str()),
            _ => None,
        })
        .collect();
    let teams: Vec<(&str, &notify::Teams)> = to
        .iter()
        .filter_map(|t| match t {
            Recipient::Teams(w) => n.teams.get(w).map(|h| (w.as_str(), h)),
            _ => None,
        })
        .collect();
    let count = usize::from(!emails.is_empty()) + hooks.len() + channels.len() + teams.len();
    if !st
        .report_rate
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take(count, n.max_per_hour, Instant::now())
    {
        return Err(format!(
            "over options.notify.max_per_hour ({}) — not sent",
            n.max_per_hour
        ));
    }
    let mut sent = 0;
    let mut errors = Vec::new();
    if !emails.is_empty() {
        let smtp = n.smtp.as_ref().ok_or("options.notify.smtp is not set")?;
        let mail = notify::email(&smtp.from, msg, files);
        match notify::send_email(smtp, &emails, &mail) {
            Ok(()) => sent += emails.len(),
            Err(e) => errors.push(format!("e-mail: {e}")),
        }
    }
    if !hooks.is_empty() {
        let body = notify::webhook_body(msg, files);
        for (name, h) in hooks {
            match notify::send_webhook(h, &body, msg.time) {
                Ok(()) => sent += 1,
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
    }
    if !channels.is_empty() {
        let slack = n.slack.as_ref().ok_or("options.notify.slack is not set")?;
        for c in channels {
            match notify::send_slack(slack, c, msg, files) {
                Ok(()) => sent += 1,
                Err(e) => errors.push(e),
            }
        }
    }
    if !teams.is_empty() {
        let card = notify::teams_card(msg, files);
        for (name, t) in teams {
            match notify::send_teams(t, &card) {
                Ok(()) => sent += 1,
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
    }
    if errors.is_empty() {
        Ok(sent)
    } else {
        Err(errors.join("; "))
    }
}

// ---- #768 R2: alerts -------------------------------------------------------

/// What one alert check did.
struct AlertRun {
    /// The message it sent (`None`: nothing changed).
    action: Option<notify::AlertAction>,
    rows: usize,
    sent: usize,
}

/// Check one alert: run its panel as its principal; on a change (D5), send
/// and remember. A failed send keeps the old state, so the next check tries
/// again.
fn run_alert(st: &Locked, a: &notify::Alert) -> Result<AlertRun, String> {
    use notify::Format;
    let n = st.opts.notify.as_ref().ok_or("options.notify is not set")?;
    let gen = st.current.get();
    let dash = gen
        .snap
        .dashes
        .find(&a.dashboard)
        .ok_or("no such dashboard")?;
    let scope = Scope {
        dashboards: Some(vec![dash.id.clone()]),
        vars: Vec::new(),
    };
    let vars = sv::parse_vars_object(Some(&Value::Object(a.vars.clone())))?;
    let who = ExportAs {
        claims: Some(&a.principal),
        scope: Some(&scope),
        method: Some("report"),
        vars,
    };
    let mut info = LogInfo::default();
    let json = export_panel(st, dash, &who, &a.panel, "json", &mut info)
        .map_err(|(_, e)| format!("panel '{}': {e}", a.panel))?;
    let rows = serde_json::from_slice::<Vec<Value>>(&json)
        .map(|r| r.len())
        .unwrap_or(0);
    let hash = (rows > 0).then(|| {
        use sha2::Digest as _;
        sha2::Sha256::digest(&json)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    });
    let now = unix_now() as u64;
    let prev = st
        .alert_states
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&a.name)
        .cloned();
    let Some((action, next)) = notify::decide(prev.as_ref(), hash.as_deref(), now, a.repeat_after)
    else {
        return Ok(AlertRun {
            action: None,
            rows,
            sent: 0,
        });
    };
    let mut files = Vec::new();
    if action != notify::AlertAction::Resolved {
        for f in &a.attach {
            let (ext, ctype, bytes) = match f {
                Format::Json(_) => ("json", "application/json", json.clone()),
                _ => (
                    "csv",
                    "text/csv; charset=utf-8",
                    export_panel(st, dash, &who, &a.panel, "csv", &mut info)
                        .map_err(|(_, e)| format!("panel '{}': {e}", a.panel))?,
                ),
            };
            files.push(notify::File {
                name: format!("{}-{}.{ext}", dash.id, a.panel),
                content_type: ctype,
                bytes,
            });
        }
    }
    let size: usize = files.iter().map(|f| f.bytes.len()).sum();
    if size > notify::MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "its rows are {size} bytes, over the limit of {} — not sent",
            notify::MAX_ATTACHMENT_BYTES
        ));
    }
    let msg = notify::Message {
        kind: "alert",
        name: a.name.clone(),
        title: dash.meta.title.clone(),
        status: Some(action.status()),
        detail: (action != notify::AlertAction::Resolved)
            .then(|| format!("{rows} row(s) in '{}'", a.panel)),
        as_of: None,
        link: n.link.as_ref().map(|l| format!("{l}/d/{}", dash.id)),
        time: std::time::SystemTime::now(),
    };
    let sent = deliver_all(st, &a.to, &msg, &files)?;
    // Sent: remember it (and on disk, so a restart doesn't send it again).
    let mut states = st.alert_states.lock().unwrap_or_else(|p| p.into_inner());
    states.insert(a.name.clone(), next);
    if let Some(path) = &n.state {
        notify::save_states(path, &states)?;
    }
    Ok(AlertRun {
        action: Some(action),
        rows,
        sent,
    })
}

/// Check one alert and record it (run log, `/metrics`).
fn record_alert(st: &Locked, a: &notify::Alert, manual: bool) -> Result<AlertRun, String> {
    let started = unix_now();
    let t0 = Instant::now();
    let res = run_alert(st, a).map_err(|e| redact(e, st.opts.data.as_deref()));
    {
        let mut m = st.alert_stats.lock().unwrap_or_else(|p| p.into_inner());
        let s = m.entry(a.name.clone()).or_default();
        s.runs += 1;
        if res.is_ok() {
            s.last_ok = Some(started as u64);
            s.failing_in_row = 0;
        } else {
            s.failures += 1;
            s.failing_in_row += 1;
        }
    }
    let line = serde_json::json!({
        "alert": a.name,
        "start": started as u64,
        "ms": t0.elapsed().as_millis() as u64,
        "ok": res.is_ok(),
        "rows": res.as_ref().ok().map(|r| r.rows),
        "status": res.as_ref().ok().and_then(|r| r.action.map(|a| a.status())),
        "sent": res.as_ref().ok().map(|r| r.sent),
        "manual": manual,
        "error": res.as_ref().err(),
    })
    .to_string();
    run_log(
        st.opts.notify.as_ref().and_then(|n| n.log.as_deref()),
        "alert",
        &line,
    );
    res
}

/// #767 + R2: a task's failure (the first in a row) or its recovery, to
/// `tasks.on_failure`.
fn task_notify(st: &Locked, to: &[notify::Recipient], t: &tasks::Task, err: Option<&str>) {
    let msg = notify::Message {
        kind: "task",
        name: t.name.clone(),
        title: format!("Task '{}'", t.name),
        status: Some(if err.is_some() { "FAILED" } else { "RECOVERED" }),
        detail: err.map(str::to_string),
        as_of: None,
        link: None,
        time: std::time::SystemTime::now(),
    };
    if let Err(e) = deliver_all(st, to, &msg, &[]) {
        eprintln!(
            "[anofox-serve] task '{}': on_failure: {}",
            t.name,
            access::diag(&e)
        );
    }
}

/// `anofox_serve_report(port, name)`: send report `name` (or check alert
/// `name`) of the locked server on `port` now (for testing; recorded like a
/// scheduled run). Only the session that started the server can call it.
pub fn send_report(port: u16, name: &str) -> Result<String, String> {
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
    if let Some(a) = locked.opts.alerts.iter().find(|a| a.name == name) {
        let r = record_alert(&locked, a, true)
            .map_err(|e| format!("anofox-visualization: alert '{name}': {e}"))?;
        return Ok(match r.action {
            Some(x) => format!(
                "alert '{name}': {} ({} row(s)), sent to {} recipient(s)",
                x.status(),
                r.rows,
                r.sent
            ),
            None => format!("alert '{name}': no change ({} row(s))", r.rows),
        });
    }
    let r = locked
        .opts
        .reports
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| {
            format!("anofox-visualization: no report or alert '{name}' (options.reports, options.alerts)")
        })?;
    let n = record_report(&locked, r, true)
        .map_err(|e| format!("anofox-visualization: report '{name}': {e}"))?;
    Ok(format!("report '{name}' sent to {n} recipient(s)"))
}

/// #768 R2: the alerts' gauges for `/metrics`.
fn alert_gauges(st: &Locked) -> Vec<metrics::Gauge> {
    let m = st.alert_stats.lock().unwrap_or_else(|p| p.into_inner());
    let states = st.alert_states.lock().unwrap_or_else(|p| p.into_inner());
    if st.opts.alerts.is_empty() {
        return Vec::new();
    }
    type Values = Vec<(Vec<(&'static str, String)>, f64)>;
    let per = |f: &dyn Fn(&TaskStat) -> Option<f64>| -> Values {
        m.iter()
            .filter_map(|(k, s)| f(s).map(|v| (vec![("alert", k.clone())], v)))
            .collect()
    };
    let g = |name, help, values| metrics::Gauge { name, help, values };
    vec![
        g(
            "anofox_alert_firing",
            "1 while an alert's panel returns rows (as last sent), else 0.",
            st.opts
                .alerts
                .iter()
                .map(|a| {
                    let f = states.get(&a.name).is_some_and(|s| s.firing);
                    (vec![("alert", a.name.clone())], f64::from(u8::from(f)))
                })
                .collect(),
        ),
        g(
            "anofox_alert_checks",
            "Checks of each alert since the start.",
            per(&|s| Some(s.runs as f64)),
        ),
        g(
            "anofox_alert_check_failures",
            "Failed checks of each alert (its panel failed, or not sent) since the start.",
            per(&|s| Some(s.failures as f64)),
        ),
    ]
}

/// #768: the reports' gauges for `/metrics`.
fn report_gauges(st: &Locked) -> Vec<metrics::Gauge> {
    let m = st.report_stats.lock().unwrap_or_else(|p| p.into_inner());
    if m.is_empty() {
        return Vec::new();
    }
    type Values = Vec<(Vec<(&'static str, String)>, f64)>;
    let per = |f: &dyn Fn(&TaskStat) -> Option<f64>| -> Values {
        m.iter()
            .filter_map(|(k, s)| f(s).map(|v| (vec![("report", k.clone())], v)))
            .collect()
    };
    let g = |name, help, values| metrics::Gauge { name, help, values };
    vec![
        g(
            "anofox_report_last_success_timestamp_seconds",
            "When each report was last sent without an error (Unix seconds).",
            per(&|s| s.last_ok.map(|v| v as f64)),
        ),
        g(
            "anofox_report_runs",
            "Runs of each report since the start.",
            per(&|s| Some(s.runs as f64)),
        ),
        g(
            "anofox_report_failures",
            "Failed runs of each report since the start (not sent, or not to everyone).",
            per(&|s| Some(s.failures as f64)),
        ),
        g(
            "anofox_report_consecutive_failures",
            "Failed runs of each report in a row (0 after a success).",
            per(&|s| Some(s.failing_in_row as f64)),
        ),
    ]
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

/// `POST /api/auth/token` (#771): an embed token for `<anofox-dashboard>`,
/// from an API key with the `embed` scope, capped by the key
/// ([`auth::embed::Embed::mint`]). The host's backend calls this; the
/// browser only ever gets the token.
fn handle_embed_token(
    st: &Locked,
    mut req: Request,
    ident: Option<&auth::Identity>,
    info: &mut LogInfo,
) -> u16 {
    let peer = req.remote_addr().map(|a| a.ip());
    let audit = |outcome: &'static str, reason: Option<&'static str>, detail: Option<&str>| {
        if let Some(a) = st.opts.auth.as_ref().and_then(|a| a.audit.as_ref()) {
            a.write(&auth::audit::Event {
                event: "embed_token",
                outcome,
                method: ident.map(|i| i.method),
                user: ident.map(|i| i.claims.user.as_str()),
                reason,
                peer,
                path: Some("/api/auth/token"),
                key_id: ident.and_then(|i| i.ref_id.as_deref()),
                detail,
                ..Default::default()
            });
        }
    };
    let Some(embed) = st.opts.auth.as_ref().and_then(|a| a.embed.as_ref()) else {
        return reply(
            req,
            404,
            "not found: this server has no embed links (an auth mode \"embed\")",
        );
    };
    let Some(id) = ident.filter(|i| i.method == "api_key" && i.key_scopes.embed) else {
        info.deny = Some("embed_scope");
        audit("deny", Some("embed_scope"), None);
        return reply(
            req,
            403,
            "forbidden: only an API key with the embed scope may mint embed tokens",
        );
    };
    let body = match read_body(&mut req, 16 * 1024) {
        Ok(b) => b,
        Err((code, e)) => return reply(req, code, &e),
    };
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return reply(req, 400, &format!("the body is not JSON: {e}")),
    };
    let key_id = id.ref_id.as_deref().unwrap_or("");
    match embed.mint(&id.claims, &id.scope, key_id, &v) {
        Ok((token, exp)) => {
            let dash = v.get("dashboard").and_then(Value::as_str).unwrap_or("");
            if !st
                .current
                .get()
                .snap
                .dashes
                .list
                .iter()
                .any(|d| d.id == dash)
            {
                return reply(req, 404, "no such dashboard");
            }
            info.dashboard = Some(dash.to_string());
            audit("ok", None, Some(dash));
            let body = serde_json::json!({
                "token": token,
                "expires": exp,
                "url": format!("/d/{dash}?embed={token}"),
            });
            respond(req, 200, "application/json", body.to_string());
            200
        }
        Err(e) => {
            audit("deny", Some("embed_cap"), Some(&e));
            reply(req, 400, &e)
        }
    }
}

/// `options.durability` (#775): `{"run": true, "location":
/// "s3://bucket/prefix", "snapshot_every_s": 300}`.
#[derive(Debug, Clone, PartialEq)]
pub struct DurabilityOptions {
    /// This replica is the one writer: it takes submissions and saves, and
    /// keeps the snapshots. The others answer `503` to writes.
    pub run: bool,
    /// Where the snapshots go: an `s3://`, `gs://` or `r2://` prefix (DuckDB's
    /// httpfs, with the S3 secret the operator created in the serving
    /// session), or a local directory.
    pub location: String,
    pub every: Duration,
}

impl DurabilityOptions {
    fn parse(v: &Value) -> Result<DurabilityOptions, String> {
        let o = v.as_object().ok_or(
            "options.durability must be an object like {\"run\": true, \"location\": \"s3://bucket/prefix\"}",
        )?;
        let mut out = DurabilityOptions {
            run: false,
            location: String::new(),
            every: Duration::from_secs(300),
        };
        for (k, v) in o {
            match k.as_str() {
                "run" => {
                    out.run = v
                        .as_bool()
                        .ok_or("options.durability.run must be true or false")?
                }
                "location" => {
                    let l = v
                        .as_str()
                        .map(str::trim)
                        .unwrap_or("")
                        .trim_end_matches('/');
                    let remote = ["s3://", "gs://", "r2://"]
                        .iter()
                        .any(|p| l.starts_with(p) && l.len() > p.len());
                    let safe = l
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/:._-".contains(&b));
                    if !(safe && (remote || l.starts_with('/'))) {
                        return Err("options.durability.location must be s3://bucket/prefix (also gs://, r2://) or an absolute directory, letters, digits and / : . _ - only".into());
                    }
                    out.location = l.to_string();
                }
                "snapshot_every_s" => {
                    out.every = Duration::from_secs(
                        v.as_u64()
                            .filter(|n| *n > 0)
                            .ok_or(
                                "options.durability.snapshot_every_s must be a positive integer",
                            )?
                            .min(86_400),
                    )
                }
                other => {
                    return Err(format!(
                    "options.durability: unknown key '{other}' (run, location, snapshot_every_s)"
                ))
                }
            }
        }
        if out.location.is_empty() {
            return Err("options.durability.location is required".into());
        }
        Ok(out)
    }

    fn is_local(&self) -> bool {
        self.location.starts_with('/')
    }

    /// The object (or file) one store's snapshot lives at.
    fn object(&self, name: &str) -> String {
        format!("{}/{name}.jsonl", self.location)
    }
}

/// The snapshots' bookkeeping.
#[derive(Default)]
struct DurableState {
    /// Unix seconds of the last snapshot that went through.
    last_ok: AtomicU64,
    /// Snapshots that failed in a row.
    failing: std::sync::atomic::AtomicU32,
    /// Per store: what the last snapshot held (its last seq and length).
    sent: Mutex<HashMap<&'static str, (u64, u64)>>,
}

/// The files `options.durability` keeps: `(name, local path)`.
fn durable_files(opts: &LockedOptions) -> Vec<(&'static str, PathBuf)> {
    let mut v = Vec::new();
    if let Some(s) = &opts.state_opts {
        v.push(("state", s.path.clone()));
    }
    if let Some(w) = &opts.writeback_opts {
        v.push(("writeback", w.path.clone()));
    }
    v
}

/// On a replica that isn't the writer: the `503` for a write.
fn durable_reader(st: &Locked) -> Option<auth::Reply> {
    st.opts.durability.as_ref().filter(|d| !d.run).map(|_| {
        auth::Reply::text(
            503,
            "this replica doesn't take writes (options.durability.run is on another one) — send them to the writer",
        )
        .header("Retry-After", "10")
    })
}

/// The bytes of a text file, through DuckDB, unchanged: one CSV row per file
/// with no quoting, escaping or other delimiter (a JSON Lines file never
/// holds a raw control character), minus the newline the CSV writer adds.
fn copy_text_sql(from: &str, to: &str) -> String {
    format!(
        "COPY (SELECT rtrim(content, chr(10)) FROM read_text({}) WHERE content <> '') TO {} \
         (FORMAT csv, HEADER false, QUOTE '', ESCAPE '', DELIMITER E'\\x01')",
        sv::sql_string_literal(from),
        sv::sql_string_literal(to)
    )
}

/// Restore one store's newest snapshot into `path` (a new node): `None` when
/// there is none yet. The file is written `0600` and renamed into place.
fn durable_restore(
    c: &Conn,
    d: &DurabilityOptions,
    name: &str,
    path: &Path,
) -> Result<Option<u64>, String> {
    let object = d.object(name);
    let found = small_rows(
        c,
        &format!(
            "SELECT count(*) AS n FROM glob({})",
            sv::sql_string_literal(&object)
        ),
    )
    .map_err(|e| format!("options.durability: can't list {object}: {e}"))?
    .first()
    .and_then(|r| r.get("n"))
    .and_then(Value::as_i64)
    .unwrap_or(0);
    if found == 0 {
        return Ok(None);
    }
    let tmp = path.with_extension("restore.tmp");
    c.exec(&copy_text_sql(&object, &tmp.to_string_lossy()))
        .map_err(|e| format!("options.durability: can't restore {object}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    let n = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(n))
}

/// Snapshot every kept store that changed since its last snapshot.
fn durable_snapshot(st: &Locked) {
    let Some(d) = &st.opts.durability else { return };
    let stores: Vec<(&'static str, &Store)> = [
        st.submit.as_ref().map(|s| ("state", &s.store)),
        st.writeback.as_ref().map(|w| ("writeback", &w.store)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut ok = true;
    for (name, store) in stores {
        let res = (|| -> Result<(), String> {
            let dir = SnapshotDir::create()?;
            let tmp = dir.0.join(format!("{name}.jsonl"));
            let seq = store.global_seq();
            let len = store.copy_to(&tmp)?;
            if st
                .durable
                .sent
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(name)
                == Some(&(seq, len))
            {
                return Ok(());
            }
            let c = st.ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
            c.exec(&copy_text_sql(&tmp.to_string_lossy(), &d.object(name)))?;
            drop(c);
            st.durable
                .sent
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(name, (seq, len));
            Ok(())
        })();
        if let Err(e) = res {
            ok = false;
            eprintln!(
                "[anofox-serve] durability: snapshot of {name} to {} failed: {e}",
                d.object(name)
            );
        }
    }
    if ok {
        st.durable
            .last_ok
            .store(unix_now() as u64, Ordering::Relaxed);
        st.durable.failing.store(0, Ordering::Relaxed);
    } else {
        st.durable.failing.fetch_add(1, Ordering::Relaxed);
    }
}

/// `options.durability.run`: a snapshot every `snapshot_every_s`, and a last
/// one at the stop.
fn durability_runner(st: &Locked, stop: &AtomicBool) {
    let Some(d) = &st.opts.durability else { return };
    let mut next = Instant::now() + d.every;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        if Instant::now() >= next {
            durable_snapshot(st);
            next = Instant::now() + d.every;
        }
    }
    durable_snapshot(st);
}

/// `options.preview` (#770).
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewOptions {
    /// `dir`: where each preview is kept as a file, so a restart (or replicas
    /// sharing the volume) keep it; `None`: in memory only.
    pub dir: Option<PathBuf>,
    /// `ttl_s`: how long a preview lives by default (24 h); a request may ask
    /// for less, or more up to `max_ttl_s` (7 days).
    pub ttl: u64,
    pub max_ttl: u64,
    /// `max_per_key`: live previews per API key (20).
    pub max_per_key: usize,
    /// `max_sql_bytes`: one preview's SQL (256 KiB).
    pub max_sql_bytes: usize,
}

impl PreviewOptions {
    fn parse(v: &Value) -> Result<PreviewOptions, String> {
        let o = v.as_object().ok_or(
            "options.preview must be an object like {\"dir\": \"/var/lib/anofox/previews\"}",
        )?;
        let mut out = PreviewOptions {
            dir: None,
            ttl: 86_400,
            max_ttl: 7 * 86_400,
            max_per_key: 20,
            max_sql_bytes: 256 * 1024,
        };
        let num = |k: &str, v: &Value| {
            v.as_u64()
                .filter(|n| *n > 0)
                .ok_or(format!("options.preview.{k} must be a positive integer"))
        };
        for (k, v) in o {
            match k.as_str() {
                "dir" => {
                    out.dir = Some(PathBuf::from(
                        v.as_str()
                            .filter(|d| !d.trim().is_empty())
                            .ok_or("options.preview.dir must be a directory path")?,
                    ))
                }
                "ttl_s" => out.ttl = num(k, v)?,
                "max_ttl_s" => out.max_ttl = num(k, v)?.min(30 * 86_400),
                "max_per_key" => out.max_per_key = num(k, v)?.min(1000) as usize,
                "max_sql_bytes" => out.max_sql_bytes = num(k, v)?.min(4 << 20) as usize,
                other => {
                    return Err(format!(
                        "options.preview: unknown key '{other}' (dir, ttl_s, max_ttl_s, max_per_key, max_sql_bytes)"
                    ))
                }
            }
        }
        out.ttl = out.ttl.min(out.max_ttl);
        Ok(out)
    }
}

/// The id prefix of a preview: `/p/preview-<32 hex>`.
const PREVIEW_PREFIX: &str = "preview-";

/// One stored preview (#770): its dashboard, and the API key it runs as.
struct Preview {
    dash: Arc<Dash>,
    /// The creating key's claims: every viewer of the preview sees what
    /// they allow.
    claims: Claims,
    key_id: String,
    /// Unix seconds.
    expires: u64,
}

/// A preview on one snapshot: its statements rewritten for the access
/// policy, and its deps (like a file dashboard's, built on first use).
struct PreviewOn {
    script: Option<Script>,
    deps: Vec<Deps>,
    deps_tag: String,
    warm_deps: Vec<Deps>,
}

/// The previews, in memory and (with `dir`) one file each.
struct PreviewStore {
    opts: PreviewOptions,
    map: Mutex<HashMap<String, Arc<Preview>>>,
    rate: RateLimiter,
}

impl PreviewStore {
    /// Read the kept previews (dropping expired or broken files).
    fn open(opts: PreviewOptions) -> Result<PreviewStore, String> {
        let mut map = HashMap::new();
        if let Some(dir) = &opts.dir {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("options.preview.dir {}: {e}", dir.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let m = std::fs::metadata(dir)
                    .map_err(|e| format!("options.preview.dir {}: {e}", dir.display()))?;
                if m.permissions().mode() & 0o022 != 0 {
                    return Err(format!(
                        "options.preview.dir {}: writable by group or others — chmod 700 it",
                        dir.display()
                    ));
                }
            }
            let now = unix_now() as u64;
            for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
                let path = e.path();
                let Some(p) = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| preview_from_json(&t))
                else {
                    continue;
                };
                if p.expires <= now
                    || path.file_stem().and_then(|s| s.to_str()) != Some(p.dash.id.as_str())
                {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                map.insert(p.dash.id.clone(), Arc::new(p));
            }
        }
        Ok(PreviewStore {
            opts,
            map: Mutex::new(map),
            rate: RateLimiter::new(30, 10, 4096),
        })
    }

    fn file(&self, id: &str) -> Option<PathBuf> {
        self.opts.dir.as_ref().map(|d| d.join(format!("{id}.json")))
    }

    /// Drop the expired previews (and their files).
    fn purge(&self, m: &mut HashMap<String, Arc<Preview>>) {
        let now = unix_now() as u64;
        let gone: Vec<String> = m
            .iter()
            .filter(|(_, p)| p.expires <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            m.remove(&id);
            if let Some(f) = self.file(&id) {
                let _ = std::fs::remove_file(f);
            }
        }
    }

    fn get(&self, id: &str) -> Option<Arc<Preview>> {
        if !id.starts_with(PREVIEW_PREFIX) {
            return None;
        }
        let mut m = self.map.lock().unwrap_or_else(|p| p.into_inner());
        self.purge(&mut m);
        m.get(id).cloned()
    }

    /// Add a preview unless its key already has `max_per_key` live ones.
    fn insert(&self, p: Preview) -> Result<Arc<Preview>, String> {
        let mut m = self.map.lock().unwrap_or_else(|p| p.into_inner());
        self.purge(&mut m);
        if m.values().filter(|x| x.key_id == p.key_id).count() >= self.opts.max_per_key {
            return Err(format!(
                "this API key already has {} live previews (options.preview.max_per_key) — delete one",
                self.opts.max_per_key
            ));
        }
        let p = Arc::new(p);
        m.insert(p.dash.id.clone(), p.clone());
        Ok(p)
    }

    /// Keep it on disk (after its checks passed).
    fn persist(&self, p: &Preview) -> Result<(), String> {
        let Some(f) = self.file(&p.dash.id) else {
            return Ok(());
        };
        let tmp = f.with_extension("json.tmp");
        write_private(&tmp, preview_to_json(p).as_bytes())?;
        std::fs::rename(&tmp, &f).map_err(|e| format!("{}: {e}", f.display()))
    }

    fn remove(&self, id: &str) -> Option<Arc<Preview>> {
        let p = self
            .map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        if let Some(f) = self.file(id) {
            let _ = std::fs::remove_file(f);
        }
        p
    }
}

/// Write a file only its owner may read (`0600`).
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn preview_to_json(p: &Preview) -> String {
    serde_json::json!({
        "id": p.dash.id,
        "sql": p.dash.sql,
        "user": p.claims.user,
        "groups": p.claims.groups,
        "claims": p.claims.attrs.iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect::<serde_json::Map<_, _>>(),
        "key": p.key_id,
        "expires": p.expires,
    })
    .to_string()
}

fn preview_from_json(text: &str) -> Option<Preview> {
    let v: Value = serde_json::from_str(text).ok()?;
    let id = v.get("id")?.as_str()?.to_string();
    if !id.starts_with(PREVIEW_PREFIX) || !sv::valid_dashboard_id(&id) {
        return None;
    }
    let sql = v.get("sql")?.as_str()?.to_string();
    let claims = Claims {
        user: v.get("user")?.as_str()?.to_string(),
        groups: v
            .get("groups")?
            .as_array()?
            .iter()
            .filter_map(|g| g.as_str().map(str::to_string))
            .collect(),
        attrs: v
            .get("claims")?
            .as_object()?
            .iter()
            .map(|(k, x)| (k.clone(), x.as_str().map(str::to_string)))
            .collect(),
    };
    let meta = sv::parse_dashboard_meta(&id, &sql);
    Some(Preview {
        dash: Arc::new(plan_dash(id, sql, meta, None)),
        claims,
        key_id: v.get("key")?.as_str()?.to_string(),
        expires: v.get("expires")?.as_u64()?,
    })
}

/// A preview's policy rewrite and deps on `gen` (built once per snapshot).
fn preview_on(
    st: &Locked,
    gen: &Generation<Snapshot>,
    p: &Preview,
) -> Result<Arc<PreviewOn>, String> {
    if let Some(x) = gen
        .snap
        .previews
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&p.dash.id)
    {
        return Ok(x.clone());
    }
    let script = match (&gen.snap.policy, &st.opts.policy_key) {
        (Some(pol), Some(key)) => {
            let c = gen.snap.db.connect()?;
            crate::register_policy_hash(&c, key.0.clone())?;
            let mut duck = |sql: &str| c.query(sql)?.string_rows(1_000_000);
            Some(
                pol.enf
                    .rewrite_script(&mut duck, &p.dash.stmts, &[])
                    .map_err(|(i, e)| {
                        format!("statement {i}: {e} (refused by the access policy)")
                    })?,
            )
        }
        _ => None,
    };
    let (deps, deps_tag, warm_deps) = dash_deps(&p.dash, script.as_ref(), &gen.snap.implicit);
    let on = Arc::new(PreviewOn {
        script,
        deps,
        deps_tag,
        warm_deps,
    });
    gen.snap
        .previews
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(p.dash.id.clone(), on.clone());
    Ok(on)
}

/// A dashboard as one request sees it on its snapshot: a file's (by index)
/// or a preview's (#770).
enum DashHeld {
    File(usize),
    Preview(Arc<Preview>, Arc<PreviewOn>),
}

impl DashHeld {
    fn resolve(
        st: &Locked,
        gen: &Generation<Snapshot>,
        id: &str,
    ) -> Result<DashHeld, (u16, String)> {
        if let Some(di) = gen.snap.dashes.list.iter().position(|d| d.id == id) {
            return Ok(DashHeld::File(di));
        }
        let p = st
            .previews
            .as_ref()
            .and_then(|ps| ps.get(id))
            .ok_or((404, "no such dashboard".to_string()))?;
        let on = preview_on(st, gen, &p).map_err(|e| (400, e))?;
        Ok(DashHeld::Preview(p, on))
    }

    fn dash<'a>(&'a self, gen: &'a Generation<Snapshot>) -> &'a Dash {
        match self {
            DashHeld::File(di) => &gen.snap.dashes.list[*di],
            DashHeld::Preview(p, _) => &p.dash,
        }
    }

    fn script<'a>(&'a self, gen: &'a Generation<Snapshot>) -> Option<&'a Script> {
        match self {
            DashHeld::File(di) => gen.snap.policy.as_ref().map(|p| &p.scripts[*di]),
            DashHeld::Preview(_, on) => on.script.as_ref(),
        }
    }

    fn deps<'a>(&'a self, gen: &'a Generation<Snapshot>) -> Option<&'a Vec<Deps>> {
        match self {
            DashHeld::File(di) => gen.snap.deps.get(*di),
            DashHeld::Preview(_, on) => Some(&on.deps),
        }
    }

    fn deps_tag(&self, gen: &Generation<Snapshot>) -> String {
        match self {
            DashHeld::File(di) => gen.snap.deps_tags.get(*di).cloned().unwrap_or_default(),
            DashHeld::Preview(_, on) => on.deps_tag.clone(),
        }
    }

    fn warm_deps<'a>(&'a self, gen: &'a Generation<Snapshot>) -> Option<&'a Vec<Deps>> {
        match self {
            DashHeld::File(di) => gen.snap.warm_deps.get(*di),
            DashHeld::Preview(_, on) => Some(&on.warm_deps),
        }
    }

    fn preview(&self) -> Option<&Preview> {
        match self {
            DashHeld::File(_) => None,
            DashHeld::Preview(p, _) => Some(p),
        }
    }
}

/// A data provider that records every panel that fails (a preview's check).
struct Recording<'a> {
    inner: ExportData<'a>,
    errors: Vec<String>,
}

impl dashboard::DataProvider for Recording<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        self.inner.execute(sql)
    }
    fn query(
        &mut self,
        sql: &str,
    ) -> Result<Vec<(String, Vec<anofox_visualization::Value>)>, String> {
        let r = self.inner.query(sql);
        if let Err(e) = &r {
            if self.errors.len() < 20 {
                let first = sql
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>();
                self.errors.push(format!("{first}: {e}"));
            }
        }
        r
    }
}

/// `POST /api/dashboards` (#770): `{"sql": "…", "ttl_s"?: n}` from an API key
/// with the `preview` scope → `201 {"id", "url", "expires"}`. The dashboard
/// is checked like a file at start, rewritten for the access policy, and
/// every panel is run once as the key; any failure is a `400`.
fn handle_preview_create(
    st: &Locked,
    mut req: Request,
    ident: Option<&auth::Identity>,
    info: &mut LogInfo,
) -> u16 {
    let peer = req.remote_addr().map(|a| a.ip());
    let audit = |outcome: &'static str, reason: Option<&'static str>, detail: Option<&str>| {
        if let Some(a) = st.opts.auth.as_ref().and_then(|a| a.audit.as_ref()) {
            a.write(&auth::audit::Event {
                event: "preview",
                outcome,
                method: ident.map(|i| i.method),
                user: ident.map(|i| i.claims.user.as_str()),
                reason,
                peer,
                path: Some("/api/dashboards"),
                key_id: ident.and_then(|i| i.ref_id.as_deref()),
                detail,
                ..Default::default()
            });
        }
    };
    let Some(store) = &st.previews else {
        return reply(
            req,
            404,
            "not found: this server has no previews (options.preview)",
        );
    };
    let Some(id_key) = ident.filter(|i| i.method == "api_key" && i.key_scopes.preview) else {
        info.deny = Some("preview_scope");
        audit("deny", Some("preview_scope"), None);
        return reply(
            req,
            403,
            "forbidden: only an API key with the preview scope may create a preview",
        );
    };
    let key_id = id_key
        .ref_id
        .clone()
        .unwrap_or_else(|| id_key.claims.user.clone());
    if let Err(wait) = store.rate.check(&key_id, Instant::now()) {
        info.deny = Some("preview_rate");
        return send(
            req,
            auth::Reply::text(429, "too many previews for this API key")
                .header("Retry-After", &wait.max(1).to_string()),
        );
    }
    let body = match read_body(&mut req, store.opts.max_sql_bytes + 4096) {
        Ok(b) => b,
        Err((code, e)) => return reply(req, code, &e),
    };
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return reply(req, 400, &format!("the body is not JSON: {e}")),
    };
    let Some(sql) = v
        .get("sql")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return reply(
            req,
            400,
            "the body must be {\"sql\": \"<the dashboard's SQL>\"}",
        );
    };
    if sql.len() > store.opts.max_sql_bytes {
        return reply(
            req,
            413,
            &format!("the SQL exceeds {} bytes", store.opts.max_sql_bytes),
        );
    }
    if let Some(k) = v
        .as_object()
        .and_then(|o| o.keys().find(|k| !matches!(k.as_str(), "sql" | "ttl_s")))
    {
        return reply(req, 400, &format!("unknown key '{k}' (sql, ttl_s)"));
    }
    let ttl = match v.get("ttl_s") {
        None => store.opts.ttl,
        Some(t) => match t.as_u64().filter(|t| *t > 0) {
            Some(t) => t.min(store.opts.max_ttl),
            None => return reply(req, 400, "ttl_s must be a positive number of seconds"),
        },
    };
    let id = format!("{PREVIEW_PREFIX}{}", sv::random_token());
    let meta = sv::parse_dashboard_meta(&id, sql);
    let dash = plan_dash(id.clone(), sql.to_string(), meta, None);
    // What a preview may not do (D5): save, submit, load extensions, or
    // read the saved tables.
    let refusal = if !dash.saves.is_empty() || !dash.submits.is_empty() {
        Some("::SAVE and ::SUBMIT are not available in a preview")
    } else if !dash.meta.loads.is_empty() {
        Some("-- @load is not available in a preview (the server loads extensions at start)")
    } else if st
        .opts
        .writeback_opts
        .as_ref()
        .is_some_and(|w| w.tables.keys().any(|t| wb::references(&dash.sql, t)))
    {
        Some("a preview may not read the saved (write-back) tables")
    } else if dash.stmts.iter().all(|(setup, _)| *setup) {
        Some("no panels: the SQL has no annotated SELECT")
    } else {
        None
    };
    if let Some(r) = refusal {
        return reply(req, 400, r);
    }
    // One statement each, and the data folder's read-only rule: as a file.
    {
        let gen = st.current.get();
        let check = || -> Result<(), String> {
            let c = gen.snap.db.connect()?;
            for (i, (_, s)) in dash.stmts.iter().enumerate() {
                match c.count_statements(s) {
                    Ok(1) => {}
                    Ok(n) => return Err(format!("statement {i}: expected 1 statement, found {n}")),
                    Err(e) => return Err(format!("statement {i}: {e}")),
                }
            }
            if st.opts.data.is_some() {
                check_read_only(&c, std::slice::from_ref(&dash))?;
            }
            Ok(())
        };
        if let Err(e) = check() {
            return reply(req, 400, &redact(e, st.opts.data.as_deref()));
        }
    }
    let expires = unix_now() as u64 + ttl;
    let pv = match store.insert(Preview {
        dash: Arc::new(dash),
        claims: id_key.claims.clone(),
        key_id: key_id.clone(),
        expires,
    }) {
        Ok(p) => p,
        Err(e) => return reply(req, 409, &e),
    };
    // The policy rewrite, then every panel once, as the key.
    let gen = st.current.get();
    if let Err(e) = preview_on(st, &gen, &pv) {
        store.remove(&id);
        return reply(req, 400, &e);
    }
    let who = ExportAs {
        claims: Some(&pv.claims),
        scope: None,
        method: Some("api_key"),
        vars: Vec::new(),
    };
    let mut rec = Recording {
        inner: who.data(st, &pv.dash),
        errors: Vec::new(),
    };
    let rendered = dashboard::render_dashboard_svg(
        &pv.dash.sql,
        &mut rec,
        &dashboard::DashboardOptions::default(),
    );
    let mut errors = rec.errors;
    if let Err(e) = rendered {
        errors.push(e);
    }
    if let Some((_, e)) = rec.inner.fatal.take() {
        errors.push(e);
    }
    if !errors.is_empty() {
        store.remove(&id);
        audit("error", Some("preview_check"), None);
        let body = serde_json::json!({"error": "the preview's checks failed", "errors": errors});
        respond(req, 400, "application/json", body.to_string());
        return 400;
    }
    if let Err(e) = store.persist(&pv) {
        store.remove(&id);
        return reply(req, 500, &format!("could not keep the preview: {e}"));
    }
    info.dashboard = Some(id.clone());
    audit("ok", None, Some(&id));
    let body = serde_json::json!({"id": id, "url": format!("/p/{id}"), "expires": expires});
    respond(req, 201, "application/json", body.to_string());
    201
}

/// `DELETE /api/dashboards/<id>` (#770): the key that made a preview removes it.
fn handle_preview_delete(
    st: &Locked,
    req: Request,
    id: &str,
    ident: Option<&auth::Identity>,
    info: &mut LogInfo,
) -> u16 {
    let Some(store) = &st.previews else {
        return reply(
            req,
            404,
            "not found: this server has no previews (options.preview)",
        );
    };
    let key = ident
        .filter(|i| i.method == "api_key" && i.key_scopes.preview)
        .map(|i| i.ref_id.clone().unwrap_or_else(|| i.claims.user.clone()));
    match (store.get(id), key) {
        (Some(p), Some(k)) if p.key_id == k => {
            store.remove(id);
            info.dashboard = Some(id.to_string());
            respond(req, 204, "text/plain", Vec::new());
            204
        }
        (Some(_), Some(_)) => reply(
            req,
            403,
            "forbidden: only the key that made a preview may delete it",
        ),
        (_, None) => reply(
            req,
            403,
            "forbidden: only an API key with the preview scope may delete a preview",
        ),
        (None, _) => reply(req, 404, "no such preview (or it expired)"),
    }
}

/// `options.ingest`'s live runtime (#769).
struct IngestRuntime {
    /// Requests per minute, per API key.
    rate: RateLimiter,
    /// One write at a time (the table checks and the insert are one step).
    write: Mutex<()>,
    /// A write asked for a refresh (`options.ingest.refresh`).
    want_refresh: (Mutex<bool>, Condvar),
}

/// At start, on the writing replica: the ingest database is attached and
/// not `READ_ONLY`.
fn check_ingest_database(c: &Conn, db: &str) -> Result<(), String> {
    let rows = small_rows(
        c,
        &format!(
            "SELECT readonly AS r FROM duckdb_databases() WHERE database_name = {}",
            sv::sql_string_literal(db)
        ),
    )?;
    match rows.first().and_then(|r| r.get("r")).and_then(Value::as_bool) {
        None => Err(format!(
            "options.ingest.database: no database '{db}' is attached (ATTACH it read-write before starting the server)"
        )),
        Some(true) => Err(format!(
            "options.ingest.database: '{db}' is attached READ_ONLY — the ingest API writes into it"
        )),
        Some(false) => Ok(()),
    }
}

/// `POST /api/data/<table>` (#769): append a JSON object or array of objects
/// to `<options.ingest.database>.main.<table>`, from an API key whose scopes
/// name the table. The rows show at the next snapshot refresh.
fn handle_ingest(
    st: &Locked,
    mut req: Request,
    table: &str,
    ident: Option<&auth::Identity>,
    info: &mut LogInfo,
) -> u16 {
    let peer = req.remote_addr().map(|a| a.ip());
    let audit = |outcome: &'static str, reason: Option<&'static str>, detail: Option<&str>| {
        if let Some(a) = st.opts.auth.as_ref().and_then(|a| a.audit.as_ref()) {
            a.write(&auth::audit::Event {
                event: "ingest",
                outcome,
                method: ident.map(|i| i.method),
                user: ident.map(|i| i.claims.user.as_str()),
                reason,
                peer,
                path: Some("/api/data"),
                key_id: ident.and_then(|i| i.ref_id.as_deref()),
                detail,
                ..Default::default()
            });
        }
    };
    let (Some(ing), Some(rt)) = (&st.opts.ingest, &st.ingest) else {
        return reply(
            req,
            404,
            "not found: this server has no ingest API (options.ingest)",
        );
    };
    if !ingest::valid_name(table) {
        return reply(
            req,
            400,
            "the table must be a plain name (letters, digits and _)",
        );
    }
    // Only an API key whose scopes name this table.
    let Some(id) = ident.filter(|i| i.method == "api_key" && i.key_scopes.may_ingest(table)) else {
        info.deny = Some("ingest_scope");
        audit("deny", Some("ingest_scope"), Some(table));
        return reply(
            req,
            403,
            &format!("forbidden: only an API key with the scope ingest:{table} may write here"),
        );
    };
    if !ing.run {
        return send(
            req,
            auth::Reply::text(
                503,
                "this replica doesn't take ingest writes (options.ingest.run) — send them to the writer",
            )
            .header("Retry-After", "10"),
        );
    }
    let key = id.ref_id.as_deref().unwrap_or(&id.claims.user);
    if let Err(wait) = rt.rate.check(key, Instant::now()) {
        info.deny = Some("ingest_rate");
        audit("deny", Some("ingest_rate"), Some(table));
        return send(
            req,
            auth::Reply::text(429, "too many requests for this API key")
                .header("Retry-After", &wait.max(1).to_string()),
        );
    }
    let body = match read_body(&mut req, ing.max_body_bytes) {
        Ok(b) => b,
        Err((code, e)) => return reply(req, code, &e),
    };
    let fixed = ing.fixed(table);
    let batch = match ingest::parse_batch(&body, ing.max_rows, fixed) {
        Ok(b) => b,
        Err(e) => return reply(req, 400, &e),
    };
    match ingest_write(st, ing, rt, table, &batch, fixed) {
        Ok((created, added)) => {
            let detail = format!("{table}: {} row(s)", batch.rows.len());
            audit("ok", None, Some(&detail));
            if ing.refresh {
                let (m, cv) = &rt.want_refresh;
                *m.lock().unwrap_or_else(|p| p.into_inner()) = true;
                cv.notify_one();
            }
            let body = serde_json::json!({
                "table": table,
                "rows": batch.rows.len(),
                "created": created,
                "added_columns": added,
            });
            respond(req, 200, "application/json", body.to_string());
            200
        }
        Err((code, e)) => {
            audit("error", Some("ingest_write"), Some(table));
            reply(req, code, &e)
        }
    }
}

/// Write one batch: create the table, or add its new columns, then insert,
/// in one transaction on the serving session (rolled back on any error).
/// `(created, added columns)`, or `(status, message)`.
fn ingest_write(
    st: &Locked,
    ing: &IngestOptions,
    rt: &IngestRuntime,
    table: &str,
    batch: &ingest::Batch,
    fixed: Option<&[(String, String)]>,
) -> Result<(bool, Vec<String>), (u16, String)> {
    let _one = rt.write.lock().unwrap_or_else(|p| p.into_inner());
    let dir = SnapshotDir::create().map_err(|e| (500, e))?;
    let file = dir.0.join("rows.ndjson");
    std::fs::write(&file, ingest::ndjson(batch))
        .map_err(|e| (500, format!("could not stage the rows: {e}")))?;
    let file_lit = sv::sql_string_literal(&file.to_string_lossy());
    let target = format!(
        "{}.main.{}",
        sv::quote_ident(&ing.database),
        sv::quote_ident(table)
    );
    let read = match fixed {
        Some(cols) => format!(
            "read_json({file_lit}, format = 'newline_delimited', columns = {{{}}})",
            cols.iter()
                .map(|(c, t)| format!(
                    "{}: {}",
                    sv::sql_string_literal(c),
                    sv::sql_string_literal(t)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => format!("read_json({file_lit}, format = 'newline_delimited')"),
    };
    let c = st.ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
    let clean = |e: String| e.replace(&file.to_string_lossy().into_owned(), "<request body>");
    let (res, fired) = c.with_deadline_fired(st.opts.timeout.max(Duration::from_secs(30)), |c| {
        c.exec("BEGIN")?;
        let exists = small_rows(
            c,
            &format!(
                "SELECT count(*) AS n FROM duckdb_tables() WHERE database_name = {} AND schema_name = 'main' AND lower(table_name) = lower({})",
                sv::sql_string_literal(&ing.database),
                sv::sql_string_literal(table)
            ),
        )?
        .first()
        .and_then(|r| r.get("n"))
        .and_then(Value::as_i64)
            == Some(1);
        let exclude = |cols: &[String]| {
            if cols.is_empty() {
                String::new()
            } else {
                format!(
                    " EXCLUDE ({})",
                    cols.iter().map(|c| sv::quote_ident(c)).collect::<Vec<_>>().join(", ")
                )
            }
        };
        let system = "gen_random_uuid() AS _id, now() AS _ts";
        if !exists {
            match fixed {
                Some(cols) => {
                    let defs: Vec<String> = cols
                        .iter()
                        .map(|(c, t)| format!("{} {t}", sv::quote_ident(c)))
                        .collect();
                    c.exec(&format!(
                        "CREATE TABLE {target} (_id UUID, _ts TIMESTAMP WITH TIME ZONE, {})",
                        defs.join(", ")
                    ))?;
                    c.exec(&format!("INSERT INTO {target} BY NAME SELECT {system}, * FROM {read}"))?;
                }
                None => {
                    let leave_out: Vec<String> = batch.all_null.iter().cloned().collect();
                    c.exec(&format!(
                        "CREATE TABLE {target} AS SELECT {system}, *{} FROM {read}",
                        exclude(&leave_out)
                    ))?;
                }
            }
            c.exec("COMMIT")?;
            return Ok(Ok((true, Vec::new())));
        }
        let mut leave_out = Vec::new();
        let mut added = Vec::new();
        let col_rows = |sql: String| -> Result<Vec<(String, String)>, String> {
            Ok(small_rows(c, &sql)?
                .iter()
                .map(|r| {
                    let g = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    (g("n"), g("t"))
                })
                .collect())
        };
        let existing = col_rows(format!(
            "SELECT column_name AS n, data_type AS t FROM duckdb_columns() WHERE database_name = {} AND schema_name = 'main' AND lower(table_name) = lower({}) ORDER BY column_index",
            sv::sql_string_literal(&ing.database),
            sv::sql_string_literal(table)
        ))?;
        // A table made before (by the operator) gets the server's columns.
        for (col, ty) in [("_id", "UUID"), ("_ts", "TIMESTAMP WITH TIME ZONE")] {
            if !existing.iter().any(|(e, _)| e.eq_ignore_ascii_case(col)) {
                c.exec(&format!("ALTER TABLE {target} ADD COLUMN {col} {ty}"))?;
            }
        }
        if fixed.is_none() {
            let incoming = col_rows(format!(
                "SELECT column_name AS n, column_type AS t FROM (DESCRIBE SELECT * FROM {read})"
            ))?;
            let (add, out) = match ingest::plan_columns(&existing, &incoming, &batch.all_null) {
                Ok(p) => p,
                Err(e) => return Ok(Err((400, e))),
            };
            for (col, ty) in &add {
                c.exec(&format!("ALTER TABLE {target} ADD COLUMN {} {ty}", sv::quote_ident(col)))?;
                added.push(col.clone());
            }
            leave_out = out;
        }
        c.exec(&format!(
            "INSERT INTO {target} BY NAME SELECT {system}, *{} FROM {read}",
            exclude(&leave_out)
        ))?;
        c.exec("COMMIT")?;
        Ok(Ok((false, added)))
    });
    // Nothing stays open on the shared connection (ROLLBACK succeeds only
    // when a transaction is).
    let rolled_back = c.exec("ROLLBACK").is_ok();
    drop(c);
    if fired {
        return Err((503, "the write timed out; nothing was written".into()));
    }
    match res {
        Ok(Ok(ok)) if !rolled_back => Ok(ok),
        Ok(Ok(_)) => Err((500, "the write left a transaction open; rolled back".into())),
        Ok(Err(e)) => Err(e),
        // DuckDB refused it (a value that doesn't convert, an overflow, …).
        Err(e) => Err((400, format!("not written: {}", clean(e)))),
    }
}

/// `options.ingest.refresh` (#769): after a write, rebuild the snapshot — at
/// most every [`ingest::REFRESH_EVERY_S`] seconds, so a stream of small
/// writes doesn't rebuild it per request.
fn ingest_refresher(st: &Locked, stop: &AtomicBool) {
    let Some(rt) = st.ingest.as_ref() else { return };
    let (m, cv) = &rt.want_refresh;
    let every = Duration::from_secs(ingest::REFRESH_EVERY_S);
    let mut last: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let mut want = m.lock().unwrap_or_else(|p| p.into_inner());
        if !*want {
            want = cv
                .wait_timeout(want, Duration::from_secs(1))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        if !*want {
            continue;
        }
        drop(want);
        // Wait out the rest of the interval (writes meanwhile join this one).
        while let Some(l) = last.filter(|l| l.elapsed() < every) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep((every - l.elapsed()).min(Duration::from_secs(1)));
        }
        *m.lock().unwrap_or_else(|p| p.into_inner()) = false;
        if let Err(e) = st.refresh(true) {
            eprintln!("[anofox-serve] ingest refresh: {e}");
        }
        last = Some(Instant::now());
    }
}

/// `anofox_serve_reload(port)` (#772): see [`Locked::reload`].
pub fn reload(port: u16) -> Result<String, String> {
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
    locked.reload()
}

// #417: `too_broad` moved to `anofox_visualization::host::serving` (`sv`) so
// `dashboard --site --data` shares the exact same rule.
use sv::too_broad;

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

// #417: `read_only_statement`, `valid_path_value[_text]`, `wrap_nav_picker_sql`
// and `nav_picker_rows_to_options` moved to `anofox_visualization::host::serving`
// (`sv`) so `dashboard --site`'s own pre-publish check and build-time
// `nav_picker` share the exact same rules instead of a second, drifting copy.
use sv::{
    daterange_bounds_from_rows, daterange_preset_bounds, format_iso_date,
    nav_picker_rows_to_options, parse_iso_date, read_only_statement, valid_path_value,
    wrap_nav_picker_sql, NAV_PICKER_MAX_OPTIONS,
};

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

/// Does `scope` (an embed link's or API key's credential) fix variable
/// `name` (`Scope::lock_vars`, case-insensitive — DuckDB names are)? Used by
/// `served_index` (security review of #410, M1) to decide whether a
/// `path_vars` name is worth carrying on a nav link, and whether
/// `nav_picker` may be shown at all: a locked variable's viewer-facing value
/// never changes no matter what the page sends, so carrying it is pointless,
/// and showing every `nav_picker` option would disclose values (e.g. every
/// tenant's model) this credential's viewer can never actually choose.
fn var_locked(scope: Option<&Scope>, name: &str) -> bool {
    scope.is_some_and(|s| s.vars.iter().any(|(l, _)| l.eq_ignore_ascii_case(name)))
}

/// A preset key ([`sv::DATERANGE_PRESET_NAMES`]) to the short label the nav
/// bar shows on its button — `renderNavPickers` (`web/app.js`) never needs
/// its own copy of this mapping.
fn daterange_preset_label(key: &str) -> &'static str {
    match key {
        "last 7d" => "Last 7 days",
        "last 30d" => "Last 30 days",
        "last 90d" => "Last 90 days",
        "all" => "All",
        _ => "",
    }
}

/// `np`/`snap` (a parallel `opts.nav_pickers`/snapshot `nav_picker_snapshots`
/// pair, already filtered for a locked variable by the caller) to the JSON
/// one entry of `window.__served.navPickers` gets. Dates are ISO text
/// (`format_iso_date`); a `daterange` with no `sql` sends `min`/`max` as
/// `null` (two plain, unbounded date inputs) and an empty `presets` list.
fn nav_picker_json(np: &NavPicker, snap: &PickerSnapshot) -> serde_json::Value {
    match (np, snap) {
        (NavPicker::Dropdown { var, label, .. }, PickerSnapshot::Dropdown(options)) => {
            serde_json::json!({
                "type": "dropdown",
                "var": var,
                "label": label,
                "options": options.iter()
                    .map(|(v, l)| serde_json::json!({"value": v, "label": l}))
                    .collect::<Vec<_>>(),
            })
        }
        (
            NavPicker::DateRange { vars, label, .. },
            PickerSnapshot::DateRange { min, max, presets },
        ) => {
            serde_json::json!({
                "type": "daterange",
                "vars": vars,
                "label": label,
                "min": min.map(format_iso_date),
                "max": max.map(format_iso_date),
                "presets": presets.iter().map(|(key, from, to)| serde_json::json!({
                    "key": key,
                    "label": daterange_preset_label(key),
                    "from": format_iso_date(*from),
                    "to": format_iso_date(*to),
                })).collect::<Vec<_>>(),
            })
        }
        // `opts.nav_pickers` and `snap.nav_picker_snapshots` are built
        // index-for-index by `build_nav_picker` (same match on `np`), so
        // the kinds always agree; this arm is unreachable in practice.
        _ => serde_json::Value::Null,
    }
}

/// One `options.nav_pickers[]` entry's computed state for a snapshot — a
/// dropdown's options, or a daterange's bounds and preset windows (day
/// counts, [`parse_iso_date`]/[`format_iso_date`]). `None` bounds: a
/// daterange picker with no `sql` (two plain, unbounded date inputs).
#[derive(Debug)]
enum PickerSnapshot {
    Dropdown(Vec<(String, String)>),
    DateRange {
        min: Option<i64>,
        max: Option<i64>,
        presets: Vec<(String, i64, i64)>,
    },
}

/// Run and validate one picker's `sql` against the snapshot just built on
/// `c` (startup, or every refresh): classified read-only by the same check
/// every `data`-folder statement gets ([`read_only_statement`]), then
/// actually executed — once per snapshot, with `anofox_data` set and no
/// viewer context at all (no claims, no viewer variables): the result is
/// the same for every viewer and is never re-run per request.
///
/// A dropdown's rows must have exactly two columns (value, label); a value
/// that is not a plain `path_vars` name — the same rule
/// [`check_path_values`] enforces on every request — is dropped and
/// logged, never sent to a page ([`nav_picker_rows_to_options`]). A
/// daterange's row must be exactly one row of two columns, `(min, max)`,
/// both real calendar dates with `min <= max`
/// ([`daterange_bounds_from_rows`]); its `presets` are then computed from
/// `max` ([`daterange_preset_bounds`]) — refusing the whole snapshot build
/// if a preset computes outside `[min, max]` can't happen: the function
/// clamps, never refuses, by construction.
fn build_nav_picker(
    c: &Conn,
    opts: &LockedOptions,
    np: &NavPicker,
) -> Result<PickerSnapshot, String> {
    let checked_rows = |sql: &str, max_rows: usize| -> Result<Vec<Vec<Option<String>>>, String> {
        let mut duck = |q: &str| c.query(q)?.string_rows(1_000_000);
        read_only_statement(sql, false, &mut duck)
            .map_err(|e| format!("options.nav_picker(s).sql: {e}"))?;
        if let Some(data) = &opts.data {
            c.exec(&format!(
                "SET VARIABLE {DATA_VAR} = {}",
                sql_string_literal(&data.to_string_lossy())
            ))?;
        }
        c.query(&wrap_nav_picker_sql(sql))
            .and_then(|mut r| r.string_rows(max_rows))
            .map_err(|e| format!("options.nav_picker(s).sql: {e}"))
    };
    match np {
        NavPicker::Dropdown { sql, .. } => {
            let rows = checked_rows(sql, NAV_PICKER_MAX_OPTIONS + 1)?;
            Ok(PickerSnapshot::Dropdown(nav_picker_rows_to_options(&rows)?))
        }
        NavPicker::DateRange { sql: None, .. } => Ok(PickerSnapshot::DateRange {
            min: None,
            max: None,
            presets: Vec::new(),
        }),
        NavPicker::DateRange {
            sql: Some(sql),
            presets,
            ..
        } => {
            // One extra row over the expected 1, same "fetch one more than
            // the limit so the real check below can see the overflow"
            // pattern as the dropdown branch above.
            let rows = checked_rows(sql, 2)?;
            let (min, max) = daterange_bounds_from_rows(&rows)?;
            let preset_bounds = presets
                .iter()
                .map(|p| {
                    daterange_preset_bounds(p, min, max)
                        .map(|(from, to)| (p.clone(), from, to))
                        .ok_or_else(|| {
                            format!("options.nav_picker(s) (daterange): unknown preset '{p}'")
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PickerSnapshot::DateRange {
                min: Some(min),
                max: Some(max),
                presets: preset_bounds,
            })
        }
    }
}

/// Load `*.sql` dashboards from `dir`. Returns them plus the union of their
/// `-- @load` extensions.
/// The dashboards of `dir` (and, with `folders`, its subfolders — #772),
/// planned for serving: [`sv::dashboard_files`] reads them.
/// One dashboard's SQL planned for serving: a file's (`load_dashboards`) or a
/// preview's (#770), the same way.
fn plan_dash(id: String, sql: String, meta: sv::DashboardMeta, updated: Option<String>) -> Dash {
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
    // #517 S2: every input's declared variable name (static — the
    // column's own name or explicit alias, [`sql::rewrite`]'s
    // `Role::Input` case; never run to find out), and every `::SUBMIT`
    // statement's own column layout. `DATERANGE`/`PERIOD` (several
    // variables each, from a name the lint already can't check
    // statically either) are left out on purpose: a `SUBMIT_VARS`
    // naming one then falls through to the ordinary "unknown
    // variable" refusal, fail-closed either way.
    let mut inputs: HashMap<String, Vec<(usize, InputKind, usize)>> = HashMap::new();
    let mut submits: HashMap<usize, SubmitSpec> = HashMap::new();
    let mut saves: HashMap<usize, SaveSpec> = HashMap::new();
    for (i, p) in planned.iter().enumerate() {
        let col = |role: Role| p.roles.iter().find(|(_, r)| *r == role).map(|(j, _)| *j);
        if let (Some(save_idx), Some(tables_col)) = (col(Role::Save), col(Role::SaveTables)) {
            let name = p
                .names
                .iter()
                .find(|(j, _)| *j == save_idx)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| format!("save_{i}"));
            saves.insert(
                i,
                SaveSpec {
                    name,
                    graph_col: col(Role::SaveGraph),
                    form_col: col(Role::SaveForm),
                    tables_col,
                },
            );
        }
        use anofox_visualization::inputs::{input_index, input_kind};
        if let Some(kind) = input_kind(&p.roles) {
            if !matches!(
                kind,
                InputKind::DateRange | InputKind::Period | InputKind::Form
            ) {
                let idx = input_index(&p.roles);
                if let Some(name) = p.names.iter().find(|(j, _)| *j == idx).map(|(_, n)| n) {
                    inputs
                        .entry(name.to_ascii_lowercase())
                        .or_default()
                        .push((i, kind, idx));
                }
            }
        }
        if p.roles.iter().any(|(_, r)| matches!(r, Role::Submit)) {
            let submit_idx = p
                .roles
                .iter()
                .find(|(_, r)| matches!(r, Role::Submit))
                .map(|(j, _)| *j)
                .unwrap_or(0);
            let name = p
                .names
                .iter()
                .find(|(j, _)| *j == submit_idx)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| format!("submit_{i}"));
            let vars_col = p
                .roles
                .iter()
                .find(|(_, r)| matches!(r, Role::SubmitVars))
                .map(|(j, _)| *j);
            let done_col = p
                .roles
                .iter()
                .find(|(_, r)| matches!(r, Role::SubmitDone))
                .map(|(j, _)| *j);
            if let Some(vars_col) = vars_col {
                submits.insert(
                    i,
                    SubmitSpec {
                        name,
                        vars_col,
                        done_col,
                    },
                );
            }
        }
    }
    let materialize = planned
        .iter()
        .map(|p| p.materialize && p.setup && materialize::split(&p.sql).is_some())
        .collect();
    let (stmts, requires) = planned
        .into_iter()
        .map(|p| ((p.setup, p.sql.trim().to_string()), p.requires))
        .unzip();
    Dash {
        id,
        sql,
        stmts,
        info,
        meta,
        updated,
        requires,
        materialize,
        tabs,
        inputs,
        submits,
        saves,
    }
}

fn load_dashboards(dir: &Path, folders: bool) -> Result<(Vec<Dash>, Vec<String>), String> {
    let mut out = Vec::new();
    let mut loads = Vec::new();
    for f in sv::dashboard_files(dir, folders)? {
        loads.extend(f.meta.loads.iter().cloned());
        let updated = sv::file_date(&f.path);
        out.push(plan_dash(f.id, f.sql, f.meta, updated));
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
    dashes: &Arc<DashSet>,
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
    let nav_picker_snapshots;
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
        let mut exts: Vec<&String> = loaded.iter().chain(dashes.loads.iter()).collect();
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
        // #727: where `-- @materialize` results are kept (no ATTACH after
        // the lockdown below).
        c.exec(&format!("ATTACH ':memory:' AS {MAT_DB}"))?;
        c.exec("SET enable_external_access = false")?;
        c.exec("SET lock_configuration = true")?;
        // Fail now, not on every request (e.g. a schema that doesn't exist).
        c.exec(&enter)
            .map_err(|e| format!("options.search_path: {}", e.lines().next().unwrap_or("")))?;
        nav_picker_snapshots = opts
            .nav_pickers
            .iter()
            .map(|np| build_nav_picker(&c, opts, np))
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok((
        Snapshot {
            warm: WarmPool::new(opts.warm.unwrap_or(opts.threads)),
            mats: MatStore::default(),
            db,
            default_db,
            enter,
            policy: None,
            deps: Vec::new(),
            deps_tags: Vec::new(),
            warm_deps: Vec::new(),
            nav_picker_snapshots,
            implicit: HashMap::new(),
            previews: Mutex::new(HashMap::new()),
            dashes: dashes.clone(),
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

/// A `/api/submit` 400 that names one of *this dashboard's own input
/// variables* (never the request's `panel`/`dashboard`/`id` fields, which
/// aren't inputs a viewer can focus): JSON `{"error": msg, "var": name}`,
/// so the viewer can mark that one input `aria-invalid` instead of only
/// showing the text (#551). `msg` is still the same human-readable string
/// `reply` would have sent as plain text; `var` is new and additive —
/// nothing here changes `msg`'s wording.
fn reply_invalid_var(req: Request, var: &str, msg: &str) -> u16 {
    let body = serde_json::json!({ "error": msg, "var": var }).to_string();
    respond(req, 400, "application/json", body.into_bytes());
    400
}

fn handle_locked(st: &Locked, req: Request) {
    let t0 = std::time::Instant::now();
    let method = req.method().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    let peer = req.remote_addr().map(|a| a.ip());
    let mut info = LogInfo::default();
    let status = route_locked(st, req, &path, &mut info);
    // #765: always counted (cheap atomics); exposed only with options.metrics.
    st.metrics
        .record(metrics::route_of(&method, &path), status, t0.elapsed());
    if let Some(d) = info.deny {
        st.metrics.denied(d);
    }
    if let Some(c) = info.cache {
        st.metrics.cache(c);
    }
    if let Some(log) = &st.opts.log {
        // Page views, data requests and refusals; not every static asset.
        // #517 S2, plan §4.6: "/api/submit always logged".
        if path == "/api/panel"
            || path == "/api/submit"
            || path == "/api/save"
            || path == "/"
            || path.starts_with("/d/")
            || status >= 400
        {
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

/// `/d/<id>/assets/<rest>` → `(id, rest)` — `None` for anything else
/// (including a bare `/d/<id>/assets` with no trailing `/`, which never
/// names a file, and a `path` that isn't under `/d/` at all). `id` is left
/// exactly as it appears in the URL (not yet checked against any real
/// dashboard — [`locked_asset`] does that); `rest` is still
/// percent-encoded, exactly as the client sent it ([`route_locked`]'s
/// `path` is `req.url()`, undecoded).
fn split_asset_path(path: &str) -> Option<(&str, &str)> {
    let tail = path.strip_prefix("/d/")?;
    let (id, after_id) = tail.split_once('/')?;
    if id.is_empty() {
        return None;
    }
    let rest = after_id.strip_prefix("assets/")?;
    Some((id, rest))
}

/// `GET`/`HEAD` `/d/<id>/assets/<rest>` (#514 PR B): one image file from
/// `<dashboards dir>/assets/<rest>`, under the same login and scope as the
/// dashboard's own page (this route is **not** in `auth.public_asset` —
/// `route_locked`'s caller already enforced `auth` above this match arm;
/// `scope` is that signed-in credential's own dashboard scope, or `None`
/// without `auth` configured). Containment, the extension allow-list, the
/// size cap and the real magic-byte MIME are all
/// [`anofox_visualization::image::read_asset`]'s job — the same function
/// `dashboard`'s `--svg`/HTML CLI output and `--site` use, so every reader
/// of `assets/` enforces exactly the same rule. Every refusal here is a
/// plain 404 (never 403/500): which reason it was (missing folder, no such
/// dashboard, traversal, wrong extension, not really an image, too big) is
/// not a viewer's business, and a distinguishable error would let a probe
/// tell "exists but refused" apart from "doesn't exist" — except a scope
/// mismatch, which (like the dashboard page itself) is its own 403.
fn locked_asset(
    st: &Locked,
    req: Request,
    id: &str,
    rest: &str,
    scope: Option<&Scope>,
    info: &mut LogInfo,
) -> u16 {
    let gen = st.current.get();
    let Some(d) = gen.snap.dashes.find(id) else {
        return reply(req, 404, "no such dashboard");
    };
    if !scope.is_none_or(|s| s.allows(&d.id)) {
        info.dashboard = Some(d.id.clone());
        return reply(req, 403, "forbidden: not in this credential's scope");
    }
    info.dashboard = Some(d.id.clone());
    let Some(root) = &gen.snap.dashes.assets_root else {
        return reply(req, 404, "not found");
    };
    let max = st.opts.max_asset_bytes as u64;
    // Review of #554 S2 (follow-up to nit 7): open + `fstat` *first*, so
    // the in-flight hold reflects the file's real size, not a pessimistic
    // `max_asset_bytes` for every request regardless of how big the file
    // actually is. `max_asset_bytes` is already clamped to `max_inflight`
    // at parse time (`LockedOptions::parse`), so this can't itself let a
    // run of requests exceed that shared budget — it just stops a page
    // full of small icons from each paying for the full cap.
    let (file, len) = match anofox_visualization::image::open_validated_asset(root, rest) {
        Ok(ok) => ok,
        Err(_) => return reply(req, 404, "not found"),
    };
    let is_head = req.method() == &Method::Head;
    // `HEAD` (via `stat_opened_asset`, below) never reads more than a
    // 4 KiB sniffing prefix — hold that, not the whole file, which only
    // `GET` actually reads.
    let hold = if is_head { len.min(4096) } else { len.min(max) } as usize;
    let mut charge = st.inflight.charge();
    if let Err(e) = charge.hold(hold) {
        busy(req, &e);
        return 503;
    }
    // Review of #554 nit 9: on an `auth` server an image must not keep
    // answering from a shared/browser cache for long after a logout or a
    // revoked credential — a minute, not the plain five-minute default
    // (still `private`, not `no-store`: the browser's own cache is fine to
    // use meanwhile, just not for long). Review of #604 N5: the rest of
    // the header list (CSP — belt-and-braces for a viewer who opens the
    // asset URL directly, no symlink-out/XSS either way — nosniff, CORP)
    // is `image::asset_header_values`, shared with both authoring servers'
    // own `authoring_asset`; only this lifetime and the mime differ here.
    let cache_control = if st.opts.auth.is_some() {
        "private, max-age=60"
    } else {
        "private, max-age=300"
    };
    let headers = |mime: &str| {
        anofox_visualization::image::asset_header_values(mime, cache_control)
            .into_iter()
            .map(|(k, v)| header(k, &v))
            .collect::<Vec<_>>()
    };
    // Read per request — no snapshot — so an edited/added asset shows
    // without a refresh, as documented.
    if is_head {
        // Review of #554 nit 6: the real length (`fstat`, already known
        // as `len`), not whatever an empty placeholder body happened to
        // report — `GET` and `HEAD` must agree, and reading the whole
        // file just to discard it for `HEAD` is wasted work besides.
        // `tiny_http` itself already skips writing a `HEAD` response's
        // body (`Request::respond`'s `do_not_send_body`); this only fixes
        // the `Content-Length` this handler reports.
        let (mime, len) = match anofox_visualization::image::stat_opened_asset(file, len, max) {
            Ok(ok) => ok,
            Err(_) => return reply(req, 404, "not found"),
        };
        info.bytes = Some(len as usize);
        let resp = Response::new(
            200.into(),
            headers(&mime),
            std::io::empty(),
            Some(len as usize),
            None,
        );
        let _ = req.respond(resp);
        return 200;
    }
    let (mime, bytes) = match anofox_visualization::image::read_opened_asset(file, len, max) {
        Ok(ok) => ok,
        Err(_) => return reply(req, 404, "not found"),
    };
    info.bytes = Some(bytes.len());
    let len = bytes.len();
    let resp = Response::new(
        200.into(),
        headers(&mime),
        std::io::Cursor::new(bytes),
        Some(len),
        None,
    );
    let _ = req.respond(resp);
    200
}

fn route_locked(st: &Locked, mut req: Request, path: &str, info: &mut LogInfo) -> u16 {
    // #763: a shared dashboard's password form (`POST /share/<id>`): its
    // small body is read now, before the request is borrowed below; it is
    // only used after the Host and Origin checks.
    let share_body = match &st.opts.auth {
        Some(a) if a.share.is_some() && path.starts_with("/share/") => {
            if req.method() == &Method::Post {
                match read_body(&mut req, 4096) {
                    Ok(b) => Some(b),
                    Err((code, e)) => return reply(req, code, &e),
                }
            } else {
                Some(String::new())
            }
        }
        _ => None,
    };
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
    // #740: the assistant's proxy is an authoring route only; a locked
    // server never has it, whatever the method or the login. (Only
    // `/assist/v1/…`: `/assist/<file>` is the UI's own code, web/assist/.)
    if assist::is_api_path(path) {
        return reply(req, 404, "not found");
    }
    // #771: the embed SDK (`<anofox-dashboard>`) is loaded by host pages of
    // other origins as a module script, which needs CORS: public files with
    // no data, no credentials, and a GET that changes nothing — so before
    // the cross-origin refusal below.
    if let (&Method::Get | &Method::Head, Some(name)) = (req.method(), path.strip_prefix("/sdk/")) {
        let file = WEB.get_file(format!("sdk/{name}")).filter(|_| {
            name.ends_with(".js") && !name.contains(['/', '\\']) && !name.starts_with('.')
        });
        return match file {
            Some(f) => {
                let resp = Response::from_data(f.contents().to_vec())
                    .with_status_code(200)
                    .with_header(header("Content-Type", "text/javascript; charset=utf-8"))
                    .with_header(header("X-Content-Type-Options", "nosniff"))
                    .with_header(header("Access-Control-Allow-Origin", "*"))
                    .with_header(header("Cross-Origin-Resource-Policy", "cross-origin"))
                    .with_header(header("Cache-Control", "public, max-age=300"));
                let _ = req.respond(resp);
                200
            }
            None => reply(req, 404, "not found"),
        };
    }
    // CSRF: a browser request from another origin is refused outright.
    if !sv::origin_matches_host(m.origin, m.host) {
        return reply(req, 403, "forbidden: cross-origin request");
    }
    // #765: Prometheus metrics, behind their own bearer token (a scraper
    // has no viewer login); without `options.metrics` there is no such route.
    if let (Some(token), "/metrics") = (&st.opts.metrics_token, path) {
        if req.method() != &Method::Get {
            return reply(req, 405, "method not allowed");
        }
        let given = req_header(&req, "Authorization")
            .and_then(|h| h.split_once(' '))
            .filter(|(s, _)| s.eq_ignore_ascii_case("bearer"))
            .map(|(_, t)| t.trim().to_string());
        let ok = given.is_some_and(|t| auth::session::ct_eq(&t, token));
        if !ok {
            info.deny = Some("metrics_token");
            let resp = Response::from_data(&b"unauthorized"[..])
                .with_status_code(401)
                .with_header(header("WWW-Authenticate", "Bearer"))
                .with_header(header("Cache-Control", "no-store"));
            let _ = req.respond(resp);
            return 401;
        }
        let body = st.metrics.render(&metric_gauges(st));
        let resp = Response::from_data(body.into_bytes())
            .with_status_code(200)
            .with_header(header("Content-Type", metrics::CONTENT_TYPE))
            .with_header(header("Cache-Control", "no-store"));
        let _ = req.respond(resp);
        return 200;
    }
    // #773: the operator's stylesheet answers without a login, like the
    // UI's own files — an embedded page's <link> can't send its session
    // header, and the file holds no data.
    if let (Some(css), &Method::Get | &Method::Head, site_css::PATH) =
        (&st.opts.site_css, req.method(), path)
    {
        let resp = Response::from_data(css.as_bytes().to_vec())
            .with_status_code(200)
            .with_header(header("Content-Type", "text/css; charset=utf-8"))
            .with_header(header("X-Content-Type-Options", "nosniff"))
            .with_header(header("Cache-Control", "public, max-age=300"));
        let _ = req.respond(resp);
        return 200;
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
            if let Some(rep) = share_body.and_then(|b| auth.share_login(&r, &b)) {
                return send(req, rep);
            }
            if auth.public_asset(&method, path) {
                serve_asset(req, path, "", false);
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
                "dashboards": gen.snap.dashes.list.len(),
                "epoch": gen.epoch,
                "snapshot_age_s": gen.built.elapsed().as_secs(),
                "refresh_s": st.opts.refresh.map(|d| d.as_secs()),
                "refreshing": st.current.refreshing(),
                "durability": st.opts.durability.as_ref().map(|d| serde_json::json!({
                    "writer": d.run,
                    "last_ok": st.durable.last_ok.load(Ordering::Relaxed),
                    "failing": st.durable.failing.load(Ordering::Relaxed),
                })),
                // The error itself is only logged / returned to the operator.
                "refresh_failed": st.current.last_failed(),
                "limits": limits_json(st),
            });
            // #517 S2, review of #538 should-fix 8: operators need to see
            // the store is reachable and how full it is without reading
            // stderr. `ok` false means every submit is currently refused
            // (503); `full` true means every submit is refused too (507),
            // and stays true even after `max_mb` is raised and the server
            // restarted picks the file back up non-full (append-only: it
            // can only grow, never shrink, in this process's lifetime).
            if let Some(submit) = &st.submit {
                if let Some(map) = body.as_object_mut() {
                    map.insert(
                        "state".into(),
                        serde_json::json!({
                            "ok": !submit.store.is_failed(),
                            "full": submit.full.load(Ordering::Relaxed),
                        }),
                    );
                }
            }
            // #684: the change log, same meaning as `state` above.
            if let Some(w) = &st.writeback {
                if let Some(map) = body.as_object_mut() {
                    map.insert(
                        "writeback".into(),
                        serde_json::json!({
                            "ok": !w.store.is_failed(),
                            "full": w.full.load(Ordering::Relaxed),
                        }),
                    );
                }
            }
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
        (&Method::Post, "/api/submit") => {
            let (origin, host) = (m.origin.map(str::to_string), m.host.map(str::to_string));
            handle_submit(
                st,
                req,
                origin.as_deref(),
                host.as_deref(),
                claims.as_ref(),
                scope,
                ident.as_ref().map(|i| i.method),
                info,
            )
        }
        (&Method::Post, "/api/save") => {
            let (origin, host) = (m.origin.map(str::to_string), m.host.map(str::to_string));
            handle_save(
                st,
                req,
                origin.as_deref(),
                host.as_deref(),
                claims.as_ref(),
                scope,
                ident.as_ref().map(|i| i.method),
                info,
            )
        }
        (&Method::Post, "/api/dashboards") => handle_preview_create(st, req, ident.as_ref(), info),
        (&Method::Post, "/api/auth/token") => handle_embed_token(st, req, ident.as_ref(), info),
        (&Method::Delete, p) if p.starts_with("/api/dashboards/") => handle_preview_delete(
            st,
            req,
            &p["/api/dashboards/".len()..],
            ident.as_ref(),
            info,
        ),
        (&Method::Get, p) if p.starts_with("/p/") => {
            let id = p["/p/".len()..].trim_end_matches('/');
            match st.previews.as_ref().and_then(|ps| ps.get(id)) {
                Some(pv) if scope.is_none_or(|s| s.allows(id)) => {
                    info.dashboard = Some(id.to_string());
                    let session = ident.as_ref().and_then(|i| i.page_session.as_deref());
                    let html =
                        String::from_utf8_lossy(&served_index(st, &pv.dash, scope, session, false))
                            .into_owned();
                    // The link is the preview's only name: never sent on.
                    respond_page(req, &html, &st.opts, &[("Referrer-Policy", "same-origin")]);
                    200
                }
                Some(_) => reply(req, 403, "forbidden: not in this credential's scope"),
                None => reply(req, 404, "no such preview (or it expired)"),
            }
        }
        (&Method::Post, p) if p.starts_with("/api/data/") => {
            handle_ingest(st, req, &p["/api/data/".len()..], ident.as_ref(), info)
        }
        (&Method::Get, p) if p.starts_with("/api/export/") => handle_export(
            st,
            req,
            p,
            claims.as_ref(),
            scope,
            ident.as_ref().map(|i| i.method),
            info,
        ),
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
                ident.as_ref().map(|i| i.method),
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
        // #514 PR B: before the generic `/d/<id>` arm below, so an asset
        // path is never mistaken for a dashboard id.
        (&Method::Get | &Method::Head, p) if split_asset_path(p).is_some() => {
            let (id, rest) = split_asset_path(p).expect("checked by the guard above");
            locked_asset(st, req, id, rest, scope, info)
        }
        (&Method::Get, p) if p.starts_with("/d/") => {
            let id = p["/d/".len()..].trim_end_matches('/');
            let gen = st.current.get();
            match gen.snap.dashes.find(id) {
                // A scoped credential (embed link, API key) reads its own
                // dashboards only.
                Some(d) if !scope.is_none_or(|s| s.allows(&d.id)) => {
                    info.dashboard = Some(d.id.clone());
                    reply(req, 403, "forbidden: not in this credential's scope")
                }
                Some(d) => {
                    info.dashboard = Some(d.id.clone());
                    let page = ident.as_ref().and_then(|i| i.page_session.as_deref());
                    // #684: may this viewer save here (the button says why not)?
                    let writer = st.writeback.as_ref().is_some_and(|w| {
                        ident.as_ref().map(|i| i.method) != Some("api_key")
                            && claims.as_ref().is_some_and(|c| {
                                !access::is_reserved_user(&c.user)
                                    && w.opts.is_writer(&c.user, &c.groups)
                            })
                    });
                    let html = String::from_utf8_lossy(&served_index(st, d, scope, page, writer))
                        .into_owned();
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
            serve_asset(req, p, "", false);
            if found {
                200
            } else {
                404
            }
        }
        _ => reply(req, 405, "method not allowed"),
    }
}

// ---- #764: exports -------------------------------------------------------

/// Most panels one export runs.
const EXPORT_MAX_PANELS: usize = 200;

/// A dashboard's data for [`dashboard::render_dashboard_svg`] through the
/// locked server's own panel path ([`locked_panel_req`]): every panel runs
/// exactly as the page's request for it would — the credential's scope and
/// locked variables, `-- @vars`, the claims, the access policy, the cache,
/// the query slots and limits. Setup statements are not run here (the
/// server runs them per panel); a `SET VARIABLE` the renderer makes for an
/// input's default becomes a request variable, unless the caller set it.
struct ExportData<'a> {
    st: &'a Locked,
    dash: &'a Dash,
    claims: Option<&'a Claims>,
    scope: Option<&'a Scope>,
    method: Option<&'static str>,
    /// The caller's variables (`?v.name=value`), then inputs' defaults.
    vars: Vec<(String, String)>,
    viewer: Vec<String>,
    deadline: Instant,
    panels: usize,
    /// The first panel failure that is the caller's to see (scope, busy).
    fatal: Option<(u16, String)>,
}

/// `SET VARIABLE <name> = <plain literal>` (what the renderer emits for an
/// input's default or a step number): the name and the literal, else `None`.
fn set_default(sql: &str) -> Option<(String, String)> {
    let rest = sql.trim().trim_end_matches(';');
    let head = rest.get(..13)?;
    if !head.eq_ignore_ascii_case("SET VARIABLE ") {
        return None;
    }
    let (name, lit) = rest[13..].split_once('=')?;
    let (name, lit) = (name.trim(), lit.trim());
    // The renderer quotes the name (`"region"`): still a plain identifier.
    let name = match name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(n) if !n.contains('"') => n,
        Some(_) => return None,
        None => name,
    };
    if !sv::valid_ident(name) || access::is_reserved_var(name) {
        return None;
    }
    // Only literals: strings, numbers, booleans, NULL, lists of them.
    use anofox_visualization::sql::lex::{tokenize, TokKind};
    let ok = tokenize(lit).iter().all(|t| match t.kind {
        TokKind::String | TokKind::Number | TokKind::Whitespace => true,
        TokKind::Word => matches!(
            t.text(lit).to_ascii_uppercase().as_str(),
            "TRUE" | "FALSE" | "NULL"
        ),
        TokKind::Punct => matches!(t.text(lit), "[" | "]" | "," | "-" | "."),
        _ => false,
    });
    (ok && !lit.is_empty()).then(|| (name.to_string(), lit.to_string()))
}

impl ExportData<'_> {
    fn run(
        &mut self,
        panel: usize,
    ) -> Result<Vec<(String, Vec<anofox_visualization::Value>)>, String> {
        if Instant::now() > self.deadline {
            return Err("export timed out".into());
        }
        self.panels += 1;
        if self.panels > EXPORT_MAX_PANELS {
            return Err(format!("more than {EXPORT_MAX_PANELS} panels"));
        }
        let r = sv::PanelRequest {
            dashboard: self.dash.id.clone(),
            panel,
            vars: self.vars.clone(),
            page: None,
        };
        let mut info = LogInfo::default();
        let mut charge = self.st.inflight.charge();
        match locked_panel_req(
            self.st,
            r,
            None,
            self.claims,
            self.scope,
            self.method,
            &mut info,
            &mut charge,
        ) {
            Ok((json, ..)) => {
                let rows: Vec<serde_json::Map<String, serde_json::Value>> =
                    serde_json::from_str(&json).map_err(|e| e.to_string())?;
                Ok(anofox_visualization::sql::result_columns(&rows))
            }
            Err((code, e)) => {
                if matches!(code, 403 | 503) && self.fatal.is_none() {
                    self.fatal = Some((code, e.clone()));
                }
                Err(e)
            }
        }
    }
}

impl dashboard::DataProvider for ExportData<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        // An input's default (or a step number): a request variable from now
        // on, unless the caller chose its own value.
        if let Some((name, lit)) = set_default(sql) {
            if !self.viewer.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                self.vars.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                self.vars.push((name, lit));
            }
        }
        Ok(())
    }

    fn query(
        &mut self,
        sql: &str,
    ) -> Result<Vec<(String, Vec<anofox_visualization::Value>)>, String> {
        let sql = sql.trim();
        let i = self
            .dash
            .stmts
            .iter()
            .position(|(setup, s)| !setup && s.trim() == sql)
            .ok_or("not available in an export")?;
        self.run(i)
    }
}

/// Who an export runs as and how it renders (#764) — a request's caller, or
/// a report's principal (#768).
struct ExportAs<'a> {
    claims: Option<&'a Claims>,
    scope: Option<&'a Scope>,
    method: Option<&'static str>,
    /// The caller's variables (`?v.name=value`, a report's `vars`).
    vars: Vec<(String, String)>,
}

impl<'a> ExportAs<'a> {
    fn data(&self, st: &'a Locked, dash: &'a Dash) -> ExportData<'a> {
        ExportData {
            st,
            dash,
            claims: self.claims,
            scope: self.scope,
            method: self.method,
            viewer: self.vars.iter().map(|(n, _)| n.clone()).collect(),
            vars: self.vars.clone(),
            deadline: Instant::now() + st.opts.timeout * 4,
            panels: 0,
            fatal: None,
        }
    }
}

/// The plan index of the panel named `-- @name <name>`.
fn named_panel(plan: &[anofox_visualization::sql::Panel], name: &str) -> Option<usize> {
    plan.iter()
        .position(|p| !p.setup && p.name.as_deref() == Some(name))
}

/// One `-- @name`d panel's rows as CSV or JSON (#764), the inputs before it
/// bound to their defaults first, as on the page.
fn export_panel(
    st: &Locked,
    dash: &Dash,
    who: &ExportAs,
    name: &str,
    ext: &str,
    info: &mut LogInfo,
) -> Result<Vec<u8>, (u16, String)> {
    let mut data = who.data(st, dash);
    let plan = anofox_visualization::sql::plan(&dash.sql);
    let Some(i) = named_panel(&plan, name) else {
        return Err((404, NO_SUCH_PANEL.into()));
    };
    info.panel = Some(i);
    for (j, p) in plan.iter().enumerate().take(i) {
        if p.setup || anofox_visualization::inputs::input_kind(&p.roles).is_none() {
            continue;
        }
        if let Ok(rows) = data.run(j) {
            for set in anofox_visualization::inputs::panel_defaults(&p.roles, &rows) {
                let _ = dashboard::DataProvider::execute(&mut data, &set);
            }
        }
    }
    let cols = match data.run(i) {
        Ok(c) => c,
        Err(e) => return Err(data.fatal.take().unwrap_or((400, e))),
    };
    // Column headers: the query's own names where it has them.
    let headers: Vec<String> = cols
        .iter()
        .map(|(k, _)| {
            k.strip_prefix('c')
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| {
                    plan[i]
                        .names
                        .iter()
                        .find(|(j, _)| *j == n)
                        .map(|(_, s)| s.clone())
                })
                .unwrap_or_else(|| k.clone())
        })
        .collect();
    let n = cols.first().map_or(0, |(_, v)| v.len());
    let cell = |v: &anofox_visualization::Value| match v {
        anofox_visualization::Value::Na => String::new(),
        v => anofox_visualization::value_text(v),
    };
    Ok(if ext == "csv" {
        let mut out = String::new();
        let row = |cells: Vec<String>| -> String {
            cells
                .iter()
                .map(|c| sv::csv_cell(c))
                .collect::<Vec<_>>()
                .join(",")
                + "\r\n"
        };
        out.push_str(&row(headers.clone()));
        for r in 0..n {
            out.push_str(&row(cols.iter().map(|(_, v)| cell(&v[r])).collect()));
        }
        out.into_bytes()
    } else {
        let rows: Vec<serde_json::Value> = (0..n)
            .map(|r| {
                let mut m = serde_json::Map::new();
                for (h, (_, v)) in headers.iter().zip(&cols) {
                    m.insert(h.clone(), anofox_visualization::value_json(&v[r]));
                }
                serde_json::Value::Object(m)
            })
            .collect();
        serde_json::to_vec(&rows).unwrap_or_default()
    })
}

const NO_SUCH_PANEL: &str = "no such panel: name it with `-- @name <name>` in the dashboard";

/// How a dashboard export looks.
struct ExportLook<'a> {
    /// One tab, by its label.
    tab: Option<&'a str>,
    /// One `-- @name`d panel alone, full width (#815).
    panel: Option<&'a str>,
    width: u32,
    dark: bool,
    /// PNG pixels per SVG pixel.
    scale: f32,
}

/// The dashboard (or one tab) as SVG, PNG or PDF (#764), and its `::ASOF`
/// text when it has one (for a report's message).
fn export_dashboard(
    st: &Locked,
    dash: &Dash,
    who: &ExportAs,
    look: &ExportLook,
    ext: &str,
) -> Result<(Vec<u8>, Option<String>), (u16, String)> {
    let mut data = who.data(st, dash);
    let plan = anofox_visualization::sql::plan(&dash.sql);
    let first_text = |c: Vec<(String, Vec<anofox_visualization::Value>)>| {
        c.first()
            .and_then(|(_, v)| v.first().map(anofox_visualization::value_text))
    };
    // One panel (#815): it and the directives right before it (::HEIGHT,
    // ::TOTALS, …), not a ::TAB / ::VIEW / ::GROUP / ::FIGURE marker.
    // One tab: its marker and the statements in it.
    let only = match (look.panel, look.tab) {
        (Some(name), _) => {
            let i = named_panel(&plan, name).ok_or((404, NO_SUCH_PANEL.to_string()))?;
            let marker = |p: &anofox_visualization::sql::Panel| {
                p.roles.iter().any(|(_, r)| {
                    use anofox_visualization::Role as R;
                    matches!(
                        r,
                        R::Tab
                            | R::View
                            | R::GroupStart
                            | R::GroupEnd
                            | R::FigureStart
                            | R::FigureEnd
                    )
                })
            };
            let mut keep = vec![i];
            for j in (0..i).rev() {
                let p = &plan[j];
                if p.setup {
                    continue;
                }
                if !anofox_visualization::roles::is_directive_panel(&p.roles) || marker(p) {
                    break;
                }
                keep.push(j);
            }
            Some(keep)
        }
        (None, None) => None,
        (None, Some(label)) => {
            let mut found = None;
            for (j, p) in plan.iter().enumerate() {
                if !p
                    .roles
                    .iter()
                    .any(|(_, r)| matches!(r, anofox_visualization::Role::Tab))
                {
                    continue;
                }
                let name = data.run(j).ok().and_then(first_text);
                if name.is_some_and(|n| n.trim().eq_ignore_ascii_case(label.trim())) {
                    found = Some(j);
                    break;
                }
            }
            let Some(j) = found else {
                return Err((404, "no such tab".into()));
            };
            Some(
                std::iter::once(j)
                    .chain(
                        (0..dash.stmts.len())
                            .filter(|i| dash.tabs.get(*i).is_some_and(|t| t.contains(&j))),
                    )
                    .collect::<Vec<_>>(),
            )
        }
    };
    let opts = dashboard::DashboardOptions {
        width: look.width,
        theme: st.opts.theme.clone(),
        variant: if look.dark {
            anofox_visualization::theme::Variant::Dark
        } else {
            anofox_visualization::theme::Variant::Light
        },
        asset_root: st.current.get().snap.dashes.assets_root.clone(),
        single: look.panel.is_some(),
        only,
        ..Default::default()
    };
    let svg = dashboard::render_dashboard_svg(&dash.sql, &mut data, &opts);
    if let Some(f) = data.fatal.take() {
        return Err(f);
    }
    let svg = svg.map_err(|e| (400, e))?;
    // The ::ASOF text (the page's "data as of" line).
    let as_of = plan
        .iter()
        .position(|p| {
            !p.setup
                && p.roles
                    .iter()
                    .any(|(_, r)| matches!(r, anofox_visualization::Role::AsOf))
        })
        .and_then(|j| data.run(j).ok().and_then(first_text));
    // #764: PNG / PDF from the same SVG, with the bundled font.
    let bytes = match ext {
        "png" => crate::raster::png(&svg, look.scale).map_err(|e| (413, e))?,
        "pdf" => crate::raster::pdf(&svg).map_err(|e| (400, e))?,
        _ => svg.into_bytes(),
    };
    Ok((bytes, as_of))
}

/// `GET /api/export/<id>.svg` (the whole dashboard, or `?tab=<label>`) and
/// `GET /api/export/<id>/<panel name>.csv|.json` (one `-- @name`d panel's
/// rows) or `.svg|.png` (it drawn alone, #815), with `?v.<name>=<value>`
/// for the variables (#764).
#[allow(clippy::too_many_arguments)]
fn handle_export(
    st: &Locked,
    req: Request,
    path: &str,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    method: Option<&'static str>,
    info: &mut LogInfo,
) -> u16 {
    let rest = &path["/api/export/".len()..];
    let (target, ext) = match rest.rsplit_once('.') {
        Some((t, e)) if matches!(e, "svg" | "png" | "pdf" | "csv" | "json") => (t, e),
        _ => return reply(
            req,
            404,
            "not found: /api/export/<dashboard>.svg|png|pdf or /api/export/<dashboard>/<panel>.svg|png|csv|json",
        ),
    };
    let (id, panel_name) = match target.split_once('/') {
        Some((d, p)) => (d, Some(p)),
        None => (target, None),
    };
    if (panel_name.is_none() && matches!(ext, "csv" | "json"))
        || (panel_name.is_some() && ext == "pdf")
    {
        return reply(
            req,
            404,
            "not found: a dashboard exports as .svg, .png or .pdf, a named panel as .svg, .png, .csv or .json",
        );
    }
    let gen = st.current.get();
    let Some(dash) = gen.snap.dashes.find(id) else {
        return reply(req, 404, "no such dashboard");
    };
    info.dashboard = Some(dash.id.clone());
    if !scope.is_none_or(|s| s.allows(&dash.id)) {
        return reply(req, 403, "forbidden: not in this credential's scope");
    }
    // ?v.name=value (strings, like a page URL's), ?tab=, ?width=, ?dark=1
    let q = sv::parse_query(req.url());
    let mut given = serde_json::Map::new();
    for (k, v) in &q {
        if let Some(name) = k.strip_prefix("v.") {
            given.insert(name.to_string(), serde_json::Value::String(v.clone()));
        } else if !matches!(k.as_str(), "tab" | "width" | "dark" | "scale") {
            return reply(
                req,
                400,
                &format!("unknown parameter '{}'", access::diag(k)),
            );
        }
    }
    let vars = match sv::parse_vars_object(Some(&serde_json::Value::Object(given))) {
        Ok(v) => v,
        Err(e) => return reply(req, 400, &e),
    };
    let who = ExportAs {
        claims,
        scope,
        method,
        vars,
    };
    let file = |e: &str| match panel_name {
        Some(p) => format!("{}-{p}.{e}", dash.id),
        None => format!("{}.{e}", dash.id),
    };
    let send = |req: Request, ctype: &str, body: Vec<u8>, name: String| -> u16 {
        let mut resp = Response::from_data(body)
            .with_status_code(200)
            .with_header(header("Content-Type", ctype))
            .with_header(header(
                "Content-Disposition",
                &format!("attachment; filename=\"{name}\""),
            ))
            .with_header(header("Cache-Control", "no-store"))
            .with_header(header("X-Content-Type-Options", "nosniff"));
        if ctype.starts_with("image/svg") {
            resp = resp.with_header(header(
                "Content-Security-Policy",
                "default-src 'none'; style-src 'unsafe-inline'; img-src data:; sandbox",
            ));
        }
        let _ = req.respond(resp);
        200
    };
    if let Some(name) = panel_name.filter(|_| matches!(ext, "csv" | "json")) {
        return match export_panel(st, dash, &who, name, ext, info) {
            Ok(b) if ext == "csv" => send(req, "text/csv; charset=utf-8", b, file("csv")),
            Ok(b) => send(req, "application/json", b, file("json")),
            Err((code, e)) => reply(req, code, &e),
        };
    }
    // #815: one panel for a chat card — narrower than a dashboard.
    let (min_w, default_w) = if panel_name.is_some() {
        (320, 640)
    } else {
        (480, 1200)
    };
    let width = match q.get("width").map(|w| w.parse::<u32>()) {
        None => default_w,
        Some(Ok(w)) if (min_w..=4000).contains(&w) => w,
        _ => return reply(req, 400, &format!("width must be {min_w}..=4000")),
    };
    if panel_name.is_some() && q.contains_key("tab") {
        return reply(req, 400, "tab doesn't apply to a panel export");
    }
    if let Some(name) = panel_name {
        info.panel = named_panel(&anofox_visualization::sql::plan(&dash.sql), name);
    }
    let scale = match q.get("scale").map(|v| v.parse::<f32>()) {
        None => 2.0,
        Some(Ok(v)) if ext == "png" && (1.0..=4.0).contains(&v) => v,
        _ => return reply(req, 400, "scale (png only) must be 1..=4"),
    };
    let look = ExportLook {
        tab: q.get("tab").map(String::as_str),
        panel: panel_name,
        width,
        dark: q.get("dark").is_some_and(|d| d == "1"),
        scale,
    };
    match export_dashboard(st, dash, &who, &look, ext) {
        Ok((b, _)) => {
            let ctype = match ext {
                "png" => "image/png",
                "pdf" => "application/pdf",
                _ => "image/svg+xml",
            };
            send(req, ctype, b, file(ext))
        }
        Err((code, e)) => reply(req, code, &e),
    }
}

/// The gauges of `GET /metrics` (#765), read now.
fn metric_gauges(st: &Locked) -> Vec<metrics::Gauge> {
    use metrics::Gauge;
    let gen = st.current.get();
    let (running, waiting, timed_out) = st.queries.stats();
    let b = |v: bool| if v { 1.0 } else { 0.0 };
    let mut g = vec![
        Gauge {
            name: "anofox_build_info",
            help: "The server's build (value 1).",
            values: vec![(
                vec![("version", anofox_visualization::BUILD_VERSION.to_string())],
                1.0,
            )],
        },
        Gauge::one(
            "anofox_dashboards",
            "Dashboards served.",
            st.current.get().snap.dashes.list.len() as f64,
        ),
        Gauge::one(
            "anofox_snapshot_epoch",
            "The snapshot's generation (+1 per refresh).",
            gen.epoch as f64,
        ),
        Gauge::one(
            "anofox_snapshot_age_seconds",
            "Seconds since the snapshot was built.",
            gen.built.elapsed().as_secs_f64(),
        ),
        Gauge::one(
            "anofox_snapshot_refreshing",
            "1 while a refresh runs.",
            b(st.current.refreshing()),
        ),
        Gauge::one(
            "anofox_snapshot_refresh_failed",
            "1 when the last refresh failed.",
            b(st.current.last_failed()),
        ),
        Gauge::one(
            "anofox_queries_running",
            "Panel queries running.",
            running as f64,
        ),
        Gauge::one(
            "anofox_queries_waiting",
            "Panel queries waiting for a slot.",
            waiting as f64,
        ),
        Gauge::one(
            "anofox_queries_refused",
            "Panel queries refused (no slot in time) since the start.",
            timed_out as f64,
        ),
        Gauge::one(
            "anofox_inflight_bytes",
            "Result bytes held until their responses are written.",
            st.inflight.used() as f64,
        ),
        Gauge::one(
            "anofox_inflight_peak_bytes",
            "Peak of anofox_inflight_bytes since the start.",
            st.inflight.peak() as f64,
        ),
        Gauge::one(
            "anofox_inflight_refused",
            "Requests refused for the in-flight budget since the start.",
            st.inflight.refused() as f64,
        ),
    ];
    if let Some(s) = &st.submit {
        g.push(Gauge::one(
            "anofox_state_store_ok",
            "1 while the per-viewer state store accepts submits.",
            b(!s.store.is_failed()),
        ));
        g.push(Gauge::one(
            "anofox_state_store_full",
            "1 when the per-viewer state store is full.",
            b(s.full.load(Ordering::Relaxed)),
        ));
    }
    if let Some(w) = &st.writeback {
        g.push(Gauge::one(
            "anofox_writeback_ok",
            "1 while the write-back change log accepts saves.",
            b(!w.store.is_failed()),
        ));
        g.push(Gauge::one(
            "anofox_writeback_full",
            "1 when the write-back change log is full.",
            b(w.full.load(Ordering::Relaxed)),
        ));
    }
    g.extend(task_gauges(st));
    g.extend(report_gauges(st));
    g.extend(alert_gauges(st));
    if st.opts.source.is_some() {
        g.push(Gauge::one(
            "anofox_source_consecutive_failures",
            "Failed snapshot builds from the Quack source in a row.",
            st.current.consecutive_failures() as f64,
        ));
        g.push(Gauge::one(
            "anofox_source_build_overdue",
            "1 while a source build runs past its deadline.",
            b(st.source_build.overdue()),
        ));
    }
    g
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
        "max_asset_bytes": o.max_asset_bytes,
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

/// A `sv::sql_literal`-shaped value's own text — the quoted string's
/// contents, or the literal itself for the all-digits/`NULL` shapes
/// [`valid_path_value`] also accepts. `None` for `NULL` (no value).
fn literal_text(lit: &str) -> Option<&str> {
    if lit == "NULL" {
        return None;
    }
    Some(
        lit.strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .unwrap_or(lit),
    )
}

/// #556: every `options.nav_pickers`/`options.nav_picker` `daterange`
/// entry's two `vars` — already shape-checked by [`check_path_values`]
/// (`path_vars`'s generic `[A-Za-z0-9_.-]` rule happens to already accept
/// `YYYY-MM-DD`) — get the picker's own, stricter rules: each value present
/// in `vars` must be a real calendar date, and when *both* halves of the
/// pair are present in this same request, `from` must not be after `to`.
/// A value outside the picker's own `[min, max]` (when it has `sql`) is
/// **clamped in place**, not refused: those bounds are an author-chosen
/// display range, not a security boundary (unlike the `path_vars` shape
/// check above, which guards a filesystem path) — the same posture a
/// calendar widget's own `min`/`max` attributes would take.
fn clamp_daterange_values(
    nav_pickers: &[NavPicker],
    snapshots: &[PickerSnapshot],
    vars: &mut [(String, String)],
) -> Result<(), (u16, String)> {
    for (np, snap) in nav_pickers.iter().zip(snapshots.iter()) {
        let (NavPicker::DateRange { vars: names, .. }, PickerSnapshot::DateRange { min, max, .. }) =
            (np, snap)
        else {
            continue;
        };
        let mut days: [Option<i64>; 2] = [None, None];
        for (i, name) in names.iter().enumerate() {
            let Some(idx) = vars.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) else {
                continue;
            };
            let Some(text) = literal_text(&vars[idx].1) else {
                // #556 nit (review-582): a NULL path_vars value ("no
                // value" — see the fn doc above) has to be retyped as
                // VARCHAR here, not left as the bare `NULL` a viewer or
                // the nav picker's own "Clear" sent — DuckDB gives a bare
                // `SET VARIABLE x = NULL` the default INTEGER type, and
                // every dashboard reading a daterange var does
                // `coalesce(getvariable(x), '…')::VARCHAR`, which then
                // 400s with "Could not convert string '…' to INT32"
                // instead of ever reaching the author's own SQL.
                vars[idx].1 = "NULL::VARCHAR".to_string();
                continue;
            };
            days[i] = Some(parse_iso_date(text).ok_or_else(|| {
                (
                    400,
                    format!("variable '{name}' must be a real calendar date (YYYY-MM-DD)"),
                )
            })?);
        }
        if let [Some(from), Some(to)] = days {
            if from > to {
                return Err((
                    400,
                    format!(
                        "variables '{}'/'{}': the time window's start must not be after its end",
                        names[0], names[1]
                    ),
                ));
            }
        }
        for (i, day) in days.into_iter().enumerate() {
            let Some(day) = day else { continue };
            let clamped = match (min, max) {
                (Some(min), Some(max)) => day.clamp(*min, *max),
                _ => day,
            };
            if clamped != day {
                if let Some(entry) = vars
                    .iter_mut()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&names[i]))
                {
                    entry.1 = sql_string_literal(&format_iso_date(clamped));
                }
            }
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

/// #517 S3, plan §5 "which rows": whether `dash_id` is read-back-enabled
/// under `options.state`, and if so, whose rows a viewer with `claims` may
/// see. `None`: no `options.state`, or `dash_id` is in none of
/// `dashboards`/`read`/`trainers.dashboards` — `viewer_state` is never
/// created for it, and the cache/warm keys are unaffected ("dashboards
/// without state don't change keys"). `Some(true)`: a trainer dashboard
/// *and* this viewer is named in `trainers.groups`/`trainers.users` —
/// every viewer's rows. `Some(false)`: this viewer's own rows only (every
/// other case, including a *non*-trainer viewer on a trainer dashboard —
/// fails safe, threat model #19: anyone else on that same dashboard still
/// gets only their own). `auth_method`: an `api_key` identity is never a
/// trainer, whatever `trainers.groups`/`trainers.users` says (#548 item 3
/// nit) — a machine credential reading every viewer's answers is a
/// materially different exposure than a signed-in trainer's own browser
/// session, the same reasoning `handle_submit` already applies to refuse
/// `api_key` from submitting as anyone.
fn state_trainer_view(
    opts: &StateOptions,
    dash_id: &str,
    claims: Option<&Claims>,
    auth_method: Option<&str>,
) -> Option<bool> {
    if let Some(t) = opts
        .trainers
        .as_ref()
        .filter(|t| t.dashboards.iter().any(|d| d == dash_id))
    {
        let is_trainer = auth_method != Some("api_key")
            && claims.is_some_and(|c| {
                t.users.iter().any(|u| u == &c.user)
                    || c.groups.iter().any(|g| t.groups.iter().any(|tg| tg == g))
            });
        return Some(is_trainer);
    }
    (opts.dashboards.iter().any(|d| d == dash_id) || opts.read.iter().any(|d| d == dash_id))
        .then_some(false)
}

/// The state generation a state-enabled dashboard's cache/warm keys fold in
/// (plan §5 "cache and warm connections") and the exact rows
/// [`seed_viewer_state`] inserts — read together under one lock (#548 item
/// 3 nit: a cache key and the rows actually inserted must never be two
/// different, racing reads of the store; a submission landing between two
/// separate locks could make a key claim a generation the inserted rows
/// don't match). The generation is the viewer's own row count, or the
/// global `seq` for a trainer's all-rows read — append-only storage means
/// rows at or below it never change, so two requests at the same
/// generation always see the same `viewer_state`. `None` for a dashboard
/// [`state_trainer_view`] says isn't state-enabled (the key is unaffected,
/// same as before #517 S3).
///
/// `Arc<Vec<Arc<Record>>>`, not `Vec<Record>` (#548 item 2; review of #599
/// SF1/SF2: `Arc<Record>` elements, not owned `Record`s — the trainer
/// branch is [`anofox_visualization::host::state::Store::snapshot_all`]'s
/// own cache, and keeping a second deep copy of up to `trainer_max_rows`
/// records' content alive in it would just move the cost this exists to
/// avoid). Wrapping the (small, per-viewer) non-trainer branch's rows the
/// same way keeps one return type for both — that branch pays one fresh
/// `Arc` alloc per record, never cached, acceptable for a count that's
/// never more than one viewer's own rows.
///
/// Review of #599 N3: the trainer branch's `submit.store.len()` call
/// below, for the truncation log, takes the store's own lock every call
/// regardless of whether `snapshot_all` just hit or missed its cache —
/// cheap (it only reads a length), but real, so "a cache hit takes no
/// lock on the store at all" ([`anofox_visualization::host::state::Store::
/// snapshot_all`]'s own doc) is true of `snapshot_all` itself, not of this
/// function as a whole.
fn state_snapshot(
    submit: &SubmitRuntime,
    dash_id: &str,
    claims: Option<&Claims>,
    auth_method: Option<&str>,
) -> Option<(u64, Arc<Vec<Arc<Record>>>)> {
    let is_trainer = state_trainer_view(&submit.opts, dash_id, claims, auth_method)?;
    Some(if is_trainer {
        let (gen, rows) = submit
            .store
            .snapshot_all(submit.opts.trainer_max_rows as usize);
        let total = submit.store.len();
        if rows.len() < total {
            eprintln!(
                "[anofox-serve] trainer read-back for '{dash_id}' truncated to the newest {} \
                 of {total} rows (options.state.trainer_max_rows)",
                rows.len()
            );
        }
        (gen, rows)
    } else {
        let user = claims.map(|c| c.user.as_str()).unwrap_or("");
        let (gen, rows) = submit.store.snapshot_for_user(user);
        (gen, Arc::new(rows.into_iter().map(Arc::new).collect()))
    })
}

/// A scalar `vars_out` value (never `Null` or `Array`, both handled by the
/// caller) as the text [`seed_viewer_state`] records in `viewer_state.value`.
fn state_scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        // `Null`/`Array` are handled by the caller before this is reached;
        // a `Map` never appears in a recorded value.
        _ => String::new(),
    }
}

/// Creates and populates the per-request `viewer_state`/`viewer_state_latest`
/// temps (plan §5) on a freshly-opened connection with `rows` ([`state_snapshot`]'s
/// own, read at the same generation [`locked_panel`] already folded into
/// this request's cache/warm key) — called only on the *cold* path of
/// [`run_panel`] (never on a warm reuse: that generation is already part
/// of `warm_key`, so a warm connection is only ever reused while its
/// existing viewer_state content, inserted the same way at the same
/// generation, is still exactly what this would build again).
///
/// Every value goes through `db::Appender`'s typed `append_*` calls (#600)
/// — no SQL text built from `rec`'s own fields at all, so there is
/// nothing here for a value to be spliced into. Review of #611 S2: one
/// exception — a record whose `ts` the strict `parse_rfc3339_micros`
/// refuses (never the server's own output; only a store file written or
/// edited some other way, #611 S1) falls back to
/// [`seed_viewer_state_row_via_sql`], a single `INSERT ... VALUES
/// (?::BIGINT, ?::TIMESTAMPTZ, ...)` per variable for *that one record* —
/// the exact cast the pre-#600 code relied on for every row — rather than
/// failing every panel that reads this generation over one odd record.
/// Review of #614 R2: `rec.ts_micros` is read here, not recomputed —
/// `Record::from_json`/`Store::append` already ran `parse_rfc3339_micros`
/// once, when the record was read off disk or newly appended, and cached
/// the answer. The slow per-variable `INSERT` fallback itself still runs
/// again on every cold panel that seeds an odd record (a cold panel's
/// `viewer_state` is a fresh per-connection `TEMP TABLE`, so there is no
/// way around re-inserting its rows), but this stops re-running the same
/// parse — and reaching the same `None` — on every one of those panels.
///
/// `&[Arc<Record>]`, not `&[Record]` (review of #599 SF1): this only ever
/// reads fields through the `Arc` (auto-deref), so taking the trainer
/// cache's own element type directly — rather than requiring its caller
/// to deep-copy into owned `Record`s first — is free and removes that
/// copy entirely, the point of the review's fix.
fn seed_viewer_state(c: &Conn, rows: &[Arc<Record>]) -> Result<(), String> {
    for stmt in anofox_visualization::sql::viewer_state_setup_statements() {
        c.exec(stmt)?;
    }
    // #600: an `Appender` (`db::Appender`), not `INSERT ... VALUES (…),
    // (…), …` built as SQL text — measured at ~22s per cold panel for a
    // 200k-row trainer store (the whole point of this rewrite; see the
    // PR for the before/after numbers). No text to build, parse or bind:
    // every value here is still a bound value in the sense that matters
    // (never spliced into SQL text), just handed to DuckDB through the
    // Appender's typed `append_*` calls instead.
    // `temp`/`main` explicit (`Appender::new_in`): `viewer_state` is
    // always the per-connection TEMP table `viewer_state_setup_statements`
    // just (re)created above, never left to the connection's own current
    // default catalog to resolve.
    // Review of #611 N4: the more likely reason an appender created
    // without a catalog/schema would miss it is that it resolves against
    // the connection's own *current default database* (whatever `USE`
    // last pointed at, which on a cold panel connection can be one of the
    // snapshot's own `READ_ONLY`-attached databases) rather than doing
    // the same `temp`-first unqualified-name resolution ordinary SQL
    // does — not necessarily a same-named object actually existing
    // there, just resolving to the wrong database before DuckDB even
    // gets as far as checking whether `viewer_state` exists in it.
    let app = crate::db::Appender::new_in(c, Some("temp"), Some("main"), "viewer_state")?;
    for rec in rows {
        // #611 N6 / #614 R2: `rec.ts_micros` was already parsed once,
        // when this record was loaded or appended — not re-parsed here,
        // and not re-parsed once per variable either.
        let ts_micros = rec.ts_micros;
        // #619 N2: `ts`/`ts_micros` must always agree — both are `pub`
        // fields a future edit could set independently (`Record::from_json`/
        // `Store::append` are the only two constructors today, but nothing
        // stops a third from forgetting the pairing). Checked here, not
        // only at the two construction sites, so a desync from *any*
        // future caller shows up the moment it would actually matter
        // (choosing the fast Appender path vs. the slow SQL fallback) —
        // debug-only: `parse_rfc3339_micros` round-trips through
        // `rfc3339()` internally, not free enough to run again on every
        // cold-panel read in a release build.
        debug_assert_eq!(
            ts_micros,
            anofox_visualization::host::access::parse_rfc3339_micros(&rec.ts),
            "Record::ts_micros desynced from Record::ts for seq={}",
            rec.seq
        );
        if ts_micros.is_none() {
            seed_viewer_state_row_via_sql(c, rec)?;
            continue;
        }
        for (name, val) in &rec.vars {
            app.begin_row()?;
            app.append_int64(
                i64::try_from(rec.seq).map_err(|_| "seq out of range for BIGINT".to_string())?,
            )?;
            app.append_timestamp_micros(ts_micros.expect("checked above"))?;
            app.append_varchar(&rec.user)?;
            app.append_varchar(&rec.dashboard)?;
            app.append_varchar(&rec.submit)?;
            app.append_varchar(name)?;
            match val {
                Value::Null => {
                    app.append_null()?; // value
                    app.append_null()?; // value_list
                }
                Value::Array(items) => {
                    // #611 B1: `value` holds the same JSON text of the
                    // whole array the pre-#600 SQL path bound and the
                    // browser's own live preview still writes
                    // (`web/app.js`) — a served dashboard reading
                    // `value` for a list (`MULTISELECT`) answer must see
                    // what it always has, not `NULL`.
                    app.append_varchar(&val.to_string())?; // value
                                                           // A `Null` item is a real SQL `NULL` inside the list,
                                                           // never an empty string (#548 item 3 nit). Owned
                                                           // first (`state_scalar_text` allocates), then
                                                           // borrowed for the call — `append_varchar_list`'s
                                                           // `&str`s only need to live for that one call.
                    let owned: Vec<Option<String>> = items
                        .iter()
                        .map(|it| (!it.is_null()).then(|| state_scalar_text(it)))
                        .collect();
                    let borrowed: Vec<Option<&str>> = owned.iter().map(|s| s.as_deref()).collect();
                    app.append_varchar_list(&borrowed)?;
                }
                other => {
                    app.append_varchar(&state_scalar_text(other))?; // value
                    app.append_null()?; // value_list
                }
            }
            app.end_row()?;
        }
    }
    app.close()
}

/// #611 S2: one record whose `ts` [`seed_viewer_state`]'s fast parser
/// refused, inserted the slow way — one `INSERT ... VALUES (?::BIGINT,
/// ?::TIMESTAMPTZ, ...)` per variable, the exact SQL cast the pre-#600
/// code relied on for every row (so a non-canonical but still valid
/// RFC3339 shape such as a missing `.mmm` or a `+00` offset, which
/// `?::TIMESTAMPTZ` accepts and `parse_rfc3339_micros` by design does
/// not, still seeds correctly) — never spliced SQL text, same as the
/// pre-#600 path. Only reached for a store file written or edited by
/// something other than this server's own [`Store::append`]
/// (`anofox_visualization::host::state::Store::append`); the server
/// never writes a `ts` this falls back for.
///
/// `temp.main.viewer_state` qualified (review of #614 R1), same as
/// `seed_viewer_state`'s own `Appender::new_in(c, Some("temp"),
/// Some("main"), …)` just above it — ordinary SQL already resolves an
/// unqualified `viewer_state` to `temp` first (#548 item 1's own pinned
/// test), so this was never the #611 N4 bug, but naming it explicitly
/// here too means a reader never has to go check that resolution order
/// to see that this inserts into the very `TEMP TABLE`
/// `viewer_state_setup_statements` just (re)created, not some other
/// same-named object.
fn seed_viewer_state_row_via_sql(c: &Conn, rec: &Record) -> Result<(), String> {
    for (name, val) in &rec.vars {
        let mut params: Vec<String> = vec![
            rec.seq.to_string(),
            rec.ts.clone(),
            rec.user.clone(),
            rec.dashboard.clone(),
            rec.submit.clone(),
            name.clone(),
        ];
        let sql = match val {
            Value::Null => {
                "INSERT INTO temp.main.viewer_state (seq, ts, \"user\", dashboard, submit, \
                 name, value, value_list) VALUES (?::BIGINT, ?::TIMESTAMPTZ, ?, ?, ?, ?, NULL, \
                 NULL)"
                    .to_string()
            }
            Value::Array(items) => {
                params.push(val.to_string()); // value: the JSON text (#611 B1)
                let tokens: Vec<&str> = items
                    .iter()
                    .map(|it| {
                        if it.is_null() {
                            "NULL"
                        } else {
                            params.push(state_scalar_text(it));
                            "?"
                        }
                    })
                    .collect();
                format!(
                    "INSERT INTO temp.main.viewer_state (seq, ts, \"user\", dashboard, submit, \
                     name, value, value_list) VALUES (?::BIGINT, ?::TIMESTAMPTZ, ?, ?, ?, ?, ?, \
                     [{}])",
                    tokens.join(", ")
                )
            }
            other => {
                params.push(state_scalar_text(other));
                "INSERT INTO temp.main.viewer_state (seq, ts, \"user\", dashboard, submit, \
                 name, value, value_list) VALUES (?::BIGINT, ?::TIMESTAMPTZ, ?, ?, ?, ?, ?, NULL)"
                    .to_string()
            }
        };
        let refs: Vec<&str> = params.iter().map(String::as_str).collect();
        c.execute_prepared(&sql, &refs)?;
    }
    Ok(())
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
#[allow(clippy::too_many_arguments)]
fn locked_panel(
    st: &Locked,
    body: &str,
    supersede: Option<&str>,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    auth_method: Option<&str>,
    info: &mut LogInfo,
    charge: &mut Charge,
) -> Result<(Arc<String>, String, String), (u16, String)> {
    let r = sv::parse_panel_request(body).map_err(|e| (400, e))?;
    locked_panel_req(st, r, supersede, claims, scope, auth_method, info, charge)
}

/// [`locked_panel`] for a request already parsed and validated (an export,
/// #764, builds its own from the URL).
#[allow(clippy::too_many_arguments)]
fn locked_panel_req(
    st: &Locked,
    mut r: sv::PanelRequest,
    supersede: Option<&str>,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    auth_method: Option<&str>,
    info: &mut LogInfo,
    charge: &mut Charge,
) -> Result<(Arc<String>, String, String), (u16, String)> {
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
    let held = DashHeld::resolve(st, &gen, &r.dashboard)?;
    // #770: a preview runs as the API key that made it — every viewer who
    // may open it (a login not limited to other dashboards) sees what the
    // key's claims allow, with no locked variables.
    let (claims, scope) = match held.preview() {
        Some(p) => {
            if scope.is_some_and(|s| !s.allows(&r.dashboard)) {
                return Err((403, "forbidden: not in this credential's scope".into()));
            }
            (Some(&p.claims), None)
        }
        None => (claims, scope),
    };
    let dash = held.dash(&gen);
    let deps_tag = held.deps_tag(&gen);
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
    clamp_daterange_values(
        &st.opts.nav_pickers,
        &gen.snap.nav_picker_snapshots,
        &mut r.vars,
    )?;
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
    let script = held.script(&gen);
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
    let deps = held
        .deps(&gen)
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
    // #517 S3, plan §5 "cache and warm connections": a state-enabled
    // dashboard's keys fold in the viewer's own generation (or the global
    // one, for a trainer's all-rows read) — a submission misses both this
    // server's cache and any warm connection holding older rows, the same
    // way a changed variable already would. `None` for every other
    // dashboard: unaffected, same keys as before #517. Read once, here
    // (#548 item 3 nit — gen and rows together, under one lock), and
    // carried into `run_panel` as `state_rows`: the rows `seed_viewer_state`
    // inserts on a cache miss are therefore *exactly* the rows this
    // generation names, never a second, separately-locked (and possibly
    // racing) read of the store.
    let state_snap = st
        .submit
        .as_deref()
        .and_then(|s| state_snapshot(s, &dash.id, claims, auth_method));
    let sgen = state_snap.as_ref().map(|(g, _)| *g);
    // `Some(rows)` even when `rows` is empty (a state-enabled dashboard
    // this viewer simply has no rows in yet) vs `None` (not state-enabled
    // at all, no viewer_state temp created) — run_panel must tell these
    // apart.
    let state_rows: Option<Arc<Vec<Arc<Record>>>> = state_snap.map(|(_, rows)| rows);
    // #684 slice 3: a dashboard reading saved tables keys on the change
    // log's generation too (a save misses the cache and warm connections),
    // and the folded tables it carries into run_panel are that generation's.
    let wb_snap = st.writeback.as_deref().and_then(|w| {
        let tables = gen
            .snap
            .dashes
            .dash_tables
            .get(&dash.id)
            .filter(|t| !t.is_empty())?;
        let (g, folded) = w.snapshot();
        Some((g, folded, &w.opts, tables.as_slice()))
    });
    let mut key = panel_key(&r, claims, &key_deps);
    if let Some(g) = sgen {
        key = format!("{key}/state{g}");
    }
    if let Some((g, ..)) = &wb_snap {
        key = format!("{key}/wb{g}");
    }
    let t0 = std::time::Instant::now();
    let mut warm = warm_key(dash, held.warm_deps(&gen), stmts, &r, claims, scope);
    if let Some(g) = sgen {
        warm = warm.map(|w| format!("{w}/state{g}"));
    }
    if let Some((g, ..)) = &wb_snap {
        warm = warm.map(|w| format!("{w}/wb{g}"));
    }
    // #727: the `-- @materialize` statements before the panel, with their keys.
    let mats: Vec<(usize, String)> = (0..r.panel)
        .filter(|&i| dash.materialize.get(i) == Some(&true))
        .filter_map(|i| {
            let k = mat_key(
                dash,
                held.warm_deps(&gen),
                i,
                &r,
                claims,
                scope,
                gen.snap.policy.is_some(),
                sgen.is_some(),
            )?;
            let k = match sgen {
                Some(g) => format!("{k}/state{g}"),
                None => k,
            };
            Some(match &wb_snap {
                Some((g, ..)) => (i, format!("{k}/wb{g}")),
                None => (i, k),
            })
        })
        .collect();
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
            state_rows.as_deref().map(|v| v.as_slice()),
            wb_snap.as_ref().map(|(_, f, o, t)| (*o, &**f, *t)),
            abort,
            charge,
            warm.as_deref(),
            &warmed,
            &mats,
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

/// `POST /api/submit`'s own body cap (plan §4.1: "separate from
/// `max_body`" — a submission is always small; nothing like a table's
/// result ever needs to flow through this endpoint).
const SUBMIT_MAX_BODY: usize = 16 << 10;

/// `429` with `Retry-After` (plan §4.4).
fn rate_limited(req: Request, retry_after_s: u64) -> u16 {
    let resp = Response::from_data(b"too many submissions".to_vec())
        .with_status_code(429)
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Retry-After", &retry_after_s.to_string()));
    let _ = req.respond(resp);
    429
}

/// `409` with `Retry-After: 1` — a second concurrent submission from the
/// same viewer found the per-viewer mutex already held (review of #538
/// round 2): told to retry shortly, never made to park an HTTP worker
/// thread on a stranger's lock (`try_lock`, not `lock`, in
/// [`handle_submit`] — this server has far fewer worker threads than
/// viewers, and one slow submission must never stall everyone else's
/// unrelated `/api/panel` requests).
fn submission_in_progress(req: Request) -> u16 {
    let resp = Response::from_data(b"submission in progress".to_vec())
        .with_status_code(409)
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Retry-After", "1"));
    let _ = req.respond(resp);
    409
}

/// `POST /api/submit` (#517 slice S2, plan §4): the one write path on the
/// locked server — a JSON line Rust appends to [`Store`]; the dashboard's
/// own SQL never writes anything ([`check_read_only`] is unchanged). Follows
/// plan §4.2's numbered steps; each refusal is logged (the caller's
/// `info.deny`) and audited here (never with a value from the request).
#[allow(clippy::too_many_arguments)]
fn handle_submit(
    st: &Locked,
    mut req: Request,
    origin: Option<&str>,
    host: Option<&str>,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    auth_method: Option<&'static str>,
    info: &mut LogInfo,
) -> u16 {
    let peer = req.remote_addr().map(|a| a.ip());
    let audit = |outcome: &'static str,
                 reason: Option<&'static str>,
                 dashboard: Option<&str>,
                 detail: Option<&str>| {
        if let Some(a) = st.opts.auth.as_ref().and_then(|a| a.audit.as_ref()) {
            a.write(&auth::audit::Event {
                event: "submit",
                outcome,
                method: auth_method,
                user: claims.map(|c| c.user.as_str()),
                reason,
                peer,
                path: Some("/api/submit"),
                dashboard,
                key_id: None,
                sid: None,
                detail,
            });
        }
    };

    if let Some(r) = durable_reader(st) {
        info.deny = Some("not_writer");
        return send(req, r);
    }
    let Some(submit) = &st.submit else {
        info.deny = Some("not_enabled");
        audit("deny", Some("not_enabled"), None, None);
        return reply(
            req,
            403,
            "forbidden: this server does not accept submissions (no options.state)",
        );
    };

    // Step 2 (plan §4.3): a write needs a stricter CSRF gate than the
    // read-only panel/page routes — `Origin` is required (not just
    // matching when present), `Sec-Fetch-Site` (when sent) must be
    // same-origin, the body must claim to be JSON, and a custom header
    // that a cross-site `fetch` can't add without a CORS preflight this
    // server never grants.
    if !sv::origin_required_and_matches_host(origin, host) {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(
            req,
            403,
            "forbidden: Origin is required on this endpoint and must match this server",
        );
    }
    if req_header(&req, "Sec-Fetch-Site").is_some_and(|v| !v.eq_ignore_ascii_case("same-origin")) {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(req, 403, "forbidden: Sec-Fetch-Site must be same-origin");
    }
    if !req_header(&req, "Content-Type").is_some_and(|v| v.eq_ignore_ascii_case("application/json"))
    {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(req, 403, "forbidden: Content-Type must be application/json");
    }
    if req_header(&req, "X-Anofox-Submit") != Some("1") {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(req, 403, "forbidden: missing X-Anofox-Submit header");
    }

    // Step 3: identity. A shared credential (an embed link without its own
    // `user`, or an API key) never submits as itself.
    let Some(claims) = claims else {
        return reply(req, 401, access::UNAUTHORIZED);
    };
    if auth_method == Some("api_key") || access::is_reserved_user(&claims.user) {
        info.deny = Some("shared_identity");
        audit("deny", Some("shared_identity"), None, None);
        return reply(req, 403, "forbidden: submissions need a personal sign-in");
    }
    info.user = Some(claims.user.clone());

    let body = match read_body(&mut req, SUBMIT_MAX_BODY) {
        Ok(b) => b,
        Err((code, e)) => return reply(req, code, &e),
    };
    let sub = match sv::parse_submit_request(&body) {
        Ok(s) => s,
        Err(e) => return reply(req, 400, &e),
    };
    info.dashboard = Some(sub.dashboard.clone());
    info.panel = Some(sub.panel);

    // Step 4: the dashboard must exist, be in the credential's scope, and
    // be submit-enabled. Scope before enablement (review of #538, nit): a
    // scoped credential outside its scope must see the same "forbidden"
    // either way, not learn whether a dashboard outside its scope happens
    // to be submit-enabled or not.
    let gen = st.current.get();
    let Some(dash) = gen.snap.dashes.find(&sub.dashboard) else {
        return reply(req, 404, "no such dashboard");
    };
    if let Some(s) = scope {
        if !s.allows(&dash.id) {
            info.deny = Some("scope");
            audit("deny", Some("scope"), Some(&dash.id), None);
            return reply(req, 403, "forbidden: not in this credential's scope");
        }
    }
    if !submit.opts.dashboards.iter().any(|d| d == &sub.dashboard) {
        info.deny = Some("not_enabled");
        audit("deny", Some("not_enabled"), Some(&sub.dashboard), None);
        return reply(
            req,
            403,
            "forbidden: this dashboard does not accept submissions",
        );
    }
    let Some(spec) = dash.submits.get(&sub.panel) else {
        return reply(req, 400, "'panel' does not name a ::SUBMIT statement");
    };

    // Step 5 (store only — rate limits move below the viewer mutex, review
    // of #538 should-fix 7).
    if submit.store.is_failed() {
        audit("error", Some("store_error"), Some(&dash.id), None);
        return reply(
            req,
            503,
            "service unavailable: the state store is unavailable",
        );
    }
    let now = Instant::now();

    // The request's raw `vars` JSON (not yet typed literals): reused as-is
    // for every synthetic `/api/panel`-shaped call below, so
    // `locked_panel` does the `-- @vars` retain / scope lock / path-var /
    // dedup work exactly once, the same way a real panel request would
    // (plan §4.2 step 6) — never duplicated here. `effective_value`
    // (step 9) re-derives the *retained/locked* value per recorded
    // variable from this same raw JSON, rather than trusting it directly.
    let raw_vars: Value = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("vars").cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let panel_body = |panel: usize| -> String {
        serde_json::json!({
            "dashboard": sub.dashboard,
            "panel": panel,
            "vars": raw_vars,
        })
        .to_string()
    };

    // Step 10: the idempotent-replay check, *before* the viewer mutex and
    // *before* any rate limit (review of #538 round 2 — a genuine retry
    // must never pay a rate-limit token, and must never wait on a mutex
    // either: `submit.recent`'s own lock is held only for this lookup,
    // never for the length of a ::SUBMIT query).
    if let Some(prior) = submit.recent.lock().unwrap_or_else(|p| p.into_inner()).get(
        &claims.user,
        &dash.id,
        &sub.id,
        now,
    ) {
        respond(req, 201, "application/json", prior.into_bytes());
        return 201;
    }

    // Step 5 continued: rate limits, now that we know this is (as far as
    // we can tell without the viewer mutex) a genuinely new submission.
    // Refusals here (and above) don't consume a rate-limit token;
    // validation failures (below) do — so a viewer can't use validation
    // as a free oracle loop. The per-user bucket runs *before* the global
    // one (review of #538 B3): checking the global bucket first let one
    // flooding user spend the whole 20/s budget on requests their own
    // limiter would have refused anyway, 429-ing every other viewer in
    // the meantime.
    if let Err(retry_after) = submit.rate.check(&claims.user, now) {
        audit("deny", Some("rate_limited"), Some(&dash.id), None);
        return rate_limited(req, retry_after);
    }
    if !submit
        .global
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take(GLOBAL_SUBMITS_PER_SEC, now)
    {
        audit("deny", Some("rate_limited"), Some(&dash.id), None);
        return rate_limited(req, 1);
    }

    // Step 7: per-viewer mutex, held from here to the append — an author's
    // "only once" SQL gate (zero rows on a second attempt) can't be raced
    // by two tabs between the gate check (step 8) and the write (step 11).
    // `try_lock`, never a blocking `lock` (review of #538 round 2, fixing
    // a new blocker my own earlier instruction introduced): this endpoint
    // runs on a small, shared pool of HTTP worker threads, and a worker
    // parked on a stranger's mutex for as long as one slow ::SUBMIT query
    // takes stalls every *other* viewer's unrelated `/api/panel` request
    // too, not just this viewer's own retries (reproduced live: 12
    // concurrent same-viewer submits against a 5.5s gate made another
    // user's panel request take 27s). A second concurrent request from
    // the same viewer is told to retry shortly instead.
    let lock = submit
        .viewer_locks
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&claims.user);
    // `WouldBlock` (someone else genuinely holds it) is told to retry;
    // `Poisoned` (a panic happened while holding it, elsewhere) is
    // recovered, same as every other mutex in this module — neither
    // leaves the lock permanently unusable for this viewer.
    let _guard = match lock.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::WouldBlock) => {
            audit("deny", Some("in_progress"), Some(&dash.id), None);
            return submission_in_progress(req);
        }
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
    };

    // Re-checked now that the mutex is actually held: the two checks
    // above ran without it, so a racing duplicate (same id) or a racing
    // submission that pushed this viewer over `max_per_viewer` may have
    // completed in between. Both must see the same, mutex-protected state
    // the append itself is about to use.
    if let Some(prior) = submit.recent.lock().unwrap_or_else(|p| p.into_inner()).get(
        &claims.user,
        &dash.id,
        &sub.id,
        now,
    ) {
        respond(req, 201, "application/json", prior.into_bytes());
        return 201;
    }
    if submit.store.user_count(&claims.user) >= submit.opts.max_per_viewer {
        audit("deny", Some("viewer_limit"), Some(&dash.id), None);
        return reply(req, 409, "submission limit reached");
    }

    // Step 8: the ::SUBMIT statement itself, through the normal panel path
    // (cache, single-flight, policy views, tab/requires gates — exactly
    // what rendered the button on the page the viewer is looking at).
    let mut scratch = LogInfo::default();
    let mut charge = st.inflight.charge();
    let rows_json = match locked_panel(
        st,
        &panel_body(sub.panel),
        None,
        Some(claims),
        scope,
        auth_method,
        &mut scratch,
        &mut charge,
    ) {
        Ok((json, _etag, _deps)) => json,
        Err((code, _)) => {
            audit(
                "deny",
                Some("not_available"),
                Some(&dash.id),
                Some(&spec.name),
            );
            return reply(req, if code == 404 { 400 } else { 409 }, "not available");
        }
    };
    let rows: Vec<Map<String, Value>> = match serde_json::from_str(&rows_json) {
        Ok(r) => r,
        Err(_) => {
            audit(
                "error",
                Some("store_error"),
                Some(&dash.id),
                Some(&spec.name),
            );
            return reply(req, 500, "internal error reading the ::SUBMIT row");
        }
    };
    if rows.len() != 1 {
        audit(
            "deny",
            Some("not_available"),
            Some(&dash.id),
            Some(&spec.name),
        );
        return reply(req, 409, "not available");
    }
    let row = &rows[0];
    let Some(vars_text) = row.values().nth(spec.vars_col).and_then(|v| v.as_str()) else {
        audit(
            "deny",
            Some("invalid_value"),
            Some(&dash.id),
            Some(&spec.name),
        );
        return reply(req, 400, "::SUBMIT_VARS did not evaluate to a string");
    };
    let names: Vec<String> = vars_text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() || names.len() > 32 {
        audit(
            "deny",
            Some("invalid_value"),
            Some(&dash.id),
            Some(&spec.name),
        );
        return reply(
            req,
            400,
            "::SUBMIT_VARS names no variables, or more than 32",
        );
    }

    // Each name declared by exactly one input statement of this dashboard
    // (`Dash.inputs`, built once at load — see `load_dashboards`).
    let mut resolved: Vec<(String, usize, InputKind, usize)> = Vec::with_capacity(names.len());
    for n in &names {
        let lower = n.to_ascii_lowercase();
        if !sv::valid_ident(&lower) || access::is_reserved_var(&lower) {
            audit(
                "deny",
                Some("invalid_value"),
                Some(&dash.id),
                Some(&spec.name),
            );
            return reply_invalid_var(req, n, &format!("'{n}' is not a recordable variable name"));
        }
        match dash.inputs.get(&lower) {
            None => {
                audit(
                    "deny",
                    Some("invalid_value"),
                    Some(&dash.id),
                    Some(&spec.name),
                );
                return reply_invalid_var(
                    req,
                    n,
                    &format!("'{n}' is not declared by an input of this dashboard"),
                );
            }
            Some(entries) if entries.len() != 1 => {
                audit(
                    "deny",
                    Some("invalid_value"),
                    Some(&dash.id),
                    Some(&spec.name),
                );
                return reply_invalid_var(
                    req,
                    n,
                    &format!("'{n}' is declared by more than one input statement"),
                );
            }
            Some(entries) => resolved.push((lower, entries[0].0, entries[0].1, entries[0].2)),
        }
    }

    // Step 9: validate each recorded value against what the viewer could
    // actually have set through `/api/panel` — never a new SQL surface.
    // `effective_value` (review of #538 B1) re-applies the dashboard's
    // `-- @vars` allow-list and the credential's locked variables to the
    // raw request value — the value the server would actually `SET`, not
    // necessarily what the viewer sent.
    let mut vars_out = Map::new();
    for (name, stmt_idx, kind, col_idx) in &resolved {
        let sent = match effective_value(name, dash, scope, &raw_vars) {
            Ok(v) => v,
            Err(()) => {
                audit(
                    "deny",
                    Some("invalid_value"),
                    Some(&dash.id),
                    Some(&spec.name),
                );
                return reply_invalid_var(
                    req,
                    name,
                    &format!("'{name}' is not a settable variable on this dashboard"),
                );
            }
        };
        match validate_recorded_value(
            st,
            name,
            *stmt_idx,
            *kind,
            *col_idx,
            sent.as_ref(),
            &panel_body,
            claims,
            scope,
            auth_method,
            submit.opts.text_max_chars,
        ) {
            Ok(v) => {
                vars_out.insert(name.clone(), v);
            }
            Err(msg) => {
                audit(
                    "deny",
                    Some("invalid_value"),
                    Some(&dash.id),
                    Some(&spec.name),
                );
                return reply_invalid_var(req, name, &msg);
            }
        }
    }

    // Step 11: append — fdatasync'd before step 12 acknowledges it.
    let new_rec = NewRecord {
        user: claims.user.clone(),
        auth: auth_method.unwrap_or("").to_string(),
        dashboard: dash.id.clone(),
        submit: spec.name.clone(),
        id: sub.id.clone(),
        vars: vars_out,
    };
    let rec = match submit.store.append(new_rec) {
        Ok(r) => r,
        Err(e) if e.contains("full") => {
            submit.full.store(true, Ordering::Relaxed);
            audit(
                "error",
                Some("store_full"),
                Some(&dash.id),
                Some(&spec.name),
            );
            return reply(req, 507, "insufficient storage: the state store is full");
        }
        Err(e) => {
            audit(
                "error",
                Some("store_error"),
                Some(&dash.id),
                Some(&spec.name),
            );
            eprintln!(
                "[anofox-serve] state store append failed: {}",
                access::diag(&e)
            );
            return reply(
                req,
                503,
                "service unavailable: could not record the submission",
            );
        }
    };
    let gen = submit.store.user_count(&claims.user);
    let body_json = serde_json::json!({"seq": rec.seq, "ts": rec.ts, "gen": gen}).to_string();
    submit
        .recent
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(&claims.user, &dash.id, &sub.id, body_json.clone(), now);
    audit(
        "ok",
        None,
        Some(&dash.id),
        Some(&format!("{} seq={}", spec.name, rec.seq)),
    );
    info.bytes = Some(body_json.len());
    respond(req, 201, "application/json", body_json.into_bytes());
    201
}

/// `POST /api/save` (#684 slice 2): the shared write-back path — a change
/// set Rust validates against `options.writeback`'s declared tables and
/// appends to the change log. Like `/api/submit`: the CSRF gate, a personal
/// sign-in, scope, the dashboard's own `::SAVE` statement re-run as the
/// viewer (it names *what* is saved), the rate limits, a per-writer mutex,
/// idempotent retries. Unlike it: the viewer must be a writer, and the
/// values come from the request's `graph`/`form`, each coerced to its
/// declared column type ([`wb::build_changes`]) — never SQL.
#[allow(clippy::too_many_arguments)]
fn handle_save(
    st: &Locked,
    mut req: Request,
    origin: Option<&str>,
    host: Option<&str>,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    auth_method: Option<&'static str>,
    info: &mut LogInfo,
) -> u16 {
    let peer = req.remote_addr().map(|a| a.ip());
    let audit = |outcome: &'static str,
                 reason: Option<&'static str>,
                 dashboard: Option<&str>,
                 detail: Option<&str>| {
        if let Some(a) = st.opts.auth.as_ref().and_then(|a| a.audit.as_ref()) {
            a.write(&auth::audit::Event {
                event: "save",
                outcome,
                method: auth_method,
                user: claims.map(|c| c.user.as_str()),
                reason,
                peer,
                path: Some("/api/save"),
                dashboard,
                key_id: None,
                sid: None,
                detail,
            });
        }
    };

    if let Some(r) = durable_reader(st) {
        info.deny = Some("not_writer");
        return send(req, r);
    }
    let Some(w) = &st.writeback else {
        info.deny = Some("not_enabled");
        audit("deny", Some("not_enabled"), None, None);
        return reply(
            req,
            403,
            "forbidden: this server does not accept saves (no options.writeback)",
        );
    };

    // The same CSRF gate as `/api/submit` (plan-517 §4.3), the same header.
    if !sv::origin_required_and_matches_host(origin, host) {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(
            req,
            403,
            "forbidden: Origin is required on this endpoint and must match this server",
        );
    }
    if req_header(&req, "Sec-Fetch-Site").is_some_and(|v| !v.eq_ignore_ascii_case("same-origin")) {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(req, 403, "forbidden: Sec-Fetch-Site must be same-origin");
    }
    if !req_header(&req, "Content-Type").is_some_and(|v| v.eq_ignore_ascii_case("application/json"))
    {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(req, 403, "forbidden: Content-Type must be application/json");
    }
    if req_header(&req, "X-Anofox-Submit") != Some("1") {
        info.deny = Some("csrf");
        audit("deny", Some("csrf"), None, None);
        return reply(req, 403, "forbidden: missing X-Anofox-Submit header");
    }

    // A personal sign-in that is one of the writers.
    let Some(claims) = claims else {
        return reply(req, 401, access::UNAUTHORIZED);
    };
    if auth_method == Some("api_key") || access::is_reserved_user(&claims.user) {
        info.deny = Some("shared_identity");
        audit("deny", Some("shared_identity"), None, None);
        return reply(req, 403, "forbidden: saving needs a personal sign-in");
    }
    info.user = Some(claims.user.clone());
    if !w.opts.is_writer(&claims.user, &claims.groups) {
        info.deny = Some("not_writer");
        audit("deny", Some("not_writer"), None, None);
        return reply(req, 403, "forbidden: you may not save on this server");
    }

    let body = match read_body(&mut req, wb::SAVE_MAX_BODY) {
        Ok(b) => b,
        Err((code, e)) => return reply(req, code, &e),
    };
    let sub = match wb::parse_save_request(&body) {
        Ok(s) => s,
        Err(e) => return reply(req, 400, &e),
    };
    info.dashboard = Some(sub.dashboard.clone());
    info.panel = Some(sub.panel);

    // Scope before enablement, as in `/api/submit`.
    let gen = st.current.get();
    let Some(dash) = gen.snap.dashes.find(&sub.dashboard) else {
        return reply(req, 404, "no such dashboard");
    };
    if let Some(s) = scope {
        if !s.allows(&dash.id) {
            info.deny = Some("scope");
            audit("deny", Some("scope"), Some(&dash.id), None);
            return reply(req, 403, "forbidden: not in this credential's scope");
        }
    }
    if !w.opts.enabled_for(&dash.id) {
        info.deny = Some("not_enabled");
        audit("deny", Some("not_enabled"), Some(&dash.id), None);
        return reply(req, 403, "forbidden: this dashboard does not accept saves");
    }
    let Some(spec) = dash.saves.get(&sub.panel) else {
        return reply(req, 400, "'panel' does not name a ::SAVE statement");
    };
    if w.store.is_failed() {
        audit("error", Some("store_error"), Some(&dash.id), None);
        return reply(
            req,
            503,
            "service unavailable: the change log is unavailable",
        );
    }
    let now = Instant::now();

    // A retry of a save that already landed: its own answer, no new token.
    let replay = |w: &WritebackRuntime| {
        w.recent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&claims.user, &dash.id, &sub.id, now)
    };
    if let Some(prior) = replay(w) {
        respond(req, 201, "application/json", prior.into_bytes());
        return 201;
    }
    if let Err(retry_after) = w.rate.check(&claims.user, now) {
        audit("deny", Some("rate_limited"), Some(&dash.id), None);
        return rate_limited(req, retry_after);
    }
    if !w
        .global
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take(GLOBAL_SUBMITS_PER_SEC, now)
    {
        audit("deny", Some("rate_limited"), Some(&dash.id), None);
        return rate_limited(req, 1);
    }
    // Per writer, `try_lock` (never park a worker thread — see handle_submit).
    let lock = w
        .writer_locks
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&claims.user);
    let _guard = match lock.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::WouldBlock) => {
            audit("deny", Some("in_progress"), Some(&dash.id), None);
            return submission_in_progress(req);
        }
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
    };
    if let Some(prior) = replay(w) {
        respond(req, 201, "application/json", prior.into_bytes());
        return 201;
    }

    // The ::SAVE statement itself, through the normal panel path (cache,
    // policy views, tab/requires gates) with the viewer's variables: it
    // names the tables and which edit is saved.
    let raw_vars: Value = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("vars").cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let panel_body = serde_json::json!({
        "dashboard": sub.dashboard,
        "panel": sub.panel,
        "vars": raw_vars,
    })
    .to_string();
    let mut scratch = LogInfo::default();
    let mut charge = st.inflight.charge();
    let rows_json = match locked_panel(
        st,
        &panel_body,
        None,
        Some(claims),
        scope,
        auth_method,
        &mut scratch,
        &mut charge,
    ) {
        Ok((json, _etag, _deps)) => json,
        Err((code, _)) => {
            audit(
                "deny",
                Some("not_available"),
                Some(&dash.id),
                Some(&spec.name),
            );
            return reply(req, if code == 404 { 400 } else { 409 }, "not available");
        }
    };
    let rows: Vec<Map<String, Value>> = serde_json::from_str(&rows_json).unwrap_or_default();
    let [row] = rows.as_slice() else {
        audit(
            "deny",
            Some("not_available"),
            Some(&dash.id),
            Some(&spec.name),
        );
        return reply(req, 409, "not available");
    };
    let cell = |c: Option<usize>| -> &str {
        c.and_then(|c| row.values().nth(c))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    let changes = wb::save_target(
        &w.opts,
        cell(spec.graph_col),
        cell(spec.form_col),
        cell(Some(spec.tables_col)),
    )
    .and_then(|target| wb::build_changes(&w.opts, &target, &sub));
    let changes = match changes {
        Ok(c) => c,
        Err(e) => {
            audit(
                "deny",
                Some("invalid_value"),
                Some(&dash.id),
                Some(&spec.name),
            );
            return reply(req, 400, &e);
        }
    };

    // #736: under an access policy, a save into a table it names is
    // checked as the writer (columns, graph replace, rows seen).
    if st.opts.policy.is_some() {
        let gen = st.current.get();
        match check_save_under_policy(&gen, w, claims, &changes, writeback_schema(&st.opts)) {
            Ok(()) => {}
            Err(PolicyRefusal::Denied(e)) => {
                info.deny = Some("policy");
                audit("deny", Some("policy"), Some(&dash.id), Some(&spec.name));
                return reply(req, 403, &format!("forbidden: {e}"));
            }
            Err(PolicyRefusal::Failed(e)) => {
                eprintln!(
                    "[anofox-serve] save policy check failed in '{}': {}",
                    dash.id,
                    access::diag(&e)
                );
                audit(
                    "error",
                    Some("policy_check"),
                    Some(&dash.id),
                    Some(&spec.name),
                );
                return reply(
                    req,
                    500,
                    "the save couldn't be checked against the access policy",
                );
            }
        }
    }

    // #684 slice 4: optimistic concurrency — a save made on an older
    // generation that would overwrite someone else's newer save is refused
    // (409, saying who saved what); saving again with the newer `base` the
    // 409 returns overwrites knowingly.
    let Some(base) = sub.base else {
        return reply(
            req,
            400,
            "'base' is required: the change log generation the page shows",
        );
    };
    let summary = wb::summary(&w.opts, &changes);
    let commit = w.commit.lock().unwrap_or_else(|p| p.into_inner());
    let (gen_now, records) = w.store.snapshot_all(usize::MAX);
    let since: Vec<Arc<Record>> = records.iter().filter(|r| r.seq > base).cloned().collect();
    let clashes = wb::conflicts(&w.opts, &changes, &since);
    if !clashes.is_empty() {
        drop(commit);
        info.deny = Some("conflict");
        audit("deny", Some("conflict"), Some(&dash.id), Some(&spec.name));
        let body = serde_json::json!({"error": "changed", "gen": gen_now, "changes": clashes});
        let resp = Response::from_data(body.to_string().into_bytes())
            .with_status_code(409)
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("X-Content-Type-Options", "nosniff"))
            .with_header(header("Cache-Control", "no-store"));
        let _ = req.respond(resp);
        return 409;
    }
    // Append — fdatasync'd before the 201.
    let appended = w.store.append(NewRecord {
        user: claims.user.clone(),
        auth: auth_method.unwrap_or("").to_string(),
        dashboard: dash.id.clone(),
        submit: spec.name.clone(),
        id: sub.id.clone(),
        vars: changes,
    });
    drop(commit);
    let rec = match appended {
        Ok(r) => r,
        Err(e) if e.contains("full") => {
            w.full.store(true, Ordering::Relaxed);
            audit(
                "error",
                Some("store_full"),
                Some(&dash.id),
                Some(&spec.name),
            );
            return reply(req, 507, "insufficient storage: the change log is full");
        }
        Err(e) => {
            audit(
                "error",
                Some("store_error"),
                Some(&dash.id),
                Some(&spec.name),
            );
            eprintln!(
                "[anofox-serve] write-back append failed: {}",
                access::diag(&e)
            );
            return reply(req, 503, "service unavailable: could not record the save");
        }
    };
    let body_json = serde_json::json!({"seq": rec.seq, "ts": rec.ts, "gen": rec.seq}).to_string();
    w.recent.lock().unwrap_or_else(|p| p.into_inner()).insert(
        &claims.user,
        &dash.id,
        &sub.id,
        body_json.clone(),
        now,
    );
    audit(
        "ok",
        None,
        Some(&dash.id),
        Some(&format!("{} seq={} {summary}", spec.name, rec.seq)),
    );
    info.bytes = Some(body_json.len());
    respond(req, 201, "application/json", body_json.into_bytes());
    201
}

/// A recorded variable's *effective* value — after the same `-- @vars`
/// allow-list and credential-lock rules `locked_panel` applies before it
/// ever runs SQL (review of #538, blocker 1; plan §4.2 step 9: "the value
/// the server would `SET`, i.e. after `lock_vars`"). `Ok(None)`: the
/// viewer didn't send this variable (and it isn't locked either) — the
/// caller treats that as "answer this question first", the same as
/// before. `Err(())`: this dashboard's `-- @vars` doesn't list `name` at
/// all, so it would never reach a `SET VARIABLE` through *any* path — the
/// caller must refuse recording it outright, never fall back to silently
/// trusting the raw request value.
fn effective_value(
    name: &str,
    dash: &Dash,
    scope: Option<&Scope>,
    raw_vars: &Value,
) -> Result<Option<Value>, ()> {
    if let Some(allowed) = &dash.meta.vars {
        if !allowed.iter().any(|a| a.eq_ignore_ascii_case(name)) {
            return Err(());
        }
    }
    // A credential-locked variable always wins over whatever the viewer
    // sent (`Scope::lock_vars`'s own rule) — recorded as the locked value,
    // never the viewer's. `Scope::vars` stores it as a typed SQL literal
    // (the same shape `sql_literal` builds for every viewer variable);
    // `literal_to_json` is its exact inverse for the scalar forms a
    // locked variable can actually take. `.rev().find(..)`, not `.find(..)`
    // (review of #538 round 2 nit): two spellings of one case-insensitive
    // name are a later-wins tie, the same rule `dedup_last_spelling`
    // applies everywhere else a variable list is built — disagreeing with
    // it here would record a different spelling's value than the one
    // `locked_panel` actually `SET`.
    if let Some(s) = scope {
        if let Some((_, lit)) = s
            .vars
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            return literal_to_json(lit).map(Some).ok_or(());
        }
    }
    Ok(raw_vars.as_object().and_then(|o| {
        o.iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }))
}

/// The exact inverse of [`sv::sql_literal`] — every form it can build for a
/// locked variable: a scalar (`NULL`, `TRUE`/`FALSE`, a quoted string with
/// `''`-doubled quotes, a plain number) *or* its list form (review of #538
/// round 2 nit: a credential locking a `::MULTISELECT` variable's value —
/// `embed::Embed::verify_link`'s own `vars` go through this same
/// `sql_literal`, which lists scalars just as readily as it does one — so
/// `[]::VARCHAR[]`/`[<item>, <item>, …]` is not some case this never
/// actually produces; refusing it would wrongly 400 every submission from
/// a credential that locks a multiselect). `None` for anything else — this
/// stays an exact inverse, not a guess.
fn literal_to_json(lit: &str) -> Option<Value> {
    if lit == "[]::VARCHAR[]" {
        return Some(Value::Array(Vec::new()));
    }
    if let Some(inner) = lit.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let mut items = Vec::new();
        let mut rest = inner;
        loop {
            let (v, tail) = take_one_scalar_literal(rest)?;
            items.push(v);
            let tail = tail.trim_start();
            match tail.strip_prefix(',') {
                Some(t) => rest = t.trim_start(),
                None if tail.is_empty() => break,
                None => return None, // trailing garbage: malformed
            }
        }
        return Some(Value::Array(items));
    }
    take_one_scalar_literal(lit).and_then(|(v, rest)| rest.is_empty().then_some(v))
}

/// One scalar literal (`scalar_literal`'s own forms) at the front of `s` —
/// the parsed value and whatever of `s` comes after it. `s` may hold more
/// list items past this one; a quoted string is scanned to its own closing
/// quote (`''`-escaped), so a literal `, ` inside it is never mistaken for
/// the list's own separator.
fn take_one_scalar_literal(s: &str) -> Option<(Value, &str)> {
    if let Some(rest) = s.strip_prefix("NULL") {
        return Some((Value::Null, rest));
    }
    if let Some(rest) = s.strip_prefix("TRUE") {
        return Some((Value::Bool(true), rest));
    }
    if let Some(rest) = s.strip_prefix("FALSE") {
        return Some((Value::Bool(false), rest));
    }
    if s.starts_with('\'') {
        let mut out = String::new();
        let mut i = 1;
        loop {
            let tail = s.get(i..)?;
            let c = tail.chars().next()?;
            if c == '\'' {
                if tail[1..].starts_with('\'') {
                    out.push('\'');
                    i += 2;
                    continue;
                }
                return Some((Value::String(out), &s[i + 1..]));
            }
            out.push(c);
            i += c.len_utf8();
        }
    }
    // A number: the run of characters `scalar_literal` can put in one
    // (digits, a leading `-`, `.`, `e`/`E` for a float's exponent — never
    // produced today, but harmless to accept on the read side).
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | 'E')))
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let (num, rest) = s.split_at(end);
    // i64, then u64 (a literal `scalar_literal` built from a `u64` JSON
    // number past `i64::MAX` parses as `i64` first and fails — review of
    // #538 round 2 nit — falling straight through to `f64` would silently
    // lose precision on a value that round-trips exactly as a `u64`), then
    // `f64` via `Number::from_f64`, never `Value::from(f64)` directly
    // (review nit): the latter accepts non-finite floats, this fails
    // closed (`None`) on one instead of guessing.
    let v = num
        .parse::<i64>()
        .map(Value::from)
        .ok()
        .or_else(|| num.parse::<u64>().map(Value::from).ok())
        .or_else(|| {
            num.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
        })?;
    Some((v, rest))
}

/// Does `a` (an option's own cell) equal `b` (what the viewer sent)? JSON
/// equality, except two numbers compare by value (`1` and `1.0` are the
/// same option) — `NULL` only matches `NULL` (plan §4.2 step 9).
fn values_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        _ => a == b,
    }
}

/// Is `c` a control character, or one of the bidi-override / zero-width
/// characters that could make submitted text read differently on a
/// trainer's page than what the viewer actually typed (review of #538,
/// nit — `is_control` alone only catches the Cc category)?
fn has_invisible_or_control(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Plan §4.2 step 9: validate one recorded value, `kind`-specific. `name`
/// is used in every error (review of #538 should-fix 5 — it used to name
/// the dashboard instead). `sent` is the *effective* value for this
/// variable ([`effective_value`]; `None` if the viewer didn't set it and
/// it isn't credential-locked either).
///
/// Every kind's own input statement is run first, through the normal
/// panel path, and must return at least one row (review of #538 should-
/// fix 4: previously only `DROPDOWN`/`BUTTONS`/`MULTISELECT` did this, so
/// a value could be recorded for an input hidden behind a tab's `--
/// @requires`, itself gated off, or returning no rows). `DROPDOWN`/
/// `BUTTONS`/`MULTISELECT` then check membership against that row's
/// *input* column (`col_idx` — review of #538 B2: not a row's first
/// column, which could be an `::OPTION_LABEL`/`::HINT` ahead of it).
#[allow(clippy::too_many_arguments)]
fn validate_recorded_value(
    st: &Locked,
    name: &str,
    stmt_idx: usize,
    kind: InputKind,
    col_idx: usize,
    sent: Option<&Value>,
    panel_body: &dyn Fn(usize) -> String,
    claims: &Claims,
    scope: Option<&Scope>,
    auth_method: Option<&'static str>,
    text_max_chars: usize,
) -> Result<Value, String> {
    let mut scratch = LogInfo::default();
    let mut charge = st.inflight.charge();
    let rows_json = locked_panel(
        st,
        &panel_body(stmt_idx),
        None,
        Some(claims),
        scope,
        auth_method,
        &mut scratch,
        &mut charge,
    )
    .map_err(|_| format!("'{name}' is not currently available"))?
    .0;
    let rows: Vec<Map<String, Value>> =
        serde_json::from_str(&rows_json).map_err(|_| format!("internal error reading '{name}'"))?;
    if rows.is_empty() {
        return Err(format!("'{name}' is not currently available"));
    }
    let options = || -> Vec<Value> {
        rows.iter()
            .filter_map(|r| r.values().nth(col_idx).cloned())
            .collect()
    };
    match kind {
        InputKind::Dropdown | InputKind::Buttons => {
            let Some(sent) = sent else {
                return Err(format!("answer '{name}' first"));
            };
            let options = options();
            if options.iter().any(|o| values_eq(o, sent)) {
                Ok(sent.clone())
            } else {
                Err(format!("'{name}' is not one of the options"))
            }
        }
        InputKind::Multiselect => {
            let Some(sent) = sent.and_then(Value::as_array) else {
                return Err(format!("answer '{name}' first (a list expected)"));
            };
            let options = options();
            if sent.len() > options.len() {
                return Err(format!("'{name}' has too many selected options"));
            }
            let mut seen = Vec::new();
            for v in sent {
                if seen.iter().any(|s| values_eq(s, v)) {
                    return Err(format!("'{name}' has a duplicate option in the selection"));
                }
                if !options.iter().any(|o| values_eq(o, v)) {
                    return Err(format!("'{name}': the selection is not among the options"));
                }
                seen.push(v.clone());
            }
            Ok(Value::Array(sent.clone()))
        }
        InputKind::Toggle => match sent {
            Some(Value::Bool(b)) => Ok(Value::Bool(*b)),
            None => Err(format!("answer '{name}' first")),
            _ => Err(format!("'{name}' must be a boolean")),
        },
        InputKind::Number => match sent {
            // Kept exactly as sent (review of #538 should-fix 6):
            // rebuilding the number via `Number::from_f64` turned an
            // integer like `5` into `5.0`, and could round one above
            // 2^53.
            Some(Value::Number(n))
                if n.as_f64().is_some_and(|f| f.is_finite() && f.abs() <= 1e15) =>
            {
                Ok(Value::Number(n.clone()))
            }
            None => Err(format!("answer '{name}' first")),
            _ => Err(format!("'{name}' must be a finite number")),
        },
        InputKind::Date => match sent.and_then(Value::as_str) {
            Some(s) if sv::valid_iso_date(s) => Ok(Value::String(s.to_string())),
            None => Err(format!("answer '{name}' first")),
            _ => Err(format!("'{name}' must be a YYYY-MM-DD date")),
        },
        InputKind::Text => match sent {
            Some(Value::String(s)) => {
                if s.chars().count() > text_max_chars {
                    Err(format!(
                        "'{name}' must be at most {text_max_chars} characters"
                    ))
                } else if s.chars().any(has_invisible_or_control) {
                    Err(format!(
                        "'{name}' must not contain control or invisible characters"
                    ))
                } else if s.trim().is_empty() {
                    Err(format!("'{name}' must not be blank"))
                } else {
                    Ok(Value::String(s.clone()))
                }
            }
            None => Ok(Value::Null),
            _ => Err(format!("'{name}' must be a string")),
        },
        // `Dash.inputs` never registers these (#517 S2: see
        // `load_dashboards`), so `SUBMIT_VARS` naming one already failed
        // the "declared by an input" check above — unreachable in
        // practice, kept fail-closed rather than `unreachable!()`.
        InputKind::DateRange | InputKind::Period | InputKind::Form => {
            Err(format!("'{name}' does not support recording"))
        }
    }
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
    state_rows: Option<&[Arc<Record>]>,
    wb_tables: Option<(&WritebackOptions, &Folded, &[String])>,
    abort: &crate::panel_cache::Abort,
    charge: &mut Charge,
    warm_key: Option<&str>,
    warmed: &AtomicBool,
    mats: &[(usize, String)],
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
        let until = Instant::now() + st.opts.timeout;
        c.exec(&gen.snap.enter)?;
        for (_, set) in &sets {
            c.exec(set)?;
        }
        // #517 S3, plan §5: viewer_state/viewer_state_latest, before the
        // policy's views and any setup statement, so every later statement
        // (setup, the policy's own row filters, the panel itself) can read
        // them. Cold path only (see seed_viewer_state's own doc comment).
        // `rows` is `locked_panel`'s own `state_snapshot` read, already
        // folded into this request's cache/warm key — never a second,
        // separately-locked read of the store here.
        if let Some(rows) = state_rows {
            seed_viewer_state(c, rows)?;
        }
        // #684 slice 3: the saved tables, the same way (cold path only; the
        // change log's generation is part of this request's keys).
        if let Some((opts, folded, tables)) = wb_tables {
            let none = BTreeMap::new();
            let under_policy = gen.snap.policy.as_ref().map_or(&none, |p| &p.saved);
            seed_writeback(c, opts, folded, tables, under_policy)?;
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
                // #727: a `-- @materialize` statement from its shared result.
                if let Some((_, key)) = mats.iter().find(|(j, _)| *j == i) {
                    if run_materialized(c, &gen.snap.mats, (dash_id, i), key, sql, until, abort)? {
                        continue;
                    }
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

/// The key of `-- @materialize` statement `i`'s shared result (#727) for
/// this request, or `None` when it isn't shared: under an access policy (its
/// views differ per viewer), or with a `-- @requires` up to it. Like
/// [`warm_key`]: the dashboard, the statement, and the request variables the
/// setup up to and including it reads (plus every credential-locked one).
/// The login only when the result can depend on it — a state-enabled
/// dashboard (`viewer_state` is the viewer's own), or setup that reads a
/// claim or can't be analysed — so one run serves every viewer otherwise.
/// The caller appends the state and change-log generations.
#[allow(clippy::too_many_arguments)]
fn mat_key(
    dash: &Dash,
    setup_deps: Option<&Vec<Deps>>,
    i: usize,
    r: &sv::PanelRequest,
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    policy: bool,
    state: bool,
) -> Option<String> {
    if policy || dash.requires.iter().take(i + 1).any(|q| !q.is_empty()) {
        return None;
    }
    let read = setup_deps.and_then(|d| d.get(i + 1)).unwrap_or(&Deps::All);
    let locked =
        |n: &str| scope.is_some_and(|s| s.vars.iter().any(|(l, _)| l.eq_ignore_ascii_case(n)));
    let mut vars: Vec<(String, &String)> = r
        .vars
        .iter()
        .filter(|(n, _)| read.contains(n) || locked(n))
        .map(|(n, lit)| (n.to_ascii_lowercase(), lit))
        .collect();
    vars.sort();
    let login = claims
        .filter(|c| state || read.is_all() || c.variables().iter().any(|(n, _)| read.contains(n)));
    Some(format!(
        "{:?}",
        (login.map(Claims::key), &r.dashboard, i, vars)
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

/// index.html with `window.__served =
/// {id,title,sql,nav,locked,deps,depsTag,pathVars,navPicker}` injected
/// (script-safe JSON: `<` is emitted as `<`, so `</script>` in a dashboard's
/// SQL — or in a `nav_picker` label — cannot break out).
/// The served page of `dash`. `scope` limits the navigation to the
/// dashboards the viewer may read; `session` is an embed link's page session
/// (the page sends it with each request, see `auth::SESSION_HEADER`).
fn served_index(
    st: &Locked,
    dash: &Dash,
    scope: Option<&Scope>,
    session: Option<&str>,
    writer: bool,
) -> Vec<u8> {
    // The nav and what each statement depends on (#210) come from one
    // snapshot (and its dashboards, #772).
    let gen = st.current.get();
    let html = WEB
        .get_file("index.html")
        .map(|f| String::from_utf8_lossy(f.contents()).into_owned())
        .unwrap_or_default();
    let mut cfg = serde_json::json!({
        // #771: the origins that may frame this page, for its postMessage
        // bridge (sdk/anofox-embed.js) — concrete origins only.
        "frameAncestors": st.opts.frame_ancestors.as_ref().map(|l| {
            l.iter().filter(|o| o.starts_with("http")).cloned().collect::<Vec<_>>()
        }).unwrap_or_default(),
        "id": dash.id, "title": dash.meta.title, "sql": dash.sql,
        "nav": nav_json(&gen.snap.dashes.list, scope), "locked": true,
        "theme": st.opts.theme,
    });
    if let Some(s) = session {
        cfg["session"] = serde_json::json!(s);
    }
    // #407 part 2: which variables are `path_vars` — config, not secret, so
    // the page can carry their current values on every nav link. Part 3: the
    // nav-bar picker's options, computed once per snapshot (never from the
    // viewer, never per-request) by `build_nav_picker`.
    //
    // Security review of #410, M1: a scoped credential (an embed link, an
    // API key) that locks one of these variables (`Scope::lock_vars`) always
    // wins — the viewer's (or the picker's, or a carried-over nav link's)
    // value is dropped and the locked one set instead (`locked_panel`). A
    // locked `path_vars` name is therefore pointless to carry (its value on
    // the link is simply ignored), and a picker's full option list/bounds
    // would otherwise disclose every other value this credential can never
    // actually choose (e.g. every tenant's model name to a credential locked
    // to one tenant). So: never carry a locked path_vars name, and omit a
    // picker entirely when any variable it sets is locked.
    let path_vars: Vec<&String> = st
        .opts
        .path_vars
        .iter()
        .filter(|p| !var_locked(scope, p))
        .collect();
    if !path_vars.is_empty() {
        cfg["pathVars"] = serde_json::json!(path_vars);
    }
    // What each statement depends on (#210): the page re-requests a panel
    // only when one of its variables changed.
    let nav_pickers: Vec<serde_json::Value> = st
        .opts
        .nav_pickers
        .iter()
        .zip(gen.snap.nav_picker_snapshots.iter())
        .filter(|(np, _)| !np.vars().iter().any(|v| var_locked(scope, v)))
        .map(|(np, snap)| nav_picker_json(np, snap))
        .collect();
    if !nav_pickers.is_empty() {
        cfg["navPickers"] = serde_json::json!(nav_pickers);
    }
    // A file's or a preview's (#770).
    if let Ok(h) = DashHeld::resolve(st, &gen, &dash.id) {
        if let Some(ds) = h.deps(&gen) {
            cfg["deps"] = serde_json::json!(ds.iter().map(Deps::to_json).collect::<Vec<_>>());
            cfg["depsTag"] = serde_json::json!(h.deps_tag(&gen));
        }
    }
    // #517 S2, plan §6 / review of #538 should-fix 8: whether *this*
    // dashboard may submit on this server — S4's button reads it to
    // render disabled-with-reason instead of posting to an endpoint that
    // would just refuse it.
    cfg["state"] = serde_json::json!({
        "enabled": st
            .submit
            .as_ref()
            .is_some_and(|s| s.opts.dashboards.iter().any(|d| d == &dash.id)),
    });
    // #684: whether this dashboard saves on this server, and whether this
    // viewer is one of its writers — the ::SAVE button reads both to render
    // disabled-with-reason instead of posting to an endpoint that refuses.
    cfg["writeback"] = serde_json::json!({
        "enabled": st.writeback.as_ref().is_some_and(|w| w.opts.enabled_for(&dash.id)),
        "writer": writer,
        // #684 slice 4: the generation the page loads at — its saves'
        // `base`, moved on only by its own saves and a 409's answer.
        "gen": st.writeback.as_ref().map(|w| w.store.global_seq()),
    });
    let cfg = json_for_script(&cfg);
    // #403: a served dashboard page has no app bar of its own (it's always
    // embed chrome — header/sidebar/toolbar hidden) and never fetches
    // app-config.json (window.__served is set, so web/app.js's
    // appBrandLoad() skips that fetch), but `options.app`'s own-property
    // settings (right now, only charts_as_tables matters without a bar —
    // the brand fields have nothing to draw) still must reach the page as
    // window.__anofoxApp, the one place web/app.js reads them from here.
    // `AppBrand::script()` is "" when nothing is set, so this is a no-op
    // for a server with no `options.app` at all.
    let app_script = st
        .opts
        .app
        .as_ref()
        .map(AppBrand::script)
        .unwrap_or_default();
    html.replacen("<head>", "<head><base href=\"/\">", 1)
        .replacen(
            "</head>",
            &format!("<script>window.__served={cfg};</script>{app_script}</head>"),
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
    let gen = st.current.get();
    let set = anofox_visualization::theme::resolve(st.opts.theme.as_deref().unwrap_or(""))
        .unwrap_or_else(|_| anofox_visualization::theme::resolve("").expect("the default theme"));
    anofox_visualization::page::landing(&anofox_visualization::page::Landing {
        theme: &set,
        app: st.opts.app.as_ref(),
        items: gen
            .snap
            .dashes
            .list
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
mod export_tests {
    use super::set_default;

    #[test]
    fn only_plain_literal_defaults_become_variables() {
        let ok = |s: &str| set_default(s);
        assert_eq!(
            ok("SET VARIABLE region = 'EU'"),
            Some(("region".into(), "'EU'".into()))
        );
        assert_eq!(
            ok("set variable n = -2.5;"),
            Some(("n".into(), "-2.5".into()))
        );
        assert_eq!(
            ok("SET VARIABLE picks = ['a', 'b''c']"),
            Some(("picks".into(), "['a', 'b''c']".into()))
        );
        assert_eq!(
            ok("SET VARIABLE b = TRUE"),
            Some(("b".into(), "TRUE".into()))
        );
        assert_eq!(
            ok("SET VARIABLE \"region\" = 'EU'"),
            Some(("region".into(), "'EU'".into()))
        );
        for bad in [
            "SET VARIABLE x = (SELECT secret FROM t)",
            "SET VARIABLE x = getvariable('y')",
            "SET VARIABLE __claim_user = 'admin'",
            "SET VARIABLE anofox_data = '/'",
            "SET VARIABLE a b = 1",
            "SET VARIABLE \"__claim_user\" = 'admin'",
            "SET VARIABLE \"a\"\"b\" = 1",
            "SET threads = 1",
            "CREATE TABLE t AS SELECT 1",
            "SET VARIABLE x = ",
            "SET VARIABLE x = 'a'; DROP TABLE t",
        ] {
            assert_eq!(set_default(bad), None, "{bad}");
        }
    }
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

    // `/check` (#199) itself needs a real DuckDB connection (`capi!` panics
    // "C API not initialised" outside a host-loaded extension, same as every
    // other DB-touching path here — `authoring_query` included) — covered
    // end-to-end by duckext/scripts/test-check-live.sh against the built
    // extension instead of a `cargo test` unit test. `run_check`'s own
    // request-parsing is pure and tested right here.

    /// `/check`'s body must name a script; a missing or wrong-typed field is
    /// a clear error ahead of ever touching a connection, not a panic or a
    /// silent empty check.
    #[test]
    fn run_check_body_needs_a_script_string() {
        // No live connection is reachable in a unit test (see above), so
        // these must fail on the body itself, before `scratch_check` runs.
        let err = |body: &str| match serde_json::from_str::<Value>(body) {
            Ok(v) => v
                .get("script")
                .and_then(Value::as_str)
                .ok_or_else(|| "\"script\" (a string) is required".to_string())
                .map(|_| ()),
            Err(e) => Err(format!("bad JSON body: {e}")),
        };
        assert!(err("{}").is_err());
        assert!(err(r#"{"script": 1}"#).is_err());
        assert!(err("not json").is_err());
        assert!(err(r#"{"script": "SELECT 1;"}"#).is_ok());
    }

    fn dash(id: &str, script: &str) -> Dash {
        Dash {
            id: id.into(),
            sql: script.into(),
            stmts: vec![],
            info: vec![],
            requires: vec![],
            materialize: vec![],
            tabs: vec![],
            meta: sv::parse_dashboard_meta(id, script),
            updated: None,
            inputs: HashMap::new(),
            submits: HashMap::new(),
            saves: HashMap::new(),
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
        let (dashboards, _) = load_dashboards(&dir, false).unwrap();
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

    /// #407 part 3: `options.nav_picker` config validation — `var` must be
    /// one of `path_vars`, `sql` must be given, unknown keys and an
    /// oversize `label` are refused, and the combination with an access
    /// policy is refused (see the comment on that check in `parse`: the
    /// policy only ever rewrites `dashboards`, never `nav_picker.sql`).
    #[test]
    fn nav_picker_option() {
        let sub = SnapshotDir::create().unwrap();
        let dir = sub.0.canonicalize().unwrap();
        let base = format!(
            r#"{{"data":{},"path_vars":["model"]"#,
            serde_json::json!(dir.to_string_lossy())
        );
        let with = |extra: &str| format!("{base},{extra}}}");

        let o = LockedOptions::parse(Some(&with(
            r#""nav_picker":{"var":"model","sql":"SELECT model_id, title FROM models","label":"Model"}"#,
        )))
        .unwrap();
        assert_eq!(o.nav_pickers.len(), 1);
        match &o.nav_pickers[0] {
            NavPicker::Dropdown { var, sql, label } => {
                assert_eq!(var, "model");
                assert_eq!(sql, "SELECT model_id, title FROM models");
                assert_eq!(label, &Some("Model".to_string()));
            }
            NavPicker::DateRange { .. } => panic!("expected a dropdown"),
        }

        // `label` is optional.
        let o = LockedOptions::parse(Some(&with(
            r#""nav_picker":{"var":"model","sql":"SELECT 1, 2"}"#,
        )))
        .unwrap();
        assert!(o.nav_pickers[0].label().is_none());

        // `var` must be one of `path_vars` …
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"var":"other","sql":"SELECT 1,2"}"#
        )))
        .is_err());
        // … and a valid identifier in the first place.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"var":"not a var","sql":"SELECT 1,2"}"#,
        )))
        .is_err());
        // `sql` is required and may not be empty.
        assert!(
            LockedOptions::parse(Some(&with(r#""nav_picker":{"var":"model","sql":""}"#))).is_err()
        );
        assert!(LockedOptions::parse(Some(&with(r#""nav_picker":{"var":"model"}"#))).is_err());
        // Unknown key inside `nav_picker`.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"var":"model","sql":"SELECT 1,2","extra":1}"#,
        )))
        .is_err());
        // `label` is capped at 100 characters.
        let long = "x".repeat(101);
        assert!(LockedOptions::parse(Some(&with(&format!(
            r#""nav_picker":{{"var":"model","sql":"SELECT 1,2","label":"{long}"}}"#
        ))))
        .is_err());
        // `nav_picker` needs `path_vars` to actually contain `var` (no
        // `path_vars` at all is the same refusal as `var` not being in it).
        assert!(LockedOptions::parse(Some(&format!(
            r#"{{"data":{},"nav_picker":{{"var":"model","sql":"SELECT 1,2"}}}}"#,
            serde_json::json!(dir.to_string_lossy())
        )))
        .is_err());

        // Refuses to start together with an access policy (it would bypass
        // the policy's row/column rewrite for whatever table it reads).
        let f = dir.join(format!("policy-{}.json", sv::random_token()));
        std::fs::write(&f, r#"{"tables": {"sales": {"rows": true}}}"#).unwrap();
        let with_policy = format!(
            r#"{{"data":{},"path_vars":["model"],"policy":{},"nav_picker":{{"var":"model","sql":"SELECT 1,2"}}}}"#,
            serde_json::json!(dir.to_string_lossy()),
            serde_json::json!(f.to_string_lossy())
        );
        assert!(LockedOptions::parse(Some(&with_policy)).is_err());
        // `"policy": "none"` is not an access policy — no such refusal.
        let with_policy_none = format!(
            r#"{{"data":{},"path_vars":["model"],"policy":"none","nav_picker":{{"var":"model","sql":"SELECT 1,2"}}}}"#,
            serde_json::json!(dir.to_string_lossy())
        );
        assert!(LockedOptions::parse(Some(&with_policy_none)).is_ok());
    }

    /// #556: `options.nav_picker` and `options.nav_pickers` may not both be
    /// set, `nav_pickers` draws several controls left to right (order
    /// preserved), an untyped entry is a dropdown (same shape as the
    /// singular key), and the list is capped.
    #[test]
    fn nav_pickers_list() {
        let sub = SnapshotDir::create().unwrap();
        let dir = sub.0.canonicalize().unwrap();
        let base = format!(
            r#"{{"data":{},"path_vars":["model","view_from","view_to"]"#,
            serde_json::json!(dir.to_string_lossy())
        );
        let with = |extra: &str| format!("{base},{extra}}}");

        let o = LockedOptions::parse(Some(&with(
            r#""nav_pickers":[
                {"type":"daterange","vars":["view_from","view_to"],"label":"Time window"},
                {"var":"model","sql":"SELECT 1,2","label":"Model"}
            ]"#,
        )))
        .unwrap();
        assert_eq!(o.nav_pickers.len(), 2);
        assert!(matches!(o.nav_pickers[0], NavPicker::DateRange { .. }));
        assert!(matches!(o.nav_pickers[1], NavPicker::Dropdown { .. }));

        // Both keys at once: refused regardless of which comes first.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"var":"model","sql":"SELECT 1,2"},"nav_pickers":[{"var":"model","sql":"SELECT 1,2"}]"#
        )))
        .is_err());

        // More than `MAX_NAV_PICKERS` entries.
        let many = (0..MAX_NAV_PICKERS + 1)
            .map(|_| r#"{"var":"model","sql":"SELECT 1,2"}"#)
            .collect::<Vec<_>>()
            .join(",");
        assert!(LockedOptions::parse(Some(&with(&format!(r#""nav_pickers":[{many}]"#)))).is_err());
        // Exactly the cap is fine.
        let ok_many = (0..MAX_NAV_PICKERS)
            .map(|_| r#"{"var":"model","sql":"SELECT 1,2"}"#)
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            LockedOptions::parse(Some(&with(&format!(r#""nav_pickers":[{ok_many}]"#)))).is_ok()
        );

        // Not an array.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_pickers":{"var":"model","sql":"SELECT 1,2"}"#
        )))
        .is_err());
    }

    /// #556: `"type": "daterange"` picker config — `vars` must be exactly
    /// two distinct variable names, both in `path_vars`; `sql` is optional
    /// but `presets` needs it; an unknown preset or `type` is refused; the
    /// policy conflict applies only when this picker actually has `sql`.
    #[test]
    fn nav_picker_daterange_option() {
        let sub = SnapshotDir::create().unwrap();
        let dir = sub.0.canonicalize().unwrap();
        let base = format!(
            r#"{{"data":{},"path_vars":["view_from","view_to"]"#,
            serde_json::json!(dir.to_string_lossy())
        );
        let with = |extra: &str| format!("{base},{extra}}}");

        let o = LockedOptions::parse(Some(&with(
            r#""nav_picker":{"type":"daterange","vars":["view_from","view_to"],"label":"Time window","sql":"SELECT min(d)::DATE, max(d)::DATE FROM t","presets":["last 7d","all"]}"#,
        )))
        .unwrap();
        match &o.nav_pickers[0] {
            NavPicker::DateRange {
                vars,
                sql,
                presets,
                label,
            } => {
                assert_eq!(vars, &["view_from".to_string(), "view_to".to_string()]);
                assert_eq!(
                    sql.as_deref(),
                    Some("SELECT min(d)::DATE, max(d)::DATE FROM t")
                );
                assert_eq!(presets, &["last 7d".to_string(), "all".to_string()]);
                assert_eq!(label.as_deref(), Some("Time window"));
            }
            NavPicker::Dropdown { .. } => panic!("expected a daterange"),
        }

        // `sql` is optional without `presets`.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"type":"daterange","vars":["view_from","view_to"]}"#
        )))
        .is_ok());

        // `vars` must have exactly two entries …
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"type":"daterange","vars":["view_from"]}"#
        )))
        .is_err());
        // … distinct …
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"type":"daterange","vars":["view_from","view_from"]}"#
        )))
        .is_err());
        // … and each one of `path_vars` (a daterange var not in path_vars).
        assert!(LockedOptions::parse(Some(&format!(
            r#"{{"data":{},"path_vars":["view_from"],"nav_picker":{{"type":"daterange","vars":["view_from","view_to"]}}}}"#,
            serde_json::json!(dir.to_string_lossy())
        )))
        .is_err());

        // `presets` without `sql`.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"type":"daterange","vars":["view_from","view_to"],"presets":["all"]}"#
        )))
        .is_err());
        // Unknown preset name.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"type":"daterange","vars":["view_from","view_to"],"sql":"SELECT 1,2","presets":["last week"]}"#
        )))
        .is_err());
        // Unknown picker type.
        assert!(LockedOptions::parse(Some(&with(
            r#""nav_picker":{"type":"weekpicker","vars":["view_from","view_to"]}"#
        )))
        .is_err());

        // Policy conflict only when this picker has `sql`.
        let f = dir.join(format!("policy-{}.json", sv::random_token()));
        std::fs::write(&f, r#"{"tables": {"sales": {"rows": true}}}"#).unwrap();
        let policy = serde_json::json!(f.to_string_lossy());
        assert!(LockedOptions::parse(Some(&format!(
            r#"{{"data":{},"path_vars":["view_from","view_to"],"policy":{policy},"nav_picker":{{"type":"daterange","vars":["view_from","view_to"],"sql":"SELECT 1,2"}}}}"#,
            serde_json::json!(dir.to_string_lossy())
        )))
        .is_err());
        assert!(LockedOptions::parse(Some(&format!(
            r#"{{"data":{},"path_vars":["view_from","view_to"],"policy":{policy},"nav_picker":{{"type":"daterange","vars":["view_from","view_to"]}}}}"#,
            serde_json::json!(dir.to_string_lossy())
        )))
        .is_ok());
    }

    /// #556: [`clamp_daterange_values`] — the request-time half of a
    /// daterange picker: a real-date check beyond `check_path_values`'s
    /// generic plain-name shape check, `from <= to` when both halves of a
    /// request carry a value, and clamping (not refusing) an out-of-range
    /// value to the picker's own `[min, max]`.
    #[test]
    fn clamp_daterange_values_test() {
        let np = vec![NavPicker::DateRange {
            vars: ["view_from".to_string(), "view_to".to_string()],
            sql: Some("SELECT 1,2".to_string()),
            presets: Vec::new(),
            label: None,
        }];
        let min = parse_iso_date("2026-01-01").unwrap();
        let max = parse_iso_date("2026-01-31").unwrap();
        let snap = vec![PickerSnapshot::DateRange {
            min: Some(min),
            max: Some(max),
            presets: Vec::new(),
        }];

        // Garbage date: not a real calendar day.
        let mut vars = vec![
            ("view_from".to_string(), "'2026-02-30'".to_string()),
            ("view_to".to_string(), "'2026-01-15'".to_string()),
        ];
        assert!(clamp_daterange_values(&np, &snap, &mut vars).is_err());

        // Wrong shape (not a date at all, e.g. SQL smuggled in).
        let mut vars = vec![(
            "view_from".to_string(),
            "'2026-01-01'' OR 1=1 --'".to_string(),
        )];
        assert!(clamp_daterange_values(&np, &snap, &mut vars).is_err());

        // from > to.
        let mut vars = vec![
            ("view_from".to_string(), "'2026-01-20'".to_string()),
            ("view_to".to_string(), "'2026-01-10'".to_string()),
        ];
        assert!(clamp_daterange_values(&np, &snap, &mut vars).is_err());

        // Clamped up to `min`, clamped down to `max`.
        let mut vars = vec![
            ("view_from".to_string(), "'2025-12-01'".to_string()),
            ("view_to".to_string(), "'2026-02-15'".to_string()),
        ];
        clamp_daterange_values(&np, &snap, &mut vars).unwrap();
        assert_eq!(vars[0].1, "'2026-01-01'");
        assert_eq!(vars[1].1, "'2026-01-31'");

        // In range: untouched.
        let mut vars = vec![
            ("view_from".to_string(), "'2026-01-05'".to_string()),
            ("view_to".to_string(), "'2026-01-10'".to_string()),
        ];
        clamp_daterange_values(&np, &snap, &mut vars).unwrap();
        assert_eq!(vars[0].1, "'2026-01-05'");
        assert_eq!(vars[1].1, "'2026-01-10'");

        // Only one half present: no from<=to comparison, still date-checked.
        let mut vars = vec![("view_to".to_string(), "'not-a-date'".to_string())];
        assert!(clamp_daterange_values(&np, &snap, &mut vars).is_err());

        // A var this picker doesn't own is left alone.
        let mut vars = vec![("other".to_string(), "'garbage'".to_string())];
        assert!(clamp_daterange_values(&np, &snap, &mut vars).is_ok());
    }

    /// #556 nit (review-582): a NULL value (the nav picker's own "Clear",
    /// or any viewer/credential value that's just NULL) is "no value" —
    /// skipped by the date check and the clamp, same as before — but is
    /// rewritten to `NULL::VARCHAR`, not left as the bare `NULL` it came
    /// in as. A bare `SET VARIABLE x = NULL` gives DuckDB's own default
    /// (INTEGER) type, and every dashboard reading a daterange var does
    /// `coalesce(getvariable(x), '…')::VARCHAR` — untyped, that 400s with
    /// a DuckDB conversion error instead of ever reaching the author's
    /// SQL (caught by an actual browser round trip through the "Clear"
    /// control, not by this unit test alone — see scripts/served-e2e.mjs).
    #[test]
    fn clamp_daterange_values_retypes_a_null_to_varchar() {
        let np = vec![NavPicker::DateRange {
            vars: ["view_from".to_string(), "view_to".to_string()],
            sql: Some("SELECT 1,2".to_string()),
            presets: Vec::new(),
            label: None,
        }];
        let min = parse_iso_date("2026-01-01").unwrap();
        let max = parse_iso_date("2026-01-31").unwrap();
        let snap = vec![PickerSnapshot::DateRange {
            min: Some(min),
            max: Some(max),
            presets: Vec::new(),
        }];

        // Both NULL ("Clear"): both retyped, no date/from<=to check applies.
        let mut vars = vec![
            ("view_from".to_string(), "NULL".to_string()),
            ("view_to".to_string(), "NULL".to_string()),
        ];
        clamp_daterange_values(&np, &snap, &mut vars).unwrap();
        assert_eq!(vars[0].1, "NULL::VARCHAR");
        assert_eq!(vars[1].1, "NULL::VARCHAR");

        // One NULL, one a real date: the NULL side is retyped, the date
        // side is checked/clamped as usual, and `from <= to` is skipped
        // (only one side present).
        let mut vars = vec![
            ("view_from".to_string(), "NULL".to_string()),
            ("view_to".to_string(), "'2025-06-01'".to_string()),
        ];
        clamp_daterange_values(&np, &snap, &mut vars).unwrap();
        assert_eq!(vars[0].1, "NULL::VARCHAR");
        assert_eq!(vars[1].1, "'2026-01-01'"); // clamped up to min
    }

    /// #407 part 3: [`nav_picker_rows_to_options`] — the pure row-to-option
    /// mapping `build_nav_picker` runs after fetching `nav_picker.sql`'s
    /// rows: wrong column count refuses the whole result, a NULL or
    /// not-plain-name value is dropped (and the row's label with it), an
    /// empty/NULL label falls back to the value, an oversize result refuses,
    /// and a label is capped in characters (not bytes — a multi-byte label
    /// must not panic).
    #[test]
    fn nav_picker_rows_to_options_test() {
        let row = |v: Option<&str>, l: Option<&str>| vec![v.map(String::from), l.map(String::from)];
        let rows = vec![
            row(Some("car-price"), Some("Car price")),
            row(Some("../etc"), Some("escape attempt — dropped")), // not a plain name
            row(Some(""), Some("empty value — dropped")),
            row(None, Some("NULL value — dropped")),
            row(Some("no-label"), None), // falls back to the value
            row(Some("empty-label"), Some("  ")),
            row(Some("123"), Some("an integer value is fine too")),
        ];
        let out = nav_picker_rows_to_options(&rows).unwrap();
        assert_eq!(
            out,
            vec![
                ("car-price".to_string(), "Car price".to_string()),
                ("no-label".to_string(), "no-label".to_string()),
                ("empty-label".to_string(), "empty-label".to_string()),
                (
                    "123".to_string(),
                    "an integer value is fine too".to_string()
                ),
            ]
        );
        // Wrong column count refuses the whole thing, not just that row.
        assert!(nav_picker_rows_to_options(&[vec![Some("a".to_string())]]).is_err());
        assert!(nav_picker_rows_to_options(&[vec![
            Some("a".to_string()),
            Some("b".to_string()),
            Some("c".to_string())
        ]])
        .is_err());
        // Too many rows for the page payload.
        let many: Vec<_> = (0..NAV_PICKER_MAX_OPTIONS + 1)
            .map(|i| row(Some(&i.to_string()), Some("x")))
            .collect();
        assert!(nav_picker_rows_to_options(&many).is_err());
        assert_eq!(
            nav_picker_rows_to_options(&many[..NAV_PICKER_MAX_OPTIONS])
                .unwrap()
                .len(),
            NAV_PICKER_MAX_OPTIONS
        );
        // A label longer than the cap is truncated in characters, not bytes
        // — a multi-byte character right at the boundary must not panic.
        let wide_label = "é".repeat(sv::NAV_PICKER_MAX_LABEL + 10);
        let out = nav_picker_rows_to_options(&[row(Some("v"), Some(&wide_label))]).unwrap();
        assert_eq!(out[0].1.chars().count(), sv::NAV_PICKER_MAX_LABEL);
    }

    /// Security review of #410, M1: `var_locked` is what `served_index` asks
    /// before carrying a `path_vars` name on a nav link or showing
    /// `nav_picker` at all — a scoped credential's locked variable (an embed
    /// link) always overrides whatever the page would otherwise send
    /// (`Scope::lock_vars`, `locked_panel`), so neither is worth exposing
    /// for it: carrying a locked value is a no-op, and showing the full
    /// `nav_picker` option list to a credential that can only ever end up
    /// with one of them discloses the others (every tenant's model name to
    /// a credential locked to one tenant, say).
    #[test]
    fn var_locked_matches_scoped_credentials_case_insensitively() {
        assert!(!var_locked(None, "model"));
        let scope = Scope {
            dashboards: Some(vec!["d".into()]),
            vars: vec![("Model".into(), "'acme'".into())],
        };
        assert!(var_locked(Some(&scope), "model"));
        assert!(var_locked(Some(&scope), "MODEL"));
        assert!(var_locked(Some(&scope), "Model"));
        assert!(!var_locked(Some(&scope), "other"));
        // A scope with no locked variables at all (e.g. an api_keys
        // credential, which only ever scopes `dashboards`) locks nothing.
        let dashboards_only = Scope {
            dashboards: Some(vec!["d".into()]),
            vars: Vec::new(),
        };
        assert!(!var_locked(Some(&dashboards_only), "model"));
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

    /// Security review of #410, L1: `wrap_nav_picker_sql`'s closing `)` is
    /// on its own line, so a `nav_picker.sql` ending in a `-- comment` (a
    /// line comment with no newline of its own) can't swallow it — checked
    /// structurally (no DuckDB needed) and, where a `duckdb` CLI is
    /// available, by actually running the wrapped query.
    #[test]
    fn nav_picker_wrapped_sql_survives_a_trailing_line_comment() {
        let sql = "SELECT 1 -- trailing comment, no newline of its own";
        let wrapped = wrap_nav_picker_sql(sql);
        assert!(
            wrapped.contains("\n)"),
            "the wrapper's closing paren must be on its own line: {wrapped}"
        );
        assert!(wrapped.trim_end().ends_with("AS __anofox_nav_picker"));
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
        assert_eq!(
            cli_rows(&wrapped).as_deref(),
            Ok(&[vec![Some("1".to_string())]][..]),
            "the trailing comment must not comment out the wrapper's closing paren"
        );
        // A comment on its own line (a real-world multi-line query) already
        // worked before this fix — still does.
        let multiline =
            "SELECT model_id AS value, title AS label -- a real column comment\nFROM models";
        assert!(cli_rows(&format!(
            "CREATE TEMP TABLE models AS SELECT 'm1' AS model_id, 'Model one' AS title; {}",
            wrap_nav_picker_sql(multiline)
        ))
        .is_ok());
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

    // too_broad's own test moved with it to
    // anofox_visualization::host::serving (#417).

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

    // #726/#736: which saved tables an access policy covers.
    #[test]
    fn writeback_policy_cover_maps_saved_tables_the_policy_names() {
        let wb_opts = WritebackOptions::parse(&serde_json::json!({
            "path": "/x/changes.jsonl",
            "dashboards": ["plan"],
            "writers": {"users": ["alice"]},
            "tables": {
                "plan_nodes": {"key": ["id"], "columns": {"id": "VARCHAR"}},
                "Plan_Params": {"key": ["id"], "columns": {"id": "VARCHAR"}}
            }
        }))
        .unwrap();
        let policy = |t: &str| {
            Policy::parse(&format!(r#"{{"tables": {{"{t}": {{"rows": true}}}}}}"#)).unwrap()
        };
        assert!(writeback_policy_cover(&wb_opts, &policy("orders"), "main")
            .unwrap()
            .is_empty());
        let c = writeback_policy_cover(&wb_opts, &policy("plan_params"), "main").unwrap();
        assert_eq!(
            c.into_iter().collect::<Vec<_>>(),
            vec![(
                "Plan_Params".to_string(),
                "__anofox_wb_plan_params".to_string()
            )]
        );
        // named in another schema than saved tables live in: ambiguous
        let e = writeback_policy_cover(&wb_opts, &policy("other.plan_nodes"), "main").unwrap_err();
        assert!(e.contains("schema 'other'"), "{e}");
    }

    // #727: a `-- @materialize` result's key.
    #[test]
    fn mat_key_tracks_what_shapes_the_result() {
        let d = warm_dash(3);
        let deps = |v: &[&str]| Deps::Vars(v.iter().map(|s| s.to_string()).collect());
        // Statement 0 reads `n`; the setup up to statement 1 is what the key needs.
        let setup = vec![deps(&[]), deps(&["n"]), deps(&["n"])];
        let alice = Claims {
            user: "alice".into(),
            groups: vec![],
            attrs: vec![],
        };
        let bob = Claims {
            user: "bob".into(),
            ..alice.clone()
        };
        let key = |r: &str, c: Option<&Claims>, policy: bool, state: bool| {
            mat_key(&d, Some(&setup), 0, &warm_req(2, r), c, None, policy, state)
        };
        let k = key(r#"{"n":3,"m":1}"#, None, false, false);
        assert!(k.is_some());
        // A variable only the panels read doesn't change it; one it reads does.
        assert_eq!(k, key(r#"{"n":3,"m":2}"#, None, false, false));
        assert_ne!(k, key(r#"{"n":4,"m":1}"#, None, false, false));
        // One result for every viewer, unless it can depend on who they are.
        assert_eq!(
            key("{}", Some(&alice), false, false),
            key("{}", Some(&bob), false, false)
        );
        assert_ne!(
            key("{}", Some(&alice), false, true),
            key("{}", Some(&bob), false, true)
        );
        let reads_claim = vec![deps(&[]), deps(&["__claim_user"]), deps(&[])];
        assert_ne!(
            mat_key(
                &d,
                Some(&reads_claim),
                0,
                &warm_req(2, "{}"),
                Some(&alice),
                None,
                false,
                false
            ),
            mat_key(
                &d,
                Some(&reads_claim),
                0,
                &warm_req(2, "{}"),
                Some(&bob),
                None,
                false,
                false
            )
        );
        assert_ne!(
            mat_key(
                &d,
                None,
                0,
                &warm_req(2, "{}"),
                Some(&alice),
                None,
                false,
                false
            ),
            mat_key(
                &d,
                None,
                0,
                &warm_req(2, "{}"),
                Some(&bob),
                None,
                false,
                false
            )
        );
        // Never under a policy, nor past a `-- @requires`.
        assert_eq!(key("{}", None, true, false), None);
        let mut gated = warm_dash(3);
        gated.requires[0] =
            anofox_visualization::sql::requires::parse("-- @requires x.parquet\nSELECT 1");
        assert_eq!(
            mat_key(
                &gated,
                Some(&setup),
                0,
                &warm_req(2, "{}"),
                None,
                None,
                false,
                false
            ),
            None
        );
    }

    #[test]
    fn mat_store_evicts_the_least_recently_used_free_entry() {
        let m = MatStore::default();
        let slots: Vec<_> = (0..MAT_CAP).map(|i| m.slot(&format!("k{i}")).0).collect();
        *slots[0].lock().unwrap() = Some("t0".into());
        *slots[2].lock().unwrap() = Some("t2".into());
        // k0 used again: k1 is now the oldest, and goes first (no table).
        m.slot("k0");
        assert_eq!(m.slot("new").1, None);
        assert!(!m.holds(&slots[1]) && m.holds(&slots[0]));
        // k2, the oldest now, is being built (locked): k3 goes instead.
        let building = slots[2].lock().unwrap();
        assert_eq!(m.slot("newer").1, None);
        assert!(m.holds(&slots[2]) && !m.holds(&slots[3]));
        drop(building);
        // Free again: evicted with its table, for the caller to drop.
        assert_eq!(m.slot("newest").1.as_deref(), Some("t2"));
    }

    #[test]
    fn sets_variables_is_a_set_or_reset() {
        assert!(sets_variables("SET VARIABLE a = 1"));
        assert!(sets_variables("-- default\n  reset variable a"));
        assert!(!sets_variables("CREATE TEMP TABLE t AS SELECT 1"));
        assert!(!sets_variables("SELECT 'SET'"));
        assert!(!sets_variables("SETTINGS_TABLE"));
    }

    // #775: options.durability.
    #[test]
    fn durability_options() {
        let d = DurabilityOptions::parse(&serde_json::json!({"run": true, "location": "s3://bucket/anofox/", "snapshot_every_s": 60})).unwrap();
        assert_eq!(
            (d.run, d.location.as_str(), d.every.as_secs()),
            (true, "s3://bucket/anofox", 60)
        );
        assert_eq!(d.object("state"), "s3://bucket/anofox/state.jsonl");
        assert!(!d.is_local());
        assert!(
            DurabilityOptions::parse(&serde_json::json!({"location": "/var/lib/anofox/snap"}))
                .unwrap()
                .is_local()
        );
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"location": "relative/dir"}),
            serde_json::json!({"location": "s3://"}),
            serde_json::json!({"location": "s3://b/x'; DROP"}),
            serde_json::json!({"location": "https://x/y"}),
            serde_json::json!({"location": "/x", "nope": 1}),
        ] {
            assert!(DurabilityOptions::parse(&bad).is_err(), "{bad}");
        }
        let e = LockedOptions::parse(Some(r#"{"durability": {"run": true, "location": "/x"}}"#))
            .unwrap_err();
        assert!(
            e.contains("needs options.state or options.writeback"),
            "{e}"
        );
        assert!(
            copy_text_sql("/a'b", "s3://x/y").contains("'/a''b'"),
            "paths are SQL literals"
        );
    }

    // #770: options.preview, and a preview's file round trip.
    #[test]
    fn preview_options_and_files() {
        let o = PreviewOptions::parse(&serde_json::json!({"ttl_s": 999999999, "max_per_key": 3}))
            .unwrap();
        assert_eq!(
            (o.ttl, o.max_per_key, o.dir.is_none()),
            (7 * 86_400, 3, true)
        );
        for bad in [
            serde_json::json!({"nope": 1}),
            serde_json::json!({"ttl_s": 0}),
            serde_json::json!("x"),
        ] {
            assert!(PreviewOptions::parse(&bad).is_err(), "{bad}");
        }
        let e = LockedOptions::parse(Some(r#"{"preview": {}}"#)).unwrap_err();
        assert!(
            e.contains("options.preview needs auth with API keys"),
            "{e}"
        );
        let id = format!("{PREVIEW_PREFIX}{}", sv::random_token());
        let sql = "SELECT 1 AS one ::METRIC;".to_string();
        let p = Preview {
            dash: Arc::new(plan_dash(
                id.clone(),
                sql.clone(),
                sv::parse_dashboard_meta(&id, &sql),
                None,
            )),
            claims: Claims {
                user: "svc".into(),
                groups: vec!["g".into()],
                attrs: vec![("region".into(), Some("EU".into()))],
            },
            key_id: "0123456789ab".into(),
            expires: 1_900_000_000,
        };
        let back = preview_from_json(&preview_to_json(&p)).unwrap();
        assert_eq!(
            (back.dash.id.as_str(), back.dash.sql.as_str()),
            (id.as_str(), sql.as_str())
        );
        assert_eq!(back.claims, p.claims);
        assert_eq!(
            (back.key_id.as_str(), back.expires),
            ("0123456789ab", 1_900_000_000)
        );
        // a file that names anything but a preview id is not one
        let forged = preview_to_json(&p).replace(&id, "sales");
        assert!(preview_from_json(&forged).is_none());
    }

    // #769: options.ingest is only for API keys, so it needs them.
    #[test]
    fn ingest_needs_api_keys() {
        let e = LockedOptions::parse(Some(r#"{"ingest": {"run": true, "database": "inbox"}}"#))
            .unwrap_err();
        assert!(e.contains("options.ingest needs auth with API keys"), "{e}");
        let e = LockedOptions::parse(Some(r#"{"ingest": {"run": true}}"#)).unwrap_err();
        assert!(e.contains("options.ingest.database is required"), "{e}");
    }

    // #773: options.css_file — checked at start, linked last in <head>.
    #[test]
    fn css_file_is_checked_and_linked() {
        let dir = std::env::temp_dir().join(format!("afx-css-{}", sv::random_token()));
        std::fs::create_dir(&dir).unwrap();
        let ok = dir.join("site.css");
        std::fs::write(&ok, ".ax-c-kpi-hero{font-size:1.4em}").unwrap();
        let opts = |f: &std::path::Path| {
            LockedOptions::parse(Some(&serde_json::json!({ "css_file": f }).to_string()))
        };
        let o = opts(&ok).unwrap();
        assert_eq!(
            o.site_css.as_deref(),
            Some(".ax-c-kpi-hero{font-size:1.4em}")
        );
        let (body, _) = with_csp("<head><title>x</title></head><body></body>", &o);
        let body = String::from_utf8(body).unwrap();
        assert!(
            body.contains("<link rel=\"stylesheet\" href=\"/static/site.css\"></head>"),
            "{body}"
        );
        // without the option, no link
        let (plain, _) = with_csp("<head></head>", &LockedOptions::default());
        assert!(!String::from_utf8(plain).unwrap().contains("site.css"));
        // a remote url(), a missing file, a directory: the server doesn't start
        let bad = dir.join("bad.css");
        std::fs::write(&bad, "a{background:url(https://x.example/p.png)}").unwrap();
        let e = opts(&bad).unwrap_err();
        assert!(e.contains("options.css_file") && e.contains("data:"), "{e}");
        assert!(opts(&dir.join("missing.css")).is_err());
        assert!(opts(&dir).is_err());
        assert!(LockedOptions::parse(Some(r#"{"css_file": ""}"#)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // #252: the pages' CSP header.
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
            assert!(sv::valid_frame_ancestor(ok), "{ok}");
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
            assert!(!sv::valid_frame_ancestor(bad), "{bad}");
        }
    }

    // ── #517 S2: /api/submit's own runtime pieces ─────────────────────────

    #[test]
    fn global_bucket_caps_per_second_and_refills() {
        let mut b = GlobalBucket::new();
        let t0 = Instant::now();
        for _ in 0..5 {
            assert!(b.take(5, t0));
        }
        assert!(!b.take(5, t0), "the 6th in the same second must be refused");
        let t1 = t0 + Duration::from_secs(1);
        assert!(b.take(5, t1), "a new second resets the window");
    }

    #[test]
    fn viewer_locks_give_the_same_mutex_back_and_evict_oldest() {
        let mut locks = ViewerLocks::new();
        let a1 = locks.get("alice");
        let a2 = locks.get("alice");
        assert!(Arc::ptr_eq(&a1, &a2), "the same user gets the same mutex");
        let b = locks.get("bob");
        assert!(!Arc::ptr_eq(&a1, &b));
    }

    #[test]
    fn viewer_locks_skips_evicting_a_mutex_someone_else_is_holding() {
        // Review of #538, nit: a bounded eviction must never take the
        // mutex out from under a request that's currently holding it —
        // that would hand the same user a *second*, uncontended mutex,
        // reopening the "only once" race for one moment.
        let mut locks = ViewerLocks::with_capacity(2);
        let alice = locks.get("alice"); // oldest
        let _alice_held = alice.clone(); // strong_count 3: map + `alice` + this
        drop(locks.get("bob")); // map: {alice, bob}, at capacity

        // A third user must evict bob (not alice, who is held) even
        // though alice is the older entry.
        let carol = locks.get("carol");
        assert!(!Arc::ptr_eq(&alice, &carol));
        drop(carol); // not held by anything else from here on

        // alice is still the same mutex as before — not evicted.
        assert!(Arc::ptr_eq(&alice, &locks.get("alice")));

        // bob is gone: asking for "bob" again (now evicting carol, the
        // actual oldest non-held entry) gets a fresh mutex.
        let bob2 = locks.get("bob");
        assert_eq!(Arc::strong_count(&bob2), 2); // map + this handle only
    }

    #[test]
    fn recent_ids_round_trip_and_expire() {
        let mut r = RecentIds::new();
        let t0 = Instant::now();
        assert!(r.get("alice", "d", "id1", t0).is_none());
        r.insert("alice", "d", "id1", "{\"seq\":1}".to_string(), t0);
        assert_eq!(
            r.get("alice", "d", "id1", t0),
            Some("{\"seq\":1}".to_string())
        );
        // A different user, dashboard or id is independent.
        assert!(r.get("bob", "d", "id1", t0).is_none());
        assert!(r.get("alice", "d2", "id1", t0).is_none());
        assert!(r.get("alice", "d", "id2", t0).is_none());
        // Past the TTL, it's gone.
        let later = t0 + RECENT_ID_TTL + Duration::from_secs(1);
        assert!(r.get("alice", "d", "id1", later).is_none());
    }

    #[test]
    fn values_eq_treats_equal_numbers_as_equal_regardless_of_shape() {
        assert!(values_eq(&serde_json::json!(1), &serde_json::json!(1.0)));
        assert!(values_eq(&serde_json::json!("A"), &serde_json::json!("A")));
        assert!(!values_eq(&serde_json::json!("A"), &serde_json::json!("B")));
        assert!(!values_eq(&serde_json::json!(null), &serde_json::json!(0)));
        assert!(values_eq(
            &serde_json::json!(null),
            &serde_json::json!(null)
        ));
    }

    // ── #538 review fixes ─────────────────────────────────────────────────

    #[test]
    fn literal_to_json_inverts_sql_literal_scalars() {
        assert_eq!(literal_to_json("NULL"), Some(Value::Null));
        assert_eq!(literal_to_json("TRUE"), Some(Value::Bool(true)));
        assert_eq!(literal_to_json("FALSE"), Some(Value::Bool(false)));
        assert_eq!(literal_to_json("42"), Some(serde_json::json!(42)));
        assert_eq!(literal_to_json("'EU'"), Some(serde_json::json!("EU")));
        // `''`-doubled quotes inside a string literal (`sql_string_literal`'s
        // own escaping) round-trip back to a single quote.
        assert_eq!(
            literal_to_json("'O''Brien'"),
            Some(serde_json::json!("O'Brien"))
        );
        // Malformed input (an unterminated string, trailing garbage) is
        // refused, never guessed at.
        assert_eq!(literal_to_json("'unterminated"), None);
        assert_eq!(literal_to_json("42 garbage"), None);
    }

    #[test]
    fn literal_to_json_inverts_a_u64_past_i64_max_exactly() {
        // Review of #538 round 2 nit: `scalar_literal`'s own forward
        // direction tries i64 then u64 (`sv::sql_literal`'s match on
        // `n.as_i64()`/`n.as_u64()`) before f64 — the inverse must too, or
        // a value that round-trips exactly as a u64 silently loses
        // precision through f64.
        let n = u64::MAX;
        assert_eq!(literal_to_json(&n.to_string()), Some(serde_json::json!(n)));
    }

    #[test]
    fn literal_to_json_refuses_a_non_finite_float_rather_than_guess() {
        // Review of #538 round 2 nit: `Number::from_f64`, not
        // `Value::from(f64)` directly — the latter accepts non-finite
        // floats. `1e400` is made of only the characters the scanner
        // accepts as a number (digits and `e`) and overflows `f64` to
        // infinity on parse — never actually produced by `scalar_literal`
        // (which checks `is_finite()` first), but this keeps the inverse
        // itself from ever claiming to parse one.
        assert_eq!(literal_to_json("1e400"), None);
    }

    #[test]
    fn literal_to_json_inverts_sql_literal_lists() {
        // Review of #538 round 2 nit: a credential locking a
        // ::MULTISELECT variable's value builds exactly this shape
        // (`sv::sql_literal`'s Array branch) — must round-trip, not be
        // refused.
        assert_eq!(
            literal_to_json("[]::VARCHAR[]"),
            Some(serde_json::json!([]))
        );
        assert_eq!(
            literal_to_json(&sv::sql_literal(&serde_json::json!(["EU", "US"])).unwrap()),
            Some(serde_json::json!(["EU", "US"]))
        );
        // A comma *inside* a quoted item is never mistaken for the list's
        // own separator.
        assert_eq!(
            literal_to_json(&sv::sql_literal(&serde_json::json!(["a, b", "c"])).unwrap()),
            Some(serde_json::json!(["a, b", "c"]))
        );
        // Mixed scalar types, same as a real ::MULTISELECT could lock.
        assert_eq!(
            literal_to_json(&sv::sql_literal(&serde_json::json!([1, "x", true, null])).unwrap()),
            Some(serde_json::json!([1, "x", true, null]))
        );
    }

    #[test]
    fn effective_value_refuses_a_name_outside_vars_allowlist() {
        let mut d = dash("d", "");
        d.meta.vars = Some(vec!["q1".to_string()]);
        let raw = serde_json::json!({"note": "not settable"});
        assert_eq!(effective_value("q1", &d, None, &raw), Ok(None));
        assert_eq!(effective_value("note", &d, None, &raw), Err(()));
    }

    #[test]
    fn effective_value_prefers_the_locked_value_over_the_viewers_own() {
        // Review of #538, blocker 1: a credential that locks `region` must
        // have its own value recorded, never whatever the viewer sent.
        let d = dash("d", "");
        let scope = Scope {
            dashboards: None,
            vars: vec![("region".to_string(), "'EU'".to_string())],
        };
        let raw = serde_json::json!({"region": "US"});
        assert_eq!(
            effective_value("region", &d, Some(&scope), &raw),
            Ok(Some(serde_json::json!("EU")))
        );
        // An unlocked name still reads the viewer's own value.
        let raw2 = serde_json::json!({"q1": "B"});
        assert_eq!(
            effective_value("q1", &d, Some(&scope), &raw2),
            Ok(Some(serde_json::json!("B")))
        );
    }

    #[test]
    fn effective_value_prefers_a_locked_list_value_too() {
        // Review of #538 round 2 nit: a credential locking a
        // ::MULTISELECT variable (a list, not a scalar) must win the same
        // way a locked scalar does — recorded as the locked list, never
        // the viewer's own selection.
        let d = dash("d", "");
        let scope = Scope {
            dashboards: None,
            vars: vec![(
                "regions".to_string(),
                sv::sql_literal(&serde_json::json!(["EU", "US"])).unwrap(),
            )],
        };
        let raw = serde_json::json!({"regions": ["APAC"]});
        assert_eq!(
            effective_value("regions", &d, Some(&scope), &raw),
            Ok(Some(serde_json::json!(["EU", "US"])))
        );
    }

    #[test]
    fn effective_value_is_none_when_the_viewer_sent_nothing() {
        let d = dash("d", "");
        let raw = serde_json::json!({});
        assert_eq!(effective_value("q1", &d, None, &raw), Ok(None));
    }

    #[test]
    fn effective_value_picks_the_last_spelling_like_dedup_last_spelling_does() {
        // Review of #538 round 2, nit: DuckDB variables are case-insensitive
        // and the later `SET` wins (`sv::dedup_last_spelling`'s own rule,
        // applied to every variable list `locked_panel` actually runs).
        // `effective_value` must agree, or it can record a value for a
        // different spelling than the one that was actually set.
        let d = dash("d", "");
        // serde_json's Map preserves insertion order (preserve_order):
        // two distinctly-cased JSON keys for the same DuckDB variable,
        // last one wins.
        let raw = serde_json::json!({"Q1": "A", "q1": "B"});
        assert_eq!(
            effective_value("q1", &d, None, &raw),
            Ok(Some(serde_json::json!("B")))
        );
        // Same rule for a credential's own locked vars.
        let scope = Scope {
            dashboards: None,
            vars: vec![
                ("Region".to_string(), "'US'".to_string()),
                ("region".to_string(), "'EU'".to_string()),
            ],
        };
        assert_eq!(
            effective_value("region", &d, Some(&scope), &serde_json::json!({})),
            Ok(Some(serde_json::json!("EU")))
        );
    }

    // ───────────────────────────────── #517 S3: read-back (plan §5) ──────

    /// `StateOptions::parse`, so these tests exercise the real parser, not
    /// a hand-built struct — `extra` overrides/adds keys onto a minimal
    /// `{"path": …}`.
    fn state_opts(extra: serde_json::Value) -> StateOptions {
        let mut o = serde_json::json!({"path": "/nonexistent/state.jsonl"});
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        StateOptions::parse(&o).unwrap()
    }

    fn claims(user: &str, groups: &[&str]) -> Claims {
        Claims {
            user: user.to_string(),
            groups: groups.iter().map(|g| g.to_string()).collect(),
            attrs: vec![],
        }
    }

    /// A real, freshly-opened [`Store`] (not `opts.path` — that one never
    /// has to exist for these tests) behind a [`SubmitRuntime`], for
    /// [`state_gen`]/[`seed_viewer_state`] tests.
    fn submit_runtime(opts: StateOptions) -> SubmitRuntime {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("submit-runtime-test-fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!("state-{}-{nanos}.jsonl", std::process::id()));
        let store = Store::open(&path, opts.max_mb).unwrap();
        SubmitRuntime {
            opts,
            store,
            rate: RateLimiter::new(600, 100, 10),
            global: Mutex::new(GlobalBucket::new()),
            viewer_locks: Mutex::new(ViewerLocks::new()),
            recent: Mutex::new(RecentIds::new()),
            full: AtomicBool::new(false),
        }
    }

    fn new_record(user: &str, dashboard: &str, id: &str) -> NewRecord {
        NewRecord {
            user: user.to_string(),
            auth: "proxy".to_string(),
            dashboard: dashboard.to_string(),
            submit: "quiz".to_string(),
            id: id.to_string(),
            vars: serde_json::Map::new(),
        }
    }

    #[test]
    fn state_trainer_view_is_none_outside_every_list() {
        let o = state_opts(serde_json::json!({"dashboards": ["d"]}));
        assert_eq!(state_trainer_view(&o, "other", None, None), None);
    }

    #[test]
    fn state_trainer_view_is_own_rows_for_a_plain_submit_or_read_dashboard() {
        let o = state_opts(serde_json::json!({"dashboards": ["d"], "read": ["progress"]}));
        assert_eq!(state_trainer_view(&o, "d", None, None), Some(false));
        assert_eq!(state_trainer_view(&o, "progress", None, None), Some(false));
    }

    #[test]
    fn state_trainer_view_is_every_row_only_for_a_named_trainer() {
        // Threat model #19: anyone else on the *same* trainer dashboard
        // still gets only their own rows — fails safe, never refused
        // outright.
        let o = state_opts(serde_json::json!({
            "trainers": {"dashboards": ["progress"], "groups": ["trainers"], "users": ["root-admin"]}
        }));
        assert_eq!(
            state_trainer_view(&o, "progress", Some(&claims("carol", &["trainers"])), None),
            Some(true)
        );
        assert_eq!(
            state_trainer_view(&o, "progress", Some(&claims("root-admin", &[])), None),
            Some(true)
        );
        assert_eq!(
            state_trainer_view(&o, "progress", Some(&claims("bob", &[])), None),
            Some(false)
        );
        assert_eq!(state_trainer_view(&o, "progress", None, None), Some(false));
    }

    #[test]
    fn state_trainer_view_refuses_an_api_key_identity_as_a_trainer() {
        // #548 item 3 nit: a machine credential matching trainers.groups/
        // trainers.users is still never a trainer — same posture
        // handle_submit already takes for submitting as anyone.
        let o = state_opts(serde_json::json!({
            "trainers": {"dashboards": ["progress"], "groups": ["trainers"], "users": ["root-admin"]}
        }));
        assert_eq!(
            state_trainer_view(
                &o,
                "progress",
                Some(&claims("carol", &["trainers"])),
                Some("api_key")
            ),
            Some(false)
        );
        assert_eq!(
            state_trainer_view(
                &o,
                "progress",
                Some(&claims("root-admin", &[])),
                Some("api_key")
            ),
            Some(false)
        );
        // A non-api_key method for the same identity is unaffected.
        assert_eq!(
            state_trainer_view(
                &o,
                "progress",
                Some(&claims("carol", &["trainers"])),
                Some("proxy")
            ),
            Some(true)
        );
    }

    #[test]
    fn state_scalar_text_stringifies_each_json_scalar() {
        assert_eq!(state_scalar_text(&serde_json::json!("B")), "B");
        assert_eq!(state_scalar_text(&serde_json::json!(true)), "true");
        assert_eq!(state_scalar_text(&serde_json::json!(false)), "false");
        assert_eq!(state_scalar_text(&serde_json::json!(42)), "42");
    }

    #[test]
    fn state_snapshot_gen_is_the_viewers_own_row_count_for_a_plain_state_dashboard() {
        let o = state_opts(serde_json::json!({"dashboards": ["d"]}));
        let rt = submit_runtime(o);
        let alice = claims("alice", &[]);
        assert_eq!(
            state_snapshot(&rt, "d", Some(&alice), None).map(|(g, _)| g),
            Some(0)
        );
        rt.store.append(new_record("alice", "d", "a1")).unwrap();
        let (gen, rows) = state_snapshot(&rt, "d", Some(&alice), None).unwrap();
        assert_eq!(gen, 1);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].user, "alice");
        // A different user's own gen is unaffected by alice's submission.
        assert_eq!(
            state_snapshot(&rt, "d", Some(&claims("bob", &[])), None).map(|(g, _)| g),
            Some(0)
        );
    }

    #[test]
    fn state_snapshot_gen_is_the_global_seq_for_a_trainers_all_rows_read() {
        let o = state_opts(serde_json::json!({
            "trainers": {"dashboards": ["progress"], "groups": ["trainers"], "users": []}
        }));
        let rt = submit_runtime(o);
        let carol = claims("carol", &["trainers"]);
        assert_eq!(
            state_snapshot(&rt, "progress", Some(&carol), None).map(|(g, _)| g),
            Some(0)
        );
        rt.store
            .append(new_record("alice", "progress", "a1"))
            .unwrap();
        rt.store
            .append(new_record("bob", "progress", "b1"))
            .unwrap();
        // The global seq (2 submissions total), not carol's own (0), and
        // both viewers' rows (gen and rows read together, under one lock —
        // #548 item 3 nit).
        let (gen, rows) = state_snapshot(&rt, "progress", Some(&carol), None).unwrap();
        assert_eq!(gen, 2);
        assert_eq!(rows.len(), 2, "{rows:?}");
    }

    #[test]
    fn state_snapshot_is_none_outside_every_list() {
        let o = state_opts(serde_json::json!({"dashboards": ["d"]}));
        let rt = submit_runtime(o);
        assert_eq!(state_snapshot(&rt, "other", None, None), None);
    }

    #[test]
    fn state_snapshot_refuses_an_api_key_identity_as_a_trainer() {
        // #548 item 3 nit, end to end through state_snapshot: an api_key
        // matching trainers.groups never gets every viewer's rows.
        let o = state_opts(serde_json::json!({
            "trainers": {"dashboards": ["progress"], "groups": ["trainers"], "users": []}
        }));
        let rt = submit_runtime(o);
        rt.store
            .append(new_record("alice", "progress", "a1"))
            .unwrap();
        rt.store
            .append(new_record("carol", "progress", "c1"))
            .unwrap();
        let carol = claims("carol", &["trainers"]);
        let (_, rows) = state_snapshot(&rt, "progress", Some(&carol), Some("api_key")).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].user, "carol");
    }
}

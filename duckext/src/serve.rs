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
//!   between viewers), under a row cap and a query timeout. Results are cached
//!   per server (the snapshot never changes), identical concurrent requests run
//!   once, and a request superseded by the same viewer's newer one is
//!   interrupted ([`crate::panel_cache`]).
//! * `anofox_serve_stop(port)` — stop a server and delete its snapshot.
//!
//! Each server's state is owned by the server (no process-wide "last load
//! wins" globals besides the port registry), and startup is transactional:
//! nothing is registered until the snapshot is built and the port is bound.

use crate::db::{json_rows, Conn, Database};
use crate::panel_cache::{PanelCache, Served, SUPERSEDED};
use anofox_visualization::host::serving::{
    self as sv, content_type, html_escape, json_for_script, quote_ident, sql_string_literal,
    AuthoringGuard, RequestMeta,
};
use include_dir::{include_dir, Dir};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
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
    workers: Vec<JoinHandle<()>>,
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
    r.stop.store(true, Ordering::SeqCst);
    for _ in 0..r.workers.len() {
        r.server.unblock();
    }
    for w in r.workers.drain(..) {
        let _ = w.join();
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

/// A cached panel result as a response body (no copy of the JSON).
struct SharedBody(Arc<String>);

impl AsRef<[u8]> for SharedBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

fn respond_panel(req: Request, body: Arc<String>, etag: &str) {
    let len = body.len();
    let resp = Response::new(
        200.into(),
        vec![
            header("Content-Type", "application/json"),
            header("X-Content-Type-Options", "nosniff"),
            header("Cache-Control", "no-store"),
            header("ETag", etag),
        ],
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

fn serve_asset(req: Request, path: &str) {
    let asset = path.trim_start_matches('/');
    let asset = if asset.is_empty() {
        "index.html"
    } else {
        asset
    };
    match WEB.get_file(asset) {
        Some(f) => respond(req, 200, content_type(asset), f.contents().to_vec()),
        None => text(req, 404, "not found"),
    }
}

// -------------------------------------------------------- authoring mode ---

struct Authoring {
    ctx: Arc<DbCtx>,
    guard: AuthoringGuard,
    max_body: usize,
}

/// `anofox_serve(port)`: start the authoring server on 127.0.0.1:`port`.
pub fn start_authoring(ctx: Arc<DbCtx>, port: u16) -> Result<String, String> {
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
    serve_asset(req, &path);
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
                other => return Err(format!("options: unknown key '{other}' (max_rows, timeout_ms, max_body_bytes, threads, cache_mb, attach, load, theme, hosts)")),
            }
        }
        Ok(o)
    }
}

struct Dash {
    id: String,
    title: String,
    sql: String,
    /// Planned statements: (is_setup, sql), indexed like the client's plan().
    stmts: Vec<(bool, String)>,
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
    // Field order = drop order: close the database before deleting its files.
    db: Database,
    /// The source session's default database (each request connection `USE`s it).
    default_db: String,
    _dir: SnapshotDir,
}

struct Locked {
    port: u16,
    dashboards: Vec<Dash>,
    snap: Snapshot,
    opts: LockedOptions,
    /// Panel results of this server's snapshot (a new snapshot = a new cache).
    cache: PanelCache,
    /// Random per snapshot; qualifies the ETags.
    epoch: String,
}

/// `anofox_serve_dashboards(dir, port[, options])`.
pub fn start_locked(
    ctx: Arc<DbCtx>,
    dir: &str,
    port: u16,
    opts: LockedOptions,
) -> Result<String, String> {
    let mut servers = lock_servers();
    if servers.contains_key(&port) {
        return Err(format!(
            "anofox-visualization is already serving on port {port}"
        ));
    }
    let (dashboards, mut loads) = load_dashboards(Path::new(dir))?;
    loads.extend(opts.load.iter().cloned());
    let (snap, warnings) = {
        let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        build_snapshot(&conn, opts.attach, &loads)?
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
    }
    let addr = format!("127.0.0.1:{port}");
    let server = Arc::new(
        Server::http(&addr)
            .map_err(|e| format!("anofox-visualization: could not bind {addr}: {e}"))?,
    );
    let n = dashboards.len();
    let threads = opts.threads;
    let state = Arc::new(Locked {
        port,
        dashboards,
        snap,
        cache: PanelCache::new(opts.cache_bytes),
        epoch: sv::random_token()[..8].to_string(),
        opts,
    });
    let stop = Arc::new(AtomicBool::new(false));
    let st = state.clone();
    let workers = spawn_workers(
        &server,
        &stop,
        threads,
        Arc::new(move |req| handle_locked(&st, req)),
    );
    servers.insert(
        port,
        Running {
            kind: "locked",
            server,
            stop,
            workers,
            _state: Box::new(state),
        },
    );
    let warn = if warnings.is_empty() {
        String::new()
    } else {
        format!(" (warnings: {})", warnings.join("; "))
    };
    Ok(format!(
        "anofox-visualization serving {n} locked, read-only dashboard(s) at http://{addr}/ — server owns the SQL{warn}"
    ))
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
        let stmts = anofox_visualization::sql::plan(&sql)
            .into_iter()
            .map(|p| (p.setup, p.sql.trim().to_string()))
            .collect();
        out.push(Dash {
            id,
            title: meta.title,
            sql,
            stmts,
        });
    }
    if out.is_empty() {
        return Err(format!("no .sql dashboards found in '{}'", dir.display()));
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    loads.sort();
    loads.dedup();
    Ok((out, loads))
}

/// Snapshot the live session into a private directory and open it through a
/// fresh, locked-down DuckDB instance:
/// * every DuckDB database attached to the session (or just the default one
///   with `attach: false`) is copied (`COPY FROM DATABASE`) and re-attached
///   READ_ONLY under its original name; other catalog types (postgres, …) are
///   skipped with a warning;
/// * extensions loaded in the session (and `-- @load`/`options.load`) are
///   loaded, best-effort, while that is still allowed;
/// * then `enable_external_access = false` (no file / network access: no
///   `COPY … TO`, `read_text`, `ATTACH`, `INSTALL`, `LOAD`), extension
///   auto-install/-load off, and `lock_configuration = true` so no request can
///   undo any of it.
fn build_snapshot(
    live: &Conn,
    attach_all: bool,
    extra_loads: &[String],
) -> Result<(Snapshot, Vec<String>), String> {
    let mut warnings = Vec::new();
    let dir = SnapshotDir::create()?;
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
    let mut copied: Vec<(String, PathBuf)> = Vec::new();
    for row in dbs {
        let (Some(name), ty) = (row[0].clone(), row[1].clone().unwrap_or_default()) else {
            continue;
        };
        if name != current && !attach_all {
            continue;
        }
        if ty != "duckdb" {
            warnings.push(format!("database '{name}' ({ty}) not snapshotted"));
            continue;
        }
        let file = dir.0.join(format!("db{}.duckdb", copied.len()));
        let alias = format!("__anofox_snap_{}", sv::random_token());
        let f = file.to_string_lossy().into_owned();
        live.exec(&format!(
            "ATTACH {} AS {}",
            sql_string_literal(&f),
            quote_ident(&alias)
        ))?;
        let r = live.exec(&format!(
            "COPY FROM DATABASE {} TO {}",
            quote_ident(&name),
            quote_ident(&alias)
        ));
        let d = live.exec(&format!("DETACH {}", quote_ident(&alias)));
        r.and(d)
            .map_err(|e| format!("snapshot of '{name}' failed: {e}"))?;
        copied.push((name, file));
    }
    if !copied.iter().any(|(n, _)| *n == current) {
        return Err(format!(
            "could not snapshot the current database '{current}'"
        ));
    }
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
        let mut exts: Vec<&String> = loaded.iter().chain(extra_loads.iter()).collect();
        exts.sort();
        exts.dedup();
        for e in exts {
            if !sv::valid_ident(e) {
                continue;
            }
            if let Err(err) = c.exec(&format!("LOAD {}", quote_ident(e))) {
                let first = err.lines().next().unwrap_or("").to_string();
                warnings.push(format!("LOAD {e}: {first}"));
            }
        }
        c.exec("SET enable_external_access = false")?;
        c.exec("SET lock_configuration = true")?;
    }
    Ok((
        Snapshot {
            db,
            default_db: current,
            _dir: dir,
        },
        warnings,
    ))
}

fn handle_locked(st: &Locked, mut req: Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let m = meta(&req);
    eprintln!("[anofox-serve] {} {}", req.method(), path);
    // DNS rebinding: only loopback names (or `options.hosts`) are served.
    if !sv::host_allowed(m.host, st.port, &st.opts.hosts) {
        return text(
            req,
            421,
            "misdirected request: unknown Host — add it to options.hosts (e.g. {\"hosts\": [\"dash.example.com\"]})",
        );
    }
    // CSRF: a browser request from another origin is refused outright.
    if !sv::origin_matches_host(m.origin, m.host) {
        return text(req, 403, "forbidden: cross-origin request");
    }
    match (req.method(), path.as_str()) {
        (&Method::Get, "/health") | (&Method::Get, "/healthz") => {
            let body = format!(
                "{{\"status\":\"ok\",\"dashboards\":{}}}",
                st.dashboards.len()
            );
            respond(req, 200, "application/json", body.into_bytes())
        }
        (&Method::Post, "/api/panel") => {
            let body = match read_body(&mut req, st.opts.max_body) {
                Ok(b) => b,
                Err((code, e)) => return text(req, code, &e),
            };
            let supersede = req_header(&req, SUPERSEDE_HEADER).map(str::to_string);
            match locked_panel(st, &body, supersede.as_deref()) {
                Ok((json, etag)) => respond_panel(req, json, &etag),
                Err((code, e)) => {
                    if code != SUPERSEDED.0 {
                        eprintln!("[anofox-serve] {code}: {e}");
                    }
                    text(req, code, &e)
                }
            }
        }
        (_, "/query") => text(
            req,
            410,
            "this server is locked: there is no SQL endpoint (use POST /api/panel)",
        ),
        (&Method::Get, "/") => respond(
            req,
            200,
            "text/html; charset=utf-8",
            list_page(st).into_bytes(),
        ),
        (&Method::Get, p) if p.starts_with("/d/") => {
            let id = p["/d/".len()..].trim_end_matches('/');
            match st.dashboards.iter().find(|d| d.id == id) {
                Some(d) => respond(req, 200, "text/html; charset=utf-8", served_index(st, d)),
                None => text(req, 404, "no such dashboard"),
            }
        }
        (&Method::Get, p) => serve_asset(req, p),
        _ => text(req, 405, "method not allowed"),
    }
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
/// besides the (immutable) snapshot and dashboards — the parsed request, with
/// the variables in a canonical order. Debug formatting quotes and escapes
/// every string, so distinct requests never collide.
fn panel_key(r: &sv::PanelRequest) -> String {
    let mut vars: Vec<&(String, String)> = r.vars.iter().collect();
    vars.sort();
    format!("{:?}", (&r.dashboard, r.panel, vars, &r.page))
}

fn etag(epoch: &str, key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    format!("\"{epoch}-{:016x}\"", h.finish())
}

/// Serve one panel of one dashboard for a viewer — from the cache, by joining
/// an identical running request, or by running it (`run_panel`). Returns the
/// JSON rows and their ETag.
fn locked_panel(
    st: &Locked,
    body: &str,
    supersede: Option<&str>,
) -> Result<(Arc<String>, String), (u16, String)> {
    let r = sv::parse_panel_request(body).map_err(|e| (400, e))?;
    let dash = st
        .dashboards
        .iter()
        .find(|d| d.id == r.dashboard)
        .ok_or((404, "no such dashboard".to_string()))?;
    let (is_setup, panel_sql) = dash
        .stmts
        .get(r.panel)
        .ok_or((404, "no such panel".to_string()))?;
    if *is_setup {
        // setup runs server-side, per request (run_panel)
        return Ok((Arc::new("[]".into()), etag(&st.epoch, "setup")));
    }
    let ticket = match supersede {
        None => None,
        // Namespaced by dashboard: a viewer id is per page.
        Some(v) if valid_supersede(v) => Some(format!("{}/{v}", r.dashboard)),
        Some(_) => {
            return Err((
                400,
                format!("{SUPERSEDE_HEADER} must be <32 hex digits>/<panel>/<kind>"),
            ))
        }
    };
    let key = panel_key(&r);
    let t0 = std::time::Instant::now();
    let (out, how) = st.cache.run(&key, ticket, |abort| {
        run_panel(st, dash, panel_sql, &r, abort)
    });
    if how == Served::Executed {
        let cancelled = matches!(&out, Err((code, _)) if *code == SUPERSEDED.0);
        eprintln!(
            "[anofox-serve] ran {}#{} in {} ms{}",
            r.dashboard,
            r.panel,
            t0.elapsed().as_millis(),
            if cancelled { " (superseded)" } else { "" }
        );
    }
    out.map(|json| (json, etag(&st.epoch, &key)))
}

/// Run one panel of one dashboard. Fresh connection per request (variables
/// and temp objects never leak between viewers), variables set as typed
/// literals, the dashboard's preceding setup statements re-run (only TEMP
/// objects can be created — the snapshot is read-only), and the panel query
/// executed as a single prepared statement under the row cap + timeout. The
/// query is interrupted when every request waiting for it has been superseded.
fn run_panel(
    st: &Locked,
    dash: &Dash,
    panel_sql: &str,
    r: &sv::PanelRequest,
    abort: &crate::panel_cache::Abort,
) -> Result<String, (u16, String)> {
    let cancelled = || (SUPERSEDED.0, SUPERSEDED.1.to_string());
    let conn = st.snap.db.connect().map_err(|e| (500, e))?;
    // Declared after `conn`, so dropped before it: no interrupt after that.
    let intr = conn.interrupter();
    let Some(_registered) = abort.register(move || intr.interrupt()) else {
        return Err(cancelled());
    };
    conn.with_deadline(st.opts.timeout, |c| {
        c.exec(&format!("USE {}", quote_ident(&st.snap.default_db)))?;
        for (name, lit) in &r.vars {
            c.exec(&format!("SET VARIABLE {name} = {lit}"))?;
        }
        for (setup, sql) in &dash.stmts[..r.panel] {
            if *setup {
                // Best effort, like the browser: a failing setup statement is
                // reported in the log, the panel still runs.
                if let Err(e) = c.execute_prepared(sql, &[]) {
                    eprintln!(
                        "[anofox-serve] setup statement failed in '{}': {e}",
                        dash.id
                    );
                }
            }
        }
        // (An interrupt between two statements may not reach the next one.)
        if abort.abandoned() {
            return Err(SUPERSEDED.1.to_string());
        }
        match &r.page {
            Some(p) => {
                let (q, params) = sv::page_sql(panel_sql, p);
                let params: Vec<&str> = params.iter().map(String::as_str).collect();
                json_rows(c, &q, &params, st.opts.max_rows)
            }
            None => json_rows(c, panel_sql, &[], st.opts.max_rows),
        }
    })
    .map_err(|e| {
        if abort.abandoned() {
            cancelled()
        } else if e.contains("row limit") {
            (413, e)
        } else {
            (400, e)
        }
    })
}

/// index.html with `window.__served = {id,title,sql,nav,locked}` injected
/// (script-safe JSON: `<` is emitted as `<`, so `</script>` in a
/// dashboard's SQL cannot break out).
fn served_index(st: &Locked, dash: &Dash) -> Vec<u8> {
    let html = WEB
        .get_file("index.html")
        .map(|f| String::from_utf8_lossy(f.contents()).into_owned())
        .unwrap_or_default();
    let nav: Vec<serde_json::Value> = st
        .dashboards
        .iter()
        .map(|d| serde_json::json!({ "id": d.id, "title": d.title }))
        .collect();
    let cfg = json_for_script(&serde_json::json!({
        "id": dash.id, "title": dash.title, "sql": dash.sql, "nav": nav, "locked": true,
        "theme": st.opts.theme,
    }));
    html.replacen("<head>", "<head><base href=\"/\">", 1)
        .replacen(
            "</head>",
            &format!("<script>window.__served={cfg};</script></head>"),
            1,
        )
        .into_bytes()
}

fn list_page(st: &Locked) -> String {
    let items: String = st
        .dashboards
        .iter()
        .map(|d| {
            format!(
                "<li><a href=\"/d/{}\">{}</a></li>",
                html_escape(&d.id),
                html_escape(&d.title)
            )
        })
        .collect();
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Dashboards</title>\
<body style=\"font:15px system-ui;padding:2rem\"><h1>Dashboards</h1><ul>{items}</ul>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn cache_options_and_keys() {
        assert_eq!(LockedOptions::parse(None).unwrap().cache_bytes, 64 << 20);
        let o = LockedOptions::parse(Some(r#"{"cache_mb":0}"#)).unwrap();
        assert_eq!(o.cache_bytes, 0);
        assert!(LockedOptions::parse(Some(r#"{"cache_mb":-1}"#)).is_err());
        // The variables' order doesn't matter; their values, the panel, the
        // dashboard and the page do.
        let key = |body: &str| panel_key(&sv::parse_panel_request(body).unwrap());
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
        assert!(etag("e1", &k).starts_with("\"e1-"));
        assert_ne!(etag("e1", &k), etag("e2", &k));
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
}

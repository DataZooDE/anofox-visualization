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
//!   per snapshot, identical concurrent requests run once, and a request
//!   superseded by the same viewer's newer one is interrupted
//!   ([`crate::panel_cache`]). The snapshot can be rebuilt while serving
//!   (`refresh_s`, `anofox_serve_refresh`) and is swapped atomically
//!   ([`crate::generation`]); `data` opens one folder of files read-only to
//!   the panels, for data too large or too live to snapshot.
//! * `anofox_serve_refresh(port)` — rebuild a locked server's snapshot now.
//! * `anofox_serve_stop(port)` — stop a server and delete its snapshot.
//!
//! Each server's state is owned by the server (no process-wide "last load
//! wins" globals besides the port registry), and startup is transactional:
//! nothing is registered until the snapshot is built and the port is bound.

use crate::db::{json_rows, Conn, Database};
use crate::generation::{Current, Generation};
use crate::panel_cache::{Served, SUPERSEDED};
use anofox_visualization::host::access::{self, Claims, LogEntry, ProxyAuth, RequestLog};
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
    workers: Vec<JoinHandle<()>>,
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
    /// Rebuild the snapshot this often (`refresh_s`; `None` = never).
    pub refresh: Option<Duration>,
    /// A folder the panels may read (not write: see `check_read_only`) — the
    /// canonical path of `options.data`.
    pub data: Option<PathBuf>,
    /// Variables whose values name a file or folder inside `data`: each must
    /// be a plain name (`[A-Za-z0-9_.-]`, not starting with `.`).
    pub path_vars: Vec<String>,
    /// `auth`: viewer identity from a trusted auth proxy (every request but
    /// `/health` needs it; the claims become `__claim_*` variables).
    pub auth: Option<ProxyAuth>,
    /// `log` (+ `log_vars`): JSON-lines request log.
    pub log: Option<Arc<RequestLog>>,
    /// Listen address (`bind`; default loopback). A specific address is also
    /// accepted as a `Host`.
    pub bind: std::net::IpAddr,
    /// DuckDB `memory_limit` of the snapshot instance (e.g. `"2GB"`).
    pub memory_limit: Option<String>,
    /// DuckDB `threads` of the snapshot instance, shared by all queries.
    pub duckdb_threads: Option<usize>,
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
            refresh: None,
            data: None,
            path_vars: Vec::new(),
            auth: None,
            log: None,
            bind: std::net::Ipv4Addr::LOCALHOST.into(),
            memory_limit: None,
            duckdb_threads: None,
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
                "auth" => o.auth = Some(ProxyAuth::from_json(val, |k| std::env::var(k).ok()).map_err(|e| format!("options.{e}"))?),
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
                        .filter(|m| !m.is_empty() && m.len() <= 32 && m.chars().all(|c| c.is_ascii_alphanumeric() || " .%".contains(c)))
                        .ok_or("options.memory_limit must be a size like \"2GB\" or \"50%\"")?;
                    o.memory_limit = Some(m.to_string());
                }
                "duckdb_threads" => o.duckdb_threads = Some(num()?.min(1024) as usize),
                other => return Err(format!("options: unknown key '{other}' (max_rows, timeout_ms, max_body_bytes, threads, cache_mb, attach, load, theme, hosts, refresh_s, data, path_vars, auth, log, log_vars, bind, memory_limit, duckdb_threads)")),
            }
        }
        if !o.path_vars.is_empty() && o.data.is_none() {
            return Err("options.path_vars needs options.data".into());
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

struct Dash {
    id: String,
    title: String,
    sql: String,
    /// Planned statements: (is_setup, sql), indexed like the client's plan().
    stmts: Vec<(bool, String)>,
    /// `-- @vars`: the variables a viewer may set (others are dropped).
    vars: Option<Vec<String>>,
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
    opts: LockedOptions,
    /// What a (re)build snapshots: the serving database's context and the
    /// extensions to load into the snapshot.
    ctx: Arc<DbCtx>,
    loads: Vec<String>,
    /// The snapshot (+ its result cache and epoch) new requests run on.
    current: Current<Snapshot>,
    /// Random per server; qualifies the ETags (with the epoch).
    server_id: String,
}

// `anofox_data`: the `SET VARIABLE` that tells panels where `options.data`
// is. Viewers can never set it (`access::is_reserved_var`, any request).
use anofox_visualization::host::access::DATA_VAR;

impl Locked {
    fn build(&self) -> Result<(Snapshot, Vec<String>), String> {
        let conn = self.ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        build_snapshot(&conn, &self.opts, &self.loads)
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
    loads.extend(opts.load.iter().cloned());
    let (snap, warnings) = {
        let conn = ctx.conn.lock().unwrap_or_else(|p| p.into_inner());
        build_snapshot(&conn, &opts, &loads)?
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
    let state = Arc::new(Locked {
        port,
        dashboards,
        current: Current::new(snap, opts.cache_bytes),
        opts,
        ctx,
        loads,
        server_id: sv::random_token()[..8].to_string(),
    });
    let auth_note = match &state.opts.auth {
        Some(a) => format!(
            "; login via auth proxy (trusted identity headers: {}; secret in {}) — the proxy must set or remove each of them on every request",
            a.trusted_headers(),
            a.secret_header
        ),
        None => String::new(),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let st = state.clone();
    let mut workers = spawn_workers(
        &server,
        &stop,
        threads,
        Arc::new(move |req| handle_locked(&st, req)),
    );
    if let Some(every) = refresh {
        let (st, stop) = (state.clone(), stop.clone());
        workers.push(std::thread::spawn(move || refresher(&st, &stop, every)));
    }
    servers.insert(
        port,
        Running {
            kind: "locked",
            server,
            stop,
            workers,
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
        "anofox-visualization serving {n} locked, read-only dashboard(s) at http://{addr}/ — server owns the SQL{auth_note}{warn}"
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
/// of the setup forms the server needs — classified by DuckDB's own parser —
/// because DuckDB can write inside `allowed_directories` too (`COPY … TO`, and
/// anything that runs a statement: `EXPLAIN ANALYZE`, `PREPARE`/`EXECUTE`,
/// `CALL`, `PRAGMA`, …). An allowlist, so new statement kinds are refused.
fn check_read_only(c: &Conn, dashboards: &[Dash]) -> Result<(), String> {
    let serialize = |sql: &str| -> Result<String, String> {
        let out = c
            .execute_prepared("SELECT json_serialize_sql(?::VARCHAR)::VARCHAR", &[sql])
            .and_then(|mut r| r.string_rows(1))
            .map_err(|e| {
                format!(
                    "checking the dashboards for a data folder needs DuckDB's json extension: {}",
                    e.lines().next().unwrap_or("")
                )
            })?;
        Ok(out
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .unwrap_or_default())
    };
    for d in dashboards {
        for (i, (setup, sql)) in d.stmts.iter().enumerate() {
            read_only_statement(sql, *setup, &serialize)
                .map_err(|e| format!("dashboard '{}' statement {i}: {e}", d.id))?;
        }
    }
    Ok(())
}

/// `Ok` if `sql` (a panel, or a setup statement if `setup`) may run with a data
/// folder open. `serialize` is DuckDB's `json_serialize_sql`: it parses without
/// binding or running, and serialises only SELECT statements — `EXPLAIN`,
/// `PREPARE`, `EXECUTE`, `COPY`, `PRAGMA`, `CALL`, a top-level `PIVOT`, … come
/// back as errors.
fn read_only_statement(
    sql: &str,
    setup: bool,
    serialize: &dyn Fn(&str) -> Result<String, String>,
) -> Result<(), String> {
    let query = if setup {
        setup_query(sql).ok_or(
            "with a data folder, setup statements must be SET VARIABLE <name> = <expression> \
             or CREATE [OR REPLACE] TEMP TABLE|VIEW <name> AS <query>",
        )?
    } else {
        sql.to_string()
    };
    let v: serde_json::Value = serde_json::from_str(&serialize(&query)?).unwrap_or_default();
    if v["error"] == serde_json::Value::Bool(false)
        && v["statements"].as_array().is_some_and(|a| a.len() == 1)
    {
        Ok(())
    } else {
        Err("with a data folder, only queries (SELECT …) are allowed".into())
    }
}

/// The query inside an allowed setup statement — `SELECT <expr>` for
/// `SET VARIABLE <name> = <expr>`, `<query>` for
/// `CREATE [OR REPLACE] TEMP|TEMPORARY TABLE|VIEW [IF NOT EXISTS] <name> AS <query>`
/// — or `None` for any other statement. (Comments are already stripped.)
fn setup_query(sql: &str) -> Option<String> {
    /// `s` after the keyword `w` (case-insensitive, whole word).
    fn kw<'a>(s: &'a str, w: &str) -> Option<&'a str> {
        let s = s.trim_start();
        let head = s.get(..w.len())?;
        let rest = &s[w.len()..];
        (head.eq_ignore_ascii_case(w)
            && !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
        .then_some(rest)
    }
    /// `s` after a (dotted, plain or `"quoted"`) name.
    fn name(s: &str) -> Option<&str> {
        let mut s = s.trim_start();
        loop {
            if let Some(q) = s.strip_prefix('"') {
                // `""` inside a quoted name is a literal quote
                let mut end = None;
                let mut chars = q.char_indices().peekable();
                while let Some((i, c)) = chars.next() {
                    if c == '"' {
                        if chars.peek().is_some_and(|(_, c)| *c == '"') {
                            chars.next();
                        } else {
                            end = Some(i);
                            break;
                        }
                    }
                }
                s = &q[end? + 1..];
            } else {
                let end = s
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(s.len());
                if end == 0 {
                    return None;
                }
                s = &s[end..];
            }
            match s.strip_prefix('.') {
                Some(r) => s = r,
                None => return Some(s),
            }
        }
    }
    let body = sql
        .trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace());
    if let Some(r) = kw(body, "SET").and_then(|r| kw(r, "VARIABLE")) {
        let r = name(r)?.trim_start();
        let expr = r.strip_prefix('=').or_else(|| kw(r, "TO"))?;
        return Some(format!("SELECT {expr}"));
    }
    let r = kw(body, "CREATE")?;
    let r = kw(r, "OR").and_then(|r| kw(r, "REPLACE")).unwrap_or(r);
    let r = kw(r, "TEMPORARY").or_else(|| kw(r, "TEMP"))?;
    let r = kw(r, "TABLE").or_else(|| kw(r, "VIEW"))?;
    let r = kw(r, "IF")
        .and_then(|r| kw(r, "NOT"))
        .and_then(|r| kw(r, "EXISTS"))
        .unwrap_or(r);
    let query = kw(name(r)?, "AS")?.trim();
    (!query.is_empty()).then(|| query.to_string())
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
        let stmts = anofox_visualization::sql::plan(&sql)
            .into_iter()
            .map(|p| (p.setup, p.sql.trim().to_string()))
            .collect();
        out.push(Dash {
            id,
            title: meta.title,
            sql,
            stmts,
            vars: meta.vars,
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
///   `COPY … TO`, `read_text`, `ATTACH`, `INSTALL`, `LOAD`) — except for the
///   `options.data` folder, which goes in `allowed_directories` — extension
///   auto-install/-load off, and `lock_configuration = true` so no request can
///   undo any of it.
fn build_snapshot(
    live: &Conn,
    opts: &LockedOptions,
    extra_loads: &[String],
) -> Result<(Snapshot, Vec<String>), String> {
    let attach_all = opts.attach;
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

/// What the request log records about one request beyond method/path/status.
#[derive(Default)]
struct LogInfo {
    user: Option<String>,
    dashboard: Option<String>,
    panel: Option<usize>,
    cache: Option<&'static str>,
    vars: Vec<(String, String)>,
    bytes: Option<usize>,
    deny: Option<&'static str>,
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
    // Login: with `options.auth` every request but the health check carries
    // the proxy's identity (and secret); without it, nobody gets anything.
    // The health check answers anyone, but only signed-in callers see more
    // than `status`.
    let claims = match &st.opts.auth {
        Some(auth) => {
            let headers: Vec<(&str, &str)> = req
                .headers()
                .iter()
                .map(|h| (h.field.as_str().as_str(), h.value.as_str()))
                .collect();
            match auth.authenticate(req.remote_addr().map(|a| a.ip()), &headers) {
                Ok(c) => {
                    info.user = Some(c.user.clone());
                    Some(c)
                }
                Err(_) if health => None,
                Err(e) => {
                    info.deny = Some(e.reason());
                    return reply(req, 401, access::UNAUTHORIZED);
                }
            }
        }
        None => None,
    };
    match (req.method(), path) {
        (&Method::Get, "/health") | (&Method::Get, "/healthz") => {
            if st.opts.auth.is_some() && claims.is_none() {
                respond(req, 200, "application/json", &b"{\"status\":\"ok\"}"[..]);
                return 200;
            }
            let gen = st.current.get();
            let body = serde_json::json!({
                "status": "ok",
                "dashboards": st.dashboards.len(),
                "epoch": gen.epoch,
                "snapshot_age_s": gen.built.elapsed().as_secs(),
                "refresh_s": st.opts.refresh.map(|d| d.as_secs()),
                "refreshing": st.current.refreshing(),
                // The error itself is only logged / returned to the operator.
                "refresh_failed": st.current.last_failed(),
            });
            respond(req, 200, "application/json", body.to_string().into_bytes());
            200
        }
        (&Method::Post, "/api/panel") => {
            let body = match read_body(&mut req, st.opts.max_body) {
                Ok(b) => b,
                Err((code, e)) => return reply(req, code, &e),
            };
            let supersede = req_header(&req, SUPERSEDE_HEADER).map(str::to_string);
            match locked_panel(st, &body, supersede.as_deref(), claims.as_ref(), info) {
                Ok((json, etag)) => {
                    info.bytes = Some(json.len());
                    respond_panel(req, json, &etag);
                    200
                }
                Err((code, e)) => {
                    if code != SUPERSEDED.0 {
                        eprintln!("[anofox-serve] {code}: {}", access::diag(&e));
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
            respond(
                req,
                200,
                "text/html; charset=utf-8",
                list_page(st).into_bytes(),
            );
            200
        }
        (&Method::Get, p) if p.starts_with("/d/") => {
            let id = p["/d/".len()..].trim_end_matches('/');
            match st.dashboards.iter().find(|d| d.id == id) {
                Some(d) => {
                    info.dashboard = Some(d.id.clone());
                    respond(req, 200, "text/html; charset=utf-8", served_index(st, d));
                    200
                }
                None => reply(req, 404, "no such dashboard"),
            }
        }
        (&Method::Get, p) => {
            let found = WEB.get_file(p.trim_start_matches('/')).is_some() || p == "/";
            serve_asset(req, p);
            if found {
                200
            } else {
                404
            }
        }
        _ => reply(req, 405, "method not allowed"),
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
/// besides the snapshot (each snapshot has its own cache), the `data` files
/// (a refresh drops the cache) and the dashboards — the parsed request, with
/// the variables in a canonical order. Debug formatting quotes and escapes
/// every string, so distinct requests never collide.
/// With a login the viewer's claims are part of the key, so one viewer's
/// result (or running query) is never handed to a viewer with other claims.
fn panel_key(r: &sv::PanelRequest, claims: Option<&Claims>) -> String {
    let mut vars: Vec<&(String, String)> = r.vars.iter().collect();
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
    info: &mut LogInfo,
) -> Result<(Arc<String>, String), (u16, String)> {
    let mut r = sv::parse_panel_request(body).map_err(|e| (400, e))?;
    info.dashboard = Some(r.dashboard.clone());
    info.panel = Some(r.panel);
    for (name, lit) in &r.vars {
        if st
            .opts
            .path_vars
            .iter()
            .any(|v| v.eq_ignore_ascii_case(name))
            && !valid_path_value(lit)
        {
            return Err((
                400,
                format!("variable '{name}' must name a file or folder: [A-Za-z0-9_.-], not starting with '.'"),
            ));
        }
    }
    // The snapshot this request runs on, start to end (a refresh only changes
    // what later requests get).
    let gen = st.current.get();
    let dash = st
        .dashboards
        .iter()
        .find(|d| d.id == r.dashboard)
        .ok_or((404, "no such dashboard".to_string()))?;
    // `-- @vars`: a variable the dashboard doesn't let viewers set is never set.
    if let Some(allowed) = &dash.vars {
        r.vars
            .retain(|(k, _)| allowed.iter().any(|a| a.eq_ignore_ascii_case(k)));
    }
    info.vars = r.vars.clone();
    let (is_setup, panel_sql) = dash
        .stmts
        .get(r.panel)
        .ok_or((404, "no such panel".to_string()))?;
    if *is_setup {
        // setup runs server-side, per request (run_panel)
        return Ok((
            Arc::new("[]".into()),
            etag(&st.server_id, gen.epoch, "setup"),
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
    let key = panel_key(&r, claims);
    let t0 = std::time::Instant::now();
    let (out, how) = gen.cache.run(&key, ticket, |abort| {
        run_panel(st, &gen, dash, panel_sql, &r, claims, abort)
    });
    info.cache = Some(match how {
        Served::Hit => "hit",
        Served::Shared => "shared",
        Served::Executed => "run",
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
    out.map(|json| (json, etag(&st.server_id, gen.epoch, &key)))
}

/// The `SET VARIABLE`s that open every panel request, in order: the viewer's
/// variables, then the login's claims and `anofox_data` — set by the server
/// after the viewer's (who can't name them: both are reserved when the
/// request is parsed).
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
fn run_panel(
    st: &Locked,
    gen: &Generation<Snapshot>,
    dash: &Dash,
    panel_sql: &str,
    r: &sv::PanelRequest,
    claims: Option<&Claims>,
    abort: &crate::panel_cache::Abort,
) -> Result<String, (u16, String)> {
    let cancelled = || (SUPERSEDED.0, SUPERSEDED.1.to_string());
    let conn = gen.snap.db.connect().map_err(|e| (500, e))?;
    // Declared after `conn`, so dropped before it: no interrupt after that.
    let intr = conn.interrupter();
    let Some(_registered) = abort.register(move || intr.interrupt()) else {
        return Err(cancelled());
    };
    conn.with_deadline(st.opts.timeout, |c| {
        c.exec(&format!("USE {}", quote_ident(&gen.snap.default_db)))?;
        for set in request_sets(r, claims, st.opts.data.as_deref()) {
            c.exec(&set)?;
        }
        for (setup, sql) in &dash.stmts[..r.panel] {
            if *setup {
                // Best effort, like the browser: a failing setup statement is
                // reported in the log, the panel still runs.
                if let Err(e) = c.execute_prepared(sql, &[]) {
                    eprintln!(
                        "[anofox-serve] setup statement failed in '{}': {}",
                        dash.id,
                        access::diag(&e)
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
            (400, redact(e, st.opts.data.as_deref()))
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
        assert!(LockedOptions::parse(Some(r#"{"duckdb_threads":0}"#)).is_err());
        // The variables' order doesn't matter; their values, the panel, the
        // dashboard and the page do.
        let key = |body: &str| panel_key(&sv::parse_panel_request(body).unwrap(), None);
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
    fn cli_serialize(sql: &str) -> Option<Result<String, String>> {
        let q = format!(
            "SELECT json_serialize_sql({})::VARCHAR AS j",
            sql_string_literal(sql)
        );
        let out = std::process::Command::new("duckdb")
            .args(["-noheader", "-list", "-c", &q])
            .output()
            .ok()?;
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
        let ser = |sql: &str| cli_serialize(sql).unwrap();
        let ok = |sql: &str, setup: bool| read_only_statement(sql, setup, &ser);
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
        let none = |_: &str| Err::<String, String>("no json_serialize_sql".into());
        assert!(read_only_statement("SELECT 1", false, &none).is_err());
        let garbage = |_: &str| Ok::<String, String>("not json".into());
        assert!(read_only_statement("SELECT 1", false, &garbage).is_err());
    }

    #[test]
    fn setup_forms() {
        assert_eq!(
            setup_query("SET VARIABLE a = 1").as_deref(),
            Some("SELECT  1")
        );
        assert_eq!(
            setup_query("CREATE TEMP VIEW v AS SELECT 2;").as_deref(),
            Some("SELECT 2")
        );
        for sql in [
            "SET a = 1",
            "SET VARIABLEa = 1",
            "CREATE TABLE t AS SELECT 1",
            "CREATE TEMP SEQUENCE s",
            "CREATE TEMP TABLE t AS",
            "CREATE TEMP TABLE t",
            "SET VARIABLE \"a = 1",
            "CREATE TEMP TABLE . AS SELECT 1",
            "CREATE TEMP TABLE a. AS SELECT 1",
        ] {
            assert_eq!(setup_query(sql), None, "{sql}");
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
        assert_eq!(o.auth.as_ref().unwrap().header_user, "X-Forwarded-User");
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
        let ka = panel_key(&r, Some(&alice));
        assert_eq!(ka, panel_key(&r, Some(&alice.clone())));
        assert_ne!(ka, panel_key(&r, Some(&bob)));
        assert_ne!(ka, panel_key(&r, Some(&alice_hr)));
        assert_ne!(ka, panel_key(&r, None));
        // through the real cache: bob never gets alice's rows
        let cache = crate::panel_cache::PanelCache::new(1 << 20);
        let (out, how) = cache.run(&ka, None, |_| Ok("alice rows".to_string()));
        assert_eq!(
            (out.unwrap().as_str(), how),
            ("alice rows", Served::Executed)
        );
        let (out, how) = cache.run(&ka, None, |_| Ok("rerun".to_string()));
        assert_eq!((out.unwrap().as_str(), how), ("alice rows", Served::Hit));
        let kb = panel_key(&r, Some(&bob));
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
        // every answer must be the asking user's, from some snapshot; the
        // cache is per user (key) and per snapshot (generation).
        use crate::generation::Current;
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        let current = Arc::new(Current::new(1u32, 1 << 20));
        let stop = Arc::new(AtomicBool::new(false));
        let runs = Arc::new(AtomicUsize::new(0));
        let r = sv::parse_panel_request(r#"{"dashboard":"who","panel":0}"#).unwrap();
        let refresher = {
            let (current, stop) = (current.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut v = 1u32;
                while !stop.load(Ordering::SeqCst) {
                    v += 1;
                    current.rebuild(true, || Ok(v)).unwrap();
                    std::thread::sleep(Duration::from_millis(2));
                }
                v
            })
        };
        let workers: Vec<_> = (0..8)
            .map(|w| {
                let (current, runs, r) = (current.clone(), runs.clone(), r.clone());
                std::thread::spawn(move || {
                    let user = if w % 2 == 0 { "alice" } else { "bob" };
                    let claims = Claims {
                        user: user.into(),
                        ..Default::default()
                    };
                    let mut seen = std::collections::BTreeSet::new();
                    for _ in 0..400 {
                        let gen = current.get();
                        let key = panel_key(&r, Some(&claims));
                        let (out, _) = gen.cache.run(&key, None, |_| {
                            runs.fetch_add(1, Ordering::SeqCst);
                            // The "session": what the panel reads is what the
                            // request's SETs put there.
                            let sets = request_sets(&r, Some(&claims), None);
                            let who = sets
                                .iter()
                                .find_map(|s| s.strip_prefix("SET VARIABLE __claim_user = "))
                                .map(|l| l.trim_matches('\'').to_string())
                                .unwrap_or_default();
                            Ok(format!("{who}@{}", gen.snap))
                        });
                        let out = out.unwrap();
                        let (who, snap) = out.split_once('@').unwrap();
                        assert_eq!(who, user, "cross-user answer {out}");
                        assert_eq!(
                            snap.parse::<u32>().unwrap(),
                            gen.snap,
                            "cross-snapshot answer"
                        );
                        seen.insert(gen.snap);
                    }
                    seen.len()
                })
            })
            .collect();
        let snapshots_seen: Vec<usize> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        stop.store(true, Ordering::SeqCst);
        let last = refresher.join().unwrap();
        assert!(last > 2, "the snapshot was refreshed during the run");
        assert!(
            snapshots_seen.iter().any(|n| *n > 1),
            "requests spanned refreshes"
        );
        // cached: far fewer executions than requests, but at least one per user
        let n = runs.load(Ordering::SeqCst);
        assert!((2..8 * 400).contains(&n), "{n} executions");
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

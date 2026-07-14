//! In-process HTTP server behind `SELECT anofox_serve(port)`. Serves the
//! embedded browser builder and a `/query` bridge backed by the **live** DuckDB
//! session — a fresh `duckdb_connect` per request (connections aren't
//! thread-safe), all via the C API (no libduckdb linking).

use crate::api;
use crate::ffi::*;
use include_dir::{include_dir, Dir};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use tiny_http::{Header, Method, Response, Server};

// The browser builder, embedded at compile time (needs web/pkg from wasm-pack).
static WEB: Dir = include_dir!("$CARGO_MANIFEST_DIR/../web");
static STARTED: AtomicBool = AtomicBool::new(false);
static CONN: AtomicUsize = AtomicUsize::new(0);

/// Stash a live connection at load time. The server processes requests serially,
/// so reusing one connection is safe (and the init-time handle is known-good —
/// re-`connect`ing the raw db handle from a worker thread doesn't work).
pub unsafe fn set_conn(conn: duckdb_connection) {
    CONN.store(conn as usize, Ordering::SeqCst);
}

/// Start the server (once) on a background thread and open the browser.
pub fn start(port: u16) -> String {
    if STARTED.swap(true, Ordering::SeqCst) {
        return "anofox-visualization is already serving".to_string();
    }
    let addr = format!("127.0.0.1:{port}");
    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            STARTED.store(false, Ordering::SeqCst);
            return format!("anofox-visualization: could not bind {addr}: {e}");
        }
    };
    let url = format!("http://{addr}/");
    let open_url = url.clone();
    thread::spawn(move || {
        let _ = open::that(&open_url);
        for req in server.incoming_requests() {
            handle(req);
        }
    });
    format!("anofox-visualization serving {url} — open it in your browser")
}

fn handle(mut req: tiny_http::Request) {
    let url = req.url().to_string();
    if req.method() == &Method::Post && url.starts_with("/query") {
        let mut sql = String::new();
        let _ = std::io::Read::read_to_string(req.as_reader(), &mut sql);
        let resp = match unsafe { query_json(&sql) } {
            Ok(json) => Response::from_string(json).with_header(ct("application/json")),
            Err(e) => Response::from_string(e).with_header(ct("text/plain")).with_status_code(400),
        };
        let _ = req.respond(resp);
        return;
    }
    // static asset
    let path = url.trim_start_matches('/').split('?').next().unwrap_or("");
    let path = if path.is_empty() { "index.html" } else { path };
    match WEB.get_file(path) {
        Some(f) => {
            let _ = req.respond(Response::from_data(f.contents()).with_header(ctype(path)));
        }
        None => {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// Run SQL on a fresh connection to the live DB. `SELECT`s are wrapped so DuckDB
/// itself emits the rows as a JSON array (typed — numbers stay numeric); other
/// statements (setup like `CREATE`) run for effect and return `[]`.
unsafe fn query_json(sql: &str) -> Result<String, String> {
    let conn = CONN.load(Ordering::SeqCst) as duckdb_connection;
    if conn.is_null() {
        return Err("no live connection".into());
    }
    // The client inlines its variables, so a body can be `SET VARIABLE …; SELECT
    // …`. Run the leading statements for effect on this (serially-used)
    // connection, then operate on the final one — so the request is self-contained.
    let stmts = split_statements(sql);
    let (last, lead) = match stmts.split_last() {
        Some(x) => x,
        None => return Ok("[]".into()),
    };
    for s in lead {
        let cs = CString::new(s.as_str()).map_err(|_| "sql contains NUL")?;
        let mut r: duckdb_result = std::mem::zeroed();
        let rc = (api().duckdb_query.unwrap())(conn, cs.as_ptr(), &mut r);
        (api().duckdb_destroy_result.unwrap())(&mut r);
        if rc != duckdb_state::DuckDBSuccess {
            return Err(format!("statement failed: {s}"));
        }
    }
    let trimmed = last.trim().trim_end_matches(';');
    let head = trimmed.get(..6).unwrap_or("").to_ascii_uppercase();
    let is_select = head.starts_with("SELECT") || head.starts_with("WITH") || trimmed.starts_with('(');
    let wrapped = if is_select {
        format!("SELECT COALESCE(to_json(array_agg(__t)), '[]')::VARCHAR FROM ({trimmed}) __t")
    } else {
        trimmed.to_string()
    };

    let c_sql = CString::new(wrapped).map_err(|_| "sql contains NUL")?;
    let mut result: duckdb_result = std::mem::zeroed();
    let rc = (api().duckdb_query.unwrap())(conn, c_sql.as_ptr(), &mut result);
    if rc != duckdb_state::DuckDBSuccess {
        let err = (api().duckdb_result_error.unwrap())(&mut result);
        let msg = if err.is_null() {
            "query failed".into()
        } else {
            CStr::from_ptr(err).to_string_lossy().into_owned()
        };
        (api().duckdb_destroy_result.unwrap())(&mut result);
        return Err(msg);
    }

    let mut json = "[]".to_string();
    if is_select {
        let chunk = (api().duckdb_fetch_chunk.unwrap())(result);
        if !chunk.is_null() {
            if (api().duckdb_data_chunk_get_size.unwrap())(chunk) > 0 {
                let vec = (api().duckdb_data_chunk_get_vector.unwrap())(chunk, 0);
                let data = (api().duckdb_vector_get_data.unwrap())(vec) as *const duckdb_string_t;
                if !data.is_null() {
                    json = read_duckdb_string(&*data);
                }
            }
            let mut ch = chunk;
            (api().duckdb_destroy_data_chunk.unwrap())(&mut ch);
        }
    }
    (api().duckdb_destroy_result.unwrap())(&mut result);
    Ok(json)
}

/// Read a `duckdb_string_t` (short strings are inlined, ≤12 bytes).
unsafe fn read_duckdb_string(s: &duckdb_string_t) -> String {
    let len = s.value.pointer.length as usize;
    let ptr = if len <= 12 {
        s.value.inlined.inlined.as_ptr() as *const u8
    } else {
        s.value.pointer.ptr as *const u8
    };
    String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned()
}

/// Run a statement for effect, surfacing DuckDB's error text on failure.
unsafe fn exec(conn: duckdb_connection, sql: &str) -> Result<(), String> {
    let c = CString::new(sql).map_err(|_| "sql contains NUL".to_string())?;
    let mut r: duckdb_result = std::mem::zeroed();
    let rc = (api().duckdb_query.unwrap())(conn, c.as_ptr(), &mut r);
    let res = if rc == duckdb_state::DuckDBSuccess {
        Ok(())
    } else {
        let err = (api().duckdb_result_error.unwrap())(&mut r);
        Err(if err.is_null() {
            format!("statement failed: {sql}")
        } else {
            CStr::from_ptr(err).to_string_lossy().into_owned()
        })
    };
    (api().duckdb_destroy_result.unwrap())(&mut r);
    res
}

/// Fetch the first cell of a query as a String (for `current_database()`).
unsafe fn scalar_string(conn: duckdb_connection, sql: &str) -> Option<String> {
    let c = CString::new(sql).ok()?;
    let mut result: duckdb_result = std::mem::zeroed();
    if (api().duckdb_query.unwrap())(conn, c.as_ptr(), &mut result) != duckdb_state::DuckDBSuccess {
        (api().duckdb_destroy_result.unwrap())(&mut result);
        return None;
    }
    let mut out = None;
    let chunk = (api().duckdb_fetch_chunk.unwrap())(result);
    if !chunk.is_null() {
        if (api().duckdb_data_chunk_get_size.unwrap())(chunk) > 0 {
            let vec = (api().duckdb_data_chunk_get_vector.unwrap())(chunk, 0);
            let data = (api().duckdb_vector_get_data.unwrap())(vec) as *const duckdb_string_t;
            if !data.is_null() {
                out = Some(read_duckdb_string(&*data));
            }
        }
        let mut ch = chunk;
        (api().duckdb_destroy_data_chunk.unwrap())(&mut ch);
    }
    (api().duckdb_destroy_result.unwrap())(&mut result);
    out
}

/// Defense-in-depth for the locked serve mode: replace the served connection
/// with a genuinely **read-only** one. `access_mode` can't be flipped at runtime
/// and a read-only `ATTACH` would leave the original writable DB reachable by
/// qualified name — so instead we snapshot the live database to a temp file
/// (`COPY FROM DATABASE`) and reopen *that* through a fresh read-only handle with
/// no writable database attached. A gate bypass therefore still cannot write.
/// Returns the snapshot path. (Serves a snapshot; re-run to refresh.)
unsafe fn switch_to_readonly_snapshot(port: u16) -> Result<String, String> {
    let rw = CONN.load(Ordering::SeqCst) as duckdb_connection;
    if rw.is_null() {
        return Err("no live connection to snapshot".into());
    }
    let src = scalar_string(rw, "SELECT current_database()").unwrap_or_else(|| "memory".into());
    let snap = std::env::temp_dir().join(format!("anofox_ro_snap_{port}.db"));
    let snap = snap.to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&snap);
    let _ = std::fs::remove_file(format!("{snap}.wal"));

    // Snapshot on the live (read-write) connection.
    exec(rw, &format!("ATTACH '{snap}' AS __anofox_ro_snap__"))?;
    exec(rw, &format!("COPY FROM DATABASE \"{src}\" TO __anofox_ro_snap__"))?;
    exec(rw, "DETACH __anofox_ro_snap__")?;

    // Open the snapshot in a brand-new database handle, read-only.
    let mut cfg: duckdb_config = std::ptr::null_mut();
    if (api().duckdb_create_config.unwrap())(&mut cfg) != duckdb_state::DuckDBSuccess {
        return Err("could not create db config".into());
    }
    let key = CString::new("access_mode").unwrap();
    let val = CString::new("READ_ONLY").unwrap();
    (api().duckdb_set_config.unwrap())(cfg, key.as_ptr(), val.as_ptr());
    let path_c = CString::new(snap.clone()).map_err(|_| "bad snapshot path".to_string())?;
    let mut db: duckdb_database = std::ptr::null_mut();
    let mut err: *mut std::os::raw::c_char = std::ptr::null_mut();
    let rc = (api().duckdb_open_ext.unwrap())(path_c.as_ptr(), &mut db, cfg, &mut err);
    (api().duckdb_destroy_config.unwrap())(&mut cfg);
    if rc != duckdb_state::DuckDBSuccess {
        let msg = if err.is_null() {
            "read-only open failed".into()
        } else {
            CStr::from_ptr(err).to_string_lossy().into_owned()
        };
        return Err(msg);
    }
    let mut roconn: duckdb_connection = std::ptr::null_mut();
    if (api().duckdb_connect.unwrap())(db, &mut roconn) != duckdb_state::DuckDBSuccess {
        return Err("could not connect to read-only snapshot".into());
    }
    // The server now runs every /query on this read-only connection; keep the db
    // + connection for the process lifetime (the server never stops).
    set_conn(roconn);
    Ok(snap)
}

// ---------- gated served mode (server owns the SQL; consumers pick id + params) ----------

struct Dash {
    id: String,
    title: String,
    sql: String,
}
static DASHBOARDS: std::sync::OnceLock<Vec<Dash>> = std::sync::OnceLock::new();
static ALLOWED: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();

/// `SELECT anofox_serve_dashboards(dir, port)` — serve the real browser client
/// LOCKED: dashboards (`.sql` files in `dir`) live server-side; the client can
/// render everything but only *run* the queries the server registered (plus the
/// `SET VARIABLE`s it inlines). Stateless per request → multi-user safe.
pub fn start_dashboards(port: u16, dir: &str) -> String {
    if STARTED.swap(true, Ordering::SeqCst) {
        return "anofox-visualization is already serving".to_string();
    }
    let mut dashboards = Vec::new();
    let mut allowed = std::collections::HashSet::new();
    allowed.insert("SELECT 1 AS ok".to_string()); // the client's backend probe
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("sql") {
                continue;
            }
            let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let Ok(sql) = std::fs::read_to_string(&p) else { continue };
            let title = sql
                .lines()
                .find_map(|l| l.trim().strip_prefix("-- @title "))
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| id.clone());
            // Register every planned statement's SQL as an allowed query.
            for panel in anofox_visualization::sql::plan(&sql) {
                allowed.insert(panel.sql.trim().to_string());
            }
            dashboards.push(Dash { id, title, sql });
        }
    }
    dashboards.sort_by(|a, b| a.id.cmp(&b.id));
    let n = dashboards.len();
    let _ = DASHBOARDS.set(dashboards);
    let _ = ALLOWED.set(allowed);

    // Defense-in-depth: serve from a read-only snapshot, not the live read-write
    // session. If we can't establish that, refuse to serve rather than silently
    // exposing a writable database to consumers.
    if let Err(e) = unsafe { switch_to_readonly_snapshot(port) } {
        STARTED.store(false, Ordering::SeqCst);
        return format!(
            "anofox-visualization: refusing to serve — could not establish a read-only snapshot: {e}"
        );
    }

    let addr = format!("127.0.0.1:{port}");
    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            STARTED.store(false, Ordering::SeqCst);
            return format!("anofox-visualization: could not bind {addr}: {e}");
        }
    };
    let url = format!("http://{addr}/");
    thread::spawn(move || {
        for req in server.incoming_requests() {
            handle_served(req);
        }
    });
    format!("anofox-visualization serving {n} locked, read-only dashboard(s) at {url} — server owns the SQL")
}

fn handle_served(mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    if req.method() == &Method::Post && path == "/query" {
        let mut sql = String::new();
        let _ = std::io::Read::read_to_string(req.as_reader(), &mut sql);
        let resp = if gate(&sql) {
            match unsafe { query_json(&sql) } {
                Ok(json) => Response::from_string(json).with_header(ct("application/json")),
                Err(e) => Response::from_string(e).with_header(ct("text/plain")).with_status_code(400),
            }
        } else {
            Response::from_string("query not permitted").with_status_code(403)
        };
        let _ = req.respond(resp);
        return;
    }
    if path == "/" {
        let _ = req.respond(Response::from_string(list_page()).with_header(ct("text/html; charset=utf-8")));
        return;
    }
    if let Some(id) = path.strip_prefix("/d/") {
        let id = id.trim_end_matches('/');
        match DASHBOARDS.get().and_then(|d| d.iter().find(|x| x.id == id)) {
            Some(dash) => {
                let _ = req.respond(Response::from_data(served_index(dash)).with_header(ct("text/html; charset=utf-8")));
            }
            None => {
                let _ = req.respond(Response::from_string("no such dashboard").with_status_code(404));
            }
        }
        return;
    }
    let asset = path.trim_start_matches('/');
    match WEB.get_file(asset) {
        Some(f) => {
            let _ = req.respond(Response::from_data(f.contents()).with_header(ctype(asset)));
        }
        None => {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// Serve index.html with `window.__served = {id,title,sql}` injected before the
/// app script runs. serde_json handles the escaping.
fn served_index(dash: &Dash) -> Vec<u8> {
    let html = WEB
        .get_file("index.html")
        .map(|f| String::from_utf8_lossy(f.contents()).into_owned())
        .unwrap_or_default();
    // The full dashboard list (id + title) so the client can draw a persistent
    // cross-dashboard nav bar linking each `/d/<id>`.
    let nav: Vec<serde_json::Value> = DASHBOARDS
        .get()
        .map(|ds| {
            ds.iter()
                .map(|d| serde_json::json!({ "id": d.id, "title": d.title }))
                .collect()
        })
        .unwrap_or_default();
    let cfg = serde_json::json!({
        "id": dash.id, "title": dash.title, "sql": dash.sql, "nav": nav
    })
    .to_string();
    // <base href="/"> so ./app.js and ./pkg/… resolve to the root (the page lives
    // at /d/<id>); the script hands the dashboard to the client.
    html.replacen("<head>", "<head><base href=\"/\">", 1)
        .replacen("</head>", &format!("<script>window.__served={cfg};</script></head>"), 1)
        .into_bytes()
}

fn list_page() -> String {
    let items: String = DASHBOARDS
        .get()
        .map(|ds| {
            ds.iter()
                .map(|d| format!("<li><a href=\"/d/{}\">{}</a></li>", d.id, html_esc(&d.title)))
                .collect()
        })
        .unwrap_or_default();
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Dashboards</title>\
<body style=\"font:15px system-ui;padding:2rem\"><h1>Dashboards</h1><ul>{items}</ul>"
    )
}

fn html_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Allow a `/query` body only if it's `‹validated SET VARIABLEs›; ‹registered
/// query›` (or a lone validated SET VARIABLE / the probe). The client inlines its
/// variables, so this is the whole surface — no arbitrary SQL runs.
fn gate(sql: &str) -> bool {
    let Some(allowed) = ALLOWED.get() else { return false };
    let stmts = split_statements(sql);
    let Some((last, lead)) = stmts.split_last() else { return false };
    if !lead.iter().all(|s| is_set_variable(s)) {
        return false;
    }
    is_set_variable(last) || allowed.contains(last.trim())
}

/// A single `SET VARIABLE <name> = <literal>` — value must be a literal (string,
/// number, bool/null, or a bracketed list), never a subquery.
fn is_set_variable(s: &str) -> bool {
    let s = s.trim();
    let up = s.to_ascii_uppercase();
    let Some(rest) = up.strip_prefix("SET VARIABLE ") else { return false };
    let Some(eq) = rest.find('=') else { return false };
    let name = rest[..eq].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    let val = s[s.find('=').unwrap() + 1..].trim();
    matches!(val.chars().next(), Some('\'') | Some('[') | Some('-') | Some('0'..='9'))
        || val == "TRUE"
        || val == "FALSE"
        || val == "NULL"
}

/// Split on `;` that are outside string literals (`'…''…'`) and bracket lists.
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut depth = 0i32;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if in_str => {
                if chars.peek() == Some(&'\'') {
                    cur.push(c);
                    cur.push(chars.next().unwrap());
                } else {
                    in_str = false;
                    cur.push(c);
                }
            }
            '\'' => {
                in_str = true;
                cur.push(c);
            }
            '[' if !in_str => {
                depth += 1;
                cur.push(c);
            }
            ']' if !in_str => {
                depth -= 1;
                cur.push(c);
            }
            ';' if !in_str && depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn ct(v: &str) -> Header {
    Header::from_bytes(&b"Content-Type"[..], v.as_bytes()).unwrap()
}

fn ctype(path: &str) -> Header {
    ct(match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    })
}

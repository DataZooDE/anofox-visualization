//! In-process HTTP server behind `SELECT duckplot_serve(port)`. Serves the
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
        return "duckplot is already serving".to_string();
    }
    let addr = format!("127.0.0.1:{port}");
    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            STARTED.store(false, Ordering::SeqCst);
            return format!("duckplot: could not bind {addr}: {e}");
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
    format!("duckplot serving {url} — open it in your browser")
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
    let trimmed = sql.trim().trim_end_matches(';');
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

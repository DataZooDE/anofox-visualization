//! `anofox-visualization serve [db] [--port N] [--no-open]` — launch the browser dashboard
//! builder wired to a **live** DuckDB. Serves the embedded `web/` UI and a
//! `/query` endpoint that runs SQL through the `duckdb` CLI against a shared
//! database file (so `CREATE`d tables persist across panel queries).
//!
//! The same UI falls back to DuckDB-Wasm when opened as a static file, so this
//! adds "explore my live data" without changing the builder.

use include_dir::{include_dir, Dir};
use std::process::Command;
use tiny_http::{Header, Method, Response, Server};

// Embedded at compile time — run `wasm-pack build … --out-dir web/pkg` first.
static WEB: Dir = include_dir!("$CARGO_MANIFEST_DIR/web");

fn main() {
    let mut port = 8080u16;
    let mut open_browser = true;
    let mut db = String::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            "--no-open" => open_browser = false,
            _ => db = a,
        }
    }
    // Default to a temp DB so setup statements persist across requests.
    if db.is_empty() {
        db = std::env::temp_dir()
            .join("anofox_serve.db")
            .to_string_lossy()
            .to_string();
    }

    let addr = format!("127.0.0.1:{port}");
    let server = Server::http(&addr).unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    let url = format!("http://{addr}/");
    println!("anofox-visualization serving {url}\n  database: {db}\n  (Ctrl-C to stop)");
    if open_browser {
        let _ = open::that(&url);
    }

    for req in server.incoming_requests() {
        let is_query = req.method() == &Method::Post && req.url().starts_with("/query");
        if is_query {
            handle_query(req, &db);
        } else {
            handle_static(req);
        }
    }
}

/// POST /query with a SQL body → JSON rows (or 400 + error text).
fn handle_query(mut req: tiny_http::Request, db: &str) {
    let mut sql = String::new();
    let _ = std::io::Read::read_to_string(req.as_reader(), &mut sql);
    let out = Command::new("duckdb")
        .arg(db)
        .arg("-json")
        .arg("-c")
        .arg(&sql)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let body = String::from_utf8_lossy(&o.stdout);
            let body = if body.trim().is_empty() {
                "[]"
            } else {
                body.trim()
            };
            let _ = req.respond(Response::from_string(body).with_header(json_header()));
        }
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr).to_string();
            let _ = req.respond(Response::from_string(msg).with_status_code(400));
        }
        Err(e) => {
            let _ =
                req.respond(Response::from_string(format!("duckdb: {e}")).with_status_code(500));
        }
    }
}

/// Serve an embedded asset; `/` → index.html.
fn handle_static(req: tiny_http::Request) {
    let path = req.url().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match WEB.get_file(path) {
        Some(f) => {
            let resp = Response::from_data(f.contents()).with_header(content_type(path));
            let _ = req.respond(resp);
        }
        None => {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
}

fn content_type(path: &str) -> Header {
    let ct = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    Header::from_bytes(&b"Content-Type"[..], ct.as_bytes()).unwrap()
}

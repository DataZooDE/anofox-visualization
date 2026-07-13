//! `anofox-visualization serve` — two modes:
//!
//! **Authoring** (default): `serve [db]` launches the embedded browser builder
//! wired to a live DuckDB via a `/query` endpoint that runs client-supplied SQL.
//! Convenient for the author on localhost; the client controls the SQL, so it is
//! NOT safe to expose to untrusted consumers.
//!
//! **Serve** (`--dashboards <dir>`): the secure, consumer-facing mode. Dashboards
//! (annotated `.sql` files) live on the server; the client only selects a
//! dashboard by id and whitelisted parameter values — it never sends SQL. Queries
//! run against a **read-only** DuckDB. See `docs/secure-serving.md`.

use anofox_visualization::{render, sql, Role};
use include_dir::{include_dir, Dir};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use tiny_http::{Header, Method, Response, Server};

// Embedded at compile time (authoring mode) — run `wasm-pack build … --out-dir
// web/pkg` first.
static WEB: Dir = include_dir!("$CARGO_MANIFEST_DIR/web");

fn main() {
    let mut port = 8080u16;
    let mut open_browser = true;
    let mut db = String::new();
    let mut dashboards_dir: Option<String> = None;
    let mut init: Option<String> = None;
    let mut bind = "127.0.0.1".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            "--no-open" => open_browser = false,
            "--dashboards" => dashboards_dir = args.next(),
            "--init" => init = args.next(),
            "--bind" => bind = args.next().unwrap_or(bind),
            "--db" => db = args.next().unwrap_or(db),
            _ => db = a,
        }
    }

    match dashboards_dir {
        Some(dir) => serve_dashboards(&dir, db, init, &bind, port),
        None => authoring_mode(db, &bind, port, open_browser),
    }
}

// ---------- authoring mode (client sends SQL — localhost/dev only) ----------

fn authoring_mode(mut db: String, bind: &str, port: u16, open_browser: bool) {
    if db.is_empty() {
        db = std::env::temp_dir()
            .join("anofox_serve.db")
            .to_string_lossy()
            .to_string();
    }
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr).unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    let url = format!("http://{addr}/");
    println!("anofox-visualization serving {url} (authoring — client SQL; localhost only)\n  database: {db}\n  (Ctrl-C to stop)");
    if open_browser {
        let _ = open::that(&url);
    }
    for req in server.incoming_requests() {
        if req.method() == &Method::Post && req.url().starts_with("/query") {
            handle_query(req, &db);
        } else {
            handle_static(req);
        }
    }
}

/// POST /query with a SQL body → JSON rows. AUTHORING ONLY — runs arbitrary SQL.
fn handle_query(mut req: tiny_http::Request, db: &str) {
    let mut sql = String::new();
    let _ = std::io::Read::read_to_string(req.as_reader(), &mut sql);
    match run_duckdb(db, &["-json"], &sql) {
        Ok(body) => {
            let body = if body.trim().is_empty() { "[]".into() } else { body.trim().to_string() };
            let _ = req.respond(Response::from_string(body).with_header(json_header()));
        }
        Err(msg) => {
            let _ = req.respond(Response::from_string(msg).with_status_code(400));
        }
    }
}

fn handle_static(req: tiny_http::Request) {
    let raw = req.url().trim_start_matches('/').to_string();
    let path = if raw.is_empty() { "index.html" } else { raw.as_str() };
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

// ---------- serve mode (server owns the SQL; consumers pick id + params) ----------

struct Param {
    name: String,
    allowed: Vec<String>,
    default: String,
}
struct Dashboard {
    id: String,
    title: String,
    script: String,
    params: Vec<Param>,
    refresh: u32,
}

/// Parse the header-comment metadata of a dashboard `.sql`:
/// `-- @title …`, `-- @refresh <seconds>`, `-- @param name [a, b, c] = a`.
fn parse_dashboard(id: &str, script: &str) -> Dashboard {
    let mut title = id.to_string();
    let mut params = Vec::new();
    let mut refresh = 0u32;
    for line in script.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("-- @title ") {
            title = r.trim().to_string();
        } else if let Some(r) = l.strip_prefix("-- @refresh ") {
            refresh = r.trim().parse().unwrap_or(0);
        } else if let Some(r) = l.strip_prefix("-- @param ") {
            if let (Some(o), Some(c)) = (r.find('['), r.find(']')) {
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
                if !name.is_empty() && !allowed.is_empty() {
                    params.push(Param { name, allowed, default });
                }
            }
        }
    }
    Dashboard { id: id.to_string(), title, script: script.to_string(), params, refresh }
}

fn load_dashboards(dir: &Path) -> Vec<Dashboard> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("sql") {
                let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                if let Ok(script) = std::fs::read_to_string(&p) {
                    out.push(parse_dashboard(&id, &script));
                }
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn serve_dashboards(dir: &str, mut db: String, init: Option<String>, bind: &str, port: u16) {
    if db.is_empty() {
        db = std::env::temp_dir()
            .join("anofox_dashboards.db")
            .to_string_lossy()
            .to_string();
    }
    // One-time read-write setup (attach sources, create views). After this the
    // server only ever opens the database read-only.
    if let Some(init) = &init {
        let script = std::fs::read_to_string(init).unwrap_or_else(|e| panic!("read {init}: {e}"));
        run_duckdb(&db, &[], &script).unwrap_or_else(|e| panic!("init failed: {e}"));
        println!("  init applied: {init}");
    }
    let dashboards = load_dashboards(Path::new(dir));
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr).unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    println!(
        "anofox-visualization serving {} dashboard(s) at http://{addr}/ (read-only; no client SQL)\n  database: {db}\n  dashboards: {dir}\n  (put TLS + auth in front for public use — docs/secure-serving.md)",
        dashboards.len()
    );
    for req in server.incoming_requests() {
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("/");
        if path == "/" {
            respond_html(req, &list_page(&dashboards));
        } else if let Some(rest) = path.strip_prefix("/d/") {
            let id = rest.trim_end_matches('/');
            match dashboards.iter().find(|d| d.id == id) {
                Some(dash) => {
                    let chosen = parse_query(&url);
                    match render_dashboard_page(&db, dash, &chosen) {
                        Ok(html) => respond_html(req, &html),
                        Err(e) => {
                            let _ = req.respond(Response::from_string(e).with_status_code(400));
                        }
                    }
                }
                None => {
                    let _ = req.respond(Response::from_string("no such dashboard").with_status_code(404));
                }
            }
        } else {
            // No /query, no static assets, no SQL — the whole surface is id + params.
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// Render one dashboard to a view-only HTML page. Validates every parameter value
/// against its declared whitelist, binds them as DuckDB variables, and runs each
/// panel's fixed query READ-ONLY. Rejects any value not in the whitelist.
fn render_dashboard_page(
    db: &str,
    dash: &Dashboard,
    chosen: &BTreeMap<String, String>,
) -> Result<String, String> {
    // Validated variable prefix (only whitelisted values reach SQL) + controls.
    let mut prefix = String::new();
    let mut selects = String::new();
    for p in &dash.params {
        let val = chosen.get(&p.name).cloned().unwrap_or_else(|| p.default.clone());
        if !p.allowed.contains(&val) {
            return Err(format!("parameter '{}' = '{}' is not allowed", p.name, val));
        }
        prefix.push_str(&format!("SET VARIABLE {} = '{}'; ", p.name, val.replace('\'', "''")));
        selects.push_str(&format!(
            "<label>{}: <select name=\"{}\" onchange=\"this.form.submit()\">",
            esc(&p.name),
            esc(&p.name)
        ));
        for opt in &p.allowed {
            let sel = if opt == &val { " selected" } else { "" };
            selects.push_str(&format!("<option{sel}>{}</option>", esc(opt)));
        }
        selects.push_str("</select></label> ");
    }
    let controls = if dash.params.is_empty() {
        String::new()
    } else {
        format!(
            "<form class=\"controls\" method=\"get\" action=\"/d/{}\">{selects}</form>",
            esc(&dash.id)
        )
    };

    let mut panels = String::new();
    for panel in sql::plan(&dash.script) {
        if panel.setup {
            continue; // read-only mode: setup is done once at --init, not per request
        }
        // Skip interactive/layout-only directives (params drive re-render instead).
        if panel.roles.iter().any(|(_, r)| {
            matches!(
                r,
                Role::Input(_)
                    | Role::Columns
                    | Role::GroupStart
                    | Role::GroupEnd
                    | Role::Span
                    | Role::Tab
                    | Role::SubTab
            )
        }) {
            continue;
        }
        let json = run_duckdb(db, &["-readonly", "-json"], &format!("{prefix}{}", panel.sql))?;
        let rows: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(json.trim()).unwrap_or_default();
        if panel.roles.len() == 1 && matches!(panel.roles[0].1, Role::Label) {
            let text = rows
                .first()
                .and_then(|r| r.values().next())
                .and_then(|v| v.as_str())
                .unwrap_or("");
            panels.push_str(&format!("<h2 class=\"section\">{}</h2>", esc(text)));
            continue;
        }
        let cols = sql::columns_from_rows(&rows, &panel.roles);
        let svg = render(&cols, 460, 300).unwrap_or_else(|e| format!("<pre>error: {e}</pre>"));
        panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
    }

    let meta = if dash.refresh > 0 {
        format!("<meta http-equiv=\"refresh\" content=\"{}\">", dash.refresh)
    } else {
        String::new()
    };
    Ok(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{meta}<title>{title}</title><style>{STYLE}</style></head>\
<body><h1>{title}</h1>{controls}<div class=\"grid\">{panels}</div></body></html>",
        title = esc(&dash.title)
    ))
}

fn list_page(dashboards: &[Dashboard]) -> String {
    let items: String = dashboards
        .iter()
        .map(|d| format!("<li><a href=\"/d/{}\">{}</a></li>", esc(&d.id), esc(&d.title)))
        .collect();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Dashboards</title><style>{STYLE}</style></head>\
<body><h1>Dashboards</h1><ul class=\"dash-list\">{items}</ul></body></html>"
    )
}

// ---------- helpers ----------

/// Run the `duckdb` CLI with flags + a SQL string; stdout on success, stderr on error.
fn run_duckdb(db: &str, flags: &[&str], sql: &str) -> Result<String, String> {
    let mut cmd = Command::new("duckdb");
    cmd.arg(db);
    for f in flags {
        cmd.arg(f);
    }
    let out = cmd
        .arg("-c")
        .arg(sql)
        .output()
        .map_err(|e| format!("duckdb: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

/// Parse `?a=b&c=d` into a map (percent-decoded). Only used to look up declared
/// param names; unknown keys are ignored and never reach SQL.
fn parse_query(url: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(q) = url.split_once('?').map(|(_, q)| q) {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                out.insert(url_decode(k), url_decode(v));
            }
        }
    }
    out
}

fn url_decode(s: &str) -> String {
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

fn respond_html(req: tiny_http::Request, html: &str) {
    let h = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
    let _ = req.respond(Response::from_string(html).with_header(h));
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
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

const STYLE: &str = r#"body{font:15px/1.55 system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;background:#f4f6f9;color:#1f2937;margin:0;padding:2rem}
h1{font-size:1.35rem;font-weight:650;margin:0 0 1rem}
.controls{display:flex;gap:1.2rem;flex-wrap:wrap;align-items:center;background:#fff;border:1px solid #e5e7eb;border-radius:12px;padding:.7rem 1rem;margin-bottom:1.2rem;font-size:.85rem;font-weight:600;color:#6b7280}
.controls select{font:inherit;font-weight:600;color:#1f2937;border:1px solid #e5e7eb;border-radius:8px;padding:.35rem .6rem;margin-left:.3rem}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(440px,1fr));gap:1.1rem;align-content:start}
.section{grid-column:1/-1;margin:.6rem 0 -.4rem;font-size:1.1rem;font-weight:650}
.panel{margin:0;background:#fff;border:1px solid #e5e7eb;border-radius:14px;padding:1rem 1.1rem;box-shadow:0 1px 2px rgba(16,24,40,.05)}
.panel svg{width:100%;height:auto;display:block}
.dash-list a{color:#1f8ca6;font-weight:600;text-decoration:none}.dash-list a:hover{text-decoration:underline}"#;

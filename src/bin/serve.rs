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
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
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
    let mut cache_secs = 0u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            "--no-open" => open_browser = false,
            "--dashboards" => dashboards_dir = args.next(),
            "--init" => init = args.next(),
            "--bind" => bind = args.next().unwrap_or(bind),
            "--cache" => cache_secs = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--db" => db = args.next().unwrap_or(db),
            _ => db = a,
        }
    }

    match dashboards_dir {
        Some(dir) => serve_dashboards(&dir, db, init, &bind, port, Duration::from_secs(cache_secs)),
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
    /// DuckDB extensions to `LOAD` per read-only session (e.g. `anofox_forecast`),
    /// so a panel can call `ts_forecast_by(...)` live. Server-declared, not client.
    loads: Vec<String>,
}

/// Parse the header-comment metadata of a dashboard `.sql`:
/// `-- @title …`, `-- @refresh <seconds>`, `-- @param name [a, b, c] = a`,
/// `-- @load <extension>`.
fn parse_dashboard(id: &str, script: &str) -> Dashboard {
    let mut title = id.to_string();
    let mut params = Vec::new();
    let mut refresh = 0u32;
    let mut loads = Vec::new();
    for line in script.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("-- @title ") {
            title = r.trim().to_string();
        } else if let Some(r) = l.strip_prefix("-- @refresh ") {
            refresh = r.trim().parse().unwrap_or(0);
        } else if let Some(r) = l.strip_prefix("-- @load ") {
            let ext = r.trim();
            // Server-authored, but keep it an identifier so a stray file can't
            // smuggle SQL into the LOAD.
            if !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                loads.push(ext.to_string());
            }
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
    Dashboard { id: id.to_string(), title, script: script.to_string(), params, refresh, loads }
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

fn serve_dashboards(
    dir: &str,
    mut db: String,
    init: Option<String>,
    bind: &str,
    port: u16,
    cache_ttl: Duration,
) {
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
    let cache_note = if cache_ttl.is_zero() {
        "off".to_string()
    } else {
        format!("{}s", cache_ttl.as_secs())
    };
    println!(
        "anofox-visualization serving {} dashboard(s) at http://{addr}/ (read-only; no client SQL; cache: {cache_note})\n  database: {db}\n  dashboards: {dir}\n  (put TLS + auth in front for public use — docs/secure-serving.md)",
        dashboards.len()
    );

    // Per-(dashboard + resolved params) rendered-page cache. The loop is
    // single-threaded, so a plain map is fine. TTL doubles as the freshness knob:
    // within it, N viewers of the same view share one render → no extra DB load.
    let mut cache: HashMap<String, (Instant, String)> = HashMap::new();

    for req in server.incoming_requests() {
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("/");
        if path == "/" {
            respond_html(req, &list_page(&dashboards));
        } else if let Some(rest) = path.strip_prefix("/d/") {
            let id = rest.trim_end_matches('/');
            let Some(dash) = dashboards.iter().find(|d| d.id == id) else {
                let _ = req.respond(Response::from_string("no such dashboard").with_status_code(404));
                continue;
            };
            // Validate params against the whitelist (unknown/disallowed → 400).
            let resolved = match resolve_params(dash, &parse_query(&url)) {
                Ok(r) => r,
                Err(e) => {
                    let _ = req.respond(Response::from_string(e).with_status_code(400));
                    continue;
                }
            };
            let key = cache_key(&dash.id, &resolved);
            if !cache_ttl.is_zero() {
                if let Some(html) = cache
                    .get(&key)
                    .filter(|(ts, _)| ts.elapsed() < cache_ttl)
                    .map(|(_, h)| h.clone())
                {
                    respond_html(req, &html); // cache hit — no DB touched
                    continue;
                }
            }
            match render_dashboard_page(&db, dash, &resolved) {
                Ok(html) => {
                    if !cache_ttl.is_zero() {
                        cache.insert(key, (Instant::now(), html.clone()));
                    }
                    respond_html(req, &html);
                }
                Err(e) => {
                    let _ = req.respond(Response::from_string(e).with_status_code(400));
                }
            }
        } else {
            // No /query, no static assets, no SQL — the whole surface is id + params.
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// Validate the requested params against each dashboard's whitelist, filling
/// defaults. Returns `(name, value)` in declaration order, or an error for any
/// value not in the declared list. The client can only choose allowed values.
fn resolve_params(
    dash: &Dashboard,
    chosen: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for p in &dash.params {
        let val = chosen.get(&p.name).cloned().unwrap_or_else(|| p.default.clone());
        if !p.allowed.contains(&val) {
            return Err(format!("parameter '{}' = '{}' is not allowed", p.name, val));
        }
        out.push((p.name.clone(), val));
    }
    Ok(out)
}

fn cache_key(id: &str, resolved: &[(String, String)]) -> String {
    let params: Vec<String> = resolved.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{id}|{}", params.join("&"))
}

/// Render one dashboard to a view-only HTML page. Validates every parameter value
/// against its declared whitelist, binds them as DuckDB variables, and runs each
/// panel's fixed query READ-ONLY. Rejects any value not in the whitelist.
fn render_dashboard_page(
    db: &str,
    dash: &Dashboard,
    resolved: &[(String, String)],
) -> Result<String, String> {
    // Bind the (already whitelisted) values as DuckDB variables, and build a
    // <select> per param.
    let value_of = |name: &str| -> String {
        resolved
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let mut prefix = String::new();
    for ext in &dash.loads {
        prefix.push_str(&format!("LOAD {ext}; ")); // e.g. anofox_forecast (per session)
    }
    for (name, val) in resolved {
        prefix.push_str(&format!("SET VARIABLE {} = '{}'; ", name, val.replace('\'', "''")));
    }
    let mut selects = String::new();
    for p in &dash.params {
        let val = value_of(&p.name);
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

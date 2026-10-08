//! `anofox-visualization serve` — two modes:
//!
//! **Authoring** (default): `serve [db]` launches the embedded browser builder
//! wired to a DuckDB file via a `/query` endpoint that runs client-supplied SQL
//! (through the `duckdb` CLI). Because it runs arbitrary SQL it is locked to
//! the author: loopback bind only, the `Host` header must be loopback (DNS
//! rebinding), cross-origin requests are refused (CSRF), and every request needs
//! the random per-run token from the printed URL (`?token=` → `HttpOnly;
//! SameSite=Strict` cookie, or the `X-Anofox-Token` header). Bodies containing
//! CLI dot-commands (`.shell`, `.read`, …) are refused before reaching the CLI.
//!
//! **Serve** (`--dashboards <dir>`): the secure, consumer-facing mode. Dashboards
//! (annotated `.sql` files) live on the server; the client only selects a
//! dashboard by id and whitelisted parameter values — it never sends SQL. Queries
//! run `duckdb -readonly` with external access disabled and the configuration
//! locked after the server-declared `-- @load`s, under a row cap and a timeout.
//! See `docs/secure-serving.md`.

use anofox_visualization::host::serving::{
    self as sv, html_escape as esc, sql_string_literal, AuthoringGuard, RequestMeta,
};
use anofox_visualization::{render, sql, Role};
use include_dir::{include_dir, Dir};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

// Embedded at compile time (authoring mode) — run `wasm-pack build … --out-dir
// web/pkg` first.
static WEB: Dir = include_dir!("$CARGO_MANIFEST_DIR/web");

/// Max authoring `/query` body.
const MAX_BODY: usize = 4 << 20;

struct Opts {
    port: u16,
    open_browser: bool,
    db: String,
    dashboards_dir: Option<String>,
    init: Option<String>,
    bind: String,
    cache_secs: u64,
    max_rows: usize,
    timeout: Duration,
    threads: usize,
}

fn usage(msg: &str) -> ! {
    eprintln!(
        "error: {msg}\n\nusage: serve [--db] <db> [--port N] [--no-open]                (authoring, loopback only)\n       serve --dashboards <dir> [--db <db>] [--init setup.sql] [--bind ADDR] [--port N]\n             [--cache SECS] [--max-rows N] [--timeout SECS] [--threads N]"
    );
    std::process::exit(2)
}

fn parse_args() -> Opts {
    let mut o = Opts {
        port: 8080,
        open_browser: true,
        db: String::new(),
        dashboards_dir: None,
        init: None,
        bind: "127.0.0.1".into(),
        cache_secs: 0,
        max_rows: 100_000,
        timeout: Duration::from_secs(30),
        threads: 4,
    };
    let mut args = std::env::args().skip(1);
    let num = |v: Option<String>, what: &str| -> u64 {
        v.and_then(|v| v.parse().ok())
            .unwrap_or_else(|| usage(&format!("{what} needs a number")))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => {
                let p = num(args.next(), "--port");
                o.port = u16::try_from(p)
                    .ok()
                    .filter(|p| *p != 0)
                    .unwrap_or_else(|| usage("--port must be 1..=65535"));
            }
            "--no-open" => o.open_browser = false,
            "--dashboards" => o.dashboards_dir = args.next(),
            "--init" => o.init = args.next(),
            "--bind" => {
                o.bind = args
                    .next()
                    .unwrap_or_else(|| usage("--bind needs an address"))
            }
            "--cache" => o.cache_secs = num(args.next(), "--cache"),
            "--max-rows" => o.max_rows = num(args.next(), "--max-rows").max(1) as usize,
            "--timeout" => o.timeout = Duration::from_secs(num(args.next(), "--timeout").max(1)),
            "--threads" => o.threads = num(args.next(), "--threads").clamp(1, 64) as usize,
            "--db" => o.db = args.next().unwrap_or_else(|| usage("--db needs a path")),
            s if s.starts_with('-') => usage(&format!("unknown option {s}")),
            _ => o.db = a,
        }
    }
    o
}

fn main() {
    let o = parse_args();
    match o.dashboards_dir.clone() {
        Some(dir) => serve_dashboards(&dir, &o),
        None => authoring_mode(&o),
    }
}

// ---------- shared helpers ----------

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

fn text(req: Request, status: u16, msg: &str) {
    respond(
        req,
        status,
        "text/plain; charset=utf-8",
        msg.as_bytes().to_vec(),
    );
}

/// Run `n` worker threads pulling requests off one server.
fn run_workers(server: Server, n: usize, handler: impl Fn(Request) + Send + Sync + 'static) {
    let server = Arc::new(server);
    let handler = Arc::new(handler);
    let workers: Vec<_> = (0..n.max(1))
        .map(|_| {
            let (server, handler) = (server.clone(), handler.clone());
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    handler(req);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}

/// A private (0700), randomly named directory under the temp dir.
fn private_dir(prefix: &str) -> PathBuf {
    let base = std::env::temp_dir();
    for _ in 0..8 {
        let p = base.join(format!("{prefix}-{}", sv::random_token()));
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        if b.create(&p).is_ok() {
            return p;
        }
    }
    eprintln!("error: could not create a private temp directory");
    std::process::exit(1)
}

// ---------- authoring mode (client sends SQL — the author only) ----------

fn authoring_mode(o: &Opts) {
    // Never expose a client-SQL endpoint beyond this machine.
    if !sv::is_loopback_bind(&o.bind) {
        eprintln!(
            "error: authoring mode runs arbitrary SQL and only binds to loopback (127.0.0.1 / ::1); \
             use `--dashboards <dir>` to serve consumers"
        );
        std::process::exit(2);
    }
    let db = if o.db.is_empty() {
        private_dir("anofox-serve")
            .join("authoring.duckdb")
            .to_string_lossy()
            .to_string()
    } else {
        o.db.clone()
    };
    let addr = format!("{}:{}", o.bind, o.port);
    let server = Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("error: bind {addr}: {e}");
        std::process::exit(1)
    });
    let guard = AuthoringGuard::new(o.port);
    let host = if o.bind.contains(':') {
        format!("[{}]", o.bind)
    } else {
        o.bind.clone()
    };
    let url = guard.login_url(&host);
    println!(
        "anofox-visualization serving {url}\n  (authoring — client SQL; loopback only; the URL carries this run's token)\n  database: {db}\n  (Ctrl-C to stop)"
    );
    if o.open_browser {
        let _ = open::that(&url);
    }
    run_workers(server, 1, move |req| handle_authoring(&guard, &db, req));
}

fn handle_authoring(guard: &AuthoringGuard, db: &str, mut req: Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let m = meta(&req);
    if guard.check_origin(&m).is_ok() {
        if let Some((location, cookie)) = guard.bootstrap(&url) {
            let resp = Response::empty(303)
                .with_header(header("Location", &location))
                .with_header(header("Set-Cookie", &cookie))
                .with_header(header("Cache-Control", "no-store"));
            let _ = req.respond(resp);
            return;
        }
    }
    if let Err(d) = guard.check(&m) {
        return text(req, d.status(), d.message());
    }
    if path == "/query" {
        if req.method() != &Method::Post {
            return text(req, 405, "POST only");
        }
        let mut buf = Vec::new();
        let _ = req
            .as_reader()
            .take(MAX_BODY as u64 + 1)
            .read_to_end(&mut buf);
        if buf.len() > MAX_BODY {
            return text(req, 413, "request body too large");
        }
        let Ok(sql) = String::from_utf8(buf) else {
            return text(req, 400, "body is not UTF-8");
        };
        // The CLI treats a line starting with `.` as a meta command (`.shell`,
        // `.read`, `.output` …). SQL never needs one: refuse before the CLI
        // ever sees the text.
        if sv::has_dot_command(&sql) {
            return text(req, 400, "CLI dot-commands are not allowed");
        }
        return match run_duckdb(db, &["-json"], &sql, Duration::ZERO) {
            Ok(body) => {
                let body = if body.trim().is_empty() {
                    "[]".into()
                } else {
                    body.trim().to_string()
                };
                respond(req, 200, "application/json", body.into_bytes())
            }
            Err(msg) => text(req, 400, &msg),
        };
    }
    if req.method() != &Method::Get {
        return text(req, 405, "method not allowed");
    }
    let asset = path.trim_start_matches('/');
    let asset = if asset.is_empty() {
        "index.html"
    } else {
        asset
    };
    match WEB.get_file(asset) {
        Some(f) => respond(req, 200, sv::content_type(asset), f.contents().to_vec()),
        None => text(req, 404, "not found"),
    }
}

// ---------- serve mode (server owns the SQL; consumers pick id + params) ----------

struct Dashboard {
    id: String,
    meta: sv::DashboardMeta,
    script: String,
}

fn load_dashboards(dir: &Path) -> Vec<Dashboard> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("sql") {
                let id = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if !sv::valid_dashboard_id(&id) {
                    continue;
                }
                if let Ok(script) = std::fs::read_to_string(&p) {
                    out.push(Dashboard {
                        meta: sv::parse_dashboard_meta(&id, &script),
                        id,
                        script,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

struct ServeState {
    db: String,
    dashboards: Vec<Dashboard>,
    cache_ttl: Duration,
    cache: Mutex<HashMap<String, (Instant, String)>>,
    max_rows: usize,
    timeout: Duration,
}

fn serve_dashboards(dir: &str, o: &Opts) {
    let db = if o.db.is_empty() {
        private_dir("anofox-dashboards")
            .join("dashboards.duckdb")
            .to_string_lossy()
            .to_string()
    } else {
        o.db.clone()
    };
    // One-time read-write setup (attach sources, create views). After this the
    // server only ever opens the database read-only.
    if let Some(init) = &o.init {
        let script = std::fs::read_to_string(init).unwrap_or_else(|e| {
            eprintln!("error: read {init}: {e}");
            std::process::exit(1)
        });
        if sv::has_dot_command(&script) {
            eprintln!("error: {init}: CLI dot-commands are not allowed in --init scripts");
            std::process::exit(1);
        }
        if let Err(e) = run_duckdb(&db, &[], &script, Duration::ZERO) {
            eprintln!("error: init failed: {e}");
            std::process::exit(1);
        }
        println!("  init applied: {init}");
    }
    let dashboards = load_dashboards(Path::new(dir));
    let addr = format!("{}:{}", o.bind, o.port);
    let server = Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("error: bind {addr}: {e}");
        std::process::exit(1)
    });
    let cache_note = if o.cache_secs == 0 {
        "off".to_string()
    } else {
        format!("{}s", o.cache_secs)
    };
    println!(
        "anofox-visualization serving {} dashboard(s) at http://{addr}/ (read-only; no client SQL; cache: {cache_note})\n  database: {db}\n  dashboards: {dir}\n  (put TLS + auth in front for public use — docs/secure-serving.md)",
        dashboards.len()
    );
    if !sv::is_loopback_bind(&o.bind) {
        println!(
            "  note: bound to {} — reachable from the network; put a TLS/auth proxy in front",
            o.bind
        );
    }
    let st = ServeState {
        db,
        dashboards,
        cache_ttl: Duration::from_secs(o.cache_secs),
        cache: Mutex::new(HashMap::new()),
        max_rows: o.max_rows,
        timeout: o.timeout,
    };
    run_workers(server, o.threads, move |req| handle_serve(&st, req));
}

fn handle_serve(st: &ServeState, req: Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/");
    if req.method() != &Method::Get {
        return text(req, 405, "method not allowed");
    }
    if path == "/" {
        return respond(
            req,
            200,
            "text/html; charset=utf-8",
            list_page(&st.dashboards).into_bytes(),
        );
    }
    let Some(rest) = path.strip_prefix("/d/") else {
        // No /query, no static assets, no SQL — the whole surface is id + params.
        return text(req, 404, "not found");
    };
    let id = rest.trim_end_matches('/');
    let Some(dash) = st.dashboards.iter().find(|d| d.id == id) else {
        return text(req, 404, "no such dashboard");
    };
    // Validate params against the whitelist (unknown/disallowed → 400).
    let resolved = match resolve_params(dash, &sv::parse_query(&url)) {
        Ok(r) => r,
        Err(e) => return text(req, 400, &e),
    };
    let key = cache_key(&dash.id, &resolved);
    if !st.cache_ttl.is_zero() {
        let hit = st
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
            .filter(|(ts, _)| ts.elapsed() < st.cache_ttl)
            .map(|(_, h)| h.clone());
        if let Some(html) = hit {
            return respond(req, 200, "text/html; charset=utf-8", html.into_bytes());
            // no DB touched
        }
    }
    match render_dashboard_page(st, dash, &resolved) {
        Ok(html) => {
            if !st.cache_ttl.is_zero() {
                st.cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(key, (Instant::now(), html.clone()));
            }
            respond(req, 200, "text/html; charset=utf-8", html.into_bytes())
        }
        Err(e) => text(req, 400, &e),
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
    for p in &dash.meta.params {
        let val = chosen
            .get(&p.name)
            .cloned()
            .unwrap_or_else(|| p.default.clone());
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

/// Render one dashboard to a view-only HTML page: binds the (whitelisted)
/// values as DuckDB variables and runs each panel's fixed query READ-ONLY, with
/// external access disabled and the configuration locked after the
/// server-declared `-- @load`s.
fn render_dashboard_page(
    st: &ServeState,
    dash: &Dashboard,
    resolved: &[(String, String)],
) -> Result<String, String> {
    let value_of = |name: &str| -> String {
        resolved
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let mut prefix = String::new();
    for ext in &dash.meta.loads {
        prefix.push_str(&format!("LOAD {ext}; ")); // identifier-validated at parse time
    }
    prefix.push_str("SET enable_external_access = false; SET lock_configuration = true; ");
    for (name, val) in resolved {
        prefix.push_str(&format!(
            "SET VARIABLE {name} = {}; ",
            sql_string_literal(val)
        ));
    }
    let mut selects = String::new();
    for p in &dash.meta.params {
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
    let controls = if dash.meta.params.is_empty() {
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
        let q = format!(
            "{prefix}SELECT * FROM (\n{}\n) LIMIT {}",
            panel.sql.trim().trim_end_matches(';'),
            st.max_rows + 1
        );
        let json = run_duckdb(&st.db, &["-readonly", "-json"], &q, st.timeout)?;
        let rows: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(json.trim()).unwrap_or_default();
        if rows.len() > st.max_rows {
            return Err(format!(
                "a panel of '{}' returned more than {} rows",
                dash.id, st.max_rows
            ));
        }
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
        let svg = anofox_visualization::host::catch_panic(|| render(&cols, 460, 300))
            .unwrap_or_else(|e| format!("<pre>error: {}</pre>", esc(&e)));
        panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
    }

    let meta = if dash.meta.refresh > 0 {
        format!(
            "<meta http-equiv=\"refresh\" content=\"{}\">",
            dash.meta.refresh
        )
    } else {
        String::new()
    };
    Ok(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{meta}<title>{title}</title><style>{STYLE}</style></head>\
<body><h1>{title}</h1>{controls}<div class=\"grid\">{panels}</div>\
<div id=\"dp-tip\" class=\"dp-tip\"></div><script>{SCRIPT}</script></body></html>",
        title = esc(&dash.meta.title)
    ))
}

fn list_page(dashboards: &[Dashboard]) -> String {
    let items: String = dashboards
        .iter()
        .map(|d| {
            format!(
                "<li><a href=\"/d/{}\">{}</a></li>",
                esc(&d.id),
                esc(&d.meta.title)
            )
        })
        .collect();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Dashboards</title><style>{STYLE}</style></head>\
<body><h1>Dashboards</h1><ul class=\"dash-list\">{items}</ul></body></html>"
    )
}

/// Run the `duckdb` CLI with flags + a SQL string; stdout on success, stderr on
/// error. The SQL is passed as one argv element after `-c` (no shell), and
/// callers refuse dot-command lines first. `timeout` > 0 kills a slow run.
fn run_duckdb(db: &str, flags: &[&str], sql: &str, timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("duckdb");
    cmd.arg(db).args(flags).arg("-c").arg(sql);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("duckdb: {e}"))?;
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| format!("duckdb: {e}"))? {
            Some(s) => break s,
            None if !timeout.is_zero() && started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("query timed out after {}s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let (stdout, stderr) = (
        t_out.join().unwrap_or_default(),
        t_err.join().unwrap_or_default(),
    );
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&stderr).to_string())
    }
}

const STYLE: &str = r#"body{font:15px/1.55 system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;background:#f4f6f9;color:#1f2937;margin:0;padding:2rem}
h1{font-size:1.35rem;font-weight:650;margin:0 0 1rem}
.controls{display:flex;gap:1.2rem;flex-wrap:wrap;align-items:center;background:#fff;border:1px solid #e5e7eb;border-radius:12px;padding:.7rem 1rem;margin-bottom:1.2rem;font-size:.85rem;font-weight:600;color:#6b7280}
.controls select{font:inherit;font-weight:600;color:#1f2937;border:1px solid #e5e7eb;border-radius:8px;padding:.35rem .6rem;margin-left:.3rem}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(440px,1fr));gap:1.1rem;align-content:start}
.section{grid-column:1/-1;margin:.6rem 0 -.4rem;font-size:1.1rem;font-weight:650}
.panel{margin:0;background:#fff;border:1px solid #e5e7eb;border-radius:14px;padding:1rem 1.1rem;box-shadow:0 1px 2px rgba(16,24,40,.05)}
.panel svg{width:100%;height:auto;display:block}
.dash-list a{color:#1f8ca6;font-weight:600;text-decoration:none}.dash-list a:hover{text-decoration:underline}
.dp-tip{position:fixed;pointer-events:none;background:#111827;color:#fff;padding:.35rem .55rem;border-radius:7px;font-size:.8rem;font-weight:500;box-shadow:0 4px 12px rgba(0,0,0,.25);opacity:0;transform:translateY(2px);transition:opacity .09s;z-index:20;white-space:nowrap}
.dp-tip.show{opacity:1;transform:translateY(0)}
.dp-hit{transition:filter .1s}.dp-hit:hover{filter:brightness(1.09) saturate(1.05)}"#;

// Hover tooltips + click-to-focus a series, wired from the `<title>` each SVG mark
// carries. Static (no wasm) — enough interactivity for a served dashboard.
const SCRIPT: &str = r#"(function(){
  var tip=document.getElementById('dp-tip'), selected=null;
  var marks=[].slice.call(document.querySelectorAll('.panel svg rect,.panel svg circle,.panel svg polygon,.panel svg polyline'))
    .filter(function(el){var t=el.querySelector('title');return t&&t.textContent.trim();});
  function apply(){marks.forEach(function(el){var s=el.getAttribute('data-series');el.style.opacity=(!selected||s===selected)?'':'0.15';});}
  marks.forEach(function(el){
    var t=el.querySelector('title'), txt=t.textContent;
    var series=txt.indexOf(': ')>=0?txt.slice(0,txt.lastIndexOf(': ')):txt;
    el.removeChild(t); el.setAttribute('data-series',series); el.classList.add('dp-hit'); el.style.cursor='pointer';
    el.addEventListener('mouseenter',function(){tip.textContent=txt;tip.classList.add('show');});
    el.addEventListener('mousemove',function(e){tip.style.left=(e.clientX+14)+'px';tip.style.top=(e.clientY+14)+'px';});
    el.addEventListener('mouseleave',function(){tip.classList.remove('show');});
    el.addEventListener('click',function(e){e.stopPropagation();selected=(selected===series)?null:series;apply();});
  });
  document.addEventListener('click',function(){if(selected!==null){selected=null;apply();}});
})();"#;

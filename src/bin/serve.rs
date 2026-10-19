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

use anofox_visualization::format::Locale;
use anofox_visualization::host::access::{self, Claims, LogEntry, RequestLog};
use anofox_visualization::host::app_brand::{self, AppBrand};
use anofox_visualization::host::auth::{self, apikey, audit::AuditLog, Auth, Identity, Scope};
use anofox_visualization::host::serving::{
    self as sv, html_escape as esc, sql_string_literal, AuthoringGuard, RequestMeta,
};
use anofox_visualization::theme::DashboardChrome;
use anofox_visualization::{page, render_with_title, sql, track, RenderOptions, Role};
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

/// Max body of an embedded page's form post.
const MAX_FORM: u64 = 16 << 10;

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
    /// `--theme`: default theme (name, `a+b`, or a `.json` file) when a
    /// dashboard sets no `::THEME`.
    theme: Option<String>,
    /// `--allow-host` (repeatable): extra `Host` names accepted besides the
    /// loopback names; `*` disables the check.
    allow_hosts: Vec<String>,
    /// `--auth <json | @file>`: viewer login (auth proxy, JWT/OIDC, embed
    /// links, API keys).
    auth: Option<Auth>,
    /// `--log <path | stderr>` (+ `--log-vars`): JSON-lines request log.
    log: Option<RequestLog>,
    /// `--app-name/-tagline/-logo/-link/-favicon/-hide-name`: the builder's
    /// app bar; with `--dashboards`, the landing page's header.
    app: AppBrand,
    /// `--home <id>` (with `--dashboards`): `/` redirects to that dashboard.
    home: Option<String>,
}

/// The `--theme` default (set once in `main`).
static THEME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
fn theme_default() -> Option<&'static str> {
    THEME.get().and_then(|t| t.as_deref())
}

/// The `--app-*` settings as the builder's `<script>` ("" = none; set once in `main`).
static APP: std::sync::OnceLock<String> = std::sync::OnceLock::new();

const USAGE: &str = "usage: serve [--db] <db> [--port N] [--no-open] [--theme T]    (authoring, loopback only)\n             [--app-name TEXT] [--app-tagline TEXT] [--app-logo FILE|URL]\n             [--app-logo-dark FILE|URL] [--app-link URL] [--app-favicon FILE|URL] [--app-hide-name]   (the builder's app bar)\n       serve --dashboards <dir> [--db <db>] [--init setup.sql] [--bind ADDR] [--port N]\n             [--cache SECS] [--max-rows N] [--timeout SECS] [--threads N] [--theme T]\n             [--allow-host NAME]...   (Host names served besides loopback; * = any)\n             [--auth JSON|@file.json] (login: auth proxy, JWT/OIDC, embed links, API keys —\n                                       see docs/secure-serving.md)\n             [--audit PATH|stderr]    (JSON-lines audit log of logins and refusals)\n             [--log PATH|stderr] [--log-vars none|names|values]   (JSON-lines request log)\n             [--home ID]              (/ redirects to that dashboard instead of the landing page)\n             [--app-name …] [--app-tagline …] [--app-logo …] [--app-logo-dark …] [--app-favicon …]   (the landing page's header)\n       (T: light | dark | print | colorblind | high-contrast | 'acme+dark' | theme.json;\n        a dashboard's own ::THEME wins)\n       serve api-key new --file KEYS.json --user U [--groups a,b] [--claim k=v]...\n             [--dashboards a,b] [--expires-days N] [--note TEXT] [--audit PATH]\n       serve api-key revoke --file KEYS.json --id ID [--audit PATH]\n       serve api-key list --file KEYS.json\n       serve --version\n\nQueries run through the `duckdb` CLI on PATH.";

fn usage(msg: &str) -> ! {
    eprintln!("error: {msg}\n\n{USAGE}");
    std::process::exit(2)
}

/// An `AppBrand::set` error, named by the flag that caused it (#267): the
/// brand's keys use `_` (`logo_dark`), the flags `-` (`--app-logo-dark`).
fn app_flag_error(e: &str) -> String {
    let n = e
        .find(|c: char| !(c.is_ascii_lowercase() || c == '_'))
        .unwrap_or(e.len());
    format!("--app-{}{}", e[..n].replace('_', "-"), &e[n..])
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
        theme: None,
        allow_hosts: Vec::new(),
        auth: None,
        log: None,
        app: AppBrand::default(),
        home: None,
    };
    let (mut log, mut log_vars, mut audit) = (None, None, None);
    let mut args = std::env::args().skip(1);
    let num = |v: Option<String>, what: &str| -> u64 {
        v.and_then(|v| v.parse().ok())
            .unwrap_or_else(|| usage(&format!("{what} needs a number")))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0)
            }
            "--version" | "-V" => {
                println!("serve {}", anofox_visualization::BUILD_VERSION);
                std::process::exit(0)
            }
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
            "--allow-host" => o.allow_hosts.push(
                args.next()
                    .unwrap_or_else(|| usage("--allow-host needs a host name")),
            ),
            "--max-rows" => o.max_rows = num(args.next(), "--max-rows").max(1) as usize,
            "--timeout" => o.timeout = Duration::from_secs(num(args.next(), "--timeout").max(1)),
            "--threads" => o.threads = num(args.next(), "--threads").clamp(1, 64) as usize,
            "--db" => o.db = args.next().unwrap_or_else(|| usage("--db needs a path")),
            "--theme" => {
                let v = args
                    .next()
                    .unwrap_or_else(|| usage("--theme needs a theme"));
                let spec = if v.ends_with(".json") {
                    std::fs::read_to_string(&v)
                        .unwrap_or_else(|e| usage(&format!("--theme {v}: {e}")))
                } else {
                    v
                };
                if let Err(e) = anofox_visualization::theme::resolve(&spec) {
                    usage(&format!("--theme: {e}"));
                }
                o.theme = Some(spec);
            }
            "--auth" => {
                let v = args
                    .next()
                    .unwrap_or_else(|| usage("--auth needs a JSON object or @file"));
                let json = match v.strip_prefix('@') {
                    Some(f) => std::fs::read_to_string(f)
                        .unwrap_or_else(|e| usage(&format!("--auth {f}: {e}"))),
                    None => v,
                };
                let val: serde_json::Value = serde_json::from_str(&json)
                    .unwrap_or_else(|e| usage(&format!("--auth: bad JSON: {e}")));
                o.auth = Some(
                    Auth::from_json(&val, |k| std::env::var(k).ok())
                        .unwrap_or_else(|e| usage(&format!("--{e}"))),
                );
            }
            "--app-name" | "--app-tagline" | "--app-logo" | "--app-logo-dark" | "--app-link"
            | "--app-favicon" => {
                let v = args
                    .next()
                    .unwrap_or_else(|| usage(&format!("{a} needs a value")));
                o.app
                    .set(&a["--app-".len()..].replace('-', "_"), &v, true)
                    .unwrap_or_else(|e| usage(&app_flag_error(&e)));
            }
            "--app-hide-name" => o.app.name_visible = Some(false),
            "--home" => {
                let h = args
                    .next()
                    .unwrap_or_else(|| usage("--home needs a dashboard id"));
                if !sv::valid_dashboard_id(&h) {
                    usage("--home: a dashboard id is its file name without .sql");
                }
                o.home = Some(h);
            }
            "--log" => {
                log = Some(
                    args.next()
                        .unwrap_or_else(|| usage("--log needs a path or stderr")),
                )
            }
            "--audit" => {
                audit = Some(
                    args.next()
                        .unwrap_or_else(|| usage("--audit needs a path or stderr")),
                )
            }
            "--log-vars" => {
                log_vars = Some(
                    args.next()
                        .unwrap_or_else(|| usage("--log-vars needs none|names|values")),
                )
            }
            s if s.starts_with('-') => usage(&format!("unknown option {s}")),
            _ => o.db = a,
        }
    }
    match (log, log_vars) {
        (Some(l), v) => {
            o.log =
                Some(RequestLog::new(&l, v.as_deref()).unwrap_or_else(|e| usage(&format!("--{e}"))))
        }
        (None, Some(_)) => usage("--log-vars needs --log"),
        (None, None) => {}
    }
    if o.dashboards_dir.is_none() && (o.auth.is_some() || o.log.is_some()) {
        usage("--auth and --log apply to --dashboards serving");
    }
    if let Err(e) = o.app.check() {
        usage(if e.starts_with("logo_dark") {
            "--app-logo-dark needs --app-logo"
        } else {
            "--app-hide-name needs --app-logo (the bar would be empty)"
        });
    }
    if o.dashboards_dir.is_none() && o.home.is_some() {
        usage("--home applies to --dashboards serving");
    }
    match (&mut o.auth, audit) {
        (Some(a), Some(p)) => a.set_audit(Arc::new(
            AuditLog::new(&p).unwrap_or_else(|e| usage(&format!("--{e}"))),
        )),
        (None, Some(_)) => usage("--audit needs --auth"),
        _ => {}
    }
    o
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("api-key") {
        api_key_cli(std::env::args().skip(2).collect());
    }
    let mut o = parse_args();
    let _ = THEME.set(o.theme.clone());
    let _ = APP.set(o.app.script());
    match o.dashboards_dir.clone() {
        Some(dir) => serve_dashboards(&dir, &mut o),
        None => authoring_mode(&o),
    }
}

/// `serve api-key new|revoke|list --file KEYS.json …`: manage the API keys of
/// `{"mode": "api_keys"}`. A new key is printed once; the file keeps its hash.
fn api_key_cli(args: Vec<String>) -> ! {
    let fail = |m: &str| -> ! {
        eprintln!("error: {m}");
        std::process::exit(2)
    };
    let mut it = args.into_iter();
    let cmd = it.next().unwrap_or_default();
    let (mut file, mut user, mut id, mut note, mut audit) = (None, None, None, None, None);
    let (mut groups, mut claims, mut dashboards, mut days) = (Vec::new(), Vec::new(), None, None);
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .unwrap_or_else(|| fail(&format!("{a} needs a value")))
        };
        match a.as_str() {
            "--file" => file = Some(PathBuf::from(val())),
            "--user" => user = Some(val()),
            "--id" => id = Some(val()),
            "--note" => note = Some(val()),
            "--audit" => audit = Some(val()),
            "--groups" => {
                groups = val()
                    .split(',')
                    .map(|g| g.trim().to_string())
                    .filter(|g| !g.is_empty())
                    .collect()
            }
            "--claim" => {
                let v = val();
                let (k, x) = v
                    .split_once('=')
                    .unwrap_or_else(|| fail("--claim needs name=value"));
                claims.push((k.to_string(), serde_json::json!(x)));
            }
            "--dashboards" => {
                dashboards = Some(
                    val()
                        .split(',')
                        .map(|d| d.trim().to_string())
                        .collect::<Vec<_>>(),
                )
            }
            "--expires-days" => {
                days = Some(
                    val()
                        .parse::<u64>()
                        .unwrap_or_else(|_| fail("--expires-days needs a number")),
                )
            }
            _ => fail(&format!("unknown option {a}")),
        }
    }
    let file = file.unwrap_or_else(|| fail("--file KEYS.json is required"));
    let audit = audit.map(|p| AuditLog::new(&p).unwrap_or_else(|e| fail(&e)));
    let log = |event: &str, outcome: &str, key_id: Option<&str>, user: Option<&str>| {
        if let Some(a) = &audit {
            a.write(&auth::audit::Event {
                event,
                outcome,
                key_id,
                user,
                detail: Some(&file.display().to_string()),
                ..Default::default()
            });
        }
    };
    let mut entries = apikey::file_entries(&file).unwrap_or_else(|e| fail(&e));
    match cmd.as_str() {
        "new" => {
            let user = user.unwrap_or_else(|| fail("--user is required"));
            let (key, kid, hash) = apikey::new_key();
            let mut e = serde_json::json!({"id": kid, "sha256": hash, "user": user});
            if !groups.is_empty() {
                e["groups"] = serde_json::json!(groups);
            }
            if !claims.is_empty() {
                e["claims"] = serde_json::Value::Object(claims.into_iter().collect());
            }
            if let Some(d) = dashboards {
                e["dashboards"] = serde_json::json!(d);
            }
            if let Some(d) = days {
                e["expires"] = serde_json::json!(auth::jwt::now_s() + d * 86400);
            }
            if let Some(n) = note {
                e["note"] = serde_json::json!(n);
            }
            e["created"] = serde_json::json!(access::rfc3339(std::time::SystemTime::now()));
            entries.push(e);
            // Validate the whole file before writing it.
            let text = serde_json::json!({"keys": entries}).to_string();
            apikey::parse_file(&text).unwrap_or_else(|e| fail(&e));
            apikey::write_file(&file, &entries).unwrap_or_else(|e| fail(&e));
            log("api_key_created", "ok", Some(&kid), Some(&user));
            eprintln!(
                "API key {kid} for '{user}' added to {} — shown once, store it now:",
                file.display()
            );
            println!("{key}");
        }
        "revoke" => {
            let id = id.unwrap_or_else(|| fail("--id is required"));
            let Some(e) = entries.iter_mut().find(|e| e["id"] == id.as_str()) else {
                log("api_key_revoked", "error", Some(&id), None);
                fail(&format!("no key with id {id}"))
            };
            e["revoked"] = serde_json::json!(true);
            let user = e["user"].as_str().map(str::to_string);
            apikey::write_file(&file, &entries).unwrap_or_else(|e| fail(&e));
            log("api_key_revoked", "ok", Some(&id), user.as_deref());
            println!(
                "revoked {id} (servers pick it up within {} s)",
                apikey::RELOAD_EVERY.as_secs()
            );
        }
        "list" => {
            for e in &entries {
                println!(
                    "{}  {}  {}{}",
                    e["id"].as_str().unwrap_or(""),
                    e["user"].as_str().unwrap_or(""),
                    e.get("dashboards")
                        .map_or("all dashboards".to_string(), |d| d.to_string()),
                    if e["revoked"] == true {
                        "  REVOKED"
                    } else {
                        ""
                    }
                );
            }
        }
        _ => fail("usage: serve api-key new|revoke|list --file KEYS.json …"),
    }
    std::process::exit(0)
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
    // same-origin: a page URL can carry an embed page session.
    let resp = Response::from_data(body.into())
        .with_status_code(status)
        .with_header(header("Content-Type", ctype))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Referrer-Policy", "same-origin"));
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
                    // bare Infinity / NaN (non-finite doubles) → null
                    sql::sanitize_json_numbers(body.trim()).into_owned()
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
    // app-config.json is the static-hosting file: a binary takes --app-* only.
    match WEB
        .get_file(asset)
        .filter(|_| asset != app_brand::STATIC_CONFIG)
    {
        // index.html carries the --theme default and the --app-* bar for the builder.
        Some(f)
            if asset == "index.html"
                && (theme_default().is_some() || APP.get().is_some_and(|a| !a.is_empty())) =>
        {
            let html = String::from_utf8_lossy(f.contents()).into_owned();
            let mut head = APP.get().cloned().unwrap_or_default();
            if let Some(t) = theme_default() {
                let spec = serde_json::to_string(t)
                    .unwrap_or_default()
                    .replace('<', "\\u003c");
                head.push_str(&format!("<script>window.__anofoxTheme={spec};</script>"));
            }
            let html = html.replacen("</head>", &format!("{head}</head>"), 1);
            respond(req, 200, sv::content_type(asset), html.into_bytes())
        }
        Some(f) => respond(req, 200, sv::content_type(asset), f.contents().to_vec()),
        None => text(req, 404, "not found"),
    }
}

// ---------- serve mode (server owns the SQL; consumers pick id + params) ----------

struct Dashboard {
    id: String,
    meta: sv::DashboardMeta,
    script: String,
    /// The day the file last changed (`YYYY-MM-DD`), for the landing page.
    updated: Option<String>,
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
                        updated: sv::file_date(&p),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

struct ServeState {
    port: u16,
    /// `--app-*`: the landing page's header; `--home`: `/` redirects there.
    app: AppBrand,
    home: Option<String>,
    allow_hosts: Vec<String>,
    db: String,
    dashboards: Vec<Dashboard>,
    cache: RenderCache,
    max_rows: usize,
    timeout: Duration,
    auth: Option<Auth>,
    log: Option<RequestLog>,
}

fn serve_dashboards(dir: &str, o: &mut Opts) {
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
    if let Some(h) = &o.home {
        if !dashboards.iter().any(|d| &d.id == h) {
            usage(&format!("--home: no dashboard '{h}' in {dir}"));
        }
    }
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
    match (&o.auth, sv::is_loopback_bind(&o.bind)) {
        (Some(a), _) => {
            println!("  login: {}", a.describe());
            println!(
                "  no row/column policies here (only anofox_serve_dashboards has them): every panel must filter on getvariable('__claim_*') itself"
            );
            for w in a.prefetch() {
                eprintln!("  WARNING: {w} — requests fail closed until it is reachable");
            }
        }
        (None, false) => eprintln!(
            "  WARNING: bound to {} WITHOUT --auth — every viewer who can reach this address sees every dashboard; \
             put a TLS/auth proxy in front and pass --auth (docs/secure-serving.md)",
            o.bind
        ),
        (None, true) => {}
    }
    let st = ServeState {
        port: o.port,
        app: o.app.clone(),
        home: o.home.clone(),
        // the --allow-host names, plus the bind address itself (direct access
        // by IP) unless it is a wildcard address
        allow_hosts: o
            .allow_hosts
            .iter()
            .cloned()
            .chain((!["0.0.0.0", "::", "[::]"].contains(&o.bind.as_str())).then(|| o.bind.clone()))
            .collect(),
        db,
        dashboards,
        cache: RenderCache::new(Duration::from_secs(o.cache_secs)),
        max_rows: o.max_rows,
        timeout: o.timeout,
        auth: o.auth.take(),
        log: o.log.take(),
    };
    run_workers(server, o.threads, move |req| handle_serve(&st, req));
}

fn handle_serve(st: &ServeState, req: Request) {
    let t0 = Instant::now();
    let method = req.method().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    let peer = req.remote_addr().map(|a| a.ip());
    let mut user = None;
    let mut deny = None;
    let mut how = None;
    let mut dashboard = None;
    let mut params = Vec::new();
    let status = route_serve(
        st,
        req,
        &mut user,
        &mut deny,
        &mut how,
        &mut dashboard,
        &mut params,
    );
    if let Some(log) = &st.log {
        log.write(&LogEntry {
            method: &method,
            path: &path,
            status,
            ms: t0.elapsed().as_millis(),
            user: user.as_deref(),
            peer,
            dashboard: dashboard.as_deref(),
            vars: &params,
            deny,
            auth: how,
            ..Default::default()
        });
    }
}

fn reply(req: Request, status: u16, msg: &str) -> u16 {
    text(req, status, msg);
    status
}

/// The `--cache` render cache: entries expire after the TTL and are purged
/// on insert; beyond [`RenderCache::MAX_ENTRIES`] entries or
/// [`RenderCache::MAX_BYTES`] the oldest go first. (With `--auth` every viewer
/// has their own entries, so it must not grow without bound.)
struct RenderCache {
    ttl: Duration,
    map: Mutex<HashMap<String, (Instant, String)>>,
}

impl RenderCache {
    const MAX_ENTRIES: usize = 1024;
    const MAX_BYTES: usize = 64 << 20;

    fn new(ttl: Duration) -> Self {
        RenderCache {
            ttl,
            map: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, key: &str, now: Instant) -> Option<String> {
        if self.ttl.is_zero() {
            return None;
        }
        let map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.get(key)
            .filter(|(ts, _)| now.duration_since(*ts) < self.ttl)
            .map(|(_, h)| h.clone())
    }

    fn insert(&self, key: String, html: String, now: Instant) {
        if self.ttl.is_zero() || html.len() > Self::MAX_BYTES / 4 {
            return;
        }
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, (ts, _)| now.duration_since(*ts) < self.ttl);
        map.remove(&key);
        let mut bytes: usize = map.values().map(|(_, h)| h.len()).sum();
        while !map.is_empty()
            && (map.len() >= Self::MAX_ENTRIES || bytes + html.len() > Self::MAX_BYTES)
        {
            let oldest = map
                .iter()
                .min_by_key(|(_, (ts, _))| *ts)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                if let Some((_, h)) = map.remove(&k) {
                    bytes -= h.len();
                }
            }
        }
        map.insert(key, (now, html));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

fn route_serve(
    st: &ServeState,
    mut req: Request,
    user: &mut Option<String>,
    deny: &mut Option<&'static str>,
    how: &mut Option<&'static str>,
    dashboard: &mut Option<String>,
    params: &mut Vec<(String, String)>,
) -> u16 {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/");
    // A bare control character in the target is never a legitimate URL.
    if access::has_control_chars(&url) {
        return reply(req, 400, "bad request: control character in the URL");
    }
    // DNS rebinding: only loopback names (or --allow-host names) are served.
    if !sv::host_allowed(req_header(&req, "Host"), st.port, &st.allow_hosts) {
        return reply(
            req,
            421,
            "misdirected request: unknown Host — start the server with --allow-host <name>",
        );
    }
    // CSRF: the only non-GET request (the OIDC logout) must be same-origin.
    if req.method() != &Method::Get
        && !sv::origin_matches_host(req_header(&req, "Origin"), req_header(&req, "Host"))
    {
        return reply(req, 403, "forbidden: cross-origin request");
    }
    // An embedded page posts its form: the page session and the params come
    // in the body (never in a URL, a history entry or a proxy log line).
    let form = match &st.auth {
        Some(a)
            if a.embed.is_some() && req.method() == &Method::Post && path.starts_with("/d/") =>
        {
            let urlencoded = req_header(&req, "Content-Type").is_some_and(|c| {
                c.trim()
                    .to_ascii_lowercase()
                    .starts_with("application/x-www-form-urlencoded")
            });
            let mut body = String::new();
            let read = (&mut req as &mut Request)
                .as_reader()
                .take(MAX_FORM + 1)
                .read_to_string(&mut body);
            if !urlencoded || read.is_err() || body.len() as u64 > MAX_FORM {
                return reply(req, 400, "bad request: expected a small form");
            }
            Some(sv::parse_query(&format!("?{body}")))
        }
        _ => None,
    };
    // Login (--auth): a credential on every request.
    let ident: Option<Identity> = match &st.auth {
        Some(auth) => {
            let headers: Vec<(&str, &str)> = req
                .headers()
                .iter()
                .map(|h| (h.field.as_str().as_str(), h.value.as_str()))
                .collect();
            let method = req.method().to_string();
            let r = auth::Req {
                method: &method,
                url: &url,
                peer: req.remote_addr().map(|a| a.ip()),
                headers: &headers,
            };
            // The OIDC login's own endpoints (/auth/*).
            if let Some(rep) = auth.route(&r) {
                return send(req, rep);
            }
            let res = match &form {
                Some(f) => auth.page_session(f.get("embed_session").map(String::as_str), &r),
                None => auth.authenticate(&r),
            };
            match res {
                Ok(id) => {
                    *user = Some(id.claims.user.clone());
                    *how = Some(id.method);
                    Some(id)
                }
                Err(d) => {
                    *deny = Some(d.reason);
                    *how = d.method;
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
    let claims = ident.as_ref().map(|i| &i.claims);
    let scope = ident.as_ref().map(|i| &i.scope);
    if req.method() != &Method::Get && form.is_none() {
        return reply(req, 405, "method not allowed");
    }
    if path == "/" {
        // --home: straight to that dashboard (if this viewer may read it)
        if let Some(h) = st
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
        respond(
            req,
            200,
            "text/html; charset=utf-8",
            list_page(st, scope).into_bytes(),
        );
        return 200;
    }
    let Some(rest) = path.strip_prefix("/d/") else {
        // No /query, no static assets, no SQL — the whole surface is id + params.
        return reply(req, 404, "not found");
    };
    let id = rest.trim_end_matches('/');
    let Some(dash) = st.dashboards.iter().find(|d| d.id == id) else {
        return reply(req, 404, "no such dashboard");
    };
    *dashboard = Some(dash.id.clone());
    // A scoped credential (embed link, API key) reads its own dashboards only.
    if !scope.is_none_or(|s| s.allows(&dash.id)) {
        return reply(req, 403, "forbidden: not in this credential's scope");
    }
    // Validate params against the whitelist (unknown/disallowed → 400).
    let chosen = form.clone().unwrap_or_else(|| sv::parse_query(&url));
    let mut resolved = match resolve_params(dash, &chosen) {
        Ok(r) => r,
        Err(e) => return reply(req, 400, &e),
    };
    // An embed link's locked variables replace the viewer's choice.
    if let Some(s) = scope {
        resolved.retain(|(n, _)| !s.vars.iter().any(|(l, _)| l.eq_ignore_ascii_case(n)));
    }
    params.clone_from(&resolved);
    // This server renders plain pages (no script): an embed link is
    // exchanged once, and the page session rides along in the page's form
    // (`embed_session`), so a `one_time` link is not needed again.
    let embed_link = match ident.as_ref() {
        Some(i) if i.page_session.is_some() => i.page_session.clone(),
        Some(i) if i.method == "embed" => {
            form.as_ref().and_then(|f| f.get("embed_session").cloned())
        }
        _ => None,
    };
    let key = cache_key(&dash.id, &resolved, ident.as_ref(), embed_link.as_deref());
    if let Some(html) = st.cache.get(&key, Instant::now()) {
        respond(req, 200, "text/html; charset=utf-8", html.into_bytes());
        return 200; // no DB touched
    }
    match render_dashboard_page(st, dash, &resolved, claims, scope, embed_link.as_deref()) {
        Ok(html) => {
            st.cache.insert(key, html.clone(), Instant::now());
            respond(req, 200, "text/html; charset=utf-8", html.into_bytes());
            200
        }
        Err(PageError::Viewer(e)) => reply(req, 400, &e),
        Err(PageError::Server(e)) => {
            eprintln!(
                "[anofox-serve] dashboard '{}' failed: {}",
                dash.id,
                access::diag(&e)
            );
            reply(req, 400, PAGE_ERROR)
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

/// The render cache key: dashboard, params and — with a login — the viewer's
/// claims and locked variables, so one viewer's page is never served to
/// another. (An embed link's page carries the link: keyed by it too.)
fn cache_key(
    id: &str,
    resolved: &[(String, String)],
    ident: Option<&Identity>,
    embed_link: Option<&str>,
) -> String {
    format!(
        "{:?}",
        (
            id,
            resolved,
            ident.map(|i| (i.claims.key(), &i.scope.vars)),
            embed_link
        )
    )
}

/// The first row of `duckdb -json` output as `(column, text)` pairs (NULL → None).
fn first_row_cells(json: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let rows = sql::parse_rows_json(json)?;
    Ok(rows
        .into_iter()
        .next()
        .map(|row| {
            row.into_iter()
                .map(|(k, v)| {
                    let t = match v {
                        serde_json::Value::Null => None,
                        serde_json::Value::String(s) => Some(s),
                        v => Some(v.to_string()),
                    };
                    (k, t)
                })
                .collect()
        })
        .unwrap_or_default())
}

/// `requires_unmet`'s answers, memoized for one render (#274 L1): keyed by
/// `(prefix, "{reqs:?}")`, since the same gate is often asked more than once
/// — a `::TAB`/`::SUBTAB`/`::VIEW` marker's own check (`Sections::step`) and
/// then, for a panel directly inside it, its own identical `-- @requires`.
type RequiresCache = HashMap<(String, String), Option<String>>;

/// `-- @requires` on the script-free page (#274), gated as the browser and
/// the locked server gate it: the shared check (`sql::requires::check`, as
/// `dashboard --check` and the static renderers use it), each query in the
/// same locked `duckdb -readonly` session the panels run in, with this
/// request's variables (`prefix`). That session can open no file
/// (`enable_external_access = false`), so a `glob` that fails counts as
/// missing here — fail closed, unlike `check` on an author's own session.
/// The glob probe itself never spawns `duckdb` (#274 L1): in this locked
/// session the only path a `glob` can ever see is the served database's own
/// file or its `.wal` (#274 I2), so any other path is decided as missing
/// from the query text alone, the same `sql::requires::glob_sql` the shared
/// `check` would have sent. `check`'s own semantics are unchanged; only this
/// closure short-circuits. `Some(requirement)` (as written) when one is not
/// met.
fn requires_unmet(
    st: &ServeState,
    prefix: &str,
    reqs: &[sql::requires::Requirement],
    cache: &mut RequiresCache,
) -> Option<String> {
    let key = (prefix.to_string(), format!("{reqs:?}"));
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    let missing = sql::requires::check(reqs, |q, session| {
        if !session {
            let n = if q == sql::requires::glob_sql(&st.db)
                || q == sql::requires::glob_sql(&format!("{}.wal", st.db))
            {
                "1"
            } else {
                "0"
            };
            return Ok(vec![("n".to_string(), Some(n.to_string()))]);
        }
        run_duckdb(
            &st.db,
            &["-readonly", "-json"],
            &format!("{prefix}{q}"),
            st.timeout,
        )
        .and_then(|json| first_row_cells(json.trim()))
    })
    .missing;
    cache.insert(key, missing.clone());
    missing
}

/// A panel whose own `-- @requires` input is missing (#274): the browser's
/// "Not available" card (web/app.js `mkNotAvailable`), without the file's
/// name — this page carries none of the dashboard's SQL.
const NOT_AVAILABLE_HTML: &str = "<figure class=\"panel dp-na\"><div class=\"nodata\" role=\"status\"><strong>Not available</strong><span>An optional input of this panel is missing.</span></div></figure>";

/// Render one dashboard to a view-only HTML page: binds the (whitelisted)
/// values as DuckDB variables and runs each panel's fixed query READ-ONLY, with
/// external access disabled and the configuration locked after the
/// server-declared `-- @load`s.
type Rows = Vec<serde_json::Map<String, serde_json::Value>>;

/// Why the script-free page could not be drawn (#279). `Viewer` text is safe
/// to show (no data, SQL or paths: a timeout, the row cap); `Server` text — a
/// DuckDB error, which can quote file paths, SQL and values — goes to the
/// server log only, and the viewer gets [`PAGE_ERROR`] (as the locked
/// server does, duckext `POLICY_PANEL_ERROR`).
enum PageError {
    Viewer(String),
    Server(String),
}

const PAGE_ERROR: &str = "this dashboard could not be shown (the server log has the details)";

/// A `run_duckdb` error as a [`PageError`]: only its own timeout is shown.
fn page_error(e: String) -> PageError {
    if e.starts_with("query timed out after ") {
        PageError::Viewer(e)
    } else {
        PageError::Server(e)
    }
}

/// One read-only panel query → rows.
fn page_rows(st: &ServeState, q: &str) -> Result<Rows, PageError> {
    let json = run_duckdb(&st.db, &["-readonly", "-json"], q, st.timeout).map_err(page_error)?;
    sql::parse_rows_json(json.trim()).map_err(PageError::Server)
}

fn render_dashboard_page(
    st: &ServeState,
    dash: &Dashboard,
    resolved: &[(String, String)],
    claims: Option<&Claims>,
    scope: Option<&Scope>,
    embed_link: Option<&str>,
) -> Result<String, PageError> {
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
    // An embed link's locked variables (typed literals), after the params.
    for (name, lit) in scope.map(|s| s.vars.clone()).unwrap_or_default() {
        prefix.push_str(&format!("SET VARIABLE {name} = {lit}; "));
    }
    // The login's claims (a @param can't name them: parse_dashboard_meta
    // ignores `__claim_*` params).
    for (name, lit) in claims.map(Claims::variables).unwrap_or_default() {
        prefix.push_str(&format!("SET VARIABLE {name} = {lit}; "));
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
            "<form class=\"controls\" method=\"{}\" action=\"/d/{}\">{selects}{}</form>",
            if embed_link.is_some() { "post" } else { "get" },
            esc(&dash.id),
            embed_link
                .map(|l| format!(
                    "<input type=\"hidden\" name=\"embed_session\" value=\"{}\">",
                    esc(l)
                ))
                .unwrap_or_default()
        )
    };

    // Phase 1: query every panel (read-only) and collect the chrome
    // directives, so a ::THEME anywhere styles the whole page.
    let mut chrome = DashboardChrome::default();
    let mut queried: Vec<(sql::Panel, Rows, bool)> = Vec::new();
    // The script-free page shows only a ::VIEW group's first alternative
    // (#214); the others are never queried, and a `?view.*` request param
    // is not a thing — `resolve_params` above only reads declared `@param`s.
    let mut view_pick = sql::views::StaticPick::default();
    // #274: a ::TAB / ::SUBTAB / ::VIEW alternative whose `-- @requires`
    // input is missing is left out with its panels (the tabs are flattened
    // here; the next shown alternative is the one drawn) — as the static
    // renderers do (src/dashboard.rs `execute`).
    let mut sections = sql::requires::Sections::default();
    // #274 L1: memoized per render — the same gate is often asked more than
    // once (a tab/view marker's own check, then a panel inside it).
    let mut req_cache = RequiresCache::new();
    for panel in sql::plan(&dash.script) {
        let hidden = sections.step(&panel, |reqs| {
            requires_unmet(st, &prefix, reqs, &mut req_cache)
        });
        if panel.setup {
            continue; // read-only mode: setup is done once at --init, not per request
        }
        if hidden.is_some() || view_pick.drop_panel(&panel) {
            continue;
        }
        let chrome_role = panel
            .roles
            .iter()
            .map(|(_, r)| *r)
            .find(|r| anofox_visualization::roles::is_chrome_directive(*r));
        // #274 L2: the panel's own `-- @requires` gates it whatever it is —
        // an ordinary chart, an `::INPUT` or a chrome directive — so a
        // missing input never reaches that panel's query and turns into a
        // 400 naming the file; it is simply left out (below).
        let gated = !panel.requires.is_empty()
            && requires_unmet(st, &prefix, &panel.requires, &mut req_cache).is_some();
        // An input binds its default (as the browser does on first load) so
        // the getvariable() panels read it — unless a whitelisted `-- @param`
        // already sets that variable. Gated: left out, so the variable keeps
        // its built-in default, as the static renderer leaves it too.
        if let Some(kind) = anofox_visualization::inputs::input_kind(&panel.roles) {
            if gated {
                continue;
            }
            let q = format!(
                "{prefix}SELECT * FROM (\n{}\n) LIMIT {}",
                panel.sql.trim().trim_end_matches(';'),
                st.max_rows
            );
            let rows: Rows = page_rows(st, &q)?;
            let idx = anofox_visualization::inputs::input_index(&panel.roles);
            for set in anofox_visualization::inputs::input_defaults(kind, idx, &rows) {
                let name = set.split('"').nth(1).unwrap_or("");
                if !resolved.iter().any(|(n, _)| n == name) && !access::is_reserved_var(name) {
                    prefix.push_str(&set);
                    prefix.push_str("; ");
                }
            }
            continue;
        }
        let section = panel.roles.iter().any(|(_, r)| *r == Role::Section);
        // Skip interactive/layout-only directives (params drive re-render instead).
        if chrome_role.is_none()
            && !section
            && anofox_visualization::roles::is_directive_panel(&panel.roles)
        {
            continue;
        }
        // Gated: a chrome directive is left out — its query never runs.
        if chrome_role.is_some() && gated {
            continue;
        }
        // #274: the panel's own `-- @requires` (never a chrome directive's or a
        // ::SECTION's): not met → a "Not available" card; a ::TRACK strip is
        // left out of its chart instead.
        if chrome_role.is_none() && !section && gated {
            if !track::is_track(&panel.roles) {
                queried.push((panel, Rows::new(), true));
            }
            continue;
        }
        let q = format!(
            "{prefix}SELECT * FROM (\n{}\n) LIMIT {}",
            panel.sql.trim().trim_end_matches(';'),
            st.max_rows + 1
        );
        let rows: Rows = page_rows(st, &q)?;
        if rows.len() > st.max_rows {
            return Err(PageError::Viewer(format!(
                "a panel of '{}' returned more than {} rows",
                dash.id, st.max_rows
            )));
        }
        if let Some(r) = chrome_role {
            let cell = |v: &serde_json::Value| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                v => Some(v.to_string()),
            };
            // The first row's cells (an ::ASOF also reads its 'stale=…' cell).
            let cells = rows
                .first()
                .map(|row| row.values().map(cell).collect())
                .unwrap_or_default();
            chrome.apply_row(r, cells);
            continue;
        }
        queried.push((panel, rows, false));
    }
    let set = chrome.resolve(theme_default()).map_err(PageError::Server)?;
    let t = set.default_variant().clone();
    let opts = RenderOptions {
        locale: t
            .locale
            .as_deref()
            .and_then(Locale::from_tag)
            .unwrap_or_default(),
        theme: Arc::new(t.clone()),
        ..RenderOptions::default()
    };

    // Phase 2: render.
    let mut panels = String::new();
    let mut queried = queried.into_iter().peekable();
    while let Some((panel, rows, na)) = queried.next() {
        if na {
            panels.push_str(NOT_AVAILABLE_HTML);
            // its strips go with it
            while queried
                .next_if(|(q, _, _)| track::is_track(&q.roles))
                .is_some()
            {}
            continue;
        }
        if let Some(cp) = &panel.contract {
            for (_, r) in sql::contract_call::render_rows_opts(cp, &rows, 460, 300, &opts) {
                let svg =
                    r.unwrap_or_else(|e| format!("<pre>error: {}</pre>", esc(&e.to_string())));
                panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
            }
            continue;
        }
        let section = panel.roles.iter().any(|(_, r)| *r == Role::Section);
        if section || (panel.roles.len() == 1 && matches!(panel.roles[0].1, Role::Label)) {
            let text = rows
                .first()
                .and_then(|r| r.values().next())
                .and_then(|v| v.as_str())
                .unwrap_or("");
            panels.push_str(&format!("<h2 class=\"section\">{}</h2>", esc(text)));
            continue;
        }
        let cols = anofox_visualization::name_columns(
            sql::columns_from_rows(&rows, &panel.roles),
            &panel.roles,
            &panel.names,
        );
        // ::TRACKs right after a chart sit under it, in its figure.
        let mut tracks = Vec::new();
        while let Some((q, r, _)) = queried.next_if(|(q, _, _)| track::is_track(&q.roles)) {
            tracks.push((sql::columns_from_rows(&r, &q.roles), 0));
        }
        // Its ::TITLE bar, as on a dashboard card (#232).
        let svg = anofox_visualization::host::catch_panic(|| {
            render_with_title(&cols, &tracks, 460, 300, &opts).map_err(|e| e.to_string())
        })
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
    Ok(page::render(&page::StaticPage {
        theme: &t,
        fallback_title: &dash.meta.title,
        subtitle_html: String::new(),
        head_extra: meta,
        controls_html: controls,
        panels_html: panels,
    }))
}

/// The `/` landing page: the dashboards the viewer may read, as cards in
/// groups, in the `--theme`, with the `--app-*` header (`page::landing`).
fn list_page(st: &ServeState, scope: Option<&Scope>) -> String {
    let set = anofox_visualization::theme::resolve(theme_default().unwrap_or(""))
        .unwrap_or_else(|_| anofox_visualization::theme::resolve("").expect("the default theme"));
    page::landing(&page::Landing {
        theme: &set,
        app: (!st.app.is_empty()).then_some(&st.app),
        items: st
            .dashboards
            .iter()
            .filter(|d| scope.is_none_or(|s| s.allows(&d.id)))
            .map(|d| page::LandingItem {
                href: format!("/d/{}", d.id),
                meta: &d.meta,
                updated: d.updated.clone(),
            })
            .collect(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_errors_show_only_safe_text() {
        assert!(matches!(
            page_error("query timed out after 30s".into()),
            PageError::Viewer(_)
        ));
        assert!(matches!(
            page_error(
                "Catalog Error: Table with name secret does not exist!\nLINE 1: SELECT …".into()
            ),
            PageError::Server(_)
        ));
    }

    #[test]
    fn app_flag_errors_name_the_flag() {
        let mut b = AppBrand::default();
        let e = b.set("logo_dark", "ftp://x/l.png", true).unwrap_err();
        assert!(
            app_flag_error(&e).starts_with("--app-logo-dark must be an http(s) URL"),
            "{}",
            app_flag_error(&e)
        );
        let e = b
            .set("logo_dark", "/nonexistent/anofox-267.png", true)
            .unwrap_err();
        assert!(
            app_flag_error(&e).starts_with("--app-logo-dark: '/nonexistent/anofox-267.png'"),
            "{}",
            app_flag_error(&e)
        );
        let e = b.set("logo", "ftp://x/l.png", true).unwrap_err();
        assert!(
            app_flag_error(&e).starts_with("--app-logo must be"),
            "{}",
            app_flag_error(&e)
        );
        let e = b.set("tagline", &"x".repeat(1000), true).unwrap_err();
        assert!(
            app_flag_error(&e).starts_with("--app-tagline must be"),
            "{}",
            app_flag_error(&e)
        );
    }

    #[test]
    fn render_cache_expires_and_is_capped() {
        let c = RenderCache::new(Duration::from_secs(60));
        let t0 = Instant::now();
        c.insert("a".into(), "A".into(), t0);
        assert_eq!(
            c.get("a", t0 + Duration::from_secs(59)).as_deref(),
            Some("A")
        );
        assert_eq!(c.get("a", t0 + Duration::from_secs(61)), None);
        // expired entries are purged on the next insert
        c.insert("b".into(), "B".into(), t0 + Duration::from_secs(61));
        assert_eq!(c.len(), 1);
        // at most MAX_ENTRIES (one per viewer with --auth): oldest evicted
        let t1 = t0 + Duration::from_secs(62);
        for i in 0..RenderCache::MAX_ENTRIES + 10 {
            c.insert(
                format!("k{i}"),
                "x".into(),
                t1 + Duration::from_millis(i as u64),
            );
        }
        assert_eq!(c.len(), RenderCache::MAX_ENTRIES);
        let late = t1 + Duration::from_secs(1);
        assert_eq!(c.get("k0", late), None);
        assert!(c
            .get(&format!("k{}", RenderCache::MAX_ENTRIES + 9), late)
            .is_some());
        // byte cap: a huge page is not cached; big pages evict old ones
        let c = RenderCache::new(Duration::from_secs(60));
        c.insert(
            "huge".into(),
            "x".repeat(RenderCache::MAX_BYTES / 4 + 1),
            t0,
        );
        assert_eq!(c.len(), 0);
        let big = "x".repeat(RenderCache::MAX_BYTES / 4);
        for i in 0..6 {
            c.insert(format!("p{i}"), big.clone(), t0 + Duration::from_millis(i));
        }
        assert_eq!(c.len(), 4);
        assert!(c.get("p0", t0).is_none() && c.get("p5", t0).is_some());
        // ttl 0 = off
        let off = RenderCache::new(Duration::ZERO);
        off.insert("a".into(), "A".into(), t0);
        assert_eq!((off.len(), off.get("a", t0)), (0, None));
    }

    #[test]
    fn cache_keys_are_per_viewer() {
        let r = vec![("region".to_string(), "EU".to_string())];
        let id = |user: &str, vars: Vec<(String, String)>| Identity {
            claims: Claims {
                user: user.into(),
                ..Default::default()
            },
            method: "proxy",
            scope: Scope {
                dashboards: None,
                vars,
            },
            page_session: None,
            ref_id: None,
        };
        let (alice, bob) = (id("alice", vec![]), id("bob", vec![]));
        assert_ne!(
            cache_key("d", &r, Some(&alice), None),
            cache_key("d", &r, Some(&bob), None)
        );
        assert_ne!(
            cache_key("d", &r, Some(&alice), None),
            cache_key("d", &r, None, None)
        );
        // the same claims with other locked variables (two embed links)
        let eu = id("embed", vec![("region".into(), "'EU'".into())]);
        let us = id("embed", vec![("region".into(), "'US'".into())]);
        assert_ne!(
            cache_key("d", &r, Some(&eu), None),
            cache_key("d", &r, Some(&us), None)
        );
    }
}

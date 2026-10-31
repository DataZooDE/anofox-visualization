//! `anofox-visualization serve` — two modes:
//!
//! **Authoring** (default): `serve [db]` launches the embedded browser builder
//! wired to a DuckDB file via a `/query` endpoint that runs client-supplied SQL
//! (through one long-lived `duckdb` CLI session, so ATTACH, TEMP objects and
//! variables persist between statements, #275). Because it runs arbitrary SQL
//! it is locked to the author: loopback bind only, the `Host` header must be
//! loopback (DNS rebinding), cross-origin requests are refused (CSRF), and
//! every request needs the random per-run token from the printed URL
//! (`?token=` → `HttpOnly; SameSite=Strict` cookie, or the `X-Anofox-Token`
//! header). Bodies containing CLI dot-commands (`.shell`, `.read`, …) are
//! refused before reaching the CLI.
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
use anofox_visualization::host::assist;
use anofox_visualization::host::auth::{self, apikey, audit::AuditLog, Auth, Identity, Scope};
use anofox_visualization::host::serving::{
    self as sv, html_escape as esc, sql_string_literal, AuthoringGuard, RequestMeta,
};
use anofox_visualization::theme::DashboardChrome;
use anofox_visualization::{page, render_with_title, roles, sql, track, RenderOptions, Role};
use include_dir::{include_dir, Dir};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// `--app-charts-as-tables` (#403) is a sibling with no header effect —
    /// `--dashboards` (mode B) has no client script to seed, only
    /// `anofox_serve_dashboards`'s own dashboard pages and the authoring
    /// builder do.
    app: AppBrand,
    /// `--home <id>` (with `--dashboards`): `/` redirects to that dashboard.
    home: Option<String>,
    /// `--query-timeout SECS` (authoring only, #304/sec-review-342 L1): a
    /// round that doesn't finish in time gets its `duckdb` child killed
    /// instead of wedging the only authoring worker forever — the
    /// structural bound behind the cheap `reads_stdin` fast path, and the
    /// only thing that also covers a computed or otherwise-missed path to
    /// the same hang.
    authoring_timeout: Duration,
    /// `--assets <dir>` (authoring only, #522): opt-in; off (`None`) means
    /// today's behaviour unchanged (no `/assets/<rest>` route, every
    /// relative `::IMAGE`/markdown source draws the alt-text placeholder).
    /// Validated and canonicalized once at startup
    /// ([`anofox_visualization::image::validated_assets_root`]), not
    /// derived from a dashboards folder — authoring has none.
    assets_dir: Option<String>,
    /// `--assist-*` / `--no-assist` (authoring only, #740): the `/assist`
    /// proxy to the model API. On when a key is set (`ANTHROPIC_API_KEY` or
    /// `--assist-key-file`), unless `--no-assist`.
    assist: assist::AssistOptions,
}

/// The `--theme` default (set once in `main`).
static THEME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
fn theme_default() -> Option<&'static str> {
    THEME.get().and_then(|t| t.as_deref())
}

/// The `--app-*` settings as the builder's `<script>` ("" = none; set once in `main`).
static APP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// #740: the builder's `assistant` block when the `/assist` proxy is on (set
/// once in `authoring_mode`; after `APP` in the page).
static ASSIST_PAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// The pages' CSP `frame-ancestors` (#252): `'none'`, or empty (no directive)
/// with an `embed` login, whose links are meant to be framed.
static FRAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();

const USAGE: &str = "usage: serve [--db] <db> [--port N] [--no-open] [--theme T]    (authoring, loopback only)\n             [--app-name TEXT] [--app-tagline TEXT] [--app-logo FILE|URL]\n             [--app-logo-dark FILE|URL] [--app-link URL] [--app-favicon FILE|URL] [--app-hide-name]\n             [--app-charts-as-tables]   (the builder's app bar; the last turns \"Charts as tables\" on by default for every viewer who hasn't chosen their own)\n             [--query-timeout SECS]   (default 120; a round that doesn't finish in time restarts the session)\n             [--assets DIR]   (opt-in: serve relative ::IMAGE/markdown image sources from DIR at /assets/<path>; off by default)\n             [--assist-key-file FILE] [--no-assist]   (the builder assistant's /assist proxy: on when ANTHROPIC_API_KEY\n                                       or a 0600 key file holds a key; the key never reaches the browser)\n             [--assist-models a,b] [--assist-base-url URL] [--assist-max-tokens N]\n             [--assist-budget-tokens-per-hour N] [--assist-log PATH|stderr] [--assist-log-content]\n       serve --dashboards <dir> [--db <db>] [--init setup.sql] [--bind ADDR] [--port N]\n             [--cache SECS] [--max-rows N] [--timeout SECS] [--threads N] [--theme T]\n             [--allow-host NAME]...   (Host names served besides loopback; * = any)\n             [--auth JSON|@file.json] (login: auth proxy, JWT/OIDC, embed links, API keys, share links —\n                                       see docs/secure-serving.md)\n             [--audit PATH|stderr]    (JSON-lines audit log of logins and refusals)\n             [--log PATH|stderr] [--log-vars none|names|values]   (JSON-lines request log)\n             [--home ID]              (/ redirects to that dashboard instead of the landing page)\n             [--app-name …] [--app-tagline …] [--app-logo …] [--app-logo-dark …] [--app-favicon …]   (the landing page's header)\n       (T: light | dark | print | colorblind | high-contrast | 'acme+dark' | theme.json;\n        a dashboard's own ::THEME wins)\n       serve api-key new --file KEYS.json --user U [--groups a,b] [--claim k=v]...\n             [--dashboards a,b] [--scopes S,…] [--expires-days N] [--note TEXT] [--audit PATH]\n             (S: ingest:<table> | preview | embed | admin; none = read dashboards only)\n       serve api-key revoke --file KEYS.json --id ID [--audit PATH]\n       serve api-key list --file KEYS.json\n       serve share-password [--generate]   (a shared dashboard's password_hash; the password on stdin)\n       serve --version\n\nQueries run through the `duckdb` CLI on PATH.";

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
        authoring_timeout: Duration::from_secs(120),
        assets_dir: None,
        assist: assist::AssistOptions::default(),
    };
    let mut assist_given = false;
    let (mut log, mut log_vars, mut audit) = (None, None, None);
    let mut query_timeout_given = false;
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
                // #604 S2: 0 is a real, meaningful value here — "ask the OS
                // for any free loopback port", not a sentinel this parser
                // should refuse. `authoring_mode`/`serve_dashboards` report
                // the port actually bound, never the literal `0` back.
                let p = num(args.next(), "--port");
                o.port = u16::try_from(p).unwrap_or_else(|_| usage("--port must be 0..=65535"));
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
            "--query-timeout" => {
                o.authoring_timeout =
                    Duration::from_secs(num(args.next(), "--query-timeout").max(1));
                query_timeout_given = true;
            }
            "--threads" => o.threads = num(args.next(), "--threads").clamp(1, 64) as usize,
            "--assets" => {
                o.assets_dir = Some(
                    args.next()
                        .unwrap_or_else(|| usage("--assets needs a directory")),
                )
            }
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
            // #403: an installation default that seeds a viewer's never-set
            // initial "Charts as tables" choice (web/app.js's dpAsTables) —
            // never overrides an explicit ?tables=/remembered choice, and
            // never forces the in-page -- @astables on bar on (#401).
            "--app-charts-as-tables" => o.app.charts_as_tables = Some(true),
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
            "--no-assist" => {
                o.assist.off = true;
                assist_given = true;
            }
            "--assist-log-content" => {
                o.assist.cfg.log_content = true;
                assist_given = true;
            }
            "--assist-models"
            | "--assist-base-url"
            | "--assist-key-file"
            | "--assist-budget-tokens-per-hour"
            | "--assist-max-tokens"
            | "--assist-log" => {
                assist_given = true;
                let v = args
                    .next()
                    .unwrap_or_else(|| usage(&format!("{a} needs a value")));
                let r = match a.as_str() {
                    "--assist-models" => o.assist.cfg.set_models(&v),
                    "--assist-base-url" => o.assist.cfg.set_base_url(&v),
                    "--assist-key-file" => {
                        o.assist.key_file = Some(PathBuf::from(v));
                        Ok(())
                    }
                    "--assist-log" => {
                        o.assist.log = Some(v);
                        Ok(())
                    }
                    _ => match v.parse::<u64>() {
                        Ok(n) if n > 0 => {
                            if a == "--assist-max-tokens" {
                                o.assist.cfg.max_tokens = n;
                            } else {
                                o.assist.cfg.budget_tokens_per_hour = n;
                            }
                            Ok(())
                        }
                        _ => Err(format!("{a} needs a positive number")),
                    },
                };
                r.unwrap_or_else(|e| usage(&e));
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
    // sec-review-342 L1 Info: --dashboards (static render) and the duckext
    // locked server never reach `Live::round_with`'s watchdog, so silently
    // accepting --query-timeout there would suggest a timeout it doesn't
    // apply — reject it with a clear message instead (`--dashboards` has
    // its own `--timeout`, for the query itself rather than one round of
    // the authoring session).
    if o.dashboards_dir.is_some() && query_timeout_given {
        usage("--query-timeout applies to authoring mode only (serve <db>); --dashboards has --timeout");
    }
    // #522: `--dashboards` serving already resolves relative assets from
    // its own folder's `assets/` subfolder (no option needed); `--assets`
    // names authoring's otherwise-nonexistent root directly.
    if o.dashboards_dir.is_some() && assist_given {
        usage("--assist-* and --no-assist apply to authoring mode only (serve <db>): a locked server has no /assist");
    }
    if o.dashboards_dir.is_some() && o.assets_dir.is_some() {
        usage("--assets applies to authoring mode only (serve <db>); --dashboards already serves <dir>/assets");
    }
    // #604 S2: `--port 0` ("ask the OS for any free loopback port") is
    // authoring-only — `serve_dashboards` doesn't resolve/report the real
    // bound port back, so `--port 0` there would silently misreport `0`.
    if o.dashboards_dir.is_some() && o.port == 0 {
        usage("--port 0 is authoring-mode only (serve <db>); --dashboards needs an explicit port");
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
    // Owner decision (#333): the anofox extensions this binary's `duckdb`
    // child sessions may LOAD default to no telemetry. Must run before any
    // thread or `duckdb` child exists — first line of `main`, see
    // `host::telemetry` for the default and its thread-safety reasoning.
    anofox_visualization::host::telemetry::ensure_default_once();
    if std::env::args().nth(1).as_deref() == Some("api-key") {
        api_key_cli(std::env::args().skip(2).collect());
    }
    if std::env::args().nth(1).as_deref() == Some("share-password") {
        share_password_cli(std::env::args().skip(2).collect());
    }
    let mut o = parse_args();
    let _ = THEME.set(o.theme.clone());
    let _ = APP.set(o.app.script());
    let embed = o
        .auth
        .as_ref()
        .is_some_and(|a| a.methods().contains(&"embed"));
    let _ = FRAME.set(if embed {
        String::new()
    } else {
        "'none'".into()
    });
    match o.dashboards_dir.clone() {
        Some(dir) => serve_dashboards(&dir, &mut o),
        None => authoring_mode(&o),
    }
}

/// `serve share-password [--generate]` (#763): the `password_hash` of a
/// password-protected shared dashboard. The password comes from stdin (one
/// line), or `--generate` makes one up and prints it once. Never takes it as
/// an argument (it would land in the shell history and the process list).
fn share_password_cli(args: Vec<String>) -> ! {
    use anofox_visualization::host::auth::share;
    let fail = |m: &str| -> ! {
        eprintln!("serve share-password: {m}");
        std::process::exit(2)
    };
    let password = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--generate"] => {
            let p = share::generate_password();
            println!("password: {p}   (shown once — give it to the viewers)");
            p
        }
        [] => {
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                fail("could not read the password from stdin");
            }
            let p = line.trim_end_matches(['\n', '\r']).to_string();
            if p.len() < share::MIN_PASSWORD {
                fail(&format!(
                    "a password of at least {} characters (or --generate)",
                    share::MIN_PASSWORD
                ));
            }
            p
        }
        _ => fail("usage: serve share-password [--generate]   (the password on stdin, one line)"),
    };
    println!("password_hash: {}", share::hash_password(&password));
    std::process::exit(0)
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
    let mut scopes: Vec<String> = Vec::new();
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
                // `--dashboards ''`: none (a key that only ingests, say).
                dashboards = Some(
                    val()
                        .split(',')
                        .map(|d| d.trim().to_string())
                        .filter(|d| !d.is_empty())
                        .collect::<Vec<_>>(),
                )
            }
            "--scopes" => {
                scopes = val()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
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
            if let Some(d) = &dashboards {
                e["dashboards"] = serde_json::json!(d);
            }
            if !scopes.is_empty() {
                let names: Vec<&str> = scopes.iter().map(String::as_str).collect();
                let s = apikey::KeyScopes::parse(&names).unwrap_or_else(|e| fail(&e));
                e["scopes"] = serde_json::json!(s.names());
                if dashboards.is_none() {
                    eprintln!(
                        "note: this key also reads every dashboard; --dashboards '' gives it none"
                    );
                }
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
                    "{}  {}  {}{}{}",
                    e["id"].as_str().unwrap_or(""),
                    e["user"].as_str().unwrap_or(""),
                    e.get("dashboards")
                        .map_or("all dashboards".to_string(), |d| d.to_string()),
                    e.get("scopes")
                        .map_or(String::new(), |s| format!("  scopes {s}")),
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
    respond_csp(req, status, ctype, body, false)
}

/// [`respond`]; `duckdb_esm`: the page may import DuckDB-Wasm's `+esm`
/// build ([`sv::page_csp_with`]) — the authoring builder with the `/assist`
/// proxy on (#740).
fn respond_csp(req: Request, status: u16, ctype: &str, body: impl Into<Vec<u8>>, duckdb_esm: bool) {
    let mut body = body.into();
    // An HTML page gets its CSP header and nonces (#252).
    let csp = ctype.starts_with("text/html").then(|| {
        let frame = FRAME.get().map_or("'none'", String::as_str);
        let (html, csp) = sv::csp_page_with(&String::from_utf8_lossy(&body), frame, duckdb_esm);
        body = html.into_bytes();
        csp
    });
    // same-origin: a page URL can carry an embed page session.
    let mut resp = Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", ctype))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Referrer-Policy", "same-origin"));
    if let Some(csp) = csp {
        resp = resp.with_header(header("Content-Security-Policy", &csp));
    }
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

/// A startup line on stdout that never panics: `println!` panics on EPIPE
/// (Rust ignores SIGPIPE), so a reader that closed its end after the first
/// line — `serve … | head -1`, a test harness that only parses the URL —
/// would take the whole server down while it is already accepting
/// connections, and every queued request would get tiny_http's empty 500.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}").and_then(|_| out.flush());
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
        #[cfg_attr(not(unix), allow(unused_mut))]
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
    // #604 S2: the port actually bound — equal to `o.port` unless that was
    // `0`, in which case this is whatever the OS handed back. Every later
    // use (the guard's cookie name, the printed/returned URL) is this one,
    // never the literal `o.port` a caller may have passed as `0`.
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(a) => a.port(),
        // Unix-only variant in tiny_http (#649).
        #[cfg(unix)]
        tiny_http::ListenAddr::Unix(_) => o.port,
    };
    let guard = AuthoringGuard::new(port);
    let host = if o.bind.contains(':') {
        format!("[{}]", o.bind)
    } else {
        o.bind.clone()
    };
    let url = guard.login_url(&host);
    // #522: opt-in only — refuse loudly at start rather than silently
    // serving nothing (an author who passed --assets gets a wrong dir
    // flagged immediately, not a page of broken images later).
    let assets_root = o.assets_dir.as_deref().map(|d| {
        anofox_visualization::image::validated_assets_root(Path::new(d)).unwrap_or_else(|e| {
            eprintln!("error: --assets {e}");
            std::process::exit(2)
        })
    });
    let assets_note = assets_root
        .as_ref()
        .map(|p: &PathBuf| format!("\n  assets: {}", p.display()))
        .unwrap_or_default();
    let a = &o.assist;
    let (proxy, assist_note) = match assist::build(
        a.off,
        a.cfg.clone(),
        a.key_file.as_deref(),
        a.log.as_deref(),
    ) {
        Ok(Some((p, note))) => {
            let _ = ASSIST_PAGE.set(p.page_script());
            (Some(p), format!("\n  {note}"))
        }
        Ok(None) => (None, String::new()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2)
        }
    };
    say(&format!(
        "anofox-visualization serving {url}\n  (authoring — client SQL; loopback only; the URL carries this run's token)\n  database: {db}{assets_note}{assist_note}\n  (Ctrl-C to stop)"
    ));
    if o.open_browser {
        let _ = open::that(&url);
    }
    // #304: clean up the session's private staging directory even if we're
    // killed (Ctrl-C, `kill`) rather than exiting normally.
    install_cleanup_on_signal();
    let session = Mutex::new(AuthoringSession::new(db.clone(), o.authoring_timeout));
    run_workers(server, 1, move |req| {
        handle_authoring(
            &guard,
            &session,
            assets_root.as_deref(),
            proxy.as_ref(),
            req,
        )
    });
}

fn handle_authoring(
    guard: &AuthoringGuard,
    session: &Mutex<AuthoringSession>,
    assets_root: Option<&Path>,
    proxy: Option<&assist::Proxy>,
    mut req: Request,
) {
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
    // #740: after the guard, like /query. 404 when the proxy is off.
    // Only `/assist/v1/…`: `/assist/<file>` is the builder's own code
    // (web/assist/).
    if assist::is_api_path(&path) {
        return assist_route(proxy, &path, req);
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
        // #304: a read of the CLI's own stdin blocks forever (it's the pipe
        // this session writes staged queries to), wedging the only
        // authoring worker until the duckdb process is killed and
        // restarted. Refuse before the CLI ever sees it.
        if sv::reads_stdin(&sql) {
            return text(
                req,
                400,
                "reading /dev/stdin is not allowed here (it is this session's own query pipe \
                 and would hang); read a file instead",
            );
        }
        return match session.lock().unwrap_or_else(|p| p.into_inner()).run(&sql) {
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
    // #522: before the generic GET-only check below (this route also
    // answers HEAD) — `guard.check` above already gated every route this
    // function handles, this one included.
    if let Some(root) = assets_root {
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
                && (theme_default().is_some()
                    || APP.get().is_some_and(|a| !a.is_empty())
                    || ASSIST_PAGE.get().is_some()) =>
        {
            let html = String::from_utf8_lossy(f.contents()).into_owned();
            let mut head = APP.get().cloned().unwrap_or_default();
            if let Some(a) = ASSIST_PAGE.get() {
                head.push_str(a);
            }
            if let Some(t) = theme_default() {
                let spec = serde_json::to_string(t)
                    .unwrap_or_default()
                    .replace('<', "\\u003c");
                head.push_str(&format!("<script>window.__anofoxTheme={spec};</script>"));
            }
            let html = html.replacen("</head>", &format!("{head}</head>"), 1);
            let esm = ASSIST_PAGE.get().is_some();
            respond_csp(req, 200, sv::content_type(asset), html.into_bytes(), esm)
        }
        Some(f) => respond(req, 200, sv::content_type(asset), f.contents().to_vec()),
        None => text(req, 404, "not found"),
    }
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

/// `GET`/`HEAD /assets/<rest>` (#522, opt-in): one image file from
/// authoring's own `--assets <dir>` root (`handle_authoring` only reaches
/// this when `assets_root` is `Some`). Same containment/extension/
/// magic-byte/size rules and response headers as the locked server's own
/// `/d/<id>/assets/<rest>` (`duckext::serve::locked_asset`) — both go
/// through `anofox_visualization::image`'s shared validator, nothing
/// duplicated here. Every refusal is a plain 404: which one it was
/// (missing, traversal, wrong extension, not really an image, too big) is
/// not a caller's business.
fn authoring_asset(root: &Path, rest: &str, req: Request) {
    const MAX: u64 = anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES;
    // Loopback-only, single-token session — still `private`, and a short
    // max-age since an asset can change under an author's feet mid-session,
    // same as the dashboard script itself. Review of #604 N5: the header
    // list itself (CSP, nosniff, CORP) is `image::asset_header_values`,
    // shared with `locked_asset` and the extension's own `authoring_asset`
    // — only this lifetime and the mime differ by caller.
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

// ---------- the authoring session (#275) ----------

/// The current `AuthoringSession`'s private staging directory (`#304`), so a
/// SIGINT/SIGTERM handler — which cannot safely reach a `Mutex<AuthoringSession>`
/// a signal might land while it's locked — can still remove it. Set once at
/// [`AuthoringSession::new`], cleared by [`Drop for AuthoringSession`] on a
/// normal exit, and otherwise removed (once) by [`cleanup_on_signal`].
static SESSION_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Install a SIGINT/SIGTERM handler that removes [`SESSION_DIR`] before the
/// process exits (`#304`): without one, `Drop for AuthoringSession` never
/// runs on a kill (the default disposition terminates the process without
/// unwinding), and every Ctrl-C or `kill` leaves an `anofox-session-*`
/// directory — 0700, but possibly still holding an in-flight query's text —
/// behind in the temp directory. The handler itself only sets a flag
/// (signal-safe); a background thread polls it and does the actual cleanup,
/// since taking a mutex or touching the filesystem from inside a signal
/// handler is not guaranteed safe. Unix only (`serve` is best-effort on
/// Windows, #275 I3); on Windows the directory is left as before.
#[cfg(unix)]
fn install_cleanup_on_signal() {
    use std::sync::atomic::AtomicI32;
    // 0 = no signal yet; a real signal number is always > 0.
    static SIGNALLED: AtomicI32 = AtomicI32::new(0);
    extern "C" fn on_signal(sig: libc::c_int) {
        SIGNALLED.store(sig, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    std::thread::spawn(|| loop {
        let sig = SIGNALLED.load(Ordering::SeqCst);
        if sig != 0 {
            if let Some(dir) = SESSION_DIR.lock().unwrap_or_else(|p| p.into_inner()).take() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            // 128 + signal number: 130 for SIGINT, 143 for SIGTERM — the
            // shell's own convention, which supervisors such as systemd
            // read as a clean stop (sec-review-342 I1; this used to always
            // exit 130, even on SIGTERM).
            std::process::exit(128 + sig);
        }
        std::thread::sleep(Duration::from_millis(50));
    });
}
#[cfg(not(unix))]
fn install_cleanup_on_signal() {}

/// One long-lived `duckdb` CLI process for the builder's `/query`s, so what a
/// statement leaves in the session — ATTACH, TEMP tables / views / macros,
/// SET VARIABLE, LOAD, SET, USE — is there for the next one (as in DuckDB-Wasm
/// and `anofox_serve()`). Each body is written to a file in a private
/// directory and run with `.read`; a `.print` marker and a `.read` of a
/// missing `end-<nonce>` file mark the end of its stdout and stderr (dot
/// commands: they still run in an aborted transaction). The nonce is fresh
/// per request, so a query can't fake the end. If the CLI exits, the next
/// query starts a new session.
struct AuthoringSession {
    db: String,
    dir: PathBuf,
    live: Option<Live>,
    /// #304/sec-review-342 L1: a round that exceeds this is killed instead
    /// of wedging the only authoring worker forever.
    round_timeout: Duration,
}

struct Live {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    out: BufReader<std::process::ChildStdout>,
    err: std::sync::mpsc::Receiver<String>,
}

enum RoundErr {
    /// The statement failed; the session is fine.
    Sql(String),
    /// The CLI is gone (stderr's tail).
    Ended(String),
    /// The round ran longer than its timeout and the child was killed
    /// (sec-review-342 L1): this is the structural backstop for *any* query
    /// that doesn't finish — a stdin read by a path `reads_stdin` doesn't
    /// catch, or anything else that hangs.
    TimedOut(Duration),
}

impl AuthoringSession {
    fn new(db: String, round_timeout: Duration) -> Self {
        let dir = private_dir("anofox-session");
        let p = dir.to_string_lossy();
        if p.contains(['\'', '\n', '\r']) {
            eprintln!(
                "error: the temp directory path ({p}) contains a quote or a line break; set TMPDIR to another directory"
            );
            std::process::exit(1);
        }
        // #304: a signal handler cleans this up if the process is killed
        // before `Drop` ever runs.
        *SESSION_DIR.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir.clone());
        AuthoringSession {
            db,
            dir,
            live: None,
            round_timeout,
        }
    }

    fn run(&mut self, sql: &str) -> Result<String, String> {
        if self.live.is_none() {
            self.live = Some(Live::spawn(&self.db, &self.dir, self.round_timeout)?);
        }
        let r =
            self.live
                .as_mut()
                .expect("a live session")
                .round(&self.dir, sql, self.round_timeout);
        let _ = std::fs::remove_file(self.dir.join("q.sql"));
        match r {
            Ok(out) => Ok(out),
            Err(RoundErr::Sql(e)) => Err(e),
            Err(RoundErr::Ended(tail)) => {
                if let Some(mut l) = self.live.take() {
                    let _ = l.child.kill();
                    let _ = l.child.wait();
                }
                Err(format!(
                    "the DuckDB session ended; the next query starts a new one (ATTACH, TEMP objects and variables are gone){}",
                    if tail.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", tail.trim())
                    }
                ))
            }
            Err(RoundErr::TimedOut(d)) => {
                if let Some(mut l) = self.live.take() {
                    let _ = l.child.kill();
                    let _ = l.child.wait();
                }
                Err(format!(
                    "query exceeded {}s and was stopped; the next query starts a new session \
                     (ATTACH, TEMP objects and variables are gone)",
                    d.as_secs()
                ))
            }
        }
    }
}

impl Drop for AuthoringSession {
    fn drop(&mut self) {
        if let Some(mut l) = self.live.take() {
            let _ = l.child.kill();
            let _ = l.child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        // #304: a clean exit removed it already; don't let the signal
        // handler remove a *later* session's directory by mistake.
        let mut g = SESSION_DIR.lock().unwrap_or_else(|p| p.into_inner());
        if g.as_deref() == Some(self.dir.as_path()) {
            *g = None;
        }
    }
}

impl Live {
    fn spawn(db: &str, dir: &Path, round_timeout: Duration) -> Result<Live, String> {
        let mut cmd = Command::new("duckdb");
        cmd.arg(db).arg("-json");
        anofox_visualization::host::duckdb_cli::prepare("duckdb", &mut cmd);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("duckdb: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let out = BufReader::new(child.stdout.take().expect("piped stdout"));
        let err_pipe = child.stderr.take().expect("piped stderr");
        let (tx, err) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(err_pipe).lines() {
                let Ok(l) = line else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut live = Live {
            child,
            stdin,
            out,
            err,
        };
        // Reset what a `~/.duckdbrc` could otherwise break or hang every
        // request with (#275 L2): `.bail on` would exit the CLI at the
        // stderr end marker's deliberate "missing file" error, and a
        // leftover `.output …` would swallow the `.print` marker. These are
        // server-written dot commands, sent before the warm-up round's own
        // markers; the client-side dot-command refusal is unaffected.
        match live.round_with(dir, ".bail off\n.output\n.mode json\n", "", round_timeout) {
            Ok(_) | Err(RoundErr::Sql(_)) => Ok(live),
            Err(RoundErr::Ended(t)) => {
                // #275 L4: don't leak a zombie when the session never came up.
                let _ = live.child.kill();
                let _ = live.child.wait();
                Err(format!("duckdb did not start: {}", t.trim()))
            }
            Err(RoundErr::TimedOut(d)) => {
                let _ = live.child.kill();
                let _ = live.child.wait();
                Err(format!("duckdb did not start within {}s", d.as_secs()))
            }
        }
    }

    fn round(&mut self, dir: &Path, sql: &str, timeout: Duration) -> Result<String, RoundErr> {
        self.round_with(dir, "", sql, timeout)
    }

    fn round_with(
        &mut self,
        dir: &Path,
        preamble: &str,
        sql: &str,
        timeout: Duration,
    ) -> Result<String, RoundErr> {
        let q = dir.join("q.sql");
        // Drop any previous (now read-only, #275 L3) staged query first.
        let _ = std::fs::remove_file(&q);
        write_private(&q, sql)
            .map_err(|e| RoundErr::Sql(format!("could not stage the query: {e}")))?;
        // Read-only once staged (#275 L3): the query can't rewrite its own
        // file out from under the CLI's still-open `.read` of it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o400));
        }
        let nonce = sv::random_token();
        let done_marker = format!("__anofox_done_{nonce}");
        let script = format!(
            "{preamble}.read '{}'\n.print {done_marker}\n.read '{}'\n",
            q.display(),
            dir.join(format!("end-{nonce}")).display()
        );
        if self
            .stdin
            .write_all(script.as_bytes())
            .and_then(|_| self.stdin.flush())
            .is_err()
        {
            return Err(RoundErr::Ended(self.drain_err()));
        }

        // sec-review-342 L1: a structural backstop, not just the cheap
        // `reads_stdin` fast path — any round that doesn't finish in
        // `timeout` (a stdin read by a path the fast path misses, or
        // anything else that hangs) gets this child killed instead of
        // wedging the only authoring worker forever.
        //
        // sec-review-342 L3: the watchdog blocks on `done_rx.recv_timeout`
        // instead of unconditionally sleeping the full timeout, so a fast
        // round (the common case) wakes it immediately — dropping
        // `done_tx` below closes the channel, which `recv_timeout` reports
        // as `Disconnected` right away — instead of leaving a thread parked
        // for up to `timeout` with nothing to do. A query loop therefore
        // can't build up one parked thread per round. `round_with` then
        // `join`s the watchdog before returning, so the child is never
        // reaped (by `run()`, `Drop`, …) while a watchdog that might still
        // call `kill` on its pid is alive — closing the kill-after-check
        // (TOCTOU) window a sleep-based watchdog leaves between "load
        // `round_done`" and "kill", where the pid could in theory be
        // reused by an unrelated process in between.
        //
        // `Builder::spawn` (not the panicking `thread::spawn`) so that
        // hitting the OS thread limit (e.g. a client spawning many rounds
        // fast enough to still have one watchdog each in flight) degrades
        // to "this one round has no timeout backstop" instead of a panic
        // that would take down the single authoring worker.
        let timed_out = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        #[cfg(unix)]
        let watchdog = {
            let (timed_out, pid) = (timed_out.clone(), self.child.id());
            std::thread::Builder::new()
                .spawn(move || {
                    use std::sync::mpsc::RecvTimeoutError;
                    if let Err(RecvTimeoutError::Timeout) = done_rx.recv_timeout(timeout) {
                        timed_out.store(true, Ordering::SeqCst);
                        unsafe {
                            libc::kill(pid as libc::pid_t, libc::SIGKILL);
                        }
                    }
                })
                .ok()
        };
        // sec-review-342 L3 (Info): no watchdog on Windows (`serve` is
        // best-effort there, #275 I3) — a hang there has no backstop beyond
        // `reads_stdin`'s fast path, and `timed_out`/`done_rx` are simply
        // unused.
        #[cfg(not(unix))]
        let _ = (&timed_out, done_rx);

        let result: Result<String, RoundErr> = 'round: {
            let mut out = String::new();
            loop {
                let mut line = String::new();
                match self.out.read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        break 'round Err(if timed_out.load(Ordering::SeqCst) {
                            RoundErr::TimedOut(timeout)
                        } else {
                            RoundErr::Ended(self.drain_err())
                        })
                    }
                    Ok(_) => {
                        // #275 L1: the marker is matched as a line's suffix, not
                        // only as a whole line, so a statement's output that
                        // doesn't end in `\n` (no trailing newline, or a `\r`
                        // line terminator) can't merge with the marker and hang
                        // this loop forever.
                        let l = line.trim_end_matches(['\r', '\n']);
                        if let Some(head) = l.strip_suffix(done_marker.as_str()) {
                            out.push_str(head);
                            break;
                        }
                        out.push_str(&line);
                    }
                }
            }
            let mut err = String::new();
            loop {
                match self.err.recv() {
                    Ok(l) if l.contains(&nonce) => break,
                    Ok(l) => {
                        err.push_str(&l);
                        err.push('\n');
                    }
                    Err(_) => {
                        break 'round Err(if timed_out.load(Ordering::SeqCst) {
                            RoundErr::TimedOut(timeout)
                        } else {
                            RoundErr::Ended(err)
                        })
                    }
                }
            }
            if err.trim().is_empty() {
                Ok(out)
            } else {
                Err(RoundErr::Sql(err.trim_end().to_string()))
            }
        };
        // Wake a still-waiting watchdog at once (dropping the sender closes
        // the channel, which its `recv_timeout` sees immediately, rather
        // than after the full timeout), then join it — the child can't be
        // reaped anywhere else while it might still be about to `kill` it.
        drop(done_tx);
        #[cfg(unix)]
        if let Some(h) = watchdog {
            let _ = h.join();
        }
        result
    }

    /// stderr's remaining lines, once the CLI is gone.
    fn drain_err(&mut self) -> String {
        let mut s = String::new();
        while let Ok(l) = self.err.recv_timeout(Duration::from_millis(200)) {
            s.push_str(&l);
            s.push('\n');
        }
        s
    }
}

/// Create/overwrite `p` (0600 on Unix) with `text`.
fn write_private(p: &Path, text: &str) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)?.write_all(text.as_bytes())
}

// ---------- serve mode (server owns the SQL; consumers pick id + params) ----------

struct Dashboard {
    id: String,
    meta: sv::DashboardMeta,
    script: String,
    /// The day the file last changed (`YYYY-MM-DD`), for the landing page.
    updated: Option<String>,
    /// #680: the plan indices of the setup statements that run before the
    /// panels on every render (`SET VARIABLE …`, `CREATE [OR REPLACE] TEMP
    /// TABLE|VIEW … AS <query>` — the locked server's data-folder rule,
    /// [`sv::read_only_statement`]), checked once at startup.
    setup_ok: std::collections::HashSet<usize>,
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
                        setup_ok: Default::default(),
                    });
                }
            }
        }
    }
    // #301: ascending `-- @order`, unordered dashboards after, then by id.
    out.sort_by(|a, b| a.meta.order_key(&a.id).cmp(&b.meta.order_key(&b.id)));
    out
}

/// #680: which setup statements of each dashboard run before its panels.
/// A TEMP VIEW/TABLE lives in one DuckDB session, and every render is a new
/// read-only session, so they run on every render (after the external-access
/// and configuration locks, see `render_dashboard_page`) rather than once.
/// Only the shapes the locked server allows with a data folder
/// ([`sv::read_only_statement`], by DuckDB's own parser); any other setup
/// statement is skipped, as before, with a warning here.
fn check_setup(dashboards: &mut [Dashboard]) {
    let mut duck = |sql: &str| -> Result<Vec<Vec<Option<String>>>, String> {
        let json = run_duckdb(":memory:", &["-json"], sql, Duration::from_secs(30))?;
        let rows: Vec<serde_json::Map<String, serde_json::Value>> = if json.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(json.trim()).map_err(|e| format!("duckdb -json: {e}"))?
        };
        Ok(rows
            .into_iter()
            .map(|r| {
                r.into_values()
                    .map(|v| match v {
                        serde_json::Value::Null => None,
                        serde_json::Value::String(s) => Some(s),
                        other => Some(other.to_string()),
                    })
                    .collect()
            })
            .collect())
    };
    for d in dashboards.iter_mut() {
        for (i, p) in sql::plan(&d.script).into_iter().enumerate() {
            if !p.setup {
                continue;
            }
            // a claim / `anofox_data` is the server's to set (re-asserted
            // after each setup statement anyway, see render_dashboard_page)
            let sets = p.sql.split_whitespace().collect::<Vec<_>>();
            let reserved = sets.len() > 2
                && sets[0].eq_ignore_ascii_case("SET")
                && sets[1].eq_ignore_ascii_case("VARIABLE")
                && access::is_reserved_var(sets[2].trim_end_matches('='));
            if reserved {
                eprintln!(
                    "  dashboard '{}': setup statement {} is skipped (it sets a reserved variable)",
                    d.id,
                    i + 1
                );
                continue;
            }
            match sv::read_only_statement(&p.sql, true, &mut duck) {
                Ok(()) => {
                    d.setup_ok.insert(i);
                }
                Err(e) => eprintln!(
                    "  dashboard '{}': setup statement {} is skipped (only SET VARIABLE and CREATE TEMP VIEW/TABLE … AS <query> run here; do the rest in --init): {e}",
                    d.id,
                    i + 1
                ),
            }
        }
    }
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
    /// `<dir>/assets`, canonicalized (#514 PR B) — `None` when there is no
    /// such folder next to `--dashboards <dir>`. This script-free page has
    /// no client JS to fetch a route with, so a relative `::IMAGE` source
    /// is inlined from here as a `data:` URI at render time instead (see
    /// `render_dashboard_page`'s `RenderOptions.asset_root`), the same as
    /// `dashboard`'s own `--svg`/HTML CLI output.
    assets_root: Option<PathBuf>,
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
        say(&format!("  init applied: {init}"));
    }
    // #680: the panels open the database read-only, so it must exist — the
    // private default is created here; a named `--db` that's missing is a
    // typo, not something to create.
    if !Path::new(&db).exists() {
        if o.db.is_empty() {
            if let Err(e) = run_duckdb(&db, &[], "SELECT 1", Duration::ZERO) {
                eprintln!("error: create {db}: {e}");
                std::process::exit(1);
            }
        } else {
            eprintln!("error: --db {db}: no such database (create it, or leave out --db)");
            std::process::exit(1);
        }
    }
    let mut dashboards = load_dashboards(Path::new(dir));
    check_setup(&mut dashboards);
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
    say(&format!(
        "anofox-visualization serving {} dashboard(s) at http://{addr}/ (read-only; no client SQL; cache: {cache_note})\n  database: {db}\n  dashboards: {dir}\n  (put TLS + auth in front for public use — docs/secure-serving.md)",
        dashboards.len()
    ));
    match (&o.auth, sv::is_loopback_bind(&o.bind)) {
        (Some(a), _) => {
            say(&format!("  login: {}", a.describe()));
            say("  no row/column policies here (only anofox_serve_dashboards has them): every panel must filter on getvariable('__claim_*') itself");
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
        assets_root: anofox_visualization::image::canonical_assets_dir(Path::new(dir)),
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
    // #740: the assistant's proxy is an authoring route only; a locked
    // server never has it, whatever the method or the login. (Only
    // `/assist/v1/…`: `/assist/<file>` is the UI's own code, web/assist/.)
    if assist::is_api_path(path) {
        return reply(req, 404, "not found");
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
            if (a.embed.is_some() || a.share.is_some())
                && req.method() == &Method::Post
                && path.starts_with("/d/") =>
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
    // #763: a shared dashboard's password form (`POST /share/<id>`).
    let share_body = match &st.auth {
        Some(a) if a.share.is_some() && path.starts_with("/share/") => {
            let mut body = String::new();
            if req.method() == &Method::Post {
                let read = (&mut req as &mut Request)
                    .as_reader()
                    .take(MAX_FORM + 1)
                    .read_to_string(&mut body);
                if read.is_err() || body.len() as u64 > MAX_FORM {
                    return reply(req, 400, "bad request: expected a small form");
                }
            }
            Some(body)
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
            if let Some(rep) = share_body.and_then(|b| auth.share_login(&r, &b)) {
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
        // #763: a share link's page session rides along the same way.
        Some(i) if matches!(i.method, "embed" | "share") => {
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

/// A statement's own `::KPI_SIZE` cell, read from its already-queried rows
/// (#405) — `Some(true)` 'compact', `Some(false)` 'normal', `None` with no
/// (or an unparseable) value or no `::KPI_SIZE` role at all.
fn kpi_size_json(roles: &[(usize, Role)], rows: &Rows) -> Option<bool> {
    let idx = roles.iter().find(|(_, r)| *r == Role::KpiSize)?.0;
    let v = rows.first()?.get(&format!("c{idx}"))?.as_str()?;
    match v.trim().to_ascii_lowercase().as_str() {
        "compact" => Some(true),
        "normal" => Some(false),
        _ => None,
    }
}

/// A caption (here, an `@param`'s name) with a trailing ":" — unless it
/// already ends with its own terminal punctuation (`? ! : … .`) once
/// trailing whitespace is trimmed (#539): a question caption like
/// "concept drift?" stays "concept drift?", never "concept drift?:".
fn caption_colon(text: &str) -> String {
    let t = text.trim_end();
    if t.is_empty() || t.ends_with(['?', '!', ':', '…', '.']) {
        t.to_string()
    } else {
        format!("{t}:")
    }
}

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

/// Why a panel's own renderer could not draw it (#285), redacted like
/// [`PAGE_ERROR`] (#279): the renderer's `<pre>error: …</pre>` text can quote
/// a role option's value (e.g. an `::ABLINE` or `::SMOOTH_METHOD` argument),
/// which can itself carry data. Details go to the server log only, via
/// `access::diag`.
const PANEL_RENDER_ERROR_HTML: &str = "<pre>this panel could not be drawn</pre>";

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

/// Append `SET VARIABLE`s for every name the credential's `scope` locks
/// (typed literals). Called once before any panel is queried, and again
/// after any later bind that could otherwise collide with a locked name
/// (#540 belt and braces): DuckDB's own rule is that the *last* `SET` for a
/// name wins, so re-asserting the locks after a bind is what actually makes
/// them stick, not just skipping them in [`may_bind_default`].
fn append_locked_vars(prefix: &mut String, scope: Option<&Scope>) {
    if let Some(s) = scope {
        for (name, lit) in &s.vars {
            prefix.push_str(&format!("SET VARIABLE {name} = {lit}; "));
        }
    }
}

/// Whether a default may still be bound for `name` while rendering a
/// script-free page: not already resolved from a `@param`, not locked by the
/// credential's `scope`, and not a reserved variable name. #540: a locked
/// variable always wins over a dashboard's own default, however the viewer
/// reaches this page — every place in `render_dashboard_page` that `SET`s a
/// variable after the locked ones (the ::INPUT defaults below, and a
/// `::GUIDE_STEP`'s own step variable, #516/review-537) must check this
/// first.
fn may_bind_default(name: &str, resolved: &[(String, String)], scope: Option<&Scope>) -> bool {
    !resolved.iter().any(|(n, _)| n == name)
        && !scope.is_some_and(|s| s.vars.iter().any(|(l, _)| l.eq_ignore_ascii_case(name)))
        && !access::is_reserved_var(name)
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
    append_locked_vars(&mut prefix, scope);
    // The login's claims (a @param can't name them: parse_dashboard_meta
    // ignores `__claim_*` params).
    for (name, lit) in claims.map(Claims::variables).unwrap_or_default() {
        prefix.push_str(&format!("SET VARIABLE {name} = {lit}; "));
    }
    let mut selects = String::new();
    for p in &dash.meta.params {
        let val = value_of(&p.name);
        // No inline `onchange` here (#557): the page's CSP (#252) has no
        // 'unsafe-inline'/'unsafe-hashes' for script-src, so a bare
        // event-handler attribute is silently blocked — `page::CONTROLS_SCRIPT`
        // wires the auto-submit instead, and the button below covers a
        // viewer whose browser runs no script at all.
        selects.push_str(&format!(
            "<label>{} <select name=\"{}\">",
            esc(&caption_colon(&p.name)),
            esc(&p.name)
        ));
        for opt in &p.allowed {
            let sel = if opt == &val { " selected" } else { "" };
            // #340 finding A: an explicit, separately escaped `value` so a
            // control character the no-JS form submits round-trips exactly,
            // instead of the option's (XML-safe, lossy) text content being
            // both the label and the implicit submitted value.
            selects.push_str(&format!(
                "<option value=\"{}\"{sel}>{}</option>",
                sv::html_escape_attr_roundtrip(opt),
                esc(opt)
            ));
        }
        selects.push_str("</select></label> ");
    }
    let controls = if dash.meta.params.is_empty() {
        String::new()
    } else {
        format!(
            "<form class=\"controls\" method=\"{}\" action=\"/d/{}\">{selects}\
             <button type=\"submit\">Apply</button>{}</form>",
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
    // The fourth element is each panel's own `::KPI_SIZE` ('compact') (#405);
    // the fifth is pre-rendered static HTML to push verbatim instead of the
    // usual chart render (#551: a `::SUBMIT` panel's disabled-button card —
    // never needs `opts`/the theme the usual render does, so it's built
    // directly in phase 1 rather than threading a marker to phase 2).
    let mut queried: Vec<(sql::Panel, Rows, bool, bool, Option<String>)> = Vec::new();
    // The script-free page shows only a ::VIEW group's first alternative
    // (#214); the others are never queried, and a `?view.*` request param
    // is not a thing — `resolve_params` above only reads declared `@param`s.
    let mut view_pick = sql::views::StaticPick::default();
    // The enclosing ::GROUP's own ::KPI_SIZE (#405) — a tile's own wins.
    let mut group_compact = false;
    // #274: a ::TAB / ::SUBTAB / ::VIEW alternative whose `-- @requires`
    // input is missing is left out with its panels (the tabs are flattened
    // here; the next shown alternative is the one drawn) — as the static
    // renderers do (src/dashboard.rs `execute`).
    let mut sections = sql::requires::Sections::default();
    // #274 L1: memoized per render — the same gate is often asked more than
    // once (a tab/view marker's own check, then a panel inside it).
    let mut req_cache = RequiresCache::new();
    // `::SHOW_IF` (#515): each condition's result, by plan index — a
    // condition always precedes what it governs (`sql::show_if::assign`),
    // so this is already populated by the time a later statement's
    // `show_if` is checked below.
    let mut show_if_cache: std::collections::HashMap<usize, bool> =
        std::collections::HashMap::new();
    // `::GUIDE_STEP` (#516): each group's running count of shown markers —
    // see `dashboard::execute`'s identical `step_n` for why "of N" can't be
    // filled in here (phase 2 does it, once every step is collected).
    let mut step_n: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
    for (i, panel) in sql::plan(&dash.script).into_iter().enumerate() {
        let hidden = sections.step(&panel, |reqs| {
            requires_unmet(st, &prefix, reqs, &mut req_cache)
        });
        if panel.setup {
            // #680: a checked setup statement (TEMP VIEW/TABLE, SET
            // VARIABLE) runs in this render's session, after the locks
            if dash.setup_ok.contains(&i) {
                prefix.push_str(panel.sql.trim().trim_end_matches(';'));
                // a newline, so a trailing `-- comment` can't swallow what follows
                prefix.push_str(";\n");
                // #540 belt and braces: a setup `SET VARIABLE` never
                // outlasts the credential's locked variables or claims —
                // the last `SET` for a name wins
                append_locked_vars(&mut prefix, scope);
                for (name, lit) in claims.map(Claims::variables).unwrap_or_default() {
                    prefix.push_str(&format!("SET VARIABLE {name} = {lit}; "));
                }
            }
            continue;
        }
        if hidden.is_some() || view_pick.drop_panel(&panel) {
            continue;
        }
        if !panel.show_if.is_empty()
            && !panel
                .show_if
                .iter()
                .all(|c| show_if_cache.get(c).copied().unwrap_or(false))
        {
            continue;
        }
        if panel.roles.len() == 1 && matches!(panel.roles[0].1, Role::ShowIf) {
            let q = format!(
                "{prefix}SELECT * FROM (\n{}\n) LIMIT 1",
                panel.sql.trim().trim_end_matches(';'),
            );
            // A query error is one of `::SHOW_IF`'s own fail-closed cases
            // (decision 2: FALSE / NULL / no row / non-boolean / an error all
            // hide the target) — never `?` here, unlike an ordinary panel's
            // query failure, which is a real error that should abort the
            // page. The raw DuckDB error can echo a viewer-supplied value
            // (e.g. a failed cast on a `getvariable()`), so only a generic
            // line reaches the log — never the error text itself.
            let v = match page_rows(st, &q) {
                Ok(rows) => rows.first().and_then(|row| row.values().next().cloned()),
                Err(_) => {
                    eprintln!(
                        "[anofox-serve] dashboard '{}' ::SHOW_IF condition (stmt {}) failed — \
                         treated as false",
                        dash.id,
                        i + 1
                    );
                    None
                }
            };
            show_if_cache.insert(
                i,
                anofox_visualization::sql::condition::holds_json(v.as_ref()),
            );
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
            let def = anofox_visualization::inputs::default_index(&panel.roles);
            for set in anofox_visualization::inputs::input_defaults_with(kind, idx, def, &rows) {
                let name = set.split('"').nth(1).unwrap_or("");
                if may_bind_default(name, resolved, scope) {
                    prefix.push_str(&set);
                    prefix.push_str("; ");
                }
            }
            // #540 belt and braces: reassert the locked variables right
            // after binding any default above, so a locked name can't stick
            // overridden even if a future default-bind site here forgets
            // `may_bind_default` — the last `SET` for a name wins.
            append_locked_vars(&mut prefix, scope);
            continue;
        }
        let section = panel.roles.iter().any(|(_, r)| *r == Role::Section);
        let step_marker = panel.step.as_ref().is_some_and(|s| s.marker);
        // `::GUIDE_STEP` (#516, decision 8): bind this step's 1-based
        // number (among the ones shown so far) before its own (and every
        // later) panel's query — this page has no persistent session, so
        // the binding is a `SET VARIABLE` appended to the shared `prefix`
        // every panel's query is built from, the same way an input's
        // default already is above.
        if step_marker {
            let s = panel.step.as_ref().unwrap();
            let n = step_n.entry(s.group).or_insert(0);
            *n += 1;
            if may_bind_default(&s.var, resolved, scope) {
                prefix.push_str(&format!("SET VARIABLE {} = {n}; ", s.var));
            }
            // #540 belt and braces: reassert the locked variables right
            // after, so a locked name can't stick overridden.
            append_locked_vars(&mut prefix, scope);
        }
        // ::GROUP / ::ENDGROUP (#405): no box here (one figure per panel),
        // but a ::GROUP's own ::KPI_SIZE still cascades onto its tiles' own
        // cards.
        if panel.roles.iter().any(|(_, r)| *r == Role::GroupStart) {
            group_compact = if panel.roles.iter().any(|(_, r)| *r == Role::KpiSize) {
                let q = format!(
                    "{prefix}SELECT * FROM (\n{}\n) LIMIT {}",
                    panel.sql.trim().trim_end_matches(';'),
                    st.max_rows
                );
                let rows: Rows = page_rows(st, &q)?;
                kpi_size_json(&panel.roles, &rows).unwrap_or(false)
            } else {
                false
            };
            continue;
        }
        if panel.roles.iter().any(|(_, r)| *r == Role::GroupEnd) {
            group_compact = false;
            continue;
        }
        // `::SUBMIT` (#551): the script-free page has no JS to ever POST
        // `/api/submit`, so this renders a disabled button naming why
        // instead of nothing — the plan §6 "never a silent no-op" rule
        // `web/app.js`'s `mkSubmit` already follows in the browser. Gated
        // (its own `-- @requires` unmet): left out entirely, like an
        // `::INPUT` above — not shown-but-disabled.
        // #684: a ::SAVE likewise (slice 1: no write-back on any server yet).
        let save = panel.roles.iter().any(|(_, r)| *r == Role::Save);
        if save || panel.roles.iter().any(|(_, r)| *r == Role::Submit) {
            if gated {
                continue;
            }
            let q = format!(
                "{prefix}SELECT * FROM (\n{}\n) LIMIT 1",
                panel.sql.trim().trim_end_matches(';'),
            );
            let idx = panel
                .roles
                .iter()
                .find(|(_, r)| *r == Role::Submit || *r == Role::Save)
                .map(|(j, _)| *j);
            let label = page_rows(st, &q)
                .ok()
                .and_then(|rows| {
                    let j = idx?;
                    rows.first()?
                        .get(&format!("c{j}"))?
                        .as_str()
                        .map(str::to_string)
                })
                .unwrap_or_else(|| if save { "Save" } else { "Submit" }.to_string());
            let reason = if save {
                "Saving is not enabled here"
            } else {
                "Submitting is not enabled here"
            };
            let html = sv::submit_disabled_html(&label, i, reason);
            queried.push((panel, Rows::new(), false, false, Some(html)));
            continue;
        }
        // A ::FIGURE (#661) is queried like a panel (its row holds the
        // figure's options); phase 2 draws its cells — the statements after
        // it carrying `figure` — as one picture.
        let figure = panel.roles.iter().any(|(_, r)| *r == Role::FigureStart);
        // Skip interactive/layout-only directives (params drive re-render instead).
        if chrome_role.is_none()
            && !section
            && !step_marker
            && !figure
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
                queried.push((panel, Rows::new(), true, false, None));
            }
            continue;
        }
        // `::GUIDE_STEP` (#516): like `::SHOW_IF` above, a failing marker
        // query (its label or its `::GATE` reads a viewer-supplied
        // variable through an expression that can error) must fail closed
        // — the gate stays closed, the label falls back — without failing
        // the whole page, and without the raw error reaching the log (the
        // #532 lesson for `::SHOW_IF`, the same risk here).
        if step_marker {
            let q = format!(
                "{prefix}SELECT * FROM (\n{}\n) LIMIT {}",
                panel.sql.trim().trim_end_matches(';'),
                st.max_rows + 1
            );
            let rows: Rows = match page_rows(st, &q) {
                Ok(r) => r,
                Err(_) => {
                    eprintln!(
                        "[anofox-serve] dashboard '{}' ::GUIDE_STEP marker (stmt {}) failed — \
                         treated as closed",
                        dash.id,
                        i + 1
                    );
                    Rows::new()
                }
            };
            let compact = kpi_size_json(&panel.roles, &rows).unwrap_or(group_compact);
            queried.push((panel, rows, false, compact, None));
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
        // #682: an editable ::DIAGRAM binds its graph variable (its rows'
        // own graph — a viewer's edits never reach this script-free page)
        // for the panels after it, like an input's default.
        let cols = sql::columns_from_rows(&rows, &panel.roles);
        if let Some(set) =
            anofox_visualization::diagram_edit::default_set(&panel.roles, &panel.names, &cols)
        {
            let name = anofox_visualization::diagram_edit::var_name(&panel.roles, &panel.names);
            if may_bind_default(&name, resolved, scope) {
                prefix.push_str(&set);
                prefix.push_str("; ");
                append_locked_vars(&mut prefix, scope);
            }
        }
        let compact = kpi_size_json(&panel.roles, &rows).unwrap_or(group_compact);
        queried.push((panel, rows, false, compact, None));
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
        // #315/sec-review-342 L2: no script on this page ever reads
        // `data-warnings`, and it's otherwise the one place a successful
        // render can still carry diagnostic text drawn from the panel's
        // own rows. Never attaching it (rather than stripping it back out
        // afterwards) covers every SVG a render produces — a title bar's
        // chart, each ::TRACK, each contract row — not just the first one.
        record_warnings: false,
        asset_root: st.assets_root.clone(),
        ..RenderOptions::default()
    };

    // Phase 2: render.
    let mut panels = String::new();
    // `::GUIDE_STEP` (#516): `queried` only ever holds what phase 1 kept
    // shown, so its total marker count per group ("of N") is known now.
    let mut static_steps = {
        let shown: Vec<&sql::Panel> = queried.iter().map(|(p, ..)| p).collect();
        sql::steps::StaticSteps::new(&shown)
    };
    let mut queried = queried.into_iter().peekable();
    while let Some((panel, rows, na, compact, static_html)) = queried.next() {
        if let Some(html) = static_html {
            panels.push_str(&html);
            continue;
        }
        let figure = panel.roles.iter().any(|(_, r)| *r == Role::FigureStart);
        if na {
            panels.push_str(NOT_AVAILABLE_HTML);
            // its strips (a ::FIGURE: its cells) go with it
            while queried
                .next_if(|(q, _, _, _, _)| {
                    track::is_track(&q.roles) || (figure && q.figure.is_some())
                })
                .is_some()
            {}
            continue;
        }
        // A ::FIGURE (#661): its cells (the entries right after it) composed
        // into one picture; a cell left out ("Not available") is an empty
        // cell.
        if figure {
            let mut cells = Vec::new();
            while let Some((q, r, na, _, _)) = queried.next_if(|(q, ..)| q.figure.is_some()) {
                cells.push(anofox_visualization::figure::Cell {
                    name: q.name.clone(),
                    zoom: None,
                    cols: if na {
                        Vec::new()
                    } else {
                        sql::columns_from_rows(&r, &q.roles)
                    },
                    // #694: a non-chart cell is drawn from the whole result
                    raw: (!na).then(|| anofox_visualization::figure::CellRows {
                        rows: sql::result_columns(&r),
                        roles: q.roles.clone(),
                        names: q.names.clone(),
                    }),
                });
            }
            let svg = anofox_visualization::figure::render_with_title(
                &sql::columns_from_rows(&rows, &panel.roles),
                &cells,
                920,
                460,
                &opts,
            )
            .unwrap_or_else(|e| {
                eprintln!(
                    "[anofox-serve] dashboard '{}' figure render failed: {}",
                    dash.id,
                    access::diag(&e.to_string())
                );
                PANEL_RENDER_ERROR_HTML.to_string()
            });
            panels.push_str(&format!("<figure class=\"panel dp-figure\">{svg}</figure>"));
            continue;
        }
        if let Some(cp) = &panel.contract {
            for (_, r) in sql::contract_call::render_rows_opts(cp, &rows, 460, 300, &opts) {
                let svg = r.unwrap_or_else(|e| {
                    eprintln!(
                        "[anofox-serve] dashboard '{}' panel render failed: {}",
                        dash.id,
                        access::diag(&e.to_string())
                    );
                    PANEL_RENDER_ERROR_HTML.to_string()
                });
                panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
            }
            continue;
        }
        let section = panel.roles.iter().any(|(_, r)| *r == Role::Section);
        let step_n_of = panel.step.as_ref().filter(|s| s.marker).and_then(|_| {
            static_steps.step(&panel).into_iter().find_map(|e| match e {
                sql::steps::StaticEvent::Heading { n, of, .. } => Some((n, of)),
                _ => None,
            })
        });
        if section
            || (panel.roles.len() == 1 && matches!(panel.roles[0].1, Role::Label))
            || step_n_of.is_some()
        {
            let text = match step_n_of {
                Some((n, of)) => {
                    let idx = panel
                        .roles
                        .iter()
                        .find(|(_, r)| *r == Role::GuideStep)
                        .map(|(i, _)| *i);
                    let label = idx
                        .and_then(|i| rows.first().and_then(|r| r.get(&format!("c{i}"))))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if label.is_empty() {
                        format!("Step {n} of {of}")
                    } else {
                        format!("Step {n} of {of} · {label}")
                    }
                }
                None => rows
                    .first()
                    .and_then(|r| r.values().next())
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
            panels.push_str(&format!("<h2 class=\"section\">{}</h2>", esc(&text)));
            continue;
        }
        let mut cols = anofox_visualization::name_columns(
            sql::columns_from_rows(&rows, &panel.roles),
            &panel.roles,
            &panel.names,
        );
        // A ::GROUP's ::KPI_SIZE cascading onto this tile (#405): a synthetic
        // column, since this isn't a tile's own cast — kpi::Kpi::from_columns
        // reads the first ::KPI_SIZE column it finds, own one first.
        if compact {
            cols.push(anofox_visualization::Column::new(
                "_kpi_size",
                Role::KpiSize,
                vec![anofox_visualization::Value::Str("compact".into())],
            ));
        }
        // ::TRACKs right after a chart sit under it, in its figure.
        let mut tracks = Vec::new();
        while let Some((q, r, _, _, _)) =
            queried.next_if(|(q, _, _, _, _)| track::is_track(&q.roles))
        {
            tracks.push((sql::columns_from_rows(&r, &q.roles), 0));
        }
        // #692: 'caption'::MARKDOWN_STYLE texts right after a chart go under
        // it, in its figure.
        let mut captions = String::new();
        while let Some(c) = queried.peek().and_then(|(q, r, na, _, _)| {
            (!na).then(|| {
                anofox_visualization::dashboard::caption_html(
                    &rows,
                    &panel.roles,
                    r,
                    &q.roles,
                    &opts,
                )
            })?
        }) {
            queried.next();
            captions.push_str(&c);
        }
        // Its ::TITLE bar, as on a dashboard card (#232).
        let svg = anofox_visualization::host::catch_panic(|| {
            render_with_title(&cols, &tracks, 460, 300, &opts).map_err(|e| e.to_string())
        })
        .unwrap_or_else(|e| {
            eprintln!(
                "[anofox-serve] dashboard '{}' panel render failed: {}",
                dash.id,
                access::diag(&e)
            );
            PANEL_RENDER_ERROR_HTML.to_string()
        });
        // #536: a ::MARKDOWN panel's chrome — same resolution as the
        // browser's resolveMdChrome(), drawn through the same CSS classes
        // (web/theme-base.css, embedded in this page by `page::render`).
        let text_of = |role: Role| -> Option<String> {
            let (i, _) = panel.roles.iter().find(|(_, r)| *r == role)?;
            rows.first()?
                .get(&format!("c{i}"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let (class, icon_attrs) = if panel.roles.iter().any(|(_, r)| matches!(r, Role::Markdown)) {
            let callout = panel
                .roles
                .iter()
                .any(|(_, r)| matches!(r, Role::Callout))
                .then(|| text_of(Role::Callout).unwrap_or_default());
            roles::markdown_figure_attrs(
                text_of(Role::MarkdownStyle).as_deref(),
                callout.as_deref(),
                text_of(Role::MarkdownTone).as_deref(),
                dash.meta.markdown_style.as_deref(),
            )
        } else {
            ("panel".to_string(), String::new())
        };
        panels.push_str(&format!(
            "<figure class=\"{class}\"{icon_attrs}>{svg}{captions}</figure>"
        ));
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
        fit: page::static_fit(&sql::plan(&dash.script)),
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
    anofox_visualization::host::duckdb_cli::prepare("duckdb", &mut cmd);
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

    /// #539: an `@param`'s label gets a trailing ":" — unless it already
    /// ends with its own terminal punctuation once trailing whitespace is
    /// trimmed. Mirrors web/app.js's `captionColon` (tests/web/caption_colon_test.mjs).
    #[test]
    fn caption_colon_skips_the_colon_after_terminal_punctuation() {
        assert_eq!(caption_colon("region"), "region:");
        assert_eq!(
            caption_colon("Which check sees concept drift?"),
            "Which check sees concept drift?"
        );
        assert_eq!(caption_colon("Great job!"), "Great job!");
        assert_eq!(caption_colon("Pick one:"), "Pick one:");
        assert_eq!(caption_colon("Loading…"), "Loading…");
        assert_eq!(caption_colon("Done."), "Done.");
        assert_eq!(
            caption_colon("Which check sees concept drift?  "),
            "Which check sees concept drift?"
        );
        assert_eq!(caption_colon("region   "), "region:");
        assert_eq!(caption_colon(""), "");
        assert_eq!(caption_colon("   "), "");
    }

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

    /// review-537 + #540/#541: a `::GUIDE_STEP`'s own step variable goes
    /// through the same `may_bind_default` / `append_locked_vars` pair an
    /// `::INPUT` default does — a reserved name or one an embed link locks
    /// never gets the step's own `SET VARIABLE`, and a locked value is
    /// still the *last* `SET` for that name in the prefix even when the
    /// step happens to share its alias (exactly the sequence the
    /// step-marker branch of `render_dashboard_page` runs).
    #[test]
    fn step_variable_binding_respects_locks_and_reserved_names() {
        let resolved: Vec<(String, String)> = Vec::new();
        let locked = Scope {
            dashboards: None,
            vars: vec![("region".into(), "'EU'".into())],
        };

        // A step aliased the same as a locked variable: its own bind is
        // skipped, and the lock is reasserted regardless.
        let mut prefix = String::new();
        if may_bind_default("region", &resolved, Some(&locked)) {
            prefix.push_str("SET VARIABLE region = 2; ");
        }
        append_locked_vars(&mut prefix, Some(&locked));
        assert!(!prefix.contains("SET VARIABLE region = 2"), "{prefix}");
        assert!(prefix.contains("SET VARIABLE region = 'EU'"), "{prefix}");

        // A step aliased a reserved name: same skip, no lock to reassert.
        let mut reserved = String::new();
        if may_bind_default("__claim_tenant", &resolved, None) {
            reserved.push_str("SET VARIABLE __claim_tenant = 2; ");
        }
        append_locked_vars(&mut reserved, None);
        assert!(
            reserved.is_empty(),
            "a reserved name binds nothing: {reserved}"
        );

        // An ordinary step name binds normally, lock or not.
        let mut ordinary = String::new();
        if may_bind_default("lesson", &resolved, Some(&locked)) {
            ordinary.push_str("SET VARIABLE lesson = 2; ");
        }
        append_locked_vars(&mut ordinary, Some(&locked));
        assert!(ordinary.contains("SET VARIABLE lesson = 2"), "{ordinary}");
    }

    #[test]
    fn authoring_session_keeps_state_and_restarts() {
        if Command::new("duckdb").arg("-version").output().is_err() {
            eprintln!("skipped authoring_session_keeps_state_and_restarts: no `duckdb` on PATH");
            return;
        }
        let mut s = AuthoringSession::new(":memory:".into(), Duration::from_secs(120));
        s.run("SET VARIABLE x = 42").unwrap();
        let out = s.run("SELECT getvariable('x') AS v").unwrap();
        assert!(out.contains("\"v\":42"), "{out}");
        s.live.as_mut().unwrap().child.kill().unwrap();
        let e = s.run("SELECT 1 AS a").unwrap_err();
        assert!(e.contains("session ended"), "{e}");
        let out = s.run("SELECT getvariable('x') AS v").unwrap();
        assert!(out.contains("\"v\":null"), "{out}");
    }

    /// #301: the static-render `load_dashboards` also orders by `-- @order`
    /// ascending, not by file name.
    #[test]
    fn load_dashboards_follows_order_header_not_file_name() {
        let dir = std::env::temp_dir().join(format!("anofox-order-test-{}", sv::random_token()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("zzz_first.sql"), "-- @order 1\nSELECT 1;").unwrap();
        std::fs::write(dir.join("aaa_second.sql"), "-- @order 2\nSELECT 1;").unwrap();
        std::fs::write(dir.join("mmm_third.sql"), "-- @order 3\nSELECT 1;").unwrap();
        std::fs::write(dir.join("zzz_unordered.sql"), "SELECT 1;").unwrap();
        let dashboards = load_dashboards(&dir);
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
            key_scopes: Default::default(),
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

    /// #540: an embed link's locked variable always wins over a dashboard's
    /// own `::INPUT` default, a whitelisted `-- @param`, or a reserved name
    /// — whatever tries to bind a default for that same name afterward.
    #[test]
    fn may_bind_default_respects_locks_params_and_reserved_names() {
        let resolved = vec![("already_set".to_string(), "'x'".to_string())];
        let locked = Scope {
            dashboards: None,
            vars: vec![("region".into(), "'US'".into())],
        };
        assert!(!may_bind_default("region", &resolved, Some(&locked)));
        // DuckDB variable names are case-insensitive, so is the lock.
        assert!(!may_bind_default("REGION", &resolved, Some(&locked)));
        assert!(!may_bind_default("already_set", &resolved, Some(&locked)));
        assert!(!may_bind_default("already_set", &resolved, None));
        assert!(!may_bind_default("anofox_data", &resolved, None));
        assert!(may_bind_default("free_name", &resolved, Some(&locked)));
        assert!(may_bind_default("free_name", &resolved, None));
    }

    /// #540 belt and braces: whatever runs between two calls of
    /// [`append_locked_vars`] — a default bind that should have been
    /// skipped, or one a future code path forgets to check against
    /// [`may_bind_default`] at all — the *last* `SET VARIABLE` for a locked
    /// name in the prefix is still the locked value, which is the one
    /// DuckDB uses for every panel query run against that prefix from then
    /// on.
    #[test]
    fn append_locked_vars_reasserts_the_lock_after_anything_between() {
        let locked = Scope {
            dashboards: None,
            vars: vec![("region".into(), "'US'".into())],
        };
        let mut prefix = String::new();
        append_locked_vars(&mut prefix, Some(&locked));
        // Simulate a bind that `may_bind_default` should have refused, or a
        // future site that never calls it.
        prefix.push_str("SET VARIABLE region = 'EU'; ");
        append_locked_vars(&mut prefix, Some(&locked));
        let last = prefix
            .rmatch_indices("SET VARIABLE region = ")
            .next()
            .map(|(i, _)| &prefix[i..])
            .expect("a SET VARIABLE for region");
        assert!(
            last.starts_with("SET VARIABLE region = 'US';"),
            "the locked value must be the last SET for 'region': {prefix}"
        );
    }
}

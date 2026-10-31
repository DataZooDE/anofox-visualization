//! Scheduled reports (#768 R1, docs/plans/plan-tasks-reports.md §2, §4):
//! `options.notify` (the channels and whether this replica sends) and
//! `options.reports`, the readable schedules, the e-mail message, the SMTP
//! client and the signed webhook. Rendering and the runner live in the
//! extension (`duckext/src/serve.rs`), which renders through the export
//! path (#764) under each report's fixed principal (D4).
//!
//! ```json
//! {"notify": {"run": true,
//!             "smtp": {"host": "smtp.example.com", "port": 587, "user_env": "SMTP_USER",
//!                      "password_env": "SMTP_PASSWORD", "from": "reports@example.com"},
//!             "webhooks": {"ops": {"url_env": "OPS_HOOK_URL", "secret_env": "OPS_HOOK_SECRET"}},
//!             "link": "https://dash.example.com", "max_per_hour": 60},
//!  "reports": [{"name": "weekly-sales", "dashboard": "sales", "tab": "Overview",
//!               "vars": {"region": "EU"}, "formats": ["png", "pdf", "csv:totals"],
//!               "schedule": "every monday 07:00 Europe/Berlin",
//!               "as": {"user": "report:sales-eu", "groups": ["sales"], "claims": {"region": "EU"}},
//!               "to": [{"email": "sales-eu@example.com"}, {"webhook": "ops"}]}]}
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value};

use crate::host::access::{
    clean_claim, valid_claim_name, Claims, MAX_ATTRS, MAX_GROUPS, REPORT_USER_PREFIX,
};

/// Most recipients of one report.
pub const MAX_RECIPIENTS: usize = 50;
/// Most bytes of one report's files together.
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
/// One connection's (SMTP or webhook) time limit.
const NET_TIMEOUT: Duration = Duration::from_secs(30);

/// `options.notify`.
#[derive(Debug, Clone)]
pub struct NotifyOptions {
    /// Send reports from this replica (one per deployment, like `tasks.run`).
    pub run: bool,
    pub smtp: Option<Smtp>,
    pub webhooks: BTreeMap<String, Webhook>,
    /// The page's public base URL, for a link in the message.
    pub link: Option<String>,
    /// Messages per hour, for all reports together (default 60).
    pub max_per_hour: usize,
    /// Where each run is recorded (a file or `"stderr"`; default stderr).
    pub log: Option<String>,
    /// The alerts' state (a JSON file), so a restart doesn't send again.
    pub state: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// `STARTTLS` (required; a server without it is an error). Default.
    StartTls,
    /// TLS from the first byte (port 465).
    Implicit,
    /// None: only to a loopback address (a local relay or a test sink).
    None,
}

#[derive(Clone)]
pub struct Smtp {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    /// `(user, password)` from the environment.
    pub login: Option<(String, String)>,
    /// The envelope sender and the `From` address.
    pub from: String,
}

impl std::fmt::Debug for Smtp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Smtp")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("login", &self.login.as_ref().map(|(u, _)| u))
            .field("from", &self.from)
            .finish()
    }
}

#[derive(Clone)]
pub struct Webhook {
    pub url: String,
    pub secret: Vec<u8>,
}

impl std::fmt::Debug for Webhook {
    // The URL can carry a token (Teams, n8n): never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Webhook { .. }")
    }
}

/// One report.
#[derive(Debug, Clone)]
pub struct Report {
    pub name: String,
    pub dashboard: String,
    pub tab: Option<String>,
    /// The variables, like a page URL's `?v.<name>=` (strings).
    pub vars: Map<String, Value>,
    pub formats: Vec<Format>,
    /// The schedule as SQL (an INTERVAL, a TIMESTAMP or `'init'`).
    pub schedule: String,
    /// The fixed principal every panel runs as (D4).
    pub principal: Claims,
    pub to: Vec<Recipient>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    /// The dashboard (or its tab) as a PNG, inline in an e-mail.
    Png,
    Pdf,
    /// A `-- @name`d panel's rows.
    Csv(String),
    Json(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipient {
    Email(String),
    Webhook(String),
}

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

fn env_secret(env: Env, o: &Map<String, Value>, key: &str, what: &str) -> Result<String, String> {
    let var = o
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{what}.{key} must name an environment variable"))?;
    env(var)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{what}.{key}: the environment variable '{var}' is not set"))
}

fn check_keys(o: &Map<String, Value>, what: &str, allowed: &[&str]) -> Result<(), String> {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!(
                "{what}: unknown key '{k}' ({})",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

/// The host part of `authority` (`host[:port]`, `[v6]:port`).
fn authority_host(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(r) => r.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A webhook URL: `https://…`, or `http://` to a loopback address only; no
/// credentials in it, no fragment.
pub fn check_webhook_url(url: &str) -> Result<(), String> {
    let bad =
        || "a webhook URL must be https://host/… (http:// only to a loopback address)".to_string();
    let (rest, https) = if let Some(r) = url.strip_prefix("https://") {
        (r, true)
    } else if let Some(r) = url.strip_prefix("http://") {
        (r, false)
    } else {
        return Err(bad());
    };
    if url.len() > 2048 || !url.bytes().all(|b| b.is_ascii_graphic()) || url.contains(['#', '\\']) {
        return Err(bad());
    }
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(bad());
    }
    if !https && !is_loopback(authority_host(authority)) {
        return Err(bad());
    }
    Ok(())
}

/// A plain address (`local@domain`): what goes into `RCPT TO:<…>` and a
/// header. No display names, no spaces, no angle brackets or line breaks.
pub fn valid_email(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else {
        return false;
    };
    a.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-/=?^_`{|}~.".contains(&b))
}

/// A report or webhook name: `[a-z0-9_-]`, 1..=64.
fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// `options.notify`, its secrets read from the environment (`env`).
pub fn parse_notify(val: &Value, env: Env) -> Result<NotifyOptions, String> {
    let o = val
        .as_object()
        .ok_or("options.notify must be an object like {\"run\": true, \"smtp\": {…}}")?;
    check_keys(
        o,
        "options.notify",
        &[
            "run",
            "smtp",
            "webhooks",
            "link",
            "max_per_hour",
            "log",
            "state",
        ],
    )?;
    let mut n = NotifyOptions {
        run: false,
        smtp: None,
        webhooks: BTreeMap::new(),
        link: None,
        max_per_hour: 60,
        log: None,
        state: None,
    };
    if let Some(v) = o.get("run") {
        n.run = v
            .as_bool()
            .ok_or("options.notify.run must be true or false")?;
    }
    if let Some(v) = o.get("smtp") {
        n.smtp = Some(parse_smtp(v, env)?);
    }
    if let Some(v) = o.get("webhooks") {
        let m = v
            .as_object()
            .ok_or("options.notify.webhooks must be an object of named webhooks")?;
        for (name, w) in m {
            let what = format!("options.notify.webhooks.{name}");
            if !valid_name(name) {
                return Err(format!("{what}: a name is a-z, 0-9, - and _"));
            }
            let w = w.as_object().ok_or(format!("{what} must be an object"))?;
            check_keys(w, &what, &["url_env", "secret_env"])?;
            let url = env_secret(env, w, "url_env", &what)?;
            check_webhook_url(&url).map_err(|e| format!("{what}: {e}"))?;
            let secret = env_secret(env, w, "secret_env", &what)?;
            if secret.len() < 16 {
                return Err(format!(
                    "{what}.secret_env: the secret must be at least 16 bytes"
                ));
            }
            n.webhooks.insert(
                name.clone(),
                Webhook {
                    url,
                    secret: secret.into_bytes(),
                },
            );
        }
    }
    if let Some(v) = o.get("link") {
        let l = v
            .as_str()
            .filter(|l| {
                (l.starts_with("https://") || l.starts_with("http://"))
                    && l.len() <= 512
                    && l.bytes().all(|b| b.is_ascii_graphic())
                    && !l.contains(['"', '<', '>', '\'', '?', '#'])
            })
            .ok_or("options.notify.link must be the server's public URL, like https://dash.example.com")?;
        n.link = Some(l.trim_end_matches('/').to_string());
    }
    if let Some(v) = o.get("max_per_hour") {
        n.max_per_hour =
            v.as_u64()
                .filter(|n| (1..=10_000).contains(n))
                .ok_or("options.notify.max_per_hour must be 1..=10000")? as usize;
    }
    if let Some(v) = o.get("log") {
        n.log = Some(
            v.as_str()
                .ok_or("options.notify.log must be \"stderr\" or a file path")?
                .to_string(),
        );
    }
    if let Some(v) = o.get("state") {
        n.state = Some(
            v.as_str()
                .filter(|p| !p.is_empty())
                .ok_or("options.notify.state must be a file path")?
                .into(),
        );
    }
    Ok(n)
}

fn parse_smtp(v: &Value, env: Env) -> Result<Smtp, String> {
    let what = "options.notify.smtp";
    let o = v.as_object().ok_or(format!("{what} must be an object"))?;
    check_keys(
        o,
        what,
        &["host", "port", "tls", "user_env", "password_env", "from"],
    )?;
    let host = o
        .get("host")
        .and_then(Value::as_str)
        .filter(|h| {
            !h.is_empty()
                && h.len() <= 253
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b))
        })
        .ok_or(format!("{what}.host must be a host name"))?
        .to_string();
    let tls = match o.get("tls").map(|t| t.as_str()) {
        None | Some(Some("starttls")) => SmtpTls::StartTls,
        Some(Some("implicit")) => SmtpTls::Implicit,
        Some(Some("none")) => SmtpTls::None,
        _ => {
            return Err(format!(
                "{what}.tls must be \"starttls\", \"implicit\" or \"none\""
            ))
        }
    };
    if tls == SmtpTls::None && !is_loopback(&host) {
        return Err(format!(
            "{what}.tls \"none\" is only for a relay on a loopback address (127.0.0.1, ::1, localhost)"
        ));
    }
    let port = match o.get("port") {
        None => match tls {
            SmtpTls::Implicit => 465,
            _ => 587,
        },
        Some(p) => p
            .as_u64()
            .filter(|p| (1..=65_535).contains(p))
            .ok_or(format!("{what}.port must be a port number"))? as u16,
    };
    let login = match (o.contains_key("user_env"), o.contains_key("password_env")) {
        (false, false) => None,
        (true, true) => Some((
            env_secret(env, o, "user_env", what)?,
            env_secret(env, o, "password_env", what)?,
        )),
        _ => return Err(format!("{what}: user_env and password_env go together")),
    };
    let from = o
        .get("from")
        .and_then(Value::as_str)
        .filter(|f| valid_email(f))
        .ok_or(format!(
            "{what}.from must be a plain address, like reports@example.com"
        ))?
        .to_string();
    Ok(Smtp {
        host,
        port,
        tls,
        login,
        from,
    })
}

/// `options.reports` (after `options.notify`: a recipient names a channel
/// that must be configured).
pub fn parse_reports(val: &Value, notify: Option<&NotifyOptions>) -> Result<Vec<Report>, String> {
    let list = val
        .as_array()
        .ok_or("options.reports must be a list of reports")?;
    let mut out: Vec<Report> = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let o = r
            .as_object()
            .ok_or(format!("options.reports[{i}] must be an object"))?;
        let name = o
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| valid_name(n))
            .ok_or(format!(
                "options.reports[{i}].name is required (a-z, 0-9, - and _)"
            ))?
            .to_string();
        let what = format!("report '{name}'");
        if out.iter().any(|x| x.name == name) {
            return Err(format!("{what}: a second report of that name"));
        }
        check_keys(
            o,
            &what,
            &[
                "name",
                "dashboard",
                "tab",
                "vars",
                "formats",
                "schedule",
                "as",
                "to",
            ],
        )?;
        let dashboard = o
            .get("dashboard")
            .and_then(Value::as_str)
            .filter(|d| crate::host::serving::valid_dashboard_id(d))
            .ok_or(format!("{what}: dashboard is required (its id)"))?
            .to_string();
        let tab = match o.get("tab") {
            None => None,
            Some(t) => Some(
                t.as_str()
                    .filter(|t| !t.trim().is_empty())
                    .ok_or(format!("{what}: tab must be a tab's label"))?
                    .to_string(),
            ),
        };
        let vars = match o.get("vars") {
            None => Map::new(),
            Some(v) => {
                let m = v
                    .as_object()
                    .ok_or(format!("{what}: vars must be an object of strings"))?;
                if !m.values().all(Value::is_string) {
                    return Err(format!("{what}: vars must be an object of strings"));
                }
                m.clone()
            }
        };
        let mut formats = Vec::new();
        for f in o.get("formats").and_then(Value::as_array).ok_or(format!(
            "{what}: formats is required, e.g. [\"png\", \"pdf\", \"csv:<panel>\"]"
        ))? {
            let f = f.as_str().unwrap_or("");
            let fmt = match f.split_once(':') {
                None if f == "png" => Format::Png,
                None if f == "pdf" => Format::Pdf,
                Some(("csv", p)) if valid_name_panel(p) => Format::Csv(p.to_string()),
                Some(("json", p)) if valid_name_panel(p) => Format::Json(p.to_string()),
                _ => {
                    return Err(format!(
                        "{what}: format '{f}' — one of png, pdf, csv:<panel>, json:<panel>"
                    ))
                }
            };
            if !formats.contains(&fmt) {
                formats.push(fmt);
            }
        }
        if formats.is_empty() {
            return Err(format!("{what}: formats is empty"));
        }
        let schedule = schedule_sql(o.get("schedule").and_then(Value::as_str).ok_or(format!(
            "{what}: schedule is required, e.g. \"every day 07:00\""
        ))?)
        .map_err(|e| format!("{what}: {e}"))?;
        let principal = parse_principal(o.get("as"), &what)?;
        let to = parse_recipients(o.get("to"), notify, &what)?;
        out.push(Report {
            name,
            dashboard,
            tab,
            vars,
            formats,
            schedule,
            principal,
            to,
        });
    }
    Ok(out)
}

/// A list of recipients (`to`): `{"email": "…"}` or `{"webhook": "<name>"}`,
/// each naming a channel `options.notify` configures; at most
/// [`MAX_RECIPIENTS`], duplicates dropped.
pub fn parse_recipients(
    to: Option<&Value>,
    notify: Option<&NotifyOptions>,
    what: &str,
) -> Result<Vec<Recipient>, String> {
    let to = to
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())
        .ok_or(format!("{what}: to is required (a list of recipients)"))?;
    if to.len() > MAX_RECIPIENTS {
        return Err(format!("{what}: more than {MAX_RECIPIENTS} recipients"));
    }
    let shape =
        || format!("{what}: a recipient is {{\"email\": \"…\"}} or {{\"webhook\": \"<name>\"}}");
    let mut rcpts = Vec::new();
    for t in to {
        let t = t.as_object().filter(|t| t.len() == 1).ok_or_else(shape)?;
        let r = match t.iter().next() {
            Some((k, Value::String(a))) if k == "email" => {
                if !valid_email(a) {
                    return Err(format!("{what}: '{a}' is not a plain e-mail address"));
                }
                if notify.is_none_or(|n| n.smtp.is_none()) {
                    return Err(format!(
                        "{what}: an e-mail recipient needs options.notify.smtp"
                    ));
                }
                Recipient::Email(a.to_ascii_lowercase())
            }
            Some((k, Value::String(w))) if k == "webhook" => {
                if notify.is_none_or(|n| !n.webhooks.contains_key(w)) {
                    return Err(format!(
                        "{what}: no webhook '{w}' in options.notify.webhooks"
                    ));
                }
                Recipient::Webhook(w.clone())
            }
            _ => return Err(shape()),
        };
        if !rcpts.contains(&r) {
            rcpts.push(r);
        }
    }
    Ok(rcpts)
}

/// An alert (#768 R2, plan §3): it fires when its panel returns rows.
#[derive(Debug, Clone)]
pub struct Alert {
    pub name: String,
    pub dashboard: String,
    /// The `-- @name`d panel whose rows are the alert.
    pub panel: String,
    pub vars: Map<String, Value>,
    /// When to check (schedule SQL, like a report's).
    pub schedule: String,
    pub principal: Claims,
    /// The rows, attached: `Format::Csv` / `Format::Json` of `panel`.
    pub attach: Vec<Format>,
    pub to: Vec<Recipient>,
    /// Send again while it keeps firing, this long after the last message.
    pub repeat_after: Option<Duration>,
}

/// `30 minutes`, `24 hours`, `2 days` (at least a minute).
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let bad = || format!("'{s}': a duration like \"30 minutes\", \"24 hours\" or \"2 days\"");
    let mut w = s.split_whitespace();
    let (Some(n), Some(unit), None) = (w.next(), w.next(), w.next()) else {
        return Err(bad());
    };
    let n: u64 = n.parse().map_err(|_| bad())?;
    let secs = match unit.to_ascii_lowercase().trim_end_matches('s') {
        "minute" => 60,
        "hour" => 3600,
        "day" => 86_400,
        _ => return Err(bad()),
    };
    if n == 0 || n > 366 * 86_400 / secs {
        return Err(bad());
    }
    Ok(Duration::from_secs(n * secs))
}

/// `options.alerts` (after `options.notify`, which must name a `state`
/// file: an alert's state survives a restart).
pub fn parse_alerts(val: &Value, notify: Option<&NotifyOptions>) -> Result<Vec<Alert>, String> {
    let list = val
        .as_array()
        .ok_or("options.alerts must be a list of alerts")?;
    if !list.is_empty() && notify.is_none_or(|n| n.state.is_none()) {
        return Err(
            "options.alerts needs options.notify.state (a file for the alerts' state)".into(),
        );
    }
    let mut out: Vec<Alert> = Vec::new();
    for (i, a) in list.iter().enumerate() {
        let o = a
            .as_object()
            .ok_or(format!("options.alerts[{i}] must be an object"))?;
        let name = o
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| valid_name(n))
            .ok_or(format!(
                "options.alerts[{i}].name is required (a-z, 0-9, - and _)"
            ))?
            .to_string();
        let what = format!("alert '{name}'");
        if out.iter().any(|x| x.name == name) {
            return Err(format!("{what}: a second alert of that name"));
        }
        check_keys(
            o,
            &what,
            &[
                "name",
                "dashboard",
                "panel",
                "vars",
                "check",
                "as",
                "attach",
                "to",
                "repeat_after",
            ],
        )?;
        let dashboard = o
            .get("dashboard")
            .and_then(Value::as_str)
            .filter(|d| crate::host::serving::valid_dashboard_id(d))
            .ok_or(format!("{what}: dashboard is required (its id)"))?
            .to_string();
        let panel = o
            .get("panel")
            .and_then(Value::as_str)
            .filter(|p| valid_name_panel(p))
            .ok_or(format!("{what}: panel is required (a `-- @name`d panel)"))?
            .to_string();
        let vars = match o.get("vars") {
            None => Map::new(),
            Some(v) => v
                .as_object()
                .filter(|m| m.values().all(Value::is_string))
                .ok_or(format!("{what}: vars must be an object of strings"))?
                .clone(),
        };
        let schedule = schedule_sql(o.get("check").and_then(Value::as_str).ok_or(format!(
            "{what}: check is required, e.g. \"every 15 minutes\""
        ))?)
        .map_err(|e| format!("{what}: {e}"))?;
        let principal = parse_principal(o.get("as"), &what)?;
        let mut attach = Vec::new();
        if let Some(v) = o.get("attach") {
            for f in v.as_array().ok_or(format!(
                "{what}: attach must be a list, [\"csv\"] or [\"json\"]"
            ))? {
                let f = match f.as_str() {
                    Some("csv") => Format::Csv(panel.clone()),
                    Some("json") => Format::Json(panel.clone()),
                    _ => return Err(format!("{what}: attach takes \"csv\" and \"json\"")),
                };
                if !attach.contains(&f) {
                    attach.push(f);
                }
            }
        }
        let to = parse_recipients(o.get("to"), notify, &what)?;
        let repeat_after = match o.get("repeat_after") {
            None => None,
            Some(v) => Some(
                parse_duration(v.as_str().unwrap_or(""))
                    .map_err(|e| format!("{what}: repeat_after {e}"))?,
            ),
        };
        out.push(Alert {
            name,
            dashboard,
            panel,
            vars,
            schedule,
            principal,
            attach,
            to,
            repeat_after,
        });
    }
    Ok(out)
}

/// What an alert remembers (D5): firing or not, the hash of the rows it last
/// told about, and when it last sent (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlertState {
    pub firing: bool,
    pub hash: String,
    pub last_sent: u64,
}

/// What a check sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertAction {
    /// It had no rows and now has some.
    Fire,
    /// Still firing, with different rows.
    Changed,
    /// Still firing, the same rows, `repeat_after` since the last message.
    Repeat,
    /// It had rows and now has none.
    Resolved,
}

impl AlertAction {
    pub fn status(self) -> &'static str {
        match self {
            AlertAction::Fire | AlertAction::Repeat => "FIRING",
            AlertAction::Changed => "CHANGED",
            AlertAction::Resolved => "RESOLVED",
        }
    }
}

/// One check: `rows` is the hash of the panel's rows, `None` when it has
/// none. The message to send (if any) and the state once it is sent.
pub fn decide(
    prev: Option<&AlertState>,
    rows: Option<&str>,
    now: u64,
    repeat_after: Option<Duration>,
) -> Option<(AlertAction, AlertState)> {
    let firing = prev.is_some_and(|p| p.firing);
    let sent = |hash: &str| AlertState {
        firing: true,
        hash: hash.to_string(),
        last_sent: now,
    };
    match rows {
        Some(h) if !firing => Some((AlertAction::Fire, sent(h))),
        Some(h) => {
            let p = prev?;
            if p.hash != h {
                Some((AlertAction::Changed, sent(h)))
            } else if repeat_after.is_some_and(|r| now >= p.last_sent + r.as_secs()) {
                Some((AlertAction::Repeat, sent(h)))
            } else {
                None
            }
        }
        None if firing => Some((
            AlertAction::Resolved,
            AlertState {
                firing: false,
                hash: String::new(),
                last_sent: now,
            },
        )),
        None => None,
    }
}

/// Read the alerts' state file (missing: empty).
pub fn load_states(path: &std::path::Path) -> Result<BTreeMap<String, AlertState>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("options.notify.state: {e}")),
    };
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("options.notify.state: not the alerts' state file: {e}"))?;
    let mut out = BTreeMap::new();
    for (k, a) in v
        .get("alerts")
        .and_then(Value::as_object)
        .ok_or("options.notify.state: not the alerts' state file")?
    {
        out.insert(
            k.clone(),
            AlertState {
                firing: a.get("firing").and_then(Value::as_bool).unwrap_or(false),
                hash: a
                    .get("hash")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                last_sent: a.get("last_sent").and_then(Value::as_u64).unwrap_or(0),
            },
        );
    }
    Ok(out)
}

/// Write the alerts' state file (a temp file renamed over it, `0600`).
pub fn save_states(
    path: &std::path::Path,
    states: &BTreeMap<String, AlertState>,
) -> Result<(), String> {
    let mut m = Map::new();
    for (k, s) in states {
        m.insert(
            k.clone(),
            serde_json::json!({"firing": s.firing, "hash": s.hash, "last_sent": s.last_sent}),
        );
    }
    let body = serde_json::to_vec_pretty(&serde_json::json!({"alerts": m})).unwrap_or_default();
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(&tmp)
        .and_then(|mut f| f.write_all(&body).and_then(|_| f.sync_all()))
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| format!("options.notify.state: {e}"))
}

/// A `-- @name` (as the export's URL takes it).
fn valid_name_panel(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 128
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A report's `as`: `{"user": "report:…", "groups"?: […], "claims"?: {…}}`.
fn parse_principal(v: Option<&Value>, what: &str) -> Result<Claims, String> {
    let o = v.and_then(Value::as_object).ok_or(format!(
        "{what}: as is required — the principal it runs as, {{\"user\": \"report:<name>\", \"groups\": […], \"claims\": {{…}}}}"
    ))?;
    check_keys(o, &format!("{what}: as"), &["user", "groups", "claims"])?;
    let user = o
        .get("user")
        .and_then(Value::as_str)
        .and_then(|u| clean_claim(u).ok())
        .filter(|u| {
            u.len() > REPORT_USER_PREFIX.len()
                && u.as_bytes()[..REPORT_USER_PREFIX.len()]
                    .eq_ignore_ascii_case(REPORT_USER_PREFIX.as_bytes())
        })
        .ok_or(format!("{what}: as.user must be report:<name>"))?;
    let mut groups = Vec::new();
    if let Some(g) = o.get("groups") {
        for x in g
            .as_array()
            .filter(|g| g.len() <= MAX_GROUPS)
            .ok_or(format!("{what}: as.groups must be a list"))?
        {
            let x = x
                .as_str()
                .and_then(|s| clean_claim(s).ok())
                .ok_or(format!("{what}: as.groups: a group name"))?;
            if !x.is_empty() {
                groups.push(x);
            }
        }
    }
    groups.sort();
    groups.dedup();
    let mut attrs = Vec::new();
    if let Some(c) = o.get("claims") {
        let c = c
            .as_object()
            .filter(|c| c.len() <= MAX_ATTRS)
            .ok_or(format!("{what}: as.claims must be an object"))?;
        for (name, v) in c {
            if !valid_claim_name(name) {
                return Err(format!("{what}: invalid claim name '{name}'"));
            }
            let v = match v {
                Value::Null => None,
                Value::String(s) => {
                    Some(clean_claim(s).map_err(|_| format!("{what}: claim '{name}': bad value"))?)
                }
                _ => return Err(format!("{what}: claim values must be strings")),
            };
            attrs.push((name.to_ascii_lowercase(), v.filter(|v| !v.is_empty())));
        }
    }
    Ok(Claims {
        user,
        groups,
        attrs,
    })
}

const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// A schedule as the SQL the runner evaluates (D8): `'init'`, a readable
/// form, or SQL (an INTERVAL or a TIMESTAMP expression).
///
/// Readable forms: `every 15 minutes`, `every 2 hours`, `every hour` (on the
/// hour), `every day 06:30`, `every monday 07:00` — the last two in the
/// server's time zone (DuckDB's `TimeZone`) or a named one appended
/// (`every monday 07:00 Europe/Berlin`).
pub fn schedule_sql(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("init") {
        return Ok("'init'".into());
    }
    let lower = s.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("every ") else {
        // SQL: checked by the runner (it must be an INTERVAL or a
        // TIMESTAMP). Operator configuration, one expression.
        if s.contains(';') {
            return Err("a schedule is one expression".into());
        }
        return Ok(s.to_string());
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    let orig: Vec<&str> = s.split_whitespace().skip(1).collect();
    let bad = || {
        format!(
            "schedule '{s}': use every N minutes | every N hours | every hour | every day HH:MM [Zone] | every <weekday> HH:MM [Zone]"
        )
    };
    match words.as_slice() {
        ["hour"] => Ok("date_trunc('hour', now()) + INTERVAL 1 HOUR".into()),
        [n, unit] if matches!(*unit, "minute" | "minutes" | "hour" | "hours") => {
            let n: u32 = n.parse().map_err(|_| bad())?;
            let mins = if unit.starts_with("hour") { n * 60 } else { n };
            if !(1..=7 * 24 * 60).contains(&mins) {
                return Err(bad());
            }
            Ok(format!("INTERVAL {mins} MINUTE"))
        }
        [day, time, zone @ ..] if zone.len() <= 1 => {
            let (h, m) = time
                .split_once(':')
                .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
                .filter(|(h, m)| *h < 24 && *m < 60 && time.len() <= 5)
                .ok_or_else(bad)?;
            let tz = match orig.get(2) {
                None => "current_setting('TimeZone')".to_string(),
                Some(z)
                    if z.len() <= 64
                        && z.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b)) =>
                {
                    format!("'{z}'")
                }
                Some(_) => return Err(bad()),
            };
            let (offset, step) = if *day == "day" {
                ("0".to_string(), "1 DAY")
            } else {
                let w = WEEKDAYS.iter().position(|d| d == day).ok_or_else(bad)? + 1;
                (format!("((({w} - isodow(t)) % 7) + 7) % 7"), "7 DAY")
            };
            Ok(format!(
                "(SELECT timezone({tz}, CASE WHEN c <= t THEN c + INTERVAL {step} ELSE c END) \
                 FROM (SELECT t, date_trunc('day', t) + INTERVAL '{h:02}:{m:02}:00' + to_days({offset}) AS c \
                 FROM (SELECT timezone({tz}, now()) AS t)))"
            ))
        }
        _ => Err(bad()),
    }
}

// ---- the message ---------------------------------------------------------

/// A file of a report.
#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// What a report says, besides its files.
#[derive(Debug, Clone)]
pub struct Message {
    /// `report`, `alert` or `task`.
    pub kind: &'static str,
    /// The report's, alert's or task's name.
    pub name: String,
    pub title: String,
    /// An alert's or task's state: `FIRING`, `CHANGED`, `RESOLVED`,
    /// `FAILED`, `RECOVERED`.
    pub status: Option<&'static str>,
    /// One more line: an alert's row count, a task's error.
    pub detail: Option<String>,
    /// The dashboard's `::ASOF` text, if it has one.
    pub as_of: Option<String>,
    /// A link to the dashboard (`notify.link` + `/d/<id>`).
    pub link: Option<String>,
    pub time: SystemTime,
}

impl Message {
    pub fn subject(&self) -> String {
        match self.status {
            Some(s) => format!("[{s}] {} — {}", self.title, self.name),
            None => format!("{} — {}", self.title, self.name),
        }
    }

    fn lines(&self) -> Vec<String> {
        let mut l = vec![self.title.clone()];
        match (self.status, &self.detail) {
            (Some(s), Some(d)) => l.push(format!("{s}: {d}")),
            (Some(s), None) => l.push(s.to_string()),
            (None, Some(d)) => l.push(d.clone()),
            (None, None) => {}
        }
        if let Some(a) = &self.as_of {
            l.push(a.clone());
        }
        let kind = match self.kind {
            "alert" => "Alert",
            "task" => "Task",
            _ => "Report",
        };
        l.push(format!(
            "{kind} '{}', {}.",
            self.name,
            crate::host::access::rfc3339(self.time)
        ));
        l
    }

    pub fn text(&self) -> String {
        let mut t = self.lines().join("\n");
        if let Some(u) = &self.link {
            t.push_str(&format!("\nOpen the dashboard: {u}"));
        }
        t
    }

    fn html(&self, inline: Option<&str>) -> String {
        let e = crate::host::serving::html_escape;
        let mut l = self.lines().into_iter();
        let mut h = format!(
            "<!doctype html><html><body style=\"font-family:sans-serif\"><h2>{}</h2>",
            e(&l.next().unwrap_or_default())
        );
        for x in l {
            h.push_str(&format!("<p>{}</p>", e(&x)));
        }
        if let Some(u) = &self.link {
            h.push_str(&format!(
                "<p><a href=\"{}\">Open the dashboard</a></p>",
                e(u)
            ));
        }
        if let Some(cid) = inline {
            h.push_str(&format!(
                "<p><img src=\"cid:{cid}\" alt=\"{}\" style=\"max-width:100%\"></p>",
                e(&self.title)
            ));
        }
        h.push_str("</body></html>");
        h
    }
}

/// Anything that isn't a plain printable character becomes a space (no
/// header or line injection from a title).
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A header value: as is when it is printable ASCII, else RFC 2047 words.
fn header_text(s: &str) -> String {
    use base64::Engine as _;
    let s = one_line(s);
    if s.bytes().all(|b| (0x20..0x7f).contains(&b)) && !s.contains("=?") {
        return s;
    }
    // Encoded words of at most 45 input bytes each, split on char bounds.
    let mut out = Vec::new();
    let mut chunk = String::new();
    for c in s.chars() {
        if chunk.len() + c.len_utf8() > 45 {
            out.push(chunk.clone());
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        out.push(chunk);
    }
    out.iter()
        .map(|w| {
            format!(
                "=?UTF-8?B?{}?=",
                base64::engine::general_purpose::STANDARD.encode(w)
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n ")
}

fn b64_lines(bytes: &[u8]) -> String {
    use base64::Engine as _;
    let s = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut out = String::with_capacity(s.len() + s.len() / 76 * 2 + 2);
    for c in s.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(c).unwrap_or(""));
        out.push_str("\r\n");
    }
    out
}

/// RFC 5322 date, UTC (`Sat, 31 Oct 2026 07:00:00 +0000`).
fn mail_date(t: SystemTime) -> String {
    let r = crate::host::access::rfc3339(t); // 2026-10-31T07:00:00Z
    let (y, mo, d) = (
        r[0..4].parse::<i64>().unwrap_or(1970),
        r[5..7].parse::<usize>().unwrap_or(1),
        r[8..10].parse::<i64>().unwrap_or(1),
    );
    let days = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0);
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize];
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][mo.clamp(1, 12) - 1];
    format!("{wd}, {d:02} {mon} {y} {} +0000", &r[11..19])
}

/// A random hex string (for the MIME boundaries and the Message-ID).
fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    let _ = aws_lc_rs::rand::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The e-mail: text and HTML, the PNG inline (`cid:`), the other files
/// attached. Recipients go in the envelope only (Bcc), never a header.
pub fn email(from: &str, msg: &Message, files: &[File]) -> Vec<u8> {
    let b = random_hex(12);
    let (mixed, related, alt) = (
        format!("mixed-{b}"),
        format!("related-{b}"),
        format!("alt-{b}"),
    );
    let inline = files.iter().find(|f| f.content_type == "image/png");
    let domain = from.rsplit_once('@').map_or("localhost", |(_, d)| d);
    let mut m = String::new();
    m.push_str(&format!("From: {from}\r\n"));
    m.push_str("To: undisclosed-recipients:;\r\n");
    m.push_str(&format!("Subject: {}\r\n", header_text(&msg.subject())));
    m.push_str(&format!("Date: {}\r\n", mail_date(msg.time)));
    m.push_str(&format!("Message-ID: <{}@{domain}>\r\n", random_hex(16)));
    m.push_str("Auto-Submitted: auto-generated\r\n");
    m.push_str("MIME-Version: 1.0\r\n");
    m.push_str(&format!(
        "Content-Type: multipart/mixed; boundary=\"{mixed}\"\r\n\r\n"
    ));
    m.push_str(&format!(
        "--{mixed}\r\nContent-Type: multipart/related; boundary=\"{related}\"\r\n\r\n"
    ));
    m.push_str(&format!(
        "--{related}\r\nContent-Type: multipart/alternative; boundary=\"{alt}\"\r\n\r\n"
    ));
    for (ctype, body) in [
        ("text/plain", msg.text()),
        ("text/html", msg.html(inline.map(|_| "dashboard"))),
    ] {
        m.push_str(&format!(
            "--{alt}\r\nContent-Type: {ctype}; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
            b64_lines(body.as_bytes())
        ));
    }
    m.push_str(&format!("--{alt}--\r\n"));
    if let Some(f) = inline {
        m.push_str(&format!(
            "--{related}\r\nContent-Type: image/png\r\nContent-ID: <dashboard>\r\n\
             Content-Disposition: inline; filename=\"{}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
            safe_filename(&f.name),
            b64_lines(&f.bytes)
        ));
    }
    m.push_str(&format!("--{related}--\r\n"));
    for f in files.iter().filter(|f| f.content_type != "image/png") {
        m.push_str(&format!(
            "--{mixed}\r\nContent-Type: {}\r\nContent-Disposition: attachment; filename=\"{}\"\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}",
            f.content_type,
            safe_filename(&f.name),
            b64_lines(&f.bytes)
        ));
    }
    m.push_str(&format!("--{mixed}--\r\n"));
    m.into_bytes()
}

fn safe_filename(n: &str) -> String {
    n.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---- SMTP ------------------------------------------------------------------

/// Read one reply (`250-…` continuation lines, then `250 …`): its code and
/// its lines' text.
fn reply<S: Read>(s: &mut S) -> Result<(u16, Vec<String>), String> {
    let mut lines = Vec::new();
    loop {
        let mut line = Vec::new();
        let mut b = [0u8; 1];
        loop {
            let n = s.read(&mut b).map_err(|e| format!("SMTP: {e}"))?;
            if n == 0 {
                return Err("SMTP: the server closed the connection".into());
            }
            if b[0] == b'\n' {
                break;
            }
            line.push(b[0]);
            if line.len() > 2048 {
                return Err("SMTP: a reply line is too long".into());
            }
        }
        let line = String::from_utf8_lossy(&line).trim_end().to_string();
        if line.len() < 3 {
            return Err(format!("SMTP: unexpected reply '{}'", one_line(&line)));
        }
        let code: u16 = line[..3]
            .parse()
            .map_err(|_| format!("SMTP: unexpected reply '{}'", one_line(&line)))?;
        let more = line.as_bytes().get(3) == Some(&b'-');
        lines.push(line.get(4..).unwrap_or("").to_string());
        if !more {
            return Ok((code, lines));
        }
        if lines.len() > 100 {
            return Err("SMTP: a reply is too long".into());
        }
    }
}

fn expect<S: Read>(s: &mut S, ok: &[u16], what: &str) -> Result<Vec<String>, String> {
    let (code, lines) = reply(s)?;
    if ok.contains(&code) {
        Ok(lines)
    } else {
        Err(format!(
            "SMTP {what}: {code} {}",
            one_line(&lines.join(" "))
                .chars()
                .take(200)
                .collect::<String>()
        ))
    }
}

fn cmd<S: Read + Write>(
    s: &mut S,
    line: &str,
    ok: &[u16],
    what: &str,
) -> Result<Vec<String>, String> {
    s.write_all(format!("{line}\r\n").as_bytes())
        .and_then(|_| s.flush())
        .map_err(|e| format!("SMTP {what}: {e}"))?;
    expect(s, ok, what)
}

fn ehlo<S: Read + Write>(s: &mut S) -> Result<Vec<String>, String> {
    cmd(s, "EHLO anofox-visualization", &[250], "EHLO")
}

/// After the greeting and EHLO: log in (if configured), then the envelope
/// and the message (dot-stuffed).
fn deliver<S: Read + Write>(
    s: &mut S,
    caps: &[String],
    cfg: &Smtp,
    rcpts: &[String],
    msg: &[u8],
) -> Result<(), String> {
    use base64::Engine as _;
    if let Some((user, pass)) = &cfg.login {
        let has = |m: &str| {
            caps.iter().any(|c| {
                let c = c.to_ascii_uppercase();
                c.starts_with("AUTH") && c.split([' ', '=']).any(|w| w == m)
            })
        };
        if has("PLAIN") {
            let token =
                base64::engine::general_purpose::STANDARD.encode(format!("\0{user}\0{pass}"));
            cmd(s, &format!("AUTH PLAIN {token}"), &[235], "AUTH")?;
        } else if has("LOGIN") {
            let b = |x: &str| base64::engine::general_purpose::STANDARD.encode(x);
            cmd(s, "AUTH LOGIN", &[334], "AUTH")?;
            cmd(s, &b(user), &[334], "AUTH")?;
            cmd(s, &b(pass), &[235], "AUTH")?;
        } else {
            return Err("SMTP: the server offers neither AUTH PLAIN nor AUTH LOGIN".into());
        }
    }
    cmd(s, &format!("MAIL FROM:<{}>", cfg.from), &[250], "MAIL FROM")?;
    for r in rcpts {
        cmd(s, &format!("RCPT TO:<{r}>"), &[250, 251], "RCPT TO")?;
    }
    cmd(s, "DATA", &[354], "DATA")?;
    let mut body = Vec::with_capacity(msg.len() + 64);
    for line in msg.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.first() == Some(&b'.') {
            body.push(b'.');
        }
        body.extend_from_slice(line);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b".\r\n");
    s.write_all(&body)
        .and_then(|_| s.flush())
        .map_err(|e| format!("SMTP DATA: {e}"))?;
    expect(s, &[250], "DATA")?;
    let _ = cmd(s, "QUIT", &[221], "QUIT");
    Ok(())
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

fn tls_wrap(
    tcp: TcpStream,
    host: &str,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, String> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| format!("SMTP: '{host}' is not a valid TLS server name"))?;
    let conn =
        rustls::ClientConnection::new(tls_config(), name).map_err(|e| format!("SMTP TLS: {e}"))?;
    Ok(rustls::StreamOwned::new(conn, tcp))
}

/// Send `msg` to `rcpts` through `cfg` (TLS as configured, `STARTTLS`
/// required by default).
pub fn send_email(cfg: &Smtp, rcpts: &[String], msg: &[u8]) -> Result<(), String> {
    let addr = (cfg.host.as_str(), cfg.port)
        .to_socket_addrs()
        .map_err(|e| format!("SMTP {}: {e}", cfg.host))?
        .next()
        .ok_or_else(|| format!("SMTP {}: no address", cfg.host))?;
    let tcp = TcpStream::connect_timeout(&addr, NET_TIMEOUT)
        .map_err(|e| format!("SMTP {}:{}: {e}", cfg.host, cfg.port))?;
    let _ = tcp.set_read_timeout(Some(NET_TIMEOUT));
    let _ = tcp.set_write_timeout(Some(NET_TIMEOUT));
    match cfg.tls {
        SmtpTls::Implicit => {
            let mut s = tls_wrap(tcp, &cfg.host)?;
            expect(&mut s, &[220], "greeting")?;
            let caps = ehlo(&mut s)?;
            deliver(&mut s, &caps, cfg, rcpts, msg)
        }
        SmtpTls::StartTls => {
            let mut plain = tcp;
            expect(&mut plain, &[220], "greeting")?;
            let caps = ehlo(&mut plain)?;
            if !caps.iter().any(|c| c.eq_ignore_ascii_case("STARTTLS")) {
                return Err("SMTP: the server does not offer STARTTLS (required)".into());
            }
            cmd(&mut plain, "STARTTLS", &[220], "STARTTLS")?;
            let mut s = tls_wrap(plain, &cfg.host)?;
            let caps = ehlo(&mut s)?;
            deliver(&mut s, &caps, cfg, rcpts, msg)
        }
        SmtpTls::None => {
            let mut s = tcp;
            expect(&mut s, &[220], "greeting")?;
            let caps = ehlo(&mut s)?;
            deliver(&mut s, &caps, cfg, rcpts, msg)
        }
    }
}

// ---- webhooks ----------------------------------------------------------------

/// The JSON a webhook gets.
pub fn webhook_body(msg: &Message, files: &[File]) -> Vec<u8> {
    use base64::Engine as _;
    serde_json::to_vec(&serde_json::json!({
        "kind": msg.kind,
        "name": msg.name,
        "status": msg.status,
        "detail": msg.detail,
        "title": msg.title,
        "time": crate::host::access::rfc3339(msg.time),
        "as_of": msg.as_of,
        "link": msg.link,
        "text": msg.text(),
        "files": files.iter().map(|f| serde_json::json!({
            "name": f.name,
            "content_type": f.content_type,
            "base64": base64::engine::general_purpose::STANDARD.encode(&f.bytes),
        })).collect::<Vec<_>>(),
    }))
    .unwrap_or_default()
}

/// `X-Anofox-Signature: sha256=<hex HMAC-SHA256(secret, "<timestamp>.<body>")>`
/// (the timestamp is `X-Anofox-Timestamp`, Unix seconds).
pub fn webhook_signature(secret: &[u8], ts: u64, body: &[u8]) -> String {
    use aws_lc_rs::hmac;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let mut ctx = hmac::Context::with_key(&key);
    ctx.update(ts.to_string().as_bytes());
    ctx.update(b".");
    ctx.update(body);
    let tag = ctx.sign();
    format!(
        "sha256={}",
        tag.as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// POST the signed body; a 2xx is success. No proxy, no redirects.
pub fn send_webhook(w: &Webhook, body: &[u8], now: SystemTime) -> Result<(), String> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::WebPki)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .build();
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(w.url.starts_with("https://"))
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent("anofox-visualization")
            .timeout_global(Some(NET_TIMEOUT))
            .build(),
    );
    let ts = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let r = agent
        .post(&w.url)
        .header("Content-Type", "application/json")
        .header("X-Anofox-Timestamp", &ts.to_string())
        .header(
            "X-Anofox-Signature",
            &webhook_signature(&w.secret, ts, body),
        )
        .send(body)
        // The URL may hold a token (in its path or query): the error is a
        // category, never ureq's own text, which can carry the URL.
        .map_err(|e| format!("webhook: {}", error_kind(&e)))?;
    let code = r.status().as_u16();
    if (200..300).contains(&code) {
        Ok(())
    } else {
        Err(format!("webhook: HTTP {code}"))
    }
}

fn error_kind(e: &ureq::Error) -> String {
    use ureq::Error as E;
    match e {
        E::Timeout(_) => "timed out".into(),
        E::HostNotFound => "host not found".into(),
        E::ConnectionFailed => "connection failed".into(),
        E::Io(io) => format!("I/O error ({:?})", io.kind()),
        E::Tls(_) | E::Rustls(_) | E::Pem(_) | E::TlsRequired => "TLS error".into(),
        E::TooManyRedirects | E::RedirectFailed => "redirected (not followed)".into(),
        _ => "request failed".into(),
    }
}

// ---- the rate ------------------------------------------------------------------

/// At most `max` messages in any hour, all reports together.
#[derive(Debug, Default)]
pub struct Rate {
    sent: VecDeque<Instant>,
}

impl Rate {
    /// Take `n` messages from the hour's budget, or refuse all of them.
    pub fn take(&mut self, n: usize, max: usize, now: Instant) -> bool {
        while self
            .sent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(3600))
        {
            self.sent.pop_front();
        }
        if self.sent.len() + n > max {
            return false;
        }
        self.sent.extend(std::iter::repeat_n(now, n));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(k: &str) -> Option<String> {
        match k {
            "U" => Some("mailer".into()),
            "P" => Some("hunter2".into()),
            "HOOK" => Some("https://hooks.example.com/x?token=abc".into()),
            "LOCALHOOK" => Some("http://127.0.0.1:9/h".into()),
            "SEC" => Some("0123456789abcdef0123".into()),
            "SHORT" => Some("short".into()),
            _ => None,
        }
    }

    fn notify() -> NotifyOptions {
        parse_notify(
            &json!({"run": true,
                    "smtp": {"host": "smtp.example.com", "user_env": "U", "password_env": "P", "from": "reports@example.com"},
                    "webhooks": {"ops": {"url_env": "HOOK", "secret_env": "SEC"}},
                    "link": "https://dash.example.com/"}),
            &env,
        )
        .unwrap()
    }

    #[test]
    fn notify_options() {
        let n = notify();
        let s = n.smtp.as_ref().unwrap();
        assert_eq!((s.port, s.tls), (587, SmtpTls::StartTls));
        assert_eq!(s.login, Some(("mailer".into(), "hunter2".into())));
        assert_eq!(n.link.as_deref(), Some("https://dash.example.com"));
        assert_eq!(n.max_per_hour, 60);
        // the secrets are not in a Debug print
        let d = format!("{n:?}");
        assert!(
            !d.contains("hunter2") && !d.contains("token=abc") && !d.contains("0123456789abcdef"),
            "{d}"
        );
        for (bad, why) in [
            (
                json!({"smtp": {"host": "smtp.example.com", "from": "x@example.com", "tls": "none"}}),
                "loopback",
            ),
            (
                json!({"smtp": {"host": "smtp.example.com", "from": "Reports <x@example.com>"}}),
                "plain address",
            ),
            (
                json!({"smtp": {"host": "h.example.com", "from": "x@example.com", "user_env": "U"}}),
                "go together",
            ),
            (
                json!({"smtp": {"host": "h.example.com", "from": "x@example.com", "user_env": "U", "password_env": "NOPE"}}),
                "'NOPE' is not set",
            ),
            (
                json!({"webhooks": {"ops": {"url_env": "HOOK", "secret_env": "SHORT"}}}),
                "16 bytes",
            ),
            (
                json!({"webhooks": {"Ops!": {"url_env": "HOOK", "secret_env": "SEC"}}}),
                "a name",
            ),
            (json!({"bogus": 1}), "unknown key"),
            (json!({"link": "javascript:alert(1)"}), "public URL"),
        ] {
            let e = parse_notify(&bad, &env).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        let l = parse_notify(
            &json!({"smtp": {"host": "127.0.0.1", "port": 2525, "tls": "none", "from": "r@example.com"},
                    "webhooks": {"local": {"url_env": "LOCALHOOK", "secret_env": "SEC"}}}),
            &env,
        )
        .unwrap();
        assert_eq!(l.smtp.unwrap().tls, SmtpTls::None);
    }

    #[test]
    fn webhook_urls() {
        for ok in [
            "https://h.example.com/x",
            "https://h.example.com/x?code=1",
            "http://127.0.0.1:8/x",
            "http://localhost/x",
            "http://[::1]:9/x",
        ] {
            assert!(check_webhook_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://h.example.com/x",
            "ftp://x",
            "https://u:p@h.example.com/",
            "https://h.example.com/#x",
            "https:// h/",
            "http://127.0.0.1.evil.com/",
        ] {
            assert!(check_webhook_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reports() {
        let n = notify();
        let r = parse_reports(
            &json!([{"name": "weekly-sales", "dashboard": "sales", "tab": "Overview",
                     "vars": {"region": "EU"}, "formats": ["png", "pdf", "csv:totals", "png"],
                     "schedule": "every monday 07:00 Europe/Berlin",
                     "as": {"user": "report:sales-eu", "groups": ["sales", "b"], "claims": {"Region": "EU"}},
                     "to": [{"email": "Sales-EU@example.com"}, {"webhook": "ops"}]}]),
            Some(&n),
        )
        .unwrap();
        let r = &r[0];
        assert_eq!(
            r.formats,
            vec![Format::Png, Format::Pdf, Format::Csv("totals".into())]
        );
        assert_eq!(r.principal.user, "report:sales-eu");
        assert_eq!(r.principal.groups, vec!["b", "sales"]);
        assert_eq!(
            r.principal.attrs,
            vec![("region".into(), Some("EU".into()))]
        );
        assert_eq!(
            r.to,
            vec![
                Recipient::Email("sales-eu@example.com".into()),
                Recipient::Webhook("ops".into())
            ]
        );
        assert!(r.schedule.contains("'Europe/Berlin'"), "{}", r.schedule);
        // no login method may claim a report's principal
        assert!(crate::host::access::is_reserved_user("Report:sales-eu"));
        let base = json!({"name": "r", "dashboard": "d", "formats": ["png"], "schedule": "every day 06:30",
                          "as": {"user": "report:r"}, "to": [{"email": "a@example.com"}]});
        let with = |k: &str, v: Value| {
            let mut b = base.clone();
            b[k] = v;
            json!([b])
        };
        assert!(parse_reports(&json!([base.clone()]), Some(&n)).is_ok());
        for (bad, why) in [
            (with("as", json!({"user": "alice"})), "report:<name>"),
            (with("as", Value::Null), "as is required"),
            (
                with("to", json!([{"email": "a@example.com\r\nBcc: x@evil.com"}])),
                "plain e-mail",
            ),
            (
                with("to", json!([{"webhook": "nope"}])),
                "no webhook 'nope'",
            ),
            (
                with(
                    "to",
                    json!((0..51)
                        .map(|i| json!({"email": format!("u{i}@example.com")}))
                        .collect::<Vec<_>>()),
                ),
                "more than 50",
            ),
            (with("formats", json!(["svg"])), "format 'svg'"),
            (with("formats", json!(["csv:../x"])), "format"),
            (with("schedule", json!("every fortnight")), "schedule"),
            (with("dashboard", json!("../x")), "dashboard"),
            (with("vars", json!({"n": 1})), "strings"),
            (json!([base.clone(), base.clone()]), "second report"),
        ] {
            let e = parse_reports(&bad, Some(&n)).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        // e-mail without smtp
        let e = parse_reports(&json!([base]), None).unwrap_err();
        assert!(e.contains("needs options.notify.smtp"), "{e}");
    }

    #[test]
    fn schedules() {
        assert_eq!(schedule_sql("init").unwrap(), "'init'");
        assert_eq!(
            schedule_sql("every 15 minutes").unwrap(),
            "INTERVAL 15 MINUTE"
        );
        assert_eq!(
            schedule_sql("every 2 hours").unwrap(),
            "INTERVAL 120 MINUTE"
        );
        assert!(schedule_sql("every hour")
            .unwrap()
            .starts_with("date_trunc('hour'"));
        let d = schedule_sql("every day 06:30").unwrap();
        assert!(
            d.contains("current_setting('TimeZone')")
                && d.contains("'06:30:00'")
                && d.contains("1 DAY"),
            "{d}"
        );
        let w = schedule_sql("Every Monday 07:00 Europe/Berlin").unwrap();
        assert!(
            w.contains("'Europe/Berlin'") && w.contains("((1 - isodow(t))") && w.contains("7 DAY"),
            "{w}"
        );
        assert_eq!(
            schedule_sql("INTERVAL '1 day'").unwrap(),
            "INTERVAL '1 day'"
        );
        for bad in [
            "every 0 minutes",
            "every day 25:00",
            "every day 7",
            "every funday 07:00",
            "every day 07:00 Europe/Berlin'; --",
            "every day 07:00 a b",
            "INTERVAL 1 DAY; DROP TABLE x",
        ] {
            assert!(schedule_sql(bad).is_err(), "{bad}");
        }
    }

    fn msg() -> Message {
        Message {
            kind: "report",
            name: "weekly".into(),
            status: None,
            detail: None,
            title: "Sales <EU>\r\nBcc: evil@example.com".into(),
            as_of: Some("Data as of 2026-10-30".into()),
            link: Some("https://dash.example.com/d/sales".into()),
            time: SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_430_000),
        }
    }

    #[test]
    fn the_email() {
        let files = vec![
            File {
                name: "sales.png".into(),
                content_type: "image/png",
                bytes: vec![0x89, b'P', b'N', b'G'],
            },
            File {
                name: "sales.pdf".into(),
                content_type: "application/pdf",
                bytes: b"%PDF".to_vec(),
            },
        ];
        let m = String::from_utf8(email("reports@example.com", &msg(), &files)).unwrap();
        let head = m.split("\r\n\r\n").next().unwrap();
        // no header injection: the title's line break is a space
        assert!(!head.contains("\r\nBcc:"), "{head}");
        assert!(
            head.contains("Subject: Sales <EU>  Bcc: evil@example.com")
                || head.contains("Subject: =?UTF-8?B?"),
            "{head}"
        );
        assert!(head.contains("To: undisclosed-recipients:;"));
        assert!(head.contains("Date: ") && head.contains(" +0000"));
        assert!(m.contains("Content-ID: <dashboard>") && m.contains("filename=\"sales.pdf\""));
        // every line is short (base64 at 76)
        assert!(m.split("\r\n").all(|l| l.len() <= 998));
        // the HTML is escaped (decode the html part)
        use base64::Engine as _;
        let html_b64: String = m
            .split("Content-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n")
            .nth(1)
            .unwrap()
            .split("--")
            .next()
            .unwrap()
            .replace("\r\n", "");
        let html = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(html_b64)
                .unwrap(),
        )
        .unwrap();
        assert!(
            html.contains("Sales &lt;EU&gt;") && html.contains("src=\"cid:dashboard\""),
            "{html}"
        );
        assert_eq!(
            mail_date(SystemTime::UNIX_EPOCH),
            "Thu, 01 Jan 1970 00:00:00 +0000"
        );
        assert_eq!(
            header_text("Umsatz — EU"),
            "=?UTF-8?B?VW1zYXR6IOKAlCBFVQ==?="
        );
    }

    /// A scripted SMTP server: the replies, in order; what the client wrote.
    struct Fake {
        replies: std::io::Cursor<Vec<u8>>,
        wrote: Vec<u8>,
    }

    impl Read for Fake {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.replies.read(b)
        }
    }

    impl Write for Fake {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.wrote.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn smtp_dialogue() {
        let cfg = notify().smtp.unwrap();
        let mut f = Fake {
            replies: std::io::Cursor::new(
                b"220 hi\r\n250-mx\r\n250-AUTH LOGIN PLAIN\r\n250 8BITMIME\r\n235 ok\r\n250 ok\r\n250 ok\r\n250 ok\r\n354 go\r\n250 queued\r\n221 bye\r\n".to_vec(),
            ),
            wrote: Vec::new(),
        };
        expect(&mut f, &[220], "greeting").unwrap();
        let caps = ehlo(&mut f).unwrap();
        deliver(
            &mut f,
            &caps,
            &cfg,
            &["a@example.com".into(), "b@example.com".into()],
            b"Subject: x\r\n\r\n.line\r\nend\r\n",
        )
        .unwrap();
        let w = String::from_utf8(f.wrote).unwrap();
        assert!(w.contains("AUTH PLAIN AG1haWxlcgBodW50ZXIy\r\n"), "{w}");
        assert!(w.contains("MAIL FROM:<reports@example.com>\r\nRCPT TO:<a@example.com>\r\nRCPT TO:<b@example.com>\r\nDATA\r\n"));
        // dot-stuffing, and the end of the data
        assert!(w.contains("\r\n..line\r\nend\r\n\r\n.\r\nQUIT\r\n"), "{w}");
        // a refused recipient stops it
        let mut f = Fake {
            replies: std::io::Cursor::new(b"250 ok\r\n550 no such user\r\n".to_vec()),
            wrote: Vec::new(),
        };
        let mut c = cfg.clone();
        c.login = None;
        let e = deliver(&mut f, &[], &c, &["a@example.com".into()], b"x").unwrap_err();
        assert!(e.contains("RCPT TO: 550 no such user"), "{e}");
    }

    #[test]
    fn alerts() {
        let mut n = notify();
        let base = json!({"name": "low", "dashboard": "stock", "panel": "below_safety", "check": "every 15 minutes",
                          "as": {"user": "report:ops"}, "attach": ["csv"], "to": [{"webhook": "ops"}], "repeat_after": "24 hours"});
        let e = parse_alerts(&json!([base.clone()]), Some(&n)).unwrap_err();
        assert!(e.contains("needs options.notify.state"), "{e}");
        n.state = Some("/tmp/x.json".into());
        let a = &parse_alerts(&json!([base.clone()]), Some(&n)).unwrap()[0];
        assert_eq!(a.attach, vec![Format::Csv("below_safety".into())]);
        assert_eq!(a.repeat_after, Some(Duration::from_secs(86_400)));
        assert_eq!(a.schedule, "INTERVAL 15 MINUTE");
        let with = |k: &str, v: Value| {
            let mut b = base.clone();
            b[k] = v;
            json!([b])
        };
        for (bad, why) in [
            (with("panel", json!("../x")), "panel is required"),
            (with("attach", json!(["png"])), "attach takes"),
            (with("repeat_after", json!("soon")), "repeat_after"),
            (with("repeat_after", json!("0 hours")), "repeat_after"),
            (with("as", json!({"user": "ops"})), "report:<name>"),
            (with("check", Value::Null), "check is required"),
            (json!([base.clone(), base.clone()]), "second alert"),
        ] {
            let e = parse_alerts(&bad, Some(&n)).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        assert_eq!(
            parse_duration("30 minutes").unwrap(),
            Duration::from_secs(1800)
        );
        assert_eq!(
            parse_duration("1 Day").unwrap(),
            Duration::from_secs(86_400)
        );
    }

    #[test]
    fn alert_transitions() {
        use AlertAction::*;
        let day = Some(Duration::from_secs(86_400));
        // nothing → rows: fire
        let (a, s) = decide(None, Some("h1"), 100, day).unwrap();
        assert_eq!((a, s.firing, s.last_sent), (Fire, true, 100));
        // same rows: nothing, until repeat_after
        assert_eq!(decide(Some(&s), Some("h1"), 200, day), None);
        assert_eq!(decide(Some(&s), Some("h1"), 200, None), None);
        let (a, s2) = decide(Some(&s), Some("h1"), 100 + 86_400, day).unwrap();
        assert_eq!((a, s2.last_sent), (Repeat, 100 + 86_400));
        // other rows: changed
        assert_eq!(decide(Some(&s), Some("h2"), 300, None).unwrap().0, Changed);
        // no rows: resolved once, then quiet
        let (a, s3) = decide(Some(&s), None, 400, day).unwrap();
        assert_eq!((a, s3.firing), (Resolved, false));
        assert_eq!(decide(Some(&s3), None, 500, day), None);
        assert_eq!(decide(None, None, 500, day), None);
        assert_eq!(Fire.status(), "FIRING");
        // the state file round-trips
        let dir = std::env::temp_dir().join(format!("anofox-alert-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.json");
        assert!(load_states(&p).unwrap().is_empty());
        let mut m = BTreeMap::new();
        m.insert("low".to_string(), s.clone());
        save_states(&p, &m).unwrap();
        assert_eq!(load_states(&p).unwrap(), m);
        std::fs::write(&p, "nope").unwrap();
        assert!(load_states(&p).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signature_and_rate() {
        // HMAC-SHA256("key", "1.body")
        let s = webhook_signature(b"key", 1, b"body");
        assert!(s.starts_with("sha256=") && s.len() == 7 + 64);
        assert_ne!(s, webhook_signature(b"key", 2, b"body"));
        let body: Value = serde_json::from_slice(&webhook_body(
            &msg(),
            &[File {
                name: "a.csv".into(),
                content_type: "text/csv",
                bytes: b"x,y".to_vec(),
            }],
        ))
        .unwrap();
        assert_eq!(body["files"][0]["base64"], "eCx5");
        assert_eq!(
            (&body["kind"], &body["name"]),
            (&json!("report"), &json!("weekly"))
        );
        let mut r = Rate::default();
        let t = Instant::now();
        assert!(r.take(2, 3, t));
        assert!(!r.take(2, 3, t));
        assert!(r.take(1, 3, t));
        assert!(r.take(3, 3, t + Duration::from_secs(3600)));
        assert_eq!(error_kind(&ureq::Error::HostNotFound), "host not found");
    }
}

//! `/assist`: the authoring servers' proxy to the model API (#740,
//! `docs/plans/plan-ai-assistant.md` §2.3). The builder's assistant sends its
//! Messages requests here instead of straight to the API, so the API key stays
//! on the server (`ANTHROPIC_API_KEY` or a key file) and never reaches the
//! browser.
//!
//! Shared by `src/bin/serve.rs` and `duckext/src/serve.rs`, and routed **only
//! by their authoring handlers**, behind the same `AuthoringGuard` as `/query`
//! (loopback Host, same Origin, the per-run token). The locked servers never
//! route `/assist/*`.
//!
//! The proxy is not an open relay to the API (security review M7, L3):
//! - an allowlist of top-level fields, tool fields and content block types;
//!   anything else is a 400. Models from a list, `max_tokens` capped, only the
//!   assistant's own tools (no server tools, no `mcp_servers`);
//! - no client header is forwarded; `anthropic-version` is pinned here and an
//!   `anthropic-beta` from the client never passes;
//! - upstream: https unless the base URL is loopback, no redirects, no proxy
//!   from the environment, a generic User-Agent, no identity in headers;
//!   upstream error bodies are never relayed (only their error type);
//! - an hourly token budget per server (429 over it), a small pool of
//!   concurrent calls (503 when full), and one JSON log line per call with
//!   no prompt text unless `log_content` is set.
//!
//! No HTTP server type in here: [`Proxy::handle`] returns a [`Reply`], and
//! [`write_http`] writes it to the raw connection (a streamed reply is
//! flushed after every read, which `tiny_http`'s own chunked writer doesn't
//! do).

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};

use crate::host::access::{open_log, rfc3339, LogSink};

/// Largest request body (the context block, the SQL and the history).
pub const MAX_REQUEST: usize = 2 << 20;
/// Largest non-streamed upstream answer we read.
const MAX_JSON_RESPONSE: u64 = 16 << 20;
/// Largest upstream error body we read (to take its error type only).
const MAX_ERROR_BODY: u64 = 64 << 10;
/// Pinned on the server; the client's is never forwarded.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODELS: &[&str] = &["claude-sonnet-5", "claude-opus-5-5"];
pub const DEFAULT_MAX_TOKENS: u64 = 32_000;
/// Tokens of every kind (input, cache writes and reads, output) per hour.
pub const DEFAULT_BUDGET: u64 = 2_000_000;
/// Concurrent upstream calls: a 30–60 s stream must not hold `/query`'s
/// worker, and two is enough for one author.
pub const DEFAULT_WORKERS: usize = 2;
/// The environment variable the key is read from.
pub const KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// The assistant's own tools (`TOOLS` in `web/assist.js`). Any other tool
/// name in a request is a 400.
pub const TOOL_NAMES: &[&str] = &[
    "list_tables",
    "profile_table",
    "run_sql",
    "check_dashboard",
    "propose_dashboard",
];
const MESSAGE_FIELDS: &[&str] = &[
    "model",
    "max_tokens",
    "system",
    "messages",
    "tools",
    "tool_choice",
    "thinking",
    "stream",
    "metadata",
    "output_config",
    "cache_control",
];
const COUNT_FIELDS: &[&str] = &[
    "model",
    "system",
    "messages",
    "tools",
    "tool_choice",
    "thinking",
    "output_config",
    "cache_control",
];
const TOOL_FIELDS: &[&str] = &[
    "name",
    "description",
    "input_schema",
    "strict",
    "cache_control",
    "eager_input_streaming",
];
/// Content blocks a conversation of the assistant holds. No images or
/// documents: those can name a URL or a file id for the API to fetch.
const BLOCK_TYPES: &[&str] = &[
    "text",
    "tool_use",
    "tool_result",
    "thinking",
    "redacted_thinking",
];
/// Upstream error types passed on to the client (anything else is
/// `api_error`); the upstream message itself never is.
const ERROR_TYPES: &[&str] = &[
    "invalid_request_error",
    "authentication_error",
    "permission_error",
    "not_found_error",
    "request_too_large",
    "rate_limit_error",
    "api_error",
    "overloaded_error",
];

/// The proxy's settings (`serve --assist-*`, `anofox_serve`'s `assist_*`).
#[derive(Debug, Clone)]
pub struct AssistConfig {
    pub models: Vec<String>,
    pub max_tokens: u64,
    pub base_url: String,
    pub budget_tokens_per_hour: u64,
    pub workers: usize,
    /// Log the request body (prompts, SQL) with each call — for audits only.
    pub log_content: bool,
}

impl Default for AssistConfig {
    fn default() -> Self {
        AssistConfig {
            models: DEFAULT_MODELS.iter().map(|s| s.to_string()).collect(),
            max_tokens: DEFAULT_MAX_TOKENS,
            base_url: DEFAULT_BASE_URL.into(),
            budget_tokens_per_hour: DEFAULT_BUDGET,
            workers: DEFAULT_WORKERS,
            log_content: false,
        }
    }
}

impl AssistConfig {
    /// `claude-a,claude-b`: each a model id (`[a-z0-9.-]`, ≤ 64).
    pub fn set_models(&mut self, list: &str) -> Result<(), String> {
        let models: Vec<String> = list
            .split(',')
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if models.is_empty() {
            return Err("assist models: give at least one model id".into());
        }
        for m in &models {
            if m.len() > 64
                || !m
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
            {
                return Err(format!("assist models: '{m}' is not a model id"));
            }
        }
        self.models = models;
        Ok(())
    }

    pub fn set_base_url(&mut self, url: &str) -> Result<(), String> {
        check_base_url(url)?;
        self.base_url = url.trim_end_matches('/').to_string();
        Ok(())
    }
}

/// The base URL must be `https://host[:port][/path]`, or `http://` to a
/// loopback address (a local gateway or a test). No userinfo, query or
/// fragment, printable ASCII only.
pub fn check_base_url(url: &str) -> Result<(), String> {
    let bad = || {
        format!("assist base URL '{url}' must be https://host[/path] (http:// only to a loopback address)")
    };
    let (rest, https) = if let Some(r) = url.strip_prefix("https://") {
        (r, true)
    } else if let Some(r) = url.strip_prefix("http://") {
        (r, false)
    } else {
        return Err(bad());
    };
    if url.len() > 512
        || !url.bytes().all(|b| b.is_ascii_graphic())
        || url.contains(['?', '#', '@', '\\'])
    {
        return Err(bad());
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err(bad());
    }
    if !https {
        let host = match authority.strip_prefix('[') {
            Some(r) => r.split(']').next().unwrap_or(""),
            None => authority.split(':').next().unwrap_or(""),
        };
        if !crate::host::serving::is_loopback_bind(host) {
            return Err(bad());
        }
    }
    Ok(())
}

/// The key: from `key_file` when given, else from `ANTHROPIC_API_KEY`;
/// `None` when neither is set (the proxy is then off).
pub fn load_key(key_file: Option<&Path>) -> Result<Option<String>, String> {
    let key = match key_file {
        Some(p) => read_key_file(p)?,
        None => match std::env::var(KEY_ENV) {
            Ok(k) if !k.trim().is_empty() => k.trim().to_string(),
            _ => return Ok(None),
        },
    };
    if key.len() > 512 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("assist key: not a valid API key (printable characters, no spaces)".into());
    }
    Ok(Some(key))
}

/// A key file: a regular file (a symlink is refused), not readable by group
/// or others, at most 4 KiB; its trimmed text is the key. Errors never show
/// the content.
pub fn read_key_file(p: &Path) -> Result<String, String> {
    let what = |e: &dyn std::fmt::Display| format!("assist key file {}: {e}", p.display());
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut f = o.open(p).map_err(|e| what(&e))?;
    let meta = f.metadata().map_err(|e| what(&e))?;
    if !meta.is_file() {
        return Err(what(&"not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(what(&"readable by others: chmod 600 it"));
        }
    }
    let mut s = String::new();
    (&mut f)
        .take(4097)
        .read_to_string(&mut s)
        .map_err(|_| what(&"not UTF-8 text"))?;
    if s.len() > 4096 {
        return Err(what(&"larger than 4 KiB"));
    }
    let s = s.trim().to_string();
    if s.is_empty() {
        return Err(what(&"empty"));
    }
    Ok(s)
}

// ------------------------------------------------------------ validation ---

/// The two routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Messages,
    CountTokens,
}

impl Endpoint {
    /// `/assist/v1/messages` and `/assist/v1/messages/count_tokens`.
    pub fn from_path(path: &str) -> Option<Endpoint> {
        match path {
            "/assist/v1/messages" => Some(Endpoint::Messages),
            "/assist/v1/messages/count_tokens" => Some(Endpoint::CountTokens),
            _ => None,
        }
    }
    fn upstream_path(self) -> &'static str {
        match self {
            Endpoint::Messages => "/v1/messages",
            Endpoint::CountTokens => "/v1/messages/count_tokens",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Endpoint::Messages => "messages",
            Endpoint::CountTokens => "count_tokens",
        }
    }
}

/// The proxy's own path space, `/assist/v1/…` — never `/assist/<file>`,
/// which is the builder's code (`web/assist/functions.js`, …) on every server.
pub fn is_api_path(path: &str) -> bool {
    path == "/assist/v1" || path.starts_with("/assist/v1/")
}

/// A validated request: the body to send and what the log needs.
#[derive(Debug)]
pub struct Checked {
    pub body: Vec<u8>,
    pub model: String,
    pub stream: bool,
}

/// Check a request body against the allowlist. The error is the message of
/// a 400 (or 413), safe to show: it names a field, never echoes a value.
pub fn validate(ep: Endpoint, body: &[u8], cfg: &AssistConfig) -> Result<Checked, (u16, String)> {
    let bad = |m: String| (400u16, m);
    if body.len() > MAX_REQUEST {
        return Err((413, "request body too large".into()));
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| bad("the body is not JSON".into()))?;
    let o = v
        .as_object()
        .ok_or_else(|| bad("the body is not a JSON object".into()))?;
    let allowed = match ep {
        Endpoint::Messages => MESSAGE_FIELDS,
        Endpoint::CountTokens => COUNT_FIELDS,
    };
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(bad(format!("field '{}' is not allowed here", short(k))));
        }
    }
    let model = o
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("model is missing".into()))?;
    if !cfg.models.iter().any(|m| m == model) {
        return Err(bad(format!(
            "model '{}' is not allowed on this server (allowed: {})",
            short(model),
            cfg.models.join(", ")
        )));
    }
    if ep == Endpoint::Messages {
        let mt = o
            .get("max_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("max_tokens is missing or not a positive integer".into()))?;
        if mt == 0 || mt > cfg.max_tokens {
            return Err(bad(format!("max_tokens must be 1..{}", cfg.max_tokens)));
        }
    }
    let stream = match o.get("stream") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("stream must be true or false".into())),
    };
    if let Some(m) = o.get("metadata") {
        check_metadata(m).map_err(bad)?;
    }
    if let Some(s) = o.get("system") {
        check_system(s).map_err(bad)?;
    }
    if let Some(t) = o.get("tools") {
        check_tools(t).map_err(bad)?;
    }
    if let Some(tc) = o.get("tool_choice") {
        check_tool_choice(tc).map_err(bad)?;
    }
    for k in ["thinking", "output_config", "cache_control"] {
        if o.get(k).is_some_and(|x| !x.is_object()) {
            return Err(bad(format!("{k} must be an object")));
        }
    }
    check_messages(o.get("messages")).map_err(bad)?;
    Ok(Checked {
        body: serde_json::to_vec(&v).unwrap_or_default(),
        model: model.to_string(),
        stream,
    })
}

/// A name or value for an error message: at most 64 characters, printable.
fn short(s: &str) -> String {
    s.chars()
        .take(64)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn check_metadata(m: &Value) -> Result<(), String> {
    let o = m.as_object().ok_or("metadata must be an object")?;
    for (k, v) in o {
        if k != "user_id" {
            return Err(format!("metadata.{} is not allowed", short(k)));
        }
        // An opaque id only: never an e-mail address or a name.
        let ok = v.as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
        if !ok {
            return Err("metadata.user_id must be an opaque id ([A-Za-z0-9_-], at most 64)".into());
        }
    }
    Ok(())
}

fn check_system(s: &Value) -> Result<(), String> {
    match s {
        Value::String(_) => Ok(()),
        Value::Array(blocks) => {
            for b in blocks {
                if b.get("type").and_then(Value::as_str) != Some("text") {
                    return Err("system blocks must be text".into());
                }
            }
            Ok(())
        }
        _ => Err("system must be text or a list of text blocks".into()),
    }
}

fn check_tools(t: &Value) -> Result<(), String> {
    let tools = t.as_array().ok_or("tools must be a list")?;
    for tool in tools {
        let o = tool.as_object().ok_or("a tool must be an object")?;
        // No `type` (server tools carry one), no other field.
        for k in o.keys() {
            if !TOOL_FIELDS.contains(&k.as_str()) {
                return Err(format!(
                    "tool field '{}' is not allowed (only the assistant's own tools)",
                    short(k)
                ));
            }
        }
        let name = o.get("name").and_then(Value::as_str).unwrap_or("");
        if !TOOL_NAMES.contains(&name) {
            return Err(format!(
                "tool '{}' is not one of the assistant's tools",
                short(name)
            ));
        }
        if !o.get("input_schema").is_some_and(Value::is_object) {
            return Err(format!("tool '{name}' needs an input_schema object"));
        }
    }
    Ok(())
}

fn check_tool_choice(tc: &Value) -> Result<(), String> {
    let o = tc.as_object().ok_or("tool_choice must be an object")?;
    match o.get("type").and_then(Value::as_str) {
        Some("auto" | "any" | "none") => {}
        Some("tool") => {
            let name = o.get("name").and_then(Value::as_str).unwrap_or("");
            if !TOOL_NAMES.contains(&name) {
                return Err("tool_choice names a tool that isn't the assistant's".into());
            }
        }
        _ => return Err("tool_choice.type must be auto, any, tool or none".into()),
    }
    for k in o.keys() {
        if !matches!(k.as_str(), "type" | "name" | "disable_parallel_tool_use") {
            return Err(format!("tool_choice.{} is not allowed", short(k)));
        }
    }
    Ok(())
}

fn check_messages(m: Option<&Value>) -> Result<(), String> {
    let msgs = m
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or("messages must be a non-empty list")?;
    for (i, msg) in msgs.iter().enumerate() {
        let o = msg
            .as_object()
            .ok_or_else(|| format!("messages[{i}] is not an object"))?;
        for k in o.keys() {
            if k != "role" && k != "content" {
                return Err(format!("messages[{i}].{} is not allowed", short(k)));
            }
        }
        if !matches!(
            o.get("role").and_then(Value::as_str),
            Some("user" | "assistant")
        ) {
            return Err(format!("messages[{i}].role must be user or assistant"));
        }
        match o.get("content") {
            Some(Value::String(_)) => {}
            Some(Value::Array(blocks)) => {
                for (j, b) in blocks.iter().enumerate() {
                    check_block(b).map_err(|e| format!("messages[{i}].content[{j}]: {e}"))?;
                }
            }
            _ => {
                return Err(format!(
                    "messages[{i}].content must be text or a list of blocks"
                ))
            }
        }
    }
    Ok(())
}

fn check_block(b: &Value) -> Result<(), String> {
    let ty = b.get("type").and_then(Value::as_str).unwrap_or("");
    if !BLOCK_TYPES.contains(&ty) {
        return Err(format!("block type '{}' is not allowed", short(ty)));
    }
    match ty {
        "tool_use" => {
            let name = b.get("name").and_then(Value::as_str).unwrap_or("");
            if !TOOL_NAMES.contains(&name) {
                return Err(format!(
                    "tool_use of '{}', not one of the assistant's tools",
                    short(name)
                ));
            }
        }
        "tool_result" => match b.get("content") {
            None | Some(Value::String(_)) => {}
            Some(Value::Array(inner)) => {
                if inner
                    .iter()
                    .any(|x| x.get("type").and_then(Value::as_str) != Some("text"))
                {
                    return Err("a tool_result may hold text blocks only".into());
                }
            }
            Some(_) => return Err("tool_result content must be text".into()),
        },
        _ => {}
    }
    Ok(())
}

// --------------------------------------------------------------- upstream ---

/// One upstream answer: its status, and its body as a stream.
pub struct UpResponse {
    pub status: u16,
    pub request_id: Option<String>,
    pub body: Box<dyn Read + Send>,
}

/// The model API. A trait so the tests can stand in a fake.
pub trait Upstream: Send + Sync {
    /// POST `body` (JSON) to `path` (`/v1/messages…`).
    fn post(&self, path: &str, body: &[u8], stream: bool) -> Result<UpResponse, String>;
}

/// The real client (ureq + rustls on aws-lc-rs, the Mozilla roots).
pub struct HttpUpstream {
    agent: ureq::Agent,
    base_url: String,
    key: String,
}

impl HttpUpstream {
    pub fn new(base_url: &str, key: String) -> Result<HttpUpstream, String> {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        check_base_url(base_url)?;
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::WebPki)
            .unversioned_rustls_crypto_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(base_url.starts_with("https://"))
            // L3: never a proxy from the environment (it would see the key).
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent("anofox-visualization")
            .timeout_connect(Some(Duration::from_secs(15)))
            // A long, non-streamed answer takes minutes to start; a whole
            // call is capped.
            .timeout_recv_response(Some(Duration::from_secs(10 * 60)))
            .timeout_global(Some(Duration::from_secs(30 * 60)))
            .build();
        Ok(HttpUpstream {
            agent: ureq::Agent::new_with_config(config),
            base_url: base_url.trim_end_matches('/').to_string(),
            key,
        })
    }
}

impl Upstream for HttpUpstream {
    fn post(&self, path: &str, body: &[u8], stream: bool) -> Result<UpResponse, String> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .agent
            .post(&url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header(
                "accept",
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .send(body)
            // The error text names the host, never the key; it goes to the
            // server's own stderr only.
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let request_id = resp
            .headers()
            .get("request-id")
            .and_then(|h| h.to_str().ok())
            .filter(|s| valid_request_id(s))
            .map(str::to_string);
        Ok(UpResponse {
            status,
            request_id,
            body: Box::new(resp.into_body().into_reader()),
        })
    }
}

fn valid_request_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// ------------------------------------------------------------------ proxy ---

/// What a call used, from the answer's `usage`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }
    /// Merge a `usage` object: each count is cumulative within a stream, so
    /// the larger one wins.
    fn merge(&mut self, u: &Value) {
        let get = |k: &str| u.get(k).and_then(Value::as_u64);
        if let Some(n) = get("input_tokens") {
            self.input = self.input.max(n);
        }
        if let Some(n) = get("output_tokens") {
            self.output = self.output.max(n);
        }
        if let Some(n) = get("cache_creation_input_tokens") {
            self.cache_write = self.cache_write.max(n);
        }
        if let Some(n) = get("cache_read_input_tokens") {
            self.cache_read = self.cache_read.max(n);
        }
    }
}

/// A reply body: whole, or streamed from upstream.
pub enum Body {
    Bytes(Vec<u8>),
    Stream(Box<dyn Read + Send>),
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub request_id: Option<String>,
    /// `false` on the proxy's own refusals: `x-should-retry: false` tells
    /// the SDK not to retry them.
    pub retry: bool,
    pub body: Body,
}

impl Reply {
    /// An error in the API's own shape, so the SDK reads it as one.
    fn error(status: u16, ty: &str, message: &str, retry: bool) -> Reply {
        let body = json!({"type": "error", "error": {"type": ty, "message": message}});
        Reply {
            status,
            content_type: "application/json",
            request_id: None,
            retry,
            body: Body::Bytes(serde_json::to_vec(&body).unwrap_or_default()),
        }
    }
}

struct Inner {
    cfg: AssistConfig,
    upstream: Box<dyn Upstream>,
    /// Tokens used, with when: the budget's sliding hour.
    used: Mutex<VecDeque<(Instant, u64)>>,
    inflight: AtomicUsize,
    log: Option<LogSink>,
}

/// The proxy of one authoring server.
#[derive(Clone)]
pub struct Proxy {
    inner: Arc<Inner>,
}

/// One of the pool's places, held until the reply is written (a stream
/// holds it until it ends).
pub struct Slot(Arc<Inner>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Proxy {
    pub fn new(cfg: AssistConfig, upstream: Box<dyn Upstream>, log: Option<LogSink>) -> Proxy {
        Proxy {
            inner: Arc::new(Inner {
                cfg,
                upstream,
                used: Mutex::new(VecDeque::new()),
                inflight: AtomicUsize::new(0),
                log,
            }),
        }
    }

    pub fn config(&self) -> &AssistConfig {
        &self.inner.cfg
    }

    /// A place in the pool, or `None` when every place is taken (503).
    pub fn try_slot(&self) -> Option<Slot> {
        let max = self.inner.cfg.workers.max(1);
        let mut n = self.inner.inflight.load(Ordering::SeqCst);
        loop {
            if n >= max {
                return None;
            }
            match self.inner.inflight.compare_exchange_weak(
                n,
                n + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(Slot(self.inner.clone())),
                Err(now) => n = now,
            }
        }
    }

    /// The reply for a pool that is full.
    pub fn busy() -> Reply {
        Reply::error(
            503,
            "overloaded_error",
            "the assistant is already answering on this server; try again when it is done",
            false,
        )
    }

    /// Tokens used in the last hour.
    pub fn used_last_hour(&self) -> u64 {
        let mut q = self.inner.used.lock().unwrap_or_else(|p| p.into_inner());
        prune(&mut q, Instant::now());
        q.iter().map(|(_, n)| n).sum()
    }

    /// Validate, check the budget, call upstream and build the reply. The
    /// slot goes with a streamed body and is released when that ends.
    pub fn handle(&self, slot: Slot, ep: Endpoint, body: &[u8]) -> Reply {
        let started = Instant::now();
        let inner = &self.inner;
        let checked = match validate(ep, body, &inner.cfg) {
            Ok(c) => c,
            Err((status, msg)) => {
                let ty = if status == 413 {
                    "request_too_large"
                } else {
                    "invalid_request_error"
                };
                return Reply::error(status, ty, &msg, false);
            }
        };
        let mut rec = Record {
            endpoint: ep,
            model: checked.model.clone(),
            status: 0,
            usage: Usage::default(),
            request_id: None,
            started,
            complete: false,
            content: inner
                .cfg
                .log_content
                .then(|| String::from_utf8_lossy(&checked.body).into_owned()),
        };
        // count_tokens is free: it never counts against the budget, and isn't
        // refused by it either.
        if ep == Endpoint::Messages && self.used_last_hour() >= inner.cfg.budget_tokens_per_hour {
            rec.status = 429;
            inner.finish(&rec);
            return Reply::error(
                429,
                "rate_limit_error",
                "this server's hourly token budget for the assistant is used up; try again later",
                false,
            );
        }
        let up = match inner
            .upstream
            .post(ep.upstream_path(), &checked.body, checked.stream)
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[anofox-serve] assist: upstream call failed: {}",
                    crate::host::access::diag(&e)
                );
                rec.status = 502;
                inner.finish(&rec);
                return Reply::error(
                    502,
                    "api_error",
                    "the server could not reach the model service",
                    true,
                );
            }
        };
        rec.status = up.status;
        rec.request_id = up.request_id.clone();
        if !(200..300).contains(&up.status) {
            // Never the upstream body (it can echo the request): its error
            // type only, when it is one we know.
            let mut buf = Vec::new();
            let _ = up.body.take(MAX_ERROR_BODY).read_to_end(&mut buf);
            let ty = serde_json::from_slice::<Value>(&buf)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/type")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .filter(|t| ERROR_TYPES.contains(&t.as_str()))
                .unwrap_or_else(|| "api_error".into());
            rec.complete = true;
            inner.finish(&rec);
            let mut r = Reply::error(
                up.status,
                &ty,
                &format!("the model service answered HTTP {} ({ty})", up.status),
                true,
            );
            r.request_id = up.request_id;
            return r;
        }
        if checked.stream && ep == Endpoint::Messages {
            return Reply {
                status: up.status,
                content_type: "text/event-stream",
                request_id: up.request_id,
                retry: true,
                body: Body::Stream(Box::new(SseTap {
                    inner: up.body,
                    line: Vec::new(),
                    rec,
                    proxy: inner.clone(),
                    _slot: slot,
                })),
            };
        }
        let mut buf = Vec::new();
        let read = up.body.take(MAX_JSON_RESPONSE + 1).read_to_end(&mut buf);
        if read.is_err() || buf.len() as u64 > MAX_JSON_RESPONSE {
            rec.status = 502;
            inner.finish(&rec);
            return Reply::error(
                502,
                "api_error",
                "the model service's answer could not be read",
                true,
            );
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
            match ep {
                Endpoint::Messages => {
                    if let Some(u) = v.get("usage") {
                        rec.usage.merge(u);
                    }
                }
                Endpoint::CountTokens => rec.usage.merge(&v),
            }
        }
        rec.complete = true;
        inner.finish(&rec);
        drop(slot);
        Reply {
            status: up.status,
            content_type: "application/json",
            request_id: up.request_id,
            retry: true,
            body: Body::Bytes(buf),
        }
    }
}

fn prune(q: &mut VecDeque<(Instant, u64)>, now: Instant) {
    while q
        .front()
        .is_some_and(|(t, _)| now.duration_since(*t) >= Duration::from_secs(3600))
    {
        q.pop_front();
    }
}

/// One call, for the budget and the log.
struct Record {
    endpoint: Endpoint,
    model: String,
    status: u16,
    usage: Usage,
    request_id: Option<String>,
    started: Instant,
    /// The answer was read to its end (a stream the client dropped isn't).
    complete: bool,
    content: Option<String>,
}

impl Inner {
    fn finish(&self, r: &Record) {
        if r.endpoint == Endpoint::Messages && r.usage.total() > 0 {
            let mut q = self.used.lock().unwrap_or_else(|p| p.into_inner());
            q.push_back((Instant::now(), r.usage.total()));
        }
        if let Some(sink) = &self.log {
            write_line(sink, &log_line(r, SystemTime::now()));
        }
    }
}

/// The log line of a call (no prompt text unless `content` is set).
fn log_line(r: &Record, now: SystemTime) -> String {
    let mut o = Map::new();
    o.insert("ts".into(), json!(rfc3339(now)));
    o.insert("kind".into(), json!("assist"));
    o.insert("endpoint".into(), json!(r.endpoint.name()));
    o.insert("model".into(), json!(r.model));
    o.insert("status".into(), json!(r.status));
    o.insert("input_tokens".into(), json!(r.usage.input));
    o.insert("cache_read".into(), json!(r.usage.cache_read));
    o.insert("cache_write".into(), json!(r.usage.cache_write));
    o.insert("output_tokens".into(), json!(r.usage.output));
    o.insert("request_id".into(), json!(r.request_id));
    o.insert("ms".into(), json!(r.started.elapsed().as_millis() as u64));
    o.insert("complete".into(), json!(r.complete));
    if let Some(c) = &r.content {
        o.insert("request".into(), json!(c));
    }
    Value::Object(o).to_string()
}

fn write_line(sink: &LogSink, line: &str) {
    match sink {
        LogSink::Stderr => eprintln!("{line}"),
        LogSink::File(p) => {
            let r = open_log(p).and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
            if let Err(e) = r {
                eprintln!("[anofox-serve] assist log {}: {e}", p.display());
            }
        }
    }
}

/// `stderr` or a file path (created `0600`, a symlink refused).
pub fn log_sink(spec: &str) -> Result<LogSink, String> {
    match spec.trim() {
        "" => Err("assist log must be \"stderr\" or a file path".into()),
        "stderr" | "-" => Ok(LogSink::Stderr),
        p => {
            let p = PathBuf::from(p);
            open_log(&p).map_err(|e| format!("assist log: cannot open {}: {e}", p.display()))?;
            Ok(LogSink::File(p))
        }
    }
}

/// The upstream event stream, passed through as it is read; on the way it
/// reads the `usage` of `message_start` / `message_delta` for the budget
/// and the log, written when the stream ends or the client goes away.
struct SseTap {
    inner: Box<dyn Read + Send>,
    line: Vec<u8>,
    rec: Record,
    proxy: Arc<Inner>,
    _slot: Slot,
}

/// A data line longer than this isn't parsed (a usage event is short).
const MAX_SSE_LINE: usize = 64 << 10;

impl SseTap {
    fn scan(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' {
                self.event_line();
                self.line.clear();
            } else if self.line.len() < MAX_SSE_LINE {
                self.line.push(b);
            }
        }
    }
    fn event_line(&mut self) {
        let Some(data) = self.line.strip_prefix(b"data:") else {
            return;
        };
        let Ok(v) = serde_json::from_slice::<Value>(data.trim_ascii()) else {
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(u) = v.pointer("/message/usage") {
                    self.rec.usage.merge(u);
                }
            }
            Some("message_delta") => {
                if let Some(u) = v.get("usage") {
                    self.rec.usage.merge(u);
                }
            }
            Some("message_stop") => self.rec.complete = true,
            _ => {}
        }
    }
}

impl Read for SseTap {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.scan(&buf[..n]);
        Ok(n)
    }
}

impl Drop for SseTap {
    fn drop(&mut self) {
        self.proxy.finish(&self.rec);
    }
}

// ------------------------------------------------------------ the writer ---

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        529 => "Overloaded",
        _ => "Status",
    }
}

/// Write `r` as an HTTP/1.1 response on the raw connection
/// (`tiny_http::Request::into_writer`), then close it. A stream goes out
/// chunked, flushed after every read, so each event reaches the browser as
/// it comes; when the client goes away the write fails, the upstream
/// connection is dropped and the model stops.
pub fn write_http(w: &mut dyn Write, r: Reply) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        r.status,
        reason(r.status),
        r.content_type
    );
    if let Some(id) = r.request_id.as_deref().filter(|s| valid_request_id(s)) {
        head.push_str(&format!("request-id: {id}\r\n"));
    }
    if !r.retry {
        head.push_str("x-should-retry: false\r\n");
    }
    match r.body {
        Body::Bytes(b) => {
            head.push_str(&format!("Content-Length: {}\r\n\r\n", b.len()));
            w.write_all(head.as_bytes())?;
            w.write_all(&b)?;
            w.flush()
        }
        Body::Stream(mut rd) => {
            head.push_str("Transfer-Encoding: chunked\r\n\r\n");
            w.write_all(head.as_bytes())?;
            w.flush()?;
            let mut buf = vec![0u8; 16 << 10];
            loop {
                let n = match rd.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    // Upstream broke off: end the chunked body cleanly, the
                    // client sees a stream without `message_stop`.
                    Err(_) => break,
                };
                write!(w, "{n:x}\r\n")?;
                w.write_all(&buf[..n])?;
                w.write_all(b"\r\n")?;
                w.flush()?;
            }
            w.write_all(b"0\r\n\r\n")?;
            w.flush()
        }
    }
}

// ------------------------------------------------------------------ setup ---

/// The `assist_*` options of `anofox_serve(port, options)`, taken out of
/// `options` (the rest goes on to the app bar). `assist: false` turns the
/// proxy off even when a key is set.
#[derive(Debug, Default)]
pub struct AssistOptions {
    pub off: bool,
    pub cfg: AssistConfig,
    pub key_file: Option<PathBuf>,
    pub log: Option<String>,
}

impl AssistOptions {
    /// Remove and parse every `assist` / `assist_*` key of `obj`.
    pub fn take(obj: &mut Map<String, Value>) -> Result<AssistOptions, String> {
        let mut o = AssistOptions::default();
        let keys: Vec<String> = obj
            .keys()
            .filter(|k| *k == "assist" || k.starts_with("assist_"))
            .cloned()
            .collect();
        for k in keys {
            let v = obj.remove(&k).unwrap_or(Value::Null);
            let s = || {
                v.as_str()
                    .map(str::to_string)
                    .ok_or(format!("options.{k} must be a string"))
            };
            let n = || {
                v.as_u64()
                    .filter(|n| *n > 0)
                    .ok_or(format!("options.{k} must be a positive integer"))
            };
            match k.as_str() {
                "assist" => o.off = !v.as_bool().ok_or("options.assist must be true or false")?,
                "assist_models" => match &v {
                    Value::Array(a) => {
                        let list: Option<Vec<&str>> = a.iter().map(Value::as_str).collect();
                        o.cfg.set_models(&list.ok_or("options.assist_models must list strings")?.join(","))?
                    }
                    _ => o.cfg.set_models(&s()?)?,
                },
                "assist_base_url" => o.cfg.set_base_url(&s()?)?,
                "assist_key_file" => o.key_file = Some(PathBuf::from(s()?)),
                "assist_budget_tokens_per_hour" => o.cfg.budget_tokens_per_hour = n()?,
                "assist_max_tokens" => o.cfg.max_tokens = n()?,
                "assist_log" => o.log = Some(s()?),
                "assist_log_content" => {
                    o.cfg.log_content = v.as_bool().ok_or("options.assist_log_content must be true or false")?
                }
                _ => {
                    return Err(format!(
                        "options.{k}: unknown (assist, assist_models, assist_base_url, assist_key_file, \
                         assist_budget_tokens_per_hour, assist_max_tokens, assist_log, assist_log_content)"
                    ))
                }
            }
        }
        Ok(o)
    }
}

/// Build the proxy from settings: `Ok(None)` when it is off (switched off,
/// or no key). The message says where the key came from, never the key.
pub fn build(
    off: bool,
    cfg: AssistConfig,
    key_file: Option<&Path>,
    log: Option<&str>,
) -> Result<Option<(Proxy, String)>, String> {
    if off {
        return Ok(None);
    }
    let Some(key) = load_key(key_file)? else {
        return Ok(None);
    };
    let from = match key_file {
        Some(p) => format!("key file {}", p.display()),
        None => KEY_ENV.to_string(),
    };
    let log = log.map(log_sink).transpose()?;
    let upstream = HttpUpstream::new(&cfg.base_url, key)?;
    let note = format!(
        "assistant proxy on at /assist (key from {from}; models {}; {} tokens/hour; upstream {})",
        cfg.models.join(", "),
        cfg.budget_tokens_per_hour,
        cfg.base_url
    );
    Ok(Some((Proxy::new(cfg, Box::new(upstream), log), note)))
}

#[cfg(test)]
mod tests;

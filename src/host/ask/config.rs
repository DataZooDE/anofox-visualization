//! `options.ask` (plan-ai-assistant.md §12.4): the server's Ask settings and
//! the checks a server runs before it starts with Ask on.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::host::assist;

/// The Claude models Ask may use (#846: Haiku 4.5 for its eval, §12.10 — it
/// runs without thinking or effort, which it doesn't take).
pub const MODELS: &[&str] = &["claude-sonnet-5", "claude-opus-5-5", "claude-haiku-4-5"];

/// Does `model` take adaptive thinking and an effort? (Haiku 4.5 doesn't.)
pub fn thinks(model: &str) -> bool {
    !model.starts_with("claude-haiku-4-5")
}
/// The efforts the operator may fix (`output_config.effort`).
const EFFORTS: &[&str] = &["low", "medium", "high"];

pub use super::mask::Mask;

/// Per viewer (the signed-in user), over a sliding hour and day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerLimits {
    pub per_hour: u32,
    pub per_day: u32,
    pub tokens_per_hour: u64,
    pub tokens_per_day: u64,
}

impl Default for ViewerLimits {
    fn default() -> Self {
        ViewerLimits {
            per_hour: 10,
            per_day: 30,
            tokens_per_hour: 200_000,
            tokens_per_day: 600_000,
        }
    }
}

/// `options.ask`. Off unless `enabled`.
#[derive(Debug, Clone, PartialEq)]
pub struct AskConfig {
    pub enabled: bool,
    pub model: String,
    /// The environment variable holding the key (default `ANTHROPIC_API_KEY`).
    pub key_env: String,
    /// Or a key file (0600, no symlink — [`assist::read_key_file`]).
    pub key_file: Option<PathBuf>,
    pub base_url: String,
    /// `"anthropic"` (M2; `"openai"` is M3).
    pub transport: String,
    /// Shown in the viewer notice ("sent to Anthropic (Claude)").
    pub provider_label: String,
    pub per_viewer: ViewerLimits,
    /// Input-equivalent tokens one question may use, checked before each turn.
    pub per_question_tokens: u64,
    pub max_question_chars: usize,
    /// Who may ask: `group:<name>` or `claim:<name>=<value>`; empty = anyone
    /// signed in.
    pub allow: Vec<String>,
    /// Ask without a login (warned at start; security review M1).
    pub anonymous: bool,
    /// The whole server, over a sliding hour.
    pub tokens_per_hour: u64,
    /// Questions at once, server-wide.
    pub concurrency: usize,
    pub mask: Mask,
    pub log_questions: bool,
    pub log_answers: bool,
    /// Ask inside signed embed links.
    pub embed: bool,
    /// Extra operator text for the viewer notice (plain text).
    pub notice: Option<String>,
    /// Fixed by the operator, never by a viewer (M2).
    pub max_tokens: u64,
    pub effort: String,
    /// One deadline per question (H2).
    pub deadline: Duration,
}

impl Default for AskConfig {
    fn default() -> Self {
        AskConfig {
            enabled: false,
            model: MODELS[0].into(),
            key_env: assist::KEY_ENV.into(),
            key_file: None,
            base_url: assist::DEFAULT_BASE_URL.into(),
            transport: "anthropic".into(),
            provider_label: "Anthropic (Claude)".into(),
            per_viewer: ViewerLimits::default(),
            per_question_tokens: 60_000,
            max_question_chars: 2_000,
            allow: Vec::new(),
            anonymous: false,
            tokens_per_hour: 2_000_000,
            concurrency: 4,
            mask: Mask::Auto,
            log_questions: false,
            log_answers: false,
            embed: false,
            notice: None,
            max_tokens: 8_000,
            effort: "medium".into(),
            deadline: Duration::from_secs(60),
        }
    }
}

fn uint(v: &Value, key: &str, max: u64) -> Result<u64, String> {
    match v.as_u64() {
        Some(n) if n >= 1 && n <= max => Ok(n),
        _ => Err(format!("ask.{key}: a whole number 1..={max}")),
    }
}

fn boolean(v: &Value, key: &str) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("ask.{key}: true or false"))
}

fn text(v: &Value, key: &str, max: usize) -> Result<String, String> {
    match v.as_str() {
        Some(s) if !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control) => {
            Ok(s.trim().to_string())
        }
        _ => Err(format!(
            "ask.{key}: a non-empty line of text (at most {max} bytes)"
        )),
    }
}

impl AskConfig {
    /// Parse `options.ask`. Unknown keys are refused, so a typo can't leave
    /// a limit at its default unnoticed.
    pub fn parse(v: &Value) -> Result<AskConfig, String> {
        let obj: &Map<String, Value> = v.as_object().ok_or("ask: an object")?;
        let mut c = AskConfig::default();
        let mut key_env_given = false;
        for (k, v) in obj {
            match k.as_str() {
                "enabled" => c.enabled = boolean(v, k)?,
                // checked below, with the transport
                "model" => c.model = text(v, k, 128)?,
                "key_env" => {
                    key_env_given = true;
                    let e = text(v, k, 64)?;
                    if !e.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                        return Err("ask.key_env: an environment variable name".into());
                    }
                    c.key_env = e;
                }
                "key_file" => c.key_file = Some(PathBuf::from(text(v, k, 4096)?)),
                "base_url" => {
                    let u = text(v, k, 512)?;
                    assist::check_base_url(&u).map_err(|e| format!("ask.base_url: {e}"))?;
                    c.base_url = u.trim_end_matches('/').to_string();
                }
                "transport" => {
                    let t = text(v, k, 32)?;
                    if t != "anthropic" && t != "openai" {
                        return Err("ask.transport: \"anthropic\" or \"openai\" (an OpenAI-compatible endpoint, #846)".into());
                    }
                    c.transport = t;
                }
                "provider_label" => c.provider_label = text(v, k, 80)?,
                "per_viewer" => {
                    let o = v.as_object().ok_or("ask.per_viewer: an object")?;
                    for (pk, pv) in o {
                        let key = format!("per_viewer.{pk}");
                        match pk.as_str() {
                            "per_hour" => c.per_viewer.per_hour = uint(pv, &key, 10_000)? as u32,
                            "per_day" => c.per_viewer.per_day = uint(pv, &key, 100_000)? as u32,
                            "tokens_per_hour" => {
                                c.per_viewer.tokens_per_hour = uint(pv, &key, 1 << 40)?
                            }
                            "tokens_per_day" => {
                                c.per_viewer.tokens_per_day = uint(pv, &key, 1 << 40)?
                            }
                            _ => return Err(format!("ask.per_viewer: unknown key '{pk}'")),
                        }
                    }
                }
                "per_question_tokens" => c.per_question_tokens = uint(v, k, 1_000_000)?,
                "max_question_chars" => c.max_question_chars = uint(v, k, 8_000)? as usize,
                "allow" => {
                    let a = v.as_array().ok_or("ask.allow: a list")?;
                    c.allow = Vec::new();
                    for e in a {
                        let s = text(e, k, 256)?;
                        let ok = match (s.strip_prefix("group:"), s.strip_prefix("claim:")) {
                            (Some(g), _) => !g.is_empty(),
                            (_, Some(cl)) => cl.split_once('=').is_some_and(|(n, _)| !n.is_empty()),
                            _ => false,
                        };
                        if !ok {
                            return Err(format!(
                                "ask.allow: '{s}' — give \"group:<name>\" or \"claim:<name>=<value>\""
                            ));
                        }
                        c.allow.push(s);
                    }
                }
                "anonymous" => c.anonymous = boolean(v, k)?,
                "tokens_per_hour" => c.tokens_per_hour = uint(v, k, 1 << 40)?,
                "concurrency" => c.concurrency = uint(v, k, 64)? as usize,
                "mask" => {
                    c.mask = match v {
                        Value::String(s) if s == "auto" => Mask::Auto,
                        Value::String(s) if s == "off" => Mask::Off,
                        Value::Array(a) => Mask::Columns(
                            a.iter()
                                .map(|e| text(e, "mask", 128))
                                .collect::<Result<_, _>>()?,
                        ),
                        _ => {
                            return Err(
                                "ask.mask: \"auto\", \"off\" or a list of column names".into()
                            )
                        }
                    }
                }
                "log_questions" => c.log_questions = boolean(v, k)?,
                "log_answers" => c.log_answers = boolean(v, k)?,
                "embed" => c.embed = boolean(v, k)?,
                "notice" => {
                    c.notice = match v {
                        Value::Null => None,
                        v => Some(text(v, k, 1_000)?),
                    }
                }
                "max_tokens" => c.max_tokens = uint(v, k, 64_000)?,
                "effort" => {
                    let e = text(v, k, 16)?;
                    if !EFFORTS.contains(&e.as_str()) {
                        return Err(format!("ask.effort: one of {}", EFFORTS.join(", ")));
                    }
                    c.effort = e;
                }
                "timeout_seconds" => c.deadline = Duration::from_secs(uint(v, k, 600)?),
                _ => return Err(format!("ask: unknown key '{k}'")),
            }
        }
        if c.transport == "openai" {
            // #846: an OpenAI-compatible endpoint (a self-hosted model): its
            // own URL and model id; its key, if any, never ANTHROPIC_API_KEY.
            if c.base_url == assist::DEFAULT_BASE_URL {
                return Err("ask.transport \"openai\" needs ask.base_url (the endpoint, e.g. http://localhost:11434/v1)".into());
            }
            if !obj.contains_key("model") {
                return Err(
                    "ask.transport \"openai\" needs ask.model (the endpoint's model id)".into(),
                );
            }
            let ok = c
                .model
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric())
                && c.model.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b':' | b'/')
                });
            if !ok {
                return Err(
                    "ask.model: the endpoint's model id (letters, digits and - . _ : /)".into(),
                );
            }
            if !key_env_given {
                c.key_env = assist::OPENAI_KEY_ENV.into();
            }
            if !obj.contains_key("provider_label") {
                c.provider_label = "the operator's model endpoint".into();
            }
        } else if !MODELS.contains(&c.model.as_str()) {
            return Err(format!("ask.model: one of {}", MODELS.join(", ")));
        }
        Ok(c)
    }

    /// The model API for this config (#846): the Messages API, or an
    /// OpenAI-compatible endpoint behind the Messages shape. `key` from
    /// [`AskConfig::start`].
    pub fn build_upstream(&self, key: Option<String>) -> Result<Box<dyn assist::Upstream>, String> {
        if self.transport == "openai" {
            return Ok(Box::new(assist::OpenAiUpstream::new(&self.base_url, key)?));
        }
        let key = key.ok_or("ask: no model key")?;
        Ok(Box::new(assist::HttpUpstream::new(&self.base_url, key)?))
    }

    /// The checks before a server starts with Ask on (§12.4): a login is
    /// configured (or `anonymous` is explicit), and the key is there. Returns
    /// the key and the start-message lines (the model, the provider, any
    /// warning). Never the key in a message.
    pub fn start(&self, auth_configured: bool) -> Result<(Option<String>, Vec<String>), String> {
        if !self.enabled {
            return Err("ask: not enabled".into());
        }
        if !auth_configured && !self.anonymous {
            return Err(
                "ask: needs a login (\"auth\") — Ask spends the operator's model key, so it is \
                 never open to anyone who can reach the server, a loopback bind behind a proxy \
                 included; set \"ask\": {\"anonymous\": true} only if that is really meant"
                    .into(),
            );
        }
        let key = match &self.key_file {
            Some(p) => Some(assist::read_key_file(p)?),
            None => match std::env::var(&self.key_env) {
                Ok(k) if !k.trim().is_empty() => Some(k.trim().to_string()),
                // #846: a self-hosted model may need no key
                _ if self.transport == "openai" => None,
                _ => {
                    return Err(format!(
                        "ask: no model key — set {} or ask.key_file",
                        self.key_env
                    ))
                }
            },
        };
        if key
            .as_ref()
            .is_some_and(|k| k.len() > 512 || !k.bytes().all(|b| b.is_ascii_graphic()))
        {
            return Err("ask: not a valid API key (printable characters, no spaces)".into());
        }
        let mut msg = vec![format!(
            "Ask on: model {} via {} ({})",
            self.model, self.provider_label, self.base_url
        )];
        if !auth_configured {
            msg.push("WARNING: Ask is open without a login (ask.anonymous) — anyone who reaches the server spends the model key".into());
        }
        if self.mask == Mask::Off {
            msg.push(
                "Ask masking is off (ask.mask): panel values go to the provider as they are".into(),
            );
        }
        if self.log_questions {
            msg.push("Ask logs question text (ask.log_questions); viewers are told".into());
        }
        Ok((key, msg))
    }
}

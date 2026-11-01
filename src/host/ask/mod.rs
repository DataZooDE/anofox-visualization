//! Ask (#195, plan-ai-assistant.md §12): answer a viewer's questions about
//! the dashboard they are looking at, on the server, from the panel results
//! **this viewer** may already fetch. M2-3 is this library; the locked
//! server's `POST /api/ask` (M2-4) and the static render server (M2-6) call
//! [`Ask::ask`].
//!
//! - **No HTTP in here.** Panel access goes through a [`PanelRunner`]: on the
//!   locked server it hands the `/api/panel` body Ask builds straight to
//!   `locked_panel` with the asking viewer's claims and scope, so the tool
//!   *is* the panel path (policies, claims, `@vars`, hidden tabs, slots,
//!   cache), not a copy of it. The body is checked here with the same
//!   [`parse_panel_request`] first, and always names the conversation's
//!   dashboard (I3).
//! - **No SQL** from the client or the model: tools are `list_panels`,
//!   `get_panel` (a panel index, vars, a page) and `answer`.
//! - **Static prefix, per-viewer tail** (H1): the system prompt holds the
//!   instructions, the role note and the catalog — source text only, the
//!   same bytes for every viewer, cached. What is excluded for this viewer,
//!   the view state and the question come after the cache breakpoint.
//! - **Variables** (M3): `@ask on` needs `-- @vars`; the model may set only
//!   variables the dashboard's inputs bind, and an option input only to one
//!   of its options (read as this viewer).
//! - **Limits** ([`limits`], H2 / M2), **masking** ([`mask`], M6),
//!   **citations** ([`cite`], M4), **conversations** ([`store`], L1).
//! - **Failure modes** (L4): a model that stops without `answer`, a refusal,
//!   an upstream error, a timeout or a cancel gives a fixed message; upstream
//!   bodies never reach the viewer or the log.
//! - **The log**: one JSON line per question with the panels read, the
//!   outcome, the model, the request ids and the token counts — no question
//!   text (unless `log_questions`), no answer text (unless `log_answers`),
//!   no panel data, and tool vars by name only.
//!
//! D2 (#746): a `get_panel` call passes the question's remaining time as the
//! runner's deadline; the locked server's runner bounds its panel request
//! (a `-- @materialize` build included) by it and by Ask's share of the
//! query slots.

pub mod catalog;
pub mod cite;
pub mod config;
pub mod limits;
pub mod mask;
pub mod store;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::io::Read as _;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};

use crate::host::access::{rfc3339, LogSink};
use crate::host::assist::{Upstream, Usage};
use crate::host::serving::{parse_dashboard_meta, parse_panel_request, DashboardMeta};

pub use catalog::Catalog;
pub use cite::Citation;
pub use config::{AskConfig, Mask};

/// The system instructions (shared with the builder's Ask, M2-5).
pub const INSTRUCTIONS: &str = include_str!("../../../web/assist/ask-instructions.md");

/// Model turns per question.
pub const MAX_MODEL_TURNS: usize = 6;
/// `get_panel` calls per question.
pub const MAX_PANEL_CALLS: usize = 8;
/// Rows of one result sent to the model.
pub const MAX_ROWS: usize = 200;
/// JSON bytes of one result sent to the model.
pub const MAX_RESULT_BYTES: usize = 16 << 10;
/// Panel data bytes in one question.
pub const MAX_QUESTION_DATA: usize = 64 << 10;
pub const MAX_ANSWER_CHARS: usize = 1_200;
/// Largest model answer read.
const MAX_RESPONSE: u64 = 4 << 20;
/// Retries on 429 / 529 (counted against the limits).
const RETRIES: usize = 2;

/// What the viewer reads when the model gives no proper answer (L4).
pub const COULDNT: &str =
    "I couldn't answer that. Try asking another way, or look at the panels directly.";

/// The panel path, for the asking viewer.
pub trait PanelRunner {
    /// Run a `POST /api/panel` body (already checked by
    /// [`parse_panel_request`]) for the asking viewer, finishing by
    /// `deadline`; the same response body `/api/panel` gives, or its status
    /// and error.
    fn panel(&self, body: &str, deadline: Instant) -> Result<String, (u16, String)>;
    /// Panels hidden from this viewer right now (a `::TAB` whose
    /// `-- @requires` is not met).
    fn hidden(&self) -> Vec<usize>;
}

/// Who asks.
#[derive(Debug, Clone, Default)]
pub struct Viewer {
    /// The login or page session.
    pub session: String,
    /// `Claims::key()`.
    pub claims_key: String,
    /// Limits are per user.
    pub user: String,
    pub groups: Vec<String>,
    pub claims: Vec<(String, String)>,
    /// An API key: refused (Ask is for people).
    pub api_key: bool,
    /// A signed embed link: the dashboard it names.
    pub embed: Option<String>,
}

/// The dashboard asked about, as the server holds it now.
pub struct Dash<'a> {
    pub id: &'a str,
    pub script: &'a str,
    /// The snapshot's epoch (a refresh starts a fresh conversation).
    pub epoch: u64,
}

/// What the client sends: never a history or SQL.
#[derive(Debug, Clone, Default)]
pub struct AskRequest {
    pub conversation: Option<String>,
    pub question: String,
    /// The open tab's label.
    pub tab: Option<String>,
    /// The view state (the `vars` object of `/api/panel`).
    pub vars: Map<String, Value>,
    /// "Ask about this panel".
    pub panel: Option<usize>,
}

/// An answer.
#[derive(Debug, Clone)]
pub struct AskAnswer {
    pub conversation: String,
    /// Plain text (the client renders it with `renderAnswer`).
    pub text: String,
    /// `answered` | `partly` | `not_on_dashboard` | `limit` | `error`.
    pub scope: String,
    pub citations: Vec<Citation>,
    /// The panels read for it.
    pub panels: Vec<usize>,
    /// The conversation started over: the data was refreshed.
    pub refreshed: bool,
    /// The viewer's first question this session (the caller writes one
    /// `audit: "ask"` event).
    pub first_in_session: bool,
}

impl AskAnswer {
    /// The reply body for the client.
    pub fn to_json(&self) -> Value {
        json!({
            "conversation": self.conversation,
            "text": self.text,
            "scope": self.scope,
            "citations": self.citations.iter().map(Citation::to_json).collect::<Vec<_>>(),
            "panels": self.panels,
            "refreshed": self.refreshed,
        })
    }
}

/// A refusal before any model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskError {
    pub status: u16,
    pub message: String,
}

fn err(status: u16, message: impl Into<String>) -> AskError {
    AskError {
        status,
        message: message.into(),
    }
}

/// Progress for the viewer's status line.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Thinking { turn: usize },
    Reading { panel: usize },
}

/// One question's bookkeeping for the log.
#[derive(Default)]
struct Run {
    usage: Usage,
    request_ids: Vec<String>,
    panels: Vec<usize>,
    calls: usize,
    tool_vars: Vec<String>,
    data_bytes: usize,
    turns: usize,
}

/// The tools (fixed order, part of the cached prefix).
fn tools() -> Value {
    let str_ = |d: &str| json!({"type": "string", "description": d});
    let pair = |d: &str| {
        json!({"type": "array", "description": d, "items": {
            "type": "object",
            "properties": {"name": {"type": "string"}, "value": {"type": "string"}},
            "required": ["name", "value"], "additionalProperties": false}})
    };
    json!([
        {
            "name": "list_panels",
            "description": "List the dashboard's panels (the catalog, no data), optionally only one tab's. The catalog is in your instructions already; use this to see which panels are available to you now.",
            "input_schema": {"type": "object", "properties": {"tab": str_("A tab's label, or an empty string for all")}, "required": ["tab"], "additionalProperties": false},
            "strict": true
        },
        {
            "name": "get_panel",
            "description": format!("Read one panel's result as the viewer sees it: its columns and rows (at most {MAX_ROWS} rows), and the row count. Without vars it uses the viewer's current inputs. vars may set only a variable one of the dashboard's inputs sets, and an option input only to one of its options. page (paged tables only) sorts, filters or skips rows."),
            "input_schema": {
                "type": "object",
                "properties": {
                    "panel": {"type": "integer", "description": "The panel's index in the catalog"},
                    "vars": pair("Variables to set for this read: [{name, value}]"),
                    "page": {"type": "object", "description": "For a paged table", "properties": {
                        "offset": {"type": "integer"}, "sort": str_("A column to sort by"),
                        "desc": {"type": "boolean"}, "filter": str_("Text to look for in a row")},
                        "required": [], "additionalProperties": false}
                },
                "required": ["panel"],
                "additionalProperties": false
            },
            "strict": true
        },
        {
            "name": "answer",
            "description": format!("Give the viewer your answer; this ends your turn. text: plain language, at most {MAX_ANSWER_CHARS} characters. citations: the values the answer rests on, each with its panel index, the column, the value as the panel shows it, and the key cells of its row. scope: answered, partly, or not_on_dashboard."),
            "input_schema": {
                "type": "object",
                "properties": {
                    "text": str_("The answer"),
                    "citations": {"type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "panel": {"type": "integer"},
                            "label": str_("A short label, e.g. the row's key"),
                            "column": str_("The cited column"),
                            "value": str_("The cited value, as the panel shows it"),
                            "row": pair("The key cells of the cited row: [{name: column, value}]")
                        },
                        "required": ["panel", "label"],
                        "additionalProperties": false}},
                    "scope": {"type": "string", "enum": ["answered", "partly", "not_on_dashboard"]}
                },
                "required": ["text", "citations", "scope"],
                "additionalProperties": false
            },
            "strict": true
        }
    ])
}

/// The display name of a result key (`c3` → the plan's name for column 3).
fn display(script_names: &[(usize, String)], key: &str) -> String {
    key.strip_prefix('c')
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| {
            script_names
                .iter()
                .find(|(j, _)| *j == n)
                .map(|(_, s)| s.clone())
        })
        .unwrap_or_else(|| key.to_string())
}

/// `-- @ask mask col,…` of statement `i`.
fn mask_marks(script: &str, i: usize) -> Vec<String> {
    crate::sql::ask::raw_statements(script)
        .get(i)
        .map(|raw| {
            crate::sql::ask::marks(raw)
                .into_iter()
                .filter(|(w, _)| w == "mask")
                .flat_map(|(_, rest)| {
                    rest.split(',')
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Is the dashboard's header `-- @ask on`?
pub fn ask_on(script: &str) -> bool {
    crate::sql::ask::header_on(script)
}

/// A short, masked, single-line error for a tool result.
fn short_err(m: &mask::Masker, e: &str) -> String {
    let line = e.lines().next().unwrap_or("").trim();
    let line: String = line.chars().take(200).collect();
    m.mask_text(&line)
}

/// The text of a variable value as the panel request takes it.
fn var_json(spec: &catalog::InputSpec, v: &str) -> Value {
    use crate::InputKind as K;
    match spec.kind {
        K::Multiselect => json!([v]),
        K::Number => v
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| json!(v), Value::Number),
        K::Toggle => json!(matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        )),
        _ => json!(v),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

pub struct Ask {
    cfg: AskConfig,
    upstream: Box<dyn Upstream>,
    store: store::Store,
    limits: limits::Limits,
    log: Option<LogSink>,
    sessions: Mutex<HashSet<String>>,
}

impl Ask {
    pub fn new(cfg: AskConfig, upstream: Box<dyn Upstream>, log: Option<LogSink>) -> Ask {
        let limits = limits::Limits::new(&cfg);
        Ask {
            cfg,
            upstream,
            store: store::Store::new(),
            limits,
            log,
            sessions: Mutex::new(HashSet::new()),
        }
    }

    pub fn config(&self) -> &AskConfig {
        &self.cfg
    }

    pub fn store(&self) -> &store::Store {
        &self.store
    }

    /// `ask.allow`: may this viewer ask at all?
    pub fn allowed(&self, v: &Viewer) -> bool {
        self.cfg.allow.is_empty()
            || self.cfg.allow.iter().any(|a| {
                if let Some(g) = a.strip_prefix("group:") {
                    v.groups.iter().any(|x| x == g)
                } else if let Some((n, val)) =
                    a.strip_prefix("claim:").and_then(|c| c.split_once('='))
                {
                    v.claims.iter().any(|(k, x)| k == n && x == val)
                } else {
                    false
                }
            })
    }

    /// The system prompt: instructions, role note, catalog — the shared,
    /// cached prefix (the same bytes for every viewer of a dashboard).
    pub fn system(&self, cat: &Catalog) -> Value {
        json!([
            {"type": "text", "text": INSTRUCTIONS.trim()},
            {"type": "text", "text": format!("{}\n{}", cat.role_note(), cat.text()),
             "cache_control": {"type": "ephemeral"}}
        ])
    }

    /// Answer `req` about `dash` for `viewer`.
    pub fn ask(
        &self,
        viewer: &Viewer,
        dash: &Dash,
        req: AskRequest,
        runner: &dyn PanelRunner,
        progress: &mut dyn FnMut(Progress),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AskAnswer, AskError> {
        let started = Instant::now();
        let deadline = started + self.cfg.deadline;
        let meta = parse_dashboard_meta(dash.id, dash.script);
        if !self.cfg.enabled || !ask_on(dash.script) {
            return Err(err(404, "Ask is not on for this dashboard"));
        }
        let Some(allowed_vars) = meta.vars.clone() else {
            // M3: `@ask on` needs `-- @vars` (the lint says so too).
            return Err(err(
                404,
                "Ask is not on for this dashboard (it needs -- @vars)",
            ));
        };
        if viewer.api_key {
            return Err(err(403, "forbidden: Ask is for people, not API keys"));
        }
        if let Some(d) = &viewer.embed {
            if !self.cfg.embed || d != dash.id {
                return Err(err(403, "forbidden: Ask is not on in embedded dashboards"));
            }
        }
        if !self.allowed(viewer) {
            return Err(err(403, "forbidden: you may not use the assistant"));
        }
        let question = req.question.trim().to_string();
        if question.is_empty() {
            return Err(err(400, "ask a question"));
        }
        if question.chars().count() > self.cfg.max_question_chars {
            return Err(err(
                400,
                format!(
                    "the question is too long (at most {} characters)",
                    self.cfg.max_question_chars
                ),
            ));
        }
        // The view state: validated exactly like a panel request's vars.
        let view_vars = req.vars.clone();
        for name in view_vars.keys() {
            if !allowed_vars.iter().any(|a| a.eq_ignore_ascii_case(name)) {
                return Err(err(400, format!("variable '{name}' can't be set here")));
            }
        }
        let probe = json!({"dashboard": dash.id, "panel": 0, "vars": view_vars}).to_string();
        parse_panel_request(&probe).map_err(|e| err(400, e))?;

        let ticket = self.limits.begin(&viewer.user).map_err(|o| err(429, o.0))?;
        let owner = store::Owner {
            session: viewer.session.clone(),
            claims: viewer.claims_key.clone(),
        };
        let taken = self
            .store
            .take(req.conversation.as_deref(), &owner, dash.id, dash.epoch);
        let mut conv = taken.conv;
        let first_in_session = {
            let mut seen = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            // Bounded: at worst a session is audited twice.
            if seen.len() >= 20_000 {
                seen.clear();
            }
            seen.insert(viewer.session.clone())
        };

        let cat = Catalog::build(dash.script, &meta);
        let mut excluded: Vec<usize> = crate::sql::ask::exclusions(dash.script)
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        excluded.extend(runner.hidden());
        excluded.sort_unstable();
        excluded.dedup();

        let mut run = Run::default();
        let ctx = Ctx {
            dash,
            meta: &meta,
            cat: &cat,
            excluded: &excluded,
            view_vars: &view_vars,
            allowed_vars: &allowed_vars,
            runner,
            deadline,
        };
        let user_msg = self.view_message(&ctx, &req, &question, taken.refreshed, &conv);
        let mut turn_msgs: Vec<Value> = Vec::new();
        let mut messages = conv.history();
        push_user(&mut messages, user_msg.clone());
        turn_msgs.push(json!({"role": "user", "content": user_msg}));

        let outcome = self.run_loop(
            &ctx,
            viewer,
            &mut conv,
            &mut messages,
            &mut turn_msgs,
            &mut run,
            progress,
            cancelled,
        );
        drop(ticket);
        let (answer, complete) = match outcome {
            Loop::Answered(a) => (a, true),
            Loop::Stopped(scope, text) => (
                Answered {
                    text,
                    scope,
                    citations: Vec::new(),
                },
                false,
            ),
        };
        if complete {
            let bytes = turn_msgs.iter().map(|m| m.to_string().len()).sum();
            conv.turns.push(store::Turn {
                messages: turn_msgs,
                bytes,
            });
        }
        let id = conv.id.clone();
        self.log_line(
            viewer,
            dash.id,
            &id,
            conv.turns.len(),
            &question,
            &answer,
            &run,
            started,
        );
        self.store.put(conv);
        Ok(AskAnswer {
            conversation: id,
            text: answer.text,
            scope: answer.scope,
            citations: answer.citations,
            panels: run.panels,
            refreshed: taken.refreshed,
            first_in_session,
        })
    }

    /// The per-viewer part of the question (after the cache breakpoint).
    fn view_message(
        &self,
        ctx: &Ctx,
        req: &AskRequest,
        question: &str,
        refreshed: bool,
        conv: &store::Conversation,
    ) -> Vec<Value> {
        let m = &conv.masker;
        let mut s = String::from("<view>\n");
        if refreshed {
            s.push_str("The data was refreshed since the last question: earlier answers may be out of date.\n");
        }
        if let Some(tab) = &req.tab {
            let on_tab: Vec<String> = ctx
                .cat
                .entries
                .iter()
                .filter(|e| e.tab.as_deref() == Some(tab.as_str()))
                .filter(|e| !ctx.excluded.contains(&e.index))
                .map(|e| e.index.to_string())
                .collect();
            s.push_str(&format!(
                "Open tab: {} (panels {})\n",
                m.mask_text(tab),
                if on_tab.is_empty() {
                    "—".into()
                } else {
                    on_tab.join(", ")
                }
            ));
        }
        if !ctx.view_vars.is_empty() {
            let vs: Vec<String> = ctx
                .view_vars
                .iter()
                .map(|(k, v)| format!("{k} = {}", m.mask_text(&text_of(v))))
                .collect();
            s.push_str(&format!("Inputs: {}\n", vs.join(", ")));
        }
        if let Some(p) = req.panel {
            s.push_str(&format!("The viewer asks about panel {p}.\n"));
        }
        let gone: Vec<String> = ctx
            .cat
            .entries
            .iter()
            .filter(|e| ctx.excluded.contains(&e.index))
            .map(|e| e.index.to_string())
            .collect();
        if !gone.is_empty() {
            s.push_str(&format!(
                "Not available to you: panels {} (hidden for this viewer or kept out of the assistant).\n",
                gone.join(", ")
            ));
        }
        s.push_str("</view>\n\n");
        s.push_str(&format!("Question: {}", m.mask_text(question)));
        vec![json!({"type": "text", "text": s})]
    }

    #[allow(clippy::too_many_arguments)]
    fn run_loop(
        &self,
        ctx: &Ctx,
        viewer: &Viewer,
        conv: &mut store::Conversation,
        messages: &mut Vec<Value>,
        turn_msgs: &mut Vec<Value>,
        run: &mut Run,
        progress: &mut dyn FnMut(Progress),
        cancelled: &dyn Fn() -> bool,
    ) -> Loop {
        let system = self.system(ctx.cat);
        let tools = tools();
        let mut spent: u64 = 0;
        for turn in 1..=MAX_MODEL_TURNS {
            if cancelled() {
                return Loop::Stopped("error".into(), "Cancelled.".into());
            }
            if Instant::now() >= ctx.deadline {
                return Loop::Stopped("error".into(), COULDNT.into());
            }
            run.turns = turn;
            progress(Progress::Thinking { turn });
            let body = json!({
                "model": self.cfg.model,
                "max_tokens": self.cfg.max_tokens,
                "system": system,
                "tools": tools,
                "messages": messages,
                "thinking": {"type": "adaptive"},
                "output_config": {"effort": self.cfg.effort},
                "cache_control": {"type": "ephemeral"},
            });
            let bytes = serde_json::to_vec(&body).unwrap_or_default();
            // Input-equivalent tokens: about 3 bytes each, plus the output.
            let est = bytes.len() as u64 / 3 + self.cfg.max_tokens;
            if spent + est > self.cfg.per_question_tokens {
                return Loop::Stopped(
                    "limit".into(),
                    "This question needs more than the assistant may use for one question — ask something narrower.".into(),
                );
            }
            let msg = match self.call(viewer, &bytes, est, ctx.deadline, run, cancelled) {
                Ok(m) => m,
                Err(Stop(scope, text)) => return Loop::Stopped(scope, text),
            };
            let u = Usage::from_value(msg.get("usage").unwrap_or(&Value::Null));
            spent += u.total();
            let content = msg.get("content").cloned().unwrap_or(json!([]));
            let assistant = json!({"role": "assistant", "content": content});
            messages.push(assistant.clone());
            turn_msgs.push(assistant);
            let uses: Vec<&Value> = content
                .as_array()
                .map(|a| a.iter().filter(|b| b["type"] == "tool_use").collect())
                .unwrap_or_default();
            if uses.is_empty() {
                // end_turn with text, max_tokens, a refusal (L4)
                return Loop::Stopped("error".into(), COULDNT.into());
            }
            let mut results = Vec::new();
            let mut answered = None;
            for u in uses {
                let id = u["id"].as_str().unwrap_or("").to_string();
                let input = &u["input"];
                let (text, is_error) = match u["name"].as_str().unwrap_or("") {
                    "answer" => match self.answer(input, conv) {
                        Ok(a) => {
                            answered = Some(a);
                            ("Delivered to the viewer.".to_string(), false)
                        }
                        Err(e) => (e, true),
                    },
                    "list_panels" => (self.list_panels(ctx, input), false),
                    "get_panel" => match self.get_panel(ctx, conv, input, run, progress) {
                        Ok(t) => (t, false),
                        Err(e) => (e, true),
                    },
                    other => (format!("no tool named '{other}'"), true),
                };
                results.push(json!({"type": "tool_result", "tool_use_id": id,
                                    "content": text, "is_error": is_error}));
            }
            let user = json!({"role": "user", "content": results});
            messages.push(user.clone());
            turn_msgs.push(user);
            if let Some(a) = answered {
                return Loop::Answered(a);
            }
        }
        Loop::Stopped("error".into(), COULDNT.into())
    }

    /// One model call, with bounded retries on 429 / 529 (each counted).
    fn call(
        &self,
        viewer: &Viewer,
        body: &[u8],
        est: u64,
        deadline: Instant,
        run: &mut Run,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Value, Stop> {
        for attempt in 0..=RETRIES {
            self.limits
                .reserve(&viewer.user, est)
                .map_err(|o| Stop("limit".into(), o.0))?;
            let resp = match self.upstream.post("/v1/messages", body, false) {
                Ok(r) => r,
                Err(e) => {
                    self.limits.reconcile(&viewer.user, est, 0);
                    eprintln!(
                        "[anofox-serve] ask: model call failed: {}",
                        e.lines().next().unwrap_or("")
                    );
                    return Err(Stop("error".into(), COULDNT.into()));
                }
            };
            if let Some(id) = &resp.request_id {
                run.request_ids.push(id.clone());
            }
            if matches!(resp.status, 429 | 529) && attempt < RETRIES {
                // The reservation stays booked: a retry counts (L4).
                let wait = Duration::from_millis(500 << attempt);
                if Instant::now() + wait >= deadline || cancelled() {
                    break;
                }
                std::thread::sleep(wait);
                continue;
            }
            if resp.status != 200 {
                self.limits.reconcile(&viewer.user, est, 0);
                return Err(Stop("error".into(), COULDNT.into()));
            }
            let mut buf = Vec::new();
            if resp.body.take(MAX_RESPONSE).read_to_end(&mut buf).is_err() {
                self.limits.reconcile(&viewer.user, est, est);
                return Err(Stop("error".into(), COULDNT.into()));
            }
            let msg: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
            let u = Usage::from_value(msg.get("usage").unwrap_or(&Value::Null));
            self.limits.reconcile(&viewer.user, est, u.total());
            run.usage.add(&u);
            if !msg.is_object() {
                return Err(Stop("error".into(), COULDNT.into()));
            }
            return Ok(msg);
        }
        Err(Stop("error".into(), COULDNT.into()))
    }

    fn list_panels(&self, ctx: &Ctx, input: &Value) -> String {
        let tab = input["tab"].as_str().unwrap_or("").trim();
        let lines: Vec<String> = ctx
            .cat
            .entries
            .iter()
            .filter(|e| !ctx.excluded.contains(&e.index))
            .filter(|e| tab.is_empty() || e.tab.as_deref() == Some(tab))
            .map(Catalog::entry_line)
            .collect();
        if lines.is_empty() {
            "No panels available.".into()
        } else {
            lines.join("\n")
        }
    }

    /// The options of the input that binds `spec`, as this viewer sees them.
    fn options(&self, ctx: &Ctx, entry: &catalog::Entry) -> Result<Vec<String>, String> {
        let body = json!({"dashboard": ctx.dash.id, "panel": entry.index, "vars": ctx.view_vars})
            .to_string();
        let out = ctx.runner.panel(&body, ctx.deadline).map_err(|(_, e)| e)?;
        let rows: Vec<Map<String, Value>> = parse_rows(&out)?;
        let plan = crate::sql::plan(ctx.dash.script);
        let idx = crate::inputs::input_index(&plan[entry.index].roles);
        let key = format!("c{idx}");
        Ok(rows
            .iter()
            .filter_map(|r| r.get(&key).or_else(|| r.values().nth(idx)))
            .map(text_of)
            .collect())
    }

    fn get_panel(
        &self,
        ctx: &Ctx,
        conv: &mut store::Conversation,
        input: &Value,
        run: &mut Run,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<String, String> {
        if run.calls >= MAX_PANEL_CALLS {
            return Err(format!(
                "The limit of {MAX_PANEL_CALLS} panel reads for this question is reached: answer with what you have."
            ));
        }
        let panel = input["panel"].as_u64().ok_or("panel: give a panel index")? as usize;
        let entry = ctx
            .cat
            .entry(panel)
            .ok_or_else(|| format!("There is no panel {panel}."))?;
        if ctx.excluded.contains(&panel) {
            return Err(format!("Panel {panel} is not available."));
        }
        run.calls += 1;
        // Vars (M3): only what an input binds, and an option input only to
        // one of its options. Tokens become real values first.
        let mut vars = ctx.view_vars.clone();
        let mut real_vars = Map::new();
        for pair in input["vars"].as_array().into_iter().flatten() {
            let name = pair["name"].as_str().unwrap_or("").trim();
            let value = conv
                .masker
                .unmask_text(pair["value"].as_str().unwrap_or(""));
            let Some((ie, spec)) = ctx
                .cat
                .inputs()
                .find(|(_, s)| s.var.eq_ignore_ascii_case(name))
            else {
                return Err(format!("No input on this dashboard sets '{name}'."));
            };
            if !ctx
                .allowed_vars
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&spec.var))
            {
                return Err(format!("'{name}' can't be set here."));
            }
            if ctx.excluded.contains(&ie.index) {
                return Err(format!("'{name}' can't be set here."));
            }
            if spec.has_options() {
                let opts = self
                    .options(ctx, ie)
                    .map_err(|e| short_err(&conv.masker, &e))?;
                if !opts.iter().any(|o| o == &value) {
                    return Err(format!(
                        "That value is not one of the options of '{}'.",
                        spec.var
                    ));
                }
            }
            run.tool_vars.push(spec.var.clone());
            let j = var_json(spec, &value);
            vars.insert(spec.var.clone(), j.clone());
            real_vars.insert(spec.var.clone(), j);
        }
        let plan = crate::sql::plan(ctx.dash.script);
        let names = &catalog::labels(ctx.dash.script, &plan, panel);
        let mut body = json!({"dashboard": ctx.dash.id, "panel": panel, "vars": vars});
        if let Some(p) = input.get("page").filter(|p| p.is_object()) {
            if !entry.kind.starts_with("table") {
                return Err("page applies to tables only.".into());
            }
            let mut page = json!({"limit": MAX_ROWS});
            if let Some(o) = p["offset"].as_u64() {
                page["offset"] = json!(o);
            }
            if let Some(s) = p["sort"].as_str().filter(|s| !s.is_empty()) {
                let key = names
                    .iter()
                    .find(|(_, n)| n.eq_ignore_ascii_case(s))
                    .map_or_else(|| s.to_string(), |(i, _)| format!("c{i}"));
                page["sort"] = json!(key);
                page["desc"] = json!(p["desc"].as_bool().unwrap_or(false));
            }
            if let Some(f) = p["filter"].as_str().filter(|s| !s.is_empty()) {
                page["filter"] = json!(conv.masker.unmask_text(f));
            }
            body["page"] = page;
        }
        let body = body.to_string();
        // The same checks as POST /api/panel, and the dashboard is always
        // this conversation's (I3).
        parse_panel_request(&body).map_err(|e| short_err(&conv.masker, &e))?;
        progress(Progress::Reading { panel });
        let out = ctx
            .runner
            .panel(&body, ctx.deadline)
            .map_err(|(_, e)| format!("Panel {panel} failed: {}", short_err(&conv.masker, &e)))?;
        let rows = parse_rows(&out).map_err(|_| format!("Panel {panel} gave no rows."))?;
        let total = rows.len();
        // Columns by display name, in result order.
        let keys: Vec<String> = rows
            .first()
            .map(|r| r.keys().cloned().collect())
            .unwrap_or_default();
        let cols: Vec<String> = keys.iter().map(|k| display(names, k)).collect();
        let extra = mask_marks(ctx.dash.script, panel);
        let whole: Vec<bool> = cols
            .iter()
            .map(|c| mask::Masker::masks_column(&self.cfg.mask, c, &extra))
            .collect();
        let mut real_rows: Vec<Map<String, Value>> = Vec::new();
        let mut sent_rows: Vec<Value> = Vec::new();
        let mut size = 0usize;
        // A column with one value in every row is sent once.
        let constant: Vec<bool> = keys
            .iter()
            .map(|k| total > 1 && rows.iter().all(|r| r.get(k) == rows[0].get(k)))
            .collect();
        let mut consts = Map::new();
        let masker = &mut conv.masker;
        let mut mask_cell = |i: usize, v: &Value| -> Result<Value, String> {
            match v {
                Value::String(s) => masker
                    .cell(&self.cfg.mask, whole[i], s)
                    .map(Value::String)
                    .map_err(|_| format!("Panel {panel} can't be sent to the assistant.")),
                v => Ok(v.clone()),
            }
        };
        for (i, k) in keys.iter().enumerate() {
            if constant[i] {
                consts.insert(cols[i].clone(), mask_cell(i, &rows[0][k])?);
            }
        }
        for r in rows.iter().take(MAX_ROWS) {
            let mut sent = Map::new();
            let mut real = Map::new();
            for (i, k) in keys.iter().enumerate() {
                let v = r.get(k).cloned().unwrap_or(Value::Null);
                real.insert(cols[i].clone(), v.clone());
                if !constant[i] {
                    sent.insert(cols[i].clone(), mask_cell(i, &v)?);
                }
            }
            let line = serde_json::to_string(&sent).unwrap_or_default();
            if size + line.len() > MAX_RESULT_BYTES {
                break;
            }
            size += line.len();
            sent_rows.push(Value::Object(sent));
            real_rows.push(real);
        }
        if run.data_bytes + size > MAX_QUESTION_DATA {
            return Err(
                "The panel data limit for this question is reached: answer with what you have."
                    .into(),
            );
        }
        run.data_bytes += size;
        if !run.panels.contains(&panel) {
            run.panels.push(panel);
        }
        let shown = sent_rows.len();
        let note = if shown < total {
            format!("first {shown} of {total} rows; sort or filter for others")
        } else {
            format!("{total} rows")
        };
        let masked_vars: Map<String, Value> = real_vars
            .iter()
            .map(|(k, v)| (k.clone(), json!(conv.masker.mask_text(&text_of(v)))))
            .collect();
        let data = json!({
            "panel": panel,
            "columns": cols,
            "constant": consts,
            "rows": sent_rows,
            "rows_note": note,
            "vars": masked_vars,
        });
        conv.reads.push(cite::Read {
            panel,
            vars: real_vars,
            columns: cols,
            rows: real_rows,
        });
        Ok(format!(
            "<panel_data panel=\"{panel}\">\n{}\n</panel_data>",
            serde_json::to_string(&data).unwrap_or_default()
        ))
    }

    /// The model's `answer`: unmasked, citations verified.
    fn answer(&self, input: &Value, conv: &store::Conversation) -> Result<Answered, String> {
        let text = input["text"].as_str().ok_or("answer: give text")?;
        let mut text = conv.masker.unmask_text(text);
        if text.chars().count() > MAX_ANSWER_CHARS {
            text = text.chars().take(MAX_ANSWER_CHARS).collect::<String>() + "…";
        }
        let text = scrub_tokens(&text);
        let scope = match input["scope"].as_str() {
            Some(s @ ("answered" | "partly" | "not_on_dashboard")) => s.to_string(),
            _ => "answered".into(),
        };
        let citations = input["citations"]
            .as_array()
            .into_iter()
            .flatten()
            .take(20)
            .filter_map(|c| {
                let panel = c["panel"].as_u64()? as usize;
                let un = |k: &str| {
                    c[k].as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| conv.masker.unmask_text(s))
                };
                let claim = cite::Claim {
                    panel,
                    label: scrub_tokens(&un("label").unwrap_or_default()),
                    column: un("column"),
                    value: un("value"),
                    row: c["row"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| {
                            Some((
                                p["name"].as_str()?.to_string(),
                                conv.masker.unmask_text(p["value"].as_str()?),
                            ))
                        })
                        .collect(),
                };
                Some(cite::verify(&claim, &conv.reads))
            })
            .collect();
        Ok(Answered {
            text,
            scope,
            citations,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn log_line(
        &self,
        viewer: &Viewer,
        dashboard: &str,
        conversation: &str,
        turn: usize,
        question: &str,
        answer: &Answered,
        run: &Run,
        started: Instant,
    ) {
        let Some(sink) = &self.log else { return };
        let mut o = Map::new();
        o.insert("ts".into(), json!(rfc3339(SystemTime::now())));
        o.insert("path".into(), json!("/api/ask"));
        o.insert("user".into(), json!(viewer.user));
        o.insert("dashboard".into(), json!(dashboard));
        o.insert("conversation".into(), json!(conversation));
        o.insert("turn".into(), json!(turn));
        o.insert("panels".into(), json!(run.panels));
        o.insert("get_panel".into(), json!(run.calls));
        o.insert("model_turns".into(), json!(run.turns));
        o.insert("tool_vars".into(), json!(run.tool_vars));
        o.insert("outcome".into(), json!(answer.scope));
        o.insert(
            "verified".into(),
            json!(answer.citations.iter().filter(|c| c.verified).count()),
        );
        o.insert("citations".into(), json!(answer.citations.len()));
        o.insert("model".into(), json!(self.cfg.model));
        o.insert("request_id".into(), json!(run.request_ids));
        o.insert(
            "tokens".into(),
            json!({"input": run.usage.input, "output": run.usage.output,
                   "cache_read": run.usage.cache_read, "cache_write": run.usage.cache_write}),
        );
        o.insert("ms".into(), json!(started.elapsed().as_millis() as u64));
        if self.cfg.log_questions {
            o.insert("question".into(), json!(question));
        }
        if self.cfg.log_answers {
            o.insert("answer".into(), json!(answer.text));
        }
        crate::host::assist::write_line(sink, &Value::Object(o).to_string());
    }
}

/// What one question's loop works with.
struct Ctx<'a> {
    dash: &'a Dash<'a>,
    #[allow(dead_code)]
    meta: &'a DashboardMeta,
    cat: &'a Catalog,
    excluded: &'a [usize],
    view_vars: &'a Map<String, Value>,
    allowed_vars: &'a [String],
    runner: &'a dyn PanelRunner,
    deadline: Instant,
}

struct Answered {
    text: String,
    scope: String,
    citations: Vec<Citation>,
}

enum Loop {
    Answered(Answered),
    Stopped(String, String),
}

struct Stop(String, String);

/// Append a user turn: into the last message when the history ends with a
/// user message (the previous question's tool results), so roles alternate.
fn push_user(messages: &mut Vec<Value>, content: Vec<Value>) {
    if let Some(last) = messages.last_mut() {
        if last["role"] == "user" {
            if let Some(a) = last["content"].as_array_mut() {
                a.extend(content);
                return;
            }
        }
    }
    messages.push(json!({"role": "user", "content": content}));
}

/// A token the map didn't know must not reach the viewer as a pseudonym.
fn scrub_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(mask::OPEN) {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        match after.find(mask::CLOSE) {
            Some(j) => {
                out.push_str("[hidden]");
                rest = &after[j + mask::CLOSE.len_utf8()..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A panel response's rows (an array of objects, or `{"rows": […]}`).
fn parse_rows(body: &str) -> Result<Vec<Map<String, Value>>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let arr = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .ok_or("no rows")?,
        _ => return Err("no rows".into()),
    };
    Ok(arr
        .into_iter()
        .filter_map(|r| match r {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .collect())
}

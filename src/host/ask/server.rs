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

use std::collections::HashSet;
use std::io::Read as _;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};

use super::core::{self, Answered, PanelReq, Tally};
use super::{limits, store, AskConfig, Catalog, Citation};
use crate::host::access::{rfc3339, LogSink};
use crate::host::assist::{Upstream, Usage};
use crate::host::serving::{parse_dashboard_meta, parse_panel_request, DashboardMeta};

pub use super::core::{
    ask_on, COULDNT, INSTRUCTIONS, MAX_ANSWER_CHARS, MAX_MODEL_TURNS, MAX_PANEL_CALLS,
    MAX_QUESTION_DATA, MAX_RESULT_BYTES, MAX_ROWS,
};

/// Largest model answer read.
const MAX_RESPONSE: u64 = 4 << 20;
/// Retries on 429 / 529 (counted against the limits).
const RETRIES: usize = 2;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

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
    /// The static render server (B, M2-6): the dashboard's `-- @param`s are
    /// the variables a viewer (and `get_panel`) may set, in place of
    /// `-- @vars` and the inputs.
    pub params: Option<&'a [crate::host::serving::Param]>,
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
    tally: Tally,
    turns: usize,
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
        core::system(cat)
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
        let params_vars = dash
            .params
            .map(|ps| ps.iter().map(|p| p.name.clone()).collect::<Vec<_>>());
        let Some(allowed_vars) = params_vars.or_else(|| meta.vars.clone()) else {
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
        // The view state, as a panel request takes it: a reserved name
        // (`__claim_*`, `anofox_data`) or a bad value is refused, a variable
        // outside `-- @vars` is dropped (the page sends its own as well).
        let probe = json!({"dashboard": dash.id, "panel": 0, "vars": req.vars}).to_string();
        parse_panel_request(&probe).map_err(|e| err(400, e))?;
        let view_vars: Map<String, Value> = req
            .vars
            .iter()
            .filter(|(k, _)| allowed_vars.iter().any(|a| a.eq_ignore_ascii_case(k)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

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
        let mut excluded: Vec<usize> = core::skipped(dash.script);
        excluded.extend(runner.hidden());
        excluded.sort_unstable();
        excluded.dedup();

        let mut run = Run::default();
        let ctx = Ctx {
            c: core::Ctx {
                dash_id: dash.id,
                script: dash.script,
                cat: &cat,
                excluded: &excluded,
                view_vars: &view_vars,
                allowed_vars: &allowed_vars,
                mask: &self.cfg.mask,
                params: dash.params,
            },
            meta: &meta,
            runner,
            deadline,
        };
        let user_msg = core::view_message(
            &ctx.c,
            &conv.masker,
            req.tab.as_deref(),
            req.panel,
            &question,
            taken.refreshed,
        );
        let mut turn_msgs: Vec<Value> = Vec::new();
        let mut messages = conv.history();
        core::push_user(&mut messages, user_msg.clone());
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
            panels: run.tally.panels,
            refreshed: taken.refreshed,
            first_in_session,
        })
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
        let system = self.system(ctx.c.cat);
        let tools = core::tools();
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
                    "answer" => match core::answer(&conv.masker, &conv.reads, input) {
                        Ok(a) => {
                            answered = Some(a);
                            ("Delivered to the viewer.".to_string(), false)
                        }
                        Err(e) => (e, true),
                    },
                    "list_panels" => (core::list_panels(&ctx.c, input), false),
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

    /// `get_panel`: [`core::panel_request`], the panel path for this
    /// viewer, then [`core::panel_result`].
    fn get_panel(
        &self,
        ctx: &Ctx,
        conv: &mut store::Conversation,
        input: &Value,
        run: &mut Run,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<String, String> {
        // An option input's options, as this viewer sees them.
        let mut options = |i: usize| -> Result<Vec<String>, String> {
            let body = json!({"dashboard": ctx.c.dash_id, "panel": i, "vars": ctx.c.view_vars})
                .to_string();
            let out = ctx.runner.panel(&body, ctx.deadline).map_err(|(_, e)| e)?;
            core::option_values(ctx.c.script, i, &out)
        };
        let req: PanelReq =
            core::panel_request(&ctx.c, &conv.masker, &mut run.tally, input, &mut options)?;
        let panel = req.panel;
        progress(Progress::Reading { panel });
        let out = ctx
            .runner
            .panel(&req.body.to_string(), ctx.deadline)
            .map_err(|(_, e)| {
                format!(
                    "Panel {panel} failed: {}",
                    core::short_err(&conv.masker, &e)
                )
            })?;
        core::panel_result(
            &ctx.c,
            &mut conv.masker,
            &mut conv.reads,
            &mut run.tally,
            &req,
            &out,
        )
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
        o.insert("panels".into(), json!(run.tally.panels));
        o.insert("get_panel".into(), json!(run.tally.calls));
        o.insert("model_turns".into(), json!(run.turns));
        o.insert("tool_vars".into(), json!(run.tally.tool_vars));
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
    c: core::Ctx<'a>,
    #[allow(dead_code)]
    meta: &'a DashboardMeta,
    runner: &'a dyn PanelRunner,
    deadline: Instant,
}

enum Loop {
    Answered(Answered),
    Stopped(String, String),
}

struct Stop(String, String);

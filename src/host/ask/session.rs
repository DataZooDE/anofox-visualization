//! Ask in the builder (M2-5, plan-ai-assistant.md §12.1 / §12.8): one
//! conversation about the dashboard on the builder's page, held in the page.
//! The loop and the model calls are the browser's (`web/assist.js`, with the
//! assistant's transport); every tool's checks, the masking, the caps and the
//! citation check are [`super::core`]'s, the code the server runs. JSON in,
//! JSON out, so the wasm `AskSession` is a thin wrapper.
//!
//! What differs from a served dashboard, and why:
//! - **No `-- @ask on` needed.** The asker is the author, on their own page;
//!   `@ask on` is the switch for *viewers* of a served dashboard. `@ask skip`
//!   and `-- @ask mask` still apply (the author's own marks).
//! - **Variables.** A `-- @vars` header narrows them as on a server; without
//!   one, the variables the dashboard's inputs bind (what the author can set
//!   on the page anyway).
//! - **Hidden tabs** (an unmet `-- @requires`) come from the page with each
//!   question (`view.hidden`).
//! - **Running a panel** is the page's: [`Session::panel_request`] gives the
//!   checked body, the page runs it (binding vars as typed `SET VARIABLE`
//!   literals) and hands the rows to [`Session::panel_result`].

use serde_json::{json, Map, Value};

use super::catalog::Catalog;
use super::cite::{Citation, Read};
use super::core::{self, Ctx, PanelReq, Tally};
use super::mask::{Mask, Masker};
use crate::host::serving::{parse_dashboard_meta, parse_panel_request};

/// The dashboard id panel bodies carry in the builder.
pub const BUILDER_ID: &str = "builder";
/// Longest question.
pub const MAX_QUESTION_CHARS: usize = 2_000;
/// Panel reads kept for the citation check (the oldest go first).
const MAX_READS: usize = 40;

/// One builder conversation.
pub struct Session {
    script: String,
    cat: Catalog,
    allowed_vars: Vec<String>,
    skipped: Vec<usize>,
    mask: Mask,
    masker: Masker,
    reads: Vec<Read>,
    /// This question's.
    excluded: Vec<usize>,
    view_vars: Map<String, Value>,
    tally: Tally,
    pending: Vec<(u32, PanelReq)>,
    next: u32,
}

fn ok(v: Value) -> String {
    json!({ "ok": v }).to_string()
}

fn fail(e: impl Into<String>) -> String {
    json!({ "error": e.into() }).to_string()
}

fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

impl Session {
    pub fn new(script: &str) -> Session {
        let meta = parse_dashboard_meta(BUILDER_ID, script);
        let cat = Catalog::build(script, &meta);
        let allowed_vars = meta
            .vars
            .clone()
            .unwrap_or_else(|| cat.inputs().map(|(_, s)| s.var.clone()).collect());
        Session {
            script: script.to_string(),
            allowed_vars,
            skipped: core::skipped(script),
            cat,
            mask: Mask::Auto,
            masker: Masker::new(),
            reads: Vec::new(),
            excluded: Vec::new(),
            view_vars: Map::new(),
            tally: Tally::default(),
            pending: Vec::new(),
            next: 0,
        }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            dash_id: BUILDER_ID,
            script: &self.script,
            cat: &self.cat,
            excluded: &self.excluded,
            view_vars: &self.view_vars,
            allowed_vars: &self.allowed_vars,
            mask: &self.mask,
            params: None,
        }
    }

    /// The system prompt (JSON array of text blocks).
    pub fn system(&self) -> String {
        core::system(&self.cat).to_string()
    }

    /// The tool definitions (JSON array).
    pub fn tools(&self) -> String {
        core::tools().to_string()
    }

    /// The limits the loop keeps (JSON).
    pub fn limits(&self) -> String {
        json!({
            "turns": core::MAX_MODEL_TURNS,
            "panel_calls": core::MAX_PANEL_CALLS,
            "rows": core::MAX_ROWS,
            "question_chars": MAX_QUESTION_CHARS,
            "answer_chars": core::MAX_ANSWER_CHARS,
            "couldnt": core::COULDNT,
        })
        .to_string()
    }

    /// *What will be sent*: the catalog's panels, `{index, tab, line,
    /// excluded}` (excluded: hidden in `hidden_json` — `[index, …]`).
    pub fn catalog(&self, hidden_json: &str) -> String {
        let hidden: Vec<usize> = serde_json::from_str(hidden_json).unwrap_or_default();
        let list: Vec<Value> = self
            .cat
            .entries
            .iter()
            .map(|e| {
                json!({
                    "index": e.index,
                    "tab": e.tab,
                    "line": Catalog::entry_line(e),
                    "excluded": hidden.contains(&e.index) || self.skipped.contains(&e.index),
                })
            })
            .collect();
        Value::Array(list).to_string()
    }

    /// A new question: `view_json` = `{tab?, vars?, panel?, hidden?}` (the
    /// page's open tab, its input values as `/api/panel` takes them, the
    /// panel asked about, the panels on hidden tabs). Gives the user turn's
    /// content (`{ok: [blocks]}`) or `{error}`.
    pub fn begin(&mut self, view_json: &str, question: &str) -> String {
        let view = parse(view_json);
        let question = question.trim();
        if question.is_empty() {
            return fail("ask a question");
        }
        if question.chars().count() > MAX_QUESTION_CHARS {
            return fail(format!(
                "the question is too long (at most {MAX_QUESTION_CHARS} characters)"
            ));
        }
        let vars = view["vars"].as_object().cloned().unwrap_or_default();
        // The view state as a panel request takes it: a reserved name or a
        // bad value is refused, a variable not allowed is dropped.
        let probe = json!({"dashboard": BUILDER_ID, "panel": 0, "vars": vars}).to_string();
        if let Err(e) = parse_panel_request(&probe) {
            return fail(e);
        }
        self.view_vars = vars
            .into_iter()
            .filter(|(k, _)| self.allowed_vars.iter().any(|a| a.eq_ignore_ascii_case(k)))
            .collect();
        let mut excluded = self.skipped.clone();
        excluded.extend(
            view["hidden"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_u64().map(|n| n as usize)),
        );
        excluded.sort_unstable();
        excluded.dedup();
        self.excluded = excluded;
        self.tally = Tally::default();
        self.pending.clear();
        let panel = view["panel"].as_u64().map(|n| n as usize);
        let content = core::view_message(
            &self.ctx(),
            &self.masker,
            view["tab"].as_str(),
            panel,
            question,
            false,
        );
        ok(Value::Array(content))
    }

    /// `list_panels` (the tool result text).
    pub fn list_panels(&self, input_json: &str) -> String {
        core::list_panels(&self.ctx(), &parse(input_json))
    }

    /// The input panels whose options the page must run first (`[index, …]`).
    pub fn option_panels(&self, input_json: &str) -> String {
        json!(core::option_panels(&self.ctx(), &parse(input_json))).to_string()
    }

    /// `get_panel`'s checks: `options_json` = `{"<input panel>": rows, …}`
    /// (each the input panel's rows, as the page has them). Gives `{ok: {id,
    /// panel, vars, page?}}` — the read to run, `vars` the complete set —
    /// or `{error}` (the tool result).
    pub fn panel_request(&mut self, input_json: &str, options_json: &str) -> String {
        let input = parse(input_json);
        let options = parse(options_json);
        let script = self.script.clone();
        let mut opts = |i: usize| -> Result<Vec<String>, String> {
            match options.get(i.to_string()) {
                Some(rows) => core::option_values(&script, i, &rows.to_string()),
                None => Err("the input's options are not available".into()),
            }
        };
        let ctx = Ctx {
            dash_id: BUILDER_ID,
            script: &self.script,
            cat: &self.cat,
            excluded: &self.excluded,
            view_vars: &self.view_vars,
            allowed_vars: &self.allowed_vars,
            mask: &self.mask,
            params: None,
        };
        match core::panel_request(&ctx, &self.masker, &mut self.tally, &input, &mut opts) {
            Ok(req) => {
                self.next += 1;
                let id = self.next;
                let mut out = json!({
                    "id": id,
                    "panel": req.panel,
                    "vars": req.body["vars"],
                });
                if let Some(p) = req.body.get("page") {
                    out["page"] = p.clone();
                }
                self.pending.push((id, req));
                ok(out)
            }
            Err(e) => fail(e),
        }
    }

    /// `get_panel`'s result for read `id`: `out` = the panel's rows (a JSON
    /// array of objects, `c0…` keys). `{ok: text}` or `{error}`.
    pub fn panel_result(&mut self, id: u32, out: &str) -> String {
        let Some(at) = self.pending.iter().position(|(i, _)| *i == id) else {
            return fail("no such panel read");
        };
        let (_, req) = self.pending.remove(at);
        let ctx = Ctx {
            dash_id: BUILDER_ID,
            script: &self.script,
            cat: &self.cat,
            excluded: &self.excluded,
            view_vars: &self.view_vars,
            allowed_vars: &self.allowed_vars,
            mask: &self.mask,
            params: None,
        };
        let r = core::panel_result(
            &ctx,
            &mut self.masker,
            &mut self.reads,
            &mut self.tally,
            &req,
            out,
        );
        if self.reads.len() > MAX_READS {
            let drop = self.reads.len() - MAX_READS;
            self.reads.drain(..drop);
        }
        match r {
            Ok(t) => ok(json!(t)),
            Err(e) => fail(e),
        }
    }

    /// A failed panel run, as the tool result says it (masked, one line).
    pub fn panel_error(&mut self, id: u32, error: &str) -> String {
        let panel = self
            .pending
            .iter()
            .position(|(i, _)| *i == id)
            .map(|at| self.pending.remove(at).1.panel);
        let e = core::short_err(&self.masker, error);
        match panel {
            Some(p) => format!("Panel {p} failed: {e}"),
            None => e,
        }
    }

    /// The model's `answer`: `{ok: {text, scope, citations, panels}}` or
    /// `{error}` (the tool result).
    pub fn answer(&self, input_json: &str) -> String {
        match core::answer(&self.masker, &self.reads, &parse(input_json)) {
            Ok(a) => ok(json!({
                "text": a.text,
                "scope": a.scope,
                "citations": a.citations.iter().map(Citation::to_json).collect::<Vec<_>>(),
                "panels": self.tally.panels,
            })),
            Err(e) => fail(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DASH: &str = "-- @title Stock
SELECT region ::DROPDOWN FROM (VALUES ('EU'), ('US')) t(region);
SELECT sku AS \"SKU\" ::TABLE, contact AS \"Contact\", cover AS \"Cover\" FROM stock WHERE region = getvariable('region');
-- @ask skip
SELECT 'secret' AS s ::TABLE, 1 AS v;
";
    // plan indices: 0 input, 1 stock table, 2 skipped

    fn okv(s: &str) -> Value {
        let v: Value = serde_json::from_str(s).unwrap();
        assert!(v.get("error").is_none(), "{s}");
        v["ok"].clone()
    }

    #[test]
    fn begin_narrows_vars_and_refuses_reserved_names() {
        let mut s = Session::new(DASH);
        let v = okv(&s.begin(
            r#"{"vars": {"region": "EU", "other": "x"}}"#,
            "Which SKU is lowest?",
        ));
        let text = v[0]["text"].as_str().unwrap();
        assert!(text.contains("Inputs: region = EU"), "{text}");
        assert!(!text.contains("other"), "{text}");
        let e = s.begin(r#"{"vars": {"__claim_user": "x"}}"#, "q");
        assert!(e.contains("error"), "{e}");
        assert!(s.begin("{}", "  ").contains("error"));
    }

    #[test]
    fn skipped_panel_is_not_in_the_catalog_and_not_readable() {
        let mut s = Session::new(DASH);
        okv(&s.begin("{}", "q"));
        assert!(!s.system().contains("secret"));
        let r = s.panel_request(r#"{"panel": 2}"#, "{}");
        assert!(r.contains("error"), "{r}");
    }

    #[test]
    fn hidden_panels_are_refused() {
        let mut s = Session::new(DASH);
        okv(&s.begin(r#"{"hidden": [1]}"#, "q"));
        let r = s.panel_request(r#"{"panel": 1}"#, "{}");
        assert!(r.contains("not available"), "{r}");
    }

    #[test]
    fn get_panel_masks_and_answer_verifies() {
        let mut s = Session::new(DASH);
        okv(&s.begin(r#"{"vars": {"region": "EU"}}"#, "q"));
        let req = okv(&s.panel_request(r#"{"panel": 1}"#, "{}"));
        assert_eq!(req["vars"]["region"], "EU");
        let id = req["id"].as_u64().unwrap() as u32;
        // role items come back as c{i}, the rest under their own names
        let rows = r#"[{"c0":"SKU-1","Contact":"ann@example.com","Cover":4},{"c0":"SKU-2","Contact":"bob@example.com","Cover":9}]"#;
        let text = okv(&s.panel_result(id, rows));
        let text = text.as_str().unwrap();
        assert!(text.contains("<panel_data panel=\"1\">"), "{text}");
        assert!(
            !text.contains("ann@example.com"),
            "an e-mail reached the model: {text}"
        );
        assert!(text.contains("SKU-1"), "{text}");
        // a token in the answer comes back as the real value; the citation
        // is verified against the real row
        let tok = text.split('⟦').nth(1).unwrap().split('⟧').next().unwrap();
        let input = json!({"text": format!("Ask ⟦{tok}⟧ about SKU-1."), "scope": "answered",
            "citations": [{"panel": 1, "label": "SKU-1", "column": "cover", "value": "4",
                           "row": [{"name": "SKU", "value": "SKU-1"}]},
                          {"panel": 1, "label": "wrong row", "column": "cover", "value": "4",
                           "row": [{"name": "SKU", "value": "SKU-2"}]}]});
        let a = okv(&s.answer(&input.to_string()));
        assert!(
            a["text"].as_str().unwrap().contains("ann@example.com"),
            "{a}"
        );
        assert_eq!(a["citations"][0]["verified"], true, "{a}");
        assert_eq!(a["citations"][1]["verified"], false, "{a}");
        assert_eq!(a["panels"], json!([1]));
    }

    #[test]
    fn option_vars_need_the_options() {
        let mut s = Session::new(DASH);
        okv(&s.begin("{}", "q"));
        let input = r#"{"panel": 1, "vars": [{"name": "region", "value": "US"}]}"#;
        assert_eq!(s.option_panels(input), "[0]");
        let opts = r#"{"0": [{"c0": "EU"}, {"c0": "US"}]}"#;
        let req = okv(&s.panel_request(input, opts));
        assert_eq!(req["vars"]["region"], "US");
        let bad = r#"{"panel": 1, "vars": [{"name": "region", "value": "MARS"}]}"#;
        assert!(s
            .panel_request(bad, opts)
            .contains("not one of the options"));
        let unknown = r#"{"panel": 1, "vars": [{"name": "anofox_data", "value": "x"}]}"#;
        assert!(s.panel_request(unknown, opts).contains("No input"));
    }

    #[test]
    fn a_read_id_is_used_once() {
        let mut s = Session::new(DASH);
        okv(&s.begin("{}", "q"));
        let id = okv(&s.panel_request(r#"{"panel": 1}"#, "{}"))["id"]
            .as_u64()
            .unwrap() as u32;
        okv(&s.panel_result(id, "[]"));
        assert!(s.panel_result(id, "[]").contains("error"));
    }
}

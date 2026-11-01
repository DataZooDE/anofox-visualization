//! Ask's tools without a transport (plan-ai-assistant.md §12.2 / §12.3): the
//! system prompt and tool definitions, the per-question view message,
//! `list_panels`, `get_panel` (its checks before the panel runs, then the
//! masking and caps of what it returns) and `answer` (unmasked, citations
//! verified). The server's loop (`server.rs`) and the builder's (M2-5, the
//! wasm `AskSession` driven by `web/assist.js`) both call these, so the
//! builder can't drift from what the server checks and sends.
//!
//! Running a panel is the caller's: [`panel_request`] gives the
//! `/api/panel`-shaped body to run (the server hands it to the panel path,
//! the builder re-runs the panel on its page), [`panel_result`] takes the
//! rows that came back.

use serde_json::{json, Map, Value};

use super::catalog::{self, Catalog};
use super::cite::{self, Citation, Read};
use super::mask::{self, Mask, Masker};
use crate::host::serving::parse_panel_request;

/// The system instructions (shared by the server and the builder).
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

/// What the asker reads when the model gives no proper answer (L4).
pub const COULDNT: &str =
    "I couldn't answer that. Try asking another way, or look at the panels directly.";

/// The tools (fixed order, part of the cached prefix): `ask-tools.json`
/// with the limits filled in (a file, not `json!`: far less wasm code).
pub fn tools() -> Value {
    let s = include_str!("../../../web/assist/ask-tools.json")
        .replace("{MAX_ROWS}", &MAX_ROWS.to_string())
        .replace("{MAX_ANSWER_CHARS}", &MAX_ANSWER_CHARS.to_string());
    serde_json::from_str(&s).unwrap_or_else(|_| json!([]))
}

/// The system prompt: instructions, role note, catalog — the shared,
/// cached prefix (the same bytes for every viewer of a dashboard).
pub fn system(cat: &Catalog) -> Value {
    json!([
        {"type": "text", "text": INSTRUCTIONS.trim()},
        {"type": "text", "text": format!("{}\n{}", cat.role_note(), cat.text()),
         "cache_control": {"type": "ephemeral"}}
    ])
}

/// Is the dashboard's header `-- @ask on`?
pub fn ask_on(script: &str) -> bool {
    crate::sql::ask::header_on(script)
}

/// The panels kept out of the assistant (`-- @ask skip`).
pub fn skipped(script: &str) -> Vec<usize> {
    crate::sql::ask::exclusions(script)
        .into_iter()
        .map(|(i, _)| i)
        .collect()
}

/// What one question works with.
pub struct Ctx<'a> {
    pub dash_id: &'a str,
    pub script: &'a str,
    pub cat: &'a Catalog,
    /// Panels not available to this asker (`@ask skip`, hidden tabs).
    pub excluded: &'a [usize],
    /// The view state, already narrowed to the allowed variables.
    pub view_vars: &'a Map<String, Value>,
    /// The variables a panel request may set (`-- @vars`).
    pub allowed_vars: &'a [String],
    pub mask: &'a Mask,
}

/// One question's tally of panel reads.
#[derive(Default, Debug, Clone)]
pub struct Tally {
    /// The panels read for it.
    pub panels: Vec<usize>,
    pub calls: usize,
    /// The variables `get_panel` set (names only, for the log).
    pub tool_vars: Vec<String>,
    pub data_bytes: usize,
}

/// A panel read to run: the `/api/panel` body, checked like one.
#[derive(Debug, Clone)]
pub struct PanelReq {
    pub panel: usize,
    /// `{dashboard, panel, vars, page?}`.
    pub body: Value,
    /// The variables `get_panel` set, real values (the citation's vars).
    pub real_vars: Map<String, Value>,
}

/// The model's answer, unmasked.
#[derive(Debug, Clone)]
pub struct Answered {
    pub text: String,
    pub scope: String,
    pub citations: Vec<Citation>,
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

/// A short, masked, single-line error for a tool result.
pub fn short_err(m: &Masker, e: &str) -> String {
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

pub fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// The view message (the per-asker part, after the cache breakpoint).
pub fn view_message(
    ctx: &Ctx,
    m: &Masker,
    tab: Option<&str>,
    panel: Option<usize>,
    question: &str,
    refreshed: bool,
) -> Vec<Value> {
    let mut s = String::from("<view>\n");
    if refreshed {
        s.push_str(
            "The data was refreshed since the last question: earlier answers may be out of date.\n",
        );
    }
    if let Some(tab) = tab {
        let on_tab: Vec<String> = ctx
            .cat
            .entries
            .iter()
            .filter(|e| e.tab.as_deref() == Some(tab))
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
    if let Some(p) = panel {
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

/// `list_panels`.
pub fn list_panels(ctx: &Ctx, input: &Value) -> String {
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

/// The input panels whose options a `get_panel` call's vars must be checked
/// against (run before [`panel_request`] by a caller that can't run them on
/// demand). A var [`panel_request`] would refuse anyway is left out.
pub fn option_panels(ctx: &Ctx, input: &Value) -> Vec<usize> {
    let mut out = Vec::new();
    for pair in input["vars"].as_array().into_iter().flatten() {
        let name = pair["name"].as_str().unwrap_or("").trim();
        let Some((ie, spec)) = ctx
            .cat
            .inputs()
            .find(|(_, s)| s.var.eq_ignore_ascii_case(name))
        else {
            continue;
        };
        let allowed = ctx
            .allowed_vars
            .iter()
            .any(|a| a.eq_ignore_ascii_case(&spec.var));
        if allowed
            && !ctx.excluded.contains(&ie.index)
            && spec.has_options()
            && !out.contains(&ie.index)
        {
            out.push(ie.index);
        }
    }
    out
}

/// An input panel's options, from its result body.
pub fn option_values(script: &str, input_panel: usize, out: &str) -> Result<Vec<String>, String> {
    let rows = parse_rows(out)?;
    let plan = crate::sql::plan(script);
    let p = plan.get(input_panel).ok_or("no such panel")?;
    let idx = crate::inputs::input_index(&p.roles);
    let key = format!("c{idx}");
    Ok(rows
        .iter()
        .filter_map(|r| r.get(&key).or_else(|| r.values().nth(idx)))
        .map(text_of)
        .collect())
}

/// `get_panel`, before the panel runs: the checks (the call cap, the panel,
/// the variables — M3 — and the page) and the body to run. `options(i)`
/// gives input panel `i`'s options as this asker sees them.
pub fn panel_request(
    ctx: &Ctx,
    masker: &Masker,
    tally: &mut Tally,
    input: &Value,
    options: &mut dyn FnMut(usize) -> Result<Vec<String>, String>,
) -> Result<PanelReq, String> {
    if tally.calls >= MAX_PANEL_CALLS {
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
    tally.calls += 1;
    // Vars (M3): only what an input binds, and an option input only to
    // one of its options. Tokens become real values first.
    let mut vars = ctx.view_vars.clone();
    let mut real_vars = Map::new();
    for pair in input["vars"].as_array().into_iter().flatten() {
        let name = pair["name"].as_str().unwrap_or("").trim();
        let value = masker.unmask_text(pair["value"].as_str().unwrap_or(""));
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
            let opts = options(ie.index).map_err(|e| short_err(masker, &e))?;
            if !opts.iter().any(|o| o == &value) {
                return Err(format!(
                    "That value is not one of the options of '{}'.",
                    spec.var
                ));
            }
        }
        tally.tool_vars.push(spec.var.clone());
        let j = var_json(spec, &value);
        vars.insert(spec.var.clone(), j.clone());
        real_vars.insert(spec.var.clone(), j);
    }
    let plan = crate::sql::plan(ctx.script);
    let names = &catalog::labels(ctx.script, &plan, panel);
    let mut body = json!({"dashboard": ctx.dash_id, "panel": panel, "vars": vars});
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
            page["filter"] = json!(masker.unmask_text(f));
        }
        body["page"] = page;
    }
    // The same checks as POST /api/panel, and the dashboard is always
    // this conversation's (I3).
    parse_panel_request(&body.to_string()).map_err(|e| short_err(masker, &e))?;
    Ok(PanelReq {
        panel,
        body,
        real_vars,
    })
}

/// `get_panel`, after the panel ran (`out`: the `/api/panel` response
/// body): masked, capped and labelled for the model; the real rows are kept
/// in `reads` for the citation check.
pub fn panel_result(
    ctx: &Ctx,
    masker: &mut Masker,
    reads: &mut Vec<Read>,
    tally: &mut Tally,
    req: &PanelReq,
    out: &str,
) -> Result<String, String> {
    let panel = req.panel;
    let rows = parse_rows(out).map_err(|_| format!("Panel {panel} gave no rows."))?;
    let total = rows.len();
    let plan = crate::sql::plan(ctx.script);
    let names = &catalog::labels(ctx.script, &plan, panel);
    // Columns by display name, in result order.
    let keys: Vec<String> = rows
        .first()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default();
    let cols: Vec<String> = keys.iter().map(|k| display(names, k)).collect();
    let extra = mask_marks(ctx.script, panel);
    let whole: Vec<bool> = cols
        .iter()
        .map(|c| Masker::masks_column(ctx.mask, c, &extra))
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
    let mut mask_cell = |i: usize, v: &Value| -> Result<Value, String> {
        match v {
            Value::String(s) => masker
                .cell(ctx.mask, whole[i], s)
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
    if tally.data_bytes + size > MAX_QUESTION_DATA {
        return Err(
            "The panel data limit for this question is reached: answer with what you have.".into(),
        );
    }
    tally.data_bytes += size;
    if !tally.panels.contains(&panel) {
        tally.panels.push(panel);
    }
    let shown = sent_rows.len();
    let note = if shown < total {
        format!("first {shown} of {total} rows; sort or filter for others")
    } else {
        format!("{total} rows")
    };
    let masked_vars: Map<String, Value> = req
        .real_vars
        .iter()
        .map(|(k, v)| (k.clone(), json!(masker.mask_text(&text_of(v)))))
        .collect();
    let data = json!({
        "panel": panel,
        "columns": cols,
        "constant": consts,
        "rows": sent_rows,
        "rows_note": note,
        "vars": masked_vars,
    });
    reads.push(Read {
        panel,
        vars: req.real_vars.clone(),
        columns: cols,
        rows: real_rows,
    });
    Ok(format!(
        "<panel_data panel=\"{panel}\">\n{}\n</panel_data>",
        serde_json::to_string(&data).unwrap_or_default()
    ))
}

/// The model's `answer`: unmasked, citations verified.
pub fn answer(masker: &Masker, reads: &[Read], input: &Value) -> Result<Answered, String> {
    let text = input["text"].as_str().ok_or("answer: give text")?;
    let mut text = masker.unmask_text(text);
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
                    .map(|s| masker.unmask_text(s))
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
                            masker.unmask_text(p["value"].as_str()?),
                        ))
                    })
                    .collect(),
            };
            Some(cite::verify(&claim, reads))
        })
        .collect();
    Ok(Answered {
        text,
        scope,
        citations,
    })
}

/// Append a user turn: into the last message when the history ends with a
/// user message (the previous question's tool results), so roles alternate.
pub fn push_user(messages: &mut Vec<Value>, content: Vec<Value>) {
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

/// A token the map didn't know must not reach the asker as a pseudonym.
pub fn scrub_tokens(s: &str) -> String {
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
pub fn parse_rows(body: &str) -> Result<Vec<Map<String, Value>>, String> {
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

#[cfg(test)]
mod tests {
    #[test]
    fn tools_parse_with_their_limits() {
        let t = super::tools();
        let names: Vec<&str> = t
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["list_panels", "get_panel", "answer"]);
        let s = t.to_string();
        assert!(
            s.contains("at most 200 rows") && s.contains("at most 1200 characters"),
            "{s}"
        );
        assert!(!s.contains("{MAX_"), "{s}");
    }
}

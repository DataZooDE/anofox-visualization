//! Ask's loop against a scripted fake model and a fake panel path (no live
//! calls): plan-ai-assistant.md §12.9's deterministic tests that M2-3 can
//! run without the locked server (M2-4 adds the HTTP and canary-policy ones).

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Map, Value};

use super::*;
use crate::host::assist::{UpResponse, Upstream};

const DASH: &str = "-- @title Sales
-- @description Sales per area.
-- @ask on
-- @vars region
SELECT 'Overview'::TAB;
SELECT region ::DROPDOWN FROM regions;
SELECT region AS \"Region\" ::XAXIS, revenue AS \"Revenue\" ::BARCHART, 'Revenue by region' ::TITLE
FROM sales WHERE region = getvariable('region');
-- @ask skip
SELECT name AS \"Name\" ::TABLE, salary FROM staff;
SELECT 'Customers'::TAB;
SELECT customer AS \"Customer\" ::TABLE, email, amount FROM orders;
SELECT employee ::CATEGORY, sales AS \"Sales\" ::BARCHART FROM perf;
";
// plan indices: 0 tab, 1 input, 2 chart, 3 skipped table, 4 tab, 5 customers,
// 6 sales per employee

/// The fake model: scripted replies (status, body), every request recorded.
#[derive(Clone, Default)]
struct Model {
    replies: Arc<Mutex<VecDeque<(u16, Value)>>>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Model {
    fn new(replies: Vec<Value>) -> Model {
        let m = Model::default();
        m.replies
            .lock()
            .unwrap()
            .extend(replies.into_iter().map(|r| (200, r)));
        m
    }
    fn push_status(&self, status: u16) {
        self.replies
            .lock()
            .unwrap()
            .push_front((status, json!({"type": "error"})));
    }
    fn requests(&self) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    fn sent(&self) -> String {
        self.seen.lock().unwrap().join("\n")
    }
}

impl Upstream for Model {
    fn post(&self, path: &str, body: &[u8], stream: bool) -> Result<UpResponse, String> {
        assert_eq!(path, "/v1/messages");
        assert!(!stream);
        self.seen
            .lock()
            .unwrap()
            .push(String::from_utf8(body.to_vec()).unwrap());
        let (status, reply) = self.replies.lock().unwrap().pop_front().unwrap_or((
            200,
            json!({"content": [{"type": "text", "text": "…"}], "stop_reason": "end_turn"}),
        ));
        Ok(UpResponse {
            status,
            request_id: Some(format!("req_{}", self.seen.lock().unwrap().len())),
            body: Box::new(Cursor::new(serde_json::to_vec(&reply).unwrap())),
        })
    }
}

/// The fake panel path: answers per panel index, every body recorded.
#[derive(Default)]
struct Panels {
    bodies: Mutex<Vec<Value>>,
    hidden: Vec<usize>,
    big: bool,
    sentinel: bool,
}

impl PanelRunner for Panels {
    fn panel(&self, body: &str, _deadline: Instant) -> Result<String, (u16, String)> {
        let b: Value = serde_json::from_str(body).unwrap();
        self.bodies.lock().unwrap().push(b.clone());
        let region = b["vars"]["region"].as_str().unwrap_or("EU").to_string();
        Ok(match b["panel"].as_u64().unwrap() {
            1 => json!([{"c0": "EU"}, {"c0": "US"}]),
            2 if self.big => Value::Array(
                (0..500)
                    .map(|i| json!({"c0": format!("R{i}"), "c1": i, "c2": "Revenue by region"}))
                    .collect(),
            ),
            2 => match region.as_str() {
                "US" => json!([{"c0": "US", "c1": 980.0, "c2": "Revenue by region"}]),
                _ => json!([{"c0": "EU", "c1": 1234.5, "c2": "Revenue by region"},
                            {"c0": "APAC", "c1": 77.0, "c2": "Revenue by region"}]),
            },
            // a ::TABLE keeps its columns' own names
            5 if self.sentinel => json!([{"Customer": "Bob ⟦P1⟧", "email": "x", "amount": 1}]),
            5 => json!([{"Customer": "Ada Lovelace", "email": "ada@example.com", "amount": 12}]),
            6 => json!([{"c0": "Grace Hopper", "c1": 3.0}]),
            n => return Err((404, format!("no panel {n}"))),
        }
        .to_string())
    }
    fn hidden(&self) -> Vec<usize> {
        self.hidden.clone()
    }
}

fn use_tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

fn reply(blocks: Vec<Value>) -> Value {
    json!({"content": blocks, "stop_reason": "tool_use",
           "usage": {"input_tokens": 1000, "output_tokens": 100, "cache_read_input_tokens": 500}})
}

fn answer(text: &str, citations: Value, scope: &str) -> Value {
    reply(vec![use_tool(
        "a",
        "answer",
        json!({"text": text, "citations": citations, "scope": scope}),
    )])
}

fn cfg() -> AskConfig {
    AskConfig {
        enabled: true,
        ..Default::default()
    }
}

fn viewer(user: &str) -> Viewer {
    Viewer {
        session: format!("sess-{user}"),
        claims_key: format!("claims-{user}"),
        user: user.into(),
        ..Default::default()
    }
}

fn dash(script: &str) -> Dash<'_> {
    Dash {
        id: "sales",
        script,
        epoch: 1,
    }
}

fn q(question: &str) -> AskRequest {
    AskRequest {
        question: question.into(),
        tab: Some("Overview".into()),
        ..Default::default()
    }
}

fn ask_with(
    ask: &Ask,
    v: &Viewer,
    script: &str,
    r: AskRequest,
    panels: &Panels,
) -> Result<AskAnswer, AskError> {
    ask.ask(v, &dash(script), r, panels, &mut |_| {}, &|| false)
}

fn new_ask(model: &Model, c: AskConfig) -> Ask {
    Ask::new(c, Box::new(model.clone()), None)
}

#[test]
fn reads_a_panel_and_answers_with_a_verified_citation() {
    let m = Model::new(vec![
        reply(vec![use_tool("t1", "get_panel", json!({"panel": 2}))]),
        answer(
            "**EU** leads with 1,234.5.",
            json!([
                {"panel": 2, "label": "EU", "column": "Revenue", "value": "1,234.5", "row": [{"name": "Region", "value": "EU"}]},
                {"panel": 2, "label": "wrong row", "column": "Revenue", "value": "1,234.5", "row": [{"name": "Region", "value": "APAC"}]},
                {"panel": 5, "label": "never read", "column": "amount", "value": "12"}
            ]),
            "answered",
        ),
    ]);
    let ask = new_ask(&m, cfg());
    let panels = Panels::default();
    let mut progress = Vec::new();
    let a = ask
        .ask(
            &viewer("alice"),
            &dash(DASH),
            q("Which region leads?"),
            &panels,
            &mut |p| progress.push(p),
            &|| false,
        )
        .unwrap();
    assert_eq!(a.scope, "answered");
    assert_eq!(a.text, "**EU** leads with 1,234.5.");
    assert_eq!(a.panels, vec![2]);
    let v: Vec<bool> = a.citations.iter().map(|c| c.verified).collect();
    assert_eq!(v, vec![true, false, false], "{:?}", a.citations);
    assert_eq!(a.citations[0].value, Some(json!(1234.5)));
    assert!(progress.contains(&Progress::Reading { panel: 2 }));
    assert!(a.first_in_session);
    // The panel data reached the model labelled as data; the title too
    // (role values come only through get_panel, H1).
    let reqs = m.requests();
    assert_eq!(reqs.len(), 2);
    let second = reqs[1]["messages"].to_string();
    assert!(second.contains("<panel_data panel=\\\"2\\\">"), "{second}");
    assert!(second.contains("Revenue by region"));
    assert!(
        !reqs[0].to_string().contains("Revenue by region"),
        "a ::TITLE value is data, not catalog"
    );
    // The view state and the question come after the cached prefix.
    let first = reqs[0]["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        first.contains("Open tab: Overview (panels 1, 2)")
            && first.ends_with("Question: Which region leads?"),
        "{first}"
    );
    assert_eq!(reqs[0]["system"][1]["cache_control"]["type"], "ephemeral");
    // The panel path got this dashboard's id, always (I3).
    for b in panels.bodies.lock().unwrap().iter() {
        assert_eq!(b["dashboard"], "sales");
    }
}

#[test]
fn skipped_and_hidden_panels_never_reach_a_request() {
    let m = Model::new(vec![
        reply(vec![
            use_tool("t1", "get_panel", json!({"panel": 3})),
            use_tool("t2", "get_panel", json!({"panel": 5})),
            use_tool("t3", "get_panel", json!({"panel": 0})),
        ]),
        answer("Not available.", json!([]), "not_on_dashboard"),
    ]);
    let ask = new_ask(&m, cfg());
    let panels = Panels {
        hidden: vec![5],
        ..Default::default()
    };
    let a = ask_with(&ask, &viewer("alice"), DASH, q("Salaries?"), &panels).unwrap();
    assert_eq!(a.scope, "not_on_dashboard");
    assert!(
        panels.bodies.lock().unwrap().is_empty(),
        "the panel path was never called"
    );
    let sent = m.sent();
    // @ask skip: not even in the catalog (its column names never leave)
    assert!(
        !sent.contains("salary") && !sent.contains("\\\"Name\\\"") && !sent.contains("[3]"),
        "{sent}"
    );
    assert!(!sent.contains("Ada"));
    let results = m.requests()[1]["messages"].to_string();
    assert!(
        results.contains("Panel 5 is not available") && results.contains("There is no panel 3"),
        "{results}"
    );
    assert!(
        results.contains("There is no panel 0"),
        "a ::TAB is not a panel"
    );
}

#[test]
fn the_prefix_is_the_same_for_every_viewer() {
    let run = |user: &str, hidden: Vec<usize>| {
        let m = Model::new(vec![answer("ok", json!([]), "answered")]);
        let ask = new_ask(&m, cfg());
        let panels = Panels {
            hidden,
            ..Default::default()
        };
        let mut r = q("hi");
        r.vars.insert(
            "region".into(),
            json!(if user == "a" { "EU" } else { "US" }),
        );
        ask_with(&ask, &viewer(user), DASH, r, &panels).unwrap();
        let req = &m.requests()[0];
        (
            req["system"].to_string(),
            req["tools"].to_string(),
            req["messages"].to_string(),
        )
    };
    let (sa, ta, ma) = run("a", vec![]);
    let (sb, tb, mb) = run("b", vec![5]);
    assert_eq!(sa, sb);
    assert_eq!(ta, tb);
    assert_ne!(ma, mb, "the per-viewer part differs");
    assert!(mb.contains("Not available to you: panels 5"));
    assert!(!sa.contains("EU") && !sa.contains("US"));
}

#[test]
fn vars_only_through_inputs_and_their_options() {
    let m = Model::new(vec![
        reply(vec![
            use_tool(
                "t1",
                "get_panel",
                json!({"panel": 2, "vars": [{"name": "region", "value": "US"}]}),
            ),
            use_tool(
                "t2",
                "get_panel",
                json!({"panel": 2, "vars": [{"name": "region", "value": "Mars"}]}),
            ),
            use_tool(
                "t3",
                "get_panel",
                json!({"panel": 2, "vars": [{"name": "__claim_user", "value": "admin"}]}),
            ),
            use_tool(
                "t4",
                "get_panel",
                json!({"panel": 2, "vars": [{"name": "anofox_data", "value": "/etc"}]}),
            ),
        ]),
        answer(
            "US: 980.",
            json!([{"panel": 2, "label": "US", "column": "Revenue", "value": "980", "row": [{"name": "Region", "value": "US"}]}]),
            "answered",
        ),
    ]);
    let ask = new_ask(&m, cfg());
    let panels = Panels::default();
    let a = ask_with(&ask, &viewer("alice"), DASH, q("And the US?"), &panels).unwrap();
    assert!(a.citations[0].verified);
    assert_eq!(
        a.citations[0].vars["region"], "US",
        "a citation carries its vars"
    );
    let bodies = panels.bodies.lock().unwrap();
    // option lookups (panel 1) and one real read with region = US
    let reads: Vec<&Value> = bodies.iter().filter(|b| b["panel"] == 2).collect();
    assert_eq!(reads.len(), 1, "{bodies:?}");
    assert_eq!(reads[0]["vars"]["region"], "US");
    let results = m.requests()[1]["messages"].to_string();
    assert!(results.contains("is not one of the options"));
    assert!(results.contains("No input on this dashboard sets '__claim_user'"));
    assert!(results.contains("No input on this dashboard sets 'anofox_data'"));
}

#[test]
fn forged_view_vars_are_refused() {
    let ask = new_ask(&Model::default(), cfg());
    for (k, v) in [
        ("__claim_user", "admin"),
        ("anofox_data", "/etc"),
        ("other", "x"),
    ] {
        let mut r = q("hi");
        r.vars.insert(k.into(), json!(v));
        let e = ask_with(&ask, &viewer("alice"), DASH, r, &Panels::default()).unwrap_err();
        assert_eq!(e.status, 400, "{k}: {e:?}");
    }
}

#[test]
fn personal_values_are_masked_both_ways() {
    let m = Model::new(vec![
        reply(vec![use_tool("t1", "get_panel", json!({"panel": 5}))]),
        answer(
            "⟦P1⟧ (⟦E2⟧) ordered 12; ⟦P77⟧ is unknown.",
            json!([{"panel": 5, "label": "⟦P1⟧", "column": "amount", "value": "12", "row": [{"name": "Customer", "value": "⟦P1⟧"}]}]),
            "answered",
        ),
        // the follow-up
        answer("Done.", json!([]), "answered"),
    ]);
    let ask = new_ask(&m, cfg());
    let panels = Panels {
        ..Default::default()
    };
    let a = ask_with(&ask, &viewer("alice"), DASH, q("Who ordered?"), &panels).unwrap();
    assert_eq!(
        a.text,
        "Ada Lovelace (ada@example.com) ordered 12; [hidden] is unknown."
    );
    assert!(a.citations[0].verified, "{:?}", a.citations);
    assert_eq!(a.citations[0].label, "Ada Lovelace");
    // a follow-up naming her: masked before it leaves
    let mut r = q("What else did Ada Lovelace buy?");
    r.conversation = Some(a.conversation.clone());
    ask_with(&ask, &viewer("alice"), DASH, r, &panels).unwrap();
    let sent = m.sent();
    assert!(
        !sent.contains("Ada Lovelace") && !sent.contains("ada@example.com"),
        "{sent}"
    );
    assert!(sent.contains("What else did ⟦P1⟧ buy?"));
}

#[test]
fn a_role_column_is_masked_by_its_source_name() {
    // `employee ::CATEGORY` comes back as c0: its name is the source's
    let m = Model::new(vec![
        reply(vec![use_tool("t1", "get_panel", json!({"panel": 6}))]),
        answer("⟦P1⟧ sold 3.", json!([]), "answered"),
    ]);
    let ask = new_ask(&m, cfg());
    let a = ask_with(
        &ask,
        &viewer("alice"),
        DASH,
        q("Who sold?"),
        &Panels::default(),
    )
    .unwrap();
    assert_eq!(a.text, "Grace Hopper sold 3.");
    let sent = m.sent();
    assert!(
        !sent.contains("Grace") && sent.contains("\\\"employee\\\""),
        "{sent}"
    );
}

#[test]
fn a_result_with_the_sentinel_is_not_sent() {
    let m = Model::new(vec![
        reply(vec![use_tool("t1", "get_panel", json!({"panel": 5}))]),
        answer("x", json!([]), "answered"),
    ]);
    let ask = new_ask(&m, cfg());
    let panels = Panels {
        sentinel: true,
        ..Default::default()
    };
    ask_with(&ask, &viewer("alice"), DASH, q("?"), &panels).unwrap();
    let sent = m.sent();
    assert!(!sent.contains("Bob"), "{sent}");
    assert!(sent.contains("can't be sent to the assistant"));
}

#[test]
fn big_results_are_capped() {
    let m = Model::new(vec![
        reply(vec![use_tool("t1", "get_panel", json!({"panel": 2}))]),
        answer("x", json!([]), "answered"),
    ]);
    let ask = new_ask(&m, cfg());
    let panels = Panels {
        big: true,
        ..Default::default()
    };
    ask_with(&ask, &viewer("alice"), DASH, q("?"), &panels).unwrap();
    let msgs = &m.requests()[1]["messages"];
    let result = msgs.as_array().unwrap().last().unwrap()["content"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        result.contains("of 500 rows; sort or filter for others"),
        "{}",
        &result[..200]
    );
    assert!(result.len() < MAX_RESULT_BYTES + 2_000);
    assert!(!result.contains("R499"));
    // the constant ::TITLE column is sent once
    assert_eq!(result.matches("Revenue by region").count(), 1);
}

#[test]
fn follow_ups_hold_the_history_on_the_server_only() {
    let m = Model::new(vec![
        reply(vec![use_tool("t1", "get_panel", json!({"panel": 2}))]),
        answer("EU leads.", json!([]), "answered"),
        answer("Still EU.", json!([]), "answered"),
        answer("Fresh start.", json!([]), "answered"),
    ]);
    let ask = new_ask(&m, cfg());
    let panels = Panels::default();
    let a = ask_with(&ask, &viewer("alice"), DASH, q("Which leads?"), &panels).unwrap();
    let mut r = q("Sure?");
    r.conversation = Some(a.conversation.clone());
    let b = ask_with(&ask, &viewer("alice"), DASH, r.clone(), &panels).unwrap();
    assert_eq!(b.conversation, a.conversation);
    let reqs = m.requests();
    let follow = reqs[2]["messages"].as_array().unwrap();
    assert!(
        follow.len() >= 4,
        "the earlier turn is sent again: {follow:?}"
    );
    assert!(follow[0]["content"].to_string().contains("Which leads?"));
    // roles alternate (the answer's tool result and the new question share a user message)
    for w in follow.windows(2) {
        assert_ne!(w[0]["role"], w[1]["role"]);
    }
    // someone else with that id gets a fresh conversation
    let c = ask_with(&ask, &viewer("bob"), DASH, r, &panels).unwrap();
    assert_ne!(c.conversation, a.conversation);
    let fresh = m.requests()[3]["messages"].as_array().unwrap().len();
    assert_eq!(fresh, 1);
}

#[test]
fn a_refresh_starts_over() {
    let m = Model::new(vec![
        answer("a", json!([]), "answered"),
        answer("b", json!([]), "answered"),
    ]);
    let ask = new_ask(&m, cfg());
    let a = ask_with(&ask, &viewer("alice"), DASH, q("?"), &Panels::default()).unwrap();
    let mut r = q("again?");
    r.conversation = Some(a.conversation.clone());
    let d = Dash {
        id: "sales",
        script: DASH,
        epoch: 2,
    };
    let b = ask
        .ask(
            &viewer("alice"),
            &d,
            r,
            &Panels::default(),
            &mut |_| {},
            &|| false,
        )
        .unwrap();
    assert!(b.refreshed);
    assert_ne!(b.conversation, a.conversation);
    assert!(m.requests()[1]
        .to_string()
        .contains("The data was refreshed"));
}

#[test]
fn failures_give_the_fixed_message() {
    // end_turn with text
    let m = Model::new(vec![
        json!({"content": [{"type": "text", "text": "<img src=x>"}], "stop_reason": "end_turn"}),
    ]);
    let a = ask_with(
        &new_ask(&m, cfg()),
        &viewer("a"),
        DASH,
        q("?"),
        &Panels::default(),
    )
    .unwrap();
    assert_eq!((a.text.as_str(), a.scope.as_str()), (COULDNT, "error"));
    // an upstream error: its body is never relayed
    let m = Model::default();
    m.replies
        .lock()
        .unwrap()
        .push_back((500, json!({"error": {"message": "secret upstream detail"}})));
    let a = ask_with(
        &new_ask(&m, cfg()),
        &viewer("a"),
        DASH,
        q("?"),
        &Panels::default(),
    )
    .unwrap();
    assert_eq!(a.text, COULDNT);
    // 529 is retried
    let m = Model::new(vec![answer("ok", json!([]), "answered")]);
    m.push_status(529);
    let a = ask_with(
        &new_ask(&m, cfg()),
        &viewer("a"),
        DASH,
        q("?"),
        &Panels::default(),
    )
    .unwrap();
    assert_eq!(a.text, "ok");
    assert_eq!(m.requests().len(), 2);
    // cancelled
    let m = Model::new(vec![]);
    let a = new_ask(&m, cfg())
        .ask(
            &viewer("a"),
            &dash(DASH),
            q("?"),
            &Panels::default(),
            &mut |_| {},
            &|| true,
        )
        .unwrap();
    assert_eq!(a.scope, "error");
    assert!(m.requests().is_empty());
}

#[test]
fn turns_and_tokens_are_capped() {
    let looping: Vec<Value> = (0..10)
        .map(|i| {
            reply(vec![use_tool(
                &format!("t{i}"),
                "list_panels",
                json!({"tab": ""}),
            )])
        })
        .collect();
    let m = Model::new(looping.clone());
    let a = ask_with(
        &new_ask(&m, cfg()),
        &viewer("a"),
        DASH,
        q("?"),
        &Panels::default(),
    )
    .unwrap();
    assert_eq!(a.text, COULDNT);
    assert_eq!(m.requests().len(), MAX_MODEL_TURNS);
    // the per-question ceiling stops before a call that could pass it
    let m = Model::new(looping);
    let mut c = cfg();
    c.per_question_tokens = 9_000;
    c.max_tokens = 1_000;
    let a = ask_with(
        &new_ask(&m, c),
        &viewer("a"),
        DASH,
        q("?"),
        &Panels::default(),
    )
    .unwrap();
    assert_eq!(a.scope, "limit");
    assert!(m.requests().len() < MAX_MODEL_TURNS);
}

#[test]
fn limits_answer_429() {
    let m = Model::new(vec![
        answer("a", json!([]), "answered"),
        answer("b", json!([]), "answered"),
    ]);
    let mut c = cfg();
    c.per_viewer.per_hour = 1;
    let ask = new_ask(&m, c);
    ask_with(&ask, &viewer("a"), DASH, q("?"), &Panels::default()).unwrap();
    let e = ask_with(&ask, &viewer("a"), DASH, q("?"), &Panels::default()).unwrap_err();
    assert_eq!(e.status, 429);
    assert!(e.message.contains("1 questions an hour"));
    // someone else still may
    ask_with(&ask, &viewer("b"), DASH, q("?"), &Panels::default()).unwrap();
}

#[test]
fn who_may_ask() {
    let ask = new_ask(&Model::default(), cfg());
    let p = Panels::default();
    // not on for the dashboard, or without -- @vars
    let off = DASH.replace("-- @ask on\n", "");
    assert_eq!(
        ask_with(&ask, &viewer("a"), &off, q("?"), &p)
            .unwrap_err()
            .status,
        404
    );
    let novars = DASH.replace("-- @vars region\n", "");
    assert_eq!(
        ask_with(&ask, &viewer("a"), &novars, q("?"), &p)
            .unwrap_err()
            .status,
        404
    );
    let disabled = new_ask(&Model::default(), AskConfig::default());
    assert_eq!(
        ask_with(&disabled, &viewer("a"), DASH, q("?"), &p)
            .unwrap_err()
            .status,
        404
    );
    // API keys, embed links (unless ask.embed, and only their dashboard)
    let mut k = viewer("svc");
    k.api_key = true;
    assert_eq!(
        ask_with(&ask, &k, DASH, q("?"), &p).unwrap_err().status,
        403
    );
    let mut e = viewer("emb");
    e.embed = Some("sales".into());
    assert_eq!(
        ask_with(&ask, &e, DASH, q("?"), &p).unwrap_err().status,
        403
    );
    let mut c = cfg();
    c.embed = true;
    let m = Model::new(vec![answer("ok", json!([]), "answered")]);
    assert!(ask_with(&new_ask(&m, c.clone()), &e, DASH, q("?"), &p).is_ok());
    e.embed = Some("other".into());
    assert_eq!(
        ask_with(&new_ask(&Model::default(), c), &e, DASH, q("?"), &p)
            .unwrap_err()
            .status,
        403
    );
    // ask.allow
    let mut c = cfg();
    c.allow = vec!["group:analysts".into(), "claim:tenant=acme".into()];
    let ask = new_ask(&Model::new(vec![answer("ok", json!([]), "answered")]), c);
    assert_eq!(
        ask_with(&ask, &viewer("a"), DASH, q("?"), &p)
            .unwrap_err()
            .status,
        403
    );
    let mut g = viewer("g");
    g.groups = vec!["analysts".into()];
    assert!(ask_with(&ask, &g, DASH, q("?"), &p).is_ok());
    // question size
    let ask = new_ask(&Model::default(), cfg());
    assert_eq!(
        ask_with(&ask, &viewer("a"), DASH, q(&"x".repeat(2_001)), &p)
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(
        ask_with(&ask, &viewer("a"), DASH, q("  "), &p)
            .unwrap_err()
            .status,
        400
    );
}

#[test]
fn the_log_has_no_question_answer_or_data() {
    let dir = std::env::temp_dir().join(format!("ask-log-{}", store::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("ask.jsonl");
    let m = Model::new(vec![
        reply(vec![use_tool(
            "t1",
            "get_panel",
            json!({"panel": 2, "vars": [{"name": "region", "value": "US"}]}),
        )]),
        answer("US made 980.", json!([]), "answered"),
    ]);
    let ask = Ask::new(cfg(), Box::new(m), Some(LogSink::File(log.clone())));
    ask_with(
        &ask,
        &viewer("alice"),
        DASH,
        q("How did the US do?"),
        &Panels::default(),
    )
    .unwrap();
    let line = std::fs::read_to_string(&log).unwrap();
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["path"], "/api/ask");
    assert_eq!(v["panels"], json!([2]));
    assert_eq!(v["tool_vars"], json!(["region"]), "tool vars by name only");
    assert_eq!(v["outcome"], "answered");
    assert_eq!(v["tokens"]["input"], 2_000);
    assert_eq!(v["request_id"], json!(["req_1", "req_2"]));
    for leak in ["How did", "980", "US made", "EU"] {
        assert!(!line.contains(leak), "{leak} in {line}");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_key_never_reaches_a_message() {
    std::env::set_var("ASK_TEST_KEY_M23", "sk-ant-test-0123456789");
    let mut c = cfg();
    c.key_env = "ASK_TEST_KEY_M23".into();
    let (key, msgs) = c.start(true).unwrap();
    assert_eq!(key, "sk-ant-test-0123456789");
    assert!(msgs.iter().all(|m| !m.contains("sk-ant")), "{msgs:?}");
    assert!(msgs[0].contains("claude-sonnet-5") && msgs[0].contains("Anthropic (Claude)"));
    // no login: refused unless anonymous, which warns
    assert!(c.start(false).unwrap_err().contains("needs a login"));
    c.anonymous = true;
    assert!(c
        .start(false)
        .unwrap()
        .1
        .iter()
        .any(|m| m.starts_with("WARNING")));
    c.key_env = "ASK_TEST_KEY_M23_MISSING".into();
    let e = c.start(true).unwrap_err();
    assert!(e.contains("no model key") && !e.contains("sk-ant"));
}

#[test]
fn config_parses_and_refuses_unknown_keys() {
    let c = AskConfig::parse(&json!({
        "enabled": true, "model": "claude-opus-5-5", "per_viewer": {"per_hour": 5},
        "mask": ["customer"], "allow": ["group:analysts"], "notice": null, "effort": "low"
    }))
    .unwrap();
    assert_eq!(c.per_viewer.per_hour, 5);
    assert_eq!(c.per_viewer.per_day, 30);
    assert_eq!(c.mask, Mask::Columns(vec!["customer".into()]));
    for bad in [
        json!({"enabled": true, "modle": "x"}),
        json!({"model": "claude-haiku-4-5"}),
        json!({"base_url": "http://example.com"}),
        json!({"allow": ["analysts"]}),
        json!({"per_viewer": {"per_week": 1}}),
        json!({"transport": "openai"}),
    ] {
        assert!(AskConfig::parse(&bad).is_err(), "{bad}");
    }
}

#[test]
fn catalog_is_source_text_only() {
    let meta = parse_dashboard_meta("sales", DASH);
    assert!(meta.ask);
    let c = Catalog::build(DASH, &meta);
    let idx: Vec<usize> = c.entries.iter().map(|e| e.index).collect();
    assert_eq!(idx, vec![1, 2, 5, 6]);
    let t = c.text();
    assert!(
        t.contains("[1] tab \"Overview\" · input (dropdown) · sets variable region"),
        "{t}"
    );
    assert!(t.contains("[2] tab \"Overview\" · barchart chart · columns: Region (XAXIS); Revenue (BARCHART); c2 (TITLE) · title from data"), "{t}");
    assert!(t.contains("[5] tab \"Customers\" · table"));
    assert!(t.contains("[6] tab \"Customers\" · barchart chart · columns: employee (CATEGORY); Sales (BARCHART)"), "{t}");
    assert!(
        !t.contains("SELECT") && !t.contains("getvariable"),
        "no SQL"
    );
    assert!(c.role_note().contains("::BARCHART — "));
    assert_eq!(
        catalog::static_literal("SELECT 'It''s' AS c0"),
        Some("It's".into())
    );
    assert_eq!(catalog::static_literal("SELECT 'a' || x AS c0"), None);
    let _ = Map::<String, Value>::new();
}

//! The agent's drift test (#194 A4b, C4.2, plan-dashboard-to-agent.md §7):
//! after every snapshot build — the start, each refresh (nightly with
//! `refresh`), each reload — every declared question's panel runs once,
//! through the locked server's own panel path under a configured
//! `drift.principal` (claims set in the server config; never no claims, never
//! a bypass, M5), with every input at its default. The result is reduced to
//! **aggregates only** — the row count, the column names, NULL counts and the
//! numeric columns' min / max — and compared with the ranges recorded at
//! release (`drift.ranges`, a file of the deployment, never the bundle: H6).
//!
//! A question fails on a panel error (a missing column, a broken key), a
//! changed column set, an empty result that wasn't empty at release, a row
//! count off by more than 10× either way, a numeric column leaving its
//! recorded range by more than its width or 10× its magnitude, or no
//! record at all. A failing
//! question is switched off — refused with the same message as an undeclared
//! one (L6), the same for every caller — and logged by id and check code,
//! never a value (M5). The other questions keep working: the agent fails safe
//! rather than answering from broken data. Anyone who can shape the source
//! data can trigger a switch-off; that trade-off is intended (M5).

use super::agent::{default_values, item_names, run, AgentError, Dash};
use super::ask::parse_rows;
use super::ask::PanelRunner;
use crate::sql::question::{self, Question};
use serde_json::{json, Map, Value};
use std::time::Instant;

/// The rows a probe reads per question by default (one more tells "more").
pub const DEFAULT_MAX_ROWS: usize = 5_000;
/// The most: one more must still fit a panel page (`page.limit` ≤ 10000).
pub const MAX_ROWS: usize = 9_999;

/// `options.agent.drift`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriftOptions {
    /// The principal the probes run as (`principal.user`, `.groups`,
    /// `.claims`): claims from the server config, like a caller's.
    pub user: String,
    pub groups: Vec<String>,
    /// `(claim, value)`, sorted by claim.
    pub claims: Vec<(String, String)>,
    /// The deployment's file of release ranges (outside the bundle).
    pub ranges: String,
    /// Write the ranges from this start's probe instead of checking them (a
    /// release step; refused when the file already exists).
    pub record: bool,
    pub max_rows: usize,
}

impl DriftOptions {
    pub fn parse(v: &Value) -> Result<DriftOptions, String> {
        let o = v.as_object().ok_or("agent.drift: must be an object")?;
        if let Some(k) = o
            .keys()
            .find(|k| !matches!(k.as_str(), "principal" | "ranges" | "record" | "max_rows"))
        {
            return Err(format!("agent.drift: unknown key '{k}'"));
        }
        let p = o
            .get("principal")
            .and_then(Value::as_object)
            .ok_or("agent.drift.principal: {\"user\": \"…\", \"groups\": […], \"claims\": {…}} — the identity the drift test runs as")?;
        if let Some(k) = p
            .keys()
            .find(|k| !matches!(k.as_str(), "user" | "groups" | "claims"))
        {
            return Err(format!("agent.drift.principal: unknown key '{k}'"));
        }
        let user = p
            .get("user")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or("agent.drift.principal.user: a non-empty name")?
            .to_string();
        let mut groups: Vec<String> = match p.get("groups") {
            None => Vec::new(),
            Some(g) => g
                .as_array()
                .ok_or("agent.drift.principal.groups: a list of names")?
                .iter()
                .map(|g| g.as_str().map(str::to_string))
                .collect::<Option<_>>()
                .ok_or("agent.drift.principal.groups: a list of names")?,
        };
        groups.sort();
        groups.dedup();
        let mut claims: Vec<(String, String)> = Vec::new();
        if let Some(c) = p.get("claims") {
            let c = c
                .as_object()
                .ok_or("agent.drift.principal.claims: {\"<claim>\": \"value\", …}")?;
            for (k, v) in c {
                if !super::serving::valid_ident(k) {
                    return Err(format!(
                        "agent.drift.principal.claims: '{k}' is not a claim name"
                    ));
                }
                let v = v
                    .as_str()
                    .ok_or(format!("agent.drift.principal.claims.{k}: a string"))?;
                claims.push((k.to_ascii_lowercase(), v.to_string()));
            }
        }
        claims.sort();
        let ranges = o
            .get("ranges")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .ok_or(
                "agent.drift.ranges: the deployment's file of release ranges (outside the bundle)",
            )?
            .to_string();
        let record = match o.get("record") {
            None => false,
            Some(b) => b.as_bool().ok_or("agent.drift.record: true or false")?,
        };
        let max_rows = match o.get("max_rows") {
            None => DEFAULT_MAX_ROWS,
            Some(n) => n
                .as_u64()
                .filter(|n| (1..=MAX_ROWS as u64).contains(n))
                .ok_or(format!("agent.drift.max_rows: 1 to {MAX_ROWS}"))?
                as usize,
        };
        Ok(DriftOptions {
            user,
            groups,
            claims,
            ranges,
            record,
            max_rows,
        })
    }

    /// The principal as claims.
    pub fn claims(&self) -> super::access::Claims {
        super::access::Claims {
            user: self.user.clone(),
            groups: self.groups.clone(),
            attrs: self
                .claims
                .iter()
                .map(|(k, v)| (k.clone(), Some(v.clone())))
                .collect(),
        }
    }
}

/// One numeric column's range.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

/// A probe's aggregates (never a value but a numeric column's min / max).
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub rows: usize,
    /// The probe read `max_rows` and more followed.
    pub truncated: bool,
    /// `(column, NULL count, numeric range)`, in output order.
    pub columns: Vec<(String, usize, Option<Range>)>,
}

impl Stats {
    pub fn to_json(&self) -> Value {
        json!({
            "rows": self.rows,
            "truncated": self.truncated,
            "columns": self.columns.iter().map(|(n, nulls, r)| {
                let mut c = json!({ "name": n, "nulls": nulls });
                if let Some(r) = r {
                    c["min"] = json!(r.min);
                    c["max"] = json!(r.max);
                }
                c
            }).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Option<Stats> {
        let columns = v
            .get("columns")?
            .as_array()?
            .iter()
            .map(|c| {
                let name = c.get("name")?.as_str()?.to_string();
                let nulls = c.get("nulls")?.as_u64()? as usize;
                let range = match (
                    c.get("min").and_then(Value::as_f64),
                    c.get("max").and_then(Value::as_f64),
                ) {
                    (Some(min), Some(max)) => Some(Range { min, max }),
                    _ => None,
                };
                Some((name, nulls, range))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Stats {
            rows: v.get("rows")?.as_u64()? as usize,
            truncated: v.get("truncated").and_then(Value::as_bool).unwrap_or(false),
            columns,
        })
    }
}

/// Reduce a panel's rows to [`Stats`]: the facts-block column names
/// ([`item_names`]: presentation columns left out), NULL counts and
/// numeric ranges.
fn stats(rows: &[Map<String, Value>], names: &[Option<String>], max_rows: usize) -> Stats {
    let mut columns: Vec<(String, usize, Option<Range>)> = Vec::new();
    let shown = &rows[..rows.len().min(max_rows)];
    for r in shown {
        for (i, (k, v)) in r.iter().enumerate() {
            let name = match names.get(i) {
                Some(Some(n)) => n.clone(),
                Some(None) => continue,
                None => k.clone(),
            };
            let at = match columns.iter().position(|(n, _, _)| *n == name) {
                Some(at) => at,
                None => {
                    columns.push((name, 0, None));
                    columns.len() - 1
                }
            };
            let c = &mut columns[at];
            match v {
                Value::Null => c.1 += 1,
                Value::Number(n) => {
                    if let Some(f) = n.as_f64().filter(|f| f.is_finite()) {
                        let r = c.2.get_or_insert(Range { min: f, max: f });
                        r.min = r.min.min(f);
                        r.max = r.max.max(f);
                    }
                }
                _ => {}
            }
        }
    }
    Stats {
        rows: shown.len(),
        truncated: rows.len() > max_rows,
        columns,
    }
}

/// Probe one declared question: its inputs in order at their defaults (as a
/// first load sets them, for the drift principal), then its panel, read up
/// to `max_rows` rows.
pub fn probe(
    d: &Dash,
    q: &Question,
    max_rows: usize,
    runner: &dyn PanelRunner,
    deadline: Instant,
    log: &mut dyn FnMut(&AgentError, u16),
) -> Result<Stats, AgentError> {
    let plan = crate::sql::plan(d.script);
    let raws = crate::sql::ask::raw_statements(d.script);
    let mut vars = Map::new();
    for (i, p) in plan.iter().enumerate() {
        let Some(kind) = crate::inputs::input_kind(&p.roles) else {
            continue;
        };
        let names: Vec<String> = crate::sql::vars::input_vars(std::slice::from_ref(p))
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        if names.is_empty() {
            continue;
        }
        let slot = q.slots.iter().any(|s| s.var == names[0]);
        let out = match run(runner, d.id, i, &vars, None, deadline, log) {
            Ok(out) => out,
            Err(_) if !slot => continue,
            Err(e) => return Err(e),
        };
        for (n, v) in default_values(kind, &p.roles, &names, &out) {
            vars.insert(n, v);
        }
    }
    if q.slots.iter().any(|s| !vars.contains_key(&s.var)) {
        // a slot's input has no option for the drift principal: its domain
        // is gone
        return Err(AgentError::new(
            422,
            "no_match",
            "A slot of this question has no value.",
        ));
    }
    let page = json!({ "offset": 0, "limit": max_rows + 1 });
    let out = run(runner, d.id, q.stmt, &vars, Some(page), deadline, log)?;
    let rows = parse_rows(&out).map_err(|_| super::agent::failed())?;
    let stmt = raws
        .get(q.stmt)
        .map(|r| crate::sql::strip_comments(r))
        .unwrap_or_default();
    let roles = plan.get(q.stmt).map(|p| p.roles.as_slice()).unwrap_or(&[]);
    Ok(stats(&rows, &item_names(stmt.trim(), roles), max_rows))
}

/// Compare a probe with its release record: `None` (in range) or the check
/// code that failed.
pub fn check(release: &Stats, now: &Stats) -> Option<&'static str> {
    let names = |s: &Stats| s.columns.iter().map(|c| c.0.clone()).collect::<Vec<_>>();
    // an empty result carries no column names: only compare non-empty ones
    if now.rows > 0 && release.rows > 0 && names(now) != names(release) {
        return Some("drift/columns");
    }
    if now.rows == 0 && release.rows > 0 {
        return Some("drift/empty");
    }
    if release.rows > 0
        && !release.truncated
        && (now.rows > release.rows * 10 || now.rows * 10 < release.rows)
    {
        return Some("drift/rows");
    }
    for (name, _, range) in &now.columns {
        let (Some(r), Some(Some(rel))) = (
            range,
            release
                .columns
                .iter()
                .find(|c| c.0 == *name)
                .map(|c| c.2.as_ref()),
        ) else {
            continue;
        };
        // the plan's 10× rule, for values too: a column may move within its
        // recorded width or up to 10× its magnitude, not further
        let mag = rel.max.abs().max(rel.min.abs());
        let width = (rel.max - rel.min).abs().max(9.0 * mag).max(1e-9);
        if r.min < rel.min - width || r.max > rel.max + width {
            return Some("drift/range");
        }
    }
    None
}

/// One drift run over every declared question: `(record, off)` — the
/// probes' aggregates by question id (what `record` writes), and the
/// questions switched off with their check code. `release`: the recorded
/// ranges (`None` while recording: nothing is switched off but failing
/// probes).
pub fn run_all(
    d: &Dash,
    max_rows: usize,
    runner: &dyn PanelRunner,
    deadline: Instant,
    release: Option<&Value>,
) -> (Map<String, Value>, Vec<(String, String)>) {
    let mut record = Map::new();
    let mut off = Vec::new();
    for q in question::declared(d.script) {
        if d.off.contains(&q.id) {
            continue;
        }
        // the panel path's status for the log (never its text, N3)
        let mut status = 0u16;
        match probe(d, &q, max_rows, runner, deadline, &mut |_, st| status = st) {
            Err(e) => {
                off.push((
                    q.id.clone(),
                    if e.code == "no_match" {
                        "drift/no-slot-value".to_string()
                    } else {
                        format!("drift/failed, panel status {status}")
                    },
                ));
            }
            Ok(now) => {
                record.insert(q.id.clone(), now.to_json());
                if let Some(rel) = release {
                    match rel
                        .get("questions")
                        .and_then(|qs| qs.get(&q.id))
                        .and_then(Stats::from_json)
                    {
                        None => off.push((q.id.clone(), "drift/unrecorded".to_string())),
                        Some(rel) => {
                            if let Some(code) = check(&rel, &now) {
                                off.push((q.id.clone(), code.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    (record, off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;

    const DASH: &str = "-- @vars supplier\n\
        SELECT s AS supplier ::DROPDOWN FROM (VALUES ('Alpine'), ('Boreal')) t(s);\n\
        -- @question How reliable is {supplier} on delivery?\n\
        SELECT getvariable('supplier') AS \"Supplier\" ::TABLE, 0.9 AS \"On time %\";\n\
        -- @question How many orders are late?\n\
        SELECT 3 AS n ::METRIC, 'Late' ::LABEL;\n";

    /// Panel 0: the supplier input; panel 1: `rows1` (what the drift probe
    /// sees for the first question); panel 2: one count.
    struct Fake {
        rows1: RefCell<Value>,
        seen: RefCell<Vec<Value>>,
        fail1: bool,
    }

    impl PanelRunner for Fake {
        fn panel(&self, body: &str, _: Instant) -> Result<String, (u16, String)> {
            let v: Value = serde_json::from_str(body).unwrap();
            self.seen.borrow_mut().push(v.clone());
            match v["panel"].as_u64() {
                Some(0) => Ok(r#"[{"c0":"Alpine"},{"c0":"Boreal"}]"#.into()),
                Some(1) if self.fail1 => {
                    Err((400, "Binder Error: column canary-qq7 not found".into()))
                }
                Some(1) => Ok(json!({ "rows": self.rows1.borrow().clone() }).to_string()),
                _ => Ok(r#"[{"c0":3,"c1":"Late"}]"#.into()),
            }
        }
        fn hidden(&self) -> Vec<usize> {
            Vec::new()
        }
    }

    fn fake(rows1: Value) -> Fake {
        Fake {
            rows1: RefCell::new(rows1),
            seen: RefCell::new(Vec::new()),
            fail1: false,
        }
    }

    fn dash() -> Dash<'static> {
        Dash {
            id: "suppliers",
            title: "Suppliers",
            script: DASH,
            off: &[],
        }
    }

    fn rows(n: usize, pct: f64) -> Value {
        Value::Array(
            (0..n)
                .map(|i| json!({"Supplier": format!("S{i}"), "On time %": pct}))
                .collect(),
        )
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn options_parse() {
        let o = DriftOptions::parse(&json!({
            "principal": {"user": "drift", "groups": ["b", "a", "a"], "claims": {"Region": "north"}},
            "ranges": "/var/lib/agent/ranges.json"
        }))
        .unwrap();
        assert_eq!(o.groups, ["a", "b"]);
        assert_eq!(o.claims, [("region".to_string(), "north".to_string())]);
        assert!(!o.record && o.max_rows == DEFAULT_MAX_ROWS);
        let c = o.claims();
        assert_eq!((c.user.as_str(), c.attrs.len()), ("drift", 1));
        for bad in [
            json!({"ranges": "r.json"}),
            json!({"principal": {"user": " "}, "ranges": "r.json"}),
            json!({"principal": {"user": "d", "role": "x"}, "ranges": "r.json"}),
            json!({"principal": {"user": "d", "claims": {"a b": "x"}}, "ranges": "r.json"}),
            json!({"principal": {"user": "d"}}),
            json!({"principal": {"user": "d"}, "ranges": "r.json", "max_rows": 0}),
            json!({"principal": {"user": "d"}, "ranges": "r.json", "bypass": true}),
        ] {
            assert!(DriftOptions::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_probe_runs_every_input_at_its_default_and_keeps_aggregates_only() {
        let f = fake(rows(4, 0.9));
        let q = &question::declared(DASH)[0];
        let s = probe(&dash(), q, 100, &f, soon(), &mut |_, _| {}).unwrap();
        // the slot took its input's first option, as a first load does
        let seen = f.seen.borrow();
        assert_eq!(seen.last().unwrap()["vars"]["supplier"], "Alpine");
        assert_eq!(seen.last().unwrap()["page"]["limit"], 101);
        assert_eq!(s.rows, 4);
        let names: Vec<&str> = s.columns.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["Supplier", "On time %"]);
        assert_eq!(s.columns[1].2, Some(Range { min: 0.9, max: 0.9 }));
        // the record holds no row value: no supplier name, only counts and
        // the numeric range
        let rec = s.to_json().to_string();
        assert!(!rec.contains("S0") && !rec.contains("S3"), "{rec}");
        assert_eq!(Stats::from_json(&s.to_json()), Some(s));
    }

    #[test]
    fn checks_flag_each_kind_of_drift() {
        let release = stats(
            &rows(20, 0.9)
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect::<Vec<_>>(),
            &[],
            100,
        );
        let at = |n: usize, pct: f64| {
            let r = rows(n, pct);
            let maps: Vec<Map<String, Value>> = r
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            stats(&maps, &[], 1000)
        };
        assert_eq!(check(&release, &at(25, 0.92)), None);
        assert_eq!(check(&release, &at(0, 0.9)), Some("drift/empty"));
        assert_eq!(check(&release, &at(201, 0.9)), Some("drift/rows"));
        assert_eq!(check(&release, &at(1, 0.9)), Some("drift/rows"));
        assert_eq!(check(&release, &at(20, 90.0)), Some("drift/range"));
        let mut renamed = at(20, 0.9);
        renamed.columns[1].0 = "On-time share".into();
        assert_eq!(check(&release, &renamed), Some("drift/columns"));
    }

    #[test]
    fn a_failing_or_drifted_question_is_switched_off_and_the_others_kept() {
        let f = fake(rows(20, 0.9));
        let (record, off) = run_all(&dash(), 100, &f, soon(), None);
        assert!(off.is_empty(), "{off:?}");
        let release = json!({ "questions": record });
        // in range: nothing off
        let (_, off) = run_all(&dash(), 100, &f, soon(), Some(&release));
        assert!(off.is_empty(), "{off:?}");
        // the first question's data drifts (empty): only it goes off
        *f.rows1.borrow_mut() = json!([]);
        let (_, off) = run_all(&dash(), 100, &f, soon(), Some(&release));
        assert_eq!(
            off,
            [(
                "how_reliable_is_supplier_on_delivery".to_string(),
                "drift/empty".to_string()
            )]
        );
        // a panel error (a missing column): off, and DuckDB's text (with its
        // canary) appears nowhere in what the run returns
        let mut g = fake(rows(20, 0.9));
        g.fail1 = true;
        let (record, off) = run_all(&dash(), 100, &g, soon(), Some(&release));
        assert_eq!(
            off,
            [(
                "how_reliable_is_supplier_on_delivery".to_string(),
                "drift/failed, panel status 400".to_string()
            )]
        );
        assert!(!format!("{record:?}{off:?}").contains("canary-qq7"));
        // a question with no record is off (fail closed)
        let partial = json!({ "questions": { "how_many_orders_are_late": release["questions"]["how_many_orders_are_late"] } });
        let (_, off) = run_all(&dash(), 100, &f, soon(), Some(&partial));
        assert!(
            off.contains(&(
                "how_reliable_is_supplier_on_delivery".to_string(),
                "drift/unrecorded".to_string()
            )),
            "{off:?}"
        );
    }
}

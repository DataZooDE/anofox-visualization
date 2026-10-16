use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use ggplot_rs::prelude::Value;

// A tiny provider: setup is a no-op; queries return canned columns based on the
// rewritten SQL's aliases (c0/c1…) so charts/KPIs get sensible shapes.
struct Mock;
fn s(v: &[&str]) -> Vec<Value> {
    v.iter().map(|x| Value::Str((*x).into())).collect()
}
fn f(v: &[f64]) -> Vec<Value> {
    v.iter().map(|x| Value::Float(*x)).collect()
}
impl DataProvider for Mock {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        // directives: a single number
        if sql.contains("AS c0")
            && !sql.contains("AS c1")
            && sql.contains("SELECT")
            && sql.len() < 40
        {
            return Ok(vec![("c0".into(), f(&[2.0]))]);
        }
        // metric query: c0 number, c1 label
        if sql.contains("CAST(") {
            return Ok(vec![
                ("c0".into(), f(&[12400.0])),
                ("c1".into(), s(&["Revenue"])),
            ]);
        }
        // heading: one string
        if !sql.contains("::") && sql.matches("AS c").count() == 1 && sql.contains("'") {
            return Ok(vec![("c0".into(), s(&["Section"]))]);
        }
        // default: a bar-shaped result (x strings, y numbers, title)
        Ok(vec![
            ("c0".into(), s(&["W1", "W2", "W3", "W4"])),
            ("c1".into(), f(&[30.0, 41.0, 26.0, 48.0])),
            ("c2".into(), s(&["Weekly"])),
        ])
    }
}

#[test]
fn composes_a_dashboard_svg() {
    let script = "\
        SELECT 2::COLUMNS;\n\
        SELECT 'Overview'::LABEL;\n\
        SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM t;\n\
        SELECT week::XAXIS, sum(n)::BARCHART, 'Weekly'::TITLE FROM t GROUP BY ALL;\n\
        SELECT week::XAXIS, sum(n)::LINECHART, 'Trend'::TITLE FROM t GROUP BY ALL;";
    let mut db = Mock;
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("</svg>"));
    // one outer svg + nested panel svgs
    assert!(
        svg.matches("<svg").count() >= 4,
        "expected several nested panels"
    );
    // the KPI value formatted as money
    assert!(
        svg.contains("$12,400"),
        "money KPI missing: {}",
        &svg[..svg.len().min(400)]
    );
}

// A contract plot call (`SELECT anofox_plot_terms(t) FROM … t`) runs as
// `to_json(list(t))` and renders through the contract renderer — one panel per
// result row, titled by ::TITLE + the GROUP BY key.
struct ContractMock;
impl DataProvider for ContractMock {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        assert!(sql.contains("to_json(list("), "{sql}");
        let rows = r#"[{"term":"price","estimate":-18.2,"conf_low":-20.1,"conf_high":-16.3},
                       {"term":"promo","estimate":14.1,"conf_low":12.0,"conf_high":16.2}]"#;
        Ok(vec![
            ("c0".into(), s(&["ols", "huber"])),
            ("c1".into(), s(&[rows, rows])),
            ("c2".into(), s(&["{}", "{}"])),
            ("c3".into(), s(&["Coefficients", "Coefficients"])),
        ])
    }
}

#[test]
fn renders_contract_calls() {
    let script = "SELECT model_id, anofox_plot_terms(t), 'Coefficients'::TITLE \
                  FROM tidy t GROUP BY model_id;";
    let svg =
        render_dashboard_svg(script, &mut ContractMock, &DashboardOptions::default()).unwrap();
    assert!(svg.contains("Coefficients — ols"), "{svg}");
    assert!(svg.contains("Coefficients — huber"));
    assert!(svg.contains("promo"));
}

// A provider answering every query with the same canned columns.
struct Fixed(Vec<(String, Vec<Value>)>);
impl DataProvider for Fixed {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        Ok(self.0.clone())
    }
}

// #44: a DATE ::XLINE (the DuckDB CLI's JSON writes it as "2026-03-15") on a
// date axis draws its dashed line instead of failing a DOUBLE cast.
#[test]
fn a_date_xline_draws_on_a_date_axis() {
    let script = "SELECT d::XAXIS, v::LINECHART, DATE '2026-02-03'::XLINE FROM t;";
    let days = [
        "2026-02-01",
        "2026-02-02",
        "2026-02-03",
        "2026-02-04",
        "2026-02-05",
    ];
    let mut db = Fixed(vec![
        ("c0".into(), s(&days)),
        ("c1".into(), f(&[1.0, 3.0, 2.0, 5.0, 4.0])),
        ("c2".into(), s(&["2026-02-03"; 5])),
    ]);
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(svg.contains("stroke-dasharray"), "no XLINE drawn");
}

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
        if sql.contains("::") == false && sql.matches("AS c").count() == 1 && sql.contains("'") {
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

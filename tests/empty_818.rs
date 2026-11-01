//! Empty results in the headless SVG (#818, as the page draws them): a chart
//! without rows says so (its `::EMPTY` message, else a generic note), a
//! table only with `::EMPTY`, and a KPI tile with no row shows "–".

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use ggplot_rs::prelude::Value;

/// A literal statement (`::EMPTY`) answers its text; every other
/// query returns no rows — as the servers do, with no columns at all.
struct NoRows;

impl DataProvider for NoRows {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        // (the plan strips the role casts: `SELECT 'msg'`)
        if let Some(m) = sql.split('\'').nth(1).filter(|_| !sql.contains("FROM")) {
            return Ok(vec![("c0".into(), vec![Value::Str(m.into())])]);
        }
        Ok(Vec::new())
    }
}

fn svg(script: &str) -> String {
    render_dashboard_svg(script, &mut NoRows, &DashboardOptions::default()).unwrap()
}

#[test]
fn a_chart_without_rows_says_so() {
    let s = svg("SELECT x ::XAXIS, y ::BARCHART FROM t;");
    assert!(s.contains(">No data for this selection</text>"), "{s}");
    let s = svg("SELECT 'Nothing in this range'::EMPTY;\nSELECT x ::XAXIS, y ::BARCHART FROM t;");
    assert!(s.contains(">Nothing in this range</text>"), "{s}");
    assert!(!s.contains("No data for this selection"));
}

#[test]
fn a_table_shows_its_empty_message_only_when_it_has_one() {
    let s = svg("SELECT a ::TABLE, b FROM t;");
    assert!(!s.contains("No data for this selection"), "{s}");
    let s = svg("SELECT 'Not the primary source of any part'::EMPTY;\nSELECT a ::TABLE, b FROM t;");
    assert!(
        s.contains(">Not the primary source of any part</text>"),
        "{s}"
    );
}

#[test]
fn the_message_is_for_the_next_panel_only() {
    let s = svg("SELECT 'first only'::EMPTY;\nSELECT a ::TABLE FROM t;\nSELECT b ::TABLE FROM t;");
    assert_eq!(s.matches(">first only</text>").count(), 1, "{s}");
}

#[test]
fn a_kpi_without_a_row_is_a_dash_tile() {
    let s = svg(
        "SELECT v ::METRIC, 'On time'::LABEL FROM t;\nSELECT w ::MONEY, 'Spend'::LABEL FROM t;",
    );
    assert!(!s.contains("No data for this selection"), "{s}");
    assert!(s.matches(">–</text>").count() >= 2, "{s}");
}

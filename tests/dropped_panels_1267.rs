//! #1267: the headless render says which panels it left out — a panel whose
//! query failed, and one whose own SELECT carries a layout role (#685) —
//! instead of returning fewer panels than the script has, without a word.

use anofox_visualization::dashboard::{
    render_dashboard, render_dashboard_svg, DashboardOptions, DataProvider, DroppedPanel,
};
use ggplot_rs::prelude::Value;

/// Three numeric columns for every query; a query on `missing` fails, as a
/// provider that doesn't serve a statement does.
struct Rows;

impl DataProvider for Rows {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if sql.contains("missing") {
            return Err("Catalog Error: Table with name missing does not exist!".into());
        }
        Ok((0..3)
            .map(|i| {
                (
                    format!("c{i}"),
                    vec![Value::Float(6.0), Value::Float(2.0 + i as f64)],
                )
            })
            .collect())
    }
}

const SCRIPT: &str = "\
SELECT x::XAXIS, y::BARCHART FROM t;
SELECT x::XAXIS, y::LINECHART FROM missing;
SELECT 8::COL, x::XAXIS, y::BARCHART FROM t;
SELECT 6::COL;
SELECT n::HEIGHT FROM missing;
SELECT x::XAXIS, y::BARCHART FROM t;";

#[test]
fn a_failed_query_and_an_inline_layout_role_are_reported() {
    let r = render_dashboard(SCRIPT, &mut Rows, &DashboardOptions::default()).unwrap();
    let stmts: Vec<usize> = r.dropped.iter().map(|d| d.stmt).collect();
    // the ::COL and ::HEIGHT directives are not panels, failed or not
    assert_eq!(stmts, vec![1, 2], "{:?}", r.dropped);
    assert!(
        r.dropped[0]
            .reason
            .starts_with("query failed: Catalog Error"),
        "{:?}",
        r.dropped[0]
    );
    assert!(
        r.dropped[1]
            .reason
            .contains("::COL makes this whole statement")
            && r.dropped[1].reason.contains("::BARCHART"),
        "{:?}",
        r.dropped[1]
    );
    // the picture itself is what render_dashboard_svg returns
    let svg = render_dashboard_svg(SCRIPT, &mut Rows, &DashboardOptions::default()).unwrap();
    assert_eq!(svg, r.svg);
}

#[test]
fn a_clean_script_drops_nothing() {
    let r = render_dashboard(
        "SELECT 6::COL;\nSELECT x::XAXIS, y::BARCHART FROM t;",
        &mut Rows,
        &DashboardOptions::default(),
    )
    .unwrap();
    assert_eq!(r.dropped, Vec::<DroppedPanel>::new());
}

#[test]
fn a_statement_left_out_on_purpose_is_not_reported() {
    // #764: one tab only — the failing panel isn't on it
    let opts = DashboardOptions {
        only: Some(vec![0, 5]),
        ..Default::default()
    };
    let r = render_dashboard(SCRIPT, &mut Rows, &opts).unwrap();
    assert!(r.dropped.is_empty(), "{:?}", r.dropped);
}

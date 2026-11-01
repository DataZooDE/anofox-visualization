//! Narrow static renders (#816, an export for a chat card): below 640 px
//! the dashboard is one column like the page's phone layout — every panel
//! full width, KPI tiles two per row, a smaller title.

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use ggplot_rs::prelude::Value;

/// Every statement answers one row: the number 6 in each of its columns
/// (`6::COL` included), text for a title.
struct Six;

impl DataProvider for Six {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if let Some(t) = sql.split('\'').nth(1) {
            return Ok(vec![("c0".into(), vec![Value::Str(t.into())])]);
        }
        let n = sql.matches(',').count() + 1;
        Ok((0..n)
            .map(|i| (format!("c{i}"), vec![Value::Float(6.0)]))
            .collect())
    }
}

const SCRIPT: &str = "SELECT 'Which suppliers put our supply at risk?'::DASHBOARD_TITLE;
SELECT 1 ::METRIC; SELECT 2 ::METRIC; SELECT 3 ::METRIC; SELECT 4 ::METRIC;
SELECT 6::COL;
SELECT x ::XAXIS, y ::BARCHART FROM t;
SELECT 6::COL;
SELECT x ::XAXIS, y ::LINECHART FROM t;";

fn svg(width: u32) -> String {
    let o = DashboardOptions {
        width,
        ..Default::default()
    };
    render_dashboard_svg(SCRIPT, &mut Six, &o).unwrap()
}

/// The (x, width) of each card.
fn cards(svg: &str) -> Vec<(f64, f64)> {
    svg.split("<rect x=\"")
        .skip(1)
        .filter(|r| r.contains("rx=\"6\""))
        .map(|r| {
            let x: f64 = r.split('"').next().unwrap().parse().unwrap();
            let w: f64 = r
                .split("width=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            (x, w)
        })
        .collect()
}

#[test]
fn below_640_px_panels_are_full_width_and_tiles_two_per_row() {
    let c = cards(&svg(480));
    assert_eq!(c.len(), 6, "{c:?}");
    // two charts: full width (480 - 2 x 16 padding - 16 gap), both at x = 16
    for (x, w) in &c[4..] {
        assert_eq!((*x, *w), (16.0, 432.0), "{c:?}");
    }
    // four tiles: two per row, two distinct x positions
    let mut xs: Vec<i64> = c[..4].iter().map(|(x, _)| *x as i64).collect();
    xs.sort_unstable();
    xs.dedup();
    assert_eq!(xs.len(), 2, "{c:?}");
}

#[test]
fn wide_renders_keep_their_columns() {
    let c = cards(&svg(1200));
    // the two charts side by side (6::COL each)
    assert!(c[4].0 < c[5].0, "{c:?}");
    // four tiles in one row
    let xs: std::collections::BTreeSet<i64> = c[..4].iter().map(|(x, _)| *x as i64).collect();
    assert_eq!(xs.len(), 4, "{c:?}");
}

#[test]
fn a_narrow_title_is_smaller() {
    assert!(svg(480).contains("font-size=\"20\" font-weight=\"800\""));
    assert!(svg(1200).contains("font-size=\"24\" font-weight=\"800\""));
}

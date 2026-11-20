//! #940: `::HEIGHT` as pixels, a ratio of the panel's width or a share of
//! the window, in the static renderer (`dashboard --svg`, exports) and the
//! lint (`layout/bad-height`).

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::lint::{self, LintOptions};
use ggplot_rs::prelude::Value;
use serde_json::{json, Map};

/// `::HEIGHT` answers `spec`; the chart a small bar series.
struct Rows(&'static str);
impl DataProvider for Rows {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if !sql.contains("AS c1") {
            return Ok(vec![("c0".into(), vec![Value::Str(self.0.into())])]);
        }
        Ok(vec![
            (
                "c0".into(),
                ["a", "b", "c"].map(|s| Value::Str(s.into())).to_vec(),
            ),
            ("c1".into(), [1.0, 3.0, 2.0].map(Value::Float).to_vec()),
        ])
    }
}

/// The whole dashboard's height with `spec` as the chart's `::HEIGHT`.
fn height(spec: &'static str) -> f64 {
    let sql = "SELECT h::HEIGHT FROM t;\nSELECT x::XAXIS, y::BARCHART FROM t;";
    let svg = render_dashboard_svg(sql, &mut Rows(spec), &DashboardOptions::default()).unwrap();
    let root = &svg[..svg.find('>').unwrap()];
    root.split(" height=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn every_form_sizes_the_panel() {
    let px = height("300");
    assert!(height("500") - px > 150.0, "pixels as before");
    assert_eq!(
        height("40vh"),
        height("360"),
        "a share of a 900 px reference window"
    );
    assert_eq!(height("0.5"), height("2:1"), "a ratio, either way");
    let (half, full) = (height("2:1"), height("1:1"));
    assert!(
        full - half > 200.0,
        "the ratio follows the panel's width: {half} {full}"
    );
    assert_eq!(height("tall"), height(""), "not a height: the default");
}

#[test]
fn lint_names_a_bad_height() {
    let codes = |spec: &str| -> Vec<&'static str> {
        let row: Map<String, serde_json::Value> =
            serde_json::from_value(json!({ "c0": spec })).unwrap();
        lint::check_opts(
            "SELECT 'h'::HEIGHT;\nSELECT 1 AS v ::METRIC;",
            |_q| Ok(vec![row.clone()]),
            LintOptions {
                design: false,
                ..Default::default()
            },
        )
        .into_iter()
        .map(|d| d.code)
        .filter(|c| c.starts_with("layout/bad-height"))
        .collect()
    };
    for ok in ["320", "16:9", "0.5", "40vh"] {
        assert!(codes(ok).is_empty(), "{ok}");
    }
    for bad in ["tall", "120vh", "7"] {
        assert_eq!(codes(bad), vec!["layout/bad-height"], "{bad}");
    }
}

/// #1286: `SELECT 12 ::SPAN, 680 ::HEIGHT;` sets both for the next panel,
/// as two statements do.
#[test]
fn span_and_height_in_one_statement() {
    struct Layout;
    impl DataProvider for Layout {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("AS c1") && !sql.contains("680") {
                return Ok(vec![
                    (
                        "c0".into(),
                        ["a", "b", "c"].map(|s| Value::Str(s.into())).to_vec(),
                    ),
                    ("c1".into(), [1.0, 3.0, 2.0].map(Value::Float).to_vec()),
                ]);
            }
            // a layout statement: one column per value
            let vals: Vec<&str> = ["12", "680"]
                .into_iter()
                .filter(|v| sql.contains(v))
                .collect();
            Ok(vals
                .iter()
                .enumerate()
                .map(|(i, v)| (format!("c{i}"), vec![Value::Float(v.parse().unwrap())]))
                .collect())
        }
    }
    let size = |layout: &str| {
        let sql = format!("{layout}\nSELECT x::XAXIS, y::BARCHART FROM t;");
        let svg = render_dashboard_svg(&sql, &mut Layout, &DashboardOptions::default()).unwrap();
        let root = svg[..svg.find('>').unwrap()].to_string();
        let attr = |a: &str| -> f64 {
            root.split(&format!(" {a}=\""))
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        (attr("width"), attr("height"))
    };
    let two = size("SELECT 12::SPAN;\nSELECT 680::HEIGHT;");
    assert_eq!(size("SELECT 12::SPAN, 680::HEIGHT;"), two);
    assert!(two.1 > size("SELECT 12::SPAN;").1 + 300.0, "{two:?}");
}

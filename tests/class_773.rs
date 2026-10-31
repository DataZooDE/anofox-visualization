//! `::CLASS` (#773): never a table cell or chart text in the headless SVG,
//! and `--check` warns about a literal name the page would drop.

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::lint;
use ggplot_rs::prelude::Value;

/// Every query: a key, a value, the title and the class (as c0..c3).
struct Mock;

impl DataProvider for Mock {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        let s = |v: &str| vec![Value::Str(v.into()), Value::Str(v.into())];
        Ok(vec![
            (
                "c0".into(),
                vec![Value::Str("a".into()), Value::Str("b".into())],
            ),
            ("c1".into(), vec![Value::Float(1.0), Value::Float(2.0)]),
            ("c2".into(), s("TitleX")),
            ("c3".into(), s("kpi-hero")),
        ])
    }
}

#[test]
fn the_class_is_not_drawn_in_a_chart_or_a_table() {
    let script = "\
        SELECT k::XAXIS, v::BARCHART, 'TitleX'::TITLE, 'kpi-hero'::CLASS FROM t;\n\
        SELECT k ::TABLE, v, 'TitleX'::TITLE, 'kpi-hero'::CLASS FROM t;";
    let svg = render_dashboard_svg(script, &mut Mock, &DashboardOptions::default()).unwrap();
    assert!(!svg.contains("kpi-hero"), "{svg}");
    assert_eq!(svg.matches(">TitleX<").count(), 2, "{svg}");
}

fn class_warnings(script: &str) -> Vec<String> {
    let row = || {
        let mut m = serde_json::Map::new();
        m.insert("c0".into(), serde_json::json!(1));
        Ok(vec![m])
    };
    lint::check(script, |_| row())
        .into_iter()
        .filter(|d| d.code == "class/name")
        .map(|d| d.message)
        .collect()
}

#[test]
fn a_literal_class_name_the_page_drops_is_a_warning() {
    for ok in [
        "SELECT 1::METRIC, 'kpi-hero'::CLASS;",
        "SELECT 1::METRIC, 'kpi-hero tall 2col'::CLASS;",
        // not a literal: the data decides, nothing to check
        "SELECT 1::METRIC, CASE WHEN 1 > 0 THEN 'warn' END::CLASS;",
    ] {
        assert_eq!(class_warnings(ok), Vec::<String>::new(), "{ok}");
    }
    for (bad, name) in [
        ("SELECT 1::METRIC, 'Kpi-Hero'::CLASS;", "Kpi-Hero"),
        ("SELECT 1::METRIC, 'kpi_hero'::CLASS;", "kpi_hero"),
        ("SELECT 1::METRIC, '-x'::CLASS;", "-x"),
        ("SELECT 1::METRIC, 'a b c d e'::CLASS;", "e"),
    ] {
        let w = class_warnings(bad);
        assert!(
            w.len() == 1 && w[0].contains(&format!("'{name}'")),
            "{bad}: {w:?}"
        );
    }
}

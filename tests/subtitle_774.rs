//! `::SUBTITLE` (#774) in the headless SVG: a muted line under the title bar
//! of a chart and of a table; in a table it is not a column.

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use ggplot_rs::prelude::Value;

/// Every query: a key, a value, the title and the subtitle (as c0..c3).
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
            ("c3".into(), s("SubLine under it")),
        ])
    }
}

#[test]
fn subtitle_under_the_title_of_a_chart_and_a_table() {
    let script = "\
        SELECT k::XAXIS, v::BARCHART, 'TitleX'::TITLE, 'SubLine under it'::SUBTITLE FROM t;\n\
        SELECT k ::TABLE, v, 'TitleX'::TITLE, 'SubLine under it'::SUBTITLE FROM t;";
    let svg = render_dashboard_svg(script, &mut Mock, &DashboardOptions::default()).unwrap();
    // once per panel, as the muted subtitle line — never a table cell or header
    assert_eq!(svg.matches("SubLine under it").count(), 2, "{svg}");
    assert_eq!(svg.matches("class=\"dp-subtitle\"").count(), 2, "{svg}");
    assert_eq!(svg.matches(">TitleX<").count(), 2, "{svg}");
}

#[test]
fn a_long_subtitle_is_shortened_with_its_full_text_on_hover() {
    struct Long;
    impl DataProvider for Long {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            let long = "word ".repeat(80);
            Ok(vec![
                ("c0".into(), vec![Value::Str("a".into())]),
                ("c1".into(), vec![Value::Float(1.0)]),
                ("c2".into(), vec![Value::Str("T".into())]),
                ("c3".into(), vec![Value::Str(long)]),
            ])
        }
    }
    let script = "SELECT k::XAXIS, v::BARCHART, 'T'::TITLE, 'x'::SUBTITLE FROM t;";
    let svg = render_dashboard_svg(script, &mut Long, &DashboardOptions::default()).unwrap();
    assert!(svg.contains("…<title>word word"), "{svg}");
}

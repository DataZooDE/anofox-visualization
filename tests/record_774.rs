//! `::RECORD` (#774) in the headless SVG: a STRUCT / JSON object's fields,
//! or the first row's columns, as name/value lines.

use anofox_visualization::dashboard::{
    record_pairs, render_dashboard_svg, DashboardOptions, DataProvider,
};
use anofox_visualization::Role;
use ggplot_rs::prelude::Value;

struct Rows(Vec<(String, Vec<Value>)>);

impl DataProvider for Rows {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        Ok(self.0.clone())
    }
}

fn s(v: &str) -> Value {
    Value::Str(v.into())
}

#[test]
fn an_objects_fields_or_the_first_rows_columns() {
    // one column holding an object (a STRUCT / MAP / JSON value), plus a title
    let rows = vec![
        (
            "r".to_string(),
            vec![s(
                r#"{"Name": "Ada", "City": "Berlin", "Tags": [1, 2], "Gone": null}"#,
            )],
        ),
        ("t".to_string(), vec![s("Customer")]),
    ];
    let (pairs, n) = record_pairs(&rows, &[(0, Role::Record), (1, Role::Title)]);
    assert_eq!(n, 1);
    assert_eq!(
        pairs,
        vec![
            ("Name".into(), "Ada".into()),
            ("City".into(), "Berlin".into()),
            ("Tags".into(), "[1,2]".into()),
            ("Gone".into(), String::new()),
        ]
    );
    // several columns: the first row's
    let rows = vec![
        ("name".to_string(), vec![s("Ada"), s("Bob")]),
        ("city".to_string(), vec![s("Berlin"), s("Paris")]),
    ];
    let (pairs, n) = record_pairs(&rows, &[(0, Role::Record)]);
    assert_eq!(
        (pairs, n),
        (
            vec![
                ("name".into(), "Ada".into()),
                ("city".into(), "Berlin".into())
            ],
            2
        )
    );
    // a plain string is a value, not an object
    let (pairs, _) = record_pairs(&[("x".into(), vec![s("plain")])], &[(0, Role::Record)]);
    assert_eq!(pairs, vec![("x".into(), "plain".into())]);
}

#[test]
fn the_svg_card() {
    let mut p = Rows(vec![
        ("name".to_string(), vec![s("Ada <b>"), s("Bob")]),
        ("city".to_string(), vec![s("Berlin"), s("Paris")]),
        ("t".to_string(), vec![s("Customer"), s("Customer")]),
    ]);
    let svg = render_dashboard_svg(
        "SELECT name ::RECORD, city, 'Customer'::TITLE FROM t;",
        &mut p,
        &DashboardOptions::default(),
    )
    .unwrap();
    assert!(svg.contains("class=\"dp-record\""), "{svg}");
    assert!(
        svg.contains(">name<") && svg.contains(">Ada &lt;b&gt;<") && svg.contains(">Berlin<"),
        "{svg}"
    );
    assert!(svg.contains("The first of 2 rows."), "{svg}");
    assert!(!svg.contains(">Bob<"), "{svg}");
}

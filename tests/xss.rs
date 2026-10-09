//! XSS regression corpus: every user-controlled text channel the Rust side
//! writes into SVG/HTML (chart titles, axis categories, legends, labels, KPI
//! captions, table headers/cells, headings, gauge/radar/candlestick/calendar
//! text, map labels, axis formats, error messages) is fed attribute- and
//! element-breaking payloads; the output must stay well-formed and inert.

mod common;

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::{render_spec, render_spec_checked, Value};
use common::{assert_safe_pre, assert_safe_svg};

const PAYLOADS: &[&str] = &[
    r#"<script>alert(1)</script>"#,
    r#""><svg onload=alert(1)>"#,
    r#"' onmouseover='alert(1)"#,
    r#"" onmouseover="alert(1)" x=""#,
    r#"</title><script>alert(1)</script>"#,
    r#"<img src=x onerror=alert(1)>"#,
    r#"]]><foreignObject><iframe src=javascript:alert(1)>"#,
    "&lt;script&gt; & &amp;",
];

fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// Render one spec and check it; `rows` gets `{P}` replaced by the payload.
fn run(name: &str, roles: &str, rows: &str, p: &str) {
    let rows = rows.replace("{P}", &js(p));
    let spec = format!(r#"{{"rows":{rows},"roles":{roles},"width":400,"height":300}}"#);
    let ctx = format!("{name} / {p}");
    match render_spec_checked(&spec) {
        Ok(svg) => {
            assert_safe_svg(&svg, &ctx);
            assert!(!svg.contains("<script"), "{ctx}: raw <script");
        }
        Err(_) => assert_safe_pre(&render_spec(&spec), &ctx),
    }
}

#[test]
fn chart_text_channels_are_escaped() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "bar title + x categories",
            r#"[[0,"XAXIS"],[1,"BARCHART"],[2,"LABEL"]]"#,
            r#"[{"c0":{P},"c1":1,"c2":{P}},{"c0":"b","c1":2,"c2":{P}}]"#,
        ),
        (
            "line legend (category)",
            r#"[[0,"XAXIS"],[1,"CATEGORY"],[2,"LINECHART"]]"#,
            r#"[{"c0":1,"c1":{P},"c2":1},{"c0":2,"c1":{P},"c2":2},{"c0":1,"c1":"b","c2":3}]"#,
        ),
        (
            "combo names",
            r#"[[0,"XAXIS"],[1,"LINECHART","<b>\"s\""],[2,"SCATTER","' x"]]"#,
            r#"[{"c0":1,"c1":1,"c2":{P}},{"c0":2,"c1":2,"c2":3}]"#,
        ),
        (
            "pie slices",
            r#"[[0,"CATEGORY"],[1,"PIE"]]"#,
            r#"[{"c0":{P},"c1":1},{"c0":"b","c1":2}]"#,
        ),
        (
            "gauge title + zone labels",
            r#"[[0,"GAUGE"],[1,"LABEL"],[2,"LABELS"]]"#,
            r#"[{"c0":5,"c1":{P},"c2":{P}}]"#,
        ),
        (
            "radar axes + series",
            r#"[[0,"XAXIS"],[1,"CATEGORY"],[2,"RADAR"],[3,"LABEL"]]"#,
            r#"[{"c0":{P},"c1":{P},"c2":1,"c3":{P}},{"c0":"b","c1":"s","c2":2},{"c0":"c","c1":"s","c2":3}]"#,
        ),
        (
            "candlestick x",
            r#"[[0,"XAXIS"],[1,"OPEN"],[2,"HIGH"],[3,"LOW"],[4,"CANDLESTICK"],[5,"LABEL"]]"#,
            r#"[{"c0":{P},"c1":1,"c2":3,"c3":0,"c4":2,"c5":{P}},{"c0":"d","c1":2,"c2":4,"c3":1,"c4":1}]"#,
        ),
        (
            "calendar title",
            r#"[[0,"XAXIS"],[1,"CALENDAR"],[2,"LABEL"]]"#,
            r#"[{"c0":"2024-01-01","c1":1,"c2":{P}},{"c0":"2024-02-01","c1":2,"c2":{P}}]"#,
        ),
        (
            "heatmap axes",
            r#"[[0,"XAXIS"],[1,"YAXIS"],[2,"HEATMAP"]]"#,
            r#"[{"c0":{P},"c1":{P},"c2":1},{"c0":"b","c1":"c","c2":2}]"#,
        ),
        (
            "boxplot groups",
            r#"[[0,"XAXIS"],[1,"BOXPLOT"]]"#,
            r#"[{"c0":{P},"c1":1},{"c0":{P},"c1":3},{"c0":"b","c1":2},{"c0":"b","c1":5}]"#,
        ),
        (
            "y format",
            r#"[[0,"XAXIS"],[1,"BARCHART"],[2,"YFORMAT"]]"#,
            r#"[{"c0":"a","c1":1000,"c2":{P}},{"c0":"b","c1":2000,"c2":{P}}]"#,
        ),
        (
            "data labels",
            r#"[[0,"XAXIS"],[1,"BARCHART"],[2,"DATALABELS"]]"#,
            r#"[{"c0":{P},"c1":1,"c2":12}]"#,
        ),
        (
            "map labels",
            r#"[[0,"MAP"],[1,"LABEL"],[2,"CHOROPLETH"]]"#,
            r#"[{"c0":"POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))","c1":{P},"c2":1}]"#,
        ),
        ("heading only", r#"[[0,"LABEL"]]"#, r#"[{"c0":{P}}]"#),
        (
            "empty-data note title",
            r#"[[0,"BARCHART"],[1,"LABEL"]]"#,
            r#"[]"#,
        ),
    ];
    for p in PAYLOADS {
        for (name, roles, rows) in cases {
            run(name, roles, rows, p);
        }
    }
}

#[test]
fn error_messages_are_escaped() {
    for p in PAYLOADS {
        // Unknown role token echoed in the error.
        let spec = format!(r#"{{"rows":[],"roles":[[0,{}]]}}"#, js(p));
        let html = render_spec(&spec);
        assert_safe_pre(&html, p);
        // A JSON syntax error near the payload.
        assert_safe_pre(&render_spec(&format!("{p}{{")), p);
    }
}

/// Canned provider returning the payload in every text cell.
struct Evil(&'static str);
impl DataProvider for Evil {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        let s = || Value::Str(self.0.to_string());
        if sql.contains("CAST(") {
            // KPI: value + caption
            return Ok(vec![
                ("c0".into(), vec![Value::Float(1.0)]),
                ("c1".into(), vec![s()]),
            ]);
        }
        Ok(vec![
            (self.0.to_string(), vec![s(), s()]),
            ("c1".into(), vec![s(), Value::Float(2.0)]),
            ("c2".into(), vec![s(), s()]),
        ])
    }
}

#[test]
fn dashboard_text_channels_are_escaped() {
    let script = "SELECT 'x'::LABEL;\n\
                  SELECT sum(v)::MONEY, 'cap'::LABEL FROM t;\n\
                  SELECT a AS \"A\" ::TABLE, b, c::TITLE FROM t;\n\
                  SELECT 'md'::MARKDOWN;\n\
                  SELECT 'txt'::TEXT_LARGE;\n\
                  SELECT x::XAXIS, y::BARCHART, 't'::TITLE FROM t;";
    for p in PAYLOADS {
        let leaked: &'static str = Box::leak(p.to_string().into_boxed_str());
        let svg =
            render_dashboard_svg(script, &mut Evil(leaked), &DashboardOptions::default()).unwrap();
        assert_safe_svg(&svg, &format!("dashboard / {p}"));
    }
}

/// Quotes in data reach the SVG verbatim (escaped as `&quot;`/`&#39;` by the
/// writers), not substituted — and the result stays well-formed and inert.
#[test]
fn quotes_in_data_are_kept_and_escaped() {
    let spec = r#"{"rows":[{"c0":"say \"hi\"","c1":3},{"c0":"it's","c1":4}],
                  "roles":[[0,"XAXIS"],[1,"BARCHART"]],"width":400,"height":300}"#;
    let svg = render_spec_checked(spec).unwrap();
    assert_safe_svg(&svg, "quotes");
    assert!(!svg.contains('”'), "quote was substituted");
    let doc = roxmltree::Document::parse(&svg).unwrap();
    let xs: Vec<&str> = doc
        .descendants()
        .filter_map(|n| n.attribute("data-x"))
        .collect();
    assert!(xs.contains(&"say \"hi\""), "data-x keeps the quote: {xs:?}");
    assert!(xs.contains(&"it's"), "data-x keeps the apostrophe: {xs:?}");
}

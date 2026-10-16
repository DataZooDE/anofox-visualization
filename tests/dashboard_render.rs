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

// Headless: an input renders nothing, but its default is bound with
// `SET VARIABLE` before the panels that read it (here a ::PERIOD).
struct InputMock {
    executed: Vec<String>,
}
impl DataProvider for InputMock {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        self.executed.push(sql.to_string());
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if sql.contains("presets=") {
            return Ok(vec![
                (
                    "p".into(),
                    s(&["presets=30d,ytd; default=ytd; today=2026-09-30"]),
                ),
                ("c1".into(), s(&["Period"])),
            ]);
        }
        Ok(vec![("c0".into(), f(&[1.0])), ("c1".into(), s(&["N"]))])
    }
}

#[test]
fn headless_binds_input_defaults() {
    let script = "SELECT 'presets=30d,ytd; default=ytd; today=2026-09-30' AS p ::PERIOD, 'Period'::LABEL;\n\
                  SELECT count(*)::METRIC, 'N'::LABEL FROM t WHERE d >= getvariable('p_from')::DATE;";
    let mut db = InputMock { executed: vec![] };
    render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        db.executed
            .contains(&"SET VARIABLE \"p_from\" = '2026-01-01'".to_string()),
        "{:?}",
        db.executed
    );
    assert!(db
        .executed
        .contains(&"SET VARIABLE \"p_prev_to\" = '2025-09-30'".to_string()));
    assert_eq!(db.executed.len(), 8);
}

/// A KPI with a change (lower is good), its label, a sparkline and a goal —
/// drawn headless in the theme's good/bad colours; two KPIs in a ::GROUP share
/// one row, sized to the tile (not the 320 px panel default).
#[test]
fn headless_kpi_tile_has_delta_sparkline_and_goal() {
    struct Kpis;
    impl DataProvider for Kpis {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            // a KPI's list is cast to DOUBLE[], not DOUBLE
            if sql.contains("list(") {
                assert!(sql.contains("AS DOUBLE[]) AS c4"), "{sql}");
            }
            if sql.contains("'Cost'") {
                return Ok(vec![
                    ("c0".into(), f(&[1100.0])),
                    ("c1".into(), f(&[1000.0])),
                    ("c2".into(), s(&["both"])),
                    ("c3".into(), s(&["vs last month"])),
                    ("c4".into(), s(&["[3, 5, 4, 6]"])),
                    ("c5".into(), f(&[1600.0])),
                    ("c6".into(), s(&["Cost"])),
                ]);
            }
            Ok(vec![("c0".into(), f(&[7.0])), ("c1".into(), s(&["Other"]))])
        }
    }
    let script = "\
        SELECT 'KPIs'::GROUP;\n\
        SELECT sum(c)::MONEY, sum(p)::DELTA_LOWER_GOOD, 'both'::DELTA_FORMAT, 'vs last month'::DELTA_LABEL, \
               list(c ORDER BY d)::SPARKLINE, 1600::GOAL, 'Cost'::LABEL FROM t;\n\
        SELECT 7::METRIC, 'Other'::LABEL;\n\
        SELECT 1::ENDGROUP;";
    let o = DashboardOptions::default();
    let svg = render_dashboard_svg(script, &mut Kpis, &o).unwrap();
    let t = anofox_visualization::theme::resolve("light").unwrap();
    let th = t.pick(anofox_visualization::theme::Variant::Light);
    let hex = anofox_visualization::theme::hex;
    // a rise in a lower-is-better KPI: the negative colour
    assert!(svg.contains("class=\"kpi-delta bad\""), "{svg}");
    assert!(
        svg.contains(&format!(
            "<tspan fill=\"{}\">▲ 10% (+$100)",
            hex(th.colors.negative)
        )),
        "{svg}"
    );
    assert!(svg.contains(">vs last month</tspan>"), "{svg}");
    assert!(
        svg.contains("class=\"kpi-spark\"") && svg.contains("<polyline"),
        "{svg}"
    );
    assert!(
        svg.contains("class=\"kpi-goal\"") && svg.contains("69% of goal $1,600"),
        "{svg}"
    );
    // both tiles in one row, each as tall as the tallest tile (< 320)
    let heights: Vec<f64> = svg
        .match_indices("rx=\"")
        .filter_map(|(i, _)| {
            let tag = &svg[svg[..i].rfind("<rect").unwrap()..i];
            let h = tag.split("height=\"").nth(1)?.split('"').next()?;
            let y = tag.split(" y=\"").nth(1)?.split('"').next()?;
            Some((y.parse::<f64>().ok()?, h.parse::<f64>().ok()?))
        })
        .filter(|(_, h)| *h > 50.0)
        .map(|(y, h)| y * 1000.0 + h)
        .collect();
    assert_eq!(heights.len(), 2, "{heights:?}");
    assert_eq!(heights[0], heights[1], "same row, same height");
    assert!(heights[0] % 1000.0 < 320.0, "{heights:?}");
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

// #31: annotations from data rows — an event label on its ::XLINE, a labelled
// shaded range, a ::REFLINE label and a point callout, all from one result.
#[test]
fn annotations_from_data_rows() {
    let script = "SELECT d::XAXIS, v::LINECHART, e::XLINE, n::XLINE_LABEL, \
                  bf::XBAND_FROM, bt::XBAND_TO, bl::XBAND_LABEL, a::ANNOTATE, \
                  4::REFLINE, 'Target'::REFLINE_LABEL FROM t;";
    let days = [
        "2026-02-01",
        "2026-02-02",
        "2026-02-03",
        "2026-02-04",
        "2026-02-05",
    ];
    // five rows, one value at row i (an event joined onto its day)
    let at = |i: usize, v: &str| -> Vec<Value> {
        (0..5)
            .map(|k| {
                if k == i {
                    Value::Str(v.into())
                } else {
                    Value::Na
                }
            })
            .collect()
    };
    let mut db = Fixed(vec![
        ("c0".into(), s(&days)),
        ("c1".into(), f(&[1.0, 3.0, 2.0, 5.0, 4.0])),
        ("c2".into(), at(1, "2026-02-02")),
        ("c3".into(), at(1, "Launch")),
        ("c4".into(), at(2, "2026-02-03")),
        ("c5".into(), at(2, "2026-02-04")),
        ("c6".into(), at(2, "Outage")),
        ("c7".into(), at(3, "Record day")),
        ("c8".into(), f(&[4.0; 5])),
        ("c9".into(), s(&["Target"; 5])),
    ]);
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    for label in ["Launch", "Outage", "Target", "Record day"] {
        assert!(svg.contains(label), "{label} missing");
    }
}

// #30: ::THRESHOLDS colours a KPI value, a gauge's zones and a chart's
// threshold lines in the theme's status colours (light: warning #b45309,
// negative #dc2626).
#[test]
fn thresholds_colour_kpis_gauges_and_charts() {
    let render = |script: &str, cols: Vec<(String, Vec<Value>)>| {
        render_dashboard_svg(script, &mut Fixed(cols), &DashboardOptions::default()).unwrap()
    };
    let rules = "<0:bad; <5:warn; good";
    let kpi = render(
        "SELECT 3.1::PERCENT, '<0:bad; <5:warn; good'::THRESHOLDS;",
        vec![("c0".into(), f(&[3.1])), ("c1".into(), s(&[rules]))],
    );
    assert!(
        kpi.contains("fill=\"#b45309\""),
        "KPI value not in the warning colour"
    );
    let gauge = render(
        "SELECT 72::GAUGE, '<50:bad; <80:warn; good'::THRESHOLDS;",
        vec![
            ("c0".into(), f(&[72.0])),
            ("c1".into(), s(&["<50:bad; <80:warn; good"])),
        ],
    );
    assert!(
        gauge.to_lowercase().contains("b45309"),
        "gauge has no warning zone"
    );
    let chart = render(
        "SELECT x::XAXIS, y::LINECHART, '<50:bad; good'::THRESHOLDS FROM t;",
        vec![
            ("c0".into(), f(&[1.0, 2.0, 3.0])),
            ("c1".into(), f(&[40.0, 60.0, 55.0])),
            ("c2".into(), s(&["<50:bad; good"; 3])),
        ],
    );
    assert!(
        chart.to_lowercase().contains("dc2626"),
        "no threshold line at 50"
    );
}

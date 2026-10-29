use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::sql;
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

// #56: the DuckDB CLI's `-json` writes a contract call's payload columns as
// nested JSON (an array of rows, an object of arguments), not as text. Both
// reach the renderer through `result_columns`, as in `dashboard --svg`.
struct CliJson;
impl DataProvider for CliJson {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        let out = r#"[{"c0":[{"term":"x1","estimate":0.5,"conf_low":0.2,"conf_high":0.8},
                             {"term":"x2","estimate":-0.3,"conf_low":-0.7,"conf_high":0.1}],
                       "c1":{"x_label":"effect on price"},"c2":"inline"}]"#;
        Ok(sql::result_columns(&sql::parse_rows_json(out)?))
    }
}

#[test]
fn renders_contract_calls_from_cli_json() {
    let script = "SELECT anofox_plot_terms(t, x_label := 'effect on price'), 'inline'::TITLE \
                  FROM (VALUES ('x1', 0.5, 0.2, 0.8)) t(term, estimate, conf_low, conf_high);";
    let svg = render_dashboard_svg(script, &mut CliJson, &DashboardOptions::default()).unwrap();
    assert!(!svg.contains("No data"), "{svg}");
    assert!(svg.contains(">x2<"), "{svg}");
    assert!(svg.contains("effect on price"), "{svg}");
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
    // #419: the sparkline is drawn through `render_sparkline` (ggplot-rs),
    // the same line/point geoms a standalone `::SPARKLINE` panel or a
    // table's `::SPARKLINE` cell use — no `kpi-spark` hand-rolled markup.
    assert!(
        svg.contains("data-geom=\"line\"") && svg.contains("data-geom=\"point\""),
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

/// #405: a `::GROUP`'s `'compact'::KPI_SIZE` cascades onto its tiles — one
/// line each (no `::SPARKLINE` / `::GOAL`, even when the data is there), at
/// the lower ~36px floor, not the normal tile's ~96px one.
#[test]
fn headless_compact_kpi_group_drops_spark_and_goal() {
    struct Kpis;
    impl DataProvider for Kpis {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("'KPIs'") {
                // the ::GROUP marker: its own title + 'compact'::KPI_SIZE
                return Ok(vec![
                    ("c0".into(), s(&["KPIs"])),
                    ("c1".into(), s(&["compact"])),
                ]);
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
            // 'Other' tile and the ::ENDGROUP marker
            Ok(vec![("c0".into(), f(&[7.0])), ("c1".into(), s(&["Other"]))])
        }
    }
    let script = "\
        SELECT 'KPIs'::GROUP, 'compact'::KPI_SIZE;\n\
        SELECT sum(c)::MONEY, sum(p)::DELTA_LOWER_GOOD, 'both'::DELTA_FORMAT, 'vs last month'::DELTA_LABEL, \
               list(c ORDER BY d)::SPARKLINE, 1600::GOAL, 'Cost'::LABEL FROM t;\n\
        SELECT 7::METRIC, 'Other'::LABEL;\n\
        SELECT 1::ENDGROUP;";
    let o = DashboardOptions::default();
    let svg = render_dashboard_svg(script, &mut Kpis, &o).unwrap();
    assert!(
        svg.matches("class=\"kpi-value kpi-compact\"").count() == 2,
        "{svg}"
    );
    // the change is still there, inline — just no spark / goal
    assert!(svg.contains("▲ 10% (+$100)"), "{svg}");
    assert!(svg.contains("Cost") && svg.contains("$1,100"), "{svg}");
    assert!(
        !svg.contains("kpi-spark") && !svg.contains("kpi-goal"),
        "{svg}"
    );
    // both tiles share one row, at the compact floor (36), not the normal one (96)
    let heights: Vec<f64> = svg
        .match_indices("rx=\"")
        .filter_map(|(i, _)| {
            let tag = &svg[svg[..i].rfind("<rect").unwrap()..i];
            let h = tag.split("height=\"").nth(1)?.split('"').next()?;
            h.parse::<f64>().ok()
        })
        .filter(|h| *h > 20.0)
        .collect();
    assert_eq!(heights, vec![36.0, 36.0], "{heights:?}");
}

/// #405: a tile's own `::KPI_SIZE` wins over its `::GROUP`'s — 'compact' on
/// a tile in a normal box, and 'normal' on a tile in a compact box.
#[test]
fn headless_kpi_size_tile_override_wins_over_its_group() {
    struct Kpis;
    impl DataProvider for Kpis {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("'Own compact'") {
                return Ok(vec![
                    ("c0".into(), f(&[55.0])),
                    ("c1".into(), s(&["Own compact"])),
                    ("c2".into(), s(&["compact"])),
                ]);
            }
            if sql.contains("'Overridden'") {
                return Ok(vec![
                    ("c0".into(), f(&[20.0])),
                    ("c1".into(), s(&["Overridden"])),
                    ("c2".into(), s(&["normal"])),
                ]);
            }
            // the two ::GROUP markers (one plain, one 'compact') and ::ENDGROUP
            Ok(vec![("c0".into(), f(&[1.0]))])
        }
    }
    let script = "\
        SELECT 'A'::GROUP;\n\
        SELECT 55::METRIC, 'Own compact'::LABEL, 'compact'::KPI_SIZE FROM t;\n\
        SELECT 1::ENDGROUP;\n\
        SELECT 'B'::GROUP, 'compact'::KPI_SIZE;\n\
        SELECT 20::METRIC, 'Overridden'::LABEL, 'normal'::KPI_SIZE FROM t;\n\
        SELECT 1::ENDGROUP;";
    let o = DashboardOptions::default();
    let svg = render_dashboard_svg(script, &mut Kpis, &o).unwrap();
    assert!(svg.contains("Own compact") && svg.contains("55"), "{svg}");
    assert!(svg.contains("Overridden") && svg.contains("20"), "{svg}");
    // exactly one compact tile (the lone 'A' tile): 'Overridden' stays normal
    assert_eq!(
        svg.matches("class=\"kpi-value kpi-compact\"").count(),
        1,
        "{svg}"
    );
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

/// `::ASOF` (#34): the headless header carries the freshness chip, amber
/// (the theme's warning colour) once older than its `stale=` threshold.
#[test]
fn headless_header_shows_the_freshness_chip() {
    struct Fresh(&'static str);
    impl DataProvider for Fresh {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("stale") {
                return Ok(vec![
                    ("c0".into(), s(&[self.0])),
                    ("o".into(), s(&["stale=24h"])),
                ]);
            }
            Ok(vec![
                ("c0".into(), s(&["W1", "W2"])),
                ("c1".into(), f(&[3.0, 4.0])),
            ])
        }
    }
    let script = "SELECT '2020-01-01 10:00:00'::ASOF, 'stale=24h';\n\
                  SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL;";
    let svg = render_dashboard_svg(
        script,
        &mut Fresh("2020-01-01 10:00:00"),
        &DashboardOptions::default(),
    )
    .unwrap();
    let chip = svg
        .split("class=\"dp-asof\"")
        .nth(1)
        .expect("a freshness chip");
    assert!(chip.contains("Data as of"), "{chip}");
    assert!(chip.contains(" d ago)"), "{chip}");
    let warn = anofox_visualization::theme::resolve("")
        .unwrap()
        .light
        .colors
        .warning;
    assert!(
        chip.contains(&anofox_visualization::theme::hex(warn)),
        "stale chip not amber: {chip}"
    );
}

/// A date-only `::ASOF` (#282) renders as a calendar day: no time, no
/// time-zone shift ("Data as of 01/01/2020 (…)", never "… UTC").
#[test]
fn date_only_asof_renders_as_a_day() {
    struct Fresh(&'static str);
    impl DataProvider for Fresh {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("stale") {
                return Ok(vec![
                    ("c0".into(), s(&[self.0])),
                    ("o".into(), s(&["stale=24h"])),
                ]);
            }
            Ok(vec![
                ("c0".into(), s(&["W1", "W2"])),
                ("c1".into(), f(&[3.0, 4.0])),
            ])
        }
    }
    let script = "SELECT '2020-01-01'::ASOF, 'stale=24h';\n\
                  SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL;";
    let svg = render_dashboard_svg(
        script,
        &mut Fresh("2020-01-01"),
        &DashboardOptions::default(),
    )
    .unwrap();
    let chip = svg
        .split("class=\"dp-asof\"")
        .nth(1)
        .and_then(|s| s.split("</text>").next())
        .expect("a freshness chip");
    assert!(chip.contains("Data as of 01/01/2020 ("), "{chip}");
    assert!(!chip.contains("UTC"), "{chip}");
}

/// `::INFO` (#36): headless panels carry an ⓘ whose SVG `<title>` is the
/// definition (a hover tooltip wherever the SVG is shown).
#[test]
fn headless_info_is_a_tooltip_on_the_title() {
    let script = "SELECT week::XAXIS, sum(n)::BARCHART, 'Weekly'::INFO FROM t GROUP BY ALL;";
    struct Bars;
    impl DataProvider for Bars {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            Ok(vec![
                ("c0".into(), s(&["W1", "W2"])),
                ("c1".into(), f(&[3.0, 4.0])),
                ("c2".into(), s(&["Weekly", "Weekly"])),
            ])
        }
    }
    let svg = render_dashboard_svg(script, &mut Bars, &DashboardOptions::default()).unwrap();
    assert!(svg.contains("class=\"dp-info\""), "no ⓘ: {svg}");
    assert!(svg.contains("ⓘ<title>Weekly</title>"), "no definition");
}

// #226: the headless table honours ::TOTALS like the browser.
struct TotalsDb {
    rows: usize,
    spec: &'static str,
}
impl DataProvider for TotalsDb {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if !sql.contains("FROM t") {
            return Ok(vec![("c0".into(), s(&[self.spec]))]);
        }
        let n = self.rows;
        Ok(vec![
            ("region".into(), s(&["North", "South", "East"][..n])),
            ("revenue".into(), f(&[1200.0, 800.0, 1000.0][..n])),
            ("units".into(), f(&[1.0, 2.0, 4.0][..n])),
            ("Per 10k".into(), f(&[0.15, 0.25, 0.45][..n])),
        ])
    }
}
const TOTALS_TABLE: &str = "SELECT region::TABLE, revenue::MONEY, units, \"Per 10k\" FROM t;";
/// The totals rows of a rendered dashboard: the text after each TOTAL label,
/// up to the end of its table's svg.
fn totals_rows(svg: &str) -> Vec<String> {
    svg.split(">TOTAL</text>")
        .skip(1)
        .map(|t| t.split("</svg>").next().unwrap_or("").to_string())
        .collect()
}
fn render_totals(spec: Option<&'static str>, tables: usize, rows: usize) -> Vec<String> {
    let mut script = String::new();
    if let Some(sp) = spec {
        script.push_str(&format!("SELECT '{sp}'::TOTALS;\n"));
    }
    for _ in 0..tables {
        script.push_str(TOTALS_TABLE);
        script.push('\n');
    }
    let mut db = TotalsDb {
        rows,
        spec: spec.unwrap_or(""),
    };
    totals_rows(&render_dashboard_svg(&script, &mut db, &DashboardOptions::default()).unwrap())
}

#[test]
fn svg_table_honours_totals() {
    // the defaults: units summed, the rate (Per 10k) without a total
    let t = render_totals(None, 1, 3);
    assert_eq!(t.len(), 1);
    assert!(
        t[0].contains(">7<") && !t[0].contains(">0.28<") && !t[0].contains(">0.85<"),
        "{t:?}"
    );
    // a named column (raw name or header, any case); the rest keep their default
    let t = render_totals(Some("Per 10k: mean"), 1, 3);
    assert!(t[0].contains(">0.28<") && t[0].contains(">7<"), "{t:?}");
    let t = render_totals(Some("Units: median; PER 10K: max"), 1, 3);
    assert!(
        t[0].contains(">2<") && t[0].contains(">0.45<") && !t[0].contains(">7<"),
        "{t:?}"
    );
    let t = render_totals(Some("units: count"), 1, 3);
    assert!(t[0].contains(">3<") && !t[0].contains(">7<"), "{t:?}");
    // a bare value: every numeric column (a rate too, as in the browser)
    let t = render_totals(Some("mean"), 1, 3);
    assert!(t[0].contains(">2.3<") && t[0].contains(">0.28<"), "{t:?}");
    let t = render_totals(Some("sum"), 1, 3);
    assert!(t[0].contains(">7<") && t[0].contains(">0.85<"), "{t:?}");
    let t = render_totals(Some("Per 10k: none; sum"), 1, 3);
    assert!(t[0].contains(">7<") && !t[0].contains(">0.85<"), "{t:?}");
    // 'none' hides the row
    assert!(render_totals(Some("none"), 1, 3).is_empty());
}

#[test]
fn svg_table_totals_are_for_the_next_table_and_show_from_two_rows() {
    let t = render_totals(Some("Per 10k: mean"), 2, 3);
    assert_eq!(t.len(), 2, "{t:?}");
    assert!(t[0].contains(">0.28<") && !t[1].contains(">0.28<"), "{t:?}");
    // two rows: no default totals row, but a ::TOTALS value shows one
    assert!(render_totals(None, 1, 2).is_empty());
    let t = render_totals(Some("units: sum"), 1, 2);
    assert!(t.len() == 1 && t[0].contains(">3<"), "{t:?}");
}

// #246: the headless card's ::TITLE is the first non-NULL value, trimmed.
#[test]
fn headless_title_is_the_first_non_null_value() {
    let script = "SELECT week::XAXIS, sum(n)::BARCHART, t::TITLE FROM t GROUP BY ALL;";
    let mut db = Fixed(vec![
        ("c0".into(), s(&["W1", "W2", "W3"])),
        ("c1".into(), f(&[3.0, 4.0, 5.0])),
        (
            "c2".into(),
            vec![
                Value::Na,
                Value::Str("  Weekly  ".into()),
                Value::Str("Other".into()),
            ],
        ),
    ]);
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(svg.contains(">Weekly</text>"), "{svg}");
    assert!(!svg.contains("Other") && !svg.contains("  Weekly"), "{svg}");
}
#[test]
fn headless_kpi_tile_has_no_title() {
    let mut db = Fixed(vec![
        ("c0".into(), f(&[7.0])),
        ("c1".into(), s(&["Revenue"])),
    ]);
    let svg = render_dashboard_svg(
        "SELECT 7::METRIC, 'Revenue'::TITLE;",
        &mut db,
        &DashboardOptions::default(),
    )
    .unwrap();
    assert!(!svg.contains(">Revenue</text>"), "{svg}");
}
#[test]
fn headless_contract_title_is_the_first_non_null_value() {
    let rows = r#"[{"term":"price","estimate":-18.2,"conf_low":-20.1,"conf_high":-16.3}]"#;
    let mut db = Fixed(vec![
        ("c0".into(), s(&["ols", "huber"])),
        ("c1".into(), s(&[rows, rows])),
        ("c2".into(), s(&["{}", "{}"])),
        (
            "c3".into(),
            vec![Value::Na, Value::Str(" Coefficients ".into())],
        ),
    ]);
    let script = "SELECT model_id, anofox_plot_terms(t), c::TITLE FROM tidy t GROUP BY model_id;";
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        svg.contains("Coefficients — ols") && svg.contains("Coefficients — huber"),
        "{svg}"
    );
}

// #214: a headless (`--svg`/FFI/Python) render of a ::VIEW group draws only
// its default (first) alternative — the others are never even queried — and
// a ::COL pending when the group opens sets the whole group's width.
struct ViewMock;
impl DataProvider for ViewMock {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        // A bare one-column query: either a ::COL's own number (its literal
        // text sits right before " AS c0") or a ::VIEW marker's value (not a
        // number — render_inner never reads a marker's rows, only its
        // static `view` field, so any reply is fine).
        if sql.contains("AS c0") && !sql.contains("AS c1") {
            let core = sql
                .split("SELECT ")
                .nth(1)
                .and_then(|s| s.split(" AS c0").next())
                .unwrap_or("");
            return Ok(vec![(
                "c0".into(),
                f(&[core.trim().parse().unwrap_or(0.0)]),
            )]);
        }
        let title = ["Weekly", "Other", "Solo"]
            .into_iter()
            .find(|t| sql.contains(&format!("'{t}'")))
            .unwrap_or("");
        Ok(vec![
            ("c0".into(), s(&["W1", "W2"])),
            ("c1".into(), f(&[1.0, 2.0])),
            ("c2".into(), s(&[title])),
        ])
    }
}

/// The width attribute of the first placed (`<svg x=…>`) panel.
fn first_panel_width(svg: &str) -> &str {
    let at = svg.find("<svg x=").expect("a placed panel");
    let rest = &svg[at..];
    let w0 = rest.find("width=\"").expect("a width attr") + "width=\"".len();
    let w1 = rest[w0..].find('"').expect("closing quote") + w0;
    &rest[w0..w1]
}

#[test]
fn view_group_draws_only_its_default_alternative() {
    let script = "\
        SELECT 'Lines' AS yoy ::VIEW;\n\
        SELECT week::XAXIS, sum(n)::BARCHART, 'Weekly'::TITLE FROM t GROUP BY ALL;\n\
        SELECT 'Bars' AS yoy2 ::VIEW;\n\
        SELECT week::XAXIS, sum(n)::LINECHART, 'Other'::TITLE FROM t GROUP BY ALL;";
    let svg = render_dashboard_svg(script, &mut ViewMock, &DashboardOptions::default()).unwrap();
    assert!(
        svg.contains("Weekly"),
        "the default alternative renders: {svg}"
    );
    assert!(
        !svg.contains("Other"),
        "the other alternative is never drawn (not even queried): {svg}"
    );
}

#[test]
fn view_group_width_is_set_by_a_pending_col_at_its_open() {
    let plain = "SELECT 4::COL;\nSELECT week::XAXIS, sum(n)::BARCHART, 'Weekly'::TITLE FROM t GROUP BY ALL;";
    let view = "\
        SELECT 4::COL;\n\
        SELECT 'Lines' AS yoy ::VIEW;\n\
        SELECT week::XAXIS, sum(n)::BARCHART, 'Weekly'::TITLE FROM t GROUP BY ALL;\n\
        SELECT 'Bars' AS yoy2 ::VIEW;\n\
        SELECT week::XAXIS, sum(n)::LINECHART, 'Other'::TITLE FROM t GROUP BY ALL;";
    let w_plain = render_dashboard_svg(plain, &mut ViewMock, &DashboardOptions::default()).unwrap();
    let w_view = render_dashboard_svg(view, &mut ViewMock, &DashboardOptions::default()).unwrap();
    assert_eq!(
        first_panel_width(&w_plain),
        first_panel_width(&w_view),
        "the group's pending ::COL sets its default alternative's width, same as an \
         ordinary panel's: {w_plain} / {w_view}"
    );
}

#[test]
fn view_group_without_a_col_spans_the_full_row() {
    let full =
        "SELECT 12::COL;\nSELECT week::XAXIS, sum(n)::BARCHART, 'Solo'::TITLE FROM t GROUP BY ALL;";
    let view = "\
        SELECT 'Lines' AS yoy ::VIEW;\n\
        SELECT week::XAXIS, sum(n)::BARCHART, 'Weekly'::TITLE FROM t GROUP BY ALL;";
    let w_full = render_dashboard_svg(full, &mut ViewMock, &DashboardOptions::default()).unwrap();
    let w_view = render_dashboard_svg(view, &mut ViewMock, &DashboardOptions::default()).unwrap();
    assert_eq!(
        first_panel_width(&w_full),
        first_panel_width(&w_view),
        "absent a pending ::COL, the group spans the full row (12), same as an explicit \
         12::COL panel's: {w_full} / {w_view}"
    );
}

#[test]
fn view_group_col_does_not_leak_into_a_later_plain_panel() {
    let script = "\
        SELECT 4::COL;\n\
        SELECT 'Lines' AS yoy ::VIEW;\n\
        SELECT week::XAXIS, sum(n)::BARCHART, 'Weekly'::TITLE FROM t GROUP BY ALL;\n\
        SELECT 1::ENDVIEW;\n\
        SELECT week::XAXIS, sum(n)::LINECHART, 'Solo'::TITLE FROM t GROUP BY ALL;";
    let baseline = "SELECT week::XAXIS, sum(n)::LINECHART, 'Solo'::TITLE FROM t GROUP BY ALL;";
    let w_script =
        render_dashboard_svg(script, &mut ViewMock, &DashboardOptions::default()).unwrap();
    let w_baseline =
        render_dashboard_svg(baseline, &mut ViewMock, &DashboardOptions::default()).unwrap();
    // The 2nd (non-view) panel's width — not the group's 1st.
    let at = w_script.rfind("<svg x=").expect("a 2nd placed panel");
    let w2 = &w_script[at..];
    let w0 = w2.find("width=\"").unwrap() + "width=\"".len();
    let w1 = w2[w0..].find('"').unwrap() + w0;
    let w2 = &w2[w0..w1];
    assert_eq!(
        w2,
        first_panel_width(&w_baseline),
        "the view group's ::COL width must not leak past ::ENDVIEW: {w_script}"
    );
}

// #327: a headless/served `::TABLE`'s `::LINK` column used to print its
// unfilled template (`?v.supplier={value}`) as plain text instead of a
// working link — `fill_link_row` (src/dashboard.rs) never ran outside the
// browser.
#[test]
fn table_link_cell_is_filled_and_linked_not_printed_literally() {
    struct LinkMock;
    impl DataProvider for LinkMock {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("Highlight") {
                return Ok(vec![
                    ("Supplier".into(), s(&["Castings", "Seals"])),
                    (
                        "Highlight".into(),
                        s(&["?v.supplier={value}", "?v.supplier={value}"]),
                    ),
                ]);
            }
            Ok(vec![("c0".into(), f(&[1.0]))])
        }
    }
    let script = "SELECT supplier AS \"Supplier\" ::TABLE, \
                   '?v.supplier={value}' AS \"Highlight\" ::LINK \
                   FROM t;";
    let mut db = LinkMock;
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        !svg.contains("{value}"),
        "the LINK template must be filled per row, not shown literally: {svg}"
    );
    assert!(
        svg.contains("<a href=\"?v.supplier=Castings\">") && svg.contains("Open \u{2192}"),
        "each row's cell must become a working, labelled link: {svg}"
    );
    assert!(
        svg.contains("<a href=\"?v.supplier=Seals\">"),
        "a 2nd row must fill its own value, not the 1st row's: {svg}"
    );
}

// #327: a headless/served `::MARKDOWN` panel's `{{col}}` placeholders used
// to print literally (`{{z}}`) instead of the statement's own row — the
// `markdown_fill`/`md_column` substitution only ran in the browser.
// `\bar L` (raw TeX outside `{{…}}`) staying as written is the existing,
// tested, intentional headless behaviour (no KaTeX without a script) — not
// part of this bug.
#[test]
fn markdown_placeholders_are_filled_not_printed_literally() {
    struct MdMock;
    impl DataProvider for MdMock {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("md:z") {
                return Ok(vec![
                    (
                        "c0".into(),
                        s(&["z is the service-level factor ({{z}} at {{lvl}}); adds \u{20ac}{{extra}}k"]),
                    ),
                    ("md:z".into(), f(&[1.64])),
                    ("md:lvl".into(), s(&["95 %"])),
                    ("md:extra".into(), f(&[18.2])),
                ]);
            }
            Ok(vec![("c0".into(), f(&[1.0]))])
        }
    }
    let script = "SELECT 'z is the service-level factor ({{z}} at {{lvl}}); adds \u{20ac}{{extra}}k' ::MARKDOWN, \
                   1.64 AS z, '95 %' AS lvl, 18.2 AS extra \
                   FROM t;";
    let mut db = MdMock;
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        !svg.contains("{{z}}") && !svg.contains("{{lvl}}") && !svg.contains("{{extra}}"),
        "every {{{{col}}}} placeholder must be filled from the row, not shown literally: {svg}"
    );
    assert!(
        svg.contains("1.64") && svg.contains("95 %") && svg.contains("18.2"),
        "the filled values must appear: {svg}"
    );
}

// #536: ::MARKDOWN_STYLE picks a ::MARKDOWN panel's outer chrome in headless
// SVG too (not just the browser): 'plain' draws no rect at all for that
// panel (no `stroke=`, the only user of it in a single, trackless panel);
// 'card' keeps today's bordered rect; 'callout' (the default, and what
// ::CALLOUT implies) additionally draws a clipped accent-bar rect. An
// explicit ::MARKDOWN_STYLE wins even over ::CALLOUT, and a dashboard's own
// `-- @markdown_style` is the fallback a panel-level role overrides.
struct FixedRows(Vec<(&'static str, Vec<Value>)>);
impl DataProvider for FixedRows {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        Ok(self
            .0
            .iter()
            .map(|(n, v)| ((*n).to_string(), v.clone()))
            .collect())
    }
}
#[test]
fn markdown_style_chrome_in_headless_svg() {
    let render = |script: &str, rows: Vec<(&'static str, Vec<Value>)>| {
        let mut db = FixedRows(rows);
        render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap()
    };

    // 'plain': no card rect — nothing strokes this single, trackless panel.
    let svg = render(
        "SELECT 'hello'::MARKDOWN, 'plain'::MARKDOWN_STYLE;",
        vec![("c0", s(&["hello"])), ("c1", s(&["plain"]))],
    );
    assert!(!svg.contains("stroke="), "{svg}");

    // 'card': today's bordered rect, no tint/bar.
    let svg = render(
        "SELECT 'hello'::MARKDOWN, 'card'::MARKDOWN_STYLE;",
        vec![("c0", s(&["hello"])), ("c1", s(&["card"]))],
    );
    assert!(svg.contains("stroke="), "{svg}");
    assert!(!svg.contains("dp-md-clip-"), "{svg}");

    // No ::MARKDOWN_STYLE / ::CALLOUT at all: 'callout', today's look —
    // the tinted box with its clipped accent bar.
    let svg = render("SELECT 'hello'::MARKDOWN;", vec![("c0", s(&["hello"]))]);
    assert!(svg.contains("dp-md-clip-"), "{svg}");

    // An explicit ::MARKDOWN_STYLE wins even over ::CALLOUT.
    let svg = render(
        "SELECT 'hello'::MARKDOWN, 'plain'::MARKDOWN_STYLE, 'warning'::CALLOUT;",
        vec![
            ("c0", s(&["hello"])),
            ("c1", s(&["plain"])),
            ("c2", s(&["warning"])),
        ],
    );
    assert!(!svg.contains("stroke="), "{svg}");

    // A dashboard-wide `-- @markdown_style` default, with no panel override.
    let svg = render(
        "-- @markdown_style plain\nSELECT 'hello'::MARKDOWN;",
        vec![("c0", s(&["hello"]))],
    );
    assert!(!svg.contains("stroke="), "{svg}");

    // #659 ::MARKDOWN_TONE: the bordered card plus a 3 px clipped bar in the
    // tone's colour (the default theme's violet for 'example'), even over a
    // 'callout' header default.
    let svg = render(
        "-- @markdown_style callout\nSELECT 'hello'::MARKDOWN, 'example'::MARKDOWN_TONE;",
        vec![("c0", s(&["hello"])), ("c1", s(&["example"]))],
    );
    assert!(svg.contains("dp-md-clip-"), "{svg}");
    assert!(svg.contains("width=\"3\""), "{svg}");
    assert!(svg.contains("#7c3aed"), "{svg}");
}

// #427: a `::TABLE`'s `list(v ORDER BY i)::SPARKLINE` column was dropped
// entirely by the headless table renderer (`table_svg`'s own `vis` filter
// excluded every `SPARKLINE`-formatted column) — the other columns rendered,
// but the trend cell was just missing. Each row's list is now drawn through
// the same ggplot-rs sparkline panel (`render_sparkline`, via
// `render_with_at`) as a standalone `::SPARKLINE`, nested in the cell.
#[test]
fn table_sparkline_cell_is_drawn_not_dropped() {
    struct SparkMock;
    impl DataProvider for SparkMock {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("Trend") {
                return Ok(vec![
                    ("feature".into(), s(&["0", "1"])),
                    ("Trend".into(), s(&["[0,1,2,3,1]", "[4,3,2,1,3]"])),
                    ("mx".into(), f(&[3.0, 4.0])),
                ]);
            }
            Ok(vec![("c0".into(), f(&[1.0]))])
        }
    }
    let script = "SELECT feature ::TABLE, trend AS \"Trend\" ::SPARKLINE, mx AS \"Mx\" FROM t;";
    let mut db = SparkMock;
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        svg.contains(">Trend<"),
        "the sparkline column's own header must still show: {svg}"
    );
    assert!(
        !svg.contains("[0,1,2,3,1]") && !svg.contains("[4,3,2,1,3]"),
        "a cell's raw JSON list must never print literally: {svg}"
    );
    assert_eq!(
        svg.matches("gg-line\" data-geom=\"line\"").count(),
        2,
        "one drawn sparkline per row, through the same ggplot-rs line geom \
         `render_sparkline` uses: {svg}"
    );
    assert!(
        svg.contains(">Mx<") && svg.contains(">3<") && svg.contains(">4<"),
        "the table's other columns must still render, unaffected: {svg}"
    );
}

// #425: a `::TABLE`'s `list(v ORDER BY i)::SPARKBAR` (one series) or
// `list({'ref','cur'} ORDER BY i)::SPARKBAR` (two overlaid series) column
// draws a tiny bar histogram per cell, through the same ggplot-rs path as a
// standalone `::SPARKBAR` panel (`render_sparkbar`, via `render_with_at`).
#[test]
fn table_sparkbar_cell_draws_bars_one_and_two_series() {
    struct SparkbarMock;
    impl DataProvider for SparkbarMock {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("Dist") {
                return Ok(vec![
                    ("feature".into(), s(&["temp", "humidity"])),
                    (
                        "Dist".into(),
                        s(&[
                            "[0.1,0.2,0.3,0.4]",
                            "[{\"ref\":0.1,\"cur\":0.3},{\"ref\":0.3,\"cur\":0.3},\
                             {\"ref\":0.4,\"cur\":0.2},{\"ref\":0.2,\"cur\":0.2}]",
                        ]),
                    ),
                    ("mx".into(), f(&[3.0, 4.0])),
                ]);
            }
            Ok(vec![("c0".into(), f(&[1.0]))])
        }
    }
    let script = "SELECT feature ::TABLE, dist AS \"Dist\" ::SPARKBAR, mx AS \"Mx\" FROM t;";
    let mut db = SparkbarMock;
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        svg.contains(">Dist<"),
        "the sparkbar column's own header must still show: {svg}"
    );
    assert!(
        !svg.contains("[0.1,0.2,0.3,0.4]") && !svg.contains("\"ref\":0.1"),
        "a cell's raw JSON must never print literally: {svg}"
    );
    assert_eq!(
        svg.matches("data-geom=\"col\"").count(),
        12,
        "every value (4 one-series bars + 4 ref + 4 cur two-series bars) draws \
         through ggplot-rs's bar geom, one rect per mark: {svg}"
    );
    assert!(
        svg.contains(">Mx<") && svg.contains(">3<") && svg.contains(">4<"),
        "the table's other columns must still render, unaffected: {svg}"
    );
}

// #457: `render_sparkline` must scale the y axis to the series' own
// min/max, not a zero baseline. `GeomArea::include_zero_baseline` forces 0
// into the trained domain unconditionally (fine for a real area chart,
// wrong for a sparkline showing shape), which compressed a narrow-range
// series like 1000-1013 into a near-flat line a few pixels tall — the KPI
// tile, a table's `::SPARKLINE` cell and a standalone panel all went
// through this one function (#419), so all three had the regression. The
// fix draws the fill with `geom_ribbon` (no zero-baseline behaviour)
// instead, matching the browser's `cellSpark` (web/app.js) and the KPI
// tile's own min-max scaling before #419.
#[test]
fn sparkline_scales_to_its_own_range_not_zero() {
    struct NarrowMock;
    impl DataProvider for NarrowMock {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            Ok(vec![(
                "c0".into(),
                f(&[1000.0, 1001.0, 1002.0, 1003.0, 1013.0]),
            )])
        }
    }
    let script = "SELECT trend AS \"Trend\" ::SPARKLINE FROM t;";
    let mut db = NarrowMock;
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    // Every y-coordinate of the drawn line, in panel pixels. A zero-anchored
    // scale would compress this 1000-1013 series into a handful of pixels
    // near one edge; a min-max scale spreads them across most of the panel.
    let points = svg
        .split("<polyline points=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("");
    let ys: Vec<f64> = points
        .split(' ')
        .filter_map(|p| p.split(',').nth(1))
        .filter_map(|y| y.parse::<f64>().ok())
        .collect();
    assert!(ys.len() >= 2, "expected a drawn line: {svg}");
    let (mn, mx) = ys
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    assert!(
        mx - mn > 5.0,
        "a narrow-range series must still show its shape, not a near-flat \
         line: y span {} in {svg}",
        mx - mn
    );
}

// #457: a standalone `::SPARKLINE` panel goes through the same
// `render_sparkline` as the KPI tile and a table cell (see
// `sparkline_scales_to_its_own_range_not_zero`'s comment) — asked for a
// height under ggplot-rs's own plot-area floor (`SPARKLINE_MIN_H`, 50px),
// it must still draw the whole line inside its viewBox, not clip the dips
// against a shorter one.
#[test]
fn standalone_sparkline_panel_minima_stay_inside_a_short_viewbox() {
    use anofox_visualization::{render, Column, Kind, Role};
    let col = Column::new(
        "value",
        Role::Value(Kind::Sparkline),
        f(&[1005.0, 1010.0, 1000.0, 1012.0, 1001.0, 1013.0, 1000.0]),
    );
    // A height well under the engine's 50px plot-area floor — exactly the
    // squeeze a KPI tile's ~30px strip or a table row put it under.
    let svg = render(&[col], 240, 20).expect("a rendered sparkline panel");
    let h: f64 = svg
        .split("viewBox=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("a viewBox")
        .split_whitespace()
        .nth(3)
        .and_then(|n| n.parse().ok())
        .expect("a viewBox height");
    let mut ys: Vec<f64> = Vec::new();
    for pts in svg.split("points=\"").skip(1) {
        let pts = pts.split('"').next().unwrap_or("");
        ys.extend(
            pts.split(' ')
                .filter_map(|p| p.split(',').nth(1))
                .filter_map(|y| y.parse::<f64>().ok()),
        );
    }
    assert!(!ys.is_empty(), "expected a drawn line: {svg}");
    let (mn, mx) = ys
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    assert!(
        mn > -0.5 && mx < h + 0.5,
        "sparkline content (y {mn}..{mx}) must stay inside its own \
         {h}px-tall viewBox, not clipped: {svg}"
    );
}

// #425/#457: a standalone `::SPARKBAR` panel goes through the same
// `render_sparkbar` as a table cell — asked for a height under ggplot-rs's
// own plot-area floor (`SPARKLINE_MIN_H`, 50px, shared with the sparkline
// clamp since the floor is generic to any ggplot-rs panel), it must still
// draw every bar's full height inside its viewBox, not clip a bar's own
// baseline against a shorter one.
#[test]
fn standalone_sparkbar_panel_minima_stay_inside_a_short_viewbox() {
    use anofox_visualization::{render, Column, Kind, Role};
    let col = Column::new(
        "value",
        Role::Value(Kind::Sparkbar),
        f(&[1.0, 9.0, 2.0, 8.0, 3.0, 7.0]),
    );
    // A height well under the engine's 50px plot-area floor — exactly the
    // squeeze a table row put it under before this panel clamped itself.
    let svg = render(&[col], 240, 20).expect("a rendered sparkbar panel");
    let h: f64 = svg
        .split("viewBox=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("a viewBox")
        .split_whitespace()
        .nth(3)
        .and_then(|n| n.parse().ok())
        .expect("a viewBox height");
    let get_attr = |attrs: &str, name: &str| -> Option<f64> {
        attrs
            .split(&format!("{name}=\""))
            .nth(1)?
            .split('"')
            .next()?
            .parse()
            .ok()
    };
    let mut bars: Vec<(f64, f64)> = Vec::new();
    for rect in svg.split("<rect ").skip(1) {
        let attrs = rect.split('>').next().unwrap_or("");
        if let (Some(y), Some(ht)) = (get_attr(attrs, "y"), get_attr(attrs, "height")) {
            bars.push((y, y + ht));
        }
    }
    assert!(!bars.is_empty(), "expected drawn bars: {svg}");
    for (y, bottom) in &bars {
        assert!(
            *y > -0.5 && *bottom < h + 0.5,
            "sparkbar content (bar y {y}..{bottom}) must stay inside its own \
             {h}px-tall viewBox, not clipped: {svg}"
        );
    }
}

// #463: Explore data's card histogram (xpMini) and the table's own
// column-profile row (st-hist) both draw a pre-binned histogram through
// `render_mini_hist` — not reachable from SQL (no ::ROLE), so plain
// values-in, SVG-out, same shape as [`render_mini_overlay`] below. Same
// 50px panel-floor clamp as render_sparkbar's own (#425/#457): asked for
// a height under it, every bar's baseline must still stay inside the
// returned viewBox, not clipped.
#[test]
fn mini_hist_floors_to_the_panel_minimum_and_clips_nothing() {
    use anofox_visualization::render_mini_hist;
    let svg =
        render_mini_hist("[1,9,2,8,3,7]", "", 1.0, 240, 20, "").expect("a rendered mini histogram");
    let h: f64 = svg
        .split("viewBox=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("a viewBox")
        .split_whitespace()
        .nth(3)
        .and_then(|n| n.parse().ok())
        .expect("a viewBox height");
    assert!(
        h >= 50.0,
        "the panel floor (50px) must apply: viewBox height {h}"
    );
    let get_attr = |attrs: &str, name: &str| -> Option<f64> {
        attrs
            .split(&format!("{name}=\""))
            .nth(1)?
            .split('"')
            .next()?
            .parse()
            .ok()
    };
    let mut bars: Vec<(f64, f64)> = Vec::new();
    for rect in svg.split("<rect ").skip(1) {
        let attrs = rect.split('>').next().unwrap_or("");
        if let (Some(y), Some(ht)) = (get_attr(attrs, "y"), get_attr(attrs, "height")) {
            bars.push((y, y + ht));
        }
    }
    assert_eq!(bars.len(), 6, "one bar per value: {svg}");
    for (y, bottom) in &bars {
        assert!(
            *y > -0.5 && *bottom < h + 0.5,
            "mini histogram content (bar y {y}..{bottom}) must stay inside \
             its own {h}px-tall viewBox, not clipped: {svg}"
        );
    }
}

// #463: `labels_json`, when given, becomes each bar's own hover-tip prefix
// (`st-hist`'s bin ranges) — ggplot-rs's own `GeomCol` tip format,
// `"{label}: {value}"`.
#[test]
fn mini_hist_labels_become_each_bars_own_tip_prefix() {
    use anofox_visualization::render_mini_hist;
    let svg = render_mini_hist("[5,9]", r#"["0 - 10","10 - 20"]"#, 0.55, 100, 24, "")
        .expect("a rendered mini histogram with labels");
    assert!(
        svg.contains("<title>0 - 10: 5</title>"),
        "the first bar's own label + value: {svg}"
    );
    assert!(
        svg.contains("<title>10 - 20: 9</title>"),
        "the second bar's own label + value: {svg}"
    );
    assert!(
        svg.contains("fill-opacity=\"0.550\""),
        "the requested fill alpha (0.55, st-hist's own translucent look): {svg}"
    );
}

// #463: `fill_alpha: 0.0` draws an outline only (xpOverlay's A series) —
// the fill is fully transparent but the stroke (unaffected by
// `fill-opacity`, ggplot-rs's own `RectStyle`) stays visible, so the bar
// still reads as a shape, not nothing.
#[test]
fn mini_hist_zero_alpha_draws_an_outline_not_a_filled_bar() {
    use anofox_visualization::render_mini_hist;
    let svg =
        render_mini_hist("[4,6,2]", "", 0.0, 100, 30, "").expect("a rendered outline histogram");
    assert!(
        svg.contains("fill-opacity=\"0.000\""),
        "alpha 0.0 must reach the mark as a fully transparent fill: {svg}"
    );
    assert!(
        svg.contains("stroke-width=\"0.50\""),
        "the outline's own stroke must still be drawn: {svg}"
    );
}

// #463: Explore data's distribution-comparison overlay (xpOverlay) — A as
// an outline, B filled, on one shared y scale (never one series dwarfing
// the other). B paints first (underneath), A second (on top), the same
// order the hand-drawn version used, so A's outline stays crisp over B's
// fill where the two overlap.
#[test]
fn mini_overlay_draws_b_filled_under_a_outline() {
    use anofox_visualization::render_mini_overlay;
    let svg =
        render_mini_overlay("[1,9,2,8]", "[8,2,9,1]", 240, 30, "").expect("a rendered overlay");
    let rect_count = svg.matches("<rect ").count();
    assert_eq!(rect_count, 8, "4 bars for A + 4 for B: {svg}");
    let b_at = svg
        .find("fill-opacity=\"0.550\"")
        .expect("B's own filled bars");
    let a_at = svg
        .find("fill-opacity=\"0.000\"")
        .expect("A's own outline bars");
    assert!(
        b_at < a_at,
        "B (filled) must paint before A (outline), so A's outline stays \
         crisp on top where the two overlap: {svg}"
    );
}

// #517 S1: a headless render of a dashboard that reads `viewer_state` gets
// an empty seed of that temp (same rule as `lint::check_opts`) so it
// renders instead of erroring on a table no real server has populated.
struct RecordingMock {
    executed: Vec<String>,
}
impl DataProvider for RecordingMock {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        self.executed.push(sql.to_string());
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        Ok(vec![("c0".into(), s(&["hi"]))])
    }
}

#[test]
fn render_seeds_an_empty_viewer_state_when_the_script_reads_it() {
    let script = "SELECT value::TEXT_SMALL FROM viewer_state;";
    let mut db = RecordingMock {
        executed: Vec::new(),
    };
    let svg = render_dashboard_svg(script, &mut db, &DashboardOptions::default())
        .expect("renders instead of erroring on a missing viewer_state");
    assert!(svg.starts_with("<svg"));
    assert!(
        db.executed
            .iter()
            .any(|s| s.contains("CREATE TEMP TABLE viewer_state")),
        "expected the viewer_state temp to be created: {:?}",
        db.executed
    );
    assert!(
        db.executed
            .iter()
            .any(|s| s.contains("CREATE TEMP VIEW viewer_state_latest")),
        "expected the viewer_state_latest view to be created: {:?}",
        db.executed
    );
}

struct ShadowingMock {
    executed: Vec<String>,
}
impl DataProvider for ShadowingMock {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        self.executed.push(sql.to_string());
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if sql.contains("duckdb_tables()") {
            // A real, non-temp `viewer_state` already exists.
            return Ok(vec![("n".into(), f(&[1.0]))]);
        }
        Ok(vec![("c0".into(), s(&["hi"]))])
    }
}

// #517, review of #530 #5: a dashboard whose connection already has a
// *real* `viewer_state` (an attached database's table, say) must not get
// an empty temp seeded on top of it — that would shadow the real one for
// the rest of the session.
#[test]
fn render_does_not_shadow_a_pre_existing_real_viewer_state() {
    let script = "SELECT value::TEXT_SMALL FROM viewer_state;";
    let mut db = ShadowingMock {
        executed: Vec::new(),
    };
    render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        !db.executed
            .iter()
            .any(|s| s.contains("CREATE TEMP TABLE viewer_state")),
        "must not seed an empty viewer_state over a real one: {:?}",
        db.executed
    );
}

#[test]
fn render_does_not_seed_viewer_state_when_not_needed() {
    let script = "SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL;";
    let mut db = RecordingMock {
        executed: Vec::new(),
    };
    render_dashboard_svg(script, &mut db, &DashboardOptions::default()).unwrap();
    assert!(
        db.executed.is_empty(),
        "an unrelated dashboard shouldn't get the viewer_state temp: {:?}",
        db.executed
    );
}

/// #692: a 'caption'::MARKDOWN_STYLE right after a chart is drawn in the
/// chart's card (a rule, then the text in the muted ink), not as a box of its
/// own; one with no chart before it is a plain text box.
#[test]
fn headless_caption_goes_under_its_chart() {
    struct Caps;
    impl DataProvider for Caps {
        fn execute(&mut self, _sql: &str) -> Result<(), String> {
            Ok(())
        }
        fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
            if sql.contains("'caption'") {
                let text = if sql.contains("Lone") {
                    "Lone caption."
                } else {
                    "Source: ERP."
                };
                return Ok(vec![
                    ("c0".into(), s(&[text])),
                    ("c1".into(), s(&["caption"])),
                ]);
            }
            Ok(vec![
                ("c0".into(), s(&["W1", "W2"])),
                ("c1".into(), f(&[3.0, 4.0])),
            ])
        }
    }
    let one = "SELECT week::XAXIS, n::BARCHART FROM t;";
    let cap = "SELECT 'Source: ERP.'::MARKDOWN, 'caption'::MARKDOWN_STYLE;";
    let svg = |script: &str| {
        render_dashboard_svg(script, &mut Caps, &DashboardOptions::default()).unwrap()
    };
    let with = svg(&format!("{one}\n{cap}"));
    let without = svg(one);
    assert!(
        with.contains("class=\"dp-caption\"") && with.contains(">Source: ERP.</text>"),
        "{with}"
    );
    // the same one card, taller by the caption — no second card
    assert_eq!(
        with.matches("<rect x=").count(),
        without.matches("<rect x=").count(),
        "{with}"
    );
    let lone = svg("SELECT 'Lone caption.'::MARKDOWN, 'caption'::MARKDOWN_STYLE;");
    assert!(
        !lone.contains("class=\"dp-caption\"") && lone.contains("Lone caption."),
        "{lone}"
    );
}

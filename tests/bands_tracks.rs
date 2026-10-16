//! Nested quantile fans, several shaded regions (#46) and `::TRACK` strips
//! under a chart (#47).

mod common;

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::{render_with_warnings, track, Column, Kind, RenderOptions, Role, Value};

fn nums(v: impl IntoIterator<Item = f64>) -> Vec<Value> {
    v.into_iter().map(Value::Float).collect()
}

fn days(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| Value::DateTime(1_790_000_000 + 86_400 * i as i64))
        .collect()
}

/// A root attribute of an SVG document.
fn root_attr(svg: &str, name: &str) -> Option<String> {
    let tag = &svg[..svg.find('>')?];
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    Some(tag[at..at + tag[at..].find('"')?].to_string())
}

fn plot_rect(svg: &str) -> Vec<f64> {
    root_attr(svg, "data-plot")
        .unwrap()
        .split_whitespace()
        .map(|t| t.parse().unwrap())
        .collect()
}

/// History rows without a band, then a forecast with a 95 / 80 / 50 % fan.
fn fan_cols() -> Vec<Column> {
    let n = 20;
    let fc = |i: usize| i >= 12;
    let edge = |z: f64, sign: f64| -> Vec<Value> {
        (0..n)
            .map(|i| match fc(i) {
                true => Value::Float(100.0 + sign * z * (i - 11) as f64),
                false => Value::Na,
            })
            .collect()
    };
    vec![
        Column::new("day", Role::X, days(n)),
        Column::new("y", Role::Value(Kind::Line), nums((0..n).map(|_| 100.0))),
        Column::new("lo95", Role::BandLower, edge(3.0, -1.0)),
        Column::new("hi95", Role::BandUpper, edge(3.0, 1.0)),
        Column::new("lo50", Role::BandLower, edge(1.0, -1.0)),
        Column::new("hi50", Role::BandUpper, edge(1.0, 1.0)),
        Column::new("lo80", Role::BandLower, edge(2.0, -1.0)),
        Column::new("hi80", Role::BandUpper, edge(2.0, 1.0)),
    ]
}

/// The `points` of each band polygon (translucent, unstroked), in draw order.
fn band_polygons(svg: &str) -> Vec<(String, String)> {
    svg.split("<polygon ")
        .skip(1)
        .filter(|p| p.contains("stroke=\"none\""))
        .map(|p| {
            let attr = |k: &str| {
                let a = p.find(&format!("{k}=\"")).unwrap() + k.len() + 2;
                p[a..a + p[a..].find('"').unwrap()].to_string()
            };
            (attr("points"), attr("fill-opacity"))
        })
        .collect()
}

#[test]
fn several_band_pairs_draw_a_nested_fan_widest_first() {
    let r = render_with_warnings(&fan_cols(), 480, 320, &RenderOptions::default()).unwrap();
    common::assert_safe_svg(&r.svg, "fan");
    // History rows carry no band: they are not drawn (nor warned about).
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let bands = band_polygons(&r.svg);
    assert_eq!(bands.len(), 3, "one ribbon per pair");
    // Graded: every band at the same low opacity, overlaps darken inwards.
    assert!(bands.iter().all(|(_, a)| a == "0.133"), "{bands:?}");
    // Widest first: the vertical extent of each polygon shrinks.
    let span = |pts: &str| {
        let ys: Vec<f64> = pts
            .split_whitespace()
            .map(|p| p.split(',').nth(1).unwrap().parse().unwrap())
            .collect();
        ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min)
    };
    let spans: Vec<f64> = bands.iter().map(|(p, _)| span(p)).collect();
    assert!(spans[0] > spans[1] && spans[1] > spans[2], "{spans:?}");
}

#[test]
fn one_band_keeps_its_opacity_and_an_unpaired_edge_draws_nothing() {
    let mut cols = fan_cols();
    cols.truncate(4);
    let svg = render_with_warnings(&cols, 480, 320, &RenderOptions::default())
        .unwrap()
        .svg;
    assert_eq!(band_polygons(&svg)[0].1, "0.250");
    cols.pop(); // a lone ::BAND_LOWER: the line still renders
    let svg = render_with_warnings(&cols, 480, 320, &RenderOptions::default())
        .unwrap()
        .svg;
    assert!(band_polygons(&svg).is_empty());
}

/// The shaded regions (`::MARKAREA` rects) as (x, width).
fn mark_rects(svg: &str) -> Vec<(f64, f64)> {
    svg.split("<rect ")
        .skip(1)
        .filter(|r| r.contains("fill-opacity=\"0.140\""))
        .map(|r| {
            let attr = |k: &str| -> f64 {
                let a = r.find(&format!(" {k}=\"")).map_or(0, |a| a + k.len() + 3);
                let a = if a == 0 {
                    r.find(&format!("{k}=\"")).unwrap() + k.len() + 2
                } else {
                    a
                };
                r[a..a + r[a..].find('"').unwrap()].parse().unwrap()
            };
            (attr("x"), attr("width"))
        })
        .collect()
}

#[test]
fn markarea_shades_every_run_and_a_single_day_by_its_step() {
    let n = 30;
    let flagged = |i: usize| (3..=6).contains(&i) || i == 12 || (20..=24).contains(&i);
    let x = days(n);
    let mark: Vec<Value> = (0..n)
        .map(|i| if flagged(i) { x[i].clone() } else { Value::Na })
        .collect();
    let cols = vec![
        Column::new("day", Role::X, x),
        Column::new("y", Role::Value(Kind::Line), nums((0..n).map(|i| i as f64))),
        Column::new("m", Role::MarkArea, mark),
    ];
    let svg = render_with_warnings(&cols, 600, 300, &RenderOptions::default())
        .unwrap()
        .svg;
    let rects = mark_rects(&svg);
    assert_eq!(rects.len(), 3, "{rects:?}");
    // px per day from the 4-day run (days 3..6 = 3 steps wide)
    let day = rects[0].1 / 3.0;
    assert!(
        (rects[1].1 - day).abs() < 0.5,
        "a single day spans one step"
    );
    assert!((rects[2].1 - 4.0 * day).abs() < 0.5);
    // A date axis stays a date axis (ticks read as dates).
    assert!(svg.contains(">2026-"), "date ticks");
}

fn chart() -> Vec<Column> {
    let n = 40;
    vec![
        Column::new("day", Role::X, days(n)),
        Column::new(
            "stock",
            Role::Value(Kind::Line),
            nums((0..n).map(|i| 1000.0 + 37.0 * i as f64)),
        ),
    ]
}

fn tracks() -> Vec<(Vec<Column>, u32)> {
    let d = days(40);
    let pick = |idx: &[usize]| -> Vec<Value> { idx.iter().map(|&i| d[i].clone()).collect() };
    let ticks = vec![
        Column::new("day", Role::X, pick(&[3, 17, 31])),
        Column::new("t", Role::Track, vec![Value::Str("Promotions".into()); 3]),
        Column::new(
            "what",
            Role::Label,
            ["Summer sale", "Back to school", "Coupon"]
                .iter()
                .map(|s| Value::Str(s.to_string()))
                .collect(),
        ),
    ];
    let lanes = vec![
        Column::new("day", Role::X, pick(&[5, 6, 7, 20])),
        Column::new(
            "detector",
            Role::Y,
            [
                "A rather long detector name",
                "A rather long detector name",
                "Lead time",
                "Lead time",
            ]
            .iter()
            .map(|s| Value::Str(s.to_string()))
            .collect(),
        ),
        Column::new(
            "tier",
            Role::Category,
            ["#d64545", "#e8a23b", "#d64545", "#e8a23b"]
                .iter()
                .map(|s| Value::Str(s.to_string()))
                .collect(),
        ),
        Column::new("n", Role::Value(Kind::Heatmap), nums([1.0; 4])),
        Column::new("t", Role::Track, vec![Value::Str("Alarms".into()); 4]),
    ];
    let price = vec![
        Column::new("day", Role::X, days(40)),
        Column::new(
            "p",
            Role::Value(Kind::Step),
            nums((0..40).map(|i| 9.0 + (i % 7) as f64)),
        ),
        Column::new("t", Role::Track, vec![Value::Str("Price".into()); 40]),
    ];
    vec![(ticks, 0), (lanes, 0), (price, 50)]
}

#[test]
fn tracks_share_the_charts_plot_area_and_x_window() {
    let svgs =
        track::render_stack(&chart(), &tracks(), 800, 300, &RenderOptions::default()).unwrap();
    assert_eq!(svgs.len(), 4);
    let main = plot_rect(&svgs[0]);
    let xdomain = root_attr(&svgs[0], "data-xdomain").unwrap();
    for (k, svg) in svgs.iter().enumerate().skip(1) {
        common::assert_safe_svg(svg, "track");
        assert_eq!(root_attr(svg, "data-track").as_deref(), Some("true"));
        let p = plot_rect(svg);
        assert!(
            (p[0] - main[0]).abs() < 0.05 && (p[2] - main[2]).abs() < 0.05,
            "track {k}: {p:?} vs {main:?}"
        );
        // the same x window, to rounding
        let (a, b): (Vec<f64>, Vec<f64>) = (
            xdomain.split(' ').map(|t| t.parse().unwrap()).collect(),
            root_attr(svg, "data-xdomain")
                .unwrap()
                .split(' ')
                .map(|t| t.parse().unwrap())
                .collect(),
        );
        assert!(
            (a[0] - b[0]).abs() < 1.0 && (a[1] - b[1]).abs() < 1.0,
            "{a:?} vs {b:?}"
        );
        // a thin strip's plot area stays inside it
        let h: f64 = root_attr(svg, "height").unwrap().parse().unwrap();
        assert!(p[1] + p[3] <= h + 0.05, "track {k}: plot {p:?} in {h}px");
    }
    // The long lane name pushed every left edge right, the chart's included.
    let alone = render_with_warnings(&chart(), 800, 300, &RenderOptions::default()).unwrap();
    assert!(main[0] > plot_rect(&alone.svg)[0] + 20.0);
    // Natural heights: 20 px per lane (+14), a set ::HEIGHT as given.
    assert_eq!(track::heights(&tracks()), vec![34, 54, 50]);
}

#[test]
fn lane_tooltips_name_the_lane_and_the_text() {
    let svgs =
        track::render_stack(&chart(), &tracks(), 800, 300, &RenderOptions::default()).unwrap();
    assert!(svgs[1].contains("data-series=\"Promotions\" data-fvalue=\"Summer sale\"><title>Promotions: Summer sale</title>"));
    // a category fill is a colour, not the tooltip's value
    assert!(svgs[2].contains("<title>Lead time: #d64545</title>"));
    assert!(!svgs[2].contains("data-value=\"#"));
    // the colour levels are drawn as given
    assert!(svgs[2].contains("fill=\"#D64545\""));
}

#[test]
fn tracks_under_a_categorical_chart_are_notes() {
    let bars = vec![
        Column::new(
            "w",
            Role::X,
            ["W1", "W2"]
                .iter()
                .map(|s| Value::Str(s.to_string()))
                .collect(),
        ),
        Column::new("n", Role::Value(Kind::Bar), nums([3.0, 4.0])),
    ];
    let svgs = track::render_stack(&bars, &tracks(), 600, 300, &RenderOptions::default()).unwrap();
    assert_eq!(svgs.len(), 4);
    assert!(svgs[1].contains("numeric or date x axis"));
}

#[test]
fn a_track_alone_needs_a_continuous_x_and_a_track_kind() {
    let bad_x = vec![
        Column::new("w", Role::X, vec![Value::Str("W1".into())]),
        Column::new("t", Role::Track, vec![Value::Str("Promos".into())]),
    ];
    let err = render_with_warnings(&bad_x, 600, 60, &RenderOptions::default()).unwrap_err();
    assert!(err.to_string().contains("numeric or date ::XAXIS"), "{err}");
    let pie = vec![
        Column::new("d", Role::X, days(2)),
        Column::new("n", Role::Value(Kind::Pie), nums([1.0, 2.0])),
        Column::new("t", Role::Track, vec![Value::Str("x".into()); 2]),
    ];
    let err = render_with_warnings(&pie, 600, 60, &RenderOptions::default()).unwrap_err();
    assert!(err.to_string().contains("not ::PIE"), "{err}");
}

/// A provider answering by the planned statement's shape.
struct Db;
impl DataProvider for Db {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        let iso = |n: usize| -> Vec<Value> {
            (0..n)
                .map(|i| Value::Str(format!("2026-10-{:02}", i + 1)))
                .collect()
        };
        if sql.contains("FROM promos") {
            return Ok(vec![
                ("c0".into(), vec![Value::Str("2026-10-05".into())]),
                ("c1".into(), vec![Value::Str("Promotions".into())]),
            ]);
        }
        if sql.contains("FROM height") {
            return Ok(vec![("c0".into(), nums([48.0]))]);
        }
        Ok(vec![
            ("c0".into(), iso(20)),
            ("c1".into(), nums((0..20).map(|i| i as f64))),
            ("c2".into(), vec![Value::Str("Units".into()); 20]),
        ])
    }
}

#[test]
fn the_headless_dashboard_stacks_tracks_in_the_charts_card() {
    let script = "\
        SELECT day::XAXIS, n::LINECHART FROM sales;\n\
        SELECT day::XAXIS, 'Promotions'::TRACK FROM promos;\n\
        SELECT 48::HEIGHT FROM height;\n\
        SELECT day::XAXIS, n::BARCHART, 'Units'::TRACK FROM sales;\n\
        SELECT day::XAXIS, n::BARCHART FROM sales;";
    let svg = render_dashboard_svg(script, &mut Db, &DashboardOptions::default()).unwrap();
    common::assert_safe_svg(&svg, "dashboard");
    // As many cards as without the strips: they sit in the chart's card.
    let plain = "\
        SELECT day::XAXIS, n::LINECHART FROM sales;\n\
        SELECT day::XAXIS, n::BARCHART FROM sales;";
    let plain = render_dashboard_svg(plain, &mut Db, &DashboardOptions::default()).unwrap();
    assert_eq!(
        svg.matches(" rx=\"").count(),
        plain.matches(" rx=\"").count()
    );
    let tracks: Vec<&str> = svg.split("data-track=\"true\"").collect();
    assert_eq!(tracks.len() - 1, 2, "two strips");
    // The 48 px ::HEIGHT sized the second strip.
    assert!(svg.contains("height=\"48\" viewBox=\"0 0"));
}

#[cfg(feature = "wasm")]
#[test]
fn wasm_render_stack_returns_the_chart_and_its_tracks() {
    use anofox_visualization::wasm::render_stack_checked;
    let spec = serde_json::json!({
        "chart": {
            "rows": r#"[{"c0":"2026-10-01","c1":1},{"c0":"2026-10-09","c1":3}]"#,
            "roles": [[0, "XAXIS", ""], [1, "LINECHART", ""]],
        },
        "tracks": [{
            "rows": r#"[{"c0":"2026-10-04","c1":"Promo"}]"#,
            "roles": [[0, "XAXIS", ""], [1, "TRACK", ""]],
            "height": 0,
        }],
    });
    let svgs = render_stack_checked(&spec.to_string(), 600, 300, "", "").unwrap();
    assert_eq!(svgs.len(), 2);
    assert_eq!(plot_rect(&svgs[0])[0], plot_rect(&svgs[1])[0]);
}

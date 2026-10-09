//! Regression tests for the statistical-graphics roles (intervals, facets,
//! scales, reference lines, top-k labels, smoothing methods) and for the
//! rendering bugs fixed alongside them.

use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn svg(cols: Vec<Column>) -> String {
    render(&cols, 480, 320).expect("renders")
}
/// `(x, width)` of every `<rect>` carrying `data-series="s"`.
fn bars(svg: &str, s: &str) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find("<rect ") {
        let tag = &rest[i..i + rest[i..].find('>').unwrap()];
        if tag.contains(&format!("data-series=\"{s}\"")) {
            let attr = |a: &str| -> f64 {
                let k = tag.find(&format!(" {a}=\"")).unwrap() + a.len() + 3;
                tag[k..k + tag[k..].find('"').unwrap()].parse().unwrap()
            };
            out.push((attr("x"), attr("width")));
        }
        rest = &rest[i + 5..];
    }
    out
}
/// Point lists of non-grid `<polyline>`s.
fn polylines(svg: &str) -> Vec<String> {
    svg.split("<polyline ")
        .skip(1)
        .filter(|t| !t.contains("#EBEBEB"))
        .map(|t| t[..t.find('>').unwrap()].to_string())
        .collect()
}

// ── A1: grouped bars are dodged, not drawn on top of each other ──────────
#[test]
fn bar_with_category_is_dodged() {
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W1", "W1", "W2", "W2"])),
        Column::new("c", Role::Category, strs(&["app", "web", "app", "web"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[30.0, 22.0, 41.0, 28.0])),
    ]);
    let (app, web) = (bars(&s, "app"), bars(&s, "web"));
    assert_eq!((app.len(), web.len()), (2, 2), "{s}");
    for (a, w) in app.iter().zip(&web) {
        assert!(
            a.0 + a.1 <= w.0 + 0.01,
            "app bar left of web bar: {a:?} {w:?}"
        );
    }
    // A real discrete x (position_dodge): every bar names its x level.
    assert!(s.contains("data-x=\"W1\"") && !s.contains("data-xticks="));
    // Data labels cannot follow a discrete dodge: that chart keeps numeric
    // slots, and hosts map a slot back to its x level.
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W1", "W1", "W2", "W2"])),
        Column::new("c", Role::Category, strs(&["app", "web", "app", "web"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[30.0, 22.0, 41.0, 28.0])),
        Column::new("l", Role::DataLabels, nums(&[11.0])),
    ]);
    let (app, web) = (bars(&s, "app"), bars(&s, "web"));
    assert!(app[0].0 + app[0].1 <= web[0].0 + 0.01, "{app:?} {web:?}");
    assert!(s.contains("data-xticks="), "x level map on the root");
}

// Dodged groups follow the legend (sorted) order, whatever the row order, and
// the x levels keep their first-seen order.
#[test]
fn dodge_order_follows_legend() {
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W2", "W2", "W1", "W1"])),
        Column::new("c", Role::Category, strs(&["web", "app", "web", "app"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[30.0, 22.0, 41.0, 28.0])),
    ]);
    let (app, web) = (bars(&s, "app"), bars(&s, "web"));
    for (a, w) in app.iter().zip(&web) {
        assert!(a.0 + a.1 <= w.0 + 0.01, "{a:?} {w:?}");
    }
    let (w2, w1) = (s.find(">W2</text>").unwrap(), s.find(">W1</text>").unwrap());
    assert!(w2 < w1, "W2 is the first x level");
}

// ── A2: ::BARCHART_PERCENT shows shares, not count × 100 ──────────────────
#[test]
fn bar_percent_is_share_of_x_total() {
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W1", "W1"])),
        Column::new("c", Role::Category, strs(&["a", "b"])),
        Column::new("n", Role::Value(Kind::BarPercent), nums(&[30.0, 10.0])),
    ]);
    assert!(!s.contains("000%"), "no 3000% ticks");
    assert!(s.contains("data-value=\"0.75\""), "30 of 40 → 0.75");
    // Fractions are drawn as given.
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["a", "b"])),
        Column::new("n", Role::Value(Kind::BarPercent), nums(&[0.42, 0.3])),
    ]);
    assert!(s.contains("data-value=\"0.42\""));
    // A line in percent units (> 1) keeps its numbers: "40%", not "4000%".
    let s = svg(vec![
        Column::new("x", Role::X, nums(&[0.0, 1.0])),
        Column::new("n", Role::Value(Kind::LinePercent), nums(&[20.0, 40.0])),
    ]);
    assert!(!s.contains("000%") && s.contains("40%"), "{s}");
}

// ── A3: ::STEP draws one line per category ───────────────────────────────
#[test]
fn step_per_category() {
    let s = svg(vec![
        Column::new("x", Role::X, nums(&[0.0, 1.0, 2.0, 0.0, 1.0, 2.0])),
        Column::new("c", Role::Category, strs(&["a", "a", "a", "b", "b", "b"])),
        Column::new(
            "y",
            Role::Value(Kind::Step),
            nums(&[1.0, 2.0, 3.0, 5.0, 6.0, 7.0]),
        ),
    ]);
    let lines = polylines(&s);
    assert_eq!(lines.len(), 2, "two step lines: {lines:?}");
    assert!(lines[0].contains("#456481") && lines[1].contains("#E86433"));
}

// ── A4: maps keep their fill (measure, numeric ::CHOROPLETH, category) ───
#[test]
fn map_fill_by_value_and_category() {
    let wkt = strs(&[
        "POLYGON((0 0,1 0,1 1,0 1,0 0))",
        "POLYGON((1 0,2 0,2 1,1 1,1 0))",
    ]);
    let fills = |s: &str| -> Vec<String> {
        s.split("<polygon ")
            .skip(1)
            .map(|t| t[t.find("fill=\"").unwrap() + 6..][..7].to_string())
            .collect()
    };
    // `v::CHOROPLETH` (an alias of ::MAP) holding numbers — as DuckDB writes
    // DECIMALs — is the measure.
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new("v", Role::Geometry, strs(&["1.0", "9.0"])),
    ]);
    let f = fills(&s);
    assert_eq!(f.len(), 2);
    assert_ne!(f[0], f[1], "gradient fill by value: {f:?}");
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new("c", Role::Category, strs(&["a", "b"])),
    ]);
    assert_eq!(fills(&s), vec!["#456481", "#E86433"]);
    // Unfilled: a brand tint, not ggplot's default blue.
    let s = svg(vec![Column::new("g", Role::Geometry, wkt)]);
    assert!(!s.contains("#619CFF"));
}

// ── A5: violins and boxes have a brand-tinted body ───────────────────────
#[test]
fn violin_and_box_are_filled() {
    for kind in [Kind::Violin, Kind::Boxplot] {
        let x: Vec<&str> = (0..40)
            .map(|i| if i % 2 == 0 { "a" } else { "b" })
            .collect();
        let y: Vec<f64> = (0..40)
            .map(|i| (i as f64).sin() * 5.0 + (i % 7) as f64)
            .collect();
        let s = svg(vec![
            Column::new("x", Role::X, strs(&x)),
            Column::new("y", Role::Value(kind), nums(&y)),
        ]);
        assert!(
            !s.contains("fill=\"#FFFFFF\" fill-opacity=\"1.000\" stroke"),
            "{kind:?}"
        );
        assert!(s.contains("#CBD4DC"), "{kind:?} brand tint");
    }
}

fn coef(kind: Kind, extra: Vec<Column>) -> Vec<Column> {
    let mut v = vec![
        Column::new("term", Role::X, strs(&["a", "b", "a", "b"])),
        Column::new("m", Role::Category, strs(&["m1", "m1", "m2", "m2"])),
        Column::new("est", Role::Value(kind), nums(&[1.0, -0.5, 0.8, -0.2])),
        Column::new("lo", Role::YMin, nums(&[0.5, -1.0, 0.4, -0.6])),
        Column::new("hi", Role::YMax, nums(&[1.5, 0.0, 1.2, 0.2])),
    ];
    v.extend(extra);
    v
}

// ── B1: intervals ─────────────────────────────────────────────────────────
#[test]
fn ymin_ymax_pointrange_flipped_and_dodged() {
    let s = svg(coef(
        Kind::Point,
        vec![
            Column::new("f", Role::Flip, nums(&[1.0])),
            Column::new("z", Role::RefLine, nums(&[0.0])),
        ],
    ));
    assert!(s.contains("data-flip=\"true\""));
    // Horizontal interval segments (flip-safe), one per row and colour.
    let segs: Vec<String> = polylines(&s)
        .into_iter()
        .filter(|p| p.contains("stroke-width=\"1.40\""))
        .collect();
    assert_eq!(segs.len(), 4, "{segs:?}");
    for p in &segs {
        let pts = &p[p.find("points=\"").unwrap() + 8..];
        let ys: Vec<&str> = pts[..pts.find('"').unwrap()]
            .split(' ')
            .map(|xy| xy.split(',').nth(1).unwrap())
            .collect();
        assert_eq!(ys[0], ys[1], "horizontal: {p}");
    }
    // Dodged: the two models of one term are at different heights.
    let mut ys: Vec<&str> = segs
        .iter()
        .map(|p| p.split(',').nth(1).unwrap().split(' ').next().unwrap())
        .collect();
    ys.sort();
    ys.dedup();
    assert_eq!(ys.len(), 4, "every interval on its own row");
    // The zero reference line survives coord_flip: a vertical rule.
    let zero = polylines(&s)
        .into_iter()
        .find(|p| p.contains("data-value=\"0\""))
        .expect("zero line");
    let pts = &zero[zero.find("points=\"").unwrap() + 8..];
    let xs: Vec<&str> = pts[..pts.find('"').unwrap()]
        .split(' ')
        .map(|xy| xy.split(',').next().unwrap())
        .collect();
    assert!(xs.windows(2).all(|w| w[0] == w[1]), "vertical: {zero}");
}

#[test]
fn ymin_ymax_on_bars_are_error_bars() {
    let s = svg(coef(Kind::Bar, vec![]));
    // 4 bars from 0 (not from ymin) + 4 capped whiskers (cap–bar–cap), each
    // centred on its dodged bar.
    let grey: Vec<String> = polylines(&s)
        .into_iter()
        .filter(|p| p.contains("#3C3C3C"))
        .collect();
    assert_eq!(grey.len(), 4, "{s}");
    let mut centres: Vec<f64> = bars(&s, "m1")
        .into_iter()
        .chain(bars(&s, "m2"))
        .map(|(x, w)| x + w / 2.0)
        .collect();
    centres.sort_by(f64::total_cmp);
    let mut whiskers: Vec<f64> = grey
        .iter()
        .map(|p| {
            let pts = &p[p.find("points=\"").unwrap() + 8..];
            pts.split(' ')
                .nth(2)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    whiskers.sort_by(f64::total_cmp);
    for (c, w) in centres.iter().zip(&whiskers) {
        assert!(
            (c - w).abs() < 0.5,
            "whisker on its bar: {centres:?} {whiskers:?}"
        );
    }
    assert_eq!(bars(&s, "m1").len() + bars(&s, "m2").len(), 4);
}

#[test]
fn lone_ymin_is_an_error() {
    let cols = vec![
        Column::new("x", Role::X, nums(&[1.0])),
        Column::new("y", Role::Value(Kind::Point), nums(&[1.0])),
        Column::new("lo", Role::YMin, nums(&[0.5])),
    ];
    let e = render(&cols, 300, 200).unwrap_err();
    assert!(e.contains("::YMIN needs a matching ::YMAX"), "{e}");
}

#[test]
fn xmin_xmax_segments() {
    let s = svg(vec![
        Column::new("x", Role::X, nums(&[1.0, 2.0])),
        Column::new("y", Role::Value(Kind::Point), nums(&[1.0, 2.0])),
        Column::new("a", Role::XMin, nums(&[0.5, 1.5])),
        Column::new("b", Role::XMax, nums(&[1.5, 2.5])),
    ]);
    assert_eq!(polylines(&s).len(), 2);
}

// ── B2: abline / identity ─────────────────────────────────────────────────
#[test]
fn abline_and_identity_in_data_units() {
    let xs: Vec<f64> = (0..10).map(|i| i as f64).collect();
    let s = svg(vec![
        Column::new("x", Role::X, nums(&xs)),
        Column::new("y", Role::Value(Kind::Point), nums(&xs)),
        Column::new("i", Role::Identity, nums(&[1.0])),
        Column::new("a", Role::AbLine, strs(&["0.5,1"])),
    ]);
    let lines = polylines(&s);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("stroke-dasharray")),
        "identity dashed"
    );
    let bad = render(
        &[
            Column::new("x", Role::X, nums(&xs)),
            Column::new("y", Role::Value(Kind::Point), nums(&xs)),
            Column::new("a", Role::AbLine, strs(&["steep"])),
        ],
        300,
        200,
    )
    .unwrap_err();
    assert!(bad.contains("slope,intercept"), "{bad}");
}

// ── B3: facets ────────────────────────────────────────────────────────────
#[test]
fn facet_wrap_and_free() {
    let n = 12;
    let x: Vec<f64> = (0..n).map(|i| (i % 4) as f64).collect();
    let g: Vec<&str> = (0..n).map(|i| ["a", "b", "c"][i / 4]).collect();
    let y: Vec<f64> = (0..n).map(|i| (i * i) as f64).collect();
    for role in [Role::Facet, Role::FacetFree] {
        let s = svg(vec![
            Column::new("x", Role::X, nums(&x)),
            Column::new("y", Role::Value(Kind::Line), nums(&y)),
            Column::new("g", role, strs(&g)),
            Column::new("n", Role::FacetCols, nums(&[3.0])),
        ]);
        for strip in ["a", "b", "c"] {
            assert!(
                s.contains(&format!(">{strip}</text>")),
                "{role:?} strip {strip}"
            );
        }
    }
    // A reference line is drawn in every panel.
    let s = svg(vec![
        Column::new("x", Role::X, nums(&x)),
        Column::new("y", Role::Value(Kind::Line), nums(&y)),
        Column::new("g", Role::Facet, strs(&g)),
        Column::new("r", Role::RefLine, nums(&[50.0])),
    ]);
    let rules = polylines(&s)
        .into_iter()
        .filter(|p| p.contains("data-value=\"50\""))
        .count();
    assert_eq!(rules, 3, "one per panel");
}

// ── B4: axis transforms ──────────────────────────────────────────────────
#[test]
fn axis_scales() {
    let x: Vec<f64> = (1..6).map(|i| 10f64.powi(i)).collect();
    let s = svg(vec![
        Column::new("x", Role::X, nums(&x)),
        Column::new("y", Role::Value(Kind::Line), nums(&x)),
        Column::new("xs", Role::XScale, strs(&["log10"])),
        Column::new("ys", Role::YScale, strs(&["log10"])),
    ]);
    // log10 ticks at powers of ten.
    assert!(
        s.contains(">1000</text>") || s.contains(">1e3</text>"),
        "{s}"
    );
    let e = render(
        &[
            Column::new("x", Role::X, nums(&x)),
            Column::new("y", Role::Value(Kind::Line), nums(&x)),
            Column::new("ys", Role::YScale, strs(&["cubic"])),
        ],
        300,
        200,
    )
    .unwrap_err();
    assert!(e.contains("'log10', 'sqrt', 'reverse'"), "{e}");
}

// ── B5: top-k labels ─────────────────────────────────────────────────────
#[test]
fn label_top_k() {
    let x: Vec<f64> = (0..10).map(|i| i as f64).collect();
    let y: Vec<f64> = (0..10)
        .map(|i| if i == 7 { 50.0 } else { i as f64 })
        .collect();
    let names: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let base = || {
        vec![
            Column::new("x", Role::X, nums(&x)),
            Column::new("y", Role::Value(Kind::Point), nums(&y)),
            Column::new("n", Role::Label, strs(&names)),
            Column::new("k", Role::LabelTop, nums(&[1.0])),
        ]
    };
    let s = svg(base());
    assert!(s.contains(">p7</text>"), "largest |y| labelled");
    assert!(!s.contains(">p3</text>"));
    assert!(!s.contains(">p0</text>"), "::LABEL is no title here");
    let mut by_rank = base();
    by_rank.push(Column::new(
        "r",
        Role::Rank,
        nums(&[0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ));
    let s = svg(by_rank);
    assert!(s.contains(">p3</text>") && !s.contains(">p7</text>"));
}

// ── B6: smoothing methods ────────────────────────────────────────────────
#[test]
fn smooth_methods() {
    let x: Vec<f64> = (0..40).map(|i| i as f64).collect();
    let y: Vec<f64> = x.iter().map(|v| (v / 5.0).sin() * 10.0).collect();
    let curve = |method: Option<&str>| -> String {
        let mut c = vec![
            Column::new("x", Role::X, nums(&x)),
            Column::new("y", Role::Value(Kind::Smooth), nums(&y)),
        ];
        if let Some(m) = method {
            c.push(Column::new("m", Role::SmoothMethod, strs(&[m])));
        }
        let s = svg(c);
        polylines(&s)
            .into_iter()
            .find(|p| p.contains("stroke-width=\"2.00\""))
            .unwrap_or_default()
    };
    let (loess, lm, gam) = (curve(None), curve(Some("lm")), curve(Some("gam")));
    assert!(!loess.is_empty() && !lm.is_empty() && !gam.is_empty());
    assert_ne!(loess, lm);
    assert_ne!(gam, lm);
    assert_eq!(curve(Some("glm")), lm, "glm = Gaussian lm");
    let e = render(
        &[
            Column::new("x", Role::X, nums(&x)),
            Column::new("y", Role::Value(Kind::Smooth), nums(&y)),
            Column::new("m", Role::SmoothMethod, strs(&["spline"])),
        ],
        300,
        200,
    )
    .unwrap_err();
    assert!(e.contains("::SMOOTH_METHOD 'spline'"), "{e}");
}

// ── Contract plots (`{"plot": …}` specs, what the anofox_plot_* macros send) ──
fn spec(plot: &str, rows: &str, options: &str) -> Result<String, String> {
    anofox_visualization::host::render_spec_checked(
        &format!(
            r#"{{"plot":"{plot}","rows":{rows},"options":{options},"width":640,"height":400}}"#
        ),
        &anofox_visualization::host::RenderLimits::default(),
    )
}

#[test]
fn contract_terms_forest_and_path() {
    let s = spec(
        "terms",
        r#"[{"model_id":"a","term":"x1","estimate":0.5,"conf_low":0.1,"conf_high":0.9},
            {"model_id":"b","term":"x1","estimate":"0.4","conf_low":0.2,"conf_high":0.6}]"#,
        "{}",
    )
    .unwrap();
    assert!(s.contains("data-flip=\"true\"") && s.contains(">x1</text>"));
    let s = spec(
        "terms",
        r#"[{"term":"x1","estimate":0.5,"index_name":"lambda","index_value":0.01},
            {"term":"x1","estimate":0.2,"index_name":"lambda","index_value":0.1},
            {"term":"x2","estimate":0.1,"index_name":"lambda","index_value":0.01},
            {"term":"x2","estimate":0.0,"index_name":"lambda","index_value":0.1}]"#,
        "{}",
    )
    .unwrap();
    assert!(s.contains(">x2</text>") && s.contains("estimate by lambda"));
}

#[test]
fn contract_prediction_splits_and_options() {
    let rows = r#"[{"x1":1,"y":1.0,"yhat":1.1,"yhat_lower":0.5,"yhat_upper":1.5,"is_training":true},
                   {"x1":2,"y":2.0,"yhat":1.9,"yhat_lower":1.4,"yhat_upper":2.4,"is_training":false},
                   {"x1":3,"y":null,"yhat":3.0,"yhat_lower":2.4,"yhat_upper":3.6,"is_training":false}]"#;
    let s = spec("prediction", rows, r#"{"x":"x1"}"#).unwrap();
    for split in ["train", "test", "future"] {
        assert!(s.contains(&format!(">{split}</text>")), "{split}");
    }
    let e = spec("prediction", rows, "{}").unwrap_err();
    assert!(e.contains("needs an x position"), "{e}");
}

#[test]
fn contract_auto_dispatch_and_errors() {
    let s = spec(
        "auto",
        r#"[{"row_id":1,"fitted":1.0,"residual":0.2},{"row_id":2,"fitted":2.0,"residual":-0.1},
            {"row_id":3,"fitted":3.0,"residual":0.0},{"row_id":4,"fitted":4.0,"residual":0.3}]"#,
        "{}",
    )
    .unwrap();
    assert!(s.contains("Normal Q-Q") && s.contains("Residuals vs fitted"));
    let e = spec("auto", r#"[{"a":1}]"#, "{}").unwrap_err();
    assert!(e.contains("match no plottable schema"), "{e}");
    let e = spec("bogus", "[]", "{}").unwrap_err();
    assert!(e.contains("unknown plot 'bogus'"), "{e}");
}

#[test]
fn every_macro_is_documented() {
    let doc = include_str!("../docs/DOCS.md");
    for m in anofox_visualization::macros::MACROS {
        assert!(
            doc.contains(m.name),
            "docs/DOCS.md does not mention {}",
            m.name
        );
    }
}

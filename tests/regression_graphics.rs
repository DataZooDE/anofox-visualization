//! Regression tests for the statistical-graphics roles (intervals, facets,
//! scales, reference lines, top-k labels, smoothing methods) and for the
//! rendering bugs fixed alongside them.

use anofox_visualization::theme::DEFAULT_PALETTE;
use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
/// Default palette slot `i` as it appears in an SVG (`#rrggbb`, upper case).
fn slot(i: usize) -> String {
    let (r, g, b) = DEFAULT_PALETTE[i];
    format!("#{r:02X}{g:02X}{b:02X}")
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
/// `(x, text)` of every `::DATALABELS` text mark (`data-geom="text"`).
fn data_label_centers(svg: &str) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find("<text ") {
        let end = i + rest[i..].find('>').unwrap();
        let tag = &rest[i..end];
        let body_end = rest[end..].find("</text>").unwrap() + end;
        if tag.contains("data-geom=\"text\"") {
            let k = tag.find(" x=\"").unwrap() + 4;
            let x: f64 = tag[k..k + tag[k..].find('"').unwrap()].parse().unwrap();
            out.push((x, rest[end + 1..body_end].to_string()));
        }
        rest = &rest[body_end + "</text>".len()..];
    }
    out
}

/// Point lists of non-grid `<polyline>`s (the grid is drawn in the theme's
/// grid colour).
fn polylines(svg: &str) -> Vec<String> {
    let (r, g, b) = anofox_visualization::theme::default_light().chart_grid();
    let grid = format!("#{r:02X}{g:02X}{b:02X}");
    svg.split("<polyline ")
        .skip(1)
        .filter(|t| !t.contains(&grid))
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
    // ggplot-rs 0.22's GeomText/GeomLabel honour `position_dodge` on a
    // discrete x too (ggplot-rs#318 B3-4), so ::DATALABELS dodges the same
    // real discrete x the bars do — no more numeric-slot fallback, and so
    // no `data-xticks` root attribute mapping a slot back to its level.
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W1", "W1", "W2", "W2"])),
        Column::new("c", Role::Category, strs(&["app", "web", "app", "web"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[30.0, 22.0, 41.0, 28.0])),
        Column::new("l", Role::DataLabels, nums(&[11.0])),
    ]);
    let (app, web) = (bars(&s, "app"), bars(&s, "web"));
    assert!(app[0].0 + app[0].1 <= web[0].0 + 0.01, "{app:?} {web:?}");
    assert!(s.contains("data-x=\"W1\""), "a real discrete x level");
    assert!(!s.contains("data-xticks="), "no numeric-slot tick map");
    // Each label sits over its own bar's centre, not its group's (a sibling
    // layer needs its own position_dodge() — GeomText honouring dodge
    // doesn't mean it inherits the bars' own position() call).
    let labels = data_label_centers(&s);
    assert_eq!(labels.len(), 4, "{s}");
    let center = |b: (f64, f64)| b.0 + b.1 / 2.0;
    let bar_for = |t: &str| match t {
        "30" => app[0],
        "22" => web[0],
        "41" => app[1],
        "28" => web[1],
        other => panic!("unexpected label {other}"),
    };
    for (x, text) in &labels {
        let want = center(bar_for(text));
        assert!(
            (x - want).abs() < 0.5,
            "label {text} at x={x} not over its bar (centre {want}): {labels:?} {app:?} {web:?}"
        );
    }
    // No two labels share (nearly) the same x: that's the overlap bug this
    // guards against (both of a group's labels stacking at one x).
    for i in 0..labels.len() {
        for j in (i + 1)..labels.len() {
            assert!(
                (labels[i].0 - labels[j].0).abs() > 1.0,
                "{:?} and {:?} overlap",
                labels[i],
                labels[j]
            );
        }
    }
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
    let (l0, l1) = (lines[0].to_uppercase(), lines[1].to_uppercase());
    assert!(l0.contains(&slot(0)) && l1.contains(&slot(1)), "{lines:?}");
}

// ── A4: maps keep their fill (measure, numeric ::CHOROPLETH, category) ───
#[test]
fn map_fill_by_value_and_category() {
    let wkt = strs(&[
        "POLYGON((0 0,1 0,1 1,0 1,0 0))",
        "POLYGON((1 0,2 0,2 1,1 1,1 0))",
    ]);
    // the features: the map writer's `<path>`s that carry a data-row
    let fills = |s: &str| -> Vec<String> {
        s.split("<path ")
            .skip(1)
            .filter(|t| t[..t.find('>').unwrap()].contains("data-row="))
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
    let f: Vec<String> = fills(&s).iter().map(|c| c.to_uppercase()).collect();
    assert_eq!(f, vec![slot(0), slot(1)]);
    // Unfilled: a brand tint, not ggplot's default blue.
    let s = svg(vec![Column::new("g", Role::Geometry, wkt)]);
    assert!(!s.contains("#619CFF"));
}

// ── A4b: map features carry their row, series, value and label ───────────
/// The opening tags of the drawn map features (`<path`, `<circle`) that
/// carry a `data-row`.
fn map_marks(svg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find(" data-row=\"") {
        let start = rest[..i].rfind('<').unwrap();
        let end = i + rest[i..].find('>').unwrap();
        out.push(rest[start..=end].to_string());
        rest = &rest[end..];
    }
    out
}

#[test]
fn map_marks_carry_row_series_value_and_label() {
    let wkt = strs(&[
        "LINESTRING(0 0, 2 1, 3 3)",
        "POINT(2 1)",
        "POINT(3 3)",
        "POLYGON((0 0,1 0,1 1,0 1,0 0))",
    ]);
    // By ::CATEGORY, with a multi-line ::LABEL: the series is the category,
    // the label rides along (newlines as &#10;), the title is unchanged.
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new(
            "c",
            Role::Category,
            strs(&["Van 1", "Van 1", "Van 2", "late \"stop\""]),
        ),
        Column::new(
            "l",
            Role::Label,
            vec![
                Value::Str("Van 1: 12 km".into()),
                Value::Str("Bakery & Co\nstop #1".into()),
                Value::Na,
                Value::Str("late".into()),
            ],
        ),
    ]);
    assert!(s.starts_with("<svg") && s[..s.find('>').unwrap()].contains(" data-map=\"\""));
    let m = map_marks(&s);
    assert_eq!(m.len(), 4, "{m:?}");
    assert!(
        // lines and areas are <path>s (a line unfilled)
        m[0].starts_with("<path ")
            && m[0].contains(" fill=\"none\"")
            && m[0].contains(" data-row=\"0\" data-series=\"Van 1\" data-label=\"Van 1: 12 km\">")
    );
    assert!(
        m[1].starts_with("<circle ")
            && m[1].contains(
                " data-row=\"1\" data-series=\"Van 1\" data-label=\"Bakery &amp; Co&#10;stop #1\">"
            )
    );
    // no label text: the title is the plain series, as before
    assert!(
        m[2].contains(" data-row=\"2\" data-series=\"Van 2\">"),
        "{}",
        m[2]
    );
    assert!(s.contains("data-series=\"Van 2\"><title>Van 2</title>"));
    assert!(m[3].starts_with("<path ") && m[3].contains(" data-series=\"late &quot;stop&quot;\""));
    assert!(s.contains("<title>Bakery &amp; Co\nstop #1: Van 1</title>"));
    assert!(
        !s.contains('\u{E000}') && !s.contains('\u{E001}'),
        "no marker left behind"
    );

    // By a measure with a ::LABEL (a choropleth): the label is the series,
    // the raw measure the value.
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.5, 3.0, 4.0])),
        Column::new("l", Role::Label, strs(&["A", "B", "C", "D"])),
    ]);
    let m = map_marks(&s);
    assert!(
        m[1].contains(" data-row=\"1\" data-series=\"B\" data-value=\"2.5\" data-label=\"B\""),
        "{}",
        m[1]
    );
    assert!(s.contains("<title>B: 2.5</title>"));

    // Unfilled and unlabelled: rows only, and no tooltip (as before).
    let s = svg(vec![Column::new("g", Role::Geometry, wkt)]);
    let m = map_marks(&s);
    assert_eq!(m.len(), 4);
    assert!(m.iter().all(|t| t.ends_with("\"/>")), "{m:?}");
    assert!(!s.contains("<title>"), "no empty tooltips");
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
        assert!(
            s.contains("#C2D3F9"),
            "{kind:?} brand tint (slot 0, lightened)"
        );
    }
}

// ── A box plot from statistics (#157): one row per box looks like the
// raw-rows box plot, light and dark; its tooltip names every number ──────
/// The `fill` / `stroke` values of every `<rect>` and `<circle>` (the
/// boxes, outlier circles and mean dots; the grid is drawn as lines).
fn mark_colours(svg: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for tag in svg
        .split('<')
        .filter(|t| t.starts_with("rect ") || t.starts_with("circle "))
    {
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        if !tag.contains("<title>") && !tag.contains("data-") && tag.starts_with("rect ") {
            continue; // background / panel rects
        }
        for a in [" fill=\"", " stroke=\""] {
            if let Some(k) = tag.find(a) {
                let v = &tag[k + a.len()..];
                out.insert(v[..v.find('"').unwrap()].to_string());
            }
        }
    }
    out
}

// ggplot-rs 0.22's GeomBoxplot widens the y scale to the outlier extent
// itself (`setup_data`'s internal ymin_final/ymax_final), so the old
// `boxstats::extent_rows` geom_blank layer is gone; the mean dot's own y
// training was always just its ordinary GeomPoint layer, same as any other
// mapped y. Both a mean and an outlier far outside the whiskers must still
// widen the y axis (no silently clipped extent).
#[test]
fn box_from_statistics_y_scale_covers_a_far_mean_and_outlier() {
    let stats = vec![
        Column::new("x", Role::X, strs(&["a"])),
        Column::new("q1", Role::Q1, nums(&[2.0])),
        Column::new("m", Role::Median, nums(&[4.0])),
        Column::new("q3", Role::Q3, nums(&[6.0])),
        Column::new("lo", Role::WhiskerLow, nums(&[0.0])),
        Column::new("hi", Role::WhiskerHigh, nums(&[8.0])),
        Column::new("mean", Role::Mean, nums(&[55.0])),
        Column::new("out", Role::Outliers, strs(&["[-40]"])),
    ];
    let s = svg(stats);
    // The mean dot (y=55) and the outlier (y=-40) must both land strictly
    // inside the panel — proof the y domain widened to cover them, not
    // clipped at the whiskers [0, 8].
    let plot: Vec<f64> = {
        let k = s.find("data-plot=\"").unwrap() + 11;
        s[k..k + s[k..].find('"').unwrap()]
            .split(' ')
            .map(|v| v.parse().unwrap())
            .collect()
    };
    let mean_dot = s
        .split("<circle ")
        .skip(1)
        .find(|t| t.contains("r=\"2.60\""))
        .expect("the mean dot (r=2.6)");
    let outlier = s
        .split("<circle ")
        .skip(1)
        .find(|t| t.contains("r=\"2.00\""))
        .expect("the outlier circle (r=2)");
    for c in [mean_dot, outlier] {
        let k = c.find("cy=\"").unwrap() + 4;
        let cy: f64 = c[k..k + c[k..].find('"').unwrap()].parse().unwrap();
        assert!(
            cy >= plot[1] - 0.5 && cy <= plot[1] + plot[3] + 0.5,
            "cy {cy} outside the panel {plot:?}: the y scale clipped it"
        );
    }
}

#[test]
fn box_from_statistics_looks_like_the_raw_box() {
    use anofox_visualization::{render_with, theme, RenderOptions};
    use std::sync::Arc;
    // Raw rows whose quartiles the engine computes …
    let x: Vec<&str> = (0..40)
        .map(|i| if i % 2 == 0 { "a" } else { "b" })
        .collect();
    let y: Vec<f64> = (0..40)
        .map(|i| (i % 9) as f64 + if i == 38 { 30.0 } else { 0.0 })
        .collect();
    let raw = vec![
        Column::new("x", Role::X, strs(&x)),
        Column::new("y", Role::Value(Kind::Boxplot), nums(&y)),
    ];
    // … and one row per box, computed elsewhere.
    let stats = vec![
        Column::new("x", Role::X, strs(&["a", "b"])),
        Column::new("q1", Role::Q1, nums(&[2.0, 3.0])),
        Column::new("m", Role::Median, nums(&[4.0, 5.0])),
        Column::new("q3", Role::Q3, nums(&[6.0, 7.0])),
        Column::new("lo", Role::WhiskerLow, nums(&[0.0, 1.0])),
        Column::new("hi", Role::WhiskerHigh, nums(&[8.0, 8.0])),
        Column::new("mean", Role::Mean, nums(&[4.4, 5.1])),
        Column::new("out", Role::Outliers, strs(&["[30, 31.5]", "[]"])),
    ];
    for dark in [false, true] {
        let set = theme::default_set();
        let o = RenderOptions {
            theme: Arc::new(if dark {
                set.dark.clone()
            } else {
                set.light.clone()
            }),
            ..RenderOptions::default()
        };
        let r = render_with(&raw, 480, 320, &o).expect("raw renders");
        let s = render_with(&stats, 480, 320, &o).expect("stats render");
        // The same body fill and outline as the raw box plot.
        let (rc, sc) = (mark_colours(&r), mark_colours(&s));
        assert!(rc.is_subset(&sc), "dark={dark}: raw {rc:?} vs stats {sc:?}");
        if !dark {
            assert!(s.contains("#C2D3F9"), "brand tint");
        }
        // Five numbers (and the mean, the outliers) in the box's tooltip.
        assert!(
            s.contains(
                "<title>a: median 4 · Q1 2 · Q3 6 · whiskers 0 – 8 · mean 4.4 · 2 outliers</title>"
            ),
            "{s}"
        );
        assert!(s.contains("<title>b: median 5 · Q1 3 · Q3 7 · whiskers 1 – 8 · mean 5.1</title>"));
        assert!(
            !s.contains("median 4 (2–6)"),
            "the engine's tooltip is replaced"
        );
        assert!(
            !s.contains("mean: "),
            "a mean dot has no tooltip of its own"
        );
        // Two outlier circles (the engine's, r = 2), inside the panel: the
        // y scale covers them.
        let open: Vec<&str> = s
            .split("<circle ")
            .skip(1)
            .filter(|t| t[..t.find('>').unwrap()].contains("r=\"2.00\""))
            .collect();
        assert_eq!(open.len(), 2, "{s}");
        let plot: Vec<f64> = {
            let k = s.find("data-plot=\"").unwrap() + 11;
            s[k..k + s[k..].find('"').unwrap()]
                .split(' ')
                .map(|v| v.parse().unwrap())
                .collect()
        };
        for c in &open {
            let k = c.find("cy=\"").unwrap() + 4;
            let cy: f64 = c[k..k + c[k..].find('"').unwrap()].parse().unwrap();
            assert!(
                cy >= plot[1] - 0.5 && cy <= plot[1] + plot[3] + 0.5,
                "outlier at {cy} outside {plot:?}"
            );
        }
    }
}

#[test]
fn box_from_statistics_needs_its_quartiles() {
    let base = || {
        vec![
            Column::new("x", Role::X, strs(&["a"])),
            Column::new("q1", Role::Q1, nums(&[2.0])),
            Column::new("m", Role::Median, nums(&[4.0])),
        ]
    };
    let e = render(&base(), 480, 320).unwrap_err();
    assert!(e.contains("::Q3"), "{e}");
    let mut with_raw = base();
    with_raw.push(Column::new("q3", Role::Q3, nums(&[6.0])));
    assert!(render(&with_raw, 480, 320).is_ok());
    with_raw.push(Column::new("y", Role::Value(Kind::Boxplot), nums(&[4.0])));
    assert!(render(&with_raw, 480, 320)
        .unwrap_err()
        .contains("no chart measure"));
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

// Labels of nearby points are repelled apart rather than dropped.
#[test]
fn label_top_k_repels_close_labels() {
    let x: Vec<f64> = (0..10)
        .map(|i| {
            if i >= 8 {
                9.0 + 0.01 * i as f64
            } else {
                i as f64
            }
        })
        .collect();
    let y: Vec<f64> = (0..10)
        .map(|i| if i >= 8 { 50.0 } else { i as f64 })
        .collect();
    let names: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let s = svg(vec![
        Column::new("x", Role::X, nums(&x)),
        Column::new("y", Role::Value(Kind::Point), nums(&y)),
        Column::new("n", Role::Label, strs(&names)),
        Column::new("k", Role::LabelTop, nums(&[2.0])),
    ]);
    assert!(s.contains(">p8</text>") && s.contains(">p9</text>"), "{s}");
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

// ── Residuals vs leverage: Cook's distance contours ─────────────────────
#[test]
fn cooks_contours_on_leverage() {
    let h: Vec<f64> = (1..30)
        .map(|i| 0.01 + 0.4 * (i as f64 / 30.0).powi(3))
        .collect();
    let r: Vec<f64> = (1..30).map(|i| ((i as f64) * 0.9).sin() * 2.0).collect();
    let s = svg(vec![
        Column::new("h", Role::X, nums(&h)),
        Column::new("r", Role::Value(Kind::Point), nums(&r)),
        Column::new("p", Role::CooksContour, nums(&[3.0])),
    ]);
    assert!(s.contains("0.5") && s.contains("stroke-dasharray"), "{s}");
    let dashed = polylines(&s)
        .into_iter()
        .filter(|p| p.contains("stroke-dasharray"))
        .count();
    assert!(dashed >= 2, "±contours: {dashed}");
}

// ── QQ: points on a pointwise envelope ───────────────────────────────────
#[test]
fn qq_has_a_band() {
    let y: Vec<f64> = (0..30).map(|i| ((i as f64) * 0.37).sin() * 2.0).collect();
    let s = svg(vec![Column::new("r", Role::Value(Kind::QQ), nums(&y))]);
    assert_eq!(s.matches("<polygon ").count(), 1, "one envelope: {s}");
    assert_eq!(s.matches("<circle ").count(), 30);
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
    assert!(s.contains(">x1</text>") && s.contains(">a</text>") && s.contains(">b</text>"));
    assert!(s.contains("estimate (CI)"), "default x label");
    let s = spec(
        "terms",
        r#"[{"term":"x1","estimate":0.5,"index_name":"lambda","index_value":0.01},
            {"term":"x1","estimate":0.2,"index_name":"lambda","index_value":0.1},
            {"term":"x2","estimate":0.1,"index_name":"lambda","index_value":0.01},
            {"term":"x2","estimate":0.0,"index_name":"lambda","index_value":0.1}]"#,
        "{}",
    )
    .unwrap();
    // One panel, direct labels, a log-λ axis; no inner title.
    assert!(s.contains(">x2</text>") && s.contains("λ (log scale)"));
    assert!(!s.contains("estimate by"));
}

#[test]
fn contract_prediction_splits_and_options() {
    let rows = r#"[{"x1":1,"y":1.0,"yhat":1.1,"yhat_lower":0.5,"yhat_upper":1.5,"is_training":true},
                   {"x1":2,"y":2.0,"yhat":1.9,"yhat_lower":1.4,"yhat_upper":2.4,"is_training":false},
                   {"x1":3,"y":null,"yhat":3.0,"yhat_lower":2.4,"yhat_upper":3.6,"is_training":false}]"#;
    let s = spec("prediction", rows, r#"{"x":"x1"}"#).unwrap();
    // Split boundaries as labelled vertical lines.
    for split in ["hold-out", "forecast"] {
        assert!(s.contains(&format!(" {split}</text>")), "{split}");
    }
    let e = spec("prediction", rows, "{}").unwrap_err();
    assert!(e.contains("needs an x position"), "{e}");
}

// A forecast with its history unioned in: history rows have no yhat, future
// rows no y. Both are part of the schema, so the SVG carries no render warning.
#[test]
fn contract_prediction_history_and_future_no_warnings() {
    let rows = r#"[{"id":"a","ds":1,"y":1.0,"yhat":null},{"id":"a","ds":2,"y":2.0,"yhat":null},
                   {"id":"a","ds":3,"y":null,"yhat":3.0,"yhat_lower":2.5,"yhat_upper":3.5},
                   {"id":"a","ds":4,"y":null,"yhat":4.0,"yhat_lower":3.2,"yhat_upper":4.8}]"#;
    let s = spec("prediction", rows, "{}").unwrap();
    assert!(!s.contains("data-warnings"), "{}", &s[..s.len().min(400)]);
    assert!(s.contains(" forecast</text>"));
}

// Kaplan–Meier: one step line per group, a step-ribbon band per group in its
// colour, and `+` censoring marks.
#[test]
fn contract_km_band_and_censor_marks() {
    let mut rows = Vec::new();
    for (g, s) in [("a", 5.0f64), ("b", 9.0)] {
        for t in 0..6 {
            let y = (-(t as f64) / s).exp();
            rows.push(format!(
                r#"{{"curve_type":"km","series":"{g}","x":{t},"y":{y},"y_low":{},"y_high":{},"n_censor":{}}}"#,
                y * 0.9,
                (y * 1.1).min(1.0),
                (t == 3) as u8
            ));
        }
    }
    let s = spec("curve", &format!("[{}]", rows.join(",")), "{}").unwrap();
    assert!(!s.contains("Kaplan-Meier") && s.contains("survival probability"));
    // Two step ribbons, filled in the two series colours.
    let ribbons: Vec<&str> = s
        .split("<polygon ")
        .skip(1)
        .filter(|t| t.contains("fill-opacity=\"0.180\""))
        .collect();
    assert_eq!(ribbons.len(), 2, "{s}");
    let (r0, r1) = (ribbons[0].to_uppercase(), ribbons[1].to_uppercase());
    assert!(
        r0.contains(&slot(0)) && r1.contains(&slot(1)),
        "{ribbons:?}"
    );
    // A `+` censoring mark per group (stroked paths).
    assert_eq!(s.matches("<path ").count(), 2, "{s}");
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
    // A 2×2 PlotGrid of nested sub-plots.
    assert!(
        s.contains("data-grid=\"2 2\"") && s.contains("data-panel=\"3\""),
        "{s}"
    );
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

/// A term without inference (NULL interval, e.g. ridge) is still drawn as a
/// point — it used to be dropped as non-finite with the whole pointrange.
#[test]
fn interval_rows_without_bounds_keep_their_point() {
    let s = svg(vec![
        Column::new("term", Role::X, strs(&["a", "b", "a", "b"])),
        Column::new("m", Role::Category, strs(&["ols", "ols", "ridge", "ridge"])),
        Column::new(
            "est",
            Role::Value(Kind::Point),
            nums(&[1.0, -0.5, 0.8, -0.2]),
        ),
        Column::new(
            "lo",
            Role::YMin,
            vec![Value::Float(0.5), Value::Float(-1.0), Value::Na, Value::Na],
        ),
        Column::new(
            "hi",
            Role::YMax,
            vec![Value::Float(1.5), Value::Float(0.0), Value::Na, Value::Na],
        ),
        Column::new("f", Role::Flip, nums(&[1.0])),
    ]);
    assert!(
        !s.contains("data-warnings"),
        "no rows dropped: {}",
        &s[..s.len().min(400)]
    );
    let ridge = s.matches("data-series=\"ridge\"").count();
    assert!(ridge >= 2, "both ridge terms drawn, found {ridge}");
}

// ── statistics-plot polish (titles, labels, colours, per-plot details) ────

fn path_rows(name: &str, model: &str) -> String {
    let mut r = Vec::new();
    for (i, x) in [0.01f64, 0.1, 1.0, 10.0].iter().enumerate() {
        for (t, b) in [("tv", 2.0), ("radio", 0.5), ("search", 1.5)] {
            r.push(format!(
                r#"{{"model_id":"{model}","term":"{t}","estimate":{},"index_name":"{name}","index_value":{x}}}"#,
                b / (1.0 + i as f64)
            ));
        }
    }
    format!("[{}]", r.join(","))
}

#[test]
fn contract_default_titles_dropped_and_options_apply() {
    let roc = r#"[{"curve_type":"roc","x":0,"y":0},{"curve_type":"roc","x":0.5,"y":0.8},{"curve_type":"roc","x":1,"y":1}]"#;
    let s = spec("curve", roc, "{}").unwrap();
    assert!(!s.contains("ROC curve"), "no inner title by default");
    assert!(s.contains("false positive rate") && s.contains("true positive rate"));
    let s = spec(
        "curve",
        roc,
        r#"{"title":"My ROC","x_label":"FPR","y_label":"TPR"}"#,
    )
    .unwrap();
    assert!(s.contains("My ROC") && s.contains(">FPR<") && s.contains(">TPR<"));
    let glance = r#"[{"model_id":"ols","metric":"r_squared","value":0.81},{"model_id":"ridge","metric":"r_squared","value":0.80},
        {"model_id":"ols","metric":"aic","value":72.4}]"#;
    let s = spec("summary", glance, "{}").unwrap();
    assert!(!s.contains("Model comparison"));
}

#[test]
fn contract_roc_auc_in_legend_and_pr_base_rate() {
    let roc = r#"[{"curve_type":"roc","model_id":"logistic","x":0,"y":0},{"curve_type":"roc","model_id":"logistic","x":0,"y":1},{"curve_type":"roc","model_id":"logistic","x":1,"y":1},
                  {"curve_type":"roc","model_id":"rule","x":0,"y":0},{"curve_type":"roc","model_id":"rule","x":1,"y":1}]"#;
    let s = spec("curve", roc, "{}").unwrap();
    assert!(
        s.contains("logistic · AUC 1.00") && s.contains("rule · AUC 0.50"),
        "{s}"
    );
    let pr = r#"[{"curve_type":"pr","x":0.0,"y":1.0,"threshold":0.9},{"curve_type":"pr","x":0.5,"y":0.6,"threshold":0.5},
                 {"curve_type":"pr","x":1.0,"y":0.3,"threshold":0.1}]"#;
    let s = spec("curve", pr, "{}").unwrap();
    assert!(s.contains("base rate 0.3"), "inferred base rate");
    let s = spec("curve", pr, r#"{"base_rate":0.25}"#).unwrap();
    assert!(s.contains("base rate 0.25"));
}

#[test]
fn contract_acf_drops_lag_zero() {
    let acf = r#"[{"curve_type":"acf","x":0,"y":1.0,"y_low":-0.2,"y_high":0.2},{"curve_type":"acf","x":1,"y":0.5,"y_low":-0.2,"y_high":0.2},
                  {"curve_type":"acf","x":2,"y":0.25,"y_low":-0.2,"y_high":0.2}]"#;
    let s = spec("curve", acf, "{}").unwrap();
    // Two lollipop heads (lags 1, 2), not three.
    assert_eq!(s.matches("<circle ").count(), 2, "{s}");
    assert!(s.contains(">lag<") && s.contains(">autocorrelation<"));
}

#[test]
fn contract_lambda_path_one_panel_with_marks() {
    let rows = path_rows("lambda", "ridge");
    let s = spec(
        "terms",
        &rows,
        r#"{"lambda_min":0.1,"lambda_1se":1.0,"highlight":["tv"]}"#,
    )
    .unwrap();
    assert!(s.contains("λ.min") && s.contains("λ.1se"));
    assert!(s.contains(">tv<") && s.contains(">radio<") && s.contains(">search<"));
    assert!(s.contains(">coefficient<"));
    // One panel: a single `<svg` document (no grid).
    assert_eq!(s.matches("<svg").count(), 1);
    // facet := true keeps the panel-per-term layout.
    let f = spec("terms", &rows, r#"{"facet":true}"#).unwrap();
    assert!(f.matches("<svg").count() > 1, "grid of term panels");
}

#[test]
fn contract_lars_path_on_l1_fraction() {
    let s = spec("terms", &path_rows("alpha", "lars"), "{}").unwrap();
    assert!(s.contains("fraction of L1 norm") && !s.contains("glmnet"));
}

#[test]
fn contract_quantile_process_with_ols_reference_and_rolling_dates() {
    let mut r = Vec::new();
    for tau in [0.1, 0.5, 0.9] {
        r.push(format!(
            r#"{{"model_id":"quantile","term":"x1","estimate":{tau},"conf_low":0,"conf_high":1,"index_name":"tau","index_value":{tau}}}"#
        ));
    }
    r.push(
        r#"{"model_id":"ols","term":"x1","estimate":0.45,"conf_low":0.4,"conf_high":0.5}"#.into(),
    );
    let s = spec("terms", &format!("[{}]", r.join(",")), "{}").unwrap();
    assert!(s.contains("τ (quantile)") && s.contains("full-sample estimate"));
    let rolling = r#"[{"term":"price","estimate":-1.0,"index_name":"window_end","index_value":20,"window_end":"2024-05-20 00:00:00"},
                      {"term":"price","estimate":-1.5,"index_name":"window_end","index_value":21,"window_end":"2024-05-27 00:00:00"},
                      {"term":"price","estimate":-2.0,"index_name":"window_end","index_value":22,"window_end":"2024-06-03 00:00:00"}]"#;
    let s = spec(
        "terms",
        rolling,
        r#"{"reference":[{"term":"price","estimate":-1.4}]}"#,
    )
    .unwrap();
    assert!(s.contains("window end") && s.contains("2024"), "date axis");
}

#[test]
fn contract_forest_marks_rows_without_ci() {
    let rows = r#"[{"model_id":"ols","term":"x1","estimate":0.5,"conf_low":0.1,"conf_high":0.9},
                   {"model_id":"ridge","term":"x1","estimate":0.4,"conf_low":null,"conf_high":null}]"#;
    let s = spec("terms", rows, "{}").unwrap();
    assert!(s.contains("no interval reported") && s.contains("(no CI)"));
}

#[test]
fn contract_model_colours_are_stable_across_charts() {
    // The same model id keeps its colour whatever else is plotted.
    let one = r#"[{"model_id":"ols","term":"x1","estimate":0.5,"conf_low":0.1,"conf_high":0.9},
                  {"model_id":"ridge","term":"x1","estimate":0.4,"conf_low":0.2,"conf_high":0.6}]"#;
    let two = r#"[{"model_id":"huber","term":"x1","estimate":0.5,"conf_low":0.1,"conf_high":0.9},
                  {"model_id":"ridge","term":"x1","estimate":0.4,"conf_low":0.2,"conf_high":0.6}]"#;
    let ridge = slot(5); // ridge = DataZoo slot 5, mapped to the default palette's slot 5
    for rows in [one, two] {
        let s = spec("terms", rows, "{}").unwrap();
        assert!(
            s.to_uppercase().contains(&ridge),
            "ridge keeps its colour in every chart"
        );
    }
}

#[test]
fn contract_summary_order_na_and_baseline() {
    let glance = r#"[{"model_id":"a","metric":"r_squared","value":0.70},{"model_id":"b","metric":"r_squared","value":0.80},
        {"model_id":"b","metric":"aic","value":10.0}]"#;
    let s = spec("summary", glance, "{}").unwrap();
    assert!(s.contains(">n/a<") && s.contains("Δ = 0.1"));
    assert!(spec("summary", glance, r#"{"baseline":"zero"}"#).is_ok());
    assert!(spec("summary", glance, r#"{"baseline":"nope"}"#).is_err());
}

#[test]
fn contract_prediction_band_and_difference() {
    let p = r#"[{"x":1,"y":1.0,"yhat":1.1,"yhat_lower":0.8,"yhat_upper":1.4,"model_id":"ols"},
        {"x":2,"y":2.0,"yhat":1.9,"yhat_lower":1.6,"yhat_upper":2.2,"model_id":"ols"},
        {"x":1,"y":1.0,"yhat":1.0,"yhat_lower":0.7,"yhat_upper":1.3,"model_id":"huber"},
        {"x":2,"y":2.0,"yhat":2.0,"yhat_lower":1.7,"yhat_upper":2.3,"model_id":"huber"}]"#;
    let s = spec("prediction", p, "{}").unwrap();
    assert_eq!(s.matches("<polygon ").count(), 1, "one band: {s}");
    let d = spec("prediction", p, r#"{"kind":"difference"}"#).unwrap();
    assert!(d.contains("difference vs ols"));
    let effects = r#"[{"model_id":"logistic","focal":1,"focal_name":"tenure_months","x":1,"yhat":0.4},
                      {"model_id":"logistic","focal":1,"focal_name":"tenure_months","x":2,"yhat":0.3}]"#;
    let s = spec("prediction", effects, "{}").unwrap();
    assert!(s.contains("P(y = 1)") && s.contains("tenure months"));
}

#[test]
fn contract_tests_fallback_without_ci_and_pair_labels() {
    let rows = r#"[{"test_id":"B-A","group1":"A","group2":"B","estimate":1.2,"p_adj":0.01,"method":"t tests"},
                   {"test_id":"C-A","group1":"A","group2":"C","estimate":0.2,"p_adj":0.6,"method":"t tests"}]"#;
    let s = spec("tests", rows, "{}").unwrap();
    assert!(s.contains("no CI reported") && s.contains("B − A"));
    assert!(s.contains("p = 0.05"), "significance line labelled");
    let s = spec("tests", rows, r#"{"label":"stars"}"#).unwrap();
    assert!(s.contains(">*<") || s.contains(">**<"));
}

#[test]
fn contract_partial_binary_term_has_group_means() {
    let mut r = Vec::new();
    for i in 0..20 {
        let x = (i % 2) as f64;
        r.push(format!(
            r#"{{"term":"promo","x":{x},"partial_residual":{},"component":{}}}"#,
            x * 2.0 + (i as f64 * 0.37).sin(),
            x * 2.0
        ));
    }
    let s = spec("partial", &format!("[{}]", r.join(",")), "{}").unwrap();
    // No smoother path through two columns of points; two mean dots.
    assert!(!s.contains("loess"));
    assert!(s.matches("<circle ").count() >= 22, "points + means");
}

#[test]
fn contract_km_median_and_number_at_risk() {
    let km = r#"[{"curve_type":"km","series":"a","x":2,"y":0.8,"n_risk":10,"n_event":2,"n_censor":0},
                 {"curve_type":"km","series":"a","x":5,"y":0.4,"n_risk":8,"n_event":4,"n_censor":1},
                 {"curve_type":"km","series":"b","x":3,"y":0.6,"n_risk":10,"n_event":4,"n_censor":0}]"#;
    let s = spec("curve", km, "{}").unwrap();
    assert!(s.contains("number at risk") && s.contains(">10<"));
}

#[test]
fn contract_cooks_index_labels_and_reference() {
    let mut r = Vec::new();
    for i in 1..=20 {
        let d = if i == 7 { 0.9 } else { 0.01 * (i % 3) as f64 };
        r.push(format!(
            r#"{{"row_id":{i},"fitted":{i},"residual":0.1,"cooks_d":{d}}}"#
        ));
    }
    let s = spec(
        "obs",
        &format!("[{}]", r.join(",")),
        r#"{"kind":"cooks","label_top":1}"#,
    )
    .unwrap();
    assert!(
        s.contains(">4/n<") && s.contains(">7<") && s.contains("Cook"),
        "{s}"
    );
}

#[test]
fn contract_rolling_accepts_a_date_index_and_compare_stars() {
    let rows = r#"[{"term":"price","estimate":-1.0,"conf_low":-2,"conf_high":0,"index_name":"window_end","index_value":"2024-05-20"},
                   {"term":"price","estimate":-1.5,"conf_low":-2.5,"conf_high":-0.5,"index_name":"window_end","index_value":"2024-05-27"}]"#;
    let s = spec("terms", rows, "{}").unwrap();
    assert!(s.contains("window end"));
    let data = r#"[{"g":"a","y":1.0},{"g":"a","y":1.4},{"g":"b","y":2.1},{"g":"b","y":2.6}]"#;
    let t = r#"[{"group1":"a","group2":"b","p_adj":0.004}]"#;
    let s = spec(
        "compare",
        data,
        &format!(r#"{{"value":"y","factor":"g","label":"stars","tests":{t}}}"#),
    )
    .unwrap();
    assert!(s.contains(">**<") && !s.contains(">stars<"));
    // A ggpubr template still works.
    let s = spec(
        "compare",
        data,
        &format!(r#"{{"value":"y","factor":"g","label":"p = {{p.adj}}","tests":{t}}}"#),
    )
    .unwrap();
    assert!(s.contains("p = 0.004"), "{s}");
}

#[test]
fn plain_bars_with_intervals_are_one_colour() {
    let s = svg(vec![
        Column::new(
            "x",
            Role::X,
            vec![
                Value::Str("a".into()),
                Value::Str("b".into()),
                Value::Str("c".into()),
            ],
        ),
        Column::new("y", Role::Value(Kind::Bar), nums(&[1.0, 2.0, 3.0])),
        Column::new("lo", Role::YMin, nums(&[0.5, 1.5, 2.5])),
        Column::new("hi", Role::YMax, nums(&[1.5, 2.5, 3.5])),
    ]);
    assert!(
        !s.contains("#E86433") && !s.contains("#e86433"),
        "no per-bar palette"
    );
}

// ── Chart chrome follows the dashboard theme ─────────────────────────────
fn hexu((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn render_in(cols: &[Column], spec: &str, dark: bool) -> String {
    use anofox_visualization::{render_with, RenderOptions};
    let set = anofox_visualization::theme::resolve(spec).unwrap();
    let t = if dark { set.dark } else { set.light };
    let o = RenderOptions {
        theme: std::sync::Arc::new(t),
        ..RenderOptions::default()
    };
    render_with(cols, 480, 320, &o).unwrap()
}

#[test]
fn chart_text_and_grid_use_theme_tokens() {
    let cols = vec![
        Column::new("w", Role::X, strs(&["W1", "W2", "W3"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[30.0, 22.0, 41.0])),
    ];
    for dark in [false, true] {
        let set = anofox_visualization::theme::resolve("light").unwrap();
        let t = if dark { &set.dark } else { &set.light };
        let s = render_in(&cols, "light", dark);
        // No ggplot defaults: grey30 tick text, grey92 grid, dark bar outlines,
        // the generic font.
        for gone in [
            "#4D4D4D",
            "#EBEBEB",
            "#323232",
            "font-family=\"sans-serif\"",
        ] {
            assert!(!s.contains(gone), "dark={dark}: {gone} left in {s}");
        }
        assert!(
            s.contains(&format!("fill=\"{}\">W1<", hexu(t.chart_text()))),
            "{s}"
        );
        assert!(
            s.contains(&hexu(t.chart_grid())),
            "grid in the theme colour"
        );
        assert!(s.contains("Hanken Grotesk"), "theme font family");
        // Bars part with a card-coloured hairline.
        assert!(
            s.contains(&format!("stroke=\"{}\"", hexu(t.colors.card))),
            "{s}"
        );
    }
    // A custom theme's muted ink paints the ticks.
    let acme =
        r##"{"name":"acme","colors":{"muted":"#5b6478"},"font":{"family":"Inter, sans-serif"}}"##;
    let s = render_in(&cols, acme, false);
    assert!(
        s.contains("fill=\"#5B6478\">W1<") && s.contains("font-family=\"Inter, sans-serif\""),
        "{s}"
    );
}

fn heatmap_cols(labels: bool) -> Vec<Column> {
    let mut cols = vec![
        Column::new("w", Role::X, strs(&["W1", "W2", "W1", "W2"])),
        Column::new("c", Role::Y, strs(&["web", "web", "api", "api"])),
        Column::new(
            "v",
            Role::Value(Kind::Heatmap),
            nums(&[1.5, 40.25, 20.0, 1234.5]),
        ),
        Column::new("f", Role::YFormat, strs(&["$"])),
    ];
    if labels {
        cols.push(Column::new("l", Role::DataLabels, strs(&[""])));
    }
    cols
}

#[test]
fn heatmap_labels_contrast_and_order() {
    let t = anofox_visualization::theme::default_light();
    let s = svg(heatmap_cols(true));
    // Values in the panel format, written into the cells.
    assert!(s.contains(">$1.5<") && s.contains(">$1,235<"), "{s}");
    // Dark text on the pale low cell, light (card) text on the deep high cell.
    assert!(
        s.contains(&format!(
            "fill=\"{}\" class=\"gg-mark gg-text\" data-geom=\"text\">$1.5<",
            hexu(t.colors.ink)
        )),
        "{s}"
    );
    assert!(
        s.contains(&format!(
            "fill=\"{}\" class=\"gg-mark gg-text\" data-geom=\"text\">$1,235<",
            hexu(t.colors.card)
        )),
        "{s}"
    );
    // Rows keep the SQL order, first on top: "web" is drawn above "api".
    let y_of = |label: &str| -> f64 {
        let i = s.find(&format!(">{label}</text>")).unwrap();
        let tag = &s[s[..i].rfind("<text").unwrap()..i];
        let k = tag.find(" y=\"").unwrap() + 4;
        tag[k..k + tag[k..].find('"').unwrap()].parse().unwrap()
    };
    assert!(y_of("web") < y_of("api"));
    // Thin card-coloured gaps between cells, no dark borders.
    assert!(s.contains(&format!(
        "stroke=\"{}\" stroke-width=\"2.00\"",
        hexu(t.colors.card)
    )));
    assert!(!s.contains("#323232"));
    // Formatted tooltip values beside the raw ones; the colourbar in dollars.
    assert!(
        s.contains("data-value=\"1234.5\"") && s.contains("data-fvalue=\"$1,234.50\""),
        "{s}"
    );
    assert!(
        s.contains(">W1, api: $20<") || s.contains("W1, api: $20"),
        "{s}"
    );
    // Without ::DATALABELS no cell text.
    assert!(!svg(heatmap_cols(false)).contains(">$1.5<"));
}

#[test]
fn heatmap_diverges_around_zero() {
    let t = anofox_visualization::theme::default_light();
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["a", "b", "c"])),
        Column::new("c", Role::Y, strs(&["r", "r", "r"])),
        Column::new("v", Role::Value(Kind::Heatmap), nums(&[-10.0, 0.0, 10.0])),
    ]);
    let [lo, mid, hi] = t.diverging_ramp();
    for c in [lo, mid, hi] {
        assert!(s.contains(&hexu(c)), "{} in {s}", hexu(c));
    }
}

fn status_heatmap(extra: Column) -> Vec<Column> {
    vec![
        Column::new("w", Role::X, strs(&["a", "b", "c"])),
        Column::new("c", Role::Y, strs(&["r", "r", "r"])),
        Column::new("v", Role::Value(Kind::Heatmap), nums(&[10.0, 30.0, 60.0])),
        Column::new("f", Role::YFormat, strs(&["percent"])),
        Column::new("l", Role::DataLabels, strs(&[""])),
        extra,
    ]
}

/// `::COLORSCALE` on a heatmap: a status ramp good → warn → bad in the
/// theme's status tiles (light, dark, colour-blind), reversed for
/// `_HIGH_GOOD`; tooltips keep the formatted value.
#[test]
fn heatmap_colorscale_runs_good_to_bad() {
    use anofox_visualization::Scale;
    for spec in ["light", "colorblind", "high-contrast"] {
        for dark in [false, true] {
            let set = anofox_visualization::theme::resolve(spec).unwrap();
            let t = if dark { &set.dark } else { &set.light };
            let [good, _, bad] = t.status_tiles();
            let cs = |sc| Column::new("s", Role::ColorScale(sc), strs(&[""]));
            let s = render_in(&status_heatmap(cs(Scale::LowGood)), spec, dark);
            let fill_at = |s: &str, label: &str| -> String {
                let i = s.find(&format!("data-fvalue=\"{label}\"")).expect(label);
                let tag = &s[s[..i].rfind('<').unwrap()..i];
                let k = tag.find("fill=\"").unwrap() + 6;
                tag[k..k + 7].to_string()
            };
            assert_eq!(fill_at(&s, "10%"), hexu(good), "{spec} dark={dark}");
            assert_eq!(fill_at(&s, "60%"), hexu(bad), "{spec} dark={dark}");
            let s = render_in(&status_heatmap(cs(Scale::HighGood)), spec, dark);
            assert_eq!(fill_at(&s, "10%"), hexu(bad), "{spec} dark={dark}");
            assert_eq!(fill_at(&s, "60%"), hexu(good), "{spec} dark={dark}");
        }
    }
}

/// `::THRESHOLDS` on a heatmap: one status colour per band, a stepped legend
/// naming the bands high to low, data labels legible on each band.
#[test]
fn heatmap_thresholds_step_by_band() {
    for dark in [false, true] {
        let set = anofox_visualization::theme::resolve("light").unwrap();
        let t = if dark { &set.dark } else { &set.light };
        let [good, warn, bad] = t.status_tiles();
        let rules = Column::new("t", Role::Thresholds, strs(&[">=50:bad; >=20:warn; good"]));
        let s = render_in(&status_heatmap(rules), "light", dark);
        for c in [good, warn, bad] {
            assert!(s.contains(&hexu(c)), "{} in {s}", hexu(c));
        }
        let hi = s.find(">≥ 50%<").expect("≥ 50% band");
        let mid = s.find(">20%–50%<").expect("20–50% band");
        let lo = s
            .find(">< 20%<")
            .or_else(|| s.find(">&lt; 20%<"))
            .expect("< 20% band");
        assert!(hi < mid && mid < lo, "bands listed high to low");
        for (v, fill) in [("10%", good), ("30%", warn), ("60%", bad)] {
            let ink = hexu(t.text_on(fill));
            assert!(
                s.contains(&format!(
                    "fill=\"{ink}\" class=\"gg-mark gg-text\" data-geom=\"text\">{v}<"
                )),
                "{v} ink {ink}"
            );
        }
        assert!(s.contains("data-fvalue=\"30%\""), "tooltips keep the value");
    }
}

/// `::COLORSCALE_DIVERGING`: negative bad, positive good, the neutral midpoint
/// at 0 — and in the dark variant the near-midpoint tiles stay data colours
/// (the dark remap greys neither them nor the theme's own diverging tiles).
#[test]
fn heatmap_diverging_status_and_dark_midpoint() {
    use anofox_visualization::Scale;
    let cols = |extra: Option<Column>| {
        let mut v = vec![
            Column::new("w", Role::X, strs(&["a", "b", "c", "d"])),
            Column::new("c", Role::Y, strs(&["r", "r", "r", "r"])),
            Column::new(
                "v",
                Role::Value(Kind::Heatmap),
                nums(&[-10.0, -1.0, 0.0, 10.0]),
            ),
        ];
        v.extend(extra);
        v
    };
    for dark in [false, true] {
        let set = anofox_visualization::theme::resolve("light").unwrap();
        let t = if dark { &set.dark } else { &set.light };
        let [bad, good] = t.status_arms();
        let mid = t.diverging_ramp()[1];
        let div = Column::new("s", Role::ColorScale(Scale::Diverging), strs(&[""]));
        let s = render_in(&cols(Some(div)), "light", dark);
        for c in [bad, mid, good] {
            assert!(s.contains(&hexu(c)), "{} in {s}", hexu(c));
        }
        // The tile just below zero: a hint of the bad (red) end, not a grey the
        // dark remap inverted; likewise on the theme's own diverging ramp.
        let fill_of = |s: &str| -> (u8, u8, u8) {
            let i = s.find("data-value=\"-1\"").expect("tile -1");
            let tag = &s[s[..i].rfind('<').unwrap()..i];
            let k = tag.find("fill=\"#").unwrap() + 7;
            let p = |j: usize| u8::from_str_radix(&tag[k + j..k + j + 2], 16).unwrap();
            (p(0), p(2), p(4))
        };
        let near = fill_of(&s);
        let d = |a: (u8, u8, u8), b: (u8, u8, u8)| {
            (a.0 as i32 - b.0 as i32).abs()
                + (a.1 as i32 - b.1 as i32).abs()
                + (a.2 as i32 - b.2 as i32).abs()
        };
        assert!(
            near.0 > near.1 && d(near, mid) < 60,
            "near-zero {near:?} (dark={dark})"
        );
        let near = fill_of(&render_in(&cols(None), "light", dark));
        assert!(
            d(near, t.diverging_ramp()[1]) < 60,
            "theme near-zero {near:?} (dark={dark})"
        );
    }
}

/// `(x, y, width, height)` of every `<rect data-geom="tile">` — a heatmap cell.
fn tile_rects(svg: &str) -> Vec<(f64, f64, f64, f64)> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find("<rect ") {
        let tag = &rest[i..i + rest[i..].find('>').unwrap()];
        if tag.contains("data-geom=\"tile\"") {
            let attr = |a: &str| -> f64 {
                let k = tag.find(&format!(" {a}=\"")).unwrap() + a.len() + 3;
                tag[k..k + tag[k..].find('"').unwrap()].parse().unwrap()
            };
            out.push((attr("x"), attr("y"), attr("width"), attr("height")));
        }
        rest = &rest[i + 6..];
    }
    out
}

/// #332: a row with fewer distinct x points than its neighbours (a sparse
/// `::YAXIS` level on a shared, dense date `::XAXIS`) still gets tiles the
/// same size as every other row — the x scale's resolution (the smallest
/// gap anywhere in the shared column, ggplot-rs `GeomTile`) and the y
/// scale's fixed category slots size every tile alike; only the sparse
/// row's row has fewer of them, with gaps where it has no data (not taller
/// or narrower ones).
#[test]
fn heatmap_tiles_are_uniform_for_a_sparse_row() {
    let dense = [
        "2026-09-01",
        "2026-09-03",
        "2026-09-05",
        "2026-09-07",
        "2026-09-09",
        "2026-09-11",
        "2026-09-13",
        "2026-09-15",
    ];
    let sparse = ["2026-09-01", "2026-09-08", "2026-09-15"];
    let mut x = Vec::new();
    let mut y = Vec::new();
    for d in dense {
        x.push(d);
        y.push("sys1");
    }
    for d in sparse {
        x.push(d);
        y.push("sys2");
    }
    let cols = vec![
        Column::new("w", Role::X, strs(&x)),
        Column::new("c", Role::Y, strs(&y)),
        Column::new(
            "v",
            Role::Value(Kind::Heatmap),
            nums(&vec![0.0; dense.len() + sparse.len()]),
        ),
    ];
    let s = svg(cols);
    let tiles = tile_rects(&s);
    assert_eq!(tiles.len(), dense.len() + sparse.len(), "{s}");
    let (w0, h0) = (tiles[0].2, tiles[0].3);
    for (x, y, w, h) in &tiles {
        assert!(
            (w - w0).abs() < 1e-6 && (h - h0).abs() < 1e-6,
            "every tile is {w0}x{h0}; got {w}x{h} at ({x}, {y}) — {s}"
        );
    }
    // sys1 (dense, on top) and sys2 (sparse, below) occupy the same height.
    let heights_of = |series: &str| -> Vec<f64> {
        let mut rest = s.as_str();
        let mut out = Vec::new();
        while let Some(i) = rest.find("<rect ") {
            let tag = &rest[i..i + rest[i..].find('>').unwrap()];
            if tag.contains("data-geom=\"tile\"")
                && tag.contains(&format!("data-series=\"{series}\""))
            {
                let k = tag.find(" y=\"").unwrap() + 4;
                out.push(tag[k..k + tag[k..].find('"').unwrap()].parse().unwrap());
            }
            rest = &rest[i + 6..];
        }
        out
    };
    let sys1_y = heights_of("sys1");
    let sys2_y = heights_of("sys2");
    assert!(!sys1_y.is_empty() && !sys2_y.is_empty(), "{s}");
    assert!(
        (sys1_y[0] - sys2_y[0]).abs() > 1.0,
        "sys1 and sys2 are drawn in different bands: {s}"
    );
}

/// #332 follow-up: a *date* `::XAXIS` (the real case — DuckDB's DATE type,
/// `Value::DateTime` here, not the text dates the test above uses) must
/// still have every tile fill its grid column, not ggplot-rs `GeomTile`'s
/// own continuous-axis `resolution()` (the smallest gap *anywhere in the
/// whole shared column*): with two rows at different, interleaved cadences
/// — sys1 every 2 days, sys2 every 7 but landing 1 day from a sys1 point —
/// that global minimum (1 day) is narrower than either row's own spacing
/// (2 and 7 days), so a tile used to fill only a fraction of the gap to its
/// neighbour. `render_heatmap`'s `heatmap_x_axis` discretizes a date x the
/// same way the `::YAXIS` already is, so every tile now fills the full
/// column width (its x step, the gap to its neighbour on the same row) —
/// asserted at >= 90% to allow the thin card-coloured gap between cells.
#[test]
fn heatmap_date_x_tiles_fill_their_column_width() {
    const DAY: i64 = 86_400;
    const T0: i64 = 1_788_220_800; // 2026-09-01 UTC, a whole day
    let dense_days = [0, 2, 4, 6, 8, 10, 12, 14]; // every 2 days
    let sparse_days = [0, 7, 14]; // every 7 days — day 7 sits 1 day from day 6/8
    let mut x = Vec::new();
    let mut y = Vec::new();
    for &d in &dense_days {
        x.push(Value::DateTime(T0 + d * DAY));
        y.push(Value::Str("sys1".to_string()));
    }
    for &d in &sparse_days {
        x.push(Value::DateTime(T0 + d * DAY));
        y.push(Value::Str("sys2".to_string()));
    }
    let n = dense_days.len() + sparse_days.len();
    let cols = vec![
        Column::new("w", Role::X, x),
        Column::new("c", Role::Y, y),
        Column::new("v", Role::Value(Kind::Heatmap), nums(&vec![0.0; n])),
    ];
    let s = svg(cols);
    let tiles = tile_rects(&s);
    assert_eq!(tiles.len(), n, "{s}");
    // Grid-column width: the x gap between any two adjacent distinct days
    // in the series — the minimum here is 1 day (sys1's day 6 to sys2's
    // day 7), which the discretized axis turns into one shared slot width.
    let mut xs: Vec<f64> = tiles.iter().map(|(x, _, _, _)| *x).collect();
    xs.sort_by(|a, b| a.total_cmp(b));
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let step = xs
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(f64::INFINITY, f64::min);
    let (w0, h0) = (tiles[0].2, tiles[0].3);
    for (tx, ty, w, h) in &tiles {
        assert!(
            (w - w0).abs() < 1e-6 && (h - h0).abs() < 1e-6,
            "every tile is {w0}x{h0}; got {w}x{h} at ({tx}, {ty}) — {s}"
        );
    }
    assert!(
        w0 >= 0.9 * step,
        "tile width {w0} is less than 90% of the x step {step} — {s}"
    );
    // The axis prints clean dates, not a half-day continuous-axis artefact.
    assert!(s.contains(">2026-09-"), "{s}");
    assert!(!s.contains("12:00:00"), "{s}");
    // #332 follow-up: thinning the printed labels (9 distinct days, this
    // panel only fits a handful) must not trip ggplot-rs 0.22's own
    // crowded-axis auto-rotate — it estimates spacing from *every* grid
    // column, not just the ones that got a label, so it still thinks this
    // is too crowded to sit flat and rotates to 45°, which `flat_date_labels`
    // (`axis_text_x_angle`, a real theme parameter) opts out of. The kept
    // labels stay centred under their tick (`text-anchor="middle"`), not
    // right-anchored the way a genuinely rotated label is.
    assert!(!s.contains("rotate(45.0"), "{s}");
    assert!(!s.contains("rotate(90.0"), "{s}");
    assert!(!s.contains("rotate(270.0"), "{s}");
    let before_label = &s[..s.find(">2026-09-01<").unwrap()];
    let tag_start = before_label.rfind("<text").unwrap();
    assert!(
        before_label[tag_start..].contains("text-anchor=\"middle\""),
        "the kept label stays centred under its tick, not right-anchored: {}",
        &before_label[tag_start..]
    );
}

/// #332: a text `::HEATMAP` measure (a status word, coloured with `::COLORS`
/// as a level map) renders — never the old `Conversion Error: Could not
/// convert string … to DOUBLE` from the SQL planner forcing the column to
/// DOUBLE, nor a blank "No numeric values to plot" note — and its legend
/// prints the words themselves, not numeric band boundaries.
#[test]
fn heatmap_text_measure_gets_a_categorical_legend() {
    let cols = vec![
        Column::new("w", Role::X, strs(&["a", "b", "c", "d"])),
        Column::new("c", Role::Y, strs(&["r", "r", "r", "r"])),
        Column::new(
            "v",
            Role::Value(Kind::Heatmap),
            strs(&["blocked", "peer violation", "QP-ready", "QP-ready"]),
        ),
        Column::new(
            "m",
            Role::GaugeColors,
            strs(&["blocked:bad; peer violation:warn; QP-ready:good"]),
        ),
    ];
    let t = anofox_visualization::theme::ResolvedTheme::default();
    let s = render(&cols, 480, 320).expect("renders").to_lowercase();
    for (word, (r, g, b)) in [
        ("blocked", t.colors.negative),
        ("peer violation", t.colors.warning),
        ("qp-ready", t.colors.positive),
    ] {
        let hex = format!("#{r:02x}{g:02x}{b:02x}");
        let rgb = format!("rgb({r},{g},{b})");
        assert!(
            s.contains(&hex) || s.contains(&rgb),
            "{word}'s colour in {s}"
        );
    }
    // The legend names the words, in the map's written order — never a
    // numeric band like "≥ 2" / "1–2".
    let (b, p, q) = (
        s.find(">blocked<"),
        s.find(">peer violation<"),
        s.find(">qp-ready<"),
    );
    assert!(b.is_some() && p.is_some() && q.is_some(), "{s}");
    assert!(b < p && p < q, "legend order follows the ::COLORS map: {s}");
    assert!(!s.contains("no numeric values"), "{s}");
    // Without ::COLORS: still a plain discrete legend (dz_scale), not an error.
    let plain = render(&cols[..3], 480, 320).expect("renders");
    assert!(
        !plain.to_lowercase().contains("no numeric values"),
        "{plain}"
    );
}

#[test]
fn tooltip_values_follow_yformat_and_locale() {
    use anofox_visualization::{format::Locale, render_with, RenderOptions};
    let cols = vec![
        Column::new("d", Role::X, strs(&["2024-03-01", "2024-03-02"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[1234.5, 12.0])),
        Column::new("f", Role::YFormat, strs(&["€"])),
    ];
    let s = svg(cols.clone());
    assert!(s.contains("data-fvalue=\"€1,234.50\""), "{s}");
    assert!(!s.contains("data-fx="), "en-US keeps ISO dates");
    let o = RenderOptions {
        locale: Locale::DE_DE,
        ..RenderOptions::default()
    };
    let s = render_with(&cols, 480, 320, &o).unwrap();
    assert!(
        s.contains("data-fvalue=\"1.234,50 €\"") && s.contains("data-fx=\"01.03.2024\""),
        "{s}"
    );
    // A percent kind's fractions read as percent.
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W1", "W1"])),
        Column::new("c", Role::Category, strs(&["a", "b"])),
        Column::new("n", Role::Value(Kind::BarPercent), nums(&[30.0, 10.0])),
    ]);
    assert!(s.contains("data-fvalue=\"75%\""), "{s}");
}

// ── a bar + line combo: the line takes the next palette colour, drawn above
// the bars (it used to share the bars' brand colour and vanish into them) ──
#[test]
fn bar_line_combo_line_is_visible() {
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W1", "W2", "W3"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[30.0, 41.0, 26.0])),
        Column::new("r", Role::Value(Kind::Line), nums(&[24.0, 32.0, 20.0])),
    ]);
    let lines = polylines(&s);
    assert_eq!(lines.len(), 1, "{s}");
    assert!(
        lines[0].contains(&slot(1)),
        "line in palette slot 1: {}",
        lines[0]
    );
    assert!(
        !lines[0].contains(&slot(0)),
        "not the bars' colour: {}",
        lines[0]
    );
    let last_bar = s.rfind("<rect ").unwrap();
    assert!(
        s.rfind("<polyline ").unwrap() > last_bar,
        "line drawn above the bars"
    );
}

// ── a ::YFORMAT starting with a space is a suffix (`' kg'` → "12 kg"), on the
// axis and in tooltips — the spec must reach the formatter untrimmed ──
#[test]
fn yformat_leading_space_is_a_suffix() {
    let s = svg(vec![
        Column::new("w", Role::X, strs(&["W1", "W2"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[12.0, 30.0])),
        Column::new("f", Role::YFormat, strs(&[" kg", " kg"])),
    ]);
    assert!(s.contains("12 kg"), "suffix in the tooltip value: {s}");
    assert!(!s.contains(">kg"), "no unit before the number: {s}");
}

// One model's coefficients: above the null value in the brand blue, below it
// in the theme's red (the sign at a glance); several models keep series colours.
#[test]
fn contract_terms_colour_by_sign() {
    let th = anofox_visualization::theme::default_light();
    let hex = |(r, g, b): (u8, u8, u8)| format!("#{r:02X}{g:02X}{b:02X}");
    let (blue, red) = (slot(0), hex(th.colors.negative));
    let s = spec(
        "terms",
        r#"[{"term":"up","estimate":0.5,"conf_low":0.2,"conf_high":0.8},
            {"term":"down","estimate":-0.4,"conf_low":-0.7,"conf_high":-0.1}]"#,
        "{}",
    )
    .unwrap();
    let circles: Vec<&str> = s.split("<circle ").skip(1).collect();
    assert!(
        circles.iter().any(|c| c.contains(&blue)),
        "a blue point: {s}"
    );
    assert!(circles.iter().any(|c| c.contains(&red)), "a red point: {s}");
    // odds ratios: below 1 is red
    let s = spec(
        "terms",
        r#"[{"term":"or","estimate":0.8,"conf_low":0.7,"conf_high":0.9}]"#,
        r#"{"reference":1}"#,
    )
    .unwrap();
    assert!(s.split("<circle ").skip(1).any(|c| c.contains(&red)), "{s}");
}

// ── #32: waterfall / bullet / funnel ─────────────────────────────────────
/// Every start tag carrying `attr="v"`.
fn tags_with<'a>(svg: &'a str, attr: &str, v: &str) -> Vec<&'a str> {
    let needle = format!(" {attr}=\"{v}\"");
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find(&needle) {
        let start = rest[..i].rfind('<').unwrap();
        let end = i + rest[i..].find('>').unwrap();
        out.push(&rest[start..end]);
        rest = &rest[end..];
    }
    out
}
fn attr_of<'a>(tag: &'a str, a: &str) -> Option<&'a str> {
    let k = tag.find(&format!(" {a}=\""))? + a.len() + 3;
    Some(&tag[k..k + tag[k..].find('"')?])
}
fn dump(name: &str, s: &str) {
    if let Ok(dir) = std::env::var("ANOFOX_DUMP_SVG") {
        std::fs::write(format!("{dir}/{name}.svg"), s).unwrap();
    }
}

fn waterfall_cols() -> Vec<Column> {
    vec![
        Column::new(
            "step",
            Role::X,
            strs(&["Budget", "Volume", "Price", "Mix", "FX"]),
        ),
        Column::new(
            "eur",
            Role::Value(Kind::Waterfall),
            nums(&[100.0, 25.0, -12.0, 8.0, -6.0]),
        ),
        Column::new("t", Role::Label, strs(&["Budget to actual"])),
    ]
}

#[test]
fn waterfall_bridges_start_to_end() {
    let s = svg(waterfall_cols());
    dump("waterfall", &s);
    // Start total, four steps and the closing End total.
    for x in ["Budget", "Volume", "Price", "Mix", "FX", "End"] {
        let t = tags_with(&s, "data-x", x);
        assert!(t.iter().any(|t| t.starts_with("<rect")), "bar for {x}: {s}");
    }
    let end = tags_with(&s, "data-x", "End");
    let end = end.iter().find(|t| t.starts_with("<rect")).unwrap();
    // Each bar reports its own value: the steps signed, the totals their level.
    assert_eq!(attr_of(end, "data-value"), Some("115"));
    // The series is the step itself: a click selects the step.
    assert_eq!(attr_of(end, "data-series"), Some("End"));
    let price = tags_with(&s, "data-x", "Price");
    let price = price.iter().find(|t| t.starts_with("<rect")).unwrap();
    assert_eq!(attr_of(price, "data-value"), Some("-12"));
    assert_eq!(attr_of(price, "data-series"), Some("Price"));
    // Increases / decreases in the theme's positive / negative colours.
    let th = anofox_visualization::theme::default_light();
    assert_eq!(
        attr_of(price, "fill"),
        Some(hexu(th.colors.negative).as_str())
    );
    let vol = tags_with(&s, "data-x", "Volume");
    let vol = vol.iter().find(|t| t.starts_with("<rect")).unwrap();
    assert_eq!(
        attr_of(vol, "fill"),
        Some(hexu(th.colors.positive).as_str())
    );
    // The Price step floats: it starts at the running level, not at 0, so it
    // is shorter than the Budget bar.
    let budget = tags_with(&s, "data-x", "Budget");
    let budget = budget.iter().find(|t| t.starts_with("<rect")).unwrap();
    let h = |t: &str| attr_of(t, "height").unwrap().parse::<f64>().unwrap();
    assert!(h(price) < h(budget) / 4.0, "{price} vs {budget}");
    // Signed data labels and connectors between the bars.
    assert!(
        s.contains(">+25<") && s.contains(">-12<") && s.contains(">115<"),
        "{s}"
    );
    assert!(s.matches("<line ").count() + s.matches("<polyline ").count() >= 5);
}

#[test]
fn waterfall_null_and_tagged_rows_are_totals() {
    let s = svg(vec![
        Column::new("s", Role::X, strs(&["Q1", "+A", "-B", "Q2", "+C", "Q3"])),
        Column::new(
            "v",
            Role::Value(Kind::Waterfall),
            vec![
                Value::Float(50.0),
                Value::Float(10.0),
                Value::Float(-5.0),
                Value::Na,
                Value::Float(20.0),
                Value::Float(70.0),
            ],
        ),
        Column::new("k", Role::Category, strs(&["", "", "", "", "", "total"])),
    ]);
    let bar = |x: &str| {
        tags_with(&s, "data-x", x)
            .into_iter()
            .find(|t| t.starts_with("<rect"))
            .unwrap()
            .to_string()
    };
    assert_eq!(attr_of(&bar("Q2"), "data-value"), Some("55"));
    let th = anofox_visualization::theme::default_light();
    let brand = hexu(th.color(0));
    assert_eq!(attr_of(&bar("Q2"), "fill"), Some(brand.as_str()));
    assert_eq!(attr_of(&bar("Q3"), "data-value"), Some("70"));
    // The last row is a total: no extra End bar.
    assert!(tags_with(&s, "data-x", "End").is_empty());
}

#[test]
fn bullet_draws_bands_measure_and_goal() {
    let s = svg(vec![
        Column::new("r", Role::X, strs(&["North", "South"])),
        Column::new("v", Role::Value(Kind::Bullet), nums(&[72.0, 91.0])),
        Column::new("g", Role::RefLine, nums(&[80.0, 85.0])),
        Column::new("b", Role::Range, strs(&["50,75,100"])),
    ]);
    dump("bullet", &s);
    let north: Vec<&str> = tags_with(&s, "data-x", "North")
        .into_iter()
        .filter(|t| t.starts_with("<rect"))
        .collect();
    // Three bands, the measure and the goal tick.
    assert_eq!(north.len(), 5, "{north:#?}");
    // Every mark of the row selects the row; only the measure has a value.
    assert!(north
        .iter()
        .all(|t| attr_of(t, "data-series") == Some("North")));
    let valued: Vec<_> = north
        .iter()
        .filter_map(|t| attr_of(t, "data-value"))
        .collect();
    assert_eq!(valued, ["72"]);
    // The goal tick and the bands say what they are in the tooltip.
    assert!(s.contains("<title>Goal 80</title>"), "{s}");
    assert!(s.contains("<title>Range 50 – 75</title>"), "{s}");
    // Horizontal: the measure bar is wider than tall, and North (first row)
    // sits above South.
    let m = north
        .iter()
        .find(|t| attr_of(t, "data-value") == Some("72"))
        .unwrap();
    let f = |t: &str, a: &str| attr_of(t, a).unwrap().parse::<f64>().unwrap();
    assert!(f(m, "width") > f(m, "height"), "{m}");
    let south = tags_with(&s, "data-x", "South")
        .into_iter()
        .find(|t| t.starts_with("<rect") && attr_of(t, "data-value") == Some("91"))
        .unwrap();
    assert!(f(m, "y") < f(south, "y"));
}

#[test]
fn funnel_widths_by_value_with_conversion() {
    let s = svg(vec![
        Column::new(
            "st",
            Role::X,
            strs(&["Visits", "Signups", "Trials", "Paid"]),
        ),
        Column::new(
            "n",
            Role::Value(Kind::Funnel),
            nums(&[10000.0, 2500.0, 1000.0, 250.0]),
        ),
    ]);
    dump("funnel", &s);
    let bar = |x: &str| {
        tags_with(&s, "data-x", x)
            .into_iter()
            .find(|t| t.starts_with("<rect"))
            .unwrap()
            .to_string()
    };
    let f = |t: &str, a: &str| attr_of(t, a).unwrap().parse::<f64>().unwrap();
    let (v, p) = (bar("Visits"), bar("Paid"));
    assert_eq!(attr_of(&v, "data-value"), Some("10000"));
    // Widths by value, centred on one axis, first stage on top.
    assert!((f(&v, "width") / f(&p, "width") - 40.0).abs() < 1.0);
    let mid = |t: &str| f(t, "x") + f(t, "width") / 2.0;
    assert!((mid(&v) - mid(&p)).abs() < 0.5);
    assert!(f(&v, "y") < f(&p, "y"));
    // Conversion from the previous stage.
    assert!(s.contains("25%") && s.contains("40%"), "{s}");
}

#[test]
fn bullet_bands_stay_quiet_in_dark_mode() {
    let cols = vec![
        Column::new("r", Role::X, strs(&["North"])),
        Column::new("v", Role::Value(Kind::Bullet), nums(&[72.0])),
        Column::new("b", Role::Range, strs(&["50,75,100"])),
    ];
    let lum = |hex: &str| {
        let p = |i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64;
        (0.299 * p(1) + 0.587 * p(3) + 0.114 * p(5)) / 255.0
    };
    for dark in [false, true] {
        let s = render_in(&cols, "default", dark);
        let bands: Vec<f64> = tags_with(&s, "data-x", "North")
            .into_iter()
            .filter(|t| t.contains("<title>Range") || attr_of(t, "data-value").is_none())
            .filter_map(|t| attr_of(t, "fill").map(lum))
            .collect();
        assert_eq!(bands.len(), 3, "{s}");
        // Light: pale greys; dark: close to the dark card, never light.
        for l in &bands {
            assert!(
                if dark { *l < 0.4 } else { *l > 0.7 },
                "dark={dark} {bands:?}"
            );
        }
    }
}

// ── #208: stacked marks under ::FACET ────────────────────────────────────
/// Cost per day by component, faceted by plan (two panels side by side).
fn stacked_facets(kind: Kind, flip: bool) -> Vec<Column> {
    let mut cols = vec![
        Column::new(
            "day",
            Role::X,
            strs(&["D1", "D1", "D2", "D2", "D1", "D1", "D2", "D2"]),
        ),
        Column::new(
            "part",
            Role::Category,
            strs(&["a", "b", "a", "b", "a", "b", "a", "b"]),
        ),
        Column::new(
            "cost",
            Role::Value(kind),
            nums(&[3.0, 5.0, 7.0, 2.0, 4.0, 1.0, 2.0, 6.0]),
        ),
        Column::new(
            "plan",
            Role::Facet,
            strs(&["P", "P", "P", "P", "Q", "Q", "Q", "Q"]),
        ),
        Column::new("n", Role::FacetCols, nums(&[2.0])),
    ];
    if flip {
        cols.push(Column::new("f", Role::Flip, nums(&[1.0])));
    }
    cols
}
/// `(x, y, width, height)` of every data `<rect>` (one per stacked segment).
fn segments(svg: &str) -> Vec<[f64; 4]> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find("<rect ") {
        let tag = &rest[i..i + rest[i..].find('>').unwrap()];
        if tag.contains("data-series=\"") {
            let attr = |a: &str| -> f64 {
                let k = tag.find(&format!(" {a}=\"")).unwrap() + a.len() + 3;
                tag[k..k + tag[k..].find('"').unwrap()].parse().unwrap()
            };
            out.push([attr("x"), attr("y"), attr("width"), attr("height")]);
        }
        rest = &rest[i + 5..];
    }
    out
}

#[test]
fn stacked_bars_start_at_zero_in_every_facet_panel() {
    for kind in [Kind::BarStacked, Kind::BarStackedPercent] {
        let s = svg(stacked_facets(kind, false));
        let segs = segments(&s);
        assert_eq!(segs.len(), 8, "{kind:?}: one rect per segment");
        // Bars (one per panel × day) keyed by their left edge; each bar's
        // lowest segment sits on the shared baseline.
        let baseline = segs.iter().map(|r| r[1] + r[3]).fold(f64::MIN, f64::max);
        let mut lefts: Vec<f64> = segs.iter().map(|r| r[0]).collect();
        lefts.sort_by(f64::total_cmp);
        lefts.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert_eq!(lefts.len(), 4, "{kind:?}: 2 panels × 2 days");
        for l in lefts {
            let bottom = segs
                .iter()
                .filter(|r| (r[0] - l).abs() < 0.01)
                .map(|r| r[1] + r[3])
                .fold(f64::MIN, f64::max);
            assert!(
                (bottom - baseline).abs() < 0.05,
                "{kind:?}: bar at x={l} floats ({bottom} vs {baseline})"
            );
        }
    }
    // Plain stacks: the y axis ends near the largest panel total (9), not at
    // the pooled one (8 + 5 = 13).
    let s = svg(stacked_facets(Kind::BarStacked, false));
    assert!(
        !s.contains(">12</text>") && !s.contains(">16</text>"),
        "pooled y range"
    );
}

#[test]
fn horizontal_stacked_bars_start_at_zero_in_every_facet_panel() {
    let mut segs = segments(&svg(stacked_facets(Kind::BarStacked, true)));
    assert_eq!(segs.len(), 8);
    // ::FLIP: a bar is a row of touching segments; its left end is 0, i.e.
    // its panel's left edge — so there are exactly 2 distinct left ends.
    segs.sort_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])));
    let mut starts: Vec<f64> = Vec::new();
    for (k, r) in segs.iter().enumerate() {
        let joined = k > 0 && {
            let p = segs[k - 1];
            (p[1] - r[1]).abs() < 0.05 && (p[0] + p[2] - r[0]).abs() < 0.05
        };
        if !joined {
            starts.push(r[0]);
        }
    }
    assert_eq!(starts.len(), 4, "2 panels × 2 days: {starts:?}");
    starts.sort_by(f64::total_cmp);
    starts.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    assert_eq!(
        starts.len(),
        2,
        "every bar starts at its panel's 0: {starts:?}"
    );
}

#[test]
fn stacked_areas_start_at_zero_in_every_facet_panel() {
    let s = svg(stacked_facets(Kind::AreaStacked, false));
    // Each band is a polygon; the lowest band of each panel closes on the
    // shared baseline (the largest y of all polygon points).
    let polys: Vec<Vec<(f64, f64)>> = s
        .match_indices("<polygon points=\"")
        .map(|(i, m)| {
            let rest = &s[i + m.len()..];
            rest[..rest.find('"').unwrap()]
                .split_whitespace()
                .map(|p| {
                    let (x, y) = p.split_once(',').unwrap();
                    (x.parse().unwrap(), y.parse().unwrap())
                })
                .collect()
        })
        .collect();
    assert_eq!(polys.len(), 4, "2 panels × 2 bands");
    let baseline = polys.iter().flatten().map(|p| p.1).fold(f64::MIN, f64::max);
    let on_baseline = polys
        .iter()
        .filter(|ps| ps.iter().filter(|p| (p.1 - baseline).abs() < 0.05).count() >= 2)
        .count();
    assert_eq!(on_baseline, 2, "one band per panel sits on 0");
}

#[test]
fn event_labels_over_stacked_facets_sit_above_the_panel_totals() {
    // An ::XLINE_LABEL sits just above the tallest stack of any one panel
    // (9), not above the pooled stacks of all panels (13).
    let mut cols = stacked_facets(Kind::BarStacked, false);
    cols[0] = Column::new(
        "day",
        Role::X,
        nums(&[1.0, 1.0, 2.0, 2.0, 1.0, 1.0, 2.0, 2.0]),
    );
    cols.push(Column::new("e", Role::VLine, nums(&[1.5; 8])));
    cols.push(Column::new("el", Role::VLineLabel, strs(&["go-live"; 8])));
    let s = svg(cols);
    assert!(s.contains("go-live"), "label drawn");
    let ticks: Vec<f64> = s
        .match_indices("</text>")
        .filter_map(|(i, _)| s[..i].rsplit('>').next()?.parse().ok())
        .collect();
    let top = ticks.iter().copied().fold(f64::MIN, f64::max);
    assert!(
        top < 12.0,
        "y axis stretched to the pooled stacks: {ticks:?}"
    );
}

// ── #218 / #219: values and data labels on stacked marks ─────────────────
/// `(data-value, data-fvalue)` of every data `<rect>`, in drawing order.
fn hover_values(svg: &str) -> Vec<(f64, String)> {
    svg.split("<rect ")
        .skip(1)
        .map(|t| &t[..t.find('>').unwrap()])
        .filter(|t| t.contains("data-value=\""))
        .map(|t| {
            let attr = |a: &str| {
                let k = t.find(&format!(" {a}=\"")).unwrap() + a.len() + 3;
                t[k..k + t[k..].find('"').unwrap()].to_string()
            };
            (attr("data-value").parse().unwrap(), attr("data-fvalue"))
        })
        .collect()
}
/// `(x, y, text)` of every data label (drawn at the ::DATALABELS size, 11).
fn data_labels(svg: &str) -> Vec<(f64, f64, String)> {
    svg.split("<text")
        .skip(1)
        .filter(|t| t.contains("font-size=\"11.00\""))
        .map(|t| {
            let attr = |a: &str| -> f64 {
                let k = t.find(&format!(" {a}=\"")).unwrap() + a.len() + 3;
                t[k..k + t[k..].find('"').unwrap()].parse().unwrap()
            };
            let text = t[t.find('>').unwrap() + 1..t.find("</text>").unwrap()].to_string();
            (attr("x"), attr("y"), text)
        })
        .collect()
}
fn with_labels(mut cols: Vec<Column>) -> Vec<Column> {
    cols.push(Column::new("l", Role::DataLabels, nums(&[11.0])));
    cols
}

#[test]
fn percent_stacked_hover_shows_each_segments_share() {
    // Shares per panel and day: P = 3/8, 5/8, 7/9, 2/9; Q = 4/5, 1/5, 2/8, 6/8.
    let want = [0.375, 0.625, 7.0 / 9.0, 2.0 / 9.0, 0.8, 0.2, 0.25, 0.75];
    for flip in [false, true] {
        let got = hover_values(&svg(stacked_facets(Kind::BarStackedPercent, flip)));
        assert_eq!(got.len(), 8, "flip={flip}");
        for ((v, fv), w) in got.iter().zip(want) {
            assert!((v - w).abs() < 1e-9, "flip={flip}: data-value {v} vs {w}");
            assert!(fv.ends_with('%'), "flip={flip}: {fv}");
            let pct: f64 = fv.trim_end_matches('%').parse().unwrap();
            assert!((pct - w * 100.0).abs() < 0.51, "flip={flip}: {fv} vs {w}");
        }
    }
    // Values that are fractions already are still divided by their stack.
    let s = svg(vec![
        Column::new("d", Role::X, strs(&["D1", "D1"])),
        Column::new("c", Role::Category, strs(&["a", "b"])),
        Column::new("v", Role::Value(Kind::BarStackedPercent), nums(&[0.3, 0.2])),
    ]);
    let got: Vec<String> = hover_values(&s).into_iter().map(|(_, f)| f).collect();
    assert_eq!(got, ["60%", "40%"]);
}

#[test]
fn stacked_data_labels_sit_in_the_middle_of_their_segment() {
    for kind in [Kind::BarStacked, Kind::BarStackedPercent] {
        for flip in [false, true] {
            let s = svg(with_labels(stacked_facets(kind, flip)));
            let (segs, labels) = (segments(&s), data_labels(&s));
            assert_eq!(labels.len(), 8, "{kind:?} flip={flip}: {labels:?}");
            for r in &segs {
                let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
                assert!(
                    labels
                        .iter()
                        .any(|l| (l.0 - cx).abs() < 0.6 && (l.1 - cy).abs() < 0.6),
                    "{kind:?} flip={flip}: no label at the middle of {r:?}: {labels:?}"
                );
            }
        }
    }
    // Percent stacks: the labels are the shares, and the y axis stays 0–100 %.
    let s = svg(with_labels(stacked_facets(Kind::BarStackedPercent, false)));
    let texts: Vec<String> = data_labels(&s).into_iter().map(|l| l.2).collect();
    assert_eq!(
        texts,
        ["37.5%", "62.5%", "77.8%", "22.2%", "80%", "20%", "25%", "75%"]
    );
    assert!(!s.contains(">200%</text>"), "y axis trained on raw values");
}

#[test]
fn stacked_area_data_labels_sit_in_the_middle_of_their_band() {
    // The same stacks as bars on the same y scale: an area's labels sit at
    // the heights of the bar segments' middles.
    let mut area: Vec<f64> =
        data_labels(&svg(with_labels(stacked_facets(Kind::AreaStacked, false))))
            .into_iter()
            .map(|l| l.1)
            .collect();
    let mut mids: Vec<f64> = segments(&svg(with_labels(stacked_facets(Kind::BarStacked, false))))
        .into_iter()
        .map(|r| r[1] + r[3] / 2.0)
        .collect();
    area.sort_by(f64::total_cmp);
    mids.sort_by(f64::total_cmp);
    assert_eq!(area.len(), 8);
    for (a, m) in area.iter().zip(&mids) {
        assert!((a - m).abs() < 0.6, "{area:?} vs {mids:?}");
    }
}

// ── #175: a top legend wraps onto rows inside the chart ───────────────────
/// `n` cartons stacked into three containers (the anofox-optimize packing
/// view), rendered at `w`×`h`.
fn cartons(kind: Kind, n: usize, w: u32, h: u32) -> anofox_visualization::Rendered {
    use anofox_visualization::{render_with_warnings, RenderOptions};
    let (mut x, mut c, mut y) = (Vec::new(), Vec::new(), Vec::new());
    for i in 1..=n {
        x.push(Value::Str(format!("container {}", i % 3 + 1)));
        c.push(Value::Str(format!("carton {i:02}")));
        y.push(Value::Float((1 + i % 4) as f64));
    }
    let mut cols = vec![
        Column::new("item", Role::Category, c),
        Column::new("size", Role::Value(kind), y),
    ];
    if kind != Kind::Pie {
        cols.insert(0, Column::new("bin", Role::X, x));
    }
    render_with_warnings(&cols, w, h, &RenderOptions::default()).expect("renders")
}
/// A number attribute of a start tag.
fn num_attr(tag: &str, a: &str) -> Option<f64> {
    let k = tag.find(&format!(" {a}=\""))? + a.len() + 3;
    tag[k..k + tag[k..].find('"')?].parse().ok()
}
/// The panel rect `[x, y, w, h]` (`data-plot`).
fn panel(svg: &str) -> [f64; 4] {
    let k = svg.find("data-plot=\"").unwrap() + 11;
    let v: Vec<f64> = svg[k..k + svg[k..].find('"').unwrap()]
        .split(' ')
        .map(|s| s.parse().unwrap())
        .collect();
    [v[0], v[1], v[2], v[3]]
}
/// The legend labels above the panel as `(x0, x1, y, text)`: from the key's
/// left edge (16 px before the label) to the label's estimated end.
fn top_legend(svg: &str) -> Vec<(f64, f64, f64, String)> {
    let top = panel(svg)[1];
    svg.split("<text ")
        .skip(1)
        .filter_map(|t| {
            let end = t.find('>')?;
            let tag = format!(" {}", &t[..end]);
            let text = t[end + 1..t.find("</text>")?].to_string();
            let (x, y, size) = (
                num_attr(&tag, "x")?,
                num_attr(&tag, "y")?,
                num_attr(&tag, "font-size")?,
            );
            let legend = text.starts_with("carton ") || text.starts_with('+');
            (legend && y < top).then(|| {
                let w = ggplot_rs::render::text::text_width(&text, size);
                (x - 16.0, x + w, y, text)
            })
        })
        .collect()
}

#[test]
fn top_legend_wraps_inside_the_chart() {
    for n in [1, 3, 12, 30] {
        let r = cartons(Kind::BarStacked, n, 640, 400);
        let l = top_legend(&r.svg);
        assert_eq!(l.len(), n, "{n} cartons: every entry shown");
        assert!(r.warnings.is_empty(), "{n} cartons: {:?}", r.warnings);
        for (x0, x1, _, t) in &l {
            assert!(*x0 >= 0.0 && *x1 <= 640.0, "{n} cartons: {t} at {x0}..{x1}");
        }
        for (i, a) in l.iter().enumerate() {
            for b in &l[i + 1..] {
                let apart = (a.2 - b.2).abs() >= 18.0 || a.1 <= b.0 || b.1 <= a.0;
                assert!(apart, "{n} cartons: {} overlaps {}", a.3, b.3);
            }
        }
        let rows = {
            let mut ys: Vec<i64> = l.iter().map(|e| e.2.round() as i64).collect();
            ys.dedup();
            ys.len()
        };
        let [_, top, _, _] = panel(&r.svg);
        let bottom = l.iter().map(|e| e.2).fold(0.0, f64::max) + 6.0;
        assert!(
            bottom < top,
            "{n} cartons: legend ends at {bottom}, panel starts at {top}"
        );
        match n {
            1 | 3 => {
                assert_eq!(rows, 1, "{n} cartons fit on one line");
                assert_eq!(
                    top, 47.5,
                    "{n} cartons: a one-line legend's band is unchanged"
                );
            }
            12 => assert_eq!(rows, 2, "12 cartons: two rows"),
            _ => assert!(rows >= 4, "30 cartons: {rows} rows"),
        }
    }
}

#[test]
fn legend_wraps_on_line_and_pie_charts() {
    for kind in [Kind::Line, Kind::Pie] {
        let r = cartons(kind, 12, 640, 400);
        let l = top_legend(&r.svg);
        assert_eq!(l.len(), 12, "{kind:?}");
        assert!(
            l.iter().all(|e| e.0 >= 0.0 && e.1 <= 640.0),
            "{kind:?}: {l:?}"
        );
        let bottom = l.iter().map(|e| e.2).fold(0.0, f64::max) + 6.0;
        assert!(bottom < panel(&r.svg)[1], "{kind:?}: legend over the panel");
    }
}

#[test]
fn a_legend_too_long_for_the_chart_ends_in_n_more_and_warns() {
    let r = cartons(Kind::BarStacked, 30, 320, 300);
    let l = top_legend(&r.svg);
    let more = l.last().expect("a legend");
    assert!(
        more.3.starts_with('+') && more.3.ends_with(" more"),
        "{more:?}"
    );
    let hidden: usize = more.3[1..more.3.len() - 5].parse().unwrap();
    assert_eq!(l.len() - 1 + hidden, 30, "shown + hidden = all cartons");
    assert!(l.iter().all(|e| e.0 >= 0.0 && e.1 <= 320.0), "{l:?}");
    assert!(
        r.warnings.iter().any(|w| w.starts_with("legend:")),
        "{:?}",
        r.warnings
    );
    assert!(
        r.svg.contains("data-warnings="),
        "the warning reaches the SVG root"
    );
    let bottom = l.iter().map(|e| e.2).fold(0.0, f64::max) + 6.0;
    assert!(bottom < panel(&r.svg)[1], "legend over the panel");
}

#[test]
fn stacked_data_labels_stay_centred_under_a_wrapped_legend() {
    // #218/#219's centred stacked labels and #175's legend wrap touch the
    // same chart from different passes (the label layer is built with the
    // chart; the legend is wrapped on the finished SVG, which redraws the
    // chart with a taller legend band reserved). Check the two combine: with
    // 12 cartons (two legend rows) every stacked label still sits in the
    // middle of its own segment.
    use anofox_visualization::{render_with_warnings, RenderOptions};
    let n = 12;
    let (mut x, mut c, mut y) = (Vec::new(), Vec::new(), Vec::new());
    for i in 1..=n {
        x.push(Value::Str(format!("container {}", i % 3 + 1)));
        c.push(Value::Str(format!("carton {i:02}")));
        y.push(Value::Float((1 + i % 4) as f64));
    }
    let cols = with_labels(vec![
        Column::new("bin", Role::X, x),
        Column::new("item", Role::Category, c),
        Column::new("size", Role::Value(Kind::BarStacked), y),
    ]);
    let r = render_with_warnings(&cols, 640, 400, &RenderOptions::default()).expect("renders");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    let l = top_legend(&r.svg);
    assert_eq!(l.len(), n, "every carton shown in the legend");
    let [_, top, _, _] = panel(&r.svg);
    assert!(
        top > 47.5,
        "12 cartons need a second legend row: panel top is {top}"
    );

    let (segs, labels) = (segments(&r.svg), data_labels(&r.svg));
    assert_eq!(labels.len(), n, "{labels:?}");
    for seg in &segs {
        let (cx, cy) = (seg[0] + seg[2] / 2.0, seg[1] + seg[3] / 2.0);
        assert!(
            labels
                .iter()
                .any(|lbl| (lbl.0 - cx).abs() < 0.6 && (lbl.1 - cy).abs() < 0.6),
            "no label centred on segment {seg:?}: {labels:?}"
        );
    }
}

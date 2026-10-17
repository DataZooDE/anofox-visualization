//! Render regression for `::MAP` panels drawn by the extension's own map
//! writer: every projection, every class method, light and dark (and the
//! colour-blind theme), holes cut out, row order, the per-feature attribute
//! contract the browser keys on, the no-data hatch, the legend and the
//! summary.

use std::sync::Arc;

use anofox_visualization::{
    render_with, render_with_warnings, theme, Column, Kind, RenderOptions, Role,
};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn konst(s: &str, n: usize) -> Vec<Value> {
    vec![Value::Str(s.to_string()); n]
}
fn opts(name: &str, dark: bool) -> RenderOptions {
    let set = theme::resolve(name).unwrap();
    RenderOptions {
        theme: Arc::new(if dark { set.dark } else { set.light }),
        ..RenderOptions::default()
    }
}
fn draw(cols: Vec<Column>, o: &RenderOptions) -> String {
    let svg = render_with(&cols, 480, 320, o).expect("renders");
    roxmltree::Document::parse(&svg).unwrap_or_else(|e| panic!("well-formed SVG: {e}\n{svg}"));
    svg
}
fn svg(cols: Vec<Column>) -> String {
    draw(cols, &RenderOptions::default())
}
/// The value of attribute `name` in the start tag `tag`.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let k = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    Some(&tag[k..k + tag[k..].find('"')?])
}
/// The start tags of the drawn features (elements with a `data-row`), in
/// document order.
fn features(svg: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find(" data-row=\"") {
        let start = rest[..i].rfind('<').unwrap();
        let end = i + rest[i..].find('>').unwrap();
        out.push(&rest[start..=end]);
        rest = &rest[end..];
    }
    out
}
fn root(svg: &str) -> &str {
    &svg[..svg.find('>').unwrap()]
}

/// Four districts of a made-up country near 10°E 50°N, the second with a hole.
fn regions() -> Vec<Value> {
    strs(&[
        "POLYGON((9 49, 10 49, 10 50, 9 50, 9 49))",
        "POLYGON((10 49, 11 49, 11 50, 10 50, 10 49), (10.3 49.3, 10.7 49.3, 10.7 49.7, 10.3 49.7, 10.3 49.3))",
        "MULTIPOLYGON(((9 50, 10 50, 10 51, 9 51, 9 50)), ((9.2 51.2, 9.4 51.2, 9.4 51.4, 9.2 51.2)))",
        "POLYGON((10 50, 11 50, 11 51, 10 51, 10 50))",
    ])
}
fn names() -> Vec<Value> {
    strs(&["Nord", "Ost", "Süd", "West"])
}

#[test]
fn every_projection_draws_and_is_recorded() {
    for (spec, extent) in [
        ("auto", "utm"),
        ("equal-earth", "equal-earth"),
        ("mercator", "mercator"),
        ("albers-europe", "albers-europe"),
        ("conic", "conic"),
        ("utm", "utm"),
        ("none", "none"),
        ("robinson", "utm"), // unknown → 'auto'
    ] {
        let s = svg(vec![
            Column::new("g", Role::Geometry, regions()),
            Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0, 3.0, 4.0])),
            Column::new("p", Role::Projection, konst(spec, 4)),
        ]);
        assert_eq!(attr(root(&s), "data-proj"), Some(extent), "{spec}");
        let f = features(&s);
        assert_eq!(f.len(), 4, "{spec}: {f:?}");
        // every feature lies inside the panel
        for t in &f {
            let d = attr(t, "d").unwrap();
            let (x, y) = d[1..]
                .split('l')
                .next()
                .map(|m| {
                    let m = m.replace('-', " -");
                    let mut p = m.split_whitespace().map(|n| n.parse::<f64>().unwrap());
                    (p.next().unwrap(), p.next().unwrap())
                })
                .unwrap();
            assert!(
                (0.0..=480.0).contains(&x) && (0.0..=320.0).contains(&y),
                "{spec}: {d}"
            );
        }
    }
    // auto by extent: a world map is Equal Earth, a city UTM, a km grid planar
    let world = svg(vec![Column::new(
        "g",
        Role::Geometry,
        strs(&["POLYGON((-170 -50, 170 -50, 170 70, -170 70, -170 -50))"]),
    )]);
    assert_eq!(attr(root(&world), "data-proj"), Some("equal-earth"));
    let city = svg(vec![Column::new(
        "g",
        Role::Geometry,
        strs(&["POINT(13.38 52.51)", "POINT(13.45 52.49)"]),
    )]);
    assert_eq!(attr(root(&city), "data-proj"), Some("utm"));
    let grid = svg(vec![Column::new(
        "g",
        Role::Geometry,
        strs(&["LINESTRING(0 0, -4 5, -10 10, 0 0)", "POINT(-4 5)"]),
    )]);
    assert_eq!(attr(root(&grid), "data-proj"), Some("none"));
}

#[test]
fn planar_coordinates_keep_their_shape() {
    // 'none' keeps x/y as they are: a 2:1 rectangle stays 2:1 on screen
    let s = svg(vec![Column::new(
        "g",
        Role::Geometry,
        strs(&["POLYGON((0 0, 200 0, 200 100, 0 100, 0 0))"]),
    )]);
    assert_eq!(attr(root(&s), "data-proj"), Some("none"));
    let d = attr(features(&s)[0], "d").unwrap().to_string();
    let steps: Vec<f64> = d[d.find('l').unwrap() + 1..d.len() - 1]
        .replace('-', " -")
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    let (w, h) = (steps[0].abs(), steps[3].abs());
    assert!((w / h - 2.0).abs() < 0.02, "{d}");
}

#[test]
fn holes_are_cut_out() {
    let s = svg(vec![
        Column::new("g", Role::Geometry, regions()),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0, 3.0, 4.0])),
    ]);
    let f = features(&s);
    let ost = f[1];
    assert!(s.contains("class=\"dp-features\" fill-rule=\"evenodd\""));
    // one path, two rings (exterior + hole): two moves, two closes
    let d = attr(ost, "d").unwrap();
    assert_eq!(d.matches('M').count(), 2, "{d}");
    assert_eq!(d.matches('z').count(), 2, "{d}");
    // a multipolygon is one feature with both parts
    assert_eq!(attr(f[2], "d").unwrap().matches('M').count(), 2);
}

#[test]
fn features_follow_row_order_and_carry_the_contract() {
    let wkt = strs(&[
        "POINT(3 3)",
        "LINESTRING(0 0, 2 1, 3 3)",
        "POLYGON((0 0,1 0,1 1,0 1,0 0))",
        "POINT(2 1)",
    ]);
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new(
            "c",
            Role::Category,
            strs(&["Van 2", "Van 1", "late \"stop\"", "Van 1"]),
        ),
        Column::new(
            "l",
            Role::Label,
            vec![
                Value::Na,
                Value::Str("Van 1: 12 km".into()),
                Value::Str("late".into()),
                Value::Str("Bakery & Co\nstop #1".into()),
            ],
        ),
    ]);
    let r = root(&s);
    assert_eq!(attr(r, "data-map"), Some(""));
    let f = features(&s);
    let rows: Vec<&str> = f.iter().map(|t| attr(t, "data-row").unwrap()).collect();
    assert_eq!(rows, ["0", "1", "2", "3"], "row order, not legend order");
    assert!(f[0].starts_with("<circle ") && f[0].contains(" data-row=\"0\" data-series=\"Van 2\""));
    assert!(f[1].starts_with("<path ") && attr(f[1], "fill") == Some("none"));
    assert!(f[1].contains(" data-row=\"1\" data-series=\"Van 1\" data-label=\"Van 1: 12 km\""));
    assert!(f[2].contains(" data-series=\"late &quot;stop&quot;\""));
    assert!(f[3].contains(" data-label=\"Bakery &amp; Co&#10;stop #1\""));
    // titles: "label: series", or the series alone without a label
    assert!(s.contains("data-series=\"Van 2\"><title>Van 2</title>"));
    assert!(s.contains("<title>Bakery &amp; Co\nstop #1: Van 1</title>"));
    // the same series, the same palette colour (levels sorted)
    assert_eq!(attr(f[1], "stroke"), attr(f[3], "fill"));

    // a choropleth: the label is the series, the raw measure the value
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.5, 3.0, 4.0])),
        Column::new("l", Role::Label, strs(&["A", "B", "C", "D"])),
    ]);
    let f = features(&s);
    assert!(
        f[1].contains(" data-row=\"1\" data-series=\"B\" data-value=\"2.5\" data-label=\"B\""),
        "{}",
        f[1]
    );
    assert!(s.contains("<title>B: 2.5</title>"));
    // the key is the label (no data-x); a feature without one is keyed by row
    assert_eq!(attr(f[1], "data-x"), None);

    // unfilled and unlabelled: rows only, no tooltip
    let s = svg(vec![Column::new("g", Role::Geometry, wkt)]);
    let f = features(&s);
    assert_eq!(f.len(), 4);
    assert!(f.iter().all(|t| t.ends_with("/>")), "{f:?}");
    assert!(!s.contains("<title>"));
}

#[test]
fn every_class_method_gives_a_stepped_legend() {
    let wkt: Vec<Value> = (0..10)
        .map(|i| Value::Str(format!("POINT({} {})", 10.0 + i as f64 * 0.3, 50.0)))
        .collect();
    let v = nums(&[1.0, 2.0, 2.5, 3.0, 20.0, 21.0, 22.0, 50.0, 51.0, 52.5]);
    for (spec, k, counts) in [
        ("quantile 5", 5, vec![2, 2, 2, 2, 2]),
        ("jenks 3", 3, vec![4, 3, 3]),
        ("equal 2", 2, vec![7, 3]),
        ("0,10,50,100", 3, vec![4, 3, 3]),
    ] {
        let s = svg(vec![
            Column::new("g", Role::Geometry, wkt.clone()),
            Column::new("v", Role::Value(Kind::Bar), v.clone()),
            Column::new("c", Role::Classes, konst(spec, 10)),
        ]);
        let classes: Vec<String> = serde_json::from_str(
            &attr(root(&s), "data-classes")
                .unwrap()
                .replace("&quot;", "\""),
        )
        .unwrap();
        assert_eq!(classes.len(), k, "{spec}: {classes:?}");
        // each feature names its class; the classes' colours step
        let f = features(&s);
        let mut seen = vec![0usize; k];
        let mut colors = vec![String::new(); k];
        for t in &f {
            let c: usize = attr(t, "data-class").unwrap().parse().unwrap();
            seen[c - 1] += 1;
            colors[c - 1] = attr(t, "fill").unwrap().to_string();
        }
        assert_eq!(seen, counts, "{spec}");
        let mut distinct = colors.clone();
        distinct.dedup();
        assert_eq!(distinct.len(), k, "{spec}: {colors:?}");
        // the legend writes each class's count
        let legend = &s[s.find("class=\"dp-legend\"").unwrap()..];
        for n in &counts {
            assert!(legend.contains(&format!(">{n}</text>")), "{spec}");
        }
        assert!(
            s.contains(&format!("of {k}: ")),
            "{spec}: the tooltip names the class"
        );
    }
    // a bad spec: a continuous ramp (and the lint says why)
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt),
        Column::new("v", Role::Value(Kind::Bar), v),
        Column::new("c", Role::Classes, konst("kmeans 4", 10)),
    ]);
    assert!(attr(root(&s), "data-classes").is_none() && s.contains("<linearGradient"));
}

#[test]
fn custom_edges_and_range_open_the_end_classes() {
    let wkt: Vec<Value> = (0..4)
        .map(|i| Value::Str(format!("POINT({i} 0.5)")))
        .collect();
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new("v", Role::Value(Kind::Bar), nums(&[-5.0, 3.0, 20.0, 150.0])),
        Column::new("c", Role::Classes, konst("0,10,50,100", 4)),
    ]);
    let classes = attr(root(&s), "data-classes").unwrap().to_string();
    assert!(
        classes.contains("&lt; 10") && classes.contains("≥ 50"),
        "{classes}"
    );
    assert!(s.contains(">≤0</text>") && s.contains(">≥100</text>"));
    // ::RANGE caps a continuous ramp: the ends read ≤lo / ≥hi
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 3.0, 20.0, 150.0])),
        Column::new("r", Role::Range, konst("2,100", 4)),
    ]);
    assert!(
        s.contains(">≤2</text>") && s.contains(">≥100</text>"),
        "{s}"
    );
    let f = features(&s);
    assert_ne!(
        attr(f[0], "fill"),
        attr(f[1], "fill"),
        "lo and inside differ"
    );
}

#[test]
fn diverging_around_zero_or_the_midpoint() {
    let t = theme::default_light();
    let [lo, mid, hi] = t.diverging_ramp();
    let hex = |c: (u8, u8, u8)| theme::hex(c);
    let wkt: Vec<Value> = (0..3)
        .map(|i| Value::Str(format!("POINT({i} 0.5)")))
        .collect();
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt.clone()),
        Column::new("v", Role::Value(Kind::Bar), nums(&[-10.0, 0.0, 10.0])),
    ]);
    let f = features(&s);
    let fills: Vec<&str> = f.iter().map(|t| attr(t, "fill").unwrap()).collect();
    assert_eq!(
        fills,
        [hex(lo).as_str(), hex(mid).as_str(), hex(hi).as_str()]
    );
    // ::MIDPOINT 100: 100 is the neutral centre
    let s = svg(vec![
        Column::new("g", Role::Geometry, wkt),
        Column::new("v", Role::Value(Kind::Bar), nums(&[50.0, 100.0, 150.0])),
        Column::new("m", Role::Midpoint, nums(&[100.0, 100.0, 100.0])),
    ]);
    assert_eq!(attr(features(&s)[1], "fill"), Some(hex(mid).as_str()));
}

#[test]
fn rows_without_a_value_are_hatched() {
    let s = svg(vec![
        Column::new("g", Role::Geometry, regions()),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            vec![
                Value::Float(1.0),
                Value::Na,
                Value::Float(3.0),
                Value::Float(4.0),
            ],
        ),
        Column::new("l", Role::Label, names()),
    ]);
    let f = features(&s);
    assert!(
        attr(f[1], "fill").unwrap().starts_with("url(#m"),
        "{}",
        f[1]
    );
    assert!(attr(f[1], "data-value").is_none());
    assert!(s.contains("<title>Ost: no data</title>"));
    assert!(s.contains("No data (1)") && s.contains("<pattern "));
    let summary = attr(root(&s), "data-summary").unwrap();
    assert!(summary.contains("1 without data"), "{summary}");
}

#[test]
fn backdrop_rows_are_not_missing_data() {
    // quakes in MAP rows, countries in BASEMAP rows (NULL measure there)
    let s = svg(vec![
        Column::new(
            "g",
            Role::Geometry,
            vec![Value::Str("POINT(10 50)".into()), Value::Na],
        ),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            vec![Value::Float(3.0), Value::Na],
        ),
        Column::new(
            "b",
            Role::Basemap,
            vec![
                Value::Na,
                Value::Str("POLYGON((8 48, 12 48, 12 52, 8 52, 8 48))".into()),
            ],
        ),
    ]);
    assert!(!s.contains("No data") && !s.contains("<pattern"), "{s}");
    assert_eq!(features(&s).len(), 1);
    assert!(s.contains("class=\"dp-basemap\""));
}

#[test]
fn summary_names_the_extremes_and_the_classes() {
    let s = svg(vec![
        Column::new("g", Role::Geometry, regions()),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            nums(&[12.0, 980.0, 40.0, 300.0]),
        ),
        Column::new("l", Role::Label, names()),
        Column::new("c", Role::Classes, konst("quantile 2", 4)),
    ]);
    let summary = attr(root(&s), "data-summary").unwrap();
    assert_eq!(
        summary,
        "4 regions (UTM). Highest: Ost 980; lowest: Nord 12. 2 quantile classes: 2 / 2."
    );
    assert!(s.contains(&format!("<desc>{summary}</desc>")));
}

#[test]
fn themes_paint_the_map_with_their_own_tokens() {
    let cols = || {
        vec![
            Column::new("g", Role::Geometry, regions()),
            Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0, 3.0, 4.0])),
            Column::new(
                "b",
                Role::Basemap,
                strs(&["POLYGON((8 48, 12 48, 12 52, 8 52, 8 48))", "", "", ""]),
            ),
            Column::new("c", Role::Classes, konst("equal 4", 4)),
        ]
    };
    for (name, dark) in [
        ("light", false),
        ("light", true),
        ("colorblind", false),
        ("colorblind", true),
    ] {
        let o = opts(name, dark);
        let s = draw(cols(), &o);
        let t = &o.theme;
        // the backdrop is a pale lift of the card, in both variants
        let land = theme::hex(theme::mix(t.colors.muted, t.colors.card, 0.92));
        assert!(
            s.contains(&format!("class=\"dp-basemap\" fill=\"{land}\"")),
            "{name} {dark}"
        );
        // the legend sits on the card and writes in the chart label ink
        assert!(s.contains(&format!(
            "fill=\"{}\" fill-opacity=\".88\"",
            theme::hex(t.colors.card)
        )));
        // no remap: a feature's fill is the ramp colour of this variant
        let f = features(&s);
        let top = attr(f[3], "fill").unwrap();
        assert!(top.starts_with('#') && top.len() == 7);
    }
    let light = draw(cols(), &opts("light", false));
    let dark = draw(cols(), &opts("light", true));
    assert_ne!(
        attr(features(&light)[0], "fill"),
        attr(features(&dark)[0], "fill"),
        "the pale end of the ramp follows the variant"
    );
}

#[test]
fn alpha_and_a_zoom_window() {
    let base = vec![
        Column::new("g", Role::Geometry, regions()),
        Column::new("a", Role::Alpha, nums(&[0.5, 0.5, 0.5, 0.5])),
        Column::new("p", Role::Projection, konst("none", 4)),
    ];
    let s = svg(base.clone());
    assert!(features(&s)
        .iter()
        .all(|t| attr(t, "fill-opacity") == Some("0.50")));
    // a zoom window (projected units) shows only part of the map, bigger
    let o = RenderOptions {
        zoom: Some(((9.0, 10.0), (49.0, 50.0))),
        ..RenderOptions::default()
    };
    let z = draw(base, &o);
    let f = features(&z);
    let d = attr(f[0], "d").unwrap();
    assert!(d.starts_with("M"), "{d}");
    // Nord (9..10 × 49..50) now spans the panel's height
    let h: f64 = d[d.find('l').unwrap() + 1..d.len() - 1]
        .replace('-', " -")
        .split_whitespace()
        .map(|n| n.parse::<f64>().unwrap())
        .collect::<Vec<_>>()[3]
        .abs();
    assert!((h - 320.0).abs() < 1.0, "{d}");
}

#[test]
fn simplification_keeps_the_svg_small() {
    // a ring with 20,000 vertices on a circle draws with far fewer
    let ring: Vec<String> = (0..=20_000)
        .map(|i| {
            let a = i as f64 / 20_000.0 * std::f64::consts::TAU;
            format!("{} {}", 10.0 + a.cos(), 50.0 + a.sin() * 0.6)
        })
        .collect();
    let wkt = format!("POLYGON(({}))", ring.join(", "));
    let s = svg(vec![Column::new("g", Role::Geometry, strs(&[&wkt]))]);
    let d = attr(features(&s)[0], "d").unwrap();
    assert!(d.len() < 4_000, "{} bytes of path data", d.len());
    assert!(s.len() < 8_000);
}

#[test]
fn many_marks_warn() {
    let wkt: Vec<Value> = (0..100_001)
        .map(|i| {
            Value::Str(format!(
                "POINT({} {})",
                (i % 400) as f64 * 0.01,
                (i / 400) as f64 * 0.01
            ))
        })
        .collect();
    let r = render_with_warnings(
        &[Column::new("g", Role::Geometry, wkt)],
        480,
        320,
        &RenderOptions::default(),
    )
    .unwrap();
    assert!(
        r.warnings.iter().any(|w| w.contains("100001 marks")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn no_geometry_is_no_data() {
    let s = svg(vec![Column::new(
        "g",
        Role::Geometry,
        vec![Value::Na, Value::Na],
    )]);
    assert!(s.contains("No data") && !s.contains("data-map"));
}

/// The districts budget (plan-maps §7): a 400-district map renders in under
/// 50 ms natively (release) into at most 200 KB of SVG at 1200 px. Needs a
/// `key,k2,k3,name,wkt` CSV of the districts in `ANOFOX_MAP_PERF_CSV`:
/// `ANOFOX_MAP_PERF_CSV=de-districts.csv cargo test --release --test maps -- --ignored`.
#[test]
#[ignore]
fn districts_render_within_budget() {
    let Ok(path) = std::env::var("ANOFOX_MAP_PERF_CSV") else {
        return;
    };
    let text = std::fs::read_to_string(path).unwrap();
    let (mut wkt, mut name, mut v) = (Vec::new(), Vec::new(), Vec::new());
    for (i, line) in text.lines().skip(1).enumerate() {
        let q = line.find('"').unwrap();
        let head: Vec<&str> = line[..q].split(',').collect();
        wkt.push(Value::Str(line[q + 1..line.len() - 1].to_string()));
        name.push(Value::Str(head[3].to_string()));
        v.push(Value::Float(((i * 7919) % 1000) as f64));
    }
    let cols = vec![
        Column::new("g", Role::Geometry, wkt),
        Column::new("l", Role::Label, name),
        Column::new("v", Role::Value(Kind::Bar), v),
        Column::new("c", Role::Classes, konst("quantile 5", 1)),
    ];
    let o = RenderOptions::default();
    let _ = render_with(&cols, 1200, 500, &o).unwrap();
    let t = std::time::Instant::now();
    let runs = 10;
    let mut s = String::new();
    for _ in 0..runs {
        s = render_with(&cols, 1200, 500, &o).unwrap();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    eprintln!(
        "districts: {} features, {} bytes, {ms:.1} ms",
        features(&s).len(),
        s.len()
    );
    if let Ok(out) = std::env::var("ANOFOX_MAP_PERF_OUT") {
        std::fs::write(out, &s).unwrap();
    }
    assert!(s.len() <= 200_000, "{} bytes", s.len());
    if !cfg!(debug_assertions) {
        assert!(ms < 50.0, "{ms:.1} ms");
    }
}

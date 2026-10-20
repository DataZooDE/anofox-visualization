use anofox_visualization::{parse_role, render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}

#[test]
fn parses_roles() {
    assert_eq!(parse_role("XAXIS"), Some(Role::X));
    assert_eq!(parse_role("category"), Some(Role::Category));
    assert_eq!(
        parse_role("BARCHART_STACKED"),
        Some(Role::Value(Kind::BarStacked))
    );
    assert_eq!(parse_role("linechart"), Some(Role::Value(Kind::Line)));
    assert_eq!(parse_role("nope"), None);
}

#[test]
fn renders_stacked_bar_by_category() {
    // SELECT week::XAXIS, category::CATEGORY, count()::BARCHART_STACKED
    let cols = vec![
        Column::new("week", Role::X, strs(&["W1", "W1", "W2", "W2"])),
        Column::new("category", Role::Category, strs(&["a", "b", "a", "b"])),
        Column::new(
            "n",
            Role::Value(Kind::BarStacked),
            nums(&[3.0, 5.0, 7.0, 2.0]),
        ),
        Column::new("t", Role::Label, strs(&["Sessions per Week"])),
    ];
    let svg = render(&cols, 480, 320).unwrap();
    assert!(
        svg.contains("<svg") && svg.matches("<rect").count() >= 4,
        "stacked bars drawn"
    );
    assert!(svg.contains("Sessions per Week"), "title from LABEL");
}

#[test]
fn renders_line_and_heading() {
    let line = vec![
        Column::new("t", Role::X, nums(&[0.0, 1.0, 2.0, 3.0])),
        Column::new("y", Role::Value(Kind::Line), nums(&[1.0, 3.0, 2.0, 5.0])),
    ];
    assert!(render(&line, 400, 260).unwrap().contains("<polyline"));

    let heading = vec![Column::new("t", Role::Label, strs(&["Overview"]))];
    assert!(render(&heading, 400, 40).unwrap().contains("Overview"));
}

/// Single-series marks (incl. geoms with explicit styles: thin line, small
/// points, step, smooth) take the brand colour; categories use the DataZoo
/// palette in sorted level order, and `#rrggbb` levels use that colour.
#[test]
fn brand_and_palette_colours() {
    use anofox_visualization::{render_with, RenderOptions};
    let x = || Column::new("x", Role::X, nums(&[0.0, 1.0, 2.0, 3.0]));
    let o = RenderOptions {
        brand: Some((0x12, 0x34, 0x56)),
        ..RenderOptions::default()
    };
    for kind in [
        Kind::Line,
        Kind::Step,
        Kind::Smooth,
        Kind::Area,
        Kind::Point,
    ] {
        let cols = vec![
            x(),
            Column::new("y", Role::Value(kind), nums(&[1.0, 3.0, 2.0, 5.0])),
        ];
        let svg = render_with(&cols, 400, 260, &o).unwrap();
        assert!(svg.contains("#123456"), "{kind:?}: brand colour used");
        assert!(
            !svg.contains("#000000\" fill-opacity"),
            "{kind:?}: no black marks"
        );
    }
    // Sorted levels: "a" → palette 0, "b" → palette 1,
    // whatever the data order; a hex level is its own colour.
    let cols = vec![
        Column::new("x", Role::X, strs(&["W1", "W1", "W1"])),
        Column::new("c", Role::Category, strs(&["b", "a", "#3a7d5c"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[1.0, 2.0, 3.0])),
    ];
    let svg = render(&cols, 400, 260).unwrap();
    let fill_of = |series: &str| {
        let i = svg.find(&format!("data-series=\"{series}\"")).unwrap();
        let tag = &svg[svg[..i].rfind('<').unwrap()..i];
        tag[tag.find("fill=\"").unwrap() + 6..][..7].to_string()
    };
    // (a hex level outside the palette: palette colours are remapped by slot)
    assert_eq!(fill_of("#3a7d5c").to_lowercase(), "#3a7d5c");
    // Sorted: "#3a7d5c" < "a" < "b" → a = palette 1, b = palette 2.
    let slot = |i: usize| {
        let (r, g, b) = anofox_visualization::theme::DEFAULT_PALETTE[i];
        format!("#{r:02x}{g:02x}{b:02x}")
    };
    assert_eq!(fill_of("a").to_lowercase(), slot(1));
    assert_eq!(fill_of("b").to_lowercase(), slot(2));
}

/// #292: several measure columns in one SELECT (no ::CATEGORY) — a
/// multi-series line/bar chart written "wide" — legend each series by its own
/// column name, not a positional `c1`, `c2`, … placeholder.
#[test]
fn combo_measures_legend_by_their_own_column_name() {
    let x = || Column::new("day", Role::X, nums(&[1.0, 2.0, 3.0]));
    // Several ::LINECHART columns: already named (regression guard).
    let line_cols = vec![
        x(),
        Column::new("events", Role::Value(Kind::Line), nums(&[10.0, 14.0, 9.0])),
        Column::new("failures", Role::Value(Kind::Line), nums(&[3.0, 5.0, 2.0])),
    ];
    let svg = render(&line_cols, 480, 320).unwrap();
    assert!(svg.contains(">events<"), "{svg}");
    assert!(svg.contains(">failures<"), "{svg}");
    assert!(!svg.contains(">c1<") && !svg.contains(">c2<"), "{svg}");
    assert!(
        svg.contains("data-series=\"events\"") && svg.contains("data-series=\"failures\""),
        "{svg}"
    );

    // Several ::BARCHART columns: each bar gets its own fill colour + a real
    // legend entry — not every series painted in the single brand colour
    // with no way to tell them apart.
    let bar_cols = vec![
        x(),
        Column::new("events", Role::Value(Kind::Bar), nums(&[10.0, 14.0, 9.0])),
        Column::new("failures", Role::Value(Kind::Bar), nums(&[3.0, 5.0, 2.0])),
    ];
    let svg = render(&bar_cols, 480, 320).unwrap();
    assert!(svg.contains(">events<"), "{svg}");
    assert!(svg.contains(">failures<"), "{svg}");
    assert!(!svg.contains(">c1<") && !svg.contains(">c2<"), "{svg}");
    assert!(
        svg.contains("data-series=\"events\"") && svg.contains("data-series=\"failures\""),
        "{svg}"
    );
    // The two series' bars are not all the same colour.
    let fill_of = |series: &str| {
        let i = svg.find(&format!("data-series=\"{series}\"")).unwrap();
        let tag = &svg[svg[..i].rfind('<').unwrap()..i];
        tag[tag.find("fill=\"").unwrap() + 6..][..7].to_string()
    };
    assert_ne!(fill_of("events"), fill_of("failures"), "{svg}");
}

/// #292: a *mixed* combo (bars with a different-kind overlay, e.g. a trend
/// line on top of bars) keeps its previous behaviour — the overlay takes its
/// own palette colour and there is no legend — unaffected by the bar-family
/// combo-legend fix above.
#[test]
fn a_mixed_bar_and_line_combo_keeps_its_own_overlay_colour_and_no_legend() {
    let cols = vec![
        Column::new("day", Role::X, nums(&[1.0, 2.0, 3.0])),
        Column::new("actual", Role::Value(Kind::Bar), nums(&[10.0, 14.0, 9.0])),
        Column::new("trend", Role::Value(Kind::Line), nums(&[9.0, 11.0, 10.0])),
    ];
    let svg = render(&cols, 480, 320).unwrap();
    assert!(
        !svg.contains(">actual<") && !svg.contains(">trend<"),
        "{svg}"
    );
    assert!(svg.contains("<rect") && svg.contains("<polyline"), "{svg}");
}

/// The browser offers zoom only for kinds that honour a zoom window; the set
/// comes from `roles_json()`.
#[test]
fn zoomable_role_set() {
    let v: serde_json::Value =
        serde_json::from_str(&anofox_visualization::roles::roles_json()).unwrap();
    let z: Vec<&str> = v["sets"]["zoomable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    for k in ["LINECHART", "BARCHART", "SCATTER", "AREACHART"] {
        assert!(z.contains(&k), "{k} zoomable: {z:?}");
    }
    for k in [
        "GAUGE",
        "PIE",
        "HEATMAP",
        "CALENDAR",
        "CANDLESTICK",
        "RADAR",
        "SPARKLINE",
    ] {
        assert!(!z.contains(&k), "{k} not zoomable");
    }
}

#[test]
fn colors_level_map_paints_category_levels_in_status_colours() {
    // SELECT d::XAXIS, p::SCATTER, status::CATEGORY, 'Critical:bad; Watch:warn; OK:good'::COLORS
    let cols = vec![
        Column::new("d", Role::X, nums(&[1.0, 5.0, 20.0])),
        Column::new(
            "p",
            parse_role("SCATTER").unwrap(),
            nums(&[90.0, 40.0, 10.0]),
        ),
        Column::new("status", Role::Category, strs(&["Critical", "Watch", "OK"])),
        Column::new(
            "c",
            Role::GaugeColors,
            strs(&["Critical:bad; Watch:warn; OK:good"; 3]),
        ),
    ];
    let svg = render(&cols, 480, 320).unwrap().to_lowercase();
    let t = anofox_visualization::theme::ResolvedTheme::default();
    for (r, g, b) in [t.colors.negative, t.colors.warning, t.colors.positive] {
        let hex = format!("#{r:02x}{g:02x}{b:02x}");
        let rgb = format!("rgb({r},{g},{b})");
        assert!(svg.contains(&hex) || svg.contains(&rgb), "{hex} drawn");
    }
    // The legend names the levels, in the map's order — never a hex code.
    let (c, w, o) = (
        svg.find(">critical<"),
        svg.find(">watch<"),
        svg.find(">ok<"),
    );
    assert!(c.is_some() && c < w && w < o, "legend Critical, Watch, OK");
}

#[test]
fn a_constant_size_sets_the_point_size_without_a_legend() {
    // SELECT d::XAXIS, p::SCATTER, s::CATEGORY, 7::SIZE — 7 is no measure
    let cols = |size: &[f64]| {
        vec![
            Column::new("d", Role::X, nums(&[1.0, 5.0, 20.0])),
            Column::new(
                "p",
                parse_role("SCATTER").unwrap(),
                nums(&[90.0, 40.0, 10.0]),
            ),
            Column::new("s", Role::Category, strs(&["a", "b", "c"])),
            Column::new("n", Role::Size, nums(size)),
        ]
    };
    let fixed = render(&cols(&[7.0; 3]), 480, 320).unwrap();
    assert!(!fixed.contains(">7<"), "no size legend for a constant");
    assert!(
        fixed.contains(">a<") && fixed.contains(">c<"),
        "the colour legend stays"
    );
    let bubbles = render(&cols(&[1.0, 5.0, 9.0]), 480, 320).unwrap();
    assert_ne!(fixed, bubbles, "a varying ::SIZE is still a measure");
}

//! Chart motion directives (#39 ::ANIMATE, #42 ::LINESTYLE 'flow', #40
//! ::STORY, #41 ::TRANSITION): what the renderer hands the browser on the SVG.
use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn root(svg: &str) -> &str {
    &svg[..svg.find('>').unwrap()]
}
fn line(extra: Vec<Column>) -> String {
    let mut cols = vec![
        Column::new(
            "d",
            Role::X,
            strs(&["2026-10-05", "2026-10-12", "2026-10-19", "2026-10-26"]),
        ),
        Column::new(
            "s",
            Role::Category,
            strs(&["Observed", "Observed", "Forecast", "Forecast"]),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Line),
            nums(&[10.0, 12.0, 11.0, 14.0]),
        ),
    ];
    cols.extend(extra);
    render(&cols, 480, 300).unwrap()
}

#[test]
fn animate_mode_rides_on_the_root() {
    let svg = line(vec![Column::new(
        "a",
        Role::Animate,
        strs(&["Grow", "Grow", "Grow", "Grow"]),
    )]);
    assert!(
        root(&svg).contains(r#"data-animate="grow""#),
        "{}",
        root(&svg)
    );
    // no directive, an unknown mode: nothing (the browser plays 'auto')
    assert!(!line(vec![]).contains("data-animate"));
    let bad = line(vec![Column::new("a", Role::Animate, strs(&["wobble"; 4]))]);
    assert!(!bad.contains("data-animate"));
}

#[test]
fn reveal_starts_at_a_date_xline() {
    let svg = line(vec![
        Column::new("t", Role::VLine, strs(&["2026-10-12"; 4])),
        Column::new("a", Role::Animate, strs(&["reveal"; 4])),
    ]);
    let r = root(&svg);
    assert!(r.contains(r#"data-animate="reveal""#), "{r}");
    // 2026-10-12 00:00 UTC in epoch seconds — the date axis' unit
    assert!(r.contains(r#"data-xline="1791763200""#), "{r}");
    // the date rule is drawn, and says its date rather than epoch seconds
    assert!(
        svg.contains("<title>2026-10-12</title>"),
        "date XLINE tooltip"
    );
    assert!(!svg.contains("x = 1791763200"));
}

#[test]
fn a_coerced_date_xline_also_says_its_date() {
    // The query path (sql::coerce_for_role) hands a DATE ::XLINE over as epoch
    // seconds, not text.
    let svg = line(vec![
        Column::new("t", Role::VLine, vec![Value::DateTime(1_791_763_200); 4]),
        Column::new("a", Role::Animate, strs(&["reveal"; 4])),
    ]);
    assert!(root(&svg).contains(r#"data-xline="1791763200""#));
    assert!(
        svg.contains("<title>2026-10-12</title>"),
        "date XLINE tooltip"
    );
    assert!(!svg.contains("x = 1791763200"));
}

#[test]
fn numeric_xline_still_draws() {
    let cols = vec![
        Column::new("x", Role::X, nums(&[1.0, 2.0, 3.0])),
        Column::new("v", Role::Value(Kind::Line), nums(&[1.0, 4.0, 2.0])),
        Column::new("t", Role::VLine, strs(&["2", "2", "2"])),
    ];
    let svg = render(&cols, 400, 300).unwrap();
    assert!(
        svg.contains("stroke-dasharray"),
        "the text-typed XLINE '2' is drawn"
    );
}

#[test]
fn reference_rules_are_marked_as_no_data() {
    // The browser's chart summary and keyboard walk skip `data-rule` marks:
    // a date XLINE ("Today") once read as a series "2026-10-12" valued -12.
    let rules = |svg: &str| svg.matches(" data-rule=\"").count();
    let svg = line(vec![
        Column::new("t", Role::VLine, vec![Value::DateTime(1_791_763_200); 4]),
        Column::new("r", Role::RefLine, nums(&[11.0; 4])),
    ]);
    assert!(
        svg.contains(r#"data-x="2026-10-12" data-rule="x""#),
        "{svg}"
    );
    assert!(svg.contains(r#"data-value="11" data-rule="y""#), "{svg}");
    assert_eq!(rules(&svg), 2);
    let cols = vec![
        Column::new("x", Role::X, nums(&[1.0, 2.0, 3.0])),
        Column::new("v", Role::Value(Kind::Line), nums(&[1.0, 4.0, 2.0])),
        Column::new("t", Role::VLine, nums(&[2.0; 3])),
    ];
    let svg = render(&cols, 400, 300).unwrap();
    assert!(svg.contains(r#"data-value="2" data-rule="x""#), "{svg}");
    assert_eq!(rules(&svg), 1);
    // a dashed data line is no rule
    let dashed = line(vec![Column::new(
        "ls",
        Role::LineStyle,
        strs(&["dashed"; 4]),
    )]);
    assert_eq!(rules(&dashed), 0);
}

#[test]
fn flow_dashes_the_series_that_asks_for_it() {
    let svg = line(vec![Column::new(
        "ls",
        Role::LineStyle,
        vec![
            Value::Na,
            Value::Na,
            Value::Str("flow".into()),
            Value::Str("flow".into()),
        ],
    )]);
    let tags: Vec<&str> = svg
        .split("<polyline ")
        .skip(1)
        .map(|t| &t[..t.find('>').unwrap()])
        .collect();
    let fc: Vec<_> = tags
        .iter()
        .filter(|t| t.contains(r#"data-series="Forecast""#))
        .collect();
    let ob: Vec<_> = tags
        .iter()
        .filter(|t| t.contains(r#"data-series="Observed""#))
        .collect();
    assert!(!fc.is_empty() && !ob.is_empty());
    assert!(
        fc.iter()
            .all(|t| t.contains(r#"class="dp-flow""#) && t.contains(r#"stroke-dasharray="6 4""#)),
        "{fc:?}"
    );
    assert!(
        ob.iter()
            .all(|t| !t.contains("dp-flow") && !t.contains("stroke-dasharray")),
        "{ob:?}"
    );
    // grid lines are untouched
    assert_eq!(svg.matches("dp-flow").count(), fc.len());
}

#[test]
fn dashed_without_a_category_styles_the_one_line() {
    let cols = vec![
        Column::new("x", Role::X, nums(&[1.0, 2.0, 3.0, 4.0])),
        Column::new("v", Role::Value(Kind::Line), nums(&[1.0, 4.0, 2.0, 3.0])),
        Column::new("ls", Role::LineStyle, strs(&["dashed"; 4])),
    ];
    let svg = render(&cols, 400, 300).unwrap();
    assert_eq!(
        svg.matches(r#"stroke-dasharray="6 4""#).count(),
        1,
        "one data line dashed"
    );
    assert!(!svg.contains("dp-flow"));
}

#[test]
fn story_steps_ride_on_the_root() {
    let cols = vec![
        Column::new("w", Role::X, strs(&["W1", "W1", "W2", "W2", "W3", "W3"])),
        Column::new("c", Role::Category, strs(&["a", "b", "a", "b", "a", "b"])),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            nums(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        ),
        Column::new(
            "s",
            Role::Story,
            vec![
                Value::Na,
                Value::Na,
                Value::Str("W2: \"both\" <peak>".into()),
                Value::Str("ignored: W2 already has a caption".into()),
                Value::Str("  ".into()),
                Value::Str("W3 & after".into()),
            ],
        ),
    ];
    let svg = render(&cols, 480, 300).unwrap();
    let r = root(&svg);
    assert!(r.contains(r#"data-story-by="x""#), "{r}");
    // escaped JSON in the attribute; one step per x, in row order
    assert!(
        r.contains(r#"data-story="[{&quot;k&quot;:&quot;W2&quot;,&quot;t&quot;:&quot;W2: \&quot;both\&quot; &lt;peak&gt;&quot;},{&quot;k&quot;:&quot;W3&quot;,&quot;t&quot;:&quot;W3 &amp; after&quot;}]""#),
        "{r}"
    );
    // by series
    let mut by = cols.clone();
    by.push(Column::new("b", Role::StoryBy, strs(&["series"; 6])));
    let svg = render(&by, 480, 300).unwrap();
    let r = root(&svg);
    assert!(
        r.contains(r#"data-story-by="series""#) && r.contains("&quot;k&quot;:&quot;a&quot;"),
        "{r}"
    );
}

#[test]
fn tween_rides_on_the_root() {
    let svg = line(vec![Column::new(
        "t",
        Role::Transition,
        strs(&["Tween"; 4]),
    )]);
    assert!(root(&svg).contains(r#"data-transition="tween""#));
    let svg = line(vec![Column::new("t", Role::Transition, strs(&["none"; 4]))]);
    assert!(!svg.contains("data-transition"));
}

/// Every `data-story` key of the SVG, unescaped.
fn story_keys(svg: &str) -> Vec<String> {
    let r = root(svg);
    let a = r.find("data-story=\"").expect("data-story") + 12;
    let json = r[a..a + r[a..].find('"').unwrap()]
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s["k"].as_str().unwrap().to_string())
        .collect()
}

/// The `data-x` of every mark in the SVG.
fn mark_xs(svg: &str) -> Vec<String> {
    svg.split(" data-x=\"")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].to_string())
        .collect()
}

/// Each step key spells its x exactly as the marks' `data-x` does, so the
/// browser lights them: dates and timestamps (not epoch seconds), numbers.
#[test]
fn story_keys_match_mark_x() {
    let day = 86_400;
    let sep1 = 1_788_220_800; // 2026-09-01
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "date",
            (0..3).map(|i| Value::DateTime(sep1 + i * day)).collect(),
        ),
        (
            "timestamp",
            (0..3)
                .map(|i| Value::DateTime(sep1 + i * day + 9 * 3600 + 30 * 60))
                .collect(),
        ),
        ("integer", (1..4).map(Value::Integer).collect()),
        ("float", nums(&[0.5, 1.12345, 2.25])),
        (
            "string date",
            strs(&["2026-09-01", "2026-09-02", "2026-09-03"]),
        ),
    ];
    for (name, xs) in cases {
        for kind in [Kind::Bar, Kind::Point, Kind::Line] {
            let cols = vec![
                Column::new("x", Role::X, xs.clone()),
                Column::new("v", Role::Value(kind), nums(&[1.0, 3.0, 2.0])),
                Column::new(
                    "s",
                    Role::Story,
                    vec![Value::Na, Value::Str("the second one".into()), Value::Na],
                ),
            ];
            let svg = render(&cols, 480, 300).unwrap();
            let keys = story_keys(&svg);
            let marks = mark_xs(&svg);
            assert_eq!(keys.len(), 1, "{name} {kind:?}");
            assert!(
                marks.contains(&keys[0]),
                "{name} {kind:?}: key {:?} is no mark's data-x {marks:?}",
                keys[0]
            );
        }
    }
    // a date axis spells the day
    let cols = vec![
        Column::new(
            "x",
            Role::X,
            vec![Value::DateTime(sep1), Value::DateTime(sep1 + day)],
        ),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0])),
        Column::new("s", Role::Story, strs(&["first", ""])),
    ];
    assert_eq!(
        story_keys(&render(&cols, 480, 300).unwrap()),
        ["2026-09-01"]
    );
    // 'series'::STORY_BY: a numeric series key as the marks' data-series
    let cols = vec![
        Column::new("w", Role::X, strs(&["W1", "W1", "W2", "W2"])),
        Column::new("c", Role::Category, nums(&[1.0, 2.12345, 1.0, 2.12345])),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0, 3.0, 4.0])),
        Column::new("s", Role::Story, strs(&["", "the second series", "", ""])),
        Column::new("b", Role::StoryBy, strs(&["series"; 4])),
    ];
    let svg = render(&cols, 480, 300).unwrap();
    let series: Vec<String> = svg
        .split(" data-series=\"")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].to_string())
        .collect();
    let keys = story_keys(&svg);
    assert!(
        series.contains(&keys[0]),
        "key {keys:?} is no mark's data-series {series:?}"
    );
}

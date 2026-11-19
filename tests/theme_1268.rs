//! #1268: a theme's own grid and axis colours, contrast checks (and an
//! opt-in lift) for a host's colours, and a theme resolved once and reused.

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::theme::{
    self, check, contrast, ContrastIssue, Variant, MIN_GRAPHIC_CONTRAST, MIN_SERIES_DISTANCE,
    MIN_TEXT_CONTRAST,
};
use anofox_visualization::{render_spec_checked, render_spec_checked_themed, Value};
use serde_json::json;
use std::sync::Arc;

const BARS: &str =
    r#"{"rows":[{"c0":"a","c1":1},{"c0":"b","c1":2}],"roles":[[0,"XAXIS"],[1,"BARCHART"]]}"#;

/// [`BARS`] in a theme (a JSON object), light or dark.
fn bars_in(theme: &serde_json::Value, variant: &str) -> String {
    let mut spec: serde_json::Value = serde_json::from_str(BARS).unwrap();
    spec["theme"] = theme.clone();
    spec["variant"] = json!(variant);
    render_spec_checked(&spec.to_string())
        .unwrap()
        .to_ascii_lowercase()
}

#[test]
fn grid_and_axis_have_their_own_colours() {
    let t = json!({"chart": {"grid": "#ff0001", "axis": "#00aa02"}});
    let svg = bars_in(&t, "light");
    assert!(svg.contains("#ff0001"), "grid colour missing");
    assert!(svg.contains("#00aa02"), "axis colour missing");
    // the default look: the derived hairline grid, no axis line
    let set = theme::resolve("").unwrap();
    assert_eq!(set.light.chart.grid, None);
    assert_eq!(set.light.chart_axis(), None);
    assert_eq!(
        set.light.chart_grid(),
        theme::mix(set.light.colors.line, set.light.colors.card, 0.25)
    );
}

#[test]
fn a_top_level_grid_is_for_the_light_card_only() {
    let set =
        theme::resolve_json(&json!({"chart": {"grid": "#f1f2f3", "axis": "#333334"}})).unwrap();
    assert_eq!(set.light.chart.grid, Some((0xf1, 0xf2, 0xf3)));
    assert_eq!(set.dark.chart.grid, None);
    assert_eq!(set.dark.chart.axis, None);
    let set = theme::resolve_json(&json!({
        "chart": {"grid": "#f1f2f3"},
        "dark": {"chart": {"grid": "#202122", "axis": "#9a9b9c"}}
    }))
    .unwrap();
    assert_eq!(set.dark.chart.grid, Some((0x20, 0x21, 0x22)));
    assert_eq!(set.dark.chart.axis, Some((0x9a, 0x9b, 0x9c)));
    // a dark-only theme keeps what it says at the top
    let set =
        theme::resolve_json(&json!({"extends": "dark", "chart": {"grid": "#202122"}})).unwrap();
    assert_eq!(set.dark.chart.grid, Some((0x20, 0x21, 0x22)));
    // the dark chart draws its own (unremapped) grid colour
    let t = json!({"dark": {"chart": {"grid": "#3a3b3c"}}});
    assert!(bars_in(&t, "dark").contains("#3a3b3c"));
}

#[test]
fn the_built_in_themes_pass_the_check() {
    for name in ["light", "dark", "print", "colorblind", "high-contrast"] {
        let set = theme::resolve(name).unwrap();
        for t in [&set.light, &set.dark] {
            let low: Vec<String> = check(t)
                .iter()
                .filter(|i| {
                    // the default palettes' light tints are for fills; text
                    // and accents must pass
                    !matches!(i, ContrastIssue::LowContrast { token, .. } if token.starts_with("palette"))
                })
                .map(|i| i.to_string())
                .collect();
            assert!(low.is_empty(), "{name} {:?}: {low:?}", t.variant);
        }
    }
}

#[test]
fn a_dark_brand_on_a_dark_card_is_flagged_and_lifted() {
    let def = json!({
        "palette": ["#4b2c70", "#3f2560", "#16a34a"],
        "dark": {"colors": {"accent": "#4b2c70"}}
    });
    let set = theme::resolve_json(&def).unwrap();
    let issues = check(&set.dark);
    let tokens: Vec<String> = issues
        .iter()
        .filter_map(|i| match i {
            ContrastIssue::LowContrast { token, .. } => Some(token.clone()),
            _ => None,
        })
        .collect();
    assert!(tokens.contains(&"palette[0]".to_string()), "{issues:?}");
    assert!(tokens.contains(&"palette[1]".to_string()), "{issues:?}");
    assert!(tokens.contains(&"colors.accent".to_string()), "{issues:?}");

    let mut lifted = def.clone();
    lifted["lift"] = json!(true);
    let set = theme::resolve_json(&lifted).unwrap();
    for t in [&set.light, &set.dark] {
        assert_eq!(check(t), Vec::new(), "{:?}", t.variant);
        for c in &t.palette {
            assert!(contrast(*c, t.colors.card) >= MIN_GRAPHIC_CONTRAST);
        }
        assert!(contrast(t.colors.muted, t.colors.card) >= MIN_TEXT_CONTRAST);
    }
    // two dark shades of one hue stay two series after the lift
    let (a, b) = (set.dark.palette[0], set.dark.palette[1]);
    let d = ((a.0 as f64 - b.0 as f64).powi(2)
        + (a.1 as f64 - b.1 as f64).powi(2)
        + (a.2 as f64 - b.2 as f64).powi(2))
    .sqrt();
    assert!(d >= MIN_SERIES_DISTANCE, "{a:?} {b:?}");
    // the light variant's colours already passed and are kept
    assert_eq!(set.light.palette[2], (0x16, 0xa3, 0x4a));
    // only true/false
    let mut bad = def;
    bad["lift"] = json!("yes");
    assert!(theme::resolve_json(&bad).is_err());
}

#[test]
fn a_resolved_theme_is_reused_for_specs() {
    let set = theme::resolve_json(&json!({"palette": ["#a1b2c3"]})).unwrap();
    let svg = render_spec_checked_themed(BARS, &set)
        .unwrap()
        .to_ascii_lowercase();
    assert!(svg.contains("#a1b2c3"), "the host's palette");
    assert!(!render_spec_checked(BARS)
        .unwrap()
        .to_ascii_lowercase()
        .contains("#a1b2c3"));
    // the spec's own theme still wins
    let own =
        r#"{"rows":[{"c0":"a","c1":1}],"roles":[[0,"XAXIS"],[1,"BARCHART"]],"theme":"light"}"#;
    assert!(!render_spec_checked_themed(own, &set)
        .unwrap()
        .contains("#a1b2c3"));
}

struct Rows;

impl DataProvider for Rows {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if sql.contains("'light'") {
            return Ok(vec![("c0".into(), vec![Value::Str("light".into())])]);
        }
        Ok(vec![
            (
                "c0".into(),
                vec![Value::Str("a".into()), Value::Str("b".into())],
            ),
            ("c1".into(), vec![Value::Float(1.0), Value::Float(2.0)]),
        ])
    }
}

#[test]
fn a_resolved_theme_is_reused_for_dashboards() {
    let set = theme::resolve_json(&json!({"palette": ["#a1b2c3"], "colors": {"card": "#fefdfc"}}))
        .unwrap();
    let opts = DashboardOptions {
        theme_set: Some(Arc::new(set)),
        variant: Variant::Light,
        ..Default::default()
    };
    let chart = "SELECT x::XAXIS, y::BARCHART FROM t;";
    let svg = render_dashboard_svg(chart, &mut Rows, &opts)
        .unwrap()
        .to_ascii_lowercase();
    assert!(svg.contains("#fefdfc") && svg.contains("#a1b2c3"));
    // the dashboard's own ::THEME wins
    let svg = render_dashboard_svg(
        &format!("SELECT 'light'::THEME;\n{chart}"),
        &mut Rows,
        &opts,
    )
    .unwrap();
    assert!(!svg.contains("#fefdfc"));
}

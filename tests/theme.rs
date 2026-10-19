//! Theme model, design tokens and locale (plan M1).

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::format::{format_number_l, metric_fmt_of, Locale};
use anofox_visualization::theme::{self, Variant, BUILTINS};
use anofox_visualization::{render_spec_checked, render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;
use std::sync::Arc;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn read(rel: &str) -> String {
    std::fs::read_to_string(format!("{ROOT}/{rel}")).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

// ───────────────────────── tokens ─────────────────────────

/// web/index.html embeds exactly the default theme's token block (the page
/// renders before wasm loads); regenerate with `dashboard --theme-css light`.
#[test]
fn index_html_default_tokens_are_in_sync() {
    let landing = read("web/landing.css");
    assert_eq!(
        literal_hits(&landing),
        Vec::<String>::new(),
        "web/landing.css"
    );
    let html = read("web/index.html");
    let a = html.find("/* ax-theme:start */\n").expect("start marker") + 21;
    let b = html.find("/* ax-theme:end */").expect("end marker");
    assert_eq!(
        html[a..b].trim(),
        theme::default_set().light.css_vars(),
        "web/index.html ax-theme block is stale — regenerate with `dashboard --theme-css light`"
    );
}

/// Colour literals (`#rgb`, `#rrggbb`, `rgb(`, `rgba(`) outside custom
/// property declarations.
fn literal_hits(css: &str) -> Vec<String> {
    let mut hits = Vec::new();
    // Strip comments.
    let mut clean = String::new();
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        clean.push_str(&rest[..i]);
        rest = rest[i..]
            .find("*/")
            .map(|j| &rest[i + j + 2..])
            .unwrap_or("");
    }
    clean.push_str(rest);
    for decl in clean.split([';', '{', '}']) {
        let d = decl.trim();
        if d.starts_with("--") || d.is_empty() {
            continue; // a token definition may hold literals
        }
        let low = d.to_ascii_lowercase();
        let hex = low.match_indices('#').any(|(i, _)| {
            let h: String = low[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            (h.len() == 3 || h.len() == 6 || h.len() == 8)
                && !low[i + 1 + h.len()..]
                    .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
        });
        if hex || low.contains("rgb(") || low.contains("rgba(") {
            hits.push(d.to_string());
        }
    }
    hits
}

/// T8: the dashboard stylesheet and the builder chrome use tokens only —
/// colour literals live in token definitions (and in src/theme.rs).
#[test]
fn css_is_token_only() {
    let base = read("web/theme-base.css");
    assert_eq!(
        literal_hits(&base),
        Vec::<String>::new(),
        "web/theme-base.css"
    );
    let html = read("web/index.html");
    let mut css = String::new();
    let mut rest = html.as_str();
    while let Some(i) = rest.find("<style") {
        let s = &rest[i..];
        let open = s.find('>').unwrap() + 1;
        let close = s.find("</style>").unwrap();
        css.push_str(&s[open..close]);
        rest = &s[close..];
    }
    assert_eq!(
        literal_hits(&css),
        Vec::<String>::new(),
        "web/index.html <style>"
    );
}

/// The Rust page writers carry no colour literals of their own.
#[test]
fn rust_page_writers_have_no_colour_literals() {
    for f in [
        "src/lib.rs",
        "src/dashboard.rs",
        "src/page.rs",
        "src/bin/dashboard.rs",
        "src/bin/serve.rs",
    ] {
        let src = read(f);
        let hits: Vec<&str> = src
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//")
                    && l.match_indices('#').any(|(i, _)| {
                        let h: String = l[i + 1..]
                            .chars()
                            .take_while(|c| c.is_ascii_hexdigit())
                            .collect();
                        h.len() == 6
                            && !l[i + 1 + 6..]
                                .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                    })
            })
            .collect();
        assert!(
            hits.is_empty(),
            "{f}: colour literals outside src/theme.rs:\n{hits:#?}"
        );
    }
}

#[test]
fn every_builtin_emits_the_same_token_names() {
    let names = |t: &theme::ResolvedTheme| -> Vec<String> {
        let mut v: Vec<String> = t.css_tokens().into_iter().map(|(k, _)| k).collect();
        v.sort();
        v
    };
    let reference = names(&theme::default_set().light);
    for (n, _) in BUILTINS {
        let set = theme::resolve(n).unwrap();
        for t in [&set.light, &set.dark] {
            assert_eq!(names(t), reference, "{n}/{:?}", t.variant);
        }
    }
    // every token the stylesheet uses is defined
    let base = read("web/theme-base.css");
    for used in base.split("var(--ax-").skip(1) {
        let name: String = used
            .chars()
            .take_while(|c| *c != ')' && *c != ',')
            .collect();
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue; // a comment like `var(--ax-*)`
        }
        assert!(
            reference.contains(&name),
            "theme-base.css uses undefined --ax-{name}"
        );
    }
}

// ───────────────────────── charts ─────────────────────────

fn bars() -> Vec<Column> {
    vec![
        Column::new(
            "w",
            Role::X,
            ["W1", "W2", "W3"]
                .iter()
                .map(|s| Value::Str(s.to_string()))
                .collect(),
        ),
        Column::new(
            "c",
            Role::Category,
            ["a", "b", "a"]
                .iter()
                .map(|s| Value::Str(s.to_string()))
                .collect(),
        ),
        Column::new(
            "n",
            Role::Value(Kind::Bar),
            [1200.0, 3400.5, 2100.0]
                .iter()
                .map(|f| Value::Float(*f))
                .collect(),
        ),
    ]
}

/// The default theme renders exactly as an explicit `light` (byte-identical).
#[test]
fn default_and_explicit_light_are_identical() {
    let d = render_with(&bars(), 480, 320, &RenderOptions::default()).unwrap();
    let o = RenderOptions {
        theme: Arc::new(theme::resolve("light").unwrap().light),
        ..RenderOptions::default()
    };
    assert_eq!(d, render_with(&bars(), 480, 320, &o).unwrap());
    let spec = r#"{"rows":[{"c0":"W1","c1":3}],"roles":[[0,"XAXIS"],[1,"BARCHART"]]}"#;
    let with = r#"{"rows":[{"c0":"W1","c1":3}],"roles":[[0,"XAXIS"],[1,"BARCHART"]],"theme":"light","variant":"light"}"#;
    assert_eq!(
        render_spec_checked(spec).unwrap(),
        render_spec_checked(with).unwrap()
    );
}

#[test]
fn dark_charts_have_dark_ink_and_lifted_palette() {
    let o = RenderOptions {
        theme: Arc::new(theme::default_set().dark.clone()),
        ..RenderOptions::default()
    };
    let svg = render_with(&bars(), 480, 320, &o).unwrap();
    let dark = &theme::default_set().dark;
    assert!(
        svg.contains(&theme::hex(dark.color(0)).to_ascii_uppercase())
            || svg.contains(&theme::hex(dark.color(0)))
    );
    assert!(!svg.contains("fill=\"#000000\""), "black text in dark");
    assert!(
        !svg.to_ascii_lowercase().contains("fill=\"#ffffff\""),
        "white fill in dark"
    );
    // via the spec / macro path
    let spec =
        r#"{"rows":[{"c0":"W1","c1":3}],"roles":[[0,"XAXIS"],[1,"BARCHART"]],"theme":"dark"}"#;
    let s = render_spec_checked(spec).unwrap();
    assert!(!s.contains("fill=\"#000000\""));
    assert!(render_spec_checked(r#"{"rows":[],"theme":"nope"}"#).is_err());
}

#[test]
fn json_theme_file_recolours_the_palette() {
    let acme = read("examples/theming/acme.json");
    let set = theme::resolve(&acme).unwrap();
    assert_eq!(set.name, "acme");
    let o = RenderOptions {
        theme: Arc::new(set.light.clone()),
        ..RenderOptions::default()
    };
    let svg = render_with(&bars(), 480, 320, &o)
        .unwrap()
        .to_ascii_lowercase();
    assert!(svg.contains("#4338ca"), "acme palette[0]");
    assert!(
        !svg.contains("#456481"),
        "DataZoo steel leaked into a custom theme"
    );
}

#[test]
fn de_de_axis_labels() {
    let o = RenderOptions {
        locale: Locale::from_tag("de-DE").unwrap(),
        ..RenderOptions::default()
    };
    let svg = render_with(&bars(), 480, 320, &o).unwrap();
    assert!(
        svg.contains(">3.000<") || svg.contains(">2.000<"),
        "grouped de-DE ticks: {svg}"
    );
    // ::YFORMAT '€' in de-DE: suffix currency
    let mut cols = bars();
    cols.push(Column::new(
        "f",
        Role::YFormat,
        vec![Value::Str("€".into())],
    ));
    let svg = render_with(&cols, 480, 320, &o).unwrap();
    assert!(svg.contains(" €<"), "{svg}");
    // en-US unchanged
    let svg = render_with(&cols, 480, 320, &RenderOptions::default()).unwrap();
    assert!(svg.contains(">€"));
}

// ───────────────────────── locale vectors (shared with the browser) ─────

#[test]
fn locale_vectors_match_the_shared_fixture() {
    let v: serde_json::Value =
        serde_json::from_str(&read("tests/fixtures/locale_vectors.json")).unwrap();
    for row in v["numbers"].as_array().unwrap() {
        let (n, fmt, tag, want) = (
            row[0].as_f64().unwrap(),
            row[1].as_str().unwrap(),
            row[2].as_str().unwrap(),
            row[3].as_str().unwrap(),
        );
        let l = Locale::from_tag(tag).unwrap();
        assert_eq!(
            format_number_l(n, metric_fmt_of(fmt), &l),
            want,
            "{n} {fmt} {tag}"
        );
    }
    for row in v["dates"].as_array().unwrap() {
        let l = Locale::from_tag(row[1].as_str().unwrap()).unwrap();
        assert_eq!(
            l.format_date(row[0].as_str().unwrap()),
            row[2].as_str().unwrap()
        );
    }
}

// ───────────────────────── headless dashboard + branding ──────────────────

/// A provider answering every query from canned data.
struct Canned;
impl DataProvider for Canned {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        let one = |s: &str| Ok(vec![("c0".to_string(), vec![Value::Str(s.into())])]);
        if sql.contains("'de-DE'") {
            return one("de-DE");
        }
        if sql.contains("'Sales cockpit'") {
            return one("Sales cockpit");
        }
        if sql.contains("'ACME'") {
            return one("ACME");
        }
        if sql.contains("'logo.svg'") {
            return one("logo.svg");
        }
        if sql.contains("THEMESPEC") {
            return one(&std::env::var("ANOFOX_TEST_THEME").unwrap_or_default());
        }
        if sql.contains("1234567.5") {
            return Ok(vec![
                ("c0".into(), vec![Value::Float(1234567.5)]),
                ("c1".into(), vec![Value::Str("Summe".into())]),
            ]);
        }
        Ok(vec![
            (
                "c0".into(),
                ["W1", "W2"]
                    .iter()
                    .map(|s| Value::Str(s.to_string()))
                    .collect(),
            ),
            (
                "c1".into(),
                vec![Value::Float(1500.0), Value::Float(2500.0)],
            ),
        ])
    }
}

const BRANDED: &str = "\
    SELECT 'Sales cockpit'::DASHBOARD_TITLE;\n\
    SELECT 'ACME'::BRAND;\n\
    SELECT 'logo.svg'::LOGO;\n\
    SELECT 'de-DE'::LOCALE;\n\
    SELECT 1234567.5::METRIC, 'Summe'::LABEL;\n\
    SELECT w::XAXIS, n::BARCHART FROM t;";

#[test]
fn headless_branding_and_locale() {
    let svg = render_dashboard_svg(BRANDED, &mut Canned, &DashboardOptions::default()).unwrap();
    assert!(svg.contains("Sales cockpit"));
    assert!(svg.contains(">ACME<"));
    assert!(svg.contains("href=\"logo.svg\""));
    assert!(svg.contains(">1.234.567,5<"), "de-DE KPI");
}

/// The owner removed the "Powered by DataZoo GmbH" credit from every output
/// (2026-10-19): it must not reappear in the headless SVG, the static page
/// HTML/landing page, or the browser builder, under any built-in theme.
#[test]
fn no_powered_by_credit_anywhere() {
    for (name, _) in BUILTINS {
        for variant in [Variant::Light, Variant::Dark] {
            let o = DashboardOptions {
                theme: Some(name.to_string()),
                variant,
                ..DashboardOptions::default()
            };
            let svg = render_dashboard_svg(BRANDED, &mut Canned, &o).unwrap();
            assert!(
                !svg.contains("Powered by DataZoo"),
                "headless {name}/{variant:?}"
            );
            let set = theme::resolve(name).unwrap();
            let html =
                anofox_visualization::page::render(&anofox_visualization::page::StaticPage {
                    theme: set.pick(variant),
                    fallback_title: "x",
                    subtitle_html: String::new(),
                    head_extra: String::new(),
                    controls_html: String::new(),
                    panels_html: String::new(),
                });
            assert!(!html.contains("Powered by DataZoo"), "page {name}");
            let list = anofox_visualization::page::list(set.pick(variant), "D", &[]);
            assert!(!list.contains("Powered by DataZoo"), "list {name}");
        }
    }
    // The browser no longer appends any credit on render (web/app.js).
    let js = read("web/app.js");
    assert!(!js.contains("Powered by DataZoo"));
    // and there is no role to hide it
    assert!(anofox_visualization::parse_role("CREDIT").is_none());
}

#[test]
fn headless_dark_has_no_white_cards() {
    let o = DashboardOptions {
        variant: Variant::Dark,
        theme: Some("light+dark".into()),
        ..DashboardOptions::default()
    };
    let svg = render_dashboard_svg(BRANDED, &mut Canned, &o).unwrap();
    assert!(!svg.to_ascii_lowercase().contains("fill=\"#ffffff\""));
    assert!(svg.contains("data-variant=\"dark\""));
}

// ───────────────────────── status tiles under CVD ─────────────────────────

/// CIELAB of an sRGB colour seen through a colour-vision-deficiency matrix
/// (Machado et al. 2009, severity 1.0; identity = normal vision).
fn lab_seen(c: (u8, u8, u8), m: &[[f64; 3]; 3]) -> [f64; 3] {
    let lin = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = [lin(c.0), lin(c.1), lin(c.2)];
    let [r, g, b] =
        [0, 1, 2].map(|i| (m[i][0] * l[0] + m[i][1] * l[1] + m[i][2] * l[2]).clamp(0.0, 1.0));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    [
        116.0 * f(y) - 16.0,
        500.0 * (f(x) - f(y)),
        200.0 * (f(y) - f(z)),
    ]
}

/// The status tiles of a heatmap `::COLORSCALE` / `::THRESHOLDS` (good, warn,
/// bad) stay apart — ΔE ≥ 12 between neighbours — in normal vision and under
/// simulated deuteranopia, protanopia and tritanopia, in every built-in theme
/// and both variants; lightness runs good → bad one way (pale → strong).
#[test]
fn status_tiles_stay_apart_under_cvd() {
    const VISION: [(&str, [[f64; 3]; 3]); 4] = [
        (
            "normal",
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        ),
        (
            "deuteranopia",
            [
                [0.367322, 0.860646, -0.227968],
                [0.280085, 0.672501, 0.047413],
                [-0.011820, 0.042940, 0.968881],
            ],
        ),
        (
            "protanopia",
            [
                [0.152286, 1.052583, -0.204868],
                [0.114503, 0.786281, 0.099216],
                [-0.003882, -0.048116, 1.051998],
            ],
        ),
        (
            "tritanopia",
            [
                [1.255528, -0.076749, -0.178779],
                [-0.078411, 0.930809, 0.147602],
                [0.004733, 0.691367, 0.303900],
            ],
        ),
    ];
    let de = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    for (name, _) in BUILTINS {
        let set = theme::resolve(name).unwrap();
        for t in [&set.light, &set.dark] {
            let tiles = t.status_tiles();
            let [bad, good] = t.status_arms();
            for (vision, m) in &VISION {
                let s = tiles.map(|c| lab_seen(c, m));
                for (a, b) in [(0, 1), (1, 2)] {
                    let d = de(s[a], s[b]);
                    assert!(
                        d >= 12.0,
                        "{name} {:?} {vision}: tiles {a}/{b} ΔE {d:.1}",
                        t.variant
                    );
                }
                let d = de(lab_seen(bad, m), lab_seen(good, m));
                assert!(
                    d >= 12.0,
                    "{name} {:?} {vision}: diverging ends ΔE {d:.1}",
                    t.variant
                );
            }
            let l = tiles.map(theme::luminance);
            let card_light = theme::luminance(t.colors.card) > 0.5;
            assert!(
                if card_light {
                    l[0] > l[1] && l[1] > l[2]
                } else {
                    l[0] < l[1] && l[1] < l[2]
                },
                "{name} {:?}: tile lightness runs good → bad ({l:?})",
                t.variant
            );
        }
    }
}

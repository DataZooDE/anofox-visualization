//! The dashboard **theme model** — one set of design tokens shared by the
//! chart renderer (ggplot-rs), the headless dashboard SVG, the browser
//! builder, `serve`, the served/locked pages, the CLI's static HTML and the
//! HTML export (plan T1/T8).
//!
//! A theme is selected or defined in SQL with `::THEME` (a built-in name, a
//! stack `'acme+dark'`, or a JSON object, e.g. `SELECT content::THEME FROM
//! read_text('acme.json')`), with `dashboard --theme` / `serve --theme`, or for
//! the SQL macros with `SET VARIABLE anofox_theme = …`. It resolves to a
//! [`ThemeSet`]: a light and a dark [`ResolvedTheme`] plus a [`Mode`] that says
//! which one a viewer sees (`auto` follows the OS preference / the 🌙 toggle /
//! `?theme=dark`).
//!
//! **Colour literals live here.** Every other module reads colours from a
//! [`ResolvedTheme`]; the built-in `light` theme is the default look. Chart
//! chrome (text, grid, gaps between marks) is painted from the same tokens
//! (`chart_text`, `chart_grid`, … below; see `crate::chart_theme`).
//!
//! ```text
//! {"name":"acme","extends":"light","mode":"auto",
//!  "palette":["#0b5fff","#e11d48",…], "sequential":["#eef3ff","#0b5fff"],
//!  "colors":{"bg":"#f4f6f9","card":"#ffffff","ink":"#1f2937","muted":"#6b7280",
//!            "line":"#e5e7eb","accent":"#0b5fff","positive":"#16a34a",…},
//!  "font":{"family":"Inter, system-ui, sans-serif","display":"Inter Tight, sans-serif"},
//!  "radius":10, "chrome":{"gradient":false,"kpi_value":"ink","table_zebra":true},
//!  "brand":{"name":"ACME Analytics","logo":"logo.svg","favicon":"favicon.svg"},
//!  "locale":"de-DE",
//!  "dark":{"colors":{"bg":"#0f1729","card":"#162033","ink":"#e5e7eb"}}}
//! ```
//!
//! The "Powered by DataZoo GmbH" credit is **not** a theme token: it is shown
//! on every dashboard under every theme (see [`CREDIT_TEXT`]).

use serde_json::{json, Map, Value as J};
use std::sync::{Arc, OnceLock};

/// An sRGB colour.
pub type Rgb = (u8, u8, u8);

/// The mandatory credit line every dashboard output carries (browser, export,
/// headless SVG, `serve`, locked pages) — under every theme.
pub const CREDIT_TEXT: &str = "Powered by DataZoo GmbH";
/// Text colours of the extension's own small SVG panels (headings, notes,
/// errors) in the light look; the dark variant maps them via
/// [`ResolvedTheme::finish_svg`].
pub const SVG_HEADING: &str = "#1f2430";
pub const SVG_NOTE: &str = "#8a93a6";
pub const SVG_ERROR: &str = "#b42318";

/// Where the credit links to.
pub const CREDIT_URL: &str = "https://data-zoo.de";

/// Which variant a viewer sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Follow the viewer: OS `prefers-color-scheme`, the 🌙 toggle, `?theme=`.
    Auto,
    /// Always the light variant.
    Light,
    /// Always the dark variant.
    Dark,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Light => "light",
            Mode::Dark => "dark",
        }
    }
}

/// A concrete colour variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Light,
    Dark,
}

impl Variant {
    /// `"dark"` → Dark; anything else → Light.
    pub fn parse(s: &str) -> Variant {
        if s.trim().eq_ignore_ascii_case("dark") {
            Variant::Dark
        } else {
            Variant::Light
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Light => "light",
            Variant::Dark => "dark",
        }
    }
}

/// Semantic page colours.
#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    /// Page background.
    pub bg: Rgb,
    /// Second page-background stop (gradient end).
    pub bg2: Rgb,
    /// Card / panel surface.
    pub card: Rgb,
    /// Body text.
    pub ink: Rgb,
    /// Secondary text (captions, table headers).
    pub muted: Rgb,
    /// Hairlines, card borders.
    pub line: Rgb,
    /// Panel / section / dashboard titles.
    pub heading: Rgb,
    /// Accent (active tab, focus, links, KPI value).
    pub accent: Rgb,
    /// Second accent (gradient end).
    pub accent2: Rgb,
    pub positive: Rgb,
    pub negative: Rgb,
    pub warning: Rgb,
    pub info: Rgb,
    /// Link colour in Markdown.
    pub link: Rgb,
    /// The "Other" bucket's colour in charts.
    pub other: Rgb,
}

/// Colours of the text/lines the extension draws itself inside SVGs
/// (headless dashboard, note/heading/error panels, gauge labels).
#[derive(Clone, Debug, PartialEq)]
pub struct ChartColors {
    /// A heading / panel title drawn in SVG.
    pub heading: Rgb,
    /// A quiet note ("No data", gauge ends).
    pub note: Rgb,
    /// Strong text (KPI values, gauge value).
    pub ink: Rgb,
    /// Secondary text (KPI captions, table headers).
    pub muted: Rgb,
    /// Card stroke / table rules in the headless SVG.
    pub line: Rgb,
    /// Error text.
    pub error: Rgb,
    /// Low end of a sequential fill (heatmap, calendar) — `None` = the
    /// built-in pale blue-grey.
    pub seq_low: Option<Rgb>,
}

/// Fonts (CSS font-family stacks).
#[derive(Clone, Debug, PartialEq)]
pub struct Fonts {
    pub family: String,
    pub display: String,
}

/// Author branding (`::DASHBOARD_TITLE`, `::BRAND`, `::LOGO`, `::FAVICON`).
/// Shown *in addition to* the mandatory DataZoo credit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brand {
    pub title: Option<String>,
    pub name: Option<String>,
    pub logo: Option<String>,
    pub logo_dark: Option<String>,
    pub favicon: Option<String>,
    pub title_suffix: Option<String>,
    /// `::ASOF`: the data's timestamp (epoch seconds, UTC) — the header's
    /// "Data as of …" chip. Set by the dashboard, never by a theme.
    pub asof: Option<i64>,
    /// `::ASOF`'s `'stale=24h'`: the chip turns amber past this age (seconds).
    pub stale_secs: Option<u64>,
}

/// Page-chrome style switches.
#[derive(Clone, Debug, PartialEq)]
pub struct ChromeStyle {
    /// Accent bars / buttons as a gradient (`true`) or solid accent.
    pub gradient: bool,
    /// Page background: `"gradient"` (soft glow + dots), `"plain"`.
    pub background: String,
    /// KPI value colour: `"gradient"` | `"accent"` | `"ink"`.
    pub kpi_value: String,
    /// Zebra-striped table rows.
    pub table_zebra: bool,
    /// Negative numbers in tables: `"minus"` (−5) | `"red"` (−5 in red) | `"parens"` ((5)).
    pub negatives: String,
    /// Zero in tables: `"muted"` (a grey 0) | `"dash"` (a grey –).
    pub zero: String,
    /// Card shadow: `"md"` | `"sm"` | `"none"`.
    pub shadow: String,
}

/// One concrete, fully-resolved theme variant.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTheme {
    /// The theme's name (`light`, `dark`, `acme`, `acme+dark`, …).
    pub name: String,
    pub variant: Variant,
    /// Categorical palette (index 0 = the default single-series colour).
    pub palette: Vec<Rgb>,
    /// Explicit sequential `[low, high]` (`None` = pale low → brand high).
    pub sequential: Option<[Rgb; 2]>,
    /// Diverging `[low, mid, high]`.
    pub diverging: [Rgb; 3],
    pub colors: Colors,
    pub chart: ChartColors,
    pub font: Fonts,
    /// Card radius in px.
    pub radius: f64,
    pub chrome: ChromeStyle,
    pub brand: Brand,
    /// BCP-47 locale tag (`de-DE`), `None` = en-US.
    pub locale: Option<String>,
}

/// A resolved theme: both variants and which one a viewer sees.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeSet {
    pub name: String,
    pub mode: Mode,
    pub light: ResolvedTheme,
    pub dark: ResolvedTheme,
}

impl ThemeSet {
    /// The variant to render for a viewer who prefers `preferred`.
    pub fn pick(&self, preferred: Variant) -> &ResolvedTheme {
        match (self.mode, preferred) {
            (Mode::Light, _) | (Mode::Auto, Variant::Light) => &self.light,
            (Mode::Dark, _) | (Mode::Auto, Variant::Dark) => &self.dark,
        }
    }
    /// The variant shown when nothing says otherwise (light unless forced dark).
    pub fn default_variant(&self) -> &ResolvedTheme {
        self.pick(Variant::Light)
    }
    /// JSON for the browser: `{name, mode, light:{css, …}, dark:{css, …}}`.
    pub fn to_json(&self) -> J {
        json!({
            "name": self.name,
            "mode": self.mode.as_str(),
            "light": self.light.to_json(),
            "dark": self.dark.to_json(),
        })
    }
}

// ───────────────────────── built-ins ──────────────────────────────────────

/// The DataZoo palette — the colours chart code writes directly (contract
/// plots); every theme maps them to its own palette slot by slot. Index 0 =
/// steel blue, 4 = near-black, 11 = slate ("Other").
pub const DZ_PALETTE: [Rgb; 12] = [
    (0x45, 0x64, 0x81), // steel blue (brand primary)
    (0xe8, 0x64, 0x33), // orange
    (0xE8, 0x33, 0x5D), // pink
    (0xef, 0xc9, 0x4c), // yellow
    (0x21, 0x21, 0x21), // near-black
    (0x3f, 0xa6, 0x6f), // green
    (0x81, 0x56, 0xa0), // purple
    (0x57, 0xb7, 0xd6), // sky blue
    (0x9c, 0x6b, 0x3f), // brown
    (0xa7, 0xc9, 0x57), // lime
    (0xd0, 0x81, 0xb8), // mauve
    (0x7a, 0x8b, 0x99), // slate grey
];

/// The default palette (the `light` theme): electric blue first, then hues
/// checked for colour-vision-deficiency separation between neighbours. The
/// slots keep the DataZoo palette's roles — 4 = near-black, 11 = grey
/// ("Other") — because chart code that writes [`DZ_PALETTE`] colours is
/// remapped slot by slot (see [`ResolvedTheme::finish_svg`]).
pub const DEFAULT_PALETTE: [Rgb; 12] = [
    (0x25, 0x63, 0xeb), // electric blue
    (0xeb, 0x68, 0x34), // orange
    (0x1b, 0xaf, 0x7a), // aqua
    (0xed, 0xa1, 0x00), // amber
    (0x1f, 0x29, 0x37), // near-black
    (0xe8, 0x7b, 0xa4), // pink
    (0x4a, 0x3a, 0xa7), // violet
    (0xe3, 0x49, 0x48), // red
    (0x00, 0x83, 0x00), // green
    (0x08, 0x91, 0xb2), // cyan
    (0xdb, 0x27, 0x77), // magenta
    (0x8b, 0x92, 0x9e), // grey ("Other")
];

/// [`DEFAULT_PALETTE`] stepped for dark cards (same hues and slots).
pub const DEFAULT_PALETTE_DARK: [Rgb; 12] = [
    (0x3b, 0x82, 0xf6),
    (0xd9, 0x59, 0x26),
    (0x19, 0x9e, 0x70),
    (0xc9, 0x85, 0x00),
    (0xd1, 0xd5, 0xdb),
    (0xd5, 0x51, 0x81),
    (0x90, 0x85, 0xe9),
    (0xe6, 0x67, 0x67),
    (0x22, 0xa3, 0x2a),
    (0x08, 0x91, 0xb2),
    (0xdb, 0x27, 0x77),
    (0x9c, 0xa3, 0xaf),
];

/// Okabe-Ito — the colour-blind-safe palette (`colorblind` theme).
pub const OKABE_ITO: [Rgb; 8] = [
    (0x00, 0x72, 0xB2),
    (0xE6, 0x9F, 0x00),
    (0x00, 0x9E, 0x73),
    (0xCC, 0x79, 0xA7),
    (0x56, 0xB4, 0xE9),
    (0xD5, 0x5E, 0x00),
    (0xF0, 0xE4, 0x42),
    (0x00, 0x00, 0x00),
];

/// Names of the built-in themes (for `--themes`, lint and docs).
pub const BUILTINS: &[(&str, &str)] = &[
    ("light", "the default graphite look (alias: default, datazoo); follows the viewer's light/dark preference"),
    ("dark", "always dark: dark cards, light ink, lifted palette"),
    ("print", "white page, black ink, no shadows or gradients — for printing/PDF"),
    ("colorblind", "the light look with the Okabe-Ito palette (safe under colour-vision deficiency)"),
    ("high-contrast", "maximum text contrast, solid accents, thicker rules"),
];

fn hexs(p: &[Rgb]) -> Vec<String> {
    p.iter().map(|c| hex(*c)).collect()
}

/// The root definition every theme extends.
fn light_json() -> J {
    json!({
        "name": "light",
        "mode": "auto",
        "palette": hexs(&DEFAULT_PALETTE),
        "sequential": null,
        "diverging": ["#dc2626", "#f3f4f6", "#2563eb"],
        "colors": {
            "bg": "#f4f5f7", "bg2": "#f4f5f7", "card": "#ffffff", "ink": "#0a0a0a",
            "muted": "#6b7280", "line": "#e5e7eb", "heading": "#0a0a0a",
            "accent": "#2563eb", "accent2": "#2563eb",
            "positive": "#15803d", "negative": "#dc2626", "warning": "#b45309",
            "info": "#2563eb", "link": "#1d4ed8", "other": "#8b929e"
        },
        "chart": {
            "heading": "#0a0a0a", "note": "#9ca3af", "ink": "#111827", "muted": "#6b7280",
            "line": "#eceef1", "error": "#b42318", "seq_low": null
        },
        "font": {
            "family": "'Hanken Grotesk',system-ui,-apple-system,'Segoe UI',sans-serif",
            "display": "'Hanken Grotesk',system-ui,-apple-system,'Segoe UI',sans-serif"
        },
        "radius": 8,
        "chrome": {"gradient": false, "background": "plain", "kpi_value": "ink",
                   "table_zebra": false, "shadow": "none", "negatives": "minus", "zero": "muted"},
        "brand": {},
        "locale": null,
        "dark": dark_overlay()
    })
}

/// The dark variant every theme gets unless it defines its own `dark` block
/// (its own block is merged over this one).
fn dark_overlay() -> J {
    json!({
        "colors": {
            "bg": "#0a0a0a", "bg2": "#0a0a0a", "card": "#141414", "ink": "#ededed",
            "muted": "#a1a1aa", "line": "#262626", "heading": "#fafafa",
            "accent": "#3b82f6", "accent2": "#3b82f6",
            "positive": "#4ade80", "negative": "#f87171", "warning": "#fbbf24",
            "info": "#60a5fa", "link": "#60a5fa", "other": "#8b8b93"
        },
        "chart": {
            "heading": "#fafafa", "note": "#8b8b93", "ink": "#ededed", "muted": "#a1a1aa",
            "line": "#262626", "error": "#f87171", "seq_low": "#16294d"
        },
        "chrome": {"background": "plain"}
    })
}

fn builtin_json(name: &str) -> Option<J> {
    let n = name.trim().to_ascii_lowercase();
    Some(match n.as_str() {
        "light" | "datazoo" | "default" => light_json(),
        "dark" => json!({"name": "dark", "extends": "light", "mode": "dark"}),
        "print" | "mono" => json!({
            "name": "print", "extends": "light", "mode": "light",
            "colors": {"bg": "#ffffff", "bg2": "#ffffff", "card": "#ffffff", "ink": "#111111",
                       "muted": "#555555", "line": "#d0d0d0", "heading": "#000000",
                       "accent": "#333333", "accent2": "#333333", "link": "#111111"},
            "chrome": {"gradient": false, "background": "plain", "kpi_value": "ink",
                       "table_zebra": true, "shadow": "none"}
        }),
        "colorblind" | "okabe-ito" => json!({
            "name": "colorblind", "extends": "light",
            "palette": hexs(&OKABE_ITO),
            "diverging": ["#0072B2", "#f5f5f5", "#E69F00"],
            "dark": {"palette": ["#56B4E9", "#E69F00", "#009E73", "#CC79A7", "#0072B2",
                                  "#D55E00", "#F0E442", "#e6ecf5"]}
        }),
        "high-contrast" | "highcontrast" => json!({
            "name": "high-contrast", "extends": "light",
            "colors": {"bg": "#ffffff", "bg2": "#ffffff", "card": "#ffffff", "ink": "#000000",
                       "muted": "#222222", "line": "#000000", "heading": "#000000",
                       "accent": "#0040c0", "accent2": "#0040c0", "link": "#0030a0",
                       "positive": "#006b2e", "negative": "#b00000"},
            "chrome": {"gradient": false, "background": "plain", "kpi_value": "ink",
                       "table_zebra": true, "shadow": "none"},
            "dark": {"colors": {"bg": "#000000", "bg2": "#000000", "card": "#000000",
                                "ink": "#ffffff", "muted": "#e0e0e0", "line": "#ffffff",
                                "heading": "#ffffff", "accent": "#ffd400", "accent2": "#ffd400",
                                "link": "#9ecbff"}}
        }),
        _ => return None,
    })
}

/// Is `name` a built-in theme (or alias)?
pub fn is_builtin(name: &str) -> bool {
    builtin_json(name).is_some()
}

// ───────────────────────── resolution ─────────────────────────────────────

/// Deep-merge `over` into `base` (objects merge key by key; anything else
/// replaces). `null` in `over` keeps the base value.
fn merge(base: &mut J, over: &J) {
    match (base, over) {
        (J::Object(b), J::Object(o)) => {
            for (k, v) in o {
                if v.is_null() {
                    continue;
                }
                match b.get_mut(k) {
                    Some(bv) if bv.is_object() && v.is_object() => merge(bv, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Expand `extends` chains of a theme JSON object (max depth 8).
fn expand(def: &J, depth: usize) -> Result<J, String> {
    if depth > 8 {
        return Err("theme `extends` chain is too deep (or cyclic)".into());
    }
    let parent = match def.get("extends") {
        None | Some(J::Null) => None,
        Some(J::String(p)) => Some(p.clone()),
        Some(_) => return Err("theme `extends` must be a theme name".into()),
    };
    let mut out = match parent {
        Some(p) => {
            let pj = builtin_json(&p)
                .ok_or_else(|| format!("unknown theme `{p}` in `extends`{}", did_you_mean(&p)))?;
            expand(&pj, depth + 1)?
        }
        None if def.get("name").and_then(|n| n.as_str()) == Some("light") => J::Object(Map::new()),
        // A JSON theme without `extends` builds on the default look.
        None => expand(&light_json(), depth + 1)?,
    };
    // The child's own dark block is merged over the parent's (not replacing it).
    merge(&mut out, def);
    if let J::Object(m) = &mut out {
        m.remove("extends");
    }
    Ok(out)
}

fn did_you_mean(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    let best = BUILTINS
        .iter()
        .map(|(b, _)| *b)
        .find(|b| b.starts_with(&n[..n.len().min(3)]) || n.contains(b));
    match best {
        Some(b) => format!(" (did you mean `{b}`?)"),
        None => format!(
            " (built-ins: {})",
            BUILTINS
                .iter()
                .map(|(b, _)| *b)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Resolve a theme spec: a built-in name, a stack `a+b` (`'acme+dark'` =
/// the light look of `acme` with a dark variant, following the viewer), or a
/// JSON object (leading `{`). An empty spec is the default (`light`).
pub fn resolve(spec: &str) -> Result<ThemeSet, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(default_set().clone());
    }
    let def: J = if spec.starts_with('{') {
        let v: J = serde_json::from_str(spec).map_err(|e| format!("theme JSON: {e}"))?;
        if !v.is_object() {
            return Err("theme JSON must be an object".into());
        }
        v
    } else {
        stack_json(spec)?
    };
    resolve_json(&def)
}

/// `a+b+…` → one theme definition.
fn stack_json(spec: &str) -> Result<J, String> {
    let parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err(format!("bad theme stack `{spec}`"));
    }
    let first = parts[0];
    let mut def = builtin_json(first)
        .ok_or_else(|| format!("unknown theme `{first}`{}", did_you_mean(first)))?;
    // A lone built-in name pins its mode: `'light'` stays light, `'dark'` dark.
    if parts.len() == 1 && first.eq_ignore_ascii_case("light") {
        def["mode"] = J::String("light".into());
    }
    for p in &parts[1..] {
        match p.to_ascii_lowercase().as_str() {
            // `x+dark`: x with a dark variant, following the viewer.
            "dark" => def["mode"] = J::String("auto".into()),
            "light" => {}
            _ => {
                let mut o = builtin_json(p)
                    .ok_or_else(|| format!("unknown theme `{p}`{}", did_you_mean(p)))?;
                if let J::Object(m) = &mut o {
                    m.remove("extends");
                    m.remove("name");
                }
                let mode = def.get("mode").cloned();
                merge(&mut def, &o);
                if let Some(m) = mode {
                    if o.get("mode").is_none() {
                        def["mode"] = m;
                    }
                }
            }
        }
    }
    def["name"] = J::String(parts.join("+").to_ascii_lowercase());
    Ok(def)
}

/// Resolve a theme definition object.
pub fn resolve_json(def: &J) -> Result<ThemeSet, String> {
    check_keys(def, "")?;
    let full = expand(def, 0)?;
    let name = full
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("custom")
        .to_string();
    let mode = match full.get("mode").and_then(|v| v.as_str()).unwrap_or("auto") {
        "auto" => Mode::Auto,
        "light" => Mode::Light,
        "dark" => Mode::Dark,
        m => return Err(format!("theme `mode` must be auto|light|dark, got `{m}`")),
    };
    let light = build(&full, &name, Variant::Light)?;
    let mut dark_def = full.clone();
    if let Some(d) = full.get("dark") {
        check_keys(d, "dark.")?;
        // The dark palette: the theme's own `dark.palette`, else the lifted
        // default palette when the theme kept the default palette.
        let custom_palette = full.get("palette") != Some(&json!(hexs(&DEFAULT_PALETTE)));
        let mut d = d.clone();
        if d.get("palette").is_none() && !custom_palette {
            d["palette"] = json!(hexs(&DEFAULT_PALETTE_DARK));
        }
        merge(&mut dark_def, &d);
    }
    let dark = build(&dark_def, &name, Variant::Dark)?;
    Ok(ThemeSet {
        name,
        mode,
        light,
        dark,
    })
}

const KEYS: &[&str] = &[
    "name",
    "extends",
    "mode",
    "palette",
    "sequential",
    "diverging",
    "colors",
    "chart",
    "font",
    "radius",
    "chrome",
    "brand",
    "locale",
    "dark",
];

/// Reject unknown top-level keys (typos) with a hint.
fn check_keys(def: &J, prefix: &str) -> Result<(), String> {
    if let J::Object(m) = def {
        for k in m.keys() {
            if !KEYS.contains(&k.as_str()) || (!prefix.is_empty() && k == "dark") {
                return Err(format!(
                    "unknown theme key `{prefix}{k}` (known: {})",
                    KEYS.join(", ")
                ));
            }
        }
    }
    Ok(())
}

fn build(full: &J, name: &str, variant: Variant) -> Result<ResolvedTheme, String> {
    let obj = |k: &str| full.get(k).cloned().unwrap_or(J::Object(Map::new()));
    let colors = obj("colors");
    let chart = obj("chart");
    let font = obj("font");
    let chrome = obj("chrome");
    let brand = obj("brand");
    let col = |o: &J, k: &str| -> Result<Rgb, String> {
        let v = o
            .get(k)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("theme colour `{k}` missing"))?;
        parse_color(v).ok_or_else(|| format!("theme colour `{k}`: `{v}` is not a #rrggbb colour"))
    };
    let opt_col = |o: &J, k: &str| -> Result<Option<Rgb>, String> {
        match o.get(k) {
            None | Some(J::Null) => Ok(None),
            Some(J::String(s)) => parse_color(s)
                .map(Some)
                .ok_or_else(|| format!("theme colour `{k}`: `{s}` is not a #rrggbb colour")),
            Some(v) => Err(format!("theme colour `{k}` must be a string, got {v}")),
        }
    };
    let list = |k: &str| -> Result<Vec<Rgb>, String> {
        match full.get(k) {
            None | Some(J::Null) => Ok(Vec::new()),
            Some(J::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_str()
                        .and_then(parse_color)
                        .ok_or_else(|| format!("theme `{k}`: `{v}` is not a #rrggbb colour"))
                })
                .collect(),
            Some(v) => Err(format!("theme `{k}` must be an array of colours, got {v}")),
        }
    };
    let palette = list("palette")?;
    if palette.is_empty() {
        return Err("theme `palette` must list at least one colour".into());
    }
    let seq = list("sequential")?;
    let sequential = match seq.as_slice() {
        [] => None,
        [a, b] => Some([*a, *b]),
        _ => return Err("theme `sequential` must be [low, high]".into()),
    };
    let div = list("diverging")?;
    let diverging = match div.as_slice() {
        [a, b, c] => [*a, *b, *c],
        _ => return Err("theme `diverging` must be [low, mid, high]".into()),
    };
    let text = |o: &J, k: &str| -> Option<String> {
        o.get(k).and_then(|v| v.as_str()).map(str::to_string)
    };
    let fam = |k: &str| -> Result<String, String> {
        let f = text(&font, k).ok_or_else(|| format!("theme `font.{k}` missing"))?;
        safe_font(&f)
            .then_some(f.clone())
            .ok_or_else(|| format!("theme `font.{k}`: `{f}` is not a plain font-family list"))
    };
    let pick = |o: &J, k: &str, allowed: &[&str]| -> Result<String, String> {
        let v = text(o, k).ok_or_else(|| format!("theme `chrome.{k}` missing"))?;
        allowed
            .contains(&v.as_str())
            .then_some(v.clone())
            .ok_or_else(|| {
                format!(
                    "theme `chrome.{k}` must be one of {}, got `{v}`",
                    allowed.join("|")
                )
            })
    };
    let flag = |o: &J, k: &str| -> Result<bool, String> {
        o.get(k)
            .and_then(|v| v.as_bool())
            .ok_or_else(|| format!("theme `chrome.{k}` must be true/false"))
    };
    let url = |k: &str| -> Result<Option<String>, String> {
        match text(&brand, k) {
            None => Ok(None),
            Some(u) if safe_url(&u) => Ok(Some(u)),
            Some(u) => Err(format!(
                "theme `brand.{k}`: `{u}` is not an allowed image URL"
            )),
        }
    };
    let radius = full
        .get("radius")
        .and_then(|v| v.as_f64())
        .filter(|r| (0.0..=40.0).contains(r))
        .ok_or("theme `radius` must be a number 0..40")?;
    let locale = text(full, "locale").filter(|s| !s.is_empty());
    if let Some(l) = &locale {
        if crate::format::Locale::from_tag(l).is_none() {
            return Err(format!(
                "unknown locale `{l}` (known: {})",
                crate::format::Locale::TAGS.join(", ")
            ));
        }
    }
    Ok(ResolvedTheme {
        name: name.to_string(),
        variant,
        palette,
        sequential,
        diverging,
        colors: Colors {
            bg: col(&colors, "bg")?,
            bg2: col(&colors, "bg2")?,
            card: col(&colors, "card")?,
            ink: col(&colors, "ink")?,
            muted: col(&colors, "muted")?,
            line: col(&colors, "line")?,
            heading: col(&colors, "heading")?,
            accent: col(&colors, "accent")?,
            accent2: col(&colors, "accent2")?,
            positive: col(&colors, "positive")?,
            negative: col(&colors, "negative")?,
            warning: col(&colors, "warning")?,
            info: col(&colors, "info")?,
            link: col(&colors, "link")?,
            other: col(&colors, "other")?,
        },
        chart: ChartColors {
            heading: col(&chart, "heading")?,
            note: col(&chart, "note")?,
            ink: col(&chart, "ink")?,
            muted: col(&chart, "muted")?,
            line: col(&chart, "line")?,
            error: col(&chart, "error")?,
            seq_low: opt_col(&chart, "seq_low")?,
        },
        font: Fonts {
            family: fam("family")?,
            display: fam("display")?,
        },
        radius,
        chrome: ChromeStyle {
            gradient: flag(&chrome, "gradient")?,
            background: pick(&chrome, "background", &["gradient", "plain"])?,
            kpi_value: pick(&chrome, "kpi_value", &["gradient", "accent", "ink"])?,
            table_zebra: flag(&chrome, "table_zebra")?,
            shadow: pick(&chrome, "shadow", &["md", "sm", "none"])?,
            negatives: pick(&chrome, "negatives", &["minus", "red", "parens"])?,
            zero: pick(&chrome, "zero", &["muted", "dash"])?,
        },
        brand: Brand {
            title: text(&brand, "title"),
            name: text(&brand, "name"),
            logo: url("logo")?,
            logo_dark: url("logo_dark")?,
            favicon: url("favicon")?,
            title_suffix: text(&brand, "title_suffix"),
            asof: None,
            stale_secs: None,
        },
        locale,
    })
}

/// A font-family list we can write into CSS unescaped.
fn safe_font(f: &str) -> bool {
    !f.is_empty()
        && f.len() <= 200
        && f.chars()
            .all(|c| c.is_ascii_alphanumeric() || " ,'\"-_.".contains(c))
}

/// An image URL allowed for logos/favicons: a relative path, `https:`, or a
/// `data:image/…` URI. Never `javascript:`, quotes, `<`, `>` or `(`/`)`.
pub fn safe_url(u: &str) -> bool {
    let t = u.trim();
    if t.is_empty()
        || t.len() > 512_000
        || t.chars()
            .any(|c| "\"'<>()\\`".contains(c) || c.is_control())
    {
        return false;
    }
    let low = t.to_ascii_lowercase();
    if low.starts_with("data:") {
        return low.starts_with("data:image/");
    }
    match low.find(':') {
        Some(i) if !low[..i].contains('/') => low.starts_with("https:") || low.starts_with("http:"),
        _ => true,
    }
}

/// Parse `#rgb` / `#rrggbb` / `rrggbb`.
pub fn parse_color(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    let n = |a: &str| u8::from_str_radix(a, 16).ok();
    match h.len() {
        6 if h.is_ascii() => Some((n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?)),
        3 if h.is_ascii() => {
            let d = |i: usize| n(&h[i..i + 1]).map(|v| v * 17);
            Some((d(0)?, d(1)?, d(2)?))
        }
        _ => None,
    }
}

/// `#rrggbb`.
pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// `rgba(r,g,b,a)` with a compact alpha.
fn rgba(c: Rgb, a: f64) -> String {
    let a = format!("{a:.3}");
    let a = a.trim_end_matches('0').trim_end_matches('.');
    let a = a.strip_prefix('0').unwrap_or(a);
    format!(
        "rgba({},{},{},{})",
        c.0,
        c.1,
        c.2,
        if a.is_empty() { "0" } else { a }
    )
}

/// `a` → `b` by `t` (0..1), per channel in sRGB.
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let l = |x: u8, y: u8| {
        (x as f64 + (y as f64 - x as f64) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// Relative luminance (WCAG).
pub fn luminance(c: Rgb) -> f64 {
    let ch = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.0) + 0.7152 * ch(c.1) + 0.0722 * ch(c.2)
}

/// WCAG contrast ratio.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// The cached default theme set (`light`, mode auto).
pub fn default_set() -> &'static ThemeSet {
    static D: OnceLock<ThemeSet> = OnceLock::new();
    D.get_or_init(|| resolve_json(&light_json()).expect("built-in light theme resolves"))
}

/// The default light variant (shared, cheap to clone).
pub fn default_light() -> Arc<ResolvedTheme> {
    static D: OnceLock<Arc<ResolvedTheme>> = OnceLock::new();
    D.get_or_init(|| Arc::new(default_set().light.clone()))
        .clone()
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        default_set().light.clone()
    }
}

// ───────────────────────── chart side ─────────────────────────────────────

impl ResolvedTheme {
    /// Is this the default light look?
    pub fn is_default_light(&self) -> bool {
        *self == default_set().light
    }

    /// Palette colour `i` (cycles).
    pub fn color(&self, i: usize) -> Rgb {
        self.palette[i % self.palette.len()]
    }

    /// The explicit sequential pair for this variant. A light-on-light pair
    /// (pale low end) inherited by a dark variant is dropped: the dark
    /// variant then ramps from its own dark low end to its brand.
    pub fn seq_pair(&self) -> Option<[Rgb; 2]> {
        self.sequential.filter(|[lo, _]| {
            self.variant == Variant::Light || luminance(*lo) <= luminance(self.colors.card) + 0.2
        })
    }

    /// Low end of the heatmap fill.
    pub fn seq_low(&self) -> Rgb {
        self.seq_pair()
            .map(|s| s[0])
            .or(self.chart.seq_low)
            .unwrap_or((0xed, 0xf1, 0xf7))
    }

    /// Low end of the calendar fill (a hair lighter in the default look).
    pub fn calendar_low(&self) -> Rgb {
        self.seq_pair()
            .map(|s| s[0])
            .or(self.chart.seq_low)
            .unwrap_or((0xeb, 0xf1, 0xf7))
    }

    /// High end of a sequential fill (`brand` unless the theme sets one).
    pub fn seq_high(&self, brand: Rgb) -> Rgb {
        self.seq_pair().map(|s| s[1]).unwrap_or(brand)
    }

    /// The diverging ramp `[low, mid, high]` for this variant: the theme's
    /// colours, with the midpoint of a light ramp moved onto the card colour
    /// family in the dark variant (a pale grey midpoint would glow).
    pub fn diverging_ramp(&self) -> [Rgb; 3] {
        let [lo, mid, hi] = self.diverging;
        match self.variant {
            Variant::Light => [lo, mid, hi],
            Variant::Dark if luminance(mid) > 0.5 => {
                [lo, mix(self.colors.card, self.colors.ink, 0.17), hi]
            }
            Variant::Dark => [lo, mid, hi],
        }
    }

    /// Chart text: tick labels, axis titles, legend titles (the muted ink).
    pub fn chart_text(&self) -> Rgb {
        self.colors.muted
    }

    /// Chart text that names data: legend entries, data labels.
    pub fn chart_label(&self) -> Rgb {
        mix(self.colors.ink, self.colors.muted, 0.35)
    }

    /// The chart grid: a hairline a step quieter than the card border.
    pub fn chart_grid(&self) -> Rgb {
        mix(self.colors.line, self.colors.card, 0.25)
    }

    /// Text on a filled cell (heatmap / calendar labels): ink or card,
    /// whichever reads better on `fill`.
    pub fn text_on(&self, fill: Rgb) -> Rgb {
        let (dark, light) = if luminance(self.colors.ink) < luminance(self.colors.card) {
            (self.colors.ink, self.colors.card)
        } else {
            (self.colors.card, self.colors.ink)
        };
        // Prefer the dark text unless the light one is clearly better: a
        // mid-tone cell reads best with dark text.
        if contrast(light, fill) > contrast(dark, fill) * 1.15 {
            light
        } else {
            dark
        }
    }

    /// The font-family list as an SVG attribute value (double quotes become
    /// single quotes; the value is escaped by the writer).
    pub fn svg_font(&self) -> String {
        self.font.family.replace('"', "'")
    }

    /// Colours the chart theme paints directly in this variant — already
    /// right for it, never remapped by [`ResolvedTheme::finish_svg`].
    fn owns(&self, c: Rgb) -> bool {
        let k = &self.colors;
        [
            k.ink,
            k.muted,
            k.line,
            k.card,
            k.heading,
            self.chart_text(),
            self.chart_label(),
            self.chart_grid(),
            self.diverging_ramp()[1],
        ]
        .contains(&c)
    }

    /// Post-process a rendered chart SVG for this variant. The light variant
    /// is returned unchanged; the dark variant maps the renderer's neutral
    /// (grey) text/grid/axis/reference colours onto the dark ink ramp —
    /// white → card, black → ink — leaving data colours (the palette, any
    /// colour with real chroma) untouched.
    ///
    /// Colours of the DataZoo slot palette that chart code still writes
    /// directly (contract plots: model colours, accents) are swapped for the
    /// same *slot* of this theme's palette, so "ols = slot 0" holds in every
    /// theme.
    ///
    /// Text the plotting engine writes in its generic `sans-serif` (geom
    /// labels, legends) or the extension's own `system-ui,sans-serif` takes
    /// the theme's font family.
    pub fn finish_svg(&self, svg: String) -> String {
        let svg = self.with_font(svg);
        let dz = self.palette.as_slice() == DZ_PALETTE.as_slice();
        if self.variant == Variant::Light && dz {
            return svg;
        }
        remap_neutrals(&svg, &|c| {
            if self.owns(c) {
                return None;
            }
            if !dz {
                if let Some(i) = DZ_PALETTE.iter().position(|&p| p == c) {
                    return Some(self.color(i));
                }
            }
            if self.variant == Variant::Dark {
                self.neutral(c)
            } else {
                None
            }
        })
    }

    /// Swap the generic font families for the theme's.
    fn with_font(&self, svg: String) -> String {
        let font = format!(
            "font-family=\"{}\"",
            crate::format::escape_xml(&self.svg_font())
        );
        let mut svg = svg;
        for generic in [
            "font-family=\"sans-serif\"",
            "font-family=\"system-ui,sans-serif\"",
        ] {
            if svg.contains(generic) {
                svg = svg.replace(generic, &font);
            }
        }
        svg
    }

    /// The dark-ramp image of a neutral colour (`None` = keep).
    fn neutral(&self, c: Rgb) -> Option<Rgb> {
        let (mx, mn) = (c.0.max(c.1).max(c.2), c.0.min(c.1).min(c.2));
        // The theme's own colours (series, the ends of a sequential fill) are
        // already dark-mode colours, however grey they look.
        let own = self.palette.contains(&c)
            || c == self.seq_low()
            || c == self.calendar_low()
            || self.sequential.is_some_and(|s| s.contains(&c))
            || self.owns(c);
        if mx - mn > 32 || own {
            return None;
        }
        // Lightness 0..1 → ink..card (inverted).
        let l = (c.0 as f64 * 0.299 + c.1 as f64 * 0.587 + c.2 as f64 * 0.114) / 255.0;
        Some(mix(self.colors.ink, self.colors.card, l))
    }

    /// JSON for the browser: the CSS block plus what the page needs.
    pub fn to_json(&self) -> J {
        json!({
            "css": self.css_vars(),
            "variant": self.variant.as_str(),
            "card": hex(self.colors.card),
            "bg": hex(self.colors.bg),
            "brand": {
                "title": self.brand.title, "name": self.brand.name,
                "logo": if self.variant == Variant::Dark { self.brand.logo_dark.clone().or(self.brand.logo.clone()) } else { self.brand.logo.clone() },
                "favicon": self.brand.favicon, "title_suffix": self.brand.title_suffix,
            },
            "locale": self.locale,
        })
    }
}

/// Rewrite every `fill="#rrggbb"` / `stroke="#rrggbb"` (also `#RRGGBB` and
/// `rgb(r,g,b)`) whose colour `f` maps.
fn remap_neutrals(svg: &str, f: &dyn Fn(Rgb) -> Option<Rgb>) -> String {
    const KEYS: [&str; 3] = ["fill=\"", "stroke=\"", "color=\""];
    let mut out = String::with_capacity(svg.len());
    // Next occurrence of each key at or after `pos` (cached, so a key that is
    // rare or absent is not rescanned from every match: linear, not quadratic).
    let find_from = |k: &str, from: usize| svg[from..].find(k).map(|i| from + i);
    let mut next: Vec<Option<usize>> = KEYS.iter().map(|k| find_from(k, 0)).collect();
    let mut pos = 0;
    loop {
        for (n, k) in next.iter_mut().zip(KEYS) {
            if matches!(*n, Some(i) if i < pos) {
                *n = find_from(k, pos);
            }
        }
        let Some((i, klen)) = next
            .iter()
            .zip(KEYS)
            .filter_map(|(n, k)| n.map(|i| (i, k.len())))
            .min()
        else {
            out.push_str(&svg[pos..]);
            return out;
        };
        let start = i + klen;
        out.push_str(&svg[pos..start]);
        let end = svg[start..].find('"').map_or(svg.len(), |e| start + e);
        let val = &svg[start..end];
        let parsed = if val.starts_with('#') && val.len() == 7 {
            parse_color(val)
        } else if let Some(inner) = val.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
            let p: Vec<u8> = inner
                .split(',')
                .filter_map(|x| x.trim().parse().ok())
                .collect();
            (p.len() == 3).then(|| (p[0], p[1], p[2]))
        } else {
            None
        };
        match parsed.and_then(f) {
            Some(c) => out.push_str(&hex(c)),
            None => out.push_str(val),
        }
        pos = end;
    }
}

// ───────────────────────── CSS tokens ─────────────────────────────────────

impl ResolvedTheme {
    /// The `--ax-*` tokens (name without the prefix, CSS value).
    pub fn css_tokens(&self) -> Vec<(String, String)> {
        let mut out = self.derived_tokens();
        // Structure tokens every theme emits (fonts, radii, brand sizes).
        out.extend([
            ("font-body".to_string(), self.font.family.clone()),
            ("font-display".to_string(), self.font.display.clone()),
            ("radius".to_string(), format!("{}px", fmt_num(self.radius))),
            (
                "radius-sm".to_string(),
                format!("{}px", fmt_num((self.radius * 2.0 / 3.0).round())),
            ),
            ("logo-h".to_string(), "40px".to_string()),
        ]);
        out
    }

    /// Tokens computed from the semantic colours (every non-default theme).
    fn derived_tokens(&self) -> Vec<(String, String)> {
        let c = &self.colors;
        let dark = luminance(c.card) < 0.25;
        // Accent paints reference --ax-accent so `?primary=` can recolour them.
        let grad = |_a: Rgb, _b: Rgb| {
            if self.chrome.gradient {
                "linear-gradient(135deg,var(--ax-accent),var(--ax-accent2))".to_string()
            } else {
                "var(--ax-accent)".to_string()
            }
        };
        let on_accent = if contrast((255, 255, 255), c.accent) >= contrast((0, 0, 0), c.accent) {
            (255, 255, 255)
        } else {
            (17, 17, 17)
        };
        let ink_a = |a: f64| rgba(c.ink, a);
        let acc_a = |a: f64| rgba(c.accent, a);
        let shadow = match (self.chrome.shadow.as_str(), dark) {
            ("none", _) => ("none".to_string(), "none".to_string()),
            ("sm", false) => (
                "0 1px 2px rgba(16,24,40,.06)".to_string(),
                "0 4px 14px rgba(16,24,40,.12)".to_string(),
            ),
            (_, false) => (
                format!(
                    "0 1px 2px {},0 18px 40px -22px {}",
                    rgba(mix(c.ink, c.accent, 0.3), 0.05),
                    rgba(mix(c.ink, c.accent, 0.3), 0.26)
                ),
                format!(
                    "0 2px 8px {},0 30px 60px -24px {}",
                    rgba(mix(c.ink, c.accent, 0.3), 0.08),
                    rgba(mix(c.ink, c.accent, 0.3), 0.4)
                ),
            ),
            (_, true) => (
                "0 1px 2px rgba(0,0,0,.3),0 18px 40px -22px rgba(0,0,0,.6)".to_string(),
                "0 2px 8px rgba(0,0,0,.35),0 30px 60px -24px rgba(0,0,0,.75)".to_string(),
            ),
        };
        let (bg_image, pattern) = if self.chrome.background == "gradient" {
            (
                format!(
                    "radial-gradient(900px 520px at 92% -12%,{},transparent 58%),radial-gradient(760px 520px at 3% 108%,{},transparent 60%),linear-gradient(180deg,{} 0%,{} 100%)",
                    acc_a(0.16),
                    rgba(c.accent2, 0.13),
                    hex(c.bg2),
                    hex(c.bg)
                ),
                format!("radial-gradient({} 1px,transparent 1px)", ink_a(0.05)),
            )
        } else {
            ("none".to_string(), "none".to_string())
        };
        let field = mix(c.card, c.ink, if dark { 0.06 } else { 0.0 });
        let soft = mix(c.card, c.ink, 0.05);
        let (kpi_bg, kpi_ink) = match self.chrome.kpi_value.as_str() {
            "gradient" => (grad(c.accent, c.accent2), "transparent".to_string()),
            "accent" => ("none".to_string(), hex(c.accent)),
            _ => ("none".to_string(), hex(c.ink)),
        };
        let badge = |base: Rgb| {
            (
                hex(mix(c.card, base, if dark { 0.22 } else { 0.12 })),
                hex(mix(base, c.ink, 0.25)),
            )
        };
        let (gb, gi) = badge(c.positive);
        let (wb, wi) = badge(c.warning);
        let (bb, bi) = badge(c.negative);
        let (nb, ni) = badge(c.muted);
        let chevron = format!(
            "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%23{:02x}{:02x}{:02x}' stroke-width='3' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E\")",
            c.muted.0, c.muted.1, c.muted.2
        );
        let brand = self.palette[0];
        let t = |k: &str, v: String| (k.to_string(), v);
        vec![
            t("bg", hex(c.bg)),
            t("bg-image", bg_image),
            t("bg-pattern", pattern),
            t("canvas", "var(--ax-bg-image),var(--ax-bg)".into()),
            t("card", hex(c.card)),
            t("ink", hex(c.ink)),
            t("muted", hex(c.muted)),
            t("line", hex(c.line)),
            t("heading", hex(c.heading)),
            t("header-ink", hex(c.heading)),
            t("accent", hex(c.accent)),
            t("accent2", hex(c.accent2)),
            t("accent-ink", hex(on_accent)),
            t("accent-bar", grad(c.accent, c.accent2)),
            t(
                "accent-glow",
                if self.chrome.gradient {
                    format!("0 2px 7px {}", acc_a(0.45))
                } else {
                    "none".into()
                },
            ),
            t("shadow", shadow.0),
            t("shadow-hover", shadow.1),
            t("card-border-hover", acc_a(0.42)),
            t("kpi-bg", kpi_bg),
            t("kpi-ink", kpi_ink),
            t("positive", hex(c.positive)),
            t("negative", hex(c.negative)),
            t("warning", hex(c.warning)),
            t("info", hex(c.info)),
            t("btn-bg", grad(c.accent, c.accent2)),
            t("btn-ink", hex(on_accent)),
            t(
                "btn-shadow",
                if self.chrome.shadow == "none" {
                    "none".into()
                } else {
                    format!("0 6px 16px -6px {}", acc_a(0.5))
                },
            ),
            t(
                "btn-shadow-hover",
                if self.chrome.shadow == "none" {
                    "none".into()
                } else {
                    format!("0 10px 22px -8px {}", acc_a(0.6))
                },
            ),
            t("btn2-bg", hex(field)),
            t("btn2-hover", hex(soft)),
            t(
                "btn2-shadow",
                if dark {
                    "none".into()
                } else {
                    "0 1px 2px rgba(16,24,40,.05)".into()
                },
            ),
            t("input-bg", hex(field)),
            t("input-border-hover", hex(mix(c.line, c.ink, 0.25))),
            t("input-border-hover2", hex(mix(c.line, c.ink, 0.2))),
            t(
                "input-shadow",
                if dark {
                    "none".into()
                } else {
                    "0 1px 2px rgba(16,24,40,.04)".into()
                },
            ),
            t("focus-ring", format!("0 0 0 3px {}", acc_a(0.22))),
            t("chevron", chevron),
            t(
                "pop-shadow",
                if dark {
                    "0 10px 30px rgba(0,0,0,.5)".into()
                } else {
                    "0 10px 30px rgba(16,24,40,.16)".into()
                },
            ),
            t("colf-border", hex(c.line)),
            t("slider-track", hex(mix(c.line, c.card, 0.2))),
            t("slider-fill", hex(mix(c.accent, c.card, 0.35))),
            t("slider-label", hex(c.muted)),
            t("slider-thumb", hex(c.accent)),
            t("slider-thumb-edge", hex(c.card)),
            t("slider-thumb-ring", format!("0 0 0 1px {}", hex(c.muted))),
            t("veil", rgba(c.bg, 0.5)),
            t("spinner-track", hex(c.line)),
            t("spinner-head", hex(c.accent)),
            t(
                "callout-bg",
                if self.chrome.gradient {
                    format!(
                        "linear-gradient(135deg,{},{})",
                        acc_a(0.13),
                        rgba(c.accent2, 0.13)
                    )
                } else {
                    acc_a(0.08)
                },
            ),
            t("callout-border", acc_a(0.22)),
            t("callout-bar", grad(c.accent, c.accent2)),
            t("md-ink", hex(mix(c.ink, c.muted, 0.35))),
            t("md-heading", hex(c.heading)),
            t("link", hex(c.link)),
            t("code-bg", ink_a(0.07)),
            t("pre-bg", hex(soft)),
            t("quote-bar", hex(mix(c.line, c.ink, 0.25))),
            t("quote-ink", hex(c.muted)),
            t("th-ink", hex(c.muted)),
            t("th-bg", hex(c.card)),
            t(
                "zebra",
                if self.chrome.table_zebra {
                    ink_a(0.035)
                } else {
                    "transparent".into()
                },
            ),
            // table number style, read by the browser table (not colours)
            t("table-negatives", self.chrome.negatives.clone()),
            t("table-zero", self.chrome.zero.clone()),
            t("row-hover", hex(mix(c.card, c.ink, 0.04))),
            t("row-hover-tint", acc_a(0.06)),
            t("row-focus", acc_a(0.16)),
            t("row-focus-kbd", acc_a(0.26)),
            t("databar-a", acc_a(0.2)),
            t("databar-b", rgba(c.accent2, 0.16)),
            t("badge-good-bg", gb),
            t("badge-good-ink", gi),
            t("badge-warn-bg", wb),
            t("badge-warn-ink", wi),
            t("badge-bad-bg", bb),
            t("badge-bad-ink", bi),
            t("badge-neutral-bg", nb),
            t("badge-neutral-ink", ni),
            t(
                "scale-lo",
                hex(mix(c.card, c.positive, if dark { 0.45 } else { 0.7 })),
            ),
            t(
                "scale-mid",
                hex(mix(c.card, c.warning, if dark { 0.45 } else { 0.55 })),
            ),
            t(
                "scale-hi",
                hex(mix(c.card, c.negative, if dark { 0.45 } else { 0.55 })),
            ),
            t("spark", hex(brand)),
            t("spark-dot", hex(brand)),
            t("err-ink", hex(c.negative)),
            t("err-bg", hex(mix(c.card, c.negative, 0.08))),
            t("err-border", hex(mix(c.card, c.negative, 0.3))),
            t("tab-ink", hex(c.muted)),
            t("tab-active", "var(--ax-accent)".into()),
            t("tab-hover-bg", acc_a(0.08)),
            t(
                "subtab-bg",
                hex(if dark { mix(c.bg, c.card, 0.5) } else { c.bg }),
            ),
            t("pill-hover", acc_a(0.1)),
            t("nav-bg", hex(c.card)),
            t("nav-border", hex(c.line)),
            t(
                "nav-shadow",
                if dark {
                    "none".into()
                } else {
                    "0 1px 3px rgba(16,24,40,.06)".into()
                },
            ),
            t("nav-ink", hex(c.ink)),
            t("nav-active-bg", hex(c.accent)),
            t("nav-active-ink", hex(on_accent)),
            t("tip-bg", rgba(c.card, 0.97)),
            t("tip-ink", hex(mix(c.ink, c.muted, 0.35))),
            t("tip-head", hex(c.heading)),
            t("tip-border", ink_a(0.1)),
            t(
                "tip-shadow",
                if dark {
                    "0 6px 26px rgba(0,0,0,.5)".into()
                } else {
                    "0 6px 26px rgba(15,23,42,.16)".into()
                },
            ),
            t("cross", ink_a(0.4)),
            t("legend-off", hex(mix(c.card, c.muted, 0.45))),
            t("tool-bg", rgba(c.card, 0.92)),
            t("tool-bg-soft", rgba(c.card, 0.85)),
            t("tool-ink", hex(c.muted)),
            t("tool-border", ink_a(0.14)),
            t("tool-hover-ink", hex(c.heading)),
            t("tool-hover-border", ink_a(0.3)),
            t("tool-hover-bg", hex(c.card)),
            t("tool-on", hex(brand)),
            t("tool-on-ink", "#fff".into()),
            t("zoom-track", hex(soft)),
            t("zoom-track-border", ink_a(0.08)),
            t("zoom-fill", rgba(brand, 0.16)),
            t("zoom-fill-border", rgba(brand, 0.5)),
            t("zoom-fill-on", rgba(brand, 0.26)),
            t("zoom-handle", hex(field)),
            t("zoom-handle-border", rgba(brand, 0.75)),
            t("zoom-handle-shadow", "0 1px 2px rgba(15,23,42,.16)".into()),
            t("zoom-grip", rgba(brand, 0.55)),
            t("brush-border", rgba(brand, 0.9)),
            t("brush-fill", rgba(brand, 0.12)),
            t("vmap-border", ink_a(0.12)),
            t(
                "vmap-track",
                format!(
                    "linear-gradient(90deg,{},{})",
                    hex(self.seq_low()),
                    hex(self.seq_high(brand))
                ),
            ),
            t("vmap-fill", rgba(c.card, 0.12)),
            t("vmap-fill-border", rgba(c.ink, 0.7)),
            t("vmap-handle-border", rgba(brand, 0.8)),
            t("vmap-handle-shadow", "0 1px 2px rgba(15,23,42,.18)".into()),
            t(
                "modal-backdrop",
                if dark {
                    "rgba(0,0,0,.55)".into()
                } else {
                    "rgba(15,23,42,.42)".into()
                },
            ),
            t(
                "modal-shadow",
                if dark {
                    "0 20px 60px rgba(0,0,0,.6)".into()
                } else {
                    "0 20px 60px rgba(15,23,42,.35)".into()
                },
            ),
            t("modal-x", hex(c.muted)),
            t("modal-x-hover", hex(c.heading)),
            t("export-bg", hex(c.card)),
        ]
    }

    /// The token block: `:root{--ax-bg:…;…}` plus the legacy aliases
    /// (`--bg --card --ink --muted --line --accent --accent2`).
    pub fn css_vars(&self) -> String {
        let mut s = String::from(":root{");
        for (k, v) in self.css_tokens() {
            s.push_str(&format!("--ax-{k}:{v};"));
        }
        s.push_str("--bg:var(--ax-bg);--card:var(--ax-card);--ink:var(--ax-ink);--muted:var(--ax-muted);--line:var(--ax-line);--accent:var(--ax-accent);--accent2:var(--ax-accent2)}");
        s
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The shared dashboard-canvas stylesheet (`web/theme-base.css`).
pub const BASE_CSS: &str = include_str!("../web/theme-base.css");

/// A complete `<style>` body for a static page: the theme tokens + the base
/// stylesheet.
pub fn page_css(t: &ResolvedTheme) -> String {
    format!("{}\n{}", t.css_vars(), BASE_CSS)
}

// ───────────────────────── dashboard chrome ───────────────────────────────

/// Dashboard-level settings collected from the chrome directives of a script
/// (`::THEME`, `::LOCALE`, `::DASHBOARD_TITLE`, `::BRAND`, `::LOGO`,
/// `::FAVICON`). The last directive of each kind wins.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashboardChrome {
    pub theme: Option<String>,
    pub locale: Option<String>,
    pub title: Option<String>,
    pub brand: Option<String>,
    pub logo: Option<String>,
    pub favicon: Option<String>,
    /// `::ASOF` value (a timestamp) and its optional `stale=…` option.
    pub asof: Option<String>,
    pub stale: Option<String>,
}

impl DashboardChrome {
    /// [`apply`](Self::apply) a directive's first row (its cells in SELECT
    /// order): the first cell is the value; an `::ASOF` also reads a
    /// `'stale=24h'` cell.
    pub fn apply_row(&mut self, role: crate::Role, cells: Vec<Option<String>>) -> bool {
        if role == crate::Role::AsOf {
            self.stale = cells
                .iter()
                .skip(1)
                .flatten()
                .find(|c| crate::freshness::is_stale_option(c))
                .cloned();
        }
        self.apply(role, cells.into_iter().next().flatten())
    }

    /// Record a directive's first value. Returns `true` when `role` is a
    /// chrome directive this struct handles.
    pub fn apply(&mut self, role: crate::Role, value: Option<String>) -> bool {
        use crate::Role as R;
        let v = value
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        match role {
            R::Theme => self.theme = v,
            R::Locale => self.locale = v,
            R::DashboardTitle => self.title = v,
            R::Brand => self.brand = v,
            R::Logo => self.logo = v.filter(|u| safe_url(u)),
            R::Favicon => self.favicon = v.filter(|u| safe_url(u)),
            R::AsOf => self.asof = v,
            _ => return false,
        }
        true
    }

    /// Resolve the theme (`::THEME`, else `fallback` — e.g. `--theme`), then
    /// apply the branding and locale directives over it.
    pub fn resolve(&self, fallback: Option<&str>) -> Result<ThemeSet, String> {
        let spec = self.theme.as_deref().or(fallback).unwrap_or("");
        let mut set = resolve(spec)?;
        if let Some(l) = &self.locale {
            if crate::format::Locale::from_tag(l).is_none() {
                return Err(format!(
                    "unknown locale `{l}` (known: {})",
                    crate::format::Locale::TAGS.join(", ")
                ));
            }
        }
        for t in [&mut set.light, &mut set.dark] {
            if let Some(v) = &self.title {
                t.brand.title = Some(v.clone());
            }
            if let Some(v) = &self.brand {
                t.brand.name = Some(v.clone());
            }
            if let Some(v) = &self.logo {
                t.brand.logo = Some(v.clone());
                t.brand.logo_dark = None;
            }
            if let Some(v) = &self.favicon {
                t.brand.favicon = Some(v.clone());
            }
            if let Some(v) = &self.locale {
                t.locale = Some(v.clone());
            }
            t.brand.asof = self
                .asof
                .as_deref()
                .and_then(crate::freshness::parse_timestamp);
            t.brand.stale_secs = self
                .stale
                .as_deref()
                .and_then(crate::freshness::parse_stale);
        }
        Ok(set)
    }
}

/// The HTML of the dashboard header (title/brand/logo), or "" when the
/// dashboard sets none.
pub fn header_html(t: &ResolvedTheme) -> String {
    let b = &t.brand;
    if b.title.is_none() && b.name.is_none() && b.logo.is_none() && b.asof.is_none() {
        return String::new();
    }
    let esc = crate::format::escape_xml;
    let logo = match (t.variant, &b.logo_dark, &b.logo) {
        (Variant::Dark, Some(d), _) => Some(d),
        (_, _, l) => l.as_ref(),
    };
    let mut s = String::from("<div class=\"dp-head\">");
    if let Some(l) = logo {
        s.push_str(&format!(
            "<img class=\"dp-logo\" src=\"{}\" alt=\"\">",
            esc(l)
        ));
    }
    s.push_str("<div class=\"dp-head-text\">");
    if let Some(n) = &b.name {
        s.push_str(&format!("<div class=\"dp-brand\">{}</div>", esc(n)));
    }
    if let Some(ti) = &b.title {
        s.push_str(&format!("<h1 class=\"dp-title\">{}</h1>", esc(ti)));
    }
    s.push_str("</div>");
    s.push_str(&asof_html(t));
    s.push_str("</div>");
    s
}

/// The `::ASOF` freshness chip (or ""): rendered now, kept current in the
/// viewer's clock and locale by the page script from `data-asof` (epoch ms)
/// and `data-stale` (seconds).
pub fn asof_html(t: &ResolvedTheme) -> String {
    let (Some(at), Some(now)) = (t.brand.asof, crate::freshness::now_epoch()) else {
        return String::new();
    };
    let l = t
        .locale
        .as_deref()
        .and_then(crate::format::Locale::from_tag)
        .unwrap_or_default();
    let (text, stale) = crate::freshness::chip_text(at, t.brand.stale_secs, now, &l);
    format!(
        "<div class=\"dp-asof{}\" role=\"status\" data-asof=\"{}\"{}>{}</div>",
        if stale { " stale" } else { "" },
        at * 1000,
        t.brand
            .stale_secs
            .map(|s| format!(" data-stale=\"{s}\""))
            .unwrap_or_default(),
        crate::format::escape_xml(&text)
    )
}

/// The mandatory credit footer (HTML).
pub fn credit_html() -> String {
    format!(
        "<div class=\"dp-credit\">Powered by <a href=\"{CREDIT_URL}\" target=\"_blank\" rel=\"noopener noreferrer\">DataZoo GmbH</a></div>"
    )
}

/// `<title>` text: the dashboard title (+ suffix), else `fallback`.
pub fn page_title(t: &ResolvedTheme, fallback: &str) -> String {
    match &t.brand.title {
        Some(ti) => format!("{ti}{}", t.brand.title_suffix.as_deref().unwrap_or("")),
        None => fallback.to_string(),
    }
}

/// `<link rel=icon>` for the favicon (or "").
pub fn favicon_html(t: &ResolvedTheme) -> String {
    t.brand
        .favicon
        .as_ref()
        .map(|f| {
            format!(
                "<link rel=\"icon\" href=\"{}\">",
                crate::format::escape_xml(f)
            )
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_resolve() {
        for (n, _) in BUILTINS {
            let s = resolve(n).unwrap_or_else(|e| panic!("{n}: {e}"));
            assert!(!s.light.palette.is_empty());
            assert_eq!(s.dark.variant, Variant::Dark);
        }
        assert_eq!(resolve("").unwrap().mode, Mode::Auto);
        assert_eq!(resolve("light").unwrap().mode, Mode::Light);
        assert_eq!(resolve("dark").unwrap().mode, Mode::Dark);
        assert_eq!(resolve("datazoo+dark").unwrap().mode, Mode::Auto);
        assert_eq!(
            resolve("colorblind").unwrap().light.palette[0],
            OKABE_ITO[0]
        );
        assert_eq!(resolve("print+dark").unwrap().mode, Mode::Auto);
    }

    #[test]
    fn default_is_the_dz_palette() {
        let d = default_set();
        assert_eq!(d.light.palette, DEFAULT_PALETTE.to_vec());
        assert!(d.light.is_default_light());
        assert_eq!(d.dark.palette, DEFAULT_PALETTE_DARK.to_vec());
    }

    #[test]
    fn json_theme_extends_and_validates() {
        let s = resolve(
            r##"{"name":"acme","extends":"light","palette":["#0b5fff","#e11d48"],
            "colors":{"accent":"#0b5fff"},"brand":{"name":"ACME"},"locale":"de-DE",
            "dark":{"colors":{"bg":"#0f1729"}}}"##,
        )
        .unwrap();
        assert_eq!(s.name, "acme");
        assert_eq!(
            s.light.palette,
            vec![(0x0b, 0x5f, 0xff), (0xe1, 0x1d, 0x48)]
        );
        // custom palette is kept in dark
        assert_eq!(s.dark.palette, s.light.palette);
        assert_eq!(s.dark.colors.bg, (0x0f, 0x17, 0x29));
        assert_eq!(s.light.brand.name.as_deref(), Some("ACME"));
        assert_eq!(s.light.locale.as_deref(), Some("de-DE"));
        assert!(resolve(r#"{"colours":{}}"#)
            .unwrap_err()
            .contains("unknown theme key"));
        assert!(resolve(r#"{"colors":{"bg":"red"}}"#).is_err());
        assert!(resolve(r#"{"font":{"family":"x;}body{color:red"}}"#).is_err());
        assert!(resolve(r#"{"brand":{"logo":"javascript:alert(1)"}}"#).is_err());
        assert!(resolve(r#"{"brand":{"logo":"x\" onerror=\"y"}}"#).is_err());
        assert!(resolve("nope").unwrap_err().contains("unknown theme"));
        assert!(resolve(r#"{"locale":"xx-YY"}"#).is_err());
    }

    #[test]
    fn urls() {
        for ok in [
            "logo.svg",
            "assets/acme.svg",
            "https://x.y/l.png",
            "data:image/svg+xml;base64,AAA",
        ] {
            assert!(safe_url(ok), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "data:text/html,x",
            "a\"b",
            "url(x)",
            "vbscript:x",
        ] {
            assert!(!safe_url(bad), "{bad}");
        }
    }

    #[test]
    fn light_css_has_no_unknown_tokens() {
        let css = default_set().light.css_vars();
        assert!(css.starts_with(":root{--ax-bg:#f4f5f7;"));
        let dark = default_set().dark.css_vars();
        // same token names in both
        let names = |s: &str| -> Vec<String> {
            s.split(";--ax-")
                .skip(1)
                .map(|t| t.split(':').next().unwrap().to_string())
                .collect()
        };
        assert_eq!(names(&css), names(&dark));
    }

    #[test]
    fn dark_remaps_neutrals_only() {
        let t = &default_set().dark;
        let svg = r##"<svg><text fill="#000000">a</text><line stroke="#EBEBEB"/><rect fill="#7fa7cf"/><rect fill="#e8335d"/></svg>"##;
        let out = t.finish_svg(svg.to_string());
        assert!(
            out.contains(&format!("fill=\"{}\"", hex(t.colors.ink))),
            "{out}"
        );
        assert!(out.contains("fill=\"#7fa7cf\""));
        // a DataZoo slot colour becomes the same slot of the dark palette
        assert!(
            out.contains(&format!("fill=\"{}\"", hex(t.color(2)))),
            "{out}"
        );
        assert!(!out.contains("#EBEBEB"));
        // light: neutrals untouched, DataZoo slot colours mapped to the default palette
        let l = &default_set().light;
        let out = l.finish_svg(svg.to_string());
        assert!(
            out.contains("fill=\"#000000\"") && out.contains("#EBEBEB"),
            "{out}"
        );
        assert!(
            out.contains(&format!("fill=\"{}\"", hex(l.color(2)))),
            "{out}"
        );
    }

    #[test]
    fn dark_keeps_its_own_sequential_colours() {
        // A greyish dark heatmap low end must not be flipped like a light-theme neutral.
        let t = &default_set().dark;
        let low = hex(t.seq_low());
        let svg = format!(r#"<svg><rect fill="{low}"/></svg>"#);
        assert!(t.finish_svg(svg).contains(&low));
    }

    #[test]
    fn chart_tokens_survive_the_dark_pass() {
        // Colours the chart theme paints directly must not collide with the
        // renderer's neutrals or the DataZoo slot colours (the dark pass
        // would remap them).
        let engine: [Rgb; 6] = [
            (0x32, 0x32, 0x32),
            (0x4d, 0x4d, 0x4d),
            (0xeb, 0xeb, 0xeb),
            (0xff, 0xff, 0xff),
            (0, 0, 0),
            (0x78, 0x80, 0x8c),
        ];
        for (n, _) in BUILTINS {
            let t = resolve(n).unwrap().dark;
            for c in [
                t.chart_text(),
                t.chart_label(),
                t.chart_grid(),
                t.diverging_ramp()[1],
            ] {
                assert!(
                    !engine.contains(&c) && !DZ_PALETTE.contains(&c),
                    "{n}: {}",
                    hex(c)
                );
                let svg = format!(r#"<svg><text fill="{}">a</text></svg>"#, hex(c));
                assert!(t.finish_svg(svg).contains(&hex(c)), "{n}: {} kept", hex(c));
            }
            // Label text on cells: whichever of ink/card reads better.
            assert_eq!(t.text_on(t.colors.ink), t.colors.card);
            assert_eq!(t.text_on(t.colors.card), t.colors.ink);
        }
        // A light sequential pair inherited by a dark variant is not used.
        let s = resolve(r##"{"name":"x","sequential":["#e0e7ff","#3730a3"]}"##).unwrap();
        assert_eq!(s.light.seq_low(), (0xe0, 0xe7, 0xff));
        assert!(
            luminance(s.dark.seq_low()) < 0.1,
            "{}",
            hex(s.dark.seq_low())
        );
    }

    #[test]
    fn contrast_of_builtins() {
        for (n, _) in BUILTINS {
            let s = resolve(n).unwrap();
            for t in [&s.light, &s.dark] {
                assert!(
                    contrast(t.colors.ink, t.colors.card) >= 4.5,
                    "{n}/{:?}: ink on card",
                    t.variant
                );
            }
        }
    }
}

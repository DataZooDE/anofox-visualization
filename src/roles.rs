//! The role registry — the **single source of truth** for the `::ROLE`
//! vocabulary.
//!
//! Every consumer derives from [`REGISTRY`]: [`crate::parse_role`] (token →
//! [`Role`]), the SQL rewriter's "is this `::TOKEN` a role?" check, the wasm
//! `plan()` output ([`Role::token`]), the static renderers' / linter's "does
//! this statement draw a panel?" check ([`Role::renders`]), `dashboard --roles`
//! ([`text`]), the browser's role sets ([`roles_json`], exported to wasm as
//! `roles_json()`), and the role table in `docs/DOCS.md` ([`markdown_table`]; a
//! test keeps the doc in sync).
//!
//! Each entry lists a canonical token (what [`Role::token`] emits — the wire
//! format the browser matches on), its aliases, the [`Role`] it parses to, a
//! category, whether a statement carrying it renders a visible panel, and a
//! one-line description.

use crate::{DownloadFmt, InputKind, Kind, MetricFmt, Role, TextSize};

/// One registered role.
#[derive(Debug)]
pub struct RoleSpec {
    /// Canonical token (the form [`Role::token`] returns).
    pub token: &'static str,
    /// Accepted alternative spellings.
    pub aliases: &'static [&'static str],
    pub role: Role,
    /// `encoding` | `chart` | `annotation` | `modifier` | `kpi` | `table` |
    /// `input` | `layout` | `chrome`.
    pub category: &'static str,
    /// `true` when a statement carrying this role draws a visible panel;
    /// `false` for directives (layout, inputs, downloads, reload, chrome) that
    /// static renderers skip.
    pub renders: bool,
    pub summary: &'static str,
}

/// Back-compat name for the old catalog entry type.
pub type RoleDoc = RoleSpec;

const fn e(
    token: &'static str,
    aliases: &'static [&'static str],
    role: Role,
    category: &'static str,
    renders: bool,
    summary: &'static str,
) -> RoleSpec {
    RoleSpec {
        token,
        aliases,
        role,
        category,
        renders,
        summary,
    }
}

use Role as R;

/// The registry. Order = documentation order (grouped by category).
#[rustfmt::skip]
pub static REGISTRY: &[RoleSpec] = &[
    // ── encoding ───────────────────────────────────────────────────────────
    e("XAXIS", &["X"], R::X, "encoding", true, "x position"),
    e("YAXIS", &["Y"], R::Y, "encoding", true, "y position (a heatmap's second axis)"),
    e("CATEGORY", &["SERIES", "COLOR", "COLOUR"], R::Category, "encoding", true,
      "grouping / colour series (discrete)"),
    e("LABEL", &[], R::Label, "encoding", true,
      "section heading when alone; chart title / per-mark or per-feature label otherwise"),
    e("TITLE", &["HEADING"], R::Title, "encoding", true, "a title bar above one panel"),
    e("OPEN", &[], R::Open, "encoding", true, "candlestick open price"),
    e("HIGH", &[], R::High, "encoding", true, "candlestick high price"),
    e("LOW", &[], R::Low, "encoding", true, "candlestick low price"),
    e("SIZE", &[], R::Size, "encoding", true, "bubble size for a scatter (maps a measure to point area)"),
    // ── charts (cast the measure column) ───────────────────────────────────
    e("BARCHART", &["BAR"], R::Value(Kind::Bar), "chart", true,
      "bar chart (dodged by CATEGORY)"),
    e("BARCHART_STACKED", &["BAR_STACKED", "STACKED_BAR"], R::Value(Kind::BarStacked), "chart", true,
      "stacked bars (by CATEGORY)"),
    e("BARCHART_PERCENT", &["BAR_PERCENT"], R::Value(Kind::BarPercent), "chart", true,
      "dodged bars, percent y-axis"),
    e("BARCHART_STACKED_PERCENT", &["BAR_STACKED_PERCENT"], R::Value(Kind::BarStackedPercent),
      "chart", true, "bars normalised to 100% per x"),
    e("LINECHART", &["LINE"], R::Value(Kind::Line), "chart", true, "line chart"),
    e("LINECHART_PERCENT", &["LINE_PERCENT"], R::Value(Kind::LinePercent), "chart", true,
      "line chart, percent y-axis"),
    e("STEP", &["STEPLINE", "STEP_LINE"], R::Value(Kind::Step), "chart", true, "step line"),
    e("SMOOTH", &["TRENDLINE", "TREND_LINE"], R::Value(Kind::Smooth), "chart", true,
      "scatter + LOESS trend line"),
    e("AREACHART", &["AREA"], R::Value(Kind::Area), "chart", true, "area chart"),
    e("AREACHART_STACKED", &["AREA_STACKED", "STACKED_AREA"], R::Value(Kind::AreaStacked), "chart",
      true, "stacked areas (by CATEGORY)"),
    e("SCATTER", &["POINT", "SCATTERCHART", "BUBBLE"], R::Value(Kind::Point), "chart", true,
      "scatter; add a ::SIZE column for a bubble chart"),
    e("JITTER", &["JITTERCHART", "STRIP"], R::Value(Kind::Jitter), "chart", true,
      "jittered scatter (reveals overlapping points)"),
    e("PIE", &["PIECHART", "PIECHART_PERCENT"], R::Value(Kind::Pie), "chart", true,
      "pie — slices by CATEGORY, sized by the measure"),
    e("DONUTCHART", &["DONUT", "DONUTCHART_PERCENT"], R::Value(Kind::Donut), "chart", true,
      "donut (pie with a hole)"),
    e("GAUGE", &["GAUGE_PERCENT"], R::Value(Kind::Gauge), "chart", true,
      "value as an arc toward ::RANGE 'min,max' (zones: ::COLORS, ::LABELS)"),
    e("RADAR", &["SPIDER"], R::Value(Kind::Radar), "chart", true,
      "radar / spider chart — axes from XAXIS, one polygon per CATEGORY"),
    e("HISTOGRAM", &["HIST"], R::Value(Kind::Histogram), "chart", true, "histogram of the measure"),
    e("DENSITY", &["KDE"], R::Value(Kind::Density), "chart", true,
      "kernel density curve (one per CATEGORY)"),
    e("BOXPLOT", &["BOX_PLOT"], R::Value(Kind::Boxplot), "chart", true,
      "box plot — XAXIS groups, measure on y (raw rows)"),
    e("VIOLIN", &["VIOLINPLOT"], R::Value(Kind::Violin), "chart", true,
      "violin plot — XAXIS groups, measure on y (raw rows)"),
    e("QQ", &["QQPLOT"], R::Value(Kind::QQ), "chart", true, "normal quantile-quantile plot"),
    e("HEATMAP", &["TILE", "TILES"], R::Value(Kind::Heatmap), "chart", true,
      "tiles at XAXIS×YAXIS coloured by the measure"),
    e("CALENDAR", &["CALENDAR_HEATMAP", "CAL_HEATMAP"], R::Value(Kind::Calendar), "chart", true,
      "calendar heatmap (date XAXIS, ≤ 50 years)"),
    e("CANDLESTICK", &["CANDLE", "OHLC"], R::Value(Kind::Candlestick), "chart", true,
      "OHLC candlesticks: XAXIS + ::OPEN/::HIGH/::LOW, close as the measure"),
    e("SPARKLINE", &["SPARK"], R::Value(Kind::Sparkline), "chart", true,
      "minimal trend line (no axes); a list() column in a table"),
    e("MAP", &["GEOMETRY", "GEO", "CHOROPLETH"], R::Geometry, "chart", true,
      "WKT-geometry map, coloured by a measure"),
    e("BASEMAP", &["MAPBASE", "BACKDROP"], R::Basemap, "chart", true,
      "grey WKT backdrop layer under a ::MAP"),
    // ── annotations on a chart ─────────────────────────────────────────────
    e("REFLINE", &["TARGET", "GOAL", "YLINE"], R::RefLine, "annotation", true,
      "horizontal reference line per distinct value"),
    e("XLINE", &[], R::VLine, "annotation", true, "vertical reference line at an x"),
    e("BAND_LOWER", &["BANDLOWER"], R::BandLower, "annotation", true,
      "lower edge of a shaded band around a line"),
    e("BAND_UPPER", &["BANDUPPER"], R::BandUpper, "annotation", true,
      "upper edge of a shaded band"),
    e("MARKAREA", &["MARK_AREA", "SHADE"], R::MarkArea, "annotation", true,
      "shade the x-region [min, max] of this column"),
    e("DATALABELS", &["DATALABEL", "VALUELABELS", "SHOWLABELS"], R::DataLabels, "annotation", true,
      "draw the value on each mark (value = font size)"),
    // ── chart modifiers ────────────────────────────────────────────────────
    e("FLIP", &["COORD_FLIP", "HORIZONTAL"], R::Flip, "modifier", true,
      "swap the axes (horizontal bars)"),
    e("YFORMAT", &["YAXISFORMAT", "YUNIT", "YCURRENCY"], R::YFormat, "modifier", true,
      "y-axis tick format: '€', '$', 'percent', 'comma', ' kg'…"),
    e("XFORMAT", &["XAXISFORMAT", "XUNIT", "XCURRENCY"], R::XFormat, "modifier", true,
      "x-axis tick format (continuous x), like ::YFORMAT"),
    e("ALPHA", &["OPACITY"], R::Alpha, "modifier", true, "map layer opacity 0..1"),
    e("RANGE", &[], R::Range, "modifier", true, "gauge domain 'min,max' (default 0,100)"),
    e("COLORS", &["COLOURS"], R::GaugeColors, "modifier", true,
      "gauge zone colours, comma-separated hex"),
    e("LABELS", &[], R::GaugeLabels, "modifier", true,
      "gauge zone labels, comma-separated (drawn at each zone)"),
    // ── KPIs & text ────────────────────────────────────────────────────────
    e("METRIC", &["KPI", "BIGNUMBER"], R::Metric(MetricFmt::Plain), "kpi", true,
      "big-number KPI (add ::LABEL for a caption)"),
    e("MONEY", &["DOLLAR", "CURRENCY"], R::Metric(MetricFmt::Money), "kpi", true,
      "currency KPI / table column format"),
    e("PERCENT", &["PCT"], R::Metric(MetricFmt::Percent), "kpi", true,
      "percent KPI / table column format"),
    e("COMPACT", &[], R::Metric(MetricFmt::Compact), "kpi", true,
      "compact-number KPI / table column format (1.2K)"),
    e("DELTA", &["COMPARE", "PREVIOUS"], R::Delta, "kpi", true,
      "comparison value → trend arrow + % on a KPI"),
    e("TEXT_SMALL", &[], R::Text(TextSize::Small), "kpi", true, "small text card"),
    e("TEXT_MEDIUM", &[], R::Text(TextSize::Medium), "kpi", true, "medium text card"),
    e("TEXT_LARGE", &[], R::Text(TextSize::Large), "kpi", true, "large text card"),
    e("MARKDOWN", &["MD", "TEXTBOX", "RICHTEXT"], R::Markdown, "kpi", true,
      "a Markdown box (browser)"),
    // ── tables ─────────────────────────────────────────────────────────────
    e("TABLE", &["GRID"], R::Table, "table", true,
      "the whole result as a table (one marker per panel)"),
    e("PAGED", &["TABLE_PAGED", "PAGINATED"], R::PagedTable, "table", true,
      "SQL-paginated table for large/remote data"),
    e("TREND", &[], R::Trend, "table", true, "▲/▼ arrow in a table cell"),
    e("COLORSCALE", &["COLOURSCALE", "HEAT", "GRADIENT"], R::ColorScale, "table", true,
      "heatmap-colour a table column's cells"),
    e("BADGE", &["STATUS", "PILL"], R::Badge, "table", true, "render a table column as status pills"),
    e("PLAIN", &["NOBAR"], R::Plain, "table", true, "a plain table column (no in-cell bar)"),
    e("DOWNLOAD_CSV", &[], R::Download(DownloadFmt::Csv), "table", false, "CSV export button"),
    e("DOWNLOAD_XLSX", &["DOWNLOAD_EXCEL"], R::Download(DownloadFmt::Xlsx), "table", false,
      "Excel export button"),
    e("DOWNLOAD_PDF", &[], R::Download(DownloadFmt::Pdf), "table", false,
      "print-to-PDF button"),
    // ── inputs (the column name becomes getvariable('<name>')) ─────────────
    e("DROPDOWN", &["OPTIONS", "SELECT_INPUT"], R::Input(InputKind::Dropdown), "input", false,
      "single-select → a DuckDB variable"),
    e("MULTISELECT", &["MULTI"], R::Input(InputKind::Multiselect), "input", false,
      "multi-select → a list variable"),
    e("NUMBER", &["SLIDER", "NUMERIC"], R::Input(InputKind::Number), "input", false,
      "numeric input (value = default)"),
    e("DATE", &["DATEPICKER"], R::Input(InputKind::Date), "input", false,
      "date picker (value = default)"),
    e("TEXT", &["SEARCH", "STRING"], R::Input(InputKind::Text), "input", false,
      "free-text input (value = default)"),
    e("DATERANGE", &["DATE_RANGE"], R::Input(InputKind::DateRange), "input", false,
      "two date columns → from/to variables"),
    e("HINT", &[], R::Hint, "input", true, "per-option hint next to a dropdown option"),
    // ── layout ─────────────────────────────────────────────────────────────
    e("COLUMNS", &["COLS"], R::Columns, "layout", false, "default panels per row"),
    e("SPAN", &["COL", "WIDTH"], R::Span, "layout", false, "next panel's width (of 12)"),
    e("HEIGHT", &["TALL"], R::Height, "layout", false, "next panel's height in px"),
    e("GROUP", &["BOX", "ROW"], R::GroupStart, "layout", false, "open a box; ::ENDGROUP closes it"),
    e("ENDGROUP", &["ENDBOX", "ENDROW"], R::GroupEnd, "layout", false, "close the current box"),
    e("TAB", &["PAGE"], R::Tab, "layout", false, "start a tab/page"),
    e("SUBTAB", &["SUB_TAB"], R::SubTab, "layout", false, "a nested tab inside a ::TAB"),
    e("PLACEHOLDER", &[], R::Placeholder, "layout", false, "an empty grid cell"),
    // ── chrome ─────────────────────────────────────────────────────────────
    e("RELOAD", &["REFRESH"], R::Reload, "chrome", false, "auto-refresh interval (seconds)"),
    e("HEADER_IMAGE", &["HEADERIMAGE"], R::HeaderImage, "chrome", false, "banner image URL"),
    e("FOOTER_LINK", &["FOOTERLINK"], R::FooterLink, "chrome", false, "link at the bottom"),
];

/// Role tokens that are **also DuckDB type names** (`x::DATE` is a real cast).
/// They count as roles only in a query statement whose SELECT list has another,
/// unambiguous role cast — or when every cast in a plain top-level `SELECT` is
/// one of these (a standalone input like `SELECT DATE '2024-01-01' AS d::DATE`).
/// They are never roles inside DDL/DML, and inside a `::TABLE`/`::PAGED`/
/// `::DOWNLOAD_*`/`::DATERANGE` panel they stay real casts.
pub const SQL_TYPE_TOKENS: &[&str] = &["DATE", "TEXT", "STRING", "NUMERIC", "MAP", "GEOMETRY"];

/// Is `token` (case-insensitive) one of [`SQL_TYPE_TOKENS`]?
pub fn is_sql_type_token(token: &str) -> bool {
    SQL_TYPE_TOKENS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(token.trim()))
}

/// Look up a token or alias (case-insensitive).
pub fn lookup(token: &str) -> Option<&'static RoleSpec> {
    let t = token.trim();
    REGISTRY.iter().find(|s| {
        s.token.eq_ignore_ascii_case(t) || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(t))
    })
}

/// The registry entry for a parsed role.
pub fn spec_of(role: Role) -> &'static RoleSpec {
    REGISTRY
        .iter()
        .find(|s| s.role == role)
        .expect("every Role variant is registered (tested)")
}

impl Role {
    /// The canonical token (e.g. `Role::Value(Kind::Bar)` → `"BARCHART"`).
    pub fn token(&self) -> &'static str {
        spec_of(*self).token
    }
    /// Does a statement carrying this role draw a visible panel? `false` for
    /// layout directives, inputs, downloads, reload and header/footer chrome.
    pub fn renders(&self) -> bool {
        spec_of(*self).renders
    }
    /// Registry category (`chart`, `input`, `layout`, …).
    pub fn category(&self) -> &'static str {
        spec_of(*self).category
    }
}

/// True when any role in a planned panel is a non-rendering directive — static
/// renderers (CLI, `serve`, headless SVG) and the linter skip such statements.
pub fn is_directive_panel(roles: &[(usize, Role)]) -> bool {
    roles.iter().any(|(_, r)| !r.renders())
}

/// Canonical tokens by category (aliases exist but prefer these).
pub fn catalog() -> &'static [RoleSpec] {
    REGISTRY
}

/// Human-readable, grouped listing (for `dashboard --roles`).
pub fn text() -> String {
    let mut out = String::new();
    let mut cat = "";
    for d in REGISTRY {
        if d.category != cat {
            cat = d.category;
            out.push_str(&format!("\n[{cat}]\n"));
        }
        let alias = if d.aliases.is_empty() {
            String::new()
        } else {
            format!("  (also {})", d.aliases.join(", "))
        };
        out.push_str(&format!("  ::{:<26} {}{alias}\n", d.token, d.summary));
    }
    out
}

/// The role table for `docs/DOCS.md` (generated; a test checks the doc embeds
/// exactly this text between the `roles-table` markers).
pub fn markdown_table() -> String {
    let mut out = String::from(
        "| Cast | Aliases | Category | Meaning |\n|------|---------|----------|---------|\n",
    );
    for d in REGISTRY {
        let aliases = d
            .aliases
            .iter()
            .map(|a| format!("`::{a}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "| `::{}` | {} | {} | {} |\n",
            d.token,
            aliases,
            d.category,
            d.summary.replace('|', "\\|")
        ));
    }
    out
}

/// JSON export of the registry for the browser (wasm `roles_json()`):
/// `{"roles":[{token, aliases, category, renders, summary}], "sets":{…}}`.
/// `sets` are the derived token lists `web/app.js` matches on — `inputs`,
/// `metrics`, `directives` (layout statements), `table_formats` (per-column
/// table formatting roles) and `text_sizes`.
pub fn roles_json() -> String {
    let roles: Vec<serde_json::Value> = REGISTRY
        .iter()
        .map(|d| {
            serde_json::json!({
                "token": d.token,
                "aliases": d.aliases,
                "category": d.category,
                "renders": d.renders,
                "summary": d.summary,
            })
        })
        .collect();
    let tokens = |f: &dyn Fn(Role) -> bool| -> Vec<&'static str> {
        REGISTRY
            .iter()
            .filter(|d| f(d.role))
            .map(|d| d.token)
            .collect()
    };
    let sets = serde_json::json!({
        "inputs": tokens(&|r| matches!(r, Role::Input(_))),
        "metrics": tokens(&|r| matches!(r, Role::Metric(_))),
        "directives": tokens(&|r| matches!(
            r,
            Role::Columns | Role::GroupStart | Role::GroupEnd | Role::Span | Role::Height
                | Role::Tab | Role::SubTab | Role::Placeholder
        )),
        "table_formats": tokens(&|r| is_table_format(r)),
        "text_sizes": tokens(&|r| matches!(r, Role::Text(_))),
    });
    serde_json::json!({ "roles": roles, "sets": sets }).to_string()
}

/// Per-column formatting roles honoured inside a `::TABLE`/`::PAGED` panel.
pub fn is_table_format(r: Role) -> bool {
    matches!(
        r,
        Role::Metric(_)
            | Role::Trend
            | Role::ColorScale
            | Role::Badge
            | Role::Value(Kind::Sparkline)
            | Role::Plain
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_role;

    #[test]
    fn every_token_and_alias_parses_to_its_role() {
        for d in REGISTRY {
            assert_eq!(parse_role(d.token), Some(d.role), "::{}", d.token);
            for a in d.aliases {
                assert_eq!(parse_role(a), Some(d.role), "alias ::{a}");
                assert_eq!(parse_role(&a.to_lowercase()), Some(d.role), "alias ::{a}");
            }
        }
    }

    #[test]
    fn tokens_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for d in REGISTRY {
            for t in std::iter::once(&d.token).chain(d.aliases.iter()) {
                assert!(seen.insert(*t), "duplicate token ::{t}");
            }
        }
    }

    #[test]
    fn canonical_token_round_trips() {
        // Role → token → Role (the wire format the browser sees).
        for d in REGISTRY {
            assert_eq!(d.role.token(), d.token);
            assert_eq!(parse_role(d.role.token()), Some(d.role));
        }
    }

    #[test]
    fn json_export_is_valid_and_complete() {
        let v: serde_json::Value = serde_json::from_str(&roles_json()).unwrap();
        assert_eq!(v["roles"].as_array().unwrap().len(), REGISTRY.len());
        let metrics = v["sets"]["metrics"].as_array().unwrap();
        assert_eq!(metrics.len(), 4);
        let dirs: Vec<&str> = v["sets"]["directives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        for t in [
            "COLUMNS",
            "GROUP",
            "ENDGROUP",
            "SPAN",
            "HEIGHT",
            "TAB",
            "SUBTAB",
            "PLACEHOLDER",
        ] {
            assert!(dirs.contains(&t), "{t}");
        }
    }

    #[test]
    fn previously_drifted_tokens_are_registered() {
        for t in [
            "YAXISFORMAT",
            "YUNIT",
            "XAXISFORMAT",
            "XUNIT",
            "AREACHART_STACKED",
            "JITTER",
            "OPEN",
            "HIGH",
            "LOW",
            "SIZE",
            "DATALABELS",
            "MARKAREA",
            "FLIP",
            "ALPHA",
            "BASEMAP",
            "HINT",
            "XFORMAT",
            "BUBBLE",
        ] {
            assert!(lookup(t).is_some(), "::{t}");
        }
        // BUBBLE is a chart (a scatter sized by ::SIZE), not the size column.
        assert_eq!(parse_role("BUBBLE"), Some(Role::Value(Kind::Point)));
    }

    /// Every `Role` value. The `match` below has no wildcard, so adding a
    /// variant without listing it here (and registering it) fails to compile.
    fn all_roles() -> Vec<Role> {
        use crate::{DownloadFmt as D, InputKind as I, Kind as K, MetricFmt as M, TextSize as T};
        let kinds = [
            K::Bar,
            K::BarStacked,
            K::BarPercent,
            K::BarStackedPercent,
            K::Line,
            K::LinePercent,
            K::Step,
            K::Smooth,
            K::Area,
            K::AreaStacked,
            K::Point,
            K::Pie,
            K::Donut,
            K::Histogram,
            K::Boxplot,
            K::Violin,
            K::Density,
            K::QQ,
            K::Heatmap,
            K::Sparkline,
            K::Gauge,
            K::Calendar,
            K::Jitter,
            K::Candlestick,
            K::Radar,
        ];
        let mut v: Vec<Role> = kinds.into_iter().map(Role::Value).collect();
        v.extend(
            [
                I::Dropdown,
                I::Number,
                I::Date,
                I::Text,
                I::Multiselect,
                I::DateRange,
            ]
            .map(Role::Input),
        );
        v.extend([M::Plain, M::Money, M::Percent, M::Compact].map(Role::Metric));
        v.extend([T::Small, T::Medium, T::Large].map(Role::Text));
        v.extend([D::Csv, D::Xlsx, D::Pdf].map(Role::Download));
        v.extend([
            Role::X,
            Role::Y,
            Role::Category,
            Role::Label,
            Role::Title,
            Role::Columns,
            Role::GroupStart,
            Role::GroupEnd,
            Role::Span,
            Role::Height,
            Role::Table,
            Role::PagedTable,
            Role::Delta,
            Role::RefLine,
            Role::VLine,
            Role::BandLower,
            Role::BandUpper,
            Role::Trend,
            Role::ColorScale,
            Role::Badge,
            Role::Plain,
            Role::Hint,
            Role::Placeholder,
            Role::HeaderImage,
            Role::FooterLink,
            Role::Reload,
            Role::Range,
            Role::GaugeLabels,
            Role::GaugeColors,
            Role::Geometry,
            Role::Basemap,
            Role::Flip,
            Role::Alpha,
            Role::Tab,
            Role::SubTab,
            Role::Size,
            Role::YFormat,
            Role::XFormat,
            Role::DataLabels,
            Role::MarkArea,
            Role::Markdown,
            Role::Open,
            Role::High,
            Role::Low,
        ]);
        for r in &v {
            // Exhaustiveness guard (no `_` arm).
            match r {
                Role::X
                | Role::Y
                | Role::Category
                | Role::Label
                | Role::Title
                | Role::Value(_)
                | Role::Input(_)
                | Role::Columns
                | Role::GroupStart
                | Role::GroupEnd
                | Role::Span
                | Role::Height
                | Role::Table
                | Role::PagedTable
                | Role::Metric(_)
                | Role::Delta
                | Role::RefLine
                | Role::VLine
                | Role::BandLower
                | Role::BandUpper
                | Role::Trend
                | Role::ColorScale
                | Role::Badge
                | Role::Plain
                | Role::Hint
                | Role::Text(_)
                | Role::Placeholder
                | Role::HeaderImage
                | Role::FooterLink
                | Role::Download(_)
                | Role::Reload
                | Role::Range
                | Role::GaugeLabels
                | Role::GaugeColors
                | Role::Geometry
                | Role::Basemap
                | Role::Flip
                | Role::Alpha
                | Role::Tab
                | Role::SubTab
                | Role::Size
                | Role::YFormat
                | Role::XFormat
                | Role::DataLabels
                | Role::MarkArea
                | Role::Markdown
                | Role::Open
                | Role::High
                | Role::Low => {}
            }
        }
        v
    }

    #[test]
    fn every_role_variant_is_registered_once() {
        for r in all_roles() {
            let n = REGISTRY.iter().filter(|s| s.role == r).count();
            assert_eq!(n, 1, "{r:?} registered {n} times");
        }
        assert_eq!(all_roles().len(), REGISTRY.len());
    }
}

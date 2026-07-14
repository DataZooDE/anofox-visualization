//! The authoritative `::ROLE` vocabulary, grouped and documented.
//!
//! This is the single source an author (or a prompt) should draw from — so a
//! generated dashboard never invents a token that doesn't exist (the way
//! `::CONTOUR` slipped into an early README). A test asserts every token here
//! actually parses via [`crate::parse_role`], so the catalog can't drift from
//! the engine.

/// One documented role: its canonical token, category, and a one-line summary.
pub struct RoleDoc {
    pub token: &'static str,
    pub category: &'static str,
    pub summary: &'static str,
}

const fn r(token: &'static str, category: &'static str, summary: &'static str) -> RoleDoc {
    RoleDoc { token, category, summary }
}

/// Canonical tokens by category (aliases exist but prefer these).
pub fn catalog() -> &'static [RoleDoc] {
    CATALOG
}

static CATALOG: &[RoleDoc] = &[
        // encoding
        r("XAXIS", "encoding", "x position"),
        r("YAXIS", "encoding", "y position (heatmap's second axis)"),
        r("CATEGORY", "encoding", "grouping / colour series (alias ::COLOR)"),
        r("TITLE", "encoding", "a title bar above one panel"),
        r("LABEL", "encoding", "section heading alone; per-mark label on a chart"),
        // charts (cast the measure column)
        r("BARCHART", "chart", "bar chart"),
        r("BARCHART_STACKED", "chart", "stacked bars (needs a CATEGORY)"),
        r("BARCHART_PERCENT", "chart", "dodged bars, percent y-axis"),
        r("BARCHART_STACKED_PERCENT", "chart", "bars normalised to 100% per x"),
        r("LINECHART", "chart", "line chart"),
        r("LINECHART_PERCENT", "chart", "line chart, percent y-axis"),
        r("STEP", "chart", "step line"),
        r("SMOOTH", "chart", "smoothed trend line"),
        r("AREACHART", "chart", "area chart"),
        r("SCATTER", "chart", "scatter (alias ::POINT)"),
        r("BUBBLE", "chart", "scatter sized by a SIZE column"),
        r("PIE", "chart", "pie — slices by CATEGORY, sized by measure"),
        r("DONUTCHART", "chart", "donut (pie with a hole)"),
        r("GAUGE", "chart", "value as an arc toward ::RANGE 'min,max'"),
        r("RADAR", "chart", "radar / spider chart"),
        r("HISTOGRAM", "chart", "histogram of the measure"),
        r("DENSITY", "chart", "kernel density curve"),
        r("BOXPLOT", "chart", "box plot — x groups, y measure"),
        r("VIOLIN", "chart", "violin plot"),
        r("HEATMAP", "chart", "tiles at XAXIS×YAXIS coloured by measure"),
        r("CALENDAR", "chart", "calendar heatmap"),
        r("CANDLESTICK", "chart", "OHLC candlesticks"),
        r("OHLC", "chart", "OHLC bars"),
        r("QQ", "chart", "quantile-quantile plot"),
        r("SPARKLINE", "chart", "mini line from a list() column"),
        r("MAP", "chart", "WKT-geometry choropleth"),
        // annotations on a chart
        r("REFLINE", "annotation", "horizontal reference line at the value (alias ::YLINE)"),
        r("XLINE", "annotation", "vertical reference line at an x"),
        r("BAND_LOWER", "annotation", "lower edge of a shaded band around a line"),
        r("BAND_UPPER", "annotation", "upper edge of a shaded band"),
        // KPIs & text
        r("METRIC", "kpi", "big-number KPI (add ::LABEL for a caption)"),
        r("MONEY", "kpi", "currency KPI"),
        r("PERCENT", "kpi", "percent KPI"),
        r("COMPACT", "kpi", "compact-number KPI (1.2K)"),
        r("DELTA", "kpi", "comparison value → trend arrow on a KPI"),
        r("TEXT_LARGE", "kpi", "a text card (also _MEDIUM / _SMALL)"),
        r("MARKDOWN", "kpi", "a Markdown box"),
        // tables
        r("TABLE", "table", "the whole result as a table (one marker per panel)"),
        r("PAGED", "table", "SQL-paginated table for large/remote data"),
        r("COLORSCALE", "table", "heatmap-colour a table column's cells"),
        r("BADGE", "table", "render a table column as status pills"),
        r("TREND", "table", "▲/▼ arrow in a table cell"),
        r("PLAIN", "table", "a plain table column (no in-cell bar)"),
        r("DOWNLOAD_CSV", "table", "an export button (also _XLSX / _PDF)"),
        // inputs (become getvariable('<column name>'))
        r("DROPDOWN", "input", "single-select → a DuckDB variable"),
        r("MULTISELECT", "input", "multi-select → a list (searchable past 50)"),
        r("NUMBER", "input", "numeric input / slider"),
        r("DATE", "input", "a date picker"),
        r("TEXT", "input", "a free-text input"),
        r("DATERANGE", "input", "two date columns → from/to pickers"),
        // layout & chrome
        r("COLUMNS", "layout", "grid column count for the dashboard"),
        r("COL", "layout", "next panel's width (of 12; alias ::SPAN)"),
        r("HEIGHT", "layout", "next panel's height in px"),
        r("GROUP", "layout", "open a box; ::ENDGROUP closes it"),
        r("ENDGROUP", "layout", "close the current ::GROUP box"),
        r("TAB", "layout", "a page/tab (alias ::PAGE)"),
        r("SUBTAB", "layout", "a nested tab inside a ::TAB"),
        r("PLACEHOLDER", "layout", "an empty grid cell"),
        r("RELOAD", "layout", "auto-refresh interval (seconds)"),
        // gauge/format modifiers
        r("RANGE", "modifier", "min,max domain for a ::GAUGE"),
        r("COLORS", "modifier", "colour stops (gauge zones / palette)"),
        r("YFORMAT", "modifier", "y-axis tick format, e.g. '€' (also ::XFORMAT)"),
];

/// Human-readable, grouped listing (for `dashboard --roles`).
pub fn text() -> String {
    let mut out = String::new();
    let mut cat = "";
    for d in catalog() {
        if d.category != cat {
            cat = d.category;
            out.push_str(&format!("\n[{cat}]\n"));
        }
        out.push_str(&format!("  ::{:<26} {}\n", d.token, d.summary));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_catalog_token_parses() {
        // Guarantees the catalog never advertises a token the engine rejects.
        for d in super::catalog() {
            assert!(
                crate::parse_role(d.token).is_some(),
                "catalog token ::{} does not parse — remove it or fix parse_role",
                d.token
            );
        }
    }
}

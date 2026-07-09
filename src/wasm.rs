//! wasm-bindgen surface for the browser dashboard builder. DuckDB-Wasm runs the
//! SQL in-page; these functions plan the annotated script and render each panel
//! to SVG — so the whole pipeline is client-side, no server, no DuckDB
//! extension. Build with: `wasm-pack build --features wasm`.

use crate::{parse_role, render, sql, Kind, Role};
use wasm_bindgen::prelude::*;

/// Plan a dashboard script into JSON:
/// `[{ "setup": bool, "sql": string, "roles": [[colIdx, "ROLE"], …] }]`.
/// The caller runs each `sql` through DuckDB-Wasm (setup statements for effect,
/// panels with `-json`) and passes the rows back to [`render_panel`].
#[wasm_bindgen]
pub fn plan(script: &str) -> String {
    let arr: Vec<serde_json::Value> = sql::plan(script)
        .iter()
        .map(|p| {
            serde_json::json!({
                "setup": p.setup,
                "sql": p.sql,
                "roles": p.roles.iter().map(|(i, r)| serde_json::json!([i, role_str(r)])).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
}

/// Render one panel to SVG. `rows_json` = `[{ "c0": …, "c1": … }, …]` (a panel
/// query's `-json` result); `roles_json` = the panel's `roles` from [`plan`].
#[wasm_bindgen]
pub fn render_panel(rows_json: &str, roles_json: &str, width: u32, height: u32) -> String {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_str(rows_json).unwrap_or_default();
    let roles: Vec<(usize, Role)> = serde_json::from_str::<Vec<(usize, String)>>(roles_json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(i, s)| parse_role(&s).map(|r| (i, r)))
        .collect();
    let cols = sql::columns_from_rows(&rows, &roles);
    render(&cols, width, height).unwrap_or_else(|e| format!("<pre>{e}</pre>"))
}

fn role_str(r: &Role) -> &'static str {
    use crate::InputKind as IK;
    match r {
        Role::X => "XAXIS",
        Role::Y => "YAXIS",
        Role::Category => "CATEGORY",
        Role::Label => "LABEL",
        Role::Title => "TITLE",
        Role::Value(Kind::Bar) => "BARCHART",
        Role::Value(Kind::BarStacked) => "BARCHART_STACKED",
        Role::Value(Kind::BarPercent) => "BARCHART_PERCENT",
        Role::Value(Kind::BarStackedPercent) => "BARCHART_STACKED_PERCENT",
        Role::Value(Kind::Line) => "LINECHART",
        Role::Value(Kind::LinePercent) => "LINECHART_PERCENT",
        Role::Value(Kind::Area) => "AREACHART",
        Role::Value(Kind::Point) => "SCATTER",
        Role::Value(Kind::Pie) => "PIE",
        Role::Value(Kind::Donut) => "DONUTCHART",
        Role::Value(Kind::Gauge) => "GAUGE",
        Role::Value(Kind::Histogram) => "HISTOGRAM",
        Role::Value(Kind::Boxplot) => "BOXPLOT",
        Role::Value(Kind::Heatmap) => "HEATMAP",
        Role::Value(Kind::Sparkline) => "SPARKLINE",
        Role::RefLine => "REFLINE",
        Role::VLine => "XLINE",
        Role::BandLower => "BAND_LOWER",
        Role::BandUpper => "BAND_UPPER",
        Role::Trend => "TREND",
        Role::ColorScale => "COLORSCALE",
        Role::Badge => "BADGE",
        Role::Hint => "HINT",
        Role::Text(crate::TextSize::Small) => "TEXT_SMALL",
        Role::Text(crate::TextSize::Medium) => "TEXT_MEDIUM",
        Role::Text(crate::TextSize::Large) => "TEXT_LARGE",
        Role::Placeholder => "PLACEHOLDER",
        Role::HeaderImage => "HEADER_IMAGE",
        Role::FooterLink => "FOOTER_LINK",
        Role::Download(crate::DownloadFmt::Csv) => "DOWNLOAD_CSV",
        Role::Download(crate::DownloadFmt::Xlsx) => "DOWNLOAD_XLSX",
        Role::Download(crate::DownloadFmt::Pdf) => "DOWNLOAD_PDF",
        Role::Reload => "RELOAD",
        Role::Range => "RANGE",
        Role::GaugeLabels => "LABELS",
        Role::GaugeColors => "COLORS",
        Role::Geometry => "MAP",
        Role::Input(IK::Dropdown) => "DROPDOWN",
        Role::Input(IK::Number) => "NUMBER",
        Role::Input(IK::Date) => "DATE",
        Role::Input(IK::Text) => "TEXT",
        Role::Input(IK::Multiselect) => "MULTISELECT",
        Role::Input(IK::DateRange) => "DATERANGE",
        Role::Delta => "DELTA",
        Role::Columns => "COLUMNS",
        Role::GroupStart => "GROUP",
        Role::GroupEnd => "ENDGROUP",
        Role::Span => "SPAN",
        Role::Table => "TABLE",
        Role::PagedTable => "PAGED",
        Role::Metric(crate::MetricFmt::Plain) => "METRIC",
        Role::Metric(crate::MetricFmt::Money) => "MONEY",
        Role::Metric(crate::MetricFmt::Percent) => "PERCENT",
        Role::Metric(crate::MetricFmt::Compact) => "COMPACT",
        Role::Tab => "TAB",
        Role::SubTab => "SUBTAB",
    }
}

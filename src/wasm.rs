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
            // Each role entry is `[colIdx, "ROLE", name]` — the trailing name
            // (a charted measure's display label, else "") is metadata the browser
            // passes back verbatim; JS role checks only read the first two.
            serde_json::json!({
                "setup": p.setup,
                "sql": p.sql,
                "roles": p.roles.iter().map(|(i, r)| {
                    let name = p.names.iter().find(|(j, _)| j == i).map(|(_, n)| n.as_str()).unwrap_or("");
                    serde_json::json!([i, role_str(r), name])
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
}

/// Render one panel to SVG. `rows_json` = `[{ "c0": …, "c1": … }, …]` (a panel
/// query's `-json` result); `roles_json` = the panel's `roles` from [`plan`].
#[wasm_bindgen]
pub fn render_panel(
    rows_json: &str,
    roles_json: &str,
    width: u32,
    height: u32,
    primary: &str,
    zoom_json: &str,
) -> String {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_str(rows_json).unwrap_or_default();
    let entries: Vec<(usize, String, String)> = parse_role_entries(roles_json);
    let roles: Vec<(usize, Role)> = entries
        .iter()
        .filter_map(|(i, s, _)| parse_role(s).map(|r| (*i, r)))
        .collect();
    let mut cols = sql::columns_from_rows(&rows, &roles);
    // Overlay the human display names (combo legends read these).
    for (i, _, name) in &entries {
        if !name.is_empty() {
            if let Some(c) = cols.iter_mut().find(|c| c.name == format!("c{i}")) {
                c.name = name.clone();
            }
        }
    }
    crate::set_brand(parse_primary(primary));
    // Optional map zoom window `[x0, x1, y0, y1]` (lon/lat). Empty = auto-fit.
    crate::set_panel_zoom(parse_zoom(zoom_json));
    let svg = render(&cols, width, height).unwrap_or_else(|e| format!("<pre>{e}</pre>"));
    crate::set_panel_zoom(None);
    crate::set_brand(None);
    svg
}

/// Bounds `[x0, x1, y0, y1]` of a map panel's geometry (the `MAP` + `BASEMAP`
/// columns), in lon/lat — the UI uses these to seed an aspect-correct zoom view.
#[wasm_bindgen]
pub fn map_bounds(rows_json: &str, roles_json: &str) -> String {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_str(rows_json).unwrap_or_default();
    let geo_cols: Vec<String> = parse_role_entries(roles_json)
        .iter()
        .filter(|(_, r, _)| r == "MAP" || r == "BASEMAP")
        .map(|(i, _, _)| format!("c{i}"))
        .collect();
    let (mut x0, mut y0, mut x1, mut y1) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for row in &rows {
        for key in &geo_cols {
            if let Some(b) = row
                .get(key)
                .and_then(|v| v.as_str())
                .and_then(ggplot_rs::spatial::parse_wkt)
                .and_then(|g| g.bounds())
            {
                x0 = x0.min(b.0);
                y0 = y0.min(b.1);
                x1 = x1.max(b.2);
                y1 = y1.max(b.3);
            }
        }
    }
    if !x0.is_finite() {
        return "[]".into();
    }
    format!("[{x0},{x1},{y0},{y1}]")
}

/// Data extent `[x0, x1, y0, y1]` of a cartesian panel — used to seed scroll/drag
/// zoom. Returns `[]` unless the x axis is continuous/datetime (so discrete bar
/// charts aren't made zoomable).
#[wasm_bindgen]
pub fn panel_bounds(rows_json: &str, roles_json: &str) -> String {
    use ggplot_rs::prelude::Value;
    let rows: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_str(rows_json).unwrap_or_default();
    let roles: Vec<(usize, Role)> = parse_role_entries(roles_json)
        .iter()
        .filter_map(|(i, s, _)| parse_role(s).map(|r| (*i, r)))
        .collect();
    let cols = sql::columns_from_rows(&rows, &roles);
    let Some(x) = cols.iter().find(|c| c.role == Role::X) else {
        return "[]".into();
    };
    // Continuous only: a plain string in x means a discrete axis → no zoom.
    if x.values.iter().any(|v| matches!(v, Value::Str(_)))
        || !x.values.iter().any(|v| v.as_f64().is_some())
    {
        return "[]".into();
    }
    let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in &x.values {
        if let Some(f) = v.as_f64() {
            if f.is_finite() {
                x0 = x0.min(f);
                x1 = x1.max(f);
            }
        }
    }
    let (mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY);
    for c in cols
        .iter()
        .filter(|c| matches!(c.role, Role::Value(_) | Role::BandLower | Role::BandUpper))
    {
        for v in &c.values {
            if let Some(f) = v.as_f64() {
                if f.is_finite() {
                    y0 = y0.min(f);
                    y1 = y1.max(f);
                }
            }
        }
    }
    if !x0.is_finite() || !y0.is_finite() || x1 <= x0 {
        return "[]".into();
    }
    format!("[{x0},{x1},{y0},{y1}]")
}

/// Parse a `[x0, x1, y0, y1]` zoom window (empty / invalid → `None`).
fn parse_zoom(s: &str) -> Option<crate::ZoomWindow> {
    let v: Vec<f64> = serde_json::from_str(s).ok()?;
    match v.as_slice() {
        [x0, x1, y0, y1] => Some(((*x0, *x1), (*y0, *y1))),
        _ => None,
    }
}

/// Parse the `roles` JSON from [`plan`] into `(colIdx, "ROLE", name)` triples.
/// Tolerates both the current `[i, "ROLE", name]` form and a legacy `[i, "ROLE"]`
/// (name defaults to empty).
fn parse_role_entries(roles_json: &str) -> Vec<(usize, String, String)> {
    let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(roles_json).unwrap_or_default();
    raw.into_iter()
        .filter_map(|e| {
            let i = e.first()?.as_u64()? as usize;
            let role = e.get(1)?.as_str()?.to_string();
            let name = e.get(2).and_then(|v| v.as_str()).unwrap_or("").to_string();
            Some((i, role, name))
        })
        .collect()
}

/// Parse a `RRGGBB` / `#rrggbb` brand colour (empty / invalid → default).
fn parse_primary(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let p = |a, b| u8::from_str_radix(&h[a..b], 16).ok();
    Some((p(0, 2)?, p(2, 4)?, p(4, 6)?))
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
        Role::Value(Kind::Step) => "STEP",
        Role::Value(Kind::Smooth) => "SMOOTH",
        Role::Value(Kind::Area) => "AREACHART",
        Role::Value(Kind::AreaStacked) => "AREACHART_STACKED",
        Role::Value(Kind::Point) => "SCATTER",
        Role::Value(Kind::Pie) => "PIE",
        Role::Value(Kind::Donut) => "DONUTCHART",
        Role::Value(Kind::Gauge) => "GAUGE",
        Role::Value(Kind::Histogram) => "HISTOGRAM",
        Role::Value(Kind::Boxplot) => "BOXPLOT",
        Role::Value(Kind::Violin) => "VIOLIN",
        Role::Value(Kind::Density) => "DENSITY",
        Role::Value(Kind::Heatmap) => "HEATMAP",
        Role::Value(Kind::Calendar) => "CALENDAR",
        Role::Value(Kind::Sparkline) => "SPARKLINE",
        Role::RefLine => "REFLINE",
        Role::VLine => "XLINE",
        Role::BandLower => "BAND_LOWER",
        Role::BandUpper => "BAND_UPPER",
        Role::Trend => "TREND",
        Role::ColorScale => "COLORSCALE",
        Role::Badge => "BADGE",
        Role::Plain => "PLAIN",
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
        Role::Basemap => "BASEMAP",
        Role::Flip => "FLIP",
        Role::Alpha => "ALPHA",
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
        Role::Height => "HEIGHT",
        Role::Table => "TABLE",
        Role::PagedTable => "PAGED",
        Role::Metric(crate::MetricFmt::Plain) => "METRIC",
        Role::Metric(crate::MetricFmt::Money) => "MONEY",
        Role::Metric(crate::MetricFmt::Percent) => "PERCENT",
        Role::Metric(crate::MetricFmt::Compact) => "COMPACT",
        Role::Tab => "TAB",
        Role::SubTab => "SUBTAB",
        Role::Size => "SIZE",
        Role::DataLabels => "DATALABELS",
        Role::MarkArea => "MARKAREA",
        Role::Markdown => "MARKDOWN",
    }
}

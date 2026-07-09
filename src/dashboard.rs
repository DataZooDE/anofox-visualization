//! Headless, whole-dashboard rendering. Parses an annotated SQL script, runs
//! each panel's query through a caller-supplied [`DataProvider`] (e.g. escurel
//! over DuckDB/Arrow), renders every panel, and composes them into one gridded
//! SVG — no browser required.
//!
//! ```no_run
//! use duckplot::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
//! # struct MyDb;
//! # impl DataProvider for MyDb {
//! #   fn execute(&mut self, _sql: &str) -> Result<(), String> { Ok(()) }
//! #   fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<duckplot::prelude::Value>)>, String> { Ok(vec![]) }
//! # }
//! let mut db = MyDb;
//! let svg = render_dashboard_svg(
//!     "SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL",
//!     &mut db,
//!     &DashboardOptions::default(),
//! ).unwrap();
//! ```

use crate::{render, set_brand, sql, value_str, Column, MetricFmt, Role};
use ggplot_rs::prelude::Value;

/// A data source for headless rendering. Implement this over your engine
/// (DuckDB, Arrow, …); the column order returned by [`query`](DataProvider::query)
/// must match the SELECT list order.
pub trait DataProvider {
    /// Run a setup statement (`CREATE`/`SET`/…) for effect.
    fn execute(&mut self, sql: &str) -> Result<(), String>;
    /// Run a query; return its columns as `(name, values)` in SELECT order.
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String>;
}

/// Layout + theming options for [`render_dashboard_svg`].
pub struct DashboardOptions {
    /// Total dashboard width in px.
    pub width: u32,
    /// Default panels per row (each unspecified panel spans `12/columns`).
    pub columns: u32,
    /// Default panel height in px (override per panel with `::HEIGHT`).
    pub panel_height: u32,
    /// Gap between panels in px.
    pub gap: u32,
    /// Brand/primary colour for single-series marks + accents.
    pub brand: Option<(u8, u8, u8)>,
    /// Page background colour.
    pub background: (u8, u8, u8),
}

impl Default for DashboardOptions {
    fn default() -> Self {
        DashboardOptions {
            width: 1200,
            columns: 2,
            panel_height: 320,
            gap: 16,
            brand: None,
            background: (0xf4, 0xf7, 0xfc),
        }
    }
}

/// Map a query result (columns in SELECT order) to typed [`Column`]s by the
/// role's output index.
fn columns_from_result(result: &[(String, Vec<Value>)], roles: &[(usize, Role)]) -> Vec<Column> {
    roles
        .iter()
        .filter_map(|(i, role)| result.get(*i).map(|(name, vals)| Column::new(name.clone(), *role, vals.clone())))
        .collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Render a whole annotated SQL script to one composed SVG dashboard.
pub fn render_dashboard_svg(
    script: &str,
    provider: &mut dyn DataProvider,
    opts: &DashboardOptions,
) -> Result<String, String> {
    set_brand(opts.brand);
    let result = render_inner(script, provider, opts);
    set_brand(None);
    result
}

fn render_inner(script: &str, provider: &mut dyn DataProvider, opts: &DashboardOptions) -> Result<String, String> {
    let pad = opts.gap as f64;
    let gap = opts.gap as f64;
    let content_w = opts.width as f64 - 2.0 * pad;
    let mut columns = opts.columns.max(1);
    let mut default_span = (12 / columns).max(1);

    // A placed panel: (x, y, w, h, inner svg).
    let mut placed: Vec<(f64, f64, f64, f64, String)> = Vec::new();
    let mut x = pad;
    let mut row_y = pad;
    let mut row_h = 0.0f64;
    let mut row_units = 0u32;
    let mut next_span = 0u32;
    let mut next_height = 0u32;

    let unit_w = content_w / 12.0;
    let new_row = |row_y: &mut f64, row_h: &mut f64, x: &mut f64, row_units: &mut u32| {
        *row_y += *row_h + gap;
        *row_h = 0.0;
        *x = pad;
        *row_units = 0;
    };

    for panel in sql::plan(script) {
        if panel.setup {
            provider.execute(&panel.sql).ok();
            continue;
        }
        let roles = &panel.roles;
        let has = |r: &dyn Fn(&Role) -> bool| roles.iter().any(|(_, role)| r(role));

        // Directives that don't render a box.
        if has(&|r| matches!(r, Role::Columns)) {
            if let Ok(rows) = provider.query(&panel.sql) {
                if let Some(n) = rows.first().and_then(|(_, v)| v.first()).and_then(|v| v.as_f64()) {
                    if n >= 1.0 {
                        columns = n as u32;
                        default_span = (12 / columns).max(1);
                    }
                }
            }
            continue;
        }
        if has(&|r| matches!(r, Role::Span)) {
            if let Ok(rows) = provider.query(&panel.sql) {
                next_span = rows.first().and_then(|(_, v)| v.first()).and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
            }
            continue;
        }
        if has(&|r| matches!(r, Role::Height)) {
            if let Ok(rows) = provider.query(&panel.sql) {
                next_height = rows.first().and_then(|(_, v)| v.first()).and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
            }
            continue;
        }
        // Non-rendering: inputs, downloads, tabs, chrome, reload, group markers.
        if has(&|r| {
            matches!(
                r,
                Role::Input(_)
                    | Role::Download(_)
                    | Role::Reload
                    | Role::Tab
                    | Role::SubTab
                    | Role::GroupStart
                    | Role::GroupEnd
                    | Role::Placeholder
                    | Role::HeaderImage
                    | Role::FooterLink
            )
        }) {
            continue;
        }

        let rows = match provider.query(&panel.sql) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Heading (::LABEL alone) → full-width section title, own row.
        if roles.len() == 1 && matches!(roles[0].1, Role::Label) {
            let text = rows.first().and_then(|(_, v)| v.first()).map(value_str).unwrap_or_default();
            if row_units > 0 {
                new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
            }
            let h = 34.0;
            placed.push((pad, row_y, content_w, h, heading_svg(&text, content_w, h)));
            row_y += h + gap;
            continue;
        }

        // Compute geometry.
        let span = if next_span == 0 { default_span } else { next_span.clamp(1, 12) };
        next_span = 0;
        let ch = if next_height > 0 { next_height as f64 } else { opts.panel_height as f64 };
        next_height = 0;
        let cw = (unit_w * span as f64) - gap;

        if row_units + span > 12 {
            new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
        }

        // Render the panel body to an inner SVG sized (cw, ch).
        let inner = render_panel_svg(&rows, roles, cw, ch, opts.brand);
        placed.push((x, row_y, cw, ch, inner));

        x += unit_w * span as f64;
        row_units += span;
        row_h = row_h.max(ch);
    }
    let total_h = row_y + row_h + pad;

    // Compose.
    let (br, bg, bb) = opts.background;
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h:.0}\" viewBox=\"0 0 {w} {h:.0}\">\
         <rect width=\"{w}\" height=\"{h:.0}\" fill=\"rgb({br},{bg},{bb})\"/>",
        w = opts.width,
        h = total_h
    );
    for (px, py, pw, ph, inner) in &placed {
        // card background (skip for headings, which have transparent inner)
        out.push_str(&format!(
            "<rect x=\"{px:.1}\" y=\"{py:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" rx=\"14\" \
             fill=\"#ffffff\" stroke=\"#e6e9f1\" stroke-width=\"1\"/>"
        ));
        // nest the inner svg at (px,py) — it is already sized (pw? no: cw,ch=pw,ph)
        out.push_str(&inner.replacen("<svg ", &format!("<svg x=\"{px:.1}\" y=\"{py:.1}\" "), 1));
    }
    out.push_str("</svg>");
    Ok(out)
}

/// One panel body → an inner `<svg>` sized (w, h). Charts go through the normal
/// renderer; KPIs / text / tables get a lightweight SVG.
fn render_panel_svg(rows: &[(String, Vec<Value>)], roles: &[(usize, Role)], w: f64, h: f64, _brand: Option<(u8, u8, u8)>) -> String {
    let cols = columns_from_result(rows, roles);
    // Title bar (::TITLE) eats a strip off the top.
    let title = roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Title))
        .and_then(|(i, _)| rows.get(*i))
        .and_then(|(_, v)| v.first())
        .map(value_str)
        .filter(|s| !s.is_empty());
    let title_h = if title.is_some() { 26.0 } else { 0.0 };
    let pad = 14.0;

    // A KPI / text card.
    if let Some((i, fmt)) = roles.iter().find_map(|(i, r)| match r {
        Role::Metric(f) => Some((*i, Some(*f))),
        Role::Text(_) => Some((*i, None)),
        _ => None,
    }) {
        let raw = rows.get(i).and_then(|(_, v)| v.first());
        let val = match fmt {
            Some(f) => raw.and_then(|v| v.as_f64()).map(|n| fmt_metric(n, f)).unwrap_or_else(|| "–".into()),
            None => raw.map(value_str).unwrap_or_default(),
        };
        let cap = roles
            .iter()
            .find(|(_, r)| matches!(r, Role::Label))
            .and_then(|(i, _)| rows.get(*i))
            .and_then(|(_, v)| v.first())
            .map(value_str)
            .unwrap_or_default();
        let cy = h / 2.0;
        return format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\">\
             <text x=\"{pad}\" y=\"{ty:.0}\" font-family=\"system-ui,sans-serif\" font-size=\"34\" font-weight=\"800\" fill=\"#1f2937\">{v}</text>\
             <text x=\"{pad}\" y=\"{cy2:.0}\" font-family=\"system-ui,sans-serif\" font-size=\"13\" font-weight=\"600\" fill=\"#667085\">{c}</text></svg>",
            ty = cy,
            cy2 = cy + 24.0,
            v = esc(&val),
            c = esc(&cap),
        );
    }

    // A table.
    if roles.iter().any(|(_, r)| matches!(r, Role::Table | Role::PagedTable)) {
        return table_svg(rows, roles, w, h, title.as_deref());
    }

    // Otherwise a chart — render at the content area, then wrap with a title.
    let body_h = (h - title_h - pad).max(30.0);
    let chart = render(&cols, w as u32, body_h as u32).unwrap_or_else(|e| format!("<pre>{e}</pre>"));
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\">"
    );
    if let Some(t) = &title {
        svg.push_str(&format!(
            "<text x=\"{pad}\" y=\"20\" font-family=\"system-ui,sans-serif\" font-size=\"14\" font-weight=\"700\" fill=\"#1f2430\">{}</text>",
            esc(t)
        ));
    }
    // nest the chart svg below the title
    svg.push_str(&chart.replacen("<svg ", &format!("<svg y=\"{title_h}\" "), 1));
    svg.push_str("</svg>");
    svg
}

fn heading_svg(text: &str, w: f64, h: f64) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\">\
         <text x=\"2\" y=\"24\" font-family=\"system-ui,sans-serif\" font-size=\"19\" font-weight=\"700\" fill=\"#1f2430\">{}</text></svg>",
        esc(text)
    )
}

/// A minimal table: header row + up to ~14 body rows. Skips a ::TITLE column.
fn table_svg(rows: &[(String, Vec<Value>)], roles: &[(usize, Role)], w: f64, h: f64, title: Option<&str>) -> String {
    let skip: std::collections::HashSet<usize> = roles
        .iter()
        .filter(|(_, r)| matches!(r, Role::Title))
        .map(|(i, _)| *i)
        .collect();
    let cols: Vec<&(String, Vec<Value>)> = rows.iter().enumerate().filter(|(i, _)| !skip.contains(i)).map(|(_, c)| c).collect();
    let ncol = cols.len().max(1);
    let nrow = cols.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
    let top = if title.is_some() { 30.0 } else { 10.0 };
    let col_w = (w - 20.0) / ncol as f64;
    let row_h = 22.0;
    let max_rows = (((h - top - 24.0) / row_h).floor() as usize).min(nrow).max(0);

    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\" \
         font-family=\"system-ui,sans-serif\" font-size=\"12\">"
    );
    if let Some(t) = title {
        s.push_str(&format!(
            "<text x=\"12\" y=\"20\" font-size=\"14\" font-weight=\"700\" fill=\"#1f2430\">{}</text>",
            esc(t)
        ));
    }
    // header
    for (c, (name, _)) in cols.iter().enumerate() {
        let cx = 12.0 + c as f64 * col_w;
        s.push_str(&format!(
            "<text x=\"{cx:.0}\" y=\"{y:.0}\" font-weight=\"700\" fill=\"#667085\">{}</text>",
            esc(name),
            y = top + 16.0
        ));
    }
    s.push_str(&format!(
        "<line x1=\"12\" y1=\"{ly:.0}\" x2=\"{x2:.0}\" y2=\"{ly:.0}\" stroke=\"#e6e9f1\"/>",
        ly = top + 22.0,
        x2 = w - 8.0
    ));
    // rows
    for r in 0..max_rows {
        let y = top + 22.0 + (r as f64 + 1.0) * row_h;
        for (c, (_, vals)) in cols.iter().enumerate() {
            let cx = 12.0 + c as f64 * col_w;
            let cell = vals.get(r).map(value_str).unwrap_or_default();
            s.push_str(&format!(
                "<text x=\"{cx:.0}\" y=\"{y:.0}\" fill=\"#1f2937\">{}</text>",
                esc(&cell)
            ));
        }
    }
    if nrow > max_rows {
        s.push_str(&format!(
            "<text x=\"12\" y=\"{y:.0}\" fill=\"#8a93a6\">… {} more rows</text>",
            nrow - max_rows,
            y = top + 22.0 + (max_rows as f64 + 1.0) * row_h
        ));
    }
    s.push_str("</svg>");
    s
}

fn fmt_metric(n: f64, fmt: MetricFmt) -> String {
    let group = |v: f64| {
        let neg = v < 0.0;
        let s = format!("{}", v.abs().round() as i64);
        let mut out = String::new();
        for (i, ch) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
        }
        let grouped: String = out.chars().rev().collect();
        if neg {
            format!("-{grouped}")
        } else {
            grouped
        }
    };
    match fmt {
        MetricFmt::Money => format!("${}", group(n)),
        MetricFmt::Percent => {
            if (n - n.round()).abs() < 1e-9 {
                format!("{}%", n.round() as i64)
            } else {
                format!("{n:.1}%")
            }
        }
        MetricFmt::Compact => {
            let a = n.abs();
            if a >= 1e9 {
                format!("{:.1}B", n / 1e9)
            } else if a >= 1e6 {
                format!("{:.1}M", n / 1e6)
            } else if a >= 1e3 {
                format!("{:.1}K", n / 1e3)
            } else {
                group(n)
            }
        }
        MetricFmt::Plain => {
            if (n - n.round()).abs() < 1e-9 {
                group(n)
            } else {
                format!("{n:.2}")
            }
        }
    }
}

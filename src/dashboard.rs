//! Headless, whole-dashboard rendering. Parses an annotated SQL script, runs
//! each panel's query through a caller-supplied [`DataProvider`] (e.g. escurel
//! over DuckDB/Arrow), renders every panel, and composes them into one gridded
//! SVG — no browser required.
//!
//! ```
//! use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
//! # struct MyDb;
//! # impl DataProvider for MyDb {
//! #   fn execute(&mut self, _sql: &str) -> Result<(), String> { Ok(()) }
//! #   fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<anofox_visualization::Value>)>, String> { Ok(vec![]) }
//! # }
//! let mut db = MyDb;
//! let svg = render_dashboard_svg(
//!     "SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL",
//!     &mut db,
//!     &DashboardOptions::default(),
//! ).unwrap();
//! assert!(svg.starts_with("<svg"));
//! ```

use crate::format::{escape_xml as esc, format_number};
use crate::{
    error_svg_at, render_with_at, roles, sql, svg_open, value_str, Column, Place, RenderOptions,
    Role,
};
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

/// The output column for planned index `i`: the column aliased `c{i}` (robust
/// when a `*` item expands to several columns), else the i-th column.
fn column_at(result: &[(String, Vec<Value>)], i: usize) -> Option<&(String, Vec<Value>)> {
    let key = format!("c{i}");
    result
        .iter()
        .find(|(n, _)| *n == key)
        .or_else(|| result.get(i))
}

/// Map a query result (columns in SELECT order) to typed [`Column`]s by the
/// role's output index.
fn columns_from_result(result: &[(String, Vec<Value>)], roles: &[(usize, Role)]) -> Vec<Column> {
    roles
        .iter()
        .filter_map(|(i, role)| {
            column_at(result, *i).map(|(name, vals)| Column::new(name.clone(), *role, vals.clone()))
        })
        .collect()
}

/// Render a whole annotated SQL script to one composed SVG dashboard.
pub fn render_dashboard_svg(
    script: &str,
    provider: &mut dyn DataProvider,
    opts: &DashboardOptions,
) -> Result<String, String> {
    render_inner(script, provider, opts)
}

fn render_inner(
    script: &str,
    provider: &mut dyn DataProvider,
    opts: &DashboardOptions,
) -> Result<String, String> {
    let pad = opts.gap as f64;
    let gap = opts.gap as f64;
    let content_w = opts.width as f64 - 2.0 * pad;
    let mut columns = opts.columns.max(1);
    let mut default_span = (12 / columns).max(1);

    // A placed panel: (x, y, w, h, nested svg fragment positioned at x, y).
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
                if let Some(n) = rows
                    .first()
                    .and_then(|(_, v)| v.first())
                    .and_then(|v| v.as_f64())
                {
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
                next_span = rows
                    .first()
                    .and_then(|(_, v)| v.first())
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as u32;
            }
            continue;
        }
        if has(&|r| matches!(r, Role::Height)) {
            if let Ok(rows) = provider.query(&panel.sql) {
                next_height = rows
                    .first()
                    .and_then(|(_, v)| v.first())
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as u32;
            }
            continue;
        }
        // Non-rendering directives (inputs, downloads, tabs, chrome, reload,
        // group markers, placeholders) — from the role registry.
        if roles::is_directive_panel(roles) {
            continue;
        }

        let rows = match provider.query(&panel.sql) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Heading (::LABEL alone) → full-width section title, own row.
        if roles.len() == 1 && matches!(roles[0].1, Role::Label) {
            let text = rows
                .first()
                .and_then(|(_, v)| v.first())
                .map(value_str)
                .unwrap_or_default();
            if row_units > 0 {
                new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
            }
            let h = 34.0;
            placed.push((
                pad,
                row_y,
                content_w,
                h,
                heading_svg(Place::At(pad, row_y), &text, content_w, h),
            ));
            row_y += h + gap;
            continue;
        }

        // Compute geometry.
        let span = if next_span == 0 {
            default_span
        } else {
            next_span.clamp(1, 12)
        };
        next_span = 0;
        let ch = if next_height > 0 {
            next_height as f64
        } else {
            opts.panel_height as f64
        };
        next_height = 0;
        let cw = (unit_w * span as f64) - gap;

        if row_units + span > 12 {
            new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
        }

        // Render the panel body to an inner SVG sized (cw, ch).
        let ropts = RenderOptions {
            brand: opts.brand,
            ..RenderOptions::default()
        };
        let inner = render_panel_svg(&rows, roles, Place::At(x, row_y), cw, ch, &ropts);
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
        // the panel itself: a nested <svg> already positioned at (px, py)
        out.push_str(inner);
    }
    out.push_str("</svg>");
    Ok(out)
}

/// One panel body → an inner `<svg>` sized (w, h). Charts go through the normal
/// renderer; KPIs / text / tables get a lightweight SVG.
fn render_panel_svg(
    rows: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    place: Place,
    w: f64,
    h: f64,
    ropts: &RenderOptions,
) -> String {
    let cols = columns_from_result(rows, roles);
    // Title bar (::TITLE) eats a strip off the top.
    let title = roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Title))
        .and_then(|(i, _)| column_at(rows, *i))
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
        let raw = column_at(rows, i).and_then(|(_, v)| v.first());
        let val = match fmt {
            Some(f) => raw
                .and_then(|v| v.as_f64())
                .map(|n| format_number(n, f))
                .unwrap_or_else(|| "–".into()),
            None => raw.map(value_str).unwrap_or_default(),
        };
        let cap = roles
            .iter()
            .find(|(_, r)| matches!(r, Role::Label))
            .and_then(|(i, _)| column_at(rows, *i))
            .and_then(|(_, v)| v.first())
            .map(value_str)
            .unwrap_or_default();
        let cy = h / 2.0;
        return format!(
            "{open}<text x=\"{pad}\" y=\"{ty:.0}\" font-family=\"system-ui,sans-serif\" font-size=\"34\" font-weight=\"800\" fill=\"#1f2937\">{v}</text>\
             <text x=\"{pad}\" y=\"{cy2:.0}\" font-family=\"system-ui,sans-serif\" font-size=\"13\" font-weight=\"600\" fill=\"#667085\">{c}</text></svg>",
            open = svg_open(place, format!("{w:.0}"), format!("{h:.0}")),
            ty = cy,
            cy2 = cy + 24.0,
            v = esc(&val),
            c = esc(&cap),
        );
    }

    // A table.
    if roles
        .iter()
        .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable))
    {
        return table_svg(rows, roles, place, w, h, title.as_deref());
    }

    // Otherwise a chart — render at the content area, then wrap with a title.
    let body_h = (h - title_h - pad).max(30.0);
    // The chart is nested below the title as a positioned fragment.
    let chart = render_with_at(&cols, 0.0, title_h, w as u32, body_h as u32, ropts)
        .unwrap_or_else(|e| error_svg_at(Place::At(0.0, title_h), &e.to_string(), w as u32));
    let mut svg = svg_open(place, format!("{w:.0}"), format!("{h:.0}"));
    if let Some(t) = &title {
        svg.push_str(&format!(
            "<text x=\"{pad}\" y=\"20\" font-family=\"system-ui,sans-serif\" font-size=\"14\" font-weight=\"700\" fill=\"#1f2430\">{}</text>",
            esc(t)
        ));
    }
    svg.push_str(&chart);
    svg.push_str("</svg>");
    svg
}

fn heading_svg(place: Place, text: &str, w: f64, h: f64) -> String {
    format!(
        "{}<text x=\"2\" y=\"24\" font-family=\"system-ui,sans-serif\" font-size=\"19\" font-weight=\"700\" fill=\"#1f2430\">{}</text></svg>",
        svg_open(place, format!("{w:.0}"), format!("{h:.0}")),
        esc(text)
    )
}

/// A minimal table: header row + up to ~14 body rows. Skips a ::TITLE column.
fn table_svg(
    rows: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    place: Place,
    w: f64,
    h: f64,
    title: Option<&str>,
) -> String {
    let skip: std::collections::HashSet<usize> = roles
        .iter()
        .filter(|(_, r)| matches!(r, Role::Title))
        .map(|(i, _)| *i)
        .collect();
    // Per-column number formats (::MONEY/::PERCENT/::COMPACT/::METRIC), the
    // same formatter the browser uses (wasm `format_number`).
    let fmt_of: std::collections::HashMap<usize, crate::MetricFmt> = roles
        .iter()
        .filter_map(|(i, r)| match r {
            Role::Metric(f) => Some((*i, *f)),
            _ => None,
        })
        .collect();
    type FmtCol<'a> = (Option<crate::MetricFmt>, &'a (String, Vec<Value>));
    let cols: Vec<FmtCol> = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| !skip.contains(i))
        .map(|(i, c)| (fmt_of.get(&i).copied(), c))
        .collect();
    let ncol = cols.len().max(1);
    let nrow = cols.iter().map(|(_, (_, v))| v.len()).max().unwrap_or(0);
    let top = if title.is_some() { 30.0 } else { 10.0 };
    let col_w = (w - 20.0) / ncol as f64;
    let row_h = 22.0;
    let max_rows = (((h - top - 24.0) / row_h).floor().max(0.0) as usize).min(nrow);

    let mut s = svg_open(place, format!("{w:.0}"), format!("{h:.0}"));
    s.push_str("<g font-family=\"system-ui,sans-serif\" font-size=\"12\">");
    if let Some(t) = title {
        s.push_str(&format!(
            "<text x=\"12\" y=\"20\" font-size=\"14\" font-weight=\"700\" fill=\"#1f2430\">{}</text>",
            esc(t)
        ));
    }
    // header
    for (c, (_, (name, _))) in cols.iter().enumerate() {
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
        for (c, (fmt, (_, vals))) in cols.iter().enumerate() {
            let cx = 12.0 + c as f64 * col_w;
            let cell = match (fmt, vals.get(r)) {
                (Some(f), Some(v)) if v.as_f64().is_some() => {
                    format_number(v.as_f64().unwrap_or(f64::NAN), *f)
                }
                (_, v) => v.map(value_str).unwrap_or_default(),
            };
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
    s.push_str("</g></svg>");
    s
}

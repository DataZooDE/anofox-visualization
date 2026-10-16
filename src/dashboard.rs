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

use crate::format::escape_xml as esc;
use crate::format::{format_number_l, Locale};
use crate::sql::contract_call;
use crate::theme::{hex, DashboardChrome, ResolvedTheme, Variant, CREDIT_TEXT, CREDIT_URL};
use crate::{
    error_svg_at, render_with_at, roles, sql, svg_open, value_str, Column, Place, RenderOptions,
    Role,
};
use ggplot_rs::prelude::Value;
use std::sync::Arc;

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
    /// Brand/primary colour for single-series marks (`None` = the theme's
    /// first palette colour).
    pub brand: Option<(u8, u8, u8)>,
    /// Theme spec used when the script sets no `::THEME` (a built-in name,
    /// `a+b`, or JSON — e.g. from `dashboard --theme`). `None` = `light`.
    pub theme: Option<String>,
    /// Which variant to draw when the theme follows the viewer (`auto`).
    pub variant: Variant,
}

impl Default for DashboardOptions {
    fn default() -> Self {
        DashboardOptions {
            width: 1200,
            columns: 2,
            panel_height: 320,
            gap: 16,
            brand: None,
            theme: None,
            variant: Variant::Light,
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
            column_at(result, *i).map(|(name, vals)| {
                // An ISO-date x axis becomes a datetime scale and a date
                // ::XLINE an x position, as in the browser
                // (`sql::columns_from_rows`).
                let vals = sql::coerce_for_role(*role, vals.clone());
                Column::new(name.clone(), *role, vals)
            })
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

/// One executed statement of the script (phase 1).
struct Step {
    panel: sql::Panel,
    rows: Result<Vec<(String, Vec<Value>)>, String>,
}

/// The first cell of a result as text.
fn first_text(rows: &[(String, Vec<Value>)]) -> Option<String> {
    rows.first().and_then(|(_, v)| v.first()).map(value_str)
}

/// Run the script in order (setup for effect, panels queried) and collect
/// the dashboard chrome directives — so a `::THEME` anywhere applies to every
/// panel.
fn execute(script: &str, provider: &mut dyn DataProvider) -> (Vec<Step>, DashboardChrome) {
    let mut chrome = DashboardChrome::default();
    let mut steps = Vec::new();
    for panel in sql::plan(script) {
        if panel.setup {
            provider.execute(&panel.sql).ok();
            continue;
        }
        let chrome_role = panel
            .roles
            .iter()
            .map(|(_, r)| *r)
            .find(|r| roles::is_chrome_directive(*r));
        if let Some(r) = chrome_role {
            let v = provider
                .query(&panel.sql)
                .ok()
                .and_then(|rows| first_text(&rows));
            chrome.apply(r, v);
            continue;
        }
        let has = |f: &dyn Fn(&Role) -> bool| panel.roles.iter().any(|(_, r)| f(r));
        let layout =
            has(&|r| matches!(r, Role::Columns | Role::Span | Role::Height | Role::Section));
        if roles::is_directive_panel(&panel.roles) && !layout {
            continue; // inputs, downloads, tabs, groups, reload, …
        }
        let rows = provider.query(&panel.sql);
        steps.push(Step { panel, rows });
    }
    (steps, chrome)
}

fn render_inner(
    script: &str,
    provider: &mut dyn DataProvider,
    opts: &DashboardOptions,
) -> Result<String, String> {
    let (steps, chrome) = execute(script, provider);
    let set = chrome.resolve(opts.theme.as_deref())?;
    let theme = set.pick(opts.variant).clone();
    let locale = theme
        .locale
        .as_deref()
        .and_then(Locale::from_tag)
        .unwrap_or_default();
    let t = &theme;
    let ropts = RenderOptions {
        brand: opts.brand,
        theme: Arc::new(theme.clone()),
        locale,
        ..RenderOptions::default()
    };

    let pad = opts.gap as f64;
    let gap = opts.gap as f64;
    let content_w = opts.width as f64 - 2.0 * pad;
    let mut columns = opts.columns.max(1);
    let mut default_span = (12 / columns).max(1);

    // A placed panel: (x, y, w, h, nested svg fragment positioned at x, y).
    let mut placed: Vec<(f64, f64, f64, f64, String)> = Vec::new();
    // Full-width headings (no card): nested svg fragments.
    let mut headings: Vec<String> = Vec::new();
    let mut x = pad;
    let mut row_y = pad;
    let mut row_h = 0.0f64;
    let mut row_units = 0u32;
    let mut next_span = 0u32;
    let mut next_height = 0u32;

    // The dashboard header (::DASHBOARD_TITLE / ::BRAND / ::LOGO).
    if let Some((svg, h)) = header_svg(t, pad, row_y, content_w) {
        headings.push(svg);
        row_y += h + gap;
    }

    let unit_w = content_w / 12.0;
    let new_row = |row_y: &mut f64, row_h: &mut f64, x: &mut f64, row_units: &mut u32| {
        *row_y += *row_h + gap;
        *row_h = 0.0;
        *x = pad;
        *row_units = 0;
    };

    for step in &steps {
        let panel = &step.panel;
        let roles = &panel.roles;
        let has = |r: &dyn Fn(&Role) -> bool| roles.iter().any(|(_, role)| r(role));
        let num = || {
            step.rows.as_ref().ok().and_then(|rows| {
                rows.first()
                    .and_then(|(_, v)| v.first())
                    .and_then(|v| v.as_f64())
            })
        };

        // Directives that don't render a box.
        if has(&|r| matches!(r, Role::Columns)) {
            if let Some(n) = num() {
                if n >= 1.0 {
                    columns = n as u32;
                    default_span = (12 / columns).max(1);
                }
            }
            continue;
        }
        if has(&|r| matches!(r, Role::Span)) {
            next_span = num().unwrap_or(0.0) as u32;
            continue;
        }
        if has(&|r| matches!(r, Role::Height)) {
            next_height = num().unwrap_or(0.0) as u32;
            continue;
        }
        let rows = match &step.rows {
            Ok(r) => r,
            Err(_) => continue,
        };

        // A contract plot call → one panel per result row (GROUP BY key).
        if let Some(cp) = &panel.contract {
            let span = if next_span == 0 {
                default_span
            } else {
                next_span.clamp(1, 12)
            };
            let ch = if next_height > 0 {
                next_height as f64
            } else {
                opts.panel_height as f64
            };
            next_span = 0;
            next_height = 0;
            let cw = (unit_w * span as f64) - gap;
            let title = roles
                .iter()
                .find(|(_, r)| matches!(r, Role::Title))
                .and_then(|(i, _)| column_at(rows, *i))
                .and_then(|(_, v)| v.first())
                .map(value_str)
                .unwrap_or_default();
            let maps = json_rows(rows);
            let title_h = 26.0;
            let body_h = (ch - title_h).max(30.0);
            for (label, r) in
                contract_call::render_rows_opts(cp, &maps, cw as u32, body_h as u32, &ropts)
            {
                if row_units + span > 12 {
                    new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
                }
                let svg = r.unwrap_or_else(|e| error_svg_at(Place::Doc, &e.to_string(), cw as u32));
                let tt = match (title.is_empty(), label.is_empty()) {
                    (false, false) => format!("{title} — {label}"),
                    (false, true) => title.clone(),
                    (true, _) => label,
                };
                let inner = format!(
                    "{open}<text x=\"14\" y=\"20\" font-family=\"{font}\" \
                     font-size=\"14\" font-weight=\"700\" fill=\"{ink}\">{tt}</text>\
                     <g transform=\"translate(0,{title_h})\">{svg}</g></svg>",
                    open = svg_open(Place::At(x, row_y), format!("{cw:.0}"), format!("{ch:.0}")),
                    font = esc(&t.font.family),
                    ink = hex(t.colors.heading),
                    tt = esc(&tt),
                );
                placed.push((x, row_y, cw, ch, inner));
                x += unit_w * span as f64;
                row_units += span;
                row_h = row_h.max(ch);
            }
            continue;
        }

        // Heading (::LABEL alone, ::SECTION) → full-width section title, own row.
        if has(&|r| matches!(r, Role::Section))
            || (roles.len() == 1 && matches!(roles[0].1, Role::Label))
        {
            let text = first_text(rows).unwrap_or_default();
            if row_units > 0 {
                new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
            }
            let h = 34.0;
            headings.push(heading_svg(t, Place::At(pad, row_y), &text, content_w, h));
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
        let inner = render_panel_svg(rows, roles, Place::At(x, row_y), cw, ch, &ropts);
        placed.push((x, row_y, cw, ch, inner));

        x += unit_w * span as f64;
        row_units += span;
        row_h = row_h.max(ch);
    }
    // The mandatory credit line, below the last row.
    let credit_y = row_y + row_h + gap + 14.0;
    let total_h = credit_y + 12.0 + pad;

    // Compose.
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h:.0}\" viewBox=\"0 0 {w} {h:.0}\" data-theme=\"{name}\" data-variant=\"{var}\">\
         <rect width=\"{w}\" height=\"{h:.0}\" fill=\"{bg}\"/>",
        w = opts.width,
        h = total_h,
        name = esc(&t.name),
        var = t.variant.as_str(),
        bg = hex(t.colors.bg),
    );
    for (px, py, pw, ph, inner) in &placed {
        out.push_str(&format!(
            "<rect x=\"{px:.1}\" y=\"{py:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" rx=\"{r}\" \
             fill=\"{card}\" stroke=\"{line}\" stroke-width=\"1\"/>",
            r = (t.radius * 0.78).round(),
            card = hex(t.colors.card),
            line = hex(t.colors.line),
        ));
        // the panel itself: a nested <svg> already positioned at (px, py)
        out.push_str(inner);
    }
    for h in &headings {
        out.push_str(h);
    }
    out.push_str(&credit_svg(t, opts.width as f64 / 2.0, credit_y));
    out.push_str("</svg>");
    Ok(out)
}

/// The header strip (logo, brand, title), or `None` when the dashboard sets
/// no branding. Returns the fragment and its height.
fn header_svg(t: &ResolvedTheme, x: f64, y: f64, w: f64) -> Option<(String, f64)> {
    let b = &t.brand;
    if b.title.is_none() && b.name.is_none() && b.logo.is_none() {
        return None;
    }
    let h = 52.0;
    let mut s = svg_open(Place::At(x, y), format!("{w:.0}"), format!("{h:.0}"));
    let mut tx = 2.0;
    let logo = match (t.variant, &b.logo_dark, &b.logo) {
        (Variant::Dark, Some(d), _) => Some(d),
        (_, _, l) => l.as_ref(),
    };
    if let Some(l) = logo {
        s.push_str(&format!(
            "<image class=\"dp-logo\" href=\"{}\" x=\"0\" y=\"6\" width=\"40\" height=\"40\" preserveAspectRatio=\"xMinYMid meet\"/>",
            esc(l)
        ));
        tx = 52.0;
    }
    let font = esc(&t.font.display);
    match (&b.name, &b.title) {
        (Some(n), Some(ti)) => s.push_str(&format!(
            "<text x=\"{tx}\" y=\"18\" font-family=\"{font}\" font-size=\"11\" font-weight=\"700\" letter-spacing=\".05em\" fill=\"{m}\">{n}</text>\
             <text x=\"{tx}\" y=\"42\" font-family=\"{font}\" font-size=\"24\" font-weight=\"800\" fill=\"{hd}\">{ti}</text>",
            m = hex(t.colors.muted),
            hd = hex(t.colors.heading),
            n = esc(&n.to_uppercase()),
            ti = esc(ti),
        )),
        (None, Some(ti)) | (Some(ti), None) => s.push_str(&format!(
            "<text x=\"{tx}\" y=\"34\" font-family=\"{font}\" font-size=\"24\" font-weight=\"800\" fill=\"{hd}\">{}</text>",
            esc(ti),
            hd = hex(t.colors.heading),
        )),
        (None, None) => {}
    }
    s.push_str("</svg>");
    Some((s, h))
}

/// The mandatory "Powered by DataZoo GmbH" credit, centred at (cx, y).
fn credit_svg(t: &ResolvedTheme, cx: f64, y: f64) -> String {
    format!(
        "<a href=\"{CREDIT_URL}\" class=\"dp-credit\"><text x=\"{cx:.0}\" y=\"{y:.0}\" text-anchor=\"middle\" font-family=\"{font}\" font-size=\"12\" fill=\"{m}\">{CREDIT_TEXT}</text></a>",
        font = esc(&t.font.family),
        m = hex(t.colors.muted),
    )
}

/// A column result as JSON row maps keyed by output column name (`c{i}`).
fn json_rows(result: &[(String, Vec<Value>)]) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let n = result.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
    (0..n)
        .map(|r| {
            result
                .iter()
                .map(|(name, vals)| {
                    let v = match vals.get(r) {
                        Some(Value::Str(s)) => serde_json::Value::String(s.clone()),
                        Some(v) => v
                            .as_f64()
                            .and_then(serde_json::Number::from_f64)
                            .map_or(serde_json::Value::Null, serde_json::Value::Number),
                        None => serde_json::Value::Null,
                    };
                    (name.clone(), v)
                })
                .collect()
        })
        .collect()
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

    // A table (before the KPI check: a table's ::MONEY / ::PERCENT columns are
    // column formats, not a KPI).
    if roles
        .iter()
        .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable))
    {
        return table_svg(rows, roles, place, w, h, title.as_deref(), ropts);
    }

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
                .map(|n| format_number_l(n, f, &ropts.locale))
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
        // `::THRESHOLDS` colours the value by its status (good / warn / bad).
        let status = roles
            .iter()
            .find(|(_, r)| *r == Role::Thresholds)
            .and_then(|(i, _)| column_at(rows, *i))
            .and_then(|(_, v)| v.first())
            .and_then(|spec| crate::thresholds::Rules::parse(&value_str(spec)).ok())
            .zip(raw.and_then(|v| v.as_f64()))
            .and_then(|(rules, n)| rules.status(n));
        let cy = h / 2.0;
        let t = &ropts.theme;
        return format!(
            "{open}<text x=\"{pad}\" y=\"{ty:.0}\" font-family=\"{fd}\" font-size=\"34\" font-weight=\"800\" fill=\"{ink}\">{v}</text>\
             <text x=\"{pad}\" y=\"{cy2:.0}\" font-family=\"{fb}\" font-size=\"13\" font-weight=\"600\" fill=\"{muted}\">{c}</text></svg>",
            open = svg_open(place, format!("{w:.0}"), format!("{h:.0}")),
            fd = esc(&t.font.display),
            fb = esc(&t.font.family),
            ink = hex(status.map_or(t.colors.ink, |st| st.color(t))),
            muted = hex(t.colors.muted),
            ty = cy,
            cy2 = cy + 24.0,
            v = esc(&val),
            c = esc(&cap),
        );
    }

    // Otherwise a chart — render at the content area, then wrap with a title.
    let body_h = (h - title_h - pad).max(30.0);
    // The chart is nested below the title as a positioned fragment.
    let chart = render_with_at(&cols, 0.0, title_h, w as u32, body_h as u32, ropts)
        .unwrap_or_else(|e| error_svg_at(Place::At(0.0, title_h), &e.to_string(), w as u32));
    let mut svg = svg_open(place, format!("{w:.0}"), format!("{h:.0}"));
    if let Some(tt) = &title {
        svg.push_str(&format!(
            "<text x=\"{pad}\" y=\"20\" font-family=\"{f}\" font-size=\"14\" font-weight=\"700\" fill=\"{hd}\">{}</text>",
            esc(tt),
            f = esc(&ropts.theme.font.display),
            hd = hex(ropts.theme.colors.heading),
        ));
    }
    svg.push_str(&chart);
    svg.push_str("</svg>");
    svg
}

fn heading_svg(t: &ResolvedTheme, place: Place, text: &str, w: f64, h: f64) -> String {
    format!(
        "{}<text x=\"2\" y=\"24\" font-family=\"{}\" font-size=\"19\" font-weight=\"700\" fill=\"{}\">{}</text></svg>",
        svg_open(place, format!("{w:.0}"), format!("{h:.0}")),
        esc(&t.font.display),
        hex(t.colors.heading),
        esc(text)
    )
}

/// The headless table: the browser table's formatting decisions (shared
/// [`crate::table`] module — humanised headers, one unit per money / compact
/// column moved into the header, aligned decimals, a true minus sign, ids and
/// years ungrouped), numbers right-aligned, natural column widths, and the
/// default totals row under a double rule (≥ 3 rows with an amount column).
/// As many rows as fit, then "… n more rows". Skips a ::TITLE column.
fn table_svg(
    rows: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    place: Place,
    w: f64,
    h: f64,
    title: Option<&str>,
    ropts: &RenderOptions,
) -> String {
    use crate::table::{
        default_total, display_text, profile_column, short_number, Cell, ColumnSpec, Negatives,
    };
    let th = &ropts.theme;
    let l = &ropts.locale;
    let neg = Negatives::parse(&th.chrome.negatives);
    let (ink, muted, line) = (
        hex(th.colors.ink),
        hex(th.colors.muted),
        hex(th.colors.line),
    );
    let skip: std::collections::HashSet<usize> = roles
        .iter()
        .filter(|(_, r)| matches!(r, Role::Title))
        .map(|(i, _)| *i)
        .collect();
    // Per-column format roles (::MONEY, ::PERCENT, ::TREND, ::COLORSCALE, …),
    // the tokens the browser table reads.
    let fmt_of: std::collections::HashMap<usize, &'static str> = roles
        .iter()
        .filter(|(_, r)| crate::roles::is_table_format(*r))
        .map(|(i, r)| (*i, r.token()))
        .collect();
    let cell = |v: &Value| match v {
        Value::Na => Cell::Null,
        Value::Str(s) => Cell::Text(s.clone()),
        Value::Bool(b) => Cell::Bool(*b),
        Value::DateTime(_) => Cell::Text(value_str(v)),
        v => v.as_f64().map_or(Cell::Null, Cell::Num),
    };
    struct Col {
        fmt: Option<&'static str>,
        cells: Vec<Cell>,
        p: crate::table::ColumnProfile,
    }
    let nrow = rows.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
    let vis: Vec<(usize, &(String, Vec<Value>))> = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| !skip.contains(i))
        .filter(|(i, _)| fmt_of.get(i) != Some(&"SPARKLINE"))
        .collect();
    let cols: Vec<Col> = vis
        .iter()
        .enumerate()
        .map(|(k, (i, (name, vals)))| {
            let cells: Vec<Cell> = vals.iter().map(cell).collect();
            let fmt = fmt_of.get(i).copied();
            let spec = ColumnSpec {
                name,
                fmt,
                first: k == 0,
                range: None,
                many: nrow > 1,
            };
            let p = profile_column(&spec, &cells, l);
            Col { fmt, cells, p }
        })
        .collect();
    let text = |c: &Col, r: usize| {
        c.cells
            .get(r)
            .map(|v| display_text(v, &c.p, c.fmt, l, neg))
            .unwrap_or_default()
    };
    // totals: the browser's default (≥ 3 rows and an amount column)
    let totals: Vec<Option<String>> = cols
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let agg = default_total(&c.p, c.fmt).filter(|_| k > 0)?;
            let nums: Vec<f64> = c.cells.iter().filter_map(Cell::num).collect();
            if nums.is_empty() {
                return None;
            }
            let sum: f64 = nums.iter().sum();
            let v = if agg == "sum" {
                sum
            } else {
                sum / nums.len() as f64
            };
            // a mean of whole numbers keeps one decimal
            let mut p = c.p.clone();
            if agg == "mean" && p.unit.is_none() && p.dec == 0 && v.fract() != 0.0 {
                p.dec = 1;
            }
            // trends and colour scales: ≤ 2 decimals, like the browser's totals
            if c.fmt
                .is_some_and(|f| f == "TREND" || f.starts_with("COLORSCALE"))
            {
                return Some(short_number(v, l, neg));
            }
            Some(crate::table::number_text(v, &p, c.fmt, l, neg))
        })
        .collect();
    let show_totals = nrow >= 3
        && cols
            .iter()
            .any(|c| matches!(c.fmt, Some("MONEY" | "COMPACT")));

    // natural widths (in characters): the header (wrapping onto ≤ 2 lines) and
    // the formatted cells, shared out over the card
    let sample = nrow.min(200);
    let chars = |s: &str| s.chars().count() as f64;
    let nat: Vec<f64> = cols
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let head = chars(&c.p.head).min(14.0);
            let cells = (0..sample).map(|r| chars(&text(c, r))).fold(0.0, f64::max);
            let tot = totals[k].as_deref().map_or(0.0, chars);
            head.max(cells).max(tot).clamp(3.0, 32.0) + 2.5
        })
        .collect();
    let total_ch: f64 = nat.iter().sum::<f64>().max(1.0);
    let inner = w - 24.0;
    let widths: Vec<f64> = nat.iter().map(|n| inner * n / total_ch).collect();
    let xs: Vec<f64> = widths
        .iter()
        .scan(12.0, |x, wd| {
            let x0 = *x;
            *x += wd;
            Some(x0)
        })
        .collect();
    let right = |k: usize| cols[k].p.numeric && !matches!(cols[k].fmt, Some("BADGE"));
    let anchor = |k: usize| {
        if right(k) {
            (xs[k] + widths[k] - 8.0, "end")
        } else {
            (xs[k], "start")
        }
    };

    let top = if title.is_some() { 30.0 } else { 10.0 };
    let row_h = 22.0;
    let foot_h = if show_totals { row_h + 6.0 } else { 0.0 };
    let max_rows = (((h - top - 30.0 - foot_h) / row_h).floor().max(0.0) as usize).min(nrow);
    let max_rows = if max_rows < nrow {
        max_rows.saturating_sub(1) // room for "… n more rows"
    } else {
        max_rows
    };

    let mut s = svg_open(place, format!("{w:.0}"), format!("{h:.0}"));
    s.push_str(&format!(
        "<g font-family=\"{}\" font-size=\"12\">",
        esc(&th.font.family)
    ));
    if let Some(t) = title {
        s.push_str(&format!(
            "<text x=\"12\" y=\"20\" font-size=\"14\" font-weight=\"700\" fill=\"{}\">{}</text>",
            hex(th.colors.heading),
            esc(t)
        ));
    }
    // header
    for (k, c) in cols.iter().enumerate() {
        let (x, a) = anchor(k);
        s.push_str(&format!(
            "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" font-size=\"11\" font-weight=\"600\" fill=\"{muted}\">{}</text>",
            esc(&c.p.head),
            y = top + 16.0
        ));
    }
    s.push_str(&format!(
        "<line x1=\"12\" y1=\"{ly:.0}\" x2=\"{x2:.0}\" y2=\"{ly:.0}\" stroke=\"{muted}\" stroke-width=\"1.5\"/>",
        ly = top + 23.0,
        x2 = w - 12.0
    ));
    // rows
    for r in 0..max_rows {
        let y = top + 23.0 + (r as f64 + 1.0) * row_h - 6.0;
        for (k, c) in cols.iter().enumerate() {
            let (x, a) = anchor(k);
            let t = text(c, r);
            let neg_num = right(k) && c.cells.get(r).and_then(Cell::num).is_some_and(|n| n < 0.0);
            let trend = c.fmt == Some("TREND");
            let fill = match c.cells.get(r).and_then(Cell::num) {
                Some(n) if trend && n >= 0.05 => hex(th.colors.positive),
                Some(n) if trend && n <= -0.05 => hex(th.colors.negative),
                _ if neg_num && neg == Negatives::Red => hex(th.colors.negative),
                _ => ink.clone(),
            };
            let weight = if k == 0 && !c.p.numeric {
                " font-weight=\"600\""
            } else {
                ""
            };
            s.push_str(&format!(
                "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" fill=\"{fill}\"{weight}>{}</text>",
                esc(&t)
            ));
        }
        if r + 1 < max_rows {
            s.push_str(&format!(
                "<line x1=\"12\" y1=\"{ly:.0}\" x2=\"{x2:.0}\" y2=\"{ly:.0}\" stroke=\"{line}\" stroke-width=\"0.6\"/>",
                ly = y + 7.0,
                x2 = w - 12.0
            ));
        }
    }
    let mut y = top + 23.0 + max_rows as f64 * row_h;
    if nrow > max_rows {
        y += row_h;
        s.push_str(&format!(
            "<text x=\"12\" y=\"{y:.0}\" fill=\"{muted}\">… {} more rows</text>",
            nrow - max_rows,
            y = y - 6.0
        ));
    }
    if show_totals {
        // the double rule, then the totals
        let ly = y + 4.0;
        s.push_str(&format!(
            "<line x1=\"12\" y1=\"{ly:.1}\" x2=\"{x2:.0}\" y2=\"{ly:.1}\" stroke=\"{ink}\" stroke-width=\"0.8\"/>\
             <line x1=\"12\" y1=\"{ly2:.1}\" x2=\"{x2:.0}\" y2=\"{ly2:.1}\" stroke=\"{ink}\" stroke-width=\"0.8\"/>",
            ly2 = ly + 2.5,
            x2 = w - 12.0
        ));
        let ty = ly + row_h - 2.0;
        s.push_str(&format!(
            "<text x=\"12\" y=\"{ty:.0}\" font-size=\"10\" font-weight=\"700\" letter-spacing=\"0.6\" fill=\"{muted}\">TOTAL</text>"
        ));
        for (k, t) in totals.iter().enumerate() {
            if let Some(t) = t {
                let (x, a) = anchor(k);
                s.push_str(&format!(
                    "<text x=\"{x:.1}\" y=\"{ty:.0}\" text-anchor=\"{a}\" font-weight=\"700\" fill=\"{ink}\">{}</text>",
                    esc(t)
                ));
            }
        }
    }
    s.push_str("</g></svg>");
    s
}

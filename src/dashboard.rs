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
use crate::format::Locale;
use crate::sql::contract_call;
use crate::theme::{hex, luminance, mix, DashboardChrome, ResolvedTheme, Rgb, Variant};
use crate::{
    error_svg_at, render_with_at, roles, sql, svg_open, value_str, Column, Kind, Place,
    RenderOptions, Role, TableOpt,
};
use ggplot_rs::prelude::Value;
use std::path::PathBuf;
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
    /// The dashboard's own `assets/` folder, canonicalized (#514 PR B) — a
    /// relative `::IMAGE`/`::IMAGE_DARK` source inlines from it as a `data:`
    /// URI ([`crate::image::resolve_relative`]); `None` draws the alt-text
    /// placeholder for every relative source, as PR A did for all of them.
    pub asset_root: Option<PathBuf>,
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
            asset_root: None,
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
    columns_named(result, roles, &[])
}

/// [`columns_from_result`], each measure named as the query names it
/// (`sum(x) AS "Revenue"` → "Revenue"; the plan's `names`), as in the browser.
fn columns_named(
    result: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    names: &[(usize, String)],
) -> Vec<Column> {
    roles
        .iter()
        .filter_map(|(i, role)| {
            column_at(result, *i).map(|(name, vals)| {
                // An ISO-date x axis becomes a datetime scale and a date
                // ::XLINE an x position, as in the browser
                // (`sql::columns_from_rows`).
                let vals = sql::coerce_for_role(*role, vals.clone());
                let name = names
                    .iter()
                    .find(|(j, _)| j == i)
                    .map_or_else(|| name.clone(), |(_, n)| n.clone());
                Column::new(name, *role, vals)
            })
        })
        .collect()
}

/// A panel's `::TITLE`: its first non-NULL value, trimmed; none when blank
/// ([`crate::title_value`], as `anofox_render` and the static pages, #246).
fn title_of(rows: &[(String, Vec<Value>)], roles: &[(usize, Role)]) -> Option<String> {
    let (i, _) = roles.iter().find(|(_, r)| matches!(r, Role::Title))?;
    crate::title_value(&column_at(rows, *i)?.1)
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

/// Run a `::SHOW_IF` condition statement at plan index `idx` (#515): the
/// first row's cell of its `Role::ShowIf` column, through
/// [`sql::condition::holds`]. `false` for a query error, no row, no such
/// column or a non-`TRUE` value (fail closed) — the same rule everywhere.
fn show_if_holds(
    provider: &mut dyn DataProvider,
    cond_src: &[(String, Vec<(usize, Role)>)],
    idx: usize,
) -> bool {
    let Some((sql, roles)) = cond_src.get(idx) else {
        return false;
    };
    let Some((i, _)) = roles.iter().find(|(_, r)| *r == Role::ShowIf) else {
        return false;
    };
    provider
        .query(sql)
        .ok()
        .and_then(|rows| column_at(&rows, *i).and_then(|(_, vals)| vals.first().cloned()))
        .is_some_and(|v| sql::condition::holds(Some(&v)))
}

/// Run the script in order (setup for effect, panels queried) and collect
/// the dashboard chrome directives — so a `::THEME` anywhere applies to every
/// panel.
fn execute(script: &str, provider: &mut dyn DataProvider) -> (Vec<Step>, DashboardChrome) {
    // #517 S1: a dashboard reading `viewer_state` (read-back, S3) or
    // carrying `::SUBMIT` gets the server's own temps, empty — see
    // `lint::check_opts`'s matching injection and
    // `sql::script_needs_viewer_state`'s doc comment. Skipped when a real
    // (non-temp) `viewer_state`/`viewer_state_latest` already exists on
    // this connection, which seeding ours would otherwise shadow (review
    // of #530 #5).
    if sql::script_needs_viewer_state(script) {
        let shadows_a_real_table = provider
            .query(sql::viewer_state_exists_probe_sql())
            .ok()
            .and_then(|cols| cols.first().and_then(|(_, vals)| vals.first()?.as_f64()))
            .is_some_and(|n| n > 0.0);
        if !shadows_a_real_table {
            for stmt in sql::viewer_state_setup_statements() {
                let _ = provider.execute(stmt);
            }
        }
    }
    let mut chrome = DashboardChrome::default();
    let mut steps = Vec::new();
    let mut sections = sql::requires::Sections::default();
    // A static (non-interactive) render shows only a ::VIEW group's first
    // shown alternative (#214) — the other alternatives' non-setup
    // statements never even reach a query.
    let mut view_pick = sql::views::StaticPick::default();
    // `::GUIDE_STEP` (#516): each group's running count of shown markers
    // (keyed by `StepAt::group`), so a marker's own `SET VARIABLE` binds
    // the right 1-based step number as the script runs — "of N" (the
    // group's total) isn't known yet at this point; `render_inner` fixes
    // the heading text up afterwards, once every step has been collected.
    let mut step_n: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
    let plan = sql::plan(script);
    // `::SHOW_IF` (#515): each condition statement's own SQL + roles, kept
    // alongside the plan so a later statement's `show_if` indices can run
    // (and cache) that condition on demand — a condition is itself a
    // directive (`renders() == false`) and never reaches a query any other
    // way below.
    let cond_src: Vec<(String, Vec<(usize, Role)>)> = plan
        .iter()
        .map(|p| (p.sql.clone(), p.roles.clone()))
        .collect();
    let mut cond_cache: std::collections::HashMap<usize, bool> = std::collections::HashMap::new();
    for panel in plan {
        // Skip any non-setup statement whose `::SHOW_IF` conditions don't
        // all hold, before the `-- @requires` / `::VIEW` checks below (a
        // hidden statement never even reaches those).
        if !panel.setup
            && !panel.show_if.is_empty()
            && !panel.show_if.iter().all(|&c| {
                *cond_cache
                    .entry(c)
                    .or_insert_with(|| show_if_holds(provider, &cond_src, c))
            })
        {
            continue;
        }
        // A ::TAB / ::VIEW alternative whose `-- @requires` input is missing
        // is hidden: none of its panels are drawn (the tabs themselves are
        // flattened here).
        let hidden = sections.step(&panel, |reqs| {
            sql::requires::check(reqs, |q, _| {
                let cols = provider.query(q)?;
                Ok(cols
                    .into_iter()
                    .map(|(name, vals)| {
                        let v = vals
                            .first()
                            .filter(|v| !matches!(v, Value::Na))
                            .map(value_str);
                        (name, v)
                    })
                    .collect())
            })
            .missing
        });
        if hidden.is_some() || view_pick.drop_panel(&panel) {
            continue;
        }
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
            // The first row's cells (an ::ASOF also reads its 'stale=…' cell).
            let cells = provider
                .query(&panel.sql)
                .map(|cols| cols.iter().map(|(_, v)| v.first().map(value_str)).collect())
                .unwrap_or_default();
            chrome.apply_row(r, cells);
            continue;
        }
        // An input renders nothing headless, but its default value is bound
        // (`SET VARIABLE`) so downstream getvariable() panels read it — the
        // same default the browser picks on first load.
        if crate::inputs::input_kind(&panel.roles).is_some() {
            if let Ok(rows) = provider.query(&panel.sql) {
                for set in crate::inputs::panel_defaults(&panel.roles, &rows) {
                    provider.execute(&set).ok();
                }
            }
            continue;
        }
        // `::GUIDE_STEP` (#516, decision 8): the static output flattens the
        // stepper — each step is drawn in order, gates ignored. Bind this
        // step's 1-based number (among the ones shown so far) before its
        // own panels run, exactly like an input's default.
        if let Some(s) = panel.step.as_ref().filter(|s| s.marker) {
            let n = step_n.entry(s.group).or_insert(0);
            *n += 1;
            provider
                .execute(&format!("SET VARIABLE {} = {n}", s.var))
                .ok();
        }
        let has = |f: &dyn Fn(&Role) -> bool| panel.roles.iter().any(|(_, r)| f(r));
        // ::TOTALS is kept for the next table (#226), like the other layout
        // directives below, so its value reaches render_inner as a Step. A
        // surviving (default-alternative) ::VIEW marker is kept too, so
        // render_inner can apply its group's pending ::COL width (#214) —
        // it never draws a box itself (see render_inner's own check).
        // ::GROUP / ::ENDGROUP are kept too (#405): a ::GROUP's own
        // ::KPI_SIZE cascades onto its tiles (render_inner's compact_flags),
        // though neither ever draws a box itself (same comment as above).
        // A `::GUIDE_STEP` marker is kept too, so `render_inner` can draw its
        // "Step n of N · Label" heading (decision 8) — it never draws a box
        // itself either.
        let layout = has(&|r| {
            matches!(
                r,
                Role::Columns
                    | Role::Span
                    | Role::Height
                    | Role::Section
                    | Role::TableOpt(TableOpt::Totals)
                    | Role::View
                    | Role::GroupStart
                    | Role::GroupEnd
                    | Role::GuideStep
            )
        });
        if roles::is_directive_panel(&panel.roles) && !layout {
            continue; // inputs, downloads, tabs, reload, …
        }
        let rows = provider.query(&panel.sql);
        steps.push(Step { panel, rows });
    }
    (steps, chrome)
}

/// Each step's effective `::KPI_SIZE` (#405): `true` once the tile itself,
/// or the `::GROUP` box it's in, is `'compact'` — a tile's own value wins.
/// `::GROUP` / `::ENDGROUP` advance the state; they are never tiles
/// themselves. An unparsable or missing value defaults to 'normal'
/// (`dashboard --lint` warns on the former separately).
fn compact_flags(steps: &[Step]) -> Vec<bool> {
    let mut group = false;
    steps
        .iter()
        .map(|st| {
            let roles = &st.panel.roles;
            let Ok(rows) = &st.rows else { return group };
            let cols = columns_from_result(rows, roles);
            if roles.iter().any(|(_, r)| *r == Role::GroupStart) {
                group = crate::kpi::kpi_size_of(&cols).unwrap_or(false);
                false
            } else if roles.iter().any(|(_, r)| *r == Role::GroupEnd) {
                group = false;
                false
            } else {
                crate::kpi::kpi_size_of(&cols).unwrap_or(group)
            }
        })
        .collect()
}

fn render_inner(
    script: &str,
    provider: &mut dyn DataProvider,
    opts: &DashboardOptions,
) -> Result<String, String> {
    let (steps, chrome) = execute(script, provider);
    let compact = compact_flags(&steps);
    // `::GUIDE_STEP` (#516): `steps` only ever holds what `execute()` kept
    // shown, so its total marker count per group ("of N") is known now —
    // unlike `execute()`'s own pass, which can't see past the current
    // statement.
    let shown_panels: Vec<&sql::Panel> = steps.iter().map(|s| &s.panel).collect();
    let mut static_steps = sql::steps::StaticSteps::new(&shown_panels);
    let set = chrome.resolve(opts.theme.as_deref())?;
    let theme = set.pick(opts.variant).clone();
    let locale = theme
        .locale
        .as_deref()
        .and_then(Locale::from_tag)
        .unwrap_or_default();
    let t = &theme;
    // #536: this dashboard's own `-- @markdown_style` default, read once —
    // every ::MARKDOWN panel without its own ::MARKDOWN_STYLE (and without
    // ::CALLOUT, which always draws 'callout') falls back to it.
    let md_default = crate::host::serving::parse_dashboard_meta("", script).markdown_style;
    let ropts = RenderOptions {
        brand: opts.brand,
        theme: Arc::new(theme.clone()),
        locale,
        asset_root: opts.asset_root.clone(),
        ..RenderOptions::default()
    };

    let pad = opts.gap as f64;
    let gap = opts.gap as f64;
    let content_w = opts.width as f64 - 2.0 * pad;
    let mut columns = opts.columns.max(1);
    let mut default_span = (12 / columns).max(1);

    // A placed panel: (x, y, w, h, nested svg fragment positioned at x, y).
    let mut placed: Vec<(f64, f64, f64, f64, String, PanelChrome)> = Vec::new();
    // Full-width headings (no card): nested svg fragments.
    let mut headings: Vec<String> = Vec::new();
    let mut x = pad;
    let mut row_y = pad;
    let mut row_h = 0.0f64;
    let mut row_units = 0u32;
    let mut next_span = 0u32;
    let mut next_height = 0u32;
    // A ::TOTALS value for the next panel — a table reads it (#226); like the
    // browser's nextTableOpts, any next panel consumes it.
    let mut next_totals: Option<String> = None;
    // A ::VIEW group's width (#214): a ::COL pending when the group's
    // (surviving) marker is reached sets it; absent one, the group spans
    // the full row. `current_group_span` is `(group id, width)` for
    // whichever group the panel just placed belongs to, so it is cleared —
    // not leaked onto later, non-view panels — the moment a step's own
    // group no longer matches.
    let mut current_group_span: Option<(usize, u32)> = None;

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

    // KPI tiles (not text cards), for the KPI strip.
    let tiles: Vec<Option<crate::kpi::Kpi>> = steps
        .iter()
        .enumerate()
        .map(|(si, st)| {
            let rows = st.rows.as_ref().ok()?;
            if st.panel.contract.is_some() {
                return None;
            }
            let cols = columns_from_result(rows, &st.panel.roles);
            crate::kpi::Kpi::from_columns(&cols, &ropts.locale, compact[si])
                .filter(|k| !k.text_card)
        })
        .collect();
    // A run of >= 2 consecutive KPI tiles (a ::GROUP of KPIs, or the
    // browser's auto-strip) shares one row, split evenly (≤ 6 per row):
    // (tiles left, tiles per row, tile width, tile height).
    let mut strip: Option<(usize, usize, f64, f64)> = None;
    // Steps drawn as ::TRACKs under the chart before them.
    let mut consumed = std::collections::HashSet::new();

    for (si, step) in steps.iter().enumerate() {
        if consumed.contains(&si) {
            continue;
        }
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

        // Entering/leaving a ::VIEW group (#214): a group's width must not
        // leak onto a later panel once we're no longer in the group that
        // set it (whether that group's statements simply ran out, or this
        // step belongs to a different one).
        if current_group_span.is_some_and(|(g, _)| Some(g) != panel.view.as_ref().map(|v| v.group))
        {
            current_group_span = None;
        }
        // The (surviving, default-alternative) ::VIEW marker itself never
        // draws a box — it only sets its group's width: a ::COL pending
        // when the group opens, else the full row (12).
        if has(&|r| matches!(r, Role::View)) {
            if let Some(v) = &panel.view {
                current_group_span = Some((
                    v.group,
                    if next_span == 0 {
                        12
                    } else {
                        next_span.clamp(1, 12)
                    },
                ));
            }
            next_span = 0;
            continue;
        }
        // A panel inside a tracked ::VIEW group takes its group's width,
        // unless it sets its own (an inner ::COL is a deliberate escape
        // hatch, not the common case).
        if next_span == 0 {
            if let Some((g, w)) = current_group_span {
                if panel.view.as_ref().is_some_and(|v| v.group == g) {
                    next_span = w;
                }
            }
        }

        // ::GROUP / ::ENDGROUP (#405): kept only so compact_flags can read a
        // box's ::KPI_SIZE; neither draws anything here (no box: the static
        // renderer packs a run of tiles directly into a strip).
        if has(&|r| matches!(r, Role::GroupStart | Role::GroupEnd)) {
            continue;
        }
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
        if has(&|r| matches!(r, Role::TableOpt(TableOpt::Totals))) {
            // the first cell, one surrounding '"' stripped (as the browser)
            next_totals = step
                .rows
                .as_ref()
                .ok()
                .and_then(|r| first_text(r))
                .map(|s| {
                    let s = s.as_str();
                    let s = s.strip_prefix('"').unwrap_or(s);
                    s.strip_suffix('"').unwrap_or(s).to_string()
                })
                .filter(|s| !s.is_empty());
            continue;
        }
        // every later panel (also a failed one) consumes the value
        let totals_spec = next_totals.take();
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
            let title = title_of(rows, roles).unwrap_or_default();
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
                placed.push((x, row_y, cw, ch, inner, PanelChrome::Card));
                x += unit_w * span as f64;
                row_units += span;
                row_h = row_h.max(ch);
            }
            continue;
        }

        // Heading (::LABEL alone, ::SECTION, a ::GUIDE_STEP marker) →
        // full-width section title, own row (#516 decision 8: the static
        // output flattens the stepper into a run of these headings).
        let step_n_of = panel.step.as_ref().filter(|s| s.marker).and_then(|_| {
            static_steps.step(panel).into_iter().find_map(|e| match e {
                sql::steps::StaticEvent::Heading { n, of, .. } => Some((n, of)),
                _ => None,
            })
        });
        if has(&|r| matches!(r, Role::Section))
            || (roles.len() == 1 && matches!(roles[0].1, Role::Label))
            || step_n_of.is_some()
        {
            let text = match step_n_of {
                Some((n, of)) => {
                    // Read the marker's own ::GUIDE_STEP column by its role
                    // index — unlike a lone ::SECTION/::LABEL, this
                    // statement can carry ::GATE / ::GATE_MESSAGE columns
                    // too, so the label isn't necessarily column 0.
                    let label = roles
                        .iter()
                        .find(|(_, r)| *r == Role::GuideStep)
                        .and_then(|(i, _)| column_at(rows, *i))
                        .and_then(|(_, vals)| vals.first())
                        .map(value_str)
                        .unwrap_or_default();
                    if label.is_empty() {
                        format!("Step {n} of {of}")
                    } else {
                        format!("Step {n} of {of} · {label}")
                    }
                }
                None => first_text(rows).unwrap_or_default(),
            };
            if row_units > 0 {
                new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
            }
            let h = 34.0;
            headings.push(heading_svg(t, Place::At(pad, row_y), &text, content_w, h));
            row_y += h + gap;
            continue;
        }

        // A KPI strip starts: the run's tiles side by side, as tall as the
        // tallest tile.
        if strip.is_none() && next_span == 0 && tiles[si].is_some() {
            let run: Vec<&crate::kpi::Kpi> = tiles[si..].iter().map_while(|t| t.as_ref()).collect();
            if run.len() >= 2 {
                if row_units > 0 {
                    new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
                }
                let per_row = run.len().min(6);
                let tw = (content_w + gap) / per_row as f64 - gap;
                // Each tile's own floor (96 normal, 36 ::KPI_SIZE 'compact' —
                // #405; mirrored in lint.rs fit_ref and web/app.js MIN_KPI /
                // MIN_KPI_COMPACT), the tallest tile wins.
                let th = run.iter().fold(0.0f64, |a, k| {
                    a.max(k.height()).max(if k.compact { 36.0 } else { 96.0 })
                });
                strip = Some((run.len(), per_row, tw, th));
            }
        }
        // Compute geometry.
        let span = match strip {
            Some(_) => 0,
            None if next_span == 0 => default_span,
            None => next_span.clamp(1, 12),
        };
        next_span = 0;
        let md = markdown_text(rows, roles);
        let img = image_panel(rows, roles, ropts.theme.variant == Variant::Dark);
        let ch = match strip {
            _ if next_height > 0 => next_height as f64,
            Some((_, _, _, th)) => th,
            // a ::MARKDOWN box: as tall as its wrapped text (≤ the default)
            None => match (&md, &img) {
                (Some(t), _) => {
                    let mw = if span == 0 {
                        content_w
                    } else {
                        unit_w * span as f64 - gap
                    };
                    let title = roles.iter().any(|(_, r)| matches!(r, Role::Title));
                    let lines = markdown_lines(t, mw).len() as f64;
                    (lines * MD_LINE + if title { 26.0 } else { 0.0 } + 26.0)
                        .min(opts.panel_height as f64)
                }
                // an ::IMAGE box: its own width × the image's own aspect
                // (fallback 9:16), plus the title bar and caption (#514).
                (None, Some(ip)) => {
                    let mw = if span == 0 {
                        content_w
                    } else {
                        unit_w * span as f64 - gap
                    };
                    let title = roles.iter().any(|(_, r)| matches!(r, Role::Title));
                    let (aw, ah) = image_aspect(&ip.src, ropts.asset_root.as_deref());
                    let cap_h = if ip.caption.is_some() { 20.0 } else { 0.0 };
                    let title_h = if title { 26.0 } else { 0.0 };
                    // The image itself draws at `mw - 28` wide (render_panel_svg's
                    // 14 px side padding), so the aspect is computed from that,
                    // not the full card width.
                    let iw = (mw - 28.0).max(10.0);
                    (iw * (ah / aw).clamp(0.05, 20.0) + title_h + cap_h + 10.0)
                        .clamp(80.0, opts.panel_height as f64)
                }
                (None, None) => opts.panel_height as f64,
            },
        };
        next_height = 0;
        // ::TRACKs right after a chart (each optionally after a ::HEIGHT) go
        // under it in its card, which grows by their heights.
        let mut tracks: Vec<(Vec<Column>, u32)> = Vec::new();
        if is_chart(rows, roles, &ropts) {
            let mut j = si + 1;
            let mut h = 0;
            while let Some(st) = steps.get(j) {
                let r = &st.panel.roles;
                let first = || {
                    st.rows.as_ref().ok().and_then(|rows| {
                        rows.first()
                            .and_then(|(_, v)| v.first())
                            .and_then(|v| v.as_f64())
                    })
                };
                if r.iter().any(|(_, r)| *r == Role::Height) {
                    h = first().unwrap_or(0.0).clamp(0.0, 2000.0) as u32;
                } else if crate::track::is_track(r) {
                    // A track that failed (its optional input is missing) is
                    // left out.
                    let h = std::mem::take(&mut h);
                    if let Ok(rows) = &st.rows {
                        tracks.push((columns_from_result(rows, r), h));
                    }
                } else {
                    break;
                }
                consumed.insert(j);
                j += 1;
            }
            // A ::HEIGHT after the last track sizes the next panel.
            if h > 0 {
                consumed.remove(&(j - 1));
            }
        }
        let ch = ch + crate::track::heights(&tracks).iter().sum::<u32>() as f64;
        // The strip's last tile closes its row.
        let strip_ends = match &mut strip {
            Some((left, _, _, _)) => {
                *left -= 1;
                *left == 0
            }
            None => false,
        };
        let (cw, advance, units) = match strip {
            Some((_, _, tw, _)) => (tw, tw + gap, 1),
            None => ((unit_w * span as f64) - gap, unit_w * span as f64, span),
        };

        let full = match strip {
            Some(_) => x + cw > pad + content_w + 0.5,
            None => row_units + span > 12,
        };
        if full {
            new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
        }

        // Render the panel body to an inner SVG sized (cw, ch).
        let inner = render_panel_svg(
            rows,
            roles,
            &panel.names,
            Place::At(x, row_y),
            cw,
            ch,
            &ropts,
            strip.is_some(),
            &tracks,
            totals_spec.as_deref(),
            compact[si],
        );
        let chrome = panel_chrome(roles, rows, md_default.as_deref(), t);
        placed.push((x, row_y, cw, ch, inner, chrome));

        x += advance;
        row_units += units;
        row_h = row_h.max(ch);
        if strip_ends {
            strip = None;
            new_row(&mut row_y, &mut row_h, &mut x, &mut row_units);
        }
    }
    let total_h = row_y + row_h + pad;

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
    let radius = (t.radius * 0.78).round();
    for (ci, (px, py, pw, ph, inner, chrome)) in placed.iter().enumerate() {
        match chrome {
            // #536 'plain'::MARKDOWN_STYLE: no rect at all — the page
            // background (painted above, full-size) shows through.
            PanelChrome::Plain => {}
            PanelChrome::Card => out.push_str(&format!(
                "<rect x=\"{px:.1}\" y=\"{py:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" rx=\"{radius}\" \
                 fill=\"{card}\" stroke=\"{line}\" stroke-width=\"1\"/>",
                card = hex(t.colors.card),
                line = hex(t.colors.line),
            )),
            // #536 'callout'::MARKDOWN_STYLE (the default; ::CALLOUT implies
            // it): a tinted box with a left accent bar, like the browser's
            // `.md-box` — the bar is clipped to the rounded corners the same
            // way (an inner `<rect>` within a `<clipPath>` of the same
            // radius).
            PanelChrome::Callout { fill, border, bar } => {
                let clip = format!("dp-md-clip-{ci}");
                out.push_str(&format!(
                    "<clipPath id=\"{clip}\"><rect x=\"{px:.1}\" y=\"{py:.1}\" width=\"{pw:.1}\" \
                     height=\"{ph:.1}\" rx=\"{radius}\"/></clipPath>\
                     <rect x=\"{px:.1}\" y=\"{py:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" rx=\"{radius}\" \
                     fill=\"{fill}\" stroke=\"{border}\" stroke-width=\"1\"/>\
                     <rect x=\"{px:.1}\" y=\"{py:.1}\" width=\"5\" height=\"{ph:.1}\" \
                     fill=\"{bar}\" clip-path=\"url(#{clip})\"/>",
                    fill = hex(*fill),
                    border = hex(*border),
                    bar = hex(*bar),
                ));
            }
        }
        // the panel itself: a nested <svg> already positioned at (px, py)
        out.push_str(inner);
    }
    for h in &headings {
        out.push_str(h);
    }
    out.push_str("</svg>");
    Ok(out)
}

/// The header strip (logo, brand, title), or `None` when the dashboard sets
/// no branding. Returns the fragment and its height.
fn header_svg(t: &ResolvedTheme, x: f64, y: f64, w: f64) -> Option<(String, f64)> {
    let b = &t.brand;
    if b.title.is_none() && b.name.is_none() && b.logo.is_none() && b.asof.is_none() {
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
    // The ::ASOF freshness chip, right-aligned (amber text once stale).
    if let (Some(at), Some(now)) = (b.asof, crate::freshness::now_epoch()) {
        let l = t
            .locale
            .as_deref()
            .and_then(Locale::from_tag)
            .unwrap_or_default();
        let (text, stale) = crate::freshness::chip_text(at, b.stale_secs, now, &l, b.asof_date);
        s.push_str(&format!(
            "<text class=\"dp-asof\" x=\"{w:.0}\" y=\"30\" text-anchor=\"end\" font-family=\"{f}\" font-size=\"12\" font-weight=\"600\" fill=\"{c}\">{}</text>",
            esc(&text),
            f = esc(&t.font.family),
            c = hex(if stale { t.colors.warning } else { t.colors.muted }),
        ));
    }
    s.push_str("</svg>");
    Some((s, h))
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

/// [`render_panel_svg`] for a single standalone panel (`Place::Doc`) — the
/// static-HTML CLI's own renderer for a `::TABLE`/`::PAGED` panel (#427):
/// that rewrite keeps every column's own name (not `cN`, as a chart/KPI's
/// does), so the CLI's usual `columns_from_rows` + `render_with_title` path
/// can't find them by position and used to render an empty table. This
/// takes the same raw, name-keyed rows `--svg` and `serve` already read
/// (`sql::result_columns`), so a table (and its `::SPARKLINE` cells) reads
/// the same everywhere.
#[allow(clippy::too_many_arguments)]
pub fn render_panel_doc_svg(
    rows: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    names: &[(usize, String)],
    w: f64,
    h: f64,
    ropts: &RenderOptions,
    kpi_top: bool,
    tracks: &[(Vec<Column>, u32)],
    totals: Option<&str>,
    compact: bool,
) -> String {
    render_panel_svg(
        rows,
        roles,
        names,
        Place::Doc,
        w,
        h,
        ropts,
        kpi_top,
        tracks,
        totals,
        compact,
    )
}

/// One panel body → an inner `<svg>` sized (w, h). Charts go through the normal
/// renderer (with the `::TRACK`s under a chart); KPIs / text / tables get a
/// lightweight SVG.
///
/// `totals`: the `::TOTALS` value before this panel (a table's totals row).
#[allow(clippy::too_many_arguments)]
fn render_panel_svg(
    rows: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    names: &[(usize, String)],
    place: Place,
    w: f64,
    h: f64,
    ropts: &RenderOptions,
    kpi_top: bool,
    tracks: &[(Vec<Column>, u32)],
    totals: Option<&str>,
    compact: bool,
) -> String {
    let cols = columns_named(rows, roles, names);
    // Title bar (::TITLE) eats a strip off the top.
    let title = title_of(rows, roles);
    // ::INFO: an ⓘ after the title whose SVG <title> is the definition (a
    // hover tooltip wherever the SVG is shown).
    let info = roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Info))
        .and_then(|(i, _)| column_at(rows, *i))
        .and_then(|(_, v)| v.first())
        .map(value_str)
        .filter(|s| !s.trim().is_empty());
    let title = match (title, &info) {
        (None, Some(_)) => Some(String::new()),
        (t, _) => t,
    };
    let title_h = if title.is_some() { 26.0 } else { 0.0 };
    let pad = 14.0;

    // A table (before the KPI check: a table's ::MONEY / ::PERCENT columns are
    // column formats, not a KPI).
    if roles
        .iter()
        .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable))
    {
        return table_svg(rows, roles, place, w, h, title.as_deref(), totals, ropts);
    }

    // A KPI / text card — the shared tile (crate::kpi), also used by the
    // static HTML.
    if let Some(k) = crate::kpi::Kpi::from_columns(&cols, &ropts.locale, compact) {
        return format!(
            "{}{}</svg>",
            svg_open(place, format!("{w:.0}"), format!("{h:.0}")),
            if kpi_top {
                k.body_top(w, h, ropts)
            } else {
                k.body(w, h, ropts)
            }
        );
    }

    // A ::MARKDOWN box: its text, plain and wrapped (no rich formatting, no
    // TeX typesetting, headless) — but its `{{col}}` / `{{col:fmt}}`
    // placeholders are still filled from the statement's own row (#327:
    // they used to print literally, e.g. `{{z}}`), under the title.
    if let Some(md) = markdown_text(rows, roles) {
        let md = markdown_fill(&md, rows, &ropts.locale);
        let t = &ropts.theme;
        let mut svg = svg_open(place, format!("{w:.0}"), format!("{h:.0}"));
        if let Some(tt) = &title {
            svg.push_str(&format!(
                "<text x=\"{pad}\" y=\"20\" font-family=\"{}\" font-size=\"14\" font-weight=\"700\" fill=\"{}\">{}</text>",
                esc(&t.font.display),
                hex(t.colors.heading),
                esc(tt),
            ));
        }
        svg.push_str(&format!(
            "<g class=\"md-text\" font-family=\"{}\" font-size=\"13\" fill=\"{}\">",
            esc(&t.font.family),
            hex(t.colors.ink)
        ));
        let mut y = title_h + 22.0;
        for line in markdown_lines(&md, w) {
            if y > h - 8.0 {
                break;
            }
            if !line.is_empty() {
                svg.push_str(&format!(
                    "<text x=\"{pad}\" y=\"{y:.0}\">{}</text>",
                    markdown_line_svg(&line)
                ));
            }
            y += MD_LINE;
        }
        svg.push_str("</g></svg>");
        return svg;
    }

    // A static ::IMAGE / ::IMAGE_DARK panel (#514): an SVG <image> inside the
    // panel frame, with the title bar and an optional ::LABEL caption like
    // other panels — panel composition, not a new SVG writer for charts
    // (plotting stays through ggplot-rs). The source is always drawn through
    // `<image href=…>`, never inlined into the DOM, `<object>` or `<iframe>`:
    // a browser (and this static renderer) runs no script and loads no
    // external resource from inside an SVG used as an image this way.
    if let Some(img) = image_panel(rows, roles, ropts.theme.variant == Variant::Dark) {
        let t = &ropts.theme;
        let mut svg = svg_open(place, format!("{w:.0}"), format!("{h:.0}"));
        if let Some(tt) = &title {
            let mark = info
                .as_deref()
                .map(|i| {
                    format!(
                        "<tspan class=\"dp-info\" fill=\"{m}\" font-weight=\"400\">{sp}ⓘ<title>{}</title></tspan>",
                        esc(i),
                        m = hex(t.colors.muted),
                        sp = if tt.is_empty() { "" } else { " " },
                    )
                })
                .unwrap_or_default();
            svg.push_str(&format!(
                "<text x=\"{pad}\" y=\"20\" font-family=\"{f}\" font-size=\"14\" font-weight=\"700\" fill=\"{hd}\">{}{mark}</text>",
                esc(tt),
                f = esc(&t.font.display),
                hd = hex(t.colors.heading),
            ));
        }
        let cap_h = if img.caption.is_some() { 20.0 } else { 0.0 };
        let ix = pad;
        let iy = title_h + 4.0;
        let iw = (w - 2.0 * pad).max(1.0);
        let ih = (h - iy - cap_h - 6.0).max(1.0);
        match image_href(&img.src, ropts.asset_root.as_deref()) {
            Some(href) => {
                let alt_title = if img.alt.is_empty() {
                    String::new()
                } else {
                    format!("<title>{}</title>", esc(&img.alt))
                };
                svg.push_str(&format!(
                    "<image href=\"{}\" x=\"{ix:.1}\" y=\"{iy:.1}\" width=\"{iw:.1}\" height=\"{ih:.1}\" preserveAspectRatio=\"xMidYMid meet\">{alt_title}</image>",
                    esc(&href),
                ));
            }
            None => {
                let label = if img.alt.is_empty() {
                    "image"
                } else {
                    &img.alt
                };
                svg.push_str(&format!(
                    "<rect x=\"{ix:.1}\" y=\"{iy:.1}\" width=\"{iw:.1}\" height=\"{ih:.1}\" fill=\"none\" stroke=\"{muted}\" stroke-dasharray=\"4 3\" rx=\"4\"/><text x=\"{cx:.1}\" y=\"{cy:.1}\" font-family=\"{f}\" font-size=\"12\" fill=\"{muted}\" text-anchor=\"middle\" font-style=\"italic\">{}</text>",
                    esc(label),
                    muted = hex(t.colors.muted),
                    f = esc(&t.font.family),
                    cx = ix + iw / 2.0,
                    cy = iy + ih / 2.0,
                ));
            }
        }
        if let Some(cap) = &img.caption {
            svg.push_str(&format!(
                "<text x=\"{pad}\" y=\"{y:.1}\" font-family=\"{f}\" font-size=\"12\" fill=\"{muted}\">{}</text>",
                esc(cap),
                f = esc(&t.font.family),
                muted = hex(t.colors.muted),
                y = h - 6.0,
            ));
        }
        svg.push_str("</svg>");
        return svg;
    }

    // Otherwise a chart — render at the content area, then wrap with a title.
    // Its ::TRACKs sit right under it, sharing its plot area and x window.
    let tracks_h: u32 = crate::track::heights(tracks).iter().sum();
    let body_h = (h - title_h - pad - tracks_h as f64).max(30.0);
    // The chart (and its tracks) are nested below the title as one
    // positioned fragment.
    let chart = crate::track::stack(
        &cols,
        tracks,
        w as u32,
        body_h as u32,
        ropts,
        Place::At(0.0, title_h),
    )
    .map(|(svgs, _)| svgs.concat())
    .unwrap_or_else(|e| error_svg_at(Place::At(0.0, title_h), &e.to_string(), w as u32));
    let mut svg = svg_open(place, format!("{w:.0}"), format!("{h:.0}"));
    if let Some(tt) = &title {
        let mark = info
            .as_deref()
            .map(|i| {
                format!(
                    "<tspan class=\"dp-info\" fill=\"{m}\" font-weight=\"400\">{sp}ⓘ<title>{}</title></tspan>",
                    esc(i),
                    m = hex(ropts.theme.colors.muted),
                    sp = if tt.is_empty() { "" } else { " " },
                )
            })
            .unwrap_or_default();
        svg.push_str(&format!(
            "<text x=\"{pad}\" y=\"20\" font-family=\"{f}\" font-size=\"14\" font-weight=\"700\" fill=\"{hd}\">{}{mark}</text>",
            esc(tt),
            f = esc(&ropts.theme.font.display),
            hd = hex(ropts.theme.colors.heading),
        ));
    }
    svg.push_str(&chart);
    svg.push_str("</svg>");
    svg
}

/// Does this panel draw a chart (that `::TRACK`s can go under)?
fn is_chart(rows: &[(String, Vec<Value>)], roles: &[(usize, Role)], o: &RenderOptions) -> bool {
    let has = |f: &dyn Fn(&Role) -> bool| roles.iter().any(|(_, r)| f(r));
    has(&|r| matches!(r, Role::Value(_)))
        && !has(&|r| {
            matches!(
                r,
                Role::Table
                    | Role::PagedTable
                    | Role::Markdown
                    | Role::Track
                    | Role::Image
                    | Role::ImageDark
            )
        })
        // `compact` doesn't change whether this is a KPI, only how it draws.
        && crate::kpi::Kpi::from_columns(&columns_from_result(rows, roles), &o.locale, false).is_none()
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

/// Shorten a table header that's wider than its own column to an ellipsis
/// (#351) — `None` when it already fits `avail_px`, so the caller can skip
/// the hover `<title>` too. Uses ggplot-rs's own (Hanken Grotesk) text-width
/// estimate (the same heuristic `axis_text_x_check_overlap` measures tick
/// labels with), nudged up a little for the header's semibold weight, which
/// is a touch wider than the regular weight the table estimates.
fn truncate_header(head: &str, size: f64, avail_px: f64) -> Option<String> {
    let fits = |s: &str| ggplot_rs::render::text::text_width(s, size) * 1.08 <= avail_px;
    if fits(head) {
        return None;
    }
    let chars: Vec<char> = head.chars().collect();
    for n in (0..chars.len()).rev() {
        let cand: String = chars[..n].iter().collect::<String>() + "\u{2026}";
        if fits(&cand) {
            return Some(cand);
        }
    }
    Some("\u{2026}".to_string())
}

/// The headless table: the browser table's formatting decisions (shared
/// [`crate::table`] module — humanised headers, one unit per money / compact
/// column moved into the header, aligned decimals, a true minus sign, ids and
/// years ungrouped), numbers right-aligned, natural column widths, and the
/// totals row under a double rule — the browser's rule: a `::TOTALS` value
/// (`totals_spec`; a column it names, else its bare aggregation, else the
/// column's default) and ≥ 2 rows, or without one the defaults with ≥ 3 rows
/// and an amount column; `'none'` hides it. Badge counts are browser-only.
/// As many rows as fit, then "… n more rows". Skips a ::TITLE column.
#[allow(clippy::too_many_arguments)]
fn table_svg(
    rows: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    place: Place,
    w: f64,
    h: f64,
    title: Option<&str>,
    totals_spec: Option<&str>,
    ropts: &RenderOptions,
) -> String {
    use crate::table::{
        aggregate, count_text, default_total, display_text, names_column, number_text,
        parse_totals, profile_column, short_number, Cell, ColumnSpec, Negatives,
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
        .filter(|(_, r)| matches!(r, Role::Title | Role::Info))
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
    // `{value}`: the row's first column, as the browser's table reads it
    // (`k0` in `web/app.js` `renderTable`) — the first not already dropped
    // as a `::TITLE` / `::INFO` column.
    let value_idx = (0..rows.len()).find(|i| !skip.contains(i));
    let text = |c: &Col, r: usize| {
        c.cells
            .get(r)
            .map(|v| display_text(v, &c.p, c.fmt, l, neg))
            .unwrap_or_default()
    };
    // ::TOTALS (#226), read as the browser does (web/app.js renderTable):
    // per column the named aggregation (raw name or header, case-insensitive;
    // a later entry wins), else the bare one, else the column's default
    let tspec = totals_spec.map(parse_totals).unwrap_or_default();
    let mut named: std::collections::HashMap<usize, &str> = std::collections::HashMap::new();
    for (n, a) in &tspec.col {
        if let Some(k) = vis.iter().position(|(_, (name, _))| names_column(name, n)) {
            named.insert(k, a.as_str());
        }
    }
    let totals_off = tspec.all.as_deref() == Some("none") && named.values().all(|a| *a == "none");
    // totals: a numeric, non-label column after the first (badges: no
    // headless count); 'none' leaves the cell empty
    let totals: Vec<Option<String>> = cols
        .iter()
        .enumerate()
        .map(|(k, c)| {
            if k == 0 || !c.p.numeric || c.p.label || c.fmt == Some("BADGE") {
                return None;
            }
            let agg = match named.get(&k).copied().or(tspec.all.as_deref()) {
                Some(a) => a,
                None => default_total(&c.p, c.fmt)?,
            };
            if agg == "none" {
                return None;
            }
            let nums: Vec<f64> = c.cells.iter().filter_map(Cell::num).collect();
            if agg == "count" {
                return Some(count_text(nums.len(), l));
            }
            let v = aggregate(&nums, agg)?;
            // a mean / median of whole numbers keeps one decimal
            let mut p = c.p.clone();
            if matches!(agg, "mean" | "median")
                && p.unit.is_none()
                && p.dec == 0
                && v.fract() != 0.0
            {
                p.dec = 1;
            }
            // trends and colour scales: ≤ 2 decimals, like the browser's totals
            if c.fmt
                .is_some_and(|f| f == "TREND" || f.starts_with("COLORSCALE"))
            {
                return Some(short_number(v, l, neg));
            }
            Some(number_text(v, &p, c.fmt, l, neg))
        })
        .collect();
    let show_totals = !totals_off
        && if totals_spec.is_some() {
            nrow >= 2
        } else {
            nrow >= 3
                && cols
                    .iter()
                    .any(|c| matches!(c.fmt, Some("MONEY" | "COMPACT")))
        };

    // natural widths (in characters): the header (wrapping onto ≤ 2 lines) and
    // the formatted cells, shared out over the card. A ::SPARKLINE/::SPARKBAR
    // cell draws a small chart, not text — it gets a fixed pixel width
    // instead of one measured from its (JSON) cell text, reserved up front
    // so the other columns still split the rest by their own natural width
    // (same approach as #427's SPARKLINE fix).
    let sample = nrow.min(200);
    let chars = |s: &str| s.chars().count() as f64;
    let is_spark = |k: usize| matches!(cols[k].fmt, Some("SPARKLINE" | "SPARKBAR"));
    const SPARK_COL_W: f64 = 92.0;
    let nat: Vec<f64> = cols
        .iter()
        .enumerate()
        .map(|(k, c)| {
            if is_spark(k) {
                return 0.0;
            }
            let head = chars(&c.p.head).min(14.0);
            let cells = (0..sample).map(|r| chars(&text(c, r))).fold(0.0, f64::max);
            let tot = totals[k].as_deref().map_or(0.0, chars);
            head.max(cells).max(tot).clamp(3.0, 32.0) + 2.5
        })
        .collect();
    let total_ch: f64 = nat.iter().sum::<f64>().max(1.0);
    let spark_w: f64 = (0..cols.len()).filter(|&k| is_spark(k)).count() as f64 * SPARK_COL_W;
    let inner = (w - 24.0 - spark_w).max(0.0);
    let widths: Vec<f64> = (0..cols.len())
        .map(|k| {
            if is_spark(k) {
                SPARK_COL_W
            } else {
                inner * nat[k] / total_ch
            }
        })
        .collect();
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
    // #457: a nested sparkline/sparkbar panel can't be asked for less than
    // `SPARKLINE_MIN_H` (50px, see its doc comment on ggplot-rs's own
    // layout floor) without clipping the line's low points (or, for
    // ::SPARKBAR, overflowing its own row — render_sparkbar has no #457-
    // style internal clamp of its own yet, #453 review), so a table with
    // one of these columns gets taller rows to fit it without spilling
    // into the next one.
    let row_h = if cols.iter().enumerate().any(|(k, _)| is_spark(k)) {
        crate::SPARKLINE_MIN_H + 6.0
    } else {
        22.0
    };
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
    // header — a header wider than its own column is shortened to an
    // ellipsis instead of overlapping the next one (#351); the full text is
    // still there on hover, via `<title>` (only when actually shortened, so
    // a dashboard with no long headers renders byte-identical to before).
    // The column's own natural width (`nat`) already hugs its header/cell
    // text with only a little padding, so truncating right up to that same
    // width would still leave adjacent headers touching edge to edge — an
    // explicit gutter (one text-size's worth) keeps a visible gap, on
    // either side, whichever side a right-aligned (numeric) header grows
    // into.
    let gutter = 11.0;
    for (k, c) in cols.iter().enumerate() {
        let (x, a) = anchor(k);
        let avail = (widths[k] - gutter).max(10.0);
        let short = truncate_header(&c.p.head, 11.0, avail);
        let shown = short.as_deref().unwrap_or(&c.p.head);
        let title = match &short {
            Some(_) => format!("<title>{}</title>", esc(&c.p.head)),
            None => String::new(),
        };
        s.push_str(&format!(
            "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" font-size=\"11\" font-weight=\"600\" fill=\"{muted}\">{}{title}</text>",
            esc(shown),
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
            // A `::LINK` cell: its raw value is a drill-through template —
            // fill it from this row ({value} = the row's first column,
            // {column} any named column), then draw a real `<a>` so the
            // link works without a script (#327: it used to print the
            // unfilled template, e.g. `?v.supplier={value}`, as plain text).
            if c.fmt == Some("LINK") {
                let raw = c
                    .cells
                    .get(r)
                    .map(|v| match v {
                        Cell::Text(s) => s.clone(),
                        _ => text(c, r),
                    })
                    .unwrap_or_default();
                let filled = fill_link_row(&raw, rows, r, value_idx);
                let label = esc(&link_label(&filled));
                match headless_link_href(&filled) {
                    Some(href) => s.push_str(&format!(
                        "<a href=\"{href}\"><text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" fill=\"{link}\" text-decoration=\"underline\">{label}</text></a>",
                        href = esc(&href),
                        link = hex(th.colors.link),
                    )),
                    None => s.push_str(&format!(
                        "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" fill=\"{muted}\">{label}</text>"
                    )),
                }
                continue;
            }
            // A `::SPARKLINE` cell (#427): a list of numbers drawn as a tiny
            // trend, through the same ggplot-rs path as a standalone
            // `::SPARKLINE` panel ([`crate::render_sparkline`], via
            // [`render_with_at`]) — no hand-written SVG here. Fewer than 2
            // numbers (the browser's own cut-off) draws the usual dash.
            if c.fmt == Some("SPARKLINE") {
                let nums = c.cells.get(r).map_or_else(Vec::new, |cell| match cell {
                    Cell::Text(s) => crate::kpi::spark_values(&Value::Str(s.clone())),
                    _ => Vec::new(),
                });
                if nums.len() < 2 {
                    s.push_str(&format!(
                        "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" fill=\"{muted}\">\u{2014}</text>"
                    ));
                    continue;
                }
                let row_top = top + 23.0 + r as f64 * row_h;
                let sw = (widths[k] - 6.0).max(32.0);
                // `SPARKLINE_MIN_H` itself, not `row_h - 6.0`: a shorter
                // request still gets clamped up to it inside
                // `render_sparkline` (#457), which would otherwise spill
                // past this row's own height without this cell centring
                // the taller panel on its own.
                let sh = crate::SPARKLINE_MIN_H;
                let sx = xs[k] + 2.0;
                let sy = row_top + (row_h - sh) / 2.0;
                let spark_col = Column::new(
                    "value",
                    Role::Value(Kind::Sparkline),
                    nums.into_iter().map(Value::Float).collect(),
                );
                match render_with_at(&[spark_col], sx, sy, sw as u32, sh as u32, ropts) {
                    Ok(frag) => s.push_str(&frag),
                    Err(_) => s.push_str(&format!(
                        "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" fill=\"{muted}\">\u{2014}</text>"
                    )),
                }
                continue;
            }
            // A `::SPARKBAR` cell (#425): a list of numbers (one series) or
            // `{'ref','cur'}` objects (two overlaid series) drawn as a tiny
            // bar histogram, through the same ggplot-rs path as a standalone
            // `::SPARKBAR` panel (`render_sparkbar`, via `render_with_at`) —
            // no hand-written SVG here. No data draws the usual dash.
            if c.fmt == Some("SPARKBAR") {
                let raw = match c.cells.get(r) {
                    Some(Cell::Text(s)) => Some(s.clone()),
                    _ => None,
                };
                let spark_cols: Vec<Column> = raw
                    .as_deref()
                    .map(|s| {
                        let v = Value::Str(s.to_string());
                        let pairs = crate::kpi::spark_pairs(&v);
                        if !pairs.is_empty() {
                            vec![
                                Column::new(
                                    "ref",
                                    Role::Value(Kind::Sparkbar),
                                    pairs.iter().map(|(r, _)| Value::Float(*r)).collect(),
                                ),
                                Column::new(
                                    "cur",
                                    Role::Value(Kind::Sparkbar),
                                    pairs.iter().map(|(_, c)| Value::Float(*c)).collect(),
                                ),
                            ]
                        } else {
                            let nums = crate::kpi::spark_values(&v);
                            vec![Column::new(
                                "value",
                                Role::Value(Kind::Sparkbar),
                                nums.into_iter().map(Value::Float).collect(),
                            )]
                        }
                    })
                    .unwrap_or_default();
                let n_vals = spark_cols.first().map_or(0, |c| c.values.len());
                if n_vals == 0 {
                    s.push_str(&format!(
                        "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" fill=\"{muted}\">\u{2014}</text>"
                    ));
                    continue;
                }
                let row_top = top + 23.0 + r as f64 * row_h;
                let sw = (widths[k] - 6.0).max(32.0);
                // `SPARKLINE_MIN_H` itself, not `row_h - 6.0`: a shorter
                // request still gets clamped up to it inside
                // `render_sparkbar`, which would otherwise spill past this
                // row's own height without this cell centring the taller
                // panel on its own (same reasoning as the `::SPARKLINE`
                // cell above, #457/#425).
                let sh = crate::SPARKLINE_MIN_H;
                let sx = xs[k] + 2.0;
                let sy = row_top + (row_h - sh) / 2.0;
                match render_with_at(&spark_cols, sx, sy, sw as u32, sh as u32, ropts) {
                    Ok(frag) => s.push_str(&frag),
                    Err(_) => s.push_str(&format!(
                        "<text x=\"{x:.1}\" y=\"{y:.0}\" text-anchor=\"{a}\" fill=\"{muted}\">\u{2014}</text>"
                    )),
                }
                continue;
            }
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

/// `{name}` placeholders of a `::LINK` table cell, filled from the row at
/// `r`: `{value}` is the row's first column (`value_idx`, as the browser's
/// table reads it — `k0` in `web/app.js` `renderTable`), `{column}` any
/// named column of the same row — URL-encoded. Mirrors the browser's
/// `fillLink` / `linkCell` (web/app.js), so a headless/served table link
/// cell fills the same template the interactive one does (#327).
fn fill_link_row(
    tpl: &str,
    rows: &[(String, Vec<Value>)],
    r: usize,
    value_idx: Option<usize>,
) -> String {
    let lookup = |name: &str| -> String {
        let v = if name == "value" {
            value_idx
                .and_then(|i| rows.get(i))
                .and_then(|(_, vs)| vs.get(r))
        } else {
            // The row's own column names — a case-sensitive lookup, as a JS
            // object property read is (`vars[k]` in web/app.js `linkCell`).
            rows.iter()
                .find(|(n, _)| n == name)
                .and_then(|(_, vs)| vs.get(r))
        };
        v.map(value_str).unwrap_or_default()
    };
    let chars: Vec<char> = tpl.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            let start = i + 1;
            let mut j = start;
            if j < chars.len() && (chars[j].is_ascii_alphabetic() || chars[j] == '_') {
                j += 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j < chars.len() && chars[j] == '}' {
                    let name: String = chars[start..j].iter().collect();
                    out.push_str(&url_encode_component(&lookup(&name)));
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `encodeURIComponent`-equivalent percent-encoding (the unreserved set:
/// letters, digits, `- _ . ! ~ * ' ( )`), matching the browser's `fillLink`.
fn url_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The filled `::LINK` target as an `href` for the headless/served page,
/// which has no tabs (every `::TAB`'s panels are flattened onto one page —
/// see `execute` / `src/bin/serve.rs` `render_dashboard_page`): `#Tab?v.x=1`
/// keeps only `?v.x=1` (a same-page reload that sets the variable — there is
/// no tab to jump to), `?v.x=1`, `/d/<id>?…` and an `http(s)://` URL are kept
/// whole. `None` when there is nothing safe or useful to link to (another
/// scheme, or a bare `#Tab` with no variable to set).
fn headless_link_href(url: &str) -> Option<String> {
    // A backslash, control character or non-space whitespace anywhere in the
    // part we actually emit lets a browser reinterpret the URL (e.g.
    // `/\evil.example` is read as `//evil.example`, an off-site,
    // protocol-relative link; a tab/LF/CR is stripped by the WHATWG URL
    // parser, which can smuggle a scheme change past this prefix check) even
    // though the checks below see a safe-looking prefix. A literal ASCII
    // space has none of those effects — a URL parser percent-encodes it in
    // place rather than stripping or reinterpreting it — so it is encoded
    // below instead of failing the whole link (#340 C).
    let has_unsafe_char = |s: &str| {
        s.chars()
            .any(|c| c == '\\' || (c.is_whitespace() && c != ' ') || c.is_control())
    };
    let encode_space = |s: &str| s.replace(' ', "%20");
    let url = url.trim();
    if let Some(rest) = url.strip_prefix('#') {
        // The tab name before `?` is discarded headless (see the doc
        // comment) and may legitimately contain spaces (`#Stock & pipeline`),
        // so only the emitted query string needs the unsafe-char check.
        let q = rest.find('?')?;
        let qs = encode_space(&rest[q..]);
        return (qs.len() > 1 && !has_unsafe_char(&qs)).then_some(qs);
    }
    let url = encode_space(url);
    if has_unsafe_char(&url) {
        return None;
    }
    if url.starts_with('?') || (url.starts_with('/') && !url.starts_with("//")) {
        return Some(url);
    }
    let low = url.to_ascii_lowercase();
    if low.starts_with("http://") || low.starts_with("https://") {
        return Some(url);
    }
    None
}

/// The cell's visible text for a filled `::LINK` — never the raw URL (an
/// internal `?v.x=…` / `#Tab…` is plumbing, not something to show), matching
/// the browser's `linkCell` label (web/app.js): the tab name for `#Tab…`,
/// "Open ↗" for an external `http(s)` URL, else "Open →".
fn link_label(url: &str) -> String {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix('#') {
        let tab = rest.split('?').next().unwrap_or("").trim();
        if !tab.is_empty() {
            return format!("{tab} →");
        }
    }
    let low = url.to_ascii_lowercase();
    if low.starts_with("http://") || low.starts_with("https://") {
        "Open ↗".to_string()
    } else {
        "Open →".to_string()
    }
}

/// Line height of a headless ::MARKDOWN box.
const MD_LINE: f64 = 18.0;

/// A static ::IMAGE panel's resolved content (#514).
struct ImagePanel {
    src: String,
    alt: String,
    caption: Option<String>,
}

/// `rows`/`roles` as an ::IMAGE panel, if it is one. `dark` picks
/// ::IMAGE_DARK when set, else falls back to ::IMAGE. `::ALT` is read even
/// when `''` (decorative — no fallback then); when ::ALT is absent
/// entirely, the alt text falls back to ::TITLE, then ::LABEL (decision 4).
fn image_panel(
    rows: &[(String, Vec<Value>)],
    roles: &[(usize, Role)],
    dark: bool,
) -> Option<ImagePanel> {
    let text_of = |role: Role| -> Option<String> {
        let (i, _) = roles.iter().find(|(_, r)| *r == role)?;
        column_at(rows, *i)
            .and_then(|(_, v)| v.first())
            .map(value_str)
    };
    let src = (dark.then(|| text_of(Role::ImageDark)).flatten())
        .or_else(|| text_of(Role::Image))
        .filter(|s| !s.trim().is_empty())?;
    let alt = if roles.iter().any(|(_, r)| *r == Role::Alt) {
        text_of(Role::Alt).unwrap_or_default()
    } else {
        text_of(Role::Title)
            .or_else(|| text_of(Role::Label))
            .unwrap_or_default()
    };
    let caption = text_of(Role::Label).filter(|s| !s.trim().is_empty());
    Some(ImagePanel { src, alt, caption })
}

/// `src`'s href for a static `<image href=…>`: `data:`/`https:` sources are
/// used as-is; a relative path is inlined from `asset_root`'s `assets/`
/// folder as a `data:` URI (PR B: [`crate::image::resolve_relative`]) when
/// one is given and accepts it; anything else — no root, a path not under
/// `assets/`, a refused file, or anything [`crate::image::src_ok`] itself
/// rejects — draws the alt-text placeholder instead (`None`).
fn image_href(src: &str, asset_root: Option<&std::path::Path>) -> Option<String> {
    use crate::image::{resolve_relative, src_ok, SrcKind};
    match src_ok(src) {
        SrcKind::Data(_) | SrcKind::Https => Some(src.to_string()),
        SrcKind::Relative => resolve_relative(src, asset_root)
            .map(|(mime, bytes)| crate::host::app_brand::data_uri(&mime, &bytes)),
        SrcKind::Bad(_) => None,
    }
}

/// `src`'s own aspect (width, height): decoded from a `data:` payload, or
/// (PR B) read from `asset_root`'s `assets/` folder for a relative path —
/// either way via [`crate::image::intrinsic_size`]. [`crate::image::FALLBACK_ASPECT`]
/// when there's nothing to read (an `https:`/bad source, no root, or a size
/// that can't be read).
fn image_aspect(src: &str, asset_root: Option<&std::path::Path>) -> (f64, f64) {
    use crate::image::{
        decode_data_uri, intrinsic_size, resolve_relative, src_ok, SrcKind, FALLBACK_ASPECT,
    };
    let payload = match src_ok(src) {
        SrcKind::Data(_) => decode_data_uri(src),
        SrcKind::Relative => resolve_relative(src, asset_root),
        SrcKind::Https | SrcKind::Bad(_) => None,
    };
    payload
        .and_then(|(mime, bytes)| intrinsic_size(&bytes, &mime))
        .unwrap_or(FALLBACK_ASPECT)
}

/// The text of a ::MARKDOWN panel, if it is one.
fn markdown_text(rows: &[(String, Vec<Value>)], roles: &[(usize, Role)]) -> Option<String> {
    let (i, _) = roles.iter().find(|(_, r)| matches!(r, Role::Markdown))?;
    column_at(rows, *i)
        .and_then(|(_, v)| v.first())
        .map(value_str)
}

/// How a panel's outer SVG `<rect>` is drawn (#536): today's bordered card
/// (`Card` — every panel but a styled `::MARKDOWN`), nothing at all
/// (`Plain` — the page background shows through) or a tinted box with a
/// left accent bar (`Callout`). Mirrors the browser's `.md-box` / `.md-
/// plain` (web/theme-base.css) and `resolveMdStyle()` (web/app.js); the
/// icon itself stays browser-only.
enum PanelChrome {
    Card,
    Plain,
    Callout { fill: Rgb, border: Rgb, bar: Rgb },
}

/// `roles`/`rows`'s [`PanelChrome`]: `Card` for every panel that isn't a
/// `::MARKDOWN`; otherwise [`crate::roles::resolve_markdown_style`] of its
/// own `::MARKDOWN_STYLE` cell, `::CALLOUT`'s presence and `md_default`
/// (the dashboard's own `-- @markdown_style`). A static render has no
/// backdrop to alpha-blend a translucent wash against, so the tinted fill
/// approximates the browser's look by mixing the kind's colour into the
/// card colour instead — the same recipe as the existing `badge-warn-bg` /
/// `badge-good-bg` tokens (`src/theme.rs`'s `derived_tokens`).
fn panel_chrome(
    roles: &[(usize, Role)],
    rows: &[(String, Vec<Value>)],
    md_default: Option<&str>,
    t: &ResolvedTheme,
) -> PanelChrome {
    if !roles.iter().any(|(_, r)| matches!(r, Role::Markdown)) {
        return PanelChrome::Card;
    }
    let text_of = |role: Role| -> Option<String> {
        let (i, _) = roles.iter().find(|(_, r)| *r == role)?;
        column_at(rows, *i)
            .and_then(|(_, v)| v.first())
            .map(value_str)
    };
    let explicit = text_of(Role::MarkdownStyle);
    let callout_present = roles.iter().any(|(_, r)| matches!(r, Role::Callout));
    let style =
        crate::roles::resolve_markdown_style(explicit.as_deref(), callout_present, md_default);
    match style {
        "plain" => PanelChrome::Plain,
        "card" => PanelChrome::Card,
        _ => {
            let kind = crate::roles::callout_kind(&text_of(Role::Callout).unwrap_or_default());
            let dark = luminance(t.colors.card) < 0.25;
            let m = if dark { 0.22 } else { 0.12 };
            let base = match kind {
                "warning" => t.colors.warning,
                "success" => t.colors.positive,
                _ => t.colors.accent,
            };
            PanelChrome::Callout {
                fill: mix(t.colors.card, base, m),
                border: mix(t.colors.card, base, (m * 1.5).min(1.0)),
                bar: base,
            }
        }
    }
}

/// `{{name}}` / `{{name:fmt}}` in a ::MARKDOWN panel's text, filled from the
/// statement's own row (`crate::sql::MD_PREFIX`'s `"md:name"` columns, else a
/// `*`-expanded column of that name — `crate::lint::md_column`). The headless
/// / served counterpart of the browser's `markdownWithValues` (web/app.js),
/// minus the rich-text rendering itself — only the value substitution, which
/// does not depend on a script. An unknown column, or anything that is not a
/// `{{…}}` placeholder (a `$…$` TeX formula, `**bold**`, …), is left exactly
/// as written for `markdown_lines` below to handle (#327).
fn markdown_fill(md: &str, rows: &[(String, Vec<Value>)], l: &Locale) -> String {
    let keys: Vec<&String> = rows.iter().map(|(n, _)| n).collect();
    let mut out = String::new();
    let mut rest = md;
    loop {
        let Some(a) = rest.find("{{") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..a]);
        let Some(b) = rest[a + 2..].find("}}") else {
            out.push_str(&rest[a..]);
            break;
        };
        let inner = &rest[a + 2..a + 2 + b];
        let next = &rest[a + 2 + b + 2..];
        let (name, spec) = match inner.split_once(':') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        let ident = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ' ');
        let filled = ident
            .then(|| crate::lint::md_column(keys.iter().copied(), name))
            .flatten()
            .and_then(|key| rows.iter().find(|(n, _)| n == key))
            .and_then(|(_, vs)| vs.first())
            .map(|v| md_format_value(v, spec, l));
        match filled {
            Some(text) => out.push_str(&text),
            None => {
                out.push_str("{{");
                out.push_str(inner);
                out.push_str("}}");
            }
        }
        rest = next;
    }
    out
}

/// One `{{col:fmt}}` value: money / percent / compact / metric (numbers, as
/// the KPI roles), `text` as written, `date` as the stored value reads
/// (headless: no locale reformat); no spec = a plain grouped number or the
/// value as written. Mirrors the browser's `mdValue` (web/app.js).
fn md_format_value(v: &Value, spec: Option<&str>, l: &Locale) -> String {
    if matches!(v, Value::Na) {
        return "–".to_string();
    }
    let spec = spec.map(|s| s.to_ascii_lowercase());
    if let Some(fmt) = spec.as_deref().and_then(md_metric_fmt) {
        if let Some(n) = v.as_f64() {
            return crate::format::format_number_l(n, fmt, l);
        }
    }
    match spec.as_deref() {
        Some("text") | Some("date") => value_str(v),
        _ => match v.as_f64() {
            Some(n) => crate::format::format_number_l(n, crate::MetricFmt::Plain, l),
            None => value_str(v),
        },
    }
}

/// `{{col:fmt}}` → [`MetricFmt`](crate::MetricFmt) (`web/app.js` `MD_FMT`).
fn md_metric_fmt(spec: &str) -> Option<crate::MetricFmt> {
    use crate::MetricFmt::*;
    Some(match spec {
        "money" | "currency" | "usd" | "eur" | "dollar" => Money,
        "percent" | "pct" => Percent,
        "compact" => Compact,
        "metric" | "number" | "num" => Plain,
        _ => return None,
    })
}

/// A forced line break in [`markdown_lines`]' word list (around `$$…$$`).
const MD_BREAK: &str = "\u{E003}";

/// Markdown as plain text lines wrapped to a `w`-wide box (13 px text):
/// emphasis / code / heading marks dropped, `[text](url)` → text, paragraphs
/// separated by an empty line. A formula is rendered to plain text by
/// [`crate::math::tex_to_text`] (#351: this has no KaTeX) — a `$$…$$` one
/// gets lines of its own.
fn markdown_lines(md: &str, w: f64) -> Vec<String> {
    let max = (((w - 28.0) / 6.6).floor() as usize).max(10);
    let mut out: Vec<String> = Vec::new();
    for para in md.split("\n\n") {
        let mut words: Vec<String> = Vec::new();
        for seg in crate::math::spans(para.trim()) {
            match seg {
                crate::math::Seg::Text(t) => {
                    let mut plain = String::new();
                    let mut rest = t.as_str();
                    // [text](url) → text; ![alt](src) → [alt] (#514: a
                    // markdown image has no pixels here — PR A draws its alt
                    // text in brackets so the reader knows a figure was
                    // there; embedding it is a documented follow-up).
                    while let Some(a) = rest.find('[') {
                        let Some(b) = rest[a..].find("](").map(|b| a + b) else {
                            break;
                        };
                        let Some(c) = rest[b..].find(')').map(|c| b + c) else {
                            break;
                        };
                        let is_image = a > 0 && rest.as_bytes()[a - 1] == b'!';
                        let pre_end = if is_image { a - 1 } else { a };
                        plain.push_str(&rest[..pre_end]);
                        if is_image {
                            plain.push('[');
                            plain.push_str(&rest[a + 1..b]);
                            plain.push(']');
                        } else {
                            plain.push_str(&rest[a + 1..b]);
                        }
                        rest = &rest[c + 1..];
                    }
                    plain.push_str(rest);
                    let plain = plain.replace(['*', '`'], "").replace("__", "");
                    words.extend(
                        plain
                            .split_whitespace()
                            .filter(|w| !w.chars().all(|c| c == '#'))
                            .map(str::to_string),
                    );
                }
                crate::math::Seg::Math { tex, display } => {
                    if display {
                        words.push(MD_BREAK.into());
                    }
                    words.extend(
                        crate::math::tex_to_text(&tex)
                            .split_whitespace()
                            .map(str::to_string),
                    );
                    if display {
                        words.push(MD_BREAK.into());
                    }
                }
            }
        }
        if words.iter().all(|w| w == MD_BREAK) {
            continue;
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        let mut line = String::new();
        let mut len = 0;
        for wd in words {
            let n = wd.chars().count();
            if wd == MD_BREAK || (!line.is_empty() && len + 1 + n > max) {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                    len = 0;
                }
                if wd == MD_BREAK {
                    continue;
                }
            }
            if !line.is_empty() {
                line.push(' ');
                len += 1;
            }
            line.push_str(&wd);
            len += n;
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

/// One [`markdown_lines`] line as SVG text content (escaped). Formula words
/// are now already plain text ([`crate::math::tex_to_text`], #351), so this
/// is no longer anything more than that.
fn markdown_line_svg(line: &str) -> String {
    esc(line)
}

#[cfg(test)]
mod md_tests {
    #[test]
    fn markdown_is_plain_and_wrapped() {
        let l = super::markdown_lines(
            "## Hi\n\nOne **bold** and `code` with a [link](https://x.y) — and a longer tail.",
            200.0,
        );
        assert_eq!(l[0], "Hi");
        assert_eq!(l[1], "");
        assert!(l[2].starts_with("One bold and code with a"), "{l:?}");
        assert!(l.iter().all(|x| x.chars().count() <= 26), "{l:?}");
        assert!(l.join(" ").contains("link —"), "{l:?}");
    }

    #[test]
    fn formulas_render_to_plain_text() {
        // #351: headless output has no KaTeX, so a formula used to show its
        // raw TeX source (`\hat{y}_t = \alpha y_t`) verbatim. The simple
        // parts (Greek letters, subscripts) now read as text; `\hat{…}` (an
        // accent) is outside this small converter's scope and is left as
        // TeX source — safe, not wrong, just not spelled out.
        let l = super::markdown_lines(
            "Smoothing $\\hat{y}_t = \\alpha y_t$ for $5 and $10:\n$$ \\sum_i x_i $$\nafter",
            400.0,
        );
        assert_eq!(l.len(), 3, "{l:?}");
        assert!(l[0].contains("\\hat{y}ₜ = α yₜ"), "{l:?}");
        assert!(l[0].contains("$5 and $10"), "{l:?}");
        assert!(l[1].contains("Σᵢ xᵢ"), "{l:?}");
        let svg = super::markdown_line_svg(&l[0]);
        assert!(!svg.contains("<tspan") && !svg.contains("md-tex"), "{svg}");
        assert!(svg.contains("Smoothing \\hat{y}ₜ = α yₜ for"), "{svg}");
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn value_is_the_rows_first_non_title_column() {
        let rows = vec![
            ("Supplier".to_string(), vec![Value::Str("Castings".into())]),
            (
                "Highlight".to_string(),
                vec![Value::Str("?v.supplier={value}".into())],
            ),
        ];
        let filled = fill_link_row("?v.supplier={value}", &rows, 0, Some(0));
        assert_eq!(filled, "?v.supplier=Castings");
    }

    #[test]
    fn named_column_placeholders_are_looked_up_by_exact_name() {
        let rows = vec![
            ("sku".to_string(), vec![Value::Str("SKU-1".into())]),
            ("city".to_string(), vec![Value::Str("Berlin, DE".into())]),
        ];
        // {value} still reads the first column; {city} the named one — and a
        // non-ASCII / reserved value gets URL-encoded, as the browser's
        // `fillLink` does.
        let filled = fill_link_row("#Detail?v.sku={value}&v.city={city}", &rows, 0, Some(0));
        assert_eq!(filled, "#Detail?v.sku=SKU-1&v.city=Berlin%2C%20DE");
    }

    #[test]
    fn an_unknown_placeholder_fills_empty_like_the_browser() {
        let rows = vec![("sku".to_string(), vec![Value::Str("SKU-1".into())])];
        assert_eq!(fill_link_row("?v.x={nope}", &rows, 0, Some(0)), "?v.x=");
    }

    #[test]
    fn headless_href_drops_a_tab_fragment_the_flattened_page_has_no_use_for() {
        // Every ::TAB is flattened onto one page headless (see `execute`) —
        // only the query string is a same-page reload that means anything.
        assert_eq!(
            headless_link_href("#Stock & pipeline?v.sku=SKU-1"),
            Some("?v.sku=SKU-1".to_string())
        );
        // A bare tab jump with nothing to set links nowhere useful headless.
        assert_eq!(headless_link_href("#Stock & pipeline"), None);
        assert_eq!(
            headless_link_href("?v.supplier=Castings"),
            Some("?v.supplier=Castings".to_string())
        );
        assert_eq!(
            headless_link_href("/d/other?v.x=1"),
            Some("/d/other?v.x=1".to_string())
        );
        assert_eq!(
            headless_link_href("https://en.wikipedia.org/wiki/Castings"),
            Some("https://en.wikipedia.org/wiki/Castings".to_string())
        );
        assert_eq!(headless_link_href("javascript:alert(1)"), None);
        assert_eq!(headless_link_href("//evil.example"), None);
        // A backslash lets a browser read `/\evil.example` as the
        // protocol-relative (off-site) `//evil.example` — refuse it even
        // though it starts with a single `/`.
        assert_eq!(headless_link_href("/\\evil.example"), None);
        assert_eq!(headless_link_href("?v.x=1\\2"), None);
        // Control characters and non-space whitespace are refused.
        assert_eq!(headless_link_href("?v.x=1\u{0}2"), None);
        assert_eq!(headless_link_href("?v.x=1\n2"), None);
        // A literal space in an author's template isn't a bypass (a URL
        // parser percent-encodes it in place, it never gets stripped or
        // reinterpreted the way backslash/tab/LF/CR can), so it's encoded
        // rather than refusing the whole link (#340 C) — the browser app
        // would still have followed it.
        assert_eq!(
            headless_link_href("?v.x=1 2"),
            Some("?v.x=1%202".to_string())
        );
        assert_eq!(
            headless_link_href("/d/other name?v.x=1"),
            Some("/d/other%20name?v.x=1".to_string())
        );
        assert_eq!(
            headless_link_href("#Stock & pipeline?v.sku=SKU 1"),
            Some("?v.sku=SKU%201".to_string())
        );
    }

    #[test]
    fn label_matches_the_browsers_linkcell_convention() {
        assert_eq!(link_label("?v.supplier=Castings"), "Open \u{2192}");
        assert_eq!(
            link_label("#Stock & pipeline?v.sku=1"),
            "Stock & pipeline \u{2192}"
        );
        assert_eq!(
            link_label("https://en.wikipedia.org/wiki/Castings"),
            "Open \u{2197}"
        );
    }

    #[test]
    fn markdown_fill_substitutes_known_columns_and_leaves_unknown_ones_as_written() {
        let rows = vec![
            ("c0".to_string(), vec![Value::Str("x".into())]),
            ("md:z".to_string(), vec![Value::Float(1.64)]),
            ("md:lvl".to_string(), vec![Value::Str("95 %".into())]),
        ];
        let l = crate::format::Locale::EN_US;
        let out = markdown_fill("z = {{z}} at {{lvl}}; {{nope}} stays", &rows, &l);
        assert_eq!(out, "z = 1.64 at 95 %; {{nope}} stays");
    }

    #[test]
    fn markdown_fill_honours_a_format_spec() {
        let rows = vec![("md:extra".to_string(), vec![Value::Float(18.2)])];
        let l = crate::format::Locale::EN_US;
        assert_eq!(
            markdown_fill("{{extra:eur}}", &rows, &l),
            crate::format::format_number_l(18.2, crate::MetricFmt::Money, &l)
        );
    }
}

#[cfg(test)]
mod doc_table_tests {
    use super::*;

    // #427: the plain static-HTML CLI (`dashboard file.sql out.html`) built
    // a `::TABLE`/`::PAGED` panel's columns the same way a chart/KPI's does
    // (role-filtered, looked up by `cN`) — but that statement's rewrite
    // keeps every column's own name instead of aliasing it to `cN`
    // (`sql::rewrite_list`'s `keep_intact`), so the lookup found nothing and
    // the whole table (not just a `::SPARKLINE` cell) rendered blank.
    // `render_panel_doc_svg` takes the same raw, name-keyed rows `--svg` and
    // `serve` already use.
    #[test]
    fn a_table_with_no_role_cast_columns_still_renders() {
        let rows = vec![
            ("feature".to_string(), vec![Value::Str("a".into())]),
            ("mx".to_string(), vec![Value::Float(4.0)]),
        ];
        let roles = vec![(0usize, Role::Table)];
        let ropts = RenderOptions::default();
        let svg = render_panel_doc_svg(
            &rows,
            &roles,
            &[],
            460.0,
            300.0,
            &ropts,
            false,
            &[],
            None,
            false,
        );
        assert!(
            svg.contains(">Feature<") && svg.contains(">Mx<") && svg.contains(">4<"),
            "every column must render, not just the ::TABLE's own: {svg}"
        );
    }

    #[test]
    fn a_table_sparkline_cell_renders_in_the_static_html_doc_path() {
        let rows = vec![
            ("feature".to_string(), vec![Value::Str("a".into())]),
            ("trend".to_string(), vec![Value::Str("[1,2,3]".into())]),
        ];
        let roles = vec![
            (0usize, Role::Table),
            (1usize, Role::Value(Kind::Sparkline)),
        ];
        let ropts = RenderOptions::default();
        let svg = render_panel_doc_svg(
            &rows,
            &roles,
            &[],
            460.0,
            300.0,
            &ropts,
            false,
            &[],
            None,
            false,
        );
        assert!(
            !svg.contains("[1,2,3]"),
            "a cell's raw JSON list must never print literally: {svg}"
        );
        assert!(
            svg.contains("data-geom=\"line\""),
            "the sparkline cell must draw through ggplot-rs, like --svg/serve: {svg}"
        );
    }

    // #457: a table's ::SPARKLINE cell goes through `render_sparkline` the
    // same way a KPI tile's does (see `kpi::tests::
    // kpi_sparkline_minima_stay_inside_the_strip`) — a zigzag with real
    // dips must stay fully inside its own cell's viewBox, not clipped at
    // the bottom by a request shorter than ggplot-rs's `SPARKLINE_MIN_H`
    // layout floor.
    #[test]
    fn a_table_sparkline_cells_minima_stay_inside_the_cell_box() {
        let rows = vec![
            ("feature".to_string(), vec![Value::Str("a".into())]),
            (
                "trend".to_string(),
                vec![Value::Str("[1005,1010,1000,1012,1001,1013,1000]".into())],
            ),
        ];
        let roles = vec![
            (0usize, Role::Table),
            (1usize, Role::Value(Kind::Sparkline)),
        ];
        let ropts = RenderOptions::default();
        let svg = render_panel_doc_svg(
            &rows,
            &roles,
            &[],
            460.0,
            300.0,
            &ropts,
            false,
            &[],
            None,
            false,
        );
        let line_at = svg
            .find("data-geom=\"line\"")
            .expect("a drawn sparkline line");
        let svg_open = svg[..line_at].rfind("<svg ").expect("its own nested <svg>");
        let svg_close =
            line_at + svg[line_at..].find("</svg>").expect("a closing </svg>") + "</svg>".len();
        let frag = &svg[svg_open..svg_close];
        let viewbox = frag
            .split("viewBox=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .expect("a viewBox");
        let h: f64 = viewbox
            .split_whitespace()
            .nth(3)
            .and_then(|n| n.parse().ok())
            .expect("a viewBox height");
        let mut ys: Vec<f64> = Vec::new();
        for pts in frag.split("points=\"").skip(1) {
            let pts = pts.split('"').next().unwrap_or("");
            ys.extend(
                pts.split(' ')
                    .filter_map(|p| p.split(',').nth(1))
                    .filter_map(|y| y.parse::<f64>().ok()),
            );
        }
        assert!(!ys.is_empty(), "expected drawn points: {frag}");
        let (mn, mx) = ys
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                (a.min(v), b.max(v))
            });
        assert!(
            mn > -0.5 && mx < h + 0.5,
            "sparkline cell content (y {mn}..{mx}) must stay inside its own \
             {h}px-tall viewBox, not clipped: {frag}"
        );
    }

    // #425/#457: a table's ::SPARKBAR cell goes through `render_sparkbar`
    // the same way a ::SPARKLINE cell goes through `render_sparkline` — it
    // must clamp to ggplot-rs's own 50px panel floor too, or a bar's own
    // baseline (sized against that floor) falls outside a shorter requested
    // viewBox and every bar's bottom gets clipped off instead of sitting on
    // the baseline.
    #[test]
    fn a_table_sparkbar_cells_stay_inside_the_cell_box() {
        let rows = vec![
            ("feature".to_string(), vec![Value::Str("a".into())]),
            ("dist".to_string(), vec![Value::Str("[1,9,2,8,3,7]".into())]),
        ];
        let roles = vec![(0usize, Role::Table), (1usize, Role::Value(Kind::Sparkbar))];
        let ropts = RenderOptions::default();
        let svg = render_panel_doc_svg(
            &rows,
            &roles,
            &[],
            460.0,
            300.0,
            &ropts,
            false,
            &[],
            None,
            false,
        );
        let geom_at = svg.find("data-geom=\"col\"").expect("a drawn sparkbar bar");
        let svg_open = svg[..geom_at].rfind("<svg ").expect("its own nested <svg>");
        let svg_close =
            geom_at + svg[geom_at..].find("</svg>").expect("a closing </svg>") + "</svg>".len();
        let frag = &svg[svg_open..svg_close];
        let viewbox = frag
            .split("viewBox=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .expect("a viewBox");
        let h: f64 = viewbox
            .split_whitespace()
            .nth(3)
            .and_then(|n| n.parse().ok())
            .expect("a viewBox height");
        let get_attr = |attrs: &str, name: &str| -> Option<f64> {
            attrs
                .split(&format!("{name}=\""))
                .nth(1)?
                .split('"')
                .next()?
                .parse()
                .ok()
        };
        let mut bars: Vec<(f64, f64)> = Vec::new();
        for rect in frag.split("<rect ").skip(1) {
            let attrs = rect.split('>').next().unwrap_or("");
            if let (Some(y), Some(ht)) = (get_attr(attrs, "y"), get_attr(attrs, "height")) {
                bars.push((y, y + ht));
            }
        }
        assert!(!bars.is_empty(), "expected drawn bars: {frag}");
        for (y, bottom) in &bars {
            assert!(
                *y > -0.5 && *bottom < h + 0.5,
                "sparkbar cell content (bar y {y}..{bottom}) must stay inside \
                 its own {h}px-tall viewBox, not clipped: {frag}"
            );
        }
    }
}

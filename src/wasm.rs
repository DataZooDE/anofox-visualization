//! wasm-bindgen surface for the browser dashboard builder. DuckDB-Wasm runs the
//! SQL in-page; these functions plan the annotated script and render each panel
//! to SVG — so the whole pipeline is client-side, no server, no DuckDB
//! extension. Build with: `wasm-pack build --features wasm`.
//!
//! Every export is wrapped so a failure returns an error value (an error SVG
//! for renders, `[]` for JSON) rather than throwing. Note that on
//! `wasm32-unknown-unknown` a Rust panic aborts (traps) instead of unwinding,
//! so `catch_unwind` here is a second line of defence for native test builds —
//! the first is that the core never panics on user input (fuzz-tested).

use crate::{
    columns_from_entries, error_svg, guard, parse_role_entries, render_with, roles, sql,
    RenderError, RenderOptions, Role,
};
use wasm_bindgen::prelude::*;

/// Plan a dashboard script into JSON:
/// `[{ "setup": bool, "sql": string, "roles": [[colIdx, "ROLE", name], …] }]`.
/// The caller runs each `sql` through DuckDB-Wasm (setup statements for effect,
/// panels with `-json`) and passes the rows back to [`render_panel`].
#[wasm_bindgen]
pub fn plan(script: &str) -> String {
    guard(|| Ok(plan_json(script))).unwrap_or_else(|_| "[]".into())
}

fn plan_json(script: &str) -> String {
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
                    serde_json::json!([i, r.token(), name])
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
}

/// Render one panel to SVG. `rows_json` = `[{ "c0": …, "c1": … }, …]` (a panel
/// query's `-json` result; bare `NaN`/`Infinity` tolerated); `roles_json` = the
/// panel's `roles` from [`plan`]; `primary` = brand colour `rrggbb` (or "");
/// `zoom_json` = `[x0, x1, y0, y1]` (or "" for auto-fit). Errors come back as a
/// small error SVG.
#[wasm_bindgen]
pub fn render_panel(
    rows_json: &str,
    roles_json: &str,
    width: u32,
    height: u32,
    primary: &str,
    zoom_json: &str,
) -> String {
    render_panel_checked(rows_json, roles_json, width, height, primary, zoom_json)
        .unwrap_or_else(|e| error_svg(&e.to_string(), width))
}

/// The `Result` form of [`render_panel`] (native callers / tests).
pub fn render_panel_checked(
    rows_json: &str,
    roles_json: &str,
    width: u32,
    height: u32,
    primary: &str,
    zoom_json: &str,
) -> Result<String, RenderError> {
    guard(|| {
        let rows = sql::parse_rows_json(rows_json).map_err(RenderError::BadSpec)?;
        let roles_v: serde_json::Value = serde_json::from_str(roles_json)
            .map_err(|e| RenderError::BadSpec(format!("roles JSON: {e}")))?;
        let entries = parse_role_entries(&roles_v).map_err(RenderError::BadSpec)?;
        let cols = columns_from_entries(&rows, &entries);
        let opts = RenderOptions {
            brand: crate::parse_rgb(primary),
            zoom: parse_zoom(zoom_json),
            ..RenderOptions::default()
        };
        render_with(&cols, width, height, &opts)
    })
}

/// Bounds `[x0, x1, y0, y1]` of a map panel's geometry (the `MAP` + `BASEMAP`
/// columns), in lon/lat — the UI uses these to seed an aspect-correct zoom view.
#[wasm_bindgen]
pub fn map_bounds(rows_json: &str, roles_json: &str) -> String {
    guard(|| Ok(map_bounds_inner(rows_json, roles_json))).unwrap_or_else(|_| "[]".into())
}

fn entries_of(roles_json: &str) -> Vec<(usize, Role, String)> {
    serde_json::from_str::<serde_json::Value>(roles_json)
        .ok()
        .and_then(|v| parse_role_entries(&v).ok())
        .unwrap_or_default()
}

fn map_bounds_inner(rows_json: &str, roles_json: &str) -> String {
    let rows = sql::parse_rows_json(rows_json).unwrap_or_default();
    let geo_cols: Vec<String> = entries_of(roles_json)
        .iter()
        .filter(|(_, r, _)| matches!(r, Role::Geometry | Role::Basemap))
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
                if [b.0, b.1, b.2, b.3].iter().all(|f| f.is_finite()) {
                    x0 = x0.min(b.0);
                    y0 = y0.min(b.1);
                    x1 = x1.max(b.2);
                    y1 = y1.max(b.3);
                }
            }
        }
    }
    if !x0.is_finite() {
        return "[]".into();
    }
    format!("[{x0},{x1},{y0},{y1}]")
}

/// Data extent `[x0, x1, y0, y1]` of a cartesian panel. Returns `[]` unless the
/// x axis is continuous/datetime.
///
/// **Deprecated** — kept only for external callers of older bundles. The
/// browser UI now reads the rendered SVG root's `data-domain` (the trained,
/// expanded `x0 x1 y0 y1` domain written by ggplot-rs ≥ 0.16, present only
/// when both axes are continuous) and `data-flip`, plus the `zoomable` role
/// set from [`roles_json`]; those match what is actually drawn, which this
/// raw data extent does not.
#[wasm_bindgen]
pub fn panel_bounds(rows_json: &str, roles_json: &str) -> String {
    guard(|| Ok(panel_bounds_inner(rows_json, roles_json))).unwrap_or_else(|_| "[]".into())
}

fn panel_bounds_inner(rows_json: &str, roles_json: &str) -> String {
    use ggplot_rs::prelude::Value;
    let rows = sql::parse_rows_json(rows_json).unwrap_or_default();
    let cols = columns_from_entries(&rows, &entries_of(roles_json));
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

/// The role registry as JSON (see [`crate::roles::roles_json`]) — the browser
/// derives its role sets (inputs, metrics, layout directives, table formats)
/// from this instead of hard-coding them.
#[wasm_bindgen]
pub fn roles_json() -> String {
    roles::roles_json()
}

/// Format a KPI / table number exactly like the headless renderer
/// (see [`crate::format::format_number`]). `fmt` is a role token
/// (`METRIC`/`MONEY`/`PERCENT`/`COMPACT` or an alias).
#[wasm_bindgen]
pub fn format_number(value: f64, fmt: &str) -> String {
    crate::format::format_number(value, crate::format::metric_fmt_of(fmt))
}

/// Parse a `[x0, x1, y0, y1]` zoom window (empty / invalid → `None`).
fn parse_zoom(s: &str) -> Option<crate::ZoomWindow> {
    let v: Vec<f64> = serde_json::from_str(s).ok()?;
    match v.as_slice() {
        [x0, x1, y0, y1] if [x0, x1, y0, y1].iter().all(|f| f.is_finite()) => {
            Some(((*x0, *x1), (*y0, *y1)))
        }
        _ => None,
    }
}

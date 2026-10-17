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
/// `[{ "setup": bool, "sql": string, "roles": [[colIdx, "ROLE", name], …],
///    "contract": null | {"macro": name, "rows": k, "args": k+1,
///                        "groups": [[colIdx, name], …]} }]`.
/// The caller runs each `sql` through DuckDB-Wasm (setup statements for effect,
/// panels with `-json`) and passes the rows back to [`render_panel`] — or, for
/// a contract plot call (`SELECT anofox_plot_terms(t) FROM … t`, see
/// [`sql::contract_call`]), each result row's `c{rows}` / `c{args}` cells to
/// [`render_contract`].
/// The release version (CalVer, from Cargo.toml) for the sidebar footer.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

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
                "contract": p.contract.as_ref().map(|c| serde_json::json!({
                    "macro": c.macro_name,
                    "rows": c.rows,
                    "args": c.args,
                    "groups": c.groups.iter().map(|(i, n)| serde_json::json!([i, n])).collect::<Vec<_>>(),
                })),
                "requires": p.requires,
                "tab_requires": p.tab_requires,
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
}

/// Render one panel to SVG. `rows_json` = `[{ "c0": …, "c1": … }, …]` (a panel
/// query's `-json` result; bare `NaN`/`Infinity` tolerated); `roles_json` = the
/// panel's `roles` from [`plan`]; `primary` = brand colour `rrggbb` (or ""),
/// or a JSON render context `{"primary","theme","variant","locale"}` (see
/// [`RenderOptions::from_primary_arg`]);
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
        let mut opts = RenderOptions::from_primary_arg(primary).map_err(RenderError::BadSpec)?;
        opts.zoom = parse_zoom(zoom_json);
        render_with(&cols, width, height, &opts)
    })
}

/// Render a chart and the `::TRACK`s under it, lined up on one x axis
/// ([`crate::track::render_stack`]). `stack_json` =
/// `{"chart": {"rows", "roles"}, "tracks": [{"rows", "roles", "height"}, …]}`
/// — `rows` a panel query's `-json` text and `roles` its planned roles, as for
/// [`render_panel`]; a track `height` of 0 is its natural height. Returns a
/// JSON array of SVGs `[chart, track 1, …]`; on an error, the error SVG for
/// the chart and nothing for the tracks.
#[wasm_bindgen]
pub fn render_stack(
    stack_json: &str,
    width: u32,
    height: u32,
    primary: &str,
    zoom_json: &str,
) -> String {
    let svgs = render_stack_checked(stack_json, width, height, primary, zoom_json)
        .unwrap_or_else(|e| vec![error_svg(&e.to_string(), width)]);
    serde_json::to_string(&svgs).unwrap_or_else(|_| "[]".into())
}

/// The `Result` form of [`render_stack`] (native callers / tests).
pub fn render_stack_checked(
    stack_json: &str,
    width: u32,
    height: u32,
    primary: &str,
    zoom_json: &str,
) -> Result<Vec<String>, RenderError> {
    guard(|| {
        let bad = |e: String| RenderError::BadSpec(e);
        let v: serde_json::Value =
            serde_json::from_str(stack_json).map_err(|e| bad(format!("stack JSON: {e}")))?;
        let part = |p: &serde_json::Value| -> Result<Vec<crate::Column>, RenderError> {
            let rows = sql::parse_rows_json(p["rows"].as_str().unwrap_or("[]")).map_err(bad)?;
            let entries = parse_role_entries(&p["roles"]).map_err(bad)?;
            Ok(columns_from_entries(&rows, &entries))
        };
        let chart = part(&v["chart"])?;
        let tracks = v["tracks"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|t| {
                let h = t["height"].as_u64().unwrap_or(0).min(2000) as u32;
                Ok((part(t)?, h))
            })
            .collect::<Result<Vec<_>, RenderError>>()?;
        let mut opts = RenderOptions::from_primary_arg(primary).map_err(bad)?;
        opts.zoom = parse_zoom(zoom_json);
        crate::track::render_stack(&chart, &tracks, width, height, &opts)
    })
}

/// Render a contract plot call to SVG: `macro_name` = the planned
/// `contract.macro` (`anofox_plot_terms`, …), `rows_json` = the rows as a JSON
/// array (or JSON text of one — the `to_json(list(alias))` cell as DuckDB-Wasm
/// returns it), `args_json` = the named arguments as a JSON object keyed by
/// parameter name (the `json_object(…)` cell). Errors come back as a small
/// error SVG; a macro this build does not know yet renders as a note.
#[wasm_bindgen]
pub fn render_contract(
    macro_name: &str,
    rows_json: &str,
    args_json: &str,
    width: u32,
    height: u32,
    primary: &str,
) -> String {
    guard(|| {
        sql::contract_call::render_call(macro_name, rows_json, args_json, width, height, primary)
    })
    .unwrap_or_else(|e| error_svg(&e.to_string(), width))
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
/// The SQL of a `::PAGED` table request (`page_json` = the same `page` object a
/// locked server receives) over the panel query `base`, with its values inlined
/// as escaped literals — so the browser's DuckDB runs exactly the query a
/// locked server would. Errors come back as `{"error": "…"}`.
#[wasm_bindgen]
pub fn page_sql(base: &str, page_json: &str) -> String {
    let built = serde_json::from_str(page_json)
        .map_err(|e| format!("bad page JSON: {e}"))
        .and_then(|v| crate::host::serving::parse_page(&v))
        .map(|p| crate::host::serving::page_sql_inline(base, &p));
    match built {
        Ok(sql) => sql,
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    }
}

/// The column profiles of a table (see [`crate::table::profile_column`]):
/// `spec_json` = `{"locale": "de-DE", "many": true, "cols": [{"name", "fmt",
/// "first", "lo", "hi", "values": [...]}]}` → a JSON array of profiles
/// (`head`, `numeric`, `label`, `dec`, `unit`, …) — the browser table and the
/// headless table read a column the same way. Errors: `{"error": "…"}`.
#[wasm_bindgen]
pub fn table_profile(spec_json: &str) -> String {
    let v: serde_json::Value = match serde_json::from_str(spec_json) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::json!({ "error": format!("bad table spec: {e}") }).to_string()
        }
    };
    let locale = v
        .get("locale")
        .and_then(|l| l.as_str())
        .and_then(crate::format::Locale::from_tag)
        .unwrap_or_default();
    let many = v.get("many").and_then(|m| m.as_bool()).unwrap_or(true);
    let empty = Vec::new();
    let cols = v.get("cols").and_then(|c| c.as_array()).unwrap_or(&empty);
    let out: Vec<serde_json::Value> = cols
        .iter()
        .map(|c| {
            let num = |k: &str| c.get(k).and_then(|x| x.as_f64());
            let spec = crate::table::ColumnSpec {
                name: c.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                fmt: c
                    .get("fmt")
                    .and_then(|f| f.as_str())
                    .filter(|f| !f.is_empty()),
                first: c.get("first").and_then(|f| f.as_bool()).unwrap_or(false),
                range: num("lo").zip(num("hi")),
                many,
            };
            let values: Vec<crate::table::Cell> = c
                .get("values")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(crate::table::Cell::from_json).collect())
                .unwrap_or_default();
            crate::table::profile_column(&spec, &values, &locale).to_json()
        })
        .collect();
    serde_json::Value::Array(out).to_string()
}

/// A readable header for a raw column name ([`crate::table::human_header`]).
#[wasm_bindgen]
pub fn human_header(raw: &str) -> String {
    crate::table::human_header(raw)
}

/// Resolve a `::PERIOD` spec + the viewer's choice to the control's model and
/// the variables to bind ([`crate::period::resolve_json`]): `state_json` =
/// `{preset?, from?, to?, grain?, compare?, today?, column?}` (today = the
/// viewer's local `YYYY-MM-DD`, column = the ::PERIOD column's name). Returns
/// `{ok, errors, prefix, presets:[{id,label}], grains,
/// compares:[{id,label}], preset, grain, grain_auto, compare, from, to, today,
/// vars:{from,to,prev_from,prev_to,grain,compare,label,prev_label}}`.
#[wasm_bindgen]
pub fn period_resolve(spec: &str, state_json: &str) -> String {
    guard(|| Ok(crate::period::resolve_json(spec, state_json).to_string()))
        .unwrap_or_else(|e| serde_json::json!({"ok": false, "errors": [e.to_string()]}).to_string())
}

/// A KPI tile's change and goal texts — the same as the headless tile
/// ([`crate::kpi`]). `req_json`: `{value, prev?, goal?, fmt, delta_format?,
/// lower_good?, locale?}` (numbers or numeric strings; `fmt` a metric token).
/// Returns `{delta: {up, good, arrow, pct, abs, text} | null, goal: {frac,
/// reached, text} | null, delta_format_ok}`.
#[wasm_bindgen]
pub fn kpi_parts(req_json: &str) -> String {
    use crate::kpi::{delta, goal, DeltaFormat};
    let r: serde_json::Value = serde_json::from_str(req_json).unwrap_or_default();
    let n = |k: &str| {
        match r.get(k) {
            Some(serde_json::Value::Number(x)) => x.as_f64(),
            Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
        .filter(|f| f.is_finite())
    };
    let s = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let l = crate::format::Locale::from_tag(&s("locale")).unwrap_or_default();
    let fmt = crate::format::metric_fmt_of(&s("fmt"));
    let df = DeltaFormat::parse(&s("delta_format"));
    let lower = r
        .get("lower_good")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let cur = n("value");
    let d = cur
        .zip(n("prev"))
        .and_then(|(c, p)| delta(c, p, fmt, df.unwrap_or_default(), lower, &l));
    let g = cur.zip(n("goal")).and_then(|(c, g)| goal(c, g, fmt, &l));
    serde_json::json!({
        "delta": d.map(|d| serde_json::json!({
            "up": d.up, "good": d.good, "arrow": d.arrow, "pct": d.pct, "abs": d.abs, "text": d.text,
        })),
        "goal": g.map(|g| serde_json::json!({"frac": g.frac, "reached": g.reached, "text": g.text})),
        "delta_format_ok": df.is_some(),
    })
    .to_string()
}

/// Parse `::THRESHOLDS` rules (a panel's, or a table directive's `column:
/// rules | …`) for the browser — see [`crate::thresholds::table_json`].
#[wasm_bindgen]
pub fn threshold_rules(spec: &str) -> String {
    crate::thresholds::table_json(spec)
}

/// A dashboard's header comments as `{"title","group","tags","refresh",
/// "description","owner","certified"}`,
/// read by the parser the servers use
/// ([`crate::host::serving::parse_dashboard_meta`]), so the builder's sidebar
/// and a served folder place a dashboard alike. `title` is "" without
/// `-- @title`, `group` is null without `-- @group`.
#[wasm_bindgen]
pub fn dashboard_meta(script: &str) -> String {
    guard(|| {
        let m = crate::host::serving::parse_dashboard_meta("", script);
        Ok(serde_json::json!({
            "title": m.title, "group": m.group, "tags": m.tags, "refresh": m.refresh,
            "description": m.description, "owner": m.owner, "certified": m.certified,
        })
        .to_string())
    })
    .unwrap_or_else(|_| {
        r#"{"title":"","group":null,"tags":[],"refresh":0,"description":null,"owner":null,"certified":null}"#.into()
    })
}

/// One step of the lint `dashboard --check` runs, for the builder's *Check*
/// ([`crate::lint::step_json`]). The lint runs queries synchronously and
/// DuckDB-Wasm answers asynchronously, so it is resumable: call with the
/// answers so far (`known_json`, `{}` at first); while the result has
/// `done: false`, run each query in `need` in a fresh session, add its rows
/// or error to `known_json` and call again. `opts_json` = `{"design",
/// "strict"}`. Returns `{done, need, diagnostics, ok, summary}`, or
/// `{"error"}`.
#[wasm_bindgen]
pub fn lint_step(script: &str, known_json: &str, opts_json: &str) -> String {
    guard(|| Ok(crate::lint::step_json(script, known_json, opts_json)))
        .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }).to_string())
}

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

/// [`format_number`] in a dashboard locale (`de-DE`, …; "" = en-US).
#[wasm_bindgen]
pub fn format_number_locale(value: f64, fmt: &str, locale: &str) -> String {
    let l = crate::format::Locale::from_tag(locale).unwrap_or_default();
    crate::format::format_number_l(value, crate::format::metric_fmt_of(fmt), &l)
}

/// Format an ISO date (`2026-10-15[ 12:00:00]`) in a dashboard locale;
/// other text is returned unchanged.
#[wasm_bindgen]
pub fn format_date_locale(iso: &str, locale: &str) -> String {
    crate::format::Locale::from_tag(locale)
        .unwrap_or_default()
        .format_date(iso)
}

/// Resolve a theme spec (a name, `a+b`, or JSON) to
/// `{"ok":true,"name","mode","light":{css,card,bg,brand,locale},"dark":{…}}`,
/// or `{"ok":false,"error":"…"}`.
#[wasm_bindgen]
pub fn resolve_theme(spec: &str) -> String {
    theme_json(spec, "")
}

/// [`resolve_theme`] plus the dashboard's chrome directives as a JSON object
/// `{"title","brand","logo","favicon","locale"}` merged over it.
#[wasm_bindgen]
pub fn resolve_dashboard_theme(spec: &str, chrome_json: &str) -> String {
    theme_json(spec, chrome_json)
}

fn theme_json(spec: &str, chrome_json: &str) -> String {
    let out = guard(|| {
        let c: serde_json::Value = serde_json::from_str(chrome_json).unwrap_or_default();
        let s = |k: &str| c.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let chrome = crate::theme::DashboardChrome {
            theme: Some(spec.to_string()).filter(|t| !t.trim().is_empty()),
            locale: s("locale"),
            title: s("title"),
            brand: s("brand"),
            logo: s("logo").filter(|u| crate::theme::safe_url(u)),
            favicon: s("favicon").filter(|u| crate::theme::safe_url(u)),
            ..Default::default()
        };
        Ok(chrome.resolve(None))
    });
    match out {
        Ok(Ok(set)) => {
            let mut v = set.to_json();
            v["ok"] = serde_json::Value::Bool(true);
            v.to_string()
        }
        Ok(Err(e)) => serde_json::json!({"ok": false, "error": e}).to_string(),
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}).to_string(),
    }
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

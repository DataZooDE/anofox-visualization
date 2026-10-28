//! `decomposition` (**provisional**, see `docs/plans/integration-contract.md`):
//! a time-series decomposition — observed, level, trend, one or more
//! seasonal components, a remainder — as stacked panels sharing one x axis,
//! each with its own y scale. Reads the long `ds | period, component, value`
//! table, wide columns (`observed`, `trend`, `seasonal`, `remainder`;
//! anofox-orchestration's `decomposition` output) and anofox-forecast's list
//! outputs as they come (`ts_mstl_decomposition_by`, `ts_forecast_explain_by`).

use super::look::{self, Labels};
use super::stats::{self, fin, s, text_of, Frame, Out};
use super::{opt_str, Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;
use serde_json::Value as J;

/// One drawn component: its canonical name (`trend`, `seasonal_7`, …), x / y
/// series and, when the table carries several models, which one.
#[derive(Clone, Debug)]
struct Comp {
    name: String,
    xs: Vec<Value>,
    ys: Vec<Value>,
    model: Option<String>,
}

/// `seasonal_7` → `seasonal (7)`; every other name unchanged.
fn display_name(name: &str) -> String {
    match name.strip_prefix("seasonal_") {
        Some(p) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            format!("seasonal ({p})")
        }
        _ => name.to_string(),
    }
}

/// Panel order: observed/y, level, trend, seasonal (bare, then `seasonal_<p>`
/// by numeric p), other components as first seen, remainder/residual last.
fn rank(name: &str) -> (u8, f64) {
    match name {
        "observed" | "y" => (0, 0.0),
        "level" => (1, 0.0),
        "trend" => (2, 0.0),
        "seasonal" => (3, 0.0),
        n if n.starts_with("seasonal_") => (
            3,
            n.trim_start_matches("seasonal_")
                .parse()
                .unwrap_or(f64::MAX),
        ),
        "remainder" | "residual" | "irregular" => (5, 0.0),
        _ => (4, 0.0),
    }
}

fn sort_by_rank(comps: &mut [Comp]) {
    comps.sort_by(|a, b| {
        let (pa, sa) = rank(&a.name);
        let (pb, sb) = rank(&b.name);
        pa.cmp(&pb).then(sa.total_cmp(&sb))
    });
}

/// Is this component drawn as points, last (`remainder` / `residual`)?
fn is_point(name: &str) -> bool {
    matches!(name, "remainder" | "residual")
}

/// The first of `model_id`, `model_name`, `method` with more than one
/// distinct value: several decomposition runs stacked in one call.
fn model_key_of(t: &Table) -> Option<&'static str> {
    ["model_id", "model_name", "method"]
        .into_iter()
        .find(|k| t.has(k) && t.distinct(k) > 1)
}

/// A number cell (JSON number or numeric string; anything else, including
/// null, is `Na`).
fn json_num(v: &J) -> Value {
    match v {
        J::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map_or(Value::Na, Value::Float),
        J::String(s) => s
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map_or(Value::Na, Value::Float),
        _ => Value::Na,
    }
}

/// Does any wide-vocabulary column of `t` hold a JSON array: the MSTL /
/// explain list outputs, rather than anofox-orchestration's wide (scalar)
/// decomposition.
fn is_list_form(t: &Table) -> bool {
    const COLS: &[&str] = &[
        "observed",
        "y",
        "level",
        "trend",
        "seasonal",
        "remainder",
        "residual",
    ];
    t.rows
        .iter()
        .any(|r| COLS.iter().any(|c| matches!(r.get(*c), Some(J::Array(_)))))
}

/// `trend` + Σ seasonal* + remainder/residual, pointwise where every one of
/// them is finite — the `observed` series when a wide or list table doesn't
/// carry it directly (wide/list forms only; long-form tables are not
/// computed).
fn fill_observed(comps: &mut Vec<Comp>) {
    let mut models: Vec<Option<String>> = Vec::new();
    for c in comps.iter() {
        if !models.contains(&c.model) {
            models.push(c.model.clone());
        }
    }
    for mv in models {
        if comps.iter().any(|c| c.model == mv && c.name == "observed") {
            continue;
        }
        let Some(trend) = comps
            .iter()
            .find(|c| c.model == mv && c.name == "trend")
            .cloned()
        else {
            continue;
        };
        let Some(rem) = comps
            .iter()
            .find(|c| c.model == mv && (c.name == "remainder" || c.name == "residual"))
            .cloned()
        else {
            continue;
        };
        let seasonals: Vec<Comp> = comps
            .iter()
            .filter(|c| c.model == mv && (c.name == "seasonal" || c.name.starts_with("seasonal_")))
            .cloned()
            .collect();
        let n = trend.ys.len();
        if rem.ys.len() != n || seasonals.iter().any(|sc| sc.ys.len() != n) {
            continue;
        }
        let mut ys = Vec::with_capacity(n);
        for i in 0..n {
            let (tv, rv) = (fin(&trend.ys[i]), fin(&rem.ys[i]));
            let mut sum = 0.0;
            let mut ok = tv.is_some() && rv.is_some();
            for sc in &seasonals {
                match fin(&sc.ys[i]) {
                    Some(v) => sum += v,
                    None => ok = false,
                }
            }
            ys.push(if ok {
                Value::Float(tv.unwrap() + sum + rv.unwrap())
            } else {
                Value::Na
            });
        }
        comps.push(Comp {
            name: "observed".into(),
            xs: trend.xs.clone(),
            ys,
            model: mv,
        });
    }
}

/// The x key of a long/wide decomposition table: the `x :=` option, else
/// `ds`, `period`, `x`; orchestration's wide table renames `t` (a seasonal
/// period index per row) to `statistic`, so a column that arrived that way
/// stands in too, labelled `t` rather than `statistic`.
fn x_key(t: &Table, options: &Row) -> Option<(String, Option<&'static str>)> {
    if let Some(x) = opt_str(options, "x") {
        return Some((x, None));
    }
    for k in ["ds", "period", "x"] {
        if t.has(k) {
            return Some((k.to_string(), None));
        }
    }
    if t.renamed.iter().any(|(o, c)| o == "t" && c == "statistic") {
        return Some(("statistic".to_string(), Some("t")));
    }
    None
}

/// The long form: a `component` text column and a `value` column, with an x
/// key (`ds` / `period` / `x`, or `x :=`), optional `id` and `model_id` /
/// `method`.
fn long_form(t: &Table, options: &Row) -> Result<(Vec<Comp>, String), String> {
    t.need(&["component", "value"])?;
    let Some((xkey, special)) = x_key(t, options) else {
        return Err(format!(
            "needs an x position: a column `ds`, `period` or `x`, or x := '<column>' (got: {})",
            t.keys()
        ));
    };
    t.need(&[xkey.as_str()])?;
    let xs = t.xpos(&xkey);
    let is_date = xs.iter().any(|v| matches!(v, Value::DateTime(_)));
    let comp = t.text("component");
    let value = t.num("value")?;
    let model_key = model_key_of(t);
    let model: Vec<Option<String>> = match model_key {
        Some(k) => t.text(k).iter().map(text_of).collect(),
        None => vec![None; t.rows.len()],
    };
    let mut names: Vec<String> = Vec::new();
    for v in &comp {
        if let Some(n) = text_of(v) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    sort_by_rank_names(&mut names);
    let mut models: Vec<Option<String>> = Vec::new();
    for v in &model {
        if !models.contains(v) {
            models.push(v.clone());
        }
    }
    let mut comps = Vec::new();
    for name in &names {
        for mv in &models {
            let idx: Vec<usize> = (0..t.rows.len())
                .filter(|&i| text_of(&comp[i]).as_deref() == Some(name.as_str()))
                .filter(|&i| model[i] == *mv)
                .collect();
            if idx.is_empty() {
                continue;
            }
            comps.push(Comp {
                name: name.clone(),
                xs: idx.iter().map(|&i| xs[i].clone()).collect(),
                ys: idx.iter().map(|&i| value[i].clone()).collect(),
                model: mv.clone(),
            });
        }
    }
    let label = if is_date {
        "date".to_string()
    } else {
        special
            .map(str::to_string)
            .unwrap_or_else(|| look::humanize(&xkey))
    };
    Ok((comps, label))
}

fn sort_by_rank_names(names: &mut [String]) {
    names.sort_by(|a, b| {
        let (pa, sa) = rank(a);
        let (pb, sb) = rank(b);
        pa.cmp(&pb).then(sa.total_cmp(&sb))
    });
}

/// Candidate wide-vocabulary component columns present (or computable, for
/// `observed`): `observed`/`y`, `level`, `trend`, `seasonal_<digits>`,
/// `remainder`/`residual`, in table order (final order comes from [`rank`]).
fn wide_names(t: &Table) -> Vec<String> {
    let mut v = Vec::new();
    if t.has("observed") {
        v.push("observed".to_string());
    } else if t.has("y") {
        v.push("y".to_string());
    } else if t.has("trend") && (t.has("remainder") || t.has("residual")) {
        v.push("observed".to_string());
    }
    for base in ["level", "trend"] {
        if t.has(base) {
            v.push(base.to_string());
        }
    }
    if t.has("seasonal") {
        v.push("seasonal".to_string());
    }
    if let Some(r0) = t.rows.first() {
        for k in r0.keys() {
            if let Some(rest) = k.strip_prefix("seasonal_") {
                if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) && t.has(k) {
                    v.push(k.clone());
                }
            }
        }
    }
    for base in ["remainder", "residual"] {
        if t.has(base) {
            v.push(base.to_string());
        }
    }
    v
}

/// The wide form: an x key plus scalar columns from the vocabulary above —
/// anofox-orchestration's `decomposition` output.
fn wide_form(t: &Table, options: &Row) -> Result<(Vec<Comp>, String), String> {
    let names: Vec<String> = wide_names(t);
    if names.is_empty() {
        // No recognised component column: the richest error is the one
        // `need` would give for the schema's identifying column.
        t.need(&["trend"])?;
    }
    let Some((xkey, special)) = x_key(t, options) else {
        return Err(format!(
            "needs an x position: a column `ds`, `period` or `x`, or x := '<column>' (got: {})",
            t.keys()
        ));
    };
    let xs = t.xpos(&xkey);
    let is_date = xs.iter().any(|v| matches!(v, Value::DateTime(_)));
    let model_key = model_key_of(t);
    let model: Vec<Option<String>> = match model_key {
        Some(k) => t.text(k).iter().map(text_of).collect(),
        None => vec![None; t.rows.len()],
    };
    let mut models: Vec<Option<String>> = Vec::new();
    for v in &model {
        if !models.contains(v) {
            models.push(v.clone());
        }
    }
    let canon = |n: &str| {
        if n == "y" {
            "observed".to_string()
        } else {
            n.to_string()
        }
    };
    let mut comps = Vec::new();
    for raw in &names {
        if raw == "observed" && !t.has("observed") && !t.has("y") {
            continue; // computed below, once per model
        }
        let col = if raw == "observed" && t.has("y") {
            "y"
        } else {
            raw.as_str()
        };
        let values = t.num(col)?;
        for mv in &models {
            let idx: Vec<usize> = (0..t.rows.len()).filter(|&i| model[i] == *mv).collect();
            if idx.is_empty() {
                continue;
            }
            comps.push(Comp {
                name: canon(raw),
                xs: idx.iter().map(|&i| xs[i].clone()).collect(),
                ys: idx.iter().map(|&i| values[i].clone()).collect(),
                model: mv.clone(),
            });
        }
    }
    fill_observed(&mut comps);
    sort_by_rank(&mut comps);
    let label = if is_date {
        "date".to_string()
    } else {
        special
            .map(str::to_string)
            .unwrap_or_else(|| look::humanize(&xkey))
    };
    Ok((comps, label))
}

/// x positions for a list-form component: the row's `ds` LIST when its
/// length matches, else 1..n (the x label is `period`).
fn list_xs(n: usize, ds: Option<&Vec<J>>) -> (Vec<Value>, Option<&'static str>) {
    if let Some(ds) = ds {
        if ds.len() == n {
            let raw: Vec<Value> = ds
                .iter()
                .map(|v| match v {
                    J::Number(num) => num.as_f64().map_or(Value::Na, Value::Float),
                    J::String(s) => match s.trim().parse::<f64>() {
                        Ok(f) if f.is_finite() => Value::Float(f),
                        _ => Value::Str(s.clone()),
                    },
                    _ => Value::Na,
                })
                .collect();
            return (crate::sql::maybe_datetime(raw), None);
        }
    }
    (
        (1..=n).map(|i| Value::Float(i as f64)).collect(),
        Some("period"),
    )
}

/// The list form (anofox-forecast's `ts_mstl_decomposition_by`,
/// `ts_forecast_explain_by`): component columns hold LISTs, one row per
/// (id, model). `seasonal` as a LIST of LISTs becomes one `seasonal_<period>`
/// per inner list (from `periods`, else 1-based); a flat `seasonal` LIST
/// (explain) stays one `seasonal` component.
fn list_form(t: &Table, options: &Row) -> Result<(Vec<Comp>, String), String> {
    let _ = options; // no `x :=` override for the list form
    let model_key = model_key_of(t);
    let mut comps: Vec<Comp> = Vec::new();
    let mut label: Option<&'static str> = None;
    let mut saw_date = false;
    for r in t.rows {
        let mv = model_key.and_then(|k| r.get(k)).and_then(|v| match v {
            J::String(s) => Some(s.clone()),
            J::Null => None,
            v => Some(v.to_string()),
        });
        let ds = r.get("ds").and_then(J::as_array);
        let periods: Option<&Vec<J>> = r.get("periods").and_then(J::as_array);
        let n = ["observed", "y", "level", "trend", "remainder", "residual"]
            .iter()
            .filter_map(|c| r.get(*c))
            .find_map(J::as_array)
            .map(Vec::len)
            .or_else(|| {
                r.get("seasonal")
                    .and_then(J::as_array)
                    .map(|a| a.first().and_then(J::as_array).map_or(a.len(), Vec::len))
            });
        let Some(n) = n else { continue };
        let (xs, lbl) = list_xs(n, ds);
        if lbl.is_some() {
            label = lbl;
        }
        if xs.iter().any(|v| matches!(v, Value::DateTime(_))) {
            saw_date = true;
        }
        for base in ["observed", "y", "level", "trend", "remainder", "residual"] {
            if let Some(J::Array(vals)) = r.get(base) {
                if vals.len() == n {
                    comps.push(Comp {
                        name: if base == "y" {
                            "observed".into()
                        } else {
                            base.to_string()
                        },
                        xs: xs.clone(),
                        ys: vals.iter().map(json_num).collect(),
                        model: mv.clone(),
                    });
                }
            }
        }
        if let Some(J::Array(outer)) = r.get("seasonal") {
            match outer.first() {
                Some(J::Array(_)) => {
                    for (k, inner) in outer.iter().enumerate() {
                        let Some(inner) = inner.as_array() else {
                            continue;
                        };
                        if inner.is_empty() {
                            continue;
                        }
                        let period = periods.and_then(|p| p.get(k)).and_then(J::as_i64);
                        let name = match period {
                            Some(p) => format!("seasonal_{p}"),
                            None => format!("seasonal_{}", k + 1),
                        };
                        let (sxs, _) = list_xs(inner.len(), ds);
                        comps.push(Comp {
                            name,
                            xs: sxs,
                            ys: inner.iter().map(json_num).collect(),
                            model: mv.clone(),
                        });
                    }
                }
                _ if outer.len() == n => {
                    comps.push(Comp {
                        name: "seasonal".into(),
                        xs: xs.clone(),
                        ys: outer.iter().map(json_num).collect(),
                        model: mv.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    fill_observed(&mut comps);
    sort_by_rank(&mut comps);
    let label = if saw_date {
        "date".to_string()
    } else {
        label.map(str::to_string).unwrap_or_else(|| "period".into())
    };
    Ok((comps, label))
}

/// The components a `decomposition` table draws, in panel order, and the
/// default x axis label.
fn components(t: &Table, options: &Row) -> Result<(Vec<Comp>, String), String> {
    if t.has("id") && t.distinct("id") > 1 {
        return Err(format!(
            "rows hold {} series (id): GROUP BY id for one plot each, or WHERE id = …",
            t.distinct("id")
        ));
    }
    let (mut comps, label) = if t.has("component") {
        long_form(t, options)?
    } else if is_list_form(t) {
        list_form(t, options)?
    } else {
        wide_form(t, options)?
    };
    // Drop components whose values are all non-finite (explain's NULL residual).
    comps.retain(|c| c.ys.iter().any(|v| fin(v).is_some()));
    sort_by_rank(&mut comps);
    if let Some(J::Array(a)) = options.get("components") {
        let avail: Vec<String> = {
            let mut v = Vec::new();
            for c in &comps {
                if !v.contains(&c.name) {
                    v.push(c.name.clone());
                }
            }
            v
        };
        let want: Vec<String> = a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let mut order: Vec<String> = Vec::new();
        for w in &want {
            match avail.iter().find(|n| n.eq_ignore_ascii_case(w)) {
                Some(n) => {
                    if !order.contains(n) {
                        order.push(n.clone());
                    }
                }
                None => {
                    return Err(format!(
                        "components := '{w}' is not one of {}",
                        avail.join(", ")
                    ))
                }
            }
        }
        comps.retain(|c| order.contains(&c.name));
        comps.sort_by_key(|c| {
            order
                .iter()
                .position(|n| n == &c.name)
                .unwrap_or(usize::MAX)
        });
    }
    Ok((comps, label))
}

/// `anofox_plot_decomposition(tbl)`: stacked panels sharing one x axis, each
/// with its own y scale — observed, level, trend, the seasonal components,
/// the remainder/residual as points, drawn last.
pub(super) fn decomposition(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let labels = Labels::from(options);
    let (comps, xdefault) = components(t, options)?;
    if comps.is_empty() {
        return Err(format!(
            "needs component data: a long table (component, value), wide columns (trend, \
             seasonal, remainder, …) or a list output (got: {})",
            t.keys()
        ));
    }
    let mut models: Vec<String> = Vec::new();
    for c in &comps {
        if let Some(m) = &c.model {
            if !models.contains(m) {
                models.push(m.clone());
            }
        }
    }
    let several = models.len() > 1;
    let mut line: Frame = vec![
        ("x".into(), Vec::new()),
        ("y".into(), Vec::new()),
        ("facet".into(), Vec::new()),
        ("model".into(), Vec::new()),
    ];
    let mut point: Frame = vec![
        ("x".into(), Vec::new()),
        ("y".into(), Vec::new()),
        ("facet".into(), Vec::new()),
        ("model".into(), Vec::new()),
    ];
    for c in &comps {
        let frame = if is_point(&c.name) {
            &mut point
        } else {
            &mut line
        };
        let disp = s(display_name(&c.name));
        let model = s(c.model.clone().unwrap_or_default());
        for i in 0..c.xs.len() {
            frame[0].1.push(c.xs[i].clone());
            frame[1].1.push(c.ys[i].clone());
            frame[2].1.push(disp.clone());
            frame[3].1.push(model.clone());
        }
    }
    let mut aes = Aes::new().x("x").y("y");
    if several {
        aes = aes.color("model").group("model");
    }
    let brand = o.brand();
    let mut plot = GGPlot::new(line).aes(aes.clone()).geom_line_with(GeomLine {
        color: brand,
        width: 1.6,
        alpha: 1.0,
        ..Default::default()
    });
    if several {
        plot = plot.scale_color(look::series_scale(Aesthetic::Color, &models));
    }
    if !point[0].1.is_empty() {
        plot = plot
            .geom_point_with(GeomPoint {
                size: 1.6,
                color: brand,
                alpha: 0.85,
                ..Default::default()
            })
            .layer_data(point)
            .layer_aes(aes);
    }
    plot = plot.facet_wrap_free("facet", Some(1), FacetScales::FreeY);
    let plot = labels.apply(plot, &xdefault, "");
    stats::finish(look::styled(plot, o, false), w, h, place)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(json: &str) -> Vec<Row> {
        serde_json::from_str(json).unwrap()
    }
    fn names(comps: &[Comp]) -> Vec<String> {
        comps.iter().map(|c| c.name.clone()).collect()
    }
    fn opts(json: &str) -> Row {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn long_form_three_components_in_order() {
        let rows = table(
            r#"[{"ds":"2024-01-01","component":"trend","value":1.0},
                {"ds":"2024-01-02","component":"trend","value":1.2},
                {"ds":"2024-01-01","component":"seasonal","value":0.1},
                {"ds":"2024-01-02","component":"seasonal","value":-0.1},
                {"ds":"2024-01-01","component":"remainder","value":0.02},
                {"ds":"2024-01-02","component":"remainder","value":-0.01}]"#,
        );
        let t = Table {
            rows: &rows,
            renamed: &[],
        };
        let (comps, label) = components(&t, &Row::new()).unwrap();
        assert_eq!(names(&comps), vec!["trend", "seasonal", "remainder"]);
        assert_eq!(label, "date");
    }

    #[test]
    fn wide_orchestration_like_four_components() {
        let rows: Vec<Row> = (0..10)
            .map(|i| {
                serde_json::from_str(&format!(
                    r#"{{"id":"a","statistic":{i},"observed":{o},"trend":1.0,"seasonal":0.1,
                        "remainder":0.0,"method":"stl"}}"#,
                    o = i as f64
                ))
                .unwrap()
            })
            .collect();
        let renamed = [("t".to_string(), "statistic".to_string())];
        let t = Table {
            rows: &rows,
            renamed: &renamed,
        };
        let (comps, label) = components(&t, &Row::new()).unwrap();
        assert_eq!(comps.len(), 4, "{:?}", names(&comps));
        assert_eq!(
            names(&comps),
            vec!["observed", "trend", "seasonal", "remainder"]
        );
        assert_eq!(label, "t");
        // method is constant: no model split.
        assert!(comps.iter().all(|c| c.model.is_none()));
    }

    #[test]
    fn mstl_list_form() {
        let rows = table(
            r#"[{"id":"B","trend":[1.0,2.0,3.0],"seasonal":[[0.1,-0.1,0.0]],
                "remainder":[0.0,0.1,-0.1],"periods":[7]}]"#,
        );
        let t = Table {
            rows: &rows,
            renamed: &[],
        };
        let (comps, _) = components(&t, &Row::new()).unwrap();
        assert_eq!(
            names(&comps),
            vec!["observed", "trend", "seasonal_7", "remainder"]
        );
    }

    #[test]
    fn explain_like_list_form_drops_null_residual() {
        // Already flattened, as `render_plot`'s flatten step would leave it.
        let rows = table(
            r#"[{"id":"A","horizon":5,"level":[1.0,1.1,1.2,1.3,1.4],
                "trend":[1.0,1.0,1.0,1.0,1.0],"seasonal":[0.1,-0.1,0.0,0.1,-0.1],
                "residual":null}]"#,
        );
        let t = Table {
            rows: &rows,
            renamed: &[],
        };
        let (comps, _) = components(&t, &Row::new()).unwrap();
        assert_eq!(names(&comps), vec!["level", "trend", "seasonal"]);
    }

    #[test]
    fn two_ids_is_an_error() {
        let rows = table(
            r#"[{"id":"A","ds":"2024-01-01","component":"trend","value":1.0},
                {"id":"B","ds":"2024-01-01","component":"trend","value":2.0}]"#,
        );
        let t = Table {
            rows: &rows,
            renamed: &[],
        };
        let e = components(&t, &Row::new()).unwrap_err();
        assert!(e.contains("GROUP BY id"), "{e}");
    }

    #[test]
    fn components_option_filters_and_orders() {
        let rows = table(
            r#"[{"ds":"2024-01-01","component":"trend","value":1.0},
                {"ds":"2024-01-01","component":"seasonal","value":0.1},
                {"ds":"2024-01-01","component":"remainder","value":0.0}]"#,
        );
        let t = Table {
            rows: &rows,
            renamed: &[],
        };
        let (comps, _) = components(&t, &opts(r#"{"components":["trend"]}"#)).unwrap();
        assert_eq!(names(&comps), vec!["trend"]);
        let e = components(&t, &opts(r#"{"components":["nope"]}"#)).unwrap_err();
        assert!(e.contains("components := 'nope' is not one of"), "{e}");
    }
}

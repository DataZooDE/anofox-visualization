//! `schedule` (#176): a Gantt chart — one lane per `resource`, a bar per
//! `task` from `start` to `end`, coloured by `status` (`on_time`/`late`,
//! else muted), a due-date mark when `due` is present, grouped (facet) by
//! `model_id` to compare the schedules of several algorithms on the same
//! resources. One row per task; anofox-optimize's `schedule_*` solvers (as a
//! `*_by` long form, DataZooDE/anofox-optimize#6) are the first intended
//! producer. Replaces drawing a schedule as `::SCATTER` + `::XMIN`/`::XMAX`
//! (error bars, not bars; #171).

use super::cv;
use super::look::{self, Labels};
use super::stats::{self, fin, s, text_of, Frame, Out};
use super::{Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;

/// One task: a bar on its resource's lane, optionally due at a mark and
/// tagged with a status, within one algorithm's schedule (`model`).
struct Task {
    model: Option<String>,
    resource: String,
    task: String,
    start: f64,
    end: f64,
    due: Option<f64>,
    status_raw: Option<String>,
}

/// Normalise a `status` value: `on time`/`on-time`/`ontime` → `on_time`,
/// `late`/`delayed`/`overdue` → `late`, anything else kept as given (drawn
/// muted, like `checks`' unknown statuses). Missing entirely, every task
/// gets the synthetic `scheduled` status (drawn in the brand colour, no
/// legend — see [`schedule`]).
fn status_of(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on_time" | "on-time" | "ontime" | "on time" => "on_time".to_string(),
        "late" | "delayed" | "overdue" => "late".to_string(),
        other => other.to_string(),
    }
}

/// The colour of a normalised status: `on_time`/`late` from the theme, the
/// synthetic `scheduled` (no `status` column at all) in the brand colour,
/// anything else muted.
fn status_color(status: &str, o: &RenderOptions) -> (u8, u8, u8) {
    match status {
        "on_time" => o.theme.colors.positive,
        "late" => o.theme.colors.negative,
        "scheduled" => o.brand(),
        _ => o.theme.colors.muted,
    }
}

/// `task · resource: start – end[ · status][ · due …]`.
fn tip(t: &Task, dates: bool) -> String {
    let mut out = format!(
        "{} · {}: {} – {}",
        t.task,
        t.resource,
        cv::x_text(t.start, dates),
        cv::x_text(t.end, dates)
    );
    if let Some(st) = &t.status_raw {
        out.push_str(&format!(" · {st}"));
    }
    if let Some(d) = t.due {
        out.push_str(&format!(" · due {}", cv::x_text(d, dates)));
    }
    out
}

/// `anofox_plot_schedule(tbl)`: a Gantt chart. One lane per `resource`, a bar
/// per `task` from `start` to `end`, coloured by `status` (`on_time`/`late`,
/// else muted), a dot at `due` when it is present, faceted by `model_id`
/// (algorithm) when there are several — up to 6, else `GROUP BY model_id` or
/// `WHERE model_id = …`.
pub(super) fn schedule(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    t.need(&["resource", "task", "start", "end"])?;
    let labels = Labels::from(options);
    let model_key = t.model_key();

    let start_v = t.xpos("start");
    let end_v = t.xpos("end");
    let dates = start_v
        .iter()
        .chain(end_v.iter())
        .any(|v| matches!(v, Value::DateTime(_)));
    let resource_v = t.text("resource");
    let task_v = t.text("task");
    let due_v = t.has("due").then(|| t.xpos("due"));
    let status_v = t.has("status").then(|| t.text("status"));
    let model_v = model_key.map(|k| t.text(k));

    let tasks: Vec<Task> = (0..t.rows.len())
        .filter_map(|i| {
            let start = fin(&start_v[i])?;
            let end = fin(&end_v[i])?;
            Some(Task {
                model: model_v.as_ref().and_then(|v| text_of(&v[i])),
                resource: text_of(&resource_v[i]).unwrap_or_else(|| "(unassigned)".into()),
                task: text_of(&task_v[i]).unwrap_or_else(|| format!("task {}", i + 1)),
                start,
                end,
                due: due_v.as_ref().and_then(|v| fin(&v[i])),
                status_raw: status_v.as_ref().and_then(|v| text_of(&v[i])),
            })
        })
        .collect();
    if tasks.is_empty() {
        return Err("no tasks (every row is NA on `start` or `end`)".to_string());
    }

    // Lanes: every resource, alphabetically — stable whichever algorithm's
    // rows happen to list it first.
    let mut resources: Vec<String> = tasks.iter().map(|tk| tk.resource.clone()).collect();
    resources.sort();
    resources.dedup();
    let n = resources.len();

    // Facet by model_id (algorithm) when there is more than one.
    let mut models: Vec<String> = Vec::new();
    if model_key.is_some() {
        for tk in &tasks {
            if let Some(m) = &tk.model {
                if !models.contains(m) {
                    models.push(m.clone());
                }
            }
        }
    }
    let facet = models.len() > 1;
    if models.len() > 6 {
        return Err(format!(
            "rows hold {} algorithms: GROUP BY model_id or WHERE model_id = … for one Gantt each",
            models.len()
        ));
    }

    let status_key = |tk: &Task| match &tk.status_raw {
        Some(raw) => status_of(raw),
        None => "scheduled".to_string(),
    };
    let mut levels: Vec<String> = Vec::new();
    for want in ["on_time", "late"] {
        if tasks.iter().any(|tk| status_key(tk) == want) {
            levels.push(want.to_string());
        }
    }
    for tk in &tasks {
        let st = status_key(tk);
        if !levels.contains(&st) {
            levels.push(st);
        }
    }
    let palette: Vec<RGBAColor> = levels
        .iter()
        .map(|l| {
            let (r, g, b) = status_color(l, o);
            RGBAColor::new(r, g, b)
        })
        .collect();

    let mk = |v: f64| {
        if dates {
            Value::DateTime(v.round() as i64)
        } else {
            Value::Float(v)
        }
    };
    let mut xmin = Vec::new();
    let mut xmax = Vec::new();
    let mut ymin = Vec::new();
    let mut ymax = Vec::new();
    let mut split_col = Vec::new();
    let mut tip_col = Vec::new();
    let mut model_col = Vec::new();
    let mut n_late = 0usize;
    for tk in &tasks {
        let idx = resources
            .iter()
            .position(|r| r == &tk.resource)
            .unwrap_or(0);
        let slot = stats::slot(idx, n);
        xmin.push(mk(tk.start));
        xmax.push(mk(tk.end));
        ymin.push(Value::Float(slot - 0.32));
        ymax.push(Value::Float(slot + 0.32));
        let st = status_key(tk);
        if st == "late" {
            n_late += 1;
        }
        split_col.push(s(st));
        tip_col.push(s(tip(tk, dates)));
        if facet {
            model_col.push(s(tk.model.clone().unwrap_or_default()));
        }
    }
    let mut frame: Frame = vec![
        ("xmin".into(), xmin),
        ("xmax".into(), xmax),
        ("ymin".into(), ymin),
        ("ymax".into(), ymax),
        ("split".into(), split_col),
        ("tip".into(), tip_col),
    ];
    if facet {
        frame.push(("model".into(), model_col));
    }

    let mut plot = cv::lane_rects(frame, levels.clone(), palette, o);
    if facet {
        plot = plot.facet_wrap("model", Some(1));
    }
    plot = plot.scale_y_continuous(stats::label_axis(&resources));

    // A due-date mark: a small dot at `due` on the task's own lane, drawn in
    // ink so it reads against every status colour.
    let due_rows: Vec<(usize, &Task)> = tasks
        .iter()
        .enumerate()
        .filter(|(_, tk)| tk.due.is_some())
        .collect();
    let has_due = !due_rows.is_empty();
    if has_due {
        let mut due_x = Vec::new();
        let mut due_y = Vec::new();
        let mut due_tip = Vec::new();
        for (_, tk) in &due_rows {
            let idx = resources
                .iter()
                .position(|r| r == &tk.resource)
                .unwrap_or(0);
            due_x.push(mk(tk.due.expect("filtered")));
            due_y.push(Value::Float(stats::slot(idx, n)));
            due_tip.push(s(format!(
                "{}: due {}",
                tk.task,
                cv::x_text(tk.due.expect("filtered"), dates)
            )));
        }
        let mut due_frame: Frame = vec![
            ("due_x".into(), due_x),
            ("due_y".into(), due_y),
            ("due_tip".into(), due_tip),
        ];
        if facet {
            due_frame.push((
                "model".into(),
                due_rows
                    .iter()
                    .map(|(_, tk)| s(tk.model.clone().unwrap_or_default()))
                    .collect(),
            ));
        }
        plot = plot
            .geom_point_with(GeomPoint {
                size: 3.4,
                color: o.theme.colors.ink,
                alpha: 0.9,
            })
            .layer_data(due_frame)
            // `tooltip`, not `label`: a plain `label` on a point gets the
            // y value appended ("text: 1", the lane slot) by GeomPoint's
            // own default tip; `tooltip` replaces it outright.
            .layer_aes(Aes::new().x("due_x").y("due_y").tooltip("due_tip"));
    }

    let xdefault = if dates {
        "date".to_string()
    } else {
        "time".to_string()
    };
    // The lanes are labelled directly: no y title by default, as the fold
    // and event timelines; ylab := still sets one.
    let plot = labels.apply(look::styled(plot, o, false), &xdefault, "");
    let n_resources = resources.len();
    let n_tasks = tasks.len();
    let mut cap = format!(
        "{} task{} on {} resource{}",
        n_tasks,
        if n_tasks == 1 { "" } else { "s" },
        n_resources,
        if n_resources == 1 { "" } else { "s" }
    );
    if n_late > 0 {
        cap.push_str(&format!(" · {n_late} late"));
    }
    if has_due {
        cap.push_str(" · due dates marked");
    }
    let mut plot = plot.caption(&cap);
    if levels.len() < 2 {
        plot = plot.legend_position(ggplot_rs::theme::LegendPosition::None);
    }
    stats::finish(plot, w, h, place)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(json: &str) -> Vec<Row> {
        serde_json::from_str(json).unwrap()
    }

    fn table(rows: &[Row]) -> Table<'_> {
        Table { rows, renamed: &[] }
    }

    #[test]
    fn status_normalizes() {
        assert_eq!(status_of("On Time"), "on_time");
        assert_eq!(status_of("on-time"), "on_time");
        assert_eq!(status_of("LATE"), "late");
        assert_eq!(status_of("delayed"), "late");
        assert_eq!(status_of("changeover"), "changeover");
    }

    #[test]
    fn two_resources_two_tasks() {
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10,"status":"on time","due":12},
                {"resource":"L2","task":"O2","start":0,"end":5,"status":"late"}]"#,
        );
        let t = table(&r);
        let svg = schedule(
            &t,
            &Row::new(),
            640,
            400,
            &RenderOptions::default(),
            Place::Doc,
        )
        .unwrap()
        .0;
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn facets_by_model_id_up_to_six() {
        let mut items = Vec::new();
        for m in 0..7 {
            items.push(format!(
                r#"{{"model_id":"m{m}","resource":"L1","task":"O1","start":0,"end":10}}"#
            ));
        }
        let json = format!("[{}]", items.join(","));
        let r = rows(&json);
        let t = table(&r);
        let e = schedule(
            &t,
            &Row::new(),
            640,
            400,
            &RenderOptions::default(),
            Place::Doc,
        )
        .unwrap_err();
        assert!(e.contains("GROUP BY model_id"), "{e}");
    }

    #[test]
    fn needs_resource_task_start_end() {
        let r = rows(r#"[{"resource":"L1","start":0,"end":10}]"#);
        let t = table(&r);
        let e = schedule(
            &t,
            &Row::new(),
            640,
            400,
            &RenderOptions::default(),
            Place::Doc,
        )
        .unwrap_err();
        assert!(e.contains("needs column(s) task"), "{e}");
    }
}

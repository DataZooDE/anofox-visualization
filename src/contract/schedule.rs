//! `schedule` (#176): a Gantt chart — one lane per `resource`, a bar per
//! `task` from `start` to `end`, coloured by `status` (`on time`/`late`,
//! else muted), a due-date mark for late tasks when `due` is present,
//! grouped (facet) by `model_id` to compare the schedules of several
//! algorithms on the same resources. One row per task; anofox-optimize's
//! `schedule_*` solvers (as a `*_by` long form, DataZooDE/anofox-optimize#6)
//! are the first intended producer. Replaces drawing a schedule as
//! `::SCATTER` + `::XMIN`/`::XMAX` (error bars, not bars; #171).

use super::cv;
use super::look::{self, Labels};
use super::stats::{self, fin, s, text_of, Frame, Out};
use super::{Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;

/// A resource with no value (NULL/empty) draws on its own lane under this
/// label, rather than being dropped — the row is still a real task.
const UNASSIGNED: &str = "(unassigned)";
/// A row whose `model_id` is NULL while others facet by it: its own panel,
/// rather than silently sharing the empty-string facet value with no title.
const NO_MODEL: &str = "(none)";

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

/// Normalise a `status` value: `on time`/`on-time`/`ontime` → `on time`,
/// `late`/`delayed`/`overdue` → `late`, anything else kept as given (drawn
/// muted, like `checks`' unknown statuses — anofox-optimize's changeover
/// slivers, say). Missing entirely, every task gets the synthetic
/// `scheduled` status (drawn in the brand colour, no legend — see
/// [`schedule`]). The canonical spelling has a space, not an underscore: it
/// is also the legend text.
fn status_of(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on_time" | "on-time" | "ontime" | "on time" => "on time".to_string(),
        "late" | "delayed" | "overdue" => "late".to_string(),
        other => other.to_string(),
    }
}

/// The colour of a normalised status: `on time`/`late` from the theme, the
/// synthetic `scheduled` (no `status` column at all) in the brand colour,
/// anything else muted.
fn status_color(status: &str, o: &RenderOptions) -> (u8, u8, u8) {
    match status {
        "on time" => o.theme.colors.positive,
        "late" => o.theme.colors.negative,
        "scheduled" => o.brand(),
        _ => o.theme.colors.muted,
    }
}

/// `cv::x_text`, plus the time of day when `with_time` (any start/end/due
/// value in this chart carries one): `YYYY-MM-DD HH:MM` instead of
/// `YYYY-MM-DD`. Producers may pass plain `DATE`s (a shift-level schedule)
/// or `TIMESTAMP`s (sequenced to the minute); detected once for the whole
/// chart ([`schedule`]) so every tooltip and the axis share one format.
/// `TIMESTAMPTZ` text is read in UTC by the shared parser
/// (`sql::parse_iso_epoch`), same as every other contract plot.
fn x_text_sched(v: f64, dates: bool, with_time: bool) -> String {
    if !with_time {
        return cv::x_text(v, dates);
    }
    let secs = v.round() as i64;
    let day_secs = secs.rem_euclid(86_400);
    let (hh, mm) = (day_secs / 3_600, (day_secs % 3_600) / 60);
    format!("{} {hh:02}:{mm:02}", cv::ymd((secs - day_secs) as f64))
}

/// `task · resource: start – end[ · status][ · due …]`.
fn tip(t: &Task, dates: bool, with_time: bool) -> String {
    let mut out = format!(
        "{} · {}: {} – {}",
        t.task,
        t.resource,
        x_text_sched(t.start, dates, with_time),
        x_text_sched(t.end, dates, with_time)
    );
    if let Some(st) = &t.status_raw {
        out.push_str(&format!(" · {st}"));
    }
    if let Some(d) = t.due {
        out.push_str(&format!(" · due {}", x_text_sched(d, dates, with_time)));
    }
    out
}

/// Natural-sort key order: text runs compare as text, digit runs compare
/// numerically (then by width, so `"007"` sorts after `"07"`), so lanes
/// read `L1, L2, L10` and `Machine 9, Machine 10` — a plain `sort()` would
/// put `L10` before `L2`. Local to `schedule`: `cv`'s fold lanes already
/// sort by their own parsed number, and `checks::sort_aware` only compares
/// a whole label as one number, not interleaved text/number runs.
fn natural_chunks(s: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let digit = bytes[i].is_ascii_digit();
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() == digit {
            i += 1;
        }
        out.push((digit, &s[start..i]));
    }
    out
}

fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (ca, cb) = (natural_chunks(a), natural_chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let ord = match (x.0, y.0) {
            (true, true) => match (x.1.parse::<u128>(), y.1.parse::<u128>()) {
                (Ok(nx), Ok(ny)) => nx.cmp(&ny).then_with(|| x.1.len().cmp(&y.1.len())),
                _ => x.1.cmp(y.1),
            },
            _ => x.1.cmp(y.1),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len())
}

/// `anofox_plot_schedule(tbl)`: a Gantt chart. One lane per `resource`, a bar
/// per `task` from `start` to `end`, coloured by `status` (`on time`/`late`,
/// else muted), a due-date connector for late tasks when `due` is present,
/// faceted by `model_id` (algorithm) when there are several — up to 6, else
/// `GROUP BY model_id` or `WHERE model_id = …`.
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

    // `due` must be the same kind of x as `start`/`end` — a numeric `start`/
    // `end` with a DATE `due` (or vice versa) would otherwise plot at
    // whatever epoch seconds its number happens to read as.
    if let Some(due_vals) = &due_v {
        let due_is_date = due_vals.iter().any(|v| matches!(v, Value::DateTime(_)));
        let due_present = due_vals.iter().any(|v| fin(v).is_some());
        if due_present && due_is_date != dates {
            return Err(format!(
                "`due` is {} but `start`/`end` are {} — both must be the same type",
                if due_is_date { "a date" } else { "not a date" },
                if dates { "dates" } else { "not dates" }
            ));
        }
    }
    // Any date value off its own midnight means the chart is really at
    // timestamp, not date, granularity: every tooltip and the axis title
    // follow suit (see `x_text_sched`).
    let with_time = dates
        && start_v
            .iter()
            .chain(end_v.iter())
            .chain(due_v.iter().flatten())
            .any(|v| fin(v).is_some_and(|f| (f.round() as i64).rem_euclid(86_400) != 0));

    let mut dropped_na = 0usize;
    let mut dropped_order = 0usize;
    let mut tasks: Vec<Task> = (0..t.rows.len())
        .filter_map(|i| {
            let (Some(start), Some(end)) = (fin(&start_v[i]), fin(&end_v[i])) else {
                dropped_na += 1;
                return None;
            };
            if end < start {
                dropped_order += 1;
                return None;
            }
            Some(Task {
                model: model_v.as_ref().and_then(|v| text_of(&v[i])),
                resource: text_of(&resource_v[i]).unwrap_or_else(|| UNASSIGNED.to_string()),
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

    // A zero-width task (start = end) still gets a bar: widened to a
    // sliver's worth of the plotted range rather than invisible.
    let span = tasks
        .iter()
        .map(|tk| tk.end)
        .fold(f64::NEG_INFINITY, f64::max)
        - tasks
            .iter()
            .map(|tk| tk.start)
            .fold(f64::INFINITY, f64::min);
    let min_width = if span > 0.0 {
        (span * 0.005).max(1e-6)
    } else {
        1.0
    };
    let mut n_widened = 0usize;
    for tk in tasks.iter_mut() {
        if (tk.end - tk.start).abs() < 1e-9 {
            tk.end = tk.start + min_width;
            n_widened += 1;
        }
    }

    // Lanes: every resource, natural order — stable whichever algorithm's
    // rows happen to list it first, and `L2` before `L10`.
    let mut resources: Vec<String> = tasks.iter().map(|tk| tk.resource.clone()).collect();
    resources.sort_by(|a, b| natural_cmp(a, b));
    resources.dedup();
    let n = resources.len();

    // Facet by model_id (algorithm) when there is more than one; a NULL
    // model_id among named ones gets its own "(none)" panel rather than
    // silently sharing the empty string.
    let model_label = |tk: &Task| tk.model.clone().unwrap_or_else(|| NO_MODEL.to_string());
    let mut models: Vec<String> = Vec::new();
    if model_key.is_some() {
        for tk in &tasks {
            let m = model_label(tk);
            if !models.contains(&m) {
                models.push(m);
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
    for want in ["on time", "late"] {
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
        tip_col.push(s(tip(tk, dates, with_time)));
        if facet {
            model_col.push(s(model_label(tk)));
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

    // A due-date connector for late tasks only: a short segment from the
    // bar's actual end to `due`, on that task's own lane, in the late
    // colour. On-time tasks keep `due` in their tooltip alone — a dot at
    // `due` for every task, on a lane several tasks may share, cannot be
    // matched back to the task it belongs to (#378 review).
    let late_due: Vec<&Task> = tasks
        .iter()
        .filter(|tk| status_key(tk) == "late" && tk.due.is_some())
        .collect();
    let has_due_marks = !late_due.is_empty();
    if has_due_marks {
        let mut seg_x = Vec::new();
        let mut seg_xend = Vec::new();
        let mut seg_y = Vec::new();
        let mut seg_tip = Vec::new();
        let mut seg_model = Vec::new();
        for tk in &late_due {
            let idx = resources
                .iter()
                .position(|r| r == &tk.resource)
                .unwrap_or(0);
            let slot = stats::slot(idx, n);
            let due = tk.due.expect("filtered");
            seg_x.push(mk(tk.end));
            seg_xend.push(mk(due));
            seg_y.push(Value::Float(slot));
            seg_tip.push(s(format!(
                "{}: due {}",
                tk.task,
                x_text_sched(due, dates, with_time)
            )));
            if facet {
                seg_model.push(s(model_label(tk)));
            }
        }
        let mut seg_frame: Frame = vec![
            ("seg_x".into(), seg_x),
            ("seg_xend".into(), seg_xend),
            ("seg_y".into(), seg_y),
            ("seg_tip".into(), seg_tip),
        ];
        if facet {
            seg_frame.push(("model".into(), seg_model));
        }
        plot = plot
            .geom_segment_with(GeomSegment {
                color: o.theme.colors.negative,
                width: 2.2,
                alpha: 0.9,
                ..Default::default()
            })
            .layer_data(seg_frame)
            .layer_aes(
                Aes::new()
                    .x("seg_x")
                    .xend("seg_xend")
                    .y("seg_y")
                    .yend("seg_y")
                    .tooltip("seg_tip"),
            );
    }

    let xdefault = if dates {
        if with_time {
            "time"
        } else {
            "date"
        }
    } else {
        "time"
    }
    .to_string();
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
        if facet {
            cap.push_str(&format!(" · {n_late} late across all models"));
        } else {
            cap.push_str(&format!(" · {n_late} late"));
        }
    }
    if has_due_marks {
        cap.push_str(" · late tasks' due dates marked");
    }
    let mut plot = plot.caption(&cap);
    if levels.len() < 2 {
        plot = plot.legend_position(ggplot_rs::theme::LegendPosition::None);
    }
    let (svg, mut warnings) = stats::finish(plot, w, h, place)?;
    if dropped_na > 0 {
        warnings.push(format!(
            "anofox_plot_schedule: {dropped_na} task{} dropped (NULL start or end)",
            if dropped_na == 1 { "" } else { "s" }
        ));
    }
    if dropped_order > 0 {
        warnings.push(format!(
            "anofox_plot_schedule: {dropped_order} task{} dropped (end before start)",
            if dropped_order == 1 { "" } else { "s" }
        ));
    }
    if n_widened > 0 {
        warnings.push(format!(
            "anofox_plot_schedule: {n_widened} task{} widened to a minimum visible width \
             (start = end)",
            if n_widened == 1 { "" } else { "s" }
        ));
    }
    Ok((svg, warnings))
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

    fn render(rows: &[Row]) -> (String, Vec<String>) {
        schedule(
            &table(rows),
            &Row::new(),
            640,
            400,
            &RenderOptions::default(),
            Place::Doc,
        )
        .unwrap()
    }

    #[test]
    fn status_normalizes() {
        assert_eq!(status_of("On Time"), "on time");
        assert_eq!(status_of("on-time"), "on time");
        assert_eq!(status_of("LATE"), "late");
        assert_eq!(status_of("delayed"), "late");
        assert_eq!(status_of("changeover"), "changeover");
    }

    #[test]
    fn natural_sort_orders_numbers_not_digits() {
        let mut v = vec!["L10", "L2", "L1", "Machine 10", "Machine 9"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["L1", "L2", "L10", "Machine 9", "Machine 10"]);
    }

    #[test]
    fn lanes_render_in_natural_order() {
        // Lanes are listed top to bottom in reverse of `label_axis`'s
        // y-ascending order (see `stats::label_axis`), so the natural-first
        // lane (L1) is the *last* `<text>` of the three in the SVG.
        let r = rows(
            r#"[{"resource":"L10","task":"O1","start":0,"end":5},
                {"resource":"L2","task":"O2","start":0,"end":5},
                {"resource":"L1","task":"O3","start":0,"end":5}]"#,
        );
        let (svg, _) = render(&r);
        let l10 = svg.find(">L10</text>").expect("L10 label");
        let l2 = svg.find(">L2</text>").expect("L2 label");
        let l1 = svg.find(">L1</text>").expect("L1 label");
        assert!(l10 < l2 && l2 < l1, "{svg}");
    }

    #[test]
    fn two_resources_two_tasks() {
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10,"status":"on time","due":12},
                {"resource":"L2","task":"O2","start":0,"end":5,"status":"late"}]"#,
        );
        let (svg, warnings) = render(&r);
        assert!(svg.contains("<svg"));
        assert!(warnings.is_empty(), "{warnings:?}");
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

    #[test]
    fn status_fill_colours_match_the_theme() {
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10,"status":"on time"},
                {"resource":"L1","task":"O2","start":10,"end":20,"status":"late"}]"#,
        );
        let (svg, _) = render(&r);
        let o = RenderOptions::default();
        let hex = |(r, g, b): (u8, u8, u8)| format!("#{r:02X}{g:02X}{b:02X}");
        assert!(svg.contains(&hex(o.theme.colors.positive)), "{svg}");
        assert!(svg.contains(&hex(o.theme.colors.negative)), "{svg}");
    }

    #[test]
    fn due_mark_only_for_late_tasks() {
        // on time with a due date: no connector (GeomSegment's own mark
        // carries data-geom="segment"), due stays in its tooltip.
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10,"status":"on time","due":12}]"#,
        );
        let (svg, _) = render(&r);
        assert!(!svg.contains("data-geom=\"segment\""), "{svg}");
        assert!(svg.contains("due 12"), "{svg}");

        // late with a due date: a connector from the bar's end (10) to due (8),
        // labelled with the task it belongs to.
        let r =
            rows(r#"[{"resource":"L1","task":"O1","start":0,"end":10,"status":"late","due":8}]"#);
        let (svg, _) = render(&r);
        assert!(svg.contains("data-geom=\"segment\""), "{svg}");
        assert!(svg.contains("<title>O1: due 8</title>"), "{svg}");

        // Two tasks share a lane: the connector belongs to the one it names,
        // not to whichever bar happens to sit under its y.
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10,"status":"on time","due":30},
                {"resource":"L1","task":"O2","start":10,"end":20,"status":"late","due":15}]"#,
        );
        let (svg, _) = render(&r);
        assert_eq!(svg.matches("data-geom=\"segment\"").count(), 1, "{svg}");
        assert!(svg.contains("<title>O2: due 15</title>"), "{svg}");
    }

    #[test]
    fn null_start_or_end_is_dropped_and_warned() {
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10},
                {"resource":"L1","task":"O2","start":null,"end":10}]"#,
        );
        let (_, warnings) = render(&r);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("1 task dropped (NULL start or end)")),
            "{warnings:?}"
        );
    }

    #[test]
    fn end_before_start_is_dropped_and_warned() {
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10},
                {"resource":"L1","task":"O2","start":10,"end":5}]"#,
        );
        let (_, warnings) = render(&r);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("1 task dropped (end before start)")),
            "{warnings:?}"
        );
    }

    #[test]
    fn zero_width_task_is_widened_and_warned() {
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":0,"end":10},
                {"resource":"L1","task":"O2","start":5,"end":5}]"#,
        );
        let (_, warnings) = render(&r);
        assert!(
            warnings.iter().any(|w| w.contains("1 task widened")),
            "{warnings:?}"
        );
    }

    #[test]
    fn numeric_start_end_with_date_due_is_an_error() {
        let r = rows(r#"[{"resource":"L1","task":"O1","start":0,"end":10,"due":"2024-01-01"}]"#);
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
        assert!(e.contains("must be the same type"), "{e}");
    }

    #[test]
    fn timestamps_off_midnight_show_the_time_and_title_the_axis_time() {
        let r = rows(
            r#"[{"resource":"L1","task":"O1","start":"2024-01-01 06:00","end":"2024-01-01 14:30"}]"#,
        );
        let (svg, _) = render(&r);
        assert!(svg.contains("06:00") && svg.contains("14:30"), "{svg}");
        assert!(svg.contains(">time</text>"), "{svg}");
    }

    #[test]
    fn plain_dates_title_the_axis_date() {
        let r = rows(r#"[{"resource":"L1","task":"O1","start":"2024-01-01","end":"2024-01-03"}]"#);
        let (svg, _) = render(&r);
        assert!(svg.contains(">date</text>"), "{svg}");
    }

    #[test]
    fn null_model_id_gets_its_own_none_panel() {
        let r = rows(
            r#"[{"model_id":"exact","resource":"L1","task":"O1","start":0,"end":10},
                {"model_id":null,"resource":"L1","task":"O2","start":0,"end":10}]"#,
        );
        let (svg, _) = render(&r);
        assert!(svg.contains(">(none)</text>"), "{svg}");
    }
}

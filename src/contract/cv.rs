//! `cv` (provisional; DataZooDE/anofox-visualization#189): a cross-validation
//! fold timeline — the train and test windows of each fold as bars along
//! `ds` / `period` (long form: `fold_id, split, ds | period | x`) or as one
//! row per fold (span form: `fold_id, train_start, train_end, test_start,
//! test_end`). Reads anofox-forecast's `ts_cv_folds_by` / `ts_cv_split_by`
//! (long) and `ts_cv_split_folds_by` (span) as they come.

use super::look::{self, Labels};
use super::stats::{self, fin, fmt_num, s, text_of, Frame, Out, ACCENT};
use super::{opt_str, Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;

/// One drawn bar: a fold's train or test window.
struct Span {
    id: Option<String>,
    fold: String,
    split: String,
    start: f64,
    end: f64,
}

/// The x keys a long-form row may carry, in order of preference.
const KEYS: &[&str] = &["ds", "period", "x"];

/// `train|training` → `train`; `test|validation|valid|holdout|hold-out` →
/// `test`; anything else is kept as given.
fn normalize_split(v: &str) -> String {
    match v.to_ascii_lowercase().as_str() {
        "train" | "training" => "train".to_string(),
        "test" | "validation" | "valid" | "holdout" | "hold-out" => "test".to_string(),
        other => other.to_string(),
    }
}

fn fold_label(f: f64) -> String {
    format!("fold {}", fmt_num(f))
}

/// `YYYY-MM-DD` from seconds since the Unix epoch (UTC, proleptic Gregorian;
/// Howard Hinnant's `civil_from_days`).
fn ymd(secs: f64) -> String {
    let days = (secs / 86_400.0).floor() as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 {
        yoe + era * 400 + 1
    } else {
        yoe + era * 400
    };
    format!("{y:04}-{m:02}-{d:02}")
}

fn x_text(v: f64, dates: bool) -> String {
    if dates {
        ymd(v)
    } else {
        fmt_num(v)
    }
}

/// The smallest positive difference between consecutive sorted distinct
/// values of `xs` (0 when there are fewer than two).
fn step_of(xs: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = xs.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut step = f64::INFINITY;
    for w in sorted.windows(2) {
        let d = w[1] - w[0];
        if d > 0.0 && d < step {
            step = d;
        }
    }
    if step.is_finite() {
        step
    } else {
        0.0
    }
}

/// Long form: one row per (id, fold_id, split, x); grouped into one [`Span`]
/// per (id, fold, split) spanning its rows' x values.
fn long_form(t: &Table, options: &Row) -> Result<(Vec<Span>, f64, bool, String), String> {
    let key = opt_str(options, "x")
        .or_else(|| KEYS.iter().find(|k| t.has(k)).map(|k| k.to_string()))
        .ok_or_else(|| {
            format!(
                "needs a time column: `ds`, `period` or `x`, or x := '<column>' (got: {})",
                t.keys()
            )
        })?;
    t.need(&["fold_id", "split", key.as_str()])?;
    let xs = t.xpos(&key);
    let dates = xs.iter().any(|v| matches!(v, Value::DateTime(_)));
    let folds = t.num("fold_id")?;
    let splits = t.text("split");
    let ids = t.has("id").then(|| t.text("id"));
    // (id, fold label, split) → the row's x values.
    type BucketKey = (Option<String>, String, String);
    let mut buckets: Vec<(BucketKey, Vec<f64>)> = Vec::new();
    for i in 0..t.rows.len() {
        let (Some(f), Some(x)) = (fin(&folds[i]), fin(&xs[i])) else {
            continue;
        };
        let split = normalize_split(&text_of(&splits[i]).unwrap_or_default());
        let gid = ids.as_ref().and_then(|v| text_of(&v[i]));
        let key = (gid, fold_label(f), split);
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(x),
            None => buckets.push((key, vec![x])),
        }
    }
    let spans: Vec<Span> = buckets
        .into_iter()
        .map(|((id, fold, split), xs)| Span {
            id,
            fold,
            split,
            start: xs.iter().cloned().fold(f64::INFINITY, f64::min),
            end: xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        })
        .collect();
    let all: Vec<f64> = xs.iter().filter_map(fin).collect();
    Ok((spans, step_of(&all), dates, key))
}

/// Span form: one row per fold_id, with train_start/train_end and/or
/// test_start/test_end giving one or two [`Span`]s.
fn span_form(t: &Table) -> Result<(Vec<Span>, f64, bool, String), String> {
    let has_train = t.has("train_start") && t.has("train_end");
    let has_test = t.has("test_start") && t.has("test_end");
    if !has_train && !has_test {
        return Err(format!(
            "needs column(s) train_start, train_end or test_start, test_end (got: {})",
            t.keys()
        ));
    }
    t.need(&["fold_id"])?;
    let folds = t.num("fold_id")?;
    let ids = t.has("id").then(|| t.text("id"));
    let (train_s, train_e) = if has_train {
        (Some(t.xpos("train_start")), Some(t.xpos("train_end")))
    } else {
        (None, None)
    };
    let (test_s, test_e) = if has_test {
        (Some(t.xpos("test_start")), Some(t.xpos("test_end")))
    } else {
        (None, None)
    };
    let mut dates = false;
    let mut spans = Vec::new();
    for i in 0..t.rows.len() {
        let Some(f) = fin(&folds[i]) else { continue };
        let fold = fold_label(f);
        let gid = ids.as_ref().and_then(|v| text_of(&v[i]));
        if let (Some(sv), Some(ev)) = (
            train_s.as_ref().and_then(|v| fin(&v[i])),
            train_e.as_ref().and_then(|v| fin(&v[i])),
        ) {
            dates |= matches!(train_s.as_ref().unwrap()[i], Value::DateTime(_));
            spans.push(Span {
                id: gid.clone(),
                fold: fold.clone(),
                split: "train".into(),
                start: sv,
                end: ev,
            });
        }
        if let (Some(sv), Some(ev)) = (
            test_s.as_ref().and_then(|v| fin(&v[i])),
            test_e.as_ref().and_then(|v| fin(&v[i])),
        ) {
            dates |= matches!(test_s.as_ref().unwrap()[i], Value::DateTime(_));
            spans.push(Span {
                id: gid,
                fold,
                split: "test".into(),
                start: sv,
                end: ev,
            });
        }
    }
    let mut step = f64::INFINITY;
    if has_train && has_test {
        for i in 0..t.rows.len() {
            if let (Some(te), Some(ts)) = (
                train_e.as_ref().and_then(|v| fin(&v[i])),
                test_s.as_ref().and_then(|v| fin(&v[i])),
            ) {
                let d = ts - te;
                if d > 0.0 && d < step {
                    step = d;
                }
            }
        }
    }
    Ok((
        spans,
        if step.is_finite() { step } else { 0.0 },
        dates,
        "train_end".to_string(),
    ))
}

/// The fold windows, their step and whether the x axis is a date axis: the
/// long form when `split` is present, else the span form.
fn spans(t: &Table, options: &Row) -> Result<(Vec<Span>, f64, bool, String), String> {
    if t.has("split") {
        long_form(t, options)
    } else {
        span_form(t)
    }
}

/// `"{n} folds"`, then whether the test windows overlap or leave gaps, then
/// how many series share these folds (when several were collapsed into one).
fn caption(spans: &[Span], step: f64, n_ids_collapsed: usize) -> String {
    let mut folds: Vec<&str> = Vec::new();
    for sp in spans {
        if !folds.contains(&sp.fold.as_str()) {
            folds.push(&sp.fold);
        }
    }
    let n = folds.len();
    let mut cap = if n == 1 {
        "1 fold".to_string()
    } else {
        format!("{n} folds")
    };
    let mut tests: Vec<(f64, f64)> = spans
        .iter()
        .filter(|sp| sp.split == "test")
        .map(|sp| (sp.start, sp.end))
        .collect();
    tests.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if tests.windows(2).any(|w| w[1].0 <= w[0].1) {
        cap.push_str(" · test windows overlap");
    } else if tests.windows(2).any(|w| w[1].0 > w[0].1 + step) {
        cap.push_str(" · gaps between test windows");
    }
    if n_ids_collapsed > 1 {
        cap.push_str(&format!(" · the same folds for {n_ids_collapsed} series"));
    }
    cap
}

/// The sorted (fold, split, start, end) signature of `id`'s spans, so two
/// ids with identical folds compare equal.
fn signature(spans: &[Span], id: &str) -> Vec<String> {
    let mut sig: Vec<String> = spans
        .iter()
        .filter(|sp| sp.id.as_deref() == Some(id))
        .map(|sp| format!("{}|{}|{:.6}|{:.6}", sp.fold, sp.split, sp.start, sp.end))
        .collect();
    sig.sort();
    sig
}

/// The fold/test-window bars as one ggplot-rs layer, so #176 (Gantt) can lift
/// it out later.
fn lane_rects(
    frame: Frame,
    levels: Vec<String>,
    palette: Vec<RGBAColor>,
    o: &RenderOptions,
) -> GGPlot {
    GGPlot::new(frame)
        .aes(
            Aes::new()
                .x("xmin")
                .xmin("xmin")
                .xmax("xmax")
                .ymin("ymin")
                .ymax("ymax")
                .fill("split")
                .label("tip"),
        )
        .geom_rect_with(GeomRect {
            fill: o.brand(),
            color: o.brand(),
            alpha: 0.85,
            line_width: 0.0,
        })
        .scale_fill(
            ScaleColorDiscrete::new(Aesthetic::Fill)
                .with_levels(levels)
                .with_palette(palette),
        )
}

/// `anofox_plot_cv(tbl)`: one lane per fold, the train and test windows as
/// bars along `ds` (or an integer period), so the fold count, where training
/// ends and whether the test windows overlap or leave gaps show at a glance.
pub(super) fn cv(t: &Table, options: &Row, w: u32, h: u32, o: &RenderOptions, place: Place) -> Out {
    let labels = Labels::from(options);
    let (all_spans, step, dates, key) = spans(t, options)?;

    let mut ids: Vec<String> = Vec::new();
    for sp in &all_spans {
        if let Some(id) = &sp.id {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    let (draw_spans, facet, n_collapsed): (Vec<Span>, bool, usize) = if ids.len() <= 1 {
        (all_spans, false, 0)
    } else if ids
        .iter()
        .all(|id| signature(&all_spans, id) == signature(&all_spans, &ids[0]))
    {
        let first = ids[0].clone();
        let only_first: Vec<Span> = all_spans
            .into_iter()
            .filter(|sp| sp.id.as_deref() == Some(first.as_str()))
            .collect();
        (only_first, false, ids.len())
    } else if ids.len() <= 6 {
        (all_spans, true, 0)
    } else {
        return Err(format!(
            "rows hold {} series with different folds: GROUP BY id or WHERE id = …",
            ids.len()
        ));
    };
    if draw_spans.is_empty() {
        return Err("no fold windows (every row is NA on the time key)".to_string());
    }

    let mut fold_labels: Vec<String> = Vec::new();
    for sp in &draw_spans {
        if !fold_labels.contains(&sp.fold) {
            fold_labels.push(sp.fold.clone());
        }
    }
    fold_labels.sort_by(|a, b| {
        let na: f64 = a.trim_start_matches("fold ").trim().parse().unwrap_or(0.0);
        let nb: f64 = b.trim_start_matches("fold ").trim().parse().unwrap_or(0.0);
        na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let n = fold_labels.len();

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
    let mut id_col = Vec::new();
    for sp in &draw_spans {
        let idx = fold_labels.iter().position(|f| f == &sp.fold).unwrap_or(0);
        let slot = stats::slot(idx, n);
        xmin.push(mk(sp.start - step / 2.0));
        xmax.push(mk(sp.end + step / 2.0));
        ymin.push(Value::Float(slot - 0.32));
        ymax.push(Value::Float(slot + 0.32));
        split_col.push(s(sp.split.clone()));
        tip_col.push(s(format!(
            "{} · {}: {} – {}",
            sp.fold,
            sp.split,
            x_text(sp.start, dates),
            x_text(sp.end, dates)
        )));
        if facet {
            id_col.push(s(sp.id.clone().unwrap_or_default()));
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
        frame.push(("id".into(), id_col));
    }

    let mut present: Vec<String> = Vec::new();
    for sp in &draw_spans {
        if !present.contains(&sp.split) {
            present.push(sp.split.clone());
        }
    }
    let mut levels: Vec<String> = Vec::new();
    for want in ["train", "test"] {
        if present.iter().any(|p| p == want) {
            levels.push(want.to_string());
        }
    }
    for p in &present {
        if !levels.contains(p) {
            levels.push(p.clone());
        }
    }
    let palette: Vec<RGBAColor> = levels
        .iter()
        .map(|l| {
            let (r, g, b) = match l.as_str() {
                "train" => o.brand(),
                "test" => ACCENT,
                _ => o.theme.colors.muted,
            };
            RGBAColor::new(r, g, b)
        })
        .collect();

    let mut plot = lane_rects(frame, levels.clone(), palette, o);
    if facet {
        plot = plot.facet_wrap("id", Some(1));
    }
    plot = plot.scale_y_continuous(stats::label_axis(&fold_labels));
    let xdefault = if dates {
        "date".to_string()
    } else {
        look::humanize(&key)
    };
    let plot = labels.apply(look::styled(plot, o, false), &xdefault, "");
    let cap = caption(&draw_spans, step, n_collapsed);
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
    fn three_expanding_folds() {
        // Expanding training windows, contiguous one-day test windows: no
        // overlap, no gap, just "3 folds".
        let r = rows(
            r#"[{"fold_id":1,"split":"train","ds":"2024-01-01"},{"fold_id":1,"split":"train","ds":"2024-01-02"},
                {"fold_id":1,"split":"test","ds":"2024-01-03"},
                {"fold_id":2,"split":"train","ds":"2024-01-01"},{"fold_id":2,"split":"train","ds":"2024-01-02"},
                {"fold_id":2,"split":"train","ds":"2024-01-03"},{"fold_id":2,"split":"test","ds":"2024-01-04"},
                {"fold_id":3,"split":"train","ds":"2024-01-01"},{"fold_id":3,"split":"train","ds":"2024-01-02"},
                {"fold_id":3,"split":"train","ds":"2024-01-03"},{"fold_id":3,"split":"train","ds":"2024-01-04"},
                {"fold_id":3,"split":"test","ds":"2024-01-05"}]"#,
        );
        let t = table(&r);
        let (spans, step, dates, key) = spans(&t, &Row::new()).unwrap();
        assert!(dates);
        assert_eq!(key, "ds");
        assert_eq!(caption(&spans, step, 0), "3 folds");
    }

    #[test]
    fn split_overlap_is_noted() {
        // ts_cv_split_by-like: training ends Feb 1 and Feb 5 give overlapping
        // test windows (Feb 2-8, Feb 6-12).
        let r = rows(
            r#"[{"fold_id":1,"split":"train","ds":"2024-02-01"},{"fold_id":1,"split":"test","ds":"2024-02-02"},
                {"fold_id":1,"split":"test","ds":"2024-02-08"},
                {"fold_id":2,"split":"train","ds":"2024-02-05"},{"fold_id":2,"split":"test","ds":"2024-02-06"},
                {"fold_id":2,"split":"test","ds":"2024-02-12"}]"#,
        );
        let t = table(&r);
        let (spans, step, _, _) = spans(&t, &Row::new()).unwrap();
        assert!(caption(&spans, step, 0).contains("test windows overlap"));
    }

    #[test]
    fn span_form_gap_is_noted() {
        let r = rows(
            r#"[{"fold_id":1,"train_start":"2024-01-01","train_end":"2024-02-01",
                  "test_start":"2024-02-02","test_end":"2024-02-08"},
                {"fold_id":2,"train_start":"2024-01-01","train_end":"2024-02-10",
                  "test_start":"2024-02-11","test_end":"2024-02-17"}]"#,
        );
        let t = table(&r);
        let (spans, step, dates, _) = spans(&t, &Row::new()).unwrap();
        assert!(dates);
        // Second fold's test (Feb 11-17) starts right after the first fold's
        // train ends; the gap check is between TEST windows: Feb 2-8 then
        // Feb 11-17, with a one-day step: a gap.
        assert!(caption(&spans, step, 0).contains("gaps between test windows"));
    }

    #[test]
    fn identical_ids_are_collapsed() {
        let r = rows(
            r#"[{"id":"A","fold_id":1,"split":"train","ds":"2024-01-01"},{"id":"A","fold_id":1,"split":"test","ds":"2024-01-08"},
                {"id":"B","fold_id":1,"split":"train","ds":"2024-01-01"},{"id":"B","fold_id":1,"split":"test","ds":"2024-01-08"}]"#,
        );
        let t = table(&r);
        let (all, step, _, _) = spans(&t, &Row::new()).unwrap();
        let ids: Vec<String> = {
            let mut v = Vec::new();
            for sp in &all {
                if let Some(id) = &sp.id {
                    if !v.contains(id) {
                        v.push(id.clone());
                    }
                }
            }
            v
        };
        assert_eq!(ids.len(), 2);
        assert!(ids
            .iter()
            .all(|id| signature(&all, id) == signature(&all, &ids[0])));
        assert!(caption(&all, step, ids.len()).contains("the same folds for 2 series"));
    }
}

//! `::FRAME` on a bar, line, area or point chart (#663, D-M3-15): one
//! result holds every frame (a week, a year, …) and the chart shows one at a
//! time, like a time map. Static output shows the last frame, named in the
//! subtitle; the browser's frame player (`web/anim.js` `frames()`) steps
//! through them by passing the frame index (`RenderOptions::frame`).
//!
//! Every frame is drawn on the same axes so a step compares: a `geom_blank`
//! layer carries every frame's points (and, for stacked kinds, each
//! frame's per-x stack totals), so ggplot-rs trains the x and y scales over
//! all frames; a discrete x keeps every frame's levels, in first-appearance
//! order. Frame order and the 200-frame cap are the maps' (D11, D15).

use crate::{find_role, value_str, Column, Kind, RenderOptions, Role, Value};

/// What a frame step needs to keep the axes still: every frame's points
/// (drawn by nothing) and a discrete x's levels over every frame.
#[doc(hidden)]
#[derive(Debug)]
pub struct ChartFrameAxes {
    pub(crate) x: Vec<Value>,
    pub(crate) y: Vec<Value>,
    pub(crate) x_levels: Option<Vec<String>>,
}

/// A stacked kind's (frame label, x) …
type StackKey = (String, String);
/// … and its positive and negative totals, with the x value itself.
type StackTotal = (f64, f64, Value);

/// The frame shown and the root attributes the browser's player reads.
pub(crate) struct ChartFrame {
    pub labels: Vec<String>,
    pub k: usize,
    pub name: String,
}

/// Whether `::FRAME` plays on a chart of this kind (else `frame/not-a-map`).
pub(crate) fn kind_plays(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Bar | Kind::BarStacked | Kind::Line | Kind::Area | Kind::AreaStacked | Kind::Point
    )
}

/// The first measure's kind.
fn kind_of(cols: &[Column]) -> Option<Kind> {
    cols.iter().find_map(|c| match c.role {
        Role::Value(k) => Some(k),
        _ => None,
    })
}

/// Frame `o.frame` (else the last) of a framed chart: its rows only, the
/// options carrying every frame's axes, and the frame context. `None` for
/// anything else (no `::FRAME`, a map, a kind that doesn't play).
pub(crate) fn prepare(
    cols: &[Column],
    o: &RenderOptions,
) -> Option<(Vec<Column>, RenderOptions, ChartFrame)> {
    let fcol = find_role(cols, Role::Frame)?;
    if crate::map::is_map(cols) || !kind_of(cols).is_some_and(kind_plays) {
        return None;
    }
    let x = find_role(cols, Role::X)?;
    let (labels, _) = crate::map::frame_order(fcol)?;
    let k = o.frame.unwrap_or(labels.len() - 1).min(labels.len() - 1);
    let n = fcol.values.len();
    let label_of = |i: usize| fcol.values.get(i).map(value_str).unwrap_or_default();
    let kept: Vec<usize> = (0..n).filter(|&i| label_of(i) == labels[k]).collect();
    // rows of a frame past the cap are dropped from the axes too
    let in_frames: Vec<usize> = (0..n).filter(|&i| labels.contains(&label_of(i))).collect();

    let view: Vec<Column> = cols
        .iter()
        .filter(|c| c.role != Role::Frame)
        .map(|c| {
            if c.values.len() == n {
                let values = kept.iter().map(|&i| c.values[i].clone()).collect();
                Column::new(c.name.clone(), c.role, values)
            } else {
                c.clone()
            }
        })
        .collect();

    // every frame's points, on the measure columns
    let discrete = x.values.iter().any(|v| matches!(v, Value::Str(_)));
    let x_at = |i: usize| -> Value {
        let v = x.values.get(i).cloned().unwrap_or(Value::Na);
        if discrete {
            Value::Str(value_str(&v))
        } else {
            v
        }
    };
    let (mut bx, mut by) = (Vec::new(), Vec::new());
    let measures: Vec<&Column> = cols
        .iter()
        .filter(|c| matches!(c.role, Role::Value(_)) && c.values.len() == n)
        .collect();
    for m in &measures {
        for &i in &in_frames {
            if let Some(v) = m.values.get(i).filter(|v| crate::cell_num(v).is_some()) {
                bx.push(x_at(i));
                by.push(v.clone());
            }
        }
    }
    // stacked kinds: each frame's per-x positive and negative totals
    if matches!(kind_of(cols), Some(Kind::BarStacked | Kind::AreaStacked))
        || (matches!(kind_of(cols), Some(Kind::Bar)) && stacks_by_default(cols))
    {
        let mut tot: Vec<(StackKey, StackTotal)> = Vec::new();
        if let Some(m) = measures.first() {
            for &i in &in_frames {
                let Some(v) = m.values.get(i).and_then(crate::cell_num) else {
                    continue;
                };
                let key = (label_of(i), value_str(&x_at(i)));
                let at = match tot.iter().position(|(k, _)| *k == key) {
                    Some(p) => p,
                    None => {
                        tot.push((key, (0.0, 0.0, x_at(i))));
                        tot.len() - 1
                    }
                };
                let t = &mut tot[at].1;
                if v >= 0.0 {
                    t.0 += v;
                } else {
                    t.1 += v;
                }
            }
        }
        for (_, (pos, neg, xv)) in tot {
            bx.push(xv.clone());
            by.push(Value::Float(pos));
            bx.push(xv);
            by.push(Value::Float(neg));
        }
    }
    let (bx, by) = extremes(bx, by);
    let x_levels = discrete.then(|| {
        let mut levels: Vec<String> = Vec::new();
        for &i in &in_frames {
            let s = value_str(&x_at(i));
            if !s.is_empty() && !levels.contains(&s) {
                levels.push(s);
            }
        }
        levels
    });
    let mut o = o.clone();
    o.frame_axes = Some(std::sync::Arc::new(ChartFrameAxes {
        x: bx,
        y: by,
        x_levels,
    }));
    let frame = ChartFrame {
        labels,
        k,
        name: fcol.name.clone(),
    };
    Some((view, o, frame))
}

/// The few points that span every frame's points: the lowest and highest
/// x and y (ggplot-rs trains the same ranges from these as from all of
/// them, without a layer as large as every frame together).
fn extremes(x: Vec<Value>, y: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let num = |v: &Value| v.as_f64().filter(|f| f.is_finite());
    let pick = |vals: &[Value], better: fn(f64, f64) -> bool| -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, v) in vals.iter().enumerate() {
            if let Some(f) = num(v) {
                if best.is_none_or(|(_, b)| better(f, b)) {
                    best = Some((i, f));
                }
            }
        }
        best.map(|(i, _)| i)
    };
    let mut keep: Vec<usize> = [
        pick(&x, |a, b| a < b),
        pick(&x, |a, b| a > b),
        pick(&y, |a, b| a < b),
        pick(&y, |a, b| a > b),
    ]
    .into_iter()
    .flatten()
    .collect();
    keep.sort_unstable();
    keep.dedup();
    (
        keep.iter().map(|&i| x[i].clone()).collect(),
        keep.iter().map(|&i| y[i].clone()).collect(),
    )
}

/// A plain `::BARCHART` with a `::CATEGORY` that cartesian stacks rather
/// than dodges (a continuous x).
fn stacks_by_default(cols: &[Column]) -> bool {
    find_role(cols, Role::Category).is_some()
        && find_role(cols, Role::X)
            .is_some_and(|x| !x.values.iter().any(|v| matches!(v, Value::Str(_))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<Value> {
        v.iter().map(|s| Value::Str(s.to_string())).collect()
    }

    #[test]
    fn keeps_the_frame_and_every_frames_axes() {
        let cols = vec![
            Column::new("r", Role::X, strs(&["N", "S", "N", "E"])),
            Column::new("w", Role::Frame, strs(&["2", "2", "1", "1"])),
            Column::new(
                "v",
                Role::Value(Kind::Bar),
                vec![
                    Value::Float(5.0),
                    Value::Float(9.0),
                    Value::Float(1.0),
                    Value::Float(20.0),
                ],
            ),
        ];
        let o = RenderOptions::default();
        let (view, o2, f) = prepare(&cols, &o).unwrap();
        assert_eq!(f.labels, ["1", "2"]); // numeric frames sort (D11)
        assert_eq!(f.k, 1); // the last by default
        assert_eq!(view.len(), 2); // the ::FRAME column is dropped
        assert_eq!(view[0].values, strs(&["N", "S"]));
        let ax = o2.frame_axes.as_ref().unwrap();
        assert_eq!(ax.x_levels.as_deref().unwrap(), ["N", "S", "E"]);
        assert!(ax.y.contains(&Value::Float(20.0)));
        // frame 0
        let o = RenderOptions {
            frame: Some(0),
            ..RenderOptions::default()
        };
        let (view, _, f) = prepare(&cols, &o).unwrap();
        assert_eq!(f.k, 0);
        assert_eq!(view[0].values, strs(&["N", "E"]));
    }

    #[test]
    fn stacked_bars_reserve_each_frames_totals() {
        let cols = vec![
            Column::new("r", Role::X, strs(&["N", "N", "N", "N"])),
            Column::new("c", Role::Category, strs(&["a", "b", "a", "b"])),
            Column::new("w", Role::Frame, strs(&["1", "1", "2", "2"])),
            Column::new(
                "v",
                Role::Value(Kind::BarStacked),
                vec![
                    Value::Float(3.0),
                    Value::Float(4.0),
                    Value::Float(10.0),
                    Value::Float(5.0),
                ],
            ),
        ];
        let (_, o2, _) = prepare(&cols, &RenderOptions::default()).unwrap();
        assert!(o2.frame_axes.unwrap().y.contains(&Value::Float(15.0)));
    }

    #[test]
    fn only_bar_line_area_point_charts_play() {
        let cols = vec![
            Column::new("r", Role::X, strs(&["N"])),
            Column::new("w", Role::Frame, strs(&["1"])),
            Column::new("v", Role::Value(Kind::Pie), vec![Value::Float(1.0)]),
        ];
        assert!(prepare(&cols, &RenderOptions::default()).is_none());
    }
}

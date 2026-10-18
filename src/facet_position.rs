//! Position adjustments per `::FACET` panel.
//!
//! ggplot-rs (0.19) applies a layer's position adjustment to the layer's
//! pooled rows and splits them into facet panels afterwards, and
//! `PositionStack` / `PositionFill` key their running sums by x alone: with a
//! `::FACET`, a panel's stacks then start where the previous panel's ended
//! (#208). [`PerFacet`] runs the adjustment on each panel's rows on their own.
//! TODO(ggplot-rs): drop once positions are computed per panel.

use ggplot_rs::data::{DataFrame, Value};
use ggplot_rs::position::{Position, PositionParams};

/// The layer column a `::FACET` travels in (see `render_cartesian`).
const FACET_COL: &str = "facet";

/// Runs `P` once per facet panel (all rows at once without a facet).
pub(crate) struct PerFacet<P>(pub(crate) P);

impl<P: Position> Position for PerFacet<P> {
    fn compute(&self, data: &mut DataFrame, params: &PositionParams) {
        let Some(facet) = data.column(FACET_COL) else {
            return self.0.compute(data, params);
        };
        // Row indices per panel, in first-seen order.
        let mut panels: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, v) in facet.iter().enumerate() {
            let key = v.key_str();
            match panels.iter_mut().find(|(k, _)| *k == key) {
                Some((_, rows)) => rows.push(i),
                None => panels.push((key.into_owned(), vec![i])),
            }
        }
        if panels.len() < 2 {
            return self.0.compute(data, params);
        }
        let n = data.nrows();
        let names: Vec<String> = data.column_names().iter().map(|s| s.to_string()).collect();
        for (_, rows) in &panels {
            let mut part = DataFrame::new();
            for name in &names {
                let col = data.column(name).unwrap_or_default();
                part.add_column(name.clone(), rows.iter().map(|&i| col[i].clone()).collect());
            }
            self.0.compute(&mut part, params);
            // Write the panel's rows back, adding the columns the adjustment
            // created (`ymin`, the raw y) on first sight.
            for name in part.column_names() {
                if !data.has_column(name) {
                    data.add_column(name.to_string(), vec![Value::Na; n]);
                }
                let (src, dst) = (part.column(name).unwrap_or_default(), data.column_mut(name));
                if let Some(dst) = dst {
                    for (&i, v) in rows.iter().zip(src) {
                        dst[i] = v.clone();
                    }
                }
            }
        }
    }

    fn name(&self) -> &str {
        self.0.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggplot_rs::prelude::{PositionFill, PositionStack};

    fn frame(facet: bool) -> DataFrame {
        let s = |v: &[&str]| v.iter().map(|s| Value::Str(s.to_string())).collect();
        let mut d = DataFrame::new();
        d.add_column("x".into(), s(&["D1", "D1", "D1", "D1"]));
        d.add_column("y".into(), [3.0, 5.0, 4.0, 1.0].map(Value::Float).to_vec());
        d.add_column("cat".into(), s(&["a", "b", "a", "b"]));
        if facet {
            d.add_column(FACET_COL.into(), s(&["P", "P", "Q", "Q"]));
        }
        d
    }
    fn nums(d: &DataFrame, c: &str) -> Vec<f64> {
        d.column(c)
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect()
    }

    #[test]
    fn stacks_each_facet_panel_from_zero() {
        let mut d = frame(true);
        PerFacet(PositionStack).compute(&mut d, &PositionParams::default());
        // First group on top, per panel: P = [5, 8] over [0, 5], Q = [1, 5] over [0, 1].
        assert_eq!(nums(&d, "y"), [8.0, 5.0, 5.0, 1.0]);
        assert_eq!(nums(&d, "ymin"), [5.0, 0.0, 1.0, 0.0]);
        assert_eq!(
            nums(&d, ggplot_rs::position::RAW_Y_COL),
            [3.0, 5.0, 4.0, 1.0]
        );
        assert_eq!(d.nrows(), 4);
        assert!(d.issues().is_empty(), "{:?}", d.issues());
    }

    #[test]
    fn fills_each_facet_panel_to_one() {
        let mut d = frame(true);
        PerFacet(PositionFill).compute(&mut d, &PositionParams::default());
        let close = |a: Vec<f64>, b: [f64; 4]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9);
        assert!(
            close(nums(&d, "y"), [1.0, 0.625, 1.0, 0.2]),
            "{:?}",
            nums(&d, "y")
        );
        assert!(
            close(nums(&d, "ymin"), [0.625, 0.0, 0.2, 0.0]),
            "{:?}",
            nums(&d, "ymin")
        );
    }

    #[test]
    fn without_a_facet_it_is_the_plain_position() {
        let (mut a, mut b) = (frame(false), frame(false));
        PerFacet(PositionStack).compute(&mut a, &PositionParams::default());
        PositionStack.compute(&mut b, &PositionParams::default());
        assert_eq!(nums(&a, "y"), nums(&b, "y"));
        assert_eq!(nums(&a, "ymin"), nums(&b, "ymin"));
        assert_eq!(PerFacet(PositionStack).name(), "stack");
    }
}

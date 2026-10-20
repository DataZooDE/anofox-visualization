//! Feature labels: where a region's name goes (its pole of inaccessibility,
//! so a C-shaped region keeps its label inside) and which labels fit
//! (greedy by priority, no overlaps, none wider than its region).

use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A screen-space point.
pub(crate) type Pt = (f64, f64);

/// The pole of inaccessibility of a polygon (`rings[0]` the exterior, the
/// rest holes): the inside point farthest from the outline, to `precision`
/// pixels (Mapbox's polylabel). Returns the point and its distance.
pub(crate) fn polylabel(rings: &[Vec<Pt>], precision: f64) -> Option<(Pt, f64)> {
    let outer = rings.first().filter(|r| r.len() >= 3)?;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in outer {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let size = w.min(h);
    if size.is_nan() || size <= 0.0 {
        return Some(((x0, y0), 0.0));
    }
    let mut heap = BinaryHeap::new();
    let mut half = size / 2.0;
    let mut x = x0;
    while x < x1 {
        let mut y = y0;
        while y < y1 {
            heap.push(Cell::new(x + half, y + half, half, rings));
            y += size;
        }
        x += size;
    }
    // the centroid of the outline is a good first guess
    let mut best = Cell::new(centroid(outer).0, centroid(outer).1, 0.0, rings);
    let bbox = Cell::new(x0 + w / 2.0, y0 + h / 2.0, 0.0, rings);
    if bbox.d > best.d {
        best = bbox;
    }
    let mut steps = 0;
    while let Some(c) = heap.pop() {
        let (cx, cy, ch, cmax) = (c.x, c.y, c.h, c.max);
        if c.d > best.d {
            best = c;
        }
        if cmax - best.d <= precision {
            continue;
        }
        steps += 1;
        if steps > 10_000 {
            break;
        }
        half = ch / 2.0;
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            heap.push(Cell::new(cx + dx * half, cy + dy * half, half, rings));
        }
    }
    Some(((best.x, best.y), best.d))
}

struct Cell {
    x: f64,
    y: f64,
    h: f64,
    /// Signed distance to the outline (negative outside).
    d: f64,
    /// The best distance any point in the cell could have.
    max: f64,
}

impl Cell {
    fn new(x: f64, y: f64, h: f64, rings: &[Vec<Pt>]) -> Cell {
        let d = signed_dist((x, y), rings);
        Cell {
            x,
            y,
            h,
            d,
            max: d + h * std::f64::consts::SQRT_2,
        }
    }
}

impl PartialEq for Cell {
    fn eq(&self, o: &Self) -> bool {
        self.max == o.max
    }
}
impl Eq for Cell {}
impl PartialOrd for Cell {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cell {
    fn cmp(&self, o: &Self) -> Ordering {
        self.max.total_cmp(&o.max)
    }
}

/// Distance from `p` to the polygon outline, negative outside.
fn signed_dist(p: Pt, rings: &[Vec<Pt>]) -> f64 {
    let mut inside = false;
    let mut best = f64::INFINITY;
    for r in rings {
        let n = r.len();
        if n < 2 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (r[i], r[j]);
            if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
                inside = !inside;
            }
            best = best.min(seg_dist2(p, a, b));
            j = i;
        }
    }
    let d = best.sqrt();
    if inside {
        d
    } else {
        -d
    }
}

fn seg_dist2(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ey) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    ex * ex + ey * ey
}

fn centroid(r: &[Pt]) -> Pt {
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    let n = r.len();
    let mut j = n - 1;
    for i in 0..n {
        let (p, q) = (r[i], r[j]);
        let f = p.0 * q.1 - q.0 * p.1;
        cx += (p.0 + q.0) * f;
        cy += (p.1 + q.1) * f;
        a += f * 3.0;
        j = i;
    }
    if a.abs() < 1e-12 {
        return r[0];
    }
    (cx / a, cy / a)
}

/// The screen area of a ring (shoelace, unsigned).
pub(crate) fn area(r: &[Pt]) -> f64 {
    let n = r.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    let mut j = n - 1;
    for i in 0..n {
        s += (r[j].0 + r[i].0) * (r[j].1 - r[i].1);
        j = i;
    }
    (s / 2.0).abs()
}

/// A label that may be placed.
pub(crate) struct Candidate {
    /// The anchor: a region's pole, or a point mark.
    pub at: Pt,
    pub text: String,
    /// Higher goes first.
    pub priority: f64,
    /// A region's width on screen (a label wider than it is dropped);
    /// `None` for a point (labelled to its right).
    pub room: Option<f64>,
    /// How far right of the anchor a point-style label starts (a plain
    /// point: 5 px; a graduated symbol, M2a: its radius + 3 px). Unused
    /// when `room` is `Some` (an area's label is centred on its anchor).
    pub dx: f64,
}

/// A placed label: its anchor, text and whether it starts at the anchor
/// (a point's label) or is centred on it.
pub(crate) struct Placed {
    pub x: f64,
    pub y: f64,
    pub text: String,
    pub start: bool,
}

/// Place labels greedily by priority: each one that doesn't overlap a
/// label already placed, a `blocked` box (the legend) or the panel edge,
/// and fits its region. With a `limit` only the `limit` most important are
/// tried (`::LABEL_TOP n` names the top n, never the next ones instead).
pub(crate) fn place(
    mut cands: Vec<Candidate>,
    limit: Option<usize>,
    blocked: &[(f64, f64, f64, f64)],
    (w, h): (f64, f64),
    size: f64,
) -> Vec<Placed> {
    cands.sort_by(|a, b| b.priority.total_cmp(&a.priority));
    if let Some(n) = limit {
        cands.truncate(n);
    }
    let mut taken: Vec<(f64, f64, f64, f64)> = blocked.to_vec();
    let mut out = Vec::new();
    for c in cands {
        let tw = ggplot_rs::render::text::text_width(&c.text, size);
        let th = size * 1.15;
        if c.room.is_some_and(|r| tw > r) {
            continue;
        }
        let start = c.room.is_none();
        let x0 = if start {
            c.at.0 + c.dx
        } else {
            c.at.0 - tw / 2.0
        };
        let b = (
            x0 - 1.0,
            c.at.1 - th / 2.0,
            x0 + tw + 1.0,
            c.at.1 + th / 2.0,
        );
        if b.0 < 0.0 || b.1 < 0.0 || b.2 > w || b.3 > h {
            continue;
        }
        if taken
            .iter()
            .any(|t| b.0 < t.2 && t.0 < b.2 && b.1 < t.3 && t.1 < b.3)
        {
            continue;
        }
        taken.push(b);
        out.push(Placed {
            x: if start { x0 } else { c.at.0 },
            y: c.at.1,
            text: c.text,
            start,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pole_of_a_c_shape_is_inside_it() {
        // a C open to the right: the centroid falls in the gap
        let c = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 2.0),
            (2.0, 2.0),
            (2.0, 8.0),
            (10.0, 8.0),
            (10.0, 10.0),
            (0.0, 10.0),
        ];
        let ((x, y), d) = polylabel(std::slice::from_ref(&c), 0.1).unwrap();
        assert!(
            signed_dist((x, y), std::slice::from_ref(&c)) > 0.0,
            "({x},{y}) inside"
        );
        assert!(x < 2.0 && d > 0.8, "({x},{y}) in the spine, d = {d}");
        let cen = centroid(&c);
        assert!(signed_dist(cen, &[c]) < 0.0, "the centroid is outside");
        // a square's pole is its centre
        let sq = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let ((x, y), _) = polylabel(&[sq], 0.01).unwrap();
        assert!((x - 2.0).abs() < 0.05 && (y - 2.0).abs() < 0.05);
    }

    #[test]
    fn labels_avoid_each_other_and_the_legend() {
        let c = |x: f64, p: f64, t: &str| Candidate {
            at: (x, 50.0),
            text: t.into(),
            priority: p,
            room: Some(80.0),
            dx: 0.0,
        };
        let placed = place(
            vec![
                c(50.0, 1.0, "Low"),
                c(55.0, 9.0, "High"),
                c(150.0, 5.0, "Far"),
                c(250.0, 3.0, "Blocked"),
            ],
            None,
            &[(230.0, 40.0, 300.0, 60.0)],
            (400.0, 100.0),
            9.0,
        );
        let t: Vec<&str> = placed.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, vec!["High", "Far"]);
        // a label wider than its region is dropped; a limit stops early
        let wide = Candidate {
            at: (100.0, 50.0),
            text: "A very long region name".into(),
            priority: 1.0,
            room: Some(20.0),
            dx: 0.0,
        };
        assert!(place(vec![wide], None, &[], (400.0, 100.0), 9.0).is_empty());
        let many = (0..5)
            .map(|i| c(20.0 + i as f64 * 60.0, i as f64, "Xy"))
            .collect();
        let top: Vec<String> = place(many, Some(2), &[], (400.0, 100.0), 9.0)
            .into_iter()
            .map(|p| p.text)
            .collect();
        assert_eq!(top.len(), 2);
        // the top 2 only: when one of them doesn't fit, no lower one stands in
        let mut few: Vec<Candidate> = (0..3)
            .map(|i| c(40.0 + i as f64 * 100.0, i as f64, "Xy"))
            .collect();
        few[2].room = Some(1.0);
        assert_eq!(place(few, Some(2), &[], (400.0, 100.0), 9.0).len(), 1);
    }
}

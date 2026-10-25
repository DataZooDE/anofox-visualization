//! A region's pole of inaccessibility (so a C-shaped region keeps its own
//! anchor inside it, [`polylabel`]) and a ring's screen area ([`area`]) —
//! shared by `symbols.rs` (a polygon's graduated-symbol anchor) and
//! `plot.rs` (`::LABEL_TOP`/`::DATALABELS` priority, `geom_sf_text` draws
//! the labels themselves now — ggplot-rs's own public port of this
//! module's old greedy placer, sipemu/ggplot-rs#195, dropped here in #420
//! slice 6).

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
}

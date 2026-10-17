//! Screen-space simplification and compact SVG path data.
//!
//! Coordinates are rounded to tenths of a pixel (kept as integers so the
//! relative steps add up exactly), consecutive duplicates dropped, then
//! Douglas-Peucker removes every vertex closer than the tolerance to the
//! simplified line. Paths are written as one absolute move per ring and
//! relative line steps (`M12.3 4l1.2-3 .5 2z`).

/// A screen point in tenths of a pixel.
pub(crate) type Tenths = (i64, i64);

/// Round a screen point to tenths of a pixel.
pub(crate) fn tenths((x, y): (f64, f64)) -> Tenths {
    ((x * 10.0).round() as i64, (y * 10.0).round() as i64)
}

/// Simplify a screen-space path to within `tol` pixels (Douglas-Peucker on
/// the tenth-pixel grid). The first and last points always stay.
pub(crate) fn simplify(pts: &[(f64, f64)], tol: f64) -> Vec<Tenths> {
    let mut q: Vec<Tenths> = Vec::with_capacity(pts.len());
    for p in pts {
        let t = tenths(*p);
        if q.last() != Some(&t) {
            q.push(t);
        }
    }
    if q.len() < 3 {
        return q;
    }
    let tol = tol * 10.0;
    let mut keep = vec![false; q.len()];
    let last = q.len() - 1;
    keep[0] = true;
    keep[last] = true;
    let mut stack = vec![(0usize, last)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut far, mut dmax) = (a, -1.0);
        for (i, p) in q.iter().enumerate().take(b).skip(a + 1) {
            let d = seg_dist(*p, q[a], q[b]);
            if d > dmax {
                dmax = d;
                far = i;
            }
        }
        if dmax > tol {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    q.into_iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(p))
        .collect()
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Tenths, a: Tenths, b: Tenths) -> f64 {
    let (px, py) = (p.0 as f64, p.1 as f64);
    let (ax, ay) = (a.0 as f64, a.1 as f64);
    let (dx, dy) = (b.0 as f64 - ax, b.1 as f64 - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ey) = (ax + t * dx - px, ay + t * dy - py);
    (ex * ex + ey * ey).sqrt()
}

/// A tenths value as the shortest SVG number: `12.3`, `12`, `.5`, `-.5`.
pub(crate) fn num(t: i64) -> String {
    let (neg, a) = (t < 0, t.unsigned_abs());
    let (i, f) = (a / 10, a % 10);
    let mut s = String::with_capacity(8);
    if neg && a != 0 {
        s.push('-');
    }
    if i != 0 || f == 0 {
        s.push_str(&i.to_string());
    }
    if f != 0 {
        s.push('.');
        s.push((b'0' + f as u8) as char);
    }
    s
}

/// Append `n` to path data, separated from the previous number only when
/// needed (a leading `-` separates by itself).
fn push_num(d: &mut String, n: i64) {
    let s = num(n);
    if !s.starts_with('-') && d.ends_with(|c: char| c.is_ascii_digit() || c == '.') {
        d.push(' ');
    }
    d.push_str(&s);
}

/// Append one ring or line to path data: `M` to its first point, relative
/// `l` steps after, `z` when `close`.
pub(crate) fn push_path(d: &mut String, pts: &[Tenths], close: bool) {
    let Some(&(x0, y0)) = pts.first() else {
        return;
    };
    d.push('M');
    push_num(d, x0);
    push_num(d, y0);
    let n = if close && pts.len() > 1 && pts[pts.len() - 1] == pts[0] {
        pts.len() - 1
    } else {
        pts.len()
    };
    if n > 1 {
        d.push('l');
        let mut prev = (x0, y0);
        for &p in &pts[1..n] {
            push_num(d, p.0 - prev.0);
            push_num(d, p.1 - prev.1);
            prev = p;
        }
    }
    if close {
        d.push('z');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(123), "12.3");
        assert_eq!(num(120), "12");
        assert_eq!(num(5), ".5");
        assert_eq!(num(-5), "-.5");
        assert_eq!(num(0), "0");
        assert_eq!(num(-30), "-3");
    }

    #[test]
    fn path_data_is_relative_and_closed() {
        let mut d = String::new();
        push_path(&mut d, &[(0, 0), (100, 0), (100, 55), (0, 0)], true);
        assert_eq!(d, "M0 0l10 0 0 5.5z");
        let mut d = String::new();
        push_path(&mut d, &[(10, -5), (5, 10)], false);
        assert_eq!(d, "M1-.5l-.5 1.5");
    }

    #[test]
    fn simplify_drops_points_on_a_line() {
        // a straight run with a 0.2 px wiggle collapses; a 3 px bump stays
        let line: Vec<(f64, f64)> = (0..=20)
            .map(|i| (i as f64, if i % 2 == 0 { 0.0 } else { 0.2 }))
            .collect();
        assert_eq!(simplify(&line, 0.5), vec![(0, 0), (200, 0)]);
        let bump = [(0.0, 0.0), (5.0, 3.0), (10.0, 0.0)];
        assert_eq!(simplify(&bump, 0.5).len(), 3);
        // duplicates on the tenth-pixel grid merge
        assert_eq!(
            simplify(&[(0.0, 0.0), (0.01, 0.02), (1.0, 1.0)], 0.5).len(),
            2
        );
    }
}

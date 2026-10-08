//! A small, dependency-free penalized-spline (P-spline) smoother — the
//! `::SMOOTH_METHOD 'gam'` trend line.
//!
//! ggplot-rs 0.16 only offers `method = "gam"` behind its `regression` feature,
//! which pulls ~200 crates (faer, anofox-regression) and does not build for
//! `wasm32-unknown-unknown`. A one-dimensional GAM smooth is just a cubic
//! B-spline basis with a second-order difference penalty (Eilers & Marx), λ
//! chosen by generalised cross-validation — a few dense k×k solves, so it is
//! computed here and drawn as a line layer.
//!
//! TODO(ggplot-rs 0.17): switch to the upstream `SmoothMethod::Gam` (and the GLM
//! families) once the regression-backed smoothers build for wasm without the
//! heavy dependency tree.

/// Fit a P-spline to `(x, y)` and evaluate it on `n_out` evenly spaced x
/// values. `None` when there are fewer than 4 distinct finite x values.
pub(crate) fn pspline(points: &[(f64, f64)], n_out: usize) -> Option<Vec<(f64, f64)>> {
    let pts: Vec<(f64, f64)> = points
        .iter()
        .copied()
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .collect();
    let (lo, hi) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| {
        (a.0.min(p.0), a.1.max(p.0))
    });
    let mut distinct: Vec<f64> = pts.iter().map(|p| p.0).collect();
    distinct.sort_by(f64::total_cmp);
    distinct.dedup();
    if distinct.len() < 4 || hi <= lo {
        return None;
    }
    // Interior segments: enough flexibility without over-fitting tiny inputs.
    let nseg = (distinct.len() / 3).clamp(3, 20);
    let k = nseg + 3; // cubic B-spline basis functions
    let basis = |x: f64| bspline_row(x, lo, hi, nseg);
    let rows: Vec<Vec<f64>> = pts.iter().map(|p| basis(p.0)).collect();
    // Normal equations B'B, B'y and the second-order difference penalty D'D.
    let mut btb = vec![vec![0.0; k]; k];
    let mut bty = vec![0.0; k];
    for (r, p) in rows.iter().zip(&pts) {
        for i in 0..k {
            if r[i] == 0.0 {
                continue;
            }
            bty[i] += r[i] * p.1;
            for j in 0..k {
                btb[i][j] += r[i] * r[j];
            }
        }
    }
    let mut dtd = vec![vec![0.0; k]; k];
    for i in 0..k - 2 {
        let d = [(i, 1.0), (i + 1, -2.0), (i + 2, 1.0)];
        for &(a, va) in &d {
            for &(b, vb) in &d {
                dtd[a][b] += va * vb;
            }
        }
    }
    let n = pts.len() as f64;
    let rss = |coef: &[f64]| -> f64 {
        rows.iter()
            .zip(&pts)
            .map(|(r, p)| {
                let f: f64 = r.iter().zip(coef).map(|(a, b)| a * b).sum();
                (p.1 - f).powi(2)
            })
            .sum()
    };
    let mut best: Option<(f64, Vec<f64>)> = None;
    for step in 0..=24 {
        let lambda = 10f64.powf(-3.0 + step as f64 * 0.375); // 1e-3 … 1e6
        let a: Vec<Vec<f64>> = (0..k)
            .map(|i| (0..k).map(|j| btb[i][j] + lambda * dtd[i][j]).collect())
            .collect();
        let Some(coef) = solve(&a, &bty) else {
            continue;
        };
        // Effective degrees of freedom: tr((B'B + λP)⁻¹ B'B).
        let mut edf = 0.0;
        for j in 0..k {
            let col: Vec<f64> = (0..k).map(|i| btb[i][j]).collect();
            if let Some(s) = solve(&a, &col) {
                edf += s[j];
            }
        }
        let denom = n - edf;
        if denom <= 0.5 {
            continue;
        }
        let gcv = n * rss(&coef) / (denom * denom);
        if gcv.is_finite() && best.as_ref().is_none_or(|b| gcv < b.0) {
            best = Some((gcv, coef));
        }
    }
    let (_, coef) = best?;
    let n_out = n_out.max(2);
    Some(
        (0..n_out)
            .map(|i| {
                let x = lo + (hi - lo) * i as f64 / (n_out - 1) as f64;
                let y = basis(x).iter().zip(&coef).map(|(a, b)| a * b).sum();
                (x, y)
            })
            .collect(),
    )
}

/// The `nseg + 3` cubic B-spline basis values at `x` on equally spaced knots
/// over `[lo, hi]` (Cox–de Boor recursion).
fn bspline_row(x: f64, lo: f64, hi: f64, nseg: usize) -> Vec<f64> {
    let deg = 3;
    let dx = (hi - lo) / nseg as f64;
    let k = nseg + deg;
    // Knots t_j = lo + (j - deg)·dx, j = 0 … nseg + 2·deg.
    let knot = |j: usize| lo + (j as f64 - deg as f64) * dx;
    // Clamp x into the domain so the right end belongs to the last segment.
    let x = x.clamp(lo, hi - 1e-12 * (hi - lo).max(1.0));
    let nk = nseg + 2 * deg + 1;
    let mut b: Vec<f64> = (0..nk - 1)
        .map(|j| {
            if knot(j) <= x && x < knot(j + 1) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    for d in 1..=deg {
        for j in 0..nk - 1 - d {
            let (t0, t1, t2) = (knot(j), knot(j + d), knot(j + d + 1));
            let left = (x - t0) / (t1 - t0) * b[j];
            let right = (knot(j + d + 1) - x) / (t2 - knot(j + 1)) * b[j + 1];
            b[j] = left + right;
        }
    }
    b.truncate(k);
    b
}

/// Solve `a · x = b` by Gaussian elimination with partial pivoting.
fn solve(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a
        .iter()
        .zip(b)
        .map(|(row, &v)| {
            let mut r = row.clone();
            r.push(v);
            r
        })
        .collect();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs()))?;
        if m[p][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, p);
        let pivot = m[c].clone();
        for row in m.iter_mut().skip(c + 1) {
            let f = row[c] / pivot[c];
            if f != 0.0 {
                for (a, b) in row.iter_mut().zip(&pivot).skip(c) {
                    *a -= f * b;
                }
            }
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|j| m[r][j] * x[j]).sum();
        x[r] = (m[r][n] - s) / m[r][r];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basis_is_a_partition_of_unity() {
        for x in [0.0, 0.3, 1.7, 4.99, 5.0] {
            let s: f64 = bspline_row(x, 0.0, 5.0, 5).iter().sum();
            assert!((s - 1.0).abs() < 1e-9, "x={x}: {s}");
        }
    }

    #[test]
    fn recovers_a_smooth_curve() {
        let pts: Vec<(f64, f64)> = (0..200)
            .map(|i| {
                let x = i as f64 / 20.0;
                // deterministic "noise"
                let e = ((i * 7919) % 13) as f64 / 13.0 - 0.5;
                (x, x.sin() + 0.1 * e)
            })
            .collect();
        let fit = pspline(&pts, 50).unwrap();
        assert_eq!(fit.len(), 50);
        let err = fit
            .iter()
            .map(|(x, y)| (y - x.sin()).abs())
            .fold(0.0, f64::max);
        assert!(err < 0.15, "max error {err}");
    }

    #[test]
    fn too_few_points() {
        assert!(pspline(&[(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)], 10).is_none());
        assert!(pspline(&[], 10).is_none());
    }
}

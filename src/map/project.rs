//! Map projections: lon/lat degrees → projected metres (y up).
//!
//! Spherical Equal Earth (Šavrič et al. 2018), spherical Web Mercator,
//! spherical Albers equal-area conic (a fixed Europe set-up and one fitted to
//! the data's extent), ellipsoidal transverse Mercator (UTM, Krüger series as
//! in Karney 2011) and the identity for planar data. The tests check each
//! against PROJ (`ST_Transform` in DuckDB spatial) to the centimetre.
//!
//! [`Projection::Auto`] picks one from the data's extent, see [`choose`].

use std::f64::consts::FRAC_PI_4;

/// The authalic Earth radius PROJ uses for `+R` spheres (metres).
const R: f64 = 6_371_008.8;
/// The Web Mercator sphere (EPSG:3857).
const R_MERC: f64 = 6_378_137.0;
/// Web Mercator's latitude cut-off.
const MERC_LAT: f64 = 85.051_128_779_806_59;

/// A `::PROJECTION` as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Projection {
    Auto,
    EqualEarth,
    Mercator,
    AlbersEurope,
    Conic,
    Utm,
    None,
}

impl Projection {
    /// Every accepted name (for messages and the lint).
    pub(crate) const NAMES: &'static [&'static str] = &[
        "auto",
        "equal-earth",
        "mercator",
        "albers-europe",
        "conic",
        "utm",
        "none",
    ];

    /// Parse a `::PROJECTION` value (case and `_`/space-insensitive).
    pub(crate) fn parse(s: &str) -> Option<Projection> {
        let k = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Some(match k.as_str() {
            "" | "auto" => Projection::Auto,
            "equal-earth" | "equalearth" => Projection::EqualEarth,
            "mercator" | "web-mercator" => Projection::Mercator,
            "albers-europe" => Projection::AlbersEurope,
            "conic" | "albers" => Projection::Conic,
            "utm" => Projection::Utm,
            "none" | "planar" | "identity" => Projection::None,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Projection::Auto => "auto",
            Projection::EqualEarth => "equal-earth",
            Projection::Mercator => "mercator",
            Projection::AlbersEurope => "albers-europe",
            Projection::Conic => "conic",
            Projection::Utm => "utm",
            Projection::None => "none",
        }
    }

    /// How a summary names it ("equal-area conic").
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Projection::Auto => "automatic projection",
            Projection::EqualEarth => "Equal Earth",
            Projection::Mercator => "Mercator",
            Projection::AlbersEurope => "Albers equal-area, Europe",
            Projection::Conic => "equal-area conic",
            Projection::Utm => "UTM",
            Projection::None => "planar coordinates",
        }
    }
}

/// The lon/lat (or planar) extent of the data: `(x0, y0, x1, y1)` plus
/// whether a vertex sits exactly on the origin.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Extent {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub origin: bool,
}

impl Extent {
    pub(crate) fn new() -> Extent {
        Extent {
            x0: f64::INFINITY,
            y0: f64::INFINITY,
            x1: f64::NEG_INFINITY,
            y1: f64::NEG_INFINITY,
            origin: false,
        }
    }

    pub(crate) fn add(&mut self, [x, y]: [f64; 2]) {
        if !(x.is_finite() && y.is_finite()) {
            return;
        }
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
        self.origin |= x == 0.0 && y == 0.0;
    }

    pub(crate) fn is_empty(&self) -> bool {
        !self.x0.is_finite()
    }
}

/// The `'auto'` rule, in order:
/// 1. a coordinate outside lon ±180 / lat ±90, or a vertex exactly at
///    (0, 0) — the origin of a planar grid (on Earth: open sea) → `'none'`;
/// 2. a longitude span over 120° → `'equal-earth'`;
/// 3. a span of at most 6° → `'utm'`;
/// 4. otherwise `'conic'`.
pub(crate) fn choose(e: &Extent) -> Projection {
    if e.is_empty() {
        return Projection::None;
    }
    let geographic = e.x0 >= -180.0 && e.x1 <= 180.0 && e.y0 >= -90.0 && e.y1 <= 90.0;
    if !geographic || e.origin {
        return Projection::None;
    }
    let span = e.x1 - e.x0;
    if span > 120.0 {
        Projection::EqualEarth
    } else if span <= 6.0 {
        Projection::Utm
    } else {
        Projection::Conic
    }
}

/// A projection set up for one map (its centre, standard parallels, zone).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Proj {
    Identity,
    EqualEarth { lon0: f64 },
    Mercator,
    Albers(Albers),
    Utm { lon0: f64 },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Albers {
    lon0: f64,
    n: f64,
    c: f64,
    rho0: f64,
}

impl Albers {
    /// Spherical Albers equal-area with standard parallels `lat1`, `lat2`
    /// and origin (`lon0`, `lat0`), degrees.
    pub(crate) fn new(lon0: f64, lat0: f64, lat1: f64, lat2: f64) -> Option<Albers> {
        let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
        let n = (p1.sin() + p2.sin()) / 2.0;
        if n.abs() < 1e-3 {
            return None;
        }
        let c = p1.cos().powi(2) + 2.0 * n * p1.sin();
        let rho0 = R * (c - 2.0 * n * lat0.to_radians().sin()).max(0.0).sqrt() / n;
        Some(Albers {
            lon0: lon0.to_radians(),
            n,
            c,
            rho0,
        })
    }

    fn forward(&self, lon: f64, lat: f64) -> [f64; 2] {
        let rho = R
            * (self.c - 2.0 * self.n * lat.to_radians().sin())
                .max(0.0)
                .sqrt()
            / self.n;
        let theta = self.n * (lon.to_radians() - self.lon0);
        [rho * theta.sin(), self.rho0 - rho * theta.cos()]
    }
}

impl Proj {
    /// Set `p` up for data with extent `e` (`'auto'` resolved first).
    /// Returns the resolved projection with it.
    pub(crate) fn setup(p: Projection, e: &Extent) -> (Projection, Proj) {
        let p = if p == Projection::Auto { choose(e) } else { p };
        let (cx, _) = if e.is_empty() {
            (0.0, 0.0)
        } else {
            (
                ((e.x0 + e.x1) / 2.0).clamp(-180.0, 180.0),
                ((e.y0 + e.y1) / 2.0).clamp(-90.0, 90.0),
            )
        };
        let proj = match p {
            Projection::Auto | Projection::None => Proj::Identity,
            Projection::EqualEarth => Proj::EqualEarth {
                lon0: if e.x1 - e.x0 > 120.0 { 0.0 } else { cx },
            },
            Projection::Mercator => Proj::Mercator,
            Projection::AlbersEurope => {
                Proj::Albers(Albers::new(10.0, 0.0, 43.0, 62.0).expect("fixed parallels"))
            }
            Projection::Conic => {
                let (y0, y1) = (e.y0.max(-89.0), e.y1.min(89.0));
                let span = (y1 - y0).max(0.0);
                Albers::new(cx, 0.0, y0 + span / 6.0, y0 + span * 5.0 / 6.0)
                    .map(Proj::Albers)
                    // parallels around the equator: the cone flattens out
                    .unwrap_or(Proj::EqualEarth { lon0: cx })
            }
            Projection::Utm => Proj::Utm {
                lon0: utm_central_meridian(cx),
            },
        };
        (p, proj)
    }

    /// Whether straight lon/lat edges bend under this projection (and so are
    /// densified before projecting).
    pub(crate) fn curves(&self) -> bool {
        !matches!(self, Proj::Identity | Proj::Mercator)
    }

    /// Project one coordinate (`None` when it has no image, e.g. a
    /// non-finite input).
    pub(crate) fn forward(&self, [lon, lat]: [f64; 2]) -> Option<[f64; 2]> {
        if !(lon.is_finite() && lat.is_finite()) {
            return None;
        }
        let out = match self {
            Proj::Identity => [lon, lat],
            Proj::EqualEarth { lon0 } => equal_earth(lon - lon0, lat.clamp(-90.0, 90.0)),
            Proj::Mercator => {
                let lat = lat.clamp(-MERC_LAT, MERC_LAT).to_radians();
                [
                    R_MERC * lon.to_radians(),
                    R_MERC * (FRAC_PI_4 + lat / 2.0).tan().ln(),
                ]
            }
            Proj::Albers(a) => a.forward(lon, lat.clamp(-90.0, 90.0)),
            Proj::Utm { lon0 } => utm(lon, lat.clamp(-89.9, 89.9), *lon0),
        };
        (out[0].is_finite() && out[1].is_finite()).then_some(out)
    }

    /// The lon/lat whose image is `p` — Newton's method from `start` with a
    /// numeric Jacobian (no closed-form inverse needed). `None` when it
    /// doesn't converge (a point outside the projection's image).
    pub(crate) fn inverse_near(&self, p: [f64; 2], start: [f64; 2]) -> Option<[f64; 2]> {
        let mut ll = start;
        for _ in 0..30 {
            let f = self.forward(ll)?;
            let (ex, ey) = (f[0] - p[0], f[1] - p[1]);
            let h = 1e-4;
            let fx = self.forward([ll[0] + h, ll[1]])?;
            let fy = self.forward([ll[0], ll[1] + h])?;
            let (a, c) = ((fx[0] - f[0]) / h, (fx[1] - f[1]) / h);
            let (b, d) = ((fy[0] - f[0]) / h, (fy[1] - f[1]) / h);
            let det = a * d - b * c;
            if det.abs() < 1e-18 {
                return None;
            }
            let dlon = (d * ex - b * ey) / det;
            let dlat = (-c * ex + a * ey) / det;
            ll = [ll[0] - dlon, (ll[1] - dlat).clamp(-89.9, 89.9)];
            if dlon.abs() < 1e-9 && dlat.abs() < 1e-9 {
                return Some(ll);
            }
        }
        let f = self.forward(ll)?;
        let scale = p[0].abs().max(p[1].abs()).max(1.0);
        ((f[0] - p[0]).abs() + (f[1] - p[1]).abs() < 1e-6 * scale).then_some(ll)
    }

    /// True north at `ll`, in degrees clockwise from straight up on the map
    /// (projected y grows up): 0 for a north-up projection.
    pub(crate) fn north_deg(&self, ll: [f64; 2]) -> f64 {
        let lat2 = (ll[1] + 0.01).min(89.99);
        match (self.forward(ll), self.forward([ll[0], lat2])) {
            (Some(a), Some(b)) => {
                let deg = (b[0] - a[0]).atan2(b[1] - a[1]).to_degrees();
                if deg.abs() < 1e-9 {
                    0.0
                } else {
                    deg
                }
            }
            _ => 0.0,
        }
    }
}

/// Equal Earth on the `R` sphere, `lon` relative to the central meridian.
fn equal_earth(lon: f64, lat: f64) -> [f64; 2] {
    const A1: f64 = 1.340264;
    const A2: f64 = -0.081106;
    const A3: f64 = 0.000893;
    const A4: f64 = 0.003796;
    let m = 3f64.sqrt() / 2.0;
    let (l, p) = (lon.to_radians(), lat.to_radians());
    let t = (m * p.sin()).asin();
    let (t2, t6) = (t * t, t.powi(6));
    let x = l * t.cos() / (m * (A1 + 3.0 * A2 * t2 + t6 * (7.0 * A3 + 9.0 * A4 * t2)));
    let y = t * (A1 + A2 * t2 + t6 * (A3 + A4 * t2));
    [R * x, R * y]
}

/// The central meridian of the UTM zone holding `lon`.
fn utm_central_meridian(lon: f64) -> f64 {
    let zone = (((lon + 180.0) / 6.0).floor() as i32).clamp(0, 59);
    zone as f64 * 6.0 - 180.0 + 3.0
}

/// Transverse Mercator on WGS84 (Krüger series to n⁶), UTM scale and
/// false easting; northing from the equator (no southern false northing).
fn utm(lon: f64, lat: f64, lon0: f64) -> [f64; 2] {
    const A: f64 = 6_378_137.0;
    const F: f64 = 1.0 / 298.257_223_563;
    const K0: f64 = 0.9996;
    let n = F / (2.0 - F);
    let (n2, n3) = (n * n, n * n * n);
    let (n4, n5, n6) = (n2 * n2, n2 * n3, n3 * n3);
    let a_r = A / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n6 / 256.0);
    let alpha = [
        n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0
            + 7891.0 * n6 / 37800.0,
        13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0
            - 1983433.0 * n6 / 1935360.0,
        61.0 * n3 / 240.0 - 103.0 * n4 / 140.0 + 15061.0 * n5 / 26880.0 + 167603.0 * n6 / 181440.0,
        49561.0 * n4 / 161280.0 - 179.0 * n5 / 168.0 + 6601661.0 * n6 / 7257600.0,
        34729.0 * n5 / 80640.0 - 3418889.0 * n6 / 1995840.0,
        212378941.0 * n6 / 319334400.0,
    ];
    let p = lat.to_radians();
    let dl = (lon - lon0).to_radians();
    let e = 2.0 * n.sqrt() / (1.0 + n);
    let t = (p.sin().atanh() - e * (e * p.sin()).atanh()).sinh();
    let xi1 = t.atan2(dl.cos());
    let eta1 = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
    let (mut xi, mut eta) = (xi1, eta1);
    for (j, a) in alpha.iter().enumerate() {
        let k = 2.0 * (j + 1) as f64;
        xi += a * (k * xi1).sin() * (k * eta1).cosh();
        eta += a * (k * xi1).cos() * (k * eta1).sinh();
    }
    [500_000.0 + K0 * a_r * eta, K0 * a_r * xi]
}

/// Densify a lon/lat path so no step is longer than `step` degrees (a
/// straight edge then follows the projection's curve).
pub(crate) fn densify(pts: &[[f64; 2]], step: f64) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            let q = pts[i - 1];
            let d = (p[0] - q[0]).abs().max((p[1] - q[1]).abs());
            if d > step && d.is_finite() {
                let k = (d / step).ceil().min(360.0) as usize;
                for s in 1..k {
                    let t = s as f64 / k as f64;
                    out.push([q[0] + (p[0] - q[0]) * t, q[1] + (p[1] - q[1]) * t]);
                }
            }
        }
        out.push(*p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2], tol: f64) {
        assert!(
            (a[0] - b[0]).abs() < tol && (a[1] - b[1]).abs() < tol,
            "{a:?} vs {b:?}"
        );
    }

    fn ext(x0: f64, y0: f64, x1: f64, y1: f64) -> Extent {
        let mut e = Extent::new();
        e.add([x0, y0]);
        e.add([x1, y1]);
        e
    }

    // #646: true north for the north arrow.
    #[test]
    fn north_follows_the_meridians() {
        let de = ext(5.9, 47.3, 15.0, 55.1);
        let (_, conic) = Proj::setup(Projection::Conic, &de);
        let cm = (5.9 + 15.0) / 2.0;
        assert!(conic.north_deg([cm, 51.0]).abs() < 0.01);
        // east of the central meridian the meridians lean west (to the left)
        let east = conic.north_deg([15.0, 51.0]);
        assert!(east < -1.0 && east > -10.0, "{east}");
        assert!(conic.north_deg([6.0, 51.0]) > 1.0);
        for p in [Projection::Mercator, Projection::None] {
            let (_, pr) = Proj::setup(p, &de);
            assert_eq!(pr.north_deg([14.0, 51.0]), 0.0, "{p:?}");
        }
    }

    #[test]
    fn inverse_near_round_trips() {
        let de = ext(5.9, 47.3, 15.0, 55.1);
        for p in [
            Projection::Conic,
            Projection::Mercator,
            Projection::Utm,
            Projection::EqualEarth,
        ] {
            let (_, pr) = Proj::setup(p, &de);
            for ll in [[13.4, 52.5], [6.1, 47.5], [10.0, 51.0]] {
                let xy = pr.forward(ll).unwrap();
                let back = pr.inverse_near(xy, [10.45, 51.2]).unwrap();
                close(back, ll, 1e-6);
            }
        }
    }

    // Reference values: DuckDB spatial (PROJ 9) ST_Transform from
    // EPSG:4326 with always_xy, e.g.
    //   ST_Transform(ST_Point(13.4, 52.5), 'EPSG:4326', '+proj=eqearth +R=6371008.8', always_xy := true)
    #[test]
    fn equal_earth_matches_proj() {
        let p = Proj::EqualEarth { lon0: 0.0 };
        close(
            p.forward([13.4, 52.5]).unwrap(),
            [1_036_974.059_041_676_3, 6_245_911.951_883_592],
            0.01,
        );
        close(
            p.forward([-120.0, -33.0]).unwrap(),
            [-10_599_506.921_602_746, -4_132_993.094_803_968_4],
            0.01,
        );
    }

    #[test]
    fn mercator_matches_proj() {
        let p = Proj::Mercator;
        close(
            p.forward([13.4, 52.5]).unwrap(),
            [1_491_681.176_629_865_8, 6_891_041.723_891_586],
            0.01,
        );
        // clamped at the Web Mercator cut-off, not infinite
        assert!(p.forward([0.0, 90.0]).unwrap()[1] < 2.01e7);
    }

    #[test]
    fn albers_europe_matches_proj() {
        let (_, p) = Proj::setup(Projection::AlbersEurope, &ext(0.0, 40.0, 20.0, 60.0));
        // +proj=aea +lat_1=43 +lat_2=62 +lat_0=0 +lon_0=10 +R=6371008.8
        close(
            p.forward([13.4, 52.5]).unwrap(),
            [226_949.661_171_036_07, 5_421_888.063_620_956],
            0.01,
        );
        close(
            p.forward([-9.1, 38.7]).unwrap(),
            [-1_658_326.311_163_812_6, 4_093_362.143_505_720_4],
            0.01,
        );
    }

    fn p_far() -> [f64; 2] {
        let (_, p) = Proj::setup(Projection::Utm, &ext(16.0, 53.0, 17.0, 54.0));
        p.forward([16.9, 54.0]).unwrap()
    }

    #[test]
    fn utm_matches_proj() {
        // EPSG:32633 (zone 33N, central meridian 15°)
        let (name, p) = Proj::setup(Projection::Utm, &ext(13.0, 52.0, 14.0, 53.0));
        assert_eq!(name, Projection::Utm);
        close(
            p.forward([13.4, 52.5]).unwrap(),
            [391_390.731_339_952_04, 5_817_855.240_817_33],
            0.01,
        );
        // EPSG:32632 (zone 32N): Munich
        let (_, p) = Proj::setup(Projection::Utm, &ext(11.0, 48.0, 12.0, 48.5));
        close(
            p.forward([11.575, 48.137]).unwrap(),
            [691_567.326_384_146_5, 5_334_734.330_481_76],
            0.01,
        );
        // 1.9° off the central meridian
        close(p_far(), [624_537.093_362_598, 5_985_192.463_540_45], 0.01);
    }

    #[test]
    fn auto_picks_by_extent() {
        assert_eq!(
            choose(&ext(-170.0, -50.0, 170.0, 80.0)),
            Projection::EqualEarth
        );
        assert_eq!(choose(&ext(5.9, 47.3, 15.0, 55.1)), Projection::Conic);
        assert_eq!(choose(&ext(13.0, 52.3, 13.8, 52.7)), Projection::Utm);
        // planar km beyond lon/lat, or a grid around the origin
        assert_eq!(choose(&ext(-500.0, 0.0, 500.0, 300.0)), Projection::None);
        let mut e = ext(-17.0, -11.0, 16.0, 14.0);
        e.add([0.0, 0.0]);
        assert_eq!(choose(&e), Projection::None);
        assert_eq!(choose(&Extent::new()), Projection::None);
    }

    #[test]
    fn parse_names() {
        for n in Projection::NAMES {
            assert_eq!(Projection::parse(n).unwrap().name(), *n);
        }
        assert_eq!(
            Projection::parse("Equal Earth"),
            Some(Projection::EqualEarth)
        );
        assert_eq!(Projection::parse("robinson"), None);
    }

    #[test]
    fn densify_splits_long_edges() {
        let d = densify(&[[0.0, 0.0], [10.0, 0.0], [10.5, 0.0]], 1.0);
        assert_eq!(d.len(), 12);
        assert_eq!(d[5], [5.0, 0.0]);
    }
}

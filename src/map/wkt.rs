//! Serialize a row's already-projected [`Shape`]s back to WKT text, so
//! `geom_sf` (which only takes a `geometry` column of WKT strings, re-
//! parsed via `ggplot_rs::spatial::parse_wkt`) can draw them.
//!
//! Slice 1 of #420 keeps our own projection pipeline (`project.rs`,
//! `Model::new`) unchanged — `Model::features`' coordinates are already in
//! projected (planar) units by the time this runs, not raw lon/lat.
//! `GeomSf::default()` (its `projection: SfProjection::PlateCarree`) is an
//! identity pass-through (`project([x, y]) == [x, y]`), so feeding it
//! these projected coordinates as "the raw geometry" and leaving its own
//! projection untouched draws them unprojected a second time, i.e. not at
//! all — the architecture `overlay.rs` already established for flows/hexbin
//! (see its own module doc). `project.rs` itself only goes away in slice 6.

// #420 WIP: not wired into `build()` yet (next slice) — nothing constructs
// a `geom_sf` DataFrame outside this module's own tests.
#![allow(dead_code)]

use super::Shape;

/// A ring's points as WKT `"x y, x y, …"`, closed (first point repeated at
/// the end) if it isn't already — `parse_wkt` doesn't require a ring to
/// close itself, but writing it closed keeps the output valid WKT by any
/// stricter reader's standard too.
fn ring_wkt(ring: &[[f64; 2]]) -> String {
    let mut pts: Vec<[f64; 2]> = ring.to_vec();
    if pts.first() != pts.last() {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    pts.iter()
        .map(|[x, y]| format!("{x} {y}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One `Shape::Area`'s rings as `"(ring0), (ring1), …"` (exterior, then
/// holes) — the inside of a `POLYGON (...)` or one part of a
/// `MULTIPOLYGON (...)`.
fn area_wkt(rings: &[Vec<[f64; 2]>]) -> String {
    rings
        .iter()
        .map(|r| format!("({})", ring_wkt(r)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// This shape's WKT, geometry-type prefix included (`POLYGON (...)`,
/// `LINESTRING (...)`, `POINT (...)`.
fn shape_wkt(s: &Shape) -> String {
    match s {
        Shape::Area(rings) => format!("POLYGON ({})", area_wkt(rings)),
        Shape::Line(pts) => format!(
            "LINESTRING ({})",
            pts.iter()
                .map(|[x, y]| format!("{x} {y}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Shape::Point([x, y]) => format!("POINT ({x} {y})"),
    }
}

/// A row's shapes as one WKT geometry (`None` for an empty row — `geom_sf`
/// skips a row whose `geometry` cell doesn't parse, same as a missing
/// value): a single shape as itself; several of the *same* kind as the
/// matching `MULTI*`; a mix (rare — a messy input `GEOMETRYCOLLECTION`)
/// as `GEOMETRYCOLLECTION (...)`.
pub(super) fn row_wkt(shapes: &[Shape]) -> Option<String> {
    match shapes {
        [] => None,
        [one] => Some(shape_wkt(one)),
        many => {
            let same_kind = |a: &Shape, b: &Shape| {
                matches!(
                    (a, b),
                    (Shape::Area(_), Shape::Area(_))
                        | (Shape::Line(_), Shape::Line(_))
                        | (Shape::Point(_), Shape::Point(_))
                )
            };
            if many.windows(2).all(|w| same_kind(&w[0], &w[1])) {
                match &many[0] {
                    Shape::Area(_) => Some(format!(
                        "MULTIPOLYGON ({})",
                        many.iter()
                            .map(|s| match s {
                                Shape::Area(rings) => format!("({})", area_wkt(rings)),
                                _ => unreachable!(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    Shape::Line(_) => Some(format!(
                        "MULTILINESTRING ({})",
                        many.iter()
                            .map(|s| match s {
                                Shape::Line(pts) => format!(
                                    "({})",
                                    pts.iter()
                                        .map(|[x, y]| format!("{x} {y}"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                                _ => unreachable!(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    Shape::Point(_) => Some(format!(
                        "MULTIPOINT ({})",
                        many.iter()
                            .map(|s| match s {
                                Shape::Point([x, y]) => format!("{x} {y}"),
                                _ => unreachable!(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            } else {
                Some(format!(
                    "GEOMETRYCOLLECTION ({})",
                    many.iter().map(shape_wkt).collect::<Vec<_>>().join(", ")
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggplot_rs::spatial::{parse_wkt, Geometry};

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]
    }

    #[test]
    fn empty_row_has_no_wkt() {
        assert_eq!(row_wkt(&[]), None);
    }

    #[test]
    fn a_single_point_round_trips() {
        let wkt = row_wkt(&[Shape::Point([1.5, 2.5])]).unwrap();
        assert_eq!(parse_wkt(&wkt), Some(Geometry::Point([1.5, 2.5])));
    }

    #[test]
    fn a_single_polygon_with_a_hole_round_trips() {
        let outer = square(0.0, 0.0, 10.0, 10.0);
        let hole = square(3.0, 3.0, 7.0, 7.0);
        let wkt = row_wkt(&[Shape::Area(vec![outer.clone(), hole.clone()])]).unwrap();
        assert_eq!(parse_wkt(&wkt), Some(Geometry::Polygon(vec![outer, hole])));
    }

    #[test]
    fn a_line_round_trips() {
        let pts = vec![[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]];
        let wkt = row_wkt(&[Shape::Line(pts.clone())]).unwrap();
        assert_eq!(parse_wkt(&wkt), Some(Geometry::LineString(pts)));
    }

    #[test]
    fn several_polygons_become_a_multipolygon() {
        let a = square(0.0, 0.0, 1.0, 1.0);
        let b = square(10.0, 10.0, 11.0, 11.0);
        let wkt = row_wkt(&[Shape::Area(vec![a.clone()]), Shape::Area(vec![b.clone()])]).unwrap();
        assert_eq!(
            parse_wkt(&wkt),
            Some(Geometry::MultiPolygon(vec![vec![a], vec![b]]))
        );
    }

    #[test]
    fn several_points_become_a_multipoint() {
        let wkt = row_wkt(&[Shape::Point([1.0, 2.0]), Shape::Point([3.0, 4.0])]).unwrap();
        assert_eq!(
            parse_wkt(&wkt),
            Some(Geometry::MultiPoint(vec![[1.0, 2.0], [3.0, 4.0]]))
        );
    }

    #[test]
    fn mixed_shapes_become_a_geometry_collection() {
        let wkt = row_wkt(&[
            Shape::Point([1.0, 2.0]),
            Shape::Line(vec![[0.0, 0.0], [1.0, 1.0]]),
        ])
        .unwrap();
        match parse_wkt(&wkt) {
            Some(Geometry::Collection(parts)) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], Geometry::Point([1.0, 2.0]));
                assert_eq!(parts[1], Geometry::LineString(vec![[0.0, 0.0], [1.0, 1.0]]));
            }
            other => panic!("expected a collection, got {other:?}"),
        }
    }

    #[test]
    fn a_ring_not_already_closed_is_closed_on_write() {
        // `ring_wkt` always repeats the first point; confirm the open
        // input (3 points) and the closed WKT (4) both parse to the
        // same polygon.
        let open = vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]];
        let wkt = row_wkt(&[Shape::Area(vec![open.clone()])]).unwrap();
        let mut closed = open.clone();
        closed.push(open[0]);
        assert_eq!(parse_wkt(&wkt), Some(Geometry::Polygon(vec![closed])));
    }
}

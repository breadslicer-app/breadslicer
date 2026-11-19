//! SoulCrafted (#138, #99): solid rafts of other shapes, each made from the
//! supports' own circles, never from the picture of a slice.
//!
//! - **Pads**: a pad under each support (or each group of supports whose pads
//!   touch) instead of one raft under them all: less to peel off the film.
//! - **Perforated**: holes through the raft between the supports, where no
//!   support stands: less area to peel, and the resin drains through.
//! - **Peel tabs**: tabs off the raft's two ends, to lever it off the plate.

use super::polygon_set_2d::{
    PolygonWithHoles, difference_polygon_sets, ring_to_polygon, union_polygon_sets,
};
use super::small::compute_footprint;
use crate::clipper::ClipperError;
use crate::supports::rafts::crenelated::raft_types::SupportBaseCircle;
use crate::three::Vector2;

/// How far a hole keeps from a support's base circle, and from the raft's
/// edge, as a share of the hole's diameter.
const HOLE_CLEARANCE_SHARE: f64 = 0.5;
/// Holes stand this many diameters apart, centre to centre.
const HOLE_PITCH: f64 = 2.0;
/// Sides of a hole and of a pad.
const RING_SIDES: usize = 24;
/// A peel tab's length out of the raft and its width, mm.
pub const PEEL_TAB_LENGTH_MM: f64 = 5.0;
pub const PEEL_TAB_WIDTH_MM: f64 = 4.0;

fn circle_ring(x: f64, y: f64, r: f64) -> Vec<Vector2> {
    (0..RING_SIDES)
        .map(|i| {
            let a = i as f64 / RING_SIDES as f64 * std::f64::consts::TAU;
            Vector2::new(x + r * a.cos(), y + r * a.sin())
        })
        .collect()
}

/// A pad under each circle, grown by `margin_mm`; pads that touch join.
pub fn pads(
    circles: &[SupportBaseCircle],
    margin_mm: f64,
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let pads: Vec<PolygonWithHoles> = circles
        .iter()
        .map(|c| {
            compute_footprint(
                std::slice::from_ref(c),
                Some(margin_mm),
                Some(RING_SIDES as f64),
            )
        })
        .filter(|ring| ring.len() >= 3)
        .map(ring_to_polygon)
        .collect();
    union_polygon_sets(&pads)
}

fn inside(ring: &[Vector2], x: f64, y: f64) -> bool {
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + n - 1) % n]);
        if (a.y > y) != (b.y > y) && x < (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

fn distance_to_ring(ring: &[Vector2], x: f64, y: f64) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 {
                (((x - a.x) * dx + (y - a.y) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ((a.x + t * dx - x).powi(2) + (a.y + t * dy - y).powi(2)).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

/// Where the holes of `footprint` go: a square lattice `diameter_mm` ×
/// [`HOLE_PITCH`] apart, each clear of every circle, of the edge (by
/// `edge_mm` more) and of the footprint's own holes.
pub fn hole_centres(
    footprint: &[PolygonWithHoles],
    circles: &[SupportBaseCircle],
    diameter_mm: f64,
    edge_mm: f64,
) -> Vec<(f64, f64)> {
    if diameter_mm <= 0.0 {
        return Vec::new();
    }
    let r = diameter_mm / 2.0;
    let clear = r + diameter_mm * HOLE_CLEARANCE_SHARE;
    let pitch = diameter_mm * HOLE_PITCH;
    let mut out = Vec::new();
    for poly in footprint {
        let (mut lo, mut hi) = (
            Vector2::new(f64::INFINITY, f64::INFINITY),
            Vector2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        for p in &poly.outer {
            lo = Vector2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vector2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let mut y = lo.y + pitch / 2.0;
        while y < hi.y {
            let mut x = lo.x + pitch / 2.0;
            while x < hi.x {
                let ok = inside(&poly.outer, x, y)
                    && distance_to_ring(&poly.outer, x, y) >= clear + edge_mm
                    && poly
                        .holes
                        .iter()
                        .all(|h| !inside(h, x, y) && distance_to_ring(h, x, y) >= clear)
                    && circles
                        .iter()
                        .all(|c| ((c.x - x).powi(2) + (c.y - y).powi(2)).sqrt() >= c.r + clear);
                if ok {
                    out.push((x, y));
                }
                x += pitch;
            }
            y += pitch;
        }
    }
    out
}

/// `footprint` with holes `diameter_mm` wide between the circles (see
/// [`hole_centres`]).
pub fn perforated(
    footprint: &[PolygonWithHoles],
    circles: &[SupportBaseCircle],
    diameter_mm: f64,
    edge_mm: f64,
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let holes: Vec<PolygonWithHoles> = hole_centres(footprint, circles, diameter_mm, edge_mm)
        .into_iter()
        .map(|(x, y)| ring_to_polygon(circle_ring(x, y, diameter_mm / 2.0)))
        .collect();
    if holes.is_empty() {
        return Ok(footprint.to_vec());
    }
    difference_polygon_sets(footprint, &holes)
}

/// `footprint` with a tab off each end of its longer side, where the raft
/// reaches furthest that way: on a raft wider at one end than the other, the
/// middle of its box is off the raft, and a tab there stood on its own.
pub fn with_peel_tabs(
    footprint: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let points: Vec<Vector2> = footprint
        .iter()
        .flat_map(|p| p.outer.iter().copied())
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .collect();
    if points.is_empty() {
        return Ok(footprint.to_vec());
    }
    let (mut lo, mut hi) = (
        Vector2::new(f64::INFINITY, f64::INFINITY),
        Vector2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for p in &points {
        lo = Vector2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vector2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let (w, l) = (PEEL_TAB_WIDTH_MM / 2.0, PEEL_TAB_LENGTH_MM);
    let along_x = hi.x - lo.x >= hi.y - lo.y;
    // Where the outline reaches `edge` along the long side (within half a
    // millimetre): the middle of that stretch across it.
    let across_at = |edge: f64| {
        let (mut a, mut b) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in &points {
            let (along, across) = if along_x { (p.x, p.y) } else { (p.y, p.x) };
            if (along - edge).abs() <= 0.5 {
                a = a.min(across);
                b = b.max(across);
            }
        }
        (a + b) / 2.0
    };
    let (from, to) = if along_x { (lo.x, hi.x) } else { (lo.y, hi.y) };
    // Into the raft a tab's length too, so it joins it however round it is.
    let tabs: Vec<PolygonWithHoles> = [
        (from - l, from + l, across_at(from)),
        (to - l, to + l, across_at(to)),
    ]
    .into_iter()
    .map(|(a, b, c)| {
        let ring = if along_x {
            vec![
                Vector2::new(a, c - w),
                Vector2::new(b, c - w),
                Vector2::new(b, c + w),
                Vector2::new(a, c + w),
            ]
        } else {
            vec![
                Vector2::new(c - w, a),
                Vector2::new(c + w, a),
                Vector2::new(c + w, b),
                Vector2::new(c - w, b),
            ]
        };
        ring_to_polygon(ring)
    })
    .collect();
    let mut all = footprint.to_vec();
    all.extend(tabs);
    union_polygon_sets(&all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circles() -> Vec<SupportBaseCircle> {
        vec![
            SupportBaseCircle {
                x: 0.0,
                y: 0.0,
                r: 1.0,
            },
            SupportBaseCircle {
                x: 20.0,
                y: 0.0,
                r: 1.0,
            },
            SupportBaseCircle {
                x: 0.0,
                y: 20.0,
                r: 1.0,
            },
            SupportBaseCircle {
                x: 20.0,
                y: 20.0,
                r: 1.0,
            },
        ]
    }

    fn area(polys: &[PolygonWithHoles]) -> f64 {
        super::super::polygon_set_2d::polygon_set_area_mm2(polys)
    }

    #[test]
    fn pads_stand_apart_under_supports_far_from_each_other() {
        let pads = pads(&circles(), 1.0).unwrap();
        assert_eq!(pads.len(), 4, "one pad each");
        let one = std::f64::consts::PI * 4.0;
        assert!(
            (area(&pads) - 4.0 * one).abs() < one * 0.1,
            "{}",
            area(&pads)
        );
    }

    #[test]
    fn holes_go_between_the_supports_and_keep_clear_of_them() {
        let hull = vec![ring_to_polygon(compute_footprint(
            &circles(),
            Some(1.0),
            Some(24.0),
        ))];
        let centres = hole_centres(&hull, &circles(), 2.0, 1.0);
        assert!(!centres.is_empty());
        for (x, y) in &centres {
            for c in circles() {
                assert!(((c.x - x).powi(2) + (c.y - y).powi(2)).sqrt() >= c.r + 2.0);
            }
        }
        let holed = perforated(&hull, &circles(), 2.0, 1.0).unwrap();
        let lost = area(&hull) - area(&holed);
        let expected = centres.len() as f64 * std::f64::consts::PI;
        assert!(
            (lost - expected).abs() < expected * 0.1,
            "{lost} {expected}"
        );
    }

    #[test]
    fn peel_tabs_stick_out_of_the_long_ends() {
        let wide = vec![
            SupportBaseCircle {
                x: 0.0,
                y: 0.0,
                r: 1.0,
            },
            SupportBaseCircle {
                x: 30.0,
                y: 0.0,
                r: 1.0,
            },
        ];
        let hull = vec![ring_to_polygon(compute_footprint(
            &wide,
            Some(1.0),
            Some(24.0),
        ))];
        let tabbed = with_peel_tabs(&hull).unwrap();
        let reach = |p: &[PolygonWithHoles]| {
            p.iter()
                .flat_map(|q| q.outer.iter())
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                    (lo.min(v.x), hi.max(v.x))
                })
        };
        let (a, b) = (reach(&hull), reach(&tabbed));
        assert!(b.0 < a.0 - 4.0 && b.1 > a.1 + 4.0, "{a:?} {b:?}");
        assert_eq!(tabbed.len(), 1, "one raft with its tabs");
    }

    #[test]
    fn peel_tabs_join_a_raft_wider_at_one_end() {
        // A funnel: wide at the back (y 40), a point at the front (y 0). The
        // middle of its box (y 20) is off the raft at both ends.
        let funnel = vec![ring_to_polygon(vec![
            Vector2::new(50.0, 0.0),
            Vector2::new(100.0, 40.0),
            Vector2::new(0.0, 40.0),
        ])];
        let tabbed = with_peel_tabs(&funnel).unwrap();
        assert_eq!(tabbed.len(), 1, "the tabs stand on the raft: {tabbed:?}");
        let reach = tabbed
            .iter()
            .flat_map(|q| q.outer.iter())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v.x), hi.max(v.x))
            });
        assert!(reach.0 < -4.0 && reach.1 > 104.0, "{reach:?}");
    }
}

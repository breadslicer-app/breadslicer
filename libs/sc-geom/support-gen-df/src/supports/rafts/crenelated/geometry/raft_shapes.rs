//! SoulCrafted (#138, #99): solid rafts of other shapes, each made from the
//! supports' own circles, never from the picture of a slice.
//!
//! - **Pads**: a pad under each support (or each group of supports whose pads
//!   touch) instead of one raft under them all: less to peel off the film.
//! - **Perforated**: holes through the raft between the supports, where no
//!   support stands: less area to peel, and the resin drains through.
//! - **Peel tabs**: tabs out of the raft's sharp corners, where peeling starts.

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

/// How far along the outline, each way, a corner's turn is read (mm): a
/// raft's corners are rounded, so one point alone turns little.
const CORNER_WINDOW_MM: f64 = 5.0;
/// The least turn, over that window, of a corner that takes a tab: about
/// 125 degrees inside or sharper.
const CORNER_MIN_TURN_DEG: f64 = 55.0;
/// Tabs stand at least this far apart along the outline (mm), and there are
/// at most so many.
const CORNER_SPACING_MM: f64 = 15.0;
const MAX_PEEL_TABS: usize = 4;

/// `footprint` with peel tabs at its sharp corners, pointing out of them:
/// peeling starts where a corner lets the film in. A raft with no sharp
/// corner (a round one) gets a tab off each end of its longer side.
pub fn with_peel_tabs(
    footprint: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let points: Vec<Vector2> = footprint
        .iter()
        .flat_map(|p| p.outer.iter().copied())
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .collect();
    if points.len() < 3 {
        return Ok(footprint.to_vec());
    }
    let hull = super::convex_hull_2d::convex_hull_2d(&points);
    let mut tips = sharp_corners(&hull);
    if tips.is_empty() {
        tips = long_ends(&points);
    }
    let (w, l) = (PEEL_TAB_WIDTH_MM / 2.0, PEEL_TAB_LENGTH_MM);
    // Into the raft a tab's length too, so it joins it however round it is.
    let tabs: Vec<PolygonWithHoles> = tips
        .into_iter()
        .map(|(at, out)| {
            let side = Vector2::new(-out.y * w, out.x * w);
            let (a, b) = (
                Vector2::new(at.x - out.x * l, at.y - out.y * l),
                Vector2::new(at.x + out.x * l, at.y + out.y * l),
            );
            ring_to_polygon(vec![
                Vector2::new(a.x - side.x, a.y - side.y),
                Vector2::new(b.x - side.x, b.y - side.y),
                Vector2::new(b.x + side.x, b.y + side.y),
                Vector2::new(a.x + side.x, a.y + side.y),
            ])
        })
        .collect();
    let mut all = footprint.to_vec();
    all.extend(tabs);
    union_polygon_sets(&all)
}

/// The sharp corners of `hull` (counter-clockwise): each with the way out of
/// it, sharpest first, apart from each other.
fn sharp_corners(hull: &[Vector2]) -> Vec<(Vector2, Vector2)> {
    let n = hull.len();
    if n < 3 {
        return Vec::new();
    }
    let mut at = Vec::with_capacity(n + 1);
    at.push(0.0);
    for i in 0..n {
        let (a, b) = (hull[i], hull[(i + 1) % n]);
        at.push(at[i] + (b.x - a.x).hypot(b.y - a.y));
    }
    let perimeter = at[n];
    if !(perimeter > 0.0) {
        return Vec::new();
    }
    // The point `s` mm along the outline.
    let point_at = |s: f64| {
        let s = s.rem_euclid(perimeter);
        let i = at.partition_point(|&d| d <= s).saturating_sub(1).min(n - 1);
        let (a, b) = (hull[i], hull[(i + 1) % n]);
        let len = at[i + 1] - at[i];
        let t = if len > 0.0 { (s - at[i]) / len } else { 0.0 };
        Vector2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
    };
    let unit = |x: f64, y: f64| {
        let len = x.hypot(y);
        (len > 0.0).then(|| Vector2::new(x / len, y / len))
    };
    let window = CORNER_WINDOW_MM.min(perimeter / 8.0);
    let mut corners: Vec<(f64, f64, Vector2, Vector2)> = (0..n)
        .filter_map(|i| {
            let (p, back, ahead) = (hull[i], point_at(at[i] - window), point_at(at[i] + window));
            let into = unit(p.x - back.x, p.y - back.y)?;
            let onto = unit(ahead.x - p.x, ahead.y - p.y)?;
            let turn = (into.x * onto.y - into.y * onto.x)
                .atan2(into.x * onto.x + into.y * onto.y)
                .to_degrees();
            let out = unit(into.x - onto.x, into.y - onto.y)?;
            (turn >= CORNER_MIN_TURN_DEG).then_some((turn, at[i], p, out))
        })
        .collect();
    corners.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut kept: Vec<(f64, Vector2, Vector2)> = Vec::new();
    for (_, s, p, out) in corners {
        let apart = |t: f64| {
            let d = (s - t).abs();
            d.min(perimeter - d) >= CORNER_SPACING_MM
        };
        if kept.len() < MAX_PEEL_TABS && kept.iter().all(|k| apart(k.0)) {
            kept.push((s, p, out));
        }
    }
    kept.into_iter().map(|(_, p, out)| (p, out)).collect()
}

/// The two ends of the longer side of `points`, where the outline reaches
/// furthest that way, each with the way out.
fn long_ends(points: &[Vector2]) -> Vec<(Vector2, Vector2)> {
    let (mut lo, mut hi) = (
        Vector2::new(f64::INFINITY, f64::INFINITY),
        Vector2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for p in points {
        lo = Vector2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vector2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let along_x = hi.x - lo.x >= hi.y - lo.y;
    // The middle, across, of where the outline reaches `edge` (within half a
    // millimetre): on a raft wider at one end, the middle of its box is off it.
    let across_at = |edge: f64| {
        let (mut a, mut b) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in points {
            let (along, across) = if along_x { (p.x, p.y) } else { (p.y, p.x) };
            if (along - edge).abs() <= 0.5 {
                a = a.min(across);
                b = b.max(across);
            }
        }
        (a + b) / 2.0
    };
    if along_x {
        vec![
            (Vector2::new(lo.x, across_at(lo.x)), Vector2::new(-1.0, 0.0)),
            (Vector2::new(hi.x, across_at(hi.x)), Vector2::new(1.0, 0.0)),
        ]
    } else {
        vec![
            (Vector2::new(across_at(lo.y), lo.y), Vector2::new(0.0, -1.0)),
            (Vector2::new(across_at(hi.y), hi.y), Vector2::new(0.0, 1.0)),
        ]
    }
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

    /// How many sharp corners of `raft` take a tab, once the tabs joined the
    /// raft and stick out of it.
    fn tabs_on(raft: Vec<Vector2>) -> usize {
        let polys = vec![ring_to_polygon(raft.clone())];
        let tabbed = with_peel_tabs(&polys).unwrap();
        assert_eq!(tabbed.len(), 1, "the tabs stand on the raft: {tabbed:?}");
        assert!(area(&tabbed) > area(&polys) + PEEL_TAB_LENGTH_MM * PEEL_TAB_WIDTH_MM);
        let hull = super::super::convex_hull_2d::convex_hull_2d(&raft);
        sharp_corners(&hull).len()
    }

    #[test]
    fn peel_tabs_go_out_of_the_sharp_corners() {
        // A funnel: wide at the back, a point at the front: three corners.
        assert_eq!(
            tabs_on(vec![
                Vector2::new(50.0, 0.0),
                Vector2::new(100.0, 40.0),
                Vector2::new(0.0, 40.0),
            ]),
            3
        );
        // A rectangle: four.
        assert_eq!(
            tabs_on(vec![
                Vector2::new(0.0, 0.0),
                Vector2::new(60.0, 0.0),
                Vector2::new(60.0, 30.0),
                Vector2::new(0.0, 30.0),
            ]),
            4
        );
        // A round raft has no corner: it gets a tab off each end instead.
        let round: Vec<Vector2> = (0..96)
            .map(|i| {
                let a = i as f64 / 96.0 * std::f64::consts::TAU;
                Vector2::new(25.0 * a.cos(), 20.0 * a.sin())
            })
            .collect();
        assert_eq!(tabs_on(round), 0);
    }
}

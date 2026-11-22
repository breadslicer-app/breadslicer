//! The body of a solid raft: its outline around the support roots, the
//! plate (its sides sloped in by the chamfer toward the plate), and the wall
//! along its rim, plain or with a gap in each long straight side.
//!
//! Outlines are counter-clockwise rings of points; a plate has its top at its
//! thickness and its bottom on the plate.

use super::super::raft_types::{RaftSettings, SupportBaseCircle};
use support_geom::geom::buffer_geometry::BufferGeometry;
use support_geom::geom::extrude_geometry::{ExtrudeOptions, extrude_geometry};
use support_geom::geom::shape::{Path, Shape};
use support_geom::geom::{Vec3, Vector2};

/// Twice-free shoelace area: positive for a counter-clockwise ring.
pub fn signed_area_2d(ring: &[Vector2]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (p, q) = (ring[i], ring[(i + 1) % n]);
            p.x * q.y - q.x * p.y
        })
        .sum::<f64>()
        * 0.5
}

/// The convex hull, counter-clockwise from the lowest `x` (then `y`), without
/// points on its edges. One point or none comes back as it is.
pub fn convex_hull_2d(points: &[Vector2]) -> Vec<Vector2> {
    if points.len() <= 1 {
        return points.to_vec();
    }
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| {
        let d = if a.x == b.x { a.y - b.y } else { a.x - b.x };
        d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    let turns_left = |o: Vector2, a: Vector2, b: Vector2| {
        (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x) > 0.0
    };
    let chain = |points: &mut dyn Iterator<Item = Vector2>| {
        let mut out: Vec<Vector2> = Vec::new();
        for p in points {
            while out.len() >= 2 && !turns_left(out[out.len() - 2], out[out.len() - 1], p) {
                out.pop();
            }
            out.push(p);
        }
        out.pop();
        out
    };
    let mut hull = chain(&mut sorted.iter().copied());
    hull.extend(chain(&mut sorted.iter().rev().copied()));
    hull
}

/// The hull of every circle grown by `margin_mm`, each sampled at
/// `samples_per_circle` points (at least 8; 24 by default).
pub fn compute_footprint(
    circles: &[SupportBaseCircle],
    margin_mm: Option<f64>,
    samples_per_circle: Option<f64>,
) -> Vec<Vector2> {
    if circles.is_empty() {
        return Vec::new();
    }
    let margin = margin_mm.unwrap_or(0.0);
    let samples = samples_per_circle.unwrap_or(24.0).floor().max(8.0) as usize;
    let step = std::f64::consts::TAU / samples as f64;
    let points: Vec<Vector2> = circles
        .iter()
        .flat_map(|c| {
            let r = (c.r + margin).max(0.0);
            (0..samples).map(move |i| {
                let t = i as f64 * step;
                Vector2::new(c.x + r * t.cos(), c.y + r * t.sin())
            })
        })
        .collect();
    convex_hull_2d(&points)
}

/// A convex ring with every side moved `d` inward (outward when negative):
/// each corner where its two moved sides meet; where they run parallel, the
/// corner moved `d` along their mean inward direction.
pub fn inset_convex_polygon(ring: &[Vector2], d: f64) -> Vec<Vector2> {
    if ring.len() < 3 || d.abs() < 1e-5 {
        return ring.to_vec();
    }
    let ccw = signed_area_2d(ring) > 0.0;
    let work: Vec<Vector2> = if ccw {
        ring.to_vec()
    } else {
        ring.iter().rev().copied().collect()
    };
    let n = work.len();
    // Each side: a point on it once moved, its direction, its inward normal.
    let sides: Vec<(Vector2, Vector2, Vector2)> = (0..n)
        .map(|i| {
            let (a, b) = (work[i], work[(i + 1) % n]);
            let along = b.sub(a);
            let normal = Vector2::new(-along.y, along.x);
            let length = normal.length();
            if length == 0.0 {
                return (a, Vector2::new(1.0, 0.0), Vector2::new(0.0, 0.0));
            }
            let inward = normal.multiply_scalar(1.0 / length);
            (a.add_scaled_vector(inward, d), along.normalize(), inward)
        })
        .collect();
    let mut out: Vec<Vector2> = (0..n)
        .map(|i| {
            let (p1, d1, n1) = sides[(i + n - 1) % n];
            let (p2, d2, n2) = sides[i];
            let denominator = d1.x * d2.y - d1.y * d2.x;
            if denominator.abs() >= 1e-8 {
                let t = ((p2.x - p1.x) * d2.y - (p2.y - p1.y) * d2.x) * (1.0 / denominator);
                return Vector2::new(p1.x + t * d1.x, p1.y + t * d1.y);
            }
            let mean = n1.add(n2);
            if mean.length_sq() > 1e-12 {
                work[i].add_scaled_vector(mean.normalize(), d)
            } else if n2.length_sq() > 1e-12 {
                work[i].add_scaled_vector(n2, d)
            } else {
                work[i]
            }
        })
        .collect();
    if !ccw {
        out.reverse();
    }
    out
}

/// How far the chamfer slopes the sides in over `height` for an angle from
/// the plate (kept within 45–90°).
fn chamfer_run(height: f64, angle_deg: f64) -> f64 {
    height * (90.0 - angle_deg.clamp(45.0, 90.0)).to_radians().tan()
}

/// A plate over a convex outline: top at the raft's thickness, bottom inset
/// by its chamfer, fanned caps.
pub fn generate_chamfered_base(profile: &[Vector2], settings: &RaftSettings) -> BufferGeometry {
    let thickness = settings.thickness.max(0.0);
    if profile.len() < 3 || thickness == 0.0 {
        return BufferGeometry::default();
    }
    let bottom = inset_convex_polygon(profile, chamfer_run(thickness, settings.chamfer_angle));
    let n = profile.len() as u32;
    let positions: Vec<f64> = profile
        .iter()
        .map(|p| [p.x, p.y, thickness])
        .chain(bottom.iter().take(n as usize).map(|p| [p.x, p.y, 0.0]))
        .flatten()
        .collect();
    let mut indices: Vec<u32> = Vec::new();
    for i in 0..n {
        let j = (i + 1) % n;
        indices.extend([i, n + j, j, i, n + i, n + j]);
    }
    for i in 1..n.saturating_sub(1) {
        indices.extend([0, i, i + 1]);
    }
    for i in 1..n.saturating_sub(1) {
        indices.extend([n, n + i + 1, n + i]);
    }
    indexed(&positions, indices)
}

fn indexed(positions: &[f64], indices: Vec<u32>) -> BufferGeometry {
    if !positions.iter().all(|v| v.is_finite()) {
        return BufferGeometry::default();
    }
    let mut geometry = BufferGeometry::default();
    geometry.set_position(positions);
    geometry.index = Some(indices);
    geometry.compute_vertex_normals();
    geometry
}

/// A wall over a convex outline: the outline and the outline inset by the
/// wall's thickness, extruded `wall_height` up from `base_z`.
pub fn generate_perimeter_wall(
    outline: &[Vector2],
    wall_height: f64,
    wall_thickness: f64,
    base_z: f64,
) -> BufferGeometry {
    let (height, thickness) = (wall_height.max(0.0), wall_thickness.max(0.0));
    if outline.len() < 3 || height == 0.0 || thickness == 0.0 {
        return BufferGeometry::default();
    }
    let inner = inset_convex_polygon(outline, thickness);
    let mut ring = Shape::new(Some(outline));
    ring.holes.push(Path::new(Some(&inner)));
    let mut geometry = extrude_geometry(
        &[ring],
        ExtrudeOptions {
            depth: height,
            steps: 1,
        },
    );
    geometry.compute_vertex_normals();
    geometry.translate(0.0, 0.0, base_z.max(0.0));
    geometry
}

/// Points closer than this are one (mm).
const RING_EPS: f64 = 1e-4;

fn turn(o: Vector2, a: Vector2, b: Vector2) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Whether segments `ab` and `cd` cross or touch.
fn segments_meet(a: Vector2, b: Vector2, c: Vector2, d: Vector2) -> bool {
    let within = |p: Vector2, q: Vector2, r: Vector2| {
        r.x >= p.x.min(q.x) - RING_EPS
            && r.x <= p.x.max(q.x) + RING_EPS
            && r.y >= p.y.min(q.y) - RING_EPS
            && r.y <= p.y.max(q.y) + RING_EPS
    };
    let (t1, t2, t3, t4) = (turn(a, b, c), turn(a, b, d), turn(c, d, a), turn(c, d, b));
    let apart = |s: f64, t: f64| (s > 0.0 && t < 0.0) || (s < 0.0 && t > 0.0);
    if apart(t1, t2) && apart(t3, t4) {
        return true;
    }
    (t1.abs() <= RING_EPS && within(a, b, c))
        || (t2.abs() <= RING_EPS && within(a, b, d))
        || (t3.abs() <= RING_EPS && within(c, d, a))
        || (t4.abs() <= RING_EPS && within(c, d, b))
}

/// A ring without repeated points, closing duplicate or straight corners;
/// empty when what is left crosses itself or has no area.
fn clean_ring(ring: &[Vector2]) -> Vec<Vector2> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let mut points: Vec<Vector2> = Vec::new();
    for &p in ring {
        if points
            .last()
            .is_none_or(|q| q.distance_to_squared(p) > RING_EPS * RING_EPS)
        {
            points.push(p);
        }
    }
    if points.len() >= 2
        && points[0].distance_to_squared(points[points.len() - 1]) <= RING_EPS * RING_EPS
    {
        points.pop();
    }
    let m = points.len();
    if m < 3 {
        return Vec::new();
    }
    let kept: Vec<Vector2> = (0..m)
        .filter(|&i| turn(points[(i + m - 1) % m], points[i], points[(i + 1) % m]).abs() > RING_EPS)
        .map(|i| points[i])
        .collect();
    let n = kept.len();
    let simple = (0..n).all(|i| {
        ((i + 1)..n).all(|j| {
            (j + 1) % n == i
                || j == (i + 1) % n
                || !segments_meet(kept[i], kept[(i + 1) % n], kept[j], kept[(j + 1) % n])
        })
    });
    if n < 3 || !simple || signed_area_2d(&kept).abs() <= RING_EPS {
        return Vec::new();
    }
    kept
}

/// A wall over a convex outline with its top leaned out by the chamfer and a
/// gap in the middle of every long straight side (over 5 mm, turning at most
/// 15° from the side before), 1 mm in from its corners; plain where the
/// rings will not line up.
pub fn generate_crenelated_wall_manual(
    outline: &[Vector2],
    wall_height: f64,
    wall_thickness: f64,
    gap_width: f64,
    base_z: f64,
    chamfer_angle: f64,
) -> BufferGeometry {
    let (height, thickness, gap) = (
        wall_height.max(0.0),
        wall_thickness.max(0.0),
        gap_width.max(0.0),
    );
    if outline.len() < 3 || height == 0.0 || thickness == 0.0 {
        return BufferGeometry::default();
    }
    let angle = chamfer_angle.clamp(45.0, 90.0).to_radians();
    let lean = if (angle - std::f64::consts::FRAC_PI_2).abs() < 0.001 {
        0.0
    } else {
        height / angle.tan()
    };
    let outer = clean_ring(outline);
    if outer.len() < 3 {
        return BufferGeometry::default();
    }
    let plain = |outer: &[Vector2]| generate_perimeter_wall(outer, height, thickness, base_z);
    let inner = clean_ring(&inset_convex_polygon(&outer, thickness));
    if inner.len() < 3 {
        return plain(&outer);
    }
    let leaned = |ring: &[Vector2]| {
        clean_ring(&if lean > 0.001 {
            inset_convex_polygon(ring, -lean)
        } else {
            ring.to_vec()
        })
    };
    let (top_outer, top_inner) = (leaned(&outer), leaned(&inner));
    let n = outer.len();
    if inner.len() != n || top_outer.len() != n || top_inner.len() != n {
        return plain(&outer);
    }
    let (z0, z1) = (base_z, base_z + height);
    let mut positions: Vec<f64> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut quad = |corners: [Vec3; 4]| {
        let base = (positions.len() / 3) as u32;
        positions.extend(corners.iter().flat_map(|v| [v.x, v.y, v.z]));
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    let at = |p: Vector2, z: f64| Vec3::new(p.x, p.y, z);
    let mix = Vector2::lerp_vectors;
    // A stretch of wall from `s` to `e` (fractions of side `i`).
    let mut block = |i: usize, s: f64, e: f64| {
        let j = (i + 1) % n;
        let o = [
            at(mix(outer[i], outer[j], s), z0),
            at(mix(outer[i], outer[j], e), z0),
            at(mix(top_outer[i], top_outer[j], e), z1),
            at(mix(top_outer[i], top_outer[j], s), z1),
        ];
        let w = [
            at(mix(inner[i], inner[j], s), z0),
            at(mix(inner[i], inner[j], e), z0),
            at(mix(top_inner[i], top_inner[j], e), z1),
            at(mix(top_inner[i], top_inner[j], s), z1),
        ];
        quad([o[0], o[1], o[2], o[3]]);
        quad([w[0], w[3], w[2], w[1]]);
        quad([o[3], o[2], w[2], w[3]]);
        quad([o[0], w[0], w[1], o[1]]);
    };
    let mut ends: Vec<(usize, f64, f64)> = Vec::new();
    for i in 0..n {
        let j = (i + 1) % n;
        let along = outer[j].sub(outer[i]);
        let length = along.length();
        let straight = length.is_finite() && length > RING_EPS && {
            let before = outer[i].sub(outer[(i + n - 1) % n]).normalize();
            let cos = before.dot(along.normalize()).clamp(-1.0, 1.0);
            cos.acos().to_degrees() <= 15.0 && length > 5.0
        };
        if !straight {
            block(i, 0.0, 1.0);
            continue;
        }
        // One gap in the middle of the side, 1 mm in from each end.
        let middle = 1.0 + (length - 2.0).max(0.0) * 0.5;
        let (gap_from, gap_to) = (
            (middle - gap * 0.5).max(0.0),
            (middle + gap * 0.5).min(length),
        );
        if gap_from > 0.1 {
            block(i, 0.0, gap_from / length);
        }
        if length > gap_to + 0.1 {
            block(i, gap_to / length, 1.0);
        }
        ends.push((i, gap_from / length, gap_to / length));
    }
    // The gaps' end faces.
    for (i, s, e) in ends {
        let j = (i + 1) % n;
        let side = |ring: &[Vector2], t: f64| mix(ring[i], ring[j], t);
        quad([
            at(side(&outer, s), z0),
            at(side(&inner, s), z0),
            at(side(&top_inner, s), z1),
            at(side(&top_outer, s), z1),
        ]);
        quad([
            at(side(&outer, e), z0),
            at(side(&top_outer, e), z1),
            at(side(&top_inner, e), z1),
            at(side(&inner, e), z0),
        ]);
    }
    let geometry = indexed(&positions, indices);
    if geometry.vertex_count() == 0 {
        return plain(&outer);
    }
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_plate_holds_its_frustum() {
        let square =
            [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)].map(|(x, y)| Vector2::new(x, y));
        let raft = RaftSettings {
            thickness: 1.0,
            chamfer_angle: 45.0,
            ..Default::default()
        };
        let plate = generate_chamfered_base(&square, &raft).to_non_indexed();
        let p = plate.position.unwrap();
        let six: f64 = p
            .as_chunks::<9>()
            .0
            .iter()
            .map(|t| {
                let v = |k: usize| {
                    [
                        f64::from(t[k * 3]),
                        f64::from(t[k * 3 + 1]),
                        f64::from(t[k * 3 + 2]),
                    ]
                };
                let (a, b, c) = (v(0), v(1), v(2));
                a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0])
            })
            .sum();
        // A frustum from 8 × 8 to 10 × 10, 1 high.
        let want = (64.0 + 100.0 + (64.0f64 * 100.0).sqrt()) / 3.0;
        assert!((six / 6.0 - want).abs() < 1e-3, "{}", six / 6.0);
        assert_eq!(convex_hull_2d(&square).len(), 4);
        assert_eq!(signed_area_2d(&square), 100.0);
    }
}

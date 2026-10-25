//! The small single-purpose files of DragonFruit `Rafts/Crenelated/geometry/`:
//! `signedArea2d.ts`, `computeFootprint.ts`, `insetConvexPolygon.ts` and
//! `computeRaftOuterBoundary.ts`.

use super::super::raft_types::{RaftSettings, SupportBaseCircle};
use super::convex_hull_2d::convex_hull_2d;
use crate::three::Vector2;
use crate::three::math_utils::{js_cos, js_max, js_sin};

/// `signedArea2d`: shoelace area, positive counter-clockwise.
pub fn signed_area_2d(poly: &[Vector2]) -> f64 {
    let mut area = 0.0;
    for i in 0..poly.len() {
        let j = (i + 1) % poly.len();
        area += poly[i].x * poly[j].y - poly[j].x * poly[i].y;
    }
    area * 0.5
}

/// `computeFootprint(circles, { marginMm, samplesPerCircle })`: the convex
/// hull of each circle sampled at `samplesPerCircle` points.
pub fn compute_footprint(
    circles: &[SupportBaseCircle],
    margin_mm: Option<f64>,
    samples_per_circle: Option<f64>,
) -> Vec<Vector2> {
    if circles.is_empty() {
        return Vec::new();
    }
    let margin = margin_mm.unwrap_or(0.0);
    let samples_per = js_max(8.0, samples_per_circle.unwrap_or(24.0).floor()) as usize;
    let d_theta = (std::f64::consts::PI * 2.0) / samples_per as f64;
    let mut pts = Vec::with_capacity(circles.len() * samples_per);
    for c in circles {
        let r = js_max(0.0, c.r + margin);
        for i in 0..samples_per {
            let t = i as f64 * d_theta;
            pts.push(Vector2::new(c.x + r * js_cos(t), c.y + r * js_sin(t)));
        }
    }
    convex_hull_2d(&pts)
}

/// `insetConvexPolygon(poly, d)`: each edge moved inward by `d`, neighbours
/// intersected; near-parallel neighbours step along their averaged normal.
pub fn inset_convex_polygon(poly: &[Vector2], d: f64) -> Vec<Vector2> {
    if poly.len() < 3 || d.abs() < 1e-5 {
        return poly.to_vec();
    }
    let is_ccw = signed_area_2d(poly) > 0.0;
    let work: Vec<Vector2> = if is_ccw {
        poly.to_vec()
    } else {
        poly.iter().rev().copied().collect()
    };
    let n = work.len();
    let mut line_points = vec![Vector2::default(); n];
    let mut line_dirs = vec![Vector2::default(); n];
    let mut inward_normals = vec![Vector2::default(); n];
    for i in 0..n {
        let a = work[i];
        let b = work[(i + 1) % n];
        let dir = b.sub(a);
        let inward = Vector2::new(-dir.y, dir.x);
        let len = inward.length();
        if len == 0.0 {
            line_points[i] = a;
            line_dirs[i] = Vector2::new(1.0, 0.0);
            inward_normals[i] = Vector2::new(0.0, 0.0);
            continue;
        }
        let inward = inward.multiply_scalar(1.0 / len);
        inward_normals[i] = inward;
        line_points[i] = Vector2::new(a.x + inward.x * d, a.y + inward.y * d);
        line_dirs[i] = dir.normalize();
    }
    let mut inset = Vec::with_capacity(n);
    for i in 0..n {
        let i_prev = (i + n - 1) % n;
        if let Some(v) = intersect_lines(
            line_points[i_prev],
            line_dirs[i_prev],
            line_points[i],
            line_dirs[i],
        ) {
            inset.push(v);
            continue;
        }
        let prev_inward = inward_normals[i_prev];
        let curr_inward = inward_normals[i];
        let avg = prev_inward.add(curr_inward);
        if avg.length_sq() > 1e-12 {
            let avg = avg.normalize();
            inset.push(Vector2::new(work[i].x + avg.x * d, work[i].y + avg.y * d));
        } else if curr_inward.length_sq() > 1e-12 {
            inset.push(Vector2::new(
                work[i].x + curr_inward.x * d,
                work[i].y + curr_inward.y * d,
            ));
        } else {
            inset.push(work[i]);
        }
    }
    if !is_ccw {
        inset.reverse();
    }
    inset
}

fn intersect_lines(p1: Vector2, d1: Vector2, p2: Vector2, d2: Vector2) -> Option<Vector2> {
    let denom = d1.x * d2.y - d1.y * d2.x;
    if denom.abs() < 1e-8 {
        return None;
    }
    let dx = p2.x - p1.x;
    let dy = p2.y - p1.y;
    let t = (dx * d2.y - dy * d2.x) * (1.0 / denom);
    Some(Vector2::new(p1.x + t * d1.x, p1.y + t * d1.y))
}

/// `computeRaftOuterBoundary(baseProfile, settings)`: the top outline grown by
/// the wall thickness.
pub fn compute_raft_outer_boundary(
    base_profile: &[Vector2],
    settings: &RaftSettings,
) -> Vec<Vector2> {
    if base_profile.len() < 3 {
        return Vec::new();
    }
    offset_polygon_outward(base_profile, settings.wall_thickness)
}

fn offset_polygon_outward(polygon: &[Vector2], distance: f64) -> Vec<Vector2> {
    // `distance <= 0` is false for NaN, which offsets.
    if polygon.len() < 3 || distance <= 0.0 {
        return polygon.to_vec();
    }
    let n = polygon.len();
    let mut result = Vec::with_capacity(n);
    for i in 0..n {
        let prev = polygon[(i + n - 1) % n];
        let curr = polygon[i];
        let next = polygon[(i + 1) % n];
        let edge1 = curr.sub(prev).normalize();
        let edge2 = next.sub(curr).normalize();
        let normal1 = Vector2::new(edge1.y, -edge1.x);
        let normal2 = Vector2::new(edge2.y, -edge2.x);
        let avg_normal = normal1.add(normal2).normalize();
        let cos_angle = normal1.dot(normal2);
        let offset_dist = distance / js_max(0.1, ((1.0 + cos_angle) / 2.0).sqrt());
        result.push(curr.add_scaled_vector(avg_normal, offset_dist));
    }
    result
}

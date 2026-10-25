//! DragonFruit `geometry/generateCrenelatedWallManual.ts`: the perimeter wall
//! built quad by quad, with one gap on every straight edge longer than 5 mm,
//! and the top flared outward by the chamfer.

use super::generate_perimeter_wall::generate_perimeter_wall;
use super::small::{inset_convex_polygon, signed_area_2d};
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::math_utils::{clamp, js_acos, js_max, js_min, js_tan};
use crate::three::{Vec3, Vector2};

const LOOP_EPS: f64 = 1e-4;

fn cross(o: Vector2, a: Vector2, b: Vector2) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

fn on_segment(a: Vector2, b: Vector2, p: Vector2) -> bool {
    p.x >= js_min(a.x, b.x) - LOOP_EPS
        && p.x <= js_max(a.x, b.x) + LOOP_EPS
        && p.y >= js_min(a.y, b.y) - LOOP_EPS
        && p.y <= js_max(a.y, b.y) + LOOP_EPS
}

fn segments_intersect(a1: Vector2, a2: Vector2, b1: Vector2, b2: Vector2) -> bool {
    let o1 = cross(a1, a2, b1);
    let o2 = cross(a1, a2, b2);
    let o3 = cross(b1, b2, a1);
    let o4 = cross(b1, b2, a2);
    if ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0))
        && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0))
    {
        return true;
    }
    (o1.abs() <= LOOP_EPS && on_segment(a1, a2, b1))
        || (o2.abs() <= LOOP_EPS && on_segment(a1, a2, b2))
        || (o3.abs() <= LOOP_EPS && on_segment(b1, b2, a1))
        || (o4.abs() <= LOOP_EPS && on_segment(b1, b2, a2))
}

fn is_simple_loop(poly: &[Vector2]) -> bool {
    let n = poly.len();
    for i in 0..n {
        let (a1, a2) = (poly[i], poly[(i + 1) % n]);
        for j in i + 1..n {
            if (j + 1) % n == i || j == (i + 1) % n {
                continue;
            }
            if segments_intersect(a1, a2, poly[j], poly[(j + 1) % n]) {
                return false;
            }
        }
    }
    true
}

fn sanitize_loop(lp: &[Vector2]) -> Vec<Vector2> {
    if lp.len() < 3 {
        return Vec::new();
    }
    let mut deduped: Vec<Vector2> = Vec::new();
    for &p in lp {
        if deduped
            .last()
            .is_none_or(|prev| prev.distance_to_squared(p) > LOOP_EPS * LOOP_EPS)
        {
            deduped.push(p);
        }
    }
    if deduped.len() >= 2
        && deduped[0].distance_to_squared(deduped[deduped.len() - 1]) <= LOOP_EPS * LOOP_EPS
    {
        deduped.pop();
    }
    if deduped.len() < 3 {
        return Vec::new();
    }
    let m = deduped.len();
    let mut compacted = Vec::new();
    for i in 0..m {
        let prev = deduped[(i + m - 1) % m];
        let cur = deduped[i];
        let next = deduped[(i + 1) % m];
        if cross(prev, cur, next).abs() <= LOOP_EPS {
            continue;
        }
        compacted.push(cur);
    }
    if compacted.len() < 3
        || !is_simple_loop(&compacted)
        || signed_area_2d(&compacted).abs() <= LOOP_EPS
    {
        return Vec::new();
    }
    compacted
}

struct Edge {
    len: f64,
    is_straight: bool,
}

/// `generateCrenelatedWallManual(topProfile, settings)`.
pub fn generate_crenelated_wall_manual(
    top_profile: &[Vector2],
    wall_height: f64,
    wall_thickness: f64,
    crenulation_gap_width: f64,
    thickness: f64,
    chamfer_angle: f64,
) -> BufferGeometry {
    let wall_height = js_max(0.0, wall_height);
    let wall_thickness = js_max(0.0, wall_thickness);
    let gap_width = js_max(0.0, crenulation_gap_width);
    if top_profile.len() < 3 || wall_height == 0.0 || wall_thickness == 0.0 {
        return BufferGeometry::default();
    }
    let angle_rad = js_max(45.0, js_min(90.0, chamfer_angle)) * std::f64::consts::PI / 180.0;
    let top_inset = if (angle_rad - std::f64::consts::PI / 2.0).abs() < 0.001 {
        0.0
    } else {
        wall_height / js_tan(angle_rad)
    };

    let outer = sanitize_loop(top_profile);
    if outer.len() < 3 {
        return BufferGeometry::default();
    }
    let perimeter =
        |outer: &[Vector2]| generate_perimeter_wall(outer, wall_height, wall_thickness, thickness);
    let inner = sanitize_loop(&inset_convex_polygon(&outer, wall_thickness));
    if inner.len() < 3 {
        return perimeter(&outer);
    }
    let top_outer = sanitize_loop(&if top_inset > 0.001 {
        inset_convex_polygon(&outer, -top_inset)
    } else {
        outer.clone()
    });
    let top_inner = sanitize_loop(&if top_inset > 0.001 {
        inset_convex_polygon(&inner, -top_inset)
    } else {
        inner.clone()
    });
    if inner.len() != outer.len()
        || top_outer.len() != outer.len()
        || top_inner.len() != outer.len()
        || top_outer.len() < 3
        || top_inner.len() < 3
    {
        return perimeter(&outer);
    }

    let n = outer.len();
    let z_base = thickness;
    let z_top = z_base + wall_height;

    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let next = (i + 1) % n;
        let dir = outer[next].sub(outer[i]);
        let len = dir.length();
        if !len.is_finite() || len <= LOOP_EPS {
            edges.push(Edge {
                len: 0.0,
                is_straight: false,
            });
            continue;
        }
        let dir = dir.normalize();
        let i_prev = (i + n - 1) % n;
        let prev_dir = outer[i].sub(outer[i_prev]).normalize();
        let dot = clamp(prev_dir.dot(dir), -1.0, 1.0);
        let ang = js_acos(dot) * 180.0 / std::f64::consts::PI;
        edges.push(Edge {
            len,
            is_straight: ang <= 15.0 && len > 5.0,
        });
    }

    let mut positions: Vec<f64> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut add_quad = |p: [Vec3; 4]| {
        let base = (positions.len() / 3) as u32;
        for v in p {
            positions.extend([v.x, v.y, v.z]);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    let v3 = |p: Vector2, z: f64| Vec3::new(p.x, p.y, z);
    let lerp = Vector2::lerp_vectors;

    for i in 0..n {
        let next = (i + 1) % n;
        let edge = &edges[i];
        let (os, oe) = (outer[i], outer[next]);
        let (is, ie) = (inner[i], inner[next]);
        let (tos, toe) = (top_outer[i], top_outer[next]);
        let (tis, tie) = (top_inner[i], top_inner[next]);
        if !edge.is_straight {
            let o = [
                v3(os, z_base),
                v3(oe, z_base),
                v3(toe, z_top),
                v3(tos, z_top),
            ];
            let iv = [
                v3(is, z_base),
                v3(ie, z_base),
                v3(tie, z_top),
                v3(tis, z_top),
            ];
            add_quad([o[0], o[1], o[2], o[3]]);
            add_quad([iv[0], iv[3], iv[2], iv[1]]);
            add_quad([o[3], o[2], iv[2], iv[3]]);
            add_quad([o[0], iv[0], iv[1], o[1]]);
            continue;
        }
        if edge.len <= LOOP_EPS {
            continue;
        }
        let gap_count = 1;
        let margin = 1.0;
        let usable_len = js_max(0.0, edge.len - 2.0 * margin);
        let segment_len = usable_len / gap_count as f64;
        let half_gap = gap_width / 2.0;
        let mut segments: Vec<(f64, f64)> = Vec::new();
        let mut gaps: Vec<(f64, f64)> = Vec::new();
        for g in 0..gap_count {
            let gap_center = margin + segment_len * (g as f64 + 0.5);
            let gap_start = js_max(0.0, gap_center - half_gap);
            let gap_end = js_min(edge.len, gap_center + half_gap);
            gaps.push((gap_start, gap_end));
            let seg_start = if g == 0 {
                0.0
            } else {
                segments[segments.len() - 1].1
            };
            let seg_end = gap_start;
            if seg_end > seg_start + 0.1 {
                segments.push((seg_start, seg_end));
            }
            if g == gap_count - 1 {
                let (final_start, final_end) = (gap_end, edge.len);
                if final_end > final_start + 0.1 {
                    segments.push((final_start, final_end));
                }
            }
        }
        for &(start, end) in &segments {
            let (ts, te) = (start / edge.len, end / edge.len);
            let o = [
                v3(lerp(os, oe, ts), z_base),
                v3(lerp(os, oe, te), z_base),
                v3(lerp(tos, toe, te), z_top),
                v3(lerp(tos, toe, ts), z_top),
            ];
            let iv = [
                v3(lerp(is, ie, ts), z_base),
                v3(lerp(is, ie, te), z_base),
                v3(lerp(tis, tie, te), z_top),
                v3(lerp(tis, tie, ts), z_top),
            ];
            add_quad([o[0], o[1], o[2], o[3]]);
            add_quad([iv[0], iv[3], iv[2], iv[1]]);
            add_quad([o[3], o[2], iv[2], iv[3]]);
            add_quad([o[0], iv[0], iv[1], o[1]]);
        }
        for &(start, end) in &gaps {
            let (ts, te) = (start / edge.len, end / edge.len);
            let (outer_s, outer_e) = (lerp(os, oe, ts), lerp(os, oe, te));
            let (inner_s, inner_e) = (lerp(is, ie, ts), lerp(is, ie, te));
            let (top_outer_s, top_outer_e) = (lerp(tos, toe, ts), lerp(tos, toe, te));
            let (top_inner_s, top_inner_e) = (lerp(tis, tie, ts), lerp(tis, tie, te));
            add_quad([
                v3(outer_s, z_base),
                v3(inner_s, z_base),
                v3(top_inner_s, z_top),
                v3(top_outer_s, z_top),
            ]);
            add_quad([
                v3(outer_e, z_base),
                v3(top_outer_e, z_top),
                v3(top_inner_e, z_top),
                v3(inner_e, z_base),
            ]);
        }
    }

    let mut geometry = BufferGeometry::default();
    geometry.set_position(&positions);
    geometry.index = Some(indices);
    geometry.compute_vertex_normals();
    if geometry.vertex_count() == 0 {
        return perimeter(&outer);
    }
    geometry
}

//! DragonFruit `src/supports/autoSupport/stabilization.ts`.
//!
//! Stabilization: a model that prints fine on its own can still fail because
//! nothing holds it — a corner resting on a point, or a long edge on a line.
//! This pass scores how the oriented mesh bears on the build plate and emits
//! anchor contacts along the low edge skeleton to broaden a deficient base.
//! Pure geometry, deterministic: welded edge graph + bearing hull + surface
//! centroid, then teeth sampled along every low edge at fixed spacing.
//!
//! DragonFruit also logs the verdict for calibration; the port does not.

use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};

use super::pose_stability::{measure_pose_stability, needs_topple_coverage, posed_positions};
use crate::supports::rafts::crenelated::geometry::convex_hull_2d::{Vec2, convex_hull_2d};
use crate::three::Matrix4;
use crate::three::math_utils::{js_cos, js_hypot, js_max, js_min, js_round, js_sin};

/// Low surface within this height of the minimum counts as the bearing locus.
const BEARING_BAND_MM: f64 = 2.0;
/// A projected bearing hull smaller than this (mm²) is unstable regardless.
const MIN_BEARING_AREA_MM2: f64 = 4.0;
/// The projected centroid may sit up to this far outside the bearing hull.
const MARGIN_MM: f64 = 2.0;
/// Tooth spacing (mm) along the low edges.
const SPACING_MM: f64 = 2.5;
/// Most anchors one run emits.
const MAX_ANCHORS: usize = 48;
/// Flank teeth climb this far up the faces when the part rests on a low edge.
const FLANK_RISE_MM: f64 = 4.0;
/// Teeth climb the radiating edges this far when the part rests on a point.
const CORNER_RISE_MM: f64 = 12.0;
/// Buttresses also climb this fraction of the part's height.
const BUTTRESS_HEIGHT_FRACTION: f64 = 0.35;
/// Hard cap on how far any anchor climbs above the base (mm).
const MAX_RISE_MM: f64 = 30.0;
/// Above this many vertices, skip the pass entirely.
const VERT_CAP: usize = 3_000_000;
/// Spacing (mm) between buttress teeth along a rising edge.
const BUTTRESS_SPACING_MM: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StabilizationAnchor {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy)]
struct Tooth {
    x: f64,
    y: f64,
    z: f64,
    tier: u8,
}

/// What a tooth is worth: a brace on the side the part lifts, then the bearing
/// line, then the rest.
fn worth(t: &Tooth) -> f64 {
    match t.tier {
        1 => 0.0,
        0 => 1.0,
        _ => 2.0,
    }
}

/// `worth(a) - worth(b) || (a.tier === 0 ? a.z - b.z : b.z - a.z)`.
fn by_rank(a: &Tooth, b: &Tooth) -> f64 {
    let w = worth(a) - worth(b);
    if w != 0.0 && !w.is_nan() {
        return w;
    }
    if a.tier == 0 { a.z - b.z } else { b.z - a.z }
}

/// `${Math.round(v * 1e4)}` as part of a weld key.
fn weld_part(v: f64) -> i64 {
    js_round(v * 1e4) as i64
}

/// Stabilization anchors for a mesh (`positions` local, `index` the geometry's
/// index, `matrix_world` current). Empty when the pose is stable.
pub fn compute_stabilization_anchors(
    positions: &[[f32; 3]],
    index: Option<&[u32]>,
    matrix_world: &Matrix4,
    has_raft: bool,
) -> Vec<StabilizationAnchor> {
    let vertex_count = positions.len();
    if !(3..=VERT_CAP).contains(&vertex_count) {
        return Vec::new();
    }
    let tri_count = match index {
        Some(i) => i.len() / 3,
        None => vertex_count / 3,
    };
    if tri_count == 0 {
        return Vec::new();
    }

    let e = &matrix_world.elements;
    let to_x = |x: f64, y: f64, z: f64| e[0] * x + e[4] * y + e[8] * z + e[12];
    let to_y = |x: f64, y: f64, z: f64| e[1] * x + e[5] * y + e[9] * z + e[13];
    let to_z = |x: f64, y: f64, z: f64| e[2] * x + e[6] * y + e[10] * z + e[14];
    let at = |i: usize| {
        let p = positions[i];
        (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
    };
    let vertex_of = |c: usize| match index {
        Some(idx) => idx[c] as usize,
        None => c,
    };

    // Welded graph: unique world vertices + unique edges.
    let mut weld: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut vx: Vec<f64> = Vec::new();
    let mut vy: Vec<f64> = Vec::new();
    let mut vz: Vec<f64> = Vec::new();
    let mut edges: IndexSet<(usize, usize)> = IndexSet::new();

    let mut com_x = 0.0;
    let mut com_y = 0.0;
    let mut total_area = 0.0;
    for t in 0..tri_count {
        let ia = vertex_of(t * 3);
        let ib = vertex_of(t * 3 + 1);
        let ic = vertex_of(t * 3 + 2);
        let (ax0, ay0, az0) = at(ia);
        let (bx0, by0, bz0) = at(ib);
        let (cx0, cy0, cz0) = at(ic);

        // Zero-area triangles are a mesh defect and must not join the vertex
        // set; tested in the raw frame, where an affine transform keeps them.
        let rx = bx0 - ax0;
        let ry = by0 - ay0;
        let rz = bz0 - az0;
        let sx = cx0 - ax0;
        let sy = cy0 - ay0;
        let sz = cz0 - az0;
        let nx = ry * sz - rz * sy;
        let ny = rz * sx - rx * sz;
        let nz = rx * sy - ry * sx;
        if nx * nx + ny * ny + nz * nz <= 0.0 {
            continue;
        }

        let mut vertex_at = |(x, y, z): (f64, f64, f64)| -> usize {
            let key = (weld_part(x), weld_part(y), weld_part(z));
            *weld.entry(key).or_insert_with(|| {
                vx.push(to_x(x, y, z));
                vy.push(to_y(x, y, z));
                vz.push(to_z(x, y, z));
                vx.len() - 1
            })
        };
        let a = vertex_at(at(ia));
        let b = vertex_at(at(ib));
        let c = vertex_at(at(ic));
        for (p, q) in [(a, b), (b, c), (c, a)] {
            if p != q {
                edges.insert(if p < q { (p, q) } else { (q, p) });
            }
        }

        let ax = to_x(ax0, ay0, az0);
        let ay = to_y(ax0, ay0, az0);
        let bx = to_x(bx0, by0, bz0);
        let by = to_y(bx0, by0, bz0);
        let cx = to_x(cx0, cy0, cz0);
        let cy = to_y(cx0, cy0, cz0);
        // A vertical face projects to zero XY area: skipped for the footprint
        // centroid only, not for the vertex set.
        let area = ((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)).abs() / 2.0;
        if area <= 0.0 {
            continue;
        }
        total_area += area;
        com_x += area * (ax + bx + cx) / 3.0;
        com_y += area * (ay + by + cy) / 3.0;
    }

    let mut z_min = f64::INFINITY;
    let mut z_max = f64::NEG_INFINITY;
    for &z in &vz {
        if z < z_min {
            z_min = z;
        }
        if z > z_max {
            z_max = z;
        }
    }
    if !z_min.is_finite() {
        return Vec::new();
    }
    if total_area > 0.0 {
        com_x /= total_area;
        com_y /= total_area;
    }

    // Bearing locus: projected hull of the low surface.
    let bearing: Vec<Vec2> = (0..vx.len())
        .filter(|&i| vz[i] - z_min <= BEARING_BAND_MM)
        .map(|i| Vec2::new(vx[i], vy[i]))
        .collect();
    let hull = convex_hull_2d(&bearing);
    let bearing_area_mm2 = if hull.len() >= 3 {
        hull_area(&hull)
    } else {
        0.0
    };
    let depth_mm = centroid_depth(com_x, com_y, &hull);

    // The measurement must see the POSE, not the authored geometry.
    let world_positions = posed_positions(positions, matrix_world);
    let pose_stability = measure_pose_stability(&world_positions, index, 0.0, 0.0, None, has_raft);
    let stands_on_base =
        hull.len() >= 3 && bearing_area_mm2 >= MIN_BEARING_AREA_MM2 && depth_mm >= -MARGIN_MM;
    if stands_on_base && !needs_topple_coverage(&pose_stability) {
        return Vec::new();
    }

    // Unstable: lay teeth along the low edge skeleton.
    let is_bearing = |id: usize| vz[id] - z_min <= BEARING_BAND_MM;
    let mut bearing_edges: Vec<(usize, usize)> = Vec::new();
    let mut climb_edges: Vec<(usize, usize)> = Vec::new();
    for &(a, b) in &edges {
        let (ba, bb) = (is_bearing(a), is_bearing(b));
        if ba && bb {
            bearing_edges.push((a, b));
        } else if ba || bb {
            climb_edges.push((a, b));
        }
    }

    // Climb the rising edges to where the moment actually acts.
    let part_height = z_max - z_min;
    let base_rise = if bearing_edges.is_empty() {
        CORNER_RISE_MM
    } else {
        FLANK_RISE_MM
    };
    let rise_cap = if pose_stability.drag_top_mm > 0.0 {
        js_min(part_height, js_max(base_rise, pose_stability.drag_top_mm))
    } else {
        js_min(
            MAX_RISE_MM,
            js_max(base_rise, part_height * BUTTRESS_HEIGHT_FRACTION),
        )
    };

    // Half-spacing XY cell, one contact per cell, keeping the better tooth.
    let cell = SPACING_MM * 0.5;
    let mut best: IndexMap<(i64, i64), Tooth> = IndexMap::new();
    let mut put = |x: f64, y: f64, z: f64, tier: u8| {
        let key = (js_round(x / cell) as i64, js_round(y / cell) as i64);
        let tooth = Tooth { x, y, z, tier };
        match best.get(&key) {
            Some(existing) if !(by_rank(&tooth, existing) < 0.0) => {}
            _ => {
                best.insert(key, tooth);
            }
        }
    };
    // The drag pushes the part over the edge on `pushDirDeg`; the material
    // that lifts is on the far side.
    let tension_rad = ((pose_stability.push_dir_deg + 180.0) * std::f64::consts::PI) / 180.0;
    let tension_x = js_cos(tension_rad);
    let tension_y = js_sin(tension_rad);
    let tension_side =
        |id: usize| (vx[id] - com_x) * tension_x + (vy[id] - com_y) * tension_y > 0.0;
    let mut emit_edge = |a: usize, b: usize, cap: f64, spacing: f64, tier: u8| {
        let lo = if vz[a] <= vz[b] { a } else { b };
        let hi = if lo == a { b } else { a };
        let (lx, ly, lz) = (vx[lo], vy[lo], vz[lo]);
        let (hx, hy, hz) = (vx[hi], vy[hi], vz[hi]);
        if lz - z_min > cap {
            return;
        }
        let len = js_hypot(&[hx - lx, hy - ly, hz - lz]);
        if len <= 1e-9 {
            return;
        }
        let steps = (len / spacing).floor() as i64;
        for k in 0..=steps {
            let t = (k as f64 * spacing) / len;
            let z = lz + (hz - lz) * t;
            if z - z_min > cap + 1e-9 {
                break;
            }
            put(lx + (hx - lx) * t, ly + (hy - ly) * t, z, tier);
        }
    };

    let sort_by_rank = |teeth: &mut Vec<Tooth>| {
        teeth.sort_by(|a, b| {
            by_rank(a, b)
                .partial_cmp(&0.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    };
    let anchor = |t: &Tooth| StabilizationAnchor {
        x: t.x,
        y: t.y,
        z: t.z,
    };

    if !bearing_edges.is_empty() {
        // Edge/face contact: the bearing line is the stance; braces climb the
        // adjacent faces, tension side first.
        for &(a, b) in &bearing_edges {
            emit_edge(a, b, f64::INFINITY, SPACING_MM, 0);
        }
        for &(a, b) in &climb_edges {
            let tier = if tension_side(a) || tension_side(b) {
                1
            } else {
                2
            };
            emit_edge(a, b, rise_cap, BUTTRESS_SPACING_MM, tier);
        }
        let mut all: Vec<Tooth> = best.into_values().collect();
        sort_by_rank(&mut all);
        return all.iter().take(MAX_ANCHORS).map(anchor).collect();
    }
    // Lone point: climb the radiating edges for a wide tripod, deliberately
    // not biased to the tension side.
    for &(a, b) in &climb_edges {
        emit_edge(a, b, rise_cap, SPACING_MM, 1);
    }
    let mut all: Vec<Tooth> = best.into_values().collect();
    sort_by_rank(&mut all);
    if all.len() <= MAX_ANCHORS {
        return all.iter().map(anchor).collect();
    }
    (0..MAX_ANCHORS)
        .map(|i| anchor(&all[(i * (all.len() - 1)) / (MAX_ANCHORS - 1)]))
        .collect()
}

fn hull_area(hull: &[Vec2]) -> f64 {
    let mut area = 0.0;
    for i in 0..hull.len() {
        let p = hull[i];
        let q = hull[(i + 1) % hull.len()];
        area += p.x * q.y - q.x * p.y;
    }
    area.abs() / 2.0
}

/// Signed distance from (px,py) to the hull's nearest edge, positive inside;
/// infinite for a hull with no edges.
fn centroid_depth(px: f64, py: f64, hull: &[Vec2]) -> f64 {
    let mut min_depth = f64::INFINITY;
    for i in 0..hull.len() {
        let a = hull[i];
        let b = hull[(i + 1) % hull.len()];
        let ex = b.x - a.x;
        let ey = b.y - a.y;
        let len = match js_hypot(&[ex, ey]) {
            l if l == 0.0 || l.is_nan() => 1.0,
            l => l,
        };
        let nx = -ey / len;
        let ny = ex / len;
        let depth = (px - a.x) * nx + (py - a.y) * ny;
        if depth < min_depth {
            min_depth = depth;
        }
    }
    min_depth
}

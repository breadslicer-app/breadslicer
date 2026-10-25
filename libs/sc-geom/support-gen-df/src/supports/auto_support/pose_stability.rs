//! DragonFruit `src/supports/autoSupport/poseStability.ts`.
//!
//! Pose stability: whether the part can stand on the plate and how hard the
//! peel drags it. The same model as the Rust `compute_stability_report`: the
//! driving moment about a bearing-hull edge is `M_e = Σ A·(n_xy·u)·z`, gravity
//! restores `ρg·V·d_e` and plate adhesion `σ·A_contact·d̄_e`; both collapse to
//! one constant each, so this reports the geometry and the constants stay in
//! one place.
//!
//! Rotation convention: Rx(a) then Ry(b), i.e. `y1 = y·ca − z·sa;
//! z1 = y·sa + z·ca; x2 = x·cb + z1·sb; z2 = −x·sb + z1·cb`.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::supports::rafts::crenelated::geometry::convex_hull_2d::{Vec2, convex_hull_2d};
use crate::three::Matrix4;
use crate::three::math_utils::{js_atan2, js_cos, js_hypot, js_max, js_round, js_sin};
use crate::volume_analysis::islands::voxel_footprint::number_key;

/// Plate-contact band (mm) the bearing hull is measured over.
const BEARING_BAND_MM: f64 = 2.0;
/// Band points are quantized to this XY cell before hulling.
const BAND_CELL_MM: f64 = 0.5;
/// ...and so is the XY shadow.
const SHADOW_CELL_MM: f64 = 0.5;
/// Below this enclosed volume there is no mass to restore anything.
const MIN_VOLUME_MM3: f64 = 1e-9;

/// Down-facing area in the contact band, by the advisor's cost buckets.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestingContact {
    pub overhang_area_mm2: f64,
    pub cup_area_mm2: f64,
    pub scar_area_mm2: f64,
    pub blocked_area_mm2: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoseStability {
    /// World Z of the lowest point under this pose (mm).
    pub plate_z_mm: f64,
    pub volume_mm3: f64,
    /// XY area of the bearing hull (mm²); 0 = no bearing polygon.
    pub bearing_area_mm2: f64,
    /// Hull edge count; 0 = a point or edge contact.
    pub bearing_edges: usize,
    /// How far the volume centroid sits inside the worst edge (mm).
    pub centroid_depth_mm: f64,
    /// The same for the bearing patch's own centroid (mm).
    pub contact_depth_mm: f64,
    /// Driving moment about the worst edge (mm³).
    pub drag_moment_mm3: f64,
    /// `V·d_e/M_e` (mm); infinite when the pose carries no lateral drag.
    pub margin_mm: f64,
    /// `A_contact·d̄_e/M_e` (dimensionless).
    pub adhesion_ratio: f64,
    /// Part height above the bearing plane (mm).
    pub height_mm: f64,
    /// `volume / footprint` (mm).
    pub thickness_mm: f64,
    /// `height / thicknessMm`.
    pub slenderness: f64,
    /// Height (mm, above the part's own base) of the top of the highest face that drags.
    pub drag_top_mm: f64,
    /// Azimuth of the worst edge's outward normal (deg, 0 = +X, 90 = +Y).
    pub push_dir_deg: f64,
    /// Down-facing area that lies in the contact band (the plate contact).
    pub resting_contact: RestingContact,
}

/// The advisor's per-triangle inputs, needed to split the resting contact the
/// same way its cost does.
pub struct SupportTerms<'a> {
    pub normals: &'a [f64],
    pub areas: &'a [f64],
    pub details: &'a [f64],
    pub blocked: Option<&'a HashSet<usize>>,
    /// cos(self-support angle) — the advisor's `threshold`.
    pub threshold: f64,
    /// cos of the near-flat cutoff — the advisor's `CUP_FLAT_COS`.
    pub cup_cos: f64,
}

/// Conservative `p/σ` for the adhesion verdict.
pub const CONSERVATIVE_P_SIGMA: f64 = 0.05;

/// How many times taller than thick a part may be before it counts as slender.
pub const SLENDER_RATIO: f64 = 4.0;

/// Is this part tall enough that sway matters?
pub fn is_slender_part(height_mm: f64, volume_mm3: f64, bearing_area_mm2: f64) -> bool {
    if !(bearing_area_mm2 > 0.0) || !(volume_mm3 > 0.0) {
        return false;
    }
    height_mm / (volume_mm3 / bearing_area_mm2) >= SLENDER_RATIO
}

/// A flat at least this big (mm², 3D) is worth anchoring a part with.
pub const STEEP_FLAT_ANCHOR_MIN_AREA_MM2: f64 = 150.0;

/// Should this steep flat be covered, given the pose's verdict?
pub fn steep_flat_needs_coverage(
    surface_area_mm2: Option<f64>,
    verdict_needs_coverage: bool,
) -> bool {
    if verdict_needs_coverage {
        return true;
    }
    surface_area_mm2.unwrap_or(0.0) >= STEEP_FLAT_ANCHOR_MIN_AREA_MM2
}

/// Does this pose need anti-topple contact at all? The adhesion verdict alone.
pub fn needs_topple_coverage(s: &PoseStability) -> bool {
    s.adhesion_ratio < CONSERVATIVE_P_SIGMA
}

/// The mesh's vertices in world space, as a `Float32Array`: every stability
/// measurement has to happen in the pose's frame.
pub fn posed_positions(local: &[[f32; 3]], matrix_world: &Matrix4) -> Vec<f32> {
    let e = &matrix_world.elements;
    let mut out = Vec::with_capacity(local.len() * 3);
    for p in local {
        let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        out.push((e[0] * x + e[4] * y + e[8] * z + e[12]) as f32);
        out.push((e[1] * x + e[5] * y + e[9] * z + e[13]) as f32);
        out.push((e[2] * x + e[6] * y + e[10] * z + e[14]) as f32);
    }
    out
}

/// A pose with no bearing polygon, or with its mass outside the base, cannot
/// stand on the plate whatever the drag.
pub fn is_statically_unstable(s: &PoseStability) -> bool {
    if s.bearing_edges == 0 {
        return true;
    }
    s.volume_mm3 > MIN_VOLUME_MM3 && s.centroid_depth_mm <= 0.0
}

fn empty_stability() -> PoseStability {
    PoseStability {
        plate_z_mm: 0.0,
        volume_mm3: 0.0,
        bearing_area_mm2: 0.0,
        bearing_edges: 0,
        centroid_depth_mm: 0.0,
        contact_depth_mm: 0.0,
        drag_moment_mm3: 0.0,
        margin_mm: 0.0,
        adhesion_ratio: 0.0,
        push_dir_deg: 0.0,
        height_mm: 0.0,
        thickness_mm: 0.0,
        slenderness: 0.0,
        drag_top_mm: 0.0,
        resting_contact: RestingContact::default(),
    }
}

/// Measure a pose. `positions` is flat xyz (the posed `Float32Array`), `index`
/// the triangle index (`None` for a soup). DragonFruit's default `has_raft` is
/// false.
pub fn measure_pose_stability(
    positions: &[f32],
    index: Option<&[u32]>,
    rot_x_rad: f64,
    rot_y_rad: f64,
    terms: Option<&SupportTerms<'_>>,
    has_raft: bool,
) -> PoseStability {
    let sa = js_sin(rot_x_rad);
    let ca = js_cos(rot_x_rad);
    let sb = js_sin(rot_y_rad);
    let cb = js_cos(rot_y_rad);
    let index = index.filter(|i| !i.is_empty());
    let tri_count = match index {
        Some(i) => i.len() / 3,
        None => positions.len() / 9,
    };
    let pos = |i: usize| f64::from(positions[i]);
    let corner = |c: usize| match index {
        Some(idx) => idx[c] as usize * 3,
        None => c * 3,
    };

    // Rotated vertices of triangle t, or None for a zero-area triangle (a mesh
    // defect that must not define the plate plane; tested in the raw frame).
    let load = |t: usize| -> Option<([f64; 3], [f64; 3], [f64; 3])> {
        let mut vx = [0.0; 3];
        let mut vy = [0.0; 3];
        let mut vz = [0.0; 3];
        for k in 0..3 {
            let i = corner(t * 3 + k);
            let (x, y, z) = (pos(i), pos(i + 1), pos(i + 2));
            let y1 = y * ca - z * sa;
            let z1 = y * sa + z * ca;
            vx[k] = x * cb + z1 * sb;
            vy[k] = y1;
            vz[k] = -x * sb + z1 * cb;
        }
        let (i0, i1, i2) = (corner(t * 3), corner(t * 3 + 1), corner(t * 3 + 2));
        let ux = pos(i1) - pos(i0);
        let uy = pos(i1 + 1) - pos(i0 + 1);
        let uz = pos(i1 + 2) - pos(i0 + 2);
        let wx = pos(i2) - pos(i0);
        let wy = pos(i2 + 1) - pos(i0 + 1);
        let wz = pos(i2 + 2) - pos(i0 + 2);
        let nx = uy * wz - uz * wy;
        let ny = uz * wx - ux * wz;
        let nz = ux * wy - uy * wx;
        if nx * nx + ny * ny + nz * nz <= 0.0 {
            return None;
        }
        Some((vx, vy, vz))
    };

    // Pass 1 — plate plane, enclosed volume, volume centroid.
    let mut z_min = f64::INFINITY;
    let mut z_max = f64::NEG_INFINITY;
    let mut vol6 = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for t in 0..tri_count {
        let Some((vx, vy, vz)) = load(t) else {
            continue;
        };
        for &z in &vz {
            if z < z_min {
                z_min = z;
            }
            if z > z_max {
                z_max = z;
            }
        }
        let det = vx[0] * (vy[1] * vz[2] - vz[1] * vy[2]) - vy[0] * (vx[1] * vz[2] - vz[1] * vx[2])
            + vz[0] * (vx[1] * vy[2] - vy[1] * vx[2]);
        vol6 += det;
        cx += (det * (vx[0] + vx[1] + vx[2])) / 4.0;
        cy += (det * (vy[0] + vy[1] + vy[2])) / 4.0;
    }
    if !z_min.is_finite() {
        return empty_stability();
    }
    let volume_mm3 = (vol6 / 6.0).abs();
    let com_x = if vol6 != 0.0 { cx / vol6 } else { 0.0 };
    let com_y = if vol6 != 0.0 { cy / vol6 } else { 0.0 };

    // Pass 2 — bearing locus, quantized: the low band's hull, or with a raft
    // the model's XY shadow (a lower bound on the raft's footprint).
    let mut band: IndexMap<u64, Vec2> = IndexMap::new();
    let cell = if has_raft {
        SHADOW_CELL_MM
    } else {
        BAND_CELL_MM
    };
    for t in 0..tri_count {
        let Some((vx, vy, vz)) = load(t) else {
            continue;
        };
        for k in 0..3 {
            if !has_raft && vz[k] - z_min > BEARING_BAND_MM {
                continue;
            }
            let qx = js_round(vx[k] / cell);
            let qy = js_round(vy[k] / cell);
            let key = number_key(qx * 1e7 + qy);
            band.entry(key).or_insert(Vec2::new(vx[k], vy[k]));
        }
    }
    let band_points: Vec<Vec2> = band.into_values().collect();
    let hull = convex_hull_2d(&band_points);
    let mut bearing_area_mm2 = 0.0;
    let mut contact_x = 0.0;
    let mut contact_y = 0.0;
    if hull.len() >= 3 {
        let mut area2 = 0.0;
        let mut sx = 0.0;
        let mut sy = 0.0;
        for i in 0..hull.len() {
            let p = hull[i];
            let q = hull[(i + 1) % hull.len()];
            let cross = p.x * q.y - q.x * p.y;
            area2 += cross;
            sx += (p.x + q.x) * cross;
            sy += (p.y + q.y) * cross;
        }
        bearing_area_mm2 = area2.abs() / 2.0;
        if area2.abs() > 1e-9 {
            contact_x = sx / (3.0 * area2);
            contact_y = sy / (3.0 * area2);
        }
    }

    // Pass 3 — driving moment about each hull edge. `hull` is CCW, so an
    // edge's OUTWARD normal is (ey, −ex)/len.
    struct Edge {
        nx: f64,
        ny: f64,
        x: f64,
        y: f64,
        sum: f64,
    }
    let mut edges: Vec<Edge> = Vec::new();
    if hull.len() >= 3 {
        for i in 0..hull.len() {
            let p = hull[i];
            let q = hull[(i + 1) % hull.len()];
            let ex = q.x - p.x;
            let ey = q.y - p.y;
            let len = js_hypot(&[ex, ey]);
            if len <= 1e-9 {
                continue;
            }
            edges.push(Edge {
                nx: ey / len,
                ny: -ex / len,
                x: p.x,
                y: p.y,
                sum: 0.0,
            });
        }
    }
    let mut drag_top_mm = 0.0;
    let mut resting = RestingContact::default();
    for t in 0..tri_count {
        let Some((vx, vy, vz)) = load(t) else {
            continue;
        };
        let ax = vx[1] - vx[0];
        let ay = vy[1] - vy[0];
        let az = vz[1] - vz[0];
        let bx = vx[2] - vx[0];
        let by = vy[2] - vy[0];
        let bz = vz[2] - vz[0];
        let nx = ay * bz - az * by;
        let ny = az * bx - ax * bz;
        let nz = ax * by - ay * bx;
        let twice_area = js_hypot(&[nx, ny, nz]);
        if twice_area <= 0.0 {
            continue;
        }
        // Up-facing and vertical faces do not peel away from the plate.
        if nz >= 0.0 {
            continue;
        }
        let height = (vz[0] + vz[1] + vz[2]) / 3.0 - z_min;
        let area = twice_area / 2.0;
        // The plate contact: down-facing area inside the band.
        if let Some(terms) = terms
            && height <= BEARING_BAND_MM
        {
            let nz_unit = nz / twice_area;
            if nz_unit < -terms.threshold {
                resting.overhang_area_mm2 += area;
                resting.scar_area_mm2 += area * terms.details[t];
                if terms.blocked.is_some_and(|b| b.contains(&t)) {
                    resting.blocked_area_mm2 += area;
                }
            }
            if nz_unit < -terms.cup_cos {
                resting.cup_area_mm2 += area;
            }
        }
        let sin_theta = js_hypot(&[nx, ny]) / twice_area;
        if sin_theta <= 1e-6 {
            continue;
        }
        if height <= 0.0 {
            continue;
        }
        let face_top = js_max(js_max(vz[0], vz[1]), vz[2]) - z_min;
        if face_top > drag_top_mm {
            drag_top_mm = face_top;
        }
        let nhx = nx / twice_area;
        let nhy = ny / twice_area;
        for e in &mut edges {
            let proj = nhx * e.nx + nhy * e.ny;
            if proj > 0.0 {
                e.sum += area * proj * height;
            }
        }
    }
    let mut worst_margin = f64::INFINITY;
    let mut worst_depth = 0.0;
    let mut worst_contact_depth = 0.0;
    let mut worst_sum = 0.0;
    let mut worst_dir_deg = 0.0;
    let mut have_worst = false;
    for e in &edges {
        let depth = (e.x - com_x) * e.nx + (e.y - com_y) * e.ny;
        let contact_depth = (e.x - contact_x) * e.nx + (e.y - contact_y) * e.ny;
        let sum = e.sum;
        let margin = if volume_mm3 <= MIN_VOLUME_MM3 {
            0.0
        } else if sum > 1e-9 {
            (volume_mm3 * depth) / sum
        } else {
            f64::INFINITY
        };
        if !have_worst || margin < worst_margin {
            have_worst = true;
            worst_margin = margin;
            worst_depth = depth;
            worst_contact_depth = contact_depth;
            worst_sum = sum;
            worst_dir_deg = (js_atan2(e.ny, e.nx) * 180.0) / std::f64::consts::PI;
        }
    }
    if edges.is_empty() {
        worst_margin = 0.0;
        worst_depth = 0.0;
        worst_contact_depth = 0.0;
        worst_sum = 0.0;
    }
    let height_mm = if z_max.is_finite() {
        z_max - z_min
    } else {
        0.0
    };
    PoseStability {
        plate_z_mm: z_min,
        volume_mm3,
        bearing_area_mm2,
        bearing_edges: edges.len(),
        centroid_depth_mm: worst_depth,
        contact_depth_mm: worst_contact_depth,
        drag_moment_mm3: worst_sum,
        margin_mm: worst_margin,
        adhesion_ratio: if worst_sum > 1e-9 {
            (bearing_area_mm2 * worst_contact_depth) / worst_sum
        } else {
            f64::INFINITY
        },
        push_dir_deg: (worst_dir_deg + 360.0) % 360.0,
        height_mm,
        thickness_mm: if bearing_area_mm2 > 0.0 {
            volume_mm3 / bearing_area_mm2
        } else {
            0.0
        },
        slenderness: if bearing_area_mm2 > 0.0 && volume_mm3 > 0.0 {
            height_mm / (volume_mm3 / bearing_area_mm2)
        } else {
            0.0
        },
        drag_top_mm,
        resting_contact: resting,
    }
}

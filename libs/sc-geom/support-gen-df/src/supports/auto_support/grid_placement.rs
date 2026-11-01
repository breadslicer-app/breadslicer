//! DragonFruit `src/supports/autoSupport/gridPlacement.ts`.
//!
//! Unified distribution: a fixed-density boundary ring plus grid infill for
//! every overhang region the density grid serves. A big sole earns a carpet
//! because it is big; a sliver earns a thin line because its 2D-projected
//! boundary is short; a tiny patch stays on the single-pillar path. Every
//! point's Z is the exact surface height (triangle sampler, voxel fallback).

use std::collections::{HashMap, HashSet};

use super::constants::{ISLAND_TWO_POINT_MAX_MM, SLOPE_RELAX_RAMP_START};
use super::contact_tip_cap::apply_contact_tip_caps;
use super::parameter_sizing::SizingBands;
use super::settings::AutoSupportSettings;
use super::support_blockers::is_support_blocked_contact;
use super::types::{AutoSupportMesh, CandidatePoint, CandidateSource, js_compare};
use crate::supports::settings::types::SupportSettings;
use crate::three::math_utils::{js_atan2, js_hypot, js_max, js_min, js_pow, js_round, js_to_fixed};
use crate::three::{Matrix3, Ray, Vec3};
use crate::three_mesh_bvh::{BvhMesh, Side};
use crate::volume_analysis::islands::types::{DetectedIsland, IslandSource, Vec3Loop};
use crate::volume_analysis::islands::voxel_footprint::{FootprintPoint, cell_key_bits};

const FOOTPRINT_TOLERANCE_MM: f64 = 0.25;

/// A surface point the samplers and ring builders produce.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub face_index: Option<usize>,
}

/// One surface sample: Z and, from the triangle path, the contact face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHit {
    pub z: f64,
    pub face_index: Option<usize>,
}

/// Boundary voxels of a region's footprint (a voxel with a missing
/// 8-neighbor), sorted by angle around the footprint centroid and sub-sampled
/// at `spacing` intervals.
pub fn build_boundary_points(
    voxels: &[FootprintPoint],
    spacing: f64,
    fallback_z: f64,
) -> Vec<SurfacePoint> {
    if voxels.is_empty() {
        return Vec::new();
    }
    let mut set: HashSet<u64> = HashSet::new();
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    for v in voxels {
        set.insert(cell_key_bits(js_round(v.x * 4.0), js_round(v.y * 4.0)));
        sum_x += v.x;
        sum_y += v.y;
    }
    let cx = sum_x / voxels.len() as f64;
    let cy = sum_y / voxels.len() as f64;

    let mut boundary: Vec<FootprintPoint> = voxels
        .iter()
        .copied()
        .filter(|v| {
            let kx = js_round(v.x * 4.0);
            let ky = js_round(v.y * 4.0);
            (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    !(dx == 0 && dy == 0)
                        && !set.contains(&cell_key_bits(kx + f64::from(dx), ky + f64::from(dy)))
                })
            })
        })
        .collect();
    if boundary.is_empty() {
        return Vec::new();
    }

    boundary
        .sort_by(|a, b| js_compare(js_atan2(a.y - cy, a.x - cx) - js_atan2(b.y - cy, b.x - cx)));
    let step = js_max(1.0, js_round(spacing / 0.25)) as usize;
    boundary
        .iter()
        .step_by(step)
        .map(|b| SurfacePoint {
            x: b.x,
            y: b.y,
            z: b.z.unwrap_or(fallback_z),
            face_index: None,
        })
        .collect()
}

/// Absolute floor on grid/poisson spacing (mm).
pub const GRID_SPACING_FLOOR_MM: f64 = 1.2;

/// A region's surface sampler: the triangle-accurate one when the mesh and the
/// region's triangles are known, the voxel nearest-neighbor fallback otherwise.
pub enum SurfaceSampler<'a> {
    Triangle {
        mesh: BvhMesh<'a>,
        tris: HashSet<usize>,
        base_z: f64,
    },
    Voxel {
        voxels: &'a [FootprintPoint],
        cell_size: f64,
        min_z: f64,
        hash: Option<HashMap<(i64, i64), Vec<FootprintPoint>>>,
    },
}

impl<'a> SurfaceSampler<'a> {
    /// `createTriangleSurfaceAt`: an upward raycast from just below the
    /// region's base, accepted only when the hit face belongs to the region,
    /// against the double-sided material DragonFruit swaps in.
    pub fn triangle(island: &DetectedIsland, mesh: Option<BvhMesh<'a>>) -> Option<Self> {
        let ids = island.triangle_ids.as_ref().filter(|ids| !ids.is_empty())?;
        Some(SurfaceSampler::Triangle {
            mesh: mesh?,
            tris: ids.iter().copied().collect(),
            base_z: island.base_z,
        })
    }

    /// `createVoxelSurfaceAt`: the nearest footprint voxel within 0.25 mm
    /// supplies containment and Z. The hash is built lazily.
    pub fn voxel(voxels: &'a [FootprintPoint], cell_size: f64, min_z: f64) -> Self {
        SurfaceSampler::Voxel {
            voxels,
            cell_size,
            min_z,
            hash: None,
        }
    }

    /// The surface at (x, y), or None outside the region.
    pub fn at(&mut self, x: f64, y: f64) -> Option<SurfaceHit> {
        match self {
            SurfaceSampler::Triangle { mesh, tris, base_z } => {
                let ray = Ray {
                    origin: Vec3::new(x, y, *base_z - 2.0),
                    direction: Vec3::new(0.0, 0.0, 1.0),
                };
                mesh.intersect_object(&ray, Side::Double, 0.0, f64::INFINITY, false)
                    .into_iter()
                    .find(|hit| tris.contains(&hit.face_index))
                    .map(|hit| SurfaceHit {
                        z: hit.point.z,
                        face_index: Some(hit.face_index),
                    })
            }
            SurfaceSampler::Voxel {
                voxels,
                cell_size,
                min_z,
                hash,
            } => {
                let cell = *cell_size;
                let key =
                    |px: f64, py: f64| ((px / cell).floor() as i64, (py / cell).floor() as i64);
                let hash = hash.get_or_insert_with(|| {
                    let mut h: HashMap<(i64, i64), Vec<FootprintPoint>> = HashMap::new();
                    for p in voxels.iter() {
                        h.entry(key(p.x, p.y)).or_default().push(*p);
                    }
                    h
                });
                let tol_sq = FOOTPRINT_TOLERANCE_MM * FOOTPRINT_TOLERANCE_MM;
                let (gx, gy) = key(x, y);
                let mut best_d2 = f64::INFINITY;
                let mut best_z = *min_z;
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        let Some(bucket) = hash.get(&(gx + dx, gy + dy)) else {
                            continue;
                        };
                        for p in bucket {
                            let ddx = x - p.x;
                            let ddy = y - p.y;
                            let d2 = ddx * ddx + ddy * ddy;
                            if d2 <= tol_sq && d2 < best_d2 {
                                best_d2 = d2;
                                if let Some(z) = p.z {
                                    best_z = z;
                                }
                            }
                        }
                    }
                }
                (best_d2 <= tol_sq).then_some(SurfaceHit {
                    z: best_z,
                    face_index: None,
                })
            }
        }
    }
}

/// Sample perimeter loops (already inset by 0.25 mm, each closed) at uniform
/// spacing, with interpolated Z.
pub fn sample_perimeter_loops(loops: &[Vec3Loop], spacing: f64, _min_z: f64) -> Vec<Vec3> {
    let mut out = Vec::new();
    if loops.is_empty() || spacing <= 0.0 {
        return out;
    }
    for lp in loops {
        if lp.len() < 2 {
            continue;
        }
        let mut total = 0.0;
        let mut seg_len: Vec<f64> = Vec::with_capacity(lp.len());
        for i in 0..lp.len() {
            let a = lp[i];
            let b = lp[(i + 1) % lp.len()];
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let dz = b[2] - a[2];
            let len = (dx * dx + dy * dy + dz * dz).sqrt();
            seg_len.push(len);
            total += len;
        }
        if total < 1e-6 {
            // Degenerate — emit the first point.
            out.push(Vec3::new(lp[0][0], lp[0][1], lp[0][2]));
            continue;
        }
        let steps = js_max(1.0, js_round(total / spacing));
        let step_dist = total / steps;
        let mut seg_idx = 0;
        let mut seg_start = 0.0;
        let mut seg_end = seg_len[0];
        let mut s = 0.0;
        while s < steps {
            let target = s * step_dist;
            while target > seg_end + 1e-9 && seg_idx + 1 < seg_len.len() {
                seg_idx += 1;
                seg_start = seg_end;
                seg_end += seg_len[seg_idx];
            }
            let a = lp[seg_idx];
            let b = lp[(seg_idx + 1) % lp.len()];
            let t = if seg_len[seg_idx] > 1e-9 {
                (target - seg_start) / seg_len[seg_idx]
            } else {
                0.0
            };
            out.push(Vec3::new(
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ));
            s += 1.0;
        }
    }
    out
}

/// Perimeter contact inset (mm): the ring and grid outer ring are generated on
/// the footprint ERODED by this amount (1 mask pixel = 0.25 mm).
pub const PERIMETER_CONTACT_INSET_MM: f64 = 0.25;

/// Mask pixels eroded per side.
const PERIMETER_ERODE_PIXELS: i32 = 1;

/// Interior voxels of a footprint: mask pixels whose neighborhood is fully
/// present (morphological erosion). Empty for regions thinner than the inset.
pub fn erode_footprint(voxels: &[FootprintPoint]) -> Vec<FootprintPoint> {
    let set: HashSet<u64> = voxels
        .iter()
        .map(|p| cell_key_bits(js_round(p.x * 4.0), js_round(p.y * 4.0)))
        .collect();
    voxels
        .iter()
        .copied()
        .filter(|p| {
            let kx = js_round(p.x * 4.0);
            let ky = js_round(p.y * 4.0);
            (-PERIMETER_ERODE_PIXELS..=PERIMETER_ERODE_PIXELS).all(|dx| {
                (-PERIMETER_ERODE_PIXELS..=PERIMETER_ERODE_PIXELS).all(|dy| {
                    (dx == 0 && dy == 0)
                        || set.contains(&cell_key_bits(kx + f64::from(dx), ky + f64::from(dy)))
                })
            })
        })
        .collect()
}

/// Boundary-ring spacing relative to the infill spacing.
pub const PERIMETER_RING_FACTOR: f64 = 0.7;

/// Floor on the boundary-ring spacing (mm).
pub const PERIMETER_SPACING_FLOOR_MM: f64 = 1.0;

/// Per-region candidate cap.
pub const MAX_GRID_CANDIDATES_PER_REGION: f64 = 800.0;

/// Target spacing for a region — the shared density formula:
///
/// `spacing = max(floor, √areaPerSupport × (flat + angleT × (relax − flat)))`,
/// `flat = flatDensityBoost × (threshold / area)^suctionAreaExponent` above the
/// area threshold.
pub fn compute_region_spacing(island: &DetectedIsland, auto: &AutoSupportSettings) -> f64 {
    let base_spacing = js_max(auto.area_per_support_mm2, 0.5).sqrt();
    let self_support_angle_deg = auto.overhang_self_support_angle_deg;
    // Relaxation is a last-stretch effect, not a ramp from flat.
    let relax_start_deg = self_support_angle_deg * SLOPE_RELAX_RAMP_START;
    let ramp_deg = js_max(1e-6, self_support_angle_deg - relax_start_deg);
    let angle_t = js_min(
        1.0,
        js_max(
            0.0,
            (island.overhang_angle_deg.unwrap_or(0.0) - relax_start_deg) / ramp_deg,
        ),
    );
    let min_factor = auto.flat_density_boost;
    let max_factor = auto.slope_relax_factor;

    let area = island.area_mm2.unwrap_or(0.0);
    let threshold = auto.grid_area_threshold_mm2;
    let exponent = auto.suction_area_exponent;
    let area_scale = if exponent > 0.0 && area > threshold {
        js_pow(threshold / area, exponent)
    } else {
        1.0
    };

    let flat_factor = min_factor * area_scale;
    let spacing = base_spacing * (flat_factor + angle_t * (max_factor - flat_factor));
    js_max(GRID_SPACING_FLOOR_MM, spacing)
}

/// Resample a region's perimeter loops as a 2D-projected boundary ring at
/// fixed arc-length spacing, each sample's Z from the region's surface
/// sampler; near-duplicates across overlapping loops are dropped. None when
/// the region carries no usable loops.
pub fn sample_boundary_2d(
    loops: Option<&[Vec3Loop]>,
    spacing: f64,
    surface_at: &mut SurfaceSampler<'_>,
) -> Option<Vec<SurfacePoint>> {
    let loops = loops.filter(|l| !l.is_empty())?;
    if spacing <= 0.0 || !loops.iter().any(|l| l.len() >= 2) {
        return None;
    }

    let mut raw: Vec<SurfacePoint> = Vec::new();
    for lp in loops {
        let mut acc = 0.0;
        for i in 0..lp.len() {
            let a = lp[i];
            let b = lp[(i + 1) % lp.len()];
            let seg_len = js_hypot(&[b[0] - a[0], b[1] - a[1]]);
            if seg_len < 1e-9 {
                continue;
            }
            if acc == 0.0
                && let Some(s) = surface_at.at(a[0], a[1])
            {
                raw.push(SurfacePoint {
                    x: a[0],
                    y: a[1],
                    z: s.z,
                    face_index: s.face_index,
                });
            }
            let mut d = spacing - acc;
            while d <= seg_len {
                let t = d / seg_len;
                let x = a[0] + (b[0] - a[0]) * t;
                let y = a[1] + (b[1] - a[1]) * t;
                if let Some(s) = surface_at.at(x, y) {
                    raw.push(SurfacePoint {
                        x,
                        y,
                        z: s.z,
                        face_index: s.face_index,
                    });
                }
                d += spacing;
            }
            acc = (acc + seg_len) % spacing;
        }
    }

    let mut deduped: Vec<SurfacePoint> = Vec::new();
    let min_dist_sq = (spacing * 0.5) * (spacing * 0.5);
    for p in raw {
        let dup = deduped.iter().any(|q| {
            let dx = p.x - q.x;
            let dy = p.y - q.y;
            dx * dx + dy * dy < min_dist_sq
        });
        if !dup {
            deduped.push(p);
        }
    }
    (!deduped.is_empty()).then_some(deduped)
}

/// Spacing multiplier for a steep flat: a sparse field for anchoring.
pub const STEEP_FLAT_SPACING_MULTIPLIER: f64 = 2.5;

/// How much of a steep flat's height its anchoring contacts may occupy.
pub const STEEP_FLAT_ANCHOR_BAND_FRACTION: f64 = 0.35;
pub const STEEP_FLAT_ANCHOR_BAND_MM: f64 = 6.0;

/// Spacing multiplier for a steep flat, capped so the rung spacing up a face
/// stays under the part's own thickness. DragonFruit's default
/// `part_thickness_mm` is 0.
pub fn steep_flat_spacing_multiplier(
    steep_flat: Option<bool>,
    base_spacing_mm: f64,
    part_thickness_mm: f64,
) -> f64 {
    if steep_flat != Some(true) {
        return 1.0;
    }
    let sparse = STEEP_FLAT_SPACING_MULTIPLIER;
    if !(part_thickness_mm > 0.0) || !(base_spacing_mm > 0.0) {
        return sparse;
    }
    js_max(1.0, js_min(sparse, part_thickness_mm / base_spacing_mm))
}

/// Top of the band a steep flat's anchoring contacts may occupy (mm), or
/// infinite for anything that is not a steep flat (or on a slender part).
pub fn steep_flat_anchor_band_top(island: &DetectedIsland, slender_part: bool) -> f64 {
    if island.steep_flat != Some(true) || slender_part {
        return f64::INFINITY;
    }
    let top = island.max_z.unwrap_or(island.base_z);
    island.base_z
        + js_max(
            STEEP_FLAT_ANCHOR_BAND_MM,
            (top - island.base_z) * STEEP_FLAT_ANCHOR_BAND_FRACTION,
        )
}

/// Footprint bounds `(minX, maxX, minY, maxY)`.
fn bounds(points: &[FootprintPoint]) -> (f64, f64, f64, f64) {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in points {
        if p.x < min_x {
            min_x = p.x;
        }
        if p.x > max_x {
            max_x = p.x;
        }
        if p.y < min_y {
            min_y = p.y;
        }
        if p.y > max_y {
            max_y = p.y;
        }
    }
    (min_x, max_x, min_y, max_y)
}

/// Does this overhang region get the ring + lattice treatment? Area decides
/// density, but a thin footprint longer than the two-point band wants its
/// perimeter sampled whatever its area.
pub fn should_use_density_grid(island: &DetectedIsland, auto: &AutoSupportSettings) -> bool {
    if island.source != IslandSource::Overhang {
        return false;
    }
    if island.area_mm2.unwrap_or(0.0) >= auto.grid_area_threshold_mm2 {
        return true;
    }
    let Some(voxels) = island.contact_voxels.as_ref().filter(|v| v.count > 0) else {
        return false;
    };
    let (min_x, max_x, min_y, max_y) = bounds(&voxels.to_points());
    js_max(max_x - min_x, max_y - min_y) >= ISLAND_TWO_POINT_MAX_MM
}

/// World-space normal of one mesh face (BVH-reordered index), or None for a
/// degenerate face.
pub fn face_normal_at(mesh: BvhMesh<'_>, face_index: usize) -> Option<Vec3> {
    let [i0, i1, i2] = mesh.bvh.face(face_index);
    let a = mesh.bvh.vertex(i0);
    let b = mesh.bvh.vertex(i1);
    let c = mesh.bvh.vertex(i2);
    let normal = b.sub(a).cross(c.sub(a));
    if normal.length_sq() < 1e-12 {
        return None;
    }
    // `applyNormalMatrix` normalizes, and DragonFruit normalizes once more.
    Some(
        normal
            .apply_normal_matrix(&Matrix3::normal_matrix(mesh.matrix_world))
            .normalize(),
    )
}

/// The density grid's candidates for every region it serves, then the
/// free-width tip cap over all of them. `slender_part` ladders steep-flat
/// contacts up the face; `part_thickness_mm` caps their spacing.
#[allow(clippy::too_many_arguments)]
pub fn generate_grid_candidates(
    overhang_islands: &[DetectedIsland],
    auto: &AutoSupportSettings,
    mesh: Option<AutoSupportMesh<'_>>,
    slender_part: bool,
    part_thickness_mm: f64,
    settings: &SupportSettings,
    bands: &SizingBands,
) -> Vec<CandidatePoint> {
    let base_spacing = js_max(auto.area_per_support_mm2, 0.5).sqrt();
    if base_spacing <= 0.0 {
        return Vec::new();
    }

    let mut candidates: Vec<CandidatePoint> = Vec::new();

    for island in overhang_islands {
        if !should_use_density_grid(island, auto) {
            continue;
        }
        let region_spacing = compute_region_spacing(island, auto);
        let spacing = region_spacing
            * steep_flat_spacing_multiplier(island.steep_flat, region_spacing, part_thickness_mm);

        let Some(voxels) = island.contact_voxels.as_ref().filter(|v| v.count > 0) else {
            continue;
        };

        let cell_size = js_max(spacing, 1.0);
        let voxel_points = voxels.to_points();
        let (min_x, max_x, min_y, max_y) = bounds(&voxel_points);

        let surface_normal = island.surface_normal.unwrap_or(Vec3::new(0.0, 0.0, -1.0));
        let mut surface_at = SurfaceSampler::triangle(island, mesh.map(|m| m.mesh.rays))
            .unwrap_or_else(|| SurfaceSampler::voxel(&voxel_points, cell_size, island.base_z));
        let min_z = island.base_z;
        let anchor_band_top = steep_flat_anchor_band_top(island, slender_part);

        let mut emit_point = |x: f64, y: f64, z: f64, kind: &str, face_index: Option<usize>| {
            if z > anchor_band_top {
                return;
            }
            // The contact leans into the face it actually lands on.
            let tip_normal = match (mesh, face_index) {
                (Some(m), Some(face)) => face_normal_at(m.mesh.rays, face),
                _ => None,
            }
            .unwrap_or(surface_normal);
            if let Some(m) = mesh
                && is_support_blocked_contact(
                    m.blocked,
                    m.mesh.rays,
                    m.side,
                    Vec3::new(x, y, z),
                    face_index,
                )
            {
                return;
            }
            candidates.push(CandidatePoint {
                id: format!(
                    "{kind}-{}-{}-{}",
                    island.id,
                    js_to_fixed(x, 2),
                    js_to_fixed(y, 2)
                ),
                tip_pos: Vec3::new(x, y, z),
                tip_normal,
                model_id: String::new(),
                source: CandidateSource::Overhang,
                island_area_mm2: auto.area_per_support_mm2,
                z_height: z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: Some(true),
                zone_size: None,
            });
        };

        // Sliver test: nothing survives footprint erosion → ring only.
        let eroded = erode_footprint(&voxel_points);
        let is_sliver = eroded.is_empty();

        // Candidate-cap guard: on overflow, fall back to the angle-only
        // spacing, then subsample evenly.
        let width = max_x - min_x;
        let height = max_y - min_y;
        let inset = PERIMETER_CONTACT_INSET_MM;
        let span_x = js_max(0.5, width - 2.0 * inset);
        let span_y = js_max(0.5, height - 2.0 * inset);
        let mut stride = 1.0;
        let estimate = ((span_x / spacing).floor() + 2.0) * ((span_y / spacing).floor() + 2.0);
        if estimate > MAX_GRID_CANDIDATES_PER_REGION {
            let fallback = AutoSupportSettings {
                suction_area_exponent: 0.0,
                ..auto.clone()
            };
            let fallback_spacing = compute_region_spacing(island, &fallback);
            let base_estimate = ((span_x / fallback_spacing).floor() + 2.0)
                * ((span_y / fallback_spacing).floor() + 2.0);
            if base_estimate > MAX_GRID_CANDIDATES_PER_REGION {
                stride = (base_estimate / MAX_GRID_CANDIDATES_PER_REGION).ceil();
            }
        }

        let nx = js_max(1.0, js_round(span_x / spacing));
        let ny = js_max(1.0, js_round(span_y / spacing));
        let spacing_x = span_x / nx;
        let spacing_y = span_y / ny;

        // Grid infill, inset by the contact radius. A footprint thinner than
        // one lattice cell gets no infill.
        let mut lattice: Vec<(f64, f64)> = Vec::new();
        if !is_sliver && js_min(width, height) >= spacing {
            let mut i = 0.0;
            while i <= nx {
                let mut j = 0.0;
                while j <= ny {
                    let x = min_x + inset + i * spacing_x;
                    let y = min_y + inset + j * spacing_y;
                    if let Some(s) = surface_at.at(x, y) {
                        lattice.push((x, y));
                        emit_point(x, y, s.z, "grid", s.face_index);
                    }
                    j += stride;
                }
                i += stride;
            }
        }

        // Boundary ring, tighter than the infill and deduped against the
        // lattice at half the ring spacing.
        let ring_spacing =
            js_max(PERIMETER_SPACING_FLOOR_MM, spacing * PERIMETER_RING_FACTOR) * stride;
        let ring_coverage_sq = (ring_spacing * 0.5) * (ring_spacing * 0.5);
        let boundary = sample_boundary_2d(
            island.perimeter_loops.as_deref(),
            ring_spacing,
            &mut surface_at,
        )
        .unwrap_or_else(|| {
            build_boundary_points(
                if eroded.is_empty() {
                    &voxel_points
                } else {
                    &eroded
                },
                ring_spacing,
                min_z,
            )
        });
        for b in boundary {
            let covered = lattice.iter().any(|&(px, py)| {
                let dx = b.x - px;
                let dy = b.y - py;
                dx * dx + dy * dy <= ring_coverage_sq
            });
            if covered {
                continue;
            }
            // Fallback boundary points carry a voxel Z and no face: sample the
            // surface at their XY so the ring keeps the lattice's Z and normal.
            let sampled = surface_at.at(b.x, b.y);
            emit_point(
                b.x,
                b.y,
                sampled.map_or(b.z, |s| s.z),
                "fill",
                sampled.and_then(|s| s.face_index).or(b.face_index),
            );
        }
    }

    apply_contact_tip_caps(&mut candidates, mesh.map(|m| m.mesh.field), settings, bands);
    candidates
}

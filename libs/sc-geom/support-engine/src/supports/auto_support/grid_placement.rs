//! DragonFruit `src/supports/autoSupport/gridPlacement.ts`.
//!
//! Unified distribution: a fixed-density boundary ring plus grid infill for
//! every overhang region the density grid serves. A big sole earns a carpet
//! because it is big; a sliver earns a thin line because its 2D-projected
//! boundary is short; a tiny patch stays on the single-pillar path. Every
//! point's Z is the exact surface height (triangle sampler, voxel fallback).

use std::collections::{HashMap, HashSet};

use super::types::js_compare;
use crate::bvh::{BvhMesh, Side};
use crate::geom::math_utils::{js_atan2, js_max, js_round};
use crate::geom::{Matrix3, Ray, Vec3};
use crate::volume_analysis::islands::types::DetectedIsland;
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

/// A region's surface sampler: the triangle-accurate one when the mesh and the
/// region's triangles are known, the voxel nearest-neighbor fallback otherwise.
///
/// The region's triangles are faces of the mesh's tree, in the order its
/// build left them in: what a ray's hit names. DragonFruit scans a soup read
/// through the index its tree already reordered, so its scan numbers them
/// that way; a scan of the soup the tree was built from does not, and its
/// regions are renumbered first (`pipeline`, #500).
#[derive(Debug)]
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

    /// SoulCrafted (#500): every surface the region has over (x, y), lowest
    /// first. A region is a connected patch of the model's underside, and it
    /// folds over itself in plan wherever one part of the model hangs over
    /// another: the underside of a raised hand over that of its arm.
    /// [`Self::at`] gives the lowest alone.
    pub fn all_at(&mut self, x: f64, y: f64) -> Vec<SurfaceHit> {
        match self {
            SurfaceSampler::Triangle { mesh, tris, base_z } => {
                let ray = Ray {
                    origin: Vec3::new(x, y, *base_z - 2.0),
                    direction: Vec3::new(0.0, 0.0, 1.0),
                };
                let mut folds: Vec<SurfaceHit> = mesh
                    .intersect_object(&ray, Side::Double, 0.0, f64::INFINITY, false)
                    .into_iter()
                    .filter(|hit| tris.contains(&hit.face_index))
                    .map(|hit| SurfaceHit {
                        z: hit.point.z,
                        face_index: Some(hit.face_index),
                    })
                    .collect();
                // A ray through an edge meets both of its faces.
                folds.dedup_by(|next, kept| (next.z - kept.z).abs() < 1e-6);
                folds
            }
            SurfaceSampler::Voxel { .. } => self.at(x, y).into_iter().collect(),
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

/// Perimeter contact inset (mm): the ring and grid outer ring are generated on
/// the footprint ERODED by this amount (1 mask pixel = 0.25 mm).
pub const PERIMETER_CONTACT_INSET_MM: f64 = 0.25;

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

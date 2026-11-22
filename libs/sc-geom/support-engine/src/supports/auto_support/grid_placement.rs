//! Reading a region's surface: its height under a point (every height where
//! the region folds over itself), the points round its edge, and a face's
//! normal.

use std::collections::{HashMap, HashSet};

use super::types::js_compare;
use crate::bvh::{BvhMesh, Side};
use crate::geom::math_utils::{js_atan2, js_max, js_round};
use crate::geom::{Matrix3, Ray, Vec3};
use crate::volume_analysis::islands::types::DetectedIsland;
use crate::volume_analysis::islands::voxel_footprint::{FootprintPoint, cell_key_bits};

/// A footprint cell within this of a point stands for it, mm.
const CELL_REACH_MM: f64 = 0.25;

/// A point on a region's surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub face_index: Option<usize>,
}

/// A height read off a region, and the face it is on when known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHit {
    pub z: f64,
    pub face_index: Option<usize>,
}

/// The cells on the edge of a footprint (missing one of their eight
/// neighbours), in turn round its middle, one every `spacing` (in 0.25 mm
/// cells); each at its own height, or `fallback_z`.
pub fn build_boundary_points(
    cells: &[FootprintPoint],
    spacing: f64,
    fallback_z: f64,
) -> Vec<SurfacePoint> {
    if cells.is_empty() {
        return Vec::new();
    }
    let at = |c: &FootprintPoint| (js_round(c.x * 4.0), js_round(c.y * 4.0));
    let present: HashSet<u64> = cells
        .iter()
        .map(|c| {
            let (kx, ky) = at(c);
            cell_key_bits(kx, ky)
        })
        .collect();
    let n = cells.len() as f64;
    let (cx, cy) = (
        cells.iter().fold(0.0, |s, c| s + c.x) / n,
        cells.iter().fold(0.0, |s, c| s + c.y) / n,
    );
    let mut edge: Vec<FootprintPoint> = cells
        .iter()
        .copied()
        .filter(|c| {
            let (kx, ky) = at(c);
            (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    (dx, dy) != (0, 0)
                        && !present.contains(&cell_key_bits(kx + f64::from(dx), ky + f64::from(dy)))
                })
            })
        })
        .collect();
    edge.sort_by(|a, b| js_compare(js_atan2(a.y - cy, a.x - cx) - js_atan2(b.y - cy, b.x - cx)));
    edge.iter()
        .step_by(js_max(1.0, js_round(spacing / 0.25)) as usize)
        .map(|c| SurfacePoint {
            x: c.x,
            y: c.y,
            z: c.z.unwrap_or(fallback_z),
            face_index: None,
        })
        .collect()
}

/// How a region's surface is read: by ray against its own faces when the
/// model and those faces are known (faces numbered as the model's tree
/// names them, #500), else from its footprint cells.
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
    /// The sampler by ray, when the region's faces and the model are known.
    pub fn triangle(island: &DetectedIsland, mesh: Option<BvhMesh<'a>>) -> Option<Self> {
        let ids = island.triangle_ids.as_ref().filter(|ids| !ids.is_empty())?;
        Some(SurfaceSampler::Triangle {
            mesh: mesh?,
            tris: ids.iter().copied().collect(),
            base_z: island.base_z,
        })
    }

    /// The sampler by footprint cells (`cell_size` buckets them).
    pub fn voxel(voxels: &'a [FootprintPoint], cell_size: f64, min_z: f64) -> Self {
        SurfaceSampler::Voxel {
            voxels,
            cell_size,
            min_z,
            hash: None,
        }
    }

    /// The region's faces a ray up through (x, y) from 2 mm under the region
    /// meets, lowest first (a ray through an edge meets one of its faces).
    fn ray_hits(
        mesh: &BvhMesh<'_>,
        tris: &HashSet<usize>,
        base_z: f64,
        x: f64,
        y: f64,
    ) -> Vec<SurfaceHit> {
        let up = Ray {
            origin: Vec3::new(x, y, base_z - 2.0),
            direction: Vec3::new(0.0, 0.0, 1.0),
        };
        mesh.intersect_object(&up, Side::Double, 0.0, f64::INFINITY, false)
            .into_iter()
            .filter(|hit| tris.contains(&hit.face_index))
            .map(|hit| SurfaceHit {
                z: hit.point.z,
                face_index: Some(hit.face_index),
            })
            .collect()
    }

    /// Every height of the region over (x, y), lowest first: a region folds
    /// over itself where one part of the model hangs over another (#500).
    pub fn all_at(&mut self, x: f64, y: f64) -> Vec<SurfaceHit> {
        match self {
            SurfaceSampler::Triangle { mesh, tris, base_z } => {
                let mut hits = Self::ray_hits(mesh, tris, *base_z, x, y);
                hits.dedup_by(|next, kept| (next.z - kept.z).abs() < 1e-6);
                hits
            }
            SurfaceSampler::Voxel { .. } => self.at(x, y).into_iter().collect(),
        }
    }

    /// The region's lowest height over (x, y); none outside it.
    pub fn at(&mut self, x: f64, y: f64) -> Option<SurfaceHit> {
        match self {
            SurfaceSampler::Triangle { mesh, tris, base_z } => {
                Self::ray_hits(mesh, tris, *base_z, x, y).into_iter().next()
            }
            SurfaceSampler::Voxel {
                voxels,
                cell_size,
                min_z,
                hash,
            } => {
                let side = *cell_size;
                let bucket_of =
                    |px: f64, py: f64| ((px / side).floor() as i64, (py / side).floor() as i64);
                let buckets = hash.get_or_insert_with(|| {
                    let mut b: HashMap<(i64, i64), Vec<FootprintPoint>> = HashMap::new();
                    for c in voxels.iter() {
                        b.entry(bucket_of(c.x, c.y)).or_default().push(*c);
                    }
                    b
                });
                // The nearest cell within reach gives the height.
                let reach2 = CELL_REACH_MM * CELL_REACH_MM;
                let (bx, by) = bucket_of(x, y);
                let (mut nearest, mut z) = (f64::INFINITY, *min_z);
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for c in buckets.get(&(bx + dx, by + dy)).into_iter().flatten() {
                            let (ex, ey) = (x - c.x, y - c.y);
                            let d2 = ex * ex + ey * ey;
                            if d2 <= reach2 && d2 < nearest {
                                nearest = d2;
                                if let Some(cz) = c.z {
                                    z = cz;
                                }
                            }
                        }
                    }
                }
                (nearest <= reach2).then_some(SurfaceHit {
                    z,
                    face_index: None,
                })
            }
        }
    }
}

/// The ring and the outer grid of a region stand this far inside its edge,
/// mm (a cell of the mask).
pub const PERIMETER_CONTACT_INSET_MM: f64 = 0.25;

/// The world normal of face `face_index` of the model's tree, unit; none for
/// a face with no area.
pub fn face_normal_at(mesh: BvhMesh<'_>, face_index: usize) -> Option<Vec3> {
    let [i0, i1, i2] = mesh.bvh.face(face_index);
    let (a, b, c) = (
        mesh.bvh.vertex(i0),
        mesh.bvh.vertex(i1),
        mesh.bvh.vertex(i2),
    );
    let normal = b.sub(a).cross(c.sub(a));
    if normal.length_sq() < 1e-12 {
        return None;
    }
    // Made unit by the normal matrix, and again.
    Some(
        normal
            .apply_normal_matrix(&Matrix3::normal_matrix(mesh.matrix_world))
            .normalize(),
    )
}

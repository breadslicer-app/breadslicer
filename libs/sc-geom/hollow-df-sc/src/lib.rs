//! Runs the DragonFruit hollowing port (`hollow-df`) on one leaf of a
//! SoulCrafted scene as a cancellable background job (SC-18, SC-41).
//!
//! The leaf keeps its mesh and its transform. The hollow is computed in the
//! leaf's **physical frame**: the world transform `M` is split into a rotation
//! (or mirror) `R` and a symmetric stretch `P` (`M = R·P`, the polar
//! decomposition), and the engine voxelizes `P·mesh`. Millimetres there are
//! world millimetres, so the wall is the requested thickness even under a
//! non-uniform scale (DragonFruit averages the scale axes instead), and moving
//! or rotating the model changes only `R` and the translation: the voxel
//! session is reused ([`PreparedHollow`]), which is DragonFruit's model-local
//! grid. A scale change rebuilds it.
//!
//! Results come back in the leaf's **local** frame (`P⁻¹` applied), so the
//! GUI can show them under the leaf's own transform or install the applied
//! mesh in place of the original (keeping the original for undo); volumes are
//! physical mm³. Drains and solid areas are given in world coordinates at the
//! current pose; [`LeafFrame`] converts for callers that persist them on the
//! model instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use hollow_df::engine::{DrainReport, HollowStats};
use hollow_df::{
    Control, DrainHole, GridRequest, HollowError, HollowSettings, Session, SolidArea, Stage,
};
use sc_geom_transport_inprocess::{JobError, JobHandle, JobProgress, JobWorker};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{NodeId, Scene, SceneError};

pub use hollow_df;

type M3 = [[f64; 3]; 3];

const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn transpose(a: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in a.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            out[j][i] = v;
        }
    }
    out
}

fn apply(m: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// Eigen-decomposition of a symmetric 3x3 by cyclic Jacobi rotations:
/// eigenvalues and the eigenvectors as the columns of `V`.
fn symmetric_eigen(mut a: M3) -> ([f64; 3], M3) {
    let mut v = IDENTITY;
    for _ in 0..64 {
        let off = a[0][1].powi(2) + a[0][2].powi(2) + a[1][2].powi(2);
        let scale = a[0][0].powi(2) + a[1][1].powi(2) + a[2][2].powi(2);
        if off <= 1e-30 * scale.max(1e-300) {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() <= 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            let mut j = IDENTITY;
            j[p][p] = c;
            j[q][q] = c;
            j[p][q] = s;
            j[q][p] = -s;
            a = mul(&transpose(&j), &mul(&a, &j));
            v = mul(&v, &j);
        }
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

/// A leaf's world transform split for hollowing: `world = R·P·local + t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeafFrame {
    /// Symmetric positive stretch: local to physical frame.
    pub stretch: M3,
    pub stretch_inverse: M3,
    /// Orthogonal (a mirror when the transform mirrors): physical to world.
    pub rotation: M3,
    pub translation: [f64; 3],
}

impl LeafFrame {
    pub fn of(world: &Transform3D) -> Result<Self, SceneError> {
        let Transform3D::Affine(a) = world else {
            return Ok(Self {
                stretch: IDENTITY,
                stretch_inverse: IDENTITY,
                rotation: IDENTITY,
                translation: [0.0; 3],
            });
        };
        if !a.is_invertible() {
            return Err(sc_mesh_core::transform::TransformError::SingularLinear.into());
        }
        let m = *a.linear();
        let (values, v) = symmetric_eigen(mul(&transpose(&m), &m));
        let root = values.map(|l| l.max(0.0).sqrt());
        let diag = |d: [f64; 3]| [[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]];
        let mut stretch = mul(&v, &mul(&diag(root), &transpose(&v)));
        let mut stretch_inverse = mul(&v, &mul(&diag(root.map(|r| 1.0 / r)), &transpose(&v)));
        // A rigid transform gives the identity up to rounding: make it exact,
        // so the session key and the vertices do not move with the pose.
        let near =
            |p: &M3, q: &M3| (0..3).all(|i| (0..3).all(|j| (p[i][j] - q[i][j]).abs() <= 1e-12));
        if near(&stretch, &IDENTITY) {
            stretch = IDENTITY;
            stretch_inverse = IDENTITY;
        }
        Ok(Self {
            rotation: mul(&m, &stretch_inverse),
            stretch,
            stretch_inverse,
            translation: *a.translation(),
        })
    }

    fn is_rigid(&self) -> bool {
        self.stretch == IDENTITY
    }

    pub fn local_to_frame(&self, p: [f32; 3]) -> [f32; 3] {
        if self.is_rigid() {
            return p;
        }
        apply(&self.stretch, p.map(f64::from)).map(|v| v as f32)
    }

    pub fn frame_to_local(&self, p: [f32; 3]) -> [f32; 3] {
        if self.is_rigid() {
            return p;
        }
        apply(&self.stretch_inverse, p.map(f64::from)).map(|v| v as f32)
    }

    /// A world point at the current pose, in the physical frame.
    pub fn world_to_frame(&self, p: [f32; 3]) -> [f32; 3] {
        let d = [0, 1, 2].map(|k| p[k] as f64 - self.translation[k]);
        apply(&transpose(&self.rotation), d).map(|v| v as f32)
    }

    /// A world direction at the current pose, in the physical frame.
    pub fn world_dir_to_frame(&self, d: [f32; 3]) -> [f32; 3] {
        apply(&transpose(&self.rotation), d.map(f64::from)).map(|v| v as f32)
    }

    /// The stretch quantized, for session keys.
    fn key(&self) -> [i64; 9] {
        let mut out = [0i64; 9];
        for i in 0..3 {
            for j in 0..3 {
                out[i * 3 + j] = (self.stretch[i][j] * 1e9).round() as i64;
            }
        }
        out
    }
}

/// What identifies a voxel session: the mesh, the stretch and the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionKey {
    mesh: u64,
    triangles: usize,
    stretch: [i64; 9],
    voxel_bits: u32,
    max_resolution: u16,
}

/// A voxel session kept between jobs (moves and rotations reuse it).
#[derive(Debug)]
pub struct PreparedHollow {
    key: SessionKey,
    session: Session,
}

impl PreparedHollow {
    pub fn session(&self) -> &Session {
        &self.session
    }
}

/// A drain at the current pose, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldDrain {
    /// A point on the outer surface.
    pub center: [f32; 3],
    /// Into the model.
    pub direction: [f32; 3],
    pub radius_mm: f32,
    pub radius_y_mm: Option<f32>,
    pub depth: hollow_df::DrainDepth,
    pub outside_mm: f32,
}

impl WorldDrain {
    pub fn new(center: [f32; 3], direction: [f32; 3], radius_mm: f32) -> Self {
        let hole = DrainHole::new(center, direction, radius_mm);
        Self {
            center,
            direction,
            radius_mm,
            radius_y_mm: None,
            depth: hole.depth,
            outside_mm: hole.outside_mm,
        }
    }
}

/// Material to keep, in world coordinates at the current pose.
#[derive(Debug, Clone, PartialEq)]
pub enum WorldSolidArea {
    /// Voxel indices of the session's grid (from a preview of the same session).
    Voxels(Vec<usize>),
    Sphere {
        center: [f32; 3],
        radius_mm: f32,
    },
}

/// Preview (fast, visual) or apply (printable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HollowKind {
    Preview,
    Apply,
}

#[derive(Debug, Clone)]
pub struct HollowRequest {
    pub kind: HollowKind,
    pub grid: GridRequest,
    /// Wall, fill and smoothing. Its `solid_areas` are ignored: use
    /// [`HollowRequest::solid_areas`].
    pub settings: HollowSettings,
    pub solid_areas: Vec<WorldSolidArea>,
    /// Apply only.
    pub drains: Vec<WorldDrain>,
    /// A session from an earlier job; reused when it matches.
    pub prepared: Option<Arc<PreparedHollow>>,
    /// Refuse grids with more cells than this.
    pub max_cells: Option<usize>,
}

impl Default for HollowRequest {
    fn default() -> Self {
        Self {
            kind: HollowKind::Preview,
            grid: GridRequest::default(),
            settings: HollowSettings::default(),
            solid_areas: Vec::new(),
            drains: Vec::new(),
            prepared: None,
            max_cells: None,
        }
    }
}

/// What a finished job hands back.
#[derive(Debug, Clone)]
pub struct HollowOutcome {
    /// The session, for the next job on this leaf.
    pub prepared: Arc<PreparedHollow>,
    /// The request's session was reused.
    pub reused_session: bool,
    pub stats: HollowStats,
    pub result: HollowResult,
}

#[derive(Debug, Clone)]
pub enum HollowResult {
    Preview {
        /// The cavity wall in the leaf's local frame, facing inward.
        cavity: IndexedMesh,
        infill: Option<IndexedMesh>,
        /// Removed voxels on the cavity wall (local frame), for solid-area editing.
        removed_voxel_centers: Vec<[f32; 3]>,
        removed_voxel_indices: Vec<u32>,
    },
    Apply {
        /// The printable hollow in the leaf's local frame, edge-manifold,
        /// carrying the original mesh's metadata.
        mesh: IndexedMesh,
        drains: Vec<DrainReport>,
    },
}

/// The engine's mesh of a leaf mesh, in the physical frame.
fn engine_mesh(mesh: &IndexedMesh, frame: &LeafFrame) -> hollow_df::IndexedMesh {
    hollow_df::IndexedMesh::new(
        mesh.vertices()
            .iter()
            .map(|v| {
                let p = frame.local_to_frame(*v.as_array());
                hollow_df::Vec3::new(p[0], p[1], p[2])
            })
            .collect(),
        mesh.faces()
            .iter()
            .map(|f| f.vertices.map(u32::from))
            .collect(),
    )
}

/// A leaf mesh of an engine mesh, back in the local frame.
fn leaf_mesh(
    mesh: &hollow_df::IndexedMesh,
    frame: &LeafFrame,
    meta: Option<MeshMetadata>,
) -> IndexedMesh {
    IndexedMesh::new(
        meta,
        mesh.positions
            .iter()
            .map(|p| Vertex::new(frame.frame_to_local(p.to_array())))
            .collect(),
        mesh.triangles
            .iter()
            .map(|&t| IndexedTriangle::from_raw_indices(t))
            .collect(),
    )
}

/// A cheap content hash of a mesh, in parallel.
fn mesh_hash(mesh: &IndexedMesh) -> u64 {
    use rayon::prelude::*;
    let mix = |h: u64, w: u32| (h ^ w as u64).wrapping_mul(0x0100_0000_01b3);
    let vertices: u64 = mesh
        .vertices()
        .par_chunks(1 << 14)
        .enumerate()
        .map(|(i, chunk)| {
            let mut h = 0xcbf2_9ce4_8422_2325u64 ^ i as u64;
            for v in chunk {
                for c in v.as_array() {
                    h = mix(h, c.to_bits());
                }
            }
            h.rotate_left((i % 63) as u32)
        })
        .reduce(|| 0, |a, b| a ^ b.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let faces: u64 = mesh
        .faces()
        .par_chunks(1 << 14)
        .enumerate()
        .map(|(i, chunk)| {
            let mut h = 0x8422_2325_cbf2_9ce4u64 ^ i as u64;
            for f in chunk {
                for v in f.vertices.map(u32::from) {
                    h = mix(h, v);
                }
            }
            h.rotate_left((i % 61) as u32)
        })
        .reduce(|| 0, |a, b| a ^ b.wrapping_mul(0xbf58_476d_1ce4_e5b9));
    vertices ^ faces.rotate_left(17)
}

/// Starts a hollow of leaf `leaf` on its own thread. Cancelling the handle
/// stops it inside the current stage.
pub fn start_hollow(
    scene: &Scene,
    leaf: NodeId,
    request: HollowRequest,
) -> Result<JobHandle<HollowOutcome>, SceneError> {
    scene.mesh_at_leaf(leaf)?;
    let mesh = scene
        .leaf_mesh_arc(leaf)
        .cloned()
        .ok_or(SceneError::NotALeaf { node_id: leaf })?;
    let world = scene.world_transform_of(leaf)?;
    let frame = LeafFrame::of(&world)?;
    Ok(JobHandle::spawn(
        move |worker: &JobWorker<HollowOutcome>| run_hollow(&mesh, &frame, &request, worker),
    ))
}

fn run_hollow(
    mesh: &IndexedMesh,
    frame: &LeafFrame,
    request: &HollowRequest,
    worker: &JobWorker<HollowOutcome>,
) -> Result<HollowOutcome, JobError> {
    let key = SessionKey {
        mesh: mesh_hash(mesh),
        triangles: mesh.faces().len(),
        stretch: frame.key(),
        voxel_bits: request.grid.voxel_size_mm.to_bits(),
        max_resolution: request.grid.max_resolution,
    };
    let reuse = request.prepared.as_ref().filter(|p| p.key == key).cloned();
    let stages = match request.kind {
        HollowKind::Preview => 2,
        HollowKind::Apply => 4 + u32::from(!request.drains.is_empty()),
    } + if reuse.is_some() { 0 } else { 4 };
    let done = AtomicU32::new(0);
    let progress = |stage: Stage| {
        worker.report_progress(JobProgress {
            phase: stage.label().to_owned(),
            completed: done.fetch_add(1, Ordering::Relaxed).min(stages),
            total: stages,
        });
    };
    let cancelled = || worker.is_cancelled();
    let ctl = Control::with_check(&cancelled, Some(&progress));
    let to_job = |e: HollowError| match e {
        HollowError::Cancelled => JobError::Cancelled,
        other => JobError::Failed(other.to_string()),
    };

    let reused_session = reuse.is_some();
    let prepared = match reuse {
        Some(prepared) => prepared,
        None => Arc::new(PreparedHollow {
            key,
            session: Session::build(
                engine_mesh(mesh, frame),
                &request.grid,
                request.max_cells,
                ctl,
            )
            .map_err(to_job)?,
        }),
    };
    let session = &prepared.session;

    let mut settings = request.settings.clone();
    settings.solid_areas = request
        .solid_areas
        .iter()
        .map(|area| match area {
            WorldSolidArea::Voxels(v) => SolidArea::Voxels(v.clone()),
            WorldSolidArea::Sphere { center, radius_mm } => SolidArea::Sphere {
                center: frame.world_to_frame(*center),
                radius_mm: *radius_mm,
            },
        })
        .collect();

    let (stats, result) = match request.kind {
        HollowKind::Preview => {
            let preview = session.preview(&settings, ctl).map_err(to_job)?;
            let centers = preview
                .removed_voxel_centers
                .chunks_exact(3)
                .map(|c| frame.frame_to_local([c[0], c[1], c[2]]))
                .collect();
            (
                preview.stats,
                HollowResult::Preview {
                    cavity: leaf_mesh(&preview.cavity, frame, None),
                    infill: preview.infill.as_ref().map(|m| leaf_mesh(m, frame, None)),
                    removed_voxel_centers: centers,
                    removed_voxel_indices: preview.removed_voxel_indices,
                },
            )
        }
        HollowKind::Apply => {
            let drains: Vec<DrainHole> = request
                .drains
                .iter()
                .map(|d| DrainHole {
                    center: frame.world_to_frame(d.center),
                    direction: frame.world_dir_to_frame(d.direction),
                    radius_mm: d.radius_mm,
                    radius_y_mm: d.radius_y_mm,
                    depth: d.depth,
                    outside_mm: d.outside_mm,
                })
                .collect();
            let applied = session.apply(&settings, &drains, ctl).map_err(to_job)?;
            (
                applied.stats,
                HollowResult::Apply {
                    mesh: leaf_mesh(&applied.mesh, frame, mesh.meta().clone()),
                    drains: applied.drains,
                },
            )
        }
    };
    worker.report_progress(JobProgress {
        phase: "Done".to_owned(),
        completed: stages,
        total: stages,
    });
    Ok(HollowOutcome {
        prepared,
        reused_session,
        stats,
        result,
    })
}

#[cfg(test)]
mod tests;

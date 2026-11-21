//! Runs the DragonFruit hollowing port (`hollow-engine`) on one leaf of a
//! SoulCrafted scene as a cancellable background job (BS-8, BS-29).
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
//!
//! A drain's plug (#318) is a model of its own, not part of the leaf: it
//! comes back standing up ([`HollowPlug`]), as it fits the hole at the
//! current pose, turned so it goes in straight down.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use hollow_engine::engine::{DrainReport, HollowStats};
use hollow_engine::{
    Control, DrainHole, GridRequest, HollowError, HollowSettings, Session, SolidArea, Stage,
};
use sc_geom_transport_inprocess::{JobError, JobHandle, JobProgress, JobWorker};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{NodeId, Scene, SceneError};

pub use hollow_engine;

/// The cavity this build's engine makes, as a number saved with each hollow
/// (#446). A later build that changes the cavity bumps it, so it can tell a
/// hollow made before the change from one it made itself, instead of
/// silently making the whole cavity over at the next drain.
///
/// 1: the exact wall (#265), the preview's cavity as the result (#29).
pub const HOLLOW_ENGINE_VERSION: u32 = 1;

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

    /// The stretch, row by row: what a hollow saves of its model's scale
    /// (#446).
    pub fn stretch_rows(&self) -> [f64; 9] {
        let mut out = [0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                out[i * 3 + j] = self.stretch[i][j];
            }
        }
        out
    }

    /// The stretch quantized: two poses with the same key share a voxel
    /// session (and a cavity); a move or a rotation keeps it, a scale not.
    pub fn stretch_key(&self) -> [i64; 9] {
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
    max_memory_bytes: usize,
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

    /// What `drain` (in world coordinates at the pose `world`) would do on
    /// the hollow `settings` make: [`Session::drain_report`] in the leaf's
    /// frame, the check an apply makes before it cuts, so a drain reported
    /// not reaching the cavity is one that apply leaves out. The caller says
    /// the session is this leaf mesh's (by identity: hashing the mesh per
    /// ask would cost more than the ask).
    ///
    /// `None` while the session holds no cavity for `settings`, or `world`
    /// scales the model otherwise than this session was voxelized for.
    pub fn drain_report(
        &self,
        world: &Transform3D,
        settings: &HollowSettings,
        drain: &WorldDrain,
    ) -> Option<DrainReport> {
        let frame = LeafFrame::of(world).ok()?;
        if frame.stretch_key() != self.key.stretch {
            return None;
        }
        self.session.drain_report(settings, &drain.in_frame(&frame))
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
    pub depth: hollow_engine::DrainDepth,
    pub outside_mm: f32,
    /// A cone's radius at the drain's end (#317); `None` a cylinder.
    pub inner_radius_mm: Option<f32>,
    /// Make a plug this much narrower across (#318); `None` none.
    pub plug_clearance_mm: Option<f32>,
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
            inner_radius_mm: hole.inner_radius_mm,
            plug_clearance_mm: hole.plug_clearance_mm,
        }
    }

    /// The engine's drain, in the leaf's physical frame.
    fn in_frame(&self, frame: &LeafFrame) -> DrainHole {
        DrainHole {
            center: frame.world_to_frame(self.center),
            direction: frame.world_dir_to_frame(self.direction),
            radius_mm: self.radius_mm,
            radius_y_mm: self.radius_y_mm,
            depth: self.depth,
            outside_mm: self.outside_mm,
            inner_radius_mm: self.inner_radius_mm,
            plug_clearance_mm: self.plug_clearance_mm,
        }
    }
}

/// A drain's plug (#318), as a model of its own.
#[derive(Debug, Clone)]
pub struct HollowPlug {
    /// Which drain, by its index in the request's.
    pub drain: usize,
    /// In world millimetres and orientation, as it fits the hole, turned so
    /// the drain's direction points straight down: the end that meets the
    /// cavity at the bottom, the one on the model's surface on top. Its
    /// axis is on `x = y = 0` and its lowest point at `z = 0`, so a
    /// translation stands it on the plate. `None` when it could not be made
    /// closed.
    pub mesh: Option<IndexedMesh>,
}

/// The orthogonal map that turns `from` (a unit vector) onto `to` (another),
/// by the shortest turn; half a turn about an axis across them when they
/// are opposite.
fn turn_onto(from: [f64; 3], to: [f64; 3]) -> M3 {
    let cross = [
        from[1] * to[2] - from[2] * to[1],
        from[2] * to[0] - from[0] * to[2],
        from[0] * to[1] - from[1] * to[0],
    ];
    let cos = from[0] * to[0] + from[1] * to[1] + from[2] * to[2];
    if cos < -1.0 + 1e-12 {
        // Opposite: half a turn about any axis across `from`.
        let helper = if from[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let d = helper[0] * from[0] + helper[1] * from[1] + helper[2] * from[2];
        let a = [0, 1, 2].map(|k| helper[k] - d * from[k]);
        let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        let a = a.map(|c| c / len);
        return [0, 1, 2].map(|i| [0, 1, 2].map(|j| 2.0 * a[i] * a[j] - f64::from(i == j)));
    }
    // Rodrigues: I + [v]x + [v]x² / (1 + cos).
    let v = cross;
    let k = 1.0 / (1.0 + cos);
    let vx = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
    let vx2 = mul(&vx, &vx);
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| f64::from(i == j) + vx[i][j] + vx2[i][j] * k))
}

/// A plug the engine made in the physical frame, as [`HollowPlug::mesh`]
/// has it: in world orientation (`R`, a mirror flipping its winding back
/// out), turned so `drain`'s direction points down, its axis on `z` and
/// its lowest point on `z = 0`.
fn standing_plug(
    plug: &hollow_engine::IndexedMesh,
    drain: &DrainHole,
    frame: &LeafFrame,
) -> IndexedMesh {
    let unit = |v: [f64; 3]| {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-300);
        v.map(|c| c / len)
    };
    let down = apply(&frame.rotation, drain.direction.map(f64::from));
    let turn = mul(&turn_onto(unit(down), [0.0, 0.0, -1.0]), &frame.rotation);
    let axis = apply(&turn, drain.center.map(f64::from));
    let turned: Vec<[f64; 3]> = plug
        .positions
        .iter()
        .map(|p| apply(&turn, [p.x, p.y, p.z].map(f64::from)))
        .collect();
    let floor = turned.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
    let vertices = turned
        .iter()
        .map(|p| Vertex::new([p[0] - axis[0], p[1] - axis[1], p[2] - floor].map(|c| c as f32)))
        .collect();
    let det = frame.rotation[0][0]
        * (frame.rotation[1][1] * frame.rotation[2][2]
            - frame.rotation[1][2] * frame.rotation[2][1])
        - frame.rotation[0][1]
            * (frame.rotation[1][0] * frame.rotation[2][2]
                - frame.rotation[1][2] * frame.rotation[2][0])
        + frame.rotation[0][2]
            * (frame.rotation[1][0] * frame.rotation[2][1]
                - frame.rotation[1][1] * frame.rotation[2][0]);
    let faces = plug
        .triangles
        .iter()
        .map(|&[a, b, c]| {
            IndexedTriangle::from_raw_indices(if det < 0.0 { [a, c, b] } else { [a, b, c] })
        })
        .collect();
    IndexedMesh::new(None, vertices, faces)
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
    /// What lies behind a curve drawn on screen (#179): its `origin` a
    /// world point, its `u`, `v`, `w` world directions, at the current pose.
    Lasso(hollow_engine::LassoRegion),
}

/// Preview (the cavity, as apply builds it) or apply (printable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HollowKind {
    Preview,
    Apply,
}

#[derive(Debug, Clone)]
pub struct HollowRequest {
    pub kind: HollowKind,
    pub grid: GridRequest,
    /// Wall and fill. Its `solid_areas` are ignored: use
    /// [`HollowRequest::solid_areas`].
    pub settings: HollowSettings,
    pub solid_areas: Vec<WorldSolidArea>,
    /// Cut by an apply; a preview shows the channels they clear through
    /// the infill (#339).
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
    },
    Apply {
        /// The printable hollow in the leaf's local frame, edge-manifold,
        /// carrying the original mesh's metadata.
        mesh: IndexedMesh,
        drains: Vec<DrainReport>,
        /// One per drain cut that asked for a plug (#318), in drain order.
        plugs: Vec<HollowPlug>,
    },
}

/// The engine's mesh of a leaf mesh, in the physical frame.
fn engine_mesh(mesh: &IndexedMesh, frame: &LeafFrame) -> hollow_engine::IndexedMesh {
    hollow_engine::IndexedMesh::new(
        mesh.vertices()
            .iter()
            .map(|v| {
                let p = frame.local_to_frame(*v.as_array());
                hollow_engine::Vec3::new(p[0], p[1], p[2])
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
    mesh: &hollow_engine::IndexedMesh,
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
    start_hollow_mesh(mesh, &world, request)
}

/// Starts a hollow of `mesh` posed by `world`, as [`start_hollow`] does for
/// a leaf. The GUI uses it to hollow a model's original mesh again (new
/// drains, other settings) after the leaf already shows a hollow.
pub fn start_hollow_mesh(
    mesh: Arc<IndexedMesh>,
    world: &Transform3D,
    request: HollowRequest,
) -> Result<JobHandle<HollowOutcome>, SceneError> {
    let frame = LeafFrame::of(world)?;
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
        stretch: frame.stretch_key(),
        voxel_bits: request.grid.voxel_size_mm.to_bits(),
        max_resolution: request.grid.max_resolution,
        max_memory_bytes: request.grid.max_memory_bytes,
    };
    let reuse = request.prepared.as_ref().filter(|p| p.key == key).cloned();
    let stages = match request.kind {
        HollowKind::Preview => 3,
        HollowKind::Apply => 5 + u32::from(!request.drains.is_empty()),
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
            WorldSolidArea::Lasso(region) => SolidArea::Lasso(hollow_engine::LassoRegion {
                origin: frame.world_to_frame(region.origin),
                u: frame.world_dir_to_frame(region.u),
                v: frame.world_dir_to_frame(region.v),
                w: frame.world_dir_to_frame(region.w),
                ..region.clone()
            }),
        })
        .collect();

    let (stats, result) = match request.kind {
        HollowKind::Preview => {
            let drains: Vec<DrainHole> = request.drains.iter().map(|d| d.in_frame(frame)).collect();
            let preview = session
                .preview_with_drains(&settings, &drains, ctl)
                .map_err(to_job)?;
            (
                preview.stats,
                HollowResult::Preview {
                    cavity: leaf_mesh(&preview.cavity, frame, None),
                    infill: preview.infill.as_ref().map(|m| leaf_mesh(m, frame, None)),
                },
            )
        }
        HollowKind::Apply => {
            let drains: Vec<DrainHole> = request.drains.iter().map(|d| d.in_frame(frame)).collect();
            let applied = session.apply(&settings, &drains, ctl).map_err(to_job)?;
            let plugs = applied
                .plugs
                .iter()
                .map(|plug| HollowPlug {
                    drain: plug.drain,
                    mesh: plug
                        .mesh
                        .as_ref()
                        .map(|m| standing_plug(m, &drains[plug.drain], frame)),
                })
                .collect();
            (
                applied.stats,
                HollowResult::Apply {
                    mesh: leaf_mesh(&applied.mesh, frame, mesh.meta().clone()),
                    drains: applied.drains,
                    plugs,
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

/// A mesh posed by a world transform, ready to measure drains drilled into
/// it as it is (#447): [`hollow_engine::DrillTarget`] in the leaf's physical
/// frame. Building it walks the mesh once (a ray tree): do it off the UI
/// thread.
#[derive(Debug)]
pub struct DrillProbe {
    target: hollow_engine::DrillTarget,
    stretch: [i64; 9],
}

/// Where the face is laid out for [`DrillProbe::drain_map`]: one sample a
/// cell of this side.
const DRAIN_MAP_CELL_MM: f32 = 2.0;
/// Rays a sample of the map looks around the air behind the wall with.
const DRAIN_MAP_RAYS: usize = 12;

impl DrillProbe {
    pub fn new(mesh: &IndexedMesh, world: &Transform3D) -> Result<Self, SceneError> {
        let frame = LeafFrame::of(world)?;
        Ok(Self {
            target: hollow_engine::DrillTarget::new(engine_mesh(mesh, &frame)),
            stretch: frame.stretch_key(),
        })
    }

    /// How many cavities are closed all round (#130).
    pub fn sealed_cavities(&self) -> usize {
        self.target.sealed_cavities()
    }

    /// Where on the model a drain would work best (#455), at the pose
    /// `world`: the faces of the outside (by index) where the wall behind is
    /// thin, at most `thin_factor` times the thinnest found, and that are
    /// low on the model (its bottom quarter, by height), as the resin runs
    /// out of a hole near the plate; none within `clear_mm` of `drains`
    /// (world points), which already drain there, nor those `avoid` marks.
    /// `None` when `world` scales the model otherwise than the probe was
    /// made for. A guide only: it places nothing.
    pub fn drain_map(
        &self,
        world: &Transform3D,
        thin_factor: f32,
        drains: &[[f32; 3]],
        clear_mm: f32,
        avoid: &dyn Fn(usize) -> bool,
    ) -> Option<Vec<u32>> {
        use std::collections::HashMap;
        let frame = LeafFrame::of(world).ok()?;
        if frame.stretch_key() != self.stretch {
            return None;
        }
        let mesh = self.target.mesh();
        let to_world = |p: hollow_engine::Vec3| {
            let q = apply(&frame.rotation, [p.x, p.y, p.z].map(f64::from));
            [0, 1, 2].map(|k| (q[k] + frame.translation[k]) as f32)
        };
        let (mut low, mut high) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in &mesh.positions {
            let z = to_world(*p)[2];
            low = low.min(z);
            high = high.max(z);
        }
        let cutoff = low + 0.25 * (high - low);
        // One sample a cell, as (its depth, whether it is low enough), and
        // which cell each face of the outside is in.
        let mut samples: HashMap<[i32; 3], Option<f32>> = HashMap::new();
        let mut cells: Vec<Option<[i32; 3]>> = vec![None; mesh.triangles.len()];
        for (f, tri) in mesh.triangles.iter().enumerate() {
            if !self.target.faces_out(f) || avoid(f) {
                continue;
            }
            let [a, b, c] = tri.map(|v| mesh.positions[v as usize]);
            let centre = a.add(b).add(c).scale(1.0 / 3.0);
            let w = to_world(centre);
            if w[2] > cutoff
                || drains.iter().any(|d| {
                    (0..3).map(|k| (d[k] - w[k]).powi(2)).sum::<f32>() < clear_mm * clear_mm
                })
            {
                continue;
            }
            let cell =
                [centre.x, centre.y, centre.z].map(|v| (v / DRAIN_MAP_CELL_MM).floor() as i32);
            cells[f] = Some(cell);
            samples.entry(cell).or_insert_with(|| {
                let normal = b.sub(a).cross(c.sub(a));
                let inward = [-normal.x, -normal.y, -normal.z];
                self.target
                    .wall_depth(centre.to_array(), inward, DRAIN_MAP_RAYS)
            });
        }
        let thinnest = samples
            .values()
            .flatten()
            .copied()
            .fold(f32::INFINITY, f32::min);
        if !thinnest.is_finite() {
            return Some(Vec::new());
        }
        let limit = thinnest * thin_factor.max(1.0);
        Some(
            cells
                .iter()
                .enumerate()
                .filter_map(|(f, cell)| {
                    let depth = (*samples.get(cell.as_ref()?)?)?;
                    (depth <= limit).then_some(f as u32)
                })
                .collect(),
        )
    }

    /// Whether the mesh is closed, as drilling it needs.
    pub fn is_closed(&self) -> bool {
        self.target.is_closed()
    }

    /// What `drain` (world, at the pose `world`) would do drilled into the
    /// mesh; `None` when `world` scales the model otherwise than the probe
    /// was made for.
    pub fn report(&self, world: &Transform3D, drain: &WorldDrain) -> Option<DrainReport> {
        let frame = LeafFrame::of(world).ok()?;
        (frame.stretch_key() == self.stretch).then(|| self.target.report(&drain.in_frame(&frame)))
    }
}

/// How many cavities of `mesh` are closed all round (#130): each traps the
/// resin in it until a drain opens it.
pub fn sealed_cavities(mesh: &IndexedMesh) -> usize {
    hollow_engine::sealed_cavities(&engine_mesh(
        mesh,
        &LeafFrame::of(&Transform3D::Identity).unwrap_or_else(|_| unreachable!()),
    ))
}

/// What a drill job hands back.
#[derive(Debug, Clone)]
pub struct DrillOutcome {
    /// The mesh with the drains cut, in the leaf's local frame, carrying
    /// the original's metadata; the very mesh when none was cut.
    pub mesh: IndexedMesh,
    /// One per drain asked for, in order.
    pub drains: Vec<DrainReport>,
    /// One per drain cut that asked for a plug, in drain order.
    pub plugs: Vec<HollowPlug>,
}

/// Starts drilling `drains` (world, at the pose `world`) into `mesh` as it
/// is, on its own thread (#447): no cavity is computed, so a hollow from a
/// file takes drains too. A mesh that is not closed fails; a drain whose
/// axis does not reach a closed cavity is reported and left uncut.
///
/// `probe`, when it is of this very mesh at this stretch, is used as it is:
/// the ray tree and the air map are not built again.
pub fn start_drill_mesh(
    mesh: Arc<IndexedMesh>,
    world: &Transform3D,
    drains: Vec<WorldDrain>,
    probe: Option<Arc<DrillProbe>>,
) -> Result<JobHandle<DrillOutcome>, SceneError> {
    let frame = LeafFrame::of(world)?;
    let probe = probe.filter(|p| p.stretch == frame.stretch_key());
    Ok(JobHandle::spawn(move |worker: &JobWorker<DrillOutcome>| {
        let total = 3;
        let report = |completed: u32, stage: Stage| {
            worker.report_progress(JobProgress {
                phase: stage.label().to_owned(),
                completed,
                total,
            });
        };
        let step = AtomicU32::new(1);
        let progress =
            |stage: Stage| report(step.fetch_add(1, Ordering::Relaxed).min(total), stage);
        let cancelled = || worker.is_cancelled();
        let ctl = Control::with_check(&cancelled, Some(&progress));
        report(0, Stage::Stabilize);
        let built;
        let target = match &probe {
            Some(probe) => &probe.target,
            None => {
                built = hollow_engine::DrillTarget::new(engine_mesh(&mesh, &frame));
                &built
            }
        };
        let holes: Vec<DrainHole> = drains.iter().map(|d| d.in_frame(&frame)).collect();
        let drilled = target.drill(&holes, ctl).map_err(|e| match e {
            HollowError::Cancelled => JobError::Cancelled,
            other => JobError::Failed(other.to_string()),
        })?;
        let plugs = drilled
            .plugs
            .iter()
            .map(|plug| HollowPlug {
                drain: plug.drain,
                mesh: plug
                    .mesh
                    .as_ref()
                    .map(|m| standing_plug(m, &holes[plug.drain], &frame)),
            })
            .collect();
        let cut = drilled.drains.iter().any(|d| d.reaches_cavity);
        let out = if cut {
            leaf_mesh(&drilled.mesh, &frame, mesh.meta().clone())
        } else {
            (*mesh).clone()
        };
        report(total, Stage::Validate);
        Ok(DrillOutcome {
            mesh: out,
            drains: drilled.drains,
            plugs,
        })
    }))
}

#[cfg(test)]
mod tests;

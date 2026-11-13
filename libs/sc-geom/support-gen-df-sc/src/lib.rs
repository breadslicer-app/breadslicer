//! Runs the DragonFruit support port (`support_gen_df::pipeline`) on a
//! SoulCrafted scene as a cancellable background job.
//!
//! Every mesh leaf of the scene (CSG refused, as the slicer refuses it) is one
//! model, baked to world space. A mesh this crate produced earlier carries
//! [`DF_SUPPORT_MESH_NAME`] and is skipped, so a second run does not support
//! the previous supports. The result is one support set per model (#197):
//! the slicer's tessellation of that model's supports and of a raft drawn
//! from its own roots (`buildSupportAndRaftWorldTriangles`), welded by exact
//! position into one [`IndexedMesh`], and the session that edits them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use sc_geom_transport_inprocess::{JobError, JobHandle, JobProgress, JobWorker};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{CsgPolicy, NodeId, Scene, SceneError, bake_mesh_to_world};
use support_gen_df::features::slicing::smooth_shafts::{ShaftStyle, SmoothShaftParams};
use support_gen_df::perf::{PerfEntry, PerfFrame};
use support_gen_df::pipeline::{
    CancelToken, ModelInput, PipelineError, PipelineSettings, Stage, plan_supports_with,
    prescan_models,
};
use support_gen_df::supports::rafts::crenelated::geometry::model_plate_footprint::ModelTransform;
use support_gen_df::supports::support_type_registry::get_support_type_descriptor;
use support_gen_df::supports::types::SupportState;
use support_gen_df::three::Vec3;
use support_gen_df::three::buffer_geometry::BufferGeometry;

mod compare;
mod document;
mod hand_log;
mod options;
mod session;
mod shift;
pub mod zone_presets;

pub use document::{DF_SUPPORT_DOCUMENT_VERSION, DF_SUPPORT_GENERATOR, RestoreError, RestoredSet};
pub use hand_log::{HandEditKind, KeptEdits, LostEdit, LostWhy};
pub use options::{DEFAULT_SUPPORT_LIFT_MM, DfSupportOptions, RaftMode, recommended_lift_mm};
pub use session::{
    AddNote, AddOptions, BraceEnd, COVERED_AGREEMENT, COVERED_SURFACE_MM, DfSupportSession,
    EditError, EditPreview, REVIEW_NUDGE_MM, SupportContact, SupportJoint, SupportPiece,
    SupportPieceKind,
};
pub use support_gen_df::native::overhang::{
    is_overhang_normal_z, overhang_and_stability_from_soup, overhang_normal_z_limit,
};
pub use support_gen_df::pipeline::ScanCache;
pub use support_gen_df::supports::auto_bracing::auto_brace::HandBraceRefusal;
pub use support_gen_df::supports::auto_support::zone_rules::{
    AreaBand, BaseTip, OverhangRule, StabilizationRule, SupportSize, ZoneRules,
};

/// The name the generated support mesh carries.
pub const DF_SUPPORT_MESH_NAME: &str = "Supports";

/// The name support meshes carried before, still found in saved projects.
const LEGACY_SUPPORT_MESH_NAME: &str = "Supports (DragonFruit)";

/// Whether a mesh named `name` is generated supports, under the current name
/// or the one projects saved earlier carry.
pub fn is_support_mesh_name(name: &str) -> bool {
    name == DF_SUPPORT_MESH_NAME || name == LEGACY_SUPPORT_MESH_NAME
}

/// Whether `mesh` is generated supports.
pub fn is_support_mesh(mesh: &IndexedMesh) -> bool {
    mesh.meta()
        .as_ref()
        .and_then(|m| m.name.as_deref())
        .is_some_and(is_support_mesh_name)
}

pub use sc_mesh_core::presupported::{
    IMPORTED_SUPPORTS_NAME, is_imported_supports, is_imported_supports_name,
};

/// Whether a mesh named `name` is supports of any kind: generated here, or
/// imported with a pre-supported file (#258).
pub fn is_any_support_mesh_name(name: &str) -> bool {
    is_support_mesh_name(name) || is_imported_supports_name(name)
}

/// Whether `mesh` is supports of any kind: generated here, or imported with
/// a pre-supported file (#258). Neither is a model: a run supports neither,
/// and neither is hollowed, oriented or arranged on its own.
pub fn is_any_support_mesh(mesh: &IndexedMesh) -> bool {
    is_support_mesh(mesh) || is_imported_supports(mesh)
}

/// What the run is asked for.
#[derive(Debug, Clone)]
pub struct DfSupportRequest {
    /// The slicing layer height the island scan bands by.
    pub layer_height_mm: f64,
    /// The printer's pixel pitch (X, Y) in mm, kept with the settings a set
    /// was made from.
    pub pixel_pitch_mm: Option<[f64; 2]>,
    /// Scans a [`start_df_prescan`] already made. Models it does not match
    /// (moved since, other settings) are scanned by the run as usual; the
    /// result is the same either way.
    pub prescanned: Option<Arc<ScanCache>>,
    /// The settings a person picked (a preset); ignored when `settings` is
    /// given.
    pub options: DfSupportOptions,
    /// Everything else DragonFruit reads from its panels and stores.
    pub settings: Option<PipelineSettings>,
    /// The models to support, by leaf, in any order; `None` for every model
    /// of the scene. Leaves that are not models are skipped.
    pub models: Option<Vec<NodeId>>,
    /// Models whose supports are made again keeping the hand edits of their
    /// set (#408): by leaf, the session of the set before. Each such model's
    /// new set gets those edits put back where they still fit, and says how
    /// it went ([`DfSupportSet::kept`]).
    pub keep_edits: Vec<(NodeId, DfSupportSession)>,
}

impl Default for DfSupportRequest {
    fn default() -> Self {
        DfSupportRequest {
            layer_height_mm: 0.05,
            pixel_pitch_mm: None,
            prescanned: None,
            options: DfSupportOptions::default(),
            settings: None,
            models: None,
            keep_edits: Vec::new(),
        }
    }
}

/// One model's supports from a run (#197).
#[derive(Debug, Clone)]
pub struct DfSupportSet {
    /// The model's leaf.
    pub model: NodeId,
    /// Its supports and raft as one welded mesh, `None` when nothing was
    /// placed on it.
    pub mesh: Option<IndexedMesh>,
    /// Its supports, kept for editing by hand.
    pub session: DfSupportSession,
    /// For a model of [`DfSupportRequest::keep_edits`]: which hand edits of
    /// its old set were put back.
    pub kept: Option<KeptEdits>,
}

/// What a finished run hands back.
#[derive(Debug, Clone)]
pub struct DfSupportOutcome {
    /// One set per model the run covered, in scene order.
    pub sets: Vec<DfSupportSet>,
    /// The support state the plans left, every model's.
    pub support: SupportState,
    /// Models the run covered, and the islands it found on them.
    pub models: usize,
    pub islands: usize,
    /// Placed supports by type (`trunk`, `branch`, ...), summed over models.
    pub placed: Vec<(String, u64)>,
    pub triangles: usize,
    /// Where the run spent its time.
    pub timings: PerfFrame,
}

impl DfSupportOutcome {
    /// Every set's mesh in one, `None` when nothing was placed: for a caller
    /// that keeps the supports as a single object (the CLI).
    pub fn merged_mesh(&self) -> Option<IndexedMesh> {
        let meshes: Vec<&IndexedMesh> = self.sets.iter().filter_map(|s| s.mesh.as_ref()).collect();
        if meshes.is_empty() {
            return None;
        }
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        for mesh in &meshes {
            let offset = vertices.len() as u32;
            vertices.extend_from_slice(mesh.vertices());
            faces.extend(mesh.faces().iter().map(|f| {
                IndexedTriangle::from_raw_indices(f.vertices.map(|v| u32::from(v) + offset))
            }));
        }
        Some(IndexedMesh::new(meshes[0].meta().clone(), vertices, faces))
    }
}

/// Whether a run takes the mesh of a leaf as a model: it is not supports,
/// made here or imported, and not empty.
fn is_model_mesh(mesh: &IndexedMesh) -> bool {
    !is_any_support_mesh(mesh) && !mesh.faces().is_empty()
}

/// One model per mesh leaf, baked to world space; previous DragonFruit
/// support meshes are left out.
pub fn scene_models(scene: &Scene) -> Result<Vec<ModelInput>, SceneError> {
    Ok(scene_model_leaves(scene, None)?
        .into_iter()
        .map(|(_, model)| model)
        .collect())
}

/// [`scene_models`] with the leaf of each, only the leaves `only` names
/// when given.
pub fn scene_model_leaves(
    scene: &Scene,
    only: Option<&[NodeId]>,
) -> Result<Vec<(NodeId, ModelInput)>, SceneError> {
    let mut models = Vec::new();
    scene.for_each_expanded_leaf(CsgPolicy::Refuse, |id, world, mesh| {
        if !is_model_mesh(mesh) || only.is_some_and(|only| !only.contains(&id)) {
            return Ok::<(), SceneError>(());
        }
        let baked = bake_mesh_to_world(mesh, world)?;
        models.push((
            id,
            model_from_world_mesh(format!("node-{}", id.get()), &baked),
        ));
        Ok(())
    })?;
    Ok(models)
}

/// Where the leaf `leaf` stands among the models of `scene`, in the order
/// [`scene_models`] lists them: the index a session made on the scene knows
/// it by. `None` when it is not a model, or stands in the scene more than
/// once.
pub fn model_index(scene: &Scene, leaf: NodeId) -> Option<usize> {
    let mut count = 0;
    let mut found = Vec::new();
    scene
        .for_each_expanded_leaf(CsgPolicy::Refuse, |id, _, mesh| {
            if is_model_mesh(mesh) {
                if id == leaf {
                    found.push(count);
                }
                count += 1;
            }
            Ok::<(), SceneError>(())
        })
        .ok()?;
    match found[..] {
        [index] => Some(index),
        _ => None,
    }
}

/// A world-space mesh as a model: the pipeline re-centres a model on its
/// bounding box and then applies its transform, so the transform here only
/// moves it back.
pub fn model_from_world_mesh(id: String, mesh: &IndexedMesh) -> ModelInput {
    let position: Vec<f32> = mesh.vertices().iter().flat_map(|v| *v.as_array()).collect();
    let index: Vec<u32> = mesh
        .faces()
        .iter()
        .flat_map(|f| f.vertices.map(u32::from))
        .collect();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in position.as_chunks::<3>().0 {
        for k in 0..3 {
            min[k] = min[k].min(v[k]);
            max[k] = max[k].max(v[k]);
        }
    }
    let center = Vec3::new(
        (min[0] as f64 + max[0] as f64) * 0.5,
        (min[1] as f64 + max[1] as f64) * 0.5,
        (min[2] as f64 + max[2] as f64) * 0.5,
    );
    ModelInput {
        id,
        geometry: Arc::new(BufferGeometry {
            position: Some(position),
            index: Some(index),
            ..Default::default()
        }),
        transform: ModelTransform {
            position: center,
            rotation: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
        },
        visible: true,
    }
}

/// How far below the plate a vertex may round and still be put on it.
///
/// The tessellation rotates each frustum into place, and a base that should
/// sit at `Z = 0` comes out a few 1e-16 mm below it; SoulCrafted's viewport
/// (and the build-volume check) reads any vertex under `Z = 0` as the object
/// breaching the plate. DragonFruit's own slicer clips there, so it never
/// cared. The port's output stays exact; only this scene mesh is snapped.
pub const PLATE_SNAP_MM: f32 = 1e-4;

/// Vertices of one piece closer than this are the same vertex. The
/// tessellation interpolates some corners (DragonFruit's crenelated wall
/// lerps along each edge), and `t = 1` does not always land on the next
/// corner to the last bit: a 1e-15 mm crack the rasteriser never saw, but an
/// open edge to the slicer's manifold check. Far below any print resolution.
pub const WELD_TOLERANCE_MM: f32 = 1e-5;

/// Welds positions within [`WELD_TOLERANCE_MM`] on a grid of that cell
/// size, looking in the neighbouring cells too so two points either side of
/// a cell boundary still meet.
#[derive(Default)]
struct Welder {
    cells: HashMap<[i64; 3], Vec<u32>, ahash::RandomState>,
}

impl Welder {
    fn index_of(&mut self, p: [f32; 3], vertices: &mut Vec<Vertex>) -> u32 {
        let cell = p.map(|v| (v / WELD_TOLERANCE_MM).floor() as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let key = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                    if let Some(ids) = self.cells.get(&key) {
                        for &id in ids {
                            let q = vertices[id as usize].as_array();
                            let d2 = (0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>();
                            if d2 <= WELD_TOLERANCE_MM * WELD_TOLERANCE_MM {
                                return id;
                            }
                        }
                    }
                }
            }
        }
        vertices.push(Vertex::new(p));
        let id = (vertices.len() - 1) as u32;
        self.cells.entry(cell).or_default().push(id);
        id
    }
}

/// One piece of a soup welded on its own: its vertices and its triangles,
/// numbered from 0.
fn weld_piece(piece: &[f32]) -> (Vec<Vertex>, Vec<[u32; 3]>) {
    let mut vertices = Vec::new();
    let mut faces = Vec::with_capacity(piece.len() / 9);
    let mut welder = Welder::default();
    for tri in piece.as_chunks::<9>().0 {
        let mut ids = [0u32; 3];
        for (k, p) in tri.as_chunks::<3>().0.iter().enumerate() {
            let z = if p[2] <= 0.0 && p[2] > -PLATE_SNAP_MM {
                0.0
            } else {
                p[2]
            };
            ids[k] = welder.index_of([p[0], p[1], z], &mut vertices);
        }
        if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
            faces.push(ids);
        }
    }
    (vertices, faces)
}

/// A triangle soup welded into one mesh **piece by piece**: each closed piece
/// of `parts` (`soup.len()` at each piece's end) gets its own vertices, welded
/// within [`WELD_TOLERANCE_MM`] inside it, so pieces that touch or overlap never share
/// an edge and the mesh stays edge-manifold (the slicer's precondition). A
/// piece identical to an earlier one is dropped. Vertices a rounding error
/// below the plate are put on it ([`PLATE_SNAP_MM`]); triangles the weld
/// collapses are dropped.
///
/// The pieces being apart, they are welded in parallel and joined in their
/// order (#3): the mesh is the one welding them in turn gives, several times
/// sooner (a set of 600k triangles took ~0.6 s, the most of a hand click).
pub fn weld_parts(soup: &[f32], parts: &[usize], name: &str) -> Option<IndexedMesh> {
    use rayon::prelude::*;
    use std::hash::{BuildHasher, Hash, Hasher};

    // The pieces in order, each exact repeat of an earlier one left out:
    // pieces are told apart by their bits, through a hash of them.
    let same_bits = |a: &[f32], b: &[f32]| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
    };
    let hasher = ahash::RandomState::with_seeds(1, 2, 3, 4);
    let mut kept: Vec<&[f32]> = Vec::new();
    let mut seen: HashMap<u64, Vec<usize>, ahash::RandomState> = HashMap::default();
    let mut start = 0;
    for &end in parts.iter().chain(std::iter::once(&soup.len())) {
        let piece = &soup[start..end.max(start)];
        start = end.max(start);
        if piece.is_empty() {
            continue;
        }
        let mut h = hasher.build_hasher();
        for v in piece {
            v.to_bits().hash(&mut h);
        }
        let same = seen.entry(h.finish()).or_default();
        if same.iter().any(|&k| same_bits(kept[k], piece)) {
            continue;
        }
        same.push(kept.len());
        kept.push(piece);
    }
    let welded: Vec<(Vec<Vertex>, Vec<[u32; 3]>)> =
        kept.par_iter().map(|piece| weld_piece(piece)).collect();
    let (vertex_count, face_count) = welded
        .iter()
        .fold((0, 0), |(v, f), (vs, fs)| (v + vs.len(), f + fs.len()));
    if face_count == 0 {
        return None;
    }
    let mut vertices: Vec<Vertex> = Vec::with_capacity(vertex_count);
    let mut faces: Vec<IndexedTriangle> = Vec::with_capacity(face_count);
    for (piece_vertices, piece_faces) in welded {
        let offset = vertices.len() as u32;
        vertices.extend(piece_vertices);
        faces.extend(
            piece_faces
                .into_iter()
                .map(|f| IndexedTriangle::from_raw_indices(f.map(|i| i + offset))),
        );
    }
    Some(IndexedMesh::new(
        Some(MeshMetadata {
            name: Some(name.to_owned()),
            ..Default::default()
        }),
        vertices,
        faces,
    ))
}

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::WorldGeometry => "Preparing models",
        Stage::ScanIslands => "Scanning islands",
        Stage::DeriveIslands => "Deriving islands",
        Stage::PlanSupports => "Placing supports",
        Stage::BuildRafts => "Building rafts",
        Stage::Tessellate => "Tessellating supports",
    }
}

static RUN: AtomicU64 = AtomicU64::new(0);

/// Starts the run on its own thread. Cancelling the handle stops it at the
/// next stage boundary.
pub fn start_df_supports(
    scene: &Scene,
    request: DfSupportRequest,
) -> Result<JobHandle<DfSupportOutcome>, SceneError> {
    // Baking a dense model takes a while: done on the worker, so the caller
    // (the UI thread) only clones the graph, whose meshes are shared.
    let scene = scene.clone();
    let run = RUN.fetch_add(1, Ordering::Relaxed) + 1;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    Ok(JobHandle::spawn(
        move |worker: &JobWorker<DfSupportOutcome>| {
            let models = scene_model_leaves(&scene, request.models.as_deref())
                .map_err(|e| JobError::Failed(e.to_string()))?;
            run_df_supports(&models, &request, worker, format!("df{stamp}-{run}"))
        },
    ))
}

/// A session of `model` with no support on it yet, made as `request` says, for
/// a set born from its first tip (#444): nothing runs and nothing is scanned.
/// `None` when `model` is no model of `scene`.
pub fn empty_df_session(
    scene: &Scene,
    model: NodeId,
    request: &DfSupportRequest,
) -> Result<Option<DfSupportSession>, SceneError> {
    let leaves = scene_model_leaves(scene, Some(&[model]))?;
    let run = RUN.fetch_add(1, Ordering::Relaxed) + 1;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    Ok((!leaves.is_empty()).then(|| {
        DfSupportSession::new(
            leaves.into_iter().map(|(_, input)| input).collect(),
            settings_for(request),
            request
                .settings
                .is_none()
                .then(|| SettingsRecipe::of(request)),
            SupportState::default(),
            format!("df{stamp}-{run}"),
        )
    }))
}

/// The contact diameter of the tips a set made as `request` says places.
pub fn tip_diameter_for(request: &DfSupportRequest) -> f64 {
    session::tip_contact_diameter(&settings_for(request))
}

/// The pipeline settings a request runs with: its explicit `settings`, or
/// SoulCrafted's defaults with the person's `options` applied. Public for a
/// caller that runs the pipeline itself with the settings a run would use,
/// as the support scorecard does to read the plan's analytics (#234).
pub fn settings_for(request: &DfSupportRequest) -> PipelineSettings {
    match &request.settings {
        Some(settings) => settings.clone(),
        None => SettingsRecipe::of(request).settings(),
    }
}

/// What a request's settings are made from when it gives none explicitly:
/// enough to make them again, so a saved session rebuilds with the settings
/// it ran with.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsRecipe {
    pub(crate) layer_height_mm: f64,
    pub(crate) pixel_pitch_mm: Option<[f64; 2]>,
    pub(crate) options: DfSupportOptions,
}

impl SettingsRecipe {
    fn of(request: &DfSupportRequest) -> Self {
        SettingsRecipe {
            layer_height_mm: request.layer_height_mm,
            pixel_pitch_mm: request.pixel_pitch_mm,
            options: request.options.clone(),
        }
    }

    /// SoulCrafted's defaults with the options applied.
    pub(crate) fn settings(&self) -> PipelineSettings {
        let mut settings = PipelineSettings {
            // SoulCrafted draws smooth tubes with bicones by default.
            shaft_style: ShaftStyle::Smooth(SmoothShaftParams::default()),
            // Walls that slice cleanly on trimmed footprints.
            robust_raft_geometry: true,
            ..PipelineSettings::defaults(self.layer_height_mm)
        };
        self.options.apply(&mut settings);
        // An island under a slanted wall lands on the next surface (#445).
        settings.supports.mesh_to_mesh.every_landing = true;
        // A fan looks near its tip, not through every shaft of the run (#445).
        settings.supports.auto_support.near_fans = true;
        // A long island branch hangs from a neighbour when it can (#464).
        settings.supports.auto_support.rehang_island_branches = true;
        // A candidate's tip stays on its own surface (#500).
        settings.supports.auto_support.tips_on_own_surface = true;
        // No support's tip cone goes into the model before its contact (#500).
        settings.supports.auto_support.tip_cones_clear = true;
        // A spine between the rays of a brace is no way through (#445).
        settings.supports.auto_bracing.exact_clearance = true;
        // A tall or bent shaft is braced where it stands above the rest (#464).
        settings.supports.auto_bracing.brace_tall_shafts = true;
        settings
    }
}

/// What a prescan hands back.
#[derive(Debug, Clone)]
pub struct DfPrescan {
    pub scans: Arc<ScanCache>,
    pub models: usize,
    pub elapsed: Duration,
}

/// The pool prescans run in: half the cores, so a scan running while the
/// user works leaves the rest to the viewport.
fn prescan_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads((cores / 2).max(1))
            .thread_name(|i| format!("df-prescan-{i}"))
            .build()
            .expect("prescan thread pool")
    })
}

/// Starts the geometry-only part of a run (the island and overhang scans) on
/// a worker, in a pool of half the cores, for a later [`start_df_supports`]
/// to reuse through [`DfSupportRequest::prescanned`]. `request` must carry
/// the settings that run will use, or the scans will not match; its own
/// `prescanned`, when set, is an earlier prescan whose models are reused
/// where they still match. Cancelling stops it between models.
pub fn start_df_prescan(scene: &Scene, request: DfSupportRequest) -> JobHandle<DfPrescan> {
    let scene = scene.clone();
    JobHandle::spawn(move |worker: &JobWorker<DfPrescan>| {
        let started = Instant::now();
        let settings = settings_for(&request);
        prescan_pool().install(|| {
            let models = scene_models(&scene).map_err(|e| JobError::Failed(e.to_string()))?;
            let mut scans = ScanCache::default();
            for model in &models {
                if worker.is_cancelled() {
                    return Err(JobError::Cancelled);
                }
                let previous = request.prescanned.as_deref();
                match prescan_models(
                    std::slice::from_ref(model),
                    &settings,
                    previous,
                    &CancelToken::new(),
                ) {
                    Ok(one) => scans.extend(one),
                    Err(PipelineError::Cancelled) => return Err(JobError::Cancelled),
                    Err(e) => return Err(JobError::Failed(e.to_string())),
                }
            }
            Ok(DfPrescan {
                scans: Arc::new(scans),
                models: models.len(),
                elapsed: started.elapsed(),
            })
        })
    })
}

fn run_df_supports(
    leaves: &[(NodeId, ModelInput)],
    request: &DfSupportRequest,
    worker: &JobWorker<DfSupportOutcome>,
    id_prefix: String,
) -> Result<DfSupportOutcome, JobError> {
    let settings = settings_for(request);
    let models: Vec<ModelInput> = leaves.iter().map(|(_, m)| m.clone()).collect();
    let models = &models[..];
    let n = models.len();
    // Reports: geometry per model, three per model, then each set drawn.
    let total = (5 * n) as u32;
    let mut completed = 0u32;
    let cancel = CancelToken::new();
    let mut next = 0u64;
    let mut new_id = || {
        next += 1;
        format!("{id_prefix}-{next}")
    };
    // Each model's supports and raft are drawn below, set by set: the
    // pipeline's scene-wide rafts and tessellation would go unused.
    let result = plan_supports_with(
        models,
        &SupportState::default(),
        &settings,
        request.prescanned.as_deref(),
        &mut new_id,
        &mut |p| {
            if worker.is_cancelled() {
                cancel.cancel();
            }
            worker.report_progress(JobProgress {
                phase: format!("{} ({}/{})", stage_label(p.stage), p.model + 1, p.models),
                completed,
                total,
            });
            completed += 1;
        },
        &cancel,
    );
    let mut out = match result {
        Ok(out) => out,
        Err(PipelineError::Cancelled) => return Err(JobError::Cancelled),
        Err(e) => return Err(JobError::Failed(e.to_string())),
    };
    let mut placed: Vec<(String, u64)> = Vec::new();
    let mut islands = 0;
    for m in &out.models {
        islands += m.islands.filtered.len();
        if let Some(plan) = &m.plan {
            for (&kind, &count) in &plan.result.placed {
                if count <= 0 {
                    continue;
                }
                let name = get_support_type_descriptor(kind).singular.to_owned();
                match placed.iter_mut().find(|(k, _)| *k == name) {
                    Some(entry) => entry.1 += count as u64,
                    None => placed.push((name, count as u64)),
                }
            }
        }
    }
    let drawing = Instant::now();
    let session = DfSupportSession::new(
        models.to_vec(),
        settings.clone(),
        request
            .settings
            .is_none()
            .then(|| SettingsRecipe::of(request)),
        out.support.clone(),
        id_prefix.clone(),
    );
    // The plans' BVHs and distance fields (filled where they routed), so the
    // first hover preview is quick without building them again.
    session.adopt_bvhs(out.bvhs);
    session.warm();
    if let Ok(mut fields) = out.distance_fields.lock() {
        session.adopt_sdfs(
            fields
                .drain(..)
                .enumerate()
                .filter_map(|(i, f)| f.map(|f| (i, f)))
                .collect(),
        );
    }
    // Each model's supports and raft are drawn on their own.
    let mut sets = Vec::with_capacity(n);
    for (i, ((model, _), session)) in leaves.iter().zip(session.split_by_model()).enumerate() {
        if worker.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        worker.report_progress(JobProgress {
            phase: format!("{} ({}/{n})", stage_label(Stage::Tessellate), i + 1),
            completed,
            total,
        });
        completed += 1;
        // The hand edits of the model's old set go back on (#408), before
        // the set is drawn: a model the run found nothing on may still
        // carry supports added by hand.
        let (session, kept) = match request.keep_edits.iter().find(|(m, _)| m == model) {
            Some((_, old)) => {
                worker.report_progress(JobProgress {
                    phase: format!("Putting back the edits made by hand ({}/{n})", i + 1),
                    completed,
                    total,
                });
                let (session, kept) = session
                    .with_hand_edits_of(old, &|| worker.is_cancelled())
                    .ok_or(JobError::Cancelled)?;
                (session, Some(kept))
            }
            None => (session, None),
        };
        let mesh = if session.is_empty() {
            None
        } else {
            session
                .mesh()
                .map_err(|e| JobError::Failed(e.to_string()))?
        };
        sets.push(DfSupportSet {
            model: *model,
            mesh,
            session,
            kept,
        });
    }
    // The sets are part of the run's time, as the pipeline's own drawing was.
    let drawn = drawing.elapsed();
    out.timings.total += drawn;
    out.timings.entries.push(PerfEntry {
        label: "stage:draw-sets".to_owned(),
        total: drawn,
        count: 1,
    });
    Ok(DfSupportOutcome {
        triangles: sets
            .iter()
            .filter_map(|s| s.mesh.as_ref())
            .map(|m| m.faces().len())
            .sum(),
        sets,
        support: out.support,
        timings: out.timings,
        models: n,
        islands,
        placed,
    })
}

//! Runs the DragonFruit support port (`support_gen_df::pipeline`) on a
//! SoulCrafted scene as a cancellable background job (SC-79).
//!
//! Every mesh leaf of the scene (CSG refused, as the slicer refuses it) is one
//! model, baked to world space. A mesh this crate produced earlier carries
//! [`DF_SUPPORT_MESH_NAME`] and is skipped, so a second run does not support
//! the previous supports. The result is the slicer's tessellation of the
//! supports and the raft (`buildSupportAndRaftWorldTriangles`), welded by
//! exact position into one [`IndexedMesh`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use sc_geom_transport_inprocess::{JobError, JobHandle, JobProgress, JobWorker};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{CsgPolicy, Scene, SceneError, bake_mesh_to_world};
use support_gen_df::features::slicing::smooth_shafts::{ShaftStyle, SmoothShaftParams};
use support_gen_df::perf::PerfFrame;
use support_gen_df::pipeline::{
    CancelToken, ModelInput, PipelineError, PipelineSettings, Stage, generate_supports,
};
use support_gen_df::supports::rafts::crenelated::geometry::model_plate_footprint::ModelTransform;
use support_gen_df::supports::support_type_registry::get_support_type_descriptor;
use support_gen_df::supports::types::SupportState;
use support_gen_df::three::Vec3;
use support_gen_df::three::buffer_geometry::BufferGeometry;

/// The name the generated support mesh carries.
pub const DF_SUPPORT_MESH_NAME: &str = "Supports (DragonFruit)";

/// What the run is asked for.
#[derive(Debug, Clone)]
pub struct DfSupportRequest {
    /// The slicing layer height the island scan bands by.
    pub layer_height_mm: f64,
    /// Everything else DragonFruit reads from its panels and stores.
    pub settings: Option<PipelineSettings>,
}

impl Default for DfSupportRequest {
    fn default() -> Self {
        DfSupportRequest {
            layer_height_mm: 0.05,
            settings: None,
        }
    }
}

/// What a finished run hands back.
#[derive(Debug, Clone)]
pub struct DfSupportOutcome {
    /// The supports and raft as one welded mesh, `None` when nothing was placed.
    pub mesh: Option<IndexedMesh>,
    /// The support state the plans left.
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

/// One model per mesh leaf, baked to world space; previous DragonFruit
/// support meshes are left out.
pub fn scene_models(scene: &Scene) -> Result<Vec<ModelInput>, SceneError> {
    let mut models = Vec::new();
    scene.for_each_expanded_leaf(CsgPolicy::Refuse, |id, world, mesh| {
        let is_ours = mesh
            .meta()
            .as_ref()
            .and_then(|m| m.name.as_deref())
            .is_some_and(|n| n == DF_SUPPORT_MESH_NAME);
        if is_ours || mesh.faces().is_empty() {
            return Ok::<(), SceneError>(());
        }
        let baked = bake_mesh_to_world(mesh, world)?;
        models.push(model_from_world_mesh(format!("node-{}", id.get()), &baked));
        Ok(())
    })?;
    Ok(models)
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
    for v in position.chunks_exact(3) {
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
        geometry: BufferGeometry {
            position: Some(position),
            index: Some(index),
            ..Default::default()
        },
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
    cells: HashMap<[i64; 3], Vec<u32>>,
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

/// A triangle soup welded into one mesh **piece by piece**: each closed piece
/// of `parts` (`soup.len()` at each piece's end) gets its own vertices, welded
/// within [`WELD_TOLERANCE_MM`] inside it, so pieces that touch or overlap never share
/// an edge and the mesh stays edge-manifold (the slicer's precondition). A
/// piece identical to an earlier one is dropped. Vertices a rounding error
/// below the plate are put on it ([`PLATE_SNAP_MM`]); triangles the weld
/// collapses are dropped.
pub fn weld_parts(soup: &[f32], parts: &[usize], name: &str) -> Option<IndexedMesh> {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut faces: Vec<IndexedTriangle> = Vec::new();
    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    let mut start = 0;
    for &end in parts.iter().chain(std::iter::once(&soup.len())) {
        let piece = &soup[start..end.max(start)];
        start = end.max(start);
        if piece.is_empty() || !seen.insert(piece.iter().map(|v| v.to_bits()).collect()) {
            continue;
        }
        let mut welder = Welder::default();
        for tri in piece.chunks_exact(9) {
            let mut ids = [0u32; 3];
            for (k, p) in tri.chunks_exact(3).enumerate() {
                let z = if p[2] <= 0.0 && p[2] > -PLATE_SNAP_MM {
                    0.0
                } else {
                    p[2]
                };
                ids[k] = welder.index_of([p[0], p[1], z], &mut vertices);
            }
            if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
                faces.push(IndexedTriangle::from_raw_indices(ids));
            }
        }
    }
    if faces.is_empty() {
        return None;
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
    let models = scene_models(scene)?;
    let run = RUN.fetch_add(1, Ordering::Relaxed) + 1;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    Ok(JobHandle::spawn(
        move |worker: &JobWorker<DfSupportOutcome>| {
            run_df_supports(&models, &request, worker, format!("df{stamp}-{run}"))
        },
    ))
}

fn run_df_supports(
    models: &[ModelInput],
    request: &DfSupportRequest,
    worker: &JobWorker<DfSupportOutcome>,
    id_prefix: String,
) -> Result<DfSupportOutcome, JobError> {
    let settings = request
        .settings
        .clone()
        .unwrap_or_else(|| PipelineSettings {
            // SoulCrafted draws smooth tubes with bicones by default.
            shaft_style: ShaftStyle::Smooth(SmoothShaftParams::default()),
            // Walls that slice cleanly on trimmed footprints.
            robust_raft_geometry: true,
            ..PipelineSettings::defaults(request.layer_height_mm)
        });
    let n = models.len();
    // Reports: geometry per model, three per model, rafts, tessellation.
    let total = (4 * n + 2) as u32;
    let mut completed = 0u32;
    let cancel = CancelToken::new();
    let mut next = 0u64;
    let mut new_id = || {
        next += 1;
        format!("{id_prefix}-{next}")
    };
    let result = generate_supports(
        models,
        &SupportState::default(),
        &settings,
        &mut new_id,
        &mut |p| {
            if worker.is_cancelled() {
                cancel.cancel();
            }
            let phase = match p.stage {
                Stage::BuildRafts | Stage::Tessellate => stage_label(p.stage).to_owned(),
                stage => format!("{} ({}/{})", stage_label(stage), p.model + 1, p.models),
            };
            worker.report_progress(JobProgress {
                phase,
                completed,
                total,
            });
            completed += 1;
        },
        &cancel,
    );
    let out = match result {
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
    let mesh = weld_parts(&out.support_soup, &out.support_parts, DF_SUPPORT_MESH_NAME);
    Ok(DfSupportOutcome {
        triangles: mesh.as_ref().map_or(0, |m| m.faces().len()),
        mesh,
        support: out.support,
        timings: out.timings,
        models: n,
        islands,
        placed,
    })
}

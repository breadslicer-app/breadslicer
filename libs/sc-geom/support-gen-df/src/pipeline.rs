//! SoulCrafted's entry point over the port (SC-78; not a DragonFruit file):
//! one call from the scene's models to their supports and rafts, chaining the
//! stages DragonFruit spreads over its panels and exporter.
//!
//! Per model, in scene order:
//! 1. **World geometry**, as `useIslands.prepareWorldGeom` builds it: the
//!    model's geometry re-centred on its bounding box, through its transform,
//!    as a triangle soup.
//! 2. **Island scan**, the native combined scan (`scan_islands`) with the
//!    Islands panel's defaults, and the overhang classification on the soup
//!    (`scan_overhangs`).
//! 3. **Island derivation** (`useIslands`), with the panel's defaults.
//! 4. **Support plan** (`computeAutoSupportPlan`, auto-bracing included), on
//!    the state the previous models left.
//!
//! Then **rafts** for every root, as `ExportManager` adds them, cleared of the
//! visible models standing on the plate.
//!
//! Cancellation is checked between stages; a cancelled run returns
//! [`PipelineError::Cancelled`] and nothing partial. Nothing is global: the
//! settings, the id generator and the state travel in the call.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::clipper::ClipperError;
use crate::features::slicing::smooth_shafts::ShaftStyle;
use crate::features::slicing::support_world_triangles::{
    SupportSliceInput, TriangleSink, build_support_and_raft_world_parts,
};
use crate::native::mesh_core::mesh::IndexedMesh;
use crate::native::mesh_minima::{CombinedIslandScanResult, scan_islands_without_overhangs};
use crate::native::overhang::{OverhangScan, overhang_and_stability_from_soup};
use crate::perf::{self, PerfFrame};
use crate::supports::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use crate::supports::auto_support::auto_place::helpers::{PlanCtx, PlanMesh};
use crate::supports::auto_support::auto_place::plan::{
    AutoSupportPlan, PlanInput, compute_auto_support_plan,
};
use crate::supports::auto_support::parameter_sizing::SizingBands;
use crate::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use crate::supports::rafts::crenelated::geometry::model_plate_footprint::{
    ModelTransform, PlateFootprintSource, quaternion_from_global_euler,
};
use crate::supports::rafts::crenelated::raft_export::{
    RaftGroup, RaftRoot, RaftSettingsStore, build_export_rafts,
};
use crate::supports::rafts::crenelated::raft_types::RaftBottomMode;
use crate::supports::settings::types::SupportSettings;
use crate::supports::types::SupportState;
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::{Matrix4, Vec3};
use crate::three_mesh_bvh::{MeshBvh, Side};
use crate::volume_analysis::islands::use_islands::{
    DerivedIslands, IslandPanelSettings, derive_islands,
};

/// A model of the scene: its loaded geometry (model space) and placement.
#[derive(Debug, Clone)]
pub struct ModelInput {
    pub id: String,
    pub geometry: BufferGeometry,
    pub transform: ModelTransform,
    pub visible: bool,
}

/// The native island scan's knobs: `useIslands`' state defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandScanSettings {
    pub px_mm: f64,
    pub support_buffer_mm: f64,
    pub connectivity: u8,
    pub minima_k: Option<usize>,
    /// `OVERHANG_FOOTPRINT_PX_MM`.
    pub overhang_px_mm: f32,
    /// The weld tolerance of the scan's indexed mesh.
    pub merge_epsilon: f32,
}

impl Default for IslandScanSettings {
    fn default() -> Self {
        IslandScanSettings {
            px_mm: 0.05,
            support_buffer_mm: 0.25,
            connectivity: 4,
            minima_k: Some(2),
            overhang_px_mm: 0.25,
            merge_epsilon: 1e-5,
        }
    }
}

/// Everything the run reads.
#[derive(Debug, Clone)]
pub struct PipelineSettings {
    /// The slicing layer height (`slicing.layerHeightMm`).
    pub layer_height_mm: f64,
    pub scan: IslandScanSettings,
    pub panel: IslandPanelSettings,
    /// The support settings store (`getSettings()`); its `autoSupport` is the
    /// plan's override.
    pub supports: SupportSettings,
    pub rafts: RaftSettingsStore,
    /// The distance-field cell of each model's pooled SDF.
    pub sdf_cell_mm: f64,
    /// `supportTipShrinkPercent` of the slice tessellation.
    pub support_tip_shrink_percent: f64,
    /// The contact tips' penetration into the model (DragonFruit's
    /// `calculateTipOffset` for the active material and printer).
    pub tip_penetration_mm: f64,
    /// How the tessellation draws shafts (DragonFruit's spheres, or smooth
    /// tubes with bicones).
    pub shaft_style: ShaftStyle,
    /// Closed, manifold raft geometry for a contour slicer (SoulCrafted's;
    /// see `robust_wall`). `false` keeps DragonFruit's.
    pub robust_raft_geometry: bool,
}

impl PipelineSettings {
    /// DragonFruit's defaults at `layer_height_mm`.
    pub fn defaults(layer_height_mm: f64) -> Self {
        PipelineSettings {
            layer_height_mm,
            scan: IslandScanSettings::default(),
            panel: IslandPanelSettings::defaults(layer_height_mm),
            supports: SupportSettings::default(),
            rafts: RaftSettingsStore::default(),
            sdf_cell_mm: 0.5,
            support_tip_shrink_percent: 0.0,
            tip_penetration_mm: 0.0,
            shaft_style: ShaftStyle::DragonFruit,
            robust_raft_geometry: false,
        }
    }
}

/// A stage of the run, for progress reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    WorldGeometry,
    ScanIslands,
    DeriveIslands,
    PlanSupports,
    BuildRafts,
    Tessellate,
}

/// One progress report: `stage` of model `model` (of `models`) is starting;
/// `model` is `models` for the raft stage, which covers the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress<'a> {
    pub stage: Stage,
    pub model: usize,
    pub models: usize,
    pub model_id: Option<&'a str>,
}

/// A shared cancellation flag; clones observe the same flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineError {
    Cancelled,
    /// The raft's Clipper threw (in DragonFruit the export aborts).
    Clipper(ClipperError),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Cancelled => write!(f, "support generation cancelled"),
            PipelineError::Clipper(e) => write!(f, "raft polygon clipping failed: {e:?}"),
        }
    }
}

impl std::error::Error for PipelineError {}

impl From<ClipperError> for PipelineError {
    fn from(e: ClipperError) -> Self {
        PipelineError::Clipper(e)
    }
}

/// What one model's stages produced.
#[derive(Debug, Clone)]
pub struct ModelSupports {
    pub model_id: String,
    /// The world-space soup the scans ran on.
    pub world_soup: Vec<f32>,
    pub combined_scan: CombinedIslandScanResult,
    pub overhang_scan: OverhangScan,
    pub islands: DerivedIslands,
    /// `None` when auto-support is disabled.
    pub plan: Option<AutoSupportPlan>,
}

#[derive(Debug, Clone)]
pub struct PipelineOutput {
    pub models: Vec<ModelSupports>,
    /// The support state after every model's plan.
    pub support: SupportState,
    pub rafts: Vec<RaftGroup>,
    /// What the slicer rasterises (`buildSupportAndRaftWorldTriangles`): every
    /// support and the global-settings raft, nine floats per triangle.
    pub support_soup: Vec<f32>,
    /// `support_soup.len()` at the end of each closed piece of it (see
    /// `TriangleSink`): weld piece by piece to keep the mesh edge-manifold.
    pub support_parts: Vec<usize>,
    /// Where the run spent its time (stages, plan phases, router, builders).
    pub timings: PerfFrame,
}

/// `Box3.getCenter` of the geometry's bounding box.
fn bounding_box_center(geometry: &BufferGeometry) -> Vec3 {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in geometry.position.as_deref().unwrap_or(&[]).chunks_exact(3) {
        let p = Vec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        min = min.min(p);
        max = max.max(p);
    }
    min.add(max).multiply_scalar(0.5)
}

fn model_matrix(t: &ModelTransform) -> Matrix4 {
    Matrix4::compose(
        t.position,
        quaternion_from_global_euler(t.rotation),
        t.scale,
    )
}

/// `prepareWorldGeom`: re-centred, transformed, non-indexed, as stored floats.
pub fn world_soup(model: &ModelInput) -> Vec<f32> {
    let mut g = model.geometry.clone();
    g.normal = None;
    g.uv = None;
    let c = bounding_box_center(&g);
    g.translate(-c.x, -c.y, -c.z);
    g.apply_matrix4(&model_matrix(&model.transform));
    g.to_non_indexed().position.unwrap_or_default()
}

fn triples(soup: &[f32]) -> Vec<[f32; 3]> {
    soup.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
}

/// `computeVertexNormals` on the soup: the face normal at every corner.
fn soup_normals(soup: &[f32]) -> Vec<[f32; 3]> {
    let mut g = BufferGeometry {
        position: Some(soup.to_vec()),
        ..Default::default()
    };
    g.compute_vertex_normals();
    triples(&g.normal.unwrap_or_default())
}

/// Scene-wide supports and rafts. `state` is the support state the run
/// builds on (and the committed store the gap-fill reads); `new_id` mints
/// entity ids.
pub fn generate_supports(
    models: &[ModelInput],
    state: &SupportState,
    settings: &PipelineSettings,
    new_id: &mut dyn FnMut() -> String,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &CancelToken,
) -> Result<PipelineOutput, PipelineError> {
    perf::begin_frame();
    let result = run_pipeline(models, state, settings, new_id, progress, cancel);
    let timings = perf::end_frame().unwrap_or_default();
    result.map(|out| PipelineOutput { timings, ..out })
}

fn run_pipeline(
    models: &[ModelInput],
    state: &SupportState,
    settings: &PipelineSettings,
    new_id: &mut dyn FnMut() -> String,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &CancelToken,
) -> Result<PipelineOutput, PipelineError> {
    let n = models.len();
    let check = || {
        if cancel.is_cancelled() {
            Err(PipelineError::Cancelled)
        } else {
            Ok(())
        }
    };
    let step = |progress: &mut dyn FnMut(Progress<'_>), stage, model: usize| {
        progress(Progress {
            stage,
            model,
            models: n,
            model_id: models.get(model).map(|m| m.id.as_str()),
        });
    };

    // World geometry and the shared per-model structures the plans read
    // (every model is registered for auto-bracing's clearance).
    let mut soups = Vec::with_capacity(n);
    for (i, model) in models.iter().enumerate() {
        check()?;
        step(progress, Stage::WorldGeometry, i);
        let _perf = perf::scope("stage:world-geometry");
        soups.push(world_soup(model));
    }
    let bvhs: Vec<Arc<MeshBvh>> = soups
        .iter()
        .map(|s| Arc::new(MeshBvh::new(triples(s), None)))
        .collect();
    let identity = Matrix4::IDENTITY;
    let brace_meshes: BraceMeshes<'_> = models
        .iter()
        .zip(&bvhs)
        .map(|(m, bvh)| {
            (
                m.id.clone(),
                BraceMesh {
                    bvh,
                    matrix: identity,
                },
            )
        })
        .collect();
    let bands = SizingBands::factory();
    let angle = settings
        .supports
        .auto_support
        .overhang_self_support_angle_deg as f32;

    let mut current = state.clone();
    let mut outputs = Vec::with_capacity(n);
    for (i, model) in models.iter().enumerate() {
        let soup = &soups[i];
        check()?;
        step(progress, Stage::ScanIslands, i);
        let has_raft = settings.rafts.for_model(Some(&model.id)).bottom_mode != RaftBottomMode::Off;
        let mesh = {
            let _perf = perf::scope("stage:weld-mesh");
            IndexedMesh::from_triangle_soup(soup, settings.scan.merge_epsilon)
        };
        let s = &settings.scan;
        // The island scan and the soup's overhang classification are
        // independent: run them side by side. (The scan's own overhang
        // classification is skipped; the soup's is the one derived from.)
        // The timing frame is per thread, so each scan is timed where it
        // runs and recorded here; `stage:scans` is the wall time of both.
        perf::mark("stage:scans");
        let ((combined, t_islands), ((regions, stability), t_overhangs)) =
            std::thread::scope(|scope| {
                let islands = scope.spawn(|| {
                    let t = std::time::Instant::now();
                    let r = scan_islands_without_overhangs(
                        &mesh,
                        settings.layer_height_mm,
                        s.px_mm,
                        s.support_buffer_mm,
                        s.connectivity,
                        s.minima_k,
                    );
                    (r, t.elapsed())
                });
                let t = std::time::Instant::now();
                let overhangs =
                    overhang_and_stability_from_soup(soup, angle, s.overhang_px_mm, has_raft);
                let overhangs = (overhangs, t.elapsed());
                (islands.join().expect("island scan panicked"), overhangs)
            });
        perf::measure("stage:scans");
        perf::record("scan:islands", t_islands);
        perf::record("scan:overhangs", t_overhangs);
        let overhang = OverhangScan { regions, stability };

        check()?;
        step(progress, Stage::DeriveIslands, i);
        let islands = {
            let _perf = perf::scope("stage:derive-islands");
            derive_islands(&combined, &overhang, &[], &settings.panel)
        };

        check()?;
        step(progress, Stage::PlanSupports, i);
        let normals = soup_normals(soup);
        let sdf = RefCell::new(SdfCache::new(
            bvhs[i].clone(),
            identity,
            Some(settings.sdf_cell_mm),
        ));
        let plan_mesh = PlanMesh {
            bvh: &bvhs[i],
            matrix: identity,
            side: Side::Front,
            normals: Some(&normals),
            sdf: &sdf,
            blocked: None,
        };
        let ctx = PlanCtx::new(&settings.supports, &bands, Some(plan_mesh), &mut *new_id);
        perf::mark("stage:plan");
        let plan = compute_auto_support_plan(
            &PlanInput {
                islands: &islands.filtered,
                model_id: &model.id,
                settings_override: Some(&settings.supports.auto_support),
                base_state: &current,
                committed: &current,
                has_raft,
                brace_meshes: &brace_meshes,
            },
            &ctx,
        );
        drop(ctx);
        perf::measure("stage:plan");
        if let Some(plan) = &plan {
            current = plan.support.clone();
        }
        outputs.push(ModelSupports {
            model_id: model.id.clone(),
            world_soup: soup.clone(),
            combined_scan: combined,
            overhang_scan: overhang,
            islands,
            plan,
        });
    }

    check()?;
    step(progress, Stage::BuildRafts, n);
    let rafts = {
        let _perf = perf::scope("stage:export-rafts");
        build_rafts(&current, models, &settings.rafts)?
    };
    check()?;
    step(progress, Stage::Tessellate, n);
    let support = {
        let _perf = perf::scope("stage:tessellate");
        support_soup(&current, models, settings)?
    };
    check()?;
    Ok(PipelineOutput {
        models: outputs,
        support: current,
        rafts,
        support_soup: support.floats,
        support_parts: support.parts,
        timings: PerfFrame::default(),
    })
}

/// What the slicer rasterises for `state` (`buildSupportAndRaftWorldTriangles`
/// as the slice job calls it): the supports of the visible models and the
/// global raft, cleared of those models.
pub fn support_soup(
    state: &SupportState,
    models: &[ModelInput],
    settings: &PipelineSettings,
) -> Result<TriangleSink, ClipperError> {
    let visible: HashSet<String> = models
        .iter()
        .filter(|m| m.visible)
        .map(|m| m.id.clone())
        .collect();
    let clearance: Vec<PlateFootprintSource<'_>> = models
        .iter()
        .filter(|m| m.visible)
        .map(|m| PlateFootprintSource {
            geometry: &m.geometry,
            center: bounding_box_center(&m.geometry),
            transform: m.transform,
            visible: Some(true),
        })
        .collect();
    build_support_and_raft_world_parts(&SupportSliceInput {
        state: &state.collections,
        visible_model_ids: &visible,
        raft: &settings.rafts.global,
        plate_clearance_models: &clearance,
        support_tip_shrink_percent: settings.support_tip_shrink_percent,
        tip_penetration_mm: settings.tip_penetration_mm,
        shaft_style: settings.shaft_style,
        robust_raft_geometry: settings.robust_raft_geometry,
    })
}

/// The rafts `ExportManager` adds for `state`'s roots, scoped to the scene's
/// models, with every visible model as plate clearance (its geometry,
/// bounding-box centre and transform, as DragonFruit hands them over).
pub fn build_rafts(
    state: &SupportState,
    models: &[ModelInput],
    store: &RaftSettingsStore,
) -> Result<Vec<RaftGroup>, ClipperError> {
    let roots: Vec<RaftRoot> = state
        .collections
        .roots
        .values()
        .map(|r| RaftRoot {
            model_id: Some(r.base.model_id.clone()),
            x: r.transform.pos.x,
            y: r.transform.pos.y,
            diameter: r.diameter,
        })
        .collect();
    let sources: Vec<PlateFootprintSource<'_>> = models
        .iter()
        .map(|m| PlateFootprintSource {
            geometry: &m.geometry,
            center: bounding_box_center(&m.geometry),
            transform: m.transform,
            visible: Some(m.visible),
        })
        .collect();
    // The export is scoped to the scene's models.
    let scope: HashSet<String> = models.iter().map(|m| m.id.clone()).collect();
    build_export_rafts(
        &roots,
        (!scope.is_empty()).then_some(&scope),
        store,
        &sources,
    )
}

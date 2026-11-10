//! SoulCrafted's entry point over the port (not a DragonFruit file):
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
    SupportSliceInput, TipDrawing, TriangleSink, build_support_and_raft_world_parts,
};
use crate::native::islands::body::IslandBodyScan;
use crate::native::mesh_core::mesh::IndexedMesh;
use crate::native::mesh_minima::{CombinedIslandScanResult, scan_islands_with_bodies};
use crate::native::overhang::{
    OVERHANG_WELD_EPSILON, OverhangScan, classify_overhangs, overhang_and_stability,
    overhang_and_stability_from_soup,
};
use crate::perf::{self, PerfFrame};
use crate::supports::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use crate::supports::auto_support::auto_place::helpers::{PlanCtx, PlanMesh};
use crate::supports::auto_support::auto_place::plan::{
    AutoSupportPlan, PlanInput, ZonePlanInput, compute_auto_support_plan,
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
use crate::volume_analysis::islands::types::DetectedIsland;
use crate::volume_analysis::islands::use_islands::{
    DerivedIslands, IslandPanelSettings, derive_islands, overhang_region_to_island,
};

/// A model of the scene: its loaded geometry (model space) and placement.
/// The geometry is shared, so a copy that only moves the model is cheap.
#[derive(Debug, Clone)]
pub struct ModelInput {
    pub id: String,
    pub geometry: Arc<BufferGeometry>,
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
    /// SoulCrafted (#360): measure every island's body (zone rules).
    pub island_bodies: Option<IslandBodyScan>,
    /// SoulCrafted (#360): also classify the down-facing regions up to this
    /// angle from horizontal (the zone rules' stabilization).
    pub steep_angle_deg: Option<f32>,
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
            island_bodies: None,
            steep_angle_deg: None,
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
    /// How contact tips are drawn (SoulCrafted; the default is DragonFruit's).
    pub tip_drawing: TipDrawing,
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
            tip_drawing: TipDrawing::default(),
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
    /// Empty from [`plan_supports_with`].
    pub rafts: Vec<RaftGroup>,
    /// What the slicer rasterises (`buildSupportAndRaftWorldTriangles`): every
    /// support and the global-settings raft, nine floats per triangle. Empty
    /// from [`plan_supports_with`].
    pub support_soup: Vec<f32>,
    /// `support_soup.len()` at the end of each closed piece of it (see
    /// `TriangleSink`): weld piece by piece to keep the mesh edge-manifold.
    pub support_parts: Vec<usize>,
    /// Where the run spent its time (stages, plan phases, router, builders).
    pub timings: PerfFrame,
    /// Per model (in input order), its distance field as the plan's routing
    /// left it: filled where supports were routed, so later placements on
    /// the same model (hand edits) start warm. Not part of the result.
    pub distance_fields: Arc<std::sync::Mutex<Vec<Option<SdfCache>>>>,
    /// Per model (in input order), the BVH of its world soup the plans read
    /// (a prescan's when it had one): later edits on the same model can
    /// take it instead of building it again.
    pub bvhs: Vec<Arc<MeshBvh>>,
}

/// `Box3.getCenter` of the geometry's bounding box.
fn bounding_box_center(geometry: &BufferGeometry) -> Vec3 {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in geometry
        .position
        .as_deref()
        .unwrap_or(&[])
        .as_chunks::<3>()
        .0
    {
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
    let mut g = BufferGeometry::clone(&model.geometry);
    g.normal = None;
    g.uv = None;
    let c = bounding_box_center(&g);
    g.translate(-c.x, -c.y, -c.z);
    g.apply_matrix4(&model_matrix(&model.transform));
    g.to_non_indexed().position.unwrap_or_default()
}

fn triples(soup: &[f32]) -> Vec<[f32; 3]> {
    soup.as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect()
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

/// The settings an island and overhang scan depend on, with the world
/// geometry it ran on (as a hash): the same key, the same scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScanKey {
    soup: u64,
    soup_len: usize,
    layer_height: u64,
    px: u64,
    support_buffer: u64,
    connectivity: u8,
    minima_k: Option<usize>,
    overhang_px: u32,
    merge_epsilon: u32,
    angle: u32,
    has_raft: bool,
    bodies: Option<([u32; 4], u8, u32)>,
    steep_angle: Option<u32>,
}

impl ScanKey {
    fn new(soup: &[f32], settings: &PipelineSettings, angle: f32, has_raft: bool) -> Self {
        let s = &settings.scan;
        ScanKey {
            soup: soup_key(soup),
            soup_len: soup.len(),
            layer_height: settings.layer_height_mm.to_bits(),
            px: s.px_mm.to_bits(),
            support_buffer: s.support_buffer_mm.to_bits(),
            connectivity: s.connectivity,
            minima_k: s.minima_k,
            overhang_px: s.overhang_px_mm.to_bits(),
            merge_epsilon: s.merge_epsilon.to_bits(),
            angle: angle.to_bits(),
            has_raft,
            bodies: s.island_bodies.map(|b| {
                (
                    b.section_heights_mm.map(f32::to_bits),
                    b.heights,
                    b.volume_cap_mm3.to_bits(),
                )
            }),
            steep_angle: s.steep_angle_deg.map(f32::to_bits),
        }
    }
}

/// A hash of a world soup's exact bits.
fn soup_key(soup: &[f32]) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = ahash::RandomState::with_seeds(0x5c, 0x82, 0x0d, 0xf5).build_hasher();
    h.write_usize(soup.len());
    for pair in soup.chunks(2) {
        let lo = u64::from(pair[0].to_bits());
        let hi = pair.get(1).map_or(0, |v| u64::from(v.to_bits()));
        h.write_u64(lo | (hi << 32));
    }
    h.finish()
}

/// One model's scans, ready to reuse.
#[derive(Debug, Clone)]
pub struct PrescannedModel {
    key: ScanKey,
    pub combined: CombinedIslandScanResult,
    pub overhang: OverhangScan,
    bvh: Arc<MeshBvh>,
}

/// What [`prescan_models`] computed ahead of a run: the geometry-only work of
/// [`generate_supports`] (the island and overhang scans, the BVH), keyed by
/// the world geometry and the scan settings. A model moved, or a setting
/// changed, simply misses and is scanned again by the run.
#[derive(Debug, Clone, Default)]
pub struct ScanCache {
    models: Vec<Arc<PrescannedModel>>,
}

impl ScanCache {
    /// Adds `other`'s models.
    pub fn extend(&mut self, other: ScanCache) {
        self.models.extend(other.models);
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    fn scans(&self, key: &ScanKey) -> Option<&PrescannedModel> {
        self.models.iter().map(|m| &**m).find(|m| &m.key == key)
    }

    fn bvh(&self, soup: u64) -> Option<Arc<MeshBvh>> {
        self.models
            .iter()
            .find(|m| m.key.soup == soup)
            .map(|m| m.bvh.clone())
    }
}

/// The island scan and the overhang classification of one world soup.
fn scan_model(
    soup: &[f32],
    settings: &PipelineSettings,
    angle: f32,
    has_raft: bool,
) -> (CombinedIslandScanResult, OverhangScan) {
    let mesh = {
        let _perf = perf::scope("stage:weld-mesh");
        IndexedMesh::from_triangle_soup(soup, settings.scan.merge_epsilon)
    };
    let s = &settings.scan;
    // The island scan and the soup's overhang classification are
    // independent: run them side by side. (The scan's own overhang
    // classification is skipped; the soup's is the one derived from.)
    // `rayon::join` keeps both, and their own parallel loops, in the pool the
    // caller runs in: a background prescan confined to a few threads stays
    // there. The timing frame is per thread, so each scan is timed where it
    // runs and recorded here; `stage:scans` is the wall time of both.
    perf::mark("stage:scans");
    let ((((regions, stability), steep_regions), t_overhangs), (combined, t_islands)) = rayon::join(
        || {
            let t = std::time::Instant::now();
            // Same weld as the island scan's by default: reuse it.
            let overhangs = if s.merge_epsilon == OVERHANG_WELD_EPSILON {
                overhang_and_stability(&mesh, angle, s.overhang_px_mm, has_raft)
            } else {
                overhang_and_stability_from_soup(soup, angle, s.overhang_px_mm, has_raft)
            };
            let steep = s
                .steep_angle_deg
                .map(|steep| classify_overhangs(&mesh, steep, s.overhang_px_mm))
                .unwrap_or_default();
            ((overhangs, steep), t.elapsed())
        },
        || {
            let t = std::time::Instant::now();
            let r = scan_islands_with_bodies(
                &mesh,
                settings.layer_height_mm,
                s.px_mm,
                s.support_buffer_mm,
                s.connectivity,
                s.minima_k,
                s.island_bodies,
            );
            (r, t.elapsed())
        },
    );
    perf::measure("stage:scans");
    perf::record("scan:islands", t_islands);
    perf::record("scan:overhangs", t_overhangs);
    (
        combined,
        OverhangScan {
            regions,
            stability,
            steep_regions,
        },
    )
}

/// SoulCrafted (#500): `scan`, each region naming its triangles by the faces
/// of `bvh`, the tree built from the soup that was scanned.
///
/// The scan numbers triangles as the soup holds them; the tree's build
/// reorders them, and the region sampler tells a region's surface by the
/// face a ray's hit names
/// ([`SurfaceSampler`](crate::supports::auto_support::grid_placement::SurfaceSampler)).
/// DragonFruit scans a soup read through the index its tree already
/// reordered, so the two orders are one there. Here they are not, and without
/// this the sampler took for a region's surface whatever face shared a number
/// with one of its triangles: tops and walls of the model, and little of an
/// underside. Overhang candidates landed on surfaces that face up (and went
/// on to whatever hung over them), and undersides went without any.
pub fn regions_on_tree_faces(mut scan: OverhangScan, bvh: &MeshBvh) -> OverhangScan {
    let Some(faces) = bvh.faces_of_soup() else {
        return scan;
    };
    for region in scan.regions.iter_mut().chain(&mut scan.steep_regions) {
        for triangle in &mut region.triangle_ids {
            *triangle = faces[*triangle as usize];
        }
    }
    scan
}

/// Runs the geometry-only part of [`generate_supports`] for `models` ahead of
/// time, for [`generate_supports_with`] to reuse: what a run would scan
/// first, done while nobody waits. A model `previous` already holds (same
/// world geometry, same settings) is taken from it instead of scanned again.
pub fn prescan_models(
    models: &[ModelInput],
    settings: &PipelineSettings,
    previous: Option<&ScanCache>,
    cancel: &CancelToken,
) -> Result<ScanCache, PipelineError> {
    let angle = scan_angle(settings);
    let mut cache = ScanCache::default();
    for model in models {
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let soup = world_soup(model);
        let has_raft = settings.rafts.for_model(Some(&model.id)).bottom_mode != RaftBottomMode::Off;
        let key = ScanKey::new(&soup, settings, angle, has_raft);
        if let Some(known) = previous.and_then(|c| c.models.iter().find(|m| m.key == key)) {
            cache.models.push(known.clone());
            continue;
        }
        let bvh = Arc::new(MeshBvh::new(triples(&soup), None));
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let (combined, overhang) = scan_model(&soup, settings, angle, has_raft);
        cache.models.push(Arc::new(PrescannedModel {
            key,
            combined,
            overhang,
            bvh,
        }));
    }
    Ok(cache)
}

/// The overhang self-support angle the scans classify by.
fn scan_angle(settings: &PipelineSettings) -> f32 {
    settings
        .supports
        .auto_support
        .overhang_self_support_angle_deg as f32
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
    generate_supports_with(models, state, settings, None, new_id, progress, cancel)
}

/// [`generate_supports`], reusing what `prescanned` ([`prescan_models`]) holds
/// for any model whose world geometry and scan settings it was taken with.
/// The output is the same as without it.
pub fn generate_supports_with(
    models: &[ModelInput],
    state: &SupportState,
    settings: &PipelineSettings,
    prescanned: Option<&ScanCache>,
    new_id: &mut dyn FnMut() -> String,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &CancelToken,
) -> Result<PipelineOutput, PipelineError> {
    timed(|| {
        run_pipeline(
            models, state, settings, prescanned, new_id, progress, cancel, true,
        )
    })
}

/// [`generate_supports_with`] up to the plans, for a caller that draws each
/// model's supports and raft itself: the scene-wide rafts and tessellation
/// are skipped (`rafts`, `support_soup` and `support_parts` come back empty,
/// and their stages are not reported). Everything else is the same.
pub fn plan_supports_with(
    models: &[ModelInput],
    state: &SupportState,
    settings: &PipelineSettings,
    prescanned: Option<&ScanCache>,
    new_id: &mut dyn FnMut() -> String,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &CancelToken,
) -> Result<PipelineOutput, PipelineError> {
    timed(|| {
        run_pipeline(
            models, state, settings, prescanned, new_id, progress, cancel, false,
        )
    })
}

/// `run` in a timing frame, whose report goes into its output.
fn timed(
    run: impl FnOnce() -> Result<PipelineOutput, PipelineError>,
) -> Result<PipelineOutput, PipelineError> {
    perf::begin_frame();
    let result = run();
    let timings = perf::end_frame().unwrap_or_default();
    result.map(|out| PipelineOutput { timings, ..out })
}

/// The run; `draw` builds the scene-wide rafts and tessellation after the
/// plans.
#[allow(clippy::too_many_arguments)]
fn run_pipeline(
    models: &[ModelInput],
    state: &SupportState,
    settings: &PipelineSettings,
    prescanned: Option<&ScanCache>,
    new_id: &mut dyn FnMut() -> String,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &CancelToken,
    draw: bool,
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
    let bvhs: Vec<Arc<MeshBvh>> = {
        let _perf = perf::scope("stage:bvh");
        soups
            .iter()
            .map(|s| {
                prescanned
                    .and_then(|c| c.bvh(soup_key(s)))
                    .unwrap_or_else(|| Arc::new(MeshBvh::new(triples(s), None)))
            })
            .collect()
    };
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
    let bands =
        SizingBands::factory().with_zone_sizes(settings.supports.auto_support.zone_rules.as_ref());
    let angle = scan_angle(settings);

    let mut current = state.clone();
    let mut outputs = Vec::with_capacity(n);
    let mut distance_fields = Vec::with_capacity(n);
    for (i, model) in models.iter().enumerate() {
        let soup = &soups[i];
        check()?;
        step(progress, Stage::ScanIslands, i);
        let has_raft = settings.rafts.for_model(Some(&model.id)).bottom_mode != RaftBottomMode::Off;
        let key = ScanKey::new(soup, settings, angle, has_raft);
        let (combined, overhang) = match prescanned.and_then(|c| c.scans(&key)) {
            Some(hit) => {
                perf::record("scan:prescanned", std::time::Duration::ZERO);
                (hit.combined.clone(), hit.overhang.clone())
            }
            None => scan_model(soup, settings, angle, has_raft),
        };
        let overhang = regions_on_tree_faces(overhang, &bvhs[i]);

        check()?;
        step(progress, Stage::DeriveIslands, i);
        let islands = {
            let _perf = perf::scope("stage:derive-islands");
            derive_islands(&combined, &overhang, &[], &settings.panel)
        };
        // The zone rules' stabilization reads the steep regions as islands.
        let steep: Vec<DetectedIsland> = overhang
            .steep_regions
            .iter()
            .enumerate()
            .map(|(i, r)| DetectedIsland {
                id: format!("s{i}"),
                ..overhang_region_to_island(r, i)
            })
            .collect();

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
                zone: Some(ZonePlanInput {
                    steep: &steep,
                    layer_height_mm: settings.layer_height_mm,
                }),
            },
            &ctx,
        );
        drop(ctx);
        distance_fields.push(Some(sdf.into_inner()));
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

    let (rafts, support) = if draw {
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
        (rafts, support)
    } else {
        (Vec::new(), TriangleSink::default())
    };
    check()?;
    drop(brace_meshes);
    Ok(PipelineOutput {
        models: outputs,
        support: current,
        rafts,
        support_soup: support.floats,
        support_parts: support.parts,
        timings: PerfFrame::default(),
        distance_fields: Arc::new(std::sync::Mutex::new(distance_fields)),
        bvhs,
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
        tip_drawing: settings.tip_drawing,
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

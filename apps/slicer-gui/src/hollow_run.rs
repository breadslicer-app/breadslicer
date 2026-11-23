/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Hollow tab (BS-8): hollows the selected model with the hollowing
//! engine (`sc_geom::hollow`) and drills drains where the model is
//! clicked.
//!
//! A hollow replaces the leaf's mesh as one undoable edit. The driver keeps,
//! per hollow mesh it installed, the original mesh and the drains (in the
//! leaf's local frame, so moving the model keeps them in place). A new drain
//! or other settings hollow the original again; Undo puts the previous mesh
//! back, and the entry it matches (by mesh identity) follows. A job that
//! finishes on a scene edited meanwhile is discarded.
//!
//! While the tab is open the selected model shows previews over it: the
//! cavity the current settings carve (recomputed as the wall or detail change
//! and re-posed as the model moves), its drains, and with the drain tool the
//! drain a click would drill. The cavity shown is the one Hollow builds (#29):
//! the engine keeps it, and Hollow with the same settings puts exactly that
//! cavity in the model, so where the preview shows cavity behind the wall a
//! drain opens into it. The drain under the cursor is green where it opens
//! into that cavity and red where Hollow would leave it out (#302): the
//! engine's own check on its own cavity, so the colour is what Hollow does;
//! neutral while that cavity is not computed.
//!
//! The Review card (#90) asks which hollow made here holds a resin trap
//! ([`HollowRun::hollow_at`]), and opens the drain tool on the model holding
//! a cavity, hollowed here or not ([`HollowRun::model_at`], #158, #431). It
//! hears of each drain that lands in a model ([`HollowRun::on_drained`],
//! #128): its row is done only then.
//!
//! The hollows made here live in the document
//! ([`SceneDocument::hollows`]), so a `.bread` save keeps them and an open
//! puts them back ([`crate::hollow_persist`], #161). Selecting a hollow puts
//! the settings it was made with in the tab, so a new drain hollows it the
//! same way again.
//!
//! The drain tool shapes the drains it places: as deep as the wall needs or
//! to a set depth (#316), a cylinder or a cone narrowing or widening to an
//! inner radius at its end (#317), with or without a plug (#318). Each drain
//! keeps its own shape. A plug is a model of its own, standing on the plate
//! beside the hollow, clear of the other models, on the end that meets the
//! cavity (the model's surface on top, as it goes back in the hole). It
//! comes and goes with its hollow in the same undo step: hollowing again
//! replaces the plugs (one left where the person moved it stays there),
//! making the model solid again takes them away.
//!
//! A model that came hollow (from a file, with no record of how it was
//! made) is not hollowed again: a drain is drilled into its mesh as it is
//! (#447), only where the air behind the wall is a cavity's, and the rest
//! of the mesh is left as it was. Its sealed cavities show in the tab
//! (#130), and with the drain tool the faces where a drain works best are
//! tinted (#455). A drain click made while a job runs waits for it, a mesh
//! the slicer refuses offers Repair, and a wall that leaves no room for a
//! cavity changes nothing (#448).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use sc_geom::hollow::bread_hollow_engine::LassoRegion;
use sc_geom::hollow::bread_hollow_engine::engine::{DEFAULT_PLUG_CLEARANCE_MM, DRAIN_PAST_WALL_MM};
use sc_geom::hollow::bread_hollow_engine::{
    DrainDepth, GridRequest, HollowFill, HollowSettings, InfillMode, ResolutionBound, Stage,
};
use sc_geom::hollow::{
    DrillOutcome, DrillProbe, HOLLOW_ENGINE_VERSION, HollowKind, HollowOutcome, HollowPlug,
    HollowRequest, HollowResult, LeafFrame, PreparedHollow, WorldDrain, WorldSolidArea,
    start_drill_mesh, start_hollow_mesh,
};
use sc_geom::{JobError, JobHandle, JobProgress, JobWorker};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_mesh_spatial::PrimedMesh;
use sc_scene::node::{LeafAttachment, LeafPayload, SceneMetadata};
use sc_scene::{NodeId, PaintKind};
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};
use crate::repair_run::RepairRun;
use crate::scene_document::{
    MarksStep, SceneDocument, SceneEvent, SceneRevision, SceneSubscription, leaf_mesh_swap,
};

/// The marks placed on a solid model before it is hollowed: its drains and
/// its solid areas (#512).
type Marks = (Vec<LocalDrain>, Vec<LassoRegion>);
use crate::surface_brush::lost_paint_note;
use crate::viewport::ViewportController;
use crate::{MainWindow, WorkspaceSection};

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How often the previews look at the tab, the selection and the settings.
const PREVIEW_TICK: Duration = Duration::from_millis(40);
/// Sides of a drawn drain.
const DRAIN_SIDES: u32 = 24;

/// Every stage of the hollow engine, in the order it runs them.
const STAGES: [Stage; 10] = [
    Stage::Voxelize,
    Stage::FloodFill,
    Stage::DistanceField,
    Stage::WallDistance,
    Stage::KeepMask,
    Stage::CavitySurface,
    Stage::Infill,
    Stage::Stabilize,
    Stage::Drains,
    Stage::Validate,
];

/// The phase a hollow job reports, in the person's language. The engine
/// reports its stage labels in English ([`Stage::label`], then `Done`); a
/// phase it does not know stays as it came.
fn phase_text(phase: &str) -> slint::SharedString {
    if phase == "Done" {
        return ctr!("HollowRun", "Done");
    }
    STAGES
        .iter()
        .find(|stage| stage.label() == phase)
        .map_or_else(|| phase.into(), |&stage| stage_text(stage))
}

fn stage_text(stage: Stage) -> slint::SharedString {
    match stage {
        Stage::Voxelize => ctr!("HollowRun", "Voxelizing surface"),
        Stage::FloodFill => ctr!("HollowRun", "Filling outside"),
        Stage::DistanceField => ctr!("HollowRun", "Measuring wall distance"),
        Stage::WallDistance => ctr!("HollowRun", "Measuring exact wall distance"),
        Stage::KeepMask => ctr!("HollowRun", "Choosing wall voxels"),
        Stage::CavitySurface => ctr!("HollowRun", "Building cavity surface"),
        Stage::Infill => ctr!("HollowRun", "Building infill"),
        Stage::Stabilize => ctr!("HollowRun", "Checking hollow mesh"),
        Stage::Drains => ctr!("HollowRun", "Cutting drain holes"),
        Stage::Validate => ctr!("HollowRun", "Validating result"),
    }
}

/// The narrowest cavity channel kept by default (#454).
pub(crate) const DEFAULT_MIN_CAVITY_WIDTH_MM: f32 = 2.0;

/// The Hollow tab's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HollowOptions {
    pub(crate) wall_mm: f32,
    pub(crate) voxel_mm: f32,
    /// 0 empty, 1 lattice, 2 pillars.
    pub(crate) fill: u8,
    pub(crate) cell_mm: f32,
    pub(crate) beam_mm: f32,
    /// The drain tool: the radius of a new drain at the surface.
    pub(crate) drain_radius_mm: f32,
    /// New drains go `drain_depth_mm` deep (#316), else as deep as the wall
    /// needs.
    pub(crate) drain_depth_set: bool,
    pub(crate) drain_depth_mm: f32,
    /// New drains are cones (#317), their radius `drain_inner_radius_mm` at
    /// their end.
    pub(crate) drain_cone: bool,
    pub(crate) drain_inner_radius_mm: f32,
    /// New drains make a plug (#318), `plug_clearance_mm` narrower across
    /// than the hole.
    pub(crate) drain_plug: bool,
    pub(crate) plug_clearance_mm: f32,
    /// The narrowest a cavity channel may be (#454): narrower ones stay
    /// solid. 0 keeps them all.
    pub(crate) min_width_mm: f32,
}

impl Default for HollowOptions {
    fn default() -> Self {
        // The engine's defaults; the drain shapes off, Damian's clearance.
        Self {
            wall_mm: 2.0,
            voxel_mm: 0.65,
            fill: 0,
            cell_mm: 4.24,
            beam_mm: 0.35,
            drain_radius_mm: 1.5,
            drain_depth_set: false,
            drain_depth_mm: 3.0,
            drain_cone: false,
            drain_inner_radius_mm: 1.0,
            drain_plug: false,
            plug_clearance_mm: DEFAULT_PLUG_CLEARANCE_MM,
            // Damian's: 1.5 mm traps resin.
            min_width_mm: DEFAULT_MIN_CAVITY_WIDTH_MM,
        }
    }
}

impl HollowOptions {
    /// These options without the drain tool's (its defaults, and no
    /// radius): what the cavity depends on.
    fn cavity_only(&self) -> Self {
        let tool = Self::default();
        Self {
            drain_radius_mm: 0.0,
            drain_depth_set: tool.drain_depth_set,
            drain_depth_mm: tool.drain_depth_mm,
            drain_cone: tool.drain_cone,
            drain_inner_radius_mm: tool.drain_inner_radius_mm,
            drain_plug: tool.drain_plug,
            plug_clearance_mm: tool.plug_clearance_mm,
            ..*self
        }
    }

    pub(crate) fn settings(&self) -> HollowSettings {
        let fill = match self.fill {
            0 => HollowFill::Cavity,
            n => HollowFill::Infill {
                pattern: if n == 1 {
                    InfillMode::Lattice
                } else {
                    InfillMode::Pillar
                },
                cell_mm: self.cell_mm,
                beam_radius_mm: self.beam_mm,
            },
        };
        HollowSettings {
            fill,
            wall_thickness_mm: self.wall_mm,
            min_cavity_width_mm: self.min_width_mm,
            ..HollowSettings::default()
        }
    }
}

/// Whether the box around `mesh` posed by `world` holds `at` (world mm).
fn box_holds(mesh: &IndexedMesh, world: &Transform3D, at: [f32; 3]) -> bool {
    // A floor on the plate stands on the box's face, within rounding.
    const SLACK_MM: f32 = 1e-3;
    let (mut low, mut high) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for vertex in mesh.vertices() {
        let posed = world.apply_to_vertex(*vertex).coords();
        for axis in 0..3 {
            low[axis] = low[axis].min(posed[axis]);
            high[axis] = high[axis].max(posed[axis]);
        }
    }
    (0..3).all(|axis| low[axis] - SLACK_MM <= at[axis] && at[axis] <= high[axis] + SLACK_MM)
}

/// Where a vertical line down from `at` (world mm) first leaves the solid of
/// `mesh` posed by `world`: below a cavity's floor point, the underside of
/// that floor. The faces are read in the mesh's own frame, where the line is
/// `at + t * down` mapped back; `t` is then the world distance, since the
/// map is affine. A face is left, not entered, when its outward normal (the
/// winding's) points along the line.
fn floor_underside(mesh: &IndexedMesh, world: &Transform3D, at: [f32; 3]) -> Option<[f32; 3]> {
    let inverse = world.inverse().ok()?;
    let local = |p: [f32; 3]| inverse.apply_to_vertex(Vertex::new(p)).coords();
    let origin = local(at);
    let below = local([at[0], at[1], at[2] - 1.0]);
    let down: [f32; 3] = std::array::from_fn(|i| below[i] - origin[i]);
    let vertices = mesh.vertices();
    let mut nearest: Option<f32> = None;
    for face in mesh.faces() {
        let corner = |k: usize| {
            vertices
                .get(usize::from(face.vertices[k]))
                .map(|v| v.coords())
        };
        let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
            continue;
        };
        let e1: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
        let e2: [f32; 3] = std::array::from_fn(|i| c[i] - a[i]);
        if dot(cross(e1, e2), down) <= 0.0 {
            continue;
        }
        // The determinant is minus the normal's projection on the line, so a
        // face the line leaves never gives 0, save by underflow.
        let h = cross(down, e2);
        let det = dot(e1, h);
        if det == 0.0 {
            continue;
        }
        let s: [f32; 3] = std::array::from_fn(|i| origin[i] - a[i]);
        let u = dot(s, h) / det;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = dot(down, q) / det;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = dot(e2, q) / det;
        if t > 1e-4 && nearest.is_none_or(|best| t < best) {
            nearest = Some(t);
        }
    }
    nearest.map(|t| [at[0], at[1], at[2] - t])
}

/// A drain in the leaf's local frame: a point on the surface and the
/// direction into the model; its sizes in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct LocalDrain {
    pub(crate) center: [f32; 3],
    pub(crate) direction: [f32; 3],
    /// At the surface.
    pub(crate) radius_mm: f32,
    /// A set depth (#316); `None` as deep as the wall needs.
    pub(crate) depth_mm: Option<f32>,
    /// A cone's radius at the drain's end (#317); `None` a cylinder.
    pub(crate) inner_radius_mm: Option<f32>,
    /// Make a plug this much narrower across (#318); `None` none.
    pub(crate) plug_clearance_mm: Option<f32>,
}

impl LocalDrain {
    /// The drain a click drills: at the world `point` hit, into the model
    /// against the surface `normal` there, in the leaf's local frame.
    fn at(point: [f32; 3], normal: [f32; 3], radius_mm: f32, world: &Transform3D) -> Option<Self> {
        let inverse = world.inverse().ok()?;
        let local = |p: [f32; 3]| inverse.apply_to_vertex(Vertex::new(p)).coords();
        let center = local(point);
        let inside = local(std::array::from_fn(|i| point[i] - normal[i]));
        Some(LocalDrain {
            center,
            direction: normalized(std::array::from_fn(|i| inside[i] - center[i])),
            radius_mm,
            ..LocalDrain::default()
        })
    }

    /// This drain shaped as the drain tool says: its depth, cone and plug.
    fn shaped(self, options: &HollowOptions) -> Self {
        Self {
            depth_mm: options.drain_depth_set.then_some(options.drain_depth_mm),
            inner_radius_mm: options.drain_cone.then_some(options.drain_inner_radius_mm),
            plug_clearance_mm: options.drain_plug.then_some(options.plug_clearance_mm),
            ..self
        }
    }

    /// The radius at the drain's end: a cone's inner one, else the same.
    fn end_radius_mm(&self) -> f32 {
        self.inner_radius_mm.unwrap_or(self.radius_mm)
    }

    pub(crate) fn to_world(self, world: &Transform3D) -> WorldDrain {
        let at = |p: [f32; 3]| world.apply_to_vertex(Vertex::new(p)).coords();
        let center = at(self.center);
        let ahead = at(std::array::from_fn(|i| self.center[i] + self.direction[i]));
        let mut drain = WorldDrain::new(
            center,
            normalized(std::array::from_fn(|i| ahead[i] - center[i])),
            self.radius_mm,
        );
        drain.depth = self.depth_mm.map_or(DrainDepth::Auto, DrainDepth::Mm);
        drain.inner_radius_mm = self.inner_radius_mm;
        drain.plug_clearance_mm = self.plug_clearance_mm;
        drain
    }
}

/// A drain's plug a hollow put in the scene (#318): which drain, by its
/// index in the hollow's drains, and the mesh its model shows, by identity.
#[derive(Debug, Clone)]
pub(crate) struct PlugLink {
    pub(crate) drain: usize,
    pub(crate) mesh: Arc<IndexedMesh>,
}

/// How far apart a plug stands from the models around it, on the plate.
const PLUG_GAP_MM: f32 = 3.0;

/// A box on the plate: its lowest and highest corner.
type PlateRect = ([f32; 2], [f32; 2]);

/// Where to stand plugs of the half widths `sizes` (around their axis) on
/// the plate: beside `model` (its box), clear of each box of `taken` and of
/// one another by [`PLUG_GAP_MM`], within `bed` when there is room there,
/// the nearest to the model first (a millimetre grid). Without room, to the
/// model's right.
fn plug_spots(
    model: PlateRect,
    taken: &[PlateRect],
    bed: PlateRect,
    sizes: &[[f32; 2]],
) -> Vec<[f32; 2]> {
    let mut taken = taken.to_vec();
    let gap = PLUG_GAP_MM;
    let distance = |a: PlateRect, b: PlateRect| {
        let dx = (b.0[0] - a.1[0]).max(a.0[0] - b.1[0]).max(0.0);
        let dy = (b.0[1] - a.1[1]).max(a.0[1] - b.1[1]).max(0.0);
        dx.hypot(dy)
    };
    let overlaps = |a: PlateRect, b: PlateRect| {
        a.0[0] < b.1[0] + gap
            && b.0[0] < a.1[0] + gap
            && a.0[1] < b.1[1] + gap
            && b.0[1] < a.1[1] + gap
    };
    sizes
        .iter()
        .map(|&half| {
            let rect = |c: [f32; 2]| {
                (
                    [c[0] - half[0], c[1] - half[1]],
                    [c[0] + half[0], c[1] + half[1]],
                )
            };
            let mut best: Option<(f32, [f32; 2])> = None;
            let (nx, ny) = (
                (bed.1[0] - bed.0[0] - 2.0 * half[0]).floor().max(-1.0) as i32,
                (bed.1[1] - bed.0[1] - 2.0 * half[1]).floor().max(-1.0) as i32,
            );
            for i in 0..=nx {
                for j in 0..=ny {
                    let c = [bed.0[0] + half[0] + i as f32, bed.0[1] + half[1] + j as f32];
                    let r = rect(c);
                    if overlaps(r, model) || taken.iter().any(|&t| overlaps(r, t)) {
                        continue;
                    }
                    let d = distance(r, model);
                    if best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, c));
                    }
                }
            }
            let spot = best.map_or(
                [model.1[0] + gap + half[0], 0.5 * (model.0[1] + model.1[1])],
                |(_, c)| c,
            );
            taken.push(rect(spot));
            spot
        })
        .collect()
}

/// A hollow this driver installed: the mesh it put on the leaf, what it was
/// made from, with which drains (the ones drilled) and settings.
#[derive(Debug, Clone)]
pub(crate) struct Hollowed {
    pub(crate) hollow: Arc<IndexedMesh>,
    pub(crate) original: Arc<IndexedMesh>,
    pub(crate) drains: Vec<LocalDrain>,
    pub(crate) options: HollowOptions,
    /// The plugs it put in the scene with it (#318).
    pub(crate) plugs: Vec<PlugLink>,
    /// The areas drawn to stay solid (#179), in the leaf's local frame.
    pub(crate) solids: Vec<LassoRegion>,
    /// The engine that made the cavity ([`HOLLOW_ENGINE_VERSION`]); `None`
    /// for a hollow read from a file saved before it was recorded (#446).
    pub(crate) engine: Option<u32>,
    /// The model's stretch when it was hollowed ([`LeafFrame::stretch_rows`]):
    /// its scale, which the cavity was made at. `None` as `engine`.
    pub(crate) stretch: Option<[f64; 9]>,
    /// The model came hollow (from a file): no cavity was made here, the
    /// drains were drilled into it as it was (#447), and `original` is the
    /// model before them. Its drains are drilled again into `original` when
    /// one goes; nothing hollows it again.
    pub(crate) drilled: bool,
}

/// What a job runs. Each lands with the mesh it made ready for picking
/// (#448), built on the job's thread.
enum Work {
    /// Hollowing the original again.
    Hollow(JobHandle<Primed<HollowOutcome>>),
    /// Drilling drains into a mesh as it is (#447).
    Drill(JobHandle<Primed<DrillOutcome>>),
}

/// A drain click waiting for a job to end: the model, the point and the
/// normal it was made at.
type QueuedDrain = (NodeId, [f32; 3], [f32; 3]);

/// A finished job.
enum Landed {
    Hollow(Box<Result<Primed<HollowOutcome>, JobError>>),
    Drill(Box<Result<Primed<DrillOutcome>, JobError>>),
}

impl Work {
    fn progress(&self) -> JobProgress {
        match self {
            Work::Hollow(h) => h.progress(),
            Work::Drill(h) => h.progress(),
        }
    }

    fn cancel(&self) {
        match self {
            Work::Hollow(h) => h.cancel(),
            Work::Drill(h) => h.cancel(),
        }
    }

    fn try_poll(&self) -> Option<Landed> {
        match self {
            Work::Hollow(h) => h.try_poll().map(|landed| Landed::Hollow(Box::new(landed))),
            Work::Drill(h) => h.try_poll().map(|landed| Landed::Drill(Box::new(landed))),
        }
    }
}

/// A job's outcome, with the mesh it made taken out of it, shared, and the
/// caches a first pick on it builds, built (#448).
struct Primed<T> {
    out: T,
    mesh: Option<Arc<IndexedMesh>>,
    primed: Vec<PrimedMesh>,
}

/// Runs `inner` and, on its thread's time, takes the mesh it made out of
/// its outcome (`take`) and builds that mesh's picking caches, so the first
/// click on a new hollow does not build them on the interface thread.
fn primed_job<T: Send + 'static>(
    inner: JobHandle<T>,
    take: fn(&mut T) -> Option<IndexedMesh>,
) -> JobHandle<Primed<T>> {
    JobHandle::spawn(move |worker: &JobWorker<Primed<T>>| {
        let mut out = loop {
            if worker.is_cancelled() {
                inner.cancel();
            }
            worker.report_progress(inner.progress());
            if let Some(out) = inner.try_poll() {
                break out?;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let mesh = take(&mut out).map(Arc::new);
        let primed = mesh.iter().map(|m| PrimedMesh::new(m.clone())).collect();
        Ok(Primed { out, mesh, primed })
    })
}

/// An empty mesh, to take a job's out of it.
fn no_mesh() -> IndexedMesh {
    IndexedMesh::new(None, Vec::new(), Vec::new())
}

/// The mesh an apply made, taken out of its outcome.
fn take_hollow(out: &mut HollowOutcome) -> Option<IndexedMesh> {
    match &mut out.result {
        HollowResult::Apply { mesh, .. } => Some(std::mem::replace(mesh, no_mesh())),
        HollowResult::Preview { .. } => None,
    }
}

/// The mesh a drill made, taken out of its outcome, when it cut a drain.
fn take_drilled(out: &mut DrillOutcome) -> Option<IndexedMesh> {
    out.drains
        .iter()
        .any(|d| d.reaches_cavity)
        .then(|| std::mem::replace(&mut out.mesh, no_mesh()))
}

/// The drains a drill onto a hollow's current mesh keeps (#447): those it
/// already has, with their plugs.
struct Onto {
    drains: Vec<LocalDrain>,
    plugs: Vec<PlugLink>,
}

/// The job in flight and what it needs to land.
struct Job {
    work: Work,
    /// A drill onto the mesh as it is: the drains and plugs it had.
    onto: Option<Onto>,
    leaf: NodeId,
    started_on: SceneRevision,
    /// The leaf's mesh when the job started: the edit's `before`.
    before: Arc<IndexedMesh>,
    original: Arc<IndexedMesh>,
    drains: Vec<LocalDrain>,
    solids: Vec<LassoRegion>,
    options: HollowOptions,
    /// The model's stretch at the start ([`LeafFrame::stretch_rows`]).
    stretch: [f64; 9],
}

pub(crate) struct HollowRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    job: RefCell<Option<Job>>,
    poll_timer: Rc<slint::Timer>,
    /// The document's hollows ([`SceneDocument::hollows`]).
    hollowed: Rc<RefCell<Vec<Hollowed>>>,
    options: RefCell<HollowOptions>,
    /// The hollow (by identity) whose settings the tab took last: selecting
    /// another hollow puts its own settings in the tab.
    adopted: Cell<Option<usize>>,
    /// The drain picked to edit (#450), by its place in the model's list,
    /// and whether the next click moves it there.
    picked_drain: Cell<Option<usize>>,
    moving_drain: Cell<bool>,
    /// The last voxel session, reused when the next job matches it (a
    /// moved or rotated model, a new drain).
    prepared: RefCell<Option<Arc<PreparedHollow>>>,
    _scene_events: SceneSubscription,
    ctrl: Rc<ViewportController>,
    preview: RefCell<CavityPreview>,
    /// Where the cursor rests on a model with the drain tool armed.
    hover: Cell<Option<DrainHover>>,
    preview_timer: slint::Timer,
    /// Drains placed on a model not hollowed yet, by its mesh: drilled when
    /// it is hollowed.
    pending: RefCell<Vec<(usize, Vec<LocalDrain>)>>,
    /// Areas drawn to stay solid on a model not hollowed yet (#179), by its
    /// mesh, in its local frame: kept when it is hollowed.
    pending_solids: RefCell<Vec<(usize, Vec<LassoRegion>)>>,
    /// The local bounding box of the last mesh asked about, by identity, for
    /// the recommended wall (a scan of every vertex once per mesh).
    bounds: RefCell<Option<MeshBounds>>,
    /// Drain clicks made while a job ran (#448): each is made once it ends,
    /// on the model it was made on.
    queued: RefCell<VecDeque<QueuedDrain>>,
    /// The Repair run, for a mesh Hollow cannot use (#448).
    repair: RefCell<Option<Rc<RepairRun>>>,
    /// The selected model read for drilling into it as it is (#447): its
    /// sealed cavities (#130), the drains it takes, its drain map (#455).
    probe: RefCell<ProbeSlot>,
    /// Whether the drain tool shows where a drain works best (#455).
    drain_map_on: Cell<bool>,
    /// Told of each model a drain landed in (#128).
    drained: RefCell<Option<DrainedHook>>,
}

/// What is told of each model a drain landed in (#128).
type DrainedHook = Rc<dyn Fn(NodeId)>;

/// What a probe is of: a mesh (by identity) and the stretch of its pose.
type ProbeKey = (usize, [i64; 9]);

/// What a drain map is of: the mesh, the pose, the drains, the detail.
type MapKey = (usize, [[f64; 4]; 4], u64, u64);

/// The selected model's [`DrillProbe`], built off the interface thread.
#[derive(Default)]
struct ProbeSlot {
    key: Option<ProbeKey>,
    building: Option<std::sync::mpsc::Receiver<Option<Arc<DrillProbe>>>>,
    ready: Option<Arc<DrillProbe>>,
    /// Built since the tab last looked: the model's state may have changed.
    fresh: bool,
    /// The drain map shown (#455), and what it was made for.
    map: Option<(MapKey, Vec<Arc<OverlayMesh>>)>,
    map_job: Option<(MapKey, std::sync::mpsc::Receiver<Vec<Arc<OverlayMesh>>>)>,
}

/// A mesh (by identity) and its local bounding box.
type MeshBounds = (usize, [f32; 3], [f32; 3]);

/// The point under the cursor with the drain tool armed (world), the surface
/// normal there, and whether it is on the selected model (a click elsewhere
/// selects that model first).
#[derive(Debug, Clone, Copy, PartialEq)]
struct DrainHover {
    point: [f32; 3],
    normal: [f32; 3],
    on_selected: bool,
}

/// What the drain under the cursor would do, as Hollow decides it (#302).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainReach {
    /// It opens into the cavity: Hollow drills it.
    Opens,
    /// It does not reach the cavity: Hollow leaves it out.
    Misses,
    /// Not known: the cavity it would open into is not computed.
    Unknown,
    /// It opens, but its set depth would run on into the cavity's far side:
    /// Hollow drills it shorter (#316).
    Capped,
}

impl DrainReach {
    fn tint(self) -> OverlayTint {
        match self {
            DrainReach::Opens | DrainReach::Capped => OverlayTint::Valid,
            DrainReach::Misses => OverlayTint::Invalid,
            DrainReach::Unknown => OverlayTint::Pending,
        }
    }

    /// The tab's `hollow-drain-reach`.
    fn code(self) -> i32 {
        match self {
            DrainReach::Opens => 1,
            DrainReach::Misses => 2,
            DrainReach::Unknown => 3,
            DrainReach::Capped => 4,
        }
    }
}

/// The wall a model of this size wants: 1.5 mm plus 1 % of its longest side,
/// between 1.5 and 4 mm, to 0.1 mm. A 50 mm miniature gets the usual 2 mm, a
/// 150 mm figure 3 mm, a large piece up to 4 mm for its own weight.
fn recommended_wall_mm(longest_side_mm: f32) -> f32 {
    ((1.5 + 0.01 * longest_side_mm).clamp(1.5, 4.0) * 10.0).round() / 10.0
}

/// What the cavity preview was computed for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PreviewKey {
    leaf: NodeId,
    /// The mesh hollowed (the original of a hollow), by identity.
    mesh: usize,
    /// The model's scale ([`LeafFrame::stretch_key`]): a scaled model has
    /// another cavity; a move or a rotation keeps it.
    stretch: [i64; 9],
    options: HollowOptions,
    /// The model's drains ([`drains_key`]) when it has infill: they clear
    /// their channels through it, and the preview shows that (#339). 0
    /// without infill, where they change nothing shown.
    drains: u64,
    /// The areas kept solid ([`solids_key`]): they change the cavity.
    solids: u64,
}

impl PreviewKey {
    /// Whether `other` is the same cavity: drains measured on one are
    /// measured on the other.
    fn same_cavity(&self, other: &PreviewKey) -> bool {
        PreviewKey { drains: 0, ..*self }
            == PreviewKey {
                drains: 0,
                ..*other
            }
    }
}

/// A hash of `solids`, to tell sets of solid areas apart; 0 when there are
/// none.
fn solids_key(solids: &[LassoRegion]) -> u64 {
    if solids.is_empty() {
        return 0;
    }
    solids
        .iter()
        .flat_map(|r| {
            [f32::from(u8::from(r.perspective))]
                .into_iter()
                .chain(r.origin)
                .chain(r.u)
                .chain(r.v)
                .chain(r.w)
                .chain(r.polygon.iter().flatten().copied())
                .chain([f32::NAN])
        })
        .fold(0x8422_2325_cbf2_9ce4u64, |h, c| {
            (h ^ u64::from(c.to_bits())).wrapping_mul(0x0100_0000_01b3)
        })
        | 1
}

/// A hash of `drains`, to tell drain sets apart; 0 when there are none.
fn drains_key(drains: &[LocalDrain]) -> u64 {
    if drains.is_empty() {
        return 0;
    }
    let shape = |v: Option<f32>| v.unwrap_or(f32::NAN);
    drains
        .iter()
        .flat_map(|d| {
            d.center.into_iter().chain(d.direction).chain([
                d.radius_mm,
                shape(d.depth_mm),
                shape(d.inner_radius_mm),
                shape(d.plug_clearance_mm),
            ])
        })
        .fold(0xcbf2_9ce4_8422_2325u64, |h, c| {
            (h ^ u64::from(c.to_bits())).wrapping_mul(0x0100_0000_01b3)
        })
        | 1
}

/// The cavity preview: the job computing one, the last one computed (in the
/// leaf's local frame, with the session that holds it, which measures drains
/// on it), the overlay pieces drawn from them (kept while their inputs hold,
/// so the cursor moving uploads only the drain under it) and the overlay
/// last shown.
#[derive(Default)]
struct CavityPreview {
    job: Option<(JobHandle<HollowOutcome>, PreviewKey)>,
    ready: Option<(PreviewKey, Arc<IndexedMesh>, Arc<PreparedHollow>)>,
    cavity_piece: Option<(CavityPieceKey, Arc<OverlayMesh>)>,
    /// The drains drawn: those that open into the cavity, then those that
    /// do not (red, #753).
    drains_piece: Option<(DrainsPieceKey, Vec<Arc<OverlayMesh>>)>,
    /// How deep the drawn drains go, as the engine measures them on the
    /// ready cavity, whether each opens into it, and what for (#315).
    depths: Option<(DepthsKey, Vec<(f32, bool)>)>,
    /// What the shown overlay was drawn from, and the overlay.
    shown: Option<(ShownKey, Overlay)>,
}

/// A drawn cavity: which cavity mesh, at which pose.
type CavityPieceKey = (usize, [[f64; 4]; 4]);
/// Drawn drains: which, at which pose, how deep (bits of each depth) and
/// whether each opens into the cavity.
type DrainsPieceKey = (Vec<LocalDrain>, [[f64; 4]; 4], Vec<(u32, bool)>);
/// Measured drain depths: which drains, at which pose, on which session,
/// with which settings.
type DepthsKey = (Vec<LocalDrain>, [[f64; 4]; 4], usize, PreviewKey);

/// Everything the overlay is drawn from.
#[derive(Debug, Clone, PartialEq)]
struct ShownKey {
    /// The drain map (#455).
    map: Option<usize>,
    cavity: Option<usize>,
    drains: Option<usize>,
    hover: Option<(DrainHover, DrainReach)>,
    world: [[f64; 4]; 4],
    /// The drain tool's settings, which shape the drain under the cursor.
    tool: HollowOptions,
    /// Bits of the depth the drain under the cursor is drawn to.
    hover_depth: u32,
    /// The drain picked to edit (#450), drawn apart.
    picked: Option<usize>,
}

impl HollowRun {
    /// Wires the Hollow tab; the caller keeps the driver alive for the
    /// window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new_cyclic(|weak: &Weak<Self>| {
            let weak = weak.clone();
            let scene_events = document.subscribe(move |event| {
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::SelectionChanged
                        | SceneEvent::Mutated
                        | SceneEvent::Restructured
                ) {
                    // Out of the notification: the refresh reads the scene.
                    let weak = weak.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(run) = weak.upgrade() {
                            run.refresh_ui();
                        }
                    });
                }
            });
            Self {
                ui: ui.as_weak(),
                document: document.clone(),
                dialogs,
                job: RefCell::new(None),
                poll_timer: Rc::new(slint::Timer::default()),
                hollowed: document.hollows(),
                options: RefCell::new(HollowOptions::default()),
                adopted: Cell::new(None),
                picked_drain: Cell::new(None),
                moving_drain: Cell::new(false),
                prepared: RefCell::new(None),
                _scene_events: scene_events,
                ctrl,
                preview: RefCell::new(CavityPreview::default()),
                hover: Cell::new(None),
                preview_timer: slint::Timer::default(),
                pending: RefCell::new(Vec::new()),
                pending_solids: RefCell::new(Vec::new()),
                bounds: RefCell::new(None),
                queued: RefCell::new(VecDeque::new()),
                repair: RefCell::new(None),
                probe: RefCell::new(ProbeSlot::default()),
                drain_map_on: Cell::new(true),
                drained: RefCell::new(None),
            }
        });
        run.push_options(ui);
        {
            let weak = Rc::downgrade(&run);
            run.preview_timer
                .start(slint::TimerMode::Repeated, PREVIEW_TICK, move || {
                    if let Some(run) = weak.upgrade() {
                        run.tick_preview();
                    }
                });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_hollow_drain_hover(move |x, y, z, nx, ny, nz, hit, on_selected| {
                if let Some(run) = weak.upgrade() {
                    run.hover.set(hit.then_some(DrainHover {
                        point: [x, y, z],
                        normal: [nx, ny, nz],
                        on_selected,
                    }));
                    run.tick_preview();
                }
            });
        }
        {
            let run = run.clone();
            ui.on_hollow_apply(move || run.on_apply());
        }
        {
            let run = run.clone();
            ui.on_hollow_cancel(move || {
                run.queued.borrow_mut().clear();
                if let Some(job) = run.job.borrow().as_ref() {
                    job.work.cancel();
                }
            });
        }
        {
            let run = run.clone();
            ui.on_hollow_restore(move || run.on_restore());
        }
        {
            let run = run.clone();
            ui.on_hollow_clear_drains(move || run.on_clear_drains());
        }
        {
            let run = run.clone();
            ui.on_hollow_drain_step(move |by| run.step_drain(by));
        }
        {
            let run = run.clone();
            ui.on_hollow_drain_remove(move || run.remove_picked_drain());
        }
        {
            let run = run.clone();
            ui.on_hollow_drain_move(move || run.move_picked_drain());
        }
        {
            let run = run.clone();
            ui.on_hollow_option_changed(move |code, value| run.on_option_changed(code, value));
        }
        {
            let run = run.clone();
            ui.on_hollow_drain_clicked(move |x, y, z, nx, ny, nz| {
                run.on_drain_click([x, y, z], [nx, ny, nz]);
            });
        }
        {
            let weak = Rc::downgrade(&run);
            run.ctrl.set_lasso_handler(Rc::new(move |region| {
                if let Some(run) = weak.upgrade() {
                    run.on_lasso(region);
                }
            }));
        }
        {
            let run = run.clone();
            ui.on_hollow_remove_last_solid(move || {
                run.edit_solids(|solids| {
                    solids.pop();
                })
            });
        }
        {
            let run = run.clone();
            ui.on_hollow_clear_solids(move || run.edit_solids(Vec::clear));
        }
        {
            let run = run.clone();
            ui.on_hollow_drain_map_toggled(move |on| {
                run.drain_map_on.set(on);
                run.tick_preview();
            });
        }
        ui.set_hollow_drain_map(true);
        run
    }

    /// The Repair run that a mesh Hollow cannot use is sent to (#448).
    pub(crate) fn set_repair(&self, repair: Rc<RepairRun>) {
        *self.repair.borrow_mut() = Some(repair);
    }

    /// Tells `drained` of each model a drain lands in, drilled or hollowed
    /// with it, once the edit is in the scene (#128). A drain left out, a
    /// failed or cancelled job tells nothing.
    pub(crate) fn on_drained(&self, drained: Rc<dyn Fn(NodeId)>) {
        *self.drained.borrow_mut() = Some(drained);
    }

    fn tell_drained(&self, leaf: NodeId) {
        let drained = self.drained.borrow().clone();
        if let Some(drained) = drained {
            drained(leaf);
        }
    }

    /// The areas a model keeps solid (#179), in its local frame: a hollow's
    /// own, or those drawn on a solid model so far.
    fn solids_for(&self, mesh: &Arc<IndexedMesh>) -> Vec<LassoRegion> {
        if let Some(h) = self
            .hollowed
            .borrow()
            .iter()
            .find(|h| Arc::ptr_eq(&h.hollow, mesh))
        {
            return h.solids.clone();
        }
        let ptr = Arc::as_ptr(mesh) as usize;
        self.pending_solids
            .borrow()
            .iter()
            .find(|(m, _)| *m == ptr)
            .map(|(_, s)| s.clone())
            .unwrap_or_default()
    }

    fn set_pending_solids_at(&self, ptr: usize, solids: Vec<LassoRegion>) {
        let mut pending = self.pending_solids.borrow_mut();
        pending.retain(|(m, _)| *m != ptr);
        if !solids.is_empty() {
            pending.push((ptr, solids));
        }
    }

    /// A lasso closed over the viewport (#179): what lies behind it, given
    /// in the world at the current pose, stays solid on the selected model.
    fn on_lasso(self: &Rc<Self>, region: LassoRegion) {
        let Some((_, _, world)) = self.selected() else {
            return;
        };
        let Some(local) = region_to_local(&region, &world) else {
            return;
        };
        self.edit_solids(|solids| solids.push(local));
    }

    /// Changes the selected model's solid areas with `change`: a hollow is
    /// hollowed again with them (one undo step), a solid model keeps them
    /// for when it is hollowed, and its cavity preview follows.
    fn edit_solids(self: &Rc<Self>, change: impl FnOnce(&mut Vec<LassoRegion>)) {
        if self.job.borrow().is_some() {
            return;
        }
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        if self.model_state(&mesh) == 2 {
            return;
        }
        let mut solids = self.solids_for(&mesh);
        let before = solids.clone();
        change(&mut solids);
        if solids == before {
            return;
        }
        if self.entry_for(&mesh).is_some() {
            let (original, drains) = self.source_for(&mesh);
            self.launch_with(leaf, mesh, original, drains, solids, &world);
        } else {
            let drains = self.source_for(&mesh).1;
            self.change_marks(&mesh, (drains, solids), "solid areas");
        }
    }

    /// The selected leaf, its mesh and its world transform.
    fn selected(&self) -> Option<(NodeId, Arc<IndexedMesh>, Transform3D)> {
        let leaf = self.document.selection().primary()?.0;
        self.document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                let mesh = scene.leaf_mesh_arc(leaf)?.clone();
                let world = scene.world_transform_of(leaf).ok()?;
                Some((leaf, mesh, world))
            })
            .flatten()
    }

    /// 0 a solid model, 1 a hollow this driver made, 2 the support mesh, 3
    /// a model that came hollow (from a file, sealed cavities in it, or one
    /// drilled here as it was): drains are drilled into it as it is (#447),
    /// and nothing hollows it again.
    fn model_state(&self, mesh: &Arc<IndexedMesh>) -> i32 {
        if sc_geom::supports::is_any_support_mesh(mesh) {
            return 2;
        }
        let drilled = self
            .hollowed
            .borrow()
            .iter()
            .find(|h| Arc::ptr_eq(&h.hollow, mesh))
            .map(|h| h.drilled);
        match drilled {
            Some(true) => 3,
            Some(false) => 1,
            None if self
                .probe_of(mesh)
                .is_some_and(|probe| probe.sealed_cavities() > 0) =>
            {
                3
            }
            None => 0,
        }
    }

    /// The record of the hollow `mesh` (by identity), made or drilled here.
    fn record_of(&self, mesh: &Arc<IndexedMesh>) -> Option<Hollowed> {
        self.hollowed
            .borrow()
            .iter()
            .find(|h| Arc::ptr_eq(&h.hollow, mesh))
            .cloned()
    }

    /// The probe of `mesh` posed by `world` (#447), once built; starts
    /// building it off this thread when it is not.
    fn probe_for(&self, mesh: &Arc<IndexedMesh>, world: &Transform3D) -> Option<Arc<DrillProbe>> {
        let key = (
            Arc::as_ptr(mesh) as usize,
            LeafFrame::of(world).ok()?.stretch_key(),
        );
        let mut slot = self.probe.borrow_mut();
        if slot.key != Some(key) {
            *slot = ProbeSlot {
                key: Some(key),
                ..ProbeSlot::default()
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let (mesh, world) = (mesh.clone(), *world);
            let _ = std::thread::Builder::new()
                .name("hollow-drill-probe".to_owned())
                .spawn(move || {
                    let _ = tx.send(DrillProbe::new(&mesh, &world).ok().map(Arc::new));
                });
            slot.building = Some(rx);
        }
        let got = slot.building.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(probe) = got {
            slot.ready = probe;
            slot.building = None;
            slot.fresh = true;
        }
        slot.ready.clone()
    }

    /// [`Self::probe_for`], waited for: a click needs the answer now.
    fn probe_now(&self, mesh: &Arc<IndexedMesh>, world: &Transform3D) -> Option<Arc<DrillProbe>> {
        if let Some(probe) = self.probe_for(mesh, world) {
            return Some(probe);
        }
        let building = self.probe.borrow_mut().building.take();
        let probe = building.and_then(|rx| rx.recv().ok()).flatten();
        let mut slot = self.probe.borrow_mut();
        slot.ready = probe.clone();
        slot.fresh = true;
        probe
    }

    /// The built probe of `mesh` (by identity), at any pose.
    fn probe_of(&self, mesh: &Arc<IndexedMesh>) -> Option<Arc<DrillProbe>> {
        let slot = self.probe.borrow();
        let ptr = Arc::as_ptr(mesh) as usize;
        slot.ready
            .clone()
            .filter(|_| slot.key.is_some_and(|(p, _)| p == ptr))
    }

    /// The entry of a hollow mesh, by identity.
    fn entry_for(&self, mesh: &Arc<IndexedMesh>) -> Option<(Arc<IndexedMesh>, Vec<LocalDrain>)> {
        self.hollowed
            .borrow()
            .iter()
            .find(|h| Arc::ptr_eq(&h.hollow, mesh))
            .map(|h| (h.original.clone(), h.drains.clone()))
    }

    /// What a model is hollowed from and its drains: a hollow's original and
    /// drilled drains, or a solid model and the drains placed on it so far.
    fn source_for(&self, mesh: &Arc<IndexedMesh>) -> (Arc<IndexedMesh>, Vec<LocalDrain>) {
        self.entry_for(mesh).unwrap_or_else(|| {
            let ptr = Arc::as_ptr(mesh) as usize;
            let drains = self
                .pending
                .borrow()
                .iter()
                .find(|(m, _)| *m == ptr)
                .map(|(_, d)| d.clone())
                .unwrap_or_default();
            (mesh.clone(), drains)
        })
    }

    fn set_pending_at(&self, ptr: usize, drains: Vec<LocalDrain>) {
        let mut pending = self.pending.borrow_mut();
        pending.retain(|(m, _)| *m != ptr);
        if !drains.is_empty() {
            pending.push((ptr, drains));
        }
    }

    /// The marks placed on a solid model so far (#512): its drains and its
    /// solid areas.
    fn marks_of(&self, mesh: &Arc<IndexedMesh>) -> Marks {
        (self.source_for(mesh).1, self.solids_for(mesh))
    }

    /// Puts `after` as the marks of the solid model `mesh`, as one step of
    /// history that Undo takes back to what it had (#512), and brings the
    /// tab and the cavity preview in line.
    fn change_marks(self: &Rc<Self>, mesh: &Arc<IndexedMesh>, after: Marks, label: &'static str) {
        let before = self.marks_of(mesh);
        if before == after {
            return;
        }
        let ptr = Arc::as_ptr(mesh) as usize;
        let put = |marks: Marks| -> Rc<dyn Fn()> {
            let weak = Rc::downgrade(self);
            Rc::new(move || {
                if let Some(run) = weak.upgrade() {
                    run.put_marks(ptr, marks.clone());
                }
            })
        };
        let step = MarksStep {
            label,
            undo: put(before),
            redo: put(after.clone()),
        };
        self.put_marks(ptr, after);
        self.document.push_marks_step(step);
    }

    fn put_marks(&self, ptr: usize, (drains, solids): Marks) {
        self.set_pending_at(ptr, drains);
        self.set_pending_solids_at(ptr, solids);
        self.refresh_ui();
        self.tick_preview();
    }

    /// Hollows the selected model with a `wall_mm` wall and a drain at
    /// each of `drains` (a world point and the surface normal there), then
    /// and there, as the tab's buttons and the poll do: for other modules'
    /// tests.
    #[cfg(test)]
    pub(crate) fn hollow_now(self: &Rc<Self>, wall_mm: f32, drains: &[([f32; 3], [f32; 3])]) {
        self.on_option(0, wall_mm);
        for &(point, normal) in drains {
            self.on_drain_click(point, normal);
            self.land_now();
        }
        self.on_apply();
        self.land_now();
    }

    /// Makes the selected hollow solid again, as the tab's button does: for
    /// other modules' tests.
    #[cfg(test)]
    pub(crate) fn restore_now(&self) {
        self.on_restore();
    }

    /// Waits for the job in flight, if any, and lands it as the poll would.
    #[cfg(test)]
    fn land_now(self: &Rc<Self>) {
        let waited = std::time::Instant::now();
        loop {
            if self.job.borrow().is_none() {
                return;
            }
            let landed = self.job.borrow().as_ref().and_then(|j| j.work.try_poll());
            if let Some(landed) = landed {
                let job = self.job.borrow_mut().take().expect("the job");
                self.land(job, landed);
                self.next_queued();
                continue;
            }
            assert!(
                waited.elapsed() < Duration::from_secs(120),
                "the hollow ends"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The selected model's longest side in the world (its bounding box at
    /// the current pose).
    fn longest_side_mm(&self, mesh: &Arc<IndexedMesh>, world: &Transform3D) -> f32 {
        let ptr = Arc::as_ptr(mesh) as usize;
        let (lo, hi) = {
            let mut cached = self.bounds.borrow_mut();
            match *cached {
                Some((p, lo, hi)) if p == ptr => (lo, hi),
                _ => {
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    for v in &mesh.vertices {
                        for k in 0..3 {
                            lo[k] = lo[k].min(v.as_array()[k]);
                            hi[k] = hi[k].max(v.as_array()[k]);
                        }
                    }
                    *cached = Some((ptr, lo, hi));
                    (lo, hi)
                }
            }
        };
        let corners = (0..8).map(|c| {
            let p = std::array::from_fn(|k| if c >> k & 1 == 0 { lo[k] } else { hi[k] });
            world.apply_to_vertex(Vertex::new(p)).coords()
        });
        let (mut wlo, mut whi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in corners {
            for k in 0..3 {
                wlo[k] = wlo[k].min(p[k]);
                whi[k] = whi[k].max(p[k]);
            }
        }
        (0..3).map(|k| whi[k] - wlo[k]).fold(0.0, f32::max)
    }

    fn refresh_ui(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (state, drains) = match self.selected() {
            Some((_, mesh, world)) => {
                self.adopt_options_of(&mesh, &ui);
                let (original, drains) = self.source_for(&mesh);
                ui.set_hollow_recommended_wall(recommended_wall_mm(
                    self.longest_side_mm(&original, &world),
                ));
                (self.model_state(&mesh), drains.len())
            }
            None => (2, 0),
        };
        let solids = self
            .selected()
            .map_or(0, |(_, mesh, _)| self.solids_for(&mesh).len());
        ui.set_hollow_model_state(state);
        ui.set_hollow_drain_count(drains as i32);
        // The cavities still closed all round (#130): -1 while not counted.
        let sealed = self
            .selected()
            .filter(|_| matches!(state, 1 | 3))
            .and_then(|(_, mesh, _)| self.probe_of(&mesh))
            .map_or(-1, |probe| probe.sealed_cavities() as i32);
        ui.set_hollow_sealed_count(sealed);
        if self.picked_drain.get().is_some_and(|i| i >= drains) {
            self.picked_drain.set(None);
            self.moving_drain.set(false);
        }
        ui.set_hollow_drain_picked(self.picked_drain.get().map_or(-1, |i| i as i32));
        ui.set_hollow_drain_moving(self.moving_drain.get());
        ui.set_hollow_solid_count(solids as i32);
        if state == 2 {
            ui.set_hollow_drain_tool(false);
            ui.set_hollow_lasso_tool(false);
        }
    }

    /// Puts the settings `mesh` was hollowed with in the tab, the first time
    /// that hollow is selected: a new drain then hollows it the same way.
    fn adopt_options_of(&self, mesh: &Arc<IndexedMesh>, ui: &MainWindow) {
        let ptr = Arc::as_ptr(mesh) as usize;
        if self.adopted.get() == Some(ptr) {
            return;
        }
        let options = self
            .hollowed
            .borrow()
            .iter()
            .find(|h| Arc::ptr_eq(&h.hollow, mesh))
            .map(|h| h.options);
        if let Some(options) = options {
            self.adopted.set(Some(ptr));
            *self.options.borrow_mut() = options;
            self.push_options(ui);
        }
    }

    fn push_options(&self, ui: &MainWindow) {
        let o = *self.options.borrow();
        ui.set_hollow_wall(o.wall_mm);
        ui.set_hollow_voxel(o.voxel_mm);
        ui.set_hollow_min_width(o.min_width_mm);
        ui.set_hollow_fill_index(i32::from(o.fill));
        ui.set_hollow_cell(o.cell_mm);
        ui.set_hollow_beam(o.beam_mm);
        ui.set_hollow_drain_radius(o.drain_radius_mm);
        ui.set_hollow_drain_depth_set(o.drain_depth_set);
        ui.set_hollow_drain_depth(o.drain_depth_mm);
        ui.set_hollow_drain_cone(o.drain_cone);
        ui.set_hollow_drain_inner_radius(o.drain_inner_radius_mm);
        ui.set_hollow_drain_plug(o.drain_plug);
        ui.set_hollow_plug_clearance(o.plug_clearance_mm);
    }

    /// An option change from the tab: 0 wall, 1 voxel, 3 fill, 4 infill
    /// cell, 5 beam radius, 6 drain radius (2 was the smoothing, gone with
    /// #29); the drain tool's shape: 7 depth set (0 or 1), 8 that depth, 9
    /// cone (0 or 1), 10 its inner radius, 11 plug (0 or 1), 12 its
    /// clearance; 13 the narrowest cavity (#454). Values are clamped to what
    /// the engine accepts.
    fn on_option(&self, code: i32, value: f32) {
        {
            let mut o = self.options.borrow_mut();
            let on = value >= 0.5;
            match code {
                0 => o.wall_mm = value.clamp(0.5, 10.0),
                1 => o.voxel_mm = value.clamp(0.1, 3.0),
                3 => o.fill = (value.round() as i32).clamp(0, 2) as u8,
                4 => o.cell_mm = value.clamp(1.0, 30.0),
                5 => o.beam_mm = value.clamp(0.1, 5.0),
                6 => o.drain_radius_mm = value.clamp(0.3, 10.0),
                7 => o.drain_depth_set = on,
                8 => o.drain_depth_mm = value.clamp(0.5, 100.0),
                9 => o.drain_cone = on,
                10 => o.drain_inner_radius_mm = value.clamp(0.3, 10.0),
                11 => o.drain_plug = on,
                12 => o.plug_clearance_mm = value.clamp(0.0, 1.0),
                13 => o.min_width_mm = value.clamp(0.0, 20.0),
                _ => return,
            }
        }
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
        self.tick_preview();
    }

    /// An option change from the tab (see [`Self::on_option`]); a drain's
    /// shape changes the picked drain too (#450).
    fn on_option_changed(self: &Rc<Self>, code: i32, value: f32) {
        self.on_option(code, value);
        if (6..=12).contains(&code) {
            self.reshape_picked_drain();
        }
    }

    fn on_apply(self: &Rc<Self>) {
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        // A model that came hollow is not hollowed again (#447).
        if matches!(self.model_state(&mesh), 2 | 3) {
            return;
        }
        // Hollowing a hollow again starts from its original, keeping drains;
        // a solid model takes the drains placed on it.
        let (original, drains) = self.source_for(&mesh);
        // What the slicer refuses, Hollow cannot use either (#448).
        if !original.is_sliceable() {
            self.offer_repair(leaf, &original);
            return;
        }
        self.launch(leaf, mesh, original, drains, &world);
    }

    /// Says the model's mesh is not one closed solid, so it cannot be
    /// hollowed or drilled, with a Repair button that mends it and then
    /// hollows it (#448).
    fn offer_repair(self: &Rc<Self>, leaf: NodeId, mesh: &Arc<IndexedMesh>) {
        let old = Arc::as_ptr(mesh) as usize;
        let weak = Rc::downgrade(self);
        self.dialogs.show_error_with_action(
            || ErrorTexts {
                heading: ctr!("HollowRun", "This model can't be hollowed as it is"),
                context: None,
                message: ctr!(
                    "HollowRun",
                    "Its mesh is not one closed solid: it has holes, or faces that cross or overlap, and the slicer would refuse it too. Repair mends it, and it is hollowed right after."
                ),
                details: Default::default(),
            },
            || ctr!("ErrorDialog", "Repair"),
            move || {
                if let Some(run) = weak.upgrade() {
                    run.repair_and_hollow(leaf, old);
                }
            },
        );
    }

    /// Repairs `leaf` and hollows it once it is mended (#448); the marks
    /// placed on its mesh before (by identity, `old`) go with it.
    fn repair_and_hollow(self: &Rc<Self>, leaf: NodeId, old: usize) {
        let Some(repair) = self.repair.borrow().clone() else {
            return;
        };
        let weak = Rc::downgrade(self);
        repair.repair_for_slice(&[leaf], move || {
            let Some(run) = weak.upgrade() else {
                return;
            };
            if let Some((selected, mesh, _)) = run.selected()
                && selected == leaf
            {
                run.move_marks(old, &mesh);
                run.on_apply();
            }
        });
    }

    /// The marks placed on the mesh `old` (by identity) move to `mesh`.
    fn move_marks(&self, old: usize, mesh: &Arc<IndexedMesh>) {
        let new = Arc::as_ptr(mesh) as usize;
        for (m, _) in self.pending.borrow_mut().iter_mut() {
            if *m == old {
                *m = new;
            }
        }
        for (m, _) in self.pending_solids.borrow_mut().iter_mut() {
            if *m == old {
                *m = new;
            }
        }
    }

    fn on_restore(&self) {
        let Some((leaf, mesh, _)) = self.selected() else {
            return;
        };
        self.back_to_original(
            leaf,
            mesh,
            ctr!("HollowRun", "Solid again. Undo brings the hollow back."),
        );
    }

    /// Puts the model the hollow `mesh` was made or drilled from back on
    /// `leaf`, its plugs taken away, saying `done`.
    fn back_to_original(&self, leaf: NodeId, mesh: Arc<IndexedMesh>, done: slint::SharedString) {
        let Some((original, _)) = self.entry_for(&mesh) else {
            return;
        };
        // The plugs go with the hollow.
        let old = self.plugs_of(&mesh);
        self.swap_mesh_and_plugs(leaf, mesh, original, &old, Vec::new(), &[]);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_last_run(done);
        }
    }

    fn on_clear_drains(self: &Rc<Self>) {
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        let Some((_, drains)) = self.entry_for(&mesh) else {
            let solids = self.solids_for(&mesh);
            self.change_marks(&mesh, (Vec::new(), solids), "drain marks cleared");
            return;
        };
        if !drains.is_empty() {
            self.set_drains(leaf, mesh, &world, Vec::new(), "drains cleared");
        }
    }

    /// A click with the drain tool at a world point and surface normal on
    /// the selected model (the viewport selects the model it hit first). A
    /// hollow is hollowed again with the new drain; a solid model keeps it
    /// for when it is hollowed.
    fn on_drain_click(self: &Rc<Self>, point: [f32; 3], normal: [f32; 3]) {
        // A click while a job runs waits for it (#448): two quick clicks
        // make two drains.
        if self.job.borrow().is_some() {
            if let Some((leaf, ..)) = self.selected() {
                self.queued.borrow_mut().push_back((leaf, point, normal));
            }
            return;
        }
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        if self.model_state(&mesh) == 2 {
            return;
        }
        let options = *self.options.borrow();
        let (_, mut drains) = self.source_for(&mesh);
        // The picked drain goes where the click is, as it is (#450).
        if self.moving_drain.replace(false)
            && let Some(i) = self.picked_drain.get()
            && let Some(old) = drains.get(i).copied()
        {
            let Some(moved) = LocalDrain::at(point, normal, old.radius_mm, &world) else {
                return;
            };
            drains[i] = LocalDrain {
                center: moved.center,
                direction: moved.direction,
                ..old
            };
            self.set_drains(leaf, mesh, &world, drains, "drain moved");
            return;
        }
        // A click on a drain picks it to edit (#450).
        if let Some(i) = self.drain_near(&drains, &world, point) {
            self.pick_drain(Some(i), &drains);
            return;
        }
        let Some(drain) = LocalDrain::at(point, normal, options.drain_radius_mm, &world)
            .map(|d| d.shaped(&options))
        else {
            return;
        };
        self.picked_drain.set(None);
        self.add_drain(leaf, mesh, &world, drain, drains);
    }

    /// Puts a new `drain` on the model, with `drains` (those it has so far):
    /// a hollow made here is hollowed again with it (the engine knows its
    /// cavity, and with its session kept that is the quicker way: 0.2 s
    /// against 1.5 s on a 91 mm figure); a model that came hollow takes it
    /// drilled into its mesh as it is (#447); a solid model keeps it for
    /// when it is hollowed.
    fn add_drain(
        self: &Rc<Self>,
        leaf: NodeId,
        mesh: Arc<IndexedMesh>,
        world: &Transform3D,
        drain: LocalDrain,
        mut drains: Vec<LocalDrain>,
    ) {
        match self.record_of(&mesh) {
            Some(h) if h.drilled => {
                let onto = Onto {
                    drains: h.drains.clone(),
                    plugs: h.plugs.clone(),
                };
                let original = h.original.clone();
                self.drill(leaf, mesh, original, vec![drain], world, Some(onto));
                return;
            }
            Some(h) => {
                drains.push(drain);
                self.launch(leaf, mesh, h.original.clone(), drains, world);
                return;
            }
            None => {}
        }
        let opens = self
            .probe_now(&mesh, world)
            .and_then(|probe| probe.report(world, &drain.to_world(world)))
            .is_some_and(|report| report.reaches_cavity);
        if opens {
            self.drill(leaf, mesh.clone(), mesh, vec![drain], world, None);
        } else {
            drains.push(drain);
            let solids = self.solids_for(&mesh);
            self.change_marks(&mesh, (drains, solids), "drain mark");
        }
    }

    /// The clicks made while the last job ran, one by one, while no job runs
    /// (#448); a click made on another model than the one selected now is
    /// dropped.
    fn next_queued(self: &Rc<Self>) {
        while self.job.borrow().is_none() {
            let Some((leaf, point, normal)) = self.queued.borrow_mut().pop_front() else {
                return;
            };
            if self
                .selected()
                .is_some_and(|(selected, ..)| selected == leaf)
            {
                self.on_drain_click(point, normal);
            }
        }
    }

    /// The drain of `drains` whose mouth `point` (world) is on: within its
    /// radius and a millimetre.
    fn drain_near(
        &self,
        drains: &[LocalDrain],
        world: &Transform3D,
        point: [f32; 3],
    ) -> Option<usize> {
        drains
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let w = d.to_world(world);
                let gap = (0..3)
                    .map(|k| (w.center[k] - point[k]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                (i, gap, w.radius_mm)
            })
            .filter(|&(_, gap, radius)| gap <= radius + 1.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, ..)| i)
    }

    /// Picks drain `index` of `drains` to edit (#450): the tab's drain fields
    /// show its shape and change it.
    fn pick_drain(&self, index: Option<usize>, drains: &[LocalDrain]) {
        self.picked_drain.set(index);
        self.moving_drain.set(false);
        if let Some(d) = index.and_then(|i| drains.get(i)) {
            let mut o = self.options.borrow_mut();
            o.drain_radius_mm = d.radius_mm;
            o.drain_depth_set = d.depth_mm.is_some();
            if let Some(depth) = d.depth_mm {
                o.drain_depth_mm = depth;
            }
            o.drain_cone = d.inner_radius_mm.is_some();
            if let Some(inner) = d.inner_radius_mm {
                o.drain_inner_radius_mm = inner;
            }
            o.drain_plug = d.plug_clearance_mm.is_some();
            if let Some(clearance) = d.plug_clearance_mm {
                o.plug_clearance_mm = clearance;
            }
        }
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
        self.refresh_ui();
        self.tick_preview();
    }

    /// Puts `drains` on the model: a hollow is hollowed again with them, a
    /// solid model keeps them as marks; one step of history either way.
    fn set_drains(
        self: &Rc<Self>,
        leaf: NodeId,
        mesh: Arc<IndexedMesh>,
        world: &Transform3D,
        drains: Vec<LocalDrain>,
        label: &'static str,
    ) {
        match self.record_of(&mesh) {
            // Drilled as it came (#447): its drains again, into the model
            // before them; none left gives that model back.
            Some(h) if h.drilled => {
                if drains.is_empty() {
                    self.back_to_original(
                        leaf,
                        mesh,
                        ctr!("HollowRun", "The drains are out. Undo puts them back."),
                    );
                } else {
                    self.drill(leaf, mesh, h.original.clone(), drains, world, None);
                }
            }
            Some(h) => self.launch(leaf, mesh, h.original.clone(), drains, world),
            None => {
                let solids = self.solids_for(&mesh);
                self.change_marks(&mesh, (drains, solids), label);
            }
        }
    }

    /// Drills `drains` into a mesh as it is (#447), no cavity computed: with
    /// `onto` (the drains it has), into the leaf's `before`; without, into
    /// `original` afresh (the model as it came, before any drain).
    fn drill(
        self: &Rc<Self>,
        leaf: NodeId,
        before: Arc<IndexedMesh>,
        original: Arc<IndexedMesh>,
        drains: Vec<LocalDrain>,
        world: &Transform3D,
        onto: Option<Onto>,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.job.borrow().is_some() || ui.get_loading() {
            return;
        }
        let Ok(frame) = LeafFrame::of(world) else {
            return;
        };
        let input = if onto.is_some() {
            before.clone()
        } else {
            original.clone()
        };
        let world_drains = drains.iter().map(|d| d.to_world(world)).collect();
        // The model as the tab read it, when that is this mesh.
        let probe = self.probe_of(&input);
        match start_drill_mesh(input, world, world_drains, probe) {
            Ok(handle) => {
                let solids = self.solids_for(&before);
                *self.job.borrow_mut() = Some(Job {
                    work: Work::Drill(primed_job(handle, take_drilled)),
                    onto,
                    leaf,
                    started_on: self.document.scene_revision(),
                    before,
                    original,
                    drains,
                    solids,
                    options: *self.options.borrow(),
                    stretch: frame.stretch_rows(),
                });
                ui.set_hollow_running(true);
                crate::breadcrumbs::started("drill");
                ui.set_hollow_progress(0.0);
                ui.set_hollow_phase(slint::SharedString::default());
                self.start_poll();
            }
            Err(err) => {
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Drilling failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
            }
        }
    }

    /// The next (`by` 1) or previous (-1) drain, to edit (#450).
    fn step_drain(&self, by: i32) {
        let Some((_, mesh, _)) = self.selected() else {
            return;
        };
        let drains = self.source_for(&mesh).1;
        if drains.is_empty() {
            return;
        }
        let n = drains.len() as i32;
        let next = match self.picked_drain.get() {
            Some(i) => (i as i32 + by).rem_euclid(n),
            None if by < 0 => n - 1,
            None => 0,
        };
        self.pick_drain(Some(next as usize), &drains);
    }

    /// Takes the picked drain away (#450), its plug with it.
    fn remove_picked_drain(self: &Rc<Self>) {
        let (Some(i), Some((leaf, mesh, world))) = (self.picked_drain.get(), self.selected())
        else {
            return;
        };
        if self.job.borrow().is_some() {
            return;
        }
        let mut drains = self.source_for(&mesh).1;
        if i >= drains.len() {
            return;
        }
        drains.remove(i);
        self.picked_drain.set(None);
        self.set_drains(leaf, mesh, &world, drains, "drain removed");
    }

    /// The next click on the model moves the picked drain there (#450).
    fn move_picked_drain(&self) {
        if self.picked_drain.get().is_none() {
            return;
        }
        self.moving_drain.set(!self.moving_drain.get());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_drain_tool(true);
        }
        self.refresh_ui();
    }

    /// The picked drain takes the tab's drain shape (#450), when one is
    /// picked; `true` when it did.
    fn reshape_picked_drain(self: &Rc<Self>) -> bool {
        let (Some(i), Some((leaf, mesh, world))) = (self.picked_drain.get(), self.selected())
        else {
            return false;
        };
        let options = *self.options.borrow();
        let mut drains = self.source_for(&mesh).1;
        let Some(d) = drains.get(i).copied() else {
            return false;
        };
        let reshaped = LocalDrain {
            radius_mm: options.drain_radius_mm,
            ..d
        }
        .shaped(&options);
        if reshaped == d {
            return true;
        }
        drains[i] = reshaped;
        if self.job.borrow().is_some() {
            return true;
        }
        self.set_drains(leaf, mesh, &world, drains, "drain changed");
        true
    }

    /// Of the leaves `wanted` takes, the one with its underside nearest below
    /// `at` (world mm), with that point.
    fn underside_below(
        &self,
        at: [f32; 3],
        wanted: impl Fn(&Arc<IndexedMesh>) -> bool,
    ) -> Option<(NodeId, Arc<IndexedMesh>, Transform3D, [f32; 3])> {
        self.document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter_map(|leaf| {
                        let mesh = scene.leaf_mesh_arc(leaf)?;
                        if !wanted(mesh) {
                            return None;
                        }
                        let world = scene.world_transform_of(leaf).ok()?;
                        let exit = floor_underside(mesh, &world, at)?;
                        Some((leaf, mesh.clone(), world, exit))
                    })
                    .max_by(|a, b| a.3[2].total_cmp(&b.3[2]))
            })
            .flatten()
    }

    /// The drain the review suggests for a cavity whose floor is at `at`
    /// (world mm): on a hollow made here, on the underside of that floor
    /// straight below `at`, pointing up into the cavity. Of several hollows,
    /// the one whose underside is nearest below `at`.
    fn drain_target(
        &self,
        at: [f32; 3],
    ) -> Option<(NodeId, Arc<IndexedMesh>, Transform3D, LocalDrain)> {
        let radius_mm = self.options.borrow().drain_radius_mm;
        let hollows: Vec<Arc<IndexedMesh>> = self
            .hollowed
            .borrow()
            .iter()
            .map(|h| h.hollow.clone())
            .collect();
        let (leaf, mesh, world, exit) =
            self.underside_below(at, |mesh| hollows.iter().any(|h| Arc::ptr_eq(h, mesh)))?;
        let drain = LocalDrain::at(exit, [0.0, 0.0, -1.0], radius_mm, &world)?;
        Some((leaf, mesh, world, drain))
    }

    /// The model hollowed here whose hollow holds the cavity at `at` (world
    /// mm), and how many drain holes that hollow has; `None` when no model
    /// hollowed here holds it.
    pub(crate) fn hollow_at(&self, at: [f32; 3]) -> Option<(NodeId, usize)> {
        let (leaf, mesh, _, _) = self.drain_target(at)?;
        Some((leaf, self.source_for(&mesh).1.len()))
    }

    /// The model holding the cavity whose floor is at `at` (world mm): a
    /// hollow made here, else the model with its underside nearest below
    /// `at`, else, for a cavity closed by the plate, with nothing under its
    /// floor, the one whose box holds `at`.
    pub(crate) fn model_at(&self, at: [f32; 3]) -> Option<NodeId> {
        if let Some((leaf, _)) = self.hollow_at(at) {
            return Some(leaf);
        }
        let is_model = |mesh: &Arc<IndexedMesh>| !sc_geom::supports::is_any_support_mesh(mesh);
        if let Some((leaf, ..)) = self.underside_below(at, is_model) {
            return Some(leaf);
        }
        self.document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .find(|&leaf| {
                        scene.leaf_mesh_arc(leaf).is_some_and(|mesh| {
                            is_model(mesh)
                                && scene
                                    .world_transform_of(leaf)
                                    .is_ok_and(|world| box_holds(mesh, &world, at))
                        })
                    })
            })
            .flatten()
    }

    /// Hollows `original` into the leaf showing `before`, with `drains` and
    /// the solid areas `before` keeps.
    fn launch(
        self: &Rc<Self>,
        leaf: NodeId,
        before: Arc<IndexedMesh>,
        original: Arc<IndexedMesh>,
        drains: Vec<LocalDrain>,
        world: &Transform3D,
    ) {
        let solids = self.solids_for(&before);
        self.launch_with(leaf, before, original, drains, solids, world);
    }

    fn launch_with(
        self: &Rc<Self>,
        leaf: NodeId,
        before: Arc<IndexedMesh>,
        original: Arc<IndexedMesh>,
        drains: Vec<LocalDrain>,
        solids: Vec<LassoRegion>,
        world: &Transform3D,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.job.borrow().is_some() || ui.get_loading() {
            return;
        }
        let options = *self.options.borrow();
        let Ok(frame) = LeafFrame::of(world) else {
            return;
        };
        let request = HollowRequest {
            kind: HollowKind::Apply,
            grid: GridRequest {
                voxel_size_mm: options.voxel_mm,
                ..GridRequest::default()
            },
            settings: options.settings(),
            drains: drains.iter().map(|d| d.to_world(world)).collect(),
            solid_areas: solid_areas(&solids, world),
            prepared: self.prepared.borrow().clone(),
            ..HollowRequest::default()
        };
        match start_hollow_mesh(original.clone(), world, request) {
            Ok(handle) => {
                *self.job.borrow_mut() = Some(Job {
                    work: Work::Hollow(primed_job(handle, take_hollow)),
                    onto: None,
                    leaf,
                    started_on: self.document.scene_revision(),
                    before,
                    original,
                    drains,
                    solids,
                    options,
                    stretch: frame.stretch_rows(),
                });
                ui.set_hollow_running(true);
                crate::breadcrumbs::started("hollow");
                ui.set_hollow_progress(0.0);
                ui.set_hollow_phase(slint::SharedString::default());
                self.start_poll();
            }
            Err(err) => {
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Hollowing failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
            }
        }
    }

    fn start_poll(self: &Rc<Self>) {
        let run = self.clone();
        let timer_weak = Rc::downgrade(&self.poll_timer);
        self.poll_timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                let stop = || {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                };
                let Some(ui) = run.ui.upgrade() else {
                    stop();
                    return;
                };
                let finished = {
                    let slot = run.job.borrow();
                    let Some(job) = slot.as_ref() else {
                        stop();
                        return;
                    };
                    let progress = job.work.progress();
                    ui.set_hollow_progress(progress.fraction());
                    ui.set_hollow_phase(phase_text(&progress.phase));
                    job.work.try_poll()
                };
                if let Some(landed) = finished {
                    stop();
                    let job = run.job.borrow_mut().take();
                    ui.set_hollow_running(false);
                    if let Some(job) = job {
                        run.land(job, landed);
                    }
                    run.next_queued();
                }
            },
        );
    }

    /// Lands a finished job.
    fn land(self: &Rc<Self>, job: Job, landed: Landed) {
        match landed {
            Landed::Hollow(outcome) => self.finish(job, *outcome),
            Landed::Drill(outcome) => self.finish_drill(job, *outcome),
        }
    }

    /// Whether the leaf still shows the mesh the job started from, on a
    /// scene not changed since; says it was discarded when not.
    fn still_on(&self, job: &Job) -> bool {
        let still_there = self
            .document
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .leaf_mesh_arc(job.leaf)
                    .is_some_and(|m| Arc::ptr_eq(m, &job.before))
            })
            .unwrap_or(false);
        if !still_there || self.document.slice_geometry_changed(job.started_on) {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("HollowRun", "Hollow discarded"),
                message: ctr!(
                    "HollowRun",
                    "The scene changed while the model was being hollowed; nothing was changed."
                ),
                details: Default::default(),
            });
            return false;
        }
        true
    }

    /// Says `missed` drains were left out: they do not reach the cavity.
    fn tell_missed(&self, missed: usize) {
        if missed == 0 {
            return;
        }
        self.dialogs.show_info(move || InfoTexts {
            heading: ctr!(
                "HollowRun",
                "Drain not drilled",
                "{n} drains not drilled",
                missed
            ),
            message: ctr!(
                "HollowRun",
                "This drain does not reach the cavity, so it was left out: drilled, it would only bore into the model, or through it. Place it where the cavity preview shows behind the wall.",
                "These drains do not reach the cavity, so they were left out: drilled, they would only bore into the model, or through it. Place them where the cavity preview shows behind the wall.",
                missed
            ),
            details: Default::default(),
        });
    }

    /// Says `failed` plugs were not made.
    fn tell_plug_failures(&self, failed: usize) {
        if failed == 0 {
            return;
        }
        self.dialogs.show_info(move || InfoTexts {
            heading: ctr!(
                "HollowRun",
                "Plug not made",
                "{n} plugs not made",
                failed
            ),
            message: ctr!(
                "HollowRun",
                "The drain was drilled, but its plug did not come out as a closed solid, so it was left out.",
                "The drains were drilled, but their plugs did not come out as closed solids, so they were left out.",
                failed
            ),
            details: Default::default(),
        });
    }

    /// What the drains drilled did, for the tab: `kept` drilled, `missed`
    /// left out, `capped` drilled shorter than set, `plugs` made.
    fn drains_summary(kept: usize, missed: usize, capped: usize, plugs: usize) -> String {
        let mut summary = ctr!(
            "HollowRun",
            "One drain drilled.",
            "{n} drains drilled.",
            kept
        )
        .to_string();
        if missed > 0 {
            summary.push(' ');
            summary.push_str(&ctr!(
                "HollowRun",
                "One left out: it does not reach the cavity.",
                "{n} left out: they do not reach the cavity.",
                missed
            ));
        }
        if capped > 0 {
            summary.push(' ');
            summary.push_str(&ctr!(
                "HollowRun",
                "One drilled shorter than set: deeper, it would reach the far side of the cavity.",
                "{n} drilled shorter than set: deeper, they would reach the far side of the cavity.",
                capped
            ));
        }
        if plugs > 0 {
            summary.push(' ');
            summary.push_str(&ctr!(
                "HollowRun",
                "One plug placed beside the model.",
                "{n} plugs placed beside the model.",
                plugs
            ));
        }
        summary
    }

    /// Lands drains drilled into a mesh as it is (#447).
    fn finish_drill(self: &Rc<Self>, job: Job, outcome: Result<Primed<DrillOutcome>, JobError>) {
        let ended = match &outcome {
            Ok(_) => "ok".to_owned(),
            Err(JobError::Cancelled) => "cancelled".to_owned(),
            Err(err) => format!("failed: {err}"),
        };
        crate::breadcrumbs::finished("drill", &ended);
        let input = if job.onto.is_some() {
            job.before.clone()
        } else {
            job.original.clone()
        };
        let landed = match outcome {
            Ok(landed) => landed,
            Err(JobError::Cancelled) => return,
            Err(err) => {
                if !input.is_sliceable() {
                    self.offer_repair(job.leaf, &input);
                    return;
                }
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Drilling failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
                return;
            }
        };
        if !self.still_on(&job) {
            return;
        }
        let Primed {
            out,
            mesh: made,
            primed,
        } = landed;
        let kept: Vec<LocalDrain> = job
            .drains
            .iter()
            .zip(&out.drains)
            .filter(|(_, report)| report.reaches_cavity)
            .map(|(drain, _)| *drain)
            .collect();
        let missed = job.drains.len() - kept.len();
        self.tell_missed(missed);
        let Some(made) = made.filter(|_| !kept.is_empty()) else {
            self.refresh_ui();
            self.tick_preview();
            return;
        };
        let (base_drains, base_links, old_plugs) = match &job.onto {
            Some(onto) => (onto.drains.clone(), onto.plugs.clone(), Vec::new()),
            None => (Vec::new(), Vec::new(), self.plugs_of(&job.before)),
        };
        let kept_index = |drain: usize| {
            out.drains[..drain]
                .iter()
                .filter(|report| report.reaches_cavity)
                .count()
        };
        let capped = out
            .drains
            .iter()
            .filter(|r| r.reaches_cavity && r.depth_capped)
            .count();
        let plug_failures = out.plugs.iter().filter(|p| p.mesh.is_none()).count();
        let new_plugs: Vec<NewPlug> = out
            .plugs
            .into_iter()
            .filter_map(|HollowPlug { drain, mesh }| {
                let index = kept_index(drain);
                Some(NewPlug {
                    drain: base_drains.len() + index,
                    shape: kept[index],
                    mesh: mesh?,
                })
            })
            .collect();
        let made_plugs = new_plugs.len();
        let Some(links) = self.swap_mesh_and_plugs(
            job.leaf,
            job.before.clone(),
            made.clone(),
            &old_plugs,
            new_plugs,
            &primed,
        ) else {
            return;
        };
        self.tell_plug_failures(plug_failures);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_last_run(
                Self::drains_summary(kept.len(), missed, capped, made_plugs).into(),
            );
        }
        let mut drains = base_drains;
        drains.extend(kept);
        let mut plugs = base_links;
        plugs.extend(links);
        let entry = match self.record_of(&job.before) {
            Some(h) => Hollowed {
                hollow: made.clone(),
                drains,
                plugs,
                ..h
            },
            None => Hollowed {
                hollow: made.clone(),
                original: job.original,
                drains,
                options: job.options,
                plugs,
                solids: Vec::new(),
                engine: None,
                stretch: Some(job.stretch),
                drilled: true,
            },
        };
        self.adopted.set(Some(Arc::as_ptr(&made) as usize));
        self.hollowed.borrow_mut().push(entry);
        self.refresh_ui();
        self.tick_preview();
        self.tell_drained(job.leaf);
    }

    fn finish(self: &Rc<Self>, job: Job, outcome: Result<Primed<HollowOutcome>, JobError>) {
        let ended = match &outcome {
            Ok(_) => "ok".to_owned(),
            Err(JobError::Cancelled) => "cancelled".to_owned(),
            Err(err) => format!("failed: {err}"),
        };
        crate::breadcrumbs::finished("hollow", &ended);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(JobError::Cancelled) => return,
            Err(err) => {
                if !job.original.is_sliceable() {
                    self.offer_repair(job.leaf, &job.original);
                    return;
                }
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Hollowing failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
                return;
            }
        };
        let Primed {
            out: outcome,
            mesh: made,
            primed,
        } = outcome;
        *self.prepared.borrow_mut() = Some(outcome.prepared.clone());
        let joined_parts = outcome.prepared.session().joined_parts();
        if !self.still_on(&job) {
            return;
        }
        // A wall that takes the whole model hollows nothing (#448).
        if outcome.stats.removed_voxels == 0 {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("HollowRun", "No room to hollow this model"),
                message: ctr!(
                    "HollowRun",
                    "The wall takes up the whole model: there is no room left for a cavity, so nothing was changed. Try a thinner wall, or leave this model solid."
                ),
                details: Default::default(),
            });
            self.refresh_ui();
            return;
        }
        let (HollowResult::Apply { drains, plugs, .. }, Some(made)) = (outcome.result, made) else {
            return;
        };
        // The engine leaves out a drain that does not reach the cavity
        // (#264): the hollow keeps the drains it has, and the person hears
        // which were left out.
        let kept: Vec<LocalDrain> = job
            .drains
            .iter()
            .zip(&drains)
            .filter(|(_, report)| report.reaches_cavity)
            .map(|(drain, _)| *drain)
            .collect();
        let missed = job.drains.len() - kept.len();
        self.tell_missed(missed);
        // A new drain that was left out changes nothing: no undo step.
        let unchanged = self.hollowed.borrow().iter().any(|h| {
            Arc::ptr_eq(&h.hollow, &job.before)
                && Arc::ptr_eq(&h.original, &job.original)
                && h.options == job.options
                && h.drains == kept
                && h.solids == job.solids
        });
        if unchanged {
            self.refresh_ui();
            self.tick_preview();
            return;
        }
        // The plugs, by their drain's index among the drains kept.
        let kept_index = |drain: usize| {
            drains[..drain]
                .iter()
                .filter(|report| report.reaches_cavity)
                .count()
        };
        let plug_failures = plugs.iter().filter(|p| p.mesh.is_none()).count();
        let new_plugs: Vec<NewPlug> = plugs
            .into_iter()
            .filter_map(|HollowPlug { drain, mesh }| {
                let index = kept_index(drain);
                Some(NewPlug {
                    drain: index,
                    shape: kept[index],
                    mesh: mesh?,
                })
            })
            .collect();
        let made_plugs = new_plugs.len();
        let capped = drains
            .iter()
            .filter(|r| r.reaches_cavity && r.depth_capped)
            .count();
        let old_plugs = self.plugs_of(&job.before);
        let hollow = made;
        let Some(plug_links) = self.swap_mesh_and_plugs(
            job.leaf,
            job.before.clone(),
            hollow.clone(),
            &old_plugs,
            new_plugs,
            &primed,
        ) else {
            return;
        };
        let stats = &outcome.stats;
        let ml = |mm3: f64| mm3 / 1000.0;
        let source = ml(stats.source_volume_mm3);
        let saved = ml(stats.removed_volume_mm3.unwrap_or(0.0));
        let percent = if source > 0.0 {
            saved / source * 100.0
        } else {
            0.0
        };
        let mut summary = ctr!(
            "HollowRun",
            "Saves {} of {} ml ({} %). Wall {} mm, voxel {} mm.",
            format!("{saved:.1}"),
            format!("{source:.1}"),
            format!("{percent:.0}"),
            format!("{:.1}", stats.wall_thickness_mm),
            format!("{:.2}", stats.grid.voxel_mm),
        )
        .to_string();
        // Say why the voxel came out larger than asked (#263).
        let coarsened = match stats.grid.bound {
            Some(ResolutionBound::Coarsened { max, .. }) => Some(
                ctr!(
                    "HollowRun",
                    "The voxel was enlarged from {} mm: at most {} voxels per side.",
                    format!("{:.2}", stats.grid.requested_voxel_mm),
                    max,
                )
                .to_string(),
            ),
            Some(ResolutionBound::MemoryLimited { limit_bytes, .. }) => Some(
                ctr!(
                    "HollowRun",
                    "The voxel was enlarged from {} mm so the hollow fits in {} GB of memory.",
                    format!("{:.2}", stats.grid.requested_voxel_mm),
                    format!("{:.0}", limit_bytes as f64 / (1u64 << 30) as f64),
                )
                .to_string(),
            ),
            Some(ResolutionBound::Refined { .. }) | None => None,
        };
        if let Some(why) = coarsened {
            summary.push(' ');
            summary.push_str(&why);
        }
        // Parts that overlapped are hollowed as one solid (#453): the model
        // keeps their union, with no faces buried inside it.
        if joined_parts {
            summary.push(' ');
            summary.push_str(&ctr!(
                "HollowRun",
                "Its parts overlapped: they were joined into one solid and hollowed together."
            ));
        }
        if !drains.is_empty() {
            summary.push(' ');
            summary.push_str(&Self::drains_summary(
                kept.len(),
                missed,
                capped,
                made_plugs,
            ));
        }
        self.tell_plug_failures(plug_failures);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_last_run(summary.into());
        }
        // The marks placed before are drilled now and live on the hollow;
        // the solid model keeps them, for when Undo brings it back (#512).
        // The tab already shows the settings this hollow was made with.
        let drilled = !kept.is_empty();
        self.adopted.set(Some(Arc::as_ptr(&hollow) as usize));
        self.hollowed.borrow_mut().push(Hollowed {
            hollow,
            original: job.original,
            drains: kept,
            options: job.options,
            plugs: plug_links,
            solids: job.solids,
            engine: Some(HOLLOW_ENGINE_VERSION),
            stretch: Some(job.stretch),
            drilled: false,
        });
        self.refresh_ui();
        if drilled {
            self.tell_drained(job.leaf);
        }
    }

    /// The plugs the hollow `mesh` (by identity) put in the scene; none for
    /// a mesh that is not a hollow made here.
    fn plugs_of(&self, mesh: &Arc<IndexedMesh>) -> Vec<(PlugLink, LocalDrain)> {
        self.hollowed
            .borrow()
            .iter()
            .find(|h| Arc::ptr_eq(&h.hollow, mesh))
            .map(|h| {
                h.plugs
                    .iter()
                    .filter_map(|p| Some((p.clone(), *h.drains.get(p.drain)?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Replaces the leaf's mesh, takes the plugs `old` (with the drains they
    /// were made for) out of the scene and stands the `new` ones on the
    /// plate, as one undoable edit. A new plug for the same drain as an old
    /// one takes its place (the person may have moved it); any other stands
    /// beside the model ([`plug_spots`]). Returns the plugs put in, or
    /// `None` when nothing changed.
    fn swap_mesh_and_plugs(
        &self,
        leaf: NodeId,
        before: Arc<IndexedMesh>,
        after: Arc<IndexedMesh>,
        old: &[(PlugLink, LocalDrain)],
        new: Vec<NewPlug>,
        primed: &[PrimedMesh],
    ) -> Option<Vec<PlugLink>> {
        let bed = {
            let volume = self.ctrl.build_volume();
            let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
            ([lo.x, lo.y], [hi.x, hi.y])
        };
        let planned = self
            .document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                // The leaf must still show `before`: never swap out a mesh
                // that changed under the job.
                if !scene
                    .leaf_mesh_arc(leaf)
                    .is_some_and(|m| Arc::ptr_eq(m, &before))
                {
                    return None;
                }
                let swap = leaf_mesh_swap(scene, leaf, after)?;
                let leaves: Vec<NodeId> =
                    (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect();
                // Copies of the model (pasted, Fill bed's) that still show
                // `before` keep its drains, so they keep its plugs (#113).
                let shared = leaves.iter().any(|&id| {
                    id != leaf
                        && scene
                            .leaf_mesh_arc(id)
                            .is_some_and(|m| Arc::ptr_eq(m, &before))
                });
                // The old plugs still in the scene, and the drain each was for.
                let gone: Vec<(NodeId, LocalDrain)> = leaves
                    .iter()
                    .filter(|_| !shared)
                    .filter_map(|&id| {
                        let mesh = scene.leaf_mesh_arc(id)?;
                        old.iter()
                            .find(|(p, _)| Arc::ptr_eq(&p.mesh, mesh))
                            .map(|(_, drain)| (id, *drain))
                    })
                    .collect();
                let rect_of = |id: NodeId| -> Option<PlateRect> {
                    let query = spatial.leaf(id)?;
                    let b = query.transformed_aabb(query.world_transform())?;
                    let (lo, hi) = (b.min().coords(), b.max().coords());
                    Some(([lo[0], lo[1]], [hi[0], hi[1]]))
                };
                let model = rect_of(leaf)?;
                let taken: Vec<PlateRect> = leaves
                    .iter()
                    .filter(|&&id| id != leaf && !gone.iter().any(|(g, _)| *g == id))
                    .filter_map(|&id| rect_of(id))
                    .collect();
                // Where each new plug goes: an old one's place, or a spot.
                let mut reused: Vec<Option<Transform3D>> = Vec::with_capacity(new.len());
                let mut claimed: Vec<NodeId> = Vec::new();
                for plug in &new {
                    let place = gone
                        .iter()
                        .find(|(id, drain)| *drain == plug.shape && !claimed.contains(id))
                        .and_then(|&(id, _)| {
                            let node = scene.node(id)?;
                            matches!(scene.attachment_of(id)?, LeafAttachment::Root { .. })
                                .then_some((id, node.local_transform))
                        });
                    if let Some((id, _)) = place {
                        claimed.push(id);
                    }
                    reused.push(place.map(|(_, t)| t));
                }
                let sizes: Vec<[f32; 2]> = new
                    .iter()
                    .zip(&reused)
                    .filter(|(_, r)| r.is_none())
                    .map(|(plug, _)| plug_half_width(&plug.mesh))
                    .collect();
                let mut spots = plug_spots(model, &taken, bed, &sizes).into_iter();
                let placed: Vec<Transform3D> = reused
                    .into_iter()
                    .map(|r| {
                        r.unwrap_or_else(|| {
                            let [x, y] = spots.next().unwrap_or_default();
                            Transform3D::Affine(AffineTransform3D::from_translation([
                                f64::from(x),
                                f64::from(y),
                                0.0,
                            ]))
                        })
                    })
                    .collect();
                // The removals, worked out on the scene the ones before leave.
                let mut graph = scene.clone();
                let mut edits = vec![swap];
                for &(id, _) in &gone {
                    if let Some(remove) = SceneEdit::remove_leaf_with_gc(&graph, id)
                        && remove.apply_to_scene(&mut graph).is_ok()
                    {
                        edits.push(remove);
                    }
                }
                Some((edits, graph.roots().len(), placed))
            })
            .flatten();
        let (mut edits, roots, placed) = planned?;
        let slots = self.document.allocate_leaf_slots(new.len());
        if slots.len() != new.len() {
            return None;
        }
        let mut links = Vec::with_capacity(new.len());
        for (k, ((plug, transform), (node_id, rid))) in
            new.into_iter().zip(placed).zip(slots).enumerate()
        {
            let mesh = Arc::new(plug.mesh);
            links.push(PlugLink {
                drain: plug.drain,
                mesh: mesh.clone(),
            });
            edits.push(SceneEdit::AddMeshResource { rid, mesh });
            edits.push(SceneEdit::AddLeaf {
                node_id,
                attachment: LeafAttachment::Root {
                    position: roots + k,
                },
                payload: LeafPayload::MeshResource(rid),
                local_transform: transform,
                meta: Some(SceneMetadata {
                    name: Some(ctr!("HollowRun", "Drain plug {}", plug.drain + 1).to_string()),
                    comment: None,
                }),
            });
        }
        let edit = if edits.len() == 1 {
            edits.pop()?
        } else {
            SceneEdit::Compound(edits)
        };
        match self.document.apply_edit_primed(edit.clone(), primed) {
            Ok(_) => {
                self.document.push_edit(edit);
                if let Some(note) = lost_paint_note(&self.document.take_paint_report()) {
                    self.dialogs.show_info(move || InfoTexts {
                        heading: ctr!("HollowRun", "Painted detail"),
                        message: note.clone(),
                        details: Default::default(),
                    });
                }
                Some(links)
            }
            Err(err) => {
                let details = format!("{err:?}");
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Hollowing failed"),
                    context: None,
                    message: ctr!(
                        "HollowRun",
                        "The scene rejected the new mesh; nothing was changed."
                    ),
                    details: details.as_str().into(),
                });
                None
            }
        }
    }
}

/// A plug to stand on the plate (#318): for which drain (its index among
/// the hollow's drains, and its shape), standing as [`HollowPlug`] has it.
struct NewPlug {
    drain: usize,
    shape: LocalDrain,
    mesh: IndexedMesh,
}

/// How far a plug standing on its axis reaches across it, along `x` and `y`.
fn plug_half_width(mesh: &IndexedMesh) -> [f32; 2] {
    mesh.vertices.iter().fold([0.0f32; 2], |half, v| {
        let [x, y, _] = *v.as_array();
        [half[0].max(x.abs()), half[1].max(y.abs())]
    })
}

impl HollowRun {
    /// Takes the previews down, if they are what the viewport shows.
    fn hide_preview(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_drain_reach(0);
        }
        // Out of the tab, nothing needs the mesh read: let it go.
        if self.probe.borrow().key.is_some() {
            *self.probe.borrow_mut() = ProbeSlot::default();
        }
        let mut preview = self.preview.borrow_mut();
        if let Some((_, shown)) = preview.shown.take()
            && self.ctrl.preview_overlay_is(&shown)
        {
            self.ctrl.set_preview_overlay(None);
        }
    }

    /// Brings the previews in line with the tab: the selected model's cavity
    /// for the current settings (computing it when they have rested; the last
    /// one stays up meanwhile), its drains, and the drain under the cursor.
    fn tick_preview(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_hollow_busy(
            self.job.borrow().is_some()
                || self
                    .preview
                    .try_borrow()
                    .is_ok_and(|preview| preview.job.is_some()),
        );
        if ui.get_workspace_section() != WorkspaceSection::Hollow || ui.get_loading() {
            self.hide_preview();
            return;
        }
        let Some((leaf, mesh, world)) = self.selected() else {
            self.hide_preview();
            return;
        };
        if self.model_state(&mesh) == 2 {
            self.hide_preview();
            return;
        }
        // The mesh as it is, read off this thread: its sealed cavities
        // (#130) and, for a model that came hollow, the drains it takes
        // (#447).
        let probe = self.probe_for(&mesh, &world);
        if std::mem::take(&mut self.probe.borrow_mut().fresh) {
            self.refresh_ui();
        }
        let state = self.model_state(&mesh);
        let came_hollow = state == 3;
        let (original, drains) = self.source_for(&mesh);
        let solids = self.solids_for(&mesh);
        let options = *self.options.borrow();
        let Ok(frame) = LeafFrame::of(&world) else {
            self.hide_preview();
            return;
        };
        let key = PreviewKey {
            leaf,
            mesh: Arc::as_ptr(&original) as usize,
            stretch: frame.stretch_key(),
            // The drain tool does not change the cavity.
            options: options.cavity_only(),
            drains: if options.fill == 0 {
                0
            } else {
                drains_key(&drains)
            },
            solids: solids_key(&solids),
        };
        // A model that came hollow has its cavity: none is computed.
        let show_cavity = ui.get_hollow_show_cavity() && !came_hollow;
        if show_cavity {
            self.advance_cavity_job(
                key,
                &original,
                &world,
                ui.get_hollow_drain_tool(),
                &drains,
                &solids,
            );
        }
        let matrix = matrix_of(&world);
        // Until the engine has measured a drain, the depth it gets on a
        // flat wall (#315).
        let fallback_depth = options.wall_mm + DRAIN_PAST_WALL_MM;
        let mut preview = self.preview.borrow_mut();
        let mut hover_depth = if options.drain_depth_set {
            options.drain_depth_mm
        } else {
            fallback_depth
        };
        let hover = self
            .hover
            .get()
            .filter(|_| ui.get_hollow_drain_tool())
            .map(|hover| {
                let (reach, depth) = if came_hollow {
                    probe_reach(probe.as_deref(), hover, &world, &options)
                } else {
                    drain_reach(preview.ready.as_ref(), key, hover, &world, &options)
                };
                hover_depth = depth.unwrap_or(hover_depth);
                (hover, reach)
            });
        ui.set_hollow_drain_reach(hover.map_or(0, |(_, reach)| reach.code()));
        let probed = came_hollow
            .then_some(probe.as_ref())
            .flatten()
            .map(|probe| {
                drains
                    .iter()
                    .map(|d| {
                        probe
                            .report(&world, &d.to_world(&world))
                            .map_or((fallback_depth, true), |r| (r.depth_mm, r.reaches_cavity))
                    })
                    .collect::<Vec<(f32, bool)>>()
            });
        // Each drain's depth, and whether it opens into the cavity: one
        // that does not is drawn red, as the drain under the cursor is, so
        // a drain Hollow leaves out never shows as if it were there (#753).
        // Not measured yet: as placed, drawn as opening.
        let depths = probed
            .or_else(|| drain_depths(&mut preview, &drains, key, &world, matrix, &options))
            .unwrap_or_else(|| {
                drains
                    .iter()
                    .map(|d| (d.depth_mm.unwrap_or(fallback_depth), true))
                    .collect()
            });

        // The cavity of this model, current or the last one while the next
        // is computed.
        let cavity = preview
            .ready
            .as_ref()
            .filter(|(k, _, _)| show_cavity && k.leaf == key.leaf && k.mesh == key.mesh)
            .map(|(_, cavity, _)| cavity.clone());
        let cavity_piece = cavity.map(|cavity| {
            let id = (Arc::as_ptr(&cavity) as usize, matrix);
            match &preview.cavity_piece {
                Some((k, piece)) if *k == id => piece.clone(),
                _ => {
                    let mut piece = crate::support_run::overlay_of(&cavity, OverlayTint::Cavity);
                    for p in &mut piece.positions {
                        *p = world.apply_to_vertex(Vertex::new(*p)).coords();
                    }
                    let piece = Arc::new(piece);
                    preview.cavity_piece = Some((id, piece.clone()));
                    piece
                }
            }
        });
        let drains_piece = (!drains.is_empty()).then(|| {
            let id = (
                drains.clone(),
                matrix,
                depths
                    .iter()
                    .map(|(d, opens)| (d.to_bits(), *opens))
                    .collect::<Vec<_>>(),
            );
            match &preview.drains_piece {
                Some((k, pieces)) if *k == id => pieces.clone(),
                _ => {
                    let mut pieces = Vec::new();
                    for (want, tint) in [(true, OverlayTint::Drain), (false, OverlayTint::Invalid)]
                    {
                        let mut mesh = (Vec::new(), Vec::new());
                        for (drain, &(depth, opens)) in drains.iter().zip(&depths) {
                            if opens != want {
                                continue;
                            }
                            let w = drain.to_world(&world);
                            let radii = [w.radius_mm, drain.end_radius_mm()];
                            drain_solid(w.center, w.direction, radii, depth, &mut mesh);
                        }
                        if !mesh.1.is_empty() {
                            pieces.push(Arc::new(OverlayMesh {
                                positions: mesh.0,
                                indices: mesh.1,
                                tint,
                            }));
                        }
                    }
                    preview.drains_piece = Some((id, pieces.clone()));
                    pieces
                }
            }
        });
        // Where a drain works best (#455), with the drain tool on a hollow.
        let map = (ui.get_hollow_drain_tool() && self.drain_map_on.get() && matches!(state, 1 | 3))
            .then_some(probe)
            .flatten()
            .map(|probe| self.drain_map(leaf, &mesh, &world, matrix, &drains, probe))
            .unwrap_or_default();
        let shown_key = ShownKey {
            map: map.first().map(|p| Arc::as_ptr(p) as usize),
            cavity: cavity_piece.as_ref().map(|p| Arc::as_ptr(p) as usize),
            drains: drains_piece
                .as_ref()
                .and_then(|p| p.first())
                .map(|p| Arc::as_ptr(p) as usize),
            hover,
            world: matrix,
            tool: options,
            hover_depth: hover_depth.to_bits(),
            picked: self.picked_drain.get(),
        };
        if let Some((k, shown)) = &preview.shown
            && *k == shown_key
            && self.ctrl.preview_overlay_is(shown)
        {
            return;
        }
        let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
        pieces.extend(map);
        pieces.extend(cavity_piece);
        pieces.extend(drains_piece.into_iter().flatten());
        // The drain picked to edit shows green over the others (#450).
        if let Some((drain, &(depth, _))) = self
            .picked_drain
            .get()
            .and_then(|i| drains.get(i).zip(depths.get(i)))
        {
            let w = drain.to_world(&world);
            let mut mesh = (Vec::new(), Vec::new());
            drain_solid(
                w.center,
                w.direction,
                [w.radius_mm * 1.05, drain.end_radius_mm() * 1.05],
                depth,
                &mut mesh,
            );
            pieces.push(Arc::new(OverlayMesh {
                positions: mesh.0,
                indices: mesh.1,
                tint: OverlayTint::Valid,
            }));
        }
        if let Some((hover, reach)) = hover {
            // Straight from the surface under the cursor, where the click
            // would drill: no search, the viewport's hit says it.
            let direction = hover.normal.map(|c| -c);
            let end = if options.drain_cone {
                options.drain_inner_radius_mm
            } else {
                options.drain_radius_mm
            };
            let mut next = (Vec::new(), Vec::new());
            drain_solid(
                hover.point,
                direction,
                [options.drain_radius_mm, end],
                hover_depth,
                &mut next,
            );
            pieces.push(Arc::new(OverlayMesh {
                positions: next.0,
                indices: next.1,
                tint: reach.tint(),
            }));
        }
        crate::breadcrumbs::note(&format!(
            "hollow: preview cavity={} drains={} hover={:?}",
            shown_key.cavity.is_some(),
            shown_key.drains.is_some(),
            shown_key.hover.map(|(_, reach)| reach)
        ));
        let overlay: Overlay = pieces.into();
        self.ctrl.set_preview_overlay(Some(overlay.clone()));
        preview.shown = Some((shown_key, overlay));
    }

    /// The drain map of the hollow `mesh` on `leaf` (#455): the faces where
    /// a drain works best, green, and those painted as detail among them,
    /// red: a place to avoid. Made off this thread; the last one stays up
    /// while the next is made.
    fn drain_map(
        &self,
        leaf: NodeId,
        mesh: &Arc<IndexedMesh>,
        world: &Transform3D,
        matrix: [[f64; 4]; 4],
        drains: &[LocalDrain],
        probe: Arc<DrillProbe>,
    ) -> Vec<Arc<OverlayMesh>> {
        let detail = self
            .document
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .surface_tags(leaf, PaintKind::Detail)
                    .cloned()
            })
            .flatten();
        let key: MapKey = (
            Arc::as_ptr(mesh) as usize,
            matrix,
            drains_key(drains),
            detail.as_ref().map_or(0, |t| t.revision()),
        );
        let mut slot = self.probe.borrow_mut();
        let got = slot
            .map_job
            .as_ref()
            .and_then(|(k, rx)| rx.try_recv().ok().map(|pieces| (*k, pieces)));
        if let Some(map) = got {
            slot.map = Some(map);
            slot.map_job = None;
        }
        let current = slot
            .map
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, m)| m.clone());
        if current.is_none() && slot.map_job.as_ref().is_none_or(|(k, _)| *k != key) {
            let (tx, rx) = std::sync::mpsc::channel();
            let (mesh, world) = (mesh.clone(), *world);
            let centres: Vec<[f32; 3]> = drains.iter().map(|d| d.to_world(&world).center).collect();
            let _ = std::thread::Builder::new()
                .name("hollow-drain-map".to_owned())
                .spawn(move || {
                    let faces = probe
                        .drain_map(
                            &world,
                            DRAIN_MAP_THIN,
                            &centres,
                            DRAIN_MAP_CLEAR_MM,
                            &|_| false,
                        )
                        .unwrap_or_default();
                    let painted = |f: u32| {
                        detail
                            .as_ref()
                            .is_some_and(|t| t.values().get(f as usize).is_some_and(|&v| v != 0))
                    };
                    let (avoid, good): (Vec<u32>, Vec<u32>) =
                        faces.into_iter().partition(|&f| painted(f));
                    let mut pieces = vec![Arc::new(map_overlay(
                        &mesh,
                        &world,
                        &good,
                        OverlayTint::Hint,
                    ))];
                    if !avoid.is_empty() {
                        pieces.push(Arc::new(map_overlay(
                            &mesh,
                            &world,
                            &avoid,
                            OverlayTint::Invalid,
                        )));
                    }
                    let _ = tx.send(pieces);
                });
            slot.map_job = Some((key, rx));
        }
        current
            .or_else(|| {
                slot.map
                    .as_ref()
                    .filter(|(k, _)| k.0 == key.0)
                    .map(|(_, m)| m.clone())
            })
            .unwrap_or_default()
    }

    /// Polls the cavity job, and starts one for `key` when none runs. A job
    /// for older settings of the same model is let finish (its cavity shows
    /// meanwhile), so dragging the wall updates the preview continuously at
    /// the engine's pace; one for another model is cancelled. With the
    /// drain tool armed, a cavity that lands gets what drains are measured
    /// against built off this thread (23 ms on a 91 mm figure), so the first
    /// hover does not wait for it; else the first hover builds it.
    fn advance_cavity_job(
        &self,
        key: PreviewKey,
        original: &Arc<IndexedMesh>,
        world: &Transform3D,
        drain_tool: bool,
        drains: &[LocalDrain],
        solids: &[LassoRegion],
    ) {
        let mut preview = self.preview.borrow_mut();
        if let Some((handle, job_key)) = &preview.job {
            let same_model = job_key.leaf == key.leaf && job_key.mesh == key.mesh;
            if !same_model {
                handle.cancel();
            }
            match handle.try_poll() {
                None if same_model => return,
                None => {}
                Some(Ok(outcome)) => {
                    crate::breadcrumbs::note("hollow: cavity preview ready");
                    let job_key = *job_key;
                    *self.prepared.borrow_mut() = Some(outcome.prepared.clone());
                    if let HollowResult::Preview { cavity, infill, .. } = outcome.result {
                        let mut cavity = cavity;
                        if let Some(infill) = infill {
                            append_mesh(&mut cavity, &infill);
                        }
                        if drain_tool {
                            prepare_drain_reports(&outcome.prepared, &job_key);
                        }
                        preview.ready = Some((job_key, Arc::new(cavity), outcome.prepared));
                    }
                }
                Some(Err(_)) => {}
            }
            preview.job = None;
        }
        let ready = preview.ready.as_ref().is_some_and(|(k, _, _)| *k == key);
        if ready || self.job.borrow().is_some() {
            return;
        }
        let request = HollowRequest {
            kind: HollowKind::Preview,
            grid: GridRequest {
                voxel_size_mm: key.options.voxel_mm,
                ..GridRequest::default()
            },
            settings: key.options.settings(),
            // With infill, the drains clear their channels through it.
            drains: if key.drains == 0 {
                Vec::new()
            } else {
                drains.iter().map(|d| d.to_world(world)).collect()
            },
            solid_areas: solid_areas(solids, world),
            prepared: self.prepared.borrow().clone(),
            ..HollowRequest::default()
        };
        if let Ok(handle) = start_hollow_mesh(original.clone(), world, request) {
            crate::breadcrumbs::note("hollow: cavity preview started");
            preview.job = Some((handle, key));
        }
    }
}

/// What a drain at `hover` would do on the selected model, posed by
/// `world`, with the tab's `options`: the engine's own check (the one
/// Hollow makes before it drills) on the cavity of `ready`, when that is the
/// cavity of `key`, and how deep it goes (#315). The drain is built as a
/// click there builds it.
fn drain_reach(
    ready: Option<&(PreviewKey, Arc<IndexedMesh>, Arc<PreparedHollow>)>,
    key: PreviewKey,
    hover: DrainHover,
    world: &Transform3D,
    options: &HollowOptions,
) -> (DrainReach, Option<f32>) {
    let Some((_, _, prepared)) = ready.filter(|(k, _, _)| k.same_cavity(&key) && hover.on_selected)
    else {
        return (DrainReach::Unknown, None);
    };
    let Some(drain) = LocalDrain::at(hover.point, hover.normal, options.drain_radius_mm, world)
        .map(|d| d.shaped(options))
    else {
        return (DrainReach::Unknown, None);
    };
    match prepared.drain_report(world, &options.settings(), &drain.to_world(world)) {
        Some(report) if report.reaches_cavity && report.depth_capped => {
            (DrainReach::Capped, Some(report.depth_mm))
        }
        Some(report) if report.reaches_cavity => (DrainReach::Opens, Some(report.depth_mm)),
        Some(report) => (DrainReach::Misses, Some(report.depth_mm)),
        None => (DrainReach::Unknown, None),
    }
}

/// What a drain at `hover` would do drilled into the model as it is
/// (#447), on its `probe`, and how deep it goes.
fn probe_reach(
    probe: Option<&DrillProbe>,
    hover: DrainHover,
    world: &Transform3D,
    options: &HollowOptions,
) -> (DrainReach, Option<f32>) {
    let Some(probe) = probe.filter(|_| hover.on_selected) else {
        return (DrainReach::Unknown, None);
    };
    let Some(drain) = LocalDrain::at(hover.point, hover.normal, options.drain_radius_mm, world)
        .map(|d| d.shaped(options))
    else {
        return (DrainReach::Unknown, None);
    };
    match probe.report(world, &drain.to_world(world)) {
        Some(report) if report.reaches_cavity && report.depth_capped => {
            (DrainReach::Capped, Some(report.depth_mm))
        }
        Some(report) if report.reaches_cavity => (DrainReach::Opens, Some(report.depth_mm)),
        Some(report) => (DrainReach::Misses, Some(report.depth_mm)),
        None => (DrainReach::Unknown, None),
    }
}

/// How thick the wall behind a face of the drain map may be (#455), as a
/// multiple of the thinnest: a drain there is about as short as one goes.
const DRAIN_MAP_THIN: f32 = 1.5;
/// How far around a drain the map fades (#455): that part already drains.
const DRAIN_MAP_CLEAR_MM: f32 = 10.0;

/// The faces `faces` of `mesh` posed by `world`, tinted `tint`, lifted a
/// hair off the surface so they show over it.
fn map_overlay(
    mesh: &IndexedMesh,
    world: &Transform3D,
    faces: &[u32],
    tint: OverlayTint,
) -> OverlayMesh {
    let mut positions = Vec::with_capacity(faces.len() * 3);
    let mut indices = Vec::with_capacity(faces.len() * 3);
    for &f in faces {
        let Some(face) = mesh.faces().get(f as usize) else {
            continue;
        };
        let p = face.vertices.map(|v| {
            world
                .apply_to_vertex(mesh.vertices()[u32::from(v) as usize])
                .coords()
        });
        let n = normalized(cross(
            [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]],
            [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]],
        ));
        for q in p {
            indices.push(positions.len() as u32);
            positions.push([0, 1, 2].map(|k| q[k] + 0.03 * n[k]));
        }
    }
    OverlayMesh {
        positions,
        indices,
        tint,
    }
}

/// How deep each of `drains` goes, as the engine measures it on the ready
/// cavity when that is the cavity of `key` (#315): the drawn drain is the
/// one Hollow drills. Kept while the drains, the pose and the cavity hold.
fn drain_depths(
    preview: &mut CavityPreview,
    drains: &[LocalDrain],
    key: PreviewKey,
    world: &Transform3D,
    matrix: [[f64; 4]; 4],
    options: &HollowOptions,
) -> Option<Vec<(f32, bool)>> {
    let (_, _, prepared) = preview
        .ready
        .as_ref()
        .filter(|(k, _, _)| k.same_cavity(&key))?;
    let id = (
        drains.to_vec(),
        matrix,
        Arc::as_ptr(prepared) as usize,
        PreviewKey { drains: 0, ..key },
    );
    if let Some((k, depths)) = &preview.depths
        && *k == id
    {
        return Some(depths.clone());
    }
    let settings = options.settings();
    let depths: Option<Vec<(f32, bool)>> = drains
        .iter()
        .map(|d| {
            prepared
                .drain_report(world, &settings, &d.to_world(world))
                .map(|r| (r.depth_mm, r.reaches_cavity))
        })
        .collect();
    let depths = depths?;
    preview.depths = Some((id, depths.clone()));
    Some(depths)
}

/// Builds what measuring a drain on the cavity of `key` reads, off the
/// interface thread, so the first drain hovered does not wait for it.
fn prepare_drain_reports(prepared: &Arc<PreparedHollow>, key: &PreviewKey) {
    let prepared = prepared.clone();
    let settings = key.options.settings();
    let _ = std::thread::Builder::new()
        .name("hollow-drain-bvh".to_owned())
        .spawn(move || prepared.session().prepare_drain_reports(&settings));
}

/// The 4x4 of a transform, to tell poses apart.
fn matrix_of(world: &Transform3D) -> [[f64; 4]; 4] {
    let at = |p: [f32; 3]| {
        world
            .apply_to_vertex(Vertex::new(p))
            .coords()
            .map(f64::from)
    };
    let (o, x, y, z) = (
        at([0.0; 3]),
        at([1.0, 0.0, 0.0]),
        at([0.0, 1.0, 0.0]),
        at([0.0, 0.0, 1.0]),
    );
    [
        [x[0] - o[0], x[1] - o[1], x[2] - o[2], 0.0],
        [y[0] - o[0], y[1] - o[1], y[2] - o[2], 0.0],
        [z[0] - o[0], z[1] - o[1], z[2] - o[2], 0.0],
        [o[0], o[1], o[2], 1.0],
    ]
}

/// `extra`'s triangles added to `mesh`.
fn append_mesh(mesh: &mut IndexedMesh, extra: &IndexedMesh) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.extend_from_slice(&extra.vertices);
    mesh.faces.extend(extra.faces.iter().map(|f| {
        sc_mesh_core::IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + base))
    }));
}

/// A closed cylinder or cone (a drain as drawn) from just outside `center`
/// to `depth` along `direction`, its radius `radii[0]` at `center` and
/// `radii[1]` at `depth` (#317), added to `out`.
fn drain_solid(
    center: [f32; 3],
    direction: [f32; 3],
    radii: [f32; 2],
    depth: f32,
    out: &mut (Vec<[f32; 3]>, Vec<u32>),
) {
    let d = normalized(direction);
    let helper = if d[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = normalized(cross(d, helper));
    let v = cross(d, u);
    let start: [f32; 3] = std::array::from_fn(|i| center[i] - d[i] * 0.5);
    let end: [f32; 3] = std::array::from_fn(|i| center[i] + d[i] * depth);
    // Along the cone, half a millimetre out of the surface.
    let start_radius = (radii[0] - (radii[1] - radii[0]) * 0.5 / depth.max(1e-3)).max(0.0);
    let (positions, indices) = out;
    let base = positions.len() as u32;
    for k in 0..DRAIN_SIDES {
        let a = k as f32 / DRAIN_SIDES as f32 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let off =
            |radius: f32| -> [f32; 3] { std::array::from_fn(|i| (u[i] * c + v[i] * s) * radius) };
        let (near, far) = (off(start_radius), off(radii[1]));
        positions.push(std::array::from_fn(|i| start[i] + near[i]));
        positions.push(std::array::from_fn(|i| end[i] + far[i]));
    }
    positions.push(start);
    positions.push(end);
    let (cs, ce) = (base + 2 * DRAIN_SIDES, base + 2 * DRAIN_SIDES + 1);
    for k in 0..DRAIN_SIDES {
        let (a0, a1) = (base + 2 * k, base + 2 * k + 1);
        let n = (k + 1) % DRAIN_SIDES;
        let (b0, b1) = (base + 2 * n, base + 2 * n + 1);
        indices.extend_from_slice(&[a0, b0, b1, a0, b1, a1, cs, b0, a0, ce, a1, b1]);
    }
}

/// `region` (a drawn solid area, #179) taken from the world at the pose
/// `world` into the leaf's local frame: its origin as a point, its axes as
/// directions, so it keeps the same material whatever the pose or scale.
fn region_to_local(region: &LassoRegion, world: &Transform3D) -> Option<LassoRegion> {
    let inverse = world.inverse().ok()?;
    Some(mapped_region(region, &inverse))
}

/// The world solid areas `solids` (local frame) make at the pose `world`.
pub(crate) fn solid_areas(solids: &[LassoRegion], world: &Transform3D) -> Vec<WorldSolidArea> {
    solids
        .iter()
        .map(|r| WorldSolidArea::Lasso(mapped_region(r, world)))
        .collect()
}

/// `region` under the affine map `map`.
fn mapped_region(region: &LassoRegion, map: &Transform3D) -> LassoRegion {
    let at = |p: [f32; 3]| map.apply_to_vertex(Vertex::new(p)).coords();
    let origin = at(region.origin);
    let along = |d: [f32; 3]| {
        let tip = at(std::array::from_fn(|i| region.origin[i] + d[i]));
        std::array::from_fn(|i| tip[i] - origin[i])
    };
    LassoRegion {
        origin,
        u: along(region.u),
        v: along(region.v),
        w: along(region.w),
        ..region.clone()
    }
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 {
        v.map(|c| c / len)
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    /// The phases of a run, as the engine reports them and as the tab shows
    /// them in Spanish.
    fn phases_in_spanish() -> Vec<(String, String)> {
        on_fresh_slint_thread(|| {
            let _window = MainWindow::new().expect("the headless platform creates a main window");
            slint::select_bundled_translation("es").expect("bundled language");
            STAGES
                .iter()
                .map(|stage| stage.label())
                .chain(["Done"])
                .map(|phase| (phase.to_owned(), phase_text(phase).to_string()))
                .collect()
        })
    }

    #[test]
    fn every_phase_of_a_run_shows_in_spanish() {
        let phases = phases_in_spanish();
        assert_eq!(phases.len(), STAGES.len() + 1);
        for (english, spanish) in phases {
            assert!(
                !spanish.is_empty() && spanish != english,
                "{english} has no Spanish"
            );
        }
    }

    #[test]
    fn an_unknown_phase_shows_as_it_came() {
        let shown = on_fresh_slint_thread(|| phase_text("Something new").to_string());
        assert_eq!(shown, "Something new");
    }

    #[test]
    fn the_recommended_wall_grows_with_the_model() {
        assert_eq!(recommended_wall_mm(20.0), 1.7);
        assert_eq!(recommended_wall_mm(50.0), 2.0);
        assert_eq!(recommended_wall_mm(150.0), 3.0);
        assert_eq!(recommended_wall_mm(400.0), 4.0);
        assert_eq!(recommended_wall_mm(0.0), 1.5);
    }

    #[test]
    fn a_drain_from_a_hit_points_into_the_model() {
        // Hit on the top face (normal +Z) of a model moved by (10, 0, 0).
        let world = Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        let drain = LocalDrain::at([10.0, 0.0, 5.0], [0.0, 0.0, 1.0], 1.5, &world).unwrap();
        assert_eq!(drain.center, [0.0, 0.0, 5.0]);
        assert_eq!(drain.direction, [0.0, 0.0, -1.0]);
    }

    /// A 10 mm box standing on the plate, hollowed to a 6 mm cavity: walls
    /// of 2 mm, the cavity floor at z = 2.
    fn hollow_box() -> IndexedMesh {
        let cube = sc_mesh_fixtures::unit_cube_mesh();
        let scaled = |k: f32| {
            cube.vertices
                .iter()
                .map(move |v| Vertex::new(v.coords().map(|c| c * k)))
        };
        let vertices: Vec<Vertex> = scaled(10.0).chain(scaled(6.0)).collect();
        let mut faces = cube.faces.clone();
        faces.extend(cube.faces.iter().map(|f| {
            let [a, b, c] = f.vertices.map(|v| usize::from(v) as u32 + 8);
            sc_mesh_core::IndexedTriangle::from_raw_indices([a, c, b])
        }));
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn standing(transform: AffineTransform3D) -> Transform3D {
        Transform3D::Affine(
            AffineTransform3D::from_translation([0.0, 0.0, 5.0]).compose(&transform),
        )
    }

    #[test]
    fn a_drain_under_a_cavity_floor_leaves_through_the_underside() {
        let mesh = hollow_box();
        let world = standing(AffineTransform3D::identity());
        // The review's floor point is on the cavity floor, give or take half
        // a layer: either way the line leaves the solid at the box's bottom.
        for z in [2.05, 2.0, 1.97] {
            let exit = floor_underside(&mesh, &world, [0.3, -0.2, z]).expect("under the floor");
            assert!((exit[2] - 0.0).abs() < 1e-4, "{z}: {exit:?}");
            assert_eq!([exit[0], exit[1]], [0.3, -0.2]);
        }
        // Beside the box, nothing.
        assert_eq!(floor_underside(&mesh, &world, [8.0, 0.0, 2.05]), None);
    }

    #[test]
    fn a_mirrored_or_scaled_hollow_keeps_its_underside() {
        let mesh = hollow_box();
        let mirrored = standing(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]));
        let exit = floor_underside(&mesh, &mirrored, [1.0, 1.0, 2.05]).expect("mirrored");
        assert!(exit[2].abs() < 1e-4, "{exit:?}");
        let scaled = Transform3D::Affine(
            AffineTransform3D::from_translation([0.0, 0.0, 10.0])
                .compose(&AffineTransform3D::from_uniform_scale(2.0)),
        );
        // Twice the size: the cavity floor is at z = 4 and the underside at 0.
        let exit = floor_underside(&mesh, &scaled, [1.0, 1.0, 4.05]).expect("scaled");
        assert!(exit[2].abs() < 1e-4, "{exit:?}");
    }

    #[test]
    fn a_drain_under_the_floor_points_up_into_the_cavity() {
        let world = standing(AffineTransform3D::identity());
        let drain = LocalDrain::at([0.3, -0.2, 0.0], [0.0, 0.0, -1.0], 1.5, &world).unwrap();
        assert_eq!(drain.center, [0.3, -0.2, -5.0]);
        assert_eq!(drain.direction, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_local_drain_follows_the_model_pose() {
        let drain = LocalDrain {
            center: [1.0, 0.0, 0.0],
            direction: [-1.0, 0.0, 0.0],
            radius_mm: 1.5,
            ..LocalDrain::default()
        };
        let world = Transform3D::Affine(AffineTransform3D::from_rotation_z(
            std::f64::consts::FRAC_PI_2,
        ));
        let w = drain.to_world(&world);
        assert!((w.center[1] - 1.0).abs() < 1e-5 && w.center[0].abs() < 1e-5);
        assert!((w.direction[1] + 1.0).abs() < 1e-5);
        assert_eq!(w.radius_mm, 1.5);
    }

    #[test]
    fn options_build_the_engine_settings() {
        let mut o = HollowOptions::default();
        assert_eq!(o.settings().fill, HollowFill::Cavity);
        o.fill = 2;
        assert!(matches!(
            o.settings().fill,
            HollowFill::Infill {
                pattern: InfillMode::Pillar,
                ..
            }
        ));
    }

    /// Dragging the wall slider changes the wall many times a second: the
    /// cavity shown must stay up throughout (the last one until the next is
    /// computed), never blink out.
    #[test]
    fn the_cavity_stays_shown_while_the_wall_changes() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        let shown = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0])),
                    LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document, ctrl.dialogs(), ctrl.clone());
            let cavity_shown = |run: &HollowRun| {
                run.preview
                    .borrow()
                    .shown
                    .as_ref()
                    .is_some_and(|(_, overlay)| {
                        ctrl.preview_overlay_is(overlay)
                            && overlay.iter().any(|p| p.tint == OverlayTint::Cavity)
                    })
            };
            let waited = std::time::Instant::now();
            while !cavity_shown(&run) && waited.elapsed() < Duration::from_secs(20) {
                run.tick_preview();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(cavity_shown(&run), "a first cavity");
            (0..30)
                .map(|i| {
                    run.on_option(0, 2.0 + 0.05 * i as f32);
                    std::thread::sleep(Duration::from_millis(15));
                    run.tick_preview();
                    cavity_shown(&run)
                })
                .collect::<Vec<_>>()
        });
        assert!(shown.iter().all(|&s| s), "{shown:?}");
    }

    /// Waits for the hollow job and lands it, as the poll timer would.
    fn land_job(run: &Rc<HollowRun>) {
        let waited = std::time::Instant::now();
        loop {
            let landed = run.job.borrow().as_ref().and_then(|j| j.work.try_poll());
            if let Some(landed) = landed {
                let job = run.job.borrow_mut().take().expect("the job");
                run.land(job, landed);
                run.next_queued();
                if run.job.borrow().is_none() {
                    return;
                }
                continue;
            }
            assert!(
                waited.elapsed() < Duration::from_secs(60),
                "the hollow ends"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A 30 mm cube at (50, 50, 15) on the plate, hollowed with a 2 mm wall:
    /// a drain up from the bottom 14 mm off the centre runs inside the side
    /// walls to the top, and is left out instead of going through (#264).
    #[test]
    fn a_drain_that_misses_the_cavity_is_left_out() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        let (kept, notices, undo_steps, after_click) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0])),
                    LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let dialogs = ctrl.dialogs();
            let run = HollowRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
            run.on_option(0, 2.0);
            let down = [0.0, 0.0, -1.0];
            // Placed on the solid model: a miss and one under the cavity.
            run.on_drain_click([64.0, 64.0, 0.0], down);
            run.on_drain_click([50.0, 50.0, 0.0], down);
            run.on_apply();
            land_job(&run);
            let kept = document
                .hollows()
                .borrow()
                .last()
                .map(|h| h.drains.iter().map(|d| d.center).collect::<Vec<_>>());
            let notices = dialogs.info_headings_for_test();
            let mut undo_steps = 0;
            while document.can_undo() {
                document.undo().unwrap();
                undo_steps += 1;
            }
            while document.can_redo() {
                document.redo().unwrap();
            }
            // On the hollow, another miss: nothing to drill, nothing to undo.
            run.on_drain_click([36.0, 64.0, 0.0], down);
            land_job(&run);
            let mut after_click = 0;
            while document.can_undo() {
                document.undo().unwrap();
                after_click += 1;
            }
            (kept, notices, undo_steps, after_click)
        });
        assert_eq!(
            kept,
            Some(vec![[0.0, 0.0, -15.0]]),
            "only the drain that opens"
        );
        assert_eq!(notices, vec!["Drain not drilled".to_owned()]);
        // The two drains placed (#512) and the hollow.
        assert_eq!(undo_steps, 3);
        assert_eq!(after_click, 3, "the left-out drain added no step");
    }

    /// #128: the Review hears of a drain once it lands in the model, and
    /// not of one left out.
    #[test]
    fn a_landed_drain_is_told_and_a_left_out_one_is_not() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        let (told, leaf) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0])),
                    LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let told = Rc::new(RefCell::new(Vec::new()));
            {
                let told = told.clone();
                run.on_drained(Rc::new(move |leaf| told.borrow_mut().push(leaf)));
            }
            run.on_option(0, 2.0);
            let down = [0.0, 0.0, -1.0];
            // Hollowed with no drain: nothing to tell.
            run.on_apply();
            land_job(&run);
            assert!(told.borrow().is_empty());
            // A miss on the hollow: left out, nothing told.
            run.on_drain_click([64.0, 64.0, 0.0], down);
            land_job(&run);
            assert!(told.borrow().is_empty());
            // One under the cavity: told once.
            run.on_drain_click([50.0, 50.0, 0.0], down);
            land_job(&run);
            let told = told.borrow().clone();
            (told, leaf)
        });
        assert_eq!(told, vec![leaf]);
    }

    /// The Review's drain button finds the model of any cavity (#431): one
    /// over its underside, and one closed by the plate, with nothing below.
    #[test]
    fn the_model_of_a_cavity_is_found_hollowed_here_or_not() {
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        let (over, on_plate, beside, hollow) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 5.0])),
                    LeafPayload::Mesh(Arc::new(hollow_box())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("imported.stl"));
            let run = HollowRun::install(&window, document, ctrl.dialogs(), ctrl.clone());
            (
                run.model_at([0.0, 0.0, 2.0]) == Some(leaf),
                run.model_at([0.0, 0.0, 0.0]) == Some(leaf),
                run.model_at([20.0, 0.0, 2.0]),
                run.hollow_at([0.0, 0.0, 2.0]),
            )
        });
        assert!(over, "the model above its underside");
        assert!(on_plate, "the model around a floor on the plate");
        assert_eq!(beside, None);
        assert_eq!(hollow, None, "not hollowed here");
    }

    /// #264's test piece: a 40 x 30 x 30 mm block with a 3 mm fin 20 mm
    /// tall on top, as one closed shell.
    fn finned_block() -> IndexedMesh {
        let outline: [(f32, f32); 8] = [
            (0.0, 0.0),
            (40.0, 0.0),
            (40.0, 30.0),
            (21.5, 30.0),
            (21.5, 50.0),
            (18.5, 50.0),
            (18.5, 30.0),
            (0.0, 30.0),
        ];
        let n = outline.len() as u32;
        let mut vertices = Vec::new();
        for y in [0.0, 30.0] {
            vertices.extend(outline.iter().map(|&(x, z)| Vertex::new([x, y, z])));
        }
        let cap: [[u32; 3]; 6] = [
            [0, 1, 2],
            [0, 2, 3],
            [6, 3, 4],
            [6, 4, 5],
            [0, 3, 6],
            [0, 6, 7],
        ];
        let mut faces = Vec::new();
        for [a, b, c] in cap {
            // Outward: the y = 0 cap faces -y, the y = 30 one +y.
            faces.push(sc_mesh_core::IndexedTriangle::from_raw_indices([a, b, c]));
            faces.push(sc_mesh_core::IndexedTriangle::from_raw_indices([
                a + n,
                c + n,
                b + n,
            ]));
        }
        for i in 0..n {
            let j = (i + 1) % n;
            faces.push(sc_mesh_core::IndexedTriangle::from_raw_indices([
                i,
                j + n,
                j,
            ]));
            faces.push(sc_mesh_core::IndexedTriangle::from_raw_indices([
                i,
                i + n,
                j + n,
            ]));
        }
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// The drain under the cursor (#302): neutral before the cavity is
    /// computed, then red on the fin (2 mm wall: no cavity in a 3 mm fin)
    /// and green on the body, neutral on a model not selected; and Hollow
    /// leaves out exactly the drain that showed red.
    #[test]
    fn the_drain_under_the_cursor_shows_whether_it_reaches_the_cavity() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        let (states, volume, kept, notices, placed) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let block = finned_block();
            let volume = block.faces.iter().fold(0.0f64, |sum, f| {
                let [a, b, c] = f.vertices.map(|v| block.vertices[usize::from(v)].coords());
                let a = a.map(f64::from);
                let (b, c) = (b.map(f64::from), c.map(f64::from));
                sum + (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            });
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([30.0, 35.0, 0.0])),
                    LeafPayload::Mesh(Arc::new(block)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("aleta.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            window.set_hollow_drain_tool(true);
            let dialogs = ctrl.dialogs();
            let run = HollowRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
            run.on_option(0, 2.0);
            // Into the fin's side, and into the body's side.
            let fin = ([51.5, 50.0, 41.0], [1.0, 0.0, 0.0]);
            let body = ([70.0, 50.0, 13.0], [1.0, 0.0, 0.0]);
            let hover_at = |(point, normal): ([f32; 3], [f32; 3]), on_selected: bool| {
                run.hover.set(Some(DrainHover {
                    point,
                    normal,
                    on_selected,
                }));
                run.tick_preview();
                let tint = run
                    .preview
                    .borrow()
                    .shown
                    .as_ref()
                    .and_then(|(_, overlay)| {
                        overlay.iter().map(|p| p.tint).find(|t| {
                            matches!(
                                t,
                                OverlayTint::Valid | OverlayTint::Invalid | OverlayTint::Pending
                            )
                        })
                    });
                (window.get_hollow_drain_reach(), tint)
            };
            let mut states = vec![hover_at(fin, true)];
            let waited = std::time::Instant::now();
            while run.preview.borrow().ready.is_none() && waited.elapsed() < Duration::from_secs(20)
            {
                std::thread::sleep(Duration::from_millis(10));
                run.tick_preview();
            }
            states.push(hover_at(fin, true));
            states.push(hover_at(body, true));
            states.push(hover_at(body, false));
            // Both placed on the solid model, then hollowed.
            run.on_drain_click(fin.0, fin.1);
            run.on_drain_click(body.0, body.1);
            // Placed, they show as Hollow takes them: the one that misses
            // red, the one that opens as a drain (#753).
            run.hover.set(None);
            run.tick_preview();
            let placed: Vec<OverlayTint> = run
                .preview
                .borrow()
                .shown
                .as_ref()
                .map(|(_, overlay)| {
                    overlay
                        .iter()
                        .map(|p| p.tint)
                        .filter(|t| matches!(t, OverlayTint::Drain | OverlayTint::Invalid))
                        .collect()
                })
                .unwrap_or_default();
            run.on_apply();
            land_job(&run);
            let kept = document
                .hollows()
                .borrow()
                .last()
                .map(|h| h.drains.iter().map(|d| d.center).collect::<Vec<_>>());
            (
                states,
                volume,
                kept,
                dialogs.info_headings_for_test(),
                placed,
            )
        });
        assert_eq!(placed, vec![OverlayTint::Drain, OverlayTint::Invalid]);
        assert!(volume > 0.0, "the piece faces outward");
        assert_eq!(
            states,
            vec![
                (3, Some(OverlayTint::Pending)),
                (2, Some(OverlayTint::Invalid)),
                (1, Some(OverlayTint::Valid)),
                (3, Some(OverlayTint::Pending)),
            ]
        );
        assert_eq!(kept, Some(vec![[40.0, 15.0, 13.0]]), "the green drain only");
        assert_eq!(notices, vec!["Drain not drilled".to_owned()]);
    }

    /// What is drawn is what Hollow makes: the drain under the cursor is
    /// drawn as deep as the engine drills it (#315), a placed one too, and
    /// with pillars the cavity shown is computed again with the placed
    /// drains, which clear their channels through the infill (#339).
    #[test]
    fn the_drawn_drains_go_as_deep_as_hollow_drills() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        let (hover, placed, with_drains) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([30.0, 35.0, 0.0])),
                    LeafPayload::Mesh(Arc::new(finned_block())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("aleta.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            window.set_hollow_drain_tool(true);
            let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            run.on_option(0, 2.0);
            run.on_option(3, 2.0);
            run.on_option(4, 6.0);
            run.on_option(5, 0.6);
            let wait = |done: &dyn Fn() -> bool| {
                let waited = std::time::Instant::now();
                while !done() && waited.elapsed() < Duration::from_secs(30) {
                    std::thread::sleep(Duration::from_millis(10));
                    run.tick_preview();
                }
                assert!(done(), "the preview lands");
            };
            // The cavity of the last settings, not of one on the way.
            let options = *run.options.borrow();
            wait(&|| {
                run.preview
                    .borrow()
                    .ready
                    .as_ref()
                    .is_some_and(|(k, _, _)| k.options == options.cavity_only())
            });
            // Into the body's side, 2 mm wall: the engine's depth.
            let (point, normal) = ([70.0, 50.0, 13.0], [1.0, 0.0, 0.0]);
            run.hover.set(Some(DrainHover {
                point,
                normal,
                on_selected: true,
            }));
            run.tick_preview();
            let world = Transform3D::Affine(AffineTransform3D::from_translation([30.0, 35.0, 0.0]));
            let drain = LocalDrain::at(point, normal, options.drain_radius_mm, &world).unwrap();
            let measured = {
                let preview = run.preview.borrow();
                let (_, _, prepared) = preview.ready.as_ref().unwrap();
                prepared
                    .drain_report(&world, &options.settings(), &drain.to_world(&world))
                    .unwrap()
                    .depth_mm
            };
            // How far in each piece of that tint reaches, from the surface.
            let reach = |tint: OverlayTint| {
                run.preview
                    .borrow()
                    .shown
                    .as_ref()
                    .and_then(|(_, overlay)| {
                        overlay.iter().find(|p| p.tint == tint).map(|p| {
                            p.positions
                                .iter()
                                .map(|q| point[0] - q[0])
                                .fold(f32::MIN, f32::max)
                        })
                    })
            };
            let hover = (reach(OverlayTint::Valid), measured);
            // Placed on the solid model: drawn as deep, and the cavity
            // shown is computed again with it.
            run.on_drain_click(point, normal);
            run.hover.set(None);
            run.tick_preview();
            wait(&|| {
                run.preview
                    .borrow()
                    .ready
                    .as_ref()
                    .is_some_and(|(k, _, _)| k.drains == drains_key(&[drain]))
            });
            run.tick_preview();
            let placed = (reach(OverlayTint::Drain), measured);
            let with_drains = run
                .preview
                .borrow()
                .ready
                .as_ref()
                .map(|(k, _, _)| k.drains != 0);
            (hover, placed, with_drains)
        });
        for (drawn, measured) in [hover, placed] {
            let drawn = drawn.expect("the drain is drawn");
            assert!(
                (2.1..=2.3).contains(&measured) && (drawn - measured).abs() < 1e-3,
                "drawn {drawn}, measured {measured}"
            );
        }
        assert_eq!(with_drains, Some(true));
    }

    /// The drain tool's shape options (#316, #317, #318) go into the drains
    /// it places, and on to the engine; clamped as the engine takes them.
    #[test]
    fn the_drain_tool_shapes_the_drains_it_places() {
        let shaped = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let run = HollowRun::install(&window, ctrl.document(), ctrl.dialogs(), ctrl.clone());
            for (code, value) in [
                (7, 1.0),
                (8, 4.5),
                (9, 1.0),
                (10, 0.8),
                (11, 1.0),
                (12, 0.15),
            ] {
                run.on_option(code, value);
            }
            let options = *run.options.borrow();
            let ui = (
                window.get_hollow_drain_depth_set(),
                window.get_hollow_drain_depth(),
                window.get_hollow_drain_cone(),
                window.get_hollow_drain_inner_radius(),
                window.get_hollow_drain_plug(),
                window.get_hollow_plug_clearance(),
            );
            run.on_option(12, 5.0);
            run.on_option(8, 0.0);
            let clamped = *run.options.borrow();
            (options, ui, clamped)
        });
        let (options, ui, clamped) = shaped;
        assert_eq!(ui, (true, 4.5, true, 0.8, true, 0.15));
        let drain = LocalDrain::at(
            [0.0, 0.0, 5.0],
            [0.0, 0.0, 1.0],
            1.5,
            &Transform3D::Identity,
        )
        .unwrap()
        .shaped(&options);
        assert_eq!(drain.depth_mm, Some(4.5));
        assert_eq!(drain.inner_radius_mm, Some(0.8));
        assert_eq!(drain.plug_clearance_mm, Some(0.15));
        let world = drain.to_world(&Transform3D::Identity);
        assert_eq!(world.depth, DrainDepth::Mm(4.5));
        assert_eq!(world.inner_radius_mm, Some(0.8));
        assert_eq!(world.plug_clearance_mm, Some(0.15));
        // Off, each part is gone from new drains, its value kept.
        let plain = LocalDrain::at([0.0; 3], [0.0, 0.0, 1.0], 1.5, &Transform3D::Identity)
            .unwrap()
            .shaped(&HollowOptions::default());
        assert_eq!(
            (
                plain.depth_mm,
                plain.inner_radius_mm,
                plain.plug_clearance_mm
            ),
            (None, None, None)
        );
        assert_eq!(
            plain.to_world(&Transform3D::Identity).depth,
            DrainDepth::Auto
        );
        assert_eq!(
            (clamped.plug_clearance_mm, clamped.drain_depth_mm),
            (1.0, 0.5)
        );
        // The drain tool does not change the cavity: the preview's key
        // leaves it out.
        assert_eq!(
            options.cavity_only(),
            HollowOptions::default().cavity_only()
        );
    }

    /// #318: plugs stand beside the model, nearest first, clear of it, of
    /// the other models and of one another, on the bed; without room, to
    /// the model's right.
    #[test]
    fn plugs_stand_beside_the_model_clear_of_the_others() {
        let model = ([40.0, 40.0], [70.0, 70.0]);
        let other = ([72.0, 30.0], [100.0, 80.0]);
        let bed = ([0.0, 0.0], [120.0, 100.0]);
        let spots = plug_spots(model, &[other], bed, &[[2.0, 2.0], [2.0, 2.0], [5.0, 3.0]]);
        let rects: Vec<PlateRect> = spots
            .iter()
            .zip([[2.0, 2.0], [2.0, 2.0], [5.0, 3.0]])
            .map(|(c, h)| ([c[0] - h[0], c[1] - h[1]], [c[0] + h[0], c[1] + h[1]]))
            .collect();
        let apart = |a: PlateRect, b: PlateRect| {
            a.1[0] + PLUG_GAP_MM <= b.0[0]
                || b.1[0] + PLUG_GAP_MM <= a.0[0]
                || a.1[1] + PLUG_GAP_MM <= b.0[1]
                || b.1[1] + PLUG_GAP_MM <= a.0[1]
        };
        for (i, &r) in rects.iter().enumerate() {
            assert!(apart(r, model) && apart(r, other), "{i}: {r:?}");
            assert!(r.0[0] >= 0.0 && r.0[1] >= 0.0 && r.1[0] <= 120.0 && r.1[1] <= 100.0);
            for &q in &rects[..i] {
                assert!(apart(r, q), "{i}: {r:?} on {q:?}");
            }
            // Beside the model: within a few gaps of it.
            let dx = (r.0[0] - model.1[0]).max(model.0[0] - r.1[0]).max(0.0);
            let dy = (r.0[1] - model.1[1]).max(model.0[1] - r.1[1]).max(0.0);
            assert!(dx.hypot(dy) < 3.0 * PLUG_GAP_MM, "{i}: {r:?}");
        }
        // A full bed: to the model's right.
        let full = plug_spots(model, &[bed], bed, &[[2.0, 2.0]]);
        assert_eq!(full, vec![[70.0 + PLUG_GAP_MM + 2.0, 55.0]]);
    }

    /// The leaves of `document`'s scene: each one's mesh, world box and name.
    fn leaves_of(
        document: &SceneDocument,
    ) -> Vec<(Arc<IndexedMesh>, PlateRect, f32, Option<String>)> {
        document
            .with_scene(|spatial| {
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter_map(|id| {
                        let query = spatial.leaf(id)?;
                        let b = query.transformed_aabb(query.world_transform())?;
                        let (lo, hi) = (b.min().coords(), b.max().coords());
                        let scene = spatial.graph();
                        Some((
                            scene.leaf_mesh_arc(id)?.clone(),
                            ([lo[0], lo[1]], [hi[0], hi[1]]),
                            lo[2],
                            scene.node(id)?.meta.as_ref().and_then(|m| m.name.clone()),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// #318 in the tab: a 30 mm cube hollowed with a cone drain that makes
    /// a plug gets a second model, the plug, standing on the plate beside
    /// it, in the same undo step; hollowing again with another drain
    /// replaces the plugs, the first where it stood; making the model solid
    /// again takes them away, and undo brings each state back. A set depth
    /// short of the cavity is left out with the usual notice (#316).
    #[test]
    fn plugs_come_and_go_with_their_hollow() {
        use crate::selection::{LeafId, Selection};
        let (states, notices, plug_shape) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0])),
                    LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let dialogs = ctrl.dialogs();
            let run = HollowRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
            run.on_option(0, 2.0);
            run.on_option(9, 1.0);
            run.on_option(10, 1.0);
            run.on_option(11, 1.0);
            let down = [0.0, 0.0, 1.0];
            let mut states = Vec::new();
            let mut snapshot = |label: &str| {
                let leaves = leaves_of(&document);
                states.push((label.to_owned(), leaves));
            };
            // A cone with a plug on top, placed on the solid model.
            run.on_drain_click([47.3, 52.1, 30.0], down);
            run.on_apply();
            land_job(&run);
            snapshot("hollowed");
            let plug_shape = document
                .hollows()
                .borrow()
                .last()
                .map(|h| (h.drains.clone(), h.plugs.len()));
            document.undo().unwrap();
            snapshot("undone");
            document.redo().unwrap();
            snapshot("redone");
            // A second drain, set too shallow: left out, nothing changes.
            run.on_option(7, 1.0);
            run.on_option(8, 1.0);
            run.on_drain_click([55.1, 44.2, 30.0], down);
            land_job(&run);
            snapshot("shallow");
            // Deep enough: hollowed again, with two plugs.
            run.on_option(8, 3.0);
            run.on_drain_click([55.1, 44.2, 30.0], down);
            land_job(&run);
            snapshot("second");
            run.on_restore();
            snapshot("solid");
            document.undo().unwrap();
            snapshot("hollow again");
            (states, dialogs.info_headings_for_test(), plug_shape)
        });
        let (drains, plugs) = plug_shape.expect("a hollow");
        assert_eq!(plugs, 1);
        assert_eq!(drains[0].inner_radius_mm, Some(1.0));
        assert_eq!(drains[0].plug_clearance_mm, Some(DEFAULT_PLUG_CLEARANCE_MM));
        assert_eq!(drains[0].depth_mm, None);
        assert_eq!(notices, vec!["Drain not drilled".to_owned()]);
        let by = |label: &str| &states.iter().find(|(l, _)| l == label).unwrap().1;
        let hollowed = by("hollowed");
        assert_eq!(hollowed.len(), 2, "the model and its plug");
        let (model, plug) = (&hollowed[0], &hollowed[1]);
        assert_eq!(plug.3.as_deref(), Some("Drain plug 1"));
        assert!(plug.2.abs() < 1e-4, "on the plate: {}", plug.2);
        let apart = model.1.1[0] <= plug.1.0[0]
            || plug.1.1[0] <= model.1.0[0]
            || model.1.1[1] <= plug.1.0[1]
            || plug.1.1[1] <= model.1.0[1];
        assert!(apart, "beside the model: {:?} {:?}", model.1, plug.1);
        // As wide as the hole at the surface (1.5 mm) less 0.05 mm, about.
        let width = plug.1.1[0] - plug.1.0[0];
        assert!((width - 2.9).abs() < 0.05, "{width}");
        assert_eq!(by("undone").len(), 1, "undo takes the plug with the hollow");
        assert!(
            Arc::ptr_eq(&by("redone")[1].0, &plug.0),
            "redo brings it back"
        );
        assert_eq!(by("shallow").len(), 2, "a drain left out changes nothing");
        let second = by("second");
        assert_eq!(second.len(), 3, "two plugs");
        let first_again = second
            .iter()
            .find(|l| l.3.as_deref() == Some("Drain plug 1"))
            .unwrap();
        assert_eq!(first_again.1, plug.1, "the first plug stays where it stood");
        assert!(
            second
                .iter()
                .any(|l| l.3.as_deref() == Some("Drain plug 2"))
        );
        assert_eq!(by("solid").len(), 1, "solid again: no plugs");
        assert_eq!(by("hollow again").len(), 3, "undo brings them back");
    }

    /// #113: a pasted copy of a hollow with a plug shows the same hollow.
    /// Hollowing the copy again stands plugs of its own beside it and
    /// leaves the model's plug, which the model still needs.
    #[test]
    fn hollowing_a_copy_again_leaves_the_model_its_plug() {
        use crate::selection::{LeafId, Selection};
        let (before, after) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0])),
                    LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            run.on_option(0, 2.0);
            run.on_option(9, 1.0);
            run.on_option(10, 1.0);
            run.on_option(11, 1.0);
            run.on_drain_click([47.3, 52.1, 30.0], [0.0, 0.0, 1.0]);
            run.on_apply();
            land_job(&run);
            let plugs = |document: &SceneDocument| {
                leaves_of(document)
                    .into_iter()
                    .filter_map(|l| l.3)
                    .filter(|name| name.starts_with("Drain plug"))
                    .count()
            };
            let before = plugs(&document);
            let mut clipboard = crate::clipboard::clipboard_from_document_selection(&document)
                .expect("the hollow copies");
            let copy = crate::clipboard::paste(
                &document,
                &clipboard,
                [150.0, 50.0],
                &crate::build_volume::NO_PRINTER_BUILD_VOLUME,
            )
            .unwrap()
            .expect("pasted");
            clipboard.bump_cascade();
            document.set_selection(Selection::One(LeafId(copy)));
            let at = match document
                .with_scene(|s| s.leaf(copy).map(|q| q.world_transform()))
                .flatten()
                .unwrap()
            {
                Transform3D::Affine(a) => *a.translation(),
                Transform3D::Identity => [0.0; 3],
            };
            run.on_drain_click(
                [at[0] as f32 + 5.1, at[1] as f32 - 5.8, 30.0],
                [0.0, 0.0, 1.0],
            );
            land_job(&run);
            (before, plugs(&document))
        });
        assert_eq!(before, 1, "the model's plug");
        assert_eq!(after, 3, "the model's, and the copy's two");
    }

    /// #179: a region drawn over a solid model's cavity preview takes it
    /// out of the cavity at once; hollowed, the hollow keeps it; taking it
    /// away hollows again, one undo step, and undo brings it back.
    #[test]
    fn a_drawn_solid_area_leaves_the_cavity_and_stays_with_the_hollow() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let world =
                Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0]));
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(None, world, LeafPayload::Mesh(Arc::new(cube)))
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            run.on_option(0, 2.0);
            // Seen from above, orthographic: everything right of x = 56.
            let region = LassoRegion {
                perspective: false,
                origin: [0.0, 0.0, 100.0],
                u: [1.0, 0.0, 0.0],
                v: [0.0, 1.0, 0.0],
                w: [0.0, 0.0, -1.0],
                polygon: vec![[56.0, 0.0], [100.0, 0.0], [100.0, 100.0], [56.0, 100.0]],
                depth: None,
            };
            let ready = |run: &HollowRun| {
                run.preview
                    .borrow()
                    .ready
                    .as_ref()
                    .map(|(key, cavity, _)| (*key, cavity.clone()))
            };
            let wait_for = |run: &HollowRun, solids: u64| {
                let waited = std::time::Instant::now();
                loop {
                    run.tick_preview();
                    if let Some((key, cavity)) = ready(run)
                        && key.solids == solids
                    {
                        return cavity;
                    }
                    assert!(
                        waited.elapsed() < Duration::from_secs(30),
                        "the preview lands"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            };
            let plain = wait_for(&run, 0);
            let reach = |cavity: &IndexedMesh| {
                cavity
                    .vertices()
                    .iter()
                    .map(|v| world.apply_to_vertex(*v).coords()[0])
                    .fold(f32::MIN, f32::max)
            };
            assert!((reach(&plain) - 63.0).abs() < 0.1, "{}", reach(&plain));

            run.on_lasso(region.clone());
            assert_eq!(window.get_hollow_solid_count(), 1);
            let model = document
                .with_scene(|s| s.graph().leaf_mesh_arc(leaf).cloned())
                .flatten()
                .unwrap();
            let local = run.solids_for(&model);
            let drawn = wait_for(&run, solids_key(&local));
            // The cavity stops on the region's side, not a voxel inside it.
            assert!((reach(&drawn) - 56.0).abs() < 0.05, "{}", reach(&drawn));

            // Hollowed: the hollow keeps the area, and its cavity stops there.
            run.on_apply();
            land_job(&run);
            let (hollow, solids) = {
                let hollows = document.hollows();
                let last = hollows.borrow().last().cloned().expect("a hollow");
                (last.hollow, last.solids)
            };
            assert_eq!(solids, local);
            let inner_reach = hollow
                .vertices()
                .iter()
                .map(|v| world.apply_to_vertex(*v).coords())
                .filter(|p| p[1] > 40.0 && p[1] < 60.0 && p[2] > 5.0 && p[2] < 25.0)
                .map(|p| p[0])
                .filter(|&x| x < 64.0)
                .fold(f32::MIN, f32::max);
            assert!((inner_reach - 56.0).abs() < 0.05, "{inner_reach}");

            // Taken away: hollowed again without it, in one undo step.
            run.edit_solids(Vec::clear);
            land_job(&run);
            assert_eq!(window.get_hollow_solid_count(), 0);
            assert!(document.undo().unwrap());
            run.refresh_ui();
            let shown = document
                .with_scene(|s| s.graph().leaf_mesh_arc(leaf).cloned())
                .flatten()
                .unwrap();
            assert!(Arc::ptr_eq(&shown, &hollow), "one undo step back");
            assert_eq!(window.get_hollow_solid_count(), 1);
        });
    }

    /// #446: a hollow made here records the engine that made it and the
    /// model's stretch, so a later build can tell what made its cavity.
    #[test]
    fn a_hollow_records_its_engine_and_the_models_stretch() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 20.0));
            }
            let world = Transform3D::Affine(
                AffineTransform3D::from_translation([50.0, 50.0, 10.0])
                    .compose(&AffineTransform3D::from_non_uniform_scale([1.5, 1.0, 0.75])),
            );
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(None, world, LeafPayload::Mesh(Arc::new(cube)))
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            run.on_option(0, 2.0);
            run.on_apply();
            land_job(&run);
            let last = document
                .hollows()
                .borrow()
                .last()
                .cloned()
                .expect("a hollow");
            assert_eq!(last.engine, Some(HOLLOW_ENGINE_VERSION));
            let stretch = last.stretch.expect("the stretch is recorded");
            let want = [1.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.75];
            for (got, want) in stretch.iter().zip(want) {
                assert!((got - want).abs() < 1e-9, "{stretch:?}");
            }
        });
    }

    /// #512: the drains and areas placed on a solid model are steps of
    /// history: Undo and Redo walk them back and forth one by one, undoing
    /// the hollow brings them back, and none of them makes the document
    /// unsaved.
    /// #450: a click on a drain picks it; the drain fields change it, Move
    /// puts it where the next click is, Remove takes it away; each one step
    /// that Undo takes back.
    #[test]
    fn a_drain_is_picked_changed_moved_and_removed_one_by_one() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let world =
                Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0]));
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(None, world, LeafPayload::Mesh(Arc::new(cube)))
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let drains = || {
                let (_, mesh, _) = run.selected().unwrap();
                run.source_for(&mesh).1
            };

            run.on_drain_click([50.0, 50.0, 30.0], [0.0, 0.0, 1.0]);
            run.on_drain_click([42.0, 42.0, 30.0], [0.0, 0.0, 1.0]);
            assert_eq!(drains().len(), 2);
            let before = drains()[0];

            // A click on the first drain picks it, adds none.
            run.on_drain_click([50.5, 50.0, 30.0], [0.0, 0.0, 1.0]);
            assert_eq!(drains().len(), 2);
            assert_eq!(window.get_hollow_drain_picked(), 0);

            // Its radius, and Undo.
            run.on_option_changed(6, 3.0);
            assert_eq!(drains()[0].radius_mm, 3.0);
            assert_eq!(drains()[1].radius_mm, before.radius_mm, "the other stays");
            assert!(document.undo().unwrap());
            assert_eq!(drains()[0].radius_mm, before.radius_mm);
            assert!(document.redo().unwrap());

            // Moved by the next click.
            run.move_picked_drain();
            assert!(window.get_hollow_drain_moving());
            run.on_drain_click([58.0, 58.0, 30.0], [0.0, 0.0, 1.0]);
            assert_eq!(drains().len(), 2);
            assert!((drains()[0].center[0] - before.center[0] - 8.0).abs() < 1e-3);
            assert_eq!(drains()[0].radius_mm, 3.0, "it keeps its shape");

            // Removed.
            run.remove_picked_drain();
            assert_eq!(drains().len(), 1);
            assert_eq!(window.get_hollow_drain_picked(), -1);
            assert!(document.undo().unwrap());
            assert_eq!(drains().len(), 2);
        });
    }

    #[test]
    fn the_marks_placed_before_hollowing_are_undone_one_by_one() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let world =
                Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, 15.0]));
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(None, world, LeafPayload::Mesh(Arc::new(cube)))
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            run.on_option(0, 2.0);
            document.mark_saved();
            let counts = || {
                (
                    window.get_hollow_drain_count(),
                    window.get_hollow_solid_count(),
                )
            };

            run.on_drain_click([50.0, 50.0, 30.0], [0.0, 0.0, 1.0]);
            run.on_drain_click([45.0, 45.0, 30.0], [0.0, 0.0, 1.0]);
            run.on_lasso(LassoRegion {
                perspective: false,
                origin: [0.0, 0.0, 100.0],
                u: [1.0, 0.0, 0.0],
                v: [0.0, 1.0, 0.0],
                w: [0.0, 0.0, -1.0],
                polygon: vec![[56.0, 0.0], [100.0, 0.0], [100.0, 100.0], [56.0, 100.0]],
                depth: None,
            });
            assert_eq!(counts(), (2, 1));
            assert!(!document.is_dirty(), "marks are not in the file");

            assert!(document.undo().unwrap());
            assert_eq!(counts(), (2, 0));
            assert!(document.undo().unwrap());
            assert_eq!(counts(), (1, 0));
            assert!(document.redo().unwrap());
            assert!(document.redo().unwrap());
            assert_eq!(counts(), (2, 1));
            run.on_clear_drains();
            assert_eq!(counts(), (0, 1));
            assert!(document.undo().unwrap());
            assert_eq!(counts(), (2, 1));
            assert!(!document.is_dirty());

            // Hollowed: the marks are drilled; Undo brings the solid model
            // back with them, Redo the hollow.
            run.on_apply();
            land_job(&run);
            assert_eq!(window.get_hollow_model_state(), 1);
            assert!(document.is_dirty());
            assert!(document.undo().unwrap());
            run.refresh_ui();
            assert_eq!(window.get_hollow_model_state(), 0);
            assert_eq!(counts(), (2, 1));
            assert!(!document.is_dirty());
            assert!(document.redo().unwrap());
            run.refresh_ui();
            assert_eq!(window.get_hollow_model_state(), 1);
            assert_eq!(counts(), (2, 1));
        });
    }

    /// A `side` mm cube around the origin, facing out, or in when `inward`.
    fn cube_of(side: f32, inward: bool) -> IndexedMesh {
        let mut cube = sc_mesh_fixtures::unit_cube_mesh();
        for v in &mut cube.vertices {
            *v = Vertex::new(v.as_array().map(|c| c * side));
        }
        if inward {
            for f in &mut cube.faces {
                let [a, b, c] = f.vertices.map(u32::from);
                *f = sc_mesh_core::IndexedTriangle::from_raw_indices([a, c, b]);
            }
        }
        cube
    }

    /// A 30 mm cube with a 26 mm cavity, as a file brings a hollow: no
    /// record of how it was made (#447).
    fn imported_hollow() -> IndexedMesh {
        let (outer, inner) = (cube_of(30.0, false), cube_of(26.0, true));
        let offset = outer.vertices.len() as u32;
        let vertices = outer
            .vertices
            .iter()
            .chain(&inner.vertices)
            .copied()
            .collect();
        let faces = outer
            .faces
            .iter()
            .copied()
            .chain(inner.faces.iter().map(|f| {
                sc_mesh_core::IndexedTriangle::from_raw_indices(
                    f.vertices.map(|v| u32::from(v) + offset),
                )
            }))
            .collect();
        IndexedMesh::new(None, vertices, faces)
    }

    /// A window with `mesh` standing at (50, 50) on the plate, `lift` mm
    /// up its centre, selected, in the Hollow tab.
    fn hollow_tab_with(
        mesh: IndexedMesh,
        lift: f64,
    ) -> (MainWindow, Rc<ViewportController>, Rc<HollowRun>, NodeId) {
        use crate::selection::{LeafId, Selection};
        let window = MainWindow::new().unwrap();
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut graph = sc_scene::Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([50.0, 50.0, lift])),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        document.install_graph(graph, std::path::PathBuf::from("model.stl"));
        document.set_selection(Selection::One(LeafId(leaf)));
        window.set_workspace_section(WorkspaceSection::Hollow);
        let run = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
        run.on_option(0, 2.0);
        (window, ctrl, run, leaf)
    }

    fn mesh_on(ctrl: &ViewportController, leaf: NodeId) -> Arc<IndexedMesh> {
        ctrl.document()
            .with_scene(|s| s.graph().leaf_mesh_arc(leaf).cloned())
            .flatten()
            .expect("the model's mesh")
    }

    /// #447: a hollow from a file takes a drain drilled into it as it is,
    /// with no cavity made: one undo, the rest of it untouched, its sealed
    /// cavity counted until the drain opens it (#130), and Remove all
    /// drains gives it back as it came.
    #[test]
    fn a_hollow_from_a_file_takes_a_drain_as_it_is() {
        on_fresh_slint_thread(|| {
            let (window, ctrl, run, leaf) = hollow_tab_with(imported_hollow(), 15.0);
            let document = ctrl.document();
            let came = mesh_on(&ctrl, leaf);
            run.tick_preview();
            run.probe_now(
                &came,
                &document
                    .with_scene(|s| s.graph().world_transform_of(leaf).unwrap())
                    .unwrap(),
            );
            run.refresh_ui();
            assert_eq!(window.get_hollow_model_state(), 3, "it came hollow");
            assert_eq!(window.get_hollow_sealed_count(), 1);
            run.on_apply();
            assert!(run.job.borrow().is_none(), "nothing hollows it again");

            run.on_drain_click([50.4, 50.3, 0.0], [0.0, 0.0, -1.0]);
            land_job(&run);
            let drilled = mesh_on(&ctrl, leaf);
            assert!(!Arc::ptr_eq(&drilled, &came));
            assert!(drilled.is_sliceable());
            let record = run.record_of(&drilled).expect("its record");
            assert!(record.drilled);
            assert!(Arc::ptr_eq(&record.original, &came));
            assert_eq!(record.drains.len(), 1);
            run.tick_preview();
            run.refresh_ui();
            assert_eq!(window.get_hollow_model_state(), 3);
            run.probe_now(
                &drilled,
                &document
                    .with_scene(|s| s.graph().world_transform_of(leaf).unwrap())
                    .unwrap(),
            );
            run.refresh_ui();
            assert_eq!(window.get_hollow_sealed_count(), 0, "the drain opened it");
            // The cavity is the one it came with: the same volume, less the hole.
            let volume = |m: &IndexedMesh| {
                m.faces()
                    .iter()
                    .map(|f| {
                        let [a, b, c] = f.vertices.map(|v| {
                            m.vertices()[u32::from(v) as usize]
                                .as_array()
                                .map(f64::from)
                        });
                        (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                            + a[2] * (b[0] * c[1] - b[1] * c[0]))
                            / 6.0
                    })
                    .sum::<f64>()
            };
            let hole = std::f64::consts::PI * 1.5f64.powi(2) * 2.0;
            let lost = volume(&came) - volume(&drilled);
            assert!((lost - hole).abs() < 0.15 * hole, "{lost} vs {hole}");

            // One undo puts it back as it came; redo drills it again.
            assert!(document.undo().unwrap());
            assert!(Arc::ptr_eq(&mesh_on(&ctrl, leaf), &came));
            assert!(document.redo().unwrap());
            run.on_clear_drains();
            assert!(Arc::ptr_eq(&mesh_on(&ctrl, leaf), &came), "back as it came");
        });
    }

    /// #448: two quick clicks with the drain tool make two drains: the
    /// second waits for the first one's job.
    #[test]
    fn two_quick_drain_clicks_make_two_drains() {
        on_fresh_slint_thread(|| {
            let (_window, ctrl, run, leaf) = hollow_tab_with(cube_of(30.0, false), 15.0);
            run.on_apply();
            land_job(&run);
            run.on_drain_click([50.0, 50.0, 0.0], [0.0, 0.0, -1.0]);
            run.on_drain_click([44.0, 44.0, 0.0], [0.0, 0.0, -1.0]);
            assert_eq!(run.queued.borrow().len(), 1, "the second waits");
            land_job(&run);
            let hollow = mesh_on(&ctrl, leaf);
            let record = run.record_of(&hollow).expect("the hollow's record");
            assert_eq!(record.drains.len(), 2);
            assert!(!record.drilled, "hollowed here");
            assert!(hollow.is_sliceable());
            assert!(run.queued.borrow().is_empty());
        });
    }

    /// #448: a model with no room for a cavity behind its wall is left as it
    /// is, and the tab says why.
    #[test]
    fn a_plate_too_thin_for_a_cavity_is_left_solid() {
        on_fresh_slint_thread(|| {
            let mut plate = cube_of(1.0, false);
            for v in &mut plate.vertices {
                let [x, y, z] = *v.as_array();
                *v = Vertex::new([x * 30.0, y * 30.0, z * 3.0]);
            }
            let (_window, ctrl, run, leaf) = hollow_tab_with(plate, 1.5);
            let before = mesh_on(&ctrl, leaf);
            run.on_apply();
            land_job(&run);
            assert!(
                Arc::ptr_eq(&mesh_on(&ctrl, leaf), &before),
                "left as it was"
            );
            assert!(run.record_of(&before).is_none());
            assert_eq!(
                ctrl.dialogs().info_headings_for_test(),
                vec!["No room to hollow this model".to_owned()]
            );
        });
    }

    /// #448: a mesh the slicer refuses is not hollowed: the tab says so and
    /// offers Repair.
    #[test]
    fn an_open_mesh_offers_repair_instead_of_hollowing() {
        on_fresh_slint_thread(|| {
            let mut open = cube_of(30.0, false);
            open.faces.pop();
            let (_window, ctrl, run, _) = hollow_tab_with(open, 15.0);
            run.on_apply();
            assert!(run.job.borrow().is_none(), "no job runs");
            assert_eq!(
                ctrl.dialogs().error_headings_for_test(),
                vec!["This model can't be hollowed as it is".to_owned()]
            );
        });
    }
}

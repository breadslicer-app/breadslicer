/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Hollow tab (BS-8): hollows the selected model with the DragonFruit
//! engine port (`sc_geom::df_hollow`) and drills drains where the model is
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
//! ([`HollowRun::hollow_at`]) and opens the drain tool on that model (#158).
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

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use sc_geom::df_hollow::hollow_df::engine::{DEFAULT_PLUG_CLEARANCE_MM, DRAIN_PAST_WALL_MM};
use sc_geom::df_hollow::hollow_df::{
    DrainDepth, GridRequest, HollowFill, HollowSettings, InfillMode, ResolutionBound, Stage,
};
use sc_geom::df_hollow::{
    HollowKind, HollowOutcome, HollowPlug, HollowRequest, HollowResult, LeafFrame, PreparedHollow,
    WorldDrain, start_hollow_mesh,
};
use sc_geom::{JobError, JobHandle};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_scene::NodeId;
use sc_scene::node::{LeafAttachment, LeafPayload, SceneMetadata};
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};
use crate::scene_document::{
    SceneDocument, SceneEvent, SceneRevision, SceneSubscription, leaf_mesh_swap,
};
use crate::viewport::ViewportController;
use crate::{MainWindow, WorkspaceSection};

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How often the previews look at the tab, the selection and the settings.
const PREVIEW_TICK: Duration = Duration::from_millis(40);
/// Sides of a drawn drain.
const DRAIN_SIDES: u32 = 24;

/// Every stage of the hollow engine, in the order it runs them.
const STAGES: [Stage; 11] = [
    Stage::Voxelize,
    Stage::FloodFill,
    Stage::ParityRefine,
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
        Stage::ParityRefine => ctr!("HollowRun", "Restoring sealed voids"),
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
}

impl Default for HollowOptions {
    fn default() -> Self {
        // DragonFruit's defaults; the drain shapes off, Damian's clearance.
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

    fn settings(&self) -> HollowSettings {
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
            ..HollowSettings::default()
        }
    }
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

    fn to_world(self, world: &Transform3D) -> WorldDrain {
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
}

/// The job in flight and what it needs to land.
struct Job {
    handle: JobHandle<HollowOutcome>,
    leaf: NodeId,
    started_on: SceneRevision,
    /// The leaf's mesh when the job started: the edit's `before`.
    before: Arc<IndexedMesh>,
    original: Arc<IndexedMesh>,
    drains: Vec<LocalDrain>,
    options: HollowOptions,
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
    /// The local bounding box of the last mesh asked about, by identity, for
    /// the recommended wall (a scan of every vertex once per mesh).
    bounds: RefCell<Option<MeshBounds>>,
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
    drains_piece: Option<(DrainsPieceKey, Arc<OverlayMesh>)>,
    /// How deep the drawn drains go, as the engine measures them on the
    /// ready cavity, and what for (#315).
    depths: Option<(DepthsKey, Vec<f32>)>,
    /// What the shown overlay was drawn from, and the overlay.
    shown: Option<(ShownKey, Overlay)>,
}

/// A drawn cavity: which cavity mesh, at which pose.
type CavityPieceKey = (usize, [[f64; 4]; 4]);
/// Drawn drains: which, at which pose, how deep (bits of each depth).
type DrainsPieceKey = (Vec<LocalDrain>, [[f64; 4]; 4], Vec<u32>);
/// Measured drain depths: which drains, at which pose, on which session,
/// with which settings.
type DepthsKey = (Vec<LocalDrain>, [[f64; 4]; 4], usize, PreviewKey);

/// Everything the overlay is drawn from.
#[derive(Debug, Clone, PartialEq)]
struct ShownKey {
    cavity: Option<usize>,
    drains: Option<usize>,
    hover: Option<(DrainHover, DrainReach)>,
    world: [[f64; 4]; 4],
    /// The drain tool's settings, which shape the drain under the cursor.
    tool: HollowOptions,
    /// Bits of the depth the drain under the cursor is drawn to.
    hover_depth: u32,
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
                prepared: RefCell::new(None),
                _scene_events: scene_events,
                ctrl,
                preview: RefCell::new(CavityPreview::default()),
                hover: Cell::new(None),
                preview_timer: slint::Timer::default(),
                pending: RefCell::new(Vec::new()),
                bounds: RefCell::new(None),
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
                if let Some(job) = run.job.borrow().as_ref() {
                    job.handle.cancel();
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
            ui.on_hollow_option_changed(move |code, value| run.on_option(code, value));
        }
        {
            let run = run.clone();
            ui.on_hollow_drain_clicked(move |x, y, z, nx, ny, nz| {
                run.on_drain_click([x, y, z], [nx, ny, nz]);
            });
        }
        run
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

    /// 0 a solid model, 1 a hollow this driver made, 2 the support mesh.
    fn model_state(&self, mesh: &Arc<IndexedMesh>) -> i32 {
        if sc_geom::df_supports::is_support_mesh(mesh) {
            2
        } else if self.entry_for(mesh).is_some() {
            1
        } else {
            0
        }
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

    fn set_pending(&self, mesh: &Arc<IndexedMesh>, drains: Vec<LocalDrain>) {
        let ptr = Arc::as_ptr(mesh) as usize;
        let mut pending = self.pending.borrow_mut();
        pending.retain(|(m, _)| *m != ptr);
        if !drains.is_empty() {
            pending.push((ptr, drains));
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
        ui.set_hollow_model_state(state);
        ui.set_hollow_drain_count(drains as i32);
        if state == 2 {
            ui.set_hollow_drain_tool(false);
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
    /// clearance. Values are clamped to what the engine accepts.
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
                _ => return,
            }
        }
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
        self.tick_preview();
    }

    fn on_apply(self: &Rc<Self>) {
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        if self.model_state(&mesh) == 2 {
            return;
        }
        // Hollowing a hollow again starts from its original, keeping drains;
        // a solid model takes the drains placed on it.
        let (original, drains) = self.source_for(&mesh);
        self.launch(leaf, mesh, original, drains, &world);
    }

    fn on_restore(&self) {
        let Some((leaf, mesh, _)) = self.selected() else {
            return;
        };
        let Some((original, _)) = self.entry_for(&mesh) else {
            return;
        };
        // The plugs go with the hollow.
        let old = self.plugs_of(&mesh);
        self.swap_mesh_and_plugs(leaf, mesh, original, &old, Vec::new());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_last_run(ctr!(
                "HollowRun",
                "Solid again. Undo brings the hollow back."
            ));
        }
    }

    fn on_clear_drains(self: &Rc<Self>) {
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        let Some((original, drains)) = self.entry_for(&mesh) else {
            self.set_pending(&mesh, Vec::new());
            self.refresh_ui();
            return;
        };
        if !drains.is_empty() {
            self.launch(leaf, mesh, original, Vec::new(), &world);
        }
    }

    /// A click with the drain tool at a world point and surface normal on
    /// the selected model (the viewport selects the model it hit first). A
    /// hollow is hollowed again with the new drain; a solid model keeps it
    /// for when it is hollowed.
    fn on_drain_click(self: &Rc<Self>, point: [f32; 3], normal: [f32; 3]) {
        if self.job.borrow().is_some() {
            return;
        }
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        if self.model_state(&mesh) == 2 {
            return;
        }
        let options = *self.options.borrow();
        let Some(drain) = LocalDrain::at(point, normal, options.drain_radius_mm, &world)
            .map(|d| d.shaped(&options))
        else {
            return;
        };
        let (original, mut drains) = self.source_for(&mesh);
        drains.push(drain);
        if self.entry_for(&mesh).is_some() {
            self.launch(leaf, mesh, original, drains, &world);
        } else {
            self.set_pending(&mesh, drains);
            self.refresh_ui();
            self.tick_preview();
        }
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
        self.document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter_map(|leaf| {
                        let mesh = scene.leaf_mesh_arc(leaf)?;
                        if !hollows.iter().any(|h| Arc::ptr_eq(h, mesh)) {
                            return None;
                        }
                        let world = scene.world_transform_of(leaf).ok()?;
                        let exit = floor_underside(mesh, &world, at)?;
                        Some((leaf, mesh.clone(), world, exit))
                    })
                    .max_by(|a, b| a.3[2].total_cmp(&b.3[2]))
            })
            .flatten()
            .and_then(|(leaf, mesh, world, exit)| {
                let drain = LocalDrain::at(exit, [0.0, 0.0, -1.0], radius_mm, &world)?;
                Some((leaf, mesh, world, drain))
            })
    }

    /// The model hollowed here whose hollow holds the cavity at `at` (world
    /// mm), and how many drain holes that hollow has; `None` when no model
    /// hollowed here holds it.
    pub(crate) fn hollow_at(&self, at: [f32; 3]) -> Option<(NodeId, usize)> {
        let (leaf, mesh, _, _) = self.drain_target(at)?;
        Some((leaf, self.source_for(&mesh).1.len()))
    }

    fn launch(
        self: &Rc<Self>,
        leaf: NodeId,
        before: Arc<IndexedMesh>,
        original: Arc<IndexedMesh>,
        drains: Vec<LocalDrain>,
        world: &Transform3D,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.job.borrow().is_some() || ui.get_loading() {
            return;
        }
        let options = *self.options.borrow();
        let request = HollowRequest {
            kind: HollowKind::Apply,
            grid: GridRequest {
                voxel_size_mm: options.voxel_mm,
                ..GridRequest::default()
            },
            settings: options.settings(),
            drains: drains.iter().map(|d| d.to_world(world)).collect(),
            prepared: self.prepared.borrow().clone(),
            ..HollowRequest::default()
        };
        match start_hollow_mesh(original.clone(), world, request) {
            Ok(handle) => {
                *self.job.borrow_mut() = Some(Job {
                    handle,
                    leaf,
                    started_on: self.document.scene_revision(),
                    before,
                    original,
                    drains,
                    options,
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
                    let progress = job.handle.progress();
                    ui.set_hollow_progress(progress.fraction());
                    ui.set_hollow_phase(phase_text(&progress.phase));
                    job.handle.try_poll()
                };
                if let Some(outcome) = finished {
                    stop();
                    let job = run.job.borrow_mut().take();
                    ui.set_hollow_running(false);
                    if let Some(job) = job {
                        run.finish(job, outcome);
                    }
                }
            },
        );
    }

    fn finish(&self, job: Job, outcome: Result<HollowOutcome, JobError>) {
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
        *self.prepared.borrow_mut() = Some(outcome.prepared.clone());
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
            return;
        }
        let HollowResult::Apply {
            mesh,
            drains,
            plugs,
        } = outcome.result
        else {
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
        if missed > 0 {
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
        // A new drain that was left out changes nothing: no undo step.
        let unchanged = self.hollowed.borrow().iter().any(|h| {
            Arc::ptr_eq(&h.hollow, &job.before)
                && Arc::ptr_eq(&h.original, &job.original)
                && h.options == job.options
                && h.drains == kept
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
        let hollow = Arc::new(mesh);
        let Some(plug_links) =
            self.swap_mesh_and_plugs(job.leaf, job.before, hollow.clone(), &old_plugs, new_plugs)
        else {
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
        if !drains.is_empty() {
            summary.push(' ');
            summary.push_str(&ctr!(
                "HollowRun",
                "One drain drilled.",
                "{n} drains drilled.",
                kept.len()
            ));
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
            if made_plugs > 0 {
                summary.push(' ');
                summary.push_str(&ctr!(
                    "HollowRun",
                    "One plug placed beside the model.",
                    "{n} plugs placed beside the model.",
                    made_plugs
                ));
            }
        }
        if plug_failures > 0 {
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!(
                    "HollowRun",
                    "Plug not made",
                    "{n} plugs not made",
                    plug_failures
                ),
                message: ctr!(
                    "HollowRun",
                    "The drain was drilled, but its plug did not come out as a closed solid, so it was left out.",
                    "The drains were drilled, but their plugs did not come out as closed solids, so they were left out.",
                    plug_failures
                ),
                details: Default::default(),
            });
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_last_run(summary.into());
        }
        // The drains placed before are drilled now: they live on the hollow.
        let original_ptr = Arc::as_ptr(&job.original) as usize;
        self.pending
            .borrow_mut()
            .retain(|(m, _)| *m != original_ptr);
        // The tab already shows the settings this hollow was made with.
        self.adopted.set(Some(Arc::as_ptr(&hollow) as usize));
        self.hollowed.borrow_mut().push(Hollowed {
            hollow,
            original: job.original,
            drains: kept,
            options: job.options,
            plugs: plug_links,
        });
        self.refresh_ui();
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
                // The old plugs still in the scene, and the drain each was for.
                let gone: Vec<(NodeId, LocalDrain)> = leaves
                    .iter()
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
        match self.document.apply_edit(edit.clone()) {
            Ok(_) => {
                self.document.push_edit(edit);
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
        let (original, drains) = self.source_for(&mesh);
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
        };
        let show_cavity = ui.get_hollow_show_cavity();
        if show_cavity {
            self.advance_cavity_job(key, &original, &world, ui.get_hollow_drain_tool(), &drains);
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
                let (reach, depth) =
                    drain_reach(preview.ready.as_ref(), key, hover, &world, &options);
                hover_depth = depth.unwrap_or(hover_depth);
                (hover, reach)
            });
        ui.set_hollow_drain_reach(hover.map_or(0, |(_, reach)| reach.code()));
        let depths = drain_depths(&mut preview, &drains, key, &world, matrix, &options)
            .unwrap_or_else(|| {
                drains
                    .iter()
                    .map(|d| d.depth_mm.unwrap_or(fallback_depth))
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
                    let mut piece = crate::df_support_run::overlay_of(&cavity, OverlayTint::Cavity);
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
                depths.iter().map(|d| d.to_bits()).collect::<Vec<_>>(),
            );
            match &preview.drains_piece {
                Some((k, piece)) if *k == id => piece.clone(),
                _ => {
                    let mut mesh = (Vec::new(), Vec::new());
                    for (drain, &depth) in drains.iter().zip(&depths) {
                        let w = drain.to_world(&world);
                        let radii = [w.radius_mm, drain.end_radius_mm()];
                        drain_solid(w.center, w.direction, radii, depth, &mut mesh);
                    }
                    let piece = Arc::new(OverlayMesh {
                        positions: mesh.0,
                        indices: mesh.1,
                        tint: OverlayTint::Drain,
                    });
                    preview.drains_piece = Some((id, piece.clone()));
                    piece
                }
            }
        });
        let shown_key = ShownKey {
            cavity: cavity_piece.as_ref().map(|p| Arc::as_ptr(p) as usize),
            drains: drains_piece.as_ref().map(|p| Arc::as_ptr(p) as usize),
            hover,
            world: matrix,
            tool: options,
            hover_depth: hover_depth.to_bits(),
        };
        if let Some((k, shown)) = &preview.shown
            && *k == shown_key
            && self.ctrl.preview_overlay_is(shown)
        {
            return;
        }
        let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
        pieces.extend(cavity_piece);
        pieces.extend(drains_piece);
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
) -> Option<Vec<f32>> {
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
    let depths: Option<Vec<f32>> = drains
        .iter()
        .map(|d| {
            prepared
                .drain_report(world, &settings, &d.to_world(world))
                .map(|r| r.depth_mm)
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
            let outcome = run.job.borrow().as_ref().and_then(|j| j.handle.try_poll());
            if let Some(outcome) = outcome {
                let job = run.job.borrow_mut().take().expect("the job");
                run.finish(job, outcome);
                return;
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
            document.redo().unwrap();
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
        assert_eq!(undo_steps, 1);
        assert_eq!(after_click, 1, "the left-out drain added no step");
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
        let (states, volume, kept, notices) = on_fresh_slint_thread(|| {
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
            run.on_apply();
            land_job(&run);
            let kept = document
                .hollows()
                .borrow()
                .last()
                .map(|h| h.drains.iter().map(|d| d.center).collect::<Vec<_>>());
            (states, volume, kept, dialogs.info_headings_for_test())
        });
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
}

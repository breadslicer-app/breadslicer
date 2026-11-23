/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for generating supports:
//! runs `sc_geom::supports` (island scan, placement, bracing, raft) on the
//! scene.
//!
//! Like `GenSupportRun`, the job runs on its own worker thread behind a
//! `JobHandle`, and a `slint::Timer` polls its progress into the shared
//! support-generation panel, which shows a Cancel button while this run is in
//! flight. On completion the supports and raft, already tessellated and
//! welded into one mesh, are pasted into the scene through
//! `SceneDocument::paste_fragment`: one undoable object, saved and exported
//! like any other. A later run leaves that object out of its models.
//!
//! **Regeneration (#153).** A model moved in Z, tilted or scaled loses its
//! supports in the edit (`support_follow`) and waits for new ones
//! ([`SceneDocument::supports_wanted`]). Once it stands where the edit left
//! it, the same run makes them for that model alone, and they join the
//! edit's undo entry: one Undo takes the model back with its old supports.
//! A later edit, or Undo, cancels the run and leaves the next edit to wait.
//!
//! **Prescan (BS-66, BS-67, BS-68).** The island and overhang scans depend only on
//! the geometry, its pose and the scan settings, so a second, lower-key job
//! runs them ahead of time (`start_prescan`, in a pool of half the cores)
//! and the next run starts from them. It starts when the Hollow or Supports
//! tab is open (the pose is settled there), or, with the Preferences >
//! Slicing switch, whenever the scene has been still for [`PRESCAN_SETTLE`].
//! Nothing of it is on the transform path: a scene event only stamps the
//! time; a poll starts the prescan, and cancels it when the scene moves on.
//! A Generate press while a prescan of the same scene is running waits for
//! it rather than scanning twice. Reuse is safe by construction: the core
//! keys the scans on the baked world geometry, so a stale scan never
//! matches.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use sc_geom::JobError;
use sc_geom::JobHandle;
use sc_geom::supports::zone_presets::{
    ZonePreset, figures_70, figures_150, minis_30,
};
use sc_geom::supports::{
    AddNote, AddOptions, BraceEnd, SupportPrescan, SupportOptions, SupportOutcome, SupportRequest,
    SupportSession, EditError, EditPreview, HandBraceRefusal, HandEditKind, ImportedPlan,
    KeptEdits, LostEdit, LostWhy, PartSizes, RaftMode, ScanCache, scene_model_leaves,
    start_prescan, start_supports,
};
use sc_mesh_core::presupported::ImportedRoute;
use sc_mesh_core::transform::Transform3D;
use sc_print_profiles::resolve_layer_height;
use sc_scene::NodeId;
use sc_scene_edit::SceneFragment;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts, MessageKey, StaleSupportsChoice};
use crate::i18n::ctr;
use crate::imported_supports::ImportedBody;
use crate::mesh_render::{OverlayMesh, OverlayTint};
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::print_selection::PrintDefaults;
use crate::raft_clash::{
    RaftClash, lift_edit, lowest_model_after_lift_mm, raft_clash, resting_lift_edit,
    support_lift_mm,
};
use crate::review::TipHold;
use crate::scene_document::{
    SceneDocument, SceneEdit, SceneEvent, SceneRevision, SceneSubscription, SupportsWanted,
    leaf_mesh_swap,
};
use crate::slice_projection::load_selected_profiles;
use crate::support_hints::Hint;
use crate::support_persist::{
    DrawnSet, StaleWhy, SupportSessions, in_force, latest_drawing, model_of, record, sets_in_force,
    sets_of_model, support_leaf, support_leaves,
};
use crate::viewport::support_drag::{
    DragPoint, DragView, Project, ScreenHit, SupportDragHandler, SupportDragKind,
};
use crate::viewport::{HiddenSet, HiddenSupports, ViewportController};
use crate::{MainWindow, WorkspaceSection};

/// How much around an island a jump to it frames (#538), in mm.
const ISLAND_VIEW_RADIUS_MM: f32 = 12.0;

/// How far from a click on a support, in mm, the Select tool takes it
/// (#567): a click lands on its surface, a trunk's radius off its line.
const SELECT_REACH_MM: f64 = 2.5;

/// How near a picked support's tip stays to where it was picked, in mm:
/// drawn again with other numbers, it touches the model in the same place.
const CHOSEN_TIP_MM: f64 = 0.05;

/// An overlay piece of a mesh in world coordinates.
pub(crate) fn overlay_of(mesh: &sc_mesh_core::IndexedMesh, tint: OverlayTint) -> OverlayMesh {
    OverlayMesh {
        positions: mesh.vertices().iter().map(|v| *v.as_array()).collect(),
        indices: mesh
            .faces()
            .iter()
            .flat_map(|f| f.vertices.map(u32::from))
            .collect(),
        tint,
    }
}

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the scene must sit still before the prescan starts.
const PRESCAN_SETTLE: Duration = Duration::from_millis(500);

/// How long the scene must sit still (and the Hollow tab idle) before the
/// supports are generated ahead of a Generate press.
const SPECULATE_SETTLE: Duration = Duration::from_millis(1500);

/// How far from the model's new surface a tip may end and its supports stay
/// as they are when the mesh changes under them (#526), mm: a tip goes into
/// the model by its radius, so a surface this close still holds it.
pub(crate) const HEAL_REACH_MM: f64 = 0.25;

/// How [`SupportRun::heal_stale`] left a set out of date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Healed {
    /// In step again, as it was.
    InForce,
    /// Being made again, or to be once the run in flight is done.
    Redoing,
    /// Left as it is: nothing to do, or already tried for this state.
    No,
}

/// Supports generated ahead of a Generate press, for the scene and request
/// they were made for. A request is told apart by its printer figures and
/// options (`ahead_key`). Each run carries the lift of the models resting on the plate it was
/// generated for (#389), which a Generate press makes with the paste.
#[derive(Default)]
struct Ahead {
    running: Option<(
        JobHandle<SupportOutcome>,
        SceneRevision,
        String,
        Option<SceneEdit>,
    )>,
    ready: Option<(SceneRevision, String, SupportOutcome, Option<SceneEdit>)>,
}

/// What tells two requests apart for [`Ahead`], with whether models resting
/// on the plate are lifted first (#389).
fn ahead_key(request: &SupportRequest, lift_on_plate: bool) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{lift_on_plate}",
        request.layer_height_mm, request.pixel_pitch_mm, request.options, request.models
    )
}

/// How often the prescan watcher looks at the scene.
const PRESCAN_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// The scene and tab grid models with supports on another grid were
/// counted on (#576), and those models.
type GridCount = (Option<(SceneRevision, Option<u64>)>, Vec<NodeId>);

/// The background prescan's state.
#[derive(Default)]
struct Prescan {
    /// The job in flight, the scene it started on and when.
    running: Option<(JobHandle<SupportPrescan>, SceneRevision, Instant)>,
    /// The last finished prescan and the scene it ran on.
    ready: Option<(SceneRevision, Arc<ScanCache>)>,
    /// A Generate press waiting for the running prescan: the run starts
    /// from its scans as soon as it finishes.
    run_waiting: bool,
}

/// Told the picked supports, their sets' leaves and tips (#245).
type ChosenListener = Rc<dyn Fn(Vec<(NodeId, [f64; 3])>)>;

pub(crate) struct SupportRun {
    ui: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    handle: RefCell<Option<JobHandle<SupportOutcome>>>,
    // The scene the run was started on: a run that finishes on edited
    // geometry is discarded rather than pasted.
    started_on: RefCell<Option<SceneRevision>>,
    /// The lift of the models resting on the plate that the run in flight
    /// was generated for (#389), not made yet: it goes into the scene with
    /// the supports, as one undo step, or not at all.
    plate_lift: RefCell<Option<SceneEdit>>,
    /// Cancel was pressed for the run in flight: whatever it returns, even a
    /// result that came in just before, changes nothing.
    cancelled: Cell<bool>,
    /// Keeps the Supports tab's "Lift ... when the model rests on the plate"
    /// box (#389) in step with the settings, shared by every window.
    lift_pref: RefCell<Option<Subscription>>,
    overhang_pref: RefCell<Option<Subscription>>,
    /// Keeps the Supports tab's raft-under-the-plate box (#492) following the
    /// shared setting.
    raft_under_plate_pref: RefCell<Option<Subscription>>,
    /// Keeps the Supports tab's support-lines box (#493) following the
    /// shared setting.
    lines_under_plate_pref: RefCell<Option<Subscription>>,
    support_view_pref: RefCell<Option<Subscription>>,
    /// For the run in flight that regenerates supports (#153): the scene
    /// revision that left its models waiting, and the models.
    regenerating: RefCell<Option<(SceneRevision, SupportsWanted)>>,
    poll_timer: Rc<slint::Timer>,
    prescan: RefCell<Prescan>,
    /// One raft for all (#556), kept in step with its sets.
    plate_rafts: crate::plate_raft::PlateRafts,
    /// Supports computed while the person works on something else.
    ahead: RefCell<Ahead>,
    /// Where supports are recommended while placed by hand (#524).
    hints: RefCell<crate::support_hints::Hints>,
    /// When the scene last changed, live drag ticks included. Stamped by the
    /// scene subscription; that stamp is all the prescan costs an edit.
    last_change: Rc<Cell<Instant>>,
    prescan_timer: slint::Timer,
    _scene_events: SceneSubscription,
    /// The models standing on a raft of their own (#198), and that raft: the
    /// tab's raft changed while they alone were picked. Every other model
    /// stands on the tab's.
    own_rafts: RefCell<Vec<(NodeId, OwnRaft)>>,
    /// The tab shows the picked model's raft (#198): told when the pick
    /// changes.
    selection_events: RefCell<Option<SceneSubscription>>,
    /// The options the next run uses (the Supports tab), saved across
    /// sessions.
    options: RefCell<SupportOptions>,
    /// Built-in presets first, then the person's own.
    presets: RefCell<Vec<Preset>>,
    /// The shape panel (#564): where its support is seen from, the size it
    /// shows (an index into the preset's sizes), the part pointed at, and
    /// the built-in preset the options in force were made from.
    shape_view: Cell<crate::support_shape::View>,
    /// The support shows as a plan (2D), not in 3D (#567); kept between
    /// sessions.
    shape_flat: Cell<bool>,
    /// The texts of the three drawings (the support, its tip's lens, its
    /// foot's): kept, and changed row by row, so a number being dragged on a
    /// drawing stays the same element while its value changes.
    shape_texts: [Rc<slint::VecModel<crate::ShapeCallout>>; 3],
    shape_size: Cell<Option<usize>>,
    shape_hot: Cell<crate::support_shape::Part>,
    based_on: Cell<Option<usize>>,
    /// The built-in preset the options in use are, which keeps each change
    /// made to them (#564); `None` for options of the person's own preset.
    editing: Cell<Option<usize>>,
    /// The preset last picked from the list: of presets with the same
    /// numbers, the one the person clicked is the one marked (#564).
    picked: Cell<Option<usize>>,
    /// Each model takes the zone preset of its height (#723), and what the
    /// last Generate press gave each, for the line after the run.
    by_height: Cell<bool>,
    height_picks: RefCell<Option<String>>,
    /// The supports the Select tool picked (#567): each by its set's leaf and
    /// where its tip touches the model, which a redraw keeps.
    chosen: RefCell<Vec<(NodeId, [f64; 3])>>,
    chosen_events: RefCell<Option<SceneSubscription>>,
    /// Told the picked supports each time they are shown, none while the
    /// Select tool is off: the FORCES card draws their load (#245).
    chosen_listeners: RefCell<Vec<ChosenListener>>,
    /// The empty set a model with none would start, kept while the scene
    /// and the options stay as they were: Add previews on it before the
    /// first click (#564: a model with no supports showed no preview).
    unborn_hover: RefCell<Option<(SceneRevision, NodeId, SupportOptions, DrawnSet)>>,
    /// The sizes the hand tools place (XS to L): the person's own, apart
    /// from every preset of Generate.
    hand: RefCell<ZonePreset>,
    /// What the export or import being asked lists, each with its tick.
    share: RefCell<Vec<(crate::support_share::Entry, bool)>>,
    /// The last run's supports and every hand edit since, each with the scene
    /// revision it was drawn into. A click edits the entry matching the
    /// current scene, so Undo and Redo keep the editor in step; a scene that
    /// moved on otherwise is refused.
    /// The document's sessions ([`SceneDocument::support_sessions`]).
    sessions: Rc<RefCell<SupportSessions>>,
    /// The viewport, for the hover preview of the hand tools.
    ctrl: Rc<ViewportController>,
    /// Computes hand-tool previews off the UI thread, always the latest.
    hover_worker: HoverWorker,
    /// The latest hover asked for.
    hover_generation: Cell<u64>,
    /// The newest answer drawn (or the hover cleared at): an answer older
    /// than it is stale.
    hover_shown: Cell<u64>,
    hover_poll: slint::Timer,
    /// The last preview drawn, for a click on the same point to reuse.
    last_preview: RefCell<Option<HoverAnswer>>,
    /// The worker's last ghost, kept on screen while the next is computed.
    last_ghost: RefCell<Vec<Arc<OverlayMesh>>>,
    /// The last point hovered with a hand tool, and whether Shift was held:
    /// a new tip size previews there again.
    hover_at: Cell<Option<([f32; 3], bool)>>,
    /// The hand click being made off the UI thread, those that came while it
    /// was, and the timer that looks for it.
    pending_click: RefCell<Option<PendingClick>>,
    /// A drag of a Supports tab tool in flight (#3), and the timer that
    /// sends a box's owed rays between the moves.
    drag: RefCell<Option<DragState>>,
    /// The Select tool's move gizmo (#377): the elbow it stands on, by the
    /// support's id and the elbow's (none: on the support's shaft), the
    /// handle under the cursor, and the arrow being dragged.
    part_focus: RefCell<Option<(String, String)>>,
    part_hover: Cell<Option<PartHandle>>,
    part_arrow: Cell<Option<ArrowGrab>>,
    /// The brace tool's first end (#3), with the set it is on: the next
    /// click on a support lays the brace.
    brace_from: RefCell<Option<(DrawnSet, BraceEnd)>>,
    /// The line tool's first point (#178): the next click places the row.
    line_from: Cell<Option<[f32; 3]>>,
    /// The line tool's spacing between supports, mm; the Area tool's grid
    /// too (#177).
    line_spacing: Cell<f64>,
    /// The Area tool's brush (#177): it paints where supports go.
    area_brush: Rc<crate::surface_brush::SurfaceBrush>,
    box_poll: slint::Timer,
    /// This driver, for what a drag starts from the viewport's side.
    me: RefCell<std::rc::Weak<SupportRun>>,
    queued_clicks: RefCell<std::collections::VecDeque<QueuedClick>>,
    click_poll: slint::Timer,
    /// The raft warning (#131) is shown from a Generate press until a new
    /// scene is installed or closed; the scene subscription lowers it.
    raft_armed: Rc<Cell<bool>>,
    /// Set for one Generate press by the warning's "Generate anyway": that
    /// press runs although the raft would take the models' first layers.
    raft_ignored: Cell<bool>,
    /// The layer height of the last Generate press, which counts the raft's
    /// layers.
    raft_layer_height_mm: Cell<f64>,
    /// When the raft warning was last brought up to date with the scene.
    raft_checked_at: Cell<Instant>,
    // What Generate goes on to once its supports are in: the islands it
    // left, found and supported (#606).
    after_generate: RefCell<Option<Box<dyn Fn() -> bool>>>,
    // Generate's step waiting for the one raft for all (#556) to be drawn
    // under its new supports, with the run's summary for the status bar
    // if the step cannot start (#606).
    islands_after_raft: RefCell<Option<String>>,
    /// Generate's supports are looked at against the plate's edge once its
    /// islands step ends (#628).
    edges_after_islands: Cell<bool>,
    /// The raft top of the supports this driver pasted last, `Some(None)`
    /// for supports without a raft.
    generated_raft_top: Cell<Option<Option<f64>>>,
    /// Clears the status bar's note of the last run.
    status_timer: slint::Timer,
    /// A row picked on the card of supports out of date (#408): the set the
    /// next run makes again. Taken by the run it starts; a Generate press
    /// drops it.
    redo: RefCell<Option<Redo>>,
    /// The models whose supports were made on another grid than the tab's
    /// (#576), the next run makes again from Regenerate. Taken by the run
    /// it starts; a Generate press drops it.
    regrid: RefCell<Option<Vec<NodeId>>>,
    /// The models with supports made on another grid than the tab's (#576),
    /// as last counted, and the scene and tab grid they were counted on.
    grid_stale: RefCell<GridCount>,
    /// The states of sets out of date already made again on their own, or
    /// found beyond keeping (#526): each is tried once.
    healing_tried: RefCell<std::collections::HashSet<String>>,
}

/// The supports of one model made again from the card of supports out of
/// date (#408), as a Generate press makes them for it.
struct Redo {
    model: NodeId,
    /// "Recalculate, keeping my edits": the session whose hand edits go
    /// back on the new set. `None` for "Regenerate".
    keep: Option<Arc<SupportSession>>,
    /// The options the set out of date was made with: it is made again with
    /// its own preset, not the one the tab has now (#490). `None` for a set
    /// that does not say, which takes the tab's.
    options: Option<SupportOptions>,
}

/// Why no set in force takes a hand tool at a point.
enum NoSet {
    /// A notice: its "Don't show again" key, heading and text.
    Notice(Option<MessageKey>, slint::SharedString, slint::SharedString),
    /// The supports there are out of date, by their leaf: the card offers
    /// to make them again (#408).
    OutOfDate(NodeId),
    /// Why the edit failed, for the last-run line.
    Failed(slint::SharedString),
}

/// How long the status bar keeps the note of a run that replaced supports.
const STATUS_NOTICE_DURATION: Duration = Duration::from_secs(6);

/// How often the UI looks for a finished preview.
const HOVER_POLL: Duration = Duration::from_millis(16);

/// A point of a hand edit, with its place among the edit's points.
type IndexedPoint = (usize, [f32; 3]);

/// What an edit knows of one of its points besides where it goes: the tips
/// of an island (#437). Placing and checking a tip are one step (#433).
#[derive(Clone, Debug, Default)]
pub(crate) struct PointPlan {
    /// The other points that would do, best first.
    pub(crate) others: Vec<[f32; 3]>,
    /// What a tip here must hold to count as placed: an island's first tip
    /// must hold its island. `None`: any tip the engine takes.
    pub(crate) holds: Option<TipHold>,
    /// The point, by its place among the edit's, whose tip this one goes
    /// with: when that one found no place, this one is not placed either.
    pub(crate) after: Option<usize>,
    /// The size its tip takes, by id; `None` for the preset's.
    pub(crate) size: Option<String>,
}

/// What an edit knows of each of its points, by their place among them.
/// Empty for an edit that knows nothing more of them.
pub(crate) type PointPlans = Arc<Vec<PointPlan>>;

/// How a hand edit of the supports ended.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HandEdit {
    /// The edit is in the scene, as one undoable step.
    Applied,
    /// Nothing changed, for this reason.
    Failed(slint::SharedString),
    /// Nothing changed, and a notice already told the person why.
    Notified,
}

/// The points of a hand edit the engine refused, by index, with why.
pub(crate) type Refusals = Vec<(usize, slint::SharedString)>;

/// How far outside a set's box (its model's for Add, its supports' for
/// Remove) a hand tool's point may land and still be asked to that set: past
/// the engine's own reaches (2 mm to add, 3 mm to remove).
const SET_REACH_MM: f64 = 3.5;

/// How far `p` is from `aabb`: 0 inside it.
fn box_distance(aabb: &sc_mesh_core::Aabb, p: [f64; 3]) -> f64 {
    let (lo, hi) = (aabb.min(), aabb.max());
    let (lo, hi) = (lo.as_array(), hi.as_array());
    (0..3)
        .map(|k| {
            let (a, b) = (f64::from(lo[k]), f64::from(hi[k]));
            (a - p[k]).max(p[k] - b).max(0.0)
        })
        .map(|d| d * d)
        .sum::<f64>()
        .sqrt()
}

/// What a hand edit does at each of its points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandTool {
    /// A click of the Add tool: a support at that very point.
    Add,
    /// A support at a check's point, or right next to it.
    AddNear,
    /// A click of the Remove tool.
    Remove,
}

impl HandTool {
    /// The Supports tab's tool code of a click, whose preview it can take.
    fn click(self) -> Option<i32> {
        match self {
            HandTool::Add => Some(1),
            HandTool::Remove => Some(2),
            HandTool::AddNear => None,
        }
    }
}

/// The zone sizes (#360) keys 1 to 4 pick for a support added by hand,
/// smallest first: the Add tool's tip size 1 to 4 (0 is the preset's).
const HAND_TIP_SIZES: [&str; 4] = ["extrasmall", "small", "medium", "large"];

/// The size id of the Add tool's tip size `code` (the `support-tip-size`
/// property), `None` for the preset's own.
fn hand_tip_size(code: i32) -> Option<String> {
    let i = usize::try_from(code).ok()?.checked_sub(1)?;
    HAND_TIP_SIZES.get(i).map(|id| (*id).to_owned())
}

/// Why an addition does not hang from a support, or cannot be placed, in
/// the person's words.
fn note_text(note: AddNote) -> slint::SharedString {
    match note {
        AddNote::TooFar => ctr!("SupportDialog", "no support close enough to hang from"),
        AddNote::NoneBelow => ctr!("SupportDialog", "no support below the tip to hang from"),
        AddNote::TooSteep => ctr!(
            "SupportDialog",
            "hanging from the supports nearby would lean too far"
        ),
        AddNote::HitsModel => ctr!("SupportDialog", "it hits the model"),
        AddNote::CrossesSupport => ctr!("SupportDialog", "it would cross another support"),
        AddNote::HostsFull => ctr!("SupportDialog", "the supports nearby carry all they can"),
        AddNote::SteepSurface => ctr!(
            "SupportDialog",
            "the surface there is too steep for a tip"
        ),
        AddNote::CarriesOthers => ctr!("SupportDialog", "other supports hang from it"),
        AddNote::FacesUp => ctr!(
            "SupportDialog",
            "the surface there faces up; nothing below can reach it"
        ),
        AddNote::Leans => ctr!("SupportDialog", "the shaft would lean too far to print"),
        AddNote::OutOfOrder => ctr!(
            "SupportDialog",
            "an elbow stays between the parts below and above it"
        ),
        AddNote::TipOverShaft => ctr!("SupportDialog", "the tip would be wider than its shaft"),
    }
}

/// The Add tool's line for a hover preview: what a click there does when it
/// does not simply hang the support from another, and whether that warns.
/// Empty for a support that hangs as asked.
fn preview_line(preview: &EditPreview, attach: bool) -> (slint::SharedString, bool) {
    let note = preview.note.map(note_text);
    match (preview.valid, preview.attached, note) {
        (true, true, _) => Default::default(),
        (true, false, Some(why)) => (ctr!("SupportDialog", "Its own trunk: {}.", why), false),
        (true, false, None) => Default::default(),
        (false, _, Some(why)) if attach => {
            (ctr!("SupportDialog", "Cannot hang it: {}.", why), true)
        }
        (false, _, Some(why)) => (ctr!("SupportDialog", "Cannot place it: {}.", why), true),
        (false, _, None) => (
            ctr!(
                "SupportDialog",
                "Cannot place it: no support can reach that point."
            ),
            true,
        ),
    }
}

/// Move tip's line for a preview: why the support stands on its own there,
/// or why it cannot go there, and whether that warns.
fn move_line(preview: &EditPreview) -> (slint::SharedString, bool) {
    match (preview.valid, preview.note.map(note_text)) {
        (true, _) => preview_line(preview, false),
        (false, Some(why)) => (
            ctr!("SupportDialog", "Cannot move it there: {}.", why),
            true,
        ),
        (false, None) => (
            ctr!(
                "SupportDialog",
                "Cannot move it there: no support can reach that point."
            ),
            true,
        ),
    }
}

/// Why a brace laid by hand cannot go there, in the person's words (#3).
fn brace_text(why: HandBraceRefusal) -> slint::SharedString {
    match why {
        HandBraceRefusal::SameSupport => {
            ctr!("SupportDialog", "both ends are on the same support")
        }
        HandBraceRefusal::OtherModel => {
            ctr!("SupportDialog", "the two supports hold different models")
        }
        HandBraceRefusal::NotAShaft => {
            ctr!("SupportDialog", "a brace joins trunks and branches only")
        }
        HandBraceRefusal::OffShaft => ctr!(
            "SupportDialog",
            "an end is off its support: too near the tip, or too low"
        ),
        HandBraceRefusal::TooClose => {
            ctr!("SupportDialog", "the supports are too close to brace")
        }
        HandBraceRefusal::TooLong => ctr!(
            "SupportDialog",
            "it would be longer than the preset lets a brace be"
        ),
        HandBraceRefusal::TooFlat => ctr!(
            "SupportDialog",
            "it would be flatter than 45°: at 45° it ends off the other support"
        ),
        HandBraceRefusal::HitsModel => ctr!("SupportDialog", "it would cross the model"),
    }
}

/// The brace tool's line for a preview of the second support (#3): what a
/// click there lays, or why it cannot, and whether that warns.
fn brace_line(preview: &EditPreview) -> (slint::SharedString, bool) {
    match (preview.valid, preview.brace) {
        (true, _) => (
            ctr!(
                "SupportDialog",
                "Click to lay the brace. Esc drops the first end."
            ),
            false,
        ),
        (false, Some(why)) => (
            ctr!("SupportDialog", "Cannot brace them: {}.", brace_text(why)),
            true,
        ),
        (false, None) => (
            ctr!("SupportDialog", "Cannot brace them: no brace fits there."),
            true,
        ),
    }
}

/// The brace tool's line once its first end is picked (#3).
fn brace_first_line() -> (slint::SharedString, bool) {
    (
        ctr!(
            "SupportDialog",
            "Now click the support to brace it to. Esc drops the first end."
        ),
        false,
    )
}

/// Why the engine refused an edit, in the person's words.
fn refusal(err: &EditError) -> slint::SharedString {
    match err {
        EditError::CannotAttach(note) => note_text(*note),
        EditError::CannotBrace(why) => brace_text(*why),
        EditError::NothingThere => ctr!(
            "SupportDialog",
            "there is no model or support within reach of that point"
        ),
        EditError::NoRoute(_) | EditError::NotHeld => ctr!(
            "SupportDialog",
            "there is no route to the plate or to another support"
        ),
        EditError::Tessellation(why) => {
            ctr!(
                "SupportDialog",
                "the supports could not be drawn: {}",
                why
            )
        }
    }
}

/// The tip `id` draws for `session`, in mm: its preset's size of that id,
/// else that of the shipped sizes (#360).
fn tip_mm(session: Option<&SupportSession>, id: &str) -> Option<f64> {
    session
        .and_then(|s| s.options())
        .and_then(|o| o.zones.as_ref())
        .and_then(|z| z.rules.sizes.iter().find(|s| s.id == id))
        .map(|s| s.tip_diameter_mm)
        .or_else(|| {
            sc_geom::supports::zone_presets::support_sizes()
                .into_iter()
                .find(|s| s.id == id)
                .map(|s| s.tip_diameter_mm)
        })
}

/// Where a set's share of a hand edit starts.
enum EditStart {
    /// The set's session as drawn: the edit is made on a copy.
    Base(Arc<SupportSession>),
    /// The session the preview of the very click left, the edit in it.
    Previewed(Box<SupportSession>),
}

/// What a set's share of a hand edit does.
#[derive(Clone, Debug)]
enum EditOp {
    /// The tool's edit at each point.
    Points(Vec<IndexedPoint>),
    /// Removes these supports, by their ids (a box or a brush, #3).
    RemoveTips(Vec<String>),
    /// Moves this support's tip to this point (#3).
    MoveTip(String, [f32; 3]),
    /// Draws these supports again with these numbers, each in its contact
    /// (#567).
    Reshape(Vec<(String, sc_geom::supports::SupportSize)>),
    /// Lays a brace from this end to the support at this point (#3).
    Brace(BraceEnd, [f32; 3]),
    /// Routes these supports again from their tips (#377): on routes of
    /// their own, or hung from the nearest support.
    Reroute(Vec<String>, bool),
    /// Moves a part of this support (#377): its trunk across the plate, or
    /// an elbow.
    MovePart(String, PartMove),
    /// Joins the supports whose tips are at these points onto this one's
    /// trunk (#377).
    Join(Vec<[f64; 3]>, String),
    /// Gives these supports' parts these widths, their routes as they are
    /// (#377).
    Resize(Vec<String>, PartSizes),
    /// Places these supports of an imported set made editable (#261), each
    /// with its numbers and on its route where it may take it.
    Imported(Vec<ImportedPlan>),
    /// Gives each of these supports its widths (#247), its route as it is.
    Reinforce(Vec<(String, PartSizes)>),
}

/// How much more a weak spot is made to hold than the frame asked (#247).
const REINFORCE_MARGIN: f64 = 1.2;

/// The most a reinforcement widens a part at once (#247): a support far
/// past what it holds takes another press.
const REINFORCE_MOST: f64 = 1.6;

/// The widest a tip is against its shaft (#247): the session refuses a tip
/// as wide as its shaft.
const TIP_OF_SHAFT: f64 = 0.9;

/// The widths that make support `id` of `session` hold `utilization` times
/// what it does now, at its `part` worst off (#247): the stress at a part
/// falls with its width cubed in bending and squared when pulled, so its
/// width grows by the cube root, with a margin, at most
/// [`REINFORCE_MOST`]. A tip that would reach its shaft widens the shaft
/// too. `None` for a support with no part to widen (a brace, a twig).
pub(crate) fn reinforced_sizes(
    session: &SupportSession,
    id: &str,
    utilization: f64,
    part: bread_structure::Part,
) -> Option<PartSizes> {
    let parts = session.parts_of(id)?;
    let grow = (utilization * REINFORCE_MARGIN)
        .cbrt()
        .clamp(1.05, REINFORCE_MOST);
    match part {
        bread_structure::Part::Tip => {
            let mut tip = parts.tip_mm * grow;
            let mut trunk_mm = None;
            if let Some(shaft) = parts.trunk_mm
                && tip > shaft * TIP_OF_SHAFT
            {
                let wider = (tip / TIP_OF_SHAFT).min(shaft * REINFORCE_MOST);
                if wider > shaft {
                    trunk_mm = Some(wider);
                }
                tip = tip.min(wider * TIP_OF_SHAFT);
            }
            (tip > parts.tip_mm * 1.001 || trunk_mm.is_some()).then_some(PartSizes {
                tip_mm: Some(tip),
                trunk_mm,
                foot_mm: None,
            })
        }
        bread_structure::Part::Shaft => {
            let shaft = parts.trunk_mm?;
            Some(PartSizes {
                trunk_mm: Some(shaft * grow),
                ..PartSizes::default()
            })
        }
    }
}

/// The numbers supports made from imported ones are drawn with (#261): the
/// shipped hand size nearest their tips' width, with the middle values the
/// imported supports have of their tip, tip length, shaft and foot. A file
/// draws its supports in a few fixed sizes; the middle value is that size,
/// clear of the few pieces read a little off. `None` with no route.
fn imported_shape<'a>(
    routes: impl Iterator<Item = &'a ImportedRoute> + Clone,
) -> Option<sc_geom::supports::SupportSize> {
    let middle = |values: Vec<f64>| -> Option<f64> {
        let mut values: Vec<f64> = values.into_iter().filter(|v| v.is_finite()).collect();
        values.sort_by(f64::total_cmp);
        values.get(values.len() / 2).copied()
    };
    // Rounded to what a print shows: a twentieth of a millimetre.
    let round = |v: f64| (v * 20.0).round() / 20.0;
    let tip = round(middle(routes.clone().map(|r| r.tip.diameter_mm).collect())?);
    let mut size = crate::support_shape_edit::hand_factory()
        .rules
        .sizes
        .into_iter()
        .min_by(|a, b| {
            (a.tip_diameter_mm - tip)
                .abs()
                .total_cmp(&(b.tip_diameter_mm - tip).abs())
        })?;
    size.id = "imported".to_owned();
    size.tip_diameter_mm = tip;
    if let Some(t) = size.base_tip.as_mut() {
        t.diameter_mm = tip;
    }
    if let Some(length) = middle(routes.clone().map(|r| r.tip_length_mm).collect()) {
        size.tip_length_mm = round(length);
    }
    if let Some(shaft) = middle(routes.clone().filter_map(|r| r.shaft_mm).collect()) {
        size.trunk_diameter_mm = round(shaft);
    }
    if let Some(foot) = middle(routes.filter_map(|r| r.foot.map(|(_, w)| w)).collect()) {
        size.base_diameter_mm = round(foot);
    }
    Some(size.clamped())
}

/// A part of a support moved by a drag of the Select tool (#377).
#[derive(Clone, Debug, PartialEq)]
enum PartMove {
    /// Its trunk, by this much across the plate (mm).
    Trunk([f64; 2]),
    /// This elbow, to this point (world mm).
    Elbow(String, [f64; 3]),
    /// Its shaft by its body, by this much (world mm): a branch's elbows
    /// all together, the knot it hangs from staying.
    Shaft([f64; 3]),
    /// The knot it hangs from, slid along its host's shaft to this height
    /// (world mm): the way it leans changes, its tip staying.
    Knot(f64),
}

impl PartMove {
    /// Makes the move on `session`.
    fn make(&self, session: &mut SupportSession, id: &str) -> Result<(), EditError> {
        match self {
            PartMove::Trunk(delta) => session.move_trunk(id, *delta),
            PartMove::Elbow(elbow, to) => session.move_elbow(id, elbow, *to),
            PartMove::Shaft(delta) => session.move_shaft(id, *delta),
            PartMove::Knot(z) => session.slide_knot(id, *z),
        }
    }

    /// What the move would do on `session`.
    fn preview(
        &self,
        session: &SupportSession,
        id: &str,
    ) -> Result<(EditPreview, Option<SupportSession>), EditError> {
        match self {
            PartMove::Trunk(delta) => session.preview_move_trunk(id, *delta),
            PartMove::Elbow(elbow, to) => session.preview_move_elbow(id, elbow, *to),
            PartMove::Shaft(delta) => session.preview_move_shaft(id, *delta),
            PartMove::Knot(z) => session.preview_slide_knot(id, *z),
        }
    }
}

/// One set's share of a hand edit: where it starts and what it does.
struct EditJob {
    start: EditStart,
    op: EditOp,
    /// The set's leaf.
    leaf: NodeId,
    /// What is known of the points of `op`.
    plans: PointPlans,
}

/// A hand edit ready for the engine: the sets it edits and, in the same
/// order, each one's share.
struct Prepared {
    sets: Vec<DrawnSet>,
    jobs: Vec<EditJob>,
}

/// How a set's share of a hand edit came out.
enum JobOutcome {
    /// The engine panicked: an edit that failed, not lost work.
    Panicked,
    /// The supports could not be drawn.
    Failed(EditError),
    /// None of the set's points took.
    Untouched,
    /// The edit took the set's last support.
    Emptied,
    /// The edited session and its mesh.
    Done(Box<SupportSession>, Arc<sc_mesh_core::IndexedMesh>),
}

/// A hand edit as the engine left it, for the UI thread to put in the scene.
struct Computed {
    outcomes: Vec<JobOutcome>,
    /// The points the engine refused, by index, with why.
    refused: Vec<(usize, EditError)>,
    /// A preview's edit was taken.
    reused: bool,
}

/// What a hand edit made off the UI thread tells and the UI reads: how
/// many of its points the engine has tried, and whether to stop (#390).
#[derive(Debug, Default)]
pub(crate) struct EditWatch {
    /// The points the edit places, all sets together.
    total: AtomicUsize,
    tried: AtomicUsize,
    cancel: AtomicBool,
}

impl EditWatch {
    /// How many points the engine has tried, of how many the edit places.
    pub(crate) fn progress(&self) -> (usize, usize) {
        (
            self.tried.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }

    /// Asks the engine to stop at its next point: what it made is dropped.
    fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// How one set's share of a hand edit came out, with the points it refused
/// and whether it took a preview's edit.
type JobDone = (JobOutcome, Vec<(usize, EditError)>, bool);

/// The engine's part of a hand edit, the slow one (placing, then drawing
/// the whole set again): it runs on any thread. The sets are independent
/// (each its own session), so each is made on a thread of its own, as many
/// at once as the computer has cores; the outcomes and refusals come back
/// in the jobs' order, the same as made one after the other (#390).
fn compute_edit(
    jobs: Vec<EditJob>,
    tool: HandTool,
    how: &AddOptions,
    watch: &EditWatch,
) -> Computed {
    let points = |job: &EditJob| match &job.op {
        EditOp::Points(points) => points.len(),
        EditOp::RemoveTips(_)
        | EditOp::MoveTip(..)
        | EditOp::Brace(..)
        | EditOp::Reshape(_)
        | EditOp::Reroute(..)
        | EditOp::MovePart(..)
        | EditOp::Join(..)
        | EditOp::Resize(..)
        | EditOp::Imported(_)
        | EditOp::Reinforce(_) => 0,
    };
    watch
        .total
        .store(jobs.iter().map(points).sum(), Ordering::Relaxed);
    let done: Vec<JobDone> = if jobs.len() < 2 {
        jobs.into_iter()
            .map(|job| compute_job(job, tool, how, watch))
            .collect()
    } else {
        compute_jobs_at_once(jobs, tool, how, watch)
    };
    let mut computed = Computed {
        outcomes: Vec::with_capacity(done.len()),
        refused: Vec::new(),
        reused: false,
    };
    for (outcome, refused, reused) in done {
        computed.outcomes.push(outcome);
        computed.refused.extend(refused);
        computed.reused |= reused;
    }
    computed
}

/// [`compute_edit`]'s sets on worker threads, each job in its slot: a
/// worker takes the next job, and a job no worker took (no thread would
/// start) is made here.
fn compute_jobs_at_once(
    jobs: Vec<EditJob>,
    tool: HandTool,
    how: &AddOptions,
    watch: &EditWatch,
) -> Vec<JobDone> {
    use std::sync::Mutex;
    let count = jobs.len();
    let slots: Vec<Mutex<Option<EditJob>>> =
        jobs.into_iter().map(|job| Mutex::new(Some(job))).collect();
    let results: Vec<Mutex<Option<JobDone>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let take = |k: usize| slots[k].lock().ok().and_then(|mut slot| slot.take());
    let work = || {
        loop {
            let k = next.fetch_add(1, Ordering::Relaxed);
            if k >= count {
                break;
            }
            if let Some(job) = take(k) {
                let done = compute_job(job, tool, how, watch);
                if let Ok(mut result) = results[k].lock() {
                    *result = Some(done);
                }
            }
        }
    };
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(count);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            // The router recurses: the same room as the hover
            // worker. A thread that does not start leaves its jobs to the
            // others, or to this one below.
            let _ = std::thread::Builder::new()
                .name("support-edit-set".into())
                .stack_size(16 << 20)
                .spawn_scoped(scope, work);
        }
    });
    results
        .into_iter()
        .enumerate()
        .map(|(k, result)| {
            let done = result.into_inner().ok().flatten();
            done.unwrap_or_else(|| match take(k) {
                Some(job) => compute_job(job, tool, how, watch),
                // Taken by a worker that did not finish it.
                None => (JobOutcome::Panicked, Vec::new(), false),
            })
        })
        .collect()
}

/// One set's share of [`compute_edit`]. Stopped through `watch`, it ends
/// at its next point, and the caller drops what it made.
fn compute_job(
    EditJob {
        start,
        op,
        leaf,
        plans,
    }: EditJob,
    tool: HandTool,
    how: &AddOptions,
    watch: &EditWatch,
) -> JobDone {
    let mut refused = Vec::new();
    let (mut edited, from_preview) = match start {
        EditStart::Base(base) => (SupportSession::clone(&base), false),
        EditStart::Previewed(edited) => (*edited, true),
    };
    // The support engine on hand-picked input: a panic in it is
    // an edit that failed, not a reason to lose the person's work.
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut took = from_preview;
        let mut one = |edit: Result<(), EditError>| match edit {
            Ok(()) => true,
            Err(err) => {
                crate::breadcrumbs::note(&format!("supports: gesture refused: {err}"));
                refused.push((0, err));
                false
            }
        };
        match (&op, from_preview) {
            (_, true) | (EditOp::Points(_), false) => {}
            (EditOp::RemoveTips(ids), false) => {
                took = one(edited.remove_tips(ids).map(|_| ()));
            }
            (EditOp::MoveTip(tip, to), false) => {
                took = one(edited.move_tip(tip, to.map(f64::from), how));
            }
            (EditOp::Brace(from, to), false) => {
                took = one(edited.add_brace(from, to.map(f64::from)));
            }
            (EditOp::Reshape(shapes), false) => {
                for (id, shape) in shapes {
                    if one(edited.reshape(id, shape)) {
                        took = true;
                    }
                }
            }
            (EditOp::Reroute(ids, own), false) => {
                for id in ids {
                    if one(edited.reroute(id, *own)) {
                        took = true;
                    }
                }
            }
            (EditOp::MovePart(id, part), false) => {
                took = one(part.make(&mut edited, id));
            }
            (EditOp::Join(tips, host), false) => {
                for tip in tips {
                    let Some(id) = edited.tip_near(*tip, 1e-6) else {
                        continue;
                    };
                    if one(edited.join_onto(&id, host)) {
                        took = true;
                    }
                }
            }
            (EditOp::Resize(ids, sizes), false) => {
                for id in ids {
                    if one(edited.resize_parts(id, *sizes)) {
                        took = true;
                    }
                }
            }
            (EditOp::Reinforce(plan), false) => {
                for (id, sizes) in plan {
                    if one(edited.resize_parts(id, *sizes)) {
                        took = true;
                    }
                }
            }
            (EditOp::Imported(plans), false) => {
                let mut routed = 0;
                for plan in plans {
                    match edited.add_imported(plan) {
                        Ok(kept) => {
                            took = true;
                            routed += usize::from(kept);
                        }
                        Err(err) => {
                            one(Err(err));
                        }
                    }
                }
                crate::breadcrumbs::note(&format!(
                    "supports: {routed} of {} imported supports kept their route",
                    plans.len()
                ));
            }
        }
        if let (EditOp::Points(points), false) = (&op, from_preview) {
            for (i, point) in points {
                if watch.stopped() {
                    return Ok(None);
                }
                let p = point.map(f64::from);
                let edit = match tool {
                    HandTool::Add => edited.add_at(p, how),
                    HandTool::AddNear => {
                        let plan = plans.get(*i);
                        let lost = plan.and_then(|plan| plan.after).and_then(|first| {
                            refused
                                .iter()
                                .find(|(place, _)| *place == first)
                                .map(|(_, why)| why.clone())
                        });
                        match lost {
                            Some(why) => Err(why),
                            None => {
                                let others: Vec<[f64; 3]> = plan
                                    .into_iter()
                                    .flat_map(|plan| &plan.others)
                                    .map(|other| other.map(f64::from))
                                    .collect();
                                let hold = plan.and_then(|plan| plan.holds.as_ref());
                                let size = plan.and_then(|plan| plan.size.as_deref());
                                edited
                                    .add_tip_among(p, &others, size, &|contacts| {
                                        hold.is_none_or(|hold| hold.by(leaf, contacts))
                                    })
                                    .map(|_| ())
                            }
                        }
                    }
                    HandTool::Remove => edited.remove_near(p).map(|_| ()),
                };
                watch.tried.fetch_add(1, Ordering::Relaxed);
                match edit {
                    Ok(()) => took = true,
                    Err(err) => {
                        crate::breadcrumbs::note(&format!("supports: point {i} refused: {err}"));
                        refused.push((*i, err));
                    }
                }
            }
        }
        if !took || watch.stopped() {
            return Ok(None);
        }
        edited.mesh().map(Some)
    }));
    let outcome = match done {
        Err(_) => JobOutcome::Panicked,
        Ok(Err(err)) => JobOutcome::Failed(err),
        Ok(Ok(None)) => JobOutcome::Untouched,
        Ok(Ok(Some(None))) => JobOutcome::Emptied,
        Ok(Ok(Some(Some(mesh)))) => JobOutcome::Done(Box::new(edited), Arc::new(mesh)),
    };
    (outcome, refused, from_preview)
}

/// A hand click being made off the UI thread (#3).
struct PendingClick {
    /// The scene it was made on: one that moved on before the edit is ready
    /// (another edit, a run) drops it.
    revision: SceneRevision,
    /// The sets it edits, in the order of the outcomes.
    sets: Vec<DrawnSet>,
    /// Points no set took, already told.
    refused: Refusals,
    done: std::sync::mpsc::Receiver<Computed>,
    /// How far the engine is, and its stop.
    watch: Arc<EditWatch>,
    /// For an edit another panel asked for (Fix All, #390): where it lands.
    landing: Option<Landing>,
    /// The undo entry it joins, at this revision, instead of one of its
    /// own: a Fix All run's later passes (#163).
    join: Option<SceneRevision>,
}

/// Where an edit made off the UI thread for another panel lands (#390).
type Landing = Box<dyn FnOnce(PointsEdit)>;

/// How an edit made off the UI thread for another panel ended.
#[derive(Debug)]
pub(crate) enum PointsEdit {
    /// It went as [`SupportRun::add_supports_at`] tells: in the scene as
    /// one undoable step, or nothing changed.
    Done(HandEdit, Refusals),
    /// It was dropped before it reached the scene, which it left as it was.
    Dropped(EditDropped),
}

/// Why an edit made off the UI thread was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditDropped {
    /// Cancel was pressed.
    Cancelled,
    /// Undo was pressed before it was made.
    Undone,
    /// The scene changed while it was made: another edit, a run, a new
    /// scene.
    SceneChanged,
}

/// An edit that came while another was being made: made once that one is
/// in.
enum QueuedClick {
    /// A click of a tool.
    Click {
        tool: HandTool,
        point: [f32; 3],
        how: AddOptions,
    },
    /// A drag's edit of the set its leaf shows.
    Gesture { leaf: NodeId, op: EditOp },
    /// A row of the line tool (#178): every point in one edit.
    Points {
        tool: HandTool,
        points: Vec<[f32; 3]>,
        how: AddOptions,
    },
}

/// A drag of a Supports tab tool in flight (#3).
enum DragState {
    /// Move tip: the support grabbed, and where its tip was last asked to.
    Move {
        drawn: DrawnSet,
        tip: String,
        to: Option<[f32; 3]>,
    },
    /// The Select tool's drag of a picked support's part (#377): the
    /// support, the point grabbed, the elbow grabbed (none: the trunk), and
    /// the move last asked for.
    MovePart {
        drawn: DrawnSet,
        id: String,
        grab: [f64; 3],
        elbow: Option<String>,
        /// It moves on the plane facing the eye: an elbow, or a branch by
        /// its body; a trunk moves across the plate.
        free: bool,
        to: Option<PartMove>,
    },
    /// Remove's box or brush: the set it removes from, its supports as
    /// points along each (tip and shaft), the ones picked so far and, for
    /// the brush, where the cursor last was.
    Remove {
        drawn: DrawnSet,
        tips: Vec<(String, Vec<[f32; 3]>)>,
        /// The box's far corner, and whether rays are still owed for it.
        corner: [f32; 2],
        owed: bool,
        /// Which support a ray hit, the lines on screen, and what each ray
        /// found: support and cell of the screen to whether it was seen
        /// there.
        locate: SupportLocator,
        seen_from: SeenFrom,
        looked: HashMap<(usize, (i32, i32)), bool>,
        picked: Vec<String>,
        press: [f32; 2],
        brush: Option<[f32; 2]>,
    },
    /// Remove's box or brush over the supports a pre-supported file came
    /// with (#260): their bodies, each with points over it in the world,
    /// looked for on screen as the supports of a set are.
    RemoveImported {
        bodies: Vec<ImportedBody>,
        points: Vec<(String, Vec<[f32; 3]>)>,
        corner: [f32; 2],
        owed: bool,
        seen_from: SeenFrom,
        looked: HashMap<(usize, (i32, i32)), bool>,
        picked: Vec<String>,
        press: [f32; 2],
        brush: Option<[f32; 2]>,
    },
}

/// What a box's step picked, to be drawn red.
enum Picks {
    Set(Box<DrawnSet>, Vec<String>),
    Imported(Vec<ImportedBody>),
}

/// The bodies of `bodies` whose ids (their place in it) `picked` names.
fn picked_bodies(bodies: &[ImportedBody], picked: &[String]) -> Vec<ImportedBody> {
    picked
        .iter()
        .filter_map(|id| bodies.get(id.parse::<usize>().ok()?).cloned())
        .collect()
}

/// The side of a cell of the screen along a support's line, in logical
/// pixels.
const SIGHT_CELL_PX: f32 = 5.0;

/// A box or a brush sends a ray through a support every so many cells of
/// its line on screen.
const SIGHT_STRIDE: usize = 3;

/// The rays a box sends a step: about 8 ms of them on a big model, the
/// cores sharing them. Those owed go on between the moves.
const BOX_RAYS: usize = 300;

/// A cell of the screen a support's line crosses, with a pixel of the line
/// in it.
type LineCell = ((i32, i32), [f32; 2]);

/// The supports' lines on screen as a drag sees them: each point projected
/// once, again only when the view changes (the wheel zooms a box drag).
struct SeenFrom {
    /// Where two fixed points land: a change says the view moved.
    probe: [Option<[f32; 2]>; 2],
    screen: Vec<Vec<Option<[f32; 2]>>>,
}

impl SeenFrom {
    const PROBE: [[f32; 3]; 2] = [[0.0, 0.0, 0.0], [100.0, 100.0, 100.0]];

    fn new(supports: &[(String, Vec<[f32; 3]>)], view: Project<'_>) -> Self {
        SeenFrom {
            probe: Self::PROBE.map(|p| view.project(p)),
            screen: supports
                .iter()
                .map(|(_, along)| along.iter().map(|p| view.project(*p)).collect())
                .collect(),
        }
    }

    /// The lines as `view` sees them: as they were, unless it moved.
    fn refresh(&mut self, supports: &[(String, Vec<[f32; 3]>)], view: Project<'_>) -> bool {
        if Self::PROBE.map(|p| view.project(p)) == self.probe {
            return false;
        }
        *self = Self::new(supports, view);
        true
    }
}

/// The side of a cell of [`SupportLocator`]'s grid, mm.
const LOCATE_CELL_MM: f32 = 2.0;

/// Which support of a set a ray hit: its leaf, and the support whose line
/// runs nearest the hit point, through a grid of the lines' points.
struct SupportLocator {
    leaf: NodeId,
    cells: HashMap<[i32; 3], Vec<(usize, [f32; 3])>>,
}

impl SupportLocator {
    fn new(leaf: NodeId, supports: &[(String, Vec<[f32; 3]>)]) -> Self {
        let mut cells: HashMap<[i32; 3], Vec<(usize, [f32; 3])>> = HashMap::new();
        for (k, (_, along)) in supports.iter().enumerate() {
            for p in along {
                cells
                    .entry(p.map(|c| (c / LOCATE_CELL_MM).floor() as i32))
                    .or_default()
                    .push((k, *p));
            }
        }
        SupportLocator { leaf, cells }
    }

    /// The support of the set a hit is on: `None` for a hit on any other
    /// leaf (a model in front, another set) or on no support's line.
    fn support_at(&self, hit: ScreenHit) -> Option<usize> {
        if hit.leaf != self.leaf {
            return None;
        }
        let c = hit.point.map(|v| (v / LOCATE_CELL_MM).floor() as i32);
        let mut best: Option<(f32, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let key = [c[0] + dx, c[1] + dy, c[2] + dz];
                    for (k, p) in self.cells.get(&key).into_iter().flatten() {
                        let d = (0..3).map(|i| (p[i] - hit.point[i]).powi(2)).sum::<f32>();
                        if best.is_none_or(|(b, _)| d < b) {
                            best = Some((d, *k));
                        }
                    }
                }
            }
        }
        best.map(|(_, k)| k)
    }
}

/// How far apart a box or a brush looks along a support, in mm.
const PICK_STEP_MM: f64 = 0.5;

/// Each support of `paths` as points along it, [`PICK_STEP_MM`] apart:
/// what a box or a brush finds it by on screen.
fn along_supports(paths: Vec<(String, Vec<[f64; 3]>)>) -> Vec<(String, Vec<[f32; 3]>)> {
    paths
        .into_iter()
        .map(|(id, path)| {
            let mut points = Vec::new();
            for w in path.windows(2) {
                let (a, b) = (w[0], w[1]);
                let len = (0..3).map(|k| (b[k] - a[k]).powi(2)).sum::<f64>().sqrt();
                let steps = (len / PICK_STEP_MM).ceil().max(1.0) as usize;
                for k in 0..steps {
                    let t = k as f64 / steps as f64;
                    points.push(std::array::from_fn(|i| (a[i] + (b[i] - a[i]) * t) as f32));
                }
            }
            points.extend(path.last().map(|p| p.map(|c| c as f32)));
            (id, points)
        })
        .collect()
}

/// How far from a support's tip a press on the support grabs it, in mm.
const GRAB_REACH_MM: f64 = 3.0;

/// Distance from `p` to the segment `a`-`b`, on screen.
/// How near an elbow of a picked support a drag of the Select tool takes
/// that elbow rather than the trunk, mm (#377).
const ELBOW_GRAB_MM: f64 = 1.5;
/// The farthest a drag moves a part in one go, mm: past it the cursor is
/// near the horizon of the plane it moves on.
const PART_REACH_MM: f64 = 200.0;

/// Where a drag of the Select tool puts the part grabbed at `grab` (world
/// mm) with the cursor at `cursor` (#377): a trunk moves across the plate,
/// on the level plane through `grab`; an elbow on the plane through it that
/// faces the eye. The point of that plane `project` puts under the cursor,
/// by Newton's steps; `None` where none is.
fn part_target(
    grab: [f64; 3],
    elbow: Option<&str>,
    free: bool,
    cursor: [f32; 2],
    project: Project<'_>,
) -> Option<PartMove> {
    let screen = |p: [f64; 3]| {
        project
            .project(p.map(|c| c as f32))
            .map(|q| [f64::from(q[0]), f64::from(q[1])])
    };
    let along =
        |p: [f64; 3], d: [f64; 3], t: f64| -> [f64; 3] { std::array::from_fn(|k| p[k] + d[k] * t) };
    const H: f64 = 0.05;
    let s0 = screen(grab)?;
    let pull = |d: [f64; 3]| -> Option<[f64; 2]> {
        let q = screen(along(grab, d, H))?;
        Some([(q[0] - s0[0]) / H, (q[1] - s0[1]) / H])
    };
    let unit = |d: [f64; 3]| -> Option<[f64; 3]> {
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        (n > 1e-9).then(|| d.map(|c| c / n))
    };
    let dirs = match elbow.is_some() || free {
        false => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        true => {
            // The ways that move the elbow most across and up the screen.
            let (x, y, z) = (
                pull([1.0, 0.0, 0.0])?,
                pull([0.0, 1.0, 0.0])?,
                pull([0.0, 0.0, 1.0])?,
            );
            [unit([x[0], y[0], z[0]])?, unit([x[1], y[1], z[1]])?]
        }
    };
    let at = |uv: [f64; 2]| along(along(grab, dirs[0], uv[0]), dirs[1], uv[1]);
    let goal = [f64::from(cursor[0]), f64::from(cursor[1])];
    let mut uv = [0.0f64; 2];
    for _ in 0..16 {
        let p = at(uv);
        let s = screen(p)?;
        let r = [goal[0] - s[0], goal[1] - s[1]];
        if r[0].hypot(r[1]) < 0.05 {
            break;
        }
        let a = screen(along(p, dirs[0], H))?;
        let b = screen(along(p, dirs[1], H))?;
        let (ja, jb) = (
            [(a[0] - s[0]) / H, (a[1] - s[1]) / H],
            [(b[0] - s[0]) / H, (b[1] - s[1]) / H],
        );
        let det = ja[0] * jb[1] - jb[0] * ja[1];
        if det.abs() < 1e-12 {
            return None;
        }
        uv[0] += (r[0] * jb[1] - jb[0] * r[1]) / det;
        uv[1] += (ja[0] * r[1] - r[0] * ja[1]) / det;
        if uv[0].hypot(uv[1]) > PART_REACH_MM {
            return None;
        }
    }
    let p = at(uv);
    if !p.iter().all(|c| c.is_finite()) {
        return None;
    }
    // Hundredths of a millimetre: a hair of the cursor asks nothing new.
    let round = |c: f64| (c * 100.0).round() / 100.0;
    Some(match elbow {
        None if free => PartMove::Shaft(std::array::from_fn(|k| round(p[k] - grab[k]))),
        None => PartMove::Trunk([round(p[0] - grab[0]), round(p[1] - grab[1])]),
        Some(e) => PartMove::Elbow(e.to_owned(), p.map(round)),
    })
}

fn screen_segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + ab[0] * t)).hypot(p[1] - (a[1] + ab[1] * t))
}

/// A preview to compute: the session to edit, where, and with which tool.
struct HoverAsk {
    generation: u64,
    /// Shared, not copied: a copy of a busy model's supports costs ~8 ms,
    /// paid on the worker instead of on every mouse move.
    base: Arc<SupportSession>,
    /// Which set and session `base` is ([`session_key`]).
    from: (NodeId, usize),
    point: [f32; 3],
    tool: i32,
    /// For an Add: how the support is made.
    how: AddOptions,
    /// For a move (`tool` 3): the support whose tip moves to `point`.
    tip: Option<String>,
    /// For a box or a brush (`tool` 5): the supports they take.
    tips: Arc<[String]>,
    /// For a warm-up (`tool` 0): the other sets' sessions, warmed with it.
    others: Vec<Arc<SupportSession>>,
    /// For the brace tool's second support (`tool` 7): the first end.
    brace_from: Option<BraceEnd>,
    /// For a part of a support moved (`tool` 10, #377): what moves where;
    /// `tip` is the support.
    part: Option<PartMove>,
}

/// What tells a drawing's session apart for the hover preview: its leaf
/// and the session itself.
fn session_key(drawn: &DrawnSet) -> (NodeId, usize) {
    (drawn.leaf, Arc::as_ptr(&drawn.session) as usize)
}

/// A computed preview.
struct HoverAnswer {
    generation: u64,
    from: (NodeId, usize),
    point: [f32; 3],
    tool: i32,
    how: AddOptions,
    tip: Option<String>,
    brace_from: Option<BraceEnd>,
    part: Option<PartMove>,
    preview: Option<EditPreview>,
    /// The preview shows only what the tool holds (the brace tool's first
    /// support, the cursor off any other): drawn as pending.
    pending: bool,
    /// For an Add that goes through: the session the click leaves.
    edited: Option<SupportSession>,
}

/// One background thread answering the newest ask; asks that queue up
/// behind a slow one are skipped.
struct HoverWorker {
    asks: std::sync::mpsc::Sender<HoverAsk>,
    answer: Arc<std::sync::Mutex<Option<HoverAnswer>>>,
    /// Answers done with, let go of off the UI thread.
    discard: std::sync::mpsc::Sender<HoverAnswer>,
}

impl HoverWorker {
    fn spawn() -> Self {
        let (asks, inbox) = std::sync::mpsc::channel::<HoverAsk>();
        let answer = Arc::new(std::sync::Mutex::new(None));
        let out = answer.clone();
        let _ = std::thread::Builder::new()
            .name("support-hover-preview".into())
            // The router recurses; room to spare on dense models.
            .stack_size(16 << 20)
            .spawn(move || {
                while let Ok(mut ask) = inbox.recv() {
                    while let Ok(newer) = inbox.try_recv() {
                        ask = newer;
                    }
                    if ask.tool == 0 {
                        // Warm-up after an edit: no answer to draw.
                        for base in std::iter::once(&ask.base).chain(&ask.others) {
                            let base = base.clone();
                            let _ = std::panic::catch_unwind(move || base.warm_removal());
                        }
                        continue;
                    }
                    let p = ask.point.map(f64::from);
                    let mut pending = matches!(ask.tool, 4 | 6);
                    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        match (ask.tool, &ask.tip) {
                            // The brace tool (#3): the support a first click
                            // takes, then the brace to the one under the
                            // cursor; off any, the first one still.
                            (6, _) => ask.base.preview_brace_end(p).map(|(v, _)| (v, None)),
                            (7, _) => match &ask.brace_from {
                                // Off any support, or still on the first:
                                // the first, as picked.
                                Some(from) => match ask.base.preview_brace(from, p) {
                                    Err(EditError::NothingThere) => {
                                        pending = true;
                                        ask.base.preview_brace_end(from.at).map(|(v, _)| (v, None))
                                    }
                                    Ok((v, _))
                                        if v.brace == Some(HandBraceRefusal::SameSupport) =>
                                    {
                                        pending = true;
                                        ask.base.preview_brace_end(from.at).map(|(v, _)| (v, None))
                                    }
                                    other => other,
                                },
                                None => Err(EditError::NothingThere),
                            },
                            (1, _) => ask.base.preview_add_with(p, &ask.how),
                            (3, Some(tip)) => ask.base.preview_move_tip(tip, p, &ask.how),
                            (10, Some(id)) => match &ask.part {
                                Some(part) => part.preview(&ask.base, id),
                                None => Err(EditError::NothingThere),
                            },
                            // Move tip's hover: what a press there grabs.
                            (4, _) => ask
                                .base
                                .tip_near(p, GRAB_REACH_MM)
                                .ok_or(EditError::NothingThere)
                                .and_then(|tip| ask.base.preview_remove_tips(&[tip]))
                                .map(|v| (v, None)),
                            (5, _) => ask.base.preview_remove_tips(&ask.tips).map(|v| (v, None)),
                            _ => ask.base.preview_remove_at(p).map(|v| (v, None)),
                        }
                    }));
                    let (preview, edited) = match done {
                        Ok(Ok((preview, edited))) => (Some(preview), edited),
                        _ => (None, None),
                    };
                    let answer = HoverAnswer {
                        generation: ask.generation,
                        from: ask.from,
                        point: ask.point,
                        tool: ask.tool,
                        how: ask.how,
                        tip: ask.tip,
                        brace_from: ask.brace_from,
                        part: ask.part,
                        preview,
                        pending,
                        edited,
                    };
                    // An answer never drawn is let go of after the lock: the
                    // UI thread takes the next one through it.
                    let unseen = out.lock().ok().and_then(|mut slot| slot.replace(answer));
                    drop(unseen);
                }
            });
        let (discard, done_with) = std::sync::mpsc::channel::<HoverAnswer>();
        let _ = std::thread::Builder::new()
            .name("support-hover-discard".into())
            .spawn(move || while done_with.recv().is_ok() {});
        HoverWorker {
            asks,
            answer,
            discard,
        }
    }
}

/// A named set of options.
#[derive(Clone)]
struct Preset {
    name: String,
    options: SupportOptions,
    built_in: bool,
    /// For a built-in preset: its values as they ship. `options` holds the
    /// person's changes to it, kept apart for each of the three (#564).
    factory: Option<SupportOptions>,
}

/// The presets that ship: the three zone presets (#360, #479). The
/// classic sizing tiers left the list (#490); a project saved with one
/// still reopens with it, and a person's own presets still show.
fn built_in_presets() -> Vec<Preset> {
    let zones = |name: slint::SharedString, zones: ZonePreset| {
        let options = SupportOptions {
            zones: Some(zones),
            ..Default::default()
        };
        Preset {
            name: name.to_string(),
            factory: Some(options.clone()),
            options,
            built_in: true,
        }
    };
    vec![
        zones(ctr!("Inspector", "Miniatures 20~65 mm"), minis_30()),
        zones(ctr!("Inspector", "Figures 65~150 mm"), figures_70()),
        zones(ctr!("Inspector", "Statues 150 mm+"), figures_150()),
    ]
}

/// The built-in presets as the shape panel's cards (#564), in the list's
/// order: their names, and the size of models each is for.
fn preset_cards() -> Vec<(slint::SharedString, &'static str)> {
    vec![
        (ctr!("Inspector", "Miniatures"), "20–65 mm"),
        (ctr!("Inspector", "Figures"), "65–150 mm"),
        (ctr!("Inspector", "Statues"), "150 mm+"),
    ]
}

/// A zone's name and what it covers, as a row of the shape panel lists it.
/// The line tool's spacing between supports, mm, until one is typed (#178).
const LINE_SPACING_MM: f64 = 2.0;

/// The supports of a row along `path` (points on the surface, in order),
/// `spacing` mm apart along it (#178): evenly over each stretch, ends
/// included. Where the path jumps (it left the model), a new stretch starts.
fn line_points(path: &[[f32; 3]], spacing: f64) -> Vec<[f32; 3]> {
    let spacing = spacing.max(0.1);
    let gap = |a: [f32; 3], b: [f32; 3]| {
        (0..3)
            .map(|k| f64::from(b[k] - a[k]).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let mut stretches: Vec<Vec<[f32; 3]>> = Vec::new();
    for &p in path {
        match stretches.last_mut() {
            Some(run)
                if gap(*run.last().expect("a stretch has a point"), p) <= spacing.max(1.0) =>
            {
                run.push(p)
            }
            _ => stretches.push(vec![p]),
        }
    }
    let mut out = Vec::new();
    for run in stretches {
        let lengths: Vec<f64> = std::iter::once(0.0)
            .chain(run.windows(2).scan(0.0, |sum, w| {
                *sum += gap(w[0], w[1]);
                Some(*sum)
            }))
            .collect();
        let total = *lengths.last().expect("a stretch has a point");
        let n = (total / spacing).round() as usize;
        if n == 0 {
            out.push(run[0]);
            continue;
        }
        for i in 0..=n {
            let at = total * i as f64 / n as f64;
            let k = lengths.partition_point(|&l| l < at).clamp(1, run.len() - 1);
            let (a, b) = (run[k - 1], run[k]);
            let span = lengths[k] - lengths[k - 1];
            let t = if span > 0.0 {
                ((at - lengths[k - 1]) / span) as f32
            } else {
                0.0
            };
            out.push([0, 1, 2].map(|c| a[c] + (b[c] - a[c]) * t));
        }
    }
    out
}

/// A dot at each of `row`, for the line tool's preview.
fn line_dots(row: &[[f32; 3]]) -> Option<crate::mesh_render::Overlay> {
    use crate::mesh_render::{OverlayMesh, OverlayTint};
    if row.is_empty() {
        return None;
    }
    let mut piece = OverlayMesh {
        positions: Vec::new(),
        indices: Vec::new(),
        tint: OverlayTint::Picked,
    };
    // Round, like the tip of a support (#661).
    for &p in row {
        crate::measure::push_ball(&mut piece, p, 0.3);
    }
    Some(Arc::from(vec![Arc::new(piece)]))
}

/// The red dot where a support cannot be placed, for a spot that draws no
/// support at all.
fn refused_dot(at: [f32; 3]) -> OverlayMesh {
    use crate::mesh_render::{OverlayMesh, OverlayTint};
    let mut piece = OverlayMesh {
        positions: Vec::new(),
        indices: Vec::new(),
        tint: OverlayTint::Invalid,
    };
    crate::measure::push_ball(&mut piece, at, REFUSED_DOT_MM);
    piece
}

/// The radius of [`refused_dot`], mm: a little larger than the Line tool's
/// dots.
const REFUSED_DOT_MM: f32 = 0.45;

/// What a bracing preset is called on the panel (#363).
fn bracing_preset_name(id: &str) -> slint::SharedString {
    match id {
        "auto support" => ctr!("Inspector", "Auto support"),
        "default" => ctr!("Inspector", "Default"),
        "small object" => ctr!("Inspector", "Small object"),
        "tall object" => ctr!("Inspector", "Tall object"),
        "strong" => ctr!("Inspector", "Strong"),
        other => other.into(),
    }
}

fn zone_texts(
    zones: &ZonePreset,
    zone: crate::support_shape_edit::Zone,
) -> (slint::SharedString, slint::SharedString) {
    use crate::support_shape_edit::Zone;
    let r = &zones.rules;
    match zone {
        Zone::Overhang => (
            ctr!("Inspector", "Overhangs"),
            ctr!("Inspector", "faces past the angle"),
        ),
        Zone::Stabilization => (
            ctr!("Inspector", "Stabilization"),
            ctr!(
                "Inspector",
                "flat faces from {} mm²",
                mm_text(r.stabilization.min_area_mm2)
            ),
        ),
        Zone::Island(i) => {
            let area = r.first_layer.get(i).map_or(0.0, |b| b.min_area_mm2);
            let detail = if area > 0.0 {
                ctr!("Inspector", "first layer from {} mm²", mm_text(area))
            } else {
                ctr!("Inspector", "every island")
            };
            (ctr!("Inspector", "Islands"), detail)
        }
        Zone::IslandBody(i) => {
            let volume = r.reinforcement.get(i).map_or(0.0, |b| b.min_volume_mm3);
            (
                ctr!("Inspector", "Island bodies"),
                ctr!("Inspector", "from {} mm³", mm_text(volume)),
            )
        }
    }
}

/// The Supports tab's summary of the zone preset of `options` (#360),
/// empty without one.
fn zone_summary(options: &SupportOptions) -> slint::SharedString {
    let Some(zones) = &options.zones else {
        return slint::SharedString::default();
    };
    let rules = &zones.rules;
    let mut tips: Vec<f64> = rules
        .first_layer
        .iter()
        .filter(|b| b.enabled)
        .map(|b| b.size.as_str())
        .chain(
            rules
                .reinforcement
                .iter()
                .filter(|b| b.enabled)
                .map(|b| b.size.as_str()),
        )
        .chain(
            rules
                .overhang
                .enabled
                .then_some(rules.overhang.size.as_str()),
        )
        .chain(
            rules
                .stabilization
                .enabled
                .then_some(rules.stabilization.size.as_str()),
        )
        .filter_map(|id| rules.size_index(id))
        .map(|i| rules.sizes[i].tip_diameter_mm)
        .collect();
    tips.sort_by(f64::total_cmp);
    tips.dedup();
    let tips: Vec<String> = tips.into_iter().map(mm_text).collect();
    let lines = [
        ctr!(
            "Inspector",
            "This preset sets the size and spacing of every support by zone: island first layers, their reinforcement, overhangs past {}° and stabilization.",
            mm_text(rules.overhang.angle_deg)
        ),
        ctr!("Inspector", "Tips: {} mm.", tips.join(", ")),
        ctr!(
            "Inspector",
            "Bracing: {} mm, from {} mm up.",
            mm_text(zones.bracing.diameter_mm),
            mm_text(zones.bracing.start_mm)
        ),
    ];
    lines.join("\n").into()
}

/// The grid spacing the built-in preset of `options` recommends, mm (#464):
/// the preset with the same zones but for the grid. `None` when no built-in
/// preset has those zones, or it leaves the grid off.
fn recommended_grid_mm(options: &SupportOptions) -> Option<f64> {
    let zones = options.zones.as_ref()?;
    [minis_30(), figures_70(), figures_150()]
        .into_iter()
        .find(|preset| {
            ZonePreset {
                grid_mm: preset.grid_mm,
                ..zones.clone()
            } == *preset
        })
        .and_then(|preset| preset.grid_mm)
}

/// The spacing the grid of `options` shows, mm: its own, else the one its
/// preset recommends, else the grid's fallback.
/// The pillar grid the supports of `options` stand on, as its spacing's
/// bits (#576): none without a zone preset's grid, nor under one raft for
/// all, where the supports drop next to their model.
fn grid_in_use(options: &SupportOptions) -> Option<u64> {
    if options.one_raft {
        return None;
    }
    options
        .zones
        .as_ref()
        .and_then(|z| z.grid_mm)
        .map(f64::to_bits)
}

fn shown_grid_mm(options: &SupportOptions) -> f64 {
    options
        .zones
        .as_ref()
        .and_then(|z| z.grid_mm)
        .or_else(|| recommended_grid_mm(options))
        .unwrap_or(DEFAULT_GRID_MM)
}

/// The grid spacing of a zone preset that names none, mm: the small models'.
const DEFAULT_GRID_MM: f64 = 7.0;

/// The Supports tab's line on the spacing the preset recommends, empty when
/// no built-in preset does.
fn grid_hint(options: &SupportOptions) -> slint::SharedString {
    recommended_grid_mm(options).map_or_else(slint::SharedString::default, |mm| {
        ctr!(
            "Inspector",
            "Recommended for this preset: {} mm.",
            mm_text(mm)
        )
    })
}

/// Where the options and presets live: the app's local data folder. Tests
/// never touch it.
fn support_data_dir() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(
        sc_app_paths::project_dirs()?
            .data_local_dir()
            .join("supports"),
    )
}

/// The options the Supports tab starts with: the ones it was left with,
/// when they carry a zone preset; else Miniatures 20~65 mm (#490), the first of
/// the presets that ship.
pub(crate) fn load_saved_options() -> SupportOptions {
    support_data_dir()
        .and_then(|d| std::fs::read_to_string(d.join("options.toml")).ok())
        .and_then(|text| toml::from_str::<SupportOptions>(&text).ok())
        .map(|o| o.clamped())
        .filter(|o| o.zones.is_some())
        .unwrap_or_else(starting_options)
}

/// The hand sizes as they are saved: a table of sizes.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SavedHandSizes {
    #[serde(default)]
    sizes: Vec<sc_geom::supports::SupportSize>,
}

/// The sizes the hand tools place: the ones left the time before, else the
/// ones that ship.
fn load_hand_sizes() -> ZonePreset {
    let saved = support_data_dir()
        .and_then(|d| std::fs::read_to_string(d.join("hand-sizes.toml")).ok())
        .and_then(|text| toml::from_str::<SavedHandSizes>(&text).ok())
        .unwrap_or_default();
    crate::support_shape_edit::hand_from_saved(saved.sizes)
}

fn save_hand_sizes(hand: &ZonePreset) {
    let saved = SavedHandSizes {
        sizes: hand.rules.sizes.clone(),
    };
    if let Some(dir) = support_data_dir()
        && std::fs::create_dir_all(&dir).is_ok()
        && let Ok(text) = toml::to_string_pretty(&saved)
    {
        let _ = std::fs::write(dir.join("hand-sizes.toml"), text);
    }
}

/// `options` as a preset holds them: without one raft for all, which is how
/// the plate prints (#556).
fn preset_part(options: &SupportOptions) -> SupportOptions {
    SupportOptions {
        one_raft: false,
        ..options.clone()
    }
}

/// Whether the shape panel shows the support as a plan, as it was left.
fn load_shape_flat() -> bool {
    support_data_dir()
        .and_then(|d| std::fs::read_to_string(d.join("shape-view")).ok())
        .is_some_and(|text| text.trim() == "plan")
}

fn save_shape_flat(flat: bool) {
    if let Some(dir) = support_data_dir()
        && std::fs::create_dir_all(&dir).is_ok()
    {
        let _ = std::fs::write(dir.join("shape-view"), if flat { "plan" } else { "3d" });
    }
}

/// The built-in zone preset for a model `height_mm` tall (#723): up to
/// 65 mm Miniatures, up to 150 mm Figures, taller Statues.
fn preset_for_height(height_mm: f64) -> usize {
    if height_mm <= 65.0 {
        0
    } else if height_mm <= 150.0 {
        1
    } else {
        2
    }
}

fn load_by_height() -> bool {
    // Tests pick their preset; the one by height is tested on its own.
    if cfg!(test) {
        return false;
    }
    support_data_dir()
        .and_then(|d| std::fs::read_to_string(d.join("by-height")).ok())
        .is_none_or(|text| text.trim() != "off")
}

fn save_by_height(on: bool) {
    if let Some(dir) = support_data_dir()
        && std::fs::create_dir_all(&dir).is_ok()
    {
        let _ = std::fs::write(dir.join("by-height"), if on { "on" } else { "off" });
    }
}

/// Miniatures 20~65 mm (#490): what the Supports tab starts with.
fn starting_options() -> SupportOptions {
    SupportOptions {
        zones: Some(minis_30()),
        ..Default::default()
    }
}

/// Where built-in preset `index`'s changes are kept (#564).
fn card_file(index: usize) -> Option<std::path::PathBuf> {
    Some(
        support_data_dir()?
            .join("built-in")
            .join(format!("{index}.toml")),
    )
}

/// Keeps built-in preset `index` as `options`; as it ships, nothing is kept.
fn save_card(index: usize, options: &SupportOptions, factory: &SupportOptions) {
    let Some(path) = card_file(index) else {
        return;
    };
    if options == factory {
        let _ = std::fs::remove_file(path);
    } else if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_ok()
        && let Ok(text) = toml::to_string_pretty(options)
    {
        let _ = std::fs::write(path, text);
    }
}

fn load_user_presets() -> Vec<Preset> {
    let Some(dir) = support_data_dir().map(|d| d.join("presets")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut presets: Vec<Preset> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| {
            let name = e.path().file_stem()?.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(e.path()).ok()?;
            let options = toml::from_str::<SupportOptions>(&text).ok()?.clamped();
            Some(Preset {
                name,
                options,
                built_in: false,
                factory: None,
            })
        })
        .collect();
    presets.sort_by_key(|p| p.name.to_lowercase());
    presets
}

/// A preset name made safe as a file name.
fn preset_file_stem(name: &str) -> Option<String> {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_().".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim().trim_matches('.').to_owned();
    (!stem.is_empty()).then_some(stem)
}

impl SupportRun {
    /// Wires the Tools menu and the panel's Cancel button; the caller keeps
    /// the driver alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let last_change = Rc::new(Cell::new(Instant::now()));
        let raft_armed = Rc::new(Cell::new(false));
        let scene_events = document.subscribe({
            let last_change = last_change.clone();
            let raft_armed = raft_armed.clone();
            move |event| {
                if matches!(event, SceneEvent::Installed | SceneEvent::Cleared) {
                    raft_armed.set(false);
                }
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::Mutated
                        | SceneEvent::Restructured
                ) {
                    last_change.set(Instant::now());
                }
            }
        });
        let sessions = document.support_sessions();
        let area_brush = crate::surface_brush::SurfaceBrush::new(
            document.clone(),
            sc_scene::PaintKind::SupportEnforce,
        );
        ctrl.set_support_brush(area_brush.clone());
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            bus,
            document,
            dialogs,
            handle: RefCell::new(None),
            started_on: RefCell::new(None),
            plate_lift: RefCell::new(None),
            cancelled: Cell::new(false),
            lift_pref: RefCell::new(None),
            overhang_pref: RefCell::new(None),
            raft_under_plate_pref: RefCell::new(None),
            lines_under_plate_pref: RefCell::new(None),
            support_view_pref: RefCell::new(None),
            regenerating: RefCell::new(None),
            poll_timer: Rc::new(slint::Timer::default()),
            prescan: RefCell::new(Prescan::default()),
            plate_rafts: crate::plate_raft::PlateRafts::default(),
            ahead: RefCell::new(Ahead::default()),
            hints: RefCell::new(crate::support_hints::Hints::default()),
            last_change,
            prescan_timer: slint::Timer::default(),
            _scene_events: scene_events,
            own_rafts: RefCell::new(Vec::new()),
            selection_events: RefCell::new(None),
            options: RefCell::new(load_saved_options()),
            presets: RefCell::new(Vec::new()),
            shape_view: Cell::new(crate::support_shape::View::default()),
            shape_flat: Cell::new(load_shape_flat()),
            shape_texts: std::array::from_fn(|_| Rc::new(slint::VecModel::default())),
            shape_size: Cell::new(None),
            shape_hot: Cell::new(crate::support_shape::Part::None),
            based_on: Cell::new(None),
            editing: Cell::new(None),
            picked: Cell::new(None),
            by_height: Cell::new(load_by_height()),
            height_picks: RefCell::new(None),
            chosen: RefCell::new(Vec::new()),
            chosen_events: RefCell::new(None),
            chosen_listeners: RefCell::new(Vec::new()),
            part_focus: RefCell::new(None),
            part_hover: Cell::new(None),
            part_arrow: Cell::new(None),
            unborn_hover: RefCell::new(None),
            hand: RefCell::new(load_hand_sizes()),
            share: RefCell::new(Vec::new()),
            sessions,
            ctrl,
            hover_worker: HoverWorker::spawn(),
            hover_generation: Cell::new(0),
            hover_shown: Cell::new(0),
            hover_poll: slint::Timer::default(),
            last_preview: RefCell::new(None),
            last_ghost: RefCell::new(Vec::new()),
            hover_at: Cell::new(None),
            pending_click: RefCell::new(None),
            drag: RefCell::new(None),
            brace_from: RefCell::new(None),
            line_from: Cell::new(None),
            line_spacing: Cell::new(LINE_SPACING_MM),
            area_brush,
            box_poll: slint::Timer::default(),
            me: RefCell::new(std::rc::Weak::new()),
            queued_clicks: RefCell::new(std::collections::VecDeque::new()),
            click_poll: slint::Timer::default(),
            raft_armed,
            raft_ignored: Cell::new(false),
            raft_layer_height_mm: Cell::new(SupportRequest::default().layer_height_mm),
            raft_checked_at: Cell::new(Instant::now()),
            generated_raft_top: Cell::new(None),
            after_generate: RefCell::new(None),
            islands_after_raft: RefCell::new(None),
            edges_after_islands: Cell::new(false),
            status_timer: slint::Timer::default(),
            redo: RefCell::new(None),
            regrid: RefCell::new(None),
            grid_stale: RefCell::new((None, Vec::new())),
            healing_tried: RefCell::new(std::collections::HashSet::new()),
        });
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_tool_hover(move |x, y, z, hit, shift| {
                if let Some(run) = weak.upgrade() {
                    run.on_tool_hover_with(hit.then_some([x, y, z]), shift);
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_tool_clicked(move |x, y, z, forced| {
                if let Some(run) = weak.upgrade() {
                    run.on_tool_click_with([x, y, z], forced);
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_tool_missed(move |add| {
                if let Some(run) = weak.upgrade()
                    && let Some(ui) = run.ui.upgrade()
                    && ui.get_support_tool() == 5
                {
                    run.select_click(&ui, None, add);
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_tool_changed(move || {
                if let Some(run) = weak.upgrade() {
                    run.on_tool_changed();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_tip_size_changed(move || {
                if let Some(run) = weak.upgrade() {
                    run.on_tip_size_changed();
                }
            });
        }
        *run.me.borrow_mut() = Rc::downgrade(&run);
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_picked_clear(move || {
                if let Some(run) = weak.upgrade() {
                    run.clear_chosen();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_picked_hide(move || {
                if let Some(run) = weak.upgrade() {
                    run.hide_chosen();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_hidden_show(move || {
                if let Some(run) = weak.upgrade() {
                    run.hide_tips(Vec::new());
                }
            });
        }
        {
            // An edit, an Undo or a new scene: the picked supports are shown
            // as they stand (#567).
            let weak = Rc::downgrade(&run);
            let sub = run.document.subscribe(move |event| {
                // Another scene shows every support again (#728).
                if matches!(event, SceneEvent::Installed | SceneEvent::Cleared)
                    && let Some(run) = weak.upgrade()
                {
                    run.hide_tips(Vec::new());
                }
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::Mutated
                        | SceneEvent::Restructured
                ) && let Some(run) = weak.upgrade()
                {
                    let weak = Rc::downgrade(&run);
                    // After the event: the sessions in force follow it.
                    slint::Timer::single_shot(Duration::from_millis(0), move || {
                        if let Some(run) = weak.upgrade()
                            && let Some(ui) = run.ui.upgrade()
                        {
                            run.show_chosen(&ui);
                            // The supports hidden stay hidden in the sets
                            // as they stand now.
                            let hidden = run.ctrl.hidden_supports().tips;
                            if !hidden.is_empty() {
                                run.hide_tips(hidden);
                            }
                        }
                    });
                }
            });
            run.chosen_events.borrow_mut().replace(sub);
        }
        {
            let weak = Rc::downgrade(&run);
            let sub = run.document.subscribe(move |event| {
                if matches!(event, SceneEvent::SelectionChanged)
                    && let Some(run) = weak.upgrade()
                    && let Some(ui) = run.ui.upgrade()
                {
                    run.push_raft(&ui);
                }
            });
            *run.selection_events.borrow_mut() = Some(sub);
        }
        {
            let handler: Rc<dyn SupportDragHandler> = run.clone();
            run.ctrl.set_support_drag_handler(Rc::downgrade(&handler));
            run.ctrl
                .set_part_handles(Rc::new(PartHandles(Rc::downgrade(&run))));
        }
        run.reload_presets();
        run.find_card();
        run.push_options(ui);
        {
            let run = run.clone();
            ui.on_support_option_changed(move |code, value| run.on_option(code, value));
        }
        {
            let run = run.clone();
            ui.on_support_preset_picked(move |i| run.on_preset_picked(i));
        }
        ui.set_support_by_height(run.by_height.get());
        {
            let run = run.clone();
            ui.on_support_by_height_toggled(move |on| run.set_by_height(on));
        }
        {
            let run = run.clone();
            ui.on_support_preset_revert(move || run.on_preset_revert());
        }
        ui.set_support_shape_flat(run.shape_flat.get());
        ui.set_support_shape_callouts(slint::ModelRc::from(run.shape_texts[0].clone()));
        ui.set_support_shape_tip_callouts(slint::ModelRc::from(run.shape_texts[1].clone()));
        ui.set_support_shape_foot_callouts(slint::ModelRc::from(run.shape_texts[2].clone()));
        {
            let run = run.clone();
            ui.on_support_shape_flat_picked(move |flat| {
                run.shape_flat.set(flat);
                save_shape_flat(flat);
                if let Some(ui) = run.ui.upgrade() {
                    ui.set_support_shape_flat(flat);
                }
                run.redraw_shape();
            });
        }
        {
            let run = run.clone();
            ui.on_support_shape_dragged(move |dx, dy| {
                run.shape_view.set(run.shape_view.get().dragged(dx, dy));
                run.redraw_shape();
            });
        }
        {
            let run = run.clone();
            ui.on_support_shape_pointed(move |part| {
                run.shape_hot
                    .set(crate::support_shape::Part::from_code(part));
                run.redraw_shape();
            });
        }
        {
            let run = run.clone();
            ui.on_support_shape_size_picked(move |i| run.on_shape_size_picked(i));
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_page_changed(move || {
                if let Some(run) = weak.upgrade()
                    && let Some(ui) = run.ui.upgrade()
                {
                    run.push_options(&ui);
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_share_export(move || {
                if let Some(run) = weak.upgrade() {
                    run.on_share_export();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_share_import(move || {
                if let Some(run) = weak.upgrade() {
                    run.on_share_import();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_share_toggled(move |i, on| {
                if let Some(run) = weak.upgrade() {
                    run.on_share_toggled(i, on);
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_share_accepted(move || {
                if let Some(run) = weak.upgrade() {
                    run.on_share_accepted();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_share_cancelled(move || {
                if let Some(run) = weak.upgrade() {
                    run.close_share();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_hand_reset(move || {
                if let Some(run) = weak.upgrade() {
                    run.set_hand(crate::support_shape_edit::hand_factory());
                }
            });
        }
        {
            let run = run.clone();
            ui.on_support_shape_value(move |code, v| run.on_shape_value(code, v));
        }
        {
            let run = run.clone();
            ui.on_support_shape_choice(move |code, v| run.on_shape_choice(code, v));
        }
        {
            let run = run.clone();
            ui.on_support_line_spacing_edited(move |v| run.on_line_spacing(v));
        }
        {
            let run = run.clone();
            ui.on_support_area_fill(move || run.fill_painted());
        }
        {
            let run = run.clone();
            ui.on_support_picked_reroute(move |own| run.reroute_chosen(own));
        }
        {
            let run = run.clone();
            ui.on_support_picked_join(move || run.join_chosen());
        }
        {
            let run = run.clone();
            ui.on_imported_supports_editable_requested(move || run.imported_to_editable());
        }
        {
            let run = run.clone();
            ui.on_support_area_clear(move || run.clear_painted());
        }
        {
            let run = run.clone();
            ui.on_support_area_brush_edited(move |v| run.on_area_brush_size(v));
        }
        {
            let run = run.clone();
            ui.on_support_zone_size(move |row, k| run.on_zone_size(row, k));
        }
        {
            let run = run.clone();
            ui.on_support_zone_spacing(move |row, v| run.on_zone_spacing(row, v));
        }
        {
            let run = run.clone();
            ui.on_support_zone_angle_edited(move |v| run.on_zone_angle(v));
        }
        {
            let run = run.clone();
            ui.on_support_preset_save(move |name| run.on_preset_save(&name));
        }
        {
            let run = run.clone();
            ui.on_support_preset_delete(move || run.on_preset_delete());
        }
        {
            let weak = Rc::downgrade(&run);
            run.prescan_timer.start(
                slint::TimerMode::Repeated,
                PRESCAN_POLL_INTERVAL,
                move || {
                    if let Some(run) = weak.upgrade() {
                        run.watch_prescan();
                    }
                },
            );
        }
        {
            let run = run.clone();
            ui.on_generate_supports_requested(move || {
                // A Generate press supports what it supports: a card's row
                // left waiting (#408) is dropped.
                run.redo.borrow_mut().take();
                run.regrid.borrow_mut().take();
                run.start();
            });
        }
        {
            let run = run.clone();
            ui.on_support_grid_regenerate(move || run.regenerate_off_grid());
        }
        {
            let weak = Rc::downgrade(&run);
            let ui_weak = ui.as_weak();
            ui.on_support_islands_go_requested(move || {
                if let (Some(run), Some(ui)) = (weak.upgrade(), ui_weak.upgrade()) {
                    run.go_to_island(&ui);
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_raft_lift_requested(move || {
                if let Some(run) = weak.upgrade() {
                    run.lift_for_warning();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_raft_off_requested(move || {
                if let Some(run) = weak.upgrade() {
                    run.raft_off_for_warning();
                }
            });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_raft_ignore_requested(move || {
                if let Some(run) = weak.upgrade() {
                    run.generate_ignoring_raft();
                }
            });
        }
        {
            let run = run.clone();
            ui.on_support_gen_cancel_requested(move || {
                // A cancelled regeneration is not started again: the models
                // stay without supports until Generate or Undo.
                if let Some((revision, wanted)) = run.regenerating.borrow().as_ref() {
                    run.document
                        .settle_supports_wanted(*revision, &wanted.ids());
                }
                if let Some(handle) = run.handle.borrow().as_ref() {
                    // Every model of the batch (#388) at once: the scene stays
                    // as it was, the plate lift (#389) included.
                    run.cancelled.set(true);
                    handle.cancel();
                }
                run.drop_waiting_run();
            });
        }
        {
            let bus = run.bus.clone();
            ui.on_support_lift_on_plate_toggled(move |on| {
                bus.update(move |snap| {
                    snap.lift_models_on_plate = on;
                    PreferencesChange::LiftModelsOnPlate
                });
            });
        }
        {
            let weak = Rc::downgrade(&run);
            let subscription = run.bus.subscribe(move |snap, change| {
                if matches!(change, PreferencesChange::LiftModelsOnPlate)
                    && let Some(run) = weak.upgrade()
                {
                    if let Some(ui) = run.ui.upgrade() {
                        ui.set_support_lift_on_plate(snap.lift_models_on_plate);
                    }
                    // A model resting on the plate is no longer, or is again,
                    // in the raft's way.
                    run.refresh_raft_warning();
                }
            });
            *run.lift_pref.borrow_mut() = Some(subscription);
        }
        ui.set_support_lift_on_plate(run.bus.snapshot().lift_models_on_plate);
        {
            let bus = run.bus.clone();
            ui.on_overhang_map_toggled(move |on| {
                bus.update(move |snap| {
                    snap.overhang_map = on;
                    PreferencesChange::OverhangMap
                });
            });
        }
        {
            let weak = Rc::downgrade(&run);
            let subscription = run.bus.subscribe(move |snap, change| {
                if matches!(change, PreferencesChange::OverhangMap)
                    && let Some(run) = weak.upgrade()
                {
                    run.show_overhang_map(snap.overhang_map);
                }
            });
            *run.overhang_pref.borrow_mut() = Some(subscription);
        }
        run.show_overhang_map(run.bus.snapshot().overhang_map);
        run.push_overhang_limit(ui);
        {
            let bus = run.bus.clone();
            ui.on_support_hide_rafts_under_plate_toggled(move |on| {
                bus.update(move |snap| {
                    snap.hide_rafts_under_plate = on;
                    PreferencesChange::HideRaftsUnderPlate
                });
            });
        }
        {
            let weak = Rc::downgrade(&run);
            let subscription = run.bus.subscribe(move |snap, change| {
                if matches!(change, PreferencesChange::HideRaftsUnderPlate)
                    && let Some(run) = weak.upgrade()
                {
                    run.hide_rafts_under_plate(snap.hide_rafts_under_plate);
                }
            });
            *run.raft_under_plate_pref.borrow_mut() = Some(subscription);
        }
        run.hide_rafts_under_plate(run.bus.snapshot().hide_rafts_under_plate);
        {
            let bus = run.bus.clone();
            ui.on_support_lines_under_plate_toggled(move |on| {
                bus.update(move |snap| {
                    snap.support_lines_under_plate = on;
                    PreferencesChange::SupportLinesUnderPlate
                });
            });
        }
        {
            let weak = Rc::downgrade(&run);
            let subscription = run.bus.subscribe(move |snap, change| {
                if matches!(change, PreferencesChange::SupportLinesUnderPlate)
                    && let Some(run) = weak.upgrade()
                {
                    run.support_lines_under_plate(snap.support_lines_under_plate);
                }
            });
            *run.lines_under_plate_pref.borrow_mut() = Some(subscription);
        }
        run.support_lines_under_plate(run.bus.snapshot().support_lines_under_plate);
        {
            let bus = run.bus.clone();
            ui.on_support_view_picked(move |view| {
                bus.update(move |snap| {
                    snap.support_view = u8::try_from(view.clamp(0, 2)).unwrap_or(0);
                    PreferencesChange::SupportView
                });
            });
        }
        {
            let weak = Rc::downgrade(&run);
            let subscription = run.bus.subscribe(move |snap, change| {
                if matches!(change, PreferencesChange::SupportView)
                    && let Some(run) = weak.upgrade()
                {
                    run.support_view(snap.support_view);
                }
            });
            *run.support_view_pref.borrow_mut() = Some(subscription);
        }
        run.support_view(run.bus.snapshot().support_view);
        run
    }

    /// How the supports are drawn at any camera height in this window
    /// (#137).
    fn support_view(&self, view: u8) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_view(i32::from(view));
        }
        self.ctrl.set_support_view(view);
    }

    /// Whether the supports are drawn as lines from under the plate in this
    /// window (#493).
    fn support_lines_under_plate(&self, on: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_lines_under_plate(on);
        }
        self.ctrl.set_support_lines_under_plate(on);
    }

    /// Whether the rafts fade out with the plate from under it in this window
    /// (#492).
    fn hide_rafts_under_plate(&self, on: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_hide_rafts_under_plate(on);
        }
        self.ctrl.set_hide_rafts_under_plate(on);
    }

    /// Turns the overhang map on or off in this window (#91).
    fn show_overhang_map(&self, on: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_overhang_map_on(on);
        }
        self.ctrl.set_overhang_map(on);
    }

    fn start(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading() || ui.get_support_gen_in_progress() || self.handle.borrow().is_some() {
            return;
        }
        // A run replaces the supports a hand click being made would edit.
        self.drop_click(
            ctr!(
                "SupportDialog",
                "Supports are being generated; the click was dropped."
            ),
            EditDropped::SceneChanged,
        );
        self.raft_layer_height_mm
            .set(self.request_for_printer().layer_height_mm);
        self.raft_armed.set(true);
        self.refresh_raft_warning();
        // A run started now would be thrown away by the warning's own lift
        // (the scene changes under it): wait for the user's answer, which
        // starts the run.
        if !self.raft_ignored.get() && self.raft_clash().is_some() {
            return;
        }
        // Supports made ahead are for the selection, without hand edits to
        // put back: a set made again from its card (#408) runs.
        if self.redo.borrow().is_none() && self.regrid.borrow().is_none() && self.take_ahead(&ui) {
            return;
        }
        {
            let mut prescan = self.prescan.borrow_mut();
            let current = prescan
                .running
                .as_ref()
                .is_some_and(|(_, _, started)| self.last_change.get() <= *started);
            if current {
                // The scene has not moved since the prescan started: wait for
                // it rather than scan the same geometry twice.
                prescan.run_waiting = true;
                ui.set_support_gen_in_progress(true);
                ui.set_support_gen_cancelable(true);
                ui.set_support_gen_progress(0.0);
                ui.set_support_gen_phase(ctr!("SupportDialog", "Preparing scans…"));
                return;
            }
            // A prescan of an older scene would not match: free its cores.
            if let Some((job, _, _)) = prescan.running.take() {
                job.cancel();
            }
        }
        self.launch_generate(&ui);
    }

    /// Starts the run of a Generate press: every model it supports (#388),
    /// those resting on the plate lifted first when the box says so (#389).
    fn launch_generate(self: &Rc<Self>, ui: &MainWindow) {
        let mut request = self.request_for_selection();
        // A set made again from its card (#408): that model alone, its hand
        // edits put back when asked.
        if let Some(redo) = self.redo.borrow_mut().take() {
            request.models = Some(vec![redo.model]);
            if let Some(options) = redo.options {
                request.options = options;
            }
            request.keep_edits = redo
                .keep
                .map(|session| vec![(redo.model, (*session).clone())])
                .unwrap_or_default();
        }
        // The supports made on another grid (#576), made again on the tab's.
        if let Some(models) = self.regrid.borrow_mut().take() {
            request.models = Some(models);
        }
        let lift = self.plate_lift(&request);
        self.launch(ui, request, lift);
    }

    /// The edit that lifts the models of `request` resting on the plate to
    /// the support lift (#389), not applied; `None` with the Supports tab's
    /// box off or no such model.
    fn plate_lift(&self, request: &SupportRequest) -> Option<SceneEdit> {
        if !self.bus.snapshot().lift_models_on_plate {
            return None;
        }
        let lift_to = support_lift_mm(&request.options, request.layer_height_mm);
        self.document
            .with_scene(|spatial| resting_lift_edit(spatial, lift_to, request.models.as_deref()))
            .flatten()
    }

    /// The scene a run works on: the document's, with `lift` made on a copy
    /// when given. The document itself stays as it is until the run's
    /// supports come in.
    fn run_scene(&self, lift: Option<&SceneEdit>) -> Option<sc_scene::Scene> {
        let mut scene = self
            .document
            .with_scene(|spatial| spatial.graph().clone())?;
        if let Some(lift) = lift {
            lift.apply_to_scene(&mut scene).ok()?;
        }
        Some(scene)
    }

    /// Starts the run of `request`, from the last prescan when it still
    /// matches, on the scene with `lift` made (#389).
    fn launch(
        self: &Rc<Self>,
        ui: &MainWindow,
        mut request: SupportRequest,
        lift: Option<SceneEdit>,
    ) {
        crate::breadcrumbs::started("supports");
        let Some(scene) = self.run_scene(lift.as_ref()) else {
            ui.set_support_gen_in_progress(false);
            ui.set_support_gen_cancelable(false);
            self.regenerating.borrow_mut().take();
            return;
        };
        {
            let prescan = self.prescan.borrow();
            // A stale entry would simply not match; there is no point
            // handing it over.
            if let Some((revision, scans)) = &prescan.ready
                && !self.document.slice_geometry_changed(*revision)
            {
                request.prescanned = Some(scans.clone());
            }
        }
        match start_supports(&scene, request) {
            Ok(handle) => {
                ui.set_support_gen_in_progress(true);
                ui.set_support_gen_cancelable(true);
                ui.set_support_gen_progress(0.0);
                ui.set_support_gen_phase(slint::SharedString::default());
                *self.handle.borrow_mut() = Some(handle);
                *self.started_on.borrow_mut() = Some(self.document.scene_revision());
                *self.plate_lift.borrow_mut() = lift;
                self.cancelled.set(false);
                self.start_poll();
            }
            Err(err) => {
                ui.set_support_gen_in_progress(false);
                ui.set_support_gen_cancelable(false);
                // Not tried again on every poll.
                if let Some((revision, wanted)) = self.regenerating.borrow_mut().take() {
                    self.document
                        .settle_supports_wanted(revision, &wanted.ids());
                }
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("SupportDialog", "Support generation failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
            }
        }
    }

    /// Cancels a Generate press that is waiting for the prescan, and the
    /// prescan with it.
    fn drop_waiting_run(&self) {
        let mut prescan = self.prescan.borrow_mut();
        if !prescan.run_waiting {
            return;
        }
        prescan.run_waiting = false;
        if let Some((job, _, _)) = prescan.running.take() {
            job.cancel();
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_gen_in_progress(false);
            ui.set_support_gen_cancelable(false);
        }
    }

    /// The prescan watcher, every [`PRESCAN_POLL_INTERVAL`]: collects a
    /// finished prescan (starting a run that waited for it), cancels one the
    /// scene has moved on from, and starts one once the scene has been still
    /// for [`PRESCAN_SETTLE`] and holds no matching scans yet, in the Hollow
    /// and Supports tabs or anywhere with the preference on. Idle while a
    /// support run is in flight or a scene is loading.
    fn watch_prescan(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.last_change.get() > self.raft_checked_at.get() {
            self.refresh_raft_warning();
        }
        self.watch_wanted(&ui);
        self.watch_grid(&ui);
        let mut prescan = self.prescan.borrow_mut();
        if let Some((job, revision, _)) = &prescan.running
            && let Some(finished) = job.try_poll()
        {
            let revision = *revision;
            prescan.running = None;
            crate::breadcrumbs::note(&format!(
                "supports: prescan finished ok={}",
                finished.is_ok()
            ));
            if let Ok(done) = finished {
                prescan.ready = Some((revision, done.scans));
            }
            if prescan.run_waiting {
                prescan.run_waiting = false;
                drop(prescan);
                self.launch_generate(&ui);
                return;
            }
        }
        drop(prescan);
        self.heal_stale_sets(&ui);
        if !ui.get_loading()
            && !ui.get_slicing_in_progress()
            && !ui.get_repairing_in_progress()
            && !ui.get_arranging()
            && self.handle.borrow().is_none()
            && !self.editing()
            && !self.ctrl.is_mid_gesture()
        {
            self.plate_rafts
                .tick(&self.document, &self.ctrl.build_volume());
            self.go_on_after_rafts();
        }
        self.support_hints(&ui);
        let mut prescan = self.prescan.borrow_mut();
        let tab_wants = matches!(
            ui.get_workspace_section(),
            WorkspaceSection::Hollow | WorkspaceSection::Supports
        );
        if !tab_wants && !self.bus.snapshot().prescan_supports {
            if !prescan.run_waiting
                && let Some((job, _, _)) = prescan.running.take()
            {
                job.cancel();
            }
            return;
        }
        let last_change = self.last_change.get();
        if let Some((job, _, started)) = &prescan.running
            && last_change > *started
        {
            // The scene moved under it: its scans would not match. A run
            // waiting for it starts on its own instead.
            job.cancel();
            prescan.running = None;
            if prescan.run_waiting {
                prescan.run_waiting = false;
                drop(prescan);
                self.launch_generate(&ui);
                return;
            }
        }
        let busy = self.handle.borrow().is_some()
            || ui.get_loading()
            || ui.get_support_gen_in_progress()
            || ui.get_slicing_in_progress();
        if ui.get_slicing_in_progress()
            && let Some((job, _, _, _)) = self.ahead.borrow_mut().running.take()
        {
            // A slice is waited on; supports ahead of time are not.
            job.cancel();
        }
        if busy || prescan.running.is_some() || last_change.elapsed() < PRESCAN_SETTLE {
            return;
        }
        if prescan
            .ready
            .as_ref()
            .is_some_and(|(revision, _)| !self.document.slice_geometry_changed(*revision))
        {
            // Scanned: with time to spare, generate ahead.
            let scans = prescan.ready.as_ref().map(|(_, scans)| scans.clone());
            drop(prescan);
            self.generate_ahead(&ui, scans);
            return;
        }
        let mut request = self.request_for_selection();
        // The geometry the Generate press will run on: models resting on the
        // plate lifted, when they will be (#389).
        let Some(scene) = self.run_scene(self.plate_lift(&request).as_ref()) else {
            return;
        };
        // Models the last prescan already scanned are taken from it.
        request.prescanned = prescan.ready.as_ref().map(|(_, scans)| scans.clone());
        let job = start_prescan(&scene, request);
        crate::breadcrumbs::note("supports: prescan started");
        prescan.running = Some((job, self.document.scene_revision(), Instant::now()));
    }

    /// Regenerates the supports of the models a move in Z, a tilt or a
    /// scale took them from (#153): starts the run once they stand where the
    /// edit left them and no other run is in flight, and cancels it when the
    /// scene moves on (another edit, Undo, a live move of one of them). The
    /// models' set goes to the same run Generate makes, for them alone; the
    /// raft warning is not asked, as the person chose the pose.
    fn watch_wanted(self: &Rc<Self>, ui: &MainWindow) {
        let wanted = self.document.supports_wanted();
        let stands = |w: &SupportsWanted| {
            self.document
                .with_scene(|spatial| w.stands_in(spatial))
                .unwrap_or(false)
        };
        if let Some((revision, _)) = self.regenerating.borrow().as_ref() {
            let current = wanted
                .as_ref()
                .is_some_and(|(r, w)| r == revision && stands(w));
            if !current && let Some(handle) = self.handle.borrow().as_ref() {
                handle.cancel();
            }
            return;
        }
        let Some((revision, wanted)) = wanted else {
            return;
        };
        // A model that came in is repaired first (#677).
        if self.handle.borrow().is_some()
            || ui.get_loading()
            || ui.get_repairing_in_progress()
            || ui.get_repair_on_import_pending()
            || self.prescan.borrow().run_waiting
            || !stands(&wanted)
        {
            return;
        }
        // As Generate makes them: own rafts, drains and groups too.
        let mut request = self.request_for_selection();
        // A member of a group is made with the whole group (#475).
        let mut models = wanted.ids();
        for group in &request.together {
            if group.iter().any(|m| models.contains(m)) {
                models.extend(group.iter().copied());
            }
        }
        models.sort_unstable();
        models.dedup();
        request.models = Some(models);
        crate::breadcrumbs::note(&format!(
            "supports: regenerating {} model(s)",
            wanted.models.len()
        ));
        *self.regenerating.borrow_mut() = Some((revision, wanted));
        // The person chose the pose: it is not lifted off the plate (#389).
        self.launch(ui, request, None);
    }

    /// Counts the models whose supports were made on another pillar grid
    /// than the tab's (#576), for the notes of the Supports tab and of
    /// Arrange: their supports are not made again on their own, and Arrange
    /// and Fill bed measure them as they are. Counted again when the scene
    /// or the tab's grid changes.
    fn watch_grid(&self, ui: &MainWindow) {
        let grid = grid_in_use(&self.options.borrow().clamped());
        let key = (self.document.scene_revision(), grid);
        if self.grid_stale.borrow().0 == Some(key) {
            return;
        }
        let mut models: Vec<NodeId> = self
            .document
            .with_scene(|spatial| sets_in_force(spatial, &self.sessions))
            .unwrap_or_default()
            .iter()
            // The grid the supports were made on: a raft drawn again under
            // them since (#519) leaves them, and their grid, as they were.
            .filter(|set| {
                !set.unborn
                    && set.session.options().is_some()
                    && set.session.grid_mm().map(f64::to_bits) != grid
            })
            .map(|set| set.model)
            .collect();
        let mut seen = std::collections::HashSet::new();
        models.retain(|&m| seen.insert(m));
        ui.set_support_grid_stale(models.len() as i32);
        *self.grid_stale.borrow_mut() = (Some(key), models);
    }

    /// Regenerate on the notes of supports made on another grid (#576): the
    /// supports of those models are made again on the tab's grid, as a
    /// Generate press of them alone would.
    fn regenerate_off_grid(self: &Rc<Self>) {
        let models = self.grid_stale.borrow().1.clone();
        if models.is_empty() {
            return;
        }
        crate::breadcrumbs::note(&format!(
            "supports: made again on the grid, {} model(s)",
            models.len()
        ));
        self.redo.borrow_mut().take();
        *self.regrid.borrow_mut() = Some(models);
        self.start();
    }

    /// Keeps the supports generated ahead in step with the scene: drops them
    /// when it moves or the Hollow tab gets busy (its previews come first),
    /// collects a finished run, and starts one once the scene has been still
    /// for [`SPECULATE_SETTLE`] with nothing matching yet.
    fn generate_ahead(&self, ui: &MainWindow, scans: Option<Arc<ScanCache>>) {
        let mut ahead = self.ahead.borrow_mut();
        let hollow_busy = ui.get_hollow_busy();
        if let Some((job, revision, _, _)) = &ahead.running {
            if hollow_busy || self.document.slice_geometry_changed(*revision) {
                job.cancel();
                ahead.running = None;
            } else if let Some(finished) = job.try_poll() {
                crate::breadcrumbs::note(&format!(
                    "supports: ahead finished ok={}",
                    finished.is_ok()
                ));
                let (_, revision, key, lift) = ahead.running.take().expect("checked above");
                if let Ok(outcome) = finished {
                    ahead.ready = Some((revision, key, outcome, lift));
                }
            }
        }
        // Supports already in the scene are being edited, not awaited.
        if ahead.running.is_some()
            || hollow_busy
            || self.last_change.get().elapsed() < SPECULATE_SETTLE
            || self.support_leaf().is_some()
        {
            return;
        }
        let mut request = self.request_for_selection();
        let key = ahead_key(&request, self.bus.snapshot().lift_models_on_plate);
        if ahead.ready.as_ref().is_some_and(|(revision, k, _, _)| {
            *k == key && !self.document.slice_geometry_changed(*revision)
        }) {
            return;
        }
        let lift = self.plate_lift(&request);
        let Some(scene) = self.run_scene(lift.as_ref()) else {
            return;
        };
        request.prescanned = scans;
        if let Ok(job) = start_supports(&scene, request) {
            crate::breadcrumbs::note("supports: ahead started");
            ahead.running = Some((job, self.document.scene_revision(), key, lift));
        }
    }

    /// Keeps the recommended places in step (#524): with Add or Move tip
    /// armed in the Supports tab, a run like Generate's works them out once
    /// the models have been still a moment, and again only when the models,
    /// their poses, the preset or the printer change; the rings show the
    /// places no tip covers yet. The islands an analysis found (#525) show
    /// in the tab with them, while the models stand as analyzed.
    pub(crate) fn support_hints(&self, ui: &MainWindow) {
        let tab = ui.get_workspace_section() == WorkspaceSection::Supports && !ui.get_loading();
        let armed = tab && matches!(ui.get_support_tool(), 1 | 3);
        let mut hints = self.hints.borrow_mut();
        if !armed && let Some((job, _)) = hints.running.take() {
            job.cancel();
        }
        if !tab {
            if hints.shown.take().is_some() {
                self.ctrl.set_hint_marks(None);
            }
            return;
        }
        if let Some((job, _)) = &hints.running
            && let Some(finished) = job.try_poll()
        {
            let (_, key) = hints.running.take().expect("checked above");
            crate::breadcrumbs::note(&format!("supports: hints finished ok={}", finished.is_ok()));
            if let Ok(outcome) = finished {
                hints.ready = Some((key, Arc::new(crate::support_hints::hints_of(&outcome))));
            }
        }
        let ready = if armed {
            self.ready_hints(ui, &mut hints)
        } else {
            None
        };
        let islands = hints
            .islands
            .as_ref()
            .filter(|(key, _)| self.models_key(None).as_deref() == Some(key.as_str()))
            .map(|(_, places)| places.clone());
        let revision = self.document.scene_revision();
        let ptr = |places: &Option<Arc<Vec<Hint>>>| {
            places.as_ref().map_or(0, |p| Arc::as_ptr(p) as usize)
        };
        let drawn = (ptr(&ready), ptr(&islands), revision);
        if hints.shown == Some(drawn) {
            return;
        }
        hints.shown = Some(drawn);
        if ready.is_none() && islands.is_none() {
            hints.left.clear();
            ui.set_support_islands_left(-1);
            self.ctrl.set_hint_marks(None);
            return;
        }
        // Where the supports in the scene touch the models, each with the
        // reach of its set's preset: every piece's contacts, twigs and
        // sticks too (#542), as the overhang map counts them (#484).
        let fallback = self.options.borrow().tip_reach_mm();
        let tips: Vec<([f64; 3], f64)> = self
            .document
            .with_scene(|spatial| {
                sets_in_force(spatial, &self.sessions)
                    .into_iter()
                    .flat_map(|drawn| {
                        let reach = drawn
                            .session
                            .options()
                            .map_or(fallback, |o| o.tip_reach_mm());
                        drawn
                            .session
                            .pieces()
                            .into_iter()
                            .flat_map(|piece| piece.contacts)
                            .map(move |contact| (contact.surface, reach))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let radius = (fallback * 0.25).clamp(0.3, 1.2);
        let mut meshes = Vec::new();
        if let Some(ready) = &ready {
            let left = crate::support_hints::uncovered(ready, &tips);
            meshes.push(Arc::new(crate::support_hints::rings(
                &left,
                radius,
                OverlayTint::Hint,
            )));
        }
        match &islands {
            Some(islands) => {
                let contacts: Vec<[f64; 3]> = tips.iter().map(|t| t.0).collect();
                let (left, waiting) =
                    self.paint_islands(islands, &contacts, &mut hints, &mut meshes);
                ui.set_support_islands_left(i32::try_from(left).unwrap_or(i32::MAX));
                if waiting {
                    // Drawn again once the models' adjacency is ready.
                    hints.shown = None;
                }
            }
            None => {
                hints.left.clear();
                ui.set_support_islands_left(-1);
            }
        }
        self.ctrl.set_hint_marks(Some(meshes.into()));
    }

    /// The islands of `islands` no support holds yet, painted on their
    /// models (#542), into `meshes`, and kept in `hints.left`: the faces
    /// looking down where each starts, seen where they show and,
    /// fainter, through everything; a dot for an island too small for its
    /// paint to read, and for one whose model's adjacency is still being
    /// built (a worker builds it). The island the camera went to (#538) is
    /// brighter. A support holds an island when one of `contacts` touches
    /// its paint ([`crate::island_paint::held`]). Returns how many are left
    /// and whether any waits for its model's adjacency.
    fn paint_islands(
        &self,
        islands: &[Hint],
        contacts: &[[f64; 3]],
        hints: &mut crate::support_hints::Hints,
        meshes: &mut Vec<Arc<OverlayMesh>>,
    ) -> (usize, bool) {
        use crate::island_paint::{DOT_UNDER_MM2, Paint, held, paint};
        use crate::support_hints::{ISLAND_DOT_PER_TIP, VISITED_DOT_SCALE, dots};
        let mut faces: Vec<[f32; 3]> = Vec::new();
        let mut visited_faces: Vec<[f32; 3]> = Vec::new();
        let mut dotted: Vec<&Hint> = Vec::new();
        let mut visited_dot: Vec<&Hint> = Vec::new();
        let mut waiting: Vec<NodeId> = Vec::new();
        self.document.with_scene(|spatial| {
            let models: Vec<NodeId> = crate::raft_clash::model_bottoms(spatial)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            let keep = |id: NodeId| models.contains(&id);
            let mut marks = sc_mesh_spatial::WalkMarks::default();
            let fallback = self.born_tip_mm().max(0.5);
            hints.left.clear();
            for island in islands {
                let painted = paint(spatial, island, &keep, &mut marks);
                if held(island, &painted, contacts, fallback) {
                    continue;
                }
                hints.left.push(island.at);
                let visited = hints.visited == Some(island.at);
                let small = match painted {
                    Paint::Faces { corners, area_mm2 } => {
                        if visited {
                            visited_faces.extend(corners);
                        } else {
                            faces.extend(corners);
                        }
                        area_mm2 < DOT_UNDER_MM2
                    }
                    Paint::Waiting(leaf) => {
                        waiting.push(leaf);
                        true
                    }
                    Paint::Nowhere => true,
                };
                if small {
                    if visited {
                        visited_dot.push(island);
                    } else {
                        dotted.push(island);
                    }
                }
            }
        });
        for leaf in &waiting {
            if hints.priming.contains(leaf) {
                continue;
            }
            hints.priming.push(*leaf);
            if let Some(job) = self
                .document
                .with_scene(|spatial| spatial.adjacency_job(*leaf))
                .flatten()
            {
                std::thread::spawn(move || job.run());
            }
        }
        let painted = |corners: Vec<[f32; 3]>, tint: OverlayTint| {
            let indices = (0..corners.len() as u32).collect();
            Arc::new(OverlayMesh {
                positions: corners,
                indices,
                tint,
            })
        };
        let through: Vec<[f32; 3]> = faces.iter().chain(&visited_faces).copied().collect();
        if !through.is_empty() {
            meshes.push(painted(through, OverlayTint::Island));
        }
        if !faces.is_empty() {
            meshes.push(painted(faces, OverlayTint::IslandFace));
        }
        if !visited_faces.is_empty() {
            meshes.push(painted(visited_faces, OverlayTint::IslandVisited));
        }
        let radius = self.born_tip_mm() * ISLAND_DOT_PER_TIP;
        if !dotted.is_empty() {
            meshes.push(Arc::new(dots(&dotted, radius, OverlayTint::IslandDot)));
        }
        if !visited_dot.is_empty() {
            meshes.push(Arc::new(dots(
                &visited_dot,
                radius * VISITED_DOT_SCALE,
                OverlayTint::IslandDot,
            )));
        }
        (hints.left.len(), !waiting.is_empty())
    }

    /// The recommended places for the models as they stand, once worked out;
    /// starts the run that works them out when it is time.
    fn ready_hints(
        &self,
        ui: &MainWindow,
        hints: &mut crate::support_hints::Hints,
    ) -> Option<Arc<Vec<Hint>>> {
        let request = self.request_for_selection();
        let key = self.hints_key(&request)?;
        let ready = hints
            .ready
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, h)| h.clone());
        if ready.is_some() {
            return ready;
        }
        let running = hints.running.as_ref().is_some_and(|(_, k)| *k == key);
        if !running {
            if let Some((job, _)) = hints.running.take() {
                job.cancel();
            }
            let idle = self.handle.borrow().is_none() && !ui.get_support_gen_in_progress();
            if idle
                && self.last_change.get().elapsed() >= SPECULATE_SETTLE
                && let Some(scene) = self.run_scene(None)
                && let Ok(job) = start_supports(&scene, request)
            {
                crate::breadcrumbs::note("supports: hints started");
                hints.running = Some((job, key));
            }
        }
        None
    }

    /// The islands an analysis found (#525), at the places their tips go:
    /// marked in the Supports tab while the models stand as they do now.
    /// `None` takes them away.
    pub(crate) fn set_island_marks(&self, places: Option<Vec<Hint>>) {
        let key = self.models_key(None);
        let mut hints = self.hints.borrow_mut();
        hints.islands = places.and_then(|places| Some((key?, Arc::new(places))));
        hints.shown = None;
    }

    /// The islands left as last marked (#538).
    #[cfg(test)]
    pub(crate) fn islands_left_for_test(&self) -> Vec<[f64; 3]> {
        self.hints.borrow().left.clone()
    }

    /// Goes to the next island left (#538), in turn: the camera centres on
    /// it, looking up at it from below, and its ring stands out.
    pub(crate) fn go_to_island(&self, ui: &MainWindow) {
        let next = {
            let mut hints = self.hints.borrow_mut();
            let next = crate::support_hints::next_island(&hints.left, hints.visited);
            hints.visited = next;
            hints.shown = None;
            next
        };
        let Some(at) = next else {
            return;
        };
        self.support_hints(ui);
        self.ctrl.look_at_island(
            glam::Vec3::from(at.map(|c| c as f32)),
            ISLAND_VIEW_RADIUS_MM,
        );
    }

    /// What the recommended places depend on: the request (preset, printer,
    /// models) and every model it covers as it stands: its mesh and pose.
    /// Supports placed by hand are not in it.
    fn hints_key(&self, request: &SupportRequest) -> Option<String> {
        let poses = self.models_key(request.models.as_deref())?;
        Some(format!("{}|{poses}", ahead_key(request, false)))
    }

    /// Every model of the scene, or of `only`, as it stands: its mesh and
    /// pose. `None` with no model.
    fn models_key(&self, only: Option<&[NodeId]>) -> Option<String> {
        let poses = self.document.with_scene(|spatial| {
            crate::raft_clash::model_bottoms(spatial)
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| only.is_none_or(|m| m.contains(id)))
                .filter_map(|id| {
                    let query = spatial.leaf(id)?;
                    let mesh = spatial.graph().leaf_mesh_arc(id)?;
                    Some(format!(
                        "{id:?}:{:?}:{:?}",
                        Arc::as_ptr(mesh),
                        query.world_transform()
                    ))
                })
                .collect::<Vec<_>>()
                .join(";")
        })?;
        (!poses.is_empty()).then_some(poses)
    }

    /// A Generate press finds the supports generated ahead: pasted at once
    /// when ready for this scene and request, or the run in flight is taken
    /// over (its progress shows). Returns whether it took them.
    fn take_ahead(self: &Rc<Self>, ui: &MainWindow) -> bool {
        let key = ahead_key(
            &self.request_for_selection(),
            self.bus.snapshot().lift_models_on_plate,
        );
        let mut ahead = self.ahead.borrow_mut();
        if let Some((revision, k, _, _)) = &ahead.ready
            && *k == key
            && !self.document.slice_geometry_changed(*revision)
        {
            let (revision, _, outcome, lift) = ahead.ready.take().expect("checked above");
            drop(ahead);
            crate::breadcrumbs::note("supports: generate used the supports made ahead");
            *self.started_on.borrow_mut() = Some(revision);
            *self.plate_lift.borrow_mut() = lift;
            self.cancelled.set(false);
            self.finish(Ok(outcome));
            return true;
        }
        let matches = ahead.running.as_ref().is_some_and(|(_, revision, k, _)| {
            *k == key && !self.document.slice_geometry_changed(*revision)
        });
        if !matches {
            return false;
        }
        let (job, revision, _, lift) = ahead.running.take().expect("checked above");
        drop(ahead);
        crate::breadcrumbs::note("supports: generate took over the run made ahead");
        ui.set_support_gen_in_progress(true);
        ui.set_support_gen_cancelable(true);
        ui.set_support_gen_progress(0.0);
        ui.set_support_gen_phase(slint::SharedString::default());
        *self.handle.borrow_mut() = Some(job);
        *self.started_on.borrow_mut() = Some(revision);
        *self.plate_lift.borrow_mut() = lift;
        self.cancelled.set(false);
        self.start_poll();
        true
    }

    /// The request for the printer and resin in force: the layer height the
    /// slice will use, and the printer's pixel pitch, which sets how far the
    /// contact tips sink into the model. The defaults (0.05 mm, tips at the
    /// surface) when no printer or resin is in force or a profile fails to
    /// load.
    fn request_for_selection(&self) -> SupportRequest {
        let mut request = self.request_for_printer();
        request.options = self.options.borrow().clone();
        request.models = self.target_models();
        // Each model the preset of its height (#723).
        if self.by_height.get() {
            let (options, picks) = self.options_by_height(request.models.as_deref());
            request.model_options = options;
            *self.height_picks.borrow_mut() = picks;
        } else {
            *self.height_picks.borrow_mut() = None;
        }
        // No tip in a hollow's drain channel (#465).
        request.keep_out = crate::raft_drains::drain_channels(&self.document);
        // A model on a raft of its own keeps it (#198), a group's members
        // stand on one raft (#475).
        let mut own: Vec<NodeId> = self.own_rafts.borrow().iter().map(|(m, _)| *m).collect();
        own.extend(self.document.groups().into_iter().flatten());
        own.sort_unstable();
        own.dedup();
        request.own_rafts = own
            .into_iter()
            .map(|model| (model, self.raft_of(model).on(&request.options)))
            .collect();
        // A group is made as one structure, braced to itself (#475).
        request.together = self.document.groups();
        request
    }

    /// The box turned on or off (#723), remembered.
    fn set_by_height(&self, on: bool) {
        if self.by_height.replace(on) != on {
            save_by_height(on);
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_by_height(on);
        }
    }

    /// The options each model of `only` (every model when `None`) is made
    /// with when it takes the zone preset of its height (#723), and the line
    /// that says which preset each took. A model as tall as the preset it
    /// takes keeps the tab's one raft for all.
    fn options_by_height(
        &self,
        only: Option<&[NodeId]>,
    ) -> (Vec<(NodeId, SupportOptions)>, Option<String>) {
        let heights: Vec<(NodeId, f64)> = self
            .document
            .with_scene(|spatial| {
                crate::raft_clash::model_bottoms(spatial)
                    .into_iter()
                    .filter(|(id, _)| only.is_none_or(|only| only.contains(id)))
                    .filter_map(|(id, _)| {
                        let query = spatial.leaf(id)?;
                        let aabb = query.transformed_aabb(query.world_transform())?;
                        let height = f64::from(aabb.max().coords()[2] - aabb.min().coords()[2]);
                        Some((id, height))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let presets = self.presets.borrow();
        let one_raft = self.options.borrow().one_raft;
        let mut counts = [0_usize; 3];
        let options = heights
            .into_iter()
            .filter_map(|(id, height)| {
                let i = preset_for_height(height);
                let preset = presets.get(i).filter(|p| p.built_in)?;
                counts[i] += 1;
                let mut options = preset.options.clone();
                options.one_raft = one_raft;
                Some((id, options))
            })
            .collect();
        let line = counts
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .filter_map(|(i, n)| presets.get(i).map(|p| format!("{} × {n}", p.name)))
            .collect::<Vec<_>>()
            .join(", ");
        (options, (!line.is_empty()).then_some(line))
    }

    /// The raft `model` stands on: its group's one raft of lines (#475), its
    /// own (#198), or the tab's.
    fn raft_of(&self, model: NodeId) -> OwnRaft {
        let own = self
            .own_rafts
            .borrow()
            .iter()
            .find(|(m, _)| *m == model)
            .map(|(_, own)| *own)
            .unwrap_or_else(|| OwnRaft::of(&self.options.borrow()));
        if self.document.group_of(model).is_some() {
            OwnRaft {
                raft: RaftMode::Lines,
                apart: true,
                ..own
            }
        } else {
            own
        }
    }

    /// Shows the raft of the picked model (#198), or the tab's.
    fn push_raft(&self, ui: &MainWindow) {
        let own = self
            .target_models()
            .and_then(|t| t.first().copied())
            .map(|model| self.raft_of(model))
            .unwrap_or_else(|| OwnRaft::of(&self.options.borrow()));
        let tab = self.options.borrow().clone();
        ui.set_support_raft_index(match own.on(&tab).raft_drawn() {
            RaftMode::Solid => 0,
            RaftMode::Lines => 1,
            RaftMode::Off => 2,
        });
        ui.set_support_raft_thickness(own.thickness_mm as f32);
        ui.set_support_raft_wall(own.wall);
        ui.set_support_raft_pads(own.pads);
        ui.set_support_raft_holes(own.holes_mm as f32);
        ui.set_support_raft_tabs(own.tabs);
    }

    /// What a model with no supports yet is measured with by Fill bed and
    /// Arrange (#556): the request a Generate press makes for it, the lift
    /// it gets resting on the plate, and the top of one raft for all.
    pub(crate) fn forecast_ctx(&self) -> crate::support_forecast::ForecastCtx {
        let mut request = self.request_for_printer();
        request.options = self.options.borrow().clone();
        let lift_to_mm = self
            .bus
            .snapshot()
            .lift_models_on_plate
            .then(|| support_lift_mm(&request.options, request.layer_height_mm));
        let raft_top_mm = if request.options.one_raft {
            request.options.raft_top_mm().unwrap_or(0.0)
        } else {
            0.0
        };
        crate::support_forecast::ForecastCtx {
            request,
            lift_to_mm,
            raft_top_mm,
        }
    }

    /// The models a Generate press supports (#197, #388): every selected
    /// model and every model selected supports stand on, in the scene's
    /// order, each given its own set by the one run. `None`, for every
    /// model, when nothing of that kind is selected or the scene holds only
    /// the one model.
    fn target_models(&self) -> Option<Vec<NodeId>> {
        let selected = self.document.selection_node_ids();
        if selected.is_empty() {
            return None;
        }
        self.document
            .with_scene(|spatial| {
                let models: Vec<NodeId> = crate::raft_clash::model_bottoms(spatial)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                if models.len() < 2 {
                    return None;
                }
                let wanted: Vec<NodeId> = selected
                    .iter()
                    .filter_map(|&id| {
                        if models.contains(&id) {
                            Some(id)
                        } else {
                            model_of(&self.sessions, id)
                        }
                    })
                    .collect();
                let targets: Vec<NodeId> = models
                    .into_iter()
                    .filter(|id| wanted.contains(id))
                    .collect();
                (!targets.is_empty()).then_some(targets)
            })
            .flatten()
    }

    /// The models the press waiting to run supports: the one whose set its
    /// card makes again (#408), else [`Self::target_models`].
    fn press_targets(&self) -> Option<Vec<NodeId>> {
        match self.redo.borrow().as_ref() {
            Some(redo) => Some(vec![redo.model]),
            None => self.target_models(),
        }
    }

    /// The layer height and pixel pitch of the printer and resin in force.
    fn request_for_printer(&self) -> SupportRequest {
        let snapshot = self.bus.snapshot();
        let selection = self
            .document
            .print_selection()
            .in_force(&PrintDefaults::from(&snapshot));
        let (Some(printer), Some(resin)) =
            (selection.printer().in_force(), selection.resin().in_force())
        else {
            return SupportRequest::default();
        };
        let Ok((printer, resin)) = load_selected_profiles(printer, resin) else {
            return SupportRequest::default();
        };
        let defaults = SupportRequest::default();
        let layer_height_mm = resolve_layer_height(&printer, &resin, None)
            .map_or(defaults.layer_height_mm, |h| h.value());
        let pitch = printer.hardware.pixel_size_microns();
        SupportRequest {
            layer_height_mm,
            pixel_pitch_mm: Some([pitch.x.value() / 1000.0, pitch.y.value() / 1000.0]),
            ..defaults
        }
    }

    /// Pushes the options and the preset list to the Supports tab.
    fn push_options(&self, ui: &MainWindow) {
        let full = self.options.borrow().clone();
        // One raft for all is how the plate prints, not part of a preset
        // (#556): the presets are told apart without it.
        let o = preset_part(&full);
        let presets = self.presets.borrow();
        let names: Vec<slint::SharedString> = presets
            .iter()
            .map(|p| slint::SharedString::from(p.name.as_str()))
            .collect();
        let current = self
            .picked
            .get()
            .filter(|&i| presets.get(i).is_some_and(|p| p.options == o))
            .or_else(|| presets.iter().position(|p| p.options == o))
            .map_or(-1, |i| i as i32);
        ui.set_support_preset_names(slint::ModelRc::new(slint::VecModel::from(names)));
        ui.set_support_preset_index(current);
        self.push_overhang_limit(ui);
        ui.set_support_bracing(o.bracing);
        self.push_raft(ui);
        ui.set_arrange_one_raft(full.one_raft);
        ui.set_support_zone_summary(zone_summary(&o));
        ui.set_support_grid_on(o.zones.as_ref().is_some_and(|z| z.grid_mm.is_some()));
        ui.set_support_plate_only(o.zones.as_ref().is_some_and(|z| z.rules.plate_only));
        ui.set_support_mini_supports(o.zones.as_ref().is_none_or(|z| !z.rules.keep_long_island_branches));
        ui.set_support_adaptive_base(o.zones.as_ref().is_some_and(|z| z.rules.adaptive_base));
        ui.set_support_braces_follow(o.zones.as_ref().is_some_and(|z| z.rules.braces_follow_trunks));
        ui.set_support_grid_spacing(shown_grid_mm(&o) as f32);
        ui.set_support_grid_hint(grid_hint(&o));
        ui.set_support_lift_on_plate_label(lift_on_plate_label(support_lift_mm(
            &o,
            self.raft_layer_height_mm.get(),
        )));
        let built_in = presets.iter().take_while(|p| p.built_in).count();
        let card = self
            .editing
            .get()
            .filter(|&i| presets.get(i).is_some_and(|p| p.options == o))
            .or_else(|| usize::try_from(current).ok().filter(|&i| i < built_in));
        let current = card.map_or(current, |i| i as i32);
        let changed = card.is_some_and(|i| presets[i].factory.as_ref() != Some(&o));
        if card.is_some() {
            self.based_on.set(card);
        } else if self.based_on.get().is_none()
            && let Some(zones) = o.zones.as_ref()
        {
            // Not known (the app opened with a preset changed before):
            // the built-in one it shares the most with.
            let bases: Vec<(usize, &ZonePreset)> = presets[..built_in]
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.factory.as_ref()?.zones.as_ref().map(|z| (i, z)))
                .collect();
            let refs: Vec<&ZonePreset> = bases.iter().map(|(_, z)| *z).collect();
            self.based_on
                .set(crate::support_shape_edit::closest_base(&refs, zones).map(|k| bases[k].0));
        }
        let own: Vec<slint::SharedString> = presets[built_in..]
            .iter()
            .map(|p| p.name.as_str().into())
            .collect();
        ui.set_support_own_presets(slint::ModelRc::new(slint::VecModel::from(own)));
        ui.set_support_own_preset_index(
            usize::try_from(current)
                .ok()
                .and_then(|i| i.checked_sub(built_in))
                .map_or(-1, |i| i as i32),
        );
        let cards: Vec<crate::PresetCard> = preset_cards()
            .into_iter()
            .map(|(title, range)| crate::PresetCard {
                title,
                range: range.into(),
            })
            .collect();
        ui.set_support_preset_cards(slint::ModelRc::new(slint::VecModel::from(cards)));
        ui.set_support_preset_card_index(card.map_or(-1, |i| i as i32));
        // What the options in use are, on one line that is always there
        // (the panel never moves when it changes), with the way back to
        // the preset as it ships when they are not that.
        let title_of = |i: usize| preset_cards().get(i).map(|(t, _)| t.clone());
        let based_on = match (card, usize::try_from(current).ok(), self.based_on.get()) {
            (Some(i), _, _) if changed => title_of(i)
                .map(|t| ctr!("Inspector", "{}, with your changes", t))
                .unwrap_or_default(),
            (Some(i), _, _) => title_of(i)
                .map(|t| ctr!("Inspector", "{}, as it ships", t))
                .unwrap_or_default(),
            (None, Some(c), Some(i)) => title_of(i)
                .map(|t| {
                    ctr!(
                        "Inspector",
                        "Your preset {}, made from {}",
                        presets[c].name,
                        t
                    )
                })
                .unwrap_or_default(),
            (None, Some(c), None) => ctr!("Inspector", "Your preset: {}", presets[c].name),
            (None, None, Some(i)) => title_of(i)
                .map(|t| ctr!("Inspector", "Made from {}, with changes", t))
                .unwrap_or_default(),
            (None, None, None) => ctr!("Inspector", "Custom preset"),
        };
        ui.set_support_based_on(based_on);
        ui.set_support_preset_resettable(
            !(card.is_some() && !changed) && (card.is_some() || self.based_on.get().is_some()),
        );
        let scope = match (card, usize::try_from(current).ok()) {
            (Some(i), _) => ctr!("Inspector", "Preset {}", preset_cards()[i].0),
            (None, Some(i)) => ctr!("Inspector", "Preset {}", presets[i].name),
            (None, None) => ctr!("Inspector", "Custom preset"),
        };
        ui.set_support_shape_scope(scope);
        drop(presets);
        self.push_shape(ui, &o);
    }

    /// The size the shape panel shows, an index into the preset's sizes:
    /// the one picked while the preset has it, else the small one.
    fn shape_size_of(&self, zones: &ZonePreset) -> Option<usize> {
        let shown = crate::support_shape_edit::shown_sizes(zones);
        self.shape_size
            .get()
            .filter(|i| shown.contains(i))
            .or_else(|| {
                shown
                    .iter()
                    .copied()
                    .find(|&i| zones.rules.sizes[i].id == "small")
            })
            .or_else(|| shown.first().copied())
    }

    /// Export asked (#564): lists the person's presets, the settings in use
    /// when they are no preset, and the hand sizes, all ticked.
    fn on_share_export(&self) {
        use crate::support_share::{Entry, SharedPreset};
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let options = self.options.borrow().clone();
        let presets = self.presets.borrow();
        let titles = preset_cards();
        let mut entries: Vec<(Entry, bool)> = presets
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let name = match (p.built_in, titles.get(i)) {
                    (true, Some((title, _))) => title.to_string(),
                    _ => p.name.clone(),
                };
                let preset = SharedPreset {
                    name,
                    options: p.options.clone(),
                };
                (Entry::Preset(Box::new(preset)), true)
            })
            .collect();
        if !presets.iter().any(|p| p.options == options) {
            let preset = SharedPreset {
                name: ctr!("Inspector", "Current settings").to_string(),
                options,
            };
            entries.insert(0, (Entry::Preset(Box::new(preset)), true));
        }
        entries.extend(
            self.hand
                .borrow()
                .rules
                .sizes
                .iter()
                .map(|size| (Entry::HandSize(size.clone()), true)),
        );
        drop(presets);
        *self.share.borrow_mut() = entries;
        self.push_share(&ui, 1);
    }

    /// Import asked: the file is picked, and what it holds is listed to
    /// tick.
    fn on_share_import(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let dialog = crate::error_dialog::file_dialog_over(ui.window())
            .set_title(ctr!("FileDialog", "Import support settings").to_string())
            .add_filter(
                ctr!("FileDialog", "Breadslicer support settings").to_string(),
                &[crate::support_share::EXTENSION],
            );
        let weak = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            let Some(picked) = dialog.pick_file().await else {
                return;
            };
            let Some(run) = weak.upgrade() else {
                return;
            };
            match std::fs::read_to_string(picked.path()) {
                Ok(text) => run.offer_import(&text),
                Err(_) => run.show_status(ctr!("Inspector", "That file could not be read.")),
            }
        }) {
            eprintln!("Failed to spawn the support settings dialog task: {err}");
        }
    }

    /// Lists what the support settings file of text `text` holds, to tick,
    /// or says why it holds nothing to take.
    fn offer_import(&self, text: &str) {
        use crate::support_share::ReadError;
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        match crate::support_share::read(text) {
            Ok(entries) => {
                *self.share.borrow_mut() = entries.into_iter().map(|e| (e, true)).collect();
                self.push_share(&ui, 2);
            }
            Err(ReadError::Newer) => self.show_status(ctr!(
                "Inspector",
                "That file was made by a newer Breadslicer."
            )),
            Err(ReadError::Empty) => {
                self.show_status(ctr!("Inspector", "That file holds no support settings."));
            }
            Err(ReadError::NotOne) => self.show_status(ctr!(
                "Inspector",
                "That is not a Breadslicer support settings file."
            )),
        }
    }

    /// The rows of the list being asked: per group (0 the presets of
    /// Generate, 1 the hand sizes), its head, then its entries, by index
    /// into the list.
    fn share_rows(&self) -> Vec<(usize, Option<usize>)> {
        use crate::support_share::Entry;
        let share = self.share.borrow();
        let group = |e: &Entry| usize::from(matches!(e, Entry::HandSize(_)));
        let mut rows = Vec::new();
        for g in 0..2 {
            rows.push((g, None));
            rows.extend(
                share
                    .iter()
                    .enumerate()
                    .filter(|(_, (e, _))| group(e) == g)
                    .map(|(i, _)| (g, Some(i))),
            );
        }
        rows
    }

    /// Shows the list of the export (1) or import (2) being asked, as a
    /// tree: each group's head ticks all of it, each entry itself.
    fn push_share(&self, ui: &MainWindow, mode: i32) {
        use crate::support_share::Entry;
        // The presets first, the hand sizes after them from XS to L.
        let order = crate::support_shape_edit::hand_factory();
        let hand_rank = |id: &str| {
            order
                .rules
                .sizes
                .iter()
                .position(|s| s.id == id)
                .unwrap_or(usize::MAX)
        };
        self.share
            .borrow_mut()
            .sort_by_key(|(entry, _)| match entry {
                Entry::Preset(_) => (0, 0),
                Entry::HandSize(s) => (1, hand_rank(&s.id)),
            });
        let rows = self.share_rows();
        let share = self.share.borrow();
        let presets = self.presets.borrow();
        let bases: Vec<(String, &ZonePreset)> = presets
            .iter()
            .filter(|p| p.built_in)
            .zip(preset_cards())
            .filter_map(|(p, (title, _))| {
                Some((title.to_string(), p.factory.as_ref()?.zones.as_ref()?))
            })
            .collect();
        let refs: Vec<&ZonePreset> = bases.iter().map(|(_, z)| *z).collect();
        let in_group = |g: usize| {
            rows.iter()
                .filter(|(rg, e)| *rg == g && e.is_some())
                .count()
        };
        let ticked = |g: usize| {
            rows.iter()
                .filter(|(rg, e)| *rg == g && e.is_some_and(|i| share[i].1))
                .count()
        };
        let none: slint::ModelRc<f32> = slint::ModelRc::default();
        let items: Vec<crate::ShareItem> = rows
            .iter()
            .enumerate()
            .map(|(k, &(g, entry))| {
                let last = rows.get(k + 1).is_none_or(|(next, e)| *next != g || e.is_none());
                let Some(i) = entry else {
                    let (n, on) = (in_group(g), ticked(g));
                    let name = if g == 0 {
                        ctr!("Inspector", "Automatic presets")
                    } else {
                        ctr!("Inspector", "Manual supports")
                    };
                    let detail = match (n, g, mode) {
                        (0, 0, 1) => ctr!(
                            "Inspector",
                            "You have no presets of your own yet: change one and save it in PRESETS to share it."
                        ),
                        (0, 0, _) => ctr!("Inspector", "The file brings no presets."),
                        (0, _, _) => ctr!("Inspector", "The file brings no hand sizes."),
                        _ => ctr!("Inspector", "{} of {} ticked", on, n),
                    };
                    return crate::ShareItem {
                        header: true,
                        group: g as i32,
                        name,
                        detail,
                        numbers: none.clone(),
                        state: if n > 0 && on == n { 2 } else if on > 0 { 1 } else { 0 },
                        last: n == 0,
                        enabled: n > 0,
                    };
                };
                let (entry, on) = &share[i];
                let (name, detail, numbers) = match entry {
                    Entry::Preset(p) if mode == 1
                        && presets.iter().zip(preset_cards()).any(|(b, (t, _))| {
                            b.built_in && t.as_str() == p.name
                        }) =>
                    {
                        let factory = presets
                            .iter()
                            .zip(preset_cards())
                            .find(|(b, (t, _))| b.built_in && t.as_str() == p.name)
                            .and_then(|(b, _)| b.factory.as_ref());
                        (
                            p.name.as_str().into(),
                            if factory == Some(&p.options) {
                                ctr!("Inspector", "Built-in, as it ships")
                            } else {
                                ctr!("Inspector", "Built-in, with your changes")
                            },
                            none.clone(),
                        )
                    }
                    Entry::Preset(p) => (
                        p.name.as_str().into(),
                        p.options
                            .zones
                            .as_ref()
                            .and_then(|z| crate::support_shape_edit::closest_base(&refs, z))
                            .map_or_else(
                                || ctr!("Inspector", "Preset of Generate"),
                                |k| ctr!("Inspector", "Made from {}", bases[k].0),
                            ),
                        none.clone(),
                    ),
                    Entry::HandSize(size) => (
                        crate::support_shape_edit::short_name(&size.id).into(),
                        Default::default(),
                        slint::ModelRc::new(slint::VecModel::from(vec![
                            size.tip_diameter_mm as f32,
                            size.trunk_diameter_mm as f32,
                            size.base_diameter_mm as f32,
                        ])),
                    ),
                };
                crate::ShareItem {
                    header: false,
                    group: g as i32,
                    name,
                    detail,
                    numbers,
                    state: if *on { 2 } else { 0 },
                    last,
                    enabled: true,
                }
            })
            .collect();
        ui.set_support_share_picked(share.iter().filter(|(_, on)| *on).count() as i32);
        ui.set_support_share_total(share.len() as i32);
        ui.set_support_share_items(slint::ModelRc::new(slint::VecModel::from(items)));
        ui.set_support_share_mode(mode);
    }

    /// Row `row` of the list ticked or not: a group's head ticks all of
    /// the group, an entry itself.
    fn on_share_toggled(&self, row: i32, on: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let rows = self.share_rows();
        let Some(&(group, entry)) = usize::try_from(row).ok().and_then(|r| rows.get(r)) else {
            return;
        };
        {
            let mut share = self.share.borrow_mut();
            match entry {
                Some(i) => share[i].1 = on,
                None => {
                    for &(_, e) in rows.iter().filter(|(g, _)| *g == group) {
                        if let Some(i) = e {
                            share[i].1 = on;
                        }
                    }
                }
            }
        }
        self.push_share(&ui, ui.get_support_share_mode());
    }

    /// Puts the list away, nothing taken.
    fn close_share(&self) {
        self.share.borrow_mut().clear();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_share_mode(0);
        }
    }

    /// The list was answered: the ticked entries go to a file (export) or
    /// come in (import).
    fn on_share_accepted(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let mode = ui.get_support_share_mode();
        let picked: Vec<crate::support_share::Entry> = self
            .share
            .borrow()
            .iter()
            .filter(|(_, on)| *on)
            .map(|(entry, _)| entry.clone())
            .collect();
        self.close_share();
        if picked.is_empty() {
            return;
        }
        if mode == 2 {
            self.take_shared(picked);
            return;
        }
        let Some(text) = crate::support_share::write(&picked) else {
            return;
        };
        let dialog = crate::error_dialog::file_dialog_over(ui.window())
            .set_title(ctr!("FileDialog", "Export support settings").to_string())
            .set_file_name(format!("supports.{}", crate::support_share::EXTENSION))
            .add_filter(
                ctr!("FileDialog", "Breadslicer support settings").to_string(),
                &[crate::support_share::EXTENSION],
            );
        let weak = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            let Some(file) = dialog.save_file().await else {
                return;
            };
            let written = std::fs::write(file.path(), text).is_ok();
            if let Some(run) = weak.upgrade() {
                run.show_status(if written {
                    ctr!("Inspector", "Support settings exported.")
                } else {
                    ctr!("Inspector", "The file could not be written.")
                });
            }
        }) {
            eprintln!("Failed to spawn the support settings dialog task: {err}");
        }
    }

    /// Takes `picked` from a file: each preset is saved as one of the
    /// person's own, under a name none of theirs has, and the hand sizes
    /// replace theirs.
    fn take_shared(self: &Rc<Self>, picked: Vec<crate::support_share::Entry>) {
        use crate::support_share::Entry;
        let mut taken: Vec<String> = self
            .presets
            .borrow()
            .iter()
            .map(|p| p.name.clone())
            .chain(preset_cards().into_iter().map(|(t, _)| t.to_string()))
            .collect();
        let dir = support_data_dir().map(|d| d.join("presets"));
        let mut presets = 0;
        let mut sizes = Vec::new();
        for entry in picked {
            match entry {
                Entry::Preset(p) => {
                    let Some(stem) = preset_file_stem(&p.name) else {
                        continue;
                    };
                    let name = crate::support_share::free_name(&stem, &taken);
                    if let Some(dir) = dir.as_ref()
                        && std::fs::create_dir_all(dir).is_ok()
                        && let Ok(text) = toml::to_string_pretty(&p.options)
                    {
                        let _ = std::fs::write(dir.join(format!("{name}.toml")), text);
                    }
                    taken.push(name);
                    presets += 1;
                }
                Entry::HandSize(size) => sizes.push(size),
            }
        }
        // The sizes taken replace those of the same name; the others stay.
        let hand = !sizes.is_empty();
        if hand {
            let mut merged = self.hand.borrow().rules.sizes.clone();
            for size in sizes {
                match merged.iter_mut().find(|s| s.id == size.id) {
                    Some(slot) => *slot = size,
                    None => merged.push(size),
                }
            }
            self.set_hand(crate::support_shape_edit::hand_from_saved(merged));
        }
        self.reload_presets();
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
        self.show_status(match (presets, hand) {
            (0, _) => ctr!("Inspector", "Hand sizes imported."),
            (n, false) => ctr!(
                "Inspector",
                "Imported {n} preset.",
                "Imported {n} presets.",
                n
            ),
            (n, true) => ctr!(
                "Inspector",
                "Imported {n} preset and the hand sizes.",
                "Imported {n} presets and the hand sizes.",
                n
            ),
        });
    }

    /// Whether the shape panel shows the hand sizes (the Manual page) and
    /// not the preset of Generate.
    fn on_hand_page(ui: &MainWindow) -> bool {
        ui.get_support_page() == 1
    }

    /// The hand size the Add tool places, an index into the hand sizes:
    /// the one keys 1 to 4 picked, the small one with none picked.
    fn hand_size_index(&self, ui: &MainWindow) -> Option<usize> {
        let hand = self.hand.borrow();
        let id = hand_tip_size(ui.get_support_tip_size()).unwrap_or_else(|| "small".to_owned());
        hand.rules.sizes.iter().position(|s| s.id == id)
    }

    /// Replaces the hand sizes, saves them and shows them.
    fn set_hand(self: &Rc<Self>, hand: ZonePreset) {
        save_hand_sizes(&hand);
        *self.hand.borrow_mut() = hand;
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
            self.refresh_tip_label(&ui, None);
        }
        if let Some((point, shift)) = self.hover_at.get() {
            self.on_tool_hover_with(Some(point), shift);
        }
    }

    /// Pushes the shape panel (#564): the sizes, the numbers of the one
    /// shown, where each size goes, and the drawing. On the Manual page the
    /// sizes are the hand tools'.
    fn push_shape(&self, ui: &MainWindow, o: &SupportOptions) {
        use crate::support_shape_edit as edit;
        let manual = Self::on_hand_page(ui);
        let hand = self.hand.borrow().clone();
        ui.set_support_hand_changed(hand != edit::hand_factory());
        if manual && let Some(shapes) = self.chosen_shapes(ui) {
            self.push_chosen_shape(ui, &hand, &shapes);
            return;
        }
        let (zones, size) = if manual {
            (&hand, self.hand_size_index(ui))
        } else {
            let Some(zones) = o.zones.as_ref() else {
                return;
            };
            (zones, self.shape_size_of(zones))
        };
        if manual {
            ui.set_support_shape_scope(ctr!("Inspector", "Hand supports"));
        }
        let shown = edit::shown_sizes(zones);
        let Some(size) = size else {
            return;
        };
        let names: Vec<String> = shown
            .iter()
            .map(|&i| edit::short_name(&zones.rules.sizes[i].id))
            .collect();
        let chips: Vec<crate::ShapeSize> = shown
            .iter()
            .zip(&names)
            .map(|(&i, name)| crate::ShapeSize {
                name: name.into(),
                tip: zones.rules.sizes[i].tip_diameter_mm as f32,
                trunk: zones.rules.sizes[i].trunk_diameter_mm as f32,
                length: zones.rules.sizes[i].tip_length_mm as f32,
            })
            .collect();
        ui.set_support_shape_sizes(slint::ModelRc::new(slint::VecModel::from(chips)));
        ui.set_support_shape_size(
            shown
                .iter()
                .position(|&i| i == size)
                .map_or(-1, |i| i as i32),
        );
        ui.set_support_shape_size_name(edit::short_name(&zones.rules.sizes[size].id).into());
        // What each field goes back to: the built-in preset it was made
        // from, for the same size.
        let base = if manual {
            Some(edit::hand_factory())
        } else {
            self.base_zones()
        };
        let defaults: Vec<f32> = match base.as_ref() {
            Some(base) => edit::defaults(base, zones, size)
                .iter()
                .map(|d| d.map_or(-1.0, |v| v as f32))
                .collect(),
            None => vec![-1.0; edit::FIELDS],
        };
        ui.set_support_shape_defaults(slint::ModelRc::new(slint::VecModel::from(defaults)));
        ui.set_support_zone_angle_default(
            base.as_ref()
                .map_or(-1.0, |b| b.rules.overhang.angle_deg as f32),
        );
        let values: Vec<f32> = edit::values(zones, size)
            .iter()
            .map(|&v| v as f32)
            .collect();
        ui.set_support_shape_values(slint::ModelRc::new(slint::VecModel::from(values)));
        if manual {
            self.draw_shape(ui, &zones.rules.sizes[size]);
            return;
        }
        // The whole preset's choices (#363), and what they go back to.
        ui.set_support_shape_choices(slint::ModelRc::new(slint::VecModel::from(
            edit::choices(zones).to_vec(),
        )));
        ui.set_support_shape_choice_defaults(slint::ModelRc::new(slint::VecModel::from(
            base.as_ref()
                .map_or(vec![-1; edit::CHOICES], |b| edit::choices(b).to_vec()),
        )));
        let bracing_names: Vec<slint::SharedString> = edit::bracing_preset_names()
            .into_iter()
            .map(bracing_preset_name)
            .collect();
        ui.set_support_bracing_presets(slint::ModelRc::new(slint::VecModel::from(bracing_names)));
        let size_names: Vec<slint::SharedString> = names.iter().map(|n| n.into()).collect();
        ui.set_support_zone_size_names(slint::ModelRc::new(slint::VecModel::from(size_names)));
        let rows: Vec<crate::ZoneRow> = edit::zones_of(zones)
            .into_iter()
            .filter_map(|zone| {
                let (id, spacing) = edit::zone_size_and_spacing(zones, zone)?;
                let (name, detail) = zone_texts(zones, zone);
                let place = |id: &str| {
                    shown
                        .iter()
                        .position(|&i| zones.rules.sizes[i].id == id)
                        .map_or(-1, |i| i as i32)
                };
                let built_in = base
                    .as_ref()
                    .and_then(|b| edit::zone_size_and_spacing(b, zone));
                Some(crate::ZoneRow {
                    name,
                    detail,
                    size: place(id),
                    spacing: spacing as f32,
                    default_size: built_in.map_or(-1, |(id, _)| place(id)),
                    default_spacing: built_in.map_or(-1.0, |(_, mm)| mm as f32),
                })
            })
            .collect();
        ui.set_support_zone_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
        ui.set_support_zone_angle(zones.rules.overhang.angle_deg as f32);
        self.draw_shape(ui, &zones.rules.sizes[size]);
    }

    /// The shape panel for the picked supports (#567): the numbers they
    /// share, "various" (below 0) where they differ, the hand size chip
    /// they all match, and the first one drawn.
    fn push_chosen_shape(
        &self,
        ui: &MainWindow,
        hand: &ZonePreset,
        shapes: &[sc_geom::supports::SupportSize],
    ) {
        use crate::support_shape_edit as edit;
        let numbers = |shape: &sc_geom::supports::SupportSize| {
            let mut one = hand.clone();
            one.rules.sizes = vec![shape.clone()];
            edit::values(&one, 0)
        };
        let first = numbers(&shapes[0]);
        let mut values: Vec<f32> = first.iter().map(|&v| v as f32).collect();
        for shape in &shapes[1..] {
            for (k, v) in numbers(shape).iter().enumerate().take(9) {
                if (v - first[k]).abs() > 1e-6 {
                    values[k] = -1.0;
                }
            }
        }
        let shown = edit::shown_sizes(hand);
        let chips: Vec<crate::ShapeSize> = shown
            .iter()
            .map(|&i| crate::ShapeSize {
                name: edit::short_name(&hand.rules.sizes[i].id).into(),
                tip: hand.rules.sizes[i].tip_diameter_mm as f32,
                trunk: hand.rules.sizes[i].trunk_diameter_mm as f32,
                length: hand.rules.sizes[i].tip_length_mm as f32,
            })
            .collect();
        ui.set_support_shape_sizes(slint::ModelRc::new(slint::VecModel::from(chips)));
        let matched = shown.iter().position(|&i| {
            let size = edit::values(hand, i);
            shapes
                .iter()
                .all(|s| (0..9).all(|k| (numbers(s)[k] - size[k]).abs() < 1e-6))
        });
        ui.set_support_shape_size(matched.map_or(-1, |i| i as i32));
        ui.set_support_shape_scope(ctr!("Inspector", "Picked supports"));
        ui.set_support_shape_size_name(match matched {
            Some(i) => edit::short_name(&hand.rules.sizes[shown[i]].id).into(),
            None => ctr!("Inspector", "Picked"),
        });
        ui.set_support_shape_defaults(slint::ModelRc::new(slint::VecModel::from(vec![
            -1.0f32;
            edit::FIELDS
        ])));
        ui.set_support_shape_values(slint::ModelRc::new(slint::VecModel::from(values)));
        self.draw_shape(ui, &shapes[0]);
    }

    /// The zones of the built-in preset the options were made from: what
    /// each field of the shape panel goes back to.
    fn base_zones(&self) -> Option<ZonePreset> {
        let presets = self.presets.borrow();
        presets
            .get(self.based_on.get()?)
            .filter(|p| p.built_in)
            .and_then(|p| p.factory.as_ref()?.zones.clone())
    }

    /// Draws the shape panel's support again: after a turn or a part
    /// pointed at, nothing else changed.
    fn redraw_shape(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if Self::on_hand_page(&ui) {
            if let Some(size) = self.hand_size_index(&ui) {
                let shape = self.hand.borrow().rules.sizes[size].clone();
                self.draw_shape(&ui, &shape);
            }
            return;
        }
        let o = self.options.borrow();
        let Some(zones) = o.zones.as_ref() else {
            return;
        };
        if let Some(size) = self.shape_size_of(zones) {
            self.draw_shape(&ui, &zones.rules.sizes[size]);
        }
    }

    /// Draws the shape panel's three drawings of `size`: the support (in
    /// 3D or as a plan) and the lenses on its tip and its foot.
    fn draw_shape(&self, ui: &MainWindow, size: &sc_geom::supports::SupportSize) {
        use crate::support_shape::{
            Drawing, LENS_HEIGHT, LENS_WIDTH, SHAPE_HEIGHT, SHAPE_WIDTH, draw, draw_foot_lens,
            draw_plan, draw_tip_lens,
        };
        let hot = self.shape_hot.get();
        let scale = ui.window().scale_factor();
        // The size as it ships, to show where this one is not it.
        let base = if Self::on_hand_page(ui) {
            Some(crate::support_shape_edit::hand_factory())
        } else {
            self.base_zones()
        };
        let factory = base
            .as_ref()
            .and_then(|b| b.rules.sizes.iter().find(|s| s.id == size.id));
        let main = if self.shape_flat.get() {
            draw_plan(size, factory, SHAPE_WIDTH, SHAPE_HEIGHT, scale, hot)
        } else {
            draw(
                size,
                factory,
                self.shape_view.get(),
                SHAPE_WIDTH,
                SHAPE_HEIGHT,
                scale,
                hot,
            )
        };
        let tip = draw_tip_lens(size, LENS_WIDTH, LENS_HEIGHT, scale, hot);
        let foot = draw_foot_lens(size, LENS_WIDTH, LENS_HEIGHT, scale, hot);
        let show = |drawing: Drawing, texts: &slint::VecModel<crate::ShapeCallout>| {
            use slint::Model;
            let rows: Vec<crate::ShapeCallout> = drawing
                .callouts
                .into_iter()
                .map(|c| crate::ShapeCallout {
                    x: c.x,
                    y: c.y,
                    anchor: c.anchor as i32,
                    part: c.part as i32,
                    chip: c.chip,
                    word: c.word as i32,
                    prefix: c.prefix.into(),
                    value: c.value as f32,
                    field: c.field,
                })
                .collect();
            if texts.row_count() == rows.len() {
                for (i, row) in rows.into_iter().enumerate() {
                    if texts.row_data(i).as_ref() != Some(&row) {
                        texts.set_row_data(i, row);
                    }
                }
            } else {
                texts.set_vec(rows);
            }
            slint::Image::from_rgba8(drawing.pixels)
        };
        ui.set_support_shape_picture(show(main, &self.shape_texts[0]));
        ui.set_support_shape_tip_picture(show(tip, &self.shape_texts[1]));
        ui.set_support_shape_foot_picture(show(foot, &self.shape_texts[2]));
        ui.set_support_shape_hot(hot as i32);
    }

    /// A size chip picked: the panel shows that size.
    fn on_shape_size_picked(self: &Rc<Self>, chip: i32) {
        if let Some(ui) = self.ui.upgrade()
            && Self::on_hand_page(&ui)
        {
            // With supports picked, a size chip draws them all with it.
            if self.chosen_shapes(&ui).is_some() {
                let size = {
                    let hand = self.hand.borrow();
                    crate::support_shape_edit::shown_sizes(&hand)
                        .get(usize::try_from(chip).unwrap_or(usize::MAX))
                        .map(|&i| hand.rules.sizes[i].clone())
                };
                if let Some(size) = size {
                    self.reshape_chosen(&ui, |_| size.clone());
                }
                return;
            }
            // The chips are the Add tool's tip sizes, as keys 1 to 4 are.
            if (0..4).contains(&chip) {
                ui.set_support_tip_size(chip + 1);
                let o = self.options.borrow().clone();
                self.push_shape(&ui, &o);
                self.refresh_tip_label(&ui, None);
            }
            return;
        }
        let picked = {
            let o = self.options.borrow();
            o.zones.as_ref().and_then(|z| {
                crate::support_shape_edit::shown_sizes(z)
                    .get(usize::try_from(chip).ok()?)
                    .copied()
            })
        };
        if picked.is_some() {
            self.shape_size.set(picked);
            if let Some(ui) = self.ui.upgrade() {
                let o = self.options.borrow().clone();
                self.push_shape(&ui, &o);
            }
        }
    }

    /// A number of the shape panel changed: the preset takes it, as any
    /// other option, and the drawing follows.
    fn on_shape_value(self: &Rc<Self>, code: i32, value: f32) {
        if let Some(ui) = self.ui.upgrade()
            && Self::on_hand_page(&ui)
        {
            // With supports picked, the number is theirs (#567), each keeping
            // its other numbers.
            if self.chosen_shapes(&ui).is_some() {
                // The widths of the tip, the trunk and the foot are given in
                // place (#377): the route stays, and a support others hang
                // from takes them too.
                let mm = Some(f64::from(value));
                let sizes = match code {
                    0 => Some(PartSizes {
                        tip_mm: mm,
                        ..PartSizes::default()
                    }),
                    1 => Some(PartSizes {
                        trunk_mm: mm,
                        ..PartSizes::default()
                    }),
                    3 => Some(PartSizes {
                        foot_mm: mm,
                        ..PartSizes::default()
                    }),
                    _ => None,
                };
                if let Some(sizes) = sizes {
                    for (drawn, ids) in self.chosen_now() {
                        self.start_gesture(&ui, drawn, EditOp::Resize(ids, sizes), None);
                    }
                    return;
                }
                if let Ok(code) = usize::try_from(code)
                    && code < 9
                {
                    let hand = self.hand.borrow().clone();
                    self.reshape_chosen(&ui, |shape| {
                        let mut one = hand.clone();
                        one.rules.sizes = vec![shape.clone()];
                        crate::support_shape_edit::set_value(&mut one, 0, code, f64::from(value));
                        one.rules.sizes.remove(0)
                    });
                }
                return;
            }
            let (Some(size), Ok(code)) = (self.hand_size_index(&ui), usize::try_from(code)) else {
                return;
            };
            if code < 9 {
                let mut hand = self.hand.borrow().clone();
                crate::support_shape_edit::set_value(&mut hand, size, code, f64::from(value));
                self.set_hand(hand);
            }
            return;
        }
        let mut o = self.options.borrow().clone();
        let Some(size) = o.zones.as_ref().and_then(|z| self.shape_size_of(z)) else {
            return;
        };
        let (Some(zones), Ok(code)) = (o.zones.as_mut(), usize::try_from(code)) else {
            return;
        };
        crate::support_shape_edit::set_value(zones, size, code, f64::from(value));
        self.set_options(o);
    }

    /// A choice of the whole preset changed (#363): a bracing pattern, the
    /// rings' diagonal, a bracing preset to start from, or ignoring small
    /// islands. The preset takes it, as any other option.
    fn on_shape_choice(&self, code: i32, value: i32) {
        let Ok(code) = usize::try_from(code) else {
            return;
        };
        let mut o = self.options.borrow().clone();
        let Some(zones) = o.zones.as_mut() else {
            return;
        };
        crate::support_shape_edit::set_choice(zones, code, value);
        self.set_options(o);
    }

    /// The zone of row `row` of the shape panel.
    fn zone_at(&self, row: i32) -> Option<crate::support_shape_edit::Zone> {
        let o = self.options.borrow();
        crate::support_shape_edit::zones_of(o.zones.as_ref()?)
            .get(usize::try_from(row).ok()?)
            .copied()
    }

    fn on_zone_size(&self, row: i32, chip: i32) {
        let Some(zone) = self.zone_at(row) else {
            return;
        };
        let mut o = self.options.borrow().clone();
        let Some(zones) = o.zones.as_mut() else {
            return;
        };
        let Some(id) = crate::support_shape_edit::shown_sizes(zones)
            .get(usize::try_from(chip).unwrap_or(usize::MAX))
            .map(|&i| zones.rules.sizes[i].id.clone())
        else {
            return;
        };
        crate::support_shape_edit::set_zone_size(zones, zone, &id);
        self.set_options(o);
    }

    fn on_zone_spacing(&self, row: i32, mm: f32) {
        let Some(zone) = self.zone_at(row) else {
            return;
        };
        let mut o = self.options.borrow().clone();
        if let Some(zones) = o.zones.as_mut() {
            crate::support_shape_edit::set_zone_spacing(zones, zone, f64::from(mm));
            self.set_options(o);
        }
    }

    fn on_zone_angle(&self, degrees: f32) {
        let mut o = self.options.borrow().clone();
        if let Some(zones) = o.zones.as_mut() {
            zones.rules.overhang.angle_deg = f64::from(degrees).clamp(15.0, 70.0);
            self.set_options(o);
        }
    }

    /// Replaces the options, saves them and shows them.
    fn set_options(&self, options: SupportOptions) {
        let options = options.clamped();
        if let Some(dir) = support_data_dir()
            && std::fs::create_dir_all(&dir).is_ok()
            && let Ok(text) = toml::to_string_pretty(&options)
        {
            let _ = std::fs::write(dir.join("options.toml"), text);
        }
        // The built-in preset in use keeps the change as its own (#564).
        if let Some(i) = self.editing.get() {
            let mut presets = self.presets.borrow_mut();
            if let Some(p) = presets.get_mut(i).filter(|p| p.built_in) {
                let kept = preset_part(&options);
                if let Some(factory) = p.factory.as_ref() {
                    save_card(i, &kept, factory);
                }
                p.options = kept;
            }
        }
        *self.options.borrow_mut() = options;
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
        self.refresh_raft_warning();
    }

    /// Hands the overhang map the angle these options classify by (#91).
    fn push_overhang_limit(&self, ui: &MainWindow) {
        let degrees = self.options.borrow().self_support_angle_deg();
        self.ctrl.set_overhang_angle(degrees as f32);
        self.ctrl
            .set_tip_reach(self.options.borrow().tip_reach_mm());
        ui.set_overhang_map_limit(format!("{}°", mm_text(degrees)).into());
    }

    /// Whether the raft of the options in force would take the models'
    /// first layers, and by how much, for the layer height of the last
    /// Generate press. Models resting on the plate count as lifted when the
    /// Supports tab's box lifts them on Generate (#389).
    fn raft_clash(&self) -> Option<RaftClash> {
        let targets = self.press_targets();
        let lifted_to = self.lift_on_plate_mm();
        let lowest = self
            .document
            .with_scene(|spatial| {
                lowest_model_after_lift_mm(spatial, targets.as_deref(), lifted_to)
            })
            .flatten();
        raft_clash(
            &self.options.borrow(),
            self.raft_layer_height_mm.get(),
            lowest,
        )
    }

    /// Where a Generate press lifts the models resting on the plate (#389),
    /// for the layer height of the last press; `None` with the box off.
    fn lift_on_plate_mm(&self) -> Option<f64> {
        self.bus
            .snapshot()
            .lift_models_on_plate
            .then(|| support_lift_mm(&self.options.borrow(), self.raft_layer_height_mm.get()))
    }

    /// Shows or clears the Supports tab's raft warning (#131), raised by a
    /// Generate press and kept up to date with the scene and the options
    /// until a new scene comes in.
    fn refresh_raft_warning(&self) {
        self.raft_checked_at.set(Instant::now());
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let clash = if self.raft_armed.get() {
            self.raft_clash()
        } else {
            None
        };
        let (message, label) = match clash {
            Some(clash) => (
                raft_clash_message(clash.layers, clash.lift_to_mm),
                raft_lift_label(clash.lift_to_mm),
            ),
            None => Default::default(),
        };
        ui.set_support_raft_clash_message(message);
        ui.set_support_raft_lift_label(label);
    }

    /// The warning's lift button: the models to support that stand in the
    /// raft rise clear of it, then the Generate press it held back runs.
    fn lift_for_warning(self: &Rc<Self>) {
        if let Some(clash) = self.raft_clash()
            && self.lift_models(
                clash.raft_top_mm,
                clash.lift_to_mm,
                self.press_targets().as_deref(),
            )
        {
            self.start();
        }
    }

    /// The warning's "No raft" button: the raft goes off, then the Generate
    /// press it held back runs.
    fn raft_off_for_warning(self: &Rc<Self>) {
        self.turn_raft_off();
        self.start();
    }

    /// The warning's "Generate anyway" button: runs the held-back press with
    /// the raft as it is.
    fn generate_ignoring_raft(self: &Rc<Self>) {
        self.raft_ignored.set(true);
        self.start();
        self.raft_ignored.set(false);
    }

    /// Lifts every model in the raft of the scene's supports (or, without
    /// them, of the options in force) so its lowest point stands at
    /// `lift_to_mm`, as one undoable edit. Returns whether a model moved.
    pub(crate) fn lift_clear_of_raft(&self, lift_to_mm: f64) -> bool {
        let raft_top = self
            .raft_top_mm()
            .or_else(|| self.options.borrow().raft_top_mm())
            .unwrap_or(lift_to_mm);
        self.lift_models(raft_top, lift_to_mm, None)
    }

    /// Lifts every model (only those `only` names, when given) lower than
    /// `below_mm` so its lowest point stands at `lift_to_mm`, as one
    /// undoable edit. Returns whether a model moved.
    fn lift_models(&self, below_mm: f64, lift_to_mm: f64, only: Option<&[NodeId]>) -> bool {
        let Some(edit) = self
            .document
            .with_scene(|spatial| lift_edit(spatial, below_mm, lift_to_mm, only))
            .flatten()
        else {
            return false;
        };
        let moved = match self.document.apply_edit(edit.clone()) {
            Ok(()) => {
                self.document.push_edit(edit);
                true
            }
            Err(_) => false,
        };
        self.refresh_raft_warning();
        moved
    }

    /// Turns the raft off in the support options, as the Supports tab's
    /// Bottom setting does.
    pub(crate) fn turn_raft_off(&self) {
        self.on_option(9, 2.0);
    }

    /// The overhang angle (degrees from horizontal) and the area per
    /// support (mm²) the engine is set to: the Orient tool (#182) counts tips
    /// as the engine places them.
    pub(crate) fn overhang_terms(&self) -> (f64, f64) {
        // As the engine reads them (#422): a zone preset's own angle, and
        // the area around each tip its spacing holds.
        let o = self.options.borrow();
        (o.self_support_angle_deg(), o.tip_reach_mm().powi(2))
    }

    /// Whether models wait for supports the last edit took away, or a run
    /// is making them (#153).
    pub(crate) fn regenerating(&self) -> bool {
        self.regenerating.borrow().is_some() || self.document.supports_wanted().is_some()
    }

    /// Supports for every model of the document, made and pasted as a run
    /// ends, with no question in the way: for tests of other tools that
    /// need a supported model.
    #[cfg(test)]
    pub(crate) fn generate_now_for_test(&self) {
        let graph = self
            .document
            .with_scene(|spatial| spatial.graph().clone())
            .expect("a scene");
        let outcome = start_supports(&graph, SupportRequest::default())
            .expect("a run starts")
            .join();
        *self.started_on.borrow_mut() = Some(self.document.scene_revision());
        self.finish(outcome);
    }

    /// The Generate button with `options` in the tab, waited for: the lift of
    /// the models on the plate (#389) and what follows (#606) as pressed.
    #[cfg(test)]
    pub(crate) fn press_generate_and_wait(self: &Rc<Self>, options: SupportOptions) {
        self.set_options(options);
        self.start();
        let handle = self.handle.borrow_mut().take().expect("a run started");
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_gen_in_progress(false);
        }
        self.finish(handle.join());
    }

    #[cfg(test)]
    pub(crate) fn generate_now(&self, options: SupportOptions) {
        let graph = self
            .document
            .with_scene(|spatial| spatial.graph().clone())
            .expect("a scene");
        let request = SupportRequest {
            options,
            ..SupportRequest::default()
        };
        let outcome = start_supports(&graph, request)
            .expect("the run starts")
            .join();
        *self.started_on.borrow_mut() = Some(self.document.scene_revision());
        self.finish(outcome);
    }

    /// Draws the plates' one raft for all (#556) until the scene stands
    /// still with it, as the watcher does: for tests.
    #[cfg(test)]
    pub(crate) fn settle_plate_rafts(&self) {
        let volume = self.ctrl.build_volume();
        for _ in 0..2000 {
            let revision = self.document.scene_revision();
            self.plate_rafts.tick(&self.document, &volume);
            std::thread::sleep(Duration::from_millis(5));
            self.plate_rafts.tick(&self.document, &volume);
            if self.document.scene_revision() == revision && !self.plate_rafts.busy() {
                self.go_on_after_rafts();
                return;
            }
        }
    }

    /// Waits for the hand edit being made off the UI thread, and settles it
    /// as the UI's poll does. Returns the longest the UI thread was held by
    /// one poll: what landing the edit costs it.
    #[cfg(test)]
    pub(crate) fn settle_edit(self: &Rc<Self>) -> Duration {
        let waited = Instant::now();
        let mut longest = Duration::ZERO;
        while self.editing() && waited.elapsed() < Duration::from_secs(120) {
            std::thread::sleep(Duration::from_millis(5));
            let poll = Instant::now();
            self.poll_click();
            longest = longest.max(poll.elapsed());
        }
        assert!(!self.editing(), "the edit settled");
        longest
    }

    /// The top of the raft the supports in the scene stand on, for the
    /// review (#131): the options of the run that made them, or the options
    /// in force for supports this window did not make. `None` without
    /// supports or without a raft.
    pub(crate) fn raft_top_mm(&self) -> Option<f64> {
        self.support_leaf()?;
        self.generated_raft_top
            .get()
            .unwrap_or_else(|| self.options.borrow().raft_top_mm())
    }

    /// An option edited in the Supports tab (codes in the order of its
    /// fields: sizing, density, angle, size scale, smallest island,
    /// coverage, stabilization, bracing, smooth, raft bottom, raft
    /// thickness, raft wall), then a zone preset's grid (#464): 12 turns it
    /// on at the spacing `value` (off at 0), 13 sets its spacing.
    fn on_option(&self, code: i32, value: f32) {
        // With some models picked, their raft alone changes (#198): it
        // becomes theirs, and the tab's raft stays the other models'.
        if matches!(code, 9..=11 | 16..=18)
            && let Some(targets) = self.target_models()
        {
            let mut own = self.raft_of(targets[0]);
            match code {
                9 => {
                    own.raft = match value as i32 {
                        1 => RaftMode::Lines,
                        2 => RaftMode::Off,
                        _ => RaftMode::Solid,
                    }
                }
                10 => own.thickness_mm = f64::from(value),
                11 => own.wall = value != 0.0,
                16 => own.pads = value != 0.0,
                17 => own.holes_mm = f64::from(value).max(0.0),
                _ => own.tabs = value != 0.0,
            }
            {
                let mut owns = self.own_rafts.borrow_mut();
                owns.retain(|(m, _)| !targets.contains(m));
                if own != OwnRaft::of(&self.options.borrow()) {
                    owns.extend(targets.iter().map(|m| (*m, own)));
                }
            }
            if let Some(ui) = self.ui.upgrade() {
                self.push_raft(&ui);
            }
            self.rafts_follow();
            return;
        }
        let mut o = self.options.borrow().clone();
        let v = f64::from(value);
        let on = value != 0.0;
        match code {
            7 => o.bracing = on,
            9 => {
                o.raft = match value as i32 {
                    1 => RaftMode::Lines,
                    2 => RaftMode::Off,
                    _ => RaftMode::Solid,
                };
                // One raft for all stands on lines (#556): lines picked keep
                // it, another raft turns it off.
                if o.one_raft && o.raft == RaftMode::Lines {
                    // As it stood: the preset keeps its raft under it.
                    o.raft = self.options.borrow().raft;
                } else {
                    o.one_raft = false;
                }
            }
            // One raft for all (#556): a raft of lines, drawn once for the
            // plate under every set; the preset keeps its own raft for when
            // it goes off.
            14 => o.one_raft = on,
            // Supports only from the plate (#471).
            15 => {
                let Some(zones) = o.zones.as_mut() else {
                    return;
                };
                zones.rules.plate_only = on;
            }
            // Mini supports, an adaptive base, braces that follow their
            // trunks (#471).
            19..=21 => {
                let Some(zones) = o.zones.as_mut() else {
                    return;
                };
                match code {
                    19 => zones.rules.keep_long_island_branches = !on,
                    20 => zones.rules.adaptive_base = on,
                    _ => zones.rules.braces_follow_trunks = on,
                }
            }
            10 => o.raft_thickness_mm = v,
            11 => o.raft_wall = on,
            // A solid raft's other shapes (#138, #99).
            16 => o.raft_pads = on,
            17 => o.raft_holes_mm = v.max(0.0),
            18 => o.raft_tabs = on,
            12 | 13 => {
                let shown = shown_grid_mm(&o);
                let Some(zones) = o.zones.as_mut() else {
                    return;
                };
                zones.grid_mm = match code {
                    12 if on => Some(if v > 0.0 { v } else { shown }),
                    12 => None,
                    _ => zones.grid_mm.map(|_| v),
                };
            }
            _ => return,
        }
        // The tab's raft is every model's again.
        if matches!(code, 9..=11 | 14 | 16..=18) {
            self.own_rafts.borrow_mut().clear();
        }
        self.set_options(o);
        if matches!(code, 9..=11 | 14 | 16..=18) {
            self.rafts_follow();
        }
    }

    /// The tab's raft changed (#519): the sets already made, of the models
    /// Generate would work on, are drawn again on it, in one undo step. The
    /// supports stay where they are: only the raft and the roots' feet on it
    /// change, so there is nothing to generate again.
    fn rafts_follow(&self) {
        self.rafts_follow_for(self.target_models());
    }

    /// [`Self::rafts_follow`] for the sets of `targets`, every set for `None`.
    fn rafts_follow_for(&self, targets: Option<Vec<NodeId>>) {
        // A run in flight lands with the raft it started with, and a hand
        // edit in flight with its set's: neither is drawn over under them.
        if self.handle.borrow().is_some()
            || self.pending_click.borrow().is_some()
            || !self.queued_clicks.borrow().is_empty()
        {
            return;
        }
        let options = self.options.borrow().clone();
        let sets: Vec<DrawnSet> = self
            .document
            .with_scene(|spatial| sets_in_force(spatial, &self.sessions))
            .unwrap_or_default()
            .into_iter()
            .filter(|d| !d.unborn && targets.as_ref().is_none_or(|t| t.contains(&d.model)))
            .collect();
        // Each model on its own raft (#198), the rest on the tab's.
        let options_of = |model: NodeId| self.raft_of(model).on(&options);
        let started = Instant::now();
        // Each set is drawn on its own thread: a set is a few tens of
        // milliseconds, a plate of them would add up.
        let redrawn: Vec<(DrawnSet, SupportSession, Arc<sc_mesh_core::IndexedMesh>)> =
            std::thread::scope(|scope| {
                let jobs: Vec<_> = sets
                    .into_iter()
                    .filter_map(|drawn| {
                        let next = drawn.session.with_raft_of(&options_of(drawn.model))?;
                        Some(scope.spawn(move || {
                            let mesh =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    next.mesh()
                                }));
                            match mesh {
                                Ok(Ok(Some(mesh))) => Some((drawn, next, Arc::new(mesh))),
                                _ => None,
                            }
                        }))
                    })
                    .collect();
                jobs.into_iter()
                    .filter_map(|job| job.join().ok().flatten())
                    .collect()
            });
        if redrawn.is_empty() {
            return;
        }
        let swaps: Option<Vec<SceneEdit>> = redrawn
            .iter()
            .map(|(drawn, _, mesh)| {
                self.document
                    .with_scene(|spatial| leaf_mesh_swap(spatial.graph(), drawn.leaf, mesh.clone()))
                    .flatten()
            })
            .collect();
        let Some(mut swaps) = swaps else {
            return;
        };
        let swap = if swaps.len() == 1 {
            swaps.pop().expect("len == 1")
        } else {
            SceneEdit::Compound(swaps)
        };
        if self.document.apply_edit(swap.clone()).is_err() {
            return;
        }
        self.document.push_edit(swap);
        let count = redrawn.len();
        for (drawn, next, _) in redrawn {
            record(
                &self.document,
                &self.sessions,
                drawn.leaf,
                drawn.model,
                Arc::new(next),
            );
        }
        crate::breadcrumbs::note(&format!(
            "supports: {count} set(s) drawn on the new raft in {} ms",
            started.elapsed().as_millis()
        ));
        self.show_status(ctr!(
            "Workspace",
            "The raft of 1 set of supports changed; the supports stayed where they were.",
            "The raft of {n} sets of supports changed; the supports stayed where they were.",
            count
        ));
    }

    fn reload_presets(&self) {
        let mut presets = built_in_presets();
        // Each built-in preset with the changes the person made to it.
        for (i, preset) in presets.iter_mut().enumerate() {
            if let Some(options) = card_file(i)
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| toml::from_str::<SupportOptions>(&text).ok())
                .map(|o| o.clamped())
                .filter(|o| o.zones.is_some())
            {
                preset.options = options;
            }
        }
        presets.extend(load_user_presets());
        *self.presets.borrow_mut() = presets;
    }

    /// The built-in preset the options in use are: the one they are, or,
    /// for options changed before the presets kept their changes, the one
    /// they share the most with, which takes them as its changes. Options
    /// that are one of the person's own presets are none.
    fn find_card(&self) {
        let o = preset_part(&self.options.borrow());
        let mut presets = self.presets.borrow_mut();
        let built_in = presets.iter().take_while(|p| p.built_in).count();
        if let Some(i) = presets.iter().position(|p| p.options == o) {
            self.editing.set((i < built_in).then_some(i));
            return;
        }
        let Some(zones) = o.zones.as_ref() else {
            return;
        };
        let bases: Vec<&ZonePreset> = presets[..built_in]
            .iter()
            .filter_map(|p| p.factory.as_ref()?.zones.as_ref())
            .collect();
        if let Some(i) = crate::support_shape_edit::closest_base(&bases, zones) {
            presets[i].options = o.clone();
            if let Some(factory) = presets[i].factory.clone() {
                save_card(i, &o, &factory);
            }
            self.editing.set(Some(i));
        }
    }

    fn on_preset_picked(&self, index: i32) {
        let picked = self
            .presets
            .borrow()
            .get(index.max(0) as usize)
            .map(|p| p.options.clone());
        if let Some(mut options) = picked {
            // One raft for all is how the plate prints, not the preset's: it
            // stays on (#556).
            options.one_raft = self.options.borrow().one_raft;
            let built_in = self
                .presets
                .borrow()
                .get(index.max(0) as usize)
                .is_some_and(|p| p.built_in);
            let card = built_in.then_some(index.max(0) as usize);
            self.based_on.set(card);
            self.editing.set(card);
            self.picked.set(Some(index.max(0) as usize));
            // A preset picked by hand is for every model (#723).
            self.set_by_height(false);
            self.set_options(options);
        }
    }

    /// Reset to default: the built-in preset in use goes back to its values
    /// as they ship; options of no built-in preset take the one they were
    /// made from.
    fn on_preset_revert(&self) {
        let card = self.editing.get().or(self.based_on.get());
        let Some(i) = card else {
            return;
        };
        let factory = self.presets.borrow().get(i).and_then(|p| p.factory.clone());
        let Some(factory) = factory else {
            return;
        };
        save_card(i, &factory, &factory);
        if let Some(p) = self.presets.borrow_mut().get_mut(i) {
            p.options = factory.clone();
        }
        self.based_on.set(Some(i));
        self.editing.set(Some(i));
        self.picked.set(Some(i));
        self.set_options(factory);
    }

    /// Saves the current options as a preset of the person.
    fn on_preset_save(&self, name: &str) {
        let Some(stem) = preset_file_stem(name) else {
            return;
        };
        let Some(dir) = support_data_dir().map(|d| d.join("presets")) else {
            return;
        };
        let options = self.options.borrow().clone();
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(text) = toml::to_string_pretty(&options)
        {
            let _ = std::fs::write(dir.join(format!("{stem}.toml")), text);
        }
        self.reload_presets();
        // The preset just saved is the one marked.
        self.picked.set(
            self.presets
                .borrow()
                .iter()
                .position(|p| !p.built_in && p.name == stem),
        );
        self.editing.set(None);
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
    }

    /// Deletes the selected preset, when it is one of the person's own.
    fn on_preset_delete(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let index = ui.get_support_preset_index();
        let target = self
            .presets
            .borrow()
            .get(index.max(0) as usize)
            .filter(|p| index >= 0 && !p.built_in)
            .map(|p| p.name.clone());
        let (Some(name), Some(dir)) = (target, support_data_dir()) else {
            return;
        };
        if let Some(stem) = preset_file_stem(&name) {
            let _ = std::fs::remove_file(dir.join("presets").join(format!("{stem}.toml")));
        }
        self.reload_presets();
        self.push_options(&ui);
    }

    /// The support mesh this driver pasted: its leaf and current mesh.
    pub(crate) fn support_leaf(&self) -> Option<(NodeId, Arc<sc_mesh_core::IndexedMesh>)> {
        self.document.with_scene(support_leaf).flatten()
    }

    /// Whether the scene has supports of ours in force: what the review may
    /// add tips to on its own (#606).
    pub(crate) fn any_set_in_force(&self) -> bool {
        !self.sets_in_force().is_empty()
    }

    /// Every support set of the scene in force.
    fn sets_in_force(&self) -> Vec<DrawnSet> {
        self.document
            .with_scene(|spatial| sets_in_force(spatial, &self.sessions))
            .unwrap_or_default()
    }

    /// Makes supports for every model of `graph`, the scene on show.
    #[cfg(test)]
    pub(crate) fn make_supports_for_test(&self, graph: &sc_scene::Scene) {
        let outcome = start_supports(graph, SupportRequest::default())
            .expect("the job starts")
            .join();
        *self.started_on.borrow_mut() = Some(self.document.scene_revision());
        self.finish(outcome);
    }

    /// The tips of every set in force, each set's sorted by id.
    #[cfg(test)]
    pub(crate) fn tips_for_test(&self) -> Vec<Vec<(String, [f64; 3])>> {
        self.sets_in_force()
            .iter()
            .map(|drawn| {
                let mut tips = drawn.session.tips();
                tips.sort_by(|a, b| a.0.cmp(&b.0));
                tips
            })
            .collect()
    }

    /// The support set Remove may take from: the selected supports, or the
    /// selected model's. A safeguard: another model's supports are never
    /// taken by a click meant for these. `None` with neither selected.
    fn selected_set(&self) -> Option<NodeId> {
        let selected = self.document.selection_primary_node_id()?;
        self.document
            .with_scene(|spatial| {
                if support_leaves(spatial).any(|(id, _)| id == selected) {
                    return Some(selected);
                }
                sets_of_model(spatial, &self.sessions, selected)
                    .into_iter()
                    .next()
            })
            .flatten()
    }

    /// The support set a hand tool at `point` (world mm) acts on, among the
    /// sets in force: for Add, the one whose model is nearest; for Remove,
    /// the selected set ([`Self::selected_set`]) when its support is there.
    /// Sets whose box is out of reach are not asked, so a hover over a busy
    /// scene asks one set at most, as a rule.
    fn set_at(&self, point: [f32; 3], tool: HandTool) -> Option<DrawnSet> {
        let p = point.map(f64::from);
        let only = match tool {
            HandTool::Remove => Some(self.selected_set()?),
            HandTool::Add | HandTool::AddNear => None,
        };
        let mut near: Vec<DrawnSet> = self
            .document
            .with_scene(|spatial| {
                sets_in_force(spatial, &self.sessions)
                    .into_iter()
                    .filter(|drawn| only.is_none_or(|leaf| drawn.leaf == leaf))
                    .filter(|drawn| {
                        let boxed = match tool {
                            HandTool::Remove => drawn.leaf,
                            HandTool::Add | HandTool::AddNear => drawn.model,
                        };
                        spatial
                            .leaf(boxed)
                            .and_then(|q| q.transformed_aabb(q.world_transform()))
                            .is_some_and(|aabb| box_distance(&aabb, p) <= SET_REACH_MM)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if near.len() <= 1 {
            return near.pop();
        }
        let distance = |drawn: &DrawnSet| match tool {
            HandTool::Remove => drawn.session.remove_distance(p),
            HandTool::Add | HandTool::AddNear => drawn.session.add_distance(p),
        };
        near.into_iter()
            .filter_map(|drawn| Some((distance(&drawn)?, drawn)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, drawn)| drawn)
    }

    /// The cursor moved over the model (`Some`) or off it with a hand tool
    /// armed, without Shift.
    #[cfg(test)]
    fn on_tool_hover(self: &Rc<Self>, point: Option<[f32; 3]>) {
        self.on_tool_hover_with(point, false);
    }

    /// The cursor moved over the model (`Some`) or off it with a hand tool
    /// armed, `shift` held or not: the newest point goes to the preview
    /// worker; the UI never waits for it. With Shift, Add previews hanging
    /// the support from the nearest one (#3).
    fn on_tool_hover_with(self: &Rc<Self>, point: Option<[f32; 3]>, shift: bool) {
        // The Select tool shows what it picked, not what a click would do.
        if self
            .ui
            .upgrade()
            .is_some_and(|ui| ui.get_support_tool() == 5)
        {
            return;
        }
        if self
            .ui
            .upgrade()
            .is_some_and(|ui| ui.get_support_tool() == 6)
        {
            self.line_hover(point);
            return;
        }
        self.hover_at.set(point.map(|p| (p, shift)));
        let generation = self.hover_generation.get() + 1;
        self.hover_generation.set(generation);
        if let Some(point) = point
            && self.imported_body_hover(point)
        {
            // What the worker answers for the points before is stale.
            self.hover_shown.set(generation);
            self.hover_poll.stop();
            return;
        }
        let bracing = self
            .ui
            .upgrade()
            .is_some_and(|ui| ui.get_support_tool() == 4)
            && self.handle.borrow().is_none()
            && self.drag.borrow().is_none();
        let ask = if bracing {
            self.brace_ask(generation, point)
        } else {
            self.tool_ask(generation, point, shift)
        };
        let Some(ask) = ask else {
            self.hover_poll.stop();
            self.hover_shown.set(generation);
            self.last_ghost.borrow_mut().clear();
            self.ctrl.set_preview_overlay(None);
            self.show_add_note(Default::default());
            return;
        };
        if self.hover_worker.asks.send(ask).is_err() {
            return;
        }
        // Started once: starting a running timer starts its wait over, and a
        // mouse that moves more often than it fires would never see an
        // answer drawn until it stopped (#3).
        if self.hover_poll.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.hover_poll
            .start(slint::TimerMode::Repeated, HOVER_POLL, move || {
                if let Some(run) = weak.upgrade() {
                    run.take_hover_answer();
                }
            });
    }

    /// The body of the imported supports (#260) the Remove tool would take
    /// at `point`, among the sets it may take from, with the leaf's mesh.
    fn imported_body_at(
        &self,
        point: [f32; 3],
    ) -> Option<(ImportedBody, Arc<sc_mesh_core::IndexedMesh>)> {
        let ui = self.ui.upgrade()?;
        if ui.get_support_tool() != 2
            || self.handle.borrow().is_some()
            || self.drag.borrow().is_some()
        {
            return None;
        }
        let selected = self.document.selection_primary_node_id();
        self.document
            .with_scene(|spatial| {
                let sets = crate::imported_supports::removable_sets(spatial, selected);
                if sets.is_empty() {
                    return None;
                }
                let body = crate::imported_supports::body_at(spatial, &sets, point.map(f64::from))?;
                let mesh = spatial.graph().leaf_mesh_arc(body.leaf)?.clone();
                Some((body, mesh))
            })
            .flatten()
    }

    /// The Remove tool's hover over a body of imported supports (#260): it
    /// shows red. Whether there was one.
    fn imported_body_hover(&self, point: [f32; 3]) -> bool {
        let Some((body, _)) = self.imported_body_at(point) else {
            return false;
        };
        let drawn = self
            .document
            .with_scene(|spatial| crate::imported_supports::body_in_world(spatial, &body))
            .flatten();
        let Some((positions, indices)) = drawn else {
            return false;
        };
        let piece = Arc::new(OverlayMesh {
            positions,
            indices,
            tint: OverlayTint::Invalid,
        });
        *self.last_ghost.borrow_mut() = vec![piece];
        self.show_hover_overlay();
        self.show_add_note((
            ctr!(
                "SupportDialog",
                "A click removes this body of the imported supports."
            ),
            false,
        ));
        true
    }

    /// A click of the Remove tool on a body of imported supports (#260): the
    /// body goes, in one undo entry, and the model stays as it is. Whether
    /// there was one.
    fn remove_imported_body(&self, point: [f32; 3]) -> bool {
        let Some((body, _)) = self.imported_body_at(point) else {
            return false;
        };
        self.remove_imported_bodies(&[body])
    }

    /// Takes the bodies `bodies` of imported supports off (#260), in one
    /// undo entry; the model stays as it is. Whether anything went.
    fn remove_imported_bodies(&self, bodies: &[ImportedBody]) -> bool {
        let taken: Vec<&ImportedBody> = bodies.iter().collect();
        let edit = self
            .document
            .with_scene(|spatial| crate::imported_supports::removal_of_bodies(spatial, &taken))
            .flatten();
        let Some(edit) = edit else {
            return false;
        };
        if self.document.apply_edit(edit.clone()).is_err() {
            return false;
        }
        self.document.push_edit(edit);
        self.last_ghost.borrow_mut().clear();
        self.ctrl.set_preview_overlay(None);
        self.show_add_note(Default::default());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "Removed {n} body of the imported supports.",
                "Removed {n} bodies of the imported supports.",
                bodies.len()
            ));
        }
        crate::breadcrumbs::note(&format!(
            "supports: {} imported bod(ies) removed ({} faces)",
            bodies.len(),
            bodies.iter().map(|b| b.faces.len()).sum::<usize>()
        ));
        true
    }

    /// A box or a brush of Remove over the selection's imported supports
    /// (#260): the drag's state, with what a brush takes where it is
    /// pressed. `None` when the selection has none.
    fn imported_sweep(
        &self,
        ui: &MainWindow,
        kind: SupportDragKind,
        press: &DragPoint,
        project: Project<'_>,
    ) -> Option<DragState> {
        let selected = self.document.selection_primary_node_id();
        let found = self.document.with_scene(|spatial| {
            let sets = crate::imported_supports::removable_sets(spatial, selected);
            crate::imported_supports::bodies_of(spatial, &sets)
        })?;
        if found.is_empty() {
            return None;
        }
        // Each body is named by its place among them, as a support by its id.
        let mut bodies = Vec::with_capacity(found.len());
        let mut points = Vec::with_capacity(found.len());
        for (k, (body, over)) in found.into_iter().enumerate() {
            bodies.push(body);
            points.push((k.to_string(), over));
        }
        let seen_from = SeenFrom::new(&points, project);
        let mut looked = HashMap::new();
        let brush = (kind == SupportDragKind::Brush).then_some(press.cursor);
        let picked = match brush {
            Some(at) => {
                let radius = ui.get_support_brush_radius();
                let locate = self.body_locator(&bodies);
                Self::taken(
                    &seen_from,
                    &locate,
                    &mut looked,
                    &|_| false,
                    &|q| screen_segment_distance(q, at, at) <= radius,
                    project,
                    usize::MAX,
                )
                .0
                .into_iter()
                .map(|k| points[k].0.clone())
                .collect()
            }
            None => Vec::new(),
        };
        crate::breadcrumbs::note(&format!(
            "supports: {kind:?} started over {} imported bodies",
            bodies.len()
        ));
        Some(DragState::RemoveImported {
            bodies,
            points,
            corner: press.cursor,
            owed: false,
            seen_from,
            looked,
            picked,
            press: press.cursor,
            brush,
        })
    }

    /// Which of `bodies` a ray hit: the body under the hit, when it is one
    /// of them; `None` for a hit on the model or anything else.
    fn body_locator<'a>(
        &'a self,
        bodies: &'a [ImportedBody],
    ) -> impl Fn(ScreenHit) -> Option<usize> + 'a {
        move |hit: ScreenHit| {
            if !bodies.iter().any(|b| b.leaf == hit.leaf) {
                return None;
            }
            let found = self
                .document
                .with_scene(|spatial| {
                    crate::imported_supports::body_at(
                        spatial,
                        &[hit.leaf],
                        hit.point.map(f64::from),
                    )
                })
                .flatten()?;
            bodies
                .iter()
                .position(|b| b.leaf == found.leaf && b.faces.first() == found.faces.first())
        }
    }

    /// A box's or a brush's picks over imported supports changed: the
    /// bodies they take are drawn red (#260).
    fn imported_picks_changed(&self, bodies: &[ImportedBody]) {
        if bodies.is_empty() {
            self.last_ghost.borrow_mut().clear();
            self.ctrl.set_preview_overlay(None);
            self.show_add_note(Default::default());
            return;
        }
        let drawn = self
            .document
            .with_scene(|spatial| {
                let (mut positions, mut indices) = (Vec::new(), Vec::new());
                for body in bodies {
                    let Some((p, i)) = crate::imported_supports::body_in_world(spatial, body)
                    else {
                        continue;
                    };
                    let base = positions.len() as u32;
                    positions.extend(p);
                    indices.extend(i.into_iter().map(|v| v + base));
                }
                (positions, indices)
            })
            .unwrap_or_default();
        *self.last_ghost.borrow_mut() = vec![Arc::new(OverlayMesh {
            positions: drawn.0,
            indices: drawn.1,
            tint: OverlayTint::Invalid,
        })];
        self.show_hover_overlay();
        self.show_add_note((
            ctr!(
                "SupportDialog",
                "Bodies of the imported supports to remove: {}.",
                bodies.len()
            ),
            true,
        ));
    }

    /// The brace tool's preview at `point` (#3): before the first click, the
    /// support it takes; after, the brace to the support under the cursor,
    /// or the first support alone with the cursor off any.
    fn brace_ask(&self, generation: u64, point: Option<[f32; 3]>) -> Option<HoverAsk> {
        let first = self.brace_from.borrow().clone();
        let (drawn, tool, at, brace_from) = match first {
            Some((drawn, from)) => {
                let at = point.unwrap_or(from.at.map(|c| c as f32));
                let tool = if point.is_some() { 7 } else { 6 };
                (drawn, tool, at, Some(from))
            }
            None => {
                let point = point?;
                (self.set_of_brace(point)?, 6, point, None)
            }
        };
        Some(HoverAsk {
            generation,
            base: drawn.session.clone(),
            from: session_key(&drawn),
            point: at,
            tool,
            how: AddOptions::default(),
            tip: None,
            tips: Arc::from([]),
            others: Vec::new(),
            brace_from,
            part: None,
        })
    }

    /// The preview of the other tools at `point`.
    fn tool_ask(&self, generation: u64, point: Option<[f32; 3]>, shift: bool) -> Option<HoverAsk> {
        point.and_then(|point| {
            let ui = self.ui.upgrade()?;
            let tool = ui.get_support_tool();
            if tool == 0 || self.handle.borrow().is_some() || self.drag.borrow().is_some() {
                return None;
            }
            if tool == 3 {
                let drawn = self.set_of_tip(point)?;
                return Some(HoverAsk {
                    generation,
                    base: drawn.session.clone(),
                    from: session_key(&drawn),
                    point,
                    tool: 4,
                    how: AddOptions::default(),
                    tip: None,
                    tips: Arc::from([]),
                    others: Vec::new(),
                    brace_from: None,
                    part: None,
                });
            }
            let (hand, how) = self.click_tool(&ui, tool, shift);
            let drawn = self
                .set_at(point, hand)
                .or_else(|| self.unborn_hover_set(self.bare_model_at(point, hand)?))?;
            if hand == HandTool::Add {
                self.refresh_tip_label(&ui, Some(&drawn.session));
            }
            Some(HoverAsk {
                generation,
                base: drawn.session.clone(),
                from: session_key(&drawn),
                point,
                tool,
                how,
                tip: None,
                tips: Arc::from([]),
                others: Vec::new(),
                brace_from: None,
                part: None,
            })
        })
    }

    /// The tool a click of the Supports tab's tool code `tool` is, and how
    /// an Add makes its support: hung from the nearest one with `shift`, at
    /// the tip size picked, with the selected preset's numbers for it (as
    /// Line and Area place theirs).
    fn click_tool(&self, ui: &MainWindow, tool: i32, shift: bool) -> (HandTool, AddOptions) {
        if tool == 1 {
            let how = AddOptions {
                attach: shift,
                size: hand_tip_size(ui.get_support_tip_size()),
                shape: None,
            };
            (HandTool::Add, how)
        } else {
            (HandTool::Remove, AddOptions::default())
        }
    }

    /// A hand tool was armed or put away: a hover before it is no point to
    /// preview again, and the Add card says which tip it places.
    fn on_tool_changed(&self) {
        self.hover_at.set(None);
        self.brace_from.borrow_mut().take();
        self.line_from.set(None);
        self.show_add_note(Default::default());
        if let Some(ui) = self.ui.upgrade() {
            // The Area tool paints where supports go, and its paint shows
            // while it is armed (#177).
            let area = ui.get_support_tool() == 7;
            if area != self.area_brush.armed() {
                self.area_brush.set_armed(area, None);
                self.ctrl
                    .show_paint_of(sc_scene::PaintKind::SupportEnforce, area);
            }
            self.refresh_tip_label(&ui, None);
            // The supports picked go with the Select tool.
            if ui.get_support_tool() != 5 {
                self.chosen.borrow_mut().clear();
                self.last_ghost.borrow_mut().clear();
                self.ctrl.set_preview_overlay(None);
                ui.set_support_picked_count(0);
            }
            self.show_chosen(&ui);
        }
    }

    /// A click of the Select tool at `point` (#567): the support there is
    /// picked, alone, or with `add` (Shift) joins the picked ones or leaves
    /// them. A click on no support lets go of them all, a Shift+click
    /// leaves them.
    fn select_click(&self, ui: &MainWindow, point: Option<[f32; 3]>, add: bool) {
        // A click takes the gizmo back to the support's own part.
        self.part_focus.borrow_mut().take();
        let found = point.map(|p| p.map(f64::from)).and_then(|p| {
            self.sets_in_force()
                .into_iter()
                .filter_map(|drawn| Some((drawn.session.remove_distance(p)?, drawn)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .and_then(|(_, drawn)| {
                    let id = support_picked_at(&drawn.session, p)?;
                    Some((drawn.leaf, drawn.session.tip_point(&id)?))
                })
        });
        // A support hidden from the view is not picked (#728).
        let hidden = self.ctrl.hidden_supports().tips;
        let found = found.filter(|(leaf, tip)| {
            !hidden
                .iter()
                .any(|(l, t)| l == leaf && (0..3).all(|k| (t[k] - tip[k]).abs() < CHOSEN_TIP_MM))
        });
        {
            let mut chosen = self.chosen.borrow_mut();
            let same = |a: [f64; 3], b: [f64; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-6);
            match found {
                Some(one) if add => {
                    match chosen.iter().position(|c| c.0 == one.0 && same(c.1, one.1)) {
                        Some(k) => {
                            chosen.remove(k);
                        }
                        None => chosen.push(one),
                    }
                }
                Some(one) => *chosen = vec![one],
                None if !add => chosen.clear(),
                None => {}
            }
        }
        self.show_chosen(ui);
    }

    /// H with the Pick tool (#728): the picked supports leave the view, as
    /// the ones hidden before, and are let go. View only: the scene keeps
    /// them.
    fn hide_chosen(&self) {
        let mut tips = self.ctrl.hidden_supports().tips;
        let same = |a: [f64; 3], b: [f64; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-6);
        for (leaf, tip) in self.chosen.borrow_mut().drain(..) {
            if !tips.iter().any(|(l, t)| *l == leaf && same(*t, tip)) {
                tips.push((leaf, tip));
            }
        }
        self.hide_tips(tips);
        if let Some(ui) = self.ui.upgrade() {
            self.show_chosen(&ui);
        }
    }

    /// Hides from the view the supports at `tips`, each by its set's leaf
    /// and its tip, in place of the ones hidden (#728): each set they are
    /// in is drawn without them. A tip no set holds any more is let go;
    /// none shows every support again (Alt+H).
    fn hide_tips(&self, tips: Vec<(NodeId, [f64; 3])>) {
        if tips.is_empty() {
            self.ctrl.set_hidden_supports(HiddenSupports::default());
            return;
        }
        let sets = self.sets_in_force();
        let mut kept = Vec::new();
        let mut by_set: Vec<(&DrawnSet, Vec<String>)> = Vec::new();
        for (leaf, tip) in tips {
            let Some(drawn) = sets.iter().find(|d| d.leaf == leaf) else {
                continue;
            };
            let Some(id) = drawn.session.tip_near(tip, CHOSEN_TIP_MM) else {
                continue;
            };
            kept.push((leaf, tip));
            match by_set.iter_mut().find(|(d, _)| d.leaf == leaf) {
                Some((_, ids)) => {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                None => by_set.push((drawn, vec![id])),
            }
        }
        let shown = self
            .document
            .with_scene(|spatial| {
                by_set
                    .iter()
                    .filter_map(|(drawn, ids)| {
                        let from = spatial.graph().leaf_mesh_arc(drawn.leaf)?.clone();
                        let without = drawn.session.mesh_without(ids).ok()?;
                        Some(HiddenSet {
                            leaf: drawn.leaf,
                            from,
                            drawn: without.map(Arc::new),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.ctrl.set_hidden_supports(HiddenSupports {
            tips: kept,
            sets: shown,
        });
    }

    /// Lets go of every picked support (Esc with the Select tool).
    fn clear_chosen(&self) {
        self.chosen.borrow_mut().clear();
        if let Some(ui) = self.ui.upgrade() {
            self.show_chosen(&ui);
        }
    }

    /// The picked supports as they stand now: each set in force with the
    /// ids of its picked supports. One no set holds any more is let go.
    fn chosen_now(&self) -> Vec<(DrawnSet, Vec<String>)> {
        let sets = self.sets_in_force();
        let wanted: Vec<(NodeId, [f64; 3])> = self.chosen.borrow().clone();
        let mut kept = Vec::new();
        let mut out: Vec<(DrawnSet, Vec<String>)> = Vec::new();
        for (leaf, tip) in wanted {
            let Some(drawn) = sets.iter().find(|d| d.leaf == leaf) else {
                continue;
            };
            let Some(id) = drawn.session.tip_near(tip, CHOSEN_TIP_MM) else {
                continue;
            };
            kept.push((leaf, tip));
            match out.iter_mut().find(|(d, _)| d.leaf == leaf) {
                Some((_, ids)) => {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                None => out.push((drawn.clone(), vec![id])),
            }
        }
        *self.chosen.borrow_mut() = kept;
        out
    }

    /// The numbers each picked support was drawn with, on the Manual page
    /// with the Select tool armed; `None` with none picked.
    fn chosen_shapes(&self, ui: &MainWindow) -> Option<Vec<sc_geom::supports::SupportSize>> {
        if ui.get_support_tool() != 5 {
            return None;
        }
        let shapes: Vec<_> = self
            .chosen_now()
            .iter()
            .flat_map(|(drawn, ids)| ids.iter().filter_map(|id| drawn.session.shape_of(id)))
            .collect();
        (!shapes.is_empty()).then_some(shapes)
    }

    /// Calls `listener` with the picked supports each time they are shown
    /// (#245).
    pub(crate) fn on_chosen(&self, listener: impl Fn(Vec<(NodeId, [f64; 3])>) + 'static) {
        self.chosen_listeners.borrow_mut().push(Rc::new(listener));
    }

    /// Shows the picked supports in the scene (amber), how many there are,
    /// and their numbers in the panel.
    fn show_chosen(&self, ui: &MainWindow) {
        let picked = if ui.get_support_tool() == 5 {
            self.chosen.borrow().clone()
        } else {
            Vec::new()
        };
        let listeners: Vec<_> = self.chosen_listeners.borrow().clone();
        for listener in listeners {
            listener(picked.clone());
        }
        if ui.get_support_tool() != 5 {
            return;
        }
        let now = self.chosen_now();
        let count: usize = now.iter().map(|(_, ids)| ids.len()).sum();
        ui.set_support_picked_count(count as i32);
        let pieces: Vec<Arc<OverlayMesh>> = now
            .iter()
            .filter_map(|(drawn, ids)| drawn.session.own_drawing_of(ids).ok())
            .filter(|mesh| !mesh.faces().is_empty())
            .map(|mesh| Arc::new(overlay_of(&mesh, OverlayTint::Picked)))
            .collect();
        *self.last_ghost.borrow_mut() = pieces;
        self.show_hover_overlay();
        let carrying: usize = now
            .iter()
            .map(|(drawn, ids)| {
                ids.iter()
                    .filter(|id| drawn.session.carries_others(id))
                    .count()
            })
            .sum();
        self.show_add_note(if count == 0 {
            Default::default()
        } else if carrying > 0 {
            (
                ctr!(
                    "Inspector",
                    "{n} picked support holds others up: it moves with its gizmo, and the sizes below leave it as it is.",
                    "{n} picked supports hold others up: they move with the gizmo, and the sizes below leave them as they are.",
                    carrying
                ),
                true,
            )
        } else {
            (
                ctr!(
                    "Inspector",
                    "{n} support picked: the numbers below draw it again; move it with its gizmo, a cube takes an elbow.",
                    "{n} supports picked: the numbers below draw them again; pick one alone to move it.",
                    count
                ),
                false,
            )
        });
        let o = self.options.borrow().clone();
        self.push_shape(ui, &o);
    }

    /// Draws every picked support again with the numbers `shape_for` gives
    /// it from its own, each set as one step of Undo (#567).
    fn reshape_chosen(
        self: &Rc<Self>,
        ui: &MainWindow,
        shape_for: impl Fn(&sc_geom::supports::SupportSize) -> sc_geom::supports::SupportSize,
    ) {
        for (drawn, ids) in self.chosen_now() {
            let shapes: Vec<(String, sc_geom::supports::SupportSize)> = ids
                .iter()
                .filter_map(|id| Some((id.clone(), shape_for(&drawn.session.shape_of(id)?))))
                .collect();
            if !shapes.is_empty() {
                self.start_gesture(ui, drawn, EditOp::Reshape(shapes), None);
            }
        }
    }

    /// The imported supports of the model picked, made editable at the same
    /// contacts (#261): each support is followed from its tip down, the
    /// imported supports go, and supports made here take every one of those
    /// contacts with the widths the imported ones had (tip, shaft, tip
    /// length, foot: the file's own, its middle values), in the same Undo
    /// step. One that stood on the raft takes its route there (its foot, its
    /// elbows) where a support may; the rest are routed as Add routes one.
    /// What no route reaches is told, as a click of Add tells it.
    fn imported_to_editable(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(leaf) = self.document.selection_primary_node_id() else {
            return;
        };
        let found = self
            .document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                let model = if sc_mesh_core::presupported::is_imported_supports(query.mesh()) {
                    crate::imported_supports::owner_of(spatial, leaf, None)?
                } else if sc_geom::supports::is_any_support_mesh(query.mesh()) {
                    return None;
                } else {
                    leaf
                };
                let sets = crate::imported_supports::imported_of(spatial, model, None);
                let mut tips: Vec<(ImportedPlan, ImportedRoute)> = Vec::new();
                for &set in &sets {
                    let q = spatial.leaf(set)?;
                    let world = q.world_transform();
                    let to_world = |p: [f64; 3]| {
                        let at = sc_mesh_core::Vertex::new(p.map(|c| c as f32));
                        world.apply_to_vertex(at).coords().map(f64::from)
                    };
                    for route in sc_mesh_core::presupported::support_routes(q.mesh()) {
                        // Its elbows from the bottom up, as a route is given.
                        let elbows: Vec<[f64; 3]> =
                            route.elbows.iter().rev().map(|p| to_world(*p)).collect();
                        let plan = ImportedPlan {
                            at: to_world(route.tip.contact),
                            shape: None,
                            route: route.foot.map(|(foot, _)| {
                                let foot = to_world(foot);
                                ([foot[0], foot[1]], elbows)
                            }),
                        };
                        tips.push((plan, route));
                    }
                }
                let removal = crate::split_bodies::removal_edit(spatial.graph(), &sets)?;
                Some((tips, removal))
            })
            .flatten();
        let Some((tips, removal)) = found else {
            return;
        };
        if tips.is_empty() {
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "No contact tips were found in these imported supports; they stay as they are."
            ));
            return;
        }
        let shape = imported_shape(tips.iter().map(|(_, route)| route));
        if self.document.apply_edit(removal.clone()).is_err() {
            return;
        }
        self.document.push_edit(removal);
        let join = self.document.scene_revision();
        crate::breadcrumbs::note(&format!(
            "supports: imported made editable, {} tips",
            tips.len()
        ));
        let how = AddOptions {
            attach: false,
            size: None,
            shape: shape.clone().map(sc_geom::supports::HandShape),
        };
        let points: Vec<[f32; 3]> = tips
            .iter()
            .map(|(plan, _)| plan.at.map(|c| c as f32))
            .collect();
        let mut refused = Vec::new();
        let Ok(Prepared { sets, mut jobs }) =
            self.prepare_edit(&ui, HandTool::Add, &points, &how, &mut refused)
        else {
            return;
        };
        // Each set places its share of the supports with their routes.
        for job in &mut jobs {
            if let EditOp::Points(points) = &job.op {
                let plans = points
                    .iter()
                    .filter_map(|(i, _)| tips.get(*i))
                    .map(|(plan, _)| ImportedPlan {
                        shape: shape.clone(),
                        ..plan.clone()
                    })
                    .collect();
                job.op = EditOp::Imported(plans);
            }
        }
        let _ = self.launch_edit(
            &ui,
            sets,
            refused,
            jobs,
            HandTool::Add,
            how,
            None,
            Some(join),
        );
    }

    /// What a press of the Select tool at `press` grabs to drag (#377): an
    /// elbow of a picked support near it, else the trunk of the picked
    /// support there. Nothing off the picked supports, or on one that hangs
    /// away from its elbows.
    fn part_grab(&self, press: &DragPoint) -> Option<DragState> {
        let at = press.support?.map(f64::from);
        for (drawn, ids) in self.chosen_now() {
            let Some(id) = support_picked_at(&drawn.session, at) else {
                continue;
            };
            if !ids.contains(&id) {
                continue;
            }
            let parts = drawn.session.parts_of(&id)?;
            let elbow = drawn.session.elbow_near(&id, at, ELBOW_GRAB_MM);
            let grab = match &elbow {
                Some(e) => parts.elbows.iter().find(|(x, _)| x == e)?.1,
                None if parts.trunk_mm.is_some() => at,
                // A tip on a knot has no shaft to move.
                None => return None,
            };
            let free = elbow.is_some() || parts.foot.is_none();
            return Some(DragState::MovePart {
                drawn,
                id,
                grab,
                elbow,
                free,
                to: None,
            });
        }
        None
    }

    /// The supports picked are routed again from their tips (#377): each on
    /// a route of its own (`own`), or hung from the nearest support. One
    /// Undo step for each set.
    fn reroute_chosen(self: &Rc<Self>, own: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        for (drawn, ids) in self.chosen_now() {
            if !ids.is_empty() {
                self.start_gesture(&ui, drawn, EditOp::Reroute(ids, own), None);
            }
        }
    }

    /// The supports picked after the first that stands on a trunk are
    /// joined onto that trunk (#377), each set as one Undo step.
    fn join_chosen(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        for (drawn, ids) in self.chosen_now() {
            let Some(host) = ids.iter().find(|id| drawn.session.is_trunk(id)).cloned() else {
                continue;
            };
            let tips: Vec<[f64; 3]> = ids
                .iter()
                .filter(|id| **id != host)
                .filter_map(|id| drawn.session.tip_point(id))
                .collect();
            if !tips.is_empty() {
                self.start_gesture(&ui, drawn, EditOp::Join(tips, host), None);
            }
        }
    }

    /// The tip size changed: the card says which, and the preview under the
    /// cursor is made again with it.
    fn on_tip_size_changed(self: &Rc<Self>) {
        if let Some(ui) = self.ui.upgrade() {
            self.refresh_tip_label(&ui, None);
            if Self::on_hand_page(&ui) {
                let o = self.options.borrow().clone();
                self.push_shape(&ui, &o);
            }
        }
        if let Some((point, shift)) = self.hover_at.get() {
            self.on_tool_hover_with(Some(point), shift);
        }
    }

    /// The Add card's line on the tip it places, for `session`'s preset (the
    /// first set in force's when `None`).
    fn refresh_tip_label(&self, ui: &MainWindow, session: Option<&SupportSession>) {
        let first = session
            .is_none()
            .then(|| self.sets_in_force().into_iter().next())
            .flatten();
        let session = session.or(first.as_ref().map(|d| d.session.as_ref()));
        let picked = hand_tip_size(ui.get_support_tip_size());
        let label = match picked.as_deref().and_then(|id| tip_mm(session, id)) {
            Some(mm) => ctr!("Inspector", "{} mm tip", mm_text(mm)),
            None => match session
                .and_then(|s| s.preset_hand_size().map(|id| (s, id)))
                .and_then(|(s, id)| tip_mm(Some(s), id))
            {
                Some(mm) => ctr!("Inspector", "{} mm tip, the preset's", mm_text(mm)),
                None => ctr!("Inspector", "Tip sized by the preset"),
            },
        };
        if ui.get_support_tip_label() != label {
            ui.set_support_tip_label(label);
        }
    }

    /// The Add tool's line under the cursor, and whether it warns.
    fn show_add_note(&self, (line, warns): (slint::SharedString, bool)) {
        if let Some(ui) = self.ui.upgrade() {
            if ui.get_support_add_note() != line {
                ui.set_support_add_note(line);
            }
            ui.set_support_add_note_warns(warns);
        }
    }

    /// Draws the worker's newest answer: the support a click adds (green, or
    /// red where nothing can hold that point) or what it removes (red). While
    /// the cursor moves the answer is for a point it has already left, and is
    /// drawn all the same: the preview follows the cursor a step behind
    /// instead of waiting for it to stop.
    fn take_hover_answer(&self) {
        let answer = {
            let Ok(mut slot) = self.hover_worker.answer.lock() else {
                return;
            };
            match slot.as_ref() {
                Some(a) if a.generation > self.hover_shown.get() => slot.take(),
                _ => return,
            }
        };
        let Some(answer) = answer else {
            return;
        };
        self.hover_shown.set(answer.generation);
        if answer.generation == self.hover_generation.get() {
            self.hover_poll.stop();
        }
        let mut pieces = Vec::new();
        if let Some(preview) = &answer.preview {
            let tint = match answer.tool {
                _ if answer.pending => OverlayTint::Pending,
                4 | 6 => OverlayTint::Pending,
                _ if preview.valid => OverlayTint::Valid,
                _ => OverlayTint::Invalid,
            };
            // A surface no tip may touch draws no support (#491): a red dot
            // where the cursor is says so, so a spot that cannot take one is
            // never just nothing.
            if !preview.mesh.faces().is_empty() {
                pieces.push(Arc::new(overlay_of(&preview.mesh, tint)));
            } else if !preview.valid && matches!(answer.tool, 1 | 3 | 10) {
                pieces.push(Arc::new(refused_dot(answer.point)));
            }
            if let Some(taken) = &preview.taken {
                pieces.push(Arc::new(overlay_of(taken, OverlayTint::Invalid)));
            }
        } else if answer.tool == 1 {
            // An addition the engine turned down altogether: the same dot.
            pieces.push(Arc::new(refused_dot(answer.point)));
        }
        match (&answer.preview, answer.tool) {
            (Some(preview), 1) => self.show_add_note(preview_line(preview, answer.how.attach)),
            (Some(preview), 3 | 10) => self.show_add_note(move_line(preview)),
            (Some(preview), 7) if !answer.pending => self.show_add_note(brace_line(preview)),
            (_, 6 | 7) if self.brace_from.borrow().is_some() => {
                self.show_add_note(brace_first_line());
            }
            (None, 1) => self.show_add_note((
                ctr!("SupportDialog", "Cannot place it: no support can reach that point."),
                true,
            )),
            // A box's or a brush's line says how many go, as they pick.
            (_, 5) => {}
            _ => self.show_add_note(Default::default()),
        }
        *self.last_ghost.borrow_mut() = pieces;
        self.show_hover_overlay();
        let older = self.last_preview.borrow_mut().replace(answer);
        // It holds a whole copy of the supports, slow to let go of on a big
        // set: a thread of its own does.
        if let Some(older) = older {
            let _ = self.hover_worker.discard.send(older);
        }
    }

    /// The worker's last ghost, the only hover overlay.
    fn show_hover_overlay(&self) {
        let pieces: Vec<Arc<OverlayMesh>> = self.last_ghost.borrow().clone();
        self.ctrl
            .set_preview_overlay((!pieces.is_empty()).then(|| pieces.into()));
    }

    /// Has the worker build the removal index of every set's session now in
    /// force, so the first removal hover on one answers quickly.
    fn warm_worker(&self) {
        let mut sets = self.sets_in_force().into_iter();
        let Some(first) = sets.next() else {
            return;
        };
        let _ = self.hover_worker.asks.send(HoverAsk {
            generation: 0,
            base: first.session.clone(),
            from: session_key(&first),
            point: [0.0; 3],
            tool: 0,
            how: AddOptions::default(),
            tip: None,
            tips: Arc::from([]),
            others: sets.map(|drawn| drawn.session).collect(),
            brace_from: None,
            part: None,
        });
    }

    /// A click of the armed tool, made then and there: what the tests drive.
    #[cfg(test)]
    fn on_tool_click(&self, point: [f32; 3]) {
        self.on_tool_click_now(point, false);
    }

    /// [`Self::on_tool_click`], with Shift held or not.
    #[cfg(test)]
    fn on_tool_click_now(&self, point: [f32; 3], forced: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let tool = ui.get_support_tool();
        if tool == 0 || self.remove_imported_body(point) {
            return;
        }
        let (tool, how) = self.click_tool(&ui, tool, forced);
        self.hand_edits(&ui, tool, &[point], &how, PointPlans::default());
    }

    /// A click of the armed tool at `point`, `forced` for a Shift+click of
    /// Add (#3). The edit is made off the UI thread: drawing a big set again
    /// takes a good part of a second. A click that comes while another is
    /// being made waits for it.
    fn on_tool_click_with(self: &Rc<Self>, point: [f32; 3], forced: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let tool = ui.get_support_tool();
        if tool == 0 {
            return;
        }
        if tool == 4 {
            self.brace_click(&ui, point);
            return;
        }
        if tool == 5 {
            self.select_click(&ui, Some(point), forced);
            return;
        }
        if tool == 6 {
            self.line_click(&ui, point);
            return;
        }
        if self.pending_click.borrow().is_none() && self.remove_imported_body(point) {
            return;
        }
        let (tool, how) = self.click_tool(&ui, tool, forced);
        if self.pending_click.borrow().is_some() {
            self.queued_clicks
                .borrow_mut()
                .push_back(QueuedClick::Click { tool, point, how });
            return;
        }
        self.start_click(&ui, tool, point, how);
    }

    /// The set in force whose trunk or branch the brace tool takes at
    /// `point` (world mm, on the supports): the nearest. Sets whose
    /// supports' box is out of reach are not asked, so a hover asks one set
    /// at most, as a rule, and with one in reach the worker's preview finds
    /// whether a support is there.
    fn set_of_brace(&self, point: [f32; 3]) -> Option<DrawnSet> {
        let p = point.map(f64::from);
        let mut near: Vec<DrawnSet> = self
            .document
            .with_scene(|spatial| {
                sets_in_force(spatial, &self.sessions)
                    .into_iter()
                    .filter(|drawn| {
                        spatial
                            .leaf(drawn.leaf)
                            .and_then(|q| q.transformed_aabb(q.world_transform()))
                            .is_some_and(|aabb| box_distance(&aabb, p) <= SET_REACH_MM)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if near.len() <= 1 {
            return near.pop();
        }
        near.into_iter()
            .filter_map(|drawn| {
                let end = drawn.session.brace_end_near(p)?;
                let d = (0..3).map(|k| (end.at[k] - p[k]).powi(2)).sum::<f64>();
                Some((d, drawn))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, drawn)| drawn)
    }

    /// A click of the brace tool at `point` (#3): the first picks the
    /// support the brace starts from, there; the second lays the brace to
    /// the support clicked, made off the UI thread as one undoable edit. A
    /// second click its preview showed red says why and keeps the first.
    fn brace_click(self: &Rc<Self>, ui: &MainWindow, point: [f32; 3]) {
        let p = point.map(f64::from);
        let first = self.brace_from.borrow().clone();
        let Some((drawn, from)) = first else {
            let end = self
                .set_of_brace(point)
                .and_then(|drawn| Some((drawn.session.brace_end_near(p)?, drawn)));
            let Some((end, drawn)) = end else {
                ui.set_supports_last_run(ctr!(
                    "SupportDialog",
                    "No trunk or branch there to brace from."
                ));
                return;
            };
            crate::breadcrumbs::note("supports: brace started");
            *self.brace_from.borrow_mut() = Some((drawn, end));
            self.show_add_note(brace_first_line());
            self.on_tool_hover_with(Some(point), false);
            return;
        };
        let in_force = self
            .sets_in_force()
            .into_iter()
            .any(|d| session_key(&d) == session_key(&drawn));
        if !in_force {
            self.drop_brace();
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "The supports changed since the first end was picked; pick it again."
            ));
            return;
        }
        // Off any support, or on the first again: the first end stays.
        let to = drawn.session.brace_end_near(p);
        if to.as_ref().is_none_or(|to| to.shaft == from.shaft) {
            self.show_add_note((
                ctr!(
                    "SupportDialog",
                    "Click another trunk or branch of the same supports to brace it to."
                ),
                true,
            ));
            return;
        }
        // The preview of this very point: red says why, and the first end
        // stays; green is the brace already laid.
        let previewed = self.last_preview.borrow_mut().take().filter(|answer| {
            answer.tool == 7
                && !answer.pending
                && answer.point == point
                && answer.from == session_key(&drawn)
                && answer.brace_from.as_ref() == Some(&from)
        });
        if let Some(answer) = &previewed
            && let Some(preview) = &answer.preview
            && !preview.valid
        {
            let line = brace_line(preview);
            ui.set_supports_last_run(line.0.clone());
            self.show_add_note(line);
            return;
        }
        self.brace_from.borrow_mut().take();
        self.show_add_note(Default::default());
        let edited = previewed.and_then(|answer| answer.edited);
        self.start_gesture(ui, drawn, EditOp::Brace(from, point), edited);
    }

    /// Drops the line tool's first point, and the dots: whether there was
    /// one (#178).
    fn drop_line(&self) -> bool {
        if self.line_from.take().is_none() {
            return false;
        }
        self.ctrl.set_preview_overlay(None);
        self.show_add_note(Default::default());
        true
    }

    /// Where the supports of a row from the line's first point to `to` go
    /// (#178): along the model's surface between them, as seen.
    fn line_row(&self, from: [f32; 3], to: [f32; 3]) -> Vec<[f32; 3]> {
        line_points(&self.ctrl.surface_path(from, to), self.line_spacing.get())
    }

    /// The line tool's hover: once the first point is in, a dot where each
    /// support of the row would go.
    fn line_hover(&self, point: Option<[f32; 3]>) {
        self.hover_poll.stop();
        let (Some(from), Some(to)) = (self.line_from.get(), point) else {
            if self.line_from.get().is_none() {
                self.ctrl.set_preview_overlay(None);
            }
            return;
        };
        let row = self.line_row(from, to);
        self.ctrl.set_preview_overlay(line_dots(&row));
        self.show_add_note((
            ctr!(
                "SupportDialog",
                "Click where the row ends: {n} support.",
                "Click where the row ends: {n} supports.",
                row.len()
            ),
            false,
        ));
    }

    /// A click of the line tool (#178): the first sets where the row
    /// starts, the second places a support at every dot, as one edit.
    fn line_click(self: &Rc<Self>, ui: &MainWindow, point: [f32; 3]) {
        let Some(from) = self.line_from.get() else {
            crate::breadcrumbs::note("supports: line started");
            self.line_from.set(Some(point));
            self.ctrl.set_preview_overlay(line_dots(&[point]));
            self.show_add_note((ctr!("SupportDialog", "Click where the row ends."), false));
            return;
        };
        let row = self.line_row(from, point);
        self.line_from.set(None);
        self.ctrl.set_preview_overlay(None);
        self.show_add_note(Default::default());
        self.place_row(ui, row);
    }

    /// Places a support at each of `row`, one edit, as Add places one, but
    /// sized by the preset of the supports they join (the Auto preset picked
    /// for a model with none yet), never by a hand size.
    fn place_row(self: &Rc<Self>, ui: &MainWindow, row: Vec<[f32; 3]>) {
        crate::breadcrumbs::note(&format!("supports: line of {}", row.len()));
        let (tool, _) = self.click_tool(ui, 1, false);
        let how = AddOptions::default();
        if self.pending_click.borrow().is_some() {
            self.queued_clicks
                .borrow_mut()
                .push_back(QueuedClick::Points {
                    tool,
                    points: row,
                    how,
                });
            return;
        }
        self.start_points(ui, tool, &row, how);
    }

    /// Places a support on every node of a grid over each patch painted
    /// with the Area tool (#177), as one edit, the spacing the field's.
    fn fill_painted(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let spacing = self.line_spacing.get();
        let mut row: Vec<[f32; 3]> = crate::support_area::painted_patches(&self.document)
            .iter()
            .flat_map(|(_, patch)| crate::support_area::grid_on(patch, spacing))
            .collect();
        if row.is_empty() {
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "Paint where the supports go first, or Shift+click a face to take its plane."
            ));
            return;
        }
        row.truncate(crate::support_area::MOST_POINTS);
        self.place_row(&ui, row);
    }

    /// Takes the Area tool's paint off every model (#177).
    fn clear_painted(&self) {
        for leaf in crate::support_area::painted_leaves(&self.document) {
            self.area_brush.clear(leaf);
        }
    }

    fn on_area_brush_size(&self, mm: f32) {
        self.area_brush.set_radius_mm(mm);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_area_brush(self.area_brush.radius_mm());
        }
    }

    fn on_line_spacing(&self, mm: f32) {
        let mm = f64::from(mm).clamp(0.3, 50.0);
        self.line_spacing.set(mm);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_line_spacing(mm as f32);
        }
    }

    /// Drops the brace tool's first end, and what shows it: returns whether
    /// there was one.
    fn drop_brace(&self) -> bool {
        if self.brace_from.borrow_mut().take().is_none() {
            return false;
        }
        crate::breadcrumbs::note("supports: brace dropped");
        self.end_previews();
        self.last_ghost.borrow_mut().clear();
        self.ctrl.set_preview_overlay(None);
        self.show_add_note(Default::default());
        true
    }

    /// Starts the edit of a click on a worker thread; [`Self::poll_click`]
    /// puts it in the scene once it is made, if the scene is still the one
    /// clicked. Undo before then drops it ([`Self::drop_click`]).
    fn start_click(
        self: &Rc<Self>,
        ui: &MainWindow,
        tool: HandTool,
        point: [f32; 3],
        how: AddOptions,
    ) {
        self.start_points(ui, tool, &[point], how);
    }

    /// [`Self::start_click`] for several points at once, one edit (#178).
    fn start_points(
        self: &Rc<Self>,
        ui: &MainWindow,
        tool: HandTool,
        points: &[[f32; 3]],
        how: AddOptions,
    ) {
        if points.is_empty() {
            return;
        }
        let mut refused = Vec::new();
        let Ok(Prepared { sets, jobs }) = self.prepare_edit(ui, tool, points, &how, &mut refused)
        else {
            self.queued_clicks.borrow_mut().clear();
            return;
        };
        let _ = self.launch_edit(ui, sets, refused, jobs, tool, how, None, None);
    }

    /// The edit of a drag (#3) on `drawn`'s set, made off the UI thread as
    /// a click is, or after the edit being made; `previewed` is the session
    /// its preview left, the edit in it.
    /// Reinforces the supports the frame found weak (#247): each one's tip
    /// or shaft, whichever is worst off, made wider in place, as one step to
    /// undo. Tells how many it took on, and how many it could not.
    pub(crate) fn reinforce(
        self: &Rc<Self>,
        ui: &MainWindow,
        weak: crate::stress_view::WeakSupports,
    ) {
        if self.pending_click.borrow().is_some() || self.handle.borrow().is_some() {
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "Wait for the supports being made, then reinforce them."
            ));
            return;
        }
        let in_force = self.sets_in_force();
        let (mut sets, mut jobs, mut planned, mut skipped) = (Vec::new(), Vec::new(), 0, 0);
        for (leaf, list) in weak {
            let Some(drawn) = in_force.iter().find(|drawn| drawn.leaf == leaf) else {
                skipped += list.len();
                continue;
            };
            let plan: Vec<(String, PartSizes)> = list
                .iter()
                .filter_map(|(id, u, part)| {
                    let sizes = reinforced_sizes(&drawn.session, id, *u, *part);
                    if sizes.is_none() {
                        skipped += 1;
                    }
                    sizes.map(|sizes| (id.clone(), sizes))
                })
                .collect();
            if plan.is_empty() {
                continue;
            }
            planned += plan.len();
            jobs.push(EditJob {
                start: EditStart::Base(drawn.session.clone()),
                op: EditOp::Reinforce(plan),
                leaf,
                plans: PointPlans::default(),
            });
            sets.push(drawn.clone());
        }
        crate::breadcrumbs::note(&format!(
            "supports: reinforce {planned}, {skipped} with nothing to widen"
        ));
        if jobs.is_empty() {
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "Nothing to reinforce: the weak spots are braces or bars between parts."
            ));
            return;
        }
        ui.set_supports_last_run(ctr!(
            "SupportDialog",
            "Reinforcing {} supports: their tips or shafts made wider.",
            planned
        ));
        let _ = self.launch_edit(
            ui,
            sets,
            Vec::new(),
            jobs,
            HandTool::Remove,
            AddOptions::default(),
            None,
            None,
        );
    }

    fn start_gesture(
        self: &Rc<Self>,
        ui: &MainWindow,
        drawn: DrawnSet,
        op: EditOp,
        previewed: Option<SupportSession>,
    ) {
        crate::breadcrumbs::note(&format!("supports: gesture {op:?}"));
        if self.pending_click.borrow().is_some() {
            self.queued_clicks
                .borrow_mut()
                .push_back(QueuedClick::Gesture {
                    leaf: drawn.leaf,
                    op,
                });
            return;
        }
        let in_force = self
            .sets_in_force()
            .into_iter()
            .any(|d| session_key(&d) == session_key(&drawn));
        if !in_force || self.handle.borrow().is_some() {
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "The scene changed before the support was placed; nothing was changed."
            ));
            return;
        }
        let start = match previewed {
            Some(edited) => EditStart::Previewed(Box::new(edited)),
            None => EditStart::Base(drawn.session.clone()),
        };
        let jobs = vec![EditJob {
            start,
            op,
            leaf: drawn.leaf,
            plans: PointPlans::default(),
        }];
        let _ = self.launch_edit(
            ui,
            vec![drawn],
            Vec::new(),
            jobs,
            HandTool::Remove,
            AddOptions::default(),
            None,
            None,
        );
    }

    /// Makes `jobs` on a worker thread; [`Self::poll_click`] puts the edit
    /// in the scene once it is made, if the scene is still the one it was
    /// made on. Undo before then drops it ([`Self::drop_click`]).
    /// `landing`, for an edit another panel asked for, hears how it ended;
    /// the returned watch tells how far the engine is. With no thread to
    /// make it, nothing is made, `landing` is dropped unheard and the points
    /// refused so far come back.
    #[allow(clippy::too_many_arguments)]
    fn launch_edit(
        self: &Rc<Self>,
        ui: &MainWindow,
        sets: Vec<DrawnSet>,
        refused: Refusals,
        jobs: Vec<EditJob>,
        tool: HandTool,
        how: AddOptions,
        landing: Option<Landing>,
        join: Option<SceneRevision>,
    ) -> Result<Arc<EditWatch>, Refusals> {
        let (send, done) = std::sync::mpsc::channel();
        let watch = Arc::new(EditWatch::default());
        let job = {
            let watch = watch.clone();
            move || {
                let _ = send.send(compute_edit(jobs, tool, &how, &watch));
            }
        };
        // The router recurses: the same room as the hover worker.
        let spawned = std::thread::Builder::new()
            .name("support-hand-edit".into())
            .stack_size(16 << 20)
            .spawn(job);
        if spawned.is_err() {
            let why = ctr!(
                "SupportDialog",
                "That edit failed inside the support engine; nothing was changed."
            );
            ui.set_supports_last_run(why);
            return Err(refused);
        }
        let shows_preview = landing.is_none();
        *self.pending_click.borrow_mut() = Some(PendingClick {
            revision: self.document.scene_revision(),
            sets,
            refused,
            done,
            watch: watch.clone(),
            landing,
            join,
        });
        let weak = Rc::downgrade(self);
        self.document.set_pending_edit(Some(Rc::new(move || {
            if let Some(run) = weak.upgrade() {
                run.drop_click(
                    ctr!(
                        "SupportDialog",
                        "Undone before it was placed; nothing was changed."
                    ),
                    EditDropped::Undone,
                );
            }
        })));
        // The preview stays on screen until the edit replaces it. Fix All's
        // points were never previewed.
        if shows_preview {
            self.show_hover_overlay();
        }
        let weak = Rc::downgrade(self);
        self.click_poll
            .start(slint::TimerMode::Repeated, HOVER_POLL, move || {
                if let Some(run) = weak.upgrade() {
                    run.poll_click();
                }
            });
        Ok(watch)
    }

    /// Puts a click's edit in the scene once it is made, as one undoable
    /// step, or drops it when the scene moved on meanwhile; then starts the
    /// next click waiting.
    fn poll_click(self: &Rc<Self>) {
        let made = {
            let pending = self.pending_click.borrow();
            let Some(pending) = pending.as_ref() else {
                self.click_poll.stop();
                return;
            };
            match pending.done.try_recv() {
                Ok(computed) => Some(computed),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
            }
        };
        self.click_poll.stop();
        self.document.set_pending_edit(None);
        let Some(pending) = self.pending_click.borrow_mut().take() else {
            return;
        };
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(computed) = made else {
            self.queued_clicks.borrow_mut().clear();
            let why = ctr!(
                "SupportDialog",
                "That edit failed inside the support engine; nothing was changed."
            );
            ui.set_supports_last_run(why.clone());
            if let Some(landing) = pending.landing {
                landing(PointsEdit::Done(HandEdit::Failed(why), pending.refused));
            }
            return;
        };
        if self.document.scene_revision() != pending.revision || self.handle.borrow().is_some() {
            // Another edit, or a run, came first: this one was made on a
            // scene that is gone.
            crate::breadcrumbs::note("supports: click dropped, the scene moved on");
            self.queued_clicks.borrow_mut().clear();
            self.last_ghost.borrow_mut().clear();
            self.ctrl.set_preview_overlay(None);
            ui.set_supports_last_run(ctr!(
                "SupportDialog",
                "The scene changed before the support was placed; nothing was changed."
            ));
            if let Some(landing) = pending.landing {
                landing(PointsEdit::Dropped(EditDropped::SceneChanged));
            }
            return;
        }
        let mut refused = pending.refused;
        let outcome = self.commit_edit(&ui, pending.sets, computed, &mut refused, pending.join);
        // The edit is in: its preview would lie over it.
        self.last_ghost.borrow_mut().clear();
        self.ctrl.set_preview_overlay(None);
        // The panel that asked for it goes on (Fix All slices again) before
        // a click that came meanwhile is made.
        if let Some(landing) = pending.landing {
            landing(PointsEdit::Done(outcome, refused));
        }
        let next = self.queued_clicks.borrow_mut().pop_front();
        match next {
            Some(QueuedClick::Click { tool, point, how }) => {
                self.start_click(&ui, tool, point, how);
            }
            Some(QueuedClick::Points { tool, points, how }) => {
                self.start_points(&ui, tool, &points, how);
            }
            Some(QueuedClick::Gesture { leaf, op }) => {
                let drawn = self
                    .sets_in_force()
                    .into_iter()
                    .find(|drawn| drawn.leaf == leaf);
                match drawn {
                    Some(drawn) => self.start_gesture(&ui, drawn, op, None),
                    None => self.queued_clicks.borrow_mut().clear(),
                }
            }
            None => {}
        }
    }

    /// Drops the click being made and those waiting, saying why: nothing of
    /// them reaches the scene. The panel that asked for the edit, if one
    /// did, hears it was dropped, and `reason`.
    fn drop_click(&self, why: slint::SharedString, reason: EditDropped) {
        self.click_poll.stop();
        self.queued_clicks.borrow_mut().clear();
        let Some(pending) = self.pending_click.borrow_mut().take() else {
            return;
        };
        // The engine stops at its next point: what it makes is not read.
        pending.watch.stop();
        // Nothing is pending any more when the panel hears it: Fix All takes
        // its earlier passes back then (#163).
        self.document.set_pending_edit(None);
        if let Some(landing) = pending.landing {
            landing(PointsEdit::Dropped(reason));
        }
        self.last_ghost.borrow_mut().clear();
        self.ctrl.set_preview_overlay(None);
        crate::breadcrumbs::note("supports: click dropped");
        if let Some(ui) = self.ui.upgrade() {
            ui.set_supports_last_run(why);
        }
    }

    /// Adds a support at every point of `points` (world mm), whose tip
    /// touches the model there or at the nearest point next to it the engine
    /// can hold (`SupportSession::add_tip_near`), as one undoable edit of
    /// the supports
    /// generated last, or a notice when there are none to edit. For the
    /// Review card's "Add support here" and Fix All (#156), whose points a
    /// check picked. Points the engine refuses are skipped, and returned by
    /// index with why; the edit fails when none took.
    #[cfg(test)]
    pub(crate) fn add_supports_at(&self, points: &[[f32; 3]]) -> (HandEdit, Refusals) {
        self.add_tips_at(points, PointPlans::default())
    }

    /// [`Self::add_supports_at`], with what is known of each point: the
    /// tips of an island (#437).
    pub(crate) fn add_tips_at(
        &self,
        points: &[[f32; 3]],
        plans: PointPlans,
    ) -> (HandEdit, Refusals) {
        let Some(ui) = self.ui.upgrade() else {
            return (HandEdit::Notified, Vec::new());
        };
        let how = AddOptions::default();
        self.hand_edits(&ui, HandTool::AddNear, points, &how, plans)
    }

    /// [`Self::add_supports_at`] made off the UI thread, for Fix All
    /// (#390): placing many points on big models takes seconds. The points
    /// go to their sets here; `landing` hears how the edit ended once the
    /// engine is done, or that it was dropped: by
    /// [`Self::cancel_points_edit`], by Undo, or because the scene changed
    /// meanwhile, which it then left as it was. Returns the engine's watch,
    /// or how the edit ended then and there (`landing` is not called), as
    /// [`Self::add_supports_at`] tells: with no set to edit, while supports
    /// are being generated, or while another edit is being made. With
    /// `join`, the edit goes into the newest undo entry, which the scene is
    /// at (that revision), instead of one of its own: a Fix All run's later
    /// passes undo with its first (#163).
    pub(crate) fn start_supports_at(
        self: &Rc<Self>,
        points: &[[f32; 3]],
        plans: PointPlans,
        join: Option<SceneRevision>,
        landing: impl FnOnce(PointsEdit) + 'static,
    ) -> Result<Arc<EditWatch>, (HandEdit, Refusals)> {
        let Some(ui) = self.ui.upgrade() else {
            return Err((HandEdit::Notified, Vec::new()));
        };
        if points.is_empty() {
            return Err((HandEdit::Notified, Vec::new()));
        }
        if self.editing() {
            return Err((
                HandEdit::Failed(ctr!(
                    "SupportDialog",
                    "Supports are being edited; try again when that is done."
                )),
                Vec::new(),
            ));
        }
        let tool = HandTool::AddNear;
        let how = AddOptions::default();
        let mut refused = Vec::new();
        let Prepared { sets, mut jobs } =
            match self.prepare_edit(&ui, tool, points, &how, &mut refused) {
                Ok(prepared) => prepared,
                Err(outcome) => return Err((outcome, refused)),
            };
        for job in &mut jobs {
            job.plans = plans.clone();
        }
        crate::breadcrumbs::note(&format!(
            "supports: {} points made off the UI thread, {} sets",
            points.len(),
            jobs.len()
        ));
        // Spawning fails only when the system is out of threads.
        self.launch_edit(
            &ui,
            sets,
            refused,
            jobs,
            tool,
            how,
            Some(Box::new(landing)),
            join,
        )
        .map_err(|refused| {
            (
                HandEdit::Failed(ctr!(
                    "SupportDialog",
                    "That edit failed inside the support engine; nothing was changed."
                )),
                refused,
            )
        })
    }

    /// Whether a hand edit is being made off the UI thread.
    pub(crate) fn editing(&self) -> bool {
        self.pending_click.borrow().is_some()
    }

    /// Cancel of an edit [`Self::start_supports_at`] made: the engine stops,
    /// and nothing of it reaches the scene.
    pub(crate) fn cancel_points_edit(&self) {
        let ours = self
            .pending_click
            .borrow()
            .as_ref()
            .is_some_and(|pending| pending.landing.is_some());
        if ours {
            self.drop_click(
                ctr!("SupportDialog", "Cancelled; nothing was changed."),
                EditDropped::Cancelled,
            );
        }
    }

    /// Edits the supports at every point of `points` with `tool`, an Add
    /// made as `how` says, as one undoable edit, then and there. A point the
    /// engine refuses is skipped and returned, by index, with why; the edit
    /// fails when none took. `plans` is what is known of each point.
    fn hand_edits(
        &self,
        ui: &MainWindow,
        tool: HandTool,
        points: &[[f32; 3]],
        how: &AddOptions,
        plans: PointPlans,
    ) -> (HandEdit, Refusals) {
        if points.is_empty() {
            return (HandEdit::Notified, Vec::new());
        }
        let mut refused = Vec::new();
        let outcome = match self.prepare_edit(ui, tool, points, how, &mut refused) {
            Ok(Prepared { sets, mut jobs }) => {
                for job in &mut jobs {
                    job.plans = plans.clone();
                }
                let computed = compute_edit(jobs, tool, how, &EditWatch::default());
                self.commit_edit(ui, sets, computed, &mut refused, None)
            }
            Err(outcome) => outcome,
        };
        (outcome, refused)
    }

    /// The part of a hand edit made on the UI thread before the engine
    /// works: each point goes to the set it acts on, and a click on the point
    /// last previewed takes the preview's edit. Points no set takes are
    /// refused; with none taken, the edit ends here, told.
    fn prepare_edit(
        &self,
        ui: &MainWindow,
        tool: HandTool,
        points: &[[f32; 3]],
        how: &AddOptions,
        refused: &mut Refusals,
    ) -> Result<Prepared, HandEdit> {
        if self.handle.borrow().is_some() {
            return Err(HandEdit::Failed(ctr!(
                "SupportDialog",
                "Supports are being generated; try again when they are done."
            )));
        }
        let failed = |message: slint::SharedString| {
            ui.set_supports_last_run(message.clone());
            HandEdit::Failed(message)
        };
        // `key`: the notice offers "Don't show again"; a notice that says the
        // edit is stale offers none.
        let notice = |key: Option<MessageKey>, heading: slint::SharedString, message: String| {
            let texts = move || InfoTexts {
                heading: heading.clone(),
                message: message.as_str().into(),
                details: Default::default(),
            };
            match key {
                Some(key) => self.dialogs.show_notice(key, texts),
                None => self.dialogs.show_info(texts),
            }
        };
        // No borrow of the sessions is held across the edit: the scene events
        // it fires, and the hover preview, read them.
        crate::breadcrumbs::note(&format!(
            "supports: click tool={tool:?} points={} attach={} size={:?}",
            points.len(),
            how.attach,
            how.size
        ));
        // Each point goes to the set it acts on.
        let mut groups: Vec<(DrawnSet, Vec<IndexedPoint>)> = Vec::new();
        let mut unset = Vec::new();
        for (i, point) in points.iter().enumerate() {
            // A model with no set starts one (#444), a single one for all
            // its points.
            let bare = self.bare_model_at(*point, tool);
            if let Some(group) = bare.and_then(|model| {
                groups
                    .iter_mut()
                    .find(|(d, _)| d.unborn && d.model == model)
            }) {
                group.1.push((i, *point));
                continue;
            }
            let drawn = self.set_at(*point, tool).or_else(|| self.unborn_set(bare?));
            let Some(drawn) = drawn else {
                unset.push(i);
                continue;
            };
            match groups.iter_mut().find(|(d, _)| d.leaf == drawn.leaf) {
                Some((_, group)) => group.push((i, *point)),
                None => groups.push((drawn, vec![(i, *point)])),
            }
        }
        if groups.is_empty() {
            return Err(match self.no_set_notice(tool, points[0]) {
                NoSet::Notice(key, heading, message) => {
                    notice(key, heading, message.to_string());
                    HandEdit::Notified
                }
                // Brought back in step with the model and tried again, or
                // made again on its own (#526); the card only when neither.
                NoSet::OutOfDate(leaf) => match self.heal_stale(leaf) {
                    Healed::InForce => return self.prepare_edit(ui, tool, points, how, refused),
                    Healed::Redoing => failed(ctr!(
                        "SupportDialog",
                        "These supports are being made again for the model as it is now; place it again in a moment."
                    )),
                    Healed::No => {
                        self.show_out_of_date(leaf);
                        HandEdit::Notified
                    }
                },
                NoSet::Failed(message) => failed(message),
            });
        }
        for i in unset {
            refused.push((i, refusal(&EditError::NothingThere)));
        }
        // The preview of this very point, made the same way, already placed
        // it: take that.
        let mut previewed = self.last_preview.borrow_mut().take().and_then(|answer| {
            (points == [answer.point]
                && tool.click() == Some(answer.tool)
                && answer.how == *how
                && answer.from == session_key(&groups[0].0))
            .then_some(answer.edited)
            .flatten()
        });
        let mut sets = Vec::with_capacity(groups.len());
        let mut jobs = Vec::with_capacity(groups.len());
        for (drawn, points) in groups {
            let start = match previewed.take() {
                Some(edited) => EditStart::Previewed(Box::new(edited)),
                None => EditStart::Base(drawn.session.clone()),
            };
            jobs.push(EditJob {
                start,
                op: EditOp::Points(points),
                leaf: drawn.leaf,
                plans: PointPlans::default(),
            });
            sets.push(drawn);
        }
        Ok(Prepared { sets, jobs })
    }

    /// The part of a hand edit made on the UI thread once the engine is
    /// done: the edited sets go into the scene as one undoable step (with
    /// `join`, into the newest one, at that revision), and the Supports
    /// tab's last-run line says how it went. `refused` gains the points the
    /// engine refused.
    fn commit_edit(
        &self,
        ui: &MainWindow,
        sets: Vec<DrawnSet>,
        computed: Computed,
        refused: &mut Refusals,
        join: Option<SceneRevision>,
    ) -> HandEdit {
        let failed = |message: slint::SharedString| {
            ui.set_supports_last_run(message.clone());
            HandEdit::Failed(message)
        };
        let last = computed.refused.last().map(|(_, err)| err.clone());
        refused.extend(computed.refused.iter().map(|(i, err)| (*i, refusal(err))));
        let reused = computed.reused;
        let mut edits = Vec::with_capacity(sets.len());
        for (drawn, outcome) in sets.into_iter().zip(computed.outcomes) {
            match outcome {
                JobOutcome::Panicked => {
                    return failed(ctr!(
                        "SupportDialog",
                        "That edit failed inside the support engine; nothing was changed."
                    ));
                }
                JobOutcome::Failed(err) => return failed(refusal(&err)),
                // Nothing of this set took.
                JobOutcome::Untouched => continue,
                JobOutcome::Emptied => {
                    return failed(ctr!(
                        "SupportDialog",
                        "That was the last support; use Undo to bring it back."
                    ));
                }
                JobOutcome::Done(edited, mesh) => edits.push((drawn, *edited, mesh)),
            }
        }
        if edits.is_empty() {
            return failed(refusal(&last.unwrap_or(EditError::NothingThere)));
        }
        crate::breadcrumbs::note(&format!(
            "supports: edit drawn ({} triangles, reused preview {reused})",
            edits.iter().map(|(_, _, m)| m.faces().len()).sum::<usize>()
        ));
        let (born, edits): (Vec<_>, Vec<_>) =
            edits.into_iter().partition(|(drawn, _, _)| drawn.unborn);
        let swaps: Option<Vec<SceneEdit>> = edits
            .iter()
            .map(|(drawn, _, mesh)| {
                self.document
                    .with_scene(|spatial| leaf_mesh_swap(spatial.graph(), drawn.leaf, mesh.clone()))
                    .flatten()
            })
            .collect();
        let Some(mut swaps) = swaps else {
            return failed(ctr!(
                "SupportDialog",
                "The support mesh is not in the scene; generate supports again."
            ));
        };
        let swap = if swaps.len() == 1 {
            swaps.pop().expect("len == 1")
        } else {
            SceneEdit::Compound(swaps)
        };
        // Sets born from this edit come in with it, as one undo step: Undo
        // takes them out of the scene again.
        if !born.is_empty() {
            let prior = (!edits.is_empty()).then(|| swap.clone());
            return self.commit_born(ui, born, edits, prior, refused, join);
        }
        // A Fix All run's later pass joins the run's entry (#163). The
        // scene is still at that revision (the edit is dropped otherwise),
        // so it joins; should it not, the edit stands as its own step.
        let joined = match join {
            Some(revision) => self
                .document
                .complete_last_edit(revision, swap.clone(), &[]),
            None => Ok(false),
        };
        let applied = match joined {
            Ok(true) => Ok(()),
            Ok(false) => self.document.apply_edit(swap.clone()).map(|()| {
                self.document.push_edit(swap);
            }),
            Err(err) => Err(crate::scene_document::EditNotApplied::Rejected(err)),
        };
        match applied {
            Ok(()) => {
                crate::breadcrumbs::note("supports: edit applied");
                let mut roots = 0;
                for (drawn, edited, _) in edits {
                    roots += edited.state().collections.roots.len();
                    record(
                        &self.document,
                        &self.sessions,
                        drawn.leaf,
                        drawn.model,
                        Arc::new(edited),
                    );
                }
                refused.sort_by_key(|(i, _)| *i);
                let line = match refused.first() {
                    None => ctr!(
                        "Inspector",
                        "{n} support root, edited by hand.",
                        "{n} support roots, edited by hand.",
                        roots
                    ),
                    Some((_, why)) => ctr!(
                        "Inspector",
                        "Support roots: {}, edited by hand. No support could hold {n} point: {}.",
                        "Support roots: {}, edited by hand. No support could hold {n} points: {}.",
                        refused.len(),
                        roots,
                        why
                    ),
                };
                ui.set_supports_last_run(line);
                self.warm_worker();
                HandEdit::Applied
            }
            Err(err) => failed(ctr!(
                "SupportDialog",
                "The edit was refused: {}",
                format!("{err:?}")
            )),
        }
    }

    /// The rest of [`Self::commit_edit`] when the edit makes new sets (#444):
    /// their leaves go into the scene, after `prior`, the edit of the sets
    /// that were there.
    fn commit_born(
        &self,
        ui: &MainWindow,
        born: Vec<(DrawnSet, SupportSession, Arc<sc_mesh_core::IndexedMesh>)>,
        edits: Vec<(DrawnSet, SupportSession, Arc<sc_mesh_core::IndexedMesh>)>,
        prior: Option<SceneEdit>,
        refused: &mut Refusals,
        join: Option<SceneRevision>,
    ) -> HandEdit {
        // Into the newest undo entry, when the scene is still at it (#261).
        let joins = join.is_some_and(|revision| self.document.scene_revision() == revision);
        let fragment = SceneFragment::from_leaf_snapshots(
            born.iter()
                .map(|(_, _, mesh)| (mesh.clone(), Transform3D::Identity)),
        );
        let pasted = self
            .document
            .paste_fragment_replacing_all_after(prior, &fragment, None, &[]);
        let leaves = match pasted {
            Ok(leaves) if leaves.len() == born.len() => leaves,
            _ => {
                let why = ctr!(
                    "SupportDialog",
                    "The scene rejected the new supports; nothing was changed."
                );
                ui.set_supports_last_run(why.clone());
                return HandEdit::Failed(why);
            }
        };
        if joins {
            self.document.join_last_steps(2);
        }
        crate::breadcrumbs::note(&format!("supports: {} sets born", leaves.len()));
        let mut roots = 0;
        for (drawn, edited, _) in edits {
            roots += edited.state().collections.roots.len();
            record(
                &self.document,
                &self.sessions,
                drawn.leaf,
                drawn.model,
                Arc::new(edited),
            );
        }
        for (leaf, (drawn, edited, _)) in leaves.into_iter().zip(born) {
            roots += edited.state().collections.roots.len();
            record(
                &self.document,
                &self.sessions,
                leaf,
                drawn.model,
                Arc::new(edited),
            );
        }
        let options = self.options.borrow().clone();
        self.generated_raft_top.set(Some(options.raft_top_mm()));
        refused.sort_by_key(|(i, _)| *i);
        ui.set_supports_last_run(ctr!(
            "Inspector",
            "{n} support root, edited by hand.",
            "{n} support roots, edited by hand.",
            roots
        ));
        self.warm_worker();
        HandEdit::Applied
    }

    /// What a set born now is made with: the options the Supports tab has
    /// in force. A model with no set of its own takes them as they stand,
    /// nothing asked; the ones with a set keep theirs.
    fn born_options_now(&self) -> SupportOptions {
        self.options.borrow().clone()
    }

    /// The contact diameter of the tips of a set born now: what the review
    /// of a plate with no set plans with.
    pub(crate) fn born_tip_mm(&self) -> f64 {
        let mut request = self.request_for_printer();
        request.options = self.born_options_now();
        sc_geom::supports::tip_diameter_for(&request)
    }

    /// The model at `point` when `tool` adds and no set of it exists, not
    /// even one out of date: its first tip starts one (#444).
    fn bare_model_at(&self, point: [f32; 3], tool: HandTool) -> Option<NodeId> {
        if tool == HandTool::Remove {
            return None;
        }
        let p = point.map(f64::from);
        self.document
            .with_scene(|spatial| {
                let nearest = crate::raft_clash::model_bottoms(spatial)
                    .into_iter()
                    .filter_map(|(id, _)| {
                        let aabb = spatial
                            .leaf(id)?
                            .transformed_aabb(spatial.leaf(id)?.world_transform())?;
                        let distance = box_distance(&aabb, p);
                        (distance <= SET_REACH_MM).then_some((distance, id))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))?;
                sets_of_model(spatial, &self.sessions, nearest.1)
                    .is_empty()
                    .then_some(nearest.1)
            })
            .flatten()
    }

    /// The set `model`'s first tip makes: an empty session, of the options
    /// in force. A model that stands on the plate gets no raft: its own base
    /// is where one would go, and it stays where it is.
    fn unborn_set(&self, model: NodeId) -> Option<DrawnSet> {
        let mut request = self.request_for_printer();
        request.options = self.born_options_now();
        self.document
            .with_scene(|spatial| {
                let rests = crate::raft_clash::model_bottoms(spatial)
                    .iter()
                    .any(|(id, z)| *id == model && *z < crate::raft_clash::RESTING_MM);
                if rests {
                    request.options.raft = RaftMode::Off;
                    request.options.one_raft = false;
                }
                let session =
                    sc_geom::supports::empty_session(spatial.graph(), model, &request)
                        .ok()??;
                DrawnSet::unborn(spatial, model, session)
            })
            .flatten()
    }

    /// [`Self::unborn_set`] for the preview under the cursor, made once for
    /// the scene and the options as they stand.
    fn unborn_hover_set(&self, model: NodeId) -> Option<DrawnSet> {
        let revision = self.document.scene_revision();
        let options = self.born_options_now();
        if let Some((r, m, o, drawn)) = self.unborn_hover.borrow().as_ref()
            && *r == revision
            && *m == model
            && *o == options
        {
            return Some(drawn.clone());
        }
        let drawn = self.unborn_set(model)?;
        *self.unborn_hover.borrow_mut() = Some((revision, model, options, drawn.clone()));
        Some(drawn)
    }

    /// Why no support set in force takes a hand tool at `point`: the
    /// supports there are not the ones selected (Remove), the model's
    /// supports are out of date, the model has none, or the supports there
    /// came in as a mesh.
    fn no_set_notice(&self, tool: HandTool, point: [f32; 3]) -> NoSet {
        let p = point.map(f64::from);
        let near = |spatial: &sc_mesh_spatial::SceneSpatial, id: NodeId| {
            spatial
                .leaf(id)
                .and_then(|q| q.transformed_aabb(q.world_transform()))
                .map(|aabb| box_distance(&aabb, p))
                .filter(|d| *d <= SET_REACH_MM)
        };
        // The supports there, or the supports of the model there.
        let (model, supports) = self
            .document
            .with_scene(|spatial| {
                let nearest = |ids: Vec<NodeId>| {
                    ids.into_iter()
                        .filter_map(|id| Some((near(spatial, id)?, id)))
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .map(|(_, id)| id)
                };
                let support_ids: Vec<NodeId> = support_leaves(spatial).map(|(id, _)| id).collect();
                let model_ids: Vec<NodeId> = crate::raft_clash::model_bottoms(spatial)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                let model = match tool {
                    HandTool::Remove => None,
                    HandTool::Add | HandTool::AddNear => nearest(model_ids),
                };
                let supports = match (tool, model) {
                    (HandTool::Remove, _) => nearest(support_ids),
                    (_, Some(model)) => sets_of_model(spatial, &self.sessions, model)
                        .into_iter()
                        .next(),
                    (_, None) => None,
                };
                (model, supports)
            })
            .unwrap_or_default();
        let no_supports = || {
            NoSet::Notice(
                Some(MessageKey::SupportsEditNeedsSupports),
                ctr!("SupportDialog", "No supports to edit"),
                ctr!(
                    "SupportDialog",
                    "Generate supports first; then add or remove them by hand."
                ),
            )
        };
        let editable = |leaf: NodeId| {
            self.document
                .with_scene(|spatial| in_force(spatial, &self.sessions, leaf).is_some())
                .unwrap_or(false)
        };
        let selected = || self.selected_set() == supports;
        match supports {
            // Another set than the one selected: Remove leaves it alone.
            Some(leaf) if tool == HandTool::Remove && editable(leaf) && !selected() => {
                NoSet::Failed(ctr!(
                    "SupportDialog",
                    "Select these supports, or their model, to remove them."
                ))
            }
            // In force, only out of reach of the point.
            Some(leaf) if editable(leaf) => NoSet::Failed(refusal(&EditError::NothingThere)),
            Some(leaf) if model_of(&self.sessions, leaf).is_some() => NoSet::OutOfDate(leaf),
            // Supports in the scene with no session came in as a mesh: from
            // a 3MF, or a project that could not keep them as data. That says
            // something the person needs to know, so it is never silenced.
            Some(_) => NoSet::Notice(
                None,
                ctr!("SupportDialog", "No supports to edit"),
                ctr!(
                    "SupportDialog",
                    "These supports were opened as a mesh, so they cannot be edited by hand. Generate supports again to edit them."
                ),
            ),
            None if model.is_some() => no_supports(),
            None if self.support_leaf().is_none() => no_supports(),
            None => NoSet::Failed(refusal(&EditError::NothingThere)),
        }
    }

    /// Brings the supports of the leaf `leaf` back in step with their model
    /// when they no longer are (#526), so no tool stops to ask for them to
    /// be made again: a model whose mesh changed where it stands (Repair,
    /// Hollow) keeps them as they are while every tip still touches it,
    /// within [`HEAL_REACH_MM`]; supports whose own mesh changed keep their
    /// data; anything else is made again on its own, keeping the edits made
    /// by hand, once for the state the scene is in. Says how it went.
    fn heal_stale(&self, leaf: NodeId) -> Healed {
        let found = self
            .document
            .with_scene(|spatial| {
                if in_force(spatial, &self.sessions, leaf).is_some() {
                    return None;
                }
                let drawn = latest_drawing(&self.sessions, leaf)?;
                let why = drawn.stale_why(spatial)?;
                let (mesh, world) = crate::support_persist::model_state(spatial, drawn.model)?;
                let shown = spatial.graph().leaf_mesh_arc(leaf)?.clone();
                let key = format!(
                    "{leaf:?}:{:?}:{:?}:{world:?}",
                    Arc::as_ptr(&shown),
                    Arc::as_ptr(&mesh)
                );
                Some((drawn, why, world, key))
            })
            .flatten();
        let Some((drawn, why, world, key)) = found else {
            return Healed::No;
        };
        if self.healing_tried.borrow().contains(&key) {
            return Healed::No;
        }
        let rebased = match why {
            StaleWhy::SupportsMesh => Some(drawn.session.clone()),
            StaleWhy::ModelMesh if world == drawn.model_world => self
                .document
                .with_scene(|spatial| {
                    let now = scene_model_leaves(spatial.graph(), Some(&[drawn.model]))
                        .ok()?
                        .pop()?
                        .1;
                    // The support engine: a panic in it leaves
                    // the set to be made again.
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        drawn.session.rebased(0, now, HEAL_REACH_MM)
                    }))
                    .ok()
                    .flatten()
                    .map(Arc::new)
                })
                .flatten(),
            _ => None,
        };
        if let Some(session) = rebased {
            crate::breadcrumbs::note(&format!(
                "supports: kept in step with their model ({why:?})"
            ));
            record(&self.document, &self.sessions, leaf, drawn.model, session);
            return Healed::InForce;
        }
        let Some(me) = self.me.borrow().upgrade() else {
            return Healed::No;
        };
        let busy = self.handle.borrow().is_some()
            || self.editing()
            || self
                .ui
                .upgrade()
                .is_none_or(|ui| ui.get_loading() || ui.get_support_gen_in_progress());
        if busy {
            return Healed::Redoing;
        }
        // Once for this state: Undo of the supports made again brings the
        // state back, and they are not made again behind the person's back.
        self.healing_tried.borrow_mut().insert(key);
        crate::breadcrumbs::note(&format!("supports: made again on their own ({why:?})"));
        let keep = drawn.session.hand_edited();
        me.make_set_again(drawn.model, drawn.session, keep);
        self.show_status(ctr!(
            "SupportDialog",
            "The model changed under its supports: they are being made again, keeping your edits."
        ));
        Healed::Redoing
    }

    /// Keeps every support set in step with its model while the scene is
    /// quiet (#526): see [`Self::heal_stale`].
    fn heal_stale_sets(&self, ui: &MainWindow) {
        if ui.get_loading()
            || ui.get_slicing_in_progress()
            || ui.get_repairing_in_progress()
            || self.handle.borrow().is_some()
            || self.editing()
            || self.last_change.get().elapsed() < SPECULATE_SETTLE
        {
            return;
        }
        let leaves: Vec<NodeId> = self
            .document
            .with_scene(|spatial| support_leaves(spatial).map(|(id, _)| id).collect())
            .unwrap_or_default();
        for leaf in leaves {
            if model_of(&self.sessions, leaf).is_some() && self.heal_stale(leaf) == Healed::Redoing
            {
                // One run at a time.
                return;
            }
        }
    }

    /// The card of the supports out of date in the leaf `leaf` (#408): why
    /// they are, and the ways to make them again: "Regenerate", and, for a
    /// set with edits made by hand, "Recalculate, keeping my edits". Close
    /// leaves them as they are.
    fn show_out_of_date(&self, leaf: NodeId) {
        let Some((drawn, why)) = self
            .document
            .with_scene(|spatial| {
                let drawn = latest_drawing(&self.sessions, leaf)?;
                let why = drawn.stale_why(spatial);
                Some((drawn, why))
            })
            .flatten()
        else {
            return;
        };
        crate::breadcrumbs::note(&format!("supports: out of date ({why:?})"));
        let model = drawn.model;
        let session = drawn.session;
        let me = self.me.borrow().clone();
        self.dialogs.show_stale_supports(
            session.hand_edited(),
            move || InfoTexts {
                heading: ctr!("SupportDialog", "Supports out of date"),
                message: match why {
                    Some(StaleWhy::ModelMesh) => ctr!(
                        "SupportDialog",
                        "This model's mesh changed since its supports were made (Repair and Hollow change it), so they can no longer be edited by hand."
                    ),
                    Some(StaleWhy::SupportsMesh) => ctr!(
                        "SupportDialog",
                        "The supports' mesh changed since they were made (Repair changes it), so they can no longer be edited by hand."
                    ),
                    Some(StaleWhy::ModelPose) | None => ctr!(
                        "SupportDialog",
                        "The scene changed since the supports were made; generate them again to edit them."
                    ),
                },
                details: Default::default(),
            },
            move |choice| {
                let Some(run) = me.upgrade() else {
                    return;
                };
                match choice {
                    StaleSupportsChoice::Regenerate => run.make_set_again(model, session, false),
                    StaleSupportsChoice::KeepEdits => run.make_set_again(model, session, true),
                    StaleSupportsChoice::Leave => {}
                }
            },
        );
    }

    /// A row of the card of supports out of date (#408): the supports of
    /// `model` made again as a Generate press makes them for it alone
    /// (progress, Cancel, one undo step, its own set), with the preset of
    /// `stale`, the set out of date (#490), and with its edits made by hand
    /// put back where they still fit when `keep_edits`. A model gone since is
    /// left alone; with a run in flight, the press is refused.
    fn make_set_again(
        self: &Rc<Self>,
        model: NodeId,
        stale: Arc<SupportSession>,
        keep_edits: bool,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let there = self
            .document
            .with_scene(|spatial| {
                crate::raft_clash::model_bottoms(spatial)
                    .iter()
                    .any(|(id, _)| *id == model)
            })
            .unwrap_or(false);
        if !there {
            return;
        }
        if ui.get_loading() || ui.get_support_gen_in_progress() || self.handle.borrow().is_some() {
            self.show_status(ctr!(
                "SupportDialog",
                "Supports are being generated; try again when they are done."
            ));
            return;
        }
        crate::breadcrumbs::note(&format!(
            "supports: made again from the card, keeping edits {keep_edits}"
        ));
        let options = stale.options().cloned();
        let keep = keep_edits.then_some(stale);
        *self.redo.borrow_mut() = Some(Redo {
            model,
            keep,
            options,
        });
        self.start();
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
                    let slot = run.handle.borrow();
                    let Some(handle) = slot.as_ref() else {
                        stop();
                        return;
                    };
                    let progress = handle.progress();
                    ui.set_support_gen_progress(progress.fraction());
                    ui.set_support_gen_phase(progress.phase.into());
                    handle.try_poll()
                };
                if let Some(outcome) = finished {
                    stop();
                    run.handle.borrow_mut().take();
                    ui.set_support_gen_in_progress(false);
                    ui.set_support_gen_cancelable(false);
                    run.finish(outcome);
                }
            },
        );
    }

    /// Generate's step, waiting for the one raft for all (#606), goes on once
    /// every raft is drawn: the step's slice sees it.
    fn go_on_after_rafts(&self) {
        if self.islands_after_raft.borrow().is_none() || !self.plate_rafts.settled(&self.document)
        {
            return;
        }
        let Some(summary) = self.islands_after_raft.borrow_mut().take() else {
            return;
        };
        let started = self.after_generate.borrow().as_ref().is_some_and(|then| then());
        if !started {
            self.show_status(summary.into());
        }
    }

    /// Whether a plate's one raft for all still has to be drawn for the
    /// scene as it stands: a slice should wait for it.
    pub(crate) fn rafts_pending(&self) -> bool {
        self.plate_rafts
            .pending(&self.document, &self.ctrl.build_volume())
    }

    /// Sets what Generate goes on to once its new supports are in (#606),
    /// which tells whether it started.
    pub(crate) fn set_after_generate(&self, then: impl Fn() -> bool + 'static) {
        *self.after_generate.borrow_mut() = Some(Box::new(then));
    }

    fn finish(&self, outcome: Result<SupportOutcome, JobError>) {
        let ended = match &outcome {
            Ok(_) => "ok".to_owned(),
            Err(JobError::Cancelled) => "cancelled".to_owned(),
            Err(err) => format!("failed: {err}"),
        };
        crate::breadcrumbs::finished("supports", &ended);
        let started_on = self.started_on.borrow_mut().take();
        let regenerating = self.regenerating.borrow_mut().take();
        let lift = self.plate_lift.borrow_mut().take();
        // Cancel pressed: nothing changes, even for a run that ended just
        // before it was seen.
        let outcome = if self.cancelled.replace(false) {
            Err(JobError::Cancelled)
        } else {
            outcome
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(JobError::Cancelled) => return,
            Err(err) => {
                if let Some((revision, wanted)) = &regenerating {
                    self.document
                        .settle_supports_wanted(*revision, &wanted.ids());
                }
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("SupportDialog", "Support generation failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
                return;
            }
        };
        if let Some((revision, wanted)) = regenerating {
            self.finish_regeneration(revision, &wanted, outcome);
            return;
        }
        if started_on.is_none_or(|revision| self.document.slice_geometry_changed(revision)) {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("SupportDialog", "Supports discarded"),
                message: ctr!(
                    "SupportDialog",
                    "The scene changed while supports were being generated; nothing was added."
                ),
                details: Default::default(),
            });
            return;
        }
        let summary = summary_message(&outcome);
        if let Some(ui) = self.ui.upgrade() {
            let seconds = outcome.timings.total.as_secs_f64();
            let mut line = format!("{summary} {seconds:.1} s.");
            // Which preset each model took by its height (#723).
            if let Some(picks) = self.height_picks.borrow_mut().take() {
                line.push(' ');
                line.push_str(&ctr!("SupportDialog", "By height: {}.", picks));
            }
            // Painted detail is avoided where it can be (#186); the tips it
            // could not be are told, with the finest tip.
            let on_detail: usize = outcome
                .sets
                .iter()
                .filter(|set| set.same_as.is_none())
                .map(|set| {
                    let tips: Vec<[f64; 3]> =
                        set.session.tips().into_iter().map(|(_, p)| p).collect();
                    crate::support_area::tips_on_detail(&self.document, set.model, &tips)
                })
                .sum();
            if on_detail > 0 {
                line.push(' ');
                line.push_str(&ctr!(
                    "SupportDialog",
                    "{n} tip on painted detail could not be kept off it; it takes the finest tip.",
                    "{n} tips on painted detail could not be kept off it; they take the finest tip.",
                    on_detail
                ));
            }
            ui.set_supports_last_run(line.into());
        }
        let mut details = details_message(&outcome);
        // A release build has no console (Windows GUI subsystem): the full
        // breakdown goes to a log file instead, named in the details.
        if let Some(path) = append_timing_log(&summary, &outcome.timings.report()) {
            details.push_str(&format!("\n\nFull timings: {}", path.display()));
        }
        let covered: Vec<NodeId> = outcome.sets.iter().map(|set| set.model).collect();
        // What became of the hand edits a set made again from its card was
        // to keep (#408).
        let kept: Vec<KeptEdits> = outcome
            .sets
            .iter()
            .filter_map(|set| set.kept.clone())
            .collect();
        let crowded = outcome.crowded.len();
        let uncleared = outcome.rafts_uncleared.len();
        // One set per model it placed supports on (#197). A copy that took
        // another's supports (#561) shows that one's mesh, turned and moved
        // onto it by its leaf: one mesh for every copy.
        let mut drawn: Vec<Option<Arc<sc_mesh_core::IndexedMesh>>> = Vec::new();
        let mut sets: Vec<(
            NodeId,
            Arc<sc_mesh_core::IndexedMesh>,
            Transform3D,
            SupportSession,
        )> = Vec::new();
        for set in outcome.sets {
            let shared = set.same_as.and_then(|same| {
                let mesh = drawn.get(same.set)?.clone()?;
                Some((mesh, same.transform()))
            });
            let shown = match (shared, set.mesh) {
                (Some(shared), Some(_)) => Some(shared),
                (None, Some(mesh)) => Some((Arc::new(mesh), Transform3D::Identity)),
                (_, None) => None,
            };
            drawn.push(
                shown
                    .as_ref()
                    .filter(|(_, t)| matches!(t, Transform3D::Identity))
                    .map(|(mesh, _)| mesh.clone()),
            );
            if let Some((mesh, at)) = shown {
                sets.push((set.model, mesh, at, set.session));
            }
        }
        if sets.is_empty() {
            self.dialogs
                .show_notice(MessageKey::SupportsNotNeeded, move || InfoTexts {
                    heading: ctr!("SupportDialog", "No supports needed"),
                    message: summary.as_str().into(),
                    details: details.as_str().into(),
                });
            return;
        }
        let fragment = SceneFragment::from_leaf_snapshots(
            sets.iter().map(|(_, mesh, at, _)| (mesh.clone(), *at)),
        );
        // A hollow's drain whose mouth the new raft covers keeps the resin
        // in (#467): said below.
        let shut_drains = {
            let mouths = crate::raft_drains::drain_mouths(&self.document);
            if mouths.is_empty() {
                0
            } else {
                let rafts: Vec<_> = sets
                    .iter()
                    .map(|(_, mesh, at, session)| {
                        let top = session.raft_top_mm();
                        (crate::raft_drains::raft_top(mesh, at, top), top)
                    })
                    .collect();
                crate::raft_drains::shut(&mouths, &rafts)
            }
        };
        // Generating again replaces the supports already there (#152), in the
        // same undoable step as the paste: the sets of the models the run
        // covered, and, from a run over every model, supports no session drew.
        // The supports a model came with go too (#259): it gets these
        // instead.
        let replaced: Vec<NodeId> = self
            .document
            .with_scene(|spatial| {
                let every = crate::raft_clash::model_bottoms(spatial)
                    .iter()
                    .all(|(id, _)| covered.contains(id));
                let mut replaced: Vec<NodeId> = support_leaves(spatial)
                    .map(|(id, _)| id)
                    .filter(|id| match model_of(&self.sessions, *id) {
                        Some(model) => covered.contains(&model),
                        None => every,
                    })
                    .collect();
                for model in covered.iter() {
                    for set in crate::imported_supports::imported_of(spatial, *model, None) {
                        if !replaced.contains(&set) {
                            replaced.push(set);
                        }
                    }
                }
                replaced
            })
            .unwrap_or_default();
        // Generating again over the same models and settings draws the same
        // supports in the same place (#188): the status bar says so, or else
        // that the previous set was replaced, as nothing seems to happen.
        let replaced_note = (!replaced.is_empty()).then(|| {
            let fresh: Vec<&sc_mesh_core::IndexedMesh> =
                sets.iter().map(|(_, mesh, _, _)| mesh.as_ref()).collect();
            // Lifted models (#389) get supports for their new place.
            if lift.is_none() && self.same_supports(&replaced, &fresh) {
                ReplacedNote::Same
            } else {
                ReplacedNote::Replaced
            }
        });
        // The very sets the scene already shows, down to their session data
        // (#287): the scene keeps them, so the press adds no undo step and
        // leaves the slice and the saved state as they were.
        let unchanged = matches!(replaced_note, Some(ReplacedNote::Same))
            && self.already_shown(&replaced, &sets);
        if !unchanged {
            // The lift of the models resting on the plate (#389) and their
            // supports: one undo step.
            let pasted = self
                .document
                .paste_fragment_replacing_all_after(lift, &fragment, None, &replaced);
            if let Ok(leaves) = &pasted
                && leaves.len() == sets.len()
            {
                for (leaf, (model, _, _, session)) in leaves.iter().zip(sets) {
                    record(
                        &self.document,
                        &self.sessions,
                        *leaf,
                        model,
                        Arc::new(session),
                    );
                }
                self.generated_raft_top
                    .set(Some(self.options.borrow().raft_top_mm()));
                self.warm_worker();
                // Models left waiting for supports have them now.
                self.document
                    .settle_supports_wanted(self.document.scene_revision(), &covered);
            }
            if let Err(error) = pasted {
                let details = error.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("SupportDialog", "Support generation failed"),
                    context: None,
                    message: ctr!(
                        "SupportDialog",
                        "The scene rejected the generated supports; nothing was changed."
                    ),
                    details: details.as_str().into(),
                });
                return;
            }
        }
        match replaced_note {
            Some(ReplacedNote::Same) => self.show_status(ctr!(
                "Workspace",
                "Supports regenerated: same result as before."
            )),
            Some(ReplacedNote::Replaced) => self.show_status(ctr!(
                "Workspace",
                "Supports regenerated: replaced the previous set."
            )),
            None => {}
        }
        // New supports: the islands they left are found and supported
        // (#606), as one procedure with Generate.
        // Under one raft for all the step waits for that raft (#556): it is
        // drawn once the scene stands still, and the slice has to see it.
        let islands_next = !unchanged
            && self.after_generate.borrow().is_some()
            && if self.options.borrow().one_raft {
                *self.islands_after_raft.borrow_mut() = Some(summary.clone());
                true
            } else {
                self.after_generate.borrow().as_ref().is_some_and(|then| then())
            };
        // Supports past the plate's edge (#628): told once Generate is done,
        // after its islands step when one follows.
        if islands_next {
            self.edges_after_islands.set(true);
        } else {
            self.edges_after_islands.set(false);
            self.tell_supports_past_plate();
        }
        // Copies given the supports of one standing like them were placed
        // without looking at their neighbours (#561): where they run into
        // one, Generate says so and moves nothing; Arrange all spaces the
        // models out (#556: it measures them with their supports).
        if crowded > 0 {
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("SupportDialog", "Supports run into a neighbour"),
                message: ctr!(
                    "SupportDialog",
                    "{} copies took the supports of a copy standing like them, and they run into a model beside them. Press Arrange all to space the models out.",
                    crowded
                ),
                details: Default::default(),
            });
        }
        if shut_drains > 0 {
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("SupportDialog", "The raft covers a drain"),
                message: ctr!(
                    "SupportDialog",
                    "The raft runs over {} drain hole(s) on the underside of a hollow model: the resin inside cannot get out there. Lift the model, move the drain, or set the raft to Off or to Lines.",
                    shut_drains
                ),
                details: Default::default(),
            });
        }
        // A raft whose clearance around the models failed is drawn under
        // them, rather than no supports at all (#467).
        if uncleared > 0 {
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("SupportDialog", "Raft drawn under the model"),
                message: ctr!(
                    "SupportDialog",
                    "The raft of {} model(s) could not be cut clear of the models standing on the plate, and runs under them. Check it in the Slice tab before printing, or set the raft to Off or to Lines.",
                    uncleared
                ),
                details: Default::default(),
            });
        }
        if !kept.is_empty() {
            self.show_kept_edits(kept, details);
            return;
        }
        // With the islands to find, the status bar says it and the islands
        // step tells its end there (#606): no card in between.
        if islands_next {
            self.show_status(ctr!(
                "Workspace",
                "Supports generated. Finding the islands and supporting them…"
            ));
            return;
        }
        self.dialogs
            .show_notice(MessageKey::SupportsGenerated, move || InfoTexts {
                heading: ctr!("SupportDialog", "Supports generated"),
                message: summary.as_str().into(),
                details: details.as_str().into(),
            });
    }
}

/// The raft a model stands on (#198): of the options' raft fields, the ones
/// a model may have of its own.
#[derive(Clone, Copy, Debug, PartialEq)]
struct OwnRaft {
    raft: RaftMode,
    thickness_mm: f64,
    wall: bool,
    pads: bool,
    holes_mm: f64,
    tabs: bool,
    /// Drawn apart, under one raft with others (#475, #556).
    apart: bool,
}

impl OwnRaft {
    fn of(options: &SupportOptions) -> Self {
        Self {
            raft: options.raft,
            thickness_mm: options.raft_thickness_mm,
            wall: options.raft_wall,
            pads: options.raft_pads,
            holes_mm: options.raft_holes_mm,
            tabs: options.raft_tabs,
            apart: options.one_raft,
        }
    }

    /// `options` with this raft.
    fn on(&self, options: &SupportOptions) -> SupportOptions {
        SupportOptions {
            raft: self.raft,
            raft_thickness_mm: self.thickness_mm,
            raft_wall: self.wall,
            raft_pads: self.pads,
            raft_holes_mm: self.holes_mm,
            raft_tabs: self.tabs,
            one_raft: self.apart,
            ..options.clone()
        }
    }
}

/// Whether two transforms place alike, to a billionth (#561): a copy's
/// supports shown under its turn and move.
fn same_place(a: &Transform3D, b: &Transform3D) -> bool {
    let parts = |t: &Transform3D| match t {
        Transform3D::Identity => (
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0; 3],
        ),
        Transform3D::Affine(a) => (*a.linear(), *a.translation()),
    };
    let ((la, ta), (lb, tb)) = (parts(a), parts(b));
    (0..3)
        .all(|i| (ta[i] - tb[i]).abs() < 1e-9 && (0..3).all(|j| (la[i][j] - lb[i][j]).abs() < 1e-9))
}

impl SupportRun {
    /// Whether the support leaves `replaced` show exactly the meshes `fresh`,
    /// in any order, where a new set is pasted: untransformed.
    fn same_supports(&self, replaced: &[NodeId], fresh: &[&sc_mesh_core::IndexedMesh]) -> bool {
        if replaced.len() != fresh.len() {
            return false;
        }
        self.document
            .with_scene(|spatial| {
                let mut unmatched: Vec<&sc_mesh_core::IndexedMesh> = fresh.to_vec();
                replaced.iter().all(|id| {
                    let Some(leaf) = spatial.leaf(*id) else {
                        return false;
                    };
                    if !untransformed(&leaf.world_transform()) {
                        return false;
                    }
                    match unmatched.iter().position(|mesh| *mesh == leaf.mesh()) {
                        Some(at) => {
                            unmatched.swap_remove(at);
                            true
                        }
                        None => false,
                    }
                })
            })
            .unwrap_or(false)
    }

    /// Whether the new `sets` are the ones the leaves `replaced` already
    /// show (#287): each leaf shows, untransformed, the mesh of the set on
    /// its model, drawn by a session in force, with no hand edits, that
    /// keeps the same data (what a save writes as `SUPP`). Pasting them would
    /// change nothing.
    fn already_shown(
        &self,
        replaced: &[NodeId],
        sets: &[(
            NodeId,
            Arc<sc_mesh_core::IndexedMesh>,
            Transform3D,
            SupportSession,
        )],
    ) -> bool {
        if replaced.len() != sets.len() {
            return false;
        }
        self.document
            .with_scene(|spatial| {
                let mut unmatched: Vec<_> = sets.iter().collect();
                replaced.iter().all(|id| {
                    let (Some(leaf), Some(drawn)) =
                        (spatial.leaf(*id), in_force(spatial, &self.sessions, *id))
                    else {
                        return false;
                    };
                    if drawn.session.hand_edited() {
                        return false;
                    }
                    // Where the set is shown: in place, or a copy's turn and
                    // move (#561).
                    let shown = leaf.world_transform();
                    let Some(at) = unmatched.iter().position(|(model, mesh, place, _)| {
                        *model == drawn.model
                            && mesh.as_ref() == leaf.mesh()
                            && same_place(&shown, place)
                    }) else {
                        return false;
                    };
                    let (_, _, _, session) = unmatched.swap_remove(at);
                    drawn.session.same_supports_as(session)
                })
            })
            .unwrap_or(false)
    }

    /// The card that ends "Recalculate, keeping my edits" (#408): the hand
    /// edits put back, and each one that could not be kept, with where it
    /// was and why. `details` are the run's.
    fn show_kept_edits(&self, kept: Vec<KeptEdits>, details: String) {
        let total: usize = kept.iter().map(KeptEdits::total).sum();
        let lost: Vec<LostEdit> = kept.iter().flat_map(|k| k.lost.clone()).collect();
        let unrecorded = kept.iter().any(|k| k.unrecorded);
        crate::breadcrumbs::note(&format!(
            "supports: recalculated keeping edits, {} of {total} lost",
            lost.len()
        ));
        self.dialogs.show_info(move || {
            let mut lines: Vec<String> = Vec::new();
            if lost.is_empty() {
                lines.push(
                    ctr!(
                        "SupportDialog",
                        "The edit made by hand was put back.",
                        "All {n} edits made by hand were put back.",
                        total
                    )
                    .to_string(),
                );
            } else {
                lines.push(if total == 1 {
                    ctr!(
                        "SupportDialog",
                        "The edit made by hand could not be kept:"
                    )
                    .to_string()
                } else {
                    ctr!(
                        "SupportDialog",
                        "{n} of your {} edits made by hand could not be kept:",
                        "{n} of your {} edits made by hand could not be kept:",
                        lost.len(),
                        total
                    )
                    .to_string()
                });
                for edit in lost.iter().take(KEPT_LINES_MAX) {
                    lines.push(format!("• {}", lost_line(edit)));
                }
                if lost.len() > KEPT_LINES_MAX {
                    lines.push(
                        ctr!(
                            "SupportDialog",
                            "…and {n} more.",
                            "…and {n} more.",
                            lost.len() - KEPT_LINES_MAX
                        )
                        .to_string(),
                    );
                }
            }
            if unrecorded {
                lines.push(
                    ctr!(
                        "SupportDialog",
                        "These supports were saved by an older version, which did not record the supports removed or moved by hand: those are back where the run places them."
                    )
                    .to_string(),
                );
            }
            if !lost.is_empty() || unrecorded {
                lines.push(
                    ctr!(
                        "SupportDialog",
                        "Undo brings back the supports as they were."
                    )
                    .to_string(),
                );
            }
            InfoTexts {
                heading: ctr!("SupportDialog", "Supports recalculated"),
                message: lines.join("\n").into(),
                details: details.as_str().into(),
            }
        });
    }

    /// Generate's islands step is over (#606): its supports are looked at
    /// against the plate's edge now, when Generate left that for then.
    pub(crate) fn tell_edges_if_pending(&self) {
        if self.edges_after_islands.replace(false) {
            self.tell_supports_past_plate();
        }
    }

    /// Says which models have supports past the edge of their plate (#628),
    /// with a button to Arrange all: it measures every model with its
    /// supports and keeps them on the plate.
    fn tell_supports_past_plate(&self) {
        let volume = self.ctrl.build_volume();
        let plates = self.document.plates();
        let past = self
            .document
            .with_scene(|spatial| {
                let past = crate::support_edges::past_the_plate(
                    spatial,
                    &volume,
                    plates,
                    &|leaf| model_of(&self.sessions, leaf),
                    crate::support_persist::raft_top_mm(spatial, &self.sessions),
                );
                let names = crate::repair_report::object_names_by_leaf(spatial.graph());
                past.iter()
                    .filter_map(|id| names.get(id).cloned())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        if past.is_empty() {
            return;
        }
        crate::breadcrumbs::note(&format!("supports: {} past the plate edge", past.len()));
        let count = past.len();
        let details = past.join("\n");
        let ui = self.ui.clone();
        self.dialogs.show_error_with_action(
            move || ErrorTexts {
                heading: ctr!("SupportDialog", "Supports past the plate edge"),
                context: None,
                message: ctr!(
                    "SupportDialog",
                    "The supports of {} model(s) reach past the edge of the plate and would not print. Press Arrange all to place the models with their supports on the plate.",
                    count
                ),
                details: details.as_str().into(),
            },
            || ctr!("SupportDialog", "Arrange all"),
            move || {
                if let Some(ui) = ui.upgrade() {
                    ui.invoke_arrange_requested(false);
                }
            },
        );
    }

    /// Shows `text` in the status bar in place of its hint for a few
    /// seconds; a newer note restarts the time.
    pub(crate) fn show_status(&self, text: slint::SharedString) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_status_notice(text);
        let ui_weak = self.ui.clone();
        self.status_timer.start(
            slint::TimerMode::SingleShot,
            STATUS_NOTICE_DURATION,
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_notice(Default::default());
                }
            },
        );
    }

    /// Ends a run that regenerated the supports of `wanted`, which the scene
    /// at `revision` left waiting (#153): the new sets join that entry. A
    /// scene that moved on drops the result quietly; the watcher starts again
    /// for what then waits.
    fn finish_regeneration(
        &self,
        revision: SceneRevision,
        wanted: &SupportsWanted,
        outcome: SupportOutcome,
    ) {
        let stands = self
            .document
            .with_scene(|spatial| wanted.stands_in(spatial))
            .unwrap_or(false);
        if self.document.scene_revision() != revision || !stands {
            return;
        }
        let ids = wanted.ids();
        // The models the run made sets for: the ones waiting, and the rest
        // of their groups (#475).
        let made: Vec<NodeId> = outcome.sets.iter().map(|set| set.model).collect();
        let summary = summary_message(&outcome);
        if let Some(ui) = self.ui.upgrade() {
            let seconds = outcome.timings.total.as_secs_f64();
            ui.set_supports_last_run(ctr!(
                "Inspector",
                "Supports regenerated for the new pose. {} {} s.",
                summary,
                format!("{seconds:.1}")
            ));
        }
        let sets: Vec<(NodeId, Arc<sc_mesh_core::IndexedMesh>, SupportSession)> = outcome
            .sets
            .into_iter()
            .filter_map(|set| Some((set.model, Arc::new(set.mesh?), set.session)))
            .collect();
        if sets.is_empty() {
            // The new pose needs none.
            self.document.settle_supports_wanted(revision, &ids);
            return;
        }
        let fragment = SceneFragment::from_leaf_snapshots(
            sets.iter()
                .map(|(_, mesh, _)| (mesh.clone(), Transform3D::Identity)),
        );
        // The edit already took the models' sets away; a set of theirs that
        // was out of date then goes now.
        let replaced: Vec<NodeId> = self
            .document
            .with_scene(|spatial| {
                support_leaves(spatial)
                    .map(|(id, _)| id)
                    .filter(|id| {
                        model_of(&self.sessions, *id)
                            .is_some_and(|m| ids.contains(&m) || made.contains(&m))
                    })
                    .collect()
            })
            .unwrap_or_default();
        match self
            .document
            .paste_fragment_completing(&fragment, &replaced, revision, &ids)
        {
            Ok(leaves) if leaves.len() == sets.len() => {
                for (leaf, (model, _, session)) in leaves.iter().zip(sets) {
                    record(
                        &self.document,
                        &self.sessions,
                        *leaf,
                        model,
                        Arc::new(session),
                    );
                }
                self.generated_raft_top
                    .set(Some(self.options.borrow().raft_top_mm()));
                self.warm_worker();
                // The edit took the old sets away when it was let go: say
                // where the new ones came from, as no button was pressed.
                self.show_status(ctr!(
                    "Workspace",
                    "Supports regenerated for the model's new pose."
                ));
                if wanted.hand_edits_lost {
                    self.dialogs
                        .show_notice(MessageKey::SupportsRegeneratedHandEdits, || InfoTexts {
                            heading: ctr!("SupportDialog", "Supports regenerated"),
                            message: ctr!(
                                "SupportDialog",
                                "The model's new pose got new supports, so the supports added or removed by hand are gone. Undo brings back the old pose with them."
                            ),
                            details: Default::default(),
                        });
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.document.settle_supports_wanted(revision, &ids);
                let details = error.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("SupportDialog", "Support generation failed"),
                    context: None,
                    message: ctr!(
                        "SupportDialog",
                        "The scene rejected the generated supports; nothing was changed."
                    ),
                    details: details.as_str().into(),
                });
            }
        }
    }
    /// This driver, shared.
    fn me(&self) -> Option<Rc<Self>> {
        self.me.borrow().upgrade()
    }

    /// The set in force whose support has its tip within grab of `point`
    /// (world mm, on the supports), the nearest: what Move tip grabs.
    fn set_of_tip(&self, point: [f32; 3]) -> Option<DrawnSet> {
        let p = point.map(f64::from);
        self.sets_in_force()
            .into_iter()
            .filter_map(|drawn| {
                let tip = drawn.session.tip_near(p, GRAB_REACH_MM)?;
                let d = drawn
                    .session
                    .tips()
                    .into_iter()
                    .find(|(id, _)| *id == tip)
                    .map(|(_, at)| (0..3).map(|k| (at[k] - p[k]).powi(2)).sum::<f64>().sqrt())?;
                Some((d, drawn))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, drawn)| drawn)
    }

    /// The set a box or a brush removes from: the selected supports, or the
    /// selected model's, in force ([`Self::selected_set`], #197).
    fn selected_set_in_force(&self) -> Option<DrawnSet> {
        let leaf = self.selected_set()?;
        self.sets_in_force()
            .into_iter()
            .find(|drawn| drawn.leaf == leaf)
    }

    /// Asks the preview worker for what a drag would do, and polls for it.
    fn ask_drag_preview(self: &Rc<Self>, ask: impl FnOnce(u64) -> HoverAsk) {
        let generation = self.hover_generation.get() + 1;
        self.hover_generation.set(generation);
        if self.hover_worker.asks.send(ask(generation)).is_err() || self.hover_poll.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.hover_poll
            .start(slint::TimerMode::Repeated, HOVER_POLL, move || {
                if let Some(run) = weak.upgrade() {
                    run.take_hover_answer();
                }
            });
    }

    /// Stops drawing previews: one still on its way is not drawn.
    fn end_previews(&self) {
        self.hover_poll.stop();
        self.hover_shown.set(self.hover_generation.get());
        self.hover_at.set(None);
    }

    /// The worker's newest answer, drawn or not.
    fn newest_answer(&self) -> Option<HoverAnswer> {
        let unseen = self
            .hover_worker
            .answer
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        match unseen {
            Some(answer) => {
                let drawn = self.last_preview.borrow_mut().take();
                if let Some(drawn) = drawn {
                    let _ = self.hover_worker.discard.send(drawn);
                }
                Some(answer)
            }
            None => self.last_preview.borrow_mut().take(),
        }
    }

    /// A box's or a brush's picks changed: what they take is drawn red.
    fn picks_changed(self: &Rc<Self>, drawn: &DrawnSet, picked: &[String]) {
        if picked.is_empty() {
            self.end_previews();
            self.last_ghost.borrow_mut().clear();
            self.ctrl.set_preview_overlay(None);
            self.show_add_note(Default::default());
            return;
        }
        self.show_add_note((
            ctr!("SupportDialog", "Supports to remove: {}.", picked.len()),
            true,
        ));
        let tips: Arc<[String]> = picked.into();
        let base = drawn.session.clone();
        let from = session_key(drawn);
        self.ask_drag_preview(move |generation| HoverAsk {
            generation,
            base,
            from,
            point: [0.0; 3],
            tool: 5,
            how: AddOptions::default(),
            tip: None,
            tips,
            others: Vec::new(),
            brace_from: None,
            part: None,
        });
    }

    /// The supports a box from `press` to `corner` takes, by their ids, and
    /// whether rays are still owed for it: at most `budget` are sent.
    #[allow(clippy::too_many_arguments)]
    fn boxed(
        tips: &[(String, Vec<[f32; 3]>)],
        seen_from: &SeenFrom,
        locate: &dyn Fn(ScreenHit) -> Option<usize>,
        looked: &mut HashMap<(usize, (i32, i32)), bool>,
        press: [f32; 2],
        corner: [f32; 2],
        view: Project<'_>,
        budget: usize,
    ) -> (Vec<String>, bool) {
        let (lo, hi) = (
            [press[0].min(corner[0]), press[1].min(corner[1])],
            [press[0].max(corner[0]), press[1].max(corner[1])],
        );
        let (mut ids, more) = Self::taken(
            seen_from,
            locate,
            looked,
            &|_| false,
            &|q| (lo[0]..=hi[0]).contains(&q[0]) && (lo[1]..=hi[1]).contains(&q[1]),
            view,
            budget,
        );
        ids.sort_unstable();
        (ids.into_iter().map(|k| tips[k].0.clone()).collect(), more)
    }

    /// Looks on between the moves while a box owes rays: a step every
    /// [`HOVER_POLL`], [`BOX_RAYS`] at a time, so no move waits for them all.
    fn keep_looking(self: &Rc<Self>, more: bool) {
        if !more {
            self.box_poll.stop();
            return;
        }
        if self.box_poll.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.box_poll
            .start(slint::TimerMode::Repeated, HOVER_POLL, move || {
                if let Some(run) = weak.upgrade() {
                    run.box_step(BOX_RAYS);
                }
            });
    }

    /// One step of a box owing rays: up to `budget` more, the picks
    /// brought up to date. Returns whether it still owes some.
    fn box_step(self: &Rc<Self>, budget: usize) -> bool {
        let stepped = self.ctrl.with_drag_view(|view| {
            let mut drag = self.drag.borrow_mut();
            match drag.as_mut() {
                Some(DragState::Remove {
                    drawn,
                    tips,
                    locate,
                    seen_from,
                    looked,
                    picked,
                    press,
                    corner,
                    owed,
                    brush: None,
                }) => {
                    if !*owed {
                        return None;
                    }
                    let (ids, more) = Self::boxed(
                        tips,
                        seen_from,
                        &|hit| locate.support_at(hit),
                        looked,
                        *press,
                        *corner,
                        view,
                        budget,
                    );
                    *owed = more;
                    let changed = ids != *picked;
                    *picked = ids;
                    Some((
                        Picks::Set(Box::new(drawn.clone()), picked.clone()),
                        changed,
                        more,
                    ))
                }
                Some(DragState::RemoveImported {
                    bodies,
                    points,
                    seen_from,
                    looked,
                    picked,
                    press,
                    corner,
                    owed,
                    brush: None,
                }) => {
                    if !*owed {
                        return None;
                    }
                    let locate = self.body_locator(bodies);
                    let (ids, more) = Self::boxed(
                        points, seen_from, &locate, looked, *press, *corner, view, budget,
                    );
                    *owed = more;
                    let changed = ids != *picked;
                    *picked = ids;
                    Some((
                        Picks::Imported(picked_bodies(bodies, picked)),
                        changed,
                        more,
                    ))
                }
                _ => None,
            }
        });
        let Some(Some((picks, changed, more))) = stepped else {
            self.box_poll.stop();
            return false;
        };
        if changed {
            match picks {
                Picks::Set(drawn, picked) => self.picks_changed(&drawn, &picked),
                Picks::Imported(bodies) => self.imported_picks_changed(&bodies),
            }
        }
        if !more {
            self.box_poll.stop();
        }
        more
    }

    /// somewhere `inside` it (#3). A ray goes through a pixel of a
    /// support's line on screen; the nearest hit among every leaf of the
    /// scene decides: on the model or another set, nothing; on this set, the
    /// support whose line runs nearest the hit. A support is looked at every
    /// [`SIGHT_STRIDE`] cells of [`SIGHT_CELL_PX`] along its line on screen,
    /// in two batches across the cores: one ray each first, which finds most
    /// supports in sight, then the rest for those still in doubt. `looked`
    /// keeps what each ray found during the drag; those `skip` names are not
    /// looked at.
    fn taken(
        seen_from: &SeenFrom,
        locate: &dyn Fn(ScreenHit) -> Option<usize>,
        looked: &mut HashMap<(usize, (i32, i32)), bool>,
        skip: &dyn Fn(usize) -> bool,
        inside: &dyn Fn([f32; 2]) -> bool,
        view: Project<'_>,
        budget: usize,
    ) -> (Vec<usize>, bool) {
        let mut taken = Vec::new();
        let mut doubt: Vec<(usize, Vec<LineCell>)> = Vec::new();
        for (k, line) in seen_from.screen.iter().enumerate() {
            if skip(k) {
                continue;
            }
            // A pixel of its line in each cell inside, in order along it.
            let mut cells: Vec<LineCell> = Vec::new();
            for q in line.iter().flatten().filter(|q| inside(**q)) {
                let c = (
                    (q[0] / SIGHT_CELL_PX).floor() as i32,
                    (q[1] / SIGHT_CELL_PX).floor() as i32,
                );
                if cells.last().is_none_or(|(last, _)| *last != c)
                    && !cells.iter().any(|(seen, _)| *seen == c)
                {
                    cells.push((c, *q));
                }
            }
            if cells
                .iter()
                .any(|(c, _)| looked.get(&(k, *c)) == Some(&true))
            {
                taken.push(k);
                continue;
            }
            let last = cells.len().saturating_sub(1);
            let sparse: Vec<_> = cells
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % SIGHT_STRIDE == 0 || *i == last)
                .map(|(_, cell)| cell)
                .filter(|(c, _)| !looked.contains_key(&(k, *c)))
                .collect();
            if !sparse.is_empty() {
                doubt.push((k, sparse));
            }
        }
        let mut ask = |asks: Vec<(usize, (i32, i32), [f32; 2])>, taken: &mut Vec<usize>| {
            let pixels: Vec<[f32; 2]> = asks.iter().map(|(_, _, q)| *q).collect();
            for ((k, c, _), hit) in asks.into_iter().zip(view.hits(&pixels)) {
                let seen = hit.is_some_and(|hit| locate(hit) == Some(k));
                looked.insert((k, c), seen);
                if seen && !taken.contains(&k) {
                    taken.push(k);
                }
            }
        };
        let owed: usize = doubt.iter().map(|(_, cells)| cells.len()).sum();
        let first: Vec<_> = doubt
            .iter_mut()
            .take(budget)
            .map(|(k, cells)| {
                let (c, q) = cells.remove(0);
                (*k, c, q)
            })
            .collect();
        let left = budget - first.len();
        let asked = first.len();
        ask(first, &mut taken);
        let rest: Vec<_> = doubt
            .into_iter()
            .filter(|(k, _)| !taken.contains(k))
            .flat_map(|(k, cells)| cells.into_iter().map(move |(c, q)| (k, c, q)))
            .take(left)
            .collect();
        let asked = asked + rest.len();
        ask(rest, &mut taken);
        (taken, asked < owed)
    }
}

/// A handle of the Select tool's move gizmo (#377).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PartHandle {
    /// The arrow along world axis `k` (X, Y, Z).
    Axis(usize),
    /// The cube on the gizmo's elbow `k`.
    Elbow(usize),
}

impl PartHandle {
    fn drawn(self) -> crate::mesh_render::ToolHandle {
        match self {
            PartHandle::Axis(k) => crate::mesh_render::ToolHandle::Axis(k),
            PartHandle::Elbow(k) => crate::mesh_render::ToolHandle::Corner(k),
        }
    }
}

/// An arrow of the move gizmo being dragged: along world axis `axis`, the
/// pointer read `from` along it at the press.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ArrowGrab {
    axis: usize,
    /// The way the arrow points (world).
    dir: [f64; 3],
    from: f64,
    /// It slides the knot the support hangs from.
    knot: bool,
}

/// The Select tool's move gizmo on the one support picked (#377).
struct PartGizmo {
    drawn: DrawnSet,
    id: String,
    /// The elbow it moves; `None`: the support's shaft.
    elbow: Option<String>,
    /// Where the part is (world mm): the gizmo's centre, following a drag.
    center: [f64; 3],
    /// Where the part was before the drag in flight.
    rest: [f64; 3],
    /// Which world axes it has an arrow along: a trunk moves across the
    /// plate alone.
    arrows: [bool; 3],
    /// The cubes on its other elbows, by id: a press on one takes it.
    cubes: Vec<(String, [f64; 3])>,
    /// The cubes on the knots supports hang from on its shaft, by the
    /// support: a press on one picks that support.
    hangs: Vec<(String, [f64; 3])>,
    /// It stands on a trunk of its own (its shaft moves across the plate).
    trunk: bool,
    /// The ways its arrows point (world): the world's axes, or for the knot
    /// a support hangs from, up its host's shaft last.
    axes: [[f64; 3]; 3],
    /// It slides the knot the support hangs from along its host's shaft.
    knot: bool,
}

/// How far up a hung support from its knot the cube that picks it stands,
/// mm (at most half way to its first bend).
const HANG_CUBE_MM: f64 = 1.5;

/// The world's axes, as the gizmo's arrows point.
const WORLD_AXES: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// How far a gizmo arrow's drag goes for the pointer's travel along it, mm:
/// to the hundredth; with Shift a tenth of it; with Ctrl to the
/// millimetre, or with both to the tenth (#275).
fn arrow_travel(travel: f64, snap: bool, fine: bool) -> f64 {
    let travel = if fine { travel * 0.1 } else { travel };
    let step = match (snap, fine) {
        (true, false) => 1.0,
        (true, true) => 0.1,
        _ => 0.01,
    };
    (travel / step).round() * step
}

/// The part move a drag of `grab`'s part `travel` mm along world axis
/// `axis` asks for.
fn arrow_move(grab: [f64; 3], elbow: Option<&str>, free: bool, arrow: ArrowGrab, travel: f64) -> PartMove {
    let d: [f64; 3] = std::array::from_fn(|k| arrow.dir[k] * travel);
    match elbow {
        Some(e) => PartMove::Elbow(e.to_owned(), std::array::from_fn(|k| grab[k] + d[k])),
        None if arrow.knot => PartMove::Knot(grab[2] + d[2]),
        None if free => PartMove::Shaft(d),
        None => PartMove::Trunk([d[0], d[1]]),
    }
}

/// How near a click must come to a support's axis to pick it, mm, past
/// its own width.
const SHAFT_PICK_MM: f64 = 1.5;

/// The support a click of the Select tool at `p` (world mm) picks (#377):
/// the one whose own shaft passes nearest, trunk, branch or tip; else the
/// one with a tip, joint or foot nearest, within reach.
pub(crate) fn support_picked_at(session: &SupportSession, p: [f64; 3]) -> Option<String> {
    session
        .shaft_near(p, SHAFT_PICK_MM)
        .or_else(|| session.support_near(p, SELECT_REACH_MM))
}

/// The handle of `g` the pointer is on: an elbow's cube first, then an
/// arrow, as the object's gizmo picks.
fn part_pick(g: &PartGizmo, pointer: &crate::cut_gizmo::ScenePointer) -> Option<PartHandle> {
    use crate::cut_gizmo::{ARROW_FROM, CORNER_PICK_PX, HANDLE_PX, LOCKOUT_COS, dot, segment_distance};
    let at = pointer.cursor;
    let cube = g
        .cubes
        .iter()
        .chain(&g.hangs)
        .enumerate()
        .filter_map(|(k, (_, p))| {
            let s = (pointer.project)(*p)?;
            let d = (s[0] - at[0]).hypot(s[1] - at[1]);
            (d <= CORNER_PICK_PX).then_some((k, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((k, _)) = cube {
        return Some(PartHandle::Elbow(k));
    }
    let shaft = (pointer.scale_at)(g.center)?;
    let along = |axis: [f64; 3], t: f64| -> [f64; 3] {
        std::array::from_fn(|k| g.center[k] + axis[k] * shaft * t)
    };
    (0..3)
        .filter(|&k| g.arrows[k])
        .filter(|&k| dot(g.axes[k], pointer.direction).abs() <= LOCKOUT_COS)
        .filter_map(|k| {
            let a = (pointer.project)(along(g.axes[k], ARROW_FROM))?;
            let b = (pointer.project)(along(g.axes[k], pointer.arrow_tip))?;
            let d = segment_distance(at, a, b);
            (d <= HANDLE_PX).then_some((k, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| PartHandle::Axis(k))
}

impl SupportRun {
    /// The Select tool's move gizmo, while one support is picked and has a
    /// shaft to move (#377).
    fn part_gizmo(&self) -> Option<PartGizmo> {
        let ui = self.ui.upgrade()?;
        if ui.get_support_tool() != 5 || self.handle.borrow().is_some() {
            return None;
        }
        let mut now = self.chosen_now();
        if now.len() != 1 || now[0].1.len() != 1 {
            return None;
        }
        let (drawn, mut ids) = now.pop()?;
        let id = ids.pop()?;
        let parts = drawn.session.parts_of(&id)?;
        let focus = self
            .part_focus
            .borrow()
            .as_ref()
            .filter(|(of, _)| *of == id)
            .map(|(_, e)| e.clone())
            .filter(|e| parts.elbows.iter().any(|(x, _)| x == e));
        let trunk = parts.foot.is_some();
        // A support hung from another: its gizmo slides the knot it hangs
        // from, so the way it leans changes.
        let knot = match (&focus, trunk) {
            (None, false) => drawn.session.knot_of(&id),
            _ => None,
        };
        if knot.is_none() && parts.trunk_mm.is_none() {
            return None;
        }
        let (rest, axes) = match (&focus, knot) {
            (Some(e), _) => (parts.elbows.iter().find(|(x, _)| x == e)?.1, WORLD_AXES),
            (None, Some((at, up))) => (at, [WORLD_AXES[0], WORLD_AXES[1], up]),
            (None, None) if trunk => (parts.anchor, WORLD_AXES),
            (None, None) => (
                std::array::from_fn(|k| (parts.anchor[k] + parts.socket[k]) / 2.0),
                WORLD_AXES,
            ),
        };
        let center = match self.drag.borrow().as_ref() {
            Some(DragState::MovePart {
                id: moving,
                to: Some(to),
                ..
            }) if *moving == id => match to {
                PartMove::Trunk([dx, dy]) => [rest[0] + dx, rest[1] + dy, rest[2]],
                PartMove::Shaft(d) => std::array::from_fn(|k| rest[k] + d[k]),
                PartMove::Elbow(_, p) => *p,
                PartMove::Knot(z) if axes[2][2].abs() > 1e-6 => {
                    let t = (z - rest[2]) / axes[2][2];
                    std::array::from_fn(|k| rest[k] + axes[2][k] * t)
                }
                PartMove::Knot(_) => rest,
            },
            _ => rest,
        };
        let cubes: Vec<(String, [f64; 3])> = parts
            .elbows
            .iter()
            .filter(|(e, _)| Some(e) != focus.as_ref())
            .cloned()
            .collect();
        let room = crate::mesh_render::MAX_TOOL_CORNERS.saturating_sub(cubes.len());
        // Each a little way up its support from the knot, off the elbow
        // the knot often sits on.
        let hangs = drawn
            .session
            .hung_from(&id)
            .into_iter()
            .take(room)
            .map(|(h, knot)| {
                let up = drawn.session.parts_of(&h).map_or(knot, |p| {
                    p.elbows.first().map_or(p.tip, |(_, e)| *e)
                });
                let d: [f64; 3] = std::array::from_fn(|k| up[k] - knot[k]);
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let t = if len > 1e-9 { (HANG_CUBE_MM / len).min(0.5) } else { 0.0 };
                (h, std::array::from_fn(|k| knot[k] + d[k] * t))
            })
            .collect();
        let arrows = match (&focus, knot) {
            (Some(_), _) => [true, true, true],
            (None, Some(_)) => [false, false, true],
            (None, None) => [true, true, !trunk],
        };
        Some(PartGizmo {
            arrows,
            drawn,
            id,
            elbow: focus,
            center,
            rest,
            cubes,
            hangs,
            trunk,
            axes,
            knot: knot.is_some(),
        })
    }

    /// Asks the preview of `part` for the part drag in flight (#377).
    fn ask_part_preview(self: &Rc<Self>, part: PartMove) {
        let mut drag = self.drag.borrow_mut();
        let Some(DragState::MovePart { drawn, id, to, .. }) = drag.as_mut() else {
            return;
        };
        if to.as_ref() == Some(&part) {
            return;
        }
        *to = Some(part.clone());
        let base = drawn.session.clone();
        let from = session_key(drawn);
        let tip = Some(id.clone());
        drop(drag);
        self.ask_drag_preview(move |generation| HoverAsk {
            generation,
            base,
            from,
            point: [0.0; 3],
            tool: 10,
            how: AddOptions::default(),
            tip,
            tips: Arc::from([]),
            others: Vec::new(),
            brace_from: None,
            part: Some(part),
        });
    }

    /// Starts a drag of `g`'s part from `grab` (#377), as a drag of the
    /// Select tool on the support itself starts.
    fn start_part_drag(&self, g: PartGizmo, elbow: Option<String>, grab: [f64; 3]) {
        let free = elbow.is_some() || !g.trunk;
        self.end_previews();
        self.ctrl.set_preview_overlay(None);
        crate::breadcrumbs::note("supports: move part started (gizmo)");
        *self.drag.borrow_mut() = Some(DragState::MovePart {
            drawn: g.drawn,
            id: g.id,
            grab,
            elbow,
            free,
            to: None,
        });
    }
}

/// The Select tool's move gizmo, as the viewport offers it the pointer in
/// the Supports tab (#377).
struct PartHandles(std::rc::Weak<SupportRun>);

impl crate::cut_gizmo::CutHandles for PartHandles {
    fn active(&self) -> bool {
        self.0.upgrade().is_some_and(|run| run.part_gizmo().is_some())
    }

    fn gizmo(
        &self,
    ) -> Option<(
        [f64; 3],
        crate::mesh_render::ToolGizmo,
        Option<crate::cut_gizmo::RingTurn>,
    )> {
        let run = self.0.upgrade()?;
        let g = run.part_gizmo()?;
        let v3 = |p: [f64; 3]| glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
        let cubes: Vec<glam::Vec3> = g.cubes.iter().chain(&g.hangs).map(|(_, p)| v3(*p)).collect();
        let active = run
            .part_arrow
            .get()
            .map(|a| PartHandle::Axis(a.axis).drawn());
        let hover = run.part_hover.get().map(PartHandle::drawn);
        let tool = crate::mesh_render::ToolGizmo::mover(
            g.axes.map(v3),
            g.arrows,
            &cubes,
            hover,
            active,
        );
        Some((g.center, tool, None))
    }

    fn hover(&self, pointer: Option<&crate::cut_gizmo::ScenePointer>) -> bool {
        let Some(run) = self.0.upgrade() else {
            return false;
        };
        let now = pointer.and_then(|p| part_pick(&run.part_gizmo()?, p));
        run.part_hover.replace(now) != now
    }

    fn press(&self, pointer: &crate::cut_gizmo::ScenePointer) -> bool {
        let Some(run) = self.0.upgrade() else {
            return false;
        };
        let Some(g) = run.part_gizmo() else {
            return false;
        };
        match part_pick(&g, pointer) {
            Some(PartHandle::Axis(axis)) => {
                let dir = g.axes[axis];
                let Some(from) = pointer.along_line(g.rest, dir) else {
                    return false;
                };
                run.part_arrow.set(Some(ArrowGrab {
                    axis,
                    dir,
                    from,
                    knot: g.knot,
                }));
                let (elbow, rest) = (g.elbow.clone(), g.rest);
                run.start_part_drag(g, elbow, rest);
                true
            }
            Some(PartHandle::Elbow(k)) if k >= g.cubes.len() => {
                // A knot a support hangs from: that support is picked, its
                // gizmo on the knot.
                let hanger = &g.hangs[k - g.cubes.len()].0;
                let Some(tip) = g.drawn.session.tip_point(hanger) else {
                    return false;
                };
                *run.chosen.borrow_mut() = vec![(g.drawn.leaf, tip)];
                run.part_focus.borrow_mut().take();
                run.part_hover.set(None);
                if let Some(ui) = run.ui.upgrade() {
                    run.show_chosen(&ui);
                }
                true
            }
            Some(PartHandle::Elbow(k)) => {
                let (e, at) = g.cubes[k].clone();
                *run.part_focus.borrow_mut() = Some((g.id.clone(), e.clone()));
                run.part_arrow.set(None);
                run.start_part_drag(g, Some(e), at);
                true
            }
            None => false,
        }
    }

    fn drag(&self, pointer: &crate::cut_gizmo::ScenePointer) {
        let Some(run) = self.0.upgrade() else {
            return;
        };
        let (grab, elbow, free) = match run.drag.borrow().as_ref() {
            Some(DragState::MovePart {
                grab, elbow, free, ..
            }) => (*grab, elbow.clone(), *free),
            _ => return,
        };
        let part = match run.part_arrow.get() {
            Some(arrow) => {
                let Some(t) = pointer.along_line(grab, arrow.dir) else {
                    return;
                };
                let (snap, fine) = run.ctrl.pointer_mods();
                let travel = arrow_travel(t - arrow.from, snap, fine);
                arrow_move(grab, elbow.as_deref(), free, arrow, travel)
            }
            None => {
                // A cube dragged: its elbow on the plane facing the eye.
                let project = |p: [f32; 3]| {
                    (pointer.project)(p.map(f64::from)).map(|q| [q[0] as f32, q[1] as f32])
                };
                let hits = |pixels: &[[f32; 2]]| vec![None; pixels.len()];
                let view = DragView {
                    project: &project,
                    hits: &hits,
                };
                let cursor = [pointer.cursor[0] as f32, pointer.cursor[1] as f32];
                let Some(part) = part_target(grab, elbow.as_deref(), free, cursor, &view) else {
                    return;
                };
                part
            }
        };
        run.ask_part_preview(part);
    }

    fn release(&self, pointer: Option<&crate::cut_gizmo::ScenePointer>, moved: bool) {
        let Some(run) = self.0.upgrade() else {
            return;
        };
        run.part_arrow.set(None);
        SupportDragHandler::drag_end(&*run, moved && pointer.is_some());
        if !moved && let Some(ui) = run.ui.upgrade() {
            run.show_chosen(&ui);
        }
    }

    fn click(&self, _: &crate::cut_gizmo::ScenePointer) -> bool {
        false
    }
}

impl SupportDragHandler for SupportRun {
    fn drag_start(&self, kind: SupportDragKind, press: &DragPoint, project: Project<'_>) -> bool {
        let Some(run) = self.me() else {
            return false;
        };
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        if self.handle.borrow().is_some() {
            return false;
        }
        // With no set in force selected, a box or a brush takes the bodies of
        // the selection's imported supports (#260).
        let mut imported = match kind {
            SupportDragKind::Box | SupportDragKind::Brush
                if self.selected_set_in_force().is_none() =>
            {
                run.imported_sweep(&ui, kind, press, project)
            }
            _ => None,
        };
        let state = match kind {
            SupportDragKind::MoveTip => {
                let Some(at) = press.support else {
                    return false;
                };
                let Some(drawn) = self.set_of_tip(at) else {
                    return false;
                };
                let Some(tip) = drawn.session.tip_near(at.map(f64::from), GRAB_REACH_MM) else {
                    return false;
                };
                crate::breadcrumbs::note("supports: move tip started");
                DragState::Move {
                    drawn,
                    tip,
                    to: None,
                }
            }
            SupportDragKind::MovePart => {
                let Some(state) = run.part_grab(press) else {
                    return false;
                };
                crate::breadcrumbs::note("supports: move part started");
                state
            }
            SupportDragKind::Box | SupportDragKind::Brush if imported.is_some() => {
                imported.take().expect("the guard saw it")
            }
            SupportDragKind::Box | SupportDragKind::Brush => {
                let Some(drawn) = self.selected_set_in_force() else {
                    self.show_add_note((
                        ctr!(
                            "SupportDialog",
                            "Select these supports, or their model, to remove them."
                        ),
                        true,
                    ));
                    return false;
                };
                // A brace is taken as a support is, by its line (#3).
                let mut paths = drawn.session.support_paths();
                paths.extend(drawn.session.brace_paths());
                let tips = along_supports(paths);
                let locate = SupportLocator::new(drawn.leaf, &tips);
                let seen_from = SeenFrom::new(&tips, project);
                let mut looked = HashMap::new();
                let brush = (kind == SupportDragKind::Brush).then_some(press.cursor);
                let picked = match brush {
                    Some(at) => {
                        let radius = ui.get_support_brush_radius();
                        Self::taken(
                            &seen_from,
                            &|hit| locate.support_at(hit),
                            &mut looked,
                            &|_| false,
                            &|q| screen_segment_distance(q, at, at) <= radius,
                            project,
                            usize::MAX,
                        )
                        .0
                        .into_iter()
                        .map(|k| tips[k].0.clone())
                        .collect()
                    }
                    None => Vec::new(),
                };
                crate::breadcrumbs::note(&format!(
                    "supports: {kind:?} started over {} supports",
                    tips.len()
                ));
                DragState::Remove {
                    drawn,
                    tips,
                    locate,
                    seen_from,
                    looked,
                    picked,
                    press: press.cursor,
                    corner: press.cursor,
                    owed: false,
                    brush,
                }
            }
        };
        self.end_previews();
        self.last_ghost.borrow_mut().clear();
        self.ctrl.set_preview_overlay(None);
        match &state {
            DragState::Remove { drawn, picked, .. } if !picked.is_empty() => {
                run.picks_changed(drawn, picked);
            }
            DragState::RemoveImported { bodies, picked, .. } if !picked.is_empty() => {
                run.imported_picks_changed(&picked_bodies(bodies, picked));
            }
            _ => {}
        }
        *self.drag.borrow_mut() = Some(state);
        true
    }

    fn drag_move(&self, at: &DragPoint, project: Project<'_>) {
        let Some(run) = self.me() else {
            return;
        };
        let radius = self
            .ui
            .upgrade()
            .map_or(24.0, |ui| ui.get_support_brush_radius());
        let mut drag = self.drag.borrow_mut();
        match drag.as_mut() {
            Some(DragState::Move { drawn, tip, to }) => {
                let Some(model) = at.model else {
                    return;
                };
                *to = Some(model);
                let base = drawn.session.clone();
                let from = session_key(drawn);
                let tip = Some(tip.clone());
                drop(drag);
                run.ask_drag_preview(move |generation| HoverAsk {
                    generation,
                    base,
                    from,
                    point: model,
                    tool: 3,
                    how: AddOptions::default(),
                    tip,
                    tips: Arc::from([]),
                    others: Vec::new(),
                    brace_from: None,
                    part: None,
                });
            }
            Some(DragState::MovePart {
                drawn,
                id,
                grab,
                elbow,
                free,
                to,
            }) => {
                let Some(part) = part_target(*grab, elbow.as_deref(), *free, at.cursor, project)
                else {
                    return;
                };
                if to.as_ref() == Some(&part) {
                    return;
                }
                *to = Some(part.clone());
                let base = drawn.session.clone();
                let from = session_key(drawn);
                let tip = Some(id.clone());
                drop(drag);
                run.ask_drag_preview(move |generation| HoverAsk {
                    generation,
                    base,
                    from,
                    point: [0.0; 3],
                    tool: 10,
                    how: AddOptions::default(),
                    tip,
                    tips: Arc::from([]),
                    others: Vec::new(),
                    brace_from: None,
                    part: Some(part),
                });
            }
            Some(DragState::Remove {
                drawn,
                tips,
                locate,
                seen_from,
                looked,
                picked,
                press,
                corner,
                owed,
                brush,
            }) => {
                if seen_from.refresh(tips, project) {
                    looked.clear();
                }
                let next: Vec<String> = match brush {
                    Some(last) => {
                        let (from, to) = (*last, at.cursor);
                        let already: Vec<usize> = (0..tips.len())
                            .filter(|&k| picked.contains(&tips[k].0))
                            .collect();
                        let mut next = picked.clone();
                        next.extend(
                            Self::taken(
                                seen_from,
                                &|hit| locate.support_at(hit),
                                looked,
                                &|k| already.contains(&k),
                                &|q| screen_segment_distance(q, from, to) <= radius,
                                project,
                                usize::MAX,
                            )
                            .0
                            .into_iter()
                            .map(|k| tips[k].0.clone()),
                        );
                        *last = at.cursor;
                        next
                    }
                    None => {
                        *corner = at.cursor;
                        let (ids, more) = Self::boxed(
                            tips,
                            seen_from,
                            &|hit| locate.support_at(hit),
                            looked,
                            *press,
                            *corner,
                            project,
                            BOX_RAYS,
                        );
                        *owed = more;
                        ids
                    }
                };
                let more = *owed;
                if next == *picked {
                    drop(drag);
                    run.keep_looking(more);
                    return;
                }
                *picked = next;
                let (drawn, picked) = (drawn.clone(), picked.clone());
                drop(drag);
                run.picks_changed(&drawn, &picked);
                run.keep_looking(more);
            }
            Some(DragState::RemoveImported {
                bodies,
                points,
                seen_from,
                looked,
                picked,
                press,
                corner,
                owed,
                brush,
            }) => {
                if seen_from.refresh(points, project) {
                    looked.clear();
                }
                let locate = run.body_locator(bodies);
                let next: Vec<String> = match brush {
                    Some(last) => {
                        let (from, to) = (*last, at.cursor);
                        let already: Vec<usize> = (0..points.len())
                            .filter(|&k| picked.contains(&points[k].0))
                            .collect();
                        let mut next = picked.clone();
                        next.extend(
                            Self::taken(
                                seen_from,
                                &locate,
                                looked,
                                &|k| already.contains(&k),
                                &|q| screen_segment_distance(q, from, to) <= radius,
                                project,
                                usize::MAX,
                            )
                            .0
                            .into_iter()
                            .map(|k| points[k].0.clone()),
                        );
                        *last = at.cursor;
                        next
                    }
                    None => {
                        *corner = at.cursor;
                        let (ids, more) = Self::boxed(
                            points, seen_from, &locate, looked, *press, *corner, project, BOX_RAYS,
                        );
                        *owed = more;
                        ids
                    }
                };
                drop(locate);
                let more = *owed;
                if next == *picked {
                    drop(drag);
                    run.keep_looking(more);
                    return;
                }
                *picked = next;
                let shown = picked_bodies(bodies, picked);
                drop(drag);
                run.imported_picks_changed(&shown);
                run.keep_looking(more);
            }
            None => {}
        }
    }

    fn cancel_pending(&self) -> bool {
        self.drop_brace() || self.drop_line()
    }

    fn drag_end(&self, commit: bool) {
        let Some(run) = self.me() else {
            return;
        };
        self.box_poll.stop();
        if commit {
            // The box finishes the rays it owes: its picks are what goes.
            while run.box_step(usize::MAX) {}
        }
        let Some(state) = self.drag.borrow_mut().take() else {
            return;
        };
        let newest = self.newest_answer();
        self.end_previews();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let drop_it = || {
            self.last_ghost.borrow_mut().clear();
            self.ctrl.set_preview_overlay(None);
        };
        if !commit {
            crate::breadcrumbs::note("supports: drag dropped");
            drop_it();
            self.show_add_note(Default::default());
            return;
        }
        match state {
            DragState::Move { drawn, tip, to } => {
                let Some(to) = to else {
                    drop_it();
                    return;
                };
                // The preview of this very spot: a red one leaves the support
                // where it was; a green one is the move already made.
                let previewed = newest.filter(|answer| {
                    answer.tool == 3
                        && answer.point == to
                        && answer.tip.as_deref() == Some(tip.as_str())
                        && answer.from == session_key(&drawn)
                });
                if let Some(answer) = &previewed
                    && let Some(preview) = &answer.preview
                    && !preview.valid
                {
                    drop_it();
                    let (why, _) = move_line(preview);
                    ui.set_supports_last_run(ctr!("SupportDialog", "Left where it was. {}", why));
                    self.show_add_note(Default::default());
                    return;
                }
                let edited = previewed.and_then(|answer| answer.edited);
                self.show_add_note(Default::default());
                run.start_gesture(&ui, drawn, EditOp::MoveTip(tip, to), edited);
            }
            DragState::MovePart { drawn, id, to, .. } => {
                let Some(to) = to else {
                    drop_it();
                    return;
                };
                let previewed = newest.filter(|answer| {
                    answer.tool == 10
                        && answer.part.as_ref() == Some(&to)
                        && answer.tip.as_deref() == Some(id.as_str())
                        && answer.from == session_key(&drawn)
                });
                if let Some(answer) = &previewed
                    && let Some(preview) = &answer.preview
                    && !preview.valid
                {
                    drop_it();
                    let (why, _) = move_line(preview);
                    ui.set_supports_last_run(ctr!("SupportDialog", "Left where it was. {}", why));
                    self.show_add_note(Default::default());
                    return;
                }
                let edited = previewed.and_then(|answer| answer.edited);
                self.show_add_note(Default::default());
                run.start_gesture(&ui, drawn, EditOp::MovePart(id, to), edited);
            }
            DragState::Remove { drawn, picked, .. } => {
                self.show_add_note(Default::default());
                if picked.is_empty() {
                    drop_it();
                    return;
                }
                run.start_gesture(&ui, drawn, EditOp::RemoveTips(picked), None);
            }
            DragState::RemoveImported { bodies, picked, .. } => {
                self.show_add_note(Default::default());
                drop_it();
                let picked = picked_bodies(&bodies, &picked);
                if !picked.is_empty() {
                    self.remove_imported_bodies(&picked);
                }
            }
        }
    }
}

/// Whether `transform` leaves every point where it is, as the paste of a
/// new set does (an affine identity, or the identity variant).
fn untransformed(transform: &Transform3D) -> bool {
    match transform {
        Transform3D::Identity => true,
        Transform3D::Affine(affine) => {
            transform.is_translation() && *affine.translation() == [0.0; 3]
        }
    }
}

/// What the status bar says of a Generate press that replaced supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplacedNote {
    /// The new sets are the ones they replaced, in the same place.
    Same,
    /// The new sets differ from the ones they replaced.
    Replaced,
}

/// The Supports tab's raft warning for a raft of `layers` layers, the
/// models to be lifted to `lift_to_mm`.
fn raft_clash_message(layers: usize, lift_to_mm: f64) -> slint::SharedString {
    ctr!(
        "Inspector",
        "The raft takes {n} layer and will interfere with the model, leaving no room for supports under it. Lift the model to {} mm above the plate, or turn the raft off.",
        "The raft takes {n} layers and will interfere with the model, leaving no room for supports under it. Lift the model to {} mm above the plate, or turn the raft off.",
        layers,
        mm_text(lift_to_mm)
    )
}

/// The label of a button that lifts the models to `lift_to_mm`.
pub(crate) fn raft_lift_label(lift_to_mm: f64) -> slint::SharedString {
    ctr!("Inspector", "Lift to {} mm", mm_text(lift_to_mm))
}

/// The Supports tab's box that lifts the models resting on the plate to
/// `lift_to_mm` on Generate (#389).
fn lift_on_plate_label(lift_to_mm: f64) -> slint::SharedString {
    ctr!(
        "Inspector",
        "Lift {} mm when the model rests on the plate",
        mm_text(lift_to_mm)
    )
}

/// Millimetres to two decimals at most, without trailing zeros ("5",
/// "0.85").
fn mm_text(mm: f64) -> String {
    let text = format!("{mm:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// How many hand edits that could not be kept the card names one by one
/// (#408); the rest are counted.
const KEPT_LINES_MAX: usize = 8;

/// One hand edit a set made again could not keep (#408): what it was,
/// where, and why, for the card's list.
fn lost_line(edit: &LostEdit) -> slint::SharedString {
    let [x, y, z] = edit.at;
    let at = format!("X {x:.1}, Y {y:.1}, Z {z:.1} mm");
    let why = match edit.why {
        LostWhy::SurfaceChanged => ctr!("SupportDialog", "the model surface there changed"),
        LostWhy::NoRoute => ctr!("SupportDialog", "no support can reach that point now"),
        LostWhy::CannotAttach => ctr!("SupportDialog", "no support to hang it from there now"),
        LostWhy::BraceEndsGone => ctr!(
            "SupportDialog",
            "the supports it joined are no longer there"
        ),
        LostWhy::BraceRefused(why) => brace_text(why),
        LostWhy::CarriesOthers => ctr!(
            "SupportDialog",
            "other supports of the new run hang from the one there"
        ),
    };
    match edit.kind {
        HandEditKind::Added => ctr!("SupportDialog", "support added at {}: {}", at, why),
        HandEditKind::Moved => ctr!("SupportDialog", "support moved to {}: {}", at, why),
        HandEditKind::Braced => ctr!("SupportDialog", "brace from {}: {}", at, why),
        HandEditKind::Reshaped => ctr!("SupportDialog", "support edited at {}: {}", at, why),
    }
}

/// One line: models, islands, roots and triangles, and the supports that
/// could not be placed again once sized (#236).
fn summary_message(outcome: &SupportOutcome) -> String {
    let mut line = ctr!(
        "Inspector",
        "Models: {}, islands: {}, support roots: {}, triangles: {}.",
        outcome.models,
        outcome.islands,
        // Every set's, a copy's (#561) too.
        outcome
            .sets
            .iter()
            .map(|set| set.session.state().collections.roots.len())
            .sum::<usize>(),
        outcome.triangles
    )
    .to_string();
    if outcome.unplaced > 0 {
        line.push(' ');
        line.push_str(&unplaced_note(outcome.unplaced));
    }
    line
}

/// Supports the plan took off once sized and could not place again (#236).
fn unplaced_note(n: usize) -> String {
    ctr!(
        "Inspector",
        "One support did not fit once sized and was left out: Analyze islands shows what it held.",
        "{n} supports did not fit once sized and were left out: Analyze islands shows what they held.",
        n
    )
    .to_string()
}

/// Appends one run's summary and full timing report to
/// `<local data dir>/logs/supports-timings.log`, returning its path, or
/// `None` when there is no data directory or the write fails (the log is a
/// diagnostic: a failure never stops the run).
fn append_timing_log(summary: &str, report: &str) -> Option<std::path::PathBuf> {
    use std::io::Write;
    // Written only while the diagnostic log is on.
    if !crate::breadcrumbs::enabled() {
        return None;
    }
    let path = crate::breadcrumbs::logs_dir()?.join("supports-timings.log");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    writeln!(file, "=== run at unix {stamp}: {summary}\n{report}").ok()?;
    Some(path)
}

/// The placed supports by type, then where the time went: the stages, the
/// plan's phases and the costliest inner measurements.
fn details_message(outcome: &SupportOutcome) -> String {
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let mut lines: Vec<String> = outcome
        .placed
        .iter()
        .map(|(kind, count)| format!("{kind}: {count}"))
        .collect();
    let t = &outcome.timings;
    lines.push(String::new());
    lines.push(format!("Time: {:.0} ms", ms(t.total)));
    if t.entries.iter().any(|e| e.label == "scan:prescanned") {
        lines.push("  scans prepared in the background".to_owned());
    }
    for prefix in ["stage:", "auto:"] {
        for e in t.by_cost(prefix) {
            lines.push(format!("  {:<24} {:>8.0} ms", e.label, ms(e.total)));
        }
    }
    let mut inner: Vec<_> = t
        .entries
        .iter()
        .filter(|e| !e.label.starts_with("stage:") && !e.label.starts_with("auto:"))
        .collect();
    inner.sort_by_key(|e| std::cmp::Reverse(e.total));
    for e in inner.iter().take(6) {
        lines.push(format!(
            "  {:<24} {:>8.0} ms ({}x)",
            e.label,
            ms(e.total),
            e.count
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod hand_edit_tests {
    use std::path::PathBuf;

    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    /// The click path end to end, through the window callbacks' handler: a
    /// generated support set, then an Add click and a Remove click under
    /// the model. A click used to panic (a sessions borrow held across the
    /// lookup) and close the app.
    /// With the scene still, the supports are generated ahead; Generate then
    /// pastes them without running again.
    #[test]
    fn supports_generated_ahead_are_pasted_on_generate() {
        let pasted = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("cube.stl"));
            let run = SupportRun::install(&window, bus, document, ctrl.dialogs(), ctrl.clone());
            run.last_change.set(Instant::now() - SPECULATE_SETTLE);
            let waited = Instant::now();
            while run.ahead.borrow().ready.is_none() && waited.elapsed() < Duration::from_secs(20) {
                run.generate_ahead(&window, None);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(run.ahead.borrow().ready.is_some(), "generated ahead");
            run.start();
            assert!(run.handle.borrow().is_none(), "no second run");
            run.support_leaf().is_some()
        });
        assert!(pasted);
    }

    #[test]
    fn hand_clicks_edit_the_supports_without_a_panic() {
        let (after_add, after_remove, edits) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let cube = sc_mesh_fixtures::unit_cube_mesh();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let before = document.scene_revision();

            window.set_support_tool(1);
            // The hover preview comes from the worker; the click on the same
            // point then takes the placement it computed.
            run.on_tool_hover(Some([20.2, 20.2, 7.5]));
            let waited = Instant::now();
            while run.last_preview.borrow().is_none() && waited.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(5));
                run.take_hover_answer();
            }
            assert!(
                run.last_preview.borrow().as_ref().is_some_and(|a| a
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.valid)
                    && a.edited.is_some()),
                "a green preview with its placement"
            );
            run.on_tool_click([20.2, 20.2, 7.5]);
            let after_add = window.get_supports_last_run().to_string();
            window.set_support_tool(2);
            run.on_tool_click([20.2, 20.2, 7.4]);
            let after_remove = window.get_supports_last_run().to_string();
            (after_add, after_remove, document.scene_revision() != before)
        });
        assert!(after_add.contains("edited by hand"), "{after_add}");
        assert!(after_remove.contains("edited by hand"), "{after_remove}");
        assert!(edits);
    }

    /// Copies of one mesh standing alike get their supports from one run
    /// on the first, and every copy shows that one support mesh, turned and
    /// moved by its leaf (#561). A support added by hand to a copy gives it
    /// a mesh of its own and leaves the first as it was.
    #[test]
    fn copies_standing_alike_show_one_support_mesh() {
        let (shared, first_kept, own) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let cube = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
            let turn = AffineTransform3D::from_translation([50.0, 30.0, 8.0])
                .compose(&AffineTransform3D::from_rotation_z(0.6));
            for at in [AffineTransform3D::from_translation([20.0, 20.0, 8.0]), turn] {
                let leaf = graph
                    .add_leaf(
                        None,
                        Transform3D::Affine(at),
                        LeafPayload::Mesh(cube.clone()),
                    )
                    .unwrap();
                graph.add_root(leaf).unwrap();
            }
            document.install_graph(graph.clone(), PathBuf::from("cubes.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let meshes = |document: &SceneDocument| {
                document
                    .with_scene(|spatial| {
                        support_leaves(spatial)
                            .map(|(id, _)| {
                                let mesh = spatial.graph().leaf_mesh_arc(id).unwrap().clone();
                                let at = spatial.leaf(id).unwrap().world_transform();
                                (mesh, at)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap()
            };
            let before = meshes(&document);
            assert_eq!(before.len(), 2);
            let shared = Arc::ptr_eq(&before[0].0, &before[1].0)
                && before.iter().filter(|(_, at)| untransformed(at)).count() == 1;
            // Each set stands under its own model.
            let under = |list: &[(Arc<sc_mesh_core::IndexedMesh>, Transform3D)]| {
                list.iter().all(|(mesh, at)| {
                    let world = sc_scene::bake_mesh_to_world(mesh, at).unwrap();
                    let (lo, hi) = world.vertices().iter().fold(
                        ([f32::MAX; 2], [f32::MIN; 2]),
                        |(lo, hi), v| {
                            let p = v.as_array();
                            (
                                [lo[0].min(p[0]), lo[1].min(p[1])],
                                [hi[0].max(p[0]), hi[1].max(p[1])],
                            )
                        },
                    );
                    let inside = |c: [f32; 2]| (0..2).all(|k| lo[k] < c[k] && c[k] < hi[k]);
                    let mine = if untransformed(at) {
                        [20.0, 20.0]
                    } else {
                        [50.0, 30.0]
                    };
                    inside(mine) && hi[0] - lo[0] < 20.0
                })
            };
            assert!(under(&before), "each set under its model");
            // Saved and opened again, they show one mesh still.
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("copies.bread");
            crate::scene_save::save_project_to(&document, &target, &PrintDefaults::default())
                .expect("the project saves");
            let loaded = sc_project_formats::load_scene(&target, None, |_| {}).unwrap();
            let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
                panic!("a .bread opens as a project");
            };
            let mut scene = project.scene;
            let restored = crate::support_persist::restore_supports(&mut scene, project.supports);
            assert!(restored.failures.is_empty(), "{:?}", restored.failures);
            document.install_project_graph(scene, target.clone(), project.payload);
            crate::support_persist::install_sets(&document, restored.sets);
            let before = meshes(&document);
            assert!(under(&before), "each set under its model, reopened");
            let shared = shared
                && before.len() == 2
                && Arc::ptr_eq(&before[0].0, &before[1].0)
                && run.sets_in_force().len() == 2;
            // A tip by hand on the turned copy, under it.
            window.set_support_tool(1);
            let under = Transform3D::Affine(turn)
                .apply_to_vertex(sc_mesh_core::Vertex::new([0.2, 0.2, -0.5]))
                .as_array()
                .to_owned();
            run.on_tool_click(under);
            let after = meshes(&document);
            let first = before
                .iter()
                .find(|(_, at)| untransformed(at))
                .map(|(m, _)| m.clone())
                .unwrap();
            let first_kept = after.iter().any(|(m, _)| Arc::ptr_eq(m, &first));
            let own = after
                .iter()
                .filter(|(m, _)| !Arc::ptr_eq(m, &first))
                .count()
                == 1;
            (shared, first_kept, own)
        });
        assert!(shared, "one support mesh, the copy's under its turn");
        assert!(first_kept && own, "the edited copy has a mesh of its own");
    }

    /// A model with no supports yet previews Add under the cursor before
    /// its first click (#564: only the hint rings showed until a support
    /// was placed).
    #[test]
    fn a_model_with_no_supports_previews_its_first_one() {
        let shown = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            window.set_support_tool(1);
            run.on_tool_hover(Some([20.2, 20.2, 7.5]));
            let waited = Instant::now();
            while run.last_preview.borrow().is_none() && waited.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(5));
                run.take_hover_answer();
            }
            let shown = run
                .last_preview
                .borrow()
                .as_ref()
                .is_some_and(|a| a.preview.as_ref().is_some_and(|p| p.valid));
            shown && !run.last_ghost.borrow().is_empty()
        });
        assert!(shown, "a green preview before the first support");
    }

    /// Add on a spot that cannot take a support (the top of a cube looks up)
    /// draws no support: a red dot at the cursor and a line say so, so the
    /// hover is never just nothing.
    #[test]
    fn a_spot_that_takes_no_support_shows_a_red_dot() {
        let (tints, note) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_, run, _) = supported(&window, &[[20.0, 20.0, 8.0]]);
            window.set_support_tool(1);
            run.on_tool_hover(Some([20.2, 20.2, 8.5]));
            let waited = Instant::now();
            while run.last_preview.borrow().is_none() && waited.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(5));
                run.take_hover_answer();
            }
            let tints: Vec<_> = run.last_ghost.borrow().iter().map(|p| p.tint).collect();
            (tints, window.get_support_add_note().to_string())
        });
        assert_eq!(tints, vec![OverlayTint::Invalid], "{note}");
        assert!(note.starts_with("Cannot place it"), "{note}");
    }

    /// Fix All's supports (#156): several points, one undoable edit; one
    /// undo takes them all back.
    #[test]
    fn supports_at_several_points_are_one_undoable_edit() {
        let (outcome, added, roots, undone) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let roots = |run: &SupportRun| {
                let sessions = run.sessions.borrow();
                sessions
                    .last()
                    .map_or(0, |s| s.session.state().collections.roots.len())
            };
            let generated = roots(&run);
            let mesh = |document: &SceneDocument| {
                document.with_scene(|spatial| {
                    let (node, _) = run.support_leaf().expect("the supports' leaf");
                    spatial.graph().leaf_mesh_arc(node).map(|m| m.faces().len())
                })
            };
            let before = mesh(&document);
            let (outcome, refused) =
                run.add_supports_at(&[[20.2, 20.2, 7.5], [19.8, 19.8, 7.5], [20.2, 19.8, 7.5]]);
            let added = 3 - refused.len();
            let after = roots(&run);
            assert_ne!(mesh(&document), before, "the supports changed");
            document.undo().expect("one undo");
            (
                outcome,
                added,
                (generated, after),
                mesh(&document) == before,
            )
        });
        assert!(matches!(outcome, HandEdit::Applied), "{outcome:?}");
        assert!(added >= 2, "{added} of 3 points took, {roots:?} roots");
        assert!(undone, "one undo takes every support back");
    }

    /// Generating again replaces the supports already there (#152), by a
    /// Generate press and by the raft warning's "Generate anyway"; one undo
    /// brings back the set before, still editable by hand. The status bar
    /// says nothing of the first set, and of a set replaced whether the new
    /// one is the same (#188). The second plain press changes nothing
    /// (#287); the thicker raft draws the same mesh on this cube, but under
    /// other settings, so it still replaces the set in an undo step.
    #[test]
    fn generating_again_replaces_the_supports() {
        let (plain, anyway, undone, editable, notices) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            // Clear of the default raft, not of a 3 mm one.
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 3.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let finish_run = |run: &Rc<SupportRun>| {
                let handle = run.handle.borrow_mut().take().expect("a run started");
                window.set_support_gen_in_progress(false);
                run.finish(handle.join());
            };
            let sets = || {
                document
                    .with_scene(|spatial| {
                        let leaves = (0..).map_while(|i| spatial.leaf_id_at_index(i)).count();
                        let supports: Vec<_> = support_leaves(spatial).map(|(_, m)| m).collect();
                        (leaves, supports)
                    })
                    .expect("a scene")
            };
            // The first set: a plain Generate press.
            run.set_options(SupportOptions::default());
            run.start();
            finish_run(&run);
            let first = sets();
            assert_eq!(first.1.len(), 1, "one set");
            let mut notices = vec![window.get_status_notice().to_string()];
            // Again, the same way.
            run.start();
            finish_run(&run);
            let plain = sets();
            notices.push(window.get_status_notice().to_string());
            // A raft that takes the model's first layers: the warning holds
            // the press back, and "Generate anyway" runs it.
            run.set_options(SupportOptions {
                raft: RaftMode::Solid,
                raft_thickness_mm: 3.0,
                ..SupportOptions::default()
            });
            run.start();
            assert!(run.handle.borrow().is_none(), "held back by the warning");
            run.generate_ignoring_raft();
            finish_run(&run);
            let anyway = sets();
            notices.push(window.get_status_notice().to_string());
            // A support added by hand, then Generate: a different set.
            run.set_options(SupportOptions::default());
            let (added, _) = run.add_supports_at(&[[20.2, 20.2, 2.5]]);
            assert_eq!(added, HandEdit::Applied, "a support added by hand");
            run.start();
            finish_run(&run);
            notices.push(window.get_status_notice().to_string());
            // Back to the scene the test checks below.
            document.undo().expect("undo that Generate press");
            document.undo().expect("undo the support added by hand");
            document.undo().expect("one undo");
            let undone = sets();
            let undone_is_plain = undone.0 == plain.0
                && undone.1.len() == 1
                && Arc::ptr_eq(&undone.1[0], &plain.1[0]);
            let editable = !run.sets_in_force().is_empty();
            (
                (plain.0, plain.1.len()),
                (anyway.0, anyway.1.len()),
                undone_is_plain,
                editable,
                notices,
            )
        });
        assert_eq!(
            notices,
            [
                "",
                "Supports regenerated: same result as before.",
                "Supports regenerated: same result as before.",
                "Supports regenerated: replaced the previous set.",
            ],
            "the status bar after each Generate press"
        );
        assert_eq!(plain, (2, 1), "the model and one support set");
        assert_eq!(anyway, (2, 1), "the model and one support set");
        assert!(undone, "one undo brings back the set before");
        assert!(editable, "the set before is editable again");
    }

    /// Generating again with nothing changed leaves the scene as it was
    /// (#287): no undo step, no new revision, so a slice taken before stays
    /// current and a saved scene stays saved; the status bar still says the
    /// result is the same (#188). One undo takes the supports away.
    #[test]
    fn generating_the_same_supports_again_changes_nothing() {
        let (same_revision, dirty, stale, notice, editable, undone) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 3.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let generate = |run: &Rc<SupportRun>| {
                run.start();
                let handle = run.handle.borrow_mut().take().expect("a run started");
                window.set_support_gen_in_progress(false);
                run.finish(handle.join());
            };
            run.set_options(SupportOptions::default());
            generate(&run);
            document.mark_saved();
            let revision = document.scene_revision();
            generate(&run);
            let same_revision = document.scene_revision() == revision;
            let dirty = document.is_dirty();
            let stale = document.slice_geometry_changed(revision);
            let notice = window.get_status_notice().to_string();
            let editable = !run.sets_in_force().is_empty();
            document.undo().expect("one undo");
            let undone = document
                .with_scene(|spatial| support_leaves(spatial).count() == 0)
                .expect("a scene")
                && !document.can_undo();
            (same_revision, dirty, stale, notice, editable, undone)
        });
        assert!(same_revision, "no new revision");
        assert!(!dirty, "the saved scene stays saved");
        assert!(!stale, "a slice taken before stays current");
        assert_eq!(notice, "Supports regenerated: same result as before.");
        assert!(editable, "the supports stay editable by hand");
        assert!(undone, "one undo step in total takes the supports away");
    }

    /// Generate goes on to the islands (#606) once its new supports are in,
    /// and not when it changed nothing (#287).
    #[test]
    fn generate_goes_on_to_the_islands_once_its_supports_are_in() {
        let calls = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 3.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let count = Rc::new(Cell::new(0));
            {
                let count = count.clone();
                run.set_after_generate(move || {
                    count.set(count.get() + 1);
                    true
                });
            }
            let generate = |run: &Rc<SupportRun>| {
                run.start();
                let handle = run.handle.borrow_mut().take().expect("a run started");
                window.set_support_gen_in_progress(false);
                run.finish(handle.join());
            };
            run.set_options(SupportOptions::default());
            generate(&run);
            let first = count.get();
            generate(&run);
            (first, count.get())
        });
        assert_eq!(calls, (1, 1), "once for the new supports, not for the same ones");
    }

    /// Generate with the model on the plate raises the raft warning (#131);
    /// its lift raises the model clear of the raft in one undoable edit, and
    /// the warning clears. Turning the raft off clears it too.
    #[test]
    fn generate_warns_of_a_model_in_the_raft_until_it_is_lifted() {
        let (on_generate, label, after_lift, undone, after_off, raft) =
            on_fresh_slint_thread(|| {
                let window =
                    MainWindow::new().expect("the headless platform creates a main window");
                let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
                let ctrl = Rc::new(ViewportController::new_for_test());
                let document = ctrl.document();
                let mut graph = Scene::new();
                let leaf = graph
                    .add_leaf(
                        None,
                        Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 0.5])),
                        LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                    )
                    .unwrap();
                graph.add_root(leaf).unwrap();
                document.install_graph(graph, PathBuf::from("cube.stl"));
                let run = SupportRun::install(
                    &window,
                    bus,
                    document.clone(),
                    ctrl.dialogs(),
                    ctrl.clone(),
                );
                run.set_options(SupportOptions::default());
                assert!(
                    window.get_support_raft_clash_message().is_empty(),
                    "not before Generate"
                );
                run.start();
                // The press waits for the warning's answer: a run started now
                // would be discarded by the lift.
                assert!(run.handle.borrow().is_none(), "Generate held back");
                let on_generate = window.get_support_raft_clash_message().to_string();
                let label = window.get_support_raft_lift_label().to_string();
                run.lift_for_warning();
                // The lift answers it: the run starts on the lifted scene.
                let started = run.handle.borrow_mut().take();
                assert!(started.is_some(), "the lift starts the held-back run");
                if let Some(handle) = started {
                    handle.cancel();
                }
                window.set_support_gen_in_progress(false);
                let after_lift = window.get_support_raft_clash_message().to_string();
                document.undo().expect("the lift undoes");
                run.refresh_raft_warning();
                let undone = window.get_support_raft_clash_message().to_string();
                run.turn_raft_off();
                let after_off = window.get_support_raft_clash_message().to_string();
                let raft = run.options.borrow().raft;
                (on_generate, label, after_lift, undone, after_off, raft)
            });
        // The default raft of lines (#435) takes its first 12 layers.
        assert!(
            on_generate.contains("12 layers")
                && on_generate.contains("to 5 mm")
                && on_generate.contains("turn the raft off"),
            "{on_generate}"
        );
        assert_eq!(label, "Lift to 5 mm");
        assert!(after_lift.is_empty(), "{after_lift}");
        assert_eq!(undone, on_generate, "Undo puts the model back in the raft");
        assert!(after_off.is_empty(), "{after_off}");
        assert_eq!(raft, RaftMode::Off);
    }

    /// The GUI click path on a real model, not a gate: `SC_STRESS_STL=...
    /// cargo test --release -- --ignored --nocapture gui_clicks_on_a_real_model`.
    #[test]
    #[ignore = "needs SC_STRESS_STL"]
    fn gui_clicks_on_a_real_model() {
        let Ok(path) = std::env::var("SC_STRESS_STL") else {
            return;
        };
        let log = on_fresh_slint_thread(move || {
            let mut log = Vec::new();
            let bytes = std::fs::read(&path).unwrap();
            let (loaded, _) =
                sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
            let leaf = loaded
                .roots()
                .iter()
                .copied()
                .find(|id| loaded.leaf_mesh_arc(*id).is_some())
                .unwrap();
            let mesh = loaded.leaf_mesh_arc(leaf).unwrap().clone();
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in mesh.vertices() {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.as_array()[k]);
                    hi[k] = hi[k].max(v.as_array()[k]);
                }
            }
            let shift = [
                60.0 - f64::from(lo[0] + hi[0]) / 2.0,
                40.0 - f64::from(lo[1] + hi[1]) / 2.0,
                5.0 - f64::from(lo[2]),
            ];
            let points: Vec<[f32; 3]> = mesh
                .faces()
                .iter()
                .step_by(997)
                .filter_map(|f| {
                    let [a, b, c] = f
                        .vertices
                        .map(|v| *mesh.vertices()[u32::from(v) as usize].as_array());
                    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                    let nz = u[0] * w[1] - u[1] * w[0];
                    let len = ((u[1] * w[2] - u[2] * w[1]).powi(2)
                        + (u[2] * w[0] - u[0] * w[2]).powi(2)
                        + nz * nz)
                        .sqrt();
                    (len > 1e-9 && nz / len < -0.5).then(|| {
                        std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0 + shift[k] as f32)
                    })
                })
                .take(12)
                .collect();
            let window = MainWindow::new().unwrap();
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation(shift)),
                    LeafPayload::Mesh(mesh),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("model.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            log.push(format!("generated: {}", window.get_supports_last_run()));
            for (i, p) in points.iter().enumerate() {
                window.set_support_tool(if i % 3 == 2 { 2 } else { 1 });
                let t = Instant::now();
                run.on_tool_hover(Some(*p));
                while run
                    .last_preview
                    .borrow()
                    .as_ref()
                    .is_none_or(|a| a.point != *p)
                    && t.elapsed() < Duration::from_secs(30)
                {
                    std::thread::sleep(Duration::from_millis(2));
                    run.take_hover_answer();
                }
                let hover_ms = t.elapsed().as_millis();
                let t = Instant::now();
                run.on_tool_click(*p);
                log.push(format!(
                    "{p:?} tool {}: hover {hover_ms} ms, click {} ms: {}",
                    window.get_support_tool(),
                    t.elapsed().as_millis(),
                    window.get_supports_last_run()
                ));
            }
            log
        });
        for line in log {
            println!("{line}");
        }
    }

    /// Supports are saved as data (#145) while a session drew them for the
    /// models as they stand: after a move in Z, which regenerates them
    /// (#153), but not once the model has a new mesh.
    #[test]
    fn supports_stay_data_through_edits_that_leave_the_models_alone() {
        use crate::support_persist::{SupportsAsMesh, support_documents};
        let (regenerated, out_of_date) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let at = Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0]));
            let model = graph
                .add_leaf(
                    None,
                    at,
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(model).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let away = Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 10.0]));
            let edit_model = |edit: SceneEdit| {
                document.apply_edit(edit.clone()).unwrap();
                document.push_edit(edit);
            };
            let kept = |document: &SceneDocument| {
                document
                    .with_scene(|spatial| {
                        let kept = support_documents(document, spatial);
                        (kept.documents.len(), kept.as_mesh)
                    })
                    .unwrap()
            };
            // Up in Z: the supports go, and come back regenerated as data
            // (#153).
            edit_model(SceneEdit::SetLocalTransform {
                node_id: model,
                before: at,
                after: away,
            });
            assert_eq!(kept(&document), (0, None), "taken away");
            run.watch_wanted(&window);
            finish_running(&run);
            let regenerated = kept(&document);
            // A new mesh for the model (a repair, a hollow) leaves them out of
            // date: saved as a mesh.
            let swap = document
                .with_scene(|spatial| {
                    leaf_mesh_swap(
                        spatial.graph(),
                        model,
                        Arc::new(sc_mesh_fixtures::cuboid_with_z_tunnel()),
                    )
                })
                .flatten()
                .expect("the model's mesh swaps");
            edit_model(swap);
            (regenerated, kept(&document))
        });
        assert_eq!(regenerated, (1, None));
        assert_eq!(out_of_date, (0, Some(SupportsAsMesh::NoSession)));
    }

    /// The scene of `cubes` (each a unit cube at its translation) with
    /// supports generated on it, the driver that pasted them, and the cubes'
    /// leaves.
    fn supported(
        window: &MainWindow,
        cubes: &[[f64; 3]],
    ) -> (Rc<SceneDocument>, Rc<SupportRun>, Vec<NodeId>) {
        let (ctrl, run, leaves) = supported_in(window, cubes);
        (ctrl.document(), run, leaves)
    }

    /// [`supported`], with the viewport that holds the scene.
    fn supported_in(
        window: &MainWindow,
        cubes: &[[f64; 3]],
    ) -> (Rc<ViewportController>, Rc<SupportRun>, Vec<NodeId>) {
        supported_by(window, cubes, SupportRequest::default())
    }

    /// [`supported_in`], the supports made by `request`.
    fn supported_by(
        window: &MainWindow,
        cubes: &[[f64; 3]],
        request: SupportRequest,
    ) -> (Rc<ViewportController>, Rc<SupportRun>, Vec<NodeId>) {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut graph = Scene::new();
        let mut leaves = Vec::new();
        for &at in cubes {
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation(at)),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            leaves.push(leaf);
        }
        document.install_graph(graph.clone(), PathBuf::from("cubes.stl"));
        let run =
            SupportRun::install(window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let outcome = start_supports(&graph, request)
            .unwrap()
            .join();
        *run.started_on.borrow_mut() = Some(document.scene_revision());
        run.finish(outcome);
        (ctrl, run, leaves)
    }

    fn translation(t: [f64; 3]) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation(t))
    }

    fn leaf_transform(document: &SceneDocument, leaf: NodeId) -> Transform3D {
        document
            .with_scene(|s| s.leaf(leaf).map(|q| q.local_transform()))
            .flatten()
            .expect("a leaf")
    }

    fn move_live(document: &SceneDocument, leaf: NodeId, before: [f64; 3], after: [f64; 3]) {
        document
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id: leaf,
                before: translation(before),
                after: translation(after),
            })
            .unwrap();
    }

    fn commit_move(document: &SceneDocument, leaf: NodeId, before: [f64; 3], after: [f64; 3]) {
        document.push_edit(SceneEdit::SetLocalTransform {
            node_id: leaf,
            before: translation(before),
            after: translation(after),
        });
    }

    /// Whether `moved` is `mesh` moved by `d` in X/Y, vertex for vertex.
    /// The mesh of the leaf `leaf` where it stands in the world: a set
    /// carried along on its leaf's transform (#560) shows there.
    fn world_mesh(document: &SceneDocument, leaf: NodeId) -> sc_mesh_core::IndexedMesh {
        document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                let world = query.world_transform();
                let mut mesh = (*spatial.graph().leaf_mesh_arc(leaf)?.clone()).clone();
                for v in &mut mesh.vertices {
                    *v = world.apply_to_vertex(*v);
                }
                Some(mesh)
            })
            .flatten()
            .expect("the leaf's mesh")
    }

    /// `mesh` moved by `t`.
    fn world_mesh_of(
        mesh: &sc_mesh_core::IndexedMesh,
        t: Transform3D,
    ) -> sc_mesh_core::IndexedMesh {
        let mut out = mesh.clone();
        for v in &mut out.vertices {
            *v = t.apply_to_vertex(*v);
        }
        out
    }

    /// The first tip of the set shown in the scene of `document`, where the
    /// overhang map and the view from under the plate put it.
    fn shown_tip(document: &SceneDocument) -> [f32; 3] {
        let sets = crate::overhang_tips::shown_sets(document);
        let set = sets.first().expect("a set shown");
        let [x, y, z] = set.session.pieces()[0].contacts[0].surface;
        set.world
            .apply_to_vertex(sc_mesh_core::primitives::Vertex::new([
                x as f32, y as f32, z as f32,
            ]))
            .coords()
    }

    fn is_moved(
        moved: &sc_mesh_core::IndexedMesh,
        mesh: &sc_mesh_core::IndexedMesh,
        d: [f32; 2],
    ) -> bool {
        moved.faces() == mesh.faces()
            && moved.vertices().len() == mesh.vertices().len()
            && moved.vertices().iter().zip(mesh.vertices()).all(|(a, b)| {
                let (a, b) = (a.as_array(), b.as_array());
                (a[0] - b[0] - d[0]).abs() < 1e-3
                    && (a[1] - b[1] - d[1]).abs() < 1e-3
                    && a[2] == b[2]
            })
    }

    /// #119: a model moved in X/Y carries its supports along live and keeps
    /// them when the move is committed, redrawn in the same undo entry and
    /// still editable; Undo and Redo take both together. A move in Z leaves
    /// them where they stood while live, and takes them away on commit
    /// (#153 regenerates them).
    #[test]
    fn supports_move_with_their_model_in_x_and_y() {
        use crate::support_persist::support_documents;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let model = leaves[0];
            let (supports, generated) = run.support_leaf().expect("the supports were pasted");
            let start = leaf_transform(&document, supports);
            let (at, there) = ([20.0, 20.0, 8.0], [27.0, 15.0, 8.0]);
            let first = in_force_session(&run).expect("the run's session");
            let tip = {
                let [x, y, z] = first.pieces()[0].contacts[0].surface;
                [x as f32, y as f32, z as f32]
            };
            // The tips the overhang map and the view from under the plate
            // show stand on the model wherever it went, never moved twice.
            let tip_at = |when: &str, d: [f32; 2]| {
                let shown = shown_tip(&document);
                let want = [tip[0] + d[0], tip[1] + d[1], tip[2]];
                assert!(
                    (0..3).all(|k| (shown[k] - want[k]).abs() < 1e-3),
                    "{when}: shown {shown:?}, the model's tip at {want:?}"
                );
            };
            tip_at("at the start", [0.0, 0.0]);

            // Live, the support leaf rides along on its transform.
            move_live(&document, model, at, [23.0, 18.0, 8.0]);
            move_live(&document, model, at, there);
            assert_eq!(
                leaf_transform(&document, supports),
                translation([7.0, -5.0, 0.0])
            );
            tip_at("live", [7.0, -5.0]);
            // A move put back puts them back.
            move_live(&document, model, there, at);
            assert_eq!(leaf_transform(&document, supports), start);
            move_live(&document, model, at, there);

            // Committed: the same mesh, carried on the leaf's transform
            // (#560).
            commit_move(&document, model, at, there);
            let (_, moved) = run.support_leaf().expect("the supports stay");
            let carried = translation([7.0, -5.0, 0.0]).compose(&start);
            assert_eq!(leaf_transform(&document, supports), carried);
            assert!(Arc::ptr_eq(&moved, &generated), "nothing drawn again");
            assert!(
                is_moved(&world_mesh(&document, supports), &generated, [7.0, -5.0]),
                "the same supports, moved"
            );
            tip_at("committed", [7.0, -5.0]);
            let moved_session = in_force_session(&run).expect("editable");
            assert!(
                !Arc::ptr_eq(&moved_session, &first),
                "a session moved along"
            );
            let kept = document
                .with_scene(|spatial| support_documents(&document, spatial).documents.len())
                .unwrap();
            assert_eq!(kept, 1, "kept as data");
            window.set_support_tool(1);
            run.on_tool_click([27.2, 15.2, 7.5]);
            assert!(
                window.get_supports_last_run().contains("edited by hand"),
                "{}",
                window.get_supports_last_run()
            );
            // Drawn anew, the set stands in world space again (#560): every
            // support it had, where the carried mesh showed it, and one more.
            assert_eq!(leaf_transform(&document, supports), Transform3D::Identity);
            let (_, edited) = run.support_leaf().expect("the supports stay");
            assert!(edited.faces().len() > generated.faces().len());
            let bounds = |m: &sc_mesh_core::IndexedMesh| {
                m.vertices()
                    .iter()
                    .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), v| {
                        let p = v.as_array();
                        (
                            [lo[0].min(p[0]), lo[1].min(p[1])],
                            [hi[0].max(p[0]), hi[1].max(p[1])],
                        )
                    })
            };
            let (was, now) = (
                bounds(&world_mesh(&document, supports)),
                bounds(&world_mesh_of(&generated, carried)),
            );
            assert!(
                (0..2).all(|k| was.0[k] <= now.0[k] + 1e-3 && was.1[k] >= now.1[k] - 1e-3),
                "the set drawn anew covers where it was carried: {was:?} {now:?}"
            );
            assert!(document.undo().unwrap(), "the hand edit undoes");
            assert_eq!(
                leaf_transform(&document, supports),
                carried,
                "carried again"
            );

            // One Undo takes the model and its supports back; Redo again.
            assert!(document.undo().unwrap());
            assert_eq!(leaf_transform(&document, model), translation(at));
            tip_at("undone", [0.0, 0.0]);
            let (_, back) = run.support_leaf().unwrap();
            assert!(Arc::ptr_eq(&back, &generated), "the supports as generated");
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &first)));
            assert!(document.redo().unwrap());
            assert_eq!(leaf_transform(&document, model), translation(there));
            tip_at("redone", [7.0, -5.0]);
            let (_, again) = run.support_leaf().unwrap();
            assert!(Arc::ptr_eq(&again, &moved));
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &moved_session)));

            // Up in Z, they stay where they were while live, and go when
            // the move is committed.
            let up = [27.0, 15.0, 10.0];
            move_live(&document, model, there, up);
            assert_eq!(leaf_transform(&document, supports), carried);
            commit_move(&document, model, there, up);
            assert!(run.support_leaf().is_none(), "taken away");
            let (_, wanted) = document.supports_wanted().expect("new ones wanted");
            assert_eq!(wanted.ids(), vec![model]);
        });
    }

    /// #560: a set carried along keeps its mesh, so the drawings of a set
    /// moved again and again are kept only as far back as Undo reaches.
    #[test]
    fn a_set_moved_many_times_keeps_only_the_drawings_undo_reaches() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let model = leaves[0];
            let (supports, _) = run.support_leaf().expect("the supports were pasted");
            let mut at = [20.0, 20.0, 8.0];
            for _ in 0..crate::scene_document::UNDO_STACK_CAP + 20 {
                let there = [at[0] + 0.01, at[1], at[2]];
                commit_move(&document, model, at, there);
                at = there;
            }
            let kept = run
                .sessions
                .borrow()
                .iter()
                .filter(|d| d.leaf == supports)
                .count();
            assert!(kept <= crate::scene_document::UNDO_STACK_CAP + 1, "{kept}");
            assert!(in_force_session(&run).is_some(), "still editable");
        });
    }

    /// #560: Arrange all pressed again with nothing done since keeps one
    /// step, and the sets' drawings of the places in the middle go.
    #[test]
    fn arranging_again_keeps_no_drawings_of_the_places_in_the_middle() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let volume = crate::build_volume::BuildVolume::new(120.0, 80.0, 100.0);
            crate::arrange_run::arrange_across_now(
                &document,
                &volume,
                Default::default(),
                crate::arrange_run::ArrangeScope::Plates,
                false,
            )
            .expect("arranged");
            let steps = document.undo_len();
            let drawings = run.sessions.borrow().len();
            for _ in 0..3 {
                let _ =
                    crate::arrange_run::arrange_again_now(&document, &volume, Default::default());
                assert_eq!(document.undo_len(), steps, "one step");
                assert!(
                    run.sessions.borrow().len() <= drawings,
                    "no drawings pile up"
                );
            }
            assert_eq!(run.sets_in_force().len(), 2, "both still editable");
            // One Undo: both models and their sets as generated.
            assert!(document.undo().unwrap());
            for &l in &leaves {
                let (set, _) = set_of(&document, &run, l);
                let at_rest = match leaf_transform(&document, set) {
                    Transform3D::Identity => true,
                    Transform3D::Affine(a) => a == AffineTransform3D::identity(),
                };
                assert!(at_rest, "the set where it was generated");
            }
            assert_eq!(run.sets_in_force().len(), 2, "editable after Undo");
        });
    }

    /// #560: models moved together take their sets along in one edit, so
    /// the scene tells its listeners once, not once per set. A hundred
    /// models arranged held the window for twenty seconds.
    #[test]
    fn sets_moved_together_are_told_once() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(
                &window,
                &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0], [80.0, 20.0, 8.0]],
            );
            let told = Rc::new(Cell::new(0));
            let _counting = document.subscribe({
                let told = told.clone();
                move |event| {
                    if event == SceneEvent::Mutated {
                        told.set(told.get() + 1);
                    }
                }
            });
            let moves: Vec<SceneEdit> = leaves
                .iter()
                .map(|&l| {
                    let before = leaf_transform(&document, l);
                    SceneEdit::SetLocalTransform {
                        node_id: l,
                        before,
                        after: translation([0.0, 30.0, 0.0]).compose(&before),
                    }
                })
                .collect();
            let edit = SceneEdit::Compound(moves);
            document.apply_edit(edit.clone()).expect("moves");
            document.push_edit(edit);
            // The move itself, then the sets carried: two, whatever the count.
            assert_eq!(
                told.get(),
                2,
                "told once for the move and once for the sets"
            );
            assert_eq!(run.sets_in_force().len(), 3, "all still editable");
            for &l in &leaves {
                let (set, _) = set_of(&document, &run, l);
                assert_ne!(
                    leaf_transform(&document, set),
                    Transform3D::Identity,
                    "the set went along"
                );
            }
        });
    }

    /// Ends the run in flight as its poll would.
    fn finish_running(run: &SupportRun) {
        let handle = run.handle.borrow_mut().take().expect("a run in flight");
        run.finish(handle.join());
    }

    fn turned(at: [f64; 3], angle: f64) -> Transform3D {
        translation(at).compose(&Transform3D::Affine(AffineTransform3D::from_rotation_z(
            angle,
        )))
    }

    fn tilted(at: [f64; 3], angle: f64) -> Transform3D {
        translation(at).compose(&Transform3D::Affine(AffineTransform3D::from_rotation_x(
            angle,
        )))
    }

    /// #153: a model turned about the vertical axis still stands on the plate
    /// the way it did: its supports turn with it, live and on commit, in the
    /// same undo entry and still editable, and nothing is generated again.
    #[test]
    fn supports_turn_with_their_model_about_the_vertical_axis() {
        use crate::support_persist::support_documents;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let model = leaves[0];
            let (supports, generated) = run.support_leaf().expect("the supports were pasted");
            let start = leaf_transform(&document, supports);
            let first = in_force_session(&run).expect("the run's session");
            let (at, turn) = (
                translation([20.0, 20.0, 8.0]),
                turned([20.0, 20.0, 8.0], 0.5),
            );
            let edit = || SceneEdit::SetLocalTransform {
                node_id: model,
                before: at,
                after: turn,
            };

            // Live, the support leaf turns along about the model's corner.
            document.apply_edit(edit()).unwrap();
            let live = leaf_transform(&document, supports);
            let goes = |p: [f32; 3]| {
                *live
                    .apply_to_vertex(sc_mesh_core::Vertex::new(p))
                    .as_array()
            };
            let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4);
            assert!(near(goes([20.0, 20.0, 0.0]), [20.0, 20.0, 0.0]));
            let (s, c) = 0.5f32.sin_cos();
            assert!(near(goes([21.0, 20.0, 3.0]), [20.0 + c, 20.0 + s, 3.0]));

            // Committed: the same mesh, carried on the leaf's transform as
            // live (#560), and nothing waits for new supports.
            document.push_edit(edit());
            assert_eq!(leaf_transform(&document, supports), live);
            assert_ne!(live, start);
            let (_, turned_mesh) = run.support_leaf().expect("the supports stay");
            assert!(Arc::ptr_eq(&turned_mesh, &generated), "nothing drawn again");
            assert_eq!(turned_mesh.faces().len(), generated.faces().len());
            assert!(
                document.supports_wanted().is_none(),
                "nothing to make again"
            );
            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_none(), "no run");
            let session = in_force_session(&run).expect("editable");
            assert!(!Arc::ptr_eq(&session, &first), "a session turned along");
            let kept = document
                .with_scene(|spatial| support_documents(&document, spatial).documents.len())
                .unwrap();
            assert_eq!(kept, 1, "kept as data");
            window.set_support_tool(1);
            run.on_tool_click(goes([20.2, 20.2, 7.5]));
            assert!(
                window.get_supports_last_run().contains("edited by hand"),
                "{}",
                window.get_supports_last_run()
            );
            assert!(document.undo().unwrap(), "the hand edit undoes");

            // One Undo takes the model and its supports back; Redo again.
            assert!(document.undo().unwrap());
            assert_eq!(leaf_transform(&document, model), at);
            let (_, back) = run.support_leaf().unwrap();
            assert!(Arc::ptr_eq(&back, &generated), "the supports as generated");
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &first)));
            assert!(document.redo().unwrap());
            assert_eq!(leaf_transform(&document, model), turn);
            let (_, again) = run.support_leaf().unwrap();
            assert!(Arc::ptr_eq(&again, &turned_mesh));
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &session)));
        });
    }

    /// #153: a model tilted loses its supports on commit and gets new ones
    /// for its new pose, editable, in the same undo entry: one Undo brings
    /// back the old pose with the old supports, Redo the new ones.
    #[test]
    fn a_tilted_model_gets_new_supports_in_the_same_undo_step() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let model = leaves[0];
            let (_, generated) = run.support_leaf().expect("the supports were pasted");
            let first = in_force_session(&run).expect("the run's session");
            let (at, tilt) = (
                translation([20.0, 20.0, 8.0]),
                tilted([20.0, 20.0, 8.0], 0.5),
            );
            document
                .apply_edit(SceneEdit::SetLocalTransform {
                    node_id: model,
                    before: at,
                    after: tilt,
                })
                .unwrap();
            document.push_edit(SceneEdit::SetLocalTransform {
                node_id: model,
                before: at,
                after: tilt,
            });
            assert!(run.support_leaf().is_none(), "the old supports go at once");
            assert!(!document.supports_wanted().unwrap().1.hand_edits_lost);

            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_some(), "regenerating");
            finish_running(&run);
            let (_, regenerated) = run.support_leaf().expect("new supports");
            assert!(!Arc::ptr_eq(&regenerated, &generated));
            assert_eq!(
                window.get_status_notice(),
                "Supports regenerated for the model's new pose.",
                "the status bar says why the supports changed (#188)"
            );
            let session = in_force_session(&run).expect("editable");
            assert!(!Arc::ptr_eq(&session, &first));
            assert!(document.supports_wanted().is_none(), "nothing waits");
            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_none(), "no second run");

            // One Undo takes the tilt and the new supports back together.
            assert!(document.undo().unwrap());
            assert_eq!(leaf_transform(&document, model), at);
            let (_, back) = run.support_leaf().expect("the old supports");
            assert!(Arc::ptr_eq(&back, &generated));
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &first)));
            // The entry before is the first run's paste.
            assert!(document.undo().unwrap());
            assert!(run.support_leaf().is_none(), "one entry for the tilt");
            assert!(document.redo().unwrap());
            assert!(document.redo().unwrap());
            assert_eq!(leaf_transform(&document, model), tilt);
            let (_, again) = run.support_leaf().unwrap();
            assert!(Arc::ptr_eq(&again, &regenerated));
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &session)));
            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_none(), "Redo needs no run");
        });
    }

    /// #153: Undo while the new supports are being made cancels the run and
    /// brings back the old pose with the old supports. A live move of the
    /// model holds the run back until it ends where the edit left it.
    #[test]
    fn regenerating_waits_for_the_model_and_stops_on_undo() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let model = leaves[0];
            let (_, generated) = run.support_leaf().expect("the supports were pasted");
            let (at, up) = ([20.0, 20.0, 8.0], [20.0, 20.0, 12.0]);
            move_live(&document, model, at, up);
            commit_move(&document, model, at, up);

            // A live move elsewhere: the run waits.
            move_live(&document, model, up, [20.0, 20.0, 14.0]);
            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_none(), "waits for the model");
            move_live(&document, model, [20.0, 20.0, 14.0], up);
            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_some(), "back where the edit left it");

            // Undo: the run is cancelled and nothing is pasted.
            assert!(document.undo().unwrap());
            run.watch_wanted(&window);
            finish_running(&run);
            assert_eq!(leaf_transform(&document, model), translation(at));
            let (_, back) = run.support_leaf().expect("the old supports");
            assert!(Arc::ptr_eq(&back, &generated));
            assert_eq!(support_sets(&document).len(), 1);
            assert!(document.supports_wanted().is_none());
        });
    }

    /// #153: with two models, the one scaled gets new supports alone; the
    /// other set is left as it was. A hand edit lost on the way is told.
    #[test]
    fn a_scaled_model_regenerates_its_set_alone() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let (_, other) = set_of(&document, &run, leaves[1]);
            // A support by hand under the first model.
            window.set_support_tool(1);
            run.on_tool_click([20.2, 20.2, 7.5]);
            assert!(window.get_supports_last_run().contains("edited by hand"));
            let (_, edited) = set_of(&document, &run, leaves[0]);

            let at = translation([20.0, 20.0, 8.0]);
            let scaled = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                [[1.5, 0.0, 0.0], [0.0, 1.5, 0.0], [0.0, 0.0, 1.5]],
                [20.0, 20.0, 8.0],
            ));
            document
                .apply_edit(SceneEdit::SetLocalTransform {
                    node_id: leaves[0],
                    before: at,
                    after: scaled,
                })
                .unwrap();
            document.push_edit(SceneEdit::SetLocalTransform {
                node_id: leaves[0],
                before: at,
                after: scaled,
            });
            let (_, wanted) = document.supports_wanted().expect("new ones wanted");
            assert_eq!(wanted.ids(), vec![leaves[0]]);
            assert!(wanted.hand_edits_lost, "the hand edit goes");
            run.watch_wanted(&window);
            finish_running(&run);

            assert_eq!(support_sets(&document).len(), 2);
            let (_, regenerated) = set_of(&document, &run, leaves[0]);
            assert!(!Arc::ptr_eq(&regenerated, &edited));
            assert!(Arc::ptr_eq(&set_of(&document, &run, leaves[1]).1, &other));
            assert_eq!(run.sets_in_force().len(), 2, "both editable");
        });
    }

    /// #153: a lift of several models in one edit (clear of the raft)
    /// regenerates each one's set, in that edit's undo entry.
    #[test]
    fn a_lift_of_two_models_regenerates_both_sets() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let lift = |node_id, x| SceneEdit::SetLocalTransform {
                node_id,
                before: translation([x, 20.0, 8.0]),
                after: translation([x, 20.0, 11.0]),
            };
            let edit = SceneEdit::Compound(vec![lift(leaves[0], 20.0), lift(leaves[1], 50.0)]);
            document.apply_edit(edit.clone()).unwrap();
            document.push_edit(edit);
            assert!(support_sets(&document).is_empty(), "both sets go");
            let (_, wanted) = document.supports_wanted().expect("new ones wanted");
            assert_eq!(wanted.ids(), leaves);
            run.watch_wanted(&window);
            finish_running(&run);
            assert_eq!(run.sets_in_force().len(), 2);

            assert!(document.undo().unwrap());
            assert_eq!(
                leaf_transform(&document, leaves[0]),
                translation([20.0, 20.0, 8.0])
            );
            assert_eq!(run.sets_in_force().len(), 2, "the old sets, in force");
        });
    }

    /// #245: each tip the review loads finds its support again through the
    /// slice's frame, and the supports a peel pulls on are drawn over
    /// themselves in the colour of their load; a support no peel reaches is
    /// left plain.
    #[test]
    fn a_tip_s_load_paints_its_own_support() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run, _leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let sets = run.sets_in_force();
            assert_eq!(sets.len(), 2);
            let frame = sc_print_glue::PlateFrame::default();
            // Every tip of both sets, with its set and support.
            let tips: Vec<(usize, String, [f64; 3])> = sets
                .iter()
                .enumerate()
                .flat_map(|(set, drawn)| {
                    drawn.session.pieces().into_iter().filter_map(move |piece| {
                        piece.contacts.first().map(|c| (set, piece.id.clone(), c.tip))
                    })
                })
                .collect();
            assert!(tips.len() >= 2, "each cube stands on a tip: {}", tips.len());
            let mut loads = bread_review::PlateLoads::default();
            for (k, (set, _, at)) in tips.iter().enumerate() {
                loads.tips.push(bread_review::TipLoad {
                    at_mm: frame.point_mm(sets[*set].leaf, *at),
                    radius_mm: 0.1,
                    layer: 10,
                    worst_n: if k == 0 { 2.0 } else { 0.0 },
                    worst_layer: 10,
                    capacity_n: bread_peel::Band::new(1.0, 1.0, 1.0),
                });
            }
            let owners = crate::stress_view::tip_owners(&loads, &frame, &sets);
            for (owner, (set, id, _)) in owners.iter().zip(&tips) {
                assert_eq!(owner.as_ref(), Some(&(*set, id.clone())));
            }
            let shares = crate::stress_view::support_shares(&loads, &owners, sets.len(), None);
            assert_eq!(shares[tips[0].0], vec![(tips[0].1.clone(), 2.0)]);
            let others: usize = shares.iter().map(Vec::len).sum();
            assert_eq!(others, 1, "the tips no peel reaches stay plain");
            let overlay = crate::stress_view::support_overlay(&sets, &shares);
            assert_eq!(overlay.len(), 1, "one support pulled on, one colour");
            assert_eq!(overlay[0].tint, OverlayTint::Load7, "past all it holds: red");
            assert!(!overlay[0].indices.is_empty());

            // As layer 1 peels, the second tip alone is pulled, on half of
            // what it holds; the first, the hardest over the print, not.
            loads.curve_layers = 1;
            loads.tip_curves = vec![Vec::new(); loads.tips.len()];
            loads.tip_curves[1] = vec![0.0, 0.5, 0.0];
            let at = crate::stress_view::support_shares(&loads, &owners, sets.len(), Some(1));
            assert_eq!(at[tips[1].0], vec![(tips[1].1.clone(), 0.5)]);
            assert_eq!(at.iter().map(Vec::len).sum::<usize>(), 1);
            let overlay = crate::stress_view::support_overlay(&sets, &at);
            assert_eq!(overlay[0].tint, crate::mesh_render::load_tint(0.5));

            // A click on a tip in the Slice tab picks its support.
            let (set, id, at) = &tips[1];
            let picked = support_picked_at(&sets[*set].session, *at).expect("its support");
            assert_eq!(&picked, id);
        });
    }

    /// Loads that pull each set's cube with `force_n` on one layer just
    /// over every tip (#244), held by all its tips.
    fn one_layer_pull(
        sets: &[DrawnSet],
        frame: &sc_print_glue::PlateFrame,
        force_n: f64,
    ) -> bread_review::PlateLoads {
        let mut loads = bread_review::PlateLoads::default();
        let mut bodies = Vec::new();
        let mut top: f64 = 0.0;
        for drawn in sets {
            let mut tips = Vec::new();
            for piece in drawn.session.pieces() {
                for contact in &piece.contacts {
                    let at_mm = frame.point_mm(drawn.leaf, contact.tip);
                    top = top.max(at_mm[2]);
                    tips.push(loads.tips.len());
                    loads.tips.push(bread_review::TipLoad {
                        at_mm,
                        radius_mm: contact.tip_radius_mm,
                        layer: 10,
                        worst_n: 0.0,
                        worst_layer: 10,
                        capacity_n: bread_peel::Band::new(1.0, 1.0, 1.0),
                    });
                }
            }
            let c = loads.tips[tips[0]].at_mm;
            bodies.push(bread_review::BodyCase {
                force_n: bread_peel::Band::new(force_n * 0.5, force_n, force_n * 1.5),
                at_mm: [c[0], c[1]],
                held: false,
                tips,
                feet: Vec::new(),
            });
        }
        loads.cases = vec![bread_review::LayerCase {
            layer: 40,
            z_mm: top + 0.2,
            bodies,
        }];
        loads
    }

    /// The supports checked as a frame (#244): each cube's set carries its
    /// cube's pull, every support a peel reaches is painted, a stop stops.
    #[test]
    fn a_critical_layer_checks_the_supports_as_a_frame() {
        use std::sync::atomic::AtomicBool;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run, _leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let sets = run.sets_in_force();
            let frame = sc_print_glue::PlateFrame::default();
            let loads = one_layer_pull(&sets, &frame, 2.0);
            let found = crate::support_frame::frame_shares(
                &loads,
                &frame,
                &sets,
                &AtomicBool::new(false),
            )
            .expect("checked");
            let (worst, layer) = found.peak.expect("pulled");
            assert!(worst > 0.0 && worst.is_finite());
            assert_eq!(layer, 40);
            assert_eq!(found.worst.len(), sets.len());
            for (set, shares) in found.worst.iter().enumerate() {
                assert!(!shares.is_empty(), "set {set} carries its cube");
            }
            assert!(found.at(40).is_some() && found.at(41).is_none());
            let overlay = crate::stress_view::support_overlay(&sets, &found.worst);
            assert!(!overlay.is_empty());
            assert!(
                crate::support_frame::frame_shares(&loads, &frame, &sets, &AtomicBool::new(true))
                    .is_none(),
                "a stop stops"
            );
        });
    }

    /// Reinforce weak spots (#247): every support the frame finds past what
    /// it holds gets its part worst off made wider, in place, as one step
    /// to undo; checked again, it is less loaded.
    #[test]
    fn reinforcing_widens_the_weak_supports_in_one_step() {
        use std::sync::atomic::AtomicBool;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, _leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let frame = sc_print_glue::PlateFrame::default();
            let check = |run: &Rc<SupportRun>| {
                let sets = run.sets_in_force();
                let loads = one_layer_pull(&sets, &frame, 40.0);
                let found =
                    crate::support_frame::frame_shares(&loads, &frame, &sets, &AtomicBool::new(false))
                        .expect("checked");
                (sets, found)
            };
            let (sets, found) = check(&run);
            let weak = found.weak();
            assert!(!weak[0].is_empty(), "40 N is past what a cube's tips hold");
            let before: Vec<(String, sc_geom::supports::SupportParts)> = weak[0]
                .iter()
                .filter_map(|(id, _, _)| Some((id.clone(), sets[0].session.parts_of(id)?)))
                .collect();
            assert!(!before.is_empty());
            for (id, u, part) in &weak[0] {
                if let Some(sizes) = reinforced_sizes(&sets[0].session, id, *u, *part) {
                    assert!(
                        sizes.tip_mm.is_some() || sizes.trunk_mm.is_some(),
                        "{id}: something widens"
                    );
                }
            }
            let peak = found.peak.expect("pulled").0;

            run.reinforce(&window, vec![(sets[0].leaf, weak[0].clone())]);
            settle_click(&run);
            let (after_sets, after) = check(&run);
            let mut wider = 0;
            for (id, old) in &before {
                let new = after_sets[0].session.parts_of(id).expect("still there");
                if new.tip_mm > old.tip_mm + 1e-9
                    || new.trunk_mm.zip(old.trunk_mm).is_some_and(|(n, o)| n > o + 1e-9)
                {
                    wider += 1;
                }
            }
            assert!(wider > 0, "the weak supports are wider");
            let reinforced = after.peak.expect("pulled").0;
            assert!(reinforced < peak, "less loaded: {reinforced} against {peak}");

            document.undo().expect("one step");
            let undone = run.sets_in_force();
            for (id, old) in &before {
                assert_eq!(undone[0].session.parts_of(id).as_ref(), Some(old));
            }
        });
    }

    /// The session in force of the scene's first support set.
    fn in_force_session(run: &SupportRun) -> Option<Arc<SupportSession>> {
        run.sets_in_force()
            .first()
            .map(|drawn| drawn.session.clone())
    }

    /// Every support leaf of the scene with its mesh, in row order.
    fn support_sets(document: &SceneDocument) -> Vec<(NodeId, Arc<sc_mesh_core::IndexedMesh>)> {
        document
            .with_scene(|spatial| support_leaves(spatial).collect())
            .unwrap_or_default()
    }

    /// The support leaf of `model`, and its mesh.
    fn set_of(
        document: &SceneDocument,
        run: &SupportRun,
        model: NodeId,
    ) -> (NodeId, Arc<sc_mesh_core::IndexedMesh>) {
        let leaf = document
            .with_scene(|spatial| sets_of_model(spatial, &run.sessions, model))
            .unwrap_or_default()
            .first()
            .copied()
            .expect("the model has a support set");
        let mesh = support_sets(document)
            .into_iter()
            .find(|(id, _)| *id == leaf)
            .map(|(_, mesh)| mesh)
            .expect("its mesh");
        (leaf, mesh)
    }

    /// #197: with two models, each has a set of its own. The one moved
    /// carries its set along live and redrawn on commit; the other set is
    /// left as it was and stays editable.
    #[test]
    fn each_of_two_models_carries_its_own_set() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            assert_eq!(support_sets(&document).len(), 2, "one set per model");
            assert_eq!(run.sets_in_force().len(), 2);
            let (first_leaf, first_mesh) = set_of(&document, &run, leaves[0]);
            let (second_leaf, second_mesh) = set_of(&document, &run, leaves[1]);
            assert_ne!(first_leaf, second_leaf);
            let start = leaf_transform(&document, second_leaf);
            let (at, there) = ([50.0, 20.0, 8.0], [50.0, 40.0, 8.0]);

            // Live, only the moved model's set rides along.
            move_live(&document, leaves[1], at, there);
            assert_eq!(
                leaf_transform(&document, second_leaf),
                translation([0.0, 20.0, 0.0])
            );
            assert_eq!(
                leaf_transform(&document, first_leaf),
                leaf_transform(&document, first_leaf)
            );
            commit_move(&document, leaves[1], at, there);
            assert_eq!(
                leaf_transform(&document, second_leaf),
                translation([0.0, 20.0, 0.0]).compose(&start)
            );
            let (_, moved) = set_of(&document, &run, leaves[1]);
            assert!(
                is_moved(
                    &world_mesh(&document, second_leaf),
                    &second_mesh,
                    [0.0, 20.0]
                ),
                "the same set, moved"
            );
            let (_, first_after) = set_of(&document, &run, leaves[0]);
            assert!(
                Arc::ptr_eq(&first_after, &first_mesh),
                "the other set is untouched"
            );
            assert_eq!(run.sets_in_force().len(), 2, "both still editable");

            // A hand edit on the first model edits its set alone.
            window.set_support_tool(1);
            run.on_tool_click([20.2, 20.2, 7.5]);
            assert!(
                window.get_supports_last_run().contains("edited by hand"),
                "{}",
                window.get_supports_last_run()
            );
            let (_, first_edited) = set_of(&document, &run, leaves[0]);
            assert!(!Arc::ptr_eq(&first_edited, &first_mesh));
            let (_, second_now) = set_of(&document, &run, leaves[1]);
            assert!(Arc::ptr_eq(&second_now, &moved));
            assert_eq!(run.sets_in_force().len(), 2);

            // Undo takes the hand edit back, then the move: each set as it was.
            assert!(document.undo().unwrap());
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[0]).1,
                &first_mesh
            ));
            assert!(document.undo().unwrap());
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[1]).1,
                &second_mesh
            ));
            assert_eq!(run.sets_in_force().len(), 2);
        });
    }

    /// #180: an arrangement moves several models in one edit. Each set goes
    /// along with its model, one moved in X/Y and one turned about the
    /// vertical axis, redrawn in that edit's undo entry and still editable;
    /// nothing is generated again. One Undo takes everything back.
    #[test]
    fn an_arrangement_of_two_models_carries_both_sets_in_one_step() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let (first_leaf, first_mesh) = set_of(&document, &run, leaves[0]);
            let (second_leaf, second_mesh) = set_of(&document, &run, leaves[1]);
            let (first_start, second_start) = (
                leaf_transform(&document, first_leaf),
                leaf_transform(&document, second_leaf),
            );
            let sessions = |run: &SupportRun| -> Vec<Arc<SupportSession>> {
                run.sets_in_force()
                    .into_iter()
                    .map(|drawn| drawn.session)
                    .collect()
            };
            let before = sessions(&run);
            let (moved_to, turn) = (
                translation([30.0, 40.0, 8.0]),
                turned([50.0, 20.0, 8.0], std::f64::consts::FRAC_PI_2),
            );
            let edit = SceneEdit::Compound(vec![
                SceneEdit::SetLocalTransform {
                    node_id: leaves[0],
                    before: translation([20.0, 20.0, 8.0]),
                    after: moved_to,
                },
                SceneEdit::SetLocalTransform {
                    node_id: leaves[1],
                    before: translation([50.0, 20.0, 8.0]),
                    after: turn,
                },
            ]);
            document.apply_edit(edit.clone()).unwrap();
            document.push_edit(edit);

            // Both sets carried where their models now are on their leaves'
            // transforms (#560), nothing to make again.
            let (_, first_moved) = set_of(&document, &run, leaves[0]);
            assert!(
                is_moved(
                    &world_mesh(&document, first_leaf),
                    &first_mesh,
                    [10.0, 20.0]
                ),
                "the first set, moved"
            );
            let (_, second_turned) = set_of(&document, &run, leaves[1]);
            assert!(
                Arc::ptr_eq(&second_turned, &second_mesh),
                "nothing drawn again"
            );
            assert_eq!(second_turned.faces().len(), second_mesh.faces().len());
            // A quarter turn about the model's corner, (50, 20).
            let turned_to = |p: &[f32; 3]| [50.0 - (p[1] - 20.0), 20.0 + (p[0] - 50.0), p[2]];
            let bounds = |points: Vec<[f32; 3]>| {
                points
                    .iter()
                    .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
                        (
                            std::array::from_fn(|k| lo[k].min(p[k])),
                            std::array::from_fn(|k| hi[k].max(p[k])),
                        )
                    })
            };
            let (lo, hi) = bounds(
                world_mesh(&document, second_leaf)
                    .vertices()
                    .iter()
                    .map(|v| *v.as_array())
                    .collect(),
            );
            let (want_lo, want_hi) = bounds(
                second_mesh
                    .vertices()
                    .iter()
                    .map(|v| turned_to(v.as_array()))
                    .collect(),
            );
            for k in 0..3 {
                assert!((lo[k] - want_lo[k]).abs() < 1e-3, "{lo:?} {want_lo:?}");
                assert!((hi[k] - want_hi[k]).abs() < 1e-3, "{hi:?} {want_hi:?}");
            }
            assert_ne!(leaf_transform(&document, first_leaf), first_start);
            assert_ne!(leaf_transform(&document, second_leaf), second_start);
            assert!(
                document.supports_wanted().is_none(),
                "nothing to make again"
            );
            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_none(), "no run");
            let after = sessions(&run);
            assert_eq!(after.len(), 2, "both editable");
            assert!(
                after
                    .iter()
                    .all(|s| before.iter().all(|b| !Arc::ptr_eq(s, b))),
                "each session went along"
            );

            // One Undo takes both models and both sets back; Redo again.
            assert!(document.undo().unwrap());
            assert_eq!(
                leaf_transform(&document, leaves[0]),
                translation([20.0, 20.0, 8.0])
            );
            assert_eq!(
                leaf_transform(&document, leaves[1]),
                translation([50.0, 20.0, 8.0])
            );
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[0]).1,
                &first_mesh
            ));
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[1]).1,
                &second_mesh
            ));
            assert_eq!(sessions(&run).len(), 2);
            assert!(document.redo().unwrap());
            assert_eq!(leaf_transform(&document, leaves[0]), moved_to);
            assert_eq!(leaf_transform(&document, leaves[1]), turn);
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[0]).1,
                &first_moved
            ));
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[1]).1,
                &second_turned
            ));
            assert_eq!(sessions(&run).len(), 2, "still editable after Redo");
        });
    }

    /// #180: Arrange all moves both supported models in one edit. Each set
    /// goes along, redrawn and still editable, nothing is generated again,
    /// and one Undo takes everything back. Arrange selection moves the
    /// selected model alone.
    #[test]
    fn arranging_the_plate_carries_every_set_in_one_step() {
        use crate::arrange_run::{ArrangeOptions, arrange_now};
        use crate::selection::{LeafId, Selection};
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let before: Vec<Transform3D> = leaves
                .iter()
                .map(|&l| leaf_transform(&document, l))
                .collect();
            let meshes: Vec<_> = leaves
                .iter()
                .map(|&l| set_of(&document, &run, l).1)
                .collect();
            let plate = bread_arrange::Plate {
                min: [0.0, 0.0],
                max: [60.0, 40.0],
                floor: 0.0,
            };
            let outcome =
                arrange_now(&document, &plate, ArrangeOptions::default(), false).expect("arranged");
            assert!(outcome.left_off().is_empty());
            let at = |l: NodeId| match leaf_transform(&document, l) {
                Transform3D::Affine(a) => *a.translation(),
                Transform3D::Identity => [0.0; 3],
            };
            for (i, &l) in leaves.iter().enumerate() {
                assert_ne!(leaf_transform(&document, l), before[i], "moved");
                assert!(
                    Arc::ptr_eq(&set_of(&document, &run, l).1, &meshes[i]),
                    "its set carried where it went, not drawn again"
                );
                assert!((at(l)[2] - 8.0).abs() < 1e-9, "still 8 mm up");
            }
            let (a, b) = (at(leaves[0]), at(leaves[1]));
            assert!((a[0] - b[0]).hypot(a[1] - b[1]) > 2.0, "kept apart");
            assert_eq!(run.sets_in_force().len(), 2, "both editable");
            assert!(
                document.supports_wanted().is_none(),
                "nothing to make again"
            );
            run.watch_wanted(&window);
            assert!(run.handle.borrow().is_none(), "no run");

            // One Undo takes both models and both sets back.
            assert!(document.undo().unwrap());
            for (i, &l) in leaves.iter().enumerate() {
                assert_eq!(leaf_transform(&document, l), before[i]);
                assert!(Arc::ptr_eq(&set_of(&document, &run, l).1, &meshes[i]));
            }
            assert_eq!(run.sets_in_force().len(), 2);

            // The selection alone: the other model stays where it is.
            document.set_selection(Selection::One(LeafId(leaves[1])));
            arrange_now(&document, &plate, ArrangeOptions::default(), true).expect("arranged");
            assert_eq!(leaf_transform(&document, leaves[0]), before[0]);
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[0]).1,
                &meshes[0]
            ));
            assert_ne!(leaf_transform(&document, leaves[1]), before[1]);
            assert_eq!(run.sets_in_force().len(), 2);
        });
    }

    /// #356: Fill bed copies a supported model with its set: each copy gets
    /// a set of its own, where it stands and in force (editable and saved as
    /// data), the original's stays as it was, nothing is generated again,
    /// and one Undo takes every copy away. Every copy shows the original's
    /// support mesh, carried there by its leaf (#561).
    #[test]
    fn filling_the_plate_gives_each_copy_its_own_set() {
        use crate::arrange_run::{ArrangeOptions, fill_now};
        use crate::selection::{LeafId, Selection};
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let original = leaves[0];
            let (_, set_mesh) = set_of(&document, &run, original);
            let model_mesh = document
                .with_scene(|s| s.graph().leaf_mesh_arc(original).cloned())
                .flatten()
                .unwrap();
            document.set_selection(Selection::One(LeafId(original)));
            let plate = bread_arrange::Plate {
                min: [0.0, 0.0],
                max: [60.0, 40.0],
                floor: 0.0,
            };
            let options = ArrangeOptions {
                max_copies: 3,
                ..ArrangeOptions::default()
            };
            let filled = fill_now(&document, &plate, options).expect("filled").0;
            assert_eq!(filled.copies.len(), 3);
            assert!(Arc::ptr_eq(&set_of(&document, &run, original).1, &set_mesh));
            let sets = run.sets_in_force();
            assert_eq!(sets.len(), 4, "each copy's set is in force");
            let top = |mesh: &sc_mesh_core::IndexedMesh| {
                mesh.vertices()
                    .iter()
                    .map(|v| v.as_array()[2])
                    .fold(f32::MIN, f32::max)
            };
            for drawn in sets.iter().filter(|d| d.model != original) {
                let (leaf, mesh) = set_of(&document, &run, drawn.model);
                assert_eq!(leaf, drawn.leaf);
                assert!(sc_geom::supports::is_support_mesh(&mesh));
                assert!(Arc::ptr_eq(&mesh, &set_mesh), "one support mesh for all");
                let carried = document
                    .with_scene(|s| s.leaf(drawn.leaf).map(|q| q.world_transform()))
                    .flatten()
                    .unwrap();
                let mesh = sc_scene::bake_mesh_to_world(&mesh, &carried).unwrap();
                let (copy_mesh, copy_at) = document
                    .with_scene(|s| {
                        let q = s.leaf(drawn.model)?;
                        let at = match q.world_transform() {
                            Transform3D::Affine(a) => *a.translation(),
                            Transform3D::Identity => [0.0; 3],
                        };
                        Some((s.graph().leaf_mesh_arc(drawn.model)?.clone(), at))
                    })
                    .flatten()
                    .unwrap();
                assert!(Arc::ptr_eq(&copy_mesh, &model_mesh), "the same mesh");
                assert!((copy_at[2] - 8.0).abs() < 1e-9, "still 8 mm up");
                // Under its copy, as high as the original's.
                let (lo, hi) =
                    mesh.vertices()
                        .iter()
                        .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), v| {
                            let p = v.as_array();
                            (
                                [lo[0].min(p[0]), lo[1].min(p[1])],
                                [hi[0].max(p[0]), hi[1].max(p[1])],
                            )
                        });
                for k in 0..2 {
                    let c = copy_at[k] as f32;
                    assert!(lo[k] < c && c < hi[k], "{lo:?} {hi:?} {copy_at:?}");
                }
                assert!((top(&mesh) - top(&set_mesh)).abs() < 0.2);
            }
            let saved = document
                .with_scene(|spatial| crate::support_persist::support_documents(&document, spatial))
                .unwrap();
            assert_eq!(saved.documents.len(), 4, "every set saved as data");
            assert!(saved.as_mesh.is_none());
            assert!(
                document.supports_wanted().is_none(),
                "nothing to make again"
            );

            // One Undo takes the copies and their sets away; Redo puts them
            // back, still in force.
            assert!(document.undo().unwrap());
            assert_eq!(run.sets_in_force().len(), 1);
            assert_eq!(support_sets(&document).len(), 1);
            assert!(document.redo().unwrap());
            assert_eq!(run.sets_in_force().len(), 4);
        });
    }

    /// #356: times Fill bed on a real model with supports generated on it,
    /// on the M7 Pro's plate: the search, drawing every copy's set, and
    /// adding the copies; then Undo and Redo.
    /// `FILL_MODEL=<stl> [FILL_SCALE=0.5] cargo test -p sc-slicer-gui --release fill_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "needs FILL_MODEL, a real model"]
    fn fill_timing() {
        use std::time::Instant;

        use crate::arrange_run::{ArrangeOptions, fill_now};
        use crate::selection::{LeafId, Selection};
        let path = std::env::var("FILL_MODEL").expect("FILL_MODEL");
        let scale: f32 = std::env::var("FILL_SCALE").map_or(1.0, |s| s.parse().unwrap());
        on_fresh_slint_thread(move || {
            let loaded = sc_mesh_formats::load_mesh_from_path(
                std::path::Path::new(&path),
                None,
                Some(sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES),
            )
            .expect("the model opens");
            let mut mesh = sc_mesh_formats::scene_writer::flatten_graph_to_mesh(&loaded.graph)
                .unwrap()
                .mesh;
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &mut mesh.vertices {
                for c in v.as_array_mut() {
                    *c *= scale;
                }
                let p = v.as_array();
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            // Its middle over the plate's, 5 mm up.
            let at = [
                111.8 - f64::from(lo[0] + hi[0]) * 0.5,
                63.5 - f64::from(lo[1] + hi[1]) * 0.5,
                5.0 - f64::from(lo[2]),
            ];
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(None, translation(at), LeafPayload::Mesh(Arc::new(mesh)))
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("model.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let t = Instant::now();
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            println!(
                "supports generated in {:.0} ms",
                t.elapsed().as_secs_f64() * 1e3
            );
            document.set_selection(Selection::One(LeafId(leaf)));
            let plate = bread_arrange::Plate {
                min: [0.0, 0.0],
                max: [223.642, 126.976],
                floor: 0.0,
            };
            let t = Instant::now();
            let (filled, sets) =
                fill_now(&document, &plate, ArrangeOptions::default()).expect("filled");
            let total = t.elapsed();
            let turned = filled
                .copies
                .iter()
                .filter(|c| c.placement.angle != 0.0)
                .count();
            println!(
                "{} copies ({turned} turned) in {:.0} ms: footprints {:.0} ms, search {:.0} ms, supports {:.0} ms, sets in force {}",
                filled.copies.len(),
                total.as_secs_f64() * 1e3,
                filled.timings.footprints.as_secs_f64() * 1e3,
                filled.timings.search.as_secs_f64() * 1e3,
                sets.as_secs_f64() * 1e3,
                run.sets_in_force().len()
            );
            let t = Instant::now();
            assert!(document.undo().unwrap());
            let undo = t.elapsed();
            let t = Instant::now();
            assert!(document.redo().unwrap());
            println!(
                "undo {:.0} ms, redo {:.0} ms",
                undo.as_secs_f64() * 1e3,
                t.elapsed().as_secs_f64() * 1e3
            );
        });
    }

    /// #356: a supported model in the plate's middle moves to let more
    /// copies in: its set is redrawn where it goes and stays in force, in
    /// the same step as the copies, and Undo puts both back.
    #[test]
    fn a_supported_model_moved_by_the_fill_takes_its_set() {
        use crate::arrange_run::{ArrangeOptions, fill_now};
        use crate::selection::{LeafId, Selection};
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            // Without the pillars' grid, whose pillars stand off the model
            // where the plate's grid has its nodes.
            let request = SupportRequest {
                options: without_grid(SupportOptions::default()),
                ..SupportRequest::default()
            };
            let (ctrl, run, leaves) = supported_by(&window, &[[20.0, 20.0, 8.0]], request);
            let document = ctrl.document();
            let model = leaves[0];
            let (set_leaf, set_mesh) = set_of(&document, &run, model);
            let start = leaf_transform(&document, model);
            // The plate around it: room for two rows and four columns of it
            // with its raft, so in its middle it splits the plate.
            let size = set_mesh
                .vertices()
                .iter()
                .fold([0.0f64; 2], |m, v| {
                    let p = v.as_array();
                    [
                        m[0].max((f64::from(p[0]) - 20.0).abs()),
                        m[1].max((f64::from(p[1]) - 20.0).abs()),
                    ]
                })
                .map(|half| half * 2.0 + 1.6);
            let plate = bread_arrange::Plate {
                min: [20.0 - 2.0 * size[0] - 5.0, 20.0 - size[1] - 5.0],
                max: [20.0 + 2.0 * size[0] + 5.0, 20.0 + size[1] + 5.0],
                floor: 0.0,
            };
            document.set_selection(Selection::One(LeafId(model)));
            let options = ArrangeOptions {
                margin_mm: 5.0,
                ..ArrangeOptions::default()
            };
            let filled = fill_now(&document, &plate, options).expect("filled").0;
            assert_eq!(filled.moved.len(), 1, "it moves; {size:?}");
            assert!(filled.copies.len() >= 7, "{}", filled.copies.len());
            assert_ne!(leaf_transform(&document, model), start);
            let (leaf, mesh) = set_of(&document, &run, model);
            assert_eq!(leaf, set_leaf, "the same set");
            assert!(!Arc::ptr_eq(&mesh, &set_mesh), "redrawn where it went");
            assert!(
                matches!(leaf_transform(&document, leaf), Transform3D::Identity)
                    || leaf_transform(&document, leaf)
                        == Transform3D::Affine(AffineTransform3D::identity()),
                "its leaf stays put: the set is drawn in world space"
            );
            let sets = run.sets_in_force();
            assert!(sets.iter().any(|d| d.model == model), "still in force");
            assert_eq!(sets.len(), 1 + filled.copies.len());

            assert!(document.undo().unwrap());
            assert_eq!(leaf_transform(&document, model), start);
            assert!(Arc::ptr_eq(&set_of(&document, &run, model).1, &set_mesh));
            assert_eq!(run.sets_in_force().len(), 1);
        });
    }

    /// #356: a copy turned a quarter turn about the vertical axis gets its
    /// model's set turned with it, as a turn of the model takes it (#153).
    #[test]
    fn a_turned_copy_gets_its_set_turned() {
        use crate::arrange_run::copied_set_now;
        use crate::selection::{LeafId, Selection};
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let (_, set_mesh) = set_of(&document, &run, leaves[0]);
            document.set_selection(Selection::One(LeafId(leaves[0])));
            // A quarter turn about the model's middle, then 15 mm along X.
            let placement = bread_arrange::Placement {
                angle: std::f64::consts::FRAC_PI_2,
                translation: [55.0, 0.0],
                on_plate: true,
            };
            let (session, mesh) = copied_set_now(&document, &placement).expect("drawn");
            assert_eq!(session.model_count(), 1);
            assert!(session.supports_only_on(0));
            assert!(sc_geom::supports::is_support_mesh(&mesh));
            let turned_to = |p: &[f32; 3]| [55.0 - p[1], p[0], p[2]];
            let bounds = |points: Vec<[f32; 3]>| {
                points
                    .iter()
                    .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
                        (
                            std::array::from_fn(|k| lo[k].min(p[k])),
                            std::array::from_fn(|k| hi[k].max(p[k])),
                        )
                    })
            };
            let (lo, hi) = bounds(mesh.vertices().iter().map(|v| *v.as_array()).collect());
            let (want_lo, want_hi) = bounds(
                set_mesh
                    .vertices()
                    .iter()
                    .map(|v| turned_to(v.as_array()))
                    .collect(),
            );
            for k in 0..3 {
                assert!((lo[k] - want_lo[k]).abs() < 0.2, "{lo:?} {want_lo:?}");
                assert!((hi[k] - want_hi[k]).abs() < 0.2, "{hi:?} {want_hi:?}");
            }
        });
    }

    /// #13: `G` with two supported models selected moves both by the same
    /// offset; their supports ride along while it is live, are redrawn where
    /// the models land, still editable, and one Undo takes everything back.
    #[test]
    fn moving_several_supported_models_together_carries_every_set_in_one_step() {
        use crate::camera::{AxisView, OrbitCamera};
        use crate::selection::{LeafId, Selection};
        use crate::viewport::ScreenPoint;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (ctrl, run, leaves) =
                supported_in(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let document = ctrl.document();
            let at = |l: NodeId| match leaf_transform(&document, l) {
                Transform3D::Affine(a) => *a.translation(),
                Transform3D::Identity => [0.0; 3],
            };
            let before: Vec<[f64; 3]> = leaves.iter().map(|&l| at(l)).collect();
            let sets: Vec<(NodeId, Arc<sc_mesh_core::IndexedMesh>)> =
                leaves.iter().map(|&l| set_of(&document, &run, l)).collect();
            let set_before: Vec<[f64; 3]> = sets.iter().map(|&(s, _)| at(s)).collect();

            ctrl.set_section(WorkspaceSection::Transform);
            ctrl.set_viewport_for_test(400.0, 200.0);
            let mut camera = OrbitCamera::new(glam::Vec3::new(35.0, 20.0, 8.0), 100.0);
            camera.set_axis_view(AxisView::Top);
            ctrl.set_camera_for_test(camera);
            document.set_selection(Selection::of(leaves.iter().map(|&l| LeafId(l))));
            ctrl.on_modal_transform_start(0, 200.0, 100.0);
            ctrl.on_pointer_move(
                ScreenPoint::new(230.0, 100.0),
                crate::PointerModifiers::default(),
            );
            let d: Vec<[f64; 3]> = leaves
                .iter()
                .zip(&before)
                .map(|(&l, b)| {
                    let a = at(l);
                    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
                })
                .collect();
            assert!(d[0][0].hypot(d[0][1]) > 1.0, "moved: {:?}", d[0]);
            assert!((0..3).all(|k| (d[0][k] - d[1][k]).abs() < 1e-9), "together");
            assert!(d[0][2].abs() < 1e-9, "in the plate plane");
            // Live, each set rides along with its model.
            for (i, &(set, _)) in sets.iter().enumerate() {
                let s = at(set);
                assert!((s[0] - set_before[i][0] - d[0][0]).abs() < 1e-9);
                assert!((s[1] - set_before[i][1] - d[0][1]).abs() < 1e-9);
            }

            ctrl.on_modal_transform_finish(true);
            for (i, &l) in leaves.iter().enumerate() {
                let (set, mesh) = set_of(&document, &run, l);
                let s = at(set);
                assert!(
                    (s[0] - set_before[i][0] - d[0][0]).abs() < 1e-9
                        && (s[1] - set_before[i][1] - d[0][1]).abs() < 1e-9,
                    "the set's leaf carried along"
                );
                assert!(
                    Arc::ptr_eq(&mesh, &sets[i].1),
                    "its set carried where it went, not drawn again"
                );
            }
            assert_eq!(run.sets_in_force().len(), 2, "both editable");
            assert!(
                document.supports_wanted().is_none(),
                "nothing to make again"
            );

            // One Undo takes both models and both sets back.
            assert!(document.undo().unwrap());
            for (i, &l) in leaves.iter().enumerate() {
                assert_eq!(at(l), before[i]);
                assert!(Arc::ptr_eq(&set_of(&document, &run, l).1, &sets[i].1));
            }
        });
    }

    /// #13, #218: `R` about Z with two supported models selected turns them
    /// together about their shared centre and their sets follow, redrawn and
    /// editable; `S` scales them together and takes the sets away for new
    /// ones (#153). Each is one Undo step.
    #[test]
    fn turning_and_scaling_several_supported_models_together_treats_their_sets_as_one_models() {
        use crate::camera::{AxisView, OrbitCamera};
        use crate::selection::{LeafId, Selection};
        use crate::viewport::ScreenPoint;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (ctrl, run, leaves) =
                supported_in(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let document = ctrl.document();
            let before: Vec<Transform3D> = leaves
                .iter()
                .map(|&l| leaf_transform(&document, l))
                .collect();
            let meshes: Vec<_> = leaves
                .iter()
                .map(|&l| set_of(&document, &run, l).1)
                .collect();
            ctrl.set_section(WorkspaceSection::Transform);
            ctrl.set_viewport_for_test(400.0, 200.0);
            let mut camera = OrbitCamera::new(glam::Vec3::new(35.0, 20.0, 8.0), 100.0);
            camera.set_axis_view(AxisView::Top);
            ctrl.set_camera_for_test(camera.clone());
            let centre = crate::picking::project_world_to_screen(
                &camera,
                crate::mesh_render::default_fov_y(),
                400.0,
                200.0,
                glam::Vec3::new(35.0, 20.0, 8.0),
            )
            .expect("in view");
            document.set_selection(Selection::of(leaves.iter().map(|&l| LeafId(l))));
            let mods = crate::PointerModifiers::default;

            // R, Z, a quarter turn of the cursor about the centre.
            ctrl.on_modal_transform_start(1, centre.0 + 40.0, centre.1);
            ctrl.on_modal_transform_axis(2, centre.0 + 40.0, centre.1);
            ctrl.on_pointer_move(ScreenPoint::new(centre.0, centre.1 - 40.0), mods());
            ctrl.on_modal_transform_finish(true);
            for (i, &l) in leaves.iter().enumerate() {
                assert_ne!(leaf_transform(&document, l), before[i], "turned");
                assert!(
                    Arc::ptr_eq(&set_of(&document, &run, l).1, &meshes[i]),
                    "its set carried where it turned to, not drawn again"
                );
            }
            assert_eq!(run.sets_in_force().len(), 2, "both editable");
            assert!(
                document.supports_wanted().is_none(),
                "nothing to make again"
            );
            assert!(document.undo().unwrap());
            for (i, &l) in leaves.iter().enumerate() {
                assert_eq!(leaf_transform(&document, l), before[i]);
                assert!(Arc::ptr_eq(&set_of(&document, &run, l).1, &meshes[i]));
            }

            // S: twice the size, about the shared centre.
            document.set_selection(Selection::of(leaves.iter().map(|&l| LeafId(l))));
            ctrl.on_modal_transform_start(2, centre.0 + 40.0, centre.1);
            ctrl.on_pointer_move(ScreenPoint::new(centre.0 + 80.0, centre.1), mods());
            ctrl.on_modal_transform_finish(true);
            let (_, wanted) = document.supports_wanted().expect("new supports wanted");
            let mut ids = wanted.ids();
            ids.sort();
            let mut expected = leaves.clone();
            expected.sort();
            assert_eq!(ids, expected, "both models wait for new supports");
            assert!(run.sets_in_force().is_empty(), "the old sets went");
            assert!(document.undo().unwrap());
            for (i, &l) in leaves.iter().enumerate() {
                assert_eq!(leaf_transform(&document, l), before[i]);
                assert!(Arc::ptr_eq(&set_of(&document, &run, l).1, &meshes[i]));
            }
        });
    }

    /// #182: an Orient option that tilts a supported model takes its set
    /// away in the same Undo step and leaves the model waiting for new
    /// supports (#153); one Undo puts the pose and the set back.
    #[test]
    fn an_orient_option_that_tilts_a_supported_model_makes_its_supports_again() {
        use crate::orient_run::OrientRun;
        use crate::selection::{LeafId, Selection};
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let before = leaf_transform(&document, leaves[0]);
            let (_, mesh) = set_of(&document, &run, leaves[0]);
            document.set_selection(Selection::One(LeafId(leaves[0])));
            let ctrl = Rc::new(ViewportController::new_for_test());
            let orient =
                OrientRun::install(&window, document.clone(), ctrl.dialogs(), ctrl, run.clone());
            let tilting = orient
                .search_now(bread_orient::SearchMode::Quick)
                .iter()
                .position(|c| c.tilt_deg >= 1.0)
                .expect("an option tilts the cube");
            assert!(window.get_orient_warning().contains("supports"));
            orient.pick(tilting as i32);
            assert!(orient.apply());
            assert_ne!(leaf_transform(&document, leaves[0]), before);
            assert!(support_sets(&document).is_empty(), "the old set is gone");
            let (_, wants) = document.supports_wanted().expect("new supports wanted");
            assert_eq!(wants.ids(), vec![leaves[0]]);

            assert!(document.undo().unwrap());
            assert_eq!(leaf_transform(&document, leaves[0]), before);
            assert!(Arc::ptr_eq(&set_of(&document, &run, leaves[0]).1, &mesh));
            assert!(document.supports_wanted().is_none());
        });
    }

    /// #15: laying a supported model on a side face tilts it: its set goes
    /// in the same Undo step and it waits for new supports (#153); one Undo
    /// puts the pose and the set back.
    #[test]
    fn laying_a_supported_model_on_a_face_makes_its_supports_again() {
        use crate::camera::{AxisView, OrbitCamera};
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (ctrl, run, leaves) = supported_in(&window, &[[20.0, 20.0, 8.0]]);
            let document = ctrl.document();
            let before = leaf_transform(&document, leaves[0]);
            let (_, mesh) = set_of(&document, &run, leaves[0]);
            ctrl.set_section(WorkspaceSection::Transform);
            ctrl.set_viewport_for_test(200.0, 100.0);
            let mut camera = OrbitCamera::new(glam::Vec3::new(20.0, 20.0, 8.0), 20.0);
            camera.set_axis_view(AxisView::Front);
            ctrl.set_camera_for_test(camera);
            ctrl.set_lay_on_face(true);
            let centre = crate::viewport::ScreenPoint::new(100.0, 50.0);
            ctrl.on_pointer_down(centre, crate::PointerModifiers::default());
            ctrl.on_pointer_up(centre, crate::PointerModifiers::default());
            assert_ne!(leaf_transform(&document, leaves[0]), before, "laid down");
            assert!(support_sets(&document).is_empty(), "the old set is gone");
            let (_, wants) = document.supports_wanted().expect("new supports wanted");
            assert_eq!(wants.ids(), vec![leaves[0]]);

            assert!(document.undo().unwrap());
            assert_eq!(leaf_transform(&document, leaves[0]), before);
            assert!(Arc::ptr_eq(&set_of(&document, &run, leaves[0]).1, &mesh));
        });
    }

    /// #197: Generate with one model selected regenerates that model's set
    /// alone, in one undoable step; with nothing selected, every set.
    #[test]
    fn generate_with_a_model_selected_replaces_its_set_alone() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let (first_leaf, first_mesh) = set_of(&document, &run, leaves[0]);
            let (_, second_mesh) = set_of(&document, &run, leaves[1]);
            let finish_run = |run: &Rc<SupportRun>| {
                let handle = run.handle.borrow_mut().take().expect("a run started");
                window.set_support_gen_in_progress(false);
                run.finish(handle.join());
            };

            // Another preset, so that a set made again is another set (the
            // same result changes nothing, #287).
            run.set_options(SupportOptions {
                zones: Some(figures_70()),
                ..Default::default()
            });
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                leaves[1],
            )));
            assert_eq!(run.target_models(), Some(vec![leaves[1]]));
            run.start();
            finish_run(&run);
            assert_eq!(support_sets(&document).len(), 2, "still one set per model");
            let (still_first, first_now) = set_of(&document, &run, leaves[0]);
            assert_eq!(still_first, first_leaf);
            assert!(Arc::ptr_eq(&first_now, &first_mesh), "the other set stays");
            let (_, second_now) = set_of(&document, &run, leaves[1]);
            assert!(!Arc::ptr_eq(&second_now, &second_mesh), "its set is new");
            assert_eq!(run.sets_in_force().len(), 2);

            // Selecting the new supports aims at their model too.
            let (second_leaf, _) = set_of(&document, &run, leaves[1]);
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                second_leaf,
            )));
            assert_eq!(run.target_models(), Some(vec![leaves[1]]));

            // One undo brings the set before back.
            assert!(document.undo().unwrap());
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[1]).1,
                &second_mesh
            ));
            assert_eq!(support_sets(&document).len(), 2);

            // Nothing selected: every set is new.
            document.set_selection(crate::selection::Selection::None);
            assert_eq!(run.target_models(), None);
            run.start();
            finish_run(&run);
            let sets = support_sets(&document);
            assert_eq!(sets.len(), 2);
            assert!(
                sets.iter()
                    .all(|(_, m)| !Arc::ptr_eq(m, &first_mesh) && !Arc::ptr_eq(m, &second_mesh))
            );
        });
    }

    /// Supports made on another grid than the tab's are counted (#576), for
    /// the notes in Supports and Arrange: changing the grid, or one raft for
    /// all taking it off, makes nothing again on its own, and Regenerate
    /// makes those models' supports again on the tab's grid.
    #[test]
    fn supports_made_on_another_grid_are_told_and_made_again_when_asked() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let count = |run: &Rc<SupportRun>| {
                run.watch_grid(&window);
                window.get_support_grid_stale()
            };
            let finish_run = |run: &Rc<SupportRun>| {
                let handle = run.handle.borrow_mut().take().expect("a run started");
                window.set_support_gen_in_progress(false);
                run.finish(handle.join());
            };
            // Made on Miniatures' 7 mm grid, as the tab's options stand.
            run.set_options(SupportOptions::default());
            assert_eq!(
                run.options.borrow().zones.as_ref().unwrap().grid_mm,
                Some(7.0)
            );
            assert_eq!(count(&run), 0);

            // Without the grid: the sets were made on one.
            let mut no_grid = SupportOptions::default();
            let mut zones = no_grid.zone_preset();
            zones.grid_mm = None;
            no_grid.zones = Some(zones);
            run.set_options(no_grid);
            assert_eq!(count(&run), 2);
            assert!(
                run.handle.borrow().is_none(),
                "nothing is made again on its own"
            );
            run.set_options(SupportOptions::default());
            assert_eq!(count(&run), 0);

            // Another spacing: told again, not made again.
            run.on_option(13, 5.0);
            assert_eq!(count(&run), 2);
            assert!(run.handle.borrow().is_none());

            // Regenerate makes both again, on the tab's grid.
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                leaves[0],
            )));
            run.regenerate_off_grid();
            finish_run(&run);
            assert_eq!(count(&run), 0);
            assert_eq!(run.sets_in_force().len(), 2);
            assert!(
                run.sets_in_force()
                    .iter()
                    .all(|s| s.session.grid_mm() == Some(5.0))
            );
            run.on_option(13, 5.0);
            assert_eq!(count(&run), 0);

            // One raft for all takes the grid off: the sets drawn again on its
            // raft (#519) keep their grid, and are told.
            run.on_option(14, 1.0);
            assert_eq!(count(&run), 2);
            run.on_option(14, 0.0);
            assert_eq!(count(&run), 0);
        });
    }

    /// A window's support driver on unit cubes centred at `cubes` (a cube
    /// centred 0.5 mm up rests on the plate), with the bus it reads the
    /// settings from: nothing generated yet.
    fn cubes_on(
        window: &MainWindow,
        cubes: &[[f64; 3]],
    ) -> (
        Rc<SceneDocument>,
        Rc<SupportRun>,
        Rc<PreferencesBus>,
        Vec<NodeId>,
    ) {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut graph = Scene::new();
        let mut leaves = Vec::new();
        for &at in cubes {
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation(at)),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            leaves.push(leaf);
        }
        document.install_graph(graph, PathBuf::from("cubes.stl"));
        let run = SupportRun::install(
            window,
            bus.clone(),
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        run.set_options(SupportOptions::default());
        (document, run, bus, leaves)
    }

    /// Runs the event loop's timers, as the app does, until the run in
    /// flight has ended and its result is in the scene.
    fn until_generated(window: &MainWindow, run: &SupportRun) {
        let started = Instant::now();
        while window.get_support_gen_in_progress() || run.handle.borrow().is_some() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "the run never ended"
            );
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The models each support set in the scene stands on, in no order.
    fn supported_models(document: &SceneDocument, run: &SupportRun) -> Vec<NodeId> {
        let mut models: Vec<NodeId> = support_sets(document)
            .into_iter()
            .filter_map(|(leaf, _)| model_of(&run.sessions, leaf))
            .collect();
        models.sort();
        models
    }

    /// The lowest point of `model` above the plate.
    fn bottom_of(document: &SceneDocument, model: NodeId) -> f64 {
        document
            .with_scene(|spatial| {
                crate::raft_clash::model_bottoms(spatial)
                    .into_iter()
                    .find(|(id, _)| *id == model)
                    .map(|(_, z)| z)
            })
            .flatten()
            .expect("the model is in the scene")
    }

    /// #388: Generate with 3 of 4 models selected, pressed in the window,
    /// gives each of the 3 a set of its own in one undo step; Cancel during
    /// the batch leaves the scene exactly as it was, Redo included.
    #[test]
    fn generate_supports_every_selected_model_in_one_undo_step() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, _, leaves) = cubes_on(
                &window,
                &[
                    [20.0, 20.0, 8.0],
                    [40.0, 20.0, 8.0],
                    [60.0, 20.0, 8.0],
                    [80.0, 20.0, 8.0],
                ],
            );
            let picked = vec![leaves[0], leaves[1], leaves[3]];
            // Picked in another order than the scene's: the run follows the
            // scene's.
            document.set_selection(crate::selection::Selection::of(
                [leaves[3], leaves[0], leaves[1]].map(crate::selection::LeafId),
            ));
            assert_eq!(run.target_models(), Some(picked.clone()));

            window.invoke_generate_supports_requested();
            until_generated(&window, &run);
            assert_eq!(supported_models(&document, &run), picked, "a set for each");
            assert_eq!(run.sets_in_force().len(), 3, "each editable by hand");

            // One undo takes the 3 sets away, and only them.
            assert!(document.undo().unwrap());
            assert!(support_sets(&document).is_empty());
            assert!(!document.can_undo(), "one undo step for the batch");
            let revision = document.scene_revision();

            // Cancel while the batch runs: nothing changes, and Redo still
            // brings the sets back.
            window.invoke_generate_supports_requested();
            assert!(window.get_support_gen_in_progress(), "the batch runs");
            window.invoke_support_gen_cancel_requested();
            until_generated(&window, &run);
            assert!(support_sets(&document).is_empty(), "nothing added");
            assert_eq!(
                document.scene_revision(),
                revision,
                "the scene is untouched"
            );
            assert!(!document.can_undo(), "no undo step");
            assert!(document.can_redo(), "the redo step stays");
            assert!(document.redo().unwrap());
            assert_eq!(supported_models(&document, &run), picked);
            assert_eq!(run.sets_in_force().len(), 3);
        });
    }

    /// #91: the box is a setting shared with every window, and the map paints
    /// by the angle the engine is set to, a zone preset's read from vertical
    /// turned around; Transform, Supports and Slice show it (#484).
    #[test]
    fn the_overhang_map_follows_its_box_the_active_preset_and_the_tab() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run, bus, _leaves) = cubes_on(&window, &[[20.0, 20.0, 0.5]]);
            let shown = || {
                run.ctrl
                    .overhang_map_for_test()
                    .map(|map| map.self_support_angle_deg())
            };
            run.ctrl.set_section(WorkspaceSection::Transform);
            assert_eq!(shown(), None);

            window.invoke_overhang_map_toggled(true);
            assert!(bus.snapshot().overhang_map);
            assert!(window.get_overhang_map_on());
            assert_eq!(shown(), Some(52.0));
            assert_eq!(window.get_overhang_map_limit(), "52°");

            run.set_options(SupportOptions {
                zones: Some(minis_30()),
                ..Default::default()
            });
            assert_eq!(shown(), Some(52.0));
            assert_eq!(window.get_overhang_map_limit(), "52°");
            // Orient scores by the angle the engine gets (#422), and by the
            // area each tip of the preset holds.
            let (angle, area) = run.overhang_terms();
            assert_eq!(angle, run.options.borrow().self_support_angle_deg());
            assert_eq!(angle, 52.0);
            assert!((area - run.options.borrow().tip_reach_mm().powi(2)).abs() < 1e-9);
            run.set_options(SupportOptions {
                zones: Some(figures_70()),
                ..Default::default()
            });
            assert_eq!(shown(), Some(55.0));

            // Supports and Slice show it too (#484); Hollow not.
            for (section, on) in [
                (WorkspaceSection::Supports, true),
                (WorkspaceSection::Slice, true),
                (WorkspaceSection::Hollow, false),
                (WorkspaceSection::Transform, true),
            ] {
                run.ctrl.set_section(section);
                assert_eq!(shown(), on.then_some(55.0), "{section:?}");
            }

            window.invoke_overhang_map_toggled(false);
            assert!(!bus.snapshot().overhang_map);
            assert!(!window.get_overhang_map_on());
            assert_eq!(shown(), None);
        });
    }

    /// Where every support of `session` touches the model, piece by piece.
    fn contact_points(session: &SupportSession) -> Vec<[f64; 3]> {
        session
            .pieces()
            .into_iter()
            .flat_map(|piece| piece.contacts)
            .map(|contact| contact.surface)
            .collect()
    }

    /// A support that runs from one part of a model to another holds what
    /// each of its ends touches, as a tip does: inside a hollow cube the
    /// supports of the cavity's ceiling stand on its floor, and the map
    /// counts both their ends.
    #[test]
    fn the_overhang_map_counts_both_ends_of_a_support_inside_a_model() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let cube = sc_mesh_fixtures::unit_cube_mesh();
            let mut vertices = Vec::new();
            let mut faces = Vec::new();
            for (side, inward) in [(20.0f32, false), (14.0, true)] {
                let base = vertices.len() as u32;
                vertices.extend(
                    cube.vertices().iter().map(|v| {
                        sc_mesh_core::primitives::Vertex::new(v.coords().map(|c| c * side))
                    }),
                );
                faces.extend(cube.faces().iter().map(|f| {
                    let [a, b, c] = f.vertices.map(|i| i.get() + base);
                    sc_mesh_core::primitives::IndexedTriangle::from_raw_indices(if inward {
                        [a, c, b]
                    } else {
                        [a, b, c]
                    })
                }));
            }
            let hollow = sc_mesh_core::IndexedMesh::new(None, vertices, faces);
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([30.0, 30.0, 10.0])),
                    LeafPayload::Mesh(Arc::new(hollow)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("hollow.stl"));
            let run = SupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            run.generate_now(SupportOptions {
                zones: Some(minis_30()),
                ..Default::default()
            });
            let sets = run.sets_in_force();
            let ends: usize = sets
                .iter()
                .flat_map(|set| set.session.pieces())
                .filter(|piece| piece.kind == sc_geom::supports::SupportPieceKind::Stick)
                .map(|piece| piece.contacts.len())
                .sum();
            let tips: usize = sets.iter().map(|set| set.session.tips().len()).sum();
            let contacts: Vec<[f32; 3]> = sets
                .iter()
                .flat_map(|set| contact_points(&set.session))
                .map(|p| p.map(|v| v as f32))
                .collect();
            assert!(ends > 0, "the cavity's ceiling stands on sticks");
            assert!(contacts.len() >= tips + ends, "{} contacts", contacts.len());
            let map: Vec<[f32; 3]> = run
                .ctrl
                .support_tips_for_test()
                .iter()
                .map(|tip| tip.pos)
                .collect();
            assert_eq!(map, contacts, "every end, where its set draws it");
        });
    }

    /// #628: Generate says when a model's supports reach past the edge of
    /// the plate, and nothing for a model whose supports fit.
    #[test]
    fn supports_past_the_plate_edge_are_told_after_generate() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let run = SupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            let edge = f64::from(ctrl.build_volume().width_mm()) / 2.0;
            let heading = "Supports past the plate edge";
            // A 10 mm cube whose supports spread about 3 mm past it: centred, then
            // with the cube inside the plate 0.5 mm from its edge; and where its
            // raft alone reaches about 0.04 mm past (a raft's edge is cut and
            // the print comes out the same).
            let mut expected = 0;
            for (x, past) in [(0.0, false), (edge - 5.5, true), (edge - 7.42, false)] {
                expected += usize::from(past);
                let mut graph = Scene::new();
                let cube = sc_mesh_fixtures::unit_cube_mesh();
                let leaf = graph
                    .add_leaf(
                        None,
                        Transform3D::Affine(
                            AffineTransform3D::from_translation([x, 0.0, 15.0])
                                .compose(&AffineTransform3D::from_uniform_scale(10.0)),
                        ),
                        LeafPayload::Mesh(Arc::new(cube)),
                    )
                    .unwrap();
                graph.add_root(leaf).unwrap();
                document.install_graph(graph, PathBuf::from("cube.stl"));
                run.generate_now(SupportOptions {
                    zones: Some(minis_30()),
                    ..Default::default()
                });
                let told = ctrl
                    .dialogs()
                    .error_headings_for_test()
                    .iter()
                    .filter(|h| h.as_str() == heading)
                    .count();
                assert_eq!(told, expected, "a model at x = {x} mm");
            }
        });
    }

    /// #484: the overhang map counts every tip of the scene's sets, where
    /// each set is drawn, with the reach of the options that set was made
    /// with, whatever the tab holds now.
    #[test]
    fn the_overhang_map_counts_each_sets_tips_with_its_own_reach() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, _, leaves) =
                cubes_on(&window, &[[20.0, 20.0, 8.0], [40.0, 20.0, 8.0]]);
            run.generate_now(SupportOptions {
                zones: Some(minis_30()),
                ..Default::default()
            });
            let reaches = || {
                let tips = run.ctrl.support_tips_for_test();
                let count = |mm: f32| tips.iter().filter(|t| t.reach_mm == mm).count();
                (tips.len(), count(1.8), count(2.8))
            };
            let in_sets = || -> usize {
                run.sets_in_force()
                    .iter()
                    .map(|set| contact_points(&set.session).len())
                    .sum()
            };
            let all = in_sets();
            assert!(all > 0, "the cubes get tips");
            assert_eq!(
                reaches(),
                (all, all, 0),
                "Minis spaces overhang tips 1.8 mm"
            );
            let first = run.ctrl.support_tips_for_test();
            let session_tips: Vec<[f32; 3]> = run
                .sets_in_force()
                .iter()
                .flat_map(|set| contact_points(&set.session))
                .map(|p| p.map(|v| v as f32))
                .collect();
            assert_eq!(
                first.iter().map(|t| t.pos).collect::<Vec<_>>(),
                session_tips,
                "each tip where its set draws it"
            );

            // The second cube's set made again with Statues 150 mm+: its
            // tips reach 2.8 mm, the first cube's keep 1.8.
            document.set_selection(crate::selection::Selection::of(
                [leaves[1]].map(crate::selection::LeafId),
            ));
            run.set_options(SupportOptions {
                zones: Some(figures_150()),
                ..Default::default()
            });
            window.invoke_generate_supports_requested();
            until_generated(&window, &run);
            let (count, short, long) = reaches();
            assert_eq!(count, in_sets());
            assert!(short > 0 && long > 0, "{short} at 1.8 mm, {long} at 2.8 mm");
            assert_eq!(short + long, count);

            // With no set left, no tip.
            while document.undo().unwrap() {}
            assert_eq!(reaches(), (0, 0, 0));
        });
    }

    /// #389: the Supports tab's box is a setting; with it on, Generate lifts
    /// the selected model resting on the plate to the support lift and
    /// leaves the one already lifted where it is, in the same undo step as
    /// the supports (#388's batch). Cancel lifts nothing.
    #[test]
    fn the_lift_on_plate_box_lifts_resting_models_with_their_supports() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, bus, leaves) = cubes_on(
                &window,
                &[[20.0, 20.0, 0.5], [40.0, 20.0, 8.0], [60.0, 20.0, 0.5]],
            );
            let (resting, lifted, left_out) = (leaves[0], leaves[1], leaves[2]);
            document.set_selection(crate::selection::Selection::of(
                [resting, lifted].map(crate::selection::LeafId),
            ));
            assert!(!window.get_support_lift_on_plate());
            assert_eq!(
                window.get_support_lift_on_plate_label(),
                "Lift 5 mm when the model rests on the plate"
            );

            // Off: the raft warning holds the press back.
            window.invoke_generate_supports_requested();
            assert!(run.handle.borrow().is_none(), "held back by the raft");
            assert!(!window.get_support_raft_clash_message().is_empty());

            // The box, ticked in the window, is stored and answers the
            // warning for the model resting on the plate.
            window.invoke_support_lift_on_plate_toggled(true);
            assert!(
                bus.snapshot().lift_models_on_plate,
                "stored in the settings"
            );
            assert!(window.get_support_lift_on_plate());
            assert!(window.get_support_raft_clash_message().is_empty());

            // Cancel: nothing moves.
            let revision = document.scene_revision();
            window.invoke_generate_supports_requested();
            window.invoke_support_gen_cancel_requested();
            until_generated(&window, &run);
            assert_eq!(document.scene_revision(), revision);
            assert!(bottom_of(&document, resting).abs() < 1e-6);
            assert!(support_sets(&document).is_empty());
            assert!(!document.can_undo());

            window.invoke_generate_supports_requested();
            until_generated(&window, &run);
            let lift = sc_geom::supports::DEFAULT_SUPPORT_LIFT_MM;
            assert!(
                (bottom_of(&document, resting) - lift).abs() < 1e-6,
                "lifted"
            );
            assert!(
                (bottom_of(&document, lifted) - 7.5).abs() < 1e-6,
                "left where it was"
            );
            assert!(bottom_of(&document, left_out).abs() < 1e-6, "not selected");
            assert_eq!(
                supported_models(&document, &run),
                {
                    let mut both = vec![resting, lifted];
                    both.sort();
                    both
                },
                "a set for each"
            );
            assert_eq!(run.sets_in_force().len(), 2, "both editable by hand");
            assert!(
                document.supports_wanted().is_none(),
                "the lift does not wait for other supports"
            );

            // One undo: back on the plate, without supports.
            assert!(document.undo().unwrap());
            assert!(bottom_of(&document, resting).abs() < 1e-6);
            assert!(support_sets(&document).is_empty());
            assert!(
                !document.can_undo(),
                "the lift and the supports are one step"
            );
            assert!(document.redo().unwrap());
            assert!((bottom_of(&document, resting) - lift).abs() < 1e-6);
            assert_eq!(run.sets_in_force().len(), 2);

            // Ticked off again: stored, and a model resting on the plate is
            // in the raft's way again.
            window.invoke_support_lift_on_plate_toggled(false);
            assert!(!bus.snapshot().lift_models_on_plate);
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                left_out,
            )));
            window.invoke_generate_supports_requested();
            assert!(run.handle.borrow().is_none(), "held back by the raft");
            assert!(bottom_of(&document, left_out).abs() < 1e-6);
        });
    }

    /// #389: a window opens with the box as the settings have it, and a
    /// switch in one window shows in the others.
    #[test]
    fn the_lift_on_plate_box_follows_the_settings() {
        on_fresh_slint_thread(|| {
            let snapshot = PreferencesSnapshot {
                lift_models_on_plate: true,
                ..PreferencesSnapshot::for_tests()
            };
            let bus = PreferencesBus::new(snapshot);
            let open = |bus: &Rc<PreferencesBus>| {
                let window =
                    MainWindow::new().expect("the headless platform creates a main window");
                let ctrl = Rc::new(ViewportController::new_for_test());
                let run = SupportRun::install(
                    &window,
                    bus.clone(),
                    ctrl.document(),
                    ctrl.dialogs(),
                    ctrl.clone(),
                );
                (window, run, ctrl)
            };
            let (first, _first_run, _first_ctrl) = open(&bus);
            let (second, _second_run, _second_ctrl) = open(&bus);
            assert!(first.get_support_lift_on_plate());
            first.invoke_support_lift_on_plate_toggled(false);
            assert!(!second.get_support_lift_on_plate());
            assert!(!bus.snapshot().lift_models_on_plate);
        });
    }

    /// #197: Remove takes only from the selected set, or the selected
    /// model's: another model's supports neither preview red nor go.
    #[test]
    fn remove_takes_only_from_the_selected_set() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, leaves) =
                supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let select = |id: NodeId| {
                document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                    id,
                )));
            };
            let last_run = || window.get_supports_last_run().to_string();
            // A support by hand under each model, so neither set is down to
            // its last one (which Remove leaves to Undo).
            window.set_support_tool(1);
            for at in [[20.2, 20.2, 7.5], [50.2, 20.2, 7.5]] {
                run.on_tool_click(at);
                assert!(last_run().contains("edited by hand"), "{}", last_run());
            }
            let (first_leaf, first_mesh) = set_of(&document, &run, leaves[0]);
            let (_, second_mesh) = set_of(&document, &run, leaves[1]);
            window.set_support_tool(2);
            let (here, there) = ([20.2, 20.2, 7.4], [50.2, 20.2, 7.4]);

            // The first set selected: the second model's supports stay.
            select(first_leaf);
            assert!(
                run.set_at(there, HandTool::Remove).is_none(),
                "no red there"
            );
            run.on_tool_click(there);
            assert!(
                last_run().contains("Select these supports"),
                "{}",
                last_run()
            );
            assert!(Arc::ptr_eq(
                &set_of(&document, &run, leaves[1]).1,
                &second_mesh
            ));
            // Its own supports go.
            assert!(run.set_at(here, HandTool::Remove).is_some());
            run.on_tool_click(here);
            assert!(last_run().contains("edited by hand"), "{}", last_run());
            assert!(!Arc::ptr_eq(
                &set_of(&document, &run, leaves[0]).1,
                &first_mesh
            ));

            // The second model selected: its supports go.
            select(leaves[1]);
            run.on_tool_click(there);
            assert!(last_run().contains("edited by hand"), "{}", last_run());
            assert!(!Arc::ptr_eq(
                &set_of(&document, &run, leaves[1]).1,
                &second_mesh
            ));

            // Nothing selected: nothing is offered.
            document.set_selection(crate::selection::Selection::None);
            assert!(run.set_at(here, HandTool::Remove).is_none());
            assert!(run.set_at(there, HandTool::Remove).is_none());
        });
    }

    /// #197: two sets save as two documents and reopen as two editable sets.
    #[test]
    fn two_sets_save_and_reopen_editable() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, _) = supported(&window, &[[20.0, 20.0, 8.0], [50.0, 20.0, 8.0]]);
            let saved: Vec<usize> = support_sets(&document)
                .iter()
                .map(|(_, m)| m.faces().len())
                .collect();
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("cubes.bread");
            crate::scene_save::save_project_to(&document, &target, &PrintDefaults::default())
                .expect("the project saves");
            let loaded = sc_project_formats::load_scene(&target, None, |_| {}).unwrap();
            let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
                panic!("a .bread opens as a project");
            };
            assert_eq!(project.supports.len(), 2, "one document per set");
            let mut scene = project.scene;
            let restored = crate::support_persist::restore_supports(&mut scene, project.supports);
            assert!(restored.failures.is_empty(), "{:?}", restored.failures);
            assert_eq!(restored.sets.len(), 2);
            document.install_project_graph(scene, target.clone(), project.payload);
            crate::support_persist::install_sets(&document, restored.sets);
            let reopened: Vec<usize> = support_sets(&document)
                .iter()
                .map(|(_, m)| m.faces().len())
                .collect();
            assert_eq!(reopened, saved);
            assert_eq!(run.sets_in_force().len(), 2, "both editable");
            window.set_support_tool(1);
            run.on_tool_click([50.2, 20.2, 7.5]);
            assert!(
                window.get_supports_last_run().contains("edited by hand"),
                "{}",
                window.get_supports_last_run()
            );
        });
    }

    #[test]
    fn supports_saved_as_bread_reopen_editable() {
        save_and_reopen(SupportRequest::default());
    }

    /// A zone preset (#360) keeps its supports through a save and reopen,
    /// and its options with them.
    #[test]
    fn zone_preset_supports_saved_as_bread_reopen_editable() {
        let mut request = SupportRequest::default();
        request.options.zones = Some(minis_30());
        save_and_reopen(request);
    }

    #[test]
    fn the_zone_presets_ship_and_round_trip_as_presets() {
        let presets = on_fresh_slint_thread(built_in_presets);
        let zoned: Vec<&Preset> = presets
            .iter()
            .filter(|p| p.options.zones.is_some())
            .collect();
        assert_eq!(zoned.len(), 3);
        assert_eq!(zoned[0].name, "Miniatures 20~65 mm");
        assert_eq!(zoned[1].name, "Figures 65~150 mm");
        assert_eq!(zoned[2].name, "Statues 150 mm+");
        for preset in zoned {
            let text = toml::to_string_pretty(&preset.options).expect("a preset saves");
            let back: SupportOptions = toml::from_str(&text).expect("a preset reads back");
            assert_eq!(back, preset.options);
            assert_eq!(back.clamped(), preset.options, "inside every bound");
            assert!(!on_fresh_slint_thread(move || zone_summary(&back)).is_empty());
        }
    }

    fn save_and_reopen(request: SupportRequest) {
        let (saved_triangles, reopened, after_add, after_undo, after_remove) =
            on_fresh_slint_thread(move || {
                let window =
                    MainWindow::new().expect("the headless platform creates a main window");
                let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
                let ctrl = Rc::new(ViewportController::new_for_test());
                let document = ctrl.document();
                let mut graph = Scene::new();
                let leaf = graph
                    .add_leaf(
                        None,
                        Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                        LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                    )
                    .unwrap();
                graph.add_root(leaf).unwrap();
                document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
                let run = SupportRun::install(
                    &window,
                    bus,
                    document.clone(),
                    ctrl.dialogs(),
                    ctrl.clone(),
                );
                let zones = request.options.zones.clone();
                let outcome = start_supports(&graph, request).unwrap().join();
                *run.started_on.borrow_mut() = Some(document.scene_revision());
                run.finish(outcome);
                let (_, saved_mesh) = run.support_leaf().expect("the supports were pasted");
                let world_box = |document: &SceneDocument| {
                    document
                        .with_scene(|s| s.graph().aabb().unwrap().unwrap())
                        .unwrap()
                };
                let saved_box = world_box(&document);

                let dir = tempfile::tempdir().unwrap();
                let target = dir.path().join("cube.bread");
                crate::scene_save::save_project_to(&document, &target, &PrintDefaults::default())
                    .expect("the project saves");
                assert_eq!(document.source_path(), Some(target.clone()));
                assert!(!document.is_dirty());

                // The file keeps the supports as data and their leaf empty.
                let stored = sc_project_formats::read_project(
                    &mut std::fs::File::open(&target).unwrap(),
                    sc_project_formats::ProjectFileType::Bread,
                )
                .unwrap();
                assert_eq!(stored.supports.len(), 1);
                assert!(
                    stored
                        .scene
                        .leaf_mesh_arc(stored.supports[0].leaf)
                        .is_some_and(|m| m.faces().is_empty())
                );

                // Reopened as the load worker and the controller do it.
                let loaded = sc_project_formats::load_scene(&target, None, |_| {}).unwrap();
                let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
                    panic!("a .bread opens as a project");
                };
                let mut scene = project.scene;
                let restored =
                    crate::support_persist::restore_supports(&mut scene, project.supports);
                assert!(restored.failures.is_empty(), "{:?}", restored.failures);
                document.install_project_graph(scene, target.clone(), project.payload);
                crate::support_persist::install_sets(&document, restored.sets);
                let reopened_session = in_force_session(&run).expect("editable");
                assert_eq!(
                    reopened_session.options().map(|o| o.zones.clone()),
                    Some(zones),
                    "the session keeps the options it ran with"
                );
                let (_, mesh) = run.support_leaf().expect("the supports are back");
                // Drawn again where the model stands: the same mesh, but for
                // the last bit of a float (the set was drawn standing alone
                // and moved there, #556).
                let reopened = mesh.faces() == saved_mesh.faces()
                    && mesh.vertices().len() == saved_mesh.vertices().len()
                    && mesh
                        .vertices()
                        .iter()
                        .zip(saved_mesh.vertices())
                        .all(|(a, b)| {
                            let (a, b) = (a.as_array(), b.as_array());
                            (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
                        })
                    && world_box(&document) == saved_box;

                window.set_support_tool(1);
                run.on_tool_click([20.2, 20.2, 7.5]);
                let after_add = window.get_supports_last_run().to_string();
                assert!(document.undo().unwrap(), "the hand edit undoes");
                let after_undo = in_force_session(&run)
                    .is_some_and(|session| Arc::ptr_eq(&session, &reopened_session));
                // The reopened session edits again after Undo: the support
                // added anew, then taken away, from the supports selected.
                run.on_tool_click([20.2, 20.2, 7.5]);
                window.set_support_tool(2);
                let (supports, _) = run.support_leaf().expect("the supports");
                document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                    supports,
                )));
                run.on_tool_click([20.2, 20.2, 7.4]);
                let after_remove = window.get_supports_last_run().to_string();
                (
                    saved_mesh.faces().len(),
                    reopened,
                    after_add,
                    after_undo,
                    after_remove,
                )
            });
        assert!(saved_triangles > 0);
        assert!(reopened, "the same supports and poses come back");
        assert!(after_add.contains("edited by hand"), "{after_add}");
        assert!(after_undo, "Undo lands on the reopened session");
        assert!(after_remove.contains("edited by hand"), "{after_remove}");
    }

    /// The tip of every support of `session` that touches the model, in mm.
    fn tips(session: &SupportSession) -> Vec<f64> {
        let c = &session.state().collections;
        c.trunks
            .values()
            .filter_map(|t| t.contact_cone.as_ref())
            .chain(c.branches.values().filter_map(|b| b.contact_cone.as_ref()))
            .chain(c.leaves.values().map(|l| &l.contact_cone))
            .map(|cone| cone.profile.contact_diameter_mm)
            .collect()
    }

    /// Of presets with the same numbers, the one clicked is the one marked
    /// (#564: clicking any of them marked the first).
    #[test]
    fn of_presets_alike_the_one_clicked_is_marked() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run, _) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let built_in = run.presets.borrow().len();
            let same = run.presets.borrow()[0].options.clone();
            for name in ["Mine", "Mine (2)"] {
                run.presets.borrow_mut().push(Preset {
                    name: name.to_owned(),
                    options: same.clone(),
                    built_in: false,
                    factory: None,
                });
            }
            run.on_preset_picked(built_in as i32 + 1);
            assert_eq!(window.get_support_own_preset_index(), 1);
            assert_eq!(window.get_support_preset_index(), built_in as i32 + 1);
            assert_eq!(window.get_support_preset_card_index(), -1);
            run.on_preset_picked(built_in as i32);
            assert_eq!(window.get_support_own_preset_index(), 0);
            run.on_preset_picked(0);
            assert_eq!(window.get_support_preset_card_index(), 0);
            assert_eq!(window.get_support_own_preset_index(), -1);
        });
    }

    /// The list of what a support settings file holds is a tree (#564): a
    /// group's head ticks all of it and shows a dash when some are ticked;
    /// each entry ticks alone; what is taken is what was ticked.
    #[test]
    fn the_share_list_is_a_tree_whose_heads_tick_their_group() {
        use crate::support_share::{Entry, SharedPreset, write};
        use slint::Model;
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run, _) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let options = |zones| SupportOptions {
                zones: Some(zones),
                ..Default::default()
            };
            let mut thick = minis_30();
            thick.rules.sizes[1].trunk_diameter_mm = 2.0;
            let mut entries = vec![
                Entry::Preset(Box::new(SharedPreset {
                    name: "Thick minis".to_owned(),
                    options: options(thick),
                })),
                Entry::Preset(Box::new(SharedPreset {
                    name: "Big statues".to_owned(),
                    options: options(figures_150()),
                })),
            ];
            let mut hand = crate::support_shape_edit::hand_factory().rules.sizes;
            hand[1].trunk_diameter_mm = 1.7;
            entries.extend(hand.into_iter().map(Entry::HandSize));
            run.offer_import(&write(&entries).expect("a file"));
            let rows = |w: &MainWindow| -> Vec<(bool, String, i32)> {
                w.get_support_share_items()
                    .iter()
                    .map(|r| (r.header, r.name.to_string(), r.state))
                    .collect()
            };
            assert_eq!(window.get_support_share_mode(), 2);
            let names: Vec<String> = rows(&window).into_iter().map(|r| r.1).collect();
            assert_eq!(
                names,
                [
                    "Automatic presets",
                    "Thick minis",
                    "Big statues",
                    "Manual supports",
                    "XS",
                    "S",
                    "M",
                    "L"
                ]
            );
            assert!(rows(&window).iter().all(|r| r.2 == 2), "all ticked");
            assert_eq!(
                window
                    .get_support_share_items()
                    .row_data(1)
                    .map(|r| r.detail.to_string()),
                Some("Made from Miniatures".to_owned())
            );
            // The head of the hand sizes: none, then S alone: a dash.
            run.on_share_toggled(3, false);
            assert!(rows(&window)[3..].iter().all(|r| r.2 == 0));
            run.on_share_toggled(5, true);
            assert_eq!(rows(&window)[3].2, 1, "some of the group");
            run.on_share_toggled(2, false);
            assert_eq!(rows(&window)[0].2, 1);
            assert_eq!(window.get_support_share_picked(), 2);
            if let Some(dir) = std::env::var_os("BREADSLICER_UI_SHOTS").map(PathBuf::from) {
                window.show().expect("the window shows");
                window
                    .window()
                    .set_size(slint::LogicalSize::new(1280.0, 860.0));
                crate::test_support::slint_thread::render_frames();
                let picture = window.window().take_snapshot().expect("a snapshot");
                image::RgbaImage::from_raw(
                    picture.width(),
                    picture.height(),
                    picture.as_bytes().to_vec(),
                )
                .expect("RGBA")
                .save(dir.join("share-tree.png"))
                .expect("written");
            }
            // Import: "Thick minis" and the small size come in.
            run.on_share_accepted();
            assert_eq!(window.get_support_share_mode(), 0);
            let hand = run.hand.borrow().clone();
            assert_eq!(hand.rules.sizes[1].trunk_diameter_mm, 1.7);
            assert_eq!(
                hand.rules.sizes[0],
                crate::support_shape_edit::hand_factory().rules.sizes[0]
            );
        });
    }

    /// The Add tool's tip size and Shift reach the engine (#3): a large tip
    /// is drawn large, a Shift+click hangs its support from another, and each
    /// click is one undo.
    #[test]
    fn tip_sizes_and_shift_click_reach_the_engine() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, _) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let generated = in_force_session(&run).expect("editable");
            assert!(!tips(&generated).contains(&0.40));
            window.set_support_tool(1);

            // Key 4: large.
            window.set_support_tip_size(4);
            run.on_tip_size_changed();
            run.on_tool_click([20.2, 20.2, 7.5]);
            assert!(window.get_supports_last_run().contains("edited by hand"));
            let large = in_force_session(&run).unwrap();
            assert!(tips(&large).contains(&0.40), "{:?}", tips(&large));
            assert!(window.get_support_tip_label().contains("0.4"));

            // The size placed is the selected preset's, whatever the hand
            // sizes say: a hand size changed to 0.55 places a 0.4 tip.
            let mut hand = run.hand.borrow().clone();
            let large_size = hand
                .rules
                .sizes
                .iter()
                .position(|s| s.id == "large")
                .expect("the large hand size");
            crate::support_shape_edit::set_value(&mut hand, large_size, 0, 0.55);
            run.set_hand(hand);
            run.on_tool_click([19.6, 20.5, 7.5]);
            let own = in_force_session(&run).unwrap();
            assert!(!tips(&own).contains(&0.55), "{:?}", tips(&own));
            assert_eq!(
                tips(&own).iter().filter(|&&t| t == 0.40).count(),
                tips(&large).iter().filter(|&&t| t == 0.40).count() + 1,
                "{:?}",
                tips(&own)
            );
            assert!(window.get_support_tip_label().contains("0.4"));
            assert!(document.undo().expect("undo"));
            run.set_hand(crate::support_shape_edit::hand_factory());

            // Shift+click: hung from a support, no root of its own.
            window.set_support_tip_size(0);
            run.on_tool_click_now([19.8, 19.8, 7.5], true);
            let hung = in_force_session(&run).unwrap();
            let c = &hung.state().collections;
            assert!(window.get_supports_last_run().contains("edited by hand"));
            assert_eq!(c.roots.len(), large.state().collections.roots.len());
            assert_eq!(c.knots.len(), large.state().collections.knots.len() + 1);

            // One undo per click.
            assert!(document.undo().unwrap());
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &large));
            assert!(document.undo().unwrap());
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));
        });
    }

    /// The hover line (#3): nothing when the support hangs as asked, why it
    /// stands on its own, why it cannot be placed or hung, and only the
    /// last two warn. The hover preview puts it under the cursor.
    #[test]
    fn the_hover_line_says_why_a_support_stands_on_its_own() {
        on_fresh_slint_thread(|| {
            let preview = |valid, attached, note| EditPreview {
                brace: None,
                mesh: sc_mesh_core::IndexedMesh::new(None, Vec::new(), Vec::new()),
                valid,
                attached,
                note,
                taken: None,
            };
            assert_eq!(
                preview_line(&preview(true, true, None), false),
                Default::default()
            );
            let (line, warns) = preview_line(&preview(true, false, Some(AddNote::TooFar)), false);
            assert_eq!(line, "Its own trunk: no support close enough to hang from.");
            assert!(!warns);
            let (line, warns) = preview_line(&preview(false, false, Some(AddNote::TooFar)), true);
            assert_eq!(
                line,
                "Cannot hang it: no support close enough to hang from."
            );
            assert!(warns);
            let (line, warns) =
                preview_line(&preview(false, false, Some(AddNote::HitsModel)), false);
            assert_eq!(line, "Cannot place it: it hits the model.");
            assert!(warns);
            assert!(preview_line(&preview(false, false, None), false).1);
            for note in [
                AddNote::TooFar,
                AddNote::NoneBelow,
                AddNote::TooSteep,
                AddNote::HitsModel,
                AddNote::CrossesSupport,
                AddNote::HostsFull,
                AddNote::SteepSurface,
            ] {
                assert!(!note_text(note).is_empty());
            }

            // The worker's answer puts its line under the cursor.
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_, run, _) = supported(&window, &[[20.0, 20.0, 8.0]]);
            window.set_support_tool(1);
            window.set_support_add_note("stale".into());
            run.on_tool_hover_with(Some([20.2, 20.2, 7.5]), true);
            let started = Instant::now();
            while run.hover_shown.get() < run.hover_generation.get() {
                assert!(started.elapsed() < Duration::from_secs(60), "no answer");
                std::thread::sleep(Duration::from_millis(5));
                run.take_hover_answer();
            }
            let answer = run.last_preview.borrow();
            let answer = answer.as_ref().expect("an answer");
            assert!(answer.how.attach, "Shift previews a forced attach");
            let preview = answer.preview.as_ref().expect("a preview");
            assert_eq!(window.get_support_add_note(), preview_line(preview, true).0);
        });
    }

    /// The Select tool (#567): a click picks a support, a number of the
    /// panel draws it again with that number in the same contact, as one
    /// step of Undo, and it stays picked.
    /// #261: a pre-supported file's imported supports made editable at the
    /// same contacts: the imported ones go and supports made here touch the
    /// model where their tips did, in one Undo step.
    #[test]
    fn imported_supports_are_made_editable_at_their_contacts() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            ctrl.dialogs().attach(&window);
            let import = crate::presupported_import::PresupportedImport::install(
                &window,
                document.clone(),
                ctrl.dialogs(),
            );
            let (mesh, _) = crate::tests_presupported_import::presupported_mesh();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("werewolf.stl"));
            import.settle_for_test();
            assert!(window.get_message_open(), "the question is up");
            window.invoke_message_answered(crate::slint_ui::MessageAnswer::Separate);
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let before = document.scene_revision();
            run.imported_to_editable();
            settle_click(&run);
            let imported = document
                .with_scene(|spatial| crate::imported_supports::imported_leaves(spatial).len())
                .unwrap();
            assert_eq!(imported, 0, "the imported supports went");
            let session = in_force_session(&run).unwrap_or_else(|| {
                panic!("supports made here: {}", window.get_supports_last_run())
            });
            let tips = session.tips();
            assert_eq!(tips.len(), 6, "{}", window.get_supports_last_run());
            for (_, tip) in &tips {
                assert!((tip[2] - 5.0).abs() < 0.3, "under the model: {tip:?}");
            }
            assert!(document.undo().unwrap());
            assert_eq!(document.scene_revision(), before, "one undo step");
        });
    }

    /// #377: a picked support is routed again from its tip, on its own or
    /// hung from the nearest, as one Undo step; the tip stays where it was.
    #[test]
    fn a_picked_support_is_routed_again_from_its_tip() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let session = run.sets_in_force()[0].session.clone();
            let tip = session
                .tips()
                .into_iter()
                .find(|(id, _)| !session.carries_others(id))
                .expect("a support at an end")
                .1;
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            run.on_tool_click_with(tip.map(|c| c as f32), false);
            assert_eq!(window.get_support_picked_count(), 1);

            let before = document.scene_revision();
            run.reroute_chosen(true);
            settle_click(&run);
            assert_ne!(
                document.scene_revision(),
                before,
                "routed again: {}",
                window.get_supports_last_run()
            );
            let drawn = run.sets_in_force()[0].clone();
            let id = drawn.session.tip_near(tip, 0.05).expect("its tip stays");
            assert!(drawn.session.state().collections.trunks.contains_key(&id));
            assert!(document.undo().unwrap());
            assert_eq!(document.scene_revision(), before, "one undo step");
        });
    }

    /// #728: H hides the picked supports from the view while the scene
    /// keeps them; Alt+H shows them again, and so does another tab.
    #[test]
    fn hidden_supports_leave_the_view_but_not_the_scene() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            ctrl.set_section(WorkspaceSection::Supports);
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let session = run.sets_in_force()[0].session.clone();
            let tip = session
                .tips()
                .into_iter()
                .find(|(id, _)| !session.carries_others(id))
                .expect("a support at an end")
                .1
                .map(|c| c as f32);
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            run.on_tool_click_with(tip, false);
            assert_eq!(window.get_support_picked_count(), 1);

            let before = document.scene_revision();
            window.invoke_support_picked_hide();
            assert_eq!(document.scene_revision(), before, "the view only");
            assert_eq!(window.get_support_picked_count(), 0, "let go");
            assert_eq!(window.get_support_hidden_count(), 1);
            let hidden = ctrl.hidden_supports();
            let set = &hidden.sets[0];
            let without = set.drawn.clone().expect("the other supports");
            assert!(without.faces().len() < set.from.faces().len());
            let shown = document
                .with_scene(|spatial| ctrl.scene_for_view(spatial.graph()))
                .flatten()
                .expect("the view draws its own scene");
            assert!(Arc::ptr_eq(
                shown.leaf_mesh_arc(set.leaf).expect("the set's leaf"),
                &without
            ));
            assert_eq!(
                run.sets_in_force()[0].session.tips().len(),
                session.tips().len()
            );

            // Where it was, a click picks nothing.
            run.on_tool_click_with(tip, false);
            assert_eq!(window.get_support_picked_count(), 0);

            window.invoke_support_hidden_show();
            assert_eq!(window.get_support_hidden_count(), 0);
            assert!(
                document
                    .with_scene(|spatial| ctrl.scene_for_view(spatial.graph()))
                    .flatten()
                    .is_none()
            );

            // Hidden again, another tab shows it.
            run.on_tool_click_with(tip, false);
            window.invoke_support_picked_hide();
            assert_eq!(window.get_support_hidden_count(), 1);
            ctrl.set_section(WorkspaceSection::Transform);
            assert_eq!(window.get_support_hidden_count(), 0);
            assert!(ctrl.hidden_supports().tips.is_empty());
        });
    }

    #[test]
    fn a_picked_support_is_drawn_again_with_the_number_typed() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            // A support no other hangs from: one that can be drawn again.
            let session = run.sets_in_force()[0].session.clone();
            let tip = session
                .tips()
                .into_iter()
                .find(|(id, _)| !session.carries_others(id))
                .expect("a support at an end")
                .1;
            let tip_f = tip.map(|c| c as f32);

            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            run.on_tool_click_with(tip_f, false);
            assert_eq!(window.get_support_picked_count(), 1, "the click picks it");
            let shown = slint::Model::row_data(&window.get_support_shape_values(), 0).unwrap();
            assert!(shown > 0.0, "the panel shows its tip: {shown}");

            let before = document.scene_revision();
            run.on_shape_value(0, 0.35);
            settle_click(&run);
            assert_ne!(
                document.scene_revision(),
                before,
                "drawn again: {}",
                window.get_supports_last_run()
            );
            let drawn = run.sets_in_force()[0].clone();
            let id = drawn
                .session
                .tip_near(tip, 0.05)
                .expect("in the same contact");
            let shape = drawn.session.shape_of(&id).expect("its numbers");
            assert!((shape.tip_diameter_mm - 0.35).abs() < 1e-6, "{shape:?}");
            assert_eq!(window.get_support_picked_count(), 1, "still picked");

            assert!(document.undo().unwrap());
            let drawn = run.sets_in_force()[0].clone();
            let id = drawn
                .session
                .tip_near(tip, 0.05)
                .expect("back where it was");
            assert!(
                (drawn.session.shape_of(&id).unwrap().tip_diameter_mm - 0.35).abs() > 1e-6,
                "Undo brings the old numbers back"
            );

            // A click on no support lets go.
            run.on_tool_click_with([100.0, 100.0, 0.0], false);
            assert_eq!(window.get_support_picked_count(), 0);
        });
    }

    /// Waits for the click being made off the UI thread to land.
    fn settle_click(run: &Rc<SupportRun>) {
        let started = Instant::now();
        while run.pending_click.borrow().is_some() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "the click hangs"
            );
            std::thread::sleep(Duration::from_millis(5));
            run.poll_click();
        }
    }

    /// #444: a click of the Add tool on a model with no set starts one, which
    /// saves and reopens as a generated one.
    #[test]
    fn a_click_on_a_model_with_no_set_starts_one_that_reopens() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            assert!(run.support_leaf().is_none());

            window.set_support_tool(1);
            run.on_tool_click_with([20.2, 20.2, 7.5], false);
            settle_click(&run);
            assert!(window.get_supports_last_run().contains("edited by hand"));
            assert!(run.support_leaf().is_some(), "the click started a set");
            assert!(in_force_session(&run).is_some(), "and it can be edited");
            // The next click goes on the same set.
            run.on_tool_click_with([19.8, 19.8, 7.5], false);
            settle_click(&run);
            let sessions = document.support_sessions();
            let sets = document
                .with_scene(|spatial| sets_in_force(spatial, &sessions).len())
                .unwrap();
            assert_eq!(sets, 1);

            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("cube.bread");
            crate::scene_save::save_project_to(&document, &target, &PrintDefaults::default())
                .expect("the project saves");
            let mut stored = sc_project_formats::read_project(
                &mut std::fs::File::open(&target).unwrap(),
                sc_project_formats::ProjectFileType::Bread,
            )
            .unwrap();
            assert_eq!(stored.supports.len(), 1);
            let restored = crate::support_persist::restore_supports(
                &mut stored.scene,
                std::mem::take(&mut stored.supports),
            );
            assert!(restored.failures.is_empty(), "{:?}", restored.failures);
            assert_eq!(restored.sets.len(), 1);

            assert!(document.undo().unwrap());
            assert!(document.undo().unwrap());
            assert!(run.support_leaf().is_none(), "undo takes the set out");
        });
    }

    /// A click is made off the UI thread (#3) and lands as one undoable
    /// step; a click while one is being made waits for it; Undo meanwhile
    /// drops it, and so does any other edit that comes first.
    #[test]
    fn a_click_made_off_the_ui_thread_lands_whole_or_not_at_all() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, _) = supported(&window, &[[20.0, 20.0, 8.0]]);
            let generated = in_force_session(&run).expect("editable");
            window.set_support_tool(1);

            // Two clicks in a row: the second waits, each is one undo.
            run.on_tool_click_with([20.2, 20.2, 7.5], false);
            run.on_tool_click_with([19.8, 19.8, 7.5], false);
            assert!(run.pending_click.borrow().is_some());
            assert!(
                Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated),
                "nothing in the scene before it is made"
            );
            settle_click(&run);
            assert!(window.get_supports_last_run().contains("edited by hand"));
            let first = {
                assert!(document.undo().unwrap());
                in_force_session(&run).unwrap()
            };
            assert!(!Arc::ptr_eq(&first, &generated));
            assert!(document.undo().unwrap());
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));

            // Undo while it is made drops it, and undoes nothing else.
            let before = document.scene_revision();
            run.on_tool_click_with([20.2, 20.2, 7.5], false);
            assert!(document.undo().unwrap());
            assert!(run.pending_click.borrow().is_none());
            assert!(window.get_supports_last_run().contains("Undone before"));
            assert_eq!(document.scene_revision(), before);
            std::thread::sleep(Duration::from_millis(50));
            run.poll_click();
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));

            // Another edit first: the click is dropped whole.
            run.on_tool_click_with([20.2, 20.2, 7.5], false);
            document.set_pending_edit(None);
            assert!(document.redo().unwrap(), "the hand edits come back");
            settle_click(&run);
            assert!(
                window.get_supports_last_run().contains("scene changed"),
                "{}",
                window.get_supports_last_run()
            );
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &first));
        });
    }

    /// A face's centre (world mm) and whether the face looks down.
    type FaceCentre = ([f32; 3], bool);

    /// A big real model (`path`, scaled to `height` mm tall when set) on the
    /// plate with its supports generated, as the measurements use it: the
    /// window, the run, and the centre of every face with whether it looks
    /// down.
    fn real_model_run(
        path: &str,
        height: Option<f32>,
    ) -> (MainWindow, Rc<SupportRun>, Vec<FaceCentre>) {
        let bytes = std::fs::read(path).unwrap();
        let (loaded, _) = sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
        let leaf = loaded.roots()[0];
        let mut mesh = (**loaded.leaf_mesh_arc(leaf).unwrap()).clone();
        let bounds = |mesh: &sc_mesh_core::IndexedMesh| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in mesh.vertices() {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.as_array()[k]);
                    hi[k] = hi[k].max(v.as_array()[k]);
                }
            }
            (lo, hi)
        };
        if let Some(h) = height {
            let (lo, hi) = bounds(&mesh);
            let s = h / (hi[2] - lo[2]);
            let vertices = mesh
                .vertices()
                .iter()
                .map(|v| sc_mesh_core::Vertex::new(v.as_array().map(|c| c * s)))
                .collect();
            mesh = sc_mesh_core::IndexedMesh::new(None, vertices, mesh.faces().to_vec());
        }
        let (lo, hi) = bounds(&mesh);
        let shift = [
            60.0 - f64::from(lo[0] + hi[0]) / 2.0,
            40.0 - f64::from(lo[1] + hi[1]) / 2.0,
            5.0 - f64::from(lo[2]),
        ];
        let faces: Vec<([f32; 3], bool)> = mesh
            .faces()
            .iter()
            .filter_map(|f| {
                let [a, b, c] = f
                    .vertices
                    .map(|v| *mesh.vertices()[u32::from(v) as usize].as_array());
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let nz = u[0] * w[1] - u[1] * w[0];
                let len = ((u[1] * w[2] - u[2] * w[1]).powi(2)
                    + (u[2] * w[0] - u[0] * w[2]).powi(2)
                    + nz * nz)
                    .sqrt();
                (len > 1e-12).then(|| {
                    let p: [f32; 3] =
                        std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0 + shift[k] as f32);
                    (p, nz / len < -0.5)
                })
            })
            .collect();
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(None, translation(shift), LeafPayload::Mesh(Arc::new(mesh)))
            .unwrap();
        graph.add_root(leaf).unwrap();
        document.install_graph(graph.clone(), PathBuf::from("model.stl"));
        let run =
            SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let outcome = start_supports(&graph, SupportRequest::default())
            .unwrap()
            .join();
        *run.started_on.borrow_mut() = Some(document.scene_revision());
        run.finish(outcome);
        (window, run, faces)
    }

    /// Measurement, not a gate (#3): what a hand click holds the UI thread
    /// for on a big real scene, now that the engine's part runs on a worker:
    /// handing the click over, then putting the edit in the scene.
    /// `SC_STRESS_STL` is the model, scaled to `SC_STRESS_HEIGHT_MM` tall
    /// when set. `cargo test --profile quick-release -p sc-slicer-gui --lib
    /// -- --ignored --nocapture ui_thread_cost_of_a_click`.
    #[test]
    #[ignore = "needs SC_STRESS_STL"]
    fn ui_thread_cost_of_a_click() {
        let Ok(path) = std::env::var("SC_STRESS_STL") else {
            return;
        };
        let height: Option<f32> = std::env::var("SC_STRESS_HEIGHT_MM")
            .ok()
            .and_then(|h| h.parse().ok());
        on_fresh_slint_thread(move || {
            let (window, run, faces) = real_model_run(&path, height);
            let points: Vec<[f32; 3]> = faces
                .iter()
                .filter(|(_, down)| *down)
                .map(|(p, _)| *p)
                .collect();
            window.set_support_tool(1);
            let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
            let (mut hand_over, mut put_in, mut waits) = (Vec::new(), Vec::new(), Vec::new());
            let step = (points.len() / 10).max(1);
            for p in points.iter().step_by(step).take(10) {
                let t = Instant::now();
                run.on_tool_click_with(*p, false);
                hand_over.push(ms(t));
                if run.pending_click.borrow().is_none() {
                    continue;
                }
                let waited = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_millis(1));
                    let t = Instant::now();
                    run.poll_click();
                    if run.pending_click.borrow().is_none() {
                        put_in.push(ms(t));
                        break;
                    }
                }
                waits.push(ms(waited));
            }
            let stats = |v: &[f64]| {
                let mut v = v.to_vec();
                v.sort_by(f64::total_cmp);
                format!(
                    "median {:.1} ms, max {:.1} ms (n {})",
                    v.get(v.len() / 2).copied().unwrap_or(0.0),
                    v.last().copied().unwrap_or(0.0),
                    v.len()
                )
            };
            println!("{}", window.get_supports_last_run());
            println!("UI thread, the click handed over: {}", stats(&hand_over));
            println!("worker, the edit made: {}", stats(&waits));
            println!("UI thread, the edit put in the scene: {}", stats(&put_in));
        });
    }

    /// What a fast hover sweep showed: how many previews were drawn while
    /// the mouse moved, how far behind the cursor the one drawn was (ms),
    /// and how long the last point took to show once it stopped.
    struct Sweep {
        drawn: usize,
        moves: usize,
        lag_ms: Vec<f64>,
        settle_ms: f64,
    }

    /// Moves the hover over `points` one every `every`, running the event
    /// loop's timers in between as the app does, then waits for the last
    /// point's preview.
    fn sweep(run: &Rc<SupportRun>, points: &[[f32; 3]], every: Duration) -> Sweep {
        let mut asked_at: Vec<Instant> = Vec::new();
        let mut drawn = 0;
        let mut lag_ms = Vec::new();
        let mut shown = run.hover_shown.get();
        let first = run.hover_generation.get() + 1;
        let mut note = |shown_now: u64, asked_at: &[Instant]| {
            if shown_now > shown {
                shown = shown_now;
                drawn += 1;
                if let Some(at) = asked_at.get((shown_now - first) as usize) {
                    lag_ms.push(at.elapsed().as_secs_f64() * 1000.0);
                }
            }
        };
        for p in points {
            asked_at.push(Instant::now());
            run.on_tool_hover_with(Some(*p), false);
            let next = Instant::now() + every;
            while Instant::now() < next {
                slint::platform::update_timers_and_animations();
                note(run.hover_shown.get(), &asked_at);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let during = drawn;
        let stopped = Instant::now();
        while run.hover_shown.get() < run.hover_generation.get() {
            assert!(stopped.elapsed() < Duration::from_secs(60), "no answer");
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(1));
        }
        Sweep {
            drawn: during,
            moves: points.len(),
            lag_ms,
            settle_ms: stopped.elapsed().as_secs_f64() * 1000.0,
        }
    }

    /// A fast mouse (#3): the preview keeps following it while it moves,
    /// instead of waiting for it to stop.
    #[test]
    fn the_preview_follows_a_fast_mouse() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_, run, _) = supported(&window, &[[20.0, 20.0, 8.0]]);
            window.set_support_tool(1);
            // Under the cube, one point every 5 ms for half a second.
            let points: Vec<[f32; 3]> = (0..100)
                .map(|i| {
                    let a = i as f32 * 0.3;
                    [20.0 + 0.3 * a.cos(), 20.0 + 0.3 * a.sin(), 7.5]
                })
                .collect();
            let swept = sweep(&run, &points, Duration::from_millis(5));
            assert!(
                swept.drawn >= 5,
                "{} previews drawn over {} moves",
                swept.drawn,
                swept.moves
            );
        });
    }

    /// Measurement, not a gate (#3): a fast hover sweep over a big real
    /// model. `SC_STRESS_STL` is the model (scaled to `SC_STRESS_HEIGHT_MM`
    /// tall when set). `cargo test --profile quick-release -p sc-slicer-gui
    /// --lib -- --ignored --nocapture a_fast_sweep_over_a_real_model`.
    #[test]
    #[ignore = "needs SC_STRESS_STL"]
    fn a_fast_sweep_over_a_real_model() {
        let Ok(path) = std::env::var("SC_STRESS_STL") else {
            return;
        };
        let height: Option<f32> = std::env::var("SC_STRESS_HEIGHT_MM")
            .ok()
            .and_then(|h| h.parse().ok());
        on_fresh_slint_thread(move || {
            let (window, run, faces) = real_model_run(&path, height);
            let points: Vec<[f32; 3]> = faces.iter().map(|(p, _)| *p).collect();
            // Face centres in turn across the model, as a mouse crossing it.
            let step = (points.len() / 400).max(1);
            let points: Vec<[f32; 3]> = points.iter().step_by(step).take(400).copied().collect();
            window.set_support_tool(1);
            for every in [8, 16] {
                let swept = sweep(&run, &points, Duration::from_millis(every));
                let mut lag = swept.lag_ms.clone();
                lag.sort_by(f64::total_cmp);
                let at = |q: f64| {
                    lag.get(((lag.len() as f64 - 1.0) * q) as usize)
                        .copied()
                        .unwrap_or(f64::NAN)
                };
                println!(
                    "a move every {every} ms: {} previews drawn over {} moves; behind the cursor median {:.0} ms, p90 {:.0} ms; last point shown {:.0} ms after it stopped",
                    swept.drawn,
                    swept.moves,
                    at(0.5),
                    at(0.9),
                    swept.settle_ms
                );
            }
        });
    }

    /// `options` with the pillars' grid off.
    fn without_grid(mut options: SupportOptions) -> SupportOptions {
        let mut zones = options.zone_preset();
        zones.grid_mm = None;
        options.zones = Some(zones);
        options
    }

    /// A slab 20 × 10 × 2 mm, its underside 9 mm up, with the supports a
    /// run gives it, those selected.
    fn supported_slab(window: &MainWindow) -> (Rc<SceneDocument>, Rc<SupportRun>) {
        supported_slab_with(window, |_| {})
    }

    /// [`supported_slab`], its options changed by `change` first.
    fn supported_slab_with(
        window: &MainWindow,
        change: impl FnOnce(&mut SupportOptions),
    ) -> (Rc<SceneDocument>, Rc<SupportRun>) {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let cube = sc_mesh_fixtures::unit_cube_mesh();
        let (lo, hi) =
            cube.vertices()
                .iter()
                .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                    let p = v.as_array();
                    (
                        std::array::from_fn(|k| lo[k].min(p[k])),
                        std::array::from_fn(|k| hi[k].max(p[k])),
                    )
                });
        let size = [20.0, 10.0, 2.0];
        let vertices = cube
            .vertices()
            .iter()
            .map(|v| {
                let p = v.as_array();
                sc_mesh_core::Vertex::new(std::array::from_fn(|k| {
                    (p[k] - lo[k]) / (hi[k] - lo[k]) * size[k]
                }))
            })
            .collect();
        let slab = sc_mesh_core::IndexedMesh::new(None, vertices, cube.faces().to_vec());
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                translation([10.0, 20.0, 9.0]),
                LeafPayload::Mesh(Arc::new(slab)),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        document.install_graph(graph.clone(), PathBuf::from("slab.stl"));
        let run =
            SupportRun::install(window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        // On a solid raft, as these tests were written before the default
        // became a raft of lines (#435).
        // And without the pillars' grid: trunks stand close enough for
        // braces laid by hand.
        // The printer's layer and pixel, as a press of Generate has them.
        let mut request = run.request_for_printer();
        request.options.raft = RaftMode::Solid;
        request.options = without_grid(request.options);
        change(&mut request.options);
        let outcome = start_supports(&graph, request).unwrap().join();
        *run.started_on.borrow_mut() = Some(document.scene_revision());
        run.finish(outcome);
        let (supports, _) = run.support_leaf().expect("the supports");
        document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
            supports,
        )));
        (document, run)
    }

    /// The view from straight above, ten pixels a millimetre.
    fn above(p: [f32; 3]) -> Option<[f32; 2]> {
        Some([p[0] * 10.0, p[1] * 10.0])
    }

    fn nothing_seen(pixels: &[[f32; 2]]) -> Vec<Option<ScreenHit>> {
        vec![None; pixels.len()]
    }

    /// The view from straight above, ten pixels a millimetre, for what
    /// projects alone (a tip moved): its rays meet nothing.
    const FROM_ABOVE: DragView<'static> = DragView {
        project: &above,
        hits: &nothing_seen,
    };

    fn cursor_at(cursor: [f32; 2]) -> DragPoint {
        DragPoint {
            cursor,
            model: None,
            support: None,
        }
    }

    /// Waits for the preview worker's answer to the newest ask, drawn.
    fn wait_preview(run: &Rc<SupportRun>) {
        let started = Instant::now();
        while run.hover_shown.get() < run.hover_generation.get() {
            assert!(started.elapsed() < Duration::from_secs(60), "no answer");
            std::thread::sleep(Duration::from_millis(2));
            run.take_hover_answer();
        }
    }

    /// A tip's place, by axis.
    #[derive(Clone, Copy)]
    struct TipAt {
        x: f64,
        y: f64,
        z: f64,
    }

    /// Move tip (#3): a tip dragged over the model is routed again where it
    /// is let go, as one undo; where it cannot go, it stays.
    #[test]
    fn a_tip_dragged_over_the_model_moves_its_support() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run) = supported_slab(&window);
            let generated = in_force_session(&run).expect("editable");
            // A tip that moves 1.5 mm over, and one others hang from, as
            // the engine itself tells them.
            let shifted = |t: [f64; 3]| [t[0] + 1.5, t[1] + 1.0, 9.0];
            let tips = generated.tips();
            let movable = |(id, t): &&(String, [f64; 3])| {
                generated
                    .preview_move_tip(id, shifted(*t), &AddOptions::default())
                    .is_ok_and(|(p, _)| p.valid)
            };
            let carrying = |(id, t): &&(String, [f64; 3])| {
                generated
                    .preview_move_tip(id, shifted(*t), &AddOptions::default())
                    .is_ok_and(|(p, _)| p.note == Some(AddNote::CarriesOthers))
            };
            let tip_of = |id: &String| {
                let t = tips.iter().find(|(i, _)| i == id).unwrap().1;
                TipAt {
                    x: t[0],
                    y: t[1],
                    z: t[2],
                }
            };
            let free = &tips.iter().find(movable).expect("a tip that moves").0;
            let from = tip_of(free);
            let grab = DragPoint {
                cursor: [0.0, 0.0],
                model: None,
                support: Some([from.x as f32, from.y as f32, from.z as f32 - 0.5]),
            };
            assert!(run.drag_start(SupportDragKind::MoveTip, &grab, &FROM_ABOVE));
            let to = shifted([from.x, from.y, from.z]).map(|c| c as f32);
            let over = |p: [f32; 3]| DragPoint {
                cursor: [0.0, 0.0],
                model: Some(p),
                support: None,
            };
            run.drag_move(&over(to), &FROM_ABOVE);
            wait_preview(&run);
            run.drag_end(true);
            settle_click(&run);
            let moved = in_force_session(&run).unwrap();
            assert!(!Arc::ptr_eq(&moved, &generated));
            let near = |s: &SupportSession, p: [f64; 3]| s.tip_near(p, 0.3).is_some();
            assert!(!near(&moved, [from.x, from.y, from.z]), "gone from there");
            assert!(near(&moved, to.map(f64::from)), "there now");
            assert!(document.undo().unwrap(), "one undo");
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));

            // A trunk others hang from cannot move: red, and it stays.
            {
                let (host, _) = tips.iter().find(carrying).expect("a tip others hang from");
                let from = tip_of(host);
                let grab = DragPoint {
                    support: Some([from.x as f32, from.y as f32, from.z as f32 - 0.3]),
                    ..grab
                };
                assert!(run.drag_start(SupportDragKind::MoveTip, &grab, &FROM_ABOVE));
                run.drag_move(
                    &over([from.x as f32 + 1.0, from.y as f32, 9.0]),
                    &FROM_ABOVE,
                );
                wait_preview(&run);
                assert!(
                    window
                        .get_support_add_note()
                        .contains("Cannot move it there")
                );
                run.drag_end(true);
                assert!(run.pending_click.borrow().is_none());
                assert!(window.get_supports_last_run().contains("Left where it was"));
                assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));
            }
        });
    }

    /// #377: with the Select tool, a picked support's trunk dragged across
    /// the plate moves there as one undo, previewed first; a press off the
    /// picked supports takes no drag.
    #[test]
    fn a_picked_trunk_dragged_across_the_plate_moves() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run) = supported_slab(&window);
            let generated = in_force_session(&run).expect("editable");
            // A trunk that moves 2 mm over, as the engine itself tells it.
            let (id, delta) = generated
                .tips()
                .into_iter()
                .filter(|(id, _)| generated.parts_of(id).is_some_and(|p| p.foot.is_some()))
                .find_map(|(id, _)| {
                    [[-2.0, 0.0], [2.0, 0.0], [0.0, -2.0], [0.0, 2.0]]
                        .into_iter()
                        .find(|d| {
                            generated
                                .preview_move_trunk(&id, *d)
                                .is_ok_and(|(p, _)| p.valid)
                        })
                        .map(|d| (id, d))
                })
                .expect("a trunk that moves");
            let parts = generated.parts_of(&id).unwrap();
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            let press_off = DragPoint {
                cursor: [0.0, 0.0],
                model: None,
                support: Some([-50.0, -50.0, 1.0]),
            };
            assert!(!run.drag_start(SupportDragKind::MovePart, &press_off, &FROM_ABOVE));
            run.on_tool_click_with(parts.tip.map(|c| c as f32), false);
            assert_eq!(window.get_support_picked_count(), 1);
            // On the shaft, just over its root, away from its elbows.
            let grab = [parts.anchor[0], parts.anchor[1], parts.anchor[2] + 0.2];
            let press = DragPoint {
                cursor: [grab[0] as f32 * 10.0, grab[1] as f32 * 10.0],
                model: None,
                support: Some(grab.map(|c| c as f32)),
            };
            assert!(run.drag_start(SupportDragKind::MovePart, &press, &FROM_ABOVE));
            let to = DragPoint {
                cursor: [
                    (grab[0] + delta[0]) as f32 * 10.0,
                    (grab[1] + delta[1]) as f32 * 10.0,
                ],
                model: None,
                support: None,
            };
            run.drag_move(&to, &FROM_ABOVE);
            wait_preview(&run);
            run.drag_end(true);
            settle_click(&run);
            let moved = in_force_session(&run).unwrap();
            let id = moved.tip_near(parts.tip, 0.05).expect("its tip stays");
            let foot = moved.parts_of(&id).unwrap().foot.unwrap();
            let was = parts.foot.unwrap();
            assert!(
                (foot[0] - was[0] - delta[0]).abs() < 0.05,
                "{foot:?} from {was:?}"
            );
            assert!(
                (foot[1] - was[1] - delta[1]).abs() < 0.05,
                "{foot:?} from {was:?}"
            );
            assert!(document.undo().unwrap(), "one undo");
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));
        });
    }

    /// A pointer straight above the world point under `cursor`, with the
    /// camera of [`FROM_ABOVE`] (10 px a millimetre) and a 12 mm gizmo shaft.
    fn pointer_above(cursor: [f64; 2]) -> crate::cut_gizmo::ScenePointer {
        crate::cut_gizmo::ScenePointer {
            origin: [cursor[0] / 10.0, cursor[1] / 10.0, 100.0],
            direction: [0.0, 0.0, -1.0],
            cursor,
            project: Box::new(|p| Some([p[0] * 10.0, p[1] * 10.0])),
            scale_at: Box::new(|_| Some(12.0)),
            ring_radius: 0.85,
            arrow_tip: 1.18,
        }
    }

    /// #377: a gizmo arrow's drag snaps to the millimetre with Ctrl, moves
    /// a tenth as far with Shift, and to the tenth with both.
    #[test]
    fn an_arrow_drag_snaps_and_moves_finely() {
        assert!((arrow_travel(2.345, false, false) - 2.35).abs() < 1e-9);
        assert!((arrow_travel(2.6, true, false) - 3.0).abs() < 1e-9);
        assert!((arrow_travel(2.6, false, true) - 0.26).abs() < 1e-9);
        assert!((arrow_travel(2.6, true, true) - 0.3).abs() < 1e-9);
        let arrow = |axis: usize, knot: bool| ArrowGrab {
            axis,
            dir: WORLD_AXES[axis],
            from: 0.0,
            knot,
        };
        assert_eq!(
            arrow_move([1.0, 2.0, 3.0], None, false, arrow(1, false), 2.0),
            PartMove::Trunk([0.0, 2.0])
        );
        assert_eq!(
            arrow_move([1.0, 2.0, 3.0], None, true, arrow(2, false), 1.0),
            PartMove::Shaft([0.0, 0.0, 1.0])
        );
        assert_eq!(
            arrow_move([1.0, 2.0, 3.0], Some("e"), true, arrow(0, false), 1.0),
            PartMove::Elbow("e".to_owned(), [2.0, 2.0, 3.0])
        );
        assert_eq!(
            arrow_move([1.0, 2.0, 3.0], None, true, arrow(2, true), -1.5),
            PartMove::Knot(1.5)
        );
    }

    /// #377: with one trunk picked the Select tool shows a move gizmo at
    /// its foot, arrows across the plate alone; dragging its X arrow moves
    /// the trunk that far as one undo, and the gizmo goes with more picked.
    #[test]
    fn a_picked_trunk_moves_by_its_gizmo_arrow() {
        on_fresh_slint_thread(|| {
            use crate::cut_gizmo::CutHandles;
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run) = supported_slab(&window);
            let generated = in_force_session(&run).expect("editable");
            let (id, dx) = generated
                .tips()
                .into_iter()
                .filter(|(id, _)| generated.is_trunk(id))
                .find_map(|(id, _)| {
                    [-2.0, 2.0]
                        .into_iter()
                        .find(|d| {
                            generated
                                .preview_move_trunk(&id, [*d, 0.0])
                                .is_ok_and(|(p, _)| p.valid)
                        })
                        .map(|d| (id, d))
                })
                .expect("a trunk that moves");
            let parts = generated.parts_of(&id).unwrap();
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            run.on_tool_click_with(parts.tip.map(|c| c as f32), false);
            assert_eq!(window.get_support_picked_count(), 1);
            let handles = PartHandles(Rc::downgrade(&run));
            assert!(handles.active());
            let (center, tool, _) = handles.gizmo().expect("its gizmo");
            assert_eq!(tool.arrows, [true, true, false], "a trunk moves across");
            assert!((center[0] - parts.anchor[0]).abs() < 1e-9);
            // On its X arrow, then along it.
            let on = [(center[0] + 12.0 * 0.9) * 10.0, center[1] * 10.0];
            assert!(handles.press(&pointer_above(on)));
            handles.drag(&pointer_above([on[0] + dx * 10.0, on[1]]));
            wait_preview(&run);
            let (moved_center, _, _) = handles.gizmo().expect("still there");
            assert!((moved_center[0] - center[0] - dx).abs() < 1e-6, "it follows");
            handles.release(Some(&pointer_above([on[0] + dx * 10.0, on[1]])), true);
            settle_click(&run);
            let moved = in_force_session(&run).unwrap();
            let now = moved.tip_near(parts.tip, 0.05).expect("its tip stays");
            let foot = moved.parts_of(&now).unwrap().foot.unwrap();
            let was = parts.foot.unwrap();
            assert!((foot[0] - was[0] - dx).abs() < 0.02, "{foot:?} from {was:?}");
            assert!((foot[1] - was[1]).abs() < 1e-6);
            assert!(document.undo().unwrap(), "one undo");
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));
            // A press off its handles is not the gizmo's.
            assert!(!handles.press(&pointer_above([-500.0, -500.0])));
        });
    }

    /// #377: a trunk others hang from shows a cube on each knot they hang
    /// from; a press on one picks that support, its gizmo on the knot.
    #[test]
    fn a_knot_cube_picks_the_support_hung_there() {
        on_fresh_slint_thread(|| {
            use crate::cut_gizmo::CutHandles;
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run) = supported_slab(&window);
            let generated = in_force_session(&run).expect("editable");
            let (trunk, hung) = generated
                .tips()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| generated.is_trunk(id))
                .find_map(|id| {
                    let hung = generated.hung_from(&id);
                    (!hung.is_empty()).then_some((id, hung))
                })
                .expect("a trunk something hangs from");
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            run.on_tool_click_with(generated.tip_point(&trunk).unwrap().map(|c| c as f32), false);
            let g = run.part_gizmo().expect("its gizmo");
            assert_eq!(g.hangs.len(), hung.len(), "a cube on each knot");
            let (hanger, knot) = hung[0].clone();
            let cube = g.hangs[0].1;
            assert!(cube[2] > knot[2] + 0.1, "the cube stands up its support, off the knot");
            let handles = PartHandles(Rc::downgrade(&run));
            // From the side: from above the knot stands right over the
            // trunk's elbow.
            let side = crate::cut_gizmo::ScenePointer {
                origin: [cube[0], -100.0, cube[2]],
                direction: [0.0, 1.0, 0.0],
                cursor: [cube[0] * 10.0, -cube[2] * 10.0],
                project: Box::new(|p| Some([p[0] * 10.0, -p[2] * 10.0])),
                scale_at: Box::new(|_| Some(12.0)),
                ring_radius: 0.85,
                arrow_tip: 1.18,
            };
            assert!(handles.press(&side));
            let now = run.chosen_now();
            assert_eq!(now[0].1, vec![hanger.clone()], "the support hung there");
            let g = run.part_gizmo().expect("its gizmo");
            assert!(g.knot && (0..3).all(|k| (g.center[k] - knot[k]).abs() < 1e-9));
        });
    }

    /// #377: a trunk is picked by a click low on its shaft, far from its
    /// tip and elbows; the scene shows it alone; a click on no support lets
    /// it go.
    #[test]
    fn a_trunk_is_picked_low_on_its_shaft_and_let_go_on_a_miss() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run) = supported_slab(&window);
            let generated = in_force_session(&run).expect("editable");
            let (id, foot) = generated
                .tips()
                .into_iter()
                .filter_map(|(id, _)| Some((id.clone(), generated.parts_of(&id)?.foot?)))
                .next()
                .expect("a trunk");
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            let low = [foot[0], foot[1], foot[2] + 1.0];
            run.on_tool_click_with(low.map(|c| c as f32), false);
            let now = run.chosen_now();
            assert_eq!(now.len(), 1);
            assert_eq!(now[0].1, vec![id.clone()], "the trunk under the click");
            let own = generated.own_drawing_of(std::slice::from_ref(&id)).unwrap();
            let shown: usize = run.last_ghost.borrow().iter().map(|m| m.indices.len() / 3).sum();
            assert_eq!(shown, own.faces().len(), "it alone is lit");
            window.invoke_support_tool_missed(false);
            assert_eq!(window.get_support_picked_count(), 0, "a miss lets it go");
        });
    }

    /// #377: a support hung from another shows a gizmo with one arrow, up
    /// its host's shaft, that slides the knot it hangs from.
    #[test]
    fn a_hung_support_slides_its_knot_by_its_gizmo() {
        on_fresh_slint_thread(|| {
            use crate::cut_gizmo::CutHandles;
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (_document, run) = supported_slab(&window);
            let generated = in_force_session(&run).expect("editable");
            let trunks: Vec<String> = generated
                .tips()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| generated.is_trunk(id))
                .collect();
            let (host, joiner) = trunks
                .iter()
                .flat_map(|a| trunks.iter().map(move |b| (a, b)))
                .find(|(a, b)| a != b && generated.as_ref().clone().join_onto(b, a).is_ok())
                .expect("two trunks that join");
            let host_tip = generated.tip_point(host).unwrap();
            let joiner_tip = generated.tip_point(joiner).unwrap();
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            run.on_tool_click_with(host_tip.map(|c| c as f32), false);
            run.on_tool_click_with(joiner_tip.map(|c| c as f32), true);
            run.join_chosen();
            settle_click(&run);
            let joined = in_force_session(&run).unwrap();
            let branch = joined.tip_near(joiner_tip, 1e-6).unwrap();
            let (knot, up) = joined.knot_of(&branch).expect("it hangs from a knot");
            run.on_tool_click_with(joiner_tip.map(|c| c as f32), false);
            let handles = PartHandles(Rc::downgrade(&run));
            let (center, tool, _) = handles.gizmo().expect("its gizmo");
            assert_eq!(tool.arrows, [false, false, true], "one arrow up the host");
            assert!((0..3).all(|k| (center[k] - knot[k]).abs() < 1e-9), "at the knot");
            let g = run.part_gizmo().unwrap();
            assert!(g.knot && (0..3).all(|k| (g.axes[2][k] - up[k]).abs() < 1e-9));
            assert!(
                joined
                    .preview_slide_knot(&branch, knot[2] - 1.0)
                    .is_ok_and(|(p, done)| p.valid && done.is_some()),
                "a slide down the host"
            );
        });
    }

    /// #377: picked supports join onto the trunk of the first picked that
    /// stands on one, their tips where they were, as one undo.
    #[test]
    fn picked_supports_join_onto_the_first_trunk() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run) = supported_slab(&window);
            let generated = in_force_session(&run).expect("editable");
            let trunks: Vec<String> = generated
                .tips()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| generated.is_trunk(id))
                .collect();
            let (host, joiner) = trunks
                .iter()
                .flat_map(|a| trunks.iter().map(move |b| (a, b)))
                .find(|(a, b)| a != b && generated.as_ref().clone().join_onto(b, a).is_ok())
                .expect("two trunks that join");
            let host_tip = generated.tip_point(host).unwrap();
            let joiner_tip = generated.tip_point(joiner).unwrap();
            window.set_support_page(1);
            window.set_support_tool(5);
            run.on_tool_changed();
            run.on_tool_click_with(host_tip.map(|c| c as f32), false);
            run.on_tool_click_with(joiner_tip.map(|c| c as f32), true);
            assert_eq!(window.get_support_picked_count(), 2);
            use crate::cut_gizmo::CutHandles;
            assert!(
                PartHandles(Rc::downgrade(&run)).gizmo().is_none(),
                "two picked: no gizmo"
            );
            run.join_chosen();
            settle_click(&run);
            let joined = in_force_session(&run).unwrap();
            let now = joined.tip_near(joiner_tip, 1e-6).expect("its tip stays");
            assert!(!joined.is_trunk(&now), "it hangs from the first");
            let count = |s: &SupportSession| s.tips().iter().filter(|(id, _)| s.is_trunk(id)).count();
            assert_eq!(count(&joined) + 1, count(&generated));
            assert!(document.undo().unwrap(), "one undo");
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));
        });
    }

    /// Measurement, not a gate (#3): what Remove's box and brush and Move
    /// tip cost on a big real model. `SC_STRESS_STL` is the model (scaled to
    /// `SC_STRESS_HEIGHT_MM` tall when set). `cargo test --profile
    /// quick-release -p sc-slicer-gui --lib -- --ignored --nocapture
    /// drag_costs_on_a_real_model`.
    #[test]
    #[ignore = "needs SC_STRESS_STL"]
    fn drag_costs_on_a_real_model() {
        let Ok(path) = std::env::var("SC_STRESS_STL") else {
            return;
        };
        let height: Option<f32> = std::env::var("SC_STRESS_HEIGHT_MM")
            .ok()
            .and_then(|h| h.parse().ok());
        on_fresh_slint_thread(move || {
            let (window, run, _) = real_model_run(&path, height);
            let document = run.document.clone();
            let (supports, _) = run.support_leaf().expect("the supports");
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                supports,
            )));
            let generated = in_force_session(&run).unwrap();
            let tips = generated.tips();
            let xs: Vec<f64> = tips.iter().map(|(_, t)| t[0]).collect();
            let (lo, hi) = xs
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
            let mid = ((lo + hi) / 2.0) as f32;
            let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
            println!("{} tips", tips.len());

            // A box over half the set, drawn in 20 moves.
            assert!(run.drag_start(SupportDragKind::Box, &cursor_at([-1e4, -1e4]), &FROM_ABOVE));
            let mut ui_ms: f64 = 0.0;
            for k in 1..=20 {
                let x = -1e4 + (mid * 10.0 + 1e4) * k as f32 / 20.0;
                let t = Instant::now();
                run.drag_move(&cursor_at([x, 1e5]), &FROM_ABOVE);
                ui_ms = ui_ms.max(ms(t));
            }
            let t = Instant::now();
            wait_preview(&run);
            let preview_ms = ms(t);
            let picked = match run.drag.borrow().as_ref() {
                Some(DragState::Remove { picked, .. }) => picked.len(),
                _ => 0,
            };
            let t = Instant::now();
            run.drag_end(true);
            let hand_over_ms = ms(t);
            let made = Instant::now();
            let mut land_ms: f64 = 0.0;
            while run.pending_click.borrow().is_some() {
                std::thread::sleep(Duration::from_millis(1));
                let t = Instant::now();
                run.poll_click();
                land_ms = land_ms.max(ms(t));
            }
            println!(
                "box over {picked} tips: UI per move max {ui_ms:.1} ms; red preview {preview_ms:.0} ms after the last move; release {hand_over_ms:.1} ms; made in {:.0} ms; put in the scene {land_ms:.1} ms; {}",
                ms(made),
                window.get_supports_last_run()
            );
            assert!(document.undo().unwrap());

            // A brush across the set, 60 moves.
            assert!(run.drag_start(
                SupportDragKind::Brush,
                &cursor_at([lo as f32 * 10.0, 0.0]),
                &FROM_ABOVE
            ));
            let ys: Vec<f64> = tips.iter().map(|(_, t)| t[1]).collect();
            let y = (ys.iter().sum::<f64>() / ys.len() as f64) as f32 * 10.0;
            let mut ui_ms: f64 = 0.0;
            for k in 0..=60 {
                let x = (lo + (hi - lo) * f64::from(k) / 60.0) as f32 * 10.0;
                let t = Instant::now();
                run.drag_move(&cursor_at([x, y]), &FROM_ABOVE);
                ui_ms = ui_ms.max(ms(t));
            }
            wait_preview(&run);
            run.drag_end(true);
            let made = Instant::now();
            settle_click(&run);
            println!(
                "brush: UI per move max {ui_ms:.1} ms; made in {:.0} ms; {}",
                ms(made),
                window.get_supports_last_run()
            );
            assert!(document.undo().unwrap());

            // Move tip: a tip dragged 2 mm, previews along the way.
            let (id, from) = tips
                .iter()
                .find(|(id, t)| {
                    generated
                        .preview_move_tip(id, [t[0] + 0.3, t[1], t[2]], &AddOptions::default())
                        .is_ok_and(|(p, _)| p.valid)
                })
                .cloned()
                .expect("a tip that moves");
            let grab = DragPoint {
                cursor: [0.0, 0.0],
                model: None,
                support: Some([from[0] as f32, from[1] as f32, from[2] as f32 - 0.3]),
            };
            assert!(run.drag_start(SupportDragKind::MoveTip, &grab, &FROM_ABOVE));
            let mut previews = Vec::new();
            for k in 1..=10 {
                let to = [
                    from[0] as f32 + 0.03 * k as f32,
                    from[1] as f32,
                    from[2] as f32,
                ];
                run.drag_move(
                    &DragPoint {
                        model: Some(to),
                        ..grab
                    },
                    &FROM_ABOVE,
                );
                let t = Instant::now();
                wait_preview(&run);
                previews.push(ms(t));
            }
            previews.sort_by(f64::total_cmp);
            run.drag_end(true);
            let made = Instant::now();
            settle_click(&run);
            println!(
                "move tip {id}: preview median {:.0} ms, max {:.0} ms; made in {:.0} ms; {}",
                previews[previews.len() / 2],
                previews.last().unwrap(),
                ms(made),
                window.get_supports_last_run()
            );
        });
    }

    /// A window driven as the app drives it (#3): its viewport callbacks
    /// reach a controller, and a slab 20 x 10 x 2 mm, its underside 9 mm up
    /// and placed by `transform`, stands with the supports a run gives it,
    /// those selected, in the Supports tab, seen by `camera`.
    fn wired_slab(
        window: &MainWindow,
        transform: Transform3D,
        camera: crate::camera::OrbitCamera,
    ) -> (Rc<ViewportController>, Rc<SceneDocument>, Rc<SupportRun>) {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = wired_viewport(window);
        let document = ctrl.document();
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(slab_mesh())))
            .unwrap();
        graph.add_root(leaf).unwrap();
        document.install_graph(graph.clone(), PathBuf::from("slab.stl"));
        let run =
            SupportRun::install(window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let request = SupportRequest {
            options: without_grid(SupportOptions::default()),
            ..SupportRequest::default()
        };
        let outcome = start_supports(&graph, request).unwrap().join();
        *run.started_on.borrow_mut() = Some(document.scene_revision());
        run.finish(outcome);
        let (supports, _) = run.support_leaf().expect("the supports");
        document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
            supports,
        )));
        ctrl.set_section(WorkspaceSection::Supports);
        ctrl.set_viewport_for_test(800.0, 600.0);
        ctrl.set_camera_for_test(camera);
        (ctrl, document, run)
    }

    /// A viewport controller the window's pointer callbacks drive.
    fn wired_viewport(window: &MainWindow) -> Rc<ViewportController> {
        let ctrl = Rc::new(ViewportController::new_for_test_with_ui(window));
        {
            let ctrl = ctrl.clone();
            window.on_viewport_pointer_down(move |x, y, m| {
                ctrl.on_pointer_down(crate::viewport::ScreenPoint::new(x, y), m)
            });
        }
        {
            let ctrl = ctrl.clone();
            window.on_viewport_pointer_move(move |x, y, m| {
                ctrl.on_pointer_move(crate::viewport::ScreenPoint::new(x, y), m)
            });
        }
        {
            let ctrl = ctrl.clone();
            window.on_viewport_pointer_up(move |x, y, m| {
                ctrl.on_pointer_up(crate::viewport::ScreenPoint::new(x, y), m)
            });
        }
        ctrl
    }

    /// Remove's brush and box over the supports a pre-supported file came
    /// with (#260), through the window's own viewport callbacks: each takes
    /// the bodies in sight it passes over, in one undo step, and leaves the
    /// model as it is.
    #[test]
    fn the_brush_and_the_box_take_bodies_of_imported_supports() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = wired_viewport(&window);
            let document = ctrl.document();
            let (graph, model, set) = crate::tests_presupported_import::separated_scene();
            document.install_graph(graph, PathBuf::from("werewolf.stl"));
            let _run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                model,
            )));
            ctrl.set_section(WorkspaceSection::Supports);
            ctrl.set_viewport_for_test(800.0, 600.0);
            ctrl.set_camera_for_test(camera_at([0.0, 0.0, 3.0], 0.3, 0.2, 70.0));
            window.set_support_tool(2);
            let faces = |leaf: NodeId| {
                document
                    .with_scene(|s| s.graph().leaf_mesh_arc(leaf).map(|m| m.faces().len()))
                    .flatten()
                    .unwrap_or(0)
            };
            let (all, model_faces) = (faces(set), faces(model));
            // Each shaft where it is seen: halfway up, in sight there.
            let on_screen: Vec<[f32; 2]> = crate::tests_presupported_import::SHAFT_SPOTS
                .iter()
                .filter_map(|&[x, y]| {
                    let mid = [x as f32, y as f32, 2.6];
                    let p = ctrl.project_for_test(mid)?;
                    let first = ctrl.first_hit_for_test(p)?;
                    let near = (0..3)
                        .map(|k| (first.point[k] - mid[k]).powi(2))
                        .sum::<f32>()
                        < 1.0;
                    (first.leaf == set
                        && near
                        && (40.0..760.0).contains(&p[0])
                        && (40.0..560.0).contains(&p[1]))
                    .then_some(p)
                })
                .collect();
            assert!(on_screen.len() >= 2, "{} shafts in sight", on_screen.len());

            // The brush: pressed on a shaft, dragged along it.
            window.set_support_remove_mode(1);
            let at = on_screen[0];
            window.invoke_viewport_pointer_down(at[0], at[1], no_keys());
            for k in 1..=6 {
                window.invoke_viewport_pointer_move(at[0], at[1] - 2.0 * k as f32, no_keys());
            }
            window.invoke_viewport_pointer_up(at[0], at[1] - 12.0, no_keys());
            assert!(
                faces(set) < all,
                "the brush took bodies: {}",
                window.get_supports_last_run()
            );
            assert_eq!(faces(model), model_faces, "the model is as it was");
            assert!(document.undo().unwrap(), "one undo");
            assert_eq!(faces(set), all);

            // The box: around another shaft.
            window.set_support_remove_mode(0);
            let at = on_screen[1];
            window.invoke_viewport_pointer_down(at[0] - 8.0, at[1] - 8.0, no_keys());
            for k in 1..=4 {
                let d = 4.0 * k as f32;
                window.invoke_viewport_pointer_move(at[0] - 8.0 + d, at[1] - 8.0 + d, no_keys());
            }
            window.invoke_viewport_pointer_up(at[0] + 8.0, at[1] + 8.0, no_keys());
            assert!(
                faces(set) < all,
                "the box took bodies: {}",
                window.get_supports_last_run()
            );
            assert_eq!(faces(model), model_faces);
            assert!(document.undo().unwrap(), "one undo");
            assert_eq!(faces(set), all);
        });
    }

    /// A model deleted takes its support set along, in the same undo step;
    /// the set by itself goes by itself.
    #[test]
    fn a_model_deleted_takes_its_supports_along() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let (supports, _) = run.support_leaf().expect("the supports");
            let leaves = || {
                document
                    .with_scene(|s| (0..).map_while(|i| s.leaf_id_at_index(i)).count())
                    .unwrap_or(0)
            };
            assert_eq!(leaves(), 2);
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                supports,
            )));
            assert!(document.delete_selected_leaf().unwrap());
            assert_eq!(leaves(), 1, "the set by itself");
            assert!(document.undo().unwrap());
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                leaf,
            )));
            assert!(document.delete_selected_leaf().unwrap());
            assert_eq!(leaves(), 0, "the model took its set along");
            assert!(document.undo().unwrap(), "one undo step");
            assert_eq!(leaves(), 2);
            assert!(in_force_session(&run).is_some(), "editable as it was");
        });
    }

    /// The line tool's hover on a real model (#178), not a gate:
    /// `SC_LINE_STL=<model.stl> cargo test -- --ignored --nocapture
    /// line_hover_on_a_real_model`. Times each hover of a cursor crossing
    /// the model 3 px at a time, after the first click, and what of it is
    /// the rays, the row, the dots.
    #[test]
    #[ignore = "needs SC_LINE_STL"]
    fn line_hover_on_a_real_model() {
        use crate::picking;
        let Ok(path) = std::env::var("SC_LINE_STL") else {
            return;
        };
        let log = on_fresh_slint_thread(move || {
            let mut log = Vec::new();
            let bytes = std::fs::read(&path).unwrap();
            let (loaded, _) =
                sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
            let leaf = loaded
                .roots()
                .iter()
                .copied()
                .find(|id| loaded.leaf_mesh_arc(*id).is_some())
                .unwrap();
            let mesh = loaded.leaf_mesh_arc(leaf).unwrap().clone();
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in mesh.vertices() {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.as_array()[k]);
                    hi[k] = hi[k].max(v.as_array()[k]);
                }
            }
            let shift = [
                60.0 - f64::from(lo[0] + hi[0]) / 2.0,
                40.0 - f64::from(lo[1] + hi[1]) / 2.0,
                5.0 - f64::from(lo[2]),
            ];
            log.push(format!("{} faces", mesh.faces().len()));
            // How tight the picker's runs of 64 faces are: a mesh whose
            // faces come in no spatial order makes wide boxes, and slow rays.
            if let Some(clusters) = sc_mesh_spatial::TriangleClusters::for_mesh(&mesh) {
                let diagonal = |lo: [f32; 3], hi: [f32; 3]| {
                    (0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f32>().sqrt()
                };
                let mean = clusters
                    .iter()
                    .map(|(_, b)| diagonal(b.min().coords(), b.max().coords()))
                    .sum::<f32>()
                    / clusters.len() as f32;
                log.push(format!(
                    "{} clusters, mean box diagonal {:.2} mm of the model's {:.1} mm",
                    clusters.len(),
                    mean,
                    diagonal(lo, hi)
                ));
            }
            let window = MainWindow::new().unwrap();
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = wired_viewport(&window);
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation(shift)),
                    LeafPayload::Mesh(mesh),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("model.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let t = Instant::now();
            let outcome = start_supports(&graph, SupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            log.push(format!(
                "generated in {} ms: {}",
                t.elapsed().as_millis(),
                window.get_supports_last_run()
            ));
            let (w, h) = (1600.0, 1000.0);
            let size = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f32::max);
            let center = [60.0, 40.0, 5.0 + (hi[2] - lo[2]) / 2.0];
            let camera = camera_at(center, 0.4, -0.35, size * 1.6);
            ctrl.set_section(WorkspaceSection::Supports);
            ctrl.set_viewport_for_test(w, h);
            ctrl.set_camera_for_test(camera.clone());
            window.set_support_tool(6);
            run.on_tool_changed();
            let fov = crate::mesh_render::default_fov_y();
            let hit_at = |x: f32, y: f32| -> Option<[f32; 3]> {
                let ray = picking::ray_from_pixel(&camera, fov, w, h, x, y);
                document
                    .with_scene(|spatial| {
                        let hit = picking::pick_face_where(&ray, spatial, |id| {
                            !spatial
                                .graph()
                                .leaf_mesh_arc(id)
                                .is_some_and(|m| sc_geom::supports::is_any_support_mesh(m))
                        })?;
                        let p = ray.origin + ray.direction * hit.t;
                        Some([p.x, p.y, p.z])
                    })
                    .flatten()
            };
            // A diagonal across the screen: the first hit is where the row
            // starts, every hit after it 3 px on is a hover.
            let cursor: Vec<(f32, f32)> = (0..)
                .map(|i| (200.0 + i as f32 * 3.0, 150.0 + i as f32 * 2.1))
                .take_while(|&(x, y)| x < w - 200.0 && y < h - 150.0)
                .collect();
            let hits: Vec<[f32; 3]> = cursor.iter().filter_map(|&(x, y)| hit_at(x, y)).collect();
            assert!(
                hits.len() > 20,
                "the cursor crosses the model: {}",
                hits.len()
            );
            let from = hits[0];
            run.on_tool_click_with(from, false);
            let (mut rays, mut row_ms, mut dots_ms, mut hover) = (0.0, 0.0, 0.0, 0.0);
            let (mut rays_cast, mut one_by_one) = (0usize, 0.0);
            let mut slowest: Vec<(f64, f64, usize)> = Vec::new();
            let spacing = run.line_spacing.get();
            let hovers = &hits[1..];
            for &p in hovers {
                let t = Instant::now();
                let path = ctrl.surface_path(from, p);
                let a = t.elapsed().as_secs_f64() * 1e3;
                rays += a;
                rays_cast += path.len();
                let t = Instant::now();
                let before = surface_path_one_by_one(&ctrl, from, p);
                one_by_one += t.elapsed().as_secs_f64() * 1e3;
                assert_eq!(path, before, "the same points as one by one");
                let t = Instant::now();
                let row = line_points(&path, spacing);
                row_ms += t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                let dots = line_dots(&row);
                dots_ms += t.elapsed().as_secs_f64() * 1e3;
                drop(dots);
                let t = Instant::now();
                run.on_tool_hover_with(Some(p), false);
                let whole = t.elapsed().as_secs_f64() * 1e3;
                hover += whole;
                slowest.push((whole, a, path.len()));
            }
            let n = hovers.len() as f64;
            slowest.sort_by(|x, y| y.0.total_cmp(&x.0));
            log.push(format!(
                "{} hovers on a {w}x{h} view: whole hover {:.2} ms mean; rays {:.2} ms \
                 ({:.0} points; {:.2} ms one by one), row {:.3} ms, dots {:.3} ms",
                hovers.len(),
                hover / n,
                rays / n,
                rays_cast as f64 / n,
                one_by_one / n,
                row_ms / n,
                dots_ms / n,
            ));
            for (whole, rays, points) in slowest.iter().take(3) {
                log.push(format!(
                    "  slow: hover {whole:.2} ms, its rays {rays:.2} ms, {points} points"
                ));
            }
            log
        });
        for line in log {
            println!("{line}");
        }
    }

    /// The line tool (#178): dots evenly along each stretch of the path,
    /// a new stretch where the path left the model.
    #[test]
    fn a_row_spreads_evenly_and_starts_again_past_a_gap() {
        let path: Vec<[f32; 3]> = (0..=40).map(|i| [i as f32 * 0.25, 0.0, 5.0]).collect();
        let row = line_points(&path, 2.0);
        assert_eq!(row.len(), 6);
        assert_eq!(row[0], [0.0, 0.0, 5.0]);
        assert_eq!(row[5], [10.0, 0.0, 5.0]);
        assert!((row[1][0] - 2.0).abs() < 1e-4);
        let mut gapped = path.clone();
        gapped.extend((0..=8).map(|i| [30.0 + i as f32 * 0.25, 0.0, 5.0]));
        let row = line_points(&gapped, 2.0);
        assert_eq!(row.len(), 6 + 2);
        assert_eq!(*row.last().unwrap(), [32.0, 0.0, 5.0]);
        assert_eq!(line_points(&[[1.0, 2.0, 3.0]], 2.0), vec![[1.0, 2.0, 3.0]]);
    }

    /// The line tool's path as its rays were cast one after the other, on
    /// the UI thread, before they ran in parallel (#178): what the row
    /// placed is measured against.
    fn surface_path_one_by_one(
        ctrl: &ViewportController,
        from: [f32; 3],
        to: [f32; 3],
    ) -> Vec<[f32; 3]> {
        use crate::picking;
        let (w, h) = ctrl.viewport_for_test().expect("a viewport");
        let camera = ctrl.camera_for_test().expect("a camera");
        let fov = crate::mesh_render::default_fov_y();
        let screen =
            |p: [f32; 3]| picking::project_world_to_screen(&camera, fov, w, h, glam::Vec3::from(p));
        let (Some(a), Some(b)) = (screen(from), screen(to)) else {
            return vec![from, to];
        };
        let steps = (((b.0 - a.0).hypot(b.1 - a.1) / 1.5).ceil() as usize).clamp(1, 2000);
        let mut out = vec![from];
        ctrl.document().with_scene(|spatial| {
            for i in 1..steps {
                let t = i as f32 / steps as f32;
                let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let ray = picking::ray_from_pixel(&camera, fov, w, h, x, y);
                let keep = |id| {
                    !spatial
                        .graph()
                        .leaf_mesh_arc(id)
                        .is_some_and(|m| sc_geom::supports::is_any_support_mesh(m))
                };
                if let Some(hit) = picking::pick_face_where(&ray, spatial, keep) {
                    let p = ray.origin + ray.direction * hit.t;
                    out.push([p.x, p.y, p.z]);
                }
            }
        });
        out.push(to);
        out
    }

    /// The line tool's rays run in parallel (#178) and find the very points
    /// they found one after the other, gap and all: the row the hover shows
    /// and the click places is the same as before, to the bit.
    #[test]
    fn the_line_path_in_parallel_is_the_path_one_by_one() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = wired_viewport(&window);
            let document = ctrl.document();
            let mut graph = Scene::new();
            for x in [10.0, 35.0] {
                let leaf = graph
                    .add_leaf(
                        None,
                        translation([x, 20.0, 9.0]),
                        LeafPayload::Mesh(Arc::new(slab_mesh())),
                    )
                    .unwrap();
                graph.add_root(leaf).unwrap();
            }
            document.install_graph(graph, PathBuf::from("slabs.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            ctrl.set_section(WorkspaceSection::Supports);
            ctrl.set_viewport_for_test(1600.0, 1000.0);
            ctrl.set_camera_for_test(camera_at([32.0, 25.0, 9.0], 0.3, -0.7, 60.0));
            let (from, to) = ([12.0, 25.0, 9.0], [52.0, 25.0, 9.0]);
            let before = surface_path_one_by_one(&ctrl, from, to);
            assert!(before.len() > 300, "{} points", before.len());
            assert_eq!(ctrl.surface_path(from, to), before);
            let gaps = before
                .windows(2)
                .filter(|w| (w[1][0] - w[0][0]).abs() > 2.0)
                .count();
            assert_eq!(gaps, 1, "the path leaves the slabs once");
            // The hover counts the row the click places, from that path.
            let row = line_points(&before, run.line_spacing.get());
            window.set_support_tool(6);
            run.on_tool_changed();
            run.on_tool_click_with(from, false);
            run.on_tool_hover_with(Some(to), false);
            assert!(
                window
                    .get_support_add_note()
                    .contains(&format!("{} supports", row.len())),
                "{} for {} dots",
                window.get_support_add_note(),
                row.len()
            );
            assert_eq!(run.line_row(from, to), row);
        });
    }

    /// The line tool (#178): two clicks under a bare slab, seen from below,
    /// place a row of supports along it, as one Undo step.
    #[test]
    fn two_clicks_of_the_line_tool_place_a_row_in_one_step() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = wired_viewport(&window);
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    translation([10.0, 20.0, 9.0]),
                    LeafPayload::Mesh(Arc::new(slab_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("slab.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            ctrl.set_section(WorkspaceSection::Supports);
            ctrl.set_viewport_for_test(800.0, 600.0);
            ctrl.set_camera_for_test(camera_at([20.0, 25.0, 9.0], 0.3, -0.7, 50.0));
            window.set_support_tool(6);
            run.on_tool_changed();
            // A hand size with a fat trunk, picked: the row still takes the
            // preset's sizes.
            for size in &mut run.hand.borrow_mut().rules.sizes {
                size.trunk_diameter_mm = 3.26;
            }
            window.set_support_tip_size(2);

            run.on_tool_click_with([12.0, 25.0, 9.0], false);
            assert!(
                run.support_leaf().is_none(),
                "the first click places nothing"
            );
            run.on_tool_hover_with(Some([28.0, 25.0, 9.0]), false);
            assert!(
                window.get_support_add_note().contains("9 supports"),
                "{}",
                window.get_support_add_note()
            );
            run.on_tool_click_with([28.0, 25.0, 9.0], false);
            settle_click(&run);
            let session = in_force_session(&run).expect("the row started a set");
            let tips: Vec<[f64; 3]> = session.tips().into_iter().map(|t| t.1).collect();
            assert_eq!(tips.len(), 9, "{tips:?}");
            for (id, _) in session.tips() {
                let shape = session.shape_of(&id).expect("each support has a shape");
                assert!(
                    shape.trunk_diameter_mm < 3.0,
                    "{id} took the hand size: {shape:?}"
                );
            }
            for tip in &tips {
                assert!((tip[1] - 25.0).abs() < 0.3, "on the line: {tip:?}");
                assert!((tip[2] - 9.0).abs() < 0.3, "on the underside: {tip:?}");
            }
            // Nothing of it below the plate: the low end of a slanted brace
            // stands on it.
            let (support, _) = run.support_leaf().expect("the row's supports");
            let lowest = world_mesh(&document, support)
                .vertices()
                .iter()
                .map(|v| v.as_array()[2])
                .fold(f32::INFINITY, f32::min);
            assert!(lowest > -1e-4, "the row's supports reach below the plate: {lowest}");
            assert!(document.undo().unwrap());
            assert!(run.support_leaf().is_none(), "one undo takes the row out");
        });
    }

    /// The Area tool (#177): the underside of a bare slab painted, one
    /// action places a support on every node of a 2 mm grid over it, as one
    /// Undo step; the paint stays until cleared.
    #[test]
    fn the_paint_of_the_area_tool_fills_with_a_grid_in_one_step() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = wired_viewport(&window);
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    translation([10.0, 20.0, 9.0]),
                    LeafPayload::Mesh(Arc::new(slab_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("slab.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            ctrl.set_section(WorkspaceSection::Supports);
            window.set_support_tool(7);
            run.on_tool_changed();
            assert!(run.area_brush.armed());

            run.fill_painted();
            assert!(
                run.support_leaf().is_none(),
                "nothing painted, nothing placed"
            );
            let bottom: Vec<u32> = slab_mesh()
                .faces()
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.vertices
                        .iter()
                        .all(|v| slab_mesh().vertices()[v.get() as usize].as_array()[2] == 0.0)
                })
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(bottom.len(), 2);
            let edit = SceneEdit::PaintSurface {
                node_id: leaf,
                kind: sc_scene::PaintKind::SupportEnforce,
                changes: sc_scene::PaintChanges::from_faces(
                    bottom
                        .iter()
                        .map(|&f| (f, sc_scene::UNPAINTED, sc_scene::PAINTED))
                        .collect(),
                ),
            };
            document.apply_edit(edit.clone()).unwrap();
            document.push_edit(edit);

            run.fill_painted();
            settle_click(&run);
            let session = in_force_session(&run).expect("the grid started a set");
            let tips: Vec<[f64; 3]> = session.tips().into_iter().map(|t| t.1).collect();
            assert!(tips.len() >= 40, "{} supports", tips.len());
            for tip in &tips {
                assert!((tip[2] - 9.0).abs() < 0.3, "on the underside: {tip:?}");
            }
            assert_eq!(crate::support_area::painted_leaves(&document), vec![leaf]);
            assert!(document.undo().unwrap());
            assert!(run.support_leaf().is_none(), "one undo takes the grid out");
            run.clear_painted();
            assert!(crate::support_area::painted_leaves(&document).is_empty());
        });
    }

    /// A slab 20 x 10 x 2 mm, its corner at the origin.
    fn slab_mesh() -> sc_mesh_core::IndexedMesh {
        let cube = sc_mesh_fixtures::unit_cube_mesh();
        let (lo, hi) =
            cube.vertices()
                .iter()
                .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                    let p = v.as_array();
                    (
                        std::array::from_fn(|k| lo[k].min(p[k])),
                        std::array::from_fn(|k| hi[k].max(p[k])),
                    )
                });
        let size = [20.0, 10.0, 2.0];
        let vertices = cube
            .vertices()
            .iter()
            .map(|v| {
                let p = v.as_array();
                sc_mesh_core::Vertex::new(std::array::from_fn(|k| {
                    (p[k] - lo[k]) / (hi[k] - lo[k]) * size[k]
                }))
            })
            .collect();
        sc_mesh_core::IndexedMesh::new(None, vertices, cube.faces().to_vec())
    }

    /// The camera looking at `target` from `distance` mm, from below the
    /// plate at `pitch` radians.
    fn camera_at(
        target: [f32; 3],
        yaw: f32,
        pitch: f32,
        distance: f32,
    ) -> crate::camera::OrbitCamera {
        let mut camera = crate::camera::OrbitCamera::new(glam::Vec3::from(target), distance);
        camera.yaw = yaw;
        camera.pitch = pitch;
        camera.distance = distance;
        camera
    }

    fn no_keys() -> crate::PointerModifiers {
        crate::PointerModifiers::default()
    }

    /// Remove's brush and box through the window's own viewport callbacks
    /// (#3): a press on a tip, moves past the click threshold, a release.
    #[test]
    fn the_brush_and_the_box_remove_through_the_viewport() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let camera = camera_at([20.0, 25.0, 9.0], 0.6, -0.5, 40.0);
            let (ctrl, document, run) = wired_slab(&window, translation([10.0, 20.0, 9.0]), camera);
            let generated = in_force_session(&run).expect("editable");
            window.set_support_tool(2);
            // Each support where it is seen: halfway down its shaft, not at
            // its tip, which the model mostly hides.
            let on_screen: Vec<(String, [f32; 2])> = along_supports(generated.support_paths())
                .into_iter()
                .filter_map(|(id, along)| {
                    // Halfway along it, in sight there, as a person sees it
                    // to paint it.
                    let mid = *along.get(along.len() / 2)?;
                    let p = ctrl.project_for_test(mid)?;
                    let first = ctrl.first_hit_for_test(p)?;
                    if (0..3)
                        .map(|k| (first.point[k] - mid[k]).powi(2))
                        .sum::<f32>()
                        >= 1.0
                    {
                        return None;
                    }
                    ((20.0..780.0).contains(&p[0]) && (20.0..580.0).contains(&p[1]))
                        .then_some((id, p))
                })
                .collect();
            assert!(on_screen.len() > 3, "{} tips on screen", on_screen.len());

            // The brush: pressed on a shaft, dragged 30 px.
            window.set_support_remove_mode(1);
            let (id, at) = on_screen[0].clone();
            window.invoke_viewport_pointer_down(at[0], at[1], no_keys());
            for k in 1..=6 {
                window.invoke_viewport_pointer_move(at[0] + 5.0 * k as f32, at[1], no_keys());
            }
            window.invoke_viewport_pointer_up(at[0] + 30.0, at[1], no_keys());
            settle_click(&run);
            let brushed = in_force_session(&run).unwrap();
            assert!(
                !brushed.tips().iter().any(|(left, _)| *left == id),
                "the support brushed is gone: {}",
                window.get_supports_last_run()
            );
            assert!(document.undo().unwrap(), "one undo");
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));

            // The box: around a shaft.
            window.set_support_remove_mode(0);
            let (id, at) = on_screen[1].clone();
            window.invoke_viewport_pointer_down(at[0] - 15.0, at[1] - 15.0, no_keys());
            for k in 1..=6 {
                let d = 5.0 * k as f32;
                window.invoke_viewport_pointer_move(at[0] - 15.0 + d, at[1] - 15.0 + d, no_keys());
            }
            window.invoke_viewport_pointer_up(at[0] + 15.0, at[1] + 15.0, no_keys());
            settle_click(&run);
            let boxed = in_force_session(&run).unwrap();
            assert!(
                !boxed.tips().iter().any(|(left, _)| *left == id),
                "the support boxed is gone: {}",
                window.get_supports_last_run()
            );
            assert!(document.undo().unwrap());
        });
    }

    /// Where a ray through pixel `at` of the 800 x 600 viewport first meets
    /// the plane through `p0` with normal `n` (world mm).
    fn ray_meets_plane(
        camera: &crate::camera::OrbitCamera,
        at: [f32; 2],
        p0: glam::Vec3,
        n: glam::Vec3,
    ) -> [f32; 3] {
        let ray = crate::picking::ray_from_pixel(
            camera,
            crate::mesh_render::default_fov_y(),
            800.0,
            600.0,
            at[0],
            at[1],
        );
        let t = (p0 - ray.origin).dot(n) / ray.direction.dot(n);
        (ray.origin + ray.direction * t).into()
    }

    fn far(a: [f32; 3], b: [f64; 3]) -> f64 {
        (0..3)
            .map(|k| (f64::from(a[k]) - b[k]).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    /// The tip `after` holds that `before` does not.
    fn tip_added(before: &SupportSession, after: &SupportSession) -> Option<[f64; 3]> {
        let old: Vec<String> = before.tips().into_iter().map(|(id, _)| id).collect();
        after
            .tips()
            .into_iter()
            .find(|(id, _)| !old.contains(id))
            .map(|(_, t)| t)
    }

    /// A tip placed by hand lands on the model surface under the cursor
    /// (#3), to the pixel: the ray through the pixel clicked meets the model
    /// there first. For Add and for Move tip, on a model turned and tilted,
    /// through the window's own viewport callbacks.
    #[test]
    fn a_tip_lands_on_the_surface_under_the_cursor() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            // The slab turned 0.5 rad about Z and tilted 0.15 rad about X,
            // its underside corner 12 mm up.
            let place = AffineTransform3D::from_translation([25.0, 22.0, 12.0])
                .compose(&AffineTransform3D::from_rotation_z(0.5))
                .compose(&AffineTransform3D::from_rotation_x(0.15));
            let local = |p: [f64; 3]| {
                let v = place.apply_to_vertex(sc_mesh_core::Vertex::new(p.map(|c| c as f32)));
                glam::Vec3::from(*v.as_array())
            };
            let p0 = local([0.0, 0.0, 0.0]);
            let n = (local([0.0, 0.0, -1.0]) - p0).normalize();
            let centre = local([10.0, 5.0, 0.0]);
            let camera = camera_at(centre.into(), 0.7, -0.6, 45.0);
            assert!(
                camera.eye().z < centre.z,
                "the camera looks up at the underside"
            );
            let (ctrl, document, run) =
                wired_slab(&window, Transform3D::Affine(place), camera.clone());
            // A point of the underside on screen, to the whole pixel.
            let pixel_of = |p: [f64; 3]| {
                let at = ctrl.project_for_test(local(p).into()).expect("on screen");
                [at[0].round(), at[1].round()]
            };

            // Add, at two pixels.
            window.set_support_tool(1);
            for p in [[6.3, 3.1, 0.0], [14.6, 7.2, 0.0]] {
                let at = pixel_of(p);
                let aimed = ray_meets_plane(&camera, at, p0, n);
                let hit = ctrl
                    .model_hit_for_test(crate::viewport::ScreenPoint::new(at[0], at[1]))
                    .expect("the model under the cursor");
                assert!(
                    far(hit, aimed.map(f64::from)) < 0.01,
                    "picked {hit:?}, ray {aimed:?}"
                );
                let before = in_force_session(&run).unwrap();
                window.invoke_viewport_pointer_down(at[0], at[1], no_keys());
                window.invoke_viewport_pointer_up(at[0], at[1], no_keys());
                settle_click(&run);
                let after = in_force_session(&run).unwrap();
                let tip = tip_added(&before, &after)
                    .unwrap_or_else(|| panic!("nothing added: {}", window.get_supports_last_run()));
                assert!(far(aimed, tip) < 0.02, "tip {tip:?}, aimed {aimed:?}");
            }

            // Move tip: a support added at the edge of the slab nearest the
            // camera, where none of the run stands in front of it; it carries
            // no other. Grabbed by its shaft just below its tip, dropped on
            // another pixel.
            let eye = camera.eye();
            let mut edge: Vec<[f64; 3]> = (1..20)
                .flat_map(|k| [[f64::from(k), 0.6, 0.0], [f64::from(k), 9.4, 0.0]])
                .chain((1..10).flat_map(|k| [[0.6, f64::from(k), 0.0], [19.4, f64::from(k), 0.0]]))
                .collect();
            edge.sort_by(|a, b| {
                (local(*a) - eye)
                    .length()
                    .total_cmp(&(local(*b) - eye).length())
            });
            let (session, id, grab) = edge
                .into_iter()
                .find_map(|p| {
                    let at = pixel_of(p);
                    let before = in_force_session(&run).unwrap();
                    window.invoke_viewport_pointer_down(at[0], at[1], no_keys());
                    window.invoke_viewport_pointer_up(at[0], at[1], no_keys());
                    settle_click(&run);
                    let after = in_force_session(&run).unwrap();
                    if Arc::ptr_eq(&before, &after) {
                        return None;
                    }
                    let old: Vec<String> = before.tips().into_iter().map(|(id, _)| id).collect();
                    let grabbed = after
                        .support_paths()
                        .into_iter()
                        .filter(|(id, path)| path.len() >= 2 && !old.contains(id))
                        .find_map(|(id, path)| {
                            let (tip, next) = (path[0], path[1]);
                            let len = (0..3)
                                .map(|k| (next[k] - tip[k]).powi(2))
                                .sum::<f64>()
                                .sqrt();
                            let below: [f64; 3] = std::array::from_fn(|k| {
                                tip[k] + (next[k] - tip[k]) * (0.8 / len.max(0.8))
                            });
                            let at = ctrl.project_for_test(below.map(|c| c as f32))?;
                            let on = ctrl.support_hit_for_test(
                                crate::viewport::ScreenPoint::new(at[0], at[1]),
                            )?;
                            (far(on, below) < 0.5).then_some((id, at))
                        });
                    match grabbed {
                        Some((id, at)) => Some((after, id, at)),
                        None => {
                            assert!(document.undo().unwrap());
                            None
                        }
                    }
                })
                .expect("a tip to grab on screen");
            window.set_support_tool(3);
            let to = pixel_of([10.2, 2.4, 0.0]);
            let aimed = ray_meets_plane(&camera, to, p0, n);
            window.invoke_viewport_pointer_down(grab[0], grab[1], no_keys());
            for k in 1..=8 {
                let t = k as f32 / 8.0;
                window.invoke_viewport_pointer_move(
                    grab[0] + (to[0] - grab[0]) * t,
                    grab[1] + (to[1] - grab[1]) * t,
                    no_keys(),
                );
            }
            window.invoke_viewport_pointer_up(to[0], to[1], no_keys());
            settle_click(&run);
            let moved = in_force_session(&run).unwrap();
            assert!(
                !moved.tips().iter().any(|(left, _)| *left == id),
                "the support grabbed moved: {}",
                window.get_supports_last_run()
            );
            let tip = tip_added(&session, &moved).expect("its new tip");
            assert!(far(aimed, tip) < 0.02, "tip {tip:?}, aimed {aimed:?}");
            assert!(document.undo().unwrap(), "one undo");
            assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &session));
        });
    }

    /// #481: from under the plate the raft fades out, and Remove sees past it
    /// to the supports above, where the plain first hit would be the raft.
    #[test]
    fn from_under_the_plate_remove_sees_past_the_raft() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let place = AffineTransform3D::from_translation([25.0, 22.0, 12.0]);
            let camera = camera_at([35.0, 27.0, 12.0], 0.4, -1.2, 60.0);
            assert!(crate::under_plate::sees_through_the_plate(&camera));
            let (ctrl, document, run) =
                wired_slab(&window, Transform3D::Affine(place), camera.clone());
            let session = in_force_session(&run).unwrap();
            let top = session.raft_top_mm();
            assert!(top > 0.0, "the default run lays a raft");

            // A point of a shaft well above the raft whose pixel's ray meets
            // the raft first.
            let raft_first = |at: [f32; 2]| {
                let ray = crate::picking::ray_from_pixel(
                    &camera,
                    crate::mesh_render::default_fov_y(),
                    800.0,
                    600.0,
                    at[0],
                    at[1],
                );
                document
                    .with_scene(|spatial| crate::picking::pick_face_where(&ray, spatial, |_| true))
                    .flatten()
                    .is_some_and(|hit| (ray.origin + ray.direction * hit.t).z <= top as f32 + 1e-3)
            };
            let (at, aimed) = session
                .support_paths()
                .iter()
                .flat_map(|(_, path)| path.iter().copied())
                .filter(|p| p[2] > top + 4.0 && p[2] < 10.0)
                .find_map(|p| {
                    let at = ctrl.project_for_test(p.map(|c| c as f32))?;
                    // Past the raft, this shaft and not the foot of one
                    // nearer the camera.
                    let seen =
                        ctrl.support_hit_for_test(crate::viewport::ScreenPoint::new(at[0], at[1]))?;
                    (raft_first(at) && far(seen, p) < 2.0).then_some((at, p))
                })
                .expect("a shaft seen through the raft");

            let hit = ctrl
                .support_hit_for_test(crate::viewport::ScreenPoint::new(at[0], at[1]))
                .expect("a support under the cursor");
            assert!(
                f64::from(hit[2]) > top + 1.0,
                "past the raft: {hit:?}, aimed {aimed:?}"
            );
            assert!(
                far(hit, aimed) < 2.0,
                "on the shaft: {hit:?}, aimed {aimed:?}"
            );
            assert!((ctrl.raft_top_under_plate_for_test() - top as f32).abs() < 1e-3);

            // With the Supports tab's box off (#492) the raft stays drawn and
            // the tool stops on it; back on, it sees past it again.
            assert!(window.get_support_hide_rafts_under_plate());
            window.invoke_support_hide_rafts_under_plate_toggled(false);
            assert!(!window.get_support_hide_rafts_under_plate());
            assert_eq!(ctrl.raft_top_under_plate_for_test(), 0.0);
            let on_raft = ctrl
                .support_hit_for_test(crate::viewport::ScreenPoint::new(at[0], at[1]))
                .expect("the raft under the cursor");
            assert!(
                f64::from(on_raft[2]) <= top + 1e-3,
                "on the raft: {on_raft:?}"
            );
            window.invoke_support_hide_rafts_under_plate_toggled(true);
            let again = ctrl
                .support_hit_for_test(crate::viewport::ScreenPoint::new(at[0], at[1]))
                .expect("a support under the cursor");
            assert!(far(again, aimed) < 2.0, "past the raft again: {again:?}");
        });
    }

    /// #493: with the Supports tab's box on, from under the plate the
    /// supports fade out and the viewport draws their lines, a ball at every
    /// tip; the rafts with them, or kept when their own box is off. From above
    /// the plate, or with the box off, the supports stay as they are.
    #[test]
    fn from_under_the_plate_the_supports_turn_into_lines() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let place = AffineTransform3D::from_translation([25.0, 22.0, 12.0]);
            let camera = camera_at([35.0, 27.0, 12.0], 0.4, -1.2, 60.0);
            let (ctrl, _document, run) =
                wired_slab(&window, Transform3D::Affine(place), camera.clone());
            let session = in_force_session(&run).unwrap();
            let top = session.raft_top_mm() as f32;
            let tips: usize = session.pieces().iter().map(|p| p.contacts.len()).sum();
            assert!(tips > 0);

            assert!(!window.get_support_lines_under_plate(), "off by default");
            let (kept, none) = ctrl.under_plate_supports_for_test();
            assert_eq!(kept, f32::MAX);
            assert!(none.is_none());

            window.invoke_support_lines_under_plate_toggled(true);
            assert!(window.get_support_lines_under_plate());
            let (faded, lines) = ctrl.under_plate_supports_for_test();
            assert!(faded < 0.0, "the rafts fade too: {faded}");
            let lines = lines.expect("the lines");
            assert!(!lines.dashes.is_empty());
            // One ball for each tip, each of the same vertices.
            assert_eq!(lines.balls.positions.len() % tips, 0);
            assert!(!lines.balls.positions.is_empty());
            // Gathered once while the sets stay.
            let (_, again) = ctrl.under_plate_supports_for_test();
            assert!(Arc::ptr_eq(&lines, &again.unwrap()));

            // The rafts kept: the supports fade above them only.
            window.invoke_support_hide_rafts_under_plate_toggled(false);
            let (above_raft, _) = ctrl.under_plate_supports_for_test();
            assert_eq!(above_raft, crate::under_plate::hidden_below_mm(top));
            window.invoke_support_hide_rafts_under_plate_toggled(true);

            // From above the plate nothing changes.
            let mut over = camera.clone();
            over.pitch = 0.6;
            ctrl.set_camera_for_test(over);
            assert_eq!(ctrl.under_plate_supports_for_test().0, f32::MAX);
            ctrl.set_camera_for_test(camera);
            window.invoke_support_lines_under_plate_toggled(false);
            let (off, none) = ctrl.under_plate_supports_for_test();
            assert_eq!(off, f32::MAX);
            assert!(none.is_none());
        });
    }

    /// #137: the view mode in Scene settings draws the supports as their
    /// contacts and lines, or see-through, at any camera height; the raft
    /// stays. Whole draws them as they print.
    #[test]
    fn the_support_view_mode_holds_from_above_the_plate() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let place = AffineTransform3D::from_translation([25.0, 22.0, 12.0]);
            let mut camera = camera_at([35.0, 27.0, 12.0], 0.4, -1.2, 60.0);
            camera.pitch = 0.6;
            let (ctrl, _document, run) =
                wired_slab(&window, Transform3D::Affine(place), camera);
            let top = in_force_session(&run).unwrap().raft_top_mm() as f32;

            assert_eq!(window.get_support_view(), 0, "whole by default");
            assert!(ctrl.under_plate_supports_for_test().1.is_none());

            window.invoke_support_view_picked(1);
            assert_eq!(window.get_support_view(), 1);
            let (shown_up_to, lines) = ctrl.under_plate_supports_for_test();
            assert!(lines.is_some(), "contacts draw the lines from above too");
            let kept = if top > 0.0 {
                crate::under_plate::hidden_below_mm(top)
            } else {
                -1.0
            };
            assert_eq!(shown_up_to, kept, "the raft stays");

            window.invoke_support_view_picked(2);
            let (see_through, none) = ctrl.under_plate_supports_for_test();
            assert_eq!(see_through, kept);
            assert!(none.is_none(), "see-through draws no lines");

            window.invoke_support_view_picked(0);
            assert_eq!(ctrl.under_plate_supports_for_test().0, f32::MAX);
        });
    }

    /// A box mesh from `lo` to `hi` (world mm).
    fn box_mesh(lo: [f32; 3], hi: [f32; 3]) -> sc_mesh_core::IndexedMesh {
        let cube = sc_mesh_fixtures::unit_cube_mesh();
        let (clo, chi) =
            cube.vertices()
                .iter()
                .fold(([f32::MAX; 3], [f32::MIN; 3]), |(a, b), v| {
                    let p = v.as_array();
                    (
                        std::array::from_fn(|k| a[k].min(p[k])),
                        std::array::from_fn(|k| b[k].max(p[k])),
                    )
                });
        let vertices = cube
            .vertices()
            .iter()
            .map(|v| {
                let p = v.as_array();
                sc_mesh_core::Vertex::new(std::array::from_fn(|k| {
                    lo[k] + (p[k] - clo[k]) / (chi[k] - clo[k]) * (hi[k] - lo[k])
                }))
            })
            .collect();
        sc_mesh_core::IndexedMesh::new(None, vertices, cube.faces().to_vec())
    }

    /// A window driven through its viewport callbacks over a slab with its
    /// supports and a wall standing in front of the left half of them (#3),
    /// seen from the front, a little from below, in perspective or
    /// orthographic. The slab's supports are selected, Remove is armed.
    struct Walled {
        window: MainWindow,
        ctrl: Rc<ViewportController>,
        document: Rc<SceneDocument>,
        run: Rc<SupportRun>,
        generated: Arc<SupportSession>,
        wall: NodeId,
        /// The supports wholly behind the wall, and those wholly beside it.
        behind: Vec<String>,
        beside: Vec<String>,
    }

    fn walled(orthographic: bool) -> Walled {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
        {
            let ctrl = ctrl.clone();
            window.on_viewport_pointer_down(move |x, y, m| {
                ctrl.on_pointer_down(crate::viewport::ScreenPoint::new(x, y), m)
            });
        }
        {
            let ctrl = ctrl.clone();
            window.on_viewport_pointer_move(move |x, y, m| {
                ctrl.on_pointer_move(crate::viewport::ScreenPoint::new(x, y), m)
            });
        }
        {
            let ctrl = ctrl.clone();
            window.on_viewport_pointer_up(move |x, y, m| {
                ctrl.on_pointer_up(crate::viewport::ScreenPoint::new(x, y), m)
            });
        }
        let document = ctrl.document();
        let mut graph = Scene::new();
        let slab = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(box_mesh([10.0, 20.0, 9.0], [30.0, 30.0, 11.0]))),
            )
            .unwrap();
        graph.add_root(slab).unwrap();
        let outcome = start_supports(&graph, SupportRequest::default())
            .unwrap()
            .join();
        // The wall, in front of the left half (y below the slab), comes after
        // the run: a model of the scene, with no supports of its own. It
        // reaches below the plate, so no ray passes under it.
        let wall = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(box_mesh([4.0, 14.0, -3.0], [19.0, 15.0, 14.0]))),
            )
            .unwrap();
        graph.add_root(wall).unwrap();
        document.install_graph(graph.clone(), PathBuf::from("slab.stl"));
        let run =
            SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        *run.started_on.borrow_mut() = Some(document.scene_revision());
        run.finish(outcome);
        document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
            slab,
        )));
        ctrl.set_section(WorkspaceSection::Supports);
        ctrl.set_viewport_for_test(800.0, 600.0);
        let target = glam::Vec3::new(20.0, 25.0, 5.0);
        let mut camera = crate::camera::OrbitCamera::new(target, 30.0);
        camera.set_axis_view(crate::camera::AxisView::Front);
        if camera.eye().y > target.y {
            camera.set_axis_view(crate::camera::AxisView::Back);
        }
        // A little from below, so the slab's underside shows in both
        // projections (edge-on, an orthographic ray would graze it).
        camera.pitch -= 0.15;
        camera.distance = 30.0;
        camera.orthographic = orthographic;
        assert!(camera.eye().y < 14.0, "the wall stands between");
        assert!(
            camera.eye().z > 0.0 && camera.eye().z < 9.0,
            "below the underside"
        );
        ctrl.set_camera_for_test(camera);
        let generated = in_force_session(&run).expect("editable");
        let wholly = |keep: &dyn Fn(f64) -> bool| -> Vec<String> {
            generated
                .support_paths()
                .into_iter()
                .filter(|(_, path)| path.iter().all(|p| keep(p[0])))
                .map(|(id, _)| id)
                .collect()
        };
        let behind = wholly(&|x| x < 18.5);
        let beside = wholly(&|x| x > 20.5);
        assert!(
            !behind.is_empty() && !beside.is_empty(),
            "{} behind, {} beside",
            behind.len(),
            beside.len()
        );
        window.set_support_tool(2);
        Walled {
            window,
            ctrl,
            document,
            run,
            generated,
            wall,
            behind,
            beside,
        }
    }

    impl Walled {
        /// A pixel where `id` shows: one along its line, behind the wall if
        /// it is, else where the camera sees it first.
        fn pixel_of(&self, id: &str, hidden: bool) -> [f32; 2] {
            let (_, along) = along_supports(self.generated.support_paths())
                .into_iter()
                .find(|(k, _)| k == id)
                .expect("the support");
            // In sight, a pixel where the hit is this support's own: not a
            // brace of it, nor a support that crosses in front.
            let its_own = |point: [f32; 3]| {
                let mut probe = (*self.generated).clone();
                probe.remove_near(point.map(f64::from)).is_ok()
                    && !probe.tips().iter().any(|(left, _)| left == id)
            };
            along
                .iter()
                .rev()
                .filter_map(|p| {
                    let at = self.ctrl.project_for_test(*p)?;
                    let first = self.ctrl.first_hit_for_test(at)?;
                    let on_it = (0..3).map(|k| (first.point[k] - p[k]).powi(2)).sum::<f32>() < 1.0;
                    let walled = first.leaf == self.wall;
                    (if hidden {
                        walled
                    } else {
                        on_it && its_own(first.point)
                    })
                    .then_some(at)
                })
                .next()
                .expect("a pixel of it")
        }

        fn session(&self) -> Arc<SupportSession> {
            in_force_session(&self.run).unwrap()
        }
    }

    /// Remove's hover and click take only a support in sight (#3): behind
    /// the wall, nothing lights up and a click removes nothing; beside it,
    /// it lights up and goes. In perspective and orthographic.
    #[test]
    fn a_click_takes_only_a_support_in_sight() {
        for orthographic in [false, true] {
            on_fresh_slint_thread(move || {
                let w = walled(orthographic);
                let hidden = w.pixel_of(&w.behind[0], true);
                w.window
                    .invoke_viewport_pointer_move(hidden[0], hidden[1], no_keys());
                assert!(
                    w.run.hover_at.get().is_none(),
                    "a support behind the wall lights up (orthographic {orthographic})"
                );
                w.window
                    .invoke_viewport_pointer_down(hidden[0], hidden[1], no_keys());
                w.window
                    .invoke_viewport_pointer_up(hidden[0], hidden[1], no_keys());
                settle_click(&w.run);
                assert!(
                    Arc::ptr_eq(&w.session(), &w.generated),
                    "a click behind the wall removed: {}",
                    w.window.get_supports_last_run()
                );

                let seen = w.pixel_of(&w.beside[0], false);
                w.window
                    .invoke_viewport_pointer_move(seen[0], seen[1], no_keys());
                assert!(w.run.hover_at.get().is_some(), "it lights up");
                w.window
                    .invoke_viewport_pointer_down(seen[0], seen[1], no_keys());
                w.window
                    .invoke_viewport_pointer_up(seen[0], seen[1], no_keys());
                settle_click(&w.run);
                let after = w.session();
                assert!(
                    !after.tips().iter().any(|(id, _)| *id == w.beside[0]),
                    "the support in sight goes: {}",
                    w.window.get_supports_last_run()
                );
                assert!(w.document.undo().unwrap(), "one undo");
            });
        }
    }

    /// Remove's box and brush take only what is in sight (#3): dragged over
    /// every support, they take those beside the wall and none behind it. In
    /// perspective and orthographic.
    #[test]
    fn the_box_and_the_brush_leave_what_is_out_of_sight() {
        for orthographic in [false, true] {
            on_fresh_slint_thread(move || {
                let w = walled(orthographic);
                let screen = |p: [f32; 3]| w.ctrl.project_for_test(p).expect("on screen");
                let left = screen([6.0, 25.0, 5.0]);
                let right = screen([30.0, 25.0, 5.0]);
                let (top, bottom) = (screen([20.0, 25.0, 8.5])[1], screen([20.0, 25.0, 1.5])[1]);
                let check = |what: &str| {
                    settle_click(&w.run);
                    let after = w.session();
                    assert!(
                        !Arc::ptr_eq(&after, &w.generated),
                        "{what} took nothing (orthographic {orthographic}): {}",
                        w.window.get_supports_last_run()
                    );
                    let left_ids: Vec<String> =
                        after.tips().into_iter().map(|(id, _)| id).collect();
                    assert!(
                        w.behind.iter().all(|id| left_ids.contains(id)),
                        "{what} took a support behind the wall (orthographic {orthographic})"
                    );
                    assert!(w.document.undo().unwrap(), "one undo");
                };

                // The brush, along the supports at mid height.
                w.window.set_support_remove_mode(1);
                let y = (top + bottom) / 2.0;
                w.window.invoke_viewport_pointer_down(left[0], y, no_keys());
                for k in 1..=40 {
                    let x = left[0] + (right[0] - left[0]) * k as f32 / 40.0;
                    w.window.invoke_viewport_pointer_move(x, y, no_keys());
                }
                w.window.invoke_viewport_pointer_up(right[0], y, no_keys());
                check("the brush");

                // The box, over all of them.
                w.window.set_support_remove_mode(0);
                w.window
                    .invoke_viewport_pointer_down(left[0], top - 20.0, no_keys());
                for k in 1..=10 {
                    let t = k as f32 / 10.0;
                    w.window.invoke_viewport_pointer_move(
                        left[0] + (right[0] - left[0]) * t,
                        top - 20.0 + (bottom - top + 40.0) * t,
                        no_keys(),
                    );
                }
                w.window
                    .invoke_viewport_pointer_up(right[0], bottom + 20.0, no_keys());
                check("the box");
            });
        }
    }

    /// Move tip grabs only a support in sight (#3): dragged from behind the
    /// wall, nothing moves; from beside it, the support follows. In
    /// perspective and orthographic.
    #[test]
    fn move_tip_grabs_only_a_support_in_sight() {
        for orthographic in [false, true] {
            on_fresh_slint_thread(move || {
                let w = walled(orthographic);
                w.window.set_support_tool(3);
                // Dropped on the slab's underside, beside the wall.
                let to = w
                    .ctrl
                    .project_for_test([26.0, 21.0, 9.0])
                    .expect("on screen");
                let drag = |from: [f32; 2]| {
                    w.window
                        .invoke_viewport_pointer_down(from[0], from[1], no_keys());
                    for k in 1..=8 {
                        let t = k as f32 / 8.0;
                        w.window.invoke_viewport_pointer_move(
                            from[0] + (to[0] - from[0]) * t,
                            from[1] + (to[1] - from[1]) * t,
                            no_keys(),
                        );
                    }
                    w.window.invoke_viewport_pointer_up(to[0], to[1], no_keys());
                    settle_click(&w.run);
                };
                // A tip is grabbed within 3 mm of it: a pixel of the line
                // near the top.
                let near_tip = |id: &str, hidden: bool| {
                    let (_, along) = along_supports(w.generated.support_paths())
                        .into_iter()
                        .find(|(k, _)| k == id)
                        .unwrap();
                    along
                        .iter()
                        .take(4)
                        .filter_map(|p| {
                            let at = w.ctrl.project_for_test(*p)?;
                            let first = w.ctrl.first_hit_for_test(at)?;
                            let on_it =
                                (0..3).map(|k| (first.point[k] - p[k]).powi(2)).sum::<f32>() < 1.0;
                            (if hidden { first.leaf == w.wall } else { on_it }).then_some(at)
                        })
                        .next()
                };
                let hidden = w
                    .behind
                    .iter()
                    .find_map(|id| near_tip(id, true))
                    .expect("a hidden tip on screen");
                drag(hidden);
                assert!(
                    Arc::ptr_eq(&w.session(), &w.generated),
                    "a tip behind the wall moved (orthographic {orthographic}): {}",
                    w.window.get_supports_last_run()
                );
                // One the engine moves there: nothing hangs from it.
                let movable = |id: &String| {
                    w.generated
                        .preview_move_tip(id, [26.0, 21.0, 9.0], &AddOptions::default())
                        .is_ok_and(|(preview, _)| preview.valid)
                };
                let (id, seen) = w
                    .beside
                    .iter()
                    .filter(|id| movable(id))
                    .find_map(|id| near_tip(id, false).map(|at| (id.clone(), at)))
                    .expect("a tip in sight that moves");
                drag(seen);
                assert!(
                    !w.session().tips().iter().any(|(k, _)| *k == id),
                    "the tip in sight moves (orthographic {orthographic}): {}",
                    w.window.get_supports_last_run()
                );
                assert!(w.document.undo().unwrap(), "one undo");
            });
        }
    }

    /// Measurement, not a gate (#3): what a box and a brush cost on the UI
    /// thread on a big real model seen as a person sees it, now that they
    /// take only what is in sight (a ray a point of a support, at most one a
    /// 2 mm stretch). `SC_STRESS_STL` is the model (scaled to
    /// `SC_STRESS_HEIGHT_MM` tall when set). `cargo test --profile
    /// quick-release -p sc-slicer-gui --lib -- --ignored --nocapture
    /// sight_costs_on_a_real_model`.
    #[test]
    #[ignore = "needs SC_STRESS_STL"]
    fn sight_costs_on_a_real_model() {
        let Ok(path) = std::env::var("SC_STRESS_STL") else {
            return;
        };
        let height: Option<f32> = std::env::var("SC_STRESS_HEIGHT_MM")
            .ok()
            .and_then(|h| h.parse().ok());
        on_fresh_slint_thread(move || {
            let (_window, run, faces) = real_model_run(&path, height);
            let document = run.document.clone();
            let (supports, _) = run.support_leaf().expect("the supports");
            document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
                supports,
            )));
            let (lo, hi) = faces.iter().fold(
                (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)),
                |(a, b), (p, _)| (a.min((*p).into()), b.max((*p).into())),
            );
            let centre = (lo + hi) / 2.0;
            let mut camera = crate::camera::OrbitCamera::new(centre, (hi - lo).length() / 2.0);
            camera.set_axis_view(crate::camera::AxisView::Front);
            let ctrl = run.ctrl.clone();
            ctrl.set_section(WorkspaceSection::Supports);
            ctrl.set_viewport_for_test(1600.0, 1000.0);
            ctrl.set_camera_for_test(camera);
            let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
            let generated = in_force_session(&run).unwrap();
            println!("{} supports", generated.support_paths().len());
            {
                let t = Instant::now();
                let mut seen = 0;
                for k in 0..200 {
                    let at = [
                        400.0 + (k % 20) as f32 * 40.0,
                        200.0 + (k / 20) as f32 * 60.0,
                    ];
                    seen += usize::from(ctrl.first_hit_for_test(at).is_some());
                }
                println!("one ray: {:.2} ms ({seen} of 200 hit)", ms(t) / 200.0);
            }

            // A box over the left half of the view, in 20 moves.
            let started = ctrl
                .with_drag_view(|view| {
                    run.drag_start(SupportDragKind::Box, &cursor_at([0.0, 0.0]), view)
                })
                .unwrap();
            assert!(started);
            let (mut worst, mut total) = (0.0f64, 0.0f64);
            for k in 1..=20 {
                let at = [800.0 * k as f32 / 20.0, 1000.0 * k as f32 / 20.0];
                let t = Instant::now();
                ctrl.with_drag_view(|view| run.drag_move(&cursor_at(at), view));
                worst = worst.max(ms(t));
                total += ms(t);
            }
            // What the box still owes goes on between the moves, a step a
            // tick: the longest step, and all of them.
            let (mut step_worst, mut steps, mut owed_total) = (0.0f64, 0, 0.0f64);
            loop {
                let t = Instant::now();
                let more = run.box_step(BOX_RAYS);
                step_worst = step_worst.max(ms(t));
                owed_total += ms(t);
                steps += 1;
                if !more {
                    break;
                }
            }
            println!(
                "box, owed rays after the moves: {steps} steps, longest {step_worst:.1} ms, all {owed_total:.0} ms"
            );
            let picked = match run.drag.borrow().as_ref() {
                Some(DragState::Remove { picked, looked, .. }) => (picked.len(), looked.len()),
                _ => (0, 0),
            };
            run.drag_end(false);
            println!(
                "box over half the view: {} supports in sight, {} rays; UI per move max {worst:.1} ms, all 20 moves {total:.0} ms",
                picked.0, picked.1
            );

            // A brush across the middle, 80 moves.
            let started = ctrl
                .with_drag_view(|view| {
                    run.drag_start(SupportDragKind::Brush, &cursor_at([0.0, 500.0]), view)
                })
                .unwrap();
            assert!(started);
            let (mut worst, mut total) = (0.0f64, 0.0f64);
            for k in 1..=80 {
                let at = [1600.0 * k as f32 / 80.0, 500.0];
                let t = Instant::now();
                ctrl.with_drag_view(|view| run.drag_move(&cursor_at(at), view));
                worst = worst.max(ms(t));
                total += ms(t);
            }
            let picked = match run.drag.borrow().as_ref() {
                Some(DragState::Remove { picked, .. }) => picked.len(),
                _ => 0,
            };
            run.drag_end(false);
            println!(
                "brush across the view: {picked} supports in sight; UI per move max {worst:.1} ms, all 80 moves {total:.0} ms"
            );
        });
    }

    /// #384: a model imported beside a supported, hollowed one. Saved as a
    /// `.bread` and opened again, both models come back where they stood,
    /// with the set in force and the hollow of the first.
    #[test]
    fn an_imported_model_saves_and_reopens_with_the_others_supports_and_hollow() {
        use crate::hollow_persist::{install_hollows, restore_hollows};
        use crate::hollow_run::{HollowOptions, Hollowed};
        use crate::print_selection::PrintDefaults;
        use crate::support_persist::{install_sets, restore_supports, sets_in_force};
        use crate::tests_model_import::{finish_loads, write_box};
        use sc_project_formats::{LoadedContent, load_scene};

        /// Every leaf in row order (name, world transform, mesh size), the
        /// sets in force and the hollows.
        fn state(document: &SceneDocument) -> (Vec<(String, String, usize)>, usize, usize) {
            document
                .with_scene(|spatial| {
                    let names = crate::viewport::object_names(spatial.graph());
                    let rows = (0..)
                        .map_while(|i| spatial.leaf_id_at_index(i))
                        .enumerate()
                        .map(|(row, id)| {
                            (
                                names[row].clone(),
                                format!("{:?}", spatial.leaf(id).unwrap().world_transform()),
                                spatial.graph().leaf_mesh_arc(id).unwrap().faces().len(),
                            )
                        })
                        .collect();
                    let sets = sets_in_force(spatial, &document.support_sessions()).len();
                    (rows, sets, document.hollows().borrow().len())
                })
                .expect("a scene is open")
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let dragon = write_box(dir.path(), "dragon.stl", [10.0, 10.0, 10.0]);
        let path = dir.path().join("plate.bread");
        let (saved, reopened) = on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (ctrl, _run, leaves) = supported_in(&window, &[[20.0, 20.0, 5.0]]);
            let document = ctrl.document();
            let shown = document
                .with_scene(|s| s.graph().leaf_mesh_arc(leaves[0]).cloned())
                .flatten()
                .expect("the model's mesh");
            document.hollows().borrow_mut().push(Hollowed {
                hollow: shown,
                original: Arc::new(sc_mesh_fixtures::unit_cube_mesh()),
                drains: Vec::new(),
                options: HollowOptions::default(),
                plugs: Vec::new(),
                solids: Vec::new(),
                engine: None,
                stretch: None,
                drilled: false,
            });

            ctrl.open_models(vec![dragon]);
            finish_loads(&ctrl);
            let saved = state(&document);
            crate::scene_save::save_project_to(&document, &path, &PrintDefaults::default())
                .expect("the project saves");

            let loaded = load_scene(&path, None, |_| {}).expect("it opens");
            let LoadedContent::Project(project) = loaded.content else {
                panic!("a .bread opens as a project");
            };
            let mut scene = project.scene;
            let supports = restore_supports(&mut scene, project.supports);
            assert!(supports.failures.is_empty(), "{:?}", supports.failures);
            let hollows = restore_hollows(&scene, project.hollows);
            assert!(hollows.failures.is_empty(), "{:?}", hollows.failures);
            let again = Rc::new(ViewportController::new_for_test()).document();
            again.install_project_graph(scene, path.clone(), project.payload);
            install_sets(&again, supports.sets);
            install_hollows(&again, hollows.hollows);
            (saved, state(&again))
        });
        let (rows, sets, hollows) = &saved;
        assert_eq!(rows.len(), 3, "the model, its supports and the import");
        assert!(rows.iter().any(|row| row.0 == "dragon"));
        assert_eq!((*sets, *hollows), (1, 1));
        assert_eq!(reopened, saved, "everything comes back as it was saved");
    }

    /// #723: with the box on, each model takes the zone preset of its
    /// height; picking a preset by hand turns the box off.
    #[test]
    fn each_model_takes_the_preset_of_its_height() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let mut leaves = Vec::new();
            for (x, height) in [(0.0, 30.0), (40.0, 100.0), (80.0, 200.0)] {
                let leaf = graph
                    .add_leaf(
                        None,
                        translation([x, 0.0, 0.0]),
                        LeafPayload::Mesh(Arc::new(box_mesh([0.0; 3], [20.0, 20.0, height]))),
                    )
                    .unwrap();
                graph.add_root(leaf).unwrap();
                leaves.push(leaf);
            }
            document.install_graph(graph, PathBuf::from("boxes.stl"));
            let run =
                SupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            assert!(run.request_for_selection().model_options.is_empty(), "off");
            run.set_by_height(true);
            assert!(window.get_support_by_height());
            let request = run.request_for_selection();
            let presets = run.presets.borrow().clone();
            for (k, leaf) in leaves.iter().enumerate() {
                let (_, options) = request
                    .model_options
                    .iter()
                    .find(|(id, _)| id == leaf)
                    .expect("each model its options");
                assert_eq!(options.zones, presets[k].options.zones, "model {k}");
            }
            let line = run.height_picks.borrow().clone().expect("the line");
            assert!(line.contains(&presets[2].name), "{line}");
            run.on_preset_picked(1);
            assert!(!window.get_support_by_height(), "picked by hand");
            assert!(run.request_for_selection().model_options.is_empty());
        });
    }

    // The card of supports out of date (#408), with the helpers above.
    #[path = "../../tests_stale_supports.rs"]
    mod stale_supports;

    // Braces laid by hand (#3), through the viewport.
    #[path = "../../tests_hand_braces.rs"]
    mod hand_braces;

    // The support test plate (#464), written on request.
    #[path = "../../tests_support_test_plate.rs"]
    mod support_test_plate;

    // A new raft redraws the sets already made (#519).
    #[path = "../../tests_raft_follows.rs"]
    mod raft_follows;

    // Where supports are recommended while placed by hand (#524).
    #[path = "../../tests_support_hints.rs"]
    mod support_hints;

    // What editing needs is held in the Supports tab alone (#269).
    #[path = "../../tests_support_caches.rs"]
    mod support_caches;
}

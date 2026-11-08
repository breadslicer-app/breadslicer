/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for Tools > "Generate Supports (DragonFruit)":
//! runs `sc_geom::df_supports` (the native port of DragonFruit's
//! auto-support: island scan, placement, bracing, raft) on the scene.
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
//! runs them ahead of time (`start_df_prescan`, in a pool of half the cores)
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
use sc_geom::df_supports::zone_presets::{ZonePreset, figures_70, figures_150, minis_30};
use sc_geom::df_supports::{
    AddNote, AddOptions, BraceEnd, DfPrescan, DfSupportOptions, DfSupportOutcome, DfSupportRequest,
    DfSupportSession, EditError, EditPreview, HandBraceRefusal, HandEditKind, KeptEdits, LostEdit,
    LostWhy, RaftMode, ScanCache, start_df_prescan, start_df_supports,
};
use sc_mesh_core::transform::Transform3D;
use sc_print_profiles::resolve_layer_height;
use sc_scene::NodeId;
use sc_scene_edit::SceneFragment;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts, MessageKey, StaleSupportsChoice};
use crate::i18n::ctr;
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
use crate::support_persist::{
    DrawnSet, StaleWhy, SupportSessions, in_force, latest_drawing, model_of, record, sets_in_force,
    sets_of_model, support_leaf, support_leaves,
};
use crate::viewport::ViewportController;
#[cfg(test)]
use crate::viewport::support_drag::DragView;
use crate::viewport::support_drag::{
    DragPoint, Project, ScreenHit, SupportDragHandler, SupportDragKind,
};
use crate::{MainWindow, WorkspaceSection};

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

/// Supports generated ahead of a Generate press, for the scene and request
/// they were made for. A request is told apart by its printer figures and
/// options (`ahead_key`). Each run carries the lift of the models resting on the plate it was
/// generated for (#389), which a Generate press makes with the paste.
#[derive(Default)]
struct Ahead {
    running: Option<(
        JobHandle<DfSupportOutcome>,
        SceneRevision,
        String,
        Option<SceneEdit>,
    )>,
    ready: Option<(SceneRevision, String, DfSupportOutcome, Option<SceneEdit>)>,
}

/// What tells two requests apart for [`Ahead`], with whether models resting
/// on the plate are lifted first (#389).
fn ahead_key(request: &DfSupportRequest, lift_on_plate: bool) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{lift_on_plate}",
        request.layer_height_mm, request.pixel_pitch_mm, request.options, request.models
    )
}

/// How often the prescan watcher looks at the scene.
const PRESCAN_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// The background prescan's state.
#[derive(Default)]
struct Prescan {
    /// The job in flight, the scene it started on and when.
    running: Option<(JobHandle<DfPrescan>, SceneRevision, Instant)>,
    /// The last finished prescan and the scene it ran on.
    ready: Option<(SceneRevision, Arc<ScanCache>)>,
    /// A Generate press waiting for the running prescan: the run starts
    /// from its scans as soon as it finishes.
    run_waiting: bool,
}

pub(crate) struct DfSupportRun {
    ui: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    handle: RefCell<Option<JobHandle<DfSupportOutcome>>>,
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
    /// For the run in flight that regenerates supports (#153): the scene
    /// revision that left its models waiting, and the models.
    regenerating: RefCell<Option<(SceneRevision, SupportsWanted)>>,
    poll_timer: Rc<slint::Timer>,
    prescan: RefCell<Prescan>,
    /// Supports computed while the person works on something else.
    ahead: RefCell<Ahead>,
    /// When the scene last changed, live drag ticks included. Stamped by the
    /// scene subscription; that stamp is all the prescan costs an edit.
    last_change: Rc<Cell<Instant>>,
    prescan_timer: slint::Timer,
    _scene_events: SceneSubscription,
    /// The options the next run uses (the Supports tab), saved across
    /// sessions.
    options: RefCell<DfSupportOptions>,
    /// Built-in presets first, then the person's own.
    presets: RefCell<Vec<Preset>>,
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
    /// The brace tool's first end (#3), with the set it is on: the next
    /// click on a support lays the brace.
    brace_from: RefCell<Option<(DrawnSet, BraceEnd)>>,
    box_poll: slint::Timer,
    /// This driver, for what a drag starts from the viewport's side.
    me: RefCell<std::rc::Weak<DfSupportRun>>,
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
    /// The raft top of the supports this driver pasted last, `Some(None)`
    /// for supports without a raft.
    generated_raft_top: Cell<Option<Option<f64>>>,
    /// Clears the status bar's note of the last run.
    status_timer: slint::Timer,
    /// A row picked on the card of supports out of date (#408): the set the
    /// next run makes again. Taken by the run it starts; a Generate press
    /// drops it.
    redo: RefCell<Option<Redo>>,
}

/// The supports of one model made again from the card of supports out of
/// date (#408), as a Generate press makes them for it.
struct Redo {
    model: NodeId,
    /// "Recalculate, keeping my edits": the session whose hand edits go
    /// back on the new set. `None` for "Regenerate".
    keep: Option<Arc<DfSupportSession>>,
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
        AddNote::TooFar => ctr!("DfSupportDialog", "no support close enough to hang from"),
        AddNote::NoneBelow => ctr!("DfSupportDialog", "no support below the tip to hang from"),
        AddNote::TooSteep => ctr!(
            "DfSupportDialog",
            "hanging from the supports nearby would lean too far"
        ),
        AddNote::HitsModel => ctr!("DfSupportDialog", "it hits the model"),
        AddNote::CrossesSupport => ctr!("DfSupportDialog", "it would cross another support"),
        AddNote::HostsFull => ctr!("DfSupportDialog", "the supports nearby carry all they can"),
        AddNote::SteepSurface => ctr!(
            "DfSupportDialog",
            "the surface there is too steep for a tip"
        ),
        AddNote::CarriesOthers => ctr!("DfSupportDialog", "other supports hang from it"),
        AddNote::FacesUp => ctr!(
            "DfSupportDialog",
            "the surface there faces up; nothing below can reach it"
        ),
    }
}

/// The Add tool's line for a hover preview: what a click there does when it
/// does not simply hang the support from another, and whether that warns.
/// Empty for a support that hangs as asked.
fn preview_line(preview: &EditPreview, attach: bool) -> (slint::SharedString, bool) {
    let note = preview.note.map(note_text);
    match (preview.valid, preview.attached, note) {
        (true, true, _) => Default::default(),
        (true, false, Some(why)) => (ctr!("DfSupportDialog", "Its own trunk: {}.", why), false),
        (true, false, None) => Default::default(),
        (false, _, Some(why)) if attach => {
            (ctr!("DfSupportDialog", "Cannot hang it: {}.", why), true)
        }
        (false, _, Some(why)) => (ctr!("DfSupportDialog", "Cannot place it: {}.", why), true),
        (false, _, None) => (
            ctr!(
                "DfSupportDialog",
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
            ctr!("DfSupportDialog", "Cannot move it there: {}.", why),
            true,
        ),
        (false, None) => (
            ctr!(
                "DfSupportDialog",
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
            ctr!("DfSupportDialog", "both ends are on the same support")
        }
        HandBraceRefusal::OtherModel => {
            ctr!("DfSupportDialog", "the two supports hold different models")
        }
        HandBraceRefusal::NotAShaft => {
            ctr!("DfSupportDialog", "a brace joins trunks and branches only")
        }
        HandBraceRefusal::OffShaft => ctr!(
            "DfSupportDialog",
            "an end is off its support: too near the tip, or too low"
        ),
        HandBraceRefusal::TooClose => {
            ctr!("DfSupportDialog", "the supports are too close to brace")
        }
        HandBraceRefusal::TooLong => ctr!(
            "DfSupportDialog",
            "it would be longer than the preset lets a brace be"
        ),
        HandBraceRefusal::TooFlat => ctr!(
            "DfSupportDialog",
            "it would be flatter than 45°: at 45° it ends off the other support"
        ),
        HandBraceRefusal::HitsModel => ctr!("DfSupportDialog", "it would cross the model"),
    }
}

/// The brace tool's line for a preview of the second support (#3): what a
/// click there lays, or why it cannot, and whether that warns.
fn brace_line(preview: &EditPreview) -> (slint::SharedString, bool) {
    match (preview.valid, preview.brace) {
        (true, _) => (
            ctr!(
                "DfSupportDialog",
                "Click to lay the brace. Esc drops the first end."
            ),
            false,
        ),
        (false, Some(why)) => (
            ctr!("DfSupportDialog", "Cannot brace them: {}.", brace_text(why)),
            true,
        ),
        (false, None) => (
            ctr!("DfSupportDialog", "Cannot brace them: no brace fits there."),
            true,
        ),
    }
}

/// The brace tool's line once its first end is picked (#3).
fn brace_first_line() -> (slint::SharedString, bool) {
    (
        ctr!(
            "DfSupportDialog",
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
            "DfSupportDialog",
            "there is no model or support within reach of that point"
        ),
        EditError::NoRoute(_) | EditError::NotHeld => ctr!(
            "DfSupportDialog",
            "there is no route to the plate or to another support"
        ),
        EditError::Tessellation(why) => {
            ctr!(
                "DfSupportDialog",
                "the supports could not be drawn: {}",
                why
            )
        }
    }
}

/// The tip `id` draws for `session`, in mm: its preset's size of that id,
/// else that of the shipped sizes (#360).
fn tip_mm(session: Option<&DfSupportSession>, id: &str) -> Option<f64> {
    session
        .and_then(|s| s.options())
        .and_then(|o| o.zones.as_ref())
        .and_then(|z| z.rules.sizes.iter().find(|s| s.id == id))
        .map(|s| s.tip_diameter_mm)
        .or_else(|| {
            sc_geom::df_supports::zone_presets::support_sizes()
                .into_iter()
                .find(|s| s.id == id)
                .map(|s| s.tip_diameter_mm)
        })
}

/// Where a set's share of a hand edit starts.
enum EditStart {
    /// The set's session as drawn: the edit is made on a copy.
    Base(Arc<DfSupportSession>),
    /// The session the preview of the very click left, the edit in it.
    Previewed(Box<DfSupportSession>),
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
    /// Lays a brace from this end to the support at this point (#3).
    Brace(BraceEnd, [f32; 3]),
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
    Done(Box<DfSupportSession>, Arc<sc_mesh_core::IndexedMesh>),
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
        EditOp::RemoveTips(_) | EditOp::MoveTip(..) | EditOp::Brace(..) => 0,
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
            // DragonFruit's router recurses: the same room as the hover
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
        EditStart::Base(base) => (DfSupportSession::clone(&base), false),
        EditStart::Previewed(edited) => (*edited, true),
    };
    // The port is DragonFruit's code on hand-picked input: a panic in it is
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
    /// It went as [`DfSupportRun::add_supports_at`] tells: in the scene as
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
}

/// A drag of a Supports tab tool in flight (#3).
enum DragState {
    /// Move tip: the support grabbed, and where its tip was last asked to.
    Move {
        drawn: DrawnSet,
        tip: String,
        to: Option<[f32; 3]>,
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
    base: Arc<DfSupportSession>,
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
    others: Vec<Arc<DfSupportSession>>,
    /// For the brace tool's second support (`tool` 7): the first end.
    brace_from: Option<BraceEnd>,
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
    preview: Option<EditPreview>,
    /// The preview shows only what the tool holds (the brace tool's first
    /// support, the cursor off any other): drawn as pending.
    pending: bool,
    /// For an Add that goes through: the session the click leaves.
    edited: Option<DfSupportSession>,
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
            // DragonFruit's router recurses; room to spare on dense models.
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
    options: DfSupportOptions,
    built_in: bool,
}

/// The presets that ship with SoulCrafted: DragonFruit's three sizing
/// tiers, then the three zone presets (#360, #479).
fn built_in_presets() -> Vec<Preset> {
    let tier = |name: slint::SharedString, sizing: &str| Preset {
        name: name.to_string(),
        options: DfSupportOptions {
            sizing: sizing.to_owned(),
            ..Default::default()
        },
        built_in: true,
    };
    let zones = |name: slint::SharedString, zones: ZonePreset| Preset {
        name: name.to_string(),
        options: DfSupportOptions {
            zones: Some(zones),
            ..Default::default()
        },
        built_in: true,
    };
    vec![
        tier(ctr!("Inspector", "Structure (default)"), "structure"),
        tier(ctr!("Inspector", "Detail (small miniatures)"), "detail"),
        tier(ctr!("Inspector", "Anchor (heavy parts)"), "anchor"),
        zones(ctr!("Inspector", "Minis 30 mm+"), minis_30()),
        zones(ctr!("Inspector", "Figures 70 mm+"), figures_70()),
        zones(ctr!("Inspector", "Figures 150 mm+"), figures_150()),
    ]
}

/// The Supports tab's summary of the zone preset of `options` (#360),
/// empty without one.
fn zone_summary(options: &DfSupportOptions) -> slint::SharedString {
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
fn recommended_grid_mm(options: &DfSupportOptions) -> Option<f64> {
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
fn shown_grid_mm(options: &DfSupportOptions) -> f64 {
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
fn grid_hint(options: &DfSupportOptions) -> slint::SharedString {
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

fn load_saved_options() -> DfSupportOptions {
    support_data_dir()
        .and_then(|d| std::fs::read_to_string(d.join("options.toml")).ok())
        .and_then(|text| toml::from_str::<DfSupportOptions>(&text).ok())
        .map(|o| o.clamped())
        .unwrap_or_default()
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
            let options = toml::from_str::<DfSupportOptions>(&text).ok()?.clamped();
            Some(Preset {
                name,
                options,
                built_in: false,
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

impl DfSupportRun {
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
            regenerating: RefCell::new(None),
            poll_timer: Rc::new(slint::Timer::default()),
            prescan: RefCell::new(Prescan::default()),
            ahead: RefCell::new(Ahead::default()),
            last_change,
            prescan_timer: slint::Timer::default(),
            _scene_events: scene_events,
            options: RefCell::new(load_saved_options()),
            presets: RefCell::new(Vec::new()),
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
            box_poll: slint::Timer::default(),
            me: RefCell::new(std::rc::Weak::new()),
            queued_clicks: RefCell::new(std::collections::VecDeque::new()),
            click_poll: slint::Timer::default(),
            raft_armed,
            raft_ignored: Cell::new(false),
            raft_layer_height_mm: Cell::new(DfSupportRequest::default().layer_height_mm),
            raft_checked_at: Cell::new(Instant::now()),
            generated_raft_top: Cell::new(None),
            status_timer: slint::Timer::default(),
            redo: RefCell::new(None),
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
            let handler: Rc<dyn SupportDragHandler> = run.clone();
            run.ctrl.set_support_drag_handler(Rc::downgrade(&handler));
        }
        run.reload_presets();
        run.push_options(ui);
        {
            let run = run.clone();
            ui.on_support_option_changed(move |code, value| run.on_option(code, value));
        }
        {
            let run = run.clone();
            ui.on_support_preset_picked(move |i| run.on_preset_picked(i));
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
            ui.on_df_support_requested(move || {
                // A Generate press supports what it supports: a card's row
                // left waiting (#408) is dropped.
                run.redo.borrow_mut().take();
                run.start();
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
        run
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
                "DfSupportDialog",
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
        if self.redo.borrow().is_none() && self.take_ahead(&ui) {
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
                ui.set_support_gen_phase(ctr!("DfSupportDialog", "Preparing scans…"));
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
            request.keep_edits = redo
                .keep
                .map(|session| vec![(redo.model, (*session).clone())])
                .unwrap_or_default();
        }
        let lift = self.plate_lift(&request);
        self.launch(ui, request, lift);
    }

    /// The edit that lifts the models of `request` resting on the plate to
    /// the support lift (#389), not applied; `None` with the Supports tab's
    /// box off or no such model.
    fn plate_lift(&self, request: &DfSupportRequest) -> Option<SceneEdit> {
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
        mut request: DfSupportRequest,
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
        match start_df_supports(&scene, request) {
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
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
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
        let job = start_df_prescan(&scene, request);
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
        if self.handle.borrow().is_some()
            || ui.get_loading()
            || self.prescan.borrow().run_waiting
            || !stands(&wanted)
        {
            return;
        }
        let mut request = self.request_for_printer();
        request.options = self.options.borrow().clone();
        request.models = Some(wanted.ids());
        crate::breadcrumbs::note(&format!(
            "supports: regenerating {} model(s)",
            wanted.models.len()
        ));
        *self.regenerating.borrow_mut() = Some((revision, wanted));
        // The person chose the pose: it is not lifted off the plate (#389).
        self.launch(ui, request, None);
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
        if let Ok(job) = start_df_supports(&scene, request) {
            crate::breadcrumbs::note("supports: ahead started");
            ahead.running = Some((job, self.document.scene_revision(), key, lift));
        }
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
    fn request_for_selection(&self) -> DfSupportRequest {
        let mut request = self.request_for_printer();
        request.options = self.options.borrow().clone();
        request.models = self.target_models();
        request
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
    fn request_for_printer(&self) -> DfSupportRequest {
        let snapshot = self.bus.snapshot();
        let selection = self
            .document
            .print_selection()
            .in_force(&PrintDefaults::from(&snapshot));
        let (Some(printer), Some(resin)) =
            (selection.printer().in_force(), selection.resin().in_force())
        else {
            return DfSupportRequest::default();
        };
        let Ok((printer, resin)) = load_selected_profiles(printer, resin) else {
            return DfSupportRequest::default();
        };
        let defaults = DfSupportRequest::default();
        let layer_height_mm = resolve_layer_height(&printer, &resin, None)
            .map_or(defaults.layer_height_mm, |h| h.value());
        let pitch = printer.hardware.pixel_size_microns();
        DfSupportRequest {
            layer_height_mm,
            pixel_pitch_mm: Some([pitch.x.value() / 1000.0, pitch.y.value() / 1000.0]),
            ..defaults
        }
    }

    /// Pushes the options and the preset list to the Supports tab.
    fn push_options(&self, ui: &MainWindow) {
        let o = self.options.borrow().clone();
        let presets = self.presets.borrow();
        let names: Vec<slint::SharedString> = presets
            .iter()
            .map(|p| slint::SharedString::from(p.name.as_str()))
            .collect();
        let current = presets
            .iter()
            .position(|p| p.options == o)
            .map_or(-1, |i| i as i32);
        ui.set_support_preset_names(slint::ModelRc::new(slint::VecModel::from(names)));
        ui.set_support_preset_index(current);
        ui.set_support_sizing_index(match o.sizing.as_str() {
            "detail" => 0,
            "anchor" => 2,
            _ => 1,
        });
        ui.set_support_density(o.area_per_support_mm2 as f32);
        ui.set_support_angle(o.overhang_angle_deg as f32);
        self.push_overhang_limit(ui);
        ui.set_support_size_scale(o.size_scale as f32);
        ui.set_support_min_island(o.min_island_area_mm2 as f32);
        ui.set_support_coverage(o.coverage_percent as f32);
        ui.set_support_stabilization(o.stabilization);
        ui.set_support_bracing(o.bracing);
        ui.set_support_smooth(o.smooth_shafts);
        ui.set_support_raft_index(match o.raft {
            RaftMode::Solid => 0,
            RaftMode::Lines => 1,
            RaftMode::Off => 2,
        });
        ui.set_support_raft_thickness(o.raft_thickness_mm as f32);
        ui.set_support_raft_wall(o.raft_wall);
        ui.set_support_zone_summary(zone_summary(&o));
        ui.set_support_grid_on(o.zones.as_ref().is_some_and(|z| z.grid_mm.is_some()));
        ui.set_support_grid_spacing(shown_grid_mm(&o) as f32);
        ui.set_support_grid_hint(grid_hint(&o));
        ui.set_support_lift_on_plate_label(lift_on_plate_label(support_lift_mm(
            &o,
            self.raft_layer_height_mm.get(),
        )));
    }

    /// Replaces the options, saves them and shows them.
    fn set_options(&self, options: DfSupportOptions) {
        let options = options.clamped();
        if let Some(dir) = support_data_dir()
            && std::fs::create_dir_all(&dir).is_ok()
            && let Ok(text) = toml::to_string_pretty(&options)
        {
            let _ = std::fs::write(dir.join("options.toml"), text);
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
        let o = self.options.borrow();
        (o.overhang_angle_deg, o.area_per_support_mm2)
    }

    /// Whether models wait for supports the last edit took away, or a run
    /// is making them (#153).
    pub(crate) fn regenerating(&self) -> bool {
        self.regenerating.borrow().is_some() || self.document.supports_wanted().is_some()
    }

    /// Generates supports for every model of the scene with `options`, then
    /// and there, as a finished run puts them in: what other modules' tests
    /// start from.
    #[cfg(test)]
    pub(crate) fn generate_now(&self, options: DfSupportOptions) {
        let graph = self
            .document
            .with_scene(|spatial| spatial.graph().clone())
            .expect("a scene");
        let request = DfSupportRequest {
            options,
            ..DfSupportRequest::default()
        };
        let outcome = start_df_supports(&graph, request)
            .expect("the run starts")
            .join();
        *self.started_on.borrow_mut() = Some(self.document.scene_revision());
        self.finish(outcome);
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
        let mut o = self.options.borrow().clone();
        let v = f64::from(value);
        let on = value != 0.0;
        match code {
            0 => {
                o.sizing = ["detail", "structure", "anchor"]
                    .get(value as usize)
                    .unwrap_or(&"structure")
                    .to_string();
            }
            1 => o.area_per_support_mm2 = v,
            2 => o.overhang_angle_deg = v,
            3 => o.size_scale = v,
            4 => o.min_island_area_mm2 = v,
            5 => o.coverage_percent = v,
            6 => o.stabilization = on,
            7 => o.bracing = on,
            8 => o.smooth_shafts = on,
            9 => {
                o.raft = match value as i32 {
                    1 => RaftMode::Lines,
                    2 => RaftMode::Off,
                    _ => RaftMode::Solid,
                };
            }
            10 => o.raft_thickness_mm = v,
            11 => o.raft_wall = on,
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
        self.set_options(o);
    }

    fn reload_presets(&self) {
        let mut presets = built_in_presets();
        presets.extend(load_user_presets());
        *self.presets.borrow_mut() = presets;
    }

    fn on_preset_picked(&self, index: i32) {
        let picked = self
            .presets
            .borrow()
            .get(index.max(0) as usize)
            .map(|p| p.options.clone());
        if let Some(options) = picked {
            self.set_options(options);
        }
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
    fn support_leaf(&self) -> Option<(NodeId, Arc<sc_mesh_core::IndexedMesh>)> {
        self.document.with_scene(support_leaf).flatten()
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
        let outcome = start_df_supports(graph, DfSupportRequest::default())
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
        self.hover_at.set(point.map(|p| (p, shift)));
        let generation = self.hover_generation.get() + 1;
        self.hover_generation.set(generation);
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
                });
            }
            let (hand, how) = self.click_tool(&ui, tool, shift);
            let drawn = self.set_at(point, hand)?;
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
            })
        })
    }

    /// The tool a click of the Supports tab's tool code `tool` is, and how
    /// an Add makes its support: hung from the nearest one with `shift`, at
    /// the tip size picked.
    fn click_tool(&self, ui: &MainWindow, tool: i32, shift: bool) -> (HandTool, AddOptions) {
        if tool == 1 {
            let how = AddOptions {
                attach: shift,
                size: hand_tip_size(ui.get_support_tip_size()),
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
        self.show_add_note(Default::default());
        if let Some(ui) = self.ui.upgrade() {
            self.refresh_tip_label(&ui, None);
        }
    }

    /// The tip size changed: the card says which, and the preview under the
    /// cursor is made again with it.
    fn on_tip_size_changed(self: &Rc<Self>) {
        if let Some(ui) = self.ui.upgrade() {
            self.refresh_tip_label(&ui, None);
        }
        if let Some((point, shift)) = self.hover_at.get() {
            self.on_tool_hover_with(Some(point), shift);
        }
    }

    /// The Add card's line on the tip it places, for `session`'s preset (the
    /// first set in force's when `None`).
    fn refresh_tip_label(&self, ui: &MainWindow, session: Option<&DfSupportSession>) {
        let first = session
            .is_none()
            .then(|| self.sets_in_force().into_iter().next())
            .flatten();
        let session = session.or(first.as_ref().map(|d| d.session.as_ref()));
        let label = match hand_tip_size(ui.get_support_tip_size()) {
            Some(id) => match tip_mm(session, &id) {
                Some(mm) => ctr!("Inspector", "{} mm tip", mm_text(mm)),
                None => Default::default(),
            },
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
            pieces.push(Arc::new(overlay_of(&preview.mesh, tint)));
            if let Some(taken) = &preview.taken {
                pieces.push(Arc::new(overlay_of(taken, OverlayTint::Invalid)));
            }
        }
        match (&answer.preview, answer.tool) {
            (Some(preview), 1) => self.show_add_note(preview_line(preview, answer.how.attach)),
            (Some(preview), 3) => self.show_add_note(move_line(preview)),
            (Some(preview), 7) if !answer.pending => self.show_add_note(brace_line(preview)),
            (_, 6 | 7) if self.brace_from.borrow().is_some() => {
                self.show_add_note(brace_first_line());
            }
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
        if tool == 0 {
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
                    "DfSupportDialog",
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
                "DfSupportDialog",
                "The supports changed since the first end was picked; pick it again."
            ));
            return;
        }
        // Off any support, or on the first again: the first end stays.
        let to = drawn.session.brace_end_near(p);
        if to.as_ref().is_none_or(|to| to.shaft == from.shaft) {
            self.show_add_note((
                ctr!(
                    "DfSupportDialog",
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
        let mut refused = Vec::new();
        let Ok(Prepared { sets, jobs }) = self.prepare_edit(ui, tool, &[point], &how, &mut refused)
        else {
            self.queued_clicks.borrow_mut().clear();
            return;
        };
        let _ = self.launch_edit(ui, sets, refused, jobs, tool, how, None, None);
    }

    /// The edit of a drag (#3) on `drawn`'s set, made off the UI thread as
    /// a click is, or after the edit being made; `previewed` is the session
    /// its preview left, the edit in it.
    fn start_gesture(
        self: &Rc<Self>,
        ui: &MainWindow,
        drawn: DrawnSet,
        op: EditOp,
        previewed: Option<DfSupportSession>,
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
                "DfSupportDialog",
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
        // DragonFruit's router recurses: the same room as the hover worker.
        let spawned = std::thread::Builder::new()
            .name("support-hand-edit".into())
            .stack_size(16 << 20)
            .spawn(job);
        if spawned.is_err() {
            let why = ctr!(
                "DfSupportDialog",
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
                        "DfSupportDialog",
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
                "DfSupportDialog",
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
                "DfSupportDialog",
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
    /// can hold (`DfSupportSession::add_tip_near`), as one undoable edit of
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
                    "DfSupportDialog",
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
                    "DfSupportDialog",
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
                ctr!("DfSupportDialog", "Cancelled; nothing was changed."),
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
                "DfSupportDialog",
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
                NoSet::OutOfDate(leaf) => {
                    self.show_out_of_date(leaf);
                    HandEdit::Notified
                }
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
                        "DfSupportDialog",
                        "That edit failed inside the support engine; nothing was changed."
                    ));
                }
                JobOutcome::Failed(err) => return failed(refusal(&err)),
                // Nothing of this set took.
                JobOutcome::Untouched => continue,
                JobOutcome::Emptied => {
                    return failed(ctr!(
                        "DfSupportDialog",
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
                "DfSupportDialog",
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
            return self.commit_born(ui, born, edits, prior, refused);
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
                "DfSupportDialog",
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
        born: Vec<(DrawnSet, DfSupportSession, Arc<sc_mesh_core::IndexedMesh>)>,
        edits: Vec<(DrawnSet, DfSupportSession, Arc<sc_mesh_core::IndexedMesh>)>,
        prior: Option<SceneEdit>,
        refused: &mut Refusals,
    ) -> HandEdit {
        let fragment = SceneFragment::from_leaf_snapshots(
            born.iter()
                .map(|(_, _, mesh)| (mesh.clone(), Transform3D::Identity)),
        );
        let pasted = self.document.paste_fragment_replacing_all_after(
            prior,
            &fragment,
            [0.0, 0.0, 0.0],
            &[],
        );
        let leaves = match pasted {
            Ok(leaves) if leaves.len() == born.len() => leaves,
            _ => {
                let why = ctr!(
                    "DfSupportDialog",
                    "The scene rejected the new supports; nothing was changed."
                );
                ui.set_supports_last_run(why.clone());
                return HandEdit::Failed(why);
            }
        };
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
    fn born_options_now(&self) -> DfSupportOptions {
        self.options.borrow().clone()
    }

    /// The contact diameter of the tips of a set born now: what the review
    /// of a plate with no set plans with.
    pub(crate) fn born_tip_mm(&self) -> f64 {
        let mut request = self.request_for_printer();
        request.options = self.born_options_now();
        sc_geom::df_supports::tip_diameter_for(&request)
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
                }
                let session =
                    sc_geom::df_supports::empty_df_session(spatial.graph(), model, &request)
                        .ok()??;
                DrawnSet::unborn(spatial, model, session)
            })
            .flatten()
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
                ctr!("DfSupportDialog", "No supports to edit"),
                ctr!(
                    "DfSupportDialog",
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
                    "DfSupportDialog",
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
                ctr!("DfSupportDialog", "No supports to edit"),
                ctr!(
                    "DfSupportDialog",
                    "These supports were opened as a mesh, so they cannot be edited by hand. Generate supports again to edit them."
                ),
            ),
            None if model.is_some() => no_supports(),
            None if self.support_leaf().is_none() => no_supports(),
            None => NoSet::Failed(refusal(&EditError::NothingThere)),
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
                heading: ctr!("DfSupportDialog", "Supports out of date"),
                message: match why {
                    Some(StaleWhy::ModelMesh) => ctr!(
                        "DfSupportDialog",
                        "This model's mesh changed since its supports were made (Repair and Hollow change it), so they can no longer be edited by hand."
                    ),
                    Some(StaleWhy::SupportsMesh) => ctr!(
                        "DfSupportDialog",
                        "The supports' mesh changed since they were made (Repair changes it), so they can no longer be edited by hand."
                    ),
                    Some(StaleWhy::ModelPose) | None => ctr!(
                        "DfSupportDialog",
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
                    StaleSupportsChoice::Regenerate => run.make_set_again(model, None),
                    StaleSupportsChoice::KeepEdits => run.make_set_again(model, Some(session)),
                    StaleSupportsChoice::Leave => {}
                }
            },
        );
    }

    /// A row of the card of supports out of date (#408): the supports of
    /// `model` made again as a Generate press makes them for it alone
    /// (progress, Cancel, one undo step, its own set), with the edits made by
    /// hand in `keep` put back where they still fit when given. A model gone
    /// since is left alone; with a run in flight, the press is refused.
    fn make_set_again(self: &Rc<Self>, model: NodeId, keep: Option<Arc<DfSupportSession>>) {
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
                "DfSupportDialog",
                "Supports are being generated; try again when they are done."
            ));
            return;
        }
        crate::breadcrumbs::note(&format!(
            "supports: made again from the card, keeping edits {}",
            keep.is_some()
        ));
        *self.redo.borrow_mut() = Some(Redo { model, keep });
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

    fn finish(&self, outcome: Result<DfSupportOutcome, JobError>) {
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
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
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
                heading: ctr!("DfSupportDialog", "Supports discarded"),
                message: ctr!(
                    "DfSupportDialog",
                    "The scene changed while supports were being generated; nothing was added."
                ),
                details: Default::default(),
            });
            return;
        }
        let summary = summary_message(&outcome);
        if let Some(ui) = self.ui.upgrade() {
            let seconds = outcome.timings.total.as_secs_f64();
            ui.set_supports_last_run(format!("{summary} {seconds:.1} s.").into());
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
        // One set per model it placed supports on (#197).
        let sets: Vec<(NodeId, Arc<sc_mesh_core::IndexedMesh>, DfSupportSession)> = outcome
            .sets
            .into_iter()
            .filter_map(|set| Some((set.model, Arc::new(set.mesh?), set.session)))
            .collect();
        if sets.is_empty() {
            self.dialogs
                .show_notice(MessageKey::SupportsNotNeeded, move || InfoTexts {
                    heading: ctr!("DfSupportDialog", "No supports needed"),
                    message: summary.as_str().into(),
                    details: details.as_str().into(),
                });
            return;
        }
        let fragment = SceneFragment::from_leaf_snapshots(
            sets.iter()
                .map(|(_, mesh, _)| (mesh.clone(), Transform3D::Identity)),
        );
        // Generating again replaces the supports already there (#152), in the
        // same undoable step as the paste: the sets of the models the run
        // covered, and, from a run over every model, supports no session drew.
        let replaced: Vec<NodeId> = self
            .document
            .with_scene(|spatial| {
                let every = crate::raft_clash::model_bottoms(spatial)
                    .iter()
                    .all(|(id, _)| covered.contains(id));
                support_leaves(spatial)
                    .map(|(id, _)| id)
                    .filter(|id| match model_of(&self.sessions, *id) {
                        Some(model) => covered.contains(&model),
                        None => every,
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Generating again over the same models and settings draws the same
        // supports in the same place (#188): the status bar says so, or else
        // that the previous set was replaced, as nothing seems to happen.
        let replaced_note = (!replaced.is_empty()).then(|| {
            let fresh: Vec<&sc_mesh_core::IndexedMesh> =
                sets.iter().map(|(_, mesh, _)| mesh.as_ref()).collect();
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
            let pasted = self.document.paste_fragment_replacing_all_after(
                lift,
                &fragment,
                [0.0, 0.0, 0.0],
                &replaced,
            );
            if let Ok(leaves) = &pasted
                && leaves.len() == sets.len()
            {
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
                // Models left waiting for supports have them now.
                self.document
                    .settle_supports_wanted(self.document.scene_revision(), &covered);
            }
            if let Err(error) = pasted {
                let details = error.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
                    context: None,
                    message: ctr!(
                        "DfSupportDialog",
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
        if !kept.is_empty() {
            self.show_kept_edits(kept, details);
            return;
        }
        self.dialogs
            .show_notice(MessageKey::SupportsGenerated, move || InfoTexts {
                heading: ctr!("DfSupportDialog", "Supports generated"),
                message: summary.as_str().into(),
                details: details.as_str().into(),
            });
    }
}

impl DfSupportRun {
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
        sets: &[(NodeId, Arc<sc_mesh_core::IndexedMesh>, DfSupportSession)],
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
                    if !untransformed(&leaf.world_transform()) || drawn.session.hand_edited() {
                        return false;
                    }
                    let Some(at) = unmatched.iter().position(|(model, mesh, _)| {
                        *model == drawn.model && mesh.as_ref() == leaf.mesh()
                    }) else {
                        return false;
                    };
                    let (_, _, session) = unmatched.swap_remove(at);
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
                        "DfSupportDialog",
                        "The edit made by hand was put back.",
                        "All {n} edits made by hand were put back.",
                        total
                    )
                    .to_string(),
                );
            } else {
                lines.push(if total == 1 {
                    ctr!(
                        "DfSupportDialog",
                        "The edit made by hand could not be kept:"
                    )
                    .to_string()
                } else {
                    ctr!(
                        "DfSupportDialog",
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
                            "DfSupportDialog",
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
                        "DfSupportDialog",
                        "These supports were saved by an older version, which did not record the supports removed or moved by hand: those are back where the run places them."
                    )
                    .to_string(),
                );
            }
            if !lost.is_empty() || unrecorded {
                lines.push(
                    ctr!(
                        "DfSupportDialog",
                        "Undo brings back the supports as they were."
                    )
                    .to_string(),
                );
            }
            InfoTexts {
                heading: ctr!("DfSupportDialog", "Supports recalculated"),
                message: lines.join("\n").into(),
                details: details.as_str().into(),
            }
        });
    }

    /// Shows `text` in the status bar in place of its hint for a few
    /// seconds; a newer note restarts the time.
    fn show_status(&self, text: slint::SharedString) {
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
        outcome: DfSupportOutcome,
    ) {
        let stands = self
            .document
            .with_scene(|spatial| wanted.stands_in(spatial))
            .unwrap_or(false);
        if self.document.scene_revision() != revision || !stands {
            return;
        }
        let ids = wanted.ids();
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
        let sets: Vec<(NodeId, Arc<sc_mesh_core::IndexedMesh>, DfSupportSession)> = outcome
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
                    .filter(|id| model_of(&self.sessions, *id).is_some_and(|m| ids.contains(&m)))
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
                            heading: ctr!("DfSupportDialog", "Supports regenerated"),
                            message: ctr!(
                                "DfSupportDialog",
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
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
                    context: None,
                    message: ctr!(
                        "DfSupportDialog",
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
            ctr!("DfSupportDialog", "Supports to remove: {}.", picked.len()),
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
        });
    }

    /// The supports a box from `press` to `corner` takes, by their ids, and
    /// whether rays are still owed for it: at most `budget` are sent.
    #[allow(clippy::too_many_arguments)]
    fn boxed(
        tips: &[(String, Vec<[f32; 3]>)],
        seen_from: &SeenFrom,
        locate: &SupportLocator,
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
            let Some(DragState::Remove {
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
            }) = drag.as_mut()
            else {
                return None;
            };
            if !*owed {
                return None;
            }
            let (ids, more) = Self::boxed(
                tips, seen_from, locate, looked, *press, *corner, view, budget,
            );
            *owed = more;
            let changed = ids != *picked;
            *picked = ids;
            Some((drawn.clone(), picked.clone(), changed, more))
        });
        let Some(Some((drawn, picked, changed, more))) = stepped else {
            self.box_poll.stop();
            return false;
        };
        if changed {
            self.picks_changed(&drawn, &picked);
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
        locate: &SupportLocator,
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
                let seen = hit.is_some_and(|hit| locate.support_at(hit) == Some(k));
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

impl SupportDragHandler for DfSupportRun {
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
            SupportDragKind::Box | SupportDragKind::Brush => {
                let Some(drawn) = self.selected_set_in_force() else {
                    self.show_add_note((
                        ctr!(
                            "DfSupportDialog",
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
                            &locate,
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
        if let DragState::Remove { drawn, picked, .. } = &state
            && !picked.is_empty()
        {
            run.picks_changed(drawn, picked);
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
                                locate,
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
                            tips, seen_from, locate, looked, *press, *corner, project, BOX_RAYS,
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
            None => {}
        }
    }

    fn cancel_pending(&self) -> bool {
        self.drop_brace()
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
                    ui.set_supports_last_run(ctr!("DfSupportDialog", "Left where it was. {}", why));
                    self.show_add_note(Default::default());
                    return;
                }
                let edited = previewed.and_then(|answer| answer.edited);
                self.show_add_note(Default::default());
                run.start_gesture(&ui, drawn, EditOp::MoveTip(tip, to), edited);
            }
            DragState::Remove { drawn, picked, .. } => {
                self.show_add_note(Default::default());
                if picked.is_empty() {
                    drop_it();
                    return;
                }
                run.start_gesture(&ui, drawn, EditOp::RemoveTips(picked), None);
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
        LostWhy::SurfaceChanged => ctr!("DfSupportDialog", "the model surface there changed"),
        LostWhy::NoRoute => ctr!("DfSupportDialog", "no support can reach that point now"),
        LostWhy::CannotAttach => ctr!("DfSupportDialog", "no support to hang it from there now"),
        LostWhy::BraceEndsGone => ctr!(
            "DfSupportDialog",
            "the supports it joined are no longer there"
        ),
        LostWhy::BraceRefused(why) => brace_text(why),
        LostWhy::CarriesOthers => ctr!(
            "DfSupportDialog",
            "other supports of the new run hang from the one there"
        ),
    };
    match edit.kind {
        HandEditKind::Added => ctr!("DfSupportDialog", "support added at {}: {}", at, why),
        HandEditKind::Moved => ctr!("DfSupportDialog", "support moved to {}: {}", at, why),
        HandEditKind::Braced => ctr!("DfSupportDialog", "brace from {}: {}", at, why),
    }
}

/// One line: models, islands, roots and triangles.
fn summary_message(outcome: &DfSupportOutcome) -> String {
    ctr!(
        "Inspector",
        "Models: {}, islands: {}, support roots: {}, triangles: {}.",
        outcome.models,
        outcome.islands,
        outcome.support.collections.roots.len(),
        outcome.triangles
    )
    .to_string()
}

/// Appends one run's summary and full timing report to
/// `<local data dir>/logs/df-supports-timings.log`, returning its path, or
/// `None` when there is no data directory or the write fails (the log is a
/// diagnostic: a failure never stops the run).
fn append_timing_log(summary: &str, report: &str) -> Option<std::path::PathBuf> {
    use std::io::Write;
    // Written only while the diagnostic log is on.
    if !crate::breadcrumbs::enabled() {
        return None;
    }
    let path = crate::breadcrumbs::logs_dir()?.join("df-supports-timings.log");
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
fn details_message(outcome: &DfSupportOutcome) -> String {
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
            let run = DfSupportRun::install(&window, bus, document, ctrl.dialogs(), ctrl.clone());
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_df_supports(&graph, DfSupportRequest::default())
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_df_supports(&graph, DfSupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let roots = |run: &DfSupportRun| {
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let finish_run = |run: &Rc<DfSupportRun>| {
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
            run.set_options(DfSupportOptions::default());
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
            run.set_options(DfSupportOptions {
                raft_thickness_mm: 3.0,
                ..DfSupportOptions::default()
            });
            run.start();
            assert!(run.handle.borrow().is_none(), "held back by the warning");
            run.generate_ignoring_raft();
            finish_run(&run);
            let anyway = sets();
            notices.push(window.get_status_notice().to_string());
            // A support added by hand, then Generate: a different set.
            run.set_options(DfSupportOptions::default());
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let generate = |run: &Rc<DfSupportRun>| {
                run.start();
                let handle = run.handle.borrow_mut().take().expect("a run started");
                window.set_support_gen_in_progress(false);
                run.finish(handle.join());
            };
            run.set_options(DfSupportOptions::default());
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
                let run = DfSupportRun::install(
                    &window,
                    bus,
                    document.clone(),
                    ctrl.dialogs(),
                    ctrl.clone(),
                );
                run.set_options(DfSupportOptions::default());
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
        assert!(
            on_generate.contains("17 layers")
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_df_supports(&graph, DfSupportRequest::default())
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_df_supports(&graph, DfSupportRequest::default())
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
    ) -> (Rc<SceneDocument>, Rc<DfSupportRun>, Vec<NodeId>) {
        let (ctrl, run, leaves) = supported_in(window, cubes);
        (ctrl.document(), run, leaves)
    }

    /// [`supported`], with the viewport that holds the scene.
    fn supported_in(
        window: &MainWindow,
        cubes: &[[f64; 3]],
    ) -> (Rc<ViewportController>, Rc<DfSupportRun>, Vec<NodeId>) {
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
            DfSupportRun::install(window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let outcome = start_df_supports(&graph, DfSupportRequest::default())
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

            // Live, the support leaf rides along on its transform.
            move_live(&document, model, at, [23.0, 18.0, 8.0]);
            move_live(&document, model, at, there);
            assert_eq!(
                leaf_transform(&document, supports),
                translation([7.0, -5.0, 0.0])
            );
            // A move put back puts them back.
            move_live(&document, model, there, at);
            assert_eq!(leaf_transform(&document, supports), start);
            move_live(&document, model, at, there);

            // Committed: drawn where the model is, the leaf where it was.
            commit_move(&document, model, at, there);
            let (_, moved) = run.support_leaf().expect("the supports stay");
            assert_eq!(leaf_transform(&document, supports), start);
            assert!(
                is_moved(&moved, &generated, [7.0, -5.0]),
                "the same supports, moved"
            );
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
            assert!(document.undo().unwrap(), "the hand edit undoes");

            // One Undo takes the model and its supports back; Redo again.
            assert!(document.undo().unwrap());
            assert_eq!(leaf_transform(&document, model), translation(at));
            let (_, back) = run.support_leaf().unwrap();
            assert!(Arc::ptr_eq(&back, &generated), "the supports as generated");
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &first)));
            assert!(document.redo().unwrap());
            assert_eq!(leaf_transform(&document, model), translation(there));
            let (_, again) = run.support_leaf().unwrap();
            assert!(Arc::ptr_eq(&again, &moved));
            assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &moved_session)));

            // Up in Z, they stay where they were while live, and go when
            // the move is committed.
            let up = [27.0, 15.0, 10.0];
            move_live(&document, model, there, up);
            assert_eq!(leaf_transform(&document, supports), start);
            commit_move(&document, model, there, up);
            assert!(run.support_leaf().is_none(), "taken away");
            let (_, wanted) = document.supports_wanted().expect("new ones wanted");
            assert_eq!(wanted.ids(), vec![model]);
        });
    }

    /// Ends the run in flight as its poll would.
    fn finish_running(run: &DfSupportRun) {
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

            // Committed: drawn where the model is, the leaf where it was, and
            // nothing waits for new supports.
            document.push_edit(edit());
            assert_eq!(leaf_transform(&document, supports), start);
            let (_, turned_mesh) = run.support_leaf().expect("the supports stay");
            assert!(!Arc::ptr_eq(&turned_mesh, &generated), "redrawn");
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

    /// The session in force of the scene's first support set.
    fn in_force_session(run: &DfSupportRun) -> Option<Arc<DfSupportSession>> {
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
        run: &DfSupportRun,
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
            assert_eq!(leaf_transform(&document, second_leaf), start);
            let (_, moved) = set_of(&document, &run, leaves[1]);
            assert!(
                is_moved(&moved, &second_mesh, [0.0, 20.0]),
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
            let sessions = |run: &DfSupportRun| -> Vec<Arc<DfSupportSession>> {
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

            // Both sets redrawn where their models now are, the leaves where
            // they were, nothing to make again.
            let (_, first_moved) = set_of(&document, &run, leaves[0]);
            assert!(
                is_moved(&first_moved, &first_mesh, [10.0, 20.0]),
                "the first set, moved"
            );
            let (_, second_turned) = set_of(&document, &run, leaves[1]);
            assert!(!Arc::ptr_eq(&second_turned, &second_mesh), "redrawn");
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
                second_turned
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
            // The rings of a cylinder start at a fixed world angle (#153).
            for k in 0..3 {
                assert!((lo[k] - want_lo[k]).abs() < 0.2, "{lo:?} {want_lo:?}");
                assert!((hi[k] - want_hi[k]).abs() < 0.2, "{hi:?} {want_hi:?}");
            }
            assert_eq!(leaf_transform(&document, first_leaf), first_start);
            assert_eq!(leaf_transform(&document, second_leaf), second_start);
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
            let plate = sc_arrange::Plate {
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
                    !Arc::ptr_eq(&set_of(&document, &run, l).1, &meshes[i]),
                    "its set redrawn where it went"
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
    /// a set of its own, drawn where it stands and in force (editable and
    /// saved as data), the original's stays as it was, nothing is generated
    /// again, and one Undo takes every copy away.
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
            let plate = sc_arrange::Plate {
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
                assert!(sc_geom::df_supports::is_support_mesh(&mesh));
                assert!(!Arc::ptr_eq(&mesh, &set_mesh), "drawn for the copy");
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let t = Instant::now();
            let outcome = start_df_supports(&graph, DfSupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            println!(
                "supports generated in {:.0} ms",
                t.elapsed().as_secs_f64() * 1e3
            );
            document.set_selection(Selection::One(LeafId(leaf)));
            let plate = sc_arrange::Plate {
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
            let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
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
            let plate = sc_arrange::Plate {
                min: [20.0 - 2.0 * size[0] - 5.0, 20.0 - size[1] - 5.0],
                max: [20.0 + 2.0 * size[0] + 5.0, 20.0 + size[1] + 5.0],
                floor: 0.0,
            };
            document.set_selection(Selection::One(LeafId(model)));
            let filled = fill_now(&document, &plate, ArrangeOptions::default())
                .expect("filled")
                .0;
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
            let placement = sc_arrange::Placement {
                angle: std::f64::consts::FRAC_PI_2,
                translation: [55.0, 0.0],
                on_plate: true,
            };
            let (session, mesh) = copied_set_now(&document, &placement).expect("drawn");
            assert_eq!(session.model_count(), 1);
            assert!(session.supports_only_on(0));
            assert!(sc_geom::df_supports::is_support_mesh(&mesh));
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
                assert_eq!(at(set), set_before[i], "the set's leaf back in place");
                assert!(
                    !Arc::ptr_eq(&mesh, &sets[i].1),
                    "its set redrawn where it went"
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
                    !Arc::ptr_eq(&set_of(&document, &run, l).1, &meshes[i]),
                    "its set redrawn where it turned to"
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
                .search_now(sc_orient::SearchMode::Quick)
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
            let finish_run = |run: &Rc<DfSupportRun>| {
                let handle = run.handle.borrow_mut().take().expect("a run started");
                window.set_support_gen_in_progress(false);
                run.finish(handle.join());
            };

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

    /// A window's support driver on unit cubes centred at `cubes` (a cube
    /// centred 0.5 mm up rests on the plate), with the bus it reads the
    /// settings from: nothing generated yet.
    fn cubes_on(
        window: &MainWindow,
        cubes: &[[f64; 3]],
    ) -> (
        Rc<SceneDocument>,
        Rc<DfSupportRun>,
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
        let run = DfSupportRun::install(
            window,
            bus.clone(),
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        run.set_options(DfSupportOptions::default());
        (document, run, bus, leaves)
    }

    /// Runs the event loop's timers, as the app does, until the run in
    /// flight has ended and its result is in the scene.
    fn until_generated(window: &MainWindow, run: &DfSupportRun) {
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
    fn supported_models(document: &SceneDocument, run: &DfSupportRun) -> Vec<NodeId> {
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

            window.invoke_df_support_requested();
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
            window.invoke_df_support_requested();
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
            assert_eq!(shown(), Some(45.0));
            assert_eq!(window.get_overhang_map_limit(), "45°");

            run.on_option(2, 30.0);
            assert_eq!(shown(), Some(30.0));

            run.set_options(DfSupportOptions {
                zones: Some(minis_30()),
                ..Default::default()
            });
            assert_eq!(shown(), Some(52.0));
            assert_eq!(window.get_overhang_map_limit(), "52°");
            run.set_options(DfSupportOptions {
                zones: Some(figures_70()),
                ..Default::default()
            });
            assert_eq!(shown(), Some(55.0));

            // Supports and Slice show it too (#484); Hollow and Export not.
            for (section, on) in [
                (WorkspaceSection::Supports, true),
                (WorkspaceSection::Slice, true),
                (WorkspaceSection::Hollow, false),
                (WorkspaceSection::Export, false),
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
    fn contact_points(session: &DfSupportSession) -> Vec<[f64; 3]> {
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
            let run = DfSupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            run.generate_now(DfSupportOptions {
                zones: Some(minis_30()),
                ..Default::default()
            });
            let sets = run.sets_in_force();
            let ends: usize = sets
                .iter()
                .flat_map(|set| set.session.pieces())
                .filter(|piece| piece.kind == sc_geom::df_supports::SupportPieceKind::Stick)
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

    /// #484: the overhang map counts every tip of the scene's sets, where
    /// each set is drawn, with the reach of the options that set was made
    /// with, whatever the tab holds now.
    #[test]
    fn the_overhang_map_counts_each_sets_tips_with_its_own_reach() {
        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let (document, run, _, leaves) =
                cubes_on(&window, &[[20.0, 20.0, 8.0], [40.0, 20.0, 8.0]]);
            run.generate_now(DfSupportOptions {
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

            // The second cube's set made again with Figures 150 mm+: its
            // tips reach 2.8 mm, the first cube's keep 1.8.
            document.set_selection(crate::selection::Selection::of(
                [leaves[1]].map(crate::selection::LeafId),
            ));
            run.set_options(DfSupportOptions {
                zones: Some(figures_150()),
                ..Default::default()
            });
            window.invoke_df_support_requested();
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
            window.invoke_df_support_requested();
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
            window.invoke_df_support_requested();
            window.invoke_support_gen_cancel_requested();
            until_generated(&window, &run);
            assert_eq!(document.scene_revision(), revision);
            assert!(bottom_of(&document, resting).abs() < 1e-6);
            assert!(support_sets(&document).is_empty());
            assert!(!document.can_undo());

            window.invoke_df_support_requested();
            until_generated(&window, &run);
            let lift = sc_geom::df_supports::DEFAULT_SUPPORT_LIFT_MM;
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
            window.invoke_df_support_requested();
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
                let run = DfSupportRun::install(
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
        save_and_reopen(DfSupportRequest::default());
    }

    /// A zone preset (#360) keeps its supports through a save and reopen,
    /// and its options with them.
    #[test]
    fn zone_preset_supports_saved_as_bread_reopen_editable() {
        let mut request = DfSupportRequest::default();
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
        assert_eq!(zoned[0].name, "Minis 30 mm+");
        assert_eq!(zoned[1].name, "Figures 70 mm+");
        assert_eq!(zoned[2].name, "Figures 150 mm+");
        for preset in zoned {
            let text = toml::to_string_pretty(&preset.options).expect("a preset saves");
            let back: DfSupportOptions = toml::from_str(&text).expect("a preset reads back");
            assert_eq!(back, preset.options);
            assert_eq!(back.clamped(), preset.options, "inside every bound");
            assert!(!on_fresh_slint_thread(move || zone_summary(&back)).is_empty());
        }
        assert!(zone_summary(&DfSupportOptions::default()).is_empty());
    }

    fn save_and_reopen(request: DfSupportRequest) {
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
                let run = DfSupportRun::install(
                    &window,
                    bus,
                    document.clone(),
                    ctrl.dialogs(),
                    ctrl.clone(),
                );
                let zones = request.options.zones.clone();
                let outcome = start_df_supports(&graph, request).unwrap().join();
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
                let reopened = mesh.faces() == saved_mesh.faces()
                    && mesh.vertices() == saved_mesh.vertices()
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
    fn tips(session: &DfSupportSession) -> Vec<f64> {
        let c = &session.state().collections;
        c.trunks
            .values()
            .filter_map(|t| t.contact_cone.as_ref())
            .chain(c.branches.values().filter_map(|b| b.contact_cone.as_ref()))
            .chain(c.leaves.values().map(|l| &l.contact_cone))
            .map(|cone| cone.profile.contact_diameter_mm)
            .collect()
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

    /// Waits for the click being made off the UI thread to land.
    fn settle_click(run: &Rc<DfSupportRun>) {
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
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
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
    ) -> (MainWindow, Rc<DfSupportRun>, Vec<FaceCentre>) {
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
            DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let outcome = start_df_supports(&graph, DfSupportRequest::default())
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
    fn sweep(run: &Rc<DfSupportRun>, points: &[[f32; 3]], every: Duration) -> Sweep {
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

    /// A slab 20 × 10 × 2 mm, its underside 9 mm up, with the supports a
    /// run gives it, those selected.
    fn supported_slab(window: &MainWindow) -> (Rc<SceneDocument>, Rc<DfSupportRun>) {
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
            DfSupportRun::install(window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let outcome = start_df_supports(&graph, DfSupportRequest::default())
            .unwrap()
            .join();
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
    fn wait_preview(run: &Rc<DfSupportRun>) {
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
            let near = |s: &DfSupportSession, p: [f64; 3]| s.tip_near(p, 0.3).is_some();
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
    ) -> (Rc<ViewportController>, Rc<SceneDocument>, Rc<DfSupportRun>) {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
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
        let document = ctrl.document();
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(slab_mesh())))
            .unwrap();
        graph.add_root(leaf).unwrap();
        document.install_graph(graph.clone(), PathBuf::from("slab.stl"));
        let run =
            DfSupportRun::install(window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let outcome = start_df_supports(&graph, DfSupportRequest::default())
            .unwrap()
            .join();
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
    fn tip_added(before: &DfSupportSession, after: &DfSupportSession) -> Option<[f64; 3]> {
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

            // Move tip: the last one added, grabbed by its shaft just below
            // its tip, dropped on another pixel.
            window.set_support_tool(3);
            let session = in_force_session(&run).unwrap();
            let paths = session.support_paths();
            let (id, grab) = paths
                .iter()
                .filter(|(_, path)| path.len() >= 2)
                .find_map(|(id, path)| {
                    let (tip, next) = (path[0], path[1]);
                    let len = (0..3)
                        .map(|k| (next[k] - tip[k]).powi(2))
                        .sum::<f64>()
                        .sqrt();
                    let below: [f64; 3] =
                        std::array::from_fn(|k| tip[k] + (next[k] - tip[k]) * (0.8 / len.max(0.8)));
                    let at = ctrl.project_for_test(below.map(|c| c as f32))?;
                    let on =
                        ctrl.support_hit_for_test(crate::viewport::ScreenPoint::new(at[0], at[1]))?;
                    (far(on, tip) < 2.0).then(|| (id.clone(), at))
                })
                .expect("a tip to grab on screen");
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
                    raft_first(at).then_some((at, p))
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
        run: Rc<DfSupportRun>,
        generated: Arc<DfSupportSession>,
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
        let outcome = start_df_supports(&graph, DfSupportRequest::default())
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
            DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
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
            along
                .iter()
                .rev()
                .filter_map(|p| {
                    let at = self.ctrl.project_for_test(*p)?;
                    let first = self.ctrl.first_hit_for_test(at)?;
                    let on_it = (0..3).map(|k| (first.point[k] - p[k]).powi(2)).sum::<f32>() < 1.0;
                    let walled = first.leaf == self.wall;
                    (if hidden { walled } else { on_it }).then_some(at)
                })
                .next()
                .expect("a pixel of it")
        }

        fn session(&self) -> Arc<DfSupportSession> {
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

    // The card of supports out of date (#408), with the helpers above.
    #[path = "../../tests_stale_supports.rs"]
    mod stale_supports;

    // Braces laid by hand (#3), through the viewport.
    #[path = "../../tests_hand_braces.rs"]
    mod hand_braces;

    // The support test plate (#464), written on request.
    #[path = "../../tests_support_test_plate.rs"]
    mod support_test_plate;
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Review card of the Slice tab (#90): the findings of the held slice's
//! review, grouped by severity, errors first.
//!
//! A click on a finding moves the layer slider to the finding's first layer
//! and centres the camera on its focus point. A finding with a suggested fix
//! offers it where the app's own tools apply it as one undoable edit:
//! "Add support here" through the Supports tab's hand tool
//! ([`DfSupportRun::add_tips_at`]). A suction cup, a cavity of a
//! millilitre or more and any cavity in a model hollowed in this window
//! offer "Add drain holes…", which opens the Hollow tab's drain tool on that
//! model, so the person picks where the holes go (#158); a hollow with no
//! drain holes at all is reported as just that.
//!
//! The findings belong to the slice they were computed for: a new slice, a
//! project's reloaded slices, a new scene or a closed one drop them. When the
//! scene changes after the slice, the card says the review is out of date;
//! the findings stay, since the layers shown are still that slice's.
//!
//! A fix is drilled or glued at the point the slice saw, so it is offered
//! only while every model stands where it stood when sliced
//! ([`ModelPose`]): editing the supports (an added support) keeps the other
//! fixes, moving, scaling or re-meshing a model withdraws them.
//!
//! A hollow's volume is the person's (#431): nothing here ever fills one. A
//! small sealed trap, outside a model hollowed here, offers "Fill on the next
//! slice": the mark keeps the cavity's pixels ([`ReviewMarks`]), and the next
//! slice cures them in the preview, the printer file and its review. "Fill
//! all small traps" marks every trap offering a fill. Any other cavity is
//! warned about, with no fill to offer ([`sc_review::checks::traps`]). An
//! island, a cavity or a sudden growth can be ignored, which hides it on
//! every list made for the same models: the list is made again on each slice,
//! and that decision is what stays, in the scene's `.bread` too (#432). The
//! notes (information) are folded until opened. Moving or re-meshing a model
//! drops every mark, as it withdraws the fixes.
//!
//! "Fix all" (#156) works on the list the held review made (#433): it does
//! not slice first, and looks for nothing between its fixes. It applies in one
//! go every fix the app can make by itself, then slices again, and the review
//! of that slice, whole, is the new list:
//!
//! - islands get their tips ([`sc_review::plan_tips`]), all of them in one
//!   undoable edit ([`DfSupportRun::start_supports_at`]);
//! - the small resin traps are marked to fill, as "Fill all small traps"
//!   does.
//!
//! Debris and what is thinner than a pixel need no fix: the review takes
//! them off the layers by itself (#432).
//!
//! The supports are placed off the UI thread (#390): on three minis that
//! takes seconds. The card shows how far it is, with Cancel; Cancel, Undo
//! or a change to the scene meanwhile drops the run, which leaves the scene
//! and the marks as they were. Once the supports are in, as one undoable
//! step, the marks are made and the slice starts, as when Fix All ran then
//! and there.
//!
//! What the new review finds is listed, and that is the end of a press.
//! With "Repeat until clean" (#163, a setting), up to [`REPEAT_PASSES`]
//! passes: the next pass fixes what Fix All can fix in that review, and
//! slices again. The run stops early when the review has nothing left Fix
//! All fixes, or a pass changes nothing. Every pass's supports go into one
//! undo step. Cancel (on the card or on the slice), Undo or an edit meanwhile
//! ends the run and puts the scene, the marks, the slice and its review back
//! as they were before the press. So does one Undo once the run is over: the
//! slice of before the press is kept for it until the next slice starts, and
//! no other slice is ([`BeforeRun`]). A run that added no support left no
//! undo step: "Clear fills" takes its marks back.
//!
//! A row offers "Add support here" exactly when Fix All would add that
//! support ([`ReviewMarks::support_offered`]): not again where one went in
//! and did not hold, and not where the engine refused one on the scene as
//! it stands.
//!
//! A region that grew at once wants tips on what it gained (#437): it gets
//! them the same way, each judged as it goes in. The next review does not
//! list one that has them all; one that got fewer says how many it has and
//! why not the rest.
//!
//! An island gets what [`sc_review::plan_tips`] says, by its row, by Fix All
//! and by the question that ends a slice (#437): "N islands have no
//! support. Add supports?". A plan is candidates: each is placed through the
//! engine and judged there and then by the very decision the review makes
//! ([`TipHold`]), so a tip that goes in holds its island, and the next review
//! does not list it (#433). An island none of them holds gets no tip at all,
//! and its row says why. The question comes only when the list has an
//! island a set can take tips for. Yes is a Fix All run of the islands alone
//! ([`RunKind::Islands`]): the tips, then the slice, and Cancel or Undo take
//! it back the same way. No leaves the list as it is. An island that never
//! gets as wide as a tip offers nothing, and its row says so.
//!
//! Everything else stays as it is: a hollow, a suction cup or a large cavity,
//! which only drain holes open, the output checks, a model in the raft and
//! the supports' own findings. On the review that follows, each finding left
//! says why Fix All left it ([`fix_all_leaves`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_print_glue::PlateFrame;
use sc_review::{
    CavityFill, CheckKind, Finding, Growth, Island, PlatePoint, Severity, SuggestedFix, TipPlan,
};
use sc_scene::NodeId;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::df_support_run::{
    DfSupportRun, EditDropped, EditWatch, HandEdit, PointPlan, PointPlans, PointsEdit, Refusals,
    raft_lift_label,
};
use crate::error_dialog::InfoTexts;
use crate::hollow_run::HollowRun;
use crate::i18n::ctr;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::review::{ReviewOutcome, TipHold};
use crate::review_marks::{FindingKey, ReviewMarks, SlicePatches};
use crate::scene_document::{SceneDocument, SceneEvent, SceneRevision, SceneSubscription};
use crate::selection::{LeafId, Selection};
use crate::slice_result::SliceResult;
use crate::viewport::ViewportController;
use crate::{MainWindow, ReviewRow, WorkspaceSection};

/// How much of the plate a jump to a finding frames, in millimetres around
/// its focus point.
const JUMP_RADIUS_MM: f32 = 15.0;

/// How often the card reads how far Fix All's supports are.
const FIX_ALL_POLL: Duration = Duration::from_millis(100);

/// The largest island that reads as a note when no support fits it, in
/// square millimetres (#163): 1.0. The engine found no valid, convenient spot
/// for a tip, and nothing more can be done for it from the review. A larger
/// one stays a warning: something that size hangs with nothing under it. On
/// a 46 mm pre-supported mini, the islands no support reached grew to 0.003
/// to 0.80 mm².
const NO_SUPPORT_NOTE_MM2: f64 = 1.0;

/// The most passes a Fix All press makes with "Repeat until clean" (#163).
/// The first fixes the list; the second what its review found (a support
/// that can now hang from one added); a third rarely finds anything, and
/// each pass slices the plate again.
pub(crate) const REPEAT_PASSES: usize = 3;

/// The card's state, as the UI's `review-state` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
enum CardState {
    /// No slice is held.
    Empty = 0,
    /// The held slice was reviewed.
    Reviewed = 1,
    /// The review of the held slice failed.
    Failed = 2,
    /// A slice is held that was not reviewed here (a reopened project's).
    NotReviewed = 3,
}

/// A row's suggested fix, as the UI's `ReviewRow.fix` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
enum FixKind {
    None = 0,
    AddSupport = 1,
    AddDrain = 2,
    /// A model inside the raft (#131): lift it clear, or turn the raft off.
    ClearRaft = 4,
}

/// Where one model (a leaf that is not generated supports) stood, and which
/// mesh it carried. The mesh is held weakly, so a dropped mesh never lingers
/// and its address is never reused while the pose is kept.
#[derive(Clone, Debug)]
struct ModelPose {
    leaf: NodeId,
    world: Transform3D,
    mesh: Weak<IndexedMesh>,
}

impl PartialEq for ModelPose {
    fn eq(&self, other: &Self) -> bool {
        self.leaf == other.leaf && self.world == other.world && self.mesh.ptr_eq(&other.mesh)
    }
}

/// Every model of `spatial` with its world pose and mesh, in row order.
/// Generated supports are left out: adding a support keeps the models where
/// the review saw them.
fn model_poses(spatial: &SceneSpatial) -> Vec<ModelPose> {
    let scene = spatial.graph();
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter_map(|leaf| {
            let mesh = scene.leaf_mesh_arc(leaf)?;
            if sc_geom::df_supports::is_support_mesh(mesh) {
                return None;
            }
            Some(ModelPose {
                leaf,
                world: scene.world_transform_of(leaf).ok()?,
                mesh: Arc::downgrade(mesh),
            })
        })
        .collect()
}

/// The review of the slice the document holds.
struct HeldReview {
    /// The document's slice generation when the review landed.
    generation: u64,
    /// The models as the slice saw them, or `None` when the scene had
    /// already changed when the review landed: the fixes are offered only
    /// while the scene's models still match.
    models: Option<Vec<ModelPose>>,
    outcome: ReviewOutcome,
    /// For each finding's cavity, by index: the drain holes of the hollow made
    /// here that holds it, or `None` when no hollow made here does.
    hollows: Vec<Option<usize>>,
    /// Findings whose fix was applied since the slice.
    fixed: BTreeSet<usize>,
    /// Whether Fix All asked for this slice: the findings left say why.
    after_fix_all: bool,
}

pub(crate) struct ReviewPanel {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    ctrl: Rc<ViewportController>,
    supports: Rc<DfSupportRun>,
    hollow: Rc<HollowRun>,
    held: RefCell<Option<HeldReview>>,
    /// The cavities to fill and the findings left alone, kept across slices
    /// of the same models.
    marks: RefCell<ReviewMarks<Vec<ModelPose>>>,
    /// Whether the notes (information) are unfolded.
    notes_open: Cell<bool>,
    /// Whether Fix All asked for the slice in flight.
    fix_all_slicing: Cell<bool>,
    /// The Fix All press being carried out (#163), pass by pass.
    run: RefCell<Option<FixRun>>,
    /// Fix All's supports being placed off the UI thread (#390): how far
    /// the engine is, and the timer that shows it.
    placing: RefCell<Option<Arc<EditWatch>>>,
    /// What the last run replaced, for its Undo (#437).
    before_run: RefCell<Option<BeforeRun>>,
    fix_poll: slint::Timer,
    /// Starts a slice without asking to overwrite the held one, and tells
    /// whether one started: Fix All promises one. Set by the Slice driver.
    reslice: RefCell<Option<Box<dyn Fn() -> bool>>>,
    /// The settings: "Repeat until clean" (#163).
    bus: Rc<PreferencesBus>,
    repeat_pref: RefCell<Option<Subscription>>,
    scene_events: RefCell<Option<SceneSubscription>>,
}

/// What a run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunKind {
    /// Everything Fix All fixes.
    FixAll,
    /// The tips of the islands alone: a Yes to the question after a slice
    /// (#437).
    Islands,
}

/// What a run that added supports replaced, kept once it is over: Undo puts
/// it back, on the scene it was made for. Its slice stays until the next one
/// starts, so a slice in the making never has two others beside it.
struct BeforeRun {
    slice: SliceResult,
    held: Option<HeldReview>,
    marks: ReviewMarks<Vec<ModelPose>>,
}

/// A Fix All press being carried out (#163): one pass, or with "Repeat
/// until clean" up to [`REPEAT_PASSES`]. A Yes to the question after a slice
/// runs the same way, in one pass (#437).
struct FixRun {
    /// What it does.
    kind: RunKind,
    /// The pass being made, from 1.
    pass: usize,
    /// The most passes this run makes.
    passes: usize,
    /// The scene revision once the run's supports are in: a later pass's
    /// supports join that undo step, and Cancel takes it back.
    entry: Option<SceneRevision>,
    /// The marks as they were before the press, for Cancel.
    marks_before: ReviewMarks<Vec<ModelPose>>,
    /// The slice the run's first slice replaced, and its review, for
    /// Cancel.
    slice_before: Option<SliceResult>,
    held_before: Option<HeldReview>,
    /// Whether the slice a pass asked for is being made.
    slicing: bool,
    /// Whether a pass found no supports to edit (none generated, or a run
    /// generating them): the passes after it leave the supports alone
    /// rather than tell it again.
    supports_off: bool,
    /// When the press came, and when the pass being made started.
    started: Instant,
    pass_started: Instant,
}

/// What follows the review of the slice a Fix All pass asked for.
enum AfterPass {
    /// No run asked for it.
    NoRun,
    /// The next pass started.
    Next,
    /// The run is over after this many passes.
    Done(usize),
    /// The run stopped, and said why.
    Stopped,
}

impl FixRun {
    /// A run of up to `passes` passes, the marks being `marks_before`.
    fn new(kind: RunKind, passes: usize, marks_before: ReviewMarks<Vec<ModelPose>>) -> Self {
        Self {
            kind,
            pass: 1,
            passes,
            entry: None,
            marks_before,
            slice_before: None,
            held_before: None,
            slicing: false,
            supports_off: false,
            started: Instant::now(),
            pass_started: Instant::now(),
        }
    }
}

impl ReviewPanel {
    /// Wires the card's callbacks and follows the document; the caller keeps
    /// the panel alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        ctrl: Rc<ViewportController>,
        supports: Rc<DfSupportRun>,
        hollow: Rc<HollowRun>,
    ) -> Rc<Self> {
        let panel = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            ctrl,
            supports,
            hollow,
            held: RefCell::new(None),
            marks: RefCell::new(ReviewMarks::default()),
            notes_open: Cell::new(false),
            fix_all_slicing: Cell::new(false),
            run: RefCell::new(None),
            placing: RefCell::new(None),
            before_run: RefCell::new(None),
            fix_poll: slint::Timer::default(),
            reslice: RefCell::new(None),
            bus,
            repeat_pref: RefCell::new(None),
            scene_events: RefCell::new(None),
        });
        {
            let weak = Rc::downgrade(&panel);
            let sub = panel.document.subscribe(move |event| {
                if let Some(panel) = weak.upgrade() {
                    panel.on_scene_event(event);
                }
            });
            *panel.scene_events.borrow_mut() = Some(sub);
        }
        {
            let weak = Rc::downgrade(&panel);
            ui.on_review_finding_clicked(move |id| {
                if let Some(panel) = weak.upgrade() {
                    panel.jump_to(id);
                }
            });
        }
        {
            let weak = Rc::downgrade(&panel);
            ui.on_review_fix_requested(move |id| {
                if let Some(panel) = weak.upgrade() {
                    panel.apply_fix(id);
                }
            });
        }
        panel.install_marks(ui);
        {
            let weak = Rc::downgrade(&panel);
            ui.on_review_raft_off_requested(move |id| {
                if let Some(panel) = weak.upgrade() {
                    panel.turn_raft_off(id);
                }
            });
        }
        panel.push(None);
        panel
    }

    /// Wires the callbacks of the fills, the left findings and the notes.
    fn install_marks(self: &Rc<Self>, ui: &MainWindow) {
        let weak = Rc::downgrade(self);
        ui.on_review_fill_toggled(move |id| {
            if let Some(panel) = weak.upgrade() {
                panel.toggle_fill(id);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_review_leave_requested(move |id| {
            if let Some(panel) = weak.upgrade() {
                panel.leave(id);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_review_fill_small_requested(move || {
            if let Some(panel) = weak.upgrade() {
                panel.fill_small();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_review_fills_cleared(move || {
            if let Some(panel) = weak.upgrade()
                && !panel.fixing()
            {
                panel.marks.borrow_mut().clear_fills();
                panel.push(None);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_review_fix_all_requested(move || {
            if let Some(panel) = weak.upgrade() {
                panel.fix_all();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_review_fix_all_cancel(move || {
            if let Some(panel) = weak.upgrade() {
                panel.cancel_fix_all();
            }
        });
        // "Repeat until clean" (#163): a setting every window shares.
        let bus = self.bus.clone();
        ui.on_review_fix_repeat_toggled(move |on| {
            bus.update(move |snap| {
                snap.fix_all_repeat = on;
                PreferencesChange::FixAllRepeat
            });
        });
        let weak = Rc::downgrade(self);
        let subscription = self.bus.subscribe(move |snap, change| {
            if matches!(change, PreferencesChange::FixAllRepeat)
                && let Some(ui) = weak.upgrade().and_then(|panel| panel.ui.upgrade())
            {
                ui.set_review_fix_repeat(snap.fix_all_repeat);
            }
        });
        *self.repeat_pref.borrow_mut() = Some(subscription);
        ui.set_review_fix_repeat(self.bus.snapshot().fix_all_repeat);
        let weak = Rc::downgrade(self);
        ui.on_review_notes_toggled(move || {
            if let Some(panel) = weak.upgrade() {
                panel.notes_open.set(!panel.notes_open.get());
                panel.push(None);
            }
        });
    }

    /// Whether a Slice press must re-slice for the fills: they differ from
    /// those the held slice cured.
    pub(crate) fn fills_pending(&self) -> bool {
        self.follow_models(SceneEvent::Mutated);
        self.marks.borrow().pending()
    }

    /// What the next slice cures, with the marks' ids for
    /// [`fills_landed`](Self::fills_landed). Marks made on models that have
    /// changed since are dropped first.
    pub(crate) fn fills_for_slice(&self) -> SlicePatches {
        self.follow_models(SceneEvent::Mutated);
        self.marks.borrow().for_slice()
    }

    /// Lets Fix All start a slice: `reslice` starts one without asking to
    /// overwrite the held slice, and tells whether one started.
    pub(crate) fn set_reslice(&self, reslice: impl Fn() -> bool + 'static) {
        *self.reslice.borrow_mut() = Some(Box::new(reslice));
    }

    /// A slice is starting: it is Fix All's when Fix All asked for it.
    pub(crate) fn slice_started(&self, by_fix_all: bool) {
        self.fix_all_slicing.set(by_fix_all);
        if let Some(before) = self.before_run.borrow_mut().take() {
            let_go(before.slice);
        }
    }

    /// Records that the slice about to be shown cured the fills `ids`.
    pub(crate) fn fills_landed(&self, ids: Vec<u64>) {
        self.marks.borrow_mut().landed(ids);
    }

    /// Drops the marks when the models are no longer those they were made
    /// on, and shows the left findings again once the scene changed.
    fn follow_models(&self, event: SceneEvent) {
        let models = self.current_models();
        let moved = self.marks.borrow_mut().follow(models.as_ref());
        // Another scene brings its own decisions.
        if moved && !matches!(event, SceneEvent::Installed | SceneEvent::Cleared) {
            self.document.set_review_decisions(Vec::new());
        }
    }

    /// The models the held review's marks are made on, while they still
    /// stand as sliced; otherwise tells why no mark can be made.
    fn markable_models(&self) -> Option<Vec<ModelPose>> {
        let models = self
            .held
            .borrow()
            .as_ref()
            .filter(|held| self.fixes_apply(held))
            .and_then(|held| held.models.clone());
        if models.is_none() {
            self.push(Some(ctr!(
                "Review",
                "The models changed since this slice. Slice again before applying a fix."
            )));
        }
        models
    }

    /// The finding with index `id` and the pixels of its fill.
    fn fillable(&self, id: i32) -> Option<(Finding, CavityFill)> {
        let held = self.held.borrow();
        let ReviewOutcome::Reviewed {
            findings, fills, ..
        } = &held.as_ref()?.outcome
        else {
            return None;
        };
        let i = usize::try_from(id).ok()?;
        let finding = findings.get(i)?;
        let cavity = fill_of(finding, hollow_drains(&held.as_ref()?.hollows, i))?;
        Some((finding.clone(), fills.get(cavity)?.clone()))
    }

    /// Marks the finding's cavity to fill on the next slice, or unmarks it.
    fn toggle_fill(&self, id: i32) {
        if self.fixing() {
            return;
        }
        let Some((finding, fill)) = self.fillable(id) else {
            return;
        };
        let Some(models) = self.markable_models() else {
            return;
        };
        let filled = self
            .marks
            .borrow_mut()
            .toggle_fill(FindingKey::of(&finding), &fill, &models);
        self.push(Some(if filled {
            ctr!("Review", "The cavity will be filled on the next slice.")
        } else {
            ctr!("Review", "The cavity will not be filled.")
        }));
    }

    /// Marks every resin trap that offers a fill.
    fn fill_small(&self) {
        if self.fixing() {
            return;
        }
        let Some(models) = self.markable_models() else {
            return;
        };
        let small: Vec<(Finding, CavityFill)> = {
            let held = self.held.borrow();
            match held.as_ref().map(|held| (&held.outcome, &held.hollows)) {
                Some((
                    ReviewOutcome::Reviewed {
                        findings, fills, ..
                    },
                    hollows,
                )) => small_traps(findings, hollows)
                    .filter_map(|i| {
                        let cavity = fill_of(&findings[i], hollow_drains(hollows, i))?;
                        Some((findings[i].clone(), fills.get(cavity)?.clone()))
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let mut marks = self.marks.borrow_mut();
        let mut count = 0;
        for (finding, fill) in &small {
            let key = FindingKey::of(finding);
            if !marks.is_left(&key) && !marks.is_filled(&key) {
                marks.fill(key, fill, &models);
                count += 1;
            }
        }
        drop(marks);
        self.push(Some(ctr!(
            "Review",
            "One cavity will be filled on the next slice.",
            "{n} cavities will be filled on the next slice.",
            count
        )));
    }

    /// What Fix All would do with the held review: the fixes it applies,
    /// without the findings left alone, fixed already or marked, nor the
    /// supports [`ReviewMarks::support_offered`] holds back: one that went
    /// in and did not hold, or one refused on the scene as it stands.
    fn fix_all_plan(&self) -> FixAllPlan {
        let held = self.held.borrow();
        let Some(held) = held.as_ref() else {
            return FixAllPlan::default();
        };
        let ReviewOutcome::Reviewed {
            findings,
            fills,
            islands,
            growths,
            tips,
            frame,
            ..
        } = &held.outcome
        else {
            return FixAllPlan::default();
        };
        let marks = self.marks.borrow();
        let now = self.document.scene_revision();
        let island_tips = IslandTips {
            islands,
            growths,
            plan: tips.as_ref(),
            frame: Some(frame),
        };
        plan_fix_all(findings, fills, &held.hollows, island_tips, |i| {
            let key = FindingKey::of(&findings[i]);
            held.fixed.contains(&i)
                || marks.is_left(&key)
                || marks.is_marked(&key)
                || support_held_back(&findings[i], &marks, now)
        })
    }

    /// [`Self::fix_all_plan`] for the run's next pass: without supports
    /// once a pass found none to edit, and only them for a Yes.
    fn pass_plan(&self) -> FixAllPlan {
        let mut plan = self.fix_all_plan();
        let run = self.run.borrow();
        if run.as_ref().is_some_and(|run| run.supports_off) {
            plan.supports.clear();
            plan.plans.clear();
        }
        if run.as_ref().is_some_and(|run| run.kind == RunKind::Islands) {
            plan.patches.clear();
        }
        plan
    }

    /// The tips the question after a slice would place.
    fn islands_plan(&self) -> FixAllPlan {
        let mut plan = self.fix_all_plan();
        plan.patches.clear();
        plan
    }

    /// Whether the held review knows which tips its islands need: a set can
    /// take them.
    fn tips_planned(&self) -> bool {
        self.held.borrow().as_ref().is_some_and(|held| {
            matches!(&held.outcome, ReviewOutcome::Reviewed { tips: Some(_), .. })
                && self.fixes_apply(held)
        })
    }

    /// Ends a slice with the question (#437), when the list has an island a
    /// set can take tips for.
    fn ask_for_supports(self: &Rc<Self>) {
        if self.fixing() || !self.tips_planned() {
            return;
        }
        let plan = self.islands_plan();
        let offered = plan.first_tips().count();
        if offered == 0 {
            return;
        }
        let regions = plan
            .first_tips()
            .filter_map(|(i, _)| self.finding(i as i32))
            .filter(|finding| finding.check == CheckKind::SurfaceGrowth)
            .count();
        let islands = offered - regions;
        crate::breadcrumbs::note(&format!(
            "review: {islands} islands, {regions} regions, asking for supports"
        ));
        let weak = Rc::downgrade(self);
        self.ctrl.dialogs().show_add_supports(
            move || InfoTexts {
                heading: question_heading(islands, regions),
                message: ctr!(
                    "AddSupportsDialog",
                    "Add supports? The tips go under them, the plate is sliced again, and one Undo takes it all back."
                ),
                details: SharedString::new(),
            },
            move || {
                if let Some(panel) = weak.upgrade() {
                    panel.add_island_supports();
                }
            },
        );
    }

    /// Yes to the question: the tips of the islands, in one undoable edit,
    /// then a new slice. It runs as Fix All does ([`Self::start_pass`]).
    fn add_island_supports(self: &Rc<Self>) {
        let slicing = self
            .ui
            .upgrade()
            .is_some_and(|ui| ui.get_slicing_in_progress());
        if self.fixing() || slicing {
            return;
        }
        if self.markable_models().is_none() || self.islands_plan().supports.is_empty() {
            return;
        }
        let points = self.islands_plan().support_points();
        self.with_preset(&points, |panel| {
            crate::breadcrumbs::note("review: adding the islands' supports");
            let marks = panel.marks.borrow().clone();
            *panel.run.borrow_mut() = Some(FixRun::new(RunKind::Islands, 1, marks));
            panel.start_pass();
        });
    }

    /// Fix All (#156): adds the supports in one undoable edit, marks the
    /// small traps to fill, then slices again; with "Repeat until clean"
    /// (#163), pass
    /// after pass ([`FixRun`]). The supports are placed off the UI thread
    /// (#390); the rest follows once they are in ([`Self::fix_all_landed`]).
    fn fix_all(self: &Rc<Self>) {
        if self.fixing()
            || self
                .ui
                .upgrade()
                .is_some_and(|ui| ui.get_slicing_in_progress())
        {
            return;
        }
        if self.markable_models().is_none() || self.fix_all_plan().is_empty() {
            return;
        }
        let passes = if self.bus.snapshot().fix_all_repeat {
            REPEAT_PASSES
        } else {
            1
        };
        let points = self.fix_all_plan().support_points();
        self.with_preset(&points, move |panel| {
            crate::breadcrumbs::note(&format!("review: fix all, up to {passes} passes"));
            let marks = panel.marks.borrow().clone();
            *panel.run.borrow_mut() = Some(FixRun::new(RunKind::FixAll, passes, marks));
            panel.start_pass();
        });
    }

    /// Starts the run's pass on the held review: its supports off the UI
    /// thread, or the rest then and there when it has none.
    fn start_pass(self: &Rc<Self>) {
        let Some(models) = self.markable_models() else {
            self.stop_run(None);
            return;
        };
        let plan = self.pass_plan();
        if plan.is_empty() {
            self.stop_run(None);
            return;
        }
        if let Some(run) = self.run.borrow_mut().as_mut() {
            run.pass_started = Instant::now();
        }
        let points = plan.support_points();
        let plans = plan.support_plans();
        let generation = self.held.borrow().as_ref().map(|held| held.generation);
        let work = FixAllWork {
            plan,
            models,
            points,
            plans,
            generation,
            revision: self.document.scene_revision(),
        };
        if work.points.is_empty() {
            self.fix_all_landed(&work, None);
            return;
        }
        // A hand edit of the supports being made would make the scene the
        // run starts from stale: Fix All waits for it.
        if self.supports.editing() {
            self.stop_run(Some(ctr!(
                "Review",
                "The supports are being edited; press Fix all again once that is done."
            )));
            return;
        }
        let work = Rc::new(work);
        let weak = Rc::downgrade(self);
        let landing = {
            let work = work.clone();
            move |edit| {
                if let Some(panel) = weak.upgrade() {
                    panel.supports_landed(&work, edit);
                }
            }
        };
        let join = self.run.borrow().as_ref().and_then(|run| run.entry);
        let plans = work.plans.clone();
        match self
            .supports
            .start_supports_at(&work.points, plans, join, landing)
        {
            Ok(watch) => {
                *self.placing.borrow_mut() = Some(watch);
                let weak = Rc::downgrade(self);
                self.fix_poll
                    .start(slint::TimerMode::Repeated, FIX_ALL_POLL, move || {
                        if let Some(panel) = weak.upgrade() {
                            panel.show_fix_progress();
                        }
                    });
                self.push(None);
            }
            // Nothing to make off the UI thread (no supports to edit, a run
            // generating them): the rest goes on as it did then and there.
            Err(outcome) => self.fix_all_landed(&work, Some(outcome)),
        }
    }

    /// Fix All as it ran before #390, every step then and there on the UI
    /// thread: what the background run must leave the same.
    #[cfg(test)]
    pub(crate) fn fix_all_then_and_there(&self) {
        let Some(models) = self.markable_models() else {
            return;
        };
        let plan = self.fix_all_plan();
        if plan.is_empty() {
            return;
        }
        let marks = self.marks.borrow().clone();
        *self.run.borrow_mut() = Some(FixRun::new(RunKind::FixAll, 1, marks));
        let points = plan.support_points();
        let plans = plan.support_plans();
        let supports =
            (!points.is_empty()).then(|| self.supports.add_tips_at(&points, plans.clone()));
        let generation = self.held.borrow().as_ref().map(|held| held.generation);
        let work = FixAllWork {
            plan,
            models,
            points,
            plans,
            generation,
            revision: self.document.scene_revision(),
        };
        self.fix_all_landed(&work, supports);
    }

    /// Whether a Fix All press is being carried out: its supports being
    /// placed, or the slice of one of its passes being made.
    pub(crate) fn fixing(&self) -> bool {
        self.run.borrow().is_some() || self.placing.borrow().is_some()
    }

    /// Cancel on the card while Fix All runs: the run ends, and the scene,
    /// the marks, the slice and its review are as they were before the
    /// press.
    fn cancel_fix_all(&self) {
        // The supports' run tells how it ended (through `supports_landed`);
        // one gone without a word is over all the same.
        self.supports.cancel_points_edit();
        if self.fixing() {
            self.revert_run(EditDropped::Cancelled);
        }
    }

    /// Fix All's supports are no longer being placed.
    fn stop_placing(&self) {
        self.fix_poll.stop();
        *self.placing.borrow_mut() = None;
    }

    /// Ends the run, keeping what it did, with `message` under the summary.
    fn stop_run(&self, message: Option<SharedString>) {
        self.stop_placing();
        let run = self.run.borrow_mut().take();
        if let Some(run) = run {
            crate::breadcrumbs::note(&format!(
                "review: fix all ended after {} of {} passes, {:?}",
                run.pass,
                run.passes,
                run.started.elapsed()
            ));
            self.keep_for_undo(run);
        }
        self.show_pass(None);
        self.push(message);
    }

    /// A run is over: what it replaced stays for its Undo, when it left one.
    fn keep_for_undo(&self, run: FixRun) {
        match (run.entry, run.slice_before) {
            (Some(_), Some(slice)) => {
                *self.before_run.borrow_mut() = Some(BeforeRun {
                    slice,
                    held: run.held_before,
                    marks: run.marks_before,
                });
            }
            (None, Some(slice)) => let_go(slice),
            _ => {}
        }
    }

    /// Ends the run for `reason` and puts back what it changed: its undo
    /// step comes out of the history, and the marks, the slice and its
    /// review are those of before the press. When the scene moved on past
    /// the run's step (an edit came after it), nothing is taken back.
    fn revert_run(&self, reason: EditDropped) {
        self.stop_placing();
        self.show_pass(None);
        let Some(run) = self.run.borrow_mut().take() else {
            self.push(Some(dropped_message(RunKind::FixAll, reason)));
            return;
        };
        let taken_back = match run.entry {
            None => true,
            Some(revision) => matches!(self.document.retract_last_edit(revision), Ok(true)),
        };
        if !taken_back {
            self.push(Some(ctr!(
                "Review",
                "The scene changed while Fix All ran; it stopped. What it did before that is one undo step."
            )));
            return;
        }
        *self.marks.borrow_mut() = run.marks_before;
        if let Some(slice) = run.slice_before {
            // The document drops the held review with its slice: the one of
            // before the press comes back with it.
            if let Some(replaced) = self.document.set_slice_result(slice) {
                let_go(replaced);
            }
            if let Some(mut held) = run.held_before {
                held.generation = self.document.slice_generation();
                *self.held.borrow_mut() = Some(held);
            }
        }
        crate::breadcrumbs::note(&format!(
            "review: fix all taken back at pass {} ({reason:?})",
            run.pass
        ));
        self.push(Some(dropped_message(run.kind, reason)));
    }

    /// The slice window's line for the run's pass, or none.
    fn show_pass(&self, pass: Option<(usize, usize)>) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_fix_pass(match pass {
                Some((pass, passes)) if passes > 1 => {
                    ctr!("Review", "Fix all: pass {} of {}", pass, passes)
                }
                _ => SharedString::new(),
            });
        }
    }

    /// Shows how far Fix All is: the pass, and its supports.
    fn show_fix_progress(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (tried, total) = self
            .placing
            .borrow()
            .as_ref()
            .map_or((0, 0), |watch| watch.progress());
        ui.set_review_fix_progress(if total == 0 {
            0.0
        } else {
            tried as f32 / total as f32
        });
        let pass = self
            .run
            .borrow()
            .as_ref()
            .filter(|run| run.passes > 1)
            .map(|run| ctr!("Review", "Pass {} of {}", run.pass, run.passes));
        let placing = (total > 0).then(|| {
            ctr!(
                "Review",
                "Placing supports: {} of {}",
                tried.min(total),
                total
            )
        });
        ui.set_review_fix_phase(match (pass, placing) {
            (Some(pass), Some(placing)) => format!("{pass} · {placing}").into(),
            (Some(line), None) | (None, Some(line)) => line,
            (None, None) => SharedString::new(),
        });
    }

    /// Fix All's supports are made, or the run was dropped.
    fn supports_landed(&self, work: &FixAllWork, edit: PointsEdit) {
        self.stop_placing();
        match edit {
            PointsEdit::Done(outcome, refused) => {
                // The supports are in; a review that was replaced meanwhile
                // is no longer the one the marks would name.
                let same =
                    self.held.borrow().as_ref().map(|held| held.generation) == work.generation;
                if same {
                    self.fix_all_landed(work, Some((outcome, refused)));
                } else {
                    self.stop_run(Some(ctr!(
                        "Review",
                        "The slice changed while Fix All ran. Slice again to review the result."
                    )));
                }
            }
            PointsEdit::Dropped(reason) => self.revert_run(reason),
        }
    }

    /// The rest of a pass once its supports are in, or failed, as
    /// `supports` tells (`None`: there were none to place): the refusals
    /// and the supports held are noted, the marks made, and the slice
    /// started. A pass that changed nothing ends the run.
    fn fix_all_landed(&self, work: &FixAllWork, supports: Option<(HandEdit, Refusals)>) {
        let FixAllWork {
            plan,
            models,
            points,
            revision,
            ..
        } = work;
        let models = models.clone();
        let mut lines: Vec<String> = Vec::new();
        let mut changed = false;
        if let Some((outcome, refused)) = supports {
            // The findings at each point the engine refused keep why, and
            // the scene it was refused on: their rows say it, and neither
            // they nor Fix All offer it again until the scene changes.
            let refusal_at = |at: [f32; 3]| {
                refused
                    .iter()
                    .find(|(p, _)| same_point(points[*p], at))
                    .map(|(_, why)| why)
            };
            // What the rows tell: an extra tip that goes with a first one
            // has no row of its own.
            let no_tip: BTreeMap<usize, SharedString> = plan
                .row_points()
                .filter_map(|(i, at)| Some((i, refusal_at(at)?.clone())))
                .collect();
            {
                let mut marks = self.marks.borrow_mut();
                for (i, why) in &no_tip {
                    let key = FindingKey::of(&self.finding(*i as i32).expect("a planned finding"));
                    marks.refuse(key, why, *revision, &models);
                }
            }
            // No set took any point (none to edit, a run generating them):
            // another pass would only tell it again.
            let unplaced = match &outcome {
                HandEdit::Applied => false,
                HandEdit::Failed(_) => refused.is_empty(),
                HandEdit::Notified => true,
            };
            match outcome {
                HandEdit::Applied => {
                    changed = true;
                    if let Some(run) = self.run.borrow_mut().as_mut() {
                        run.entry = Some(self.document.scene_revision());
                    }
                    for (i, _) in plan.first_tips() {
                        if !no_tip.contains_key(&i) {
                            let key =
                                FindingKey::of(&self.finding(i as i32).expect("a planned finding"));
                            self.marks.borrow_mut().supported(key, &models);
                            self.mark_fixed(i);
                        }
                    }
                    let added = points.len() - refused.len();
                    lines.push(
                        ctr!("Review", "One support added.", "{n} supports added.", added)
                            .to_string(),
                    );
                    if !no_tip.is_empty() {
                        lines.push(
                            ctr!(
                                "Review",
                                "No support can hold one point; its row says why.",
                                "No support can hold {n} points; their rows say why.",
                                no_tip.len()
                            )
                            .to_string(),
                        );
                    }
                }
                // The engine refused every point: each row says why.
                HandEdit::Failed(_) if !refused.is_empty() => lines.push(
                    ctr!(
                        "Review",
                        "No support can hold one point; its row says why.",
                        "No support can hold {n} points; their rows say why.",
                        no_tip.len()
                    )
                    .to_string(),
                ),
                HandEdit::Failed(why) => lines
                    .push(ctr!("Review", "The supports could not be added: {}", why).to_string()),
                HandEdit::Notified => lines
                    .push(ctr!("Review", "No supports were added: see the notice.").to_string()),
            }
            if unplaced && let Some(run) = self.run.borrow_mut().as_mut() {
                run.supports_off = true;
            }
        }
        {
            let mut marks = self.marks.borrow_mut();
            for (i, fill) in &plan.patches {
                let key = FindingKey::of(&self.finding(*i as i32).expect("a planned finding"));
                marks.fill(key, fill, &models);
            }
        }
        let filled = plan.patches.len();
        if filled > 0 {
            lines.push(
                ctr!(
                    "Review",
                    "One cavity to fill.",
                    "{n} cavities to fill.",
                    filled
                )
                .to_string(),
            );
        }
        changed |= filled > 0;
        let pass = self.run.borrow().as_ref().map(|run| {
            crate::breadcrumbs::note(&format!(
                "review: fix all pass {} of {}: supports and marks in {:?}",
                run.pass,
                run.passes,
                run.pass_started.elapsed()
            ));
            (run.pass, run.passes)
        });
        if let Some((pass, passes)) = pass.filter(|(_, passes)| *passes > 1) {
            lines.insert(
                0,
                ctr!("Review", "Pass {} of {}:", pass, passes).to_string(),
            );
        }
        if !changed {
            // Nothing this pass did would change the next slice: the run is
            // over, and the review held is the last one.
            if pass.is_some_and(|(pass, _)| pass > 1) {
                lines.push(
                    ctr!(
                        "Review",
                        "Nothing more changed, so Fix All stopped. Each finding left says why it was left."
                    )
                    .to_string(),
                );
            }
            self.stop_run(Some(lines.join(" ").into()));
            return;
        }
        lines.push(ctr!("Review", "Slicing again…").to_string());
        self.push(Some(lines.join(" ").into()));
        if let Some(run) = self.run.borrow_mut().as_mut() {
            run.slicing = true;
        }
        self.show_pass(pass);
        let started = self
            .reslice
            .borrow()
            .as_ref()
            .is_some_and(|reslice| reslice());
        if !started {
            // No slice could start (no printer or resin in force): what the
            // pass did stays, for the next Slice press to show.
            if let Some(run) = self.run.borrow_mut().as_mut() {
                run.slicing = false;
            }
            lines.pop();
            lines.push(ctr!("Review", "Slice to check the result.").to_string());
            self.stop_run(Some(lines.join(" ").into()));
        }
    }

    /// The slice a pass asked for ended without a result: cancelled (Fix
    /// All is cancelled with it, and taken back) or failed (the run stops,
    /// what it did stays).
    pub(crate) fn slice_ended_without_result(&self, cancelled: bool) {
        self.fix_all_slicing.set(false);
        let slicing = self.run.borrow().as_ref().is_some_and(|run| run.slicing);
        if !slicing {
            return;
        }
        if cancelled {
            self.revert_run(EditDropped::Cancelled);
        } else {
            self.stop_run(Some(ctr!(
                "Review",
                "Fix All stopped: the slice did not finish. What it did is one undo step."
            )));
        }
    }

    /// A new slice replaced `replaced`: a run keeps the one of before its
    /// press, to put back on Cancel. Any other goes now: no pass leaves a
    /// slice behind.
    pub(crate) fn slice_replaced(&self, replaced: Option<SliceResult>) {
        let mut run = self.run.borrow_mut();
        match run
            .as_mut()
            .filter(|run| run.slicing && run.slice_before.is_none())
        {
            Some(run) => run.slice_before = replaced,
            None => {
                if let Some(slice) = replaced {
                    let_go(slice);
                }
            }
        }
    }

    /// Once the review of the slice a pass asked for is held: the next pass,
    /// when the run makes more and the review has something Fix All fixes;
    /// else the run is over.
    fn after_pass_slice(self: &Rc<Self>) -> AfterPass {
        let (made, more) = {
            let mut run = self.run.borrow_mut();
            let Some(run) = run.as_mut().filter(|run| run.slicing) else {
                return AfterPass::NoRun;
            };
            run.slicing = false;
            crate::breadcrumbs::note(&format!(
                "review: fix all pass {} of {} done in {:?}",
                run.pass,
                run.passes,
                run.pass_started.elapsed()
            ));
            (run.pass, run.pass < run.passes)
        };
        self.show_pass(None);
        // The scene changed while the slice was made (an undo from the
        // menu): the next pass would fix a scene the review did not see.
        if self.is_stale() {
            self.stop_run(Some(ctr!(
                "Review",
                "The scene changed while Fix All ran; it stopped. What it did before that is one undo step."
            )));
            return AfterPass::Stopped;
        }
        if more && !self.pass_plan().is_empty() {
            if let Some(run) = self.run.borrow_mut().as_mut() {
                run.pass += 1;
            }
            self.start_pass();
            return AfterPass::Next;
        }
        let run = self.run.borrow_mut().take();
        if let Some(run) = run {
            crate::breadcrumbs::note(&format!(
                "review: fix all done after {made} passes, {:?}",
                run.started.elapsed()
            ));
            self.keep_for_undo(run);
        }
        AfterPass::Done(made)
    }

    /// Hides the finding until the scene changes.
    fn leave(&self, id: i32) {
        if self.fixing() {
            return;
        }
        let Some(finding) = self.finding(id) else {
            return;
        };
        let Some(models) = self.markable_models() else {
            return;
        };
        self.marks
            .borrow_mut()
            .leave(FindingKey::of(&finding), &models);
        self.document
            .set_review_decisions(self.marks.borrow().decisions());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_review_selected(-1);
        }
        self.ctrl.set_review_highlight(None);
        self.push(None);
    }

    /// Opens the Hollow tab with its drain tool on, the model holding the
    /// cavity at `at` selected, for the person to place the drain holes: a
    /// drain drilled right at the finding is rarely where it looks good
    /// (#158). `false` when no model holds it. Never marks a fill (#431).
    fn open_drain_tool(&self, at: [f32; 3]) -> bool {
        let Some(leaf) = self.hollow.model_at(at) else {
            return false;
        };
        self.document.set_selection(Selection::One(LeafId(leaf)));
        self.ctrl.set_section(WorkspaceSection::Hollow);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_drain_tool(true);
        }
        true
    }

    /// Shows the review of the slice the document has just stored; for the
    /// slice a Fix All pass asked for, goes on with the run (#163). Any other
    /// slice ends with the question about its islands (#437).
    pub(crate) fn show(self: &Rc<Self>, outcome: ReviewOutcome) {
        let hollows = match &outcome {
            ReviewOutcome::Reviewed { findings, .. } => findings
                .iter()
                .map(|finding| match finding.fix {
                    Some(SuggestedFix::AddDrain { at } | SuggestedFix::Fill { at, .. }) => {
                        self.hollow.hollow_at(world(at)).map(|(_, drains)| drains)
                    }
                    _ => None,
                })
                .collect(),
            ReviewOutcome::Cancelled | ReviewOutcome::Failed(_) => Vec::new(),
        };
        let models = if self.is_stale() {
            None
        } else {
            self.current_models()
        };
        // The list is made anew; what the person decided stays.
        if let Some(models) = &models {
            self.marks
                .borrow_mut()
                .restore_decisions(&self.document.review_decisions(), models);
        }
        let by_run = self.fix_all_slicing.replace(false);
        let yes = by_run
            && self
                .run
                .borrow()
                .as_ref()
                .is_some_and(|run| run.kind == RunKind::Islands);
        let after_fix_all = by_run && !yes;
        let left = match &outcome {
            ReviewOutcome::Reviewed { findings, .. } => findings.len(),
            ReviewOutcome::Cancelled | ReviewOutcome::Failed(_) => 0,
        };
        *self.held.borrow_mut() = Some(HeldReview {
            generation: self.document.slice_generation(),
            models,
            outcome,
            hollows,
            fixed: BTreeSet::new(),
            after_fix_all,
        });
        if let Some(ui) = self.ui.upgrade() {
            ui.set_review_selected(-1);
        }
        // A new review: no finding is selected, nothing is highlighted.
        self.ctrl.set_review_highlight(None);
        let passes = if by_run {
            match self.after_pass_slice() {
                AfterPass::Done(passes) => passes,
                // The next pass is on its way, or the run said why it
                // stopped.
                AfterPass::Next | AfterPass::Stopped => return,
                AfterPass::NoRun => 1,
            }
        } else {
            0
        };
        if yes {
            self.push(Some(self.after_yes()));
            return;
        }
        let nothing_left = self.fix_all_plan().is_empty();
        self.push((after_fix_all && left > 0).then(|| match (passes > 1, nothing_left) {
            (false, true) => ctr!(
                "Review",
                "Fix All is done, and nothing is left to fix automatically. Each finding left says why it was left."
            ),
            (false, false) => ctr!(
                "Review",
                "Fix All is done. Each finding left says why it was left."
            ),
            (true, true) => ctr!(
                "Review",
                "Fix All is done after {} passes, and nothing is left to fix automatically. Each finding left says why it was left.",
                passes
            ),
            (true, false) => ctr!(
                "Review",
                "Fix All made {} passes, the most it makes in one press. Press Fix all to go on; each finding left says why it was left.",
                passes
            ),
        }));
        if !by_run {
            self.ask_for_supports();
        }
    }

    /// What the card says of the review that follows a Yes.
    fn after_yes(&self) -> SharedString {
        let held = self.held.borrow();
        let islands = match held.as_ref().map(|held| &held.outcome) {
            Some(ReviewOutcome::Reviewed { findings, .. }) => {
                let marks = self.marks.borrow();
                findings
                    .iter()
                    .filter(|finding| finding.check == CheckKind::Island)
                    .filter(|finding| !marks.is_left(&FindingKey::of(finding)))
                    .count()
            }
            _ => 0,
        };
        if islands == 0 {
            ctr!(
                "Review",
                "The supports are in: no island is left without one."
            )
        } else {
            ctr!(
                "Review",
                "The supports are in. One island is still without one; its row says why.",
                "The supports are in. {n} islands are still without one; their rows say why.",
                islands
            )
        }
    }

    /// Puts back the slice, the review and the marks of before a run once
    /// Undo took its supports out of the scene (#437). Says whether it did.
    fn undo_run(&self) -> bool {
        let back = self.before_run.borrow().as_ref().is_some_and(|before| {
            !self
                .document
                .slice_geometry_changed(before.slice.params.geometry)
        });
        if !back {
            return false;
        }
        let Some(before) = self.before_run.borrow_mut().take() else {
            return false;
        };
        *self.marks.borrow_mut() = before.marks;
        // The document drops the held review with its slice.
        if let Some(replaced) = self.document.set_slice_result(before.slice) {
            let_go(replaced);
        }
        if let Some(mut held) = before.held {
            held.generation = self.document.slice_generation();
            *self.held.borrow_mut() = Some(held);
        }
        crate::breadcrumbs::note("review: the slice of before the run is back");
        self.push(None);
        true
    }

    fn on_scene_event(&self, event: SceneEvent) {
        if !matches!(
            event,
            SceneEvent::SelectionChanged | SceneEvent::DirtyChanged
        ) {
            self.follow_models(event);
        }
        match event {
            SceneEvent::SliceChanged | SceneEvent::Installed | SceneEvent::Cleared => {
                if !matches!(event, SceneEvent::SliceChanged) {
                    // Another scene: nothing of a Fix All run applies to it.
                    self.stop_placing();
                    self.run.borrow_mut().take();
                    self.before_run.borrow_mut().take();
                    self.show_pass(None);
                }
                let current = self.document.slice_generation();
                let gone = self.document.slice_layer_count().is_none()
                    || self
                        .held
                        .borrow()
                        .as_ref()
                        .is_some_and(|held| held.generation != current);
                if gone {
                    self.ctrl.set_review_highlight(None);
                    let dropped = self.held.borrow_mut().take();
                    // The review of before a Fix All press, put back with its
                    // slice on Cancel (#163).
                    if let Some(run) = self.run.borrow_mut().as_mut()
                        && run.slicing
                        && run.held_before.is_none()
                    {
                        run.held_before = dropped;
                    }
                }
                self.push(None);
            }
            SceneEvent::Mutated | SceneEvent::Restructured | SceneEvent::UndoStackChanged => {
                if self.undo_run() {
                    return;
                }
                // Back on the sliced geometry (an undo of the fixes), the fixes
                // are undone too.
                if !self.is_stale()
                    && let Some(held) = self.held.borrow_mut().as_mut()
                {
                    held.fixed.clear();
                }
                self.push(None);
            }
            SceneEvent::SelectionChanged | SceneEvent::DirtyChanged | SceneEvent::Painted => {}
        }
    }

    /// Whether the scene changed since the held slice ran.
    fn is_stale(&self) -> bool {
        self.document
            .slice_params()
            .is_some_and(|params| self.document.slice_geometry_changed(params.geometry))
    }

    /// The scene's models as they stand now.
    fn current_models(&self) -> Option<Vec<ModelPose>> {
        self.document.with_scene(model_poses)
    }

    /// Whether the held review's fixes still land where the slice saw them:
    /// every model stands as it stood when sliced.
    fn fixes_apply(&self, held: &HeldReview) -> bool {
        held.models.is_some() && held.models == self.current_models()
    }

    /// Pushes the card to the window, with `message` (fix feedback) under
    /// the summary.
    fn push(&self, message: Option<SharedString>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let held = self.held.borrow();
        let marks = self.marks.borrow();
        let sliced = self.document.slice_layer_count().is_some();
        let mut notes = 0;
        let mut small = 0;
        let fix_all = if held
            .as_ref()
            .is_some_and(|held| sliced && self.fixes_apply(held))
        {
            self.fix_all_plan().len()
        } else {
            0
        };
        let (state, rows, summary, worst, failure) = match held.as_ref() {
            _ if !sliced => (CardState::Empty, Vec::new(), String::new(), -1, None),
            None => (CardState::NotReviewed, Vec::new(), String::new(), -1, None),
            Some(held) => match &held.outcome {
                ReviewOutcome::Reviewed {
                    findings,
                    fills,
                    islands,
                    growths,
                    tips,
                    frame,
                    ..
                } => {
                    let fixable = self.fixes_apply(held);
                    let mut row_marks = RowMarks::of(
                        findings,
                        &marks,
                        fixable,
                        self.notes_open.get(),
                        self.document.scene_revision(),
                    );
                    let island_tips = IslandTips {
                        islands,
                        growths,
                        plan: tips.as_ref(),
                        frame: Some(frame),
                    };
                    row_marks.no_tip = island_tips.too_narrow(findings);
                    row_marks.growth_tips = island_tips.growth_counts(findings);
                    let findings = &no_support_fits(findings, &row_marks);
                    // A finding fixed since the slice leaves the list (the
                    // next slice shows it again if the fix did not hold).
                    let visible: Vec<Finding> = findings
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !row_marks.left.contains(i) && !held.fixed.contains(i))
                        .map(|(_, finding)| finding.clone())
                        .collect();
                    notes = visible
                        .iter()
                        .filter(|f| f.severity == Severity::Info)
                        .count();
                    small = if fixable {
                        small_traps(findings, &held.hollows)
                            .filter(|i| {
                                !row_marks.left.contains(i) && !row_marks.filled.contains(i)
                            })
                            .count()
                    } else {
                        0
                    };
                    (
                        CardState::Reviewed,
                        with_fill_resin(
                            mark_rows(
                                review_rows(
                                    findings,
                                    &held.hollows,
                                    &held.fixed,
                                    fixable,
                                    held.after_fix_all,
                                ),
                                findings,
                                &row_marks,
                            ),
                            findings,
                            &held.hollows,
                            fills,
                        ),
                        summary_with_fixed(
                            summary_with_left(review_summary(&visible), row_marks.left.len()),
                            held.fixed.len(),
                        ),
                        worst_severity(&visible),
                        None,
                    )
                }
                ReviewOutcome::Failed(why) => (
                    CardState::Failed,
                    Vec::new(),
                    String::new(),
                    -1,
                    Some(ctr!("Review", "The checks could not run: {}", why)),
                ),
                // A cancelled review cancels its slice: none lands with it.
                ReviewOutcome::Cancelled => {
                    (CardState::NotReviewed, Vec::new(), String::new(), -1, None)
                }
            },
        };
        ui.set_review_state(state as i32);
        ui.set_review_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_review_summary(summary.into());
        ui.set_review_worst(worst);
        ui.set_review_stale(sliced && self.is_stale());
        ui.set_review_message(message.or(failure).unwrap_or_default());
        ui.set_review_notes(notes as i32);
        ui.set_review_notes_open(self.notes_open.get());
        ui.set_review_fill_small(small as i32);
        ui.set_review_fix_all(fix_all as i32);
        ui.set_review_fixing(self.fixing());
        self.show_fix_progress();
        ui.set_review_fills_note(
            fills_note(marks.fill_count(), marks.cured_count(), marks.pending()).into(),
        );
        ui.set_review_fills_clearable(marks.fill_count() > 0);
        ui.set_review_fills_clear_label(ctr!("Review", "Clear fills"));
    }

    /// The finding with index `id` in the held review.
    fn finding(&self, id: i32) -> Option<Finding> {
        let held = self.held.borrow();
        let ReviewOutcome::Reviewed { findings, .. } = &held.as_ref()?.outcome else {
            return None;
        };
        findings.get(usize::try_from(id).ok()?).cloned()
    }

    /// Moves the layer slider to the finding's first layer, centres the
    /// camera on its focus point and highlights its pixels on the cut and in
    /// the slice view (#409), on every layer it spans, until another finding
    /// is clicked or the card goes.
    fn jump_to(&self, id: i32) {
        let (Some(ui), Some(finding)) = (self.ui.upgrade(), self.finding(id)) else {
            return;
        };
        let layers = self.document.slice_layer_count().unwrap_or(0) as usize;
        let layer = finding.first_layer.min(layers.saturating_sub(1));
        ui.set_selected_layer_index(layer as f32);
        self.ctrl.on_selected_layer_changed();
        self.ctrl.set_review_highlight(finding.footprint.clone());
        let at = world(finding.focus);
        self.ctrl
            .centre_camera_on(glam::Vec3::from(at), JUMP_RADIUS_MM);
    }

    /// Applies the finding's suggested fix, and tells how it went.
    fn apply_fix(self: &Rc<Self>, id: i32) {
        // Fix All's run settles first: an edit now would drop it.
        if self.fixing() {
            return;
        }
        let Some(finding) = self.finding(id) else {
            return;
        };
        let index = id as usize;
        let applies = self
            .held
            .borrow()
            .as_ref()
            .is_some_and(|held| self.fixes_apply(held));
        if finding.fix.is_some() && !applies {
            self.push(Some(ctr!(
                "Review",
                "The models changed since this slice. Slice again before applying a fix."
            )));
            return;
        }
        let message = match finding.fix {
            Some(SuggestedFix::AddSupport { at }) => {
                let key = FindingKey::of(&finding);
                let tips = self.tips_of(&finding, at);
                let Some((first, _)) = tips.first() else {
                    return;
                };
                let first = *first;
                let points: Vec<[f32; 3]> = tips.iter().map(|tip| tip.0).collect();
                // A model with no set asks for its preset first (#444).
                if self.supports.makes_a_set(&points) && !self.supports.preset_named() {
                    self.with_preset(&points, move |panel| panel.apply_fix(id));
                    return;
                }
                // The first tip holds the island, or none of them goes in;
                // each tip of a region counts by itself.
                let plans: Vec<PointPlan> = tips
                    .into_iter()
                    .enumerate()
                    .map(|(k, (_, plan))| PointPlan {
                        after: (k > 0 && plan.holds.is_none()).then_some(0),
                        ..plan
                    })
                    .collect();
                match self.supports.add_tips_at(&points, Arc::new(plans)) {
                    // The tip that starts it stands, or none of it holds.
                    (HandEdit::Applied, refused) if refused.iter().any(|(i, _)| *i == 0) => {
                        let why = refused[0].1.clone();
                        if let Some(models) = self.markable_models() {
                            let on = self.document.scene_revision();
                            self.marks.borrow_mut().refuse(key, &why, on, &models);
                        }
                        crate::breadcrumbs::note(&format!(
                            "review: first tip refused at {first:?}"
                        ));
                        Some(ctr!("Review", "The support could not be added: {}", why))
                    }
                    (HandEdit::Applied, _) => {
                        if let Some(models) = self.markable_models() {
                            self.marks.borrow_mut().supported(key, &models);
                        }
                        self.mark_fixed(index);
                        Some(ctr!(
                            "Review",
                            "Support added. Slice again to check the result."
                        ))
                    }
                    (HandEdit::Failed(why), refused) => {
                        // The engine refused the point (not, say, a run in
                        // progress): the row keeps why.
                        if !refused.is_empty()
                            && let Some(models) = self.markable_models()
                        {
                            let on = self.document.scene_revision();
                            self.marks.borrow_mut().refuse(key, &why, on, &models);
                        }
                        Some(ctr!("Review", "The support could not be added: {}", why))
                    }
                    (HandEdit::Notified, _) => None,
                }
            }
            // A small trap offers its fill, unless a hollow made here holds
            // it: that row's button is the drain tool, as a large cavity's.
            Some(SuggestedFix::Fill { at, .. }) if self.hollow.hollow_at(world(at)).is_none() => {
                self.toggle_fill(id);
                return;
            }
            Some(SuggestedFix::AddDrain { at } | SuggestedFix::Fill { at, .. }) => {
                if self.open_drain_tool(world(at)) {
                    return;
                }
                Some(ctr!(
                    "Review",
                    "This cavity is not in a model hollowed here, so the drain cannot be drilled."
                ))
            }
            Some(SuggestedFix::ClearRaft { lift_to_mm, .. }) => {
                if self.supports.lift_clear_of_raft(lift_to_mm) {
                    self.mark_fixed(index);
                    // Supports that were in force are made again on their
                    // own (#153).
                    Some(if self.supports.regenerating() {
                        ctr!(
                            "Review",
                            "Model lifted clear of the raft. Its supports are being generated again; slice once they are in."
                        )
                    } else {
                        ctr!(
                            "Review",
                            "Model lifted clear of the raft. Generate the supports again, then slice."
                        )
                    })
                } else {
                    Some(ctr!(
                        "Review",
                        "The model already stands clear of the raft."
                    ))
                }
            }
            None => return,
        };
        self.push(message);
    }

    /// The tips of the island `finding`, whose fix points at `at`, each with
    /// what is known of it.
    fn tips_of(&self, finding: &Finding, at: PlatePoint) -> Vec<FindingTip> {
        let held = self.held.borrow();
        match held.as_ref().map(|held| &held.outcome) {
            Some(ReviewOutcome::Reviewed {
                islands,
                growths,
                tips,
                frame,
                ..
            }) => IslandTips {
                islands,
                growths,
                plan: tips.as_ref(),
                frame: Some(frame),
            }
            .of(finding, at),
            _ => vec![(world(at), PointPlan::default())],
        }
    }

    /// The second fix of a model inside the raft (#131): turns the raft off
    /// in the support options.
    fn turn_raft_off(&self, id: i32) {
        if self.fixing() {
            return;
        }
        let Some(finding) = self.finding(id) else {
            return;
        };
        if !matches!(finding.fix, Some(SuggestedFix::ClearRaft { .. })) {
            return;
        }
        self.supports.turn_raft_off();
        self.mark_fixed(id as usize);
        self.push(Some(ctr!(
            "Review",
            "Raft turned off. Generate the supports again, then slice."
        )));
    }

    /// The contact of the tips a plate with no set would get (#444).
    pub(crate) fn born_tip_mm(&self) -> f64 {
        self.supports.born_tip_mm()
    }

    /// Runs `then`, once a preset is picked if `points` make a set: a model
    /// with no set starts one with its first tip (#444). Nothing happens if
    /// the question is dismissed.
    fn with_preset(self: &Rc<Self>, points: &[[f32; 3]], then: impl FnOnce(&Rc<Self>) + 'static) {
        if !self.supports.makes_a_set(points) {
            then(self);
            return;
        }
        let (names, current) = self.supports.preset_choice();
        let weak = Rc::downgrade(self);
        self.ctrl
            .dialogs()
            .show_choose_preset(names, current, move |chosen| {
                if let Some(panel) = weak.upgrade() {
                    panel.supports.use_preset_for_new_sets(chosen);
                    then(&panel);
                }
            });
    }

    /// The top of the raft the scene's supports stand on, for the review of
    /// the next slice (#131).
    pub(crate) fn raft_top_mm(&self) -> Option<f64> {
        self.supports.raft_top_mm()
    }

    fn mark_fixed(&self, index: usize) {
        if let Some(held) = self.held.borrow_mut().as_mut() {
            held.fixed.insert(index);
        }
    }
}

/// Frees `slice` off the UI thread: the millions of rings of a heavy scene
/// take some 80 ms to free, and the window would stand still for it.
fn let_go(slice: SliceResult) {
    // With no thread to be had it goes here, with the closure.
    let _ = std::thread::Builder::new()
        .name("slice-release".to_owned())
        .spawn(move || drop(slice));
}

/// A plate point as the scene's world coordinates: the same frame, in single
/// precision.
fn world(point: PlatePoint) -> [f32; 3] {
    [point.x_mm, point.y_mm, point.z_mm].map(|c| c as f32)
}

/// Whether two support points are the same spot, found twice.
fn same_point(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

/// The UI code of a severity: 0 error, 1 warning, 2 info.
fn severity_code(severity: Severity) -> i32 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}

/// The worst severity among `findings` as its UI code, or -1 for none.
fn worst_severity(findings: &[Finding]) -> i32 {
    findings
        .iter()
        .map(|f| severity_code(f.severity))
        .min()
        .unwrap_or(-1)
}

/// The card's rows: errors, then warnings, then information, each group in
/// listing order (layer, then check), the first row of a group carrying its
/// heading. A row's `id` is the finding's index in `findings`. Unless
/// `fixable` (the models still stand as sliced), no row offers a fix; a row
/// whose fix was applied keeps its kind, for its mark. `after_fix_all`: the
/// slice Fix All asked for, so every row says why Fix All left it.
fn review_rows(
    findings: &[Finding],
    hollows: &[Option<usize>],
    fixed: &BTreeSet<usize>,
    fixable: bool,
    after_fix_all: bool,
) -> Vec<ReviewRow> {
    let mut order: Vec<usize> = (0..findings.len()).collect();
    // Stable: listing order stays within a severity.
    order.sort_by_key(|&i| severity_code(findings[i].severity));
    let mut rows = Vec::with_capacity(order.len());
    let mut group = None;
    for i in order {
        let finding = &findings[i];
        let code = severity_code(finding.severity);
        let header = if group == Some(code) {
            SharedString::default()
        } else {
            group = Some(code);
            group_heading(finding.severity)
        };
        let fix = match finding.fix {
            _ if !fixable && !fixed.contains(&i) => FixKind::None,
            None => FixKind::None,
            Some(SuggestedFix::AddSupport { .. }) => FixKind::AddSupport,
            // Offered through the row's fill, see `mark_rows`.
            Some(SuggestedFix::Fill { .. }) if hollow_drains(hollows, i).is_none() => FixKind::None,
            Some(SuggestedFix::AddDrain { .. } | SuggestedFix::Fill { .. }) => FixKind::AddDrain,
            Some(SuggestedFix::ClearRaft { .. }) => FixKind::ClearRaft,
        };
        let lift_label = match finding.fix {
            Some(SuggestedFix::ClearRaft { lift_to_mm }) => raft_lift_label(lift_to_mm),
            _ => SharedString::default(),
        };
        let drains = hollow_drains(hollows, i);
        // A hollow made here with no drain holes at all: the cavity is the
        // hollow, and what is missing is the drain holes (#158).
        let undrained = matches!(
            finding.fix,
            Some(SuggestedFix::AddDrain { .. } | SuggestedFix::Fill { .. })
        ) && drains == Some(0);
        // The drain tool reaches the cavities the Hollow tab carves: one
        // that came with the file may be out of its reach (#431).
        let imported =
            matches!(finding.fix, Some(SuggestedFix::AddDrain { .. })) && drains.is_none();
        let (title, detail) = if undrained {
            (
                ctr!("Review", "Hollow without drain holes"),
                ctr!(
                    "Review",
                    "This model is hollow, but its hollow has no drain holes, so its liquid resin stays inside. Add drain holes in the Hollow tab."
                ),
            )
        } else if imported {
            (
                finding.title.as_str().into(),
                format!(
                    "{} {}",
                    finding.detail,
                    ctr!(
                        "Review",
                        "This model was not hollowed here: the drain tool only reaches a cavity made in the Hollow tab, so it may not open this one."
                    )
                )
                .into(),
            )
        } else {
            (
                finding.title.as_str().into(),
                finding.detail.as_str().into(),
            )
        };
        rows.push(ReviewRow {
            id: i as i32,
            severity: code,
            header,
            title,
            layers: layers_label(finding.first_layer, finding.last_layer),
            detail,
            fix: fix as i32,
            fixed: fixed.contains(&i),
            // Offered for now; `mark_rows` says whether it is marked.
            fill: i32::from(fill_of(finding, hollow_drains(hollows, i)).is_some()),
            fill_resin: SharedString::default(),
            leave: false,
            lift_label,
            remains: if after_fix_all {
                fix_all_leaves(finding, hollow_drains(hollows, i))
            } else {
                SharedString::default()
            },
        });
    }
    rows
}

/// A tip of an island, and what is known of it.
type FindingTip = ([f32; 3], PointPlan);

/// What the review knows of where its islands' tips go (#437).
#[derive(Clone, Copy, Debug, Default)]
struct IslandTips<'r> {
    islands: &'r [Island],
    /// The regions of the review that grew at once.
    growths: &'r [Growth],
    /// The tips of `islands` and `growths`, `None` with no set to add them to.
    plan: Option<&'r TipPlan>,
    /// How their slice read the scene's points.
    frame: Option<&'r PlateFrame>,
}

impl IslandTips<'_> {
    /// The index in `islands` of the island `finding`, whose fix points at
    /// `at`.
    fn island(&self, finding: &Finding, at: PlatePoint) -> Option<usize> {
        self.islands
            .iter()
            .position(|island| island.first_layer == finding.first_layer && island.at == at)
    }

    /// The index in `growths` of the region `finding`, whose fix points at
    /// `at`.
    fn growth(&self, finding: &Finding, at: PlatePoint) -> Option<usize> {
        self.growths
            .iter()
            .position(|growth| growth.layer == finding.first_layer && growth.at == at)
    }

    /// What a tip of a region must do, each of them by itself.
    fn hold(&self, start: &Arc<sc_review::IslandStart>) -> TipHold {
        TipHold {
            start: start.clone(),
            frame: self.frame.cloned().unwrap_or_default(),
        }
    }

    /// The tips of the finding whose fix points at `at`: an island's first
    /// one asked to hold it, a region's each asked to count for it. The one
    /// point, for a plate with no set.
    fn of(&self, finding: &Finding, at: PlatePoint) -> Vec<FindingTip> {
        if finding.check == CheckKind::SurfaceGrowth
            && let (Some(plan), Some(growth)) = (self.plan, self.growth(finding, at))
        {
            let hold = self.hold(&self.growths[growth].start);
            return plan
                .of_growth(growth)
                .map(|tip| {
                    let known = PointPlan {
                        others: tip.others.iter().copied().map(world).collect(),
                        holds: Some(hold.clone()),
                        after: None,
                    };
                    (world(tip.at), known)
                })
                .collect();
        }
        match (self.plan, self.island(finding, at)) {
            (Some(plan), Some(island)) => plan
                .of(island)
                .enumerate()
                .map(|(k, tip)| {
                    let hold = self.hold(&self.islands[island].start);
                    let plan = PointPlan {
                        others: tip.others.iter().copied().map(world).collect(),
                        holds: (k == 0).then_some(hold),
                        after: None,
                    };
                    (world(tip.at), plan)
                })
                .collect(),
            _ => vec![(world(at), PointPlan::default())],
        }
    }

    /// What each region of `findings` has and wants: the tips under what it
    /// gained, the tips it asks for, and whether a tip fits on it at all.
    fn growth_counts(&self, findings: &[Finding]) -> BTreeMap<usize, GrowthTips> {
        findings
            .iter()
            .enumerate()
            .filter_map(|(i, finding)| {
                let Some(SuggestedFix::AddSupport { at }) = finding.fix else {
                    return None;
                };
                let index = self.growth(finding, at)?;
                let growth = &self.growths[index];
                let counts = GrowthTips {
                    has: growth.tips_mm.len(),
                    wants: growth.wanted,
                    no_room: self.plan.is_some_and(|plan| plan.no_room.contains(&index)),
                };
                Some((i, counts))
            })
            .collect()
    }

    /// The findings of `findings` whose island no tip fits on.
    fn too_narrow(&self, findings: &[Finding]) -> BTreeSet<usize> {
        let Some(plan) = self.plan else {
            return BTreeSet::new();
        };
        findings
            .iter()
            .enumerate()
            .filter(|(_, finding)| match finding.fix {
                Some(SuggestedFix::AddSupport { at }) => self
                    .island(finding, at)
                    .is_some_and(|island| plan.too_narrow.contains(&island)),
                _ => false,
            })
            .map(|(i, _)| i)
            .collect()
    }
}

/// What Fix All does with a review (#156): the supports to add, by finding
/// and point, an island's first tip first, and the pixels to mark for the
/// next slice, by finding.
#[derive(Debug, Default)]
struct FixAllPlan {
    supports: Vec<(usize, [f32; 3])>,
    /// What is known of each of `supports`, by its place there.
    plans: Vec<PointPlan>,
    patches: Vec<(usize, CavityFill)>,
}

impl FixAllPlan {
    /// The places in `supports` of the points to place, each point once: an
    /// island that is also a part above it is the same spot, found twice.
    fn unique_supports(&self) -> Vec<usize> {
        let mut unique: Vec<usize> = Vec::new();
        for (k, &(_, at)) in self.supports.iter().enumerate() {
            let twin = unique
                .iter()
                .any(|&other| same_point(self.supports[other].1, at));
            if !twin {
                unique.push(k);
            }
        }
        unique
    }

    /// Where the supports go, each point once.
    fn support_points(&self) -> Vec<[f32; 3]> {
        let unique = self.unique_supports();
        unique.iter().map(|&k| self.supports[k].1).collect()
    }

    /// What is known of each of [`Self::support_points`]: the extra tips
    /// of an island go with its first one, which holds it or none goes in. A
    /// tip that must do something itself goes by itself.
    fn support_plans(&self) -> PointPlans {
        let unique = self.unique_supports();
        let of = |place: usize| self.supports[unique[place]].0;
        let plans = (0..unique.len()).map(|place| {
            let plan = self.plans.get(unique[place]).cloned().unwrap_or_default();
            let first = (0..place).find(|&other| of(other) == of(place));
            PointPlan {
                after: first.filter(|_| plan.holds.is_none()),
                ..plan
            }
        });
        Arc::new(plans.collect())
    }

    /// The points that decide for their finding's row: its first one, and
    /// any other that must do something itself.
    fn row_points(&self) -> impl Iterator<Item = (usize, [f32; 3])> + '_ {
        self.supports.iter().enumerate().filter_map(|(k, support)| {
            let first = !self.supports[..k]
                .iter()
                .any(|(other, _)| *other == support.0);
            let holds = self.plans.get(k).is_some_and(|plan| plan.holds.is_some());
            (first || holds).then_some(*support)
        })
    }

    /// The first tip of each finding that gets supports: what holds it or
    /// not.
    fn first_tips(&self) -> impl Iterator<Item = (usize, [f32; 3])> + '_ {
        self.supports
            .iter()
            .enumerate()
            .filter(|(k, (i, _))| !self.supports[..*k].iter().any(|(other, _)| other == i))
            .map(|(_, support)| *support)
    }

    /// How many findings it fixes.
    fn len(&self) -> usize {
        self.first_tips().count() + self.patches.len()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What a Fix All run works on, kept while its supports are placed off
/// the UI thread (#390).
struct FixAllWork {
    plan: FixAllPlan,
    /// The models the marks are made on: as the review saw them.
    models: Vec<ModelPose>,
    /// Where the supports go, each point once.
    points: Vec<[f32; 3]>,
    /// What is known of each of them.
    plans: PointPlans,
    /// The slice generation of the review the plan was made from.
    generation: Option<u64>,
    /// The scene the plan's supports are placed on: a refusal is kept with
    /// it.
    revision: SceneRevision,
}

/// What the card says of a run dropped before its supports were in: nothing
/// of it was made.
fn dropped_message(kind: RunKind, reason: EditDropped) -> SharedString {
    if kind == RunKind::Islands {
        return match reason {
            EditDropped::Cancelled => {
                ctr!(
                    "Review",
                    "Adding supports was cancelled; nothing was changed."
                )
            }
            EditDropped::Undone => ctr!(
                "Review",
                "Adding supports was undone before it was done; nothing was changed."
            ),
            EditDropped::SceneChanged => ctr!(
                "Review",
                "The scene changed while the supports were added; nothing was changed."
            ),
        };
    }
    match reason {
        EditDropped::Cancelled => ctr!("Review", "Fix All was cancelled; nothing was changed."),
        EditDropped::Undone => ctr!(
            "Review",
            "Fix All was undone before it was done; nothing was changed."
        ),
        EditDropped::SceneChanged => ctr!(
            "Review",
            "The scene changed while Fix All ran; nothing was changed."
        ),
    }
}

/// Fix All's plan for `findings` and the `fills` their fixes name, without
/// the findings `skip` names: the tips of every island, as `tips` says, and
/// a fill in every small resin trap outside the `hollows` made here (as "Fill
/// all small traps"). Everything else is left, and with it every hollow,
/// suction cup and large cavity (#431).
fn plan_fix_all(
    findings: &[Finding],
    fills: &[CavityFill],
    hollows: &[Option<usize>],
    tips: IslandTips<'_>,
    skip: impl Fn(usize) -> bool,
) -> FixAllPlan {
    let mut plan = FixAllPlan::default();
    for (i, finding) in findings.iter().enumerate() {
        if skip(i) {
            continue;
        }
        match finding.fix {
            Some(SuggestedFix::AddSupport { at }) => {
                for (point, known) in tips.of(finding, at) {
                    plan.supports.push((i, point));
                    plan.plans.push(known);
                }
            }
            // Small traps, as "Fill all small traps".
            Some(SuggestedFix::Fill { .. }) => {
                let cavity = fill_of(finding, hollow_drains(hollows, i));
                if let Some(fill) = cavity.and_then(|cavity| fills.get(cavity)) {
                    plan.patches.push((i, fill.clone()));
                }
            }
            _ => {}
        }
    }
    plan
}

/// Why Fix All left `finding`, for the review of the slice it asked for.
/// `drains`: the drain holes of the hollow made here that holds its cavity.
fn fix_all_leaves(finding: &Finding, drains: Option<usize>) -> SharedString {
    match (finding.check, finding.fix) {
        (_, Some(SuggestedFix::AddDrain { .. } | SuggestedFix::Fill { .. }))
            if fill_of(finding, drains).is_none() =>
        {
            ctr!(
                "Review",
                "Left by Fix All: drain holes are placed by hand, in the Hollow tab."
            )
        }
        (CheckKind::ResinTrap | CheckKind::SuctionCup, _) => ctr!(
            "Review",
            "Left by Fix All: this cavity needs an opening, which only a change to the model makes."
        ),
        (CheckKind::ModelInRaft, _) => ctr!(
            "Review",
            "Left by Fix All: lift the model or turn the raft off with this finding's buttons."
        ),
        (CheckKind::CleanUp, _) => ctr!("Review", "Done by the review itself: nothing to fix."),
        (CheckKind::SurfaceGrowth, _) => ctr!(
            "Review",
            "Left by Fix All: more tips here are placed by hand, in the Supports tab."
        ),
        (CheckKind::PlateEdge | CheckKind::TooTall | CheckKind::EmptyLayer, _) => ctr!(
            "Review",
            "Left by Fix All: it does not move or resize models. Arrange the plate by hand."
        ),
        (_, Some(SuggestedFix::AddSupport { .. })) => ctr!(
            "Review",
            "Left by Fix All: no support held here. Add one by hand."
        ),
        _ => ctr!(
            "Review",
            "Left by Fix All: it could not fix this. Change the model by hand."
        ),
    }
}

/// `findings` as the card shows them: an island whose support the engine
/// refused on the scene as it stands reads "No support fits here", and it
/// is a warning, not an error (#235): nothing more can be done for it from
/// the review, and the row says why. One that never grows larger than
/// [`NO_SUPPORT_NOTE_MM2`] is a note (#163): too small to be worth a support
/// where none fits.
fn no_support_fits(findings: &[Finding], marks: &RowMarks) -> Vec<Finding> {
    findings
        .iter()
        .enumerate()
        .map(|(i, finding)| {
            let mut finding = finding.clone();
            if marks.held_back.contains(&i)
                && marks.refused.contains_key(&i)
                && finding.check == CheckKind::Island
            {
                finding.title = ctr!("Review", "No support fits here").to_string();
                finding.severity = if marks.too_small.contains(&i) {
                    Severity::Info
                } else {
                    finding.severity.max(Severity::Warning)
                };
            }
            finding
        })
        .collect()
}

/// Whether the support `finding` asks for is held back on the scene at
/// `now`, by Fix All and by the finding's row alike (#163): it went in and
/// did not hold, or it was refused on this very scene.
fn support_held_back<P: PartialEq + Clone>(
    finding: &Finding,
    marks: &ReviewMarks<P>,
    now: SceneRevision,
) -> bool {
    matches!(finding.fix, Some(SuggestedFix::AddSupport { .. }))
        && !marks.support_offered(&FindingKey::of(finding), now)
}

/// The drain holes of the hollow made here that holds finding `i`'s cavity.
fn hollow_drains(hollows: &[Option<usize>], i: usize) -> Option<usize> {
    hollows.get(i).copied().flatten()
}

/// The tips of a region that grew at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GrowthTips {
    has: usize,
    wants: usize,
    /// No tip fits on what it gained.
    no_room: bool,
}

/// What the question that ends a slice opens with: the islands and the
/// regions it offers tips for.
fn question_heading(islands: usize, regions: usize) -> SharedString {
    let islands = (islands > 0).then(|| {
        ctr!(
            "AddSupportsDialog",
            "One island has no support.",
            "{n} islands have no support.",
            islands
        )
    });
    let regions = (regions > 0).then(|| {
        ctr!(
            "AddSupportsDialog",
            "One region grows at once.",
            "{n} regions grow at once.",
            regions
        )
    });
    let parts: Vec<SharedString> = islands.into_iter().chain(regions).collect();
    parts
        .iter()
        .map(SharedString::as_str)
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

/// What the marks make of each finding of the held review, by index.
#[derive(Debug, Default)]
struct RowMarks {
    /// Findings whose cavity is marked to fill.
    filled: BTreeSet<usize>,
    /// Findings left alone: hidden.
    left: BTreeSet<usize>,
    /// Findings whose support the engine refused, with why.
    refused: BTreeMap<usize, SharedString>,
    /// Findings found again after a support went in: it did not hold them.
    unheld: BTreeSet<usize>,
    /// Findings whose support is not offered again, by their row as by Fix
    /// All (#163): it did not hold, or it was refused on this very scene.
    held_back: BTreeSet<usize>,
    /// Islands no support reaches that never grow larger than
    /// [`NO_SUPPORT_NOTE_MM2`]: notes, not warnings (#163).
    too_small: BTreeSet<usize>,
    /// Islands that never get as wide as a tip: none goes on them (#437).
    no_tip: BTreeSet<usize>,
    /// What the regions that grew at once have and want.
    growth_tips: BTreeMap<usize, GrowthTips>,
    /// Whether fills can be marked: the models stand as sliced.
    fixable: bool,
    /// Whether the notes are unfolded.
    notes_open: bool,
}

impl RowMarks {
    fn of<P: PartialEq + Clone>(
        findings: &[Finding],
        marks: &ReviewMarks<P>,
        fixable: bool,
        notes_open: bool,
        now: SceneRevision,
    ) -> Self {
        let mut row_marks = Self {
            fixable,
            notes_open,
            ..Self::default()
        };
        for (i, finding) in findings.iter().enumerate() {
            let key = FindingKey::of(finding);
            if marks.is_filled(&key) {
                row_marks.filled.insert(i);
            }
            if marks.is_left(&key) {
                row_marks.left.insert(i);
            }
            if let Some(why) = marks.refusal(&key) {
                row_marks.refused.insert(i, why.into());
            } else if marks.was_supported(&key) {
                row_marks.unheld.insert(i);
            }
            if support_held_back(finding, marks, now) {
                row_marks.held_back.insert(i);
            }
            if row_marks.held_back.contains(&i)
                && row_marks.refused.contains_key(&i)
                && finding.check == CheckKind::Island
                && finding
                    .largest_mm2
                    .is_some_and(|mm2| mm2 <= NO_SUPPORT_NOTE_MM2 + 1e-12)
            {
                row_marks.too_small.insert(i);
            }
        }
        row_marks
    }
}

/// The rows of [`review_rows`] with the marks applied: left findings and,
/// unless unfolded, the notes are taken out; a finding offering a fill says
/// whether it is marked (`fill` 2) or can be (`fill` 1, while `fixable`);
/// an island, a cavity or a sudden growth can be ignored, while `fixable`; a
/// support held back is not offered (#163). Each severity's first remaining
/// row carries its heading.
fn mark_rows(rows: Vec<ReviewRow>, findings: &[Finding], marks: &RowMarks) -> Vec<ReviewRow> {
    let mut group = None;
    rows.into_iter()
        .filter(|row| {
            let i = row.id as usize;
            !row.fixed
                && !marks.left.contains(&i)
                && (marks.notes_open || findings[i].severity != Severity::Info)
        })
        .map(|mut row| {
            let i = row.id as usize;
            let finding = &findings[i];
            if row.fill > 0 {
                row.fill = if marks.filled.contains(&i) {
                    2
                } else if marks.fixable {
                    1
                } else {
                    0
                };
            }
            row.leave = marks.fixable
                && row.fill != 2
                && matches!(
                    finding.check,
                    CheckKind::Island
                        | CheckKind::ResinTrap
                        | CheckKind::SuctionCup
                        | CheckKind::SurfaceGrowth
                );
            let growth = marks.growth_tips.get(&i);
            if (marks.held_back.contains(&i)
                || marks.no_tip.contains(&i)
                || growth.is_some_and(|tips| tips.no_room))
                && row.fix == FixKind::AddSupport as i32
            {
                row.fix = FixKind::None as i32;
            }
            if let Some(tips) = growth {
                row.remains = match marks.refused.get(&i) {
                    Some(why) => ctr!(
                        "Review",
                        "It has {} of the {} tips it wants. No support could be added for the rest: {}. Try a point nearby with the Add tool in the Supports tab.",
                        tips.has,
                        tips.wants,
                        why
                    ),
                    None if tips.no_room || marks.unheld.contains(&i) => ctr!(
                        "Review",
                        "It has {} of the {} tips it wants: no more fit on what it gained. Place them by hand in the Supports tab, or leave it.",
                        tips.has,
                        tips.wants
                    ),
                    None => row.remains.clone(),
                };
            } else if let (true, Some(mm2), Some(why)) = (
                marks.too_small.contains(&i),
                finding.largest_mm2,
                marks.refused.get(&i),
            ) {
                row.remains = ctr!(
                    "Review",
                    "No support could be added here: {}. It never grows past {} mm²: too small to be worth a support where none fits.",
                    why,
                    format!("{mm2:.3}")
                );
            } else if let Some(why) = marks.refused.get(&i) {
                row.remains = if marks.held_back.contains(&i) {
                    ctr!(
                        "Review",
                        "No support could be added here: {}. Try a point nearby with the Add tool in the Supports tab.",
                        why
                    )
                } else {
                    ctr!(
                        "Review",
                        "No support could be added here: {}. Supports were added since, so Fix All tries it again.",
                        why
                    )
                };
            } else if marks.no_tip.contains(&i) {
                row.remains = ctr!(
                    "Review",
                    "It never gets as wide as a support's tip, so none goes on it. Place one by hand in the Supports tab, or leave it."
                );
            } else if marks.unheld.contains(&i) {
                row.remains = ctr!(
                    "Review",
                    "A support was added here, but this still starts with nothing cured below it. Place one by hand in the Supports tab, or leave it."
                );
            }
            row.header = if group == Some(row.severity) {
                SharedString::default()
            } else {
                group = Some(row.severity);
                group_heading(finding.severity)
            };
            row
        })
        .collect()
}

/// The indices of the traps that offer a fill: those "Fill all small traps"
/// marks. Only a small sealed trap has one ([`sc_review::checks::traps`]).
fn small_traps<'f>(
    findings: &'f [Finding],
    hollows: &'f [Option<usize>],
) -> impl Iterator<Item = usize> + 'f {
    (0..findings.len()).filter(|&i| fill_of(&findings[i], hollow_drains(hollows, i)).is_some())
}

/// The fill finding `finding` offers, by index among the review's fills:
/// a small trap's, except in a model hollowed here (`drains`, its drain
/// holes), where the volume is the person's whatever its size (#431).
fn fill_of(finding: &Finding, drains: Option<usize>) -> Option<usize> {
    match finding.fix {
        Some(SuggestedFix::Fill { cavity, .. }) if drains.is_none() => Some(cavity),
        _ => None,
    }
}

/// The rows with the resin each offered fill cures ("0.67 ml").
fn with_fill_resin(
    mut rows: Vec<ReviewRow>,
    findings: &[Finding],
    hollows: &[Option<usize>],
    fills: &[CavityFill],
) -> Vec<ReviewRow> {
    for row in &mut rows {
        let i = row.id as usize;
        if let Some(fill) =
            fill_of(&findings[i], hollow_drains(hollows, i)).and_then(|c| fills.get(c))
        {
            row.fill_resin = format!("{:.2} ml", fill.volume_mm3() / 1000.0).into();
        }
    }
    rows
}

/// The summary, with how many findings are left alone.
fn summary_with_left(summary: String, left: usize) -> String {
    if left == 0 {
        return summary;
    }
    format!(
        "{summary} · {}",
        ctr!("Review", "{n} left alone", "{n} left alone", left)
    )
}

/// The summary, plus how many findings were fixed since the slice (they
/// left the list).
fn summary_with_fixed(summary: String, fixed: usize) -> String {
    if fixed == 0 {
        return summary;
    }
    format!(
        "{summary} · {}",
        ctr!("Review", "{n} fixed", "{n} fixed", fixed)
    )
}

/// The line about the fills: those marked for the next slice, or those the
/// held slice cured.
fn fills_note(marked: usize, cured: usize, pending: bool) -> String {
    match (pending, marked) {
        (true, 0) => ctr!("Review", "The fills are cleared. Slice again to apply.").to_string(),
        (true, n) => ctr!(
            "Review",
            "One cavity will be filled. Slice again to apply.",
            "{n} cavities will be filled. Slice again to apply.",
            n
        )
        .to_string(),
        (false, _) if cured > 0 => ctr!(
            "Review",
            "One cavity is filled in this slice.",
            "{n} cavities are filled in this slice.",
            cured
        )
        .to_string(),
        _ => String::new(),
    }
}

/// The heading of a severity's group.
fn group_heading(severity: Severity) -> SharedString {
    match severity {
        Severity::Error => ctr!("Review", "Errors"),
        Severity::Warning => ctr!("Review", "Warnings"),
        Severity::Info => ctr!("Review", "Notes"),
    }
}

/// "Layer 12" or "Layers 11–25", numbered from 1 as the slider shows them.
fn layers_label(first: usize, last: usize) -> SharedString {
    if first == last {
        ctr!("Review", "Layer {}", first + 1)
    } else {
        ctr!("Review", "Layers {}–{}", first + 1, last + 1)
    }
}

/// One line for the card's header: the count of each severity found, or
/// "No problems found".
fn review_summary(findings: &[Finding]) -> String {
    let count = |severity: Severity| findings.iter().filter(|f| f.severity == severity).count();
    let (errors, warnings, notes) = (
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Info),
    );
    let mut parts: Vec<SharedString> = Vec::new();
    if errors > 0 {
        parts.push(ctr!("Review", "{n} error", "{n} errors", errors));
    }
    if warnings > 0 {
        parts.push(ctr!("Review", "{n} warning", "{n} warnings", warnings));
    }
    if notes > 0 {
        parts.push(ctr!("Review", "{n} note", "{n} notes", notes));
    }
    if parts.is_empty() {
        return ctr!("Review", "No problems found").to_string();
    }
    parts
        .iter()
        .map(SharedString::as_str)
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
#[path = "tests_review_panel.rs"]
mod tests;

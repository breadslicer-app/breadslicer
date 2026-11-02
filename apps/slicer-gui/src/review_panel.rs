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
//! ([`DfSupportRun::add_supports_at`]). A cavity in a model hollowed in this
//! window offers "Add drain holes…", which opens the Hollow tab's drain tool
//! on that model, so the person picks where the holes go (#158); a hollow
//! with no drain holes at all is reported as just that.
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
//! Resin traps come in tiers by volume (#132). A tiny or medium trap, and a
//! small cup no drain helps, offer "Fill on the next slice": the mark keeps
//! the cavity's pixels ([`ReviewMarks`]), and the next slice cures them in
//! the preview, the printer file and its review. "Fill all small traps"
//! marks every trap offering a fill. A tiny trap can also be left alone,
//! which hides it until the scene changes. The notes (information) are
//! folded until opened. Moving or re-meshing a model drops every mark, as it
//! withdraws the fixes.
//!
//! "Fix all" (#156) resolves in one go every finding the app can fix by
//! itself, then slices again so the new review shows the result:
//!
//! - islands and floating parts get a support at their suggested point, all
//!   of them in one undoable edit ([`DfSupportRun::add_supports_at`]);
//! - the small resin traps are marked to fill, as "Fill all small traps"
//!   does;
//! - floating specks are marked to erase, and thin model features to
//!   thicken ([`PatchKind`]): the next slice removes the speck's pixels from
//!   the models' layers, or cures the feature's pixels grown a pixel on
//!   every side.
//!
//! The supports are placed off the UI thread (#390): on three minis that
//! takes seconds. The card shows how far it is, with Cancel; Cancel, Undo
//! or a change to the scene meanwhile drops the run, which leaves the scene
//! and the marks as they were. Once the supports are in, as one undoable
//! step, the marks are made and the slice starts, as when Fix All ran then
//! and there.
//!
//! Everything else stays as it is: a hollow without drain holes, suction
//! cups, the output checks, a model in the raft and the supports' own
//! findings. On the review that follows, each finding left says why Fix All
//! left it ([`fix_all_leaves`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::Duration;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_review::{CavityFill, CheckKind, Finding, Severity, SuggestedFix};
use sc_scene::NodeId;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::df_support_run::{
    DfSupportRun, EditDropped, EditWatch, HandEdit, PointsEdit, Refusals, raft_lift_label,
};
use crate::hollow_run::HollowRun;
use crate::i18n::ctr;
use crate::review::ReviewOutcome;
use crate::review_marks::{FindingKey, PatchKind, ReviewMarks, SlicePatches};
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
use crate::selection::{LeafId, Selection};
use crate::viewport::ViewportController;
use crate::{MainWindow, ReviewRow, WorkspaceSection};

/// How much of the plate a jump to a finding frames, in millimetres around
/// its focus point.
const JUMP_RADIUS_MM: f32 = 15.0;

/// How often the card reads how far Fix All's supports are.
const FIX_ALL_POLL: Duration = Duration::from_millis(100);

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
    /// For each finding's drain, by index: the drain holes of the hollow made
    /// here that holds its cavity, or `None` when no hollow made here does.
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
    /// Fix All's supports being placed off the UI thread (#390): how far
    /// the engine is, and the timer that shows it.
    fixing: RefCell<Option<Arc<EditWatch>>>,
    fix_poll: slint::Timer,
    /// Starts a slice without asking to overwrite the held one: Fix All
    /// promises one. Set by the Slice driver.
    reslice: RefCell<Option<Box<dyn Fn()>>>,
    scene_events: RefCell<Option<SceneSubscription>>,
}

impl ReviewPanel {
    /// Wires the card's callbacks and follows the document; the caller keeps
    /// the panel alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
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
            fixing: RefCell::new(None),
            fix_poll: slint::Timer::default(),
            reslice: RefCell::new(None),
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
        self.follow_models();
        self.marks.borrow().pending()
    }

    /// What the next slice cures and erases, with the marks' ids for
    /// [`fills_landed`](Self::fills_landed). Marks made on models that have
    /// changed since are dropped first.
    pub(crate) fn fills_for_slice(&self) -> SlicePatches {
        self.follow_models();
        self.marks.borrow().for_slice()
    }

    /// Lets Fix All start a slice: `reslice` starts one without asking to
    /// overwrite the held slice.
    pub(crate) fn set_reslice(&self, reslice: impl Fn() + 'static) {
        *self.reslice.borrow_mut() = Some(Box::new(reslice));
    }

    /// A slice is starting: it is Fix All's when Fix All asked for it.
    pub(crate) fn slice_started(&self, by_fix_all: bool) {
        self.fix_all_slicing.set(by_fix_all);
    }

    /// Records that the slice about to be shown cured the fills `ids`.
    pub(crate) fn fills_landed(&self, ids: Vec<u64>) {
        self.marks.borrow_mut().landed(ids);
    }

    /// Drops the marks when the models are no longer those they were made
    /// on, and shows the left findings again once the scene changed.
    fn follow_models(&self) {
        let models = self.current_models();
        let mut marks = self.marks.borrow_mut();
        marks.follow(models.as_ref());
        if self.is_stale() {
            marks.forget_left();
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
            match held.as_ref().map(|held| &held.outcome) {
                Some(ReviewOutcome::Reviewed {
                    findings, fills, ..
                }) => small_traps(findings)
                    .filter_map(|i| {
                        let Some(SuggestedFix::Fill { cavity, .. }) = findings[i].fix else {
                            return None;
                        };
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
    /// without the findings left alone, fixed already, marked, or whose
    /// support the engine refused or did not hold.
    fn fix_all_plan(&self) -> FixAllPlan {
        let held = self.held.borrow();
        let Some(held) = held.as_ref() else {
            return FixAllPlan::default();
        };
        let ReviewOutcome::Reviewed {
            findings,
            fills,
            erasures,
            thickenings,
            ..
        } = &held.outcome
        else {
            return FixAllPlan::default();
        };
        let marks = self.marks.borrow();
        plan_fix_all(findings, fills, erasures, thickenings, |i| {
            let key = FindingKey::of(&findings[i]);
            held.fixed.contains(&i)
                || marks.is_left(&key)
                || marks.is_marked(&key)
                || marks.refusal(&key).is_some()
                || marks.was_supported(&key)
        })
    }

    /// Fix All (#156): adds the supports in one undoable edit, marks the
    /// small traps to fill, the specks to erase and the thin features to
    /// thicken, then slices again. The supports are placed off the UI thread
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
        let Some(models) = self.markable_models() else {
            return;
        };
        let plan = self.fix_all_plan();
        if plan.is_empty() {
            return;
        }
        let points = plan.support_points();
        let generation = self.held.borrow().as_ref().map(|held| held.generation);
        let work = FixAllWork {
            plan,
            models,
            points,
            generation,
        };
        if work.points.is_empty() {
            self.fix_all_landed(&work, None);
            return;
        }
        // A hand edit of the supports being made would make the scene the
        // run starts from stale: Fix All waits for it.
        if self.supports.editing() {
            self.push(Some(ctr!(
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
        match self.supports.start_supports_at(&work.points, landing) {
            Ok(watch) => {
                *self.fixing.borrow_mut() = Some(watch);
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
        let points = plan.support_points();
        let supports = (!points.is_empty()).then(|| self.supports.add_supports_at(&points));
        let generation = self.held.borrow().as_ref().map(|held| held.generation);
        let work = FixAllWork {
            plan,
            models,
            points,
            generation,
        };
        self.fix_all_landed(&work, supports);
    }

    /// Whether Fix All's supports are being placed.
    pub(crate) fn fixing(&self) -> bool {
        self.fixing.borrow().is_some()
    }

    /// Cancel on the card while Fix All's supports are placed: nothing of
    /// Fix All reaches the scene or the marks.
    fn cancel_fix_all(&self) {
        self.supports.cancel_points_edit();
        // The supports' run tells how it ended; one gone without a word is
        // over all the same.
        if self.fixing() {
            self.stop_fixing();
            self.push(Some(dropped_message(EditDropped::Cancelled)));
        }
    }

    /// Fix All's supports are no longer being placed.
    fn stop_fixing(&self) {
        self.fix_poll.stop();
        *self.fixing.borrow_mut() = None;
    }

    /// Shows how far Fix All's supports are.
    fn show_fix_progress(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (tried, total) = self
            .fixing
            .borrow()
            .as_ref()
            .map_or((0, 0), |watch| watch.progress());
        ui.set_review_fix_progress(if total == 0 {
            0.0
        } else {
            tried as f32 / total as f32
        });
        ui.set_review_fix_phase(if total == 0 {
            SharedString::new()
        } else {
            ctr!(
                "Review",
                "Placing supports: {} of {}",
                tried.min(total),
                total
            )
        });
    }

    /// Fix All's supports are made, or the run was dropped.
    fn supports_landed(&self, work: &FixAllWork, edit: PointsEdit) {
        self.stop_fixing();
        match edit {
            PointsEdit::Done(outcome, refused) => {
                // The supports are in; a review that was replaced meanwhile
                // is no longer the one the marks would name.
                let same =
                    self.held.borrow().as_ref().map(|held| held.generation) == work.generation;
                if same {
                    self.fix_all_landed(work, Some((outcome, refused)));
                } else {
                    self.push(Some(ctr!(
                        "Review",
                        "The slice changed while Fix All ran. Slice again to review the result."
                    )));
                }
            }
            PointsEdit::Dropped(reason) => self.push(Some(dropped_message(reason))),
        }
    }

    /// The rest of Fix All once its supports are in, or failed, as
    /// `supports` tells (`None`: there were none to place): the refusals
    /// and the supports held are noted, the marks made, and the slice
    /// started.
    fn fix_all_landed(&self, work: &FixAllWork, supports: Option<(HandEdit, Refusals)>) {
        let FixAllWork {
            plan,
            models,
            points,
            ..
        } = work;
        let models = models.clone();
        let mut lines: Vec<String> = Vec::new();
        let mut changed = false;
        if let Some((outcome, refused)) = supports {
            // The findings at each point the engine refused keep why, so
            // their rows say it and Fix All does not offer them again.
            let refusal_at = |at: [f32; 3]| {
                refused
                    .iter()
                    .find(|(p, _)| same_point(points[*p], at))
                    .map(|(_, why)| why)
            };
            {
                let mut marks = self.marks.borrow_mut();
                for &(i, at) in &plan.supports {
                    if let Some(why) = refusal_at(at) {
                        let key =
                            FindingKey::of(&self.finding(i as i32).expect("a planned finding"));
                        marks.refuse(key, why, &models);
                    }
                }
            }
            match outcome {
                HandEdit::Applied => {
                    changed = true;
                    for &(i, at) in &plan.supports {
                        if refusal_at(at).is_none() {
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
                    if !refused.is_empty() {
                        lines.push(
                            ctr!(
                                "Review",
                                "No support can hold one point; its row says why.",
                                "No support can hold {n} points; their rows say why.",
                                refused.len()
                            )
                            .to_string(),
                        );
                    }
                }
                HandEdit::Failed(why) => lines
                    .push(ctr!("Review", "The supports could not be added: {}", why).to_string()),
                HandEdit::Notified => lines
                    .push(ctr!("Review", "No supports were added: see the notice.").to_string()),
            }
        }
        let mut counts = [0usize; 3];
        {
            let mut marks = self.marks.borrow_mut();
            for (i, kind, fill) in &plan.patches {
                let key = FindingKey::of(&self.finding(*i as i32).expect("a planned finding"));
                marks.mark(*kind, key, fill, &models);
                counts[*kind as usize] += 1;
            }
        }
        let [filled, thickened, erased] = counts;
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
        if erased > 0 {
            lines.push(
                ctr!(
                    "Review",
                    "One speck to erase.",
                    "{n} specks to erase.",
                    erased
                )
                .to_string(),
            );
        }
        if thickened > 0 {
            lines.push(
                ctr!(
                    "Review",
                    "One thin feature to thicken.",
                    "{n} thin features to thicken.",
                    thickened
                )
                .to_string(),
            );
        }
        changed |= filled + erased + thickened > 0;
        let reslice = self.reslice.borrow();
        if changed && let Some(reslice) = reslice.as_ref() {
            lines.push(ctr!("Review", "Slicing again…").to_string());
            self.push(Some(lines.join(" ").into()));
            reslice();
        } else {
            self.push(Some(lines.join(" ").into()));
        }
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
        if let Some(ui) = self.ui.upgrade() {
            ui.set_review_selected(-1);
        }
        self.push(None);
    }

    /// Opens the Hollow tab with its drain tool on, the hollowed model
    /// `leaf` selected, for the person to place the drain holes.
    fn open_drain_tool(&self, leaf: NodeId) {
        self.document.set_selection(Selection::One(LeafId(leaf)));
        self.ctrl.set_section(WorkspaceSection::Hollow);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_drain_tool(true);
        }
    }

    /// Shows the review of the slice the document has just stored.
    pub(crate) fn show(&self, outcome: ReviewOutcome) {
        let hollows = match &outcome {
            ReviewOutcome::Reviewed { findings, .. } => findings
                .iter()
                .map(|finding| match finding.fix {
                    Some(SuggestedFix::AddDrain { at, .. }) => {
                        self.hollow.hollow_at(world(at)).map(|(_, drains)| drains)
                    }
                    _ => None,
                })
                .collect(),
            ReviewOutcome::Failed(_) => Vec::new(),
        };
        let models = if self.is_stale() {
            None
        } else {
            self.current_models()
        };
        let after_fix_all = self.fix_all_slicing.replace(false);
        let left = match &outcome {
            ReviewOutcome::Reviewed { findings, .. } => findings.len(),
            ReviewOutcome::Failed(_) => 0,
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
        let nothing_left = self.fix_all_plan().is_empty();
        self.push((after_fix_all && left > 0).then(|| {
            if nothing_left {
                ctr!(
                    "Review",
                    "Fix All is done, and nothing is left to fix automatically. Each finding left says why it was left."
                )
            } else {
                ctr!(
                    "Review",
                    "Fix All is done. Each finding left says why it was left."
                )
            }
        }));
    }

    fn on_scene_event(&self, event: SceneEvent) {
        if !matches!(
            event,
            SceneEvent::SelectionChanged | SceneEvent::DirtyChanged
        ) {
            self.follow_models();
        }
        match event {
            SceneEvent::SliceChanged | SceneEvent::Installed | SceneEvent::Cleared => {
                let current = self.document.slice_generation();
                let gone = self.document.slice_layer_count().is_none()
                    || self
                        .held
                        .borrow()
                        .as_ref()
                        .is_some_and(|held| held.generation != current);
                if gone {
                    *self.held.borrow_mut() = None;
                }
                self.push(None);
            }
            SceneEvent::Mutated | SceneEvent::Restructured | SceneEvent::UndoStackChanged => {
                // Back on the sliced geometry (an undo of the fixes), the fixes
                // are undone too.
                if !self.is_stale()
                    && let Some(held) = self.held.borrow_mut().as_mut()
                {
                    held.fixed.clear();
                }
                self.push(None);
            }
            SceneEvent::SelectionChanged | SceneEvent::DirtyChanged => {}
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
                    findings, fills, ..
                } => {
                    let fixable = self.fixes_apply(held);
                    let row_marks = RowMarks::of(findings, &marks, fixable, self.notes_open.get());
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
                        small_traps(findings)
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
            fills_note(
                marks.fill_count(),
                marks.cured_count(),
                marks.pending(),
                marks.only_cavities(),
            )
            .into(),
        );
        ui.set_review_fills_clearable(marks.fill_count() > 0);
        ui.set_review_fills_clear_label(if marks.only_cavities() {
            ctr!("Review", "Clear fills")
        } else {
            ctr!("Review", "Clear fixes")
        });
    }

    /// The finding with index `id` in the held review.
    fn finding(&self, id: i32) -> Option<Finding> {
        let held = self.held.borrow();
        let ReviewOutcome::Reviewed { findings, .. } = &held.as_ref()?.outcome else {
            return None;
        };
        findings.get(usize::try_from(id).ok()?).cloned()
    }

    /// Moves the layer slider to the finding's first layer and centres the
    /// camera on its focus point.
    fn jump_to(&self, id: i32) {
        let (Some(ui), Some(finding)) = (self.ui.upgrade(), self.finding(id)) else {
            return;
        };
        let layers = self.document.slice_layer_count().unwrap_or(0) as usize;
        let layer = finding.first_layer.min(layers.saturating_sub(1));
        ui.set_selected_layer_index(layer as f32);
        self.ctrl.on_selected_layer_changed();
        let at = world(finding.focus);
        self.ctrl
            .centre_camera_on(glam::Vec3::from(at), JUMP_RADIUS_MM);
    }

    /// Applies the finding's suggested fix, and tells how it went.
    fn apply_fix(&self, id: i32) {
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
                match self.supports.add_supports_at(&[world(at)]) {
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
                            self.marks.borrow_mut().refuse(key, &why, &models);
                        }
                        Some(ctr!("Review", "The support could not be added: {}", why))
                    }
                    (HandEdit::Notified, _) => None,
                }
            }
            Some(SuggestedFix::Fill { .. }) => {
                self.toggle_fill(id);
                return;
            }
            // The person picks where the holes go: a drain drilled right at
            // the finding is rarely where it looks good (#158).
            // A cavity in a model not hollowed here is filled instead (#156).
            Some(SuggestedFix::AddDrain { at, .. }) => match self.hollow.hollow_at(world(at)) {
                Some((leaf, _)) => {
                    self.open_drain_tool(leaf);
                    return;
                }
                None => {
                    self.toggle_fill(id);
                    return;
                }
            },
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
            // Applied by Fix All only, which offers no button per row.
            Some(SuggestedFix::Erase { .. } | SuggestedFix::Thicken { .. }) | None => return,
        };
        self.push(message);
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

/// A plate point as the scene's world coordinates: the same frame, in single
/// precision.
fn world(point: sc_review::PlatePoint) -> [f32; 3] {
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
            Some(SuggestedFix::AddDrain { .. }) if hollow_drains(hollows, i).is_some() => {
                FixKind::AddDrain
            }
            // Offered through the row's fill, see `mark_rows`.
            Some(SuggestedFix::AddDrain { .. } | SuggestedFix::Fill { .. }) => FixKind::None,
            Some(SuggestedFix::ClearRaft { .. }) => FixKind::ClearRaft,
            // Applied by Fix All only.
            Some(SuggestedFix::Erase { .. } | SuggestedFix::Thicken { .. }) => FixKind::None,
        };
        let lift_label = match finding.fix {
            Some(SuggestedFix::ClearRaft { lift_to_mm }) => raft_lift_label(lift_to_mm),
            _ => SharedString::default(),
        };
        // A hollow made here with no drain holes at all: the cavity is the
        // hollow, and what is missing is the drain holes (#158).
        let undrained = matches!(finding.fix, Some(SuggestedFix::AddDrain { .. }))
            && hollow_drains(hollows, i) == Some(0);
        let (title, detail) = if undrained {
            (
                ctr!("Review", "Hollow without drain holes"),
                ctr!(
                    "Review",
                    "This model is hollow, but its hollow has no drain holes, so its liquid resin stays inside. Add drain holes in the Hollow tab."
                ),
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

/// What Fix All does with a review (#156): the supports to add, by finding
/// and point, and the pixels to mark for the next slice, by finding.
#[derive(Debug, Default)]
struct FixAllPlan {
    supports: Vec<(usize, [f32; 3])>,
    patches: Vec<(usize, PatchKind, CavityFill)>,
}

impl FixAllPlan {
    /// Where the supports go, each point once: an island that is also a
    /// floating part is the same spot, found twice.
    fn support_points(&self) -> Vec<[f32; 3]> {
        let mut points: Vec<[f32; 3]> = Vec::new();
        for &(_, at) in &self.supports {
            let twin = points.iter().any(|other| same_point(*other, at));
            if !twin {
                points.push(at);
            }
        }
        points
    }

    /// How many findings it fixes.
    fn len(&self) -> usize {
        self.supports.len() + self.patches.len()
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
    /// The slice generation of the review the plan was made from.
    generation: Option<u64>,
}

/// What the card says of a Fix All run dropped before its supports were
/// in: nothing of it was made.
fn dropped_message(reason: EditDropped) -> SharedString {
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

/// Fix All's plan for `findings` and the pixels their fixes name, without
/// the findings `skip` names: a support under every island and floating
/// part, a fill in every small resin trap (as "Fill all small traps"), an
/// erasure of every speck no support goes in and a thickening of every thin
/// model feature that no erasure touches. Everything else is left.
fn plan_fix_all(
    findings: &[Finding],
    fills: &[CavityFill],
    erasures: &[CavityFill],
    thickenings: &[CavityFill],
    skip: impl Fn(usize) -> bool,
) -> FixAllPlan {
    let mut plan = FixAllPlan::default();
    let mut thicken = Vec::new();
    for (i, finding) in findings.iter().enumerate() {
        if skip(i) {
            continue;
        }
        match finding.fix {
            Some(SuggestedFix::AddSupport { at }) => plan.supports.push((i, world(at))),
            // Small traps, as "Fill all small traps", and suction cups.
            Some(SuggestedFix::Fill { cavity, .. } | SuggestedFix::AddDrain { cavity, .. })
                if is_small_trap(finding) || finding.check == CheckKind::SuctionCup =>
            {
                if let Some(fill) = fills.get(cavity) {
                    plan.patches.push((i, PatchKind::Fill, fill.clone()));
                }
            }
            Some(SuggestedFix::Erase { speck, .. }) => {
                if let Some(erasure) = erasures.get(speck) {
                    plan.patches.push((i, PatchKind::Erase, erasure.clone()));
                }
            }
            Some(SuggestedFix::Thicken { feature, .. }) => {
                if let Some(thickening) = thickenings.get(feature) {
                    thicken.push((i, thickening));
                }
            }
            _ => {}
        }
    }
    // A speck where an island starts is the island's first layer: the
    // support holds it, and erasing it would leave the island one layer up
    // with nothing below it.
    plan.patches.retain(|(_, kind, erasure)| {
        *kind != PatchKind::Erase || !plan.supports.iter().any(|(_, at)| in_slab(erasure, *at))
    });
    // An erasure wins over a thickening of the same pixels: the speck goes.
    for (i, thickening) in thicken {
        let erased = plan
            .patches
            .iter()
            .any(|(_, kind, erasure)| *kind == PatchKind::Erase && erasure.overlaps(thickening));
        if !erased {
            plan.patches
                .push((i, PatchKind::Thicken, thickening.clone()));
        }
    }
    plan
}

/// Whether a slab of `fill` holds `at` (world mm).
fn in_slab(fill: &CavityFill, at: [f32; 3]) -> bool {
    let [x, y, z] = at.map(f64::from);
    fill.slabs.iter().any(|slab| {
        slab.bottom_mm <= z + 1e-6
            && z < slab.top_mm
            && slab
                .rects
                .iter()
                .any(|r| r.x0_mm <= x && x <= r.x1_mm && r.y0_mm <= y && y <= r.y1_mm)
    })
}

/// Why Fix All left `finding`, for the review of the slice it asked for.
/// `drains`: the drain holes of the hollow made here that holds its cavity.
fn fix_all_leaves(finding: &Finding, drains: Option<usize>) -> SharedString {
    match (finding.check, finding.fix) {
        (_, Some(SuggestedFix::AddDrain { .. })) if drains.is_some() => ctr!(
            "Review",
            "Left by Fix All: drain holes are placed by hand, in the Hollow tab."
        ),
        (CheckKind::ResinTrap, Some(SuggestedFix::Fill { .. } | SuggestedFix::AddDrain { .. })) => {
            ctr!(
                "Review",
                "Left by Fix All: it holds too much resin to fill without asking. Fill it from this row if the cavity is part of the model, or hollow the model in the Hollow tab and drill drain holes."
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
        (CheckKind::PlateEdge | CheckKind::TooTall | CheckKind::EmptyLayer, _) => ctr!(
            "Review",
            "Left by Fix All: it does not move or resize models. Arrange the plate by hand."
        ),
        _ if finding.scope == sc_review::RoleScope::Support => ctr!(
            "Review",
            "Left by Fix All: this is in the supports. Generate them again."
        ),
        (_, Some(SuggestedFix::Erase { .. })) => ctr!(
            "Review",
            "Left by Fix All: an island starts on this speck, so it got a support instead. Press Fix All again to erase it."
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

/// The drain holes of the hollow made here that holds finding `i`'s cavity.
fn hollow_drains(hollows: &[Option<usize>], i: usize) -> Option<usize> {
    hollows.get(i).copied().flatten()
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
        }
        row_marks
    }
}

/// The rows of [`review_rows`] with the marks applied: left findings and,
/// unless unfolded, the notes are taken out; a finding offering a fill says
/// whether it is marked (`fill` 2) or can be (`fill` 1, while `fixable`),
/// and a tiny one can be left alone. Each severity's first remaining row
/// carries its heading.
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
                row.leave = marks.fixable && finding.severity == Severity::Info;
            }
            if let Some(why) = marks.refused.get(&i) {
                row.remains = ctr!(
                    "Review",
                    "No support could be added here: {}. Try a point nearby with the Add tool in the Supports tab.",
                    why
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

/// The indices of the resin traps that offer a fill: those "Fill all small
/// traps" marks. Cups are left out: filling one changes the model's surface,
/// so it is marked one by one.
fn small_traps(findings: &[Finding]) -> impl Iterator<Item = usize> + '_ {
    findings
        .iter()
        .enumerate()
        .filter_map(|(i, finding)| is_small_trap(finding).then_some(i))
}

/// Whether `finding` is a resin trap small enough to fill without asking: a
/// note or a warning offering a fill. A large one offers a fill too, when
/// no drain helps it, but holds too much resin to fill unasked.
fn is_small_trap(finding: &Finding) -> bool {
    finding.check == CheckKind::ResinTrap
        && finding.severity != Severity::Error
        && matches!(finding.fix, Some(SuggestedFix::Fill { .. }))
}

/// The fill finding `finding` offers, by index among the review's fills:
/// every cavity's, except a drain's in a model hollowed here (`drains`, its
/// drain holes), where the hollow is on purpose.
fn fill_of(finding: &Finding, drains: Option<usize>) -> Option<usize> {
    match finding.fix {
        Some(SuggestedFix::Fill { cavity, .. }) => Some(cavity),
        Some(SuggestedFix::AddDrain { cavity, .. }) if drains.is_none() => Some(cavity),
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
/// held slice cured. Unless `only_cavities`, the marks include Fix All's
/// erasures and thickenings, and the line speaks of fixes.
fn fills_note(marked: usize, cured: usize, pending: bool, only_cavities: bool) -> String {
    if !only_cavities {
        return match (pending, marked) {
            (true, 0) => ctr!("Review", "The fixes are cleared. Slice again to apply.").to_string(),
            (true, n) => ctr!(
                "Review",
                "One fix will be applied. Slice again to apply.",
                "{n} fixes will be applied. Slice again to apply.",
                n
            )
            .to_string(),
            (false, _) if cured > 0 => ctr!(
                "Review",
                "One fix is applied in this slice.",
                "{n} fixes are applied in this slice.",
                cured
            )
            .to_string(),
            _ => String::new(),
        };
    }
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

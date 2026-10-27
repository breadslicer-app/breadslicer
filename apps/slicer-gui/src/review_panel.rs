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
//! ([`DfSupportRun::add_support_at`]), "Add drain here" through the Hollow
//! tab's drains, on a model hollowed in this window
//! ([`HollowRun::add_drain_at`]).
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

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_review::{CavityFill, CheckKind, Finding, Severity, SuggestedFix};
use sc_scene::NodeId;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::df_support_run::{DfSupportRun, HandEdit};
use crate::hollow_run::{DrainRequest, HollowRun};
use crate::i18n::ctr;
use crate::review::ReviewOutcome;
use crate::review_marks::{FindingKey, ReviewMarks};
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
use crate::viewport::ViewportController;
use crate::{MainWindow, ReviewRow};

/// How much of the plate a jump to a finding frames, in millimetres around
/// its focus point.
const JUMP_RADIUS_MM: f32 = 15.0;

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
    /// A drain would help, but no model hollowed in this window holds the
    /// cavity: the row says so instead of offering a button.
    DrainNeedsHollow = 3,
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
    /// Whether each finding's drain can be drilled here, by index.
    drainable: Vec<bool>,
    /// Findings whose fix was applied since the slice.
    fixed: BTreeSet<usize>,
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
            if let Some(panel) = weak.upgrade() {
                panel.marks.borrow_mut().clear_fills();
                panel.push(None);
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

    /// The fills the next slice cures, and their ids for
    /// [`fills_landed`](Self::fills_landed). Marks made on models that have
    /// changed since are dropped first.
    pub(crate) fn fills_for_slice(&self) -> (Vec<u64>, Vec<Arc<CavityFill>>) {
        self.follow_models();
        self.marks.borrow().for_slice()
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
        let finding = findings.get(usize::try_from(id).ok()?)?;
        let Some(SuggestedFix::Fill { cavity, .. }) = finding.fix else {
            return None;
        };
        Some((finding.clone(), fills.get(cavity)?.clone()))
    }

    /// Marks the finding's cavity to fill on the next slice, or unmarks it.
    fn toggle_fill(&self, id: i32) {
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

    /// Hides the finding until the scene changes.
    fn leave(&self, id: i32) {
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

    /// Shows the review of the slice the document has just stored.
    pub(crate) fn show(&self, outcome: ReviewOutcome) {
        let drainable = match &outcome {
            ReviewOutcome::Reviewed { findings, .. } => findings
                .iter()
                .map(|finding| match finding.fix {
                    Some(SuggestedFix::AddDrain { at }) => self.hollow.can_drain_at(world(at)),
                    _ => false,
                })
                .collect(),
            ReviewOutcome::Failed(_) => Vec::new(),
        };
        let models = if self.is_stale() {
            None
        } else {
            self.current_models()
        };
        *self.held.borrow_mut() = Some(HeldReview {
            generation: self.document.slice_generation(),
            models,
            outcome,
            drainable,
            fixed: BTreeSet::new(),
        });
        if let Some(ui) = self.ui.upgrade() {
            ui.set_review_selected(-1);
        }
        self.push(None);
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
        let (state, rows, summary, worst, failure) = match held.as_ref() {
            _ if !sliced => (CardState::Empty, Vec::new(), String::new(), -1, None),
            None => (CardState::NotReviewed, Vec::new(), String::new(), -1, None),
            Some(held) => match &held.outcome {
                ReviewOutcome::Reviewed { findings, .. } => {
                    let fixable = self.fixes_apply(held);
                    let row_marks = RowMarks::of(findings, &marks, fixable, self.notes_open.get());
                    let visible: Vec<Finding> = findings
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !row_marks.left.contains(i))
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
                        mark_rows(
                            review_rows(findings, &held.drainable, &held.fixed, fixable),
                            findings,
                            &row_marks,
                        ),
                        summary_with_left(review_summary(&visible), row_marks.left.len()),
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
        ui.set_review_fills_note(
            fills_note(marks.fill_count(), marks.cured_count(), marks.pending()).into(),
        );
        ui.set_review_fills_clearable(marks.fill_count() > 0);
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
                match self.supports.add_support_at(world(at)) {
                    HandEdit::Applied => {
                        self.mark_fixed(index);
                        Some(ctr!(
                            "Review",
                            "Support added. Slice again to check the result."
                        ))
                    }
                    HandEdit::Failed(why) => {
                        Some(ctr!("Review", "The support could not be added: {}", why))
                    }
                    HandEdit::Notified => None,
                }
            }
            Some(SuggestedFix::Fill { .. }) => {
                self.toggle_fill(id);
                return;
            }
            Some(SuggestedFix::AddDrain { at }) => match self.hollow.add_drain_at(world(at)) {
                DrainRequest::Started => {
                    self.mark_fixed(index);
                    Some(ctr!(
                        "Review",
                        "Hollowing the model again with the drain. Slice again when it is done."
                    ))
                }
                DrainRequest::Busy => Some(ctr!(
                    "Review",
                    "A model is being hollowed; try again when it is done."
                )),
                DrainRequest::NoHollow => Some(ctr!(
                    "Review",
                    "This cavity is not in a model hollowed here, so the drain cannot be drilled."
                )),
            },
            None => return,
        };
        self.push(message);
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
/// whose fix was applied keeps its kind, for its mark.
fn review_rows(
    findings: &[Finding],
    drainable: &[bool],
    fixed: &BTreeSet<usize>,
    fixable: bool,
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
            Some(SuggestedFix::AddDrain { .. }) if drainable.get(i).copied().unwrap_or(false) => {
                FixKind::AddDrain
            }
            Some(SuggestedFix::AddDrain { .. }) => FixKind::DrainNeedsHollow,
            // Offered through the row's fill, see `mark_rows`.
            Some(SuggestedFix::Fill { .. }) => FixKind::None,
        };
        rows.push(ReviewRow {
            id: i as i32,
            severity: code,
            header,
            title: finding.title.as_str().into(),
            layers: layers_label(finding.first_layer, finding.last_layer),
            detail: finding.detail.as_str().into(),
            fix: fix as i32,
            fixed: fixed.contains(&i),
            fill: 0,
            leave: false,
        });
    }
    rows
}

/// What the marks make of each finding of the held review, by index.
#[derive(Debug, Default)]
struct RowMarks {
    /// Findings whose cavity is marked to fill.
    filled: BTreeSet<usize>,
    /// Findings left alone: hidden.
    left: BTreeSet<usize>,
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
            !marks.left.contains(&i) && (marks.notes_open || findings[i].severity != Severity::Info)
        })
        .map(|mut row| {
            let i = row.id as usize;
            let finding = &findings[i];
            if matches!(finding.fix, Some(SuggestedFix::Fill { .. })) {
                row.fill = if marks.filled.contains(&i) {
                    2
                } else if marks.fixable {
                    1
                } else {
                    0
                };
                row.leave = marks.fixable && finding.severity == Severity::Info;
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
    findings.iter().enumerate().filter_map(|(i, finding)| {
        (finding.check == CheckKind::ResinTrap
            && matches!(finding.fix, Some(SuggestedFix::Fill { .. })))
        .then_some(i)
    })
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

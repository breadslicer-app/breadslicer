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

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_review::{Finding, Severity, SuggestedFix};
use sc_scene::NodeId;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::df_support_run::{DfSupportRun, HandEdit, raft_lift_label};
use crate::hollow_run::{DrainRequest, HollowRun};
use crate::i18n::ctr;
use crate::review::ReviewOutcome;
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
        let sliced = self.document.slice_layer_count().is_some();
        let (state, rows, summary, worst, failure) = match held.as_ref() {
            _ if !sliced => (CardState::Empty, Vec::new(), String::new(), -1, None),
            None => (CardState::NotReviewed, Vec::new(), String::new(), -1, None),
            Some(held) => match &held.outcome {
                ReviewOutcome::Reviewed { findings, .. } => (
                    CardState::Reviewed,
                    review_rows(
                        findings,
                        &held.drainable,
                        &held.fixed,
                        self.fixes_apply(held),
                    ),
                    review_summary(findings),
                    worst_severity(findings),
                    None,
                ),
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
            Some(SuggestedFix::ClearRaft { lift_to_mm, .. }) => {
                if self.supports.lift_clear_of_raft(lift_to_mm) {
                    self.mark_fixed(index);
                    Some(ctr!(
                        "Review",
                        "Model lifted clear of the raft. Generate the supports again, then slice."
                    ))
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

    /// The second fix of a model inside the raft (#131): turns the raft off
    /// in the support options.
    fn turn_raft_off(&self, id: i32) {
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
            Some(SuggestedFix::ClearRaft { .. }) => FixKind::ClearRaft,
        };
        let lift_label = match finding.fix {
            Some(SuggestedFix::ClearRaft { lift_to_mm }) => raft_lift_label(lift_to_mm),
            _ => SharedString::default(),
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
            lift_label,
        });
    }
    rows
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

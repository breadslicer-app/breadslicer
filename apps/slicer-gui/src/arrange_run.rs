/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Arrange all and Arrange selection (#180).
//!
//! The Transform tab's Arrange card packs the plate's objects by their real
//! footprints ([`sc_arrange`]): each model with its supports and raft, kept
//! a gap apart at every height (a part may reach over a shorter neighbour),
//! or by their whole shadow. Arrange selection finds a spot for the selected
//! model and leaves the others where they are.
//!
//! The footprints and the search run on a worker. The result lands as one
//! undoable edit that moves each model in the plate plane, maybe turned
//! about the vertical axis: its supports go along without being generated
//! again ([`crate::support_follow`]). A result worked out for a scene that
//! has changed since is dropped. Objects that do not fit go beside the
//! plate, and are named.
//!
//! Fill bed (#356, [`fill`]) leaves everything where it is and fills the
//! room left with copies of the selected models.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use sc_arrange::{Body, Outcome, Part, Plate, Settings};
use sc_geom::df_supports::is_any_support_mesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::scene_document::{EditNotApplied, SceneDocument, SceneRevision};
use crate::support_persist::{in_force, model_of};
use crate::viewport::ViewportController;

mod fill;

#[cfg(test)]
pub(crate) use fill::{copied_set_now, fill_now};

/// What the Arrange card sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ArrangeOptions {
    /// The smallest distance between two objects, at any height (mm).
    pub(crate) gap_mm: f32,
    /// The room kept clear along the plate's edges (mm).
    pub(crate) margin_mm: f32,
    /// Whether objects may turn about the vertical axis.
    pub(crate) turns: bool,
    /// Keep each object's whole shadow clear of the others.
    pub(crate) shadow_only: bool,
    /// Fill bed adds at most this many copies; 0 adds as many as fit.
    pub(crate) max_copies: u32,
}

impl Default for ArrangeOptions {
    fn default() -> Self {
        Self {
            gap_mm: 1.0,
            margin_mm: 5.0,
            turns: true,
            shadow_only: false,
            max_copies: 0,
        }
    }
}

impl ArrangeOptions {
    pub(crate) fn settings(self) -> Settings {
        Settings {
            gap: f64::from(self.gap_mm),
            margin: f64::from(self.margin_mm),
            shadow_only: self.shadow_only,
            turns: if self.turns { 8 } else { 1 },
            ..Settings::default()
        }
    }
}

/// The plate's objects as the arrangement sees them.
struct Gathered {
    bodies: Vec<Body>,
    /// Per body, the leaves its move moves: the model, and supports that
    /// do not follow it by themselves. A set in force follows its model.
    leaves: Vec<Vec<NodeId>>,
    /// Per body, the support leaves whose set in force follows its model.
    follows: Vec<Vec<NodeId>>,
    names: Vec<String>,
    /// Per body, the leaf that says which plate it stands on (#545): its
    /// model, or the supports no model owns.
    anchors: Vec<NodeId>,
}

/// A run in flight.
struct Pending {
    revision: SceneRevision,
    /// Per plate arranged (#545), each body's leaves and name.
    leaves: Vec<Vec<Vec<NodeId>>>,
    names: Vec<Vec<String>>,
    started: Instant,
}

/// Which plates an Arrange press works on (#545).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrangeScope {
    /// Every plate with objects, each on its own: the card's buttons and A.
    Plates,
    /// The active plate alone: its own tool.
    ActivePlate,
}

/// The worker's outcome: one per plate arranged.
type ArrangeSlot = Arc<Mutex<Option<Result<Option<Vec<Outcome>>, String>>>>;

pub(crate) struct ArrangeRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    ctrl: Rc<ViewportController>,
    options: Cell<ArrangeOptions>,
    slot: ArrangeSlot,
    pending: RefCell<Option<Pending>>,
    cancel: Arc<AtomicBool>,
    fill: fill::Fill,
}

impl ArrangeRun {
    /// Wires the Arrange card's callbacks and returns the driver, which the
    /// caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            ctrl,
            options: Cell::new(ArrangeOptions::default()),
            slot: Arc::new(Mutex::new(None)),
            pending: RefCell::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            fill: fill::Fill::default(),
        });
        run.push_options(ui);
        {
            let run = run.clone();
            ui.on_arrange_requested(move |selection| run.start(selection, ArrangeScope::Plates));
        }
        {
            // The active plate's own Arrange (#545): all of its objects.
            let run = run.clone();
            ui.on_plate_arrange_requested(move || run.start(false, ArrangeScope::ActivePlate));
        }
        {
            let run = run.clone();
            ui.on_arrange_finished(move || run.finish());
        }
        {
            let run = run.clone();
            ui.on_arrange_option_changed(move |code, value| run.on_option(code, value));
        }
        {
            let run = run.clone();
            ui.on_arrange_cancel(move || run.cancel());
        }
        {
            let run = run.clone();
            ui.on_arrange_fill_requested(move || run.start_fill());
        }
        run
    }

    fn push_options(&self, ui: &MainWindow) {
        let o = self.options.get();
        ui.set_arrange_gap(o.gap_mm);
        ui.set_arrange_margin(o.margin_mm);
        ui.set_arrange_turns(o.turns);
        ui.set_arrange_shadow_only(o.shadow_only);
        ui.set_arrange_max_copies(o.max_copies as f32);
    }

    fn on_option(&self, code: i32, value: f32) {
        let mut o = self.options.get();
        match code {
            0 => o.gap_mm = value.clamp(0.0, 20.0),
            1 => o.margin_mm = value.clamp(0.0, 50.0),
            2 => o.turns = value != 0.0,
            3 => o.shadow_only = value != 0.0,
            4 => o.max_copies = value.round().clamp(0.0, 9999.0) as u32,
            _ => return,
        }
        self.options.set(o);
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
    }

    /// Arrange all, or (`selection`) the selected models alone.
    pub(crate) fn start(self: &Rc<Self>, selection: bool, scope: ArrangeScope) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading()
            || ui.get_slicing_in_progress()
            || ui.get_repairing_in_progress()
            || ui.get_arranging()
            || !self.ctrl.allows_layout_tools()
        {
            return;
        }
        let jobs = jobs_on_plates(&self.document, &self.ctrl.build_volume(), selection, scope);
        if jobs.is_empty() {
            return;
        }
        let settings = self.options.get().settings();
        let mut leaves = Vec::new();
        let mut names = Vec::new();
        let mut work = Vec::new();
        for (gathered, plate) in jobs {
            leaves.push(gathered.leaves);
            names.push(gathered.names);
            work.push((gathered.bodies, plate));
        }
        *self.pending.borrow_mut() = Some(Pending {
            revision: self.document.scene_revision(),
            leaves,
            names,
            started: Instant::now(),
        });
        self.cancel.store(false, Ordering::Relaxed);
        ui.set_arranging(true);
        crate::breadcrumbs::note(&format!(
            "arrange: {} plate(s), {} object(s), selection {selection}",
            work.len(),
            work.iter().map(|(b, _)| b.len()).sum::<usize>()
        ));
        let slot = self.slot.clone();
        let cancel = self.cancel.clone();
        let ui_weak = self.ui.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work.iter()
                    .map(|(bodies, plate)| {
                        sc_arrange::arrange(bodies, plate, &settings, &cancel).ok()
                    })
                    .collect::<Option<Vec<Outcome>>>()
            }))
            .map_err(|payload| panic_message(&payload));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            let _ = ui_weak.upgrade_in_event_loop(|ui| ui.invoke_arrange_finished());
        });
    }

    /// The printer's plate.
    fn plate(&self) -> Plate {
        plate_of(&self.ctrl.build_volume())
    }

    /// Takes the worker's result on the UI thread and applies it, when the
    /// scene is still the one it was worked out for.
    fn finish(&self) {
        if self.fill.is_running() {
            self.finish_fill();
            return;
        }
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let pending = self.pending.borrow_mut().take();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_arranging(false);
        let (Some(result), Some(pending)) = (taken, pending) else {
            return;
        };
        let outcomes = match result {
            Ok(Some(outcomes)) => outcomes,
            // Cancelled.
            Ok(None) => return,
            Err(panic) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Arrange failed"),
                    context: None,
                    message: ctr!("ArrangeDialog", "The arrangement stopped unexpectedly."),
                    details: panic.as_str().into(),
                });
                return;
            }
        };
        if self.document.scene_revision() != pending.revision {
            ui.set_arrange_last_run(ctr!(
                "Inspector",
                "The scene changed while arranging; nothing was moved."
            ));
            return;
        }
        for outcome in &outcomes {
            crate::breadcrumbs::note(&format!(
                "arrange: footprints {:?}, search {:?}",
                outcome.timings.footprints, outcome.timings.search
            ));
        }
        // Each plate's moves, in one undo step.
        let mut edits: Vec<SceneEdit> = outcomes
            .iter()
            .zip(&pending.leaves)
            .filter_map(|(outcome, leaves)| arrangement_edit(&self.document, outcome, leaves))
            .collect();
        let edit = match edits.len() {
            0 => {
                ui.set_arrange_last_run(ctr!("Inspector", "Everything is already in place."));
                return;
            }
            1 => edits.remove(0),
            _ => SceneEdit::Compound(edits),
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => self.document.push_edit(edit),
            Err(EditNotApplied::Rejected(error)) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Arrange failed"),
                    context: None,
                    message: ctr!(
                        "ArrangeDialog",
                        "The scene rejected the arrangement; nothing was moved."
                    ),
                    details: error.to_string().into(),
                });
                return;
            }
            Err(EditNotApplied::NoSceneLoaded) => return,
        }
        let moved: usize = outcomes
            .iter()
            .map(|o| o.placements.iter().flatten().count())
            .sum();
        let left_off: Vec<String> = outcomes
            .iter()
            .zip(&pending.names)
            .flat_map(|(outcome, names)| {
                outcome
                    .left_off()
                    .into_iter()
                    .filter_map(|i| names.get(i).cloned())
                    .collect::<Vec<_>>()
            })
            .collect();
        let seconds = pending.started.elapsed().as_secs_f64();
        ui.set_arrange_last_run(last_run_text(moved, left_off.len(), seconds).into());
        if !left_off.is_empty() {
            let count = left_off.len();
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("ArrangeDialog", "Not everything fits"),
                message: ctr!(
                    "ArrangeDialog",
                    "{n} object does not fit on the plate. It is beside the plate, on the right.",
                    "{n} objects do not fit on the plate. They are beside the plate, on the right.",
                    count
                ),
                details: left_off.join("\n").into(),
            });
        }
    }

    /// Stops a run in flight; its result is dropped.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The plate of the printer whose build volume is `volume`.
pub(crate) fn plate_of(volume: &crate::build_volume::BuildVolume) -> Plate {
    let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
    Plate {
        min: [f64::from(lo.x), f64::from(lo.y)],
        max: [f64::from(hi.x), f64::from(hi.y)],
        floor: f64::from(lo.z),
    }
}

/// The plate's objects as they stand, each model with its supports, none of
/// them to move: what models imported into the scene keep clear of (#384).
pub(crate) fn standing_bodies(document: &SceneDocument) -> Vec<Body> {
    gather_all(document, false)
        .map(|gathered| {
            gathered
                .bodies
                .into_iter()
                .map(|body| Body {
                    fixed: true,
                    ..body
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The card's line after a run.
fn last_run_text(moved: usize, left_off: usize, seconds: f64) -> String {
    let time = format!("{seconds:.1}");
    if left_off == 0 {
        ctr!(
            "Inspector",
            "Arranged {n} object in {} s.",
            "Arranged {n} objects in {} s.",
            moved,
            time
        )
        .to_string()
    } else {
        ctr!(
            "Inspector",
            "{} of {} objects fit; the rest are beside the plate.",
            moved - left_off,
            moved
        )
        .to_string()
    }
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => *a,
    }
}

/// The plate's objects: one body per model, with its supports. Supports
/// whose model is unknown stay where they are, and so does everything but
/// the selected models when arranging the selection (#13). `None` when
/// nothing would move.
fn gather(
    document: &SceneDocument,
    selection: bool,
    on: Option<(crate::build_volume::BuildVolume, crate::plates::Plates, u32)>,
) -> Option<Gathered> {
    let mut gathered = gather_all(document, selection)?;
    if let Some((volume, plates, plate)) = on {
        // The objects of one plate (#545): each body by its first leaf, a
        // model or the supports no model owns.
        let keep: Vec<bool> = document
            .with_scene(|spatial| {
                gathered
                    .anchors
                    .iter()
                    .map(|&leaf| {
                        crate::plate_layout::plate_of_leaf(spatial, &volume, plates, leaf)
                            == Some(plate)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut i = 0;
        gathered.bodies.retain(|_| {
            i += 1;
            keep.get(i - 1).copied().unwrap_or(false)
        });
        for list in [&mut gathered.leaves, &mut gathered.follows] {
            let mut i = 0;
            list.retain(|_| {
                i += 1;
                keep.get(i - 1).copied().unwrap_or(false)
            });
        }
        let mut i = 0;
        gathered.names.retain(|_| {
            i += 1;
            keep.get(i - 1).copied().unwrap_or(false)
        });
    }
    Some(gathered).filter(|g| g.bodies.iter().any(|b| !b.fixed))
}

/// [`gather`], also when nothing would move.
fn gather_all(document: &SceneDocument, selection: bool) -> Option<Gathered> {
    let sessions = document.support_sessions();
    let selected = document.selection_node_ids();
    document.with_scene(|spatial| {
        let names = crate::viewport::object_names(spatial.graph());
        let rows: Vec<NodeId> = (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect();
        let mut gathered = Gathered {
            bodies: Vec::new(),
            leaves: Vec::new(),
            follows: Vec::new(),
            names: Vec::new(),
            anchors: Vec::new(),
        };
        let mut body_of: HashMap<NodeId, usize> = HashMap::new();
        let part = |leaf: NodeId| {
            let query = spatial.leaf(leaf)?;
            Some(Part {
                mesh: spatial.graph().leaf_mesh_arc(leaf)?.clone(),
                world: affine(&query.world_transform()),
            })
        };
        let name = |row: usize| names.get(row).cloned().unwrap_or_default();
        for (row, &leaf) in rows.iter().enumerate() {
            let Some(query) = spatial.leaf(leaf) else {
                continue;
            };
            if is_any_support_mesh(query.mesh()) {
                continue;
            }
            let Some(part) = part(leaf) else {
                continue;
            };
            body_of.insert(leaf, gathered.bodies.len());
            gathered.bodies.push(Body {
                parts: vec![part],
                fixed: selection && !selected.contains(&leaf),
            });
            gathered.leaves.push(vec![leaf]);
            gathered.follows.push(Vec::new());
            gathered.names.push(name(row));
            gathered.anchors.push(leaf);
        }
        for (row, &leaf) in rows.iter().enumerate() {
            let Some(query) = spatial.leaf(leaf) else {
                continue;
            };
            if !is_any_support_mesh(query.mesh()) {
                continue;
            }
            let Some(support) = part(leaf) else {
                continue;
            };
            // The supports a model came with (#259) belong to the model
            // they stand under.
            let imported = sc_geom::df_supports::is_imported_supports(query.mesh());
            let model = if imported {
                crate::imported_supports::owner_of(spatial, leaf, None)
            } else {
                model_of(&sessions, leaf)
            };
            match model.and_then(|m| body_of.get(&m).copied()) {
                Some(i) => {
                    gathered.bodies[i].parts.push(support);
                    // A set in force is redrawn where its model goes, and
                    // imported supports follow it; any other moves with it
                    // as it is.
                    let follows = imported
                        || in_force(spatial, &sessions, leaf)
                            .is_some_and(|drawn| Some(drawn.model) == model);
                    if follows {
                        gathered.follows[i].push(leaf);
                    } else {
                        gathered.leaves[i].push(leaf);
                    }
                }
                None => {
                    gathered.bodies.push(Body {
                        parts: vec![support],
                        fixed: true,
                    });
                    gathered.leaves.push(Vec::new());
                    gathered.follows.push(Vec::new());
                    gathered.names.push(name(row));
                    gathered.anchors.push(leaf);
                }
            }
        }
        gathered
    })
}

/// What an Arrange press works on: the objects, and the plate they go on.
/// With several plates (#545), each plate's objects on it, plate by plate:
/// every plate with objects to move, or the active one alone.
fn jobs_on_plates(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    selection: bool,
    scope: ArrangeScope,
) -> Vec<(Gathered, Plate)> {
    let plates = document.plates();
    if plates.count() == 1 {
        return gather(document, selection, None)
            .map(|g| vec![(g, plate_of(volume))])
            .unwrap_or_default();
    }
    let which: Vec<u32> = match scope {
        ArrangeScope::ActivePlate => vec![plates.active()],
        ArrangeScope::Plates => (0..plates.count()).collect(),
    };
    which
        .into_iter()
        .filter_map(|p| {
            let gathered = gather(document, selection, Some((*volume, plates, p)))?;
            let mut plate = plate_of(volume);
            let at = crate::plates::offset(volume, p);
            let (dx, dy) = (f64::from(at.x), f64::from(at.y));
            plate.min = [plate.min[0] + dx, plate.min[1] + dy];
            plate.max = [plate.max[0] + dx, plate.max[1] + dy];
            Some((gathered, plate))
        })
        .collect()
}

/// An Arrange press on this thread, on the plates of `document` on
/// `volume` (#545), applied as a run applies it: for tests.
#[cfg(test)]
pub(crate) fn arrange_on_plates_now(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    options: ArrangeOptions,
    scope: ArrangeScope,
) -> Option<usize> {
    let mut edits = Vec::new();
    for (gathered, plate) in jobs_on_plates(document, volume, false, scope) {
        let outcome = sc_arrange::arrange(
            &gathered.bodies,
            &plate,
            &options.settings(),
            &AtomicBool::new(false),
        )
        .ok()?;
        edits.extend(arrangement_edit(document, &outcome, &gathered.leaves));
    }
    let count = edits.len();
    let edit = SceneEdit::Compound(edits);
    document.apply_edit(edit.clone()).ok()?;
    document.push_edit(edit);
    Some(count)
}

/// Arranges on this thread and applies the result, as a run does: for
/// tests.
#[cfg(test)]
pub(crate) fn arrange_now(
    document: &SceneDocument,
    plate: &Plate,
    options: ArrangeOptions,
    selection: bool,
) -> Option<Outcome> {
    let gathered = gather(document, selection, None)?;
    let outcome = sc_arrange::arrange(
        &gathered.bodies,
        plate,
        &options.settings(),
        &AtomicBool::new(false),
    )
    .ok()?;
    let edit = arrangement_edit(document, &outcome, &gathered.leaves)?;
    document.apply_edit(edit.clone()).ok()?;
    document.push_edit(edit);
    Some(outcome)
}

/// One edit that moves every placed body's leaves: one undo step.
fn arrangement_edit(
    document: &SceneDocument,
    outcome: &Outcome,
    leaves: &[Vec<NodeId>],
) -> Option<SceneEdit> {
    let mut edits: Vec<SceneEdit> = document
        .with_scene(|spatial| {
            let mut edits = Vec::new();
            for (placement, leaves) in outcome.placements.iter().zip(leaves) {
                let Some(placement) = placement else {
                    continue;
                };
                let motion = Transform3D::Affine(placement.motion());
                for &leaf in leaves {
                    let Some(query) = spatial.leaf(leaf) else {
                        continue;
                    };
                    let before = query.local_transform();
                    let Ok(up) = query.parent_world().inverse() else {
                        continue;
                    };
                    let after = up.compose(&motion.compose(&query.world_transform()));
                    if after != before {
                        edits.push(SceneEdit::SetLocalTransform {
                            node_id: leaf,
                            before,
                            after,
                        });
                    }
                }
            }
            edits
        })
        .unwrap_or_default();
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(SceneEdit::Compound(edits)),
    }
}

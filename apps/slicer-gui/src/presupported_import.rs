/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pre-supported files told on import (#258).
//!
//! A file that comes with its supports opens as one object: for the support
//! engine a model shaped like a forest, for the review one model. Each model
//! that comes into the scene from a mesh file is looked at off the UI thread
//! ([`sc_mesh_core::presupported`]); a file with supports in shells of their
//! own gets a question, "This file has supports: separate them?". Separate
//! puts the model and its supports, raft included, in two objects where the
//! file's object stood, in one undo entry: the model keeps the file's name,
//! and the supports are "Imported supports", which the engine, the review and
//! the tools take as supports ([`sc_geom::df_supports::is_any_support_mesh`]).
//! Leave as is keeps the file as it came. A file whose supports are fused
//! into the model, in one shell, cannot be separated: a notice says so.
//!
//! A project is not looked at again when it opens: its objects are as the
//! person left them. Each mesh is looked at once while the window is open,
//! and an object left as is is not asked about again, even once Repair
//! gives it a new mesh.
//!
//! The Supports tab's IMPORTED SUPPORTS card (#259) is driven from here too:
//! the selected model's imported supports, removed, or replaced with
//! supports made here in one undo entry.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use sc_geom::df_supports::{is_any_support_mesh, is_imported_supports};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::presupported::{Presupported, find_supports, fused_supports};
use sc_mesh_core::shells::FaceShells;
use sc_scene::NodeId;
use sc_scene::node::SceneMetadata;
use slint::{ComponentHandle, SharedString};

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts, MessageKey};
use crate::i18n::ctr;
use crate::imported_supports::{imported_of, owner_of};
use crate::scene_document::{EditNotApplied, SceneDocument, SceneEvent, SceneSubscription};
use crate::selection::{LeafId, Selection};
use crate::split_bodies::{leaf_name, removal_edit, split_edit};
use crate::support_persist::sets_of_model;

/// How often the UI thread looks for what the worker found.
const POLL: Duration = Duration::from_millis(50);
/// How long a separation says what it did in the status bar.
const STATUS_NOTICE_DURATION: Duration = Duration::from_secs(6);

/// What a model turned out to be.
enum Verdict {
    /// Supports in shells of their own, and the model without them.
    Separable(Box<Presupported>),
    /// Supports fused into the model.
    Fused,
}

/// A model the worker found supports on.
struct Found {
    leaf: NodeId,
    mesh: Weak<IndexedMesh>,
    verdict: Verdict,
}

type FoundSlot = Arc<Mutex<Option<Vec<Found>>>>;

pub(crate) struct PresupportedImport {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    /// The meshes looked at, held weakly: a mesh still alive keeps its
    /// address, so one looked at is never taken for another.
    seen: RefCell<Vec<Weak<IndexedMesh>>>,
    /// Models waiting for the worker, while it works on others.
    waiting: RefCell<Vec<(NodeId, Arc<IndexedMesh>)>>,
    /// What the worker in flight hands back; `None` while it works.
    slot: RefCell<Option<FoundSlot>>,
    poll: slint::Timer,
    status_timer: slint::Timer,
    /// Set while a separation lands: its 2 objects are not looked at.
    quiet: Cell<bool>,
    /// The objects the person chose to leave as they came.
    declined: RefCell<Vec<NodeId>>,
    /// The bodies of each imported set counted, by its mesh.
    bodies: RefCell<Vec<(Weak<IndexedMesh>, usize)>>,
    subscription: RefCell<Option<SceneSubscription>>,
}

impl PresupportedImport {
    /// Starts watching the models that come into the window's scene, and
    /// returns the driver, which the caller keeps alive for the window's
    /// lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            seen: RefCell::new(Vec::new()),
            waiting: RefCell::new(Vec::new()),
            slot: RefCell::new(None),
            poll: slint::Timer::default(),
            status_timer: slint::Timer::default(),
            quiet: Cell::new(false),
            declined: RefCell::new(Vec::new()),
            bodies: RefCell::new(Vec::new()),
            subscription: RefCell::new(None),
        });
        {
            let run = Rc::downgrade(&run);
            ui.on_imported_supports_remove_requested(move || {
                if let Some(run) = run.upgrade() {
                    run.remove_imported(false);
                }
            });
        }
        {
            let run = Rc::downgrade(&run);
            ui.on_imported_supports_replace_requested(move || {
                if let Some(run) = run.upgrade() {
                    run.remove_imported(true);
                }
            });
        }
        let weak = Rc::downgrade(&run);
        let subscription = run.document.subscribe(move |event| {
            if let Some(run) = weak.upgrade() {
                run.on_scene_event(event);
            }
        });
        *run.subscription.borrow_mut() = Some(subscription);
        run
    }

    fn on_scene_event(self: &Rc<Self>, event: SceneEvent) {
        if matches!(
            event,
            SceneEvent::Installed
                | SceneEvent::Cleared
                | SceneEvent::Restructured
                | SceneEvent::SelectionChanged
        ) {
            self.refresh_card();
        }
        if self.quiet.get() {
            return;
        }
        match event {
            SceneEvent::Installed => {
                if self.document.is_project() {
                    // As the person left it.
                    self.mark_all_seen();
                } else {
                    self.look();
                }
            }
            SceneEvent::Restructured => self.look(),
            _ => {}
        }
    }

    /// The model the selection stands for, with its imported support sets:
    /// a model selected, or one of its imported sets.
    fn selected_imported(&self) -> Option<(NodeId, Vec<NodeId>)> {
        let leaf = self.document.selection_primary_node_id()?;
        self.document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                let model = if is_imported_supports(query.mesh()) {
                    owner_of(spatial, leaf, None)?
                } else if is_any_support_mesh(query.mesh()) {
                    return None;
                } else {
                    leaf
                };
                let sets = imported_of(spatial, model, None);
                (!sets.is_empty()).then_some((model, sets))
            })
            .flatten()
    }

    /// The bodies of the mesh `mesh`, counted once.
    fn body_count(&self, mesh: &Arc<IndexedMesh>) -> usize {
        let mut counted = self.bodies.borrow_mut();
        counted.retain(|(w, _)| w.strong_count() > 0);
        if let Some((_, n)) = counted
            .iter()
            .find(|(w, _)| w.as_ptr() == Arc::as_ptr(mesh))
        {
            return *n;
        }
        let n = FaceShells::of_mesh(mesh).count() as usize;
        counted.push((Arc::downgrade(mesh), n));
        n
    }

    /// Mirrors the selected model's imported supports on the Supports tab.
    fn refresh_card(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let meshes: Vec<Arc<IndexedMesh>> = self
            .selected_imported()
            .map(|(_, sets)| {
                self.document
                    .with_scene(|spatial| {
                        sets.iter()
                            .filter_map(|&set| spatial.graph().leaf_mesh_arc(set).cloned())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let bodies: usize = meshes.iter().map(|m| self.body_count(m)).sum();
        ui.set_imported_support_bodies(i32::try_from(bodies).unwrap_or(i32::MAX));
    }

    /// Removes the selected model's imported supports in one undo entry;
    /// with `replace`, the model then waits for supports made here, which
    /// join that entry (#259).
    fn remove_imported(&self, replace: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading() {
            return;
        }
        let Some((model, sets)) = self.selected_imported() else {
            return;
        };
        self.remove_imported_of(model, &sets, replace);
    }

    /// Asks, right after a separation, whether `model`'s imported supports
    /// go for supports made here; Replace does what the Supports tab's
    /// button does.
    fn offer_replace(self: &Rc<Self>, model: NodeId, context: SharedString) {
        let run = Rc::downgrade(self);
        self.dialogs.show_replace_imported(context, move |replace| {
            let Some(run) = run.upgrade() else {
                return;
            };
            if !replace {
                return;
            }
            let sets = run
                .document
                .with_scene(|spatial| imported_of(spatial, model, None))
                .unwrap_or_default();
            if sets.is_empty() {
                // Undone, or taken away meanwhile.
                return;
            }
            run.remove_imported_of(model, &sets, true);
        });
    }

    /// Removes `sets`, the imported supports of `model`, in one undo entry;
    /// with `replace`, the model then waits for supports made here.
    fn remove_imported_of(&self, model: NodeId, sets: &[NodeId], replace: bool) {
        let Some(edit) = self
            .document
            .with_scene(|spatial| removal_edit(spatial.graph(), sets))
            .flatten()
        else {
            return;
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => {
                if replace {
                    self.document.push_edit_wanting(edit, &[model]);
                    self.show_status(ctr!(
                        "SeparateSupportsDialog",
                        "Imported supports removed; generating supports for the model."
                    ));
                } else {
                    self.document.push_edit(edit);
                    self.show_status(ctr!("SeparateSupportsDialog", "Imported supports removed."));
                }
                self.document.set_selection(Selection::One(LeafId(model)));
            }
            Err(EditNotApplied::NoSceneLoaded) => {}
            Err(EditNotApplied::Rejected(error)) => self.show_rejected(error.to_string()),
        }
    }

    fn show_rejected(&self, details: String) {
        self.dialogs.show_error(move || ErrorTexts {
            heading: ctr!("ErrorDialog", "Edit failed"),
            context: None,
            message: ctr!(
                "ErrorDialog",
                "The scene rejected the last edit; nothing was changed."
            ),
            details: details.clone().into(),
        });
    }

    fn is_seen(&self, mesh: &Arc<IndexedMesh>) -> bool {
        let ptr = Arc::as_ptr(mesh);
        self.seen.borrow().iter().any(|w| w.as_ptr() == ptr)
    }

    fn mark_seen(&self, mesh: &Arc<IndexedMesh>) {
        let mut seen = self.seen.borrow_mut();
        seen.retain(|w| w.strong_count() > 0);
        if !seen.iter().any(|w| w.as_ptr() == Arc::as_ptr(mesh)) {
            seen.push(Arc::downgrade(mesh));
        }
    }

    /// The models of the scene with their meshes.
    fn models(&self) -> Vec<(NodeId, Arc<IndexedMesh>)> {
        let hollows = self.document.hollows();
        let hollows = hollows.borrow();
        self.document
            .with_scene(|spatial| {
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter_map(|id| {
                        let mesh = spatial.graph().leaf_mesh_arc(id)?;
                        // A hollow made here is a model the person worked
                        // on, not a file that came in.
                        let hollowed = hollows.iter().any(|h| Arc::ptr_eq(&h.hollow, mesh));
                        (!is_any_support_mesh(mesh) && !mesh.faces().is_empty() && !hollowed)
                            .then(|| (id, mesh.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn mark_all_seen(&self) {
        for (_, mesh) in self.models() {
            self.mark_seen(&mesh);
        }
    }

    /// Hands the models not looked at yet to the worker.
    fn look(self: &Rc<Self>) {
        let new: Vec<(NodeId, Arc<IndexedMesh>)> = self
            .models()
            .into_iter()
            .filter(|(id, mesh)| !self.is_seen(mesh) && !self.declined.borrow().contains(id))
            .collect();
        for (_, mesh) in &new {
            self.mark_seen(mesh);
        }
        if new.is_empty() {
            return;
        }
        self.waiting.borrow_mut().extend(new);
        self.start();
    }

    /// Starts the worker on the models waiting, when none is in flight.
    fn start(self: &Rc<Self>) {
        if self.slot.borrow().is_some() {
            return;
        }
        let models = std::mem::take(&mut *self.waiting.borrow_mut());
        if models.is_empty() {
            return;
        }
        let slot: FoundSlot = Arc::new(Mutex::new(None));
        *self.slot.borrow_mut() = Some(slot.clone());
        std::thread::Builder::new()
            .name("presupported".into())
            .spawn(move || {
                let found = look_at(models);
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
            })
            .expect("spawn the pre-supported worker");
        let weak = Rc::downgrade(self);
        self.poll.start(slint::TimerMode::Repeated, POLL, move || {
            if let Some(run) = weak.upgrade() {
                run.collect();
            }
        });
    }

    /// Takes what the worker found, once it is done, and asks about it.
    fn collect(self: &Rc<Self>) {
        let found = {
            let slot = self.slot.borrow();
            let Some(slot) = slot.as_ref() else {
                self.poll.stop();
                return;
            };
            let taken = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
            match taken {
                Some(found) => found,
                None => return,
            }
        };
        self.poll.stop();
        *self.slot.borrow_mut() = None;
        for found in found {
            self.offer(found);
        }
        // Models that came in while the worker was busy.
        self.start();
    }

    /// Whether `leaf` still shows `mesh`.
    fn still_shows(&self, leaf: NodeId, mesh: &Weak<IndexedMesh>) -> Option<Arc<IndexedMesh>> {
        let mesh = mesh.upgrade()?;
        self.document
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .leaf_mesh_arc(leaf)
                    .is_some_and(|m| Arc::ptr_eq(m, &mesh))
            })
            .unwrap_or(false)
            .then_some(mesh)
    }

    fn name_of(&self, leaf: NodeId) -> SharedString {
        self.document
            .with_scene(|spatial| leaf_name(spatial.graph(), leaf))
            .flatten()
            .unwrap_or_default()
            .into()
    }

    fn offer(self: &Rc<Self>, found: Found) {
        if self.still_shows(found.leaf, &found.mesh).is_none() {
            return;
        }
        let context = self.name_of(found.leaf);
        match found.verdict {
            Verdict::Fused => {
                crate::breadcrumbs::note("presupported: supports fused into the model");
                self.dialogs.show_notice(MessageKey::FusedSupports, || InfoTexts {
                    heading: ctr!("SeparateSupportsDialog", "Supports fused into the model"),
                    message: ctr!(
                        "SeparateSupportsDialog",
                        "The supports of this file are fused with the model and cannot be separated. Breadslicer takes the whole file as one model."
                    ),
                    details: Default::default(),
                });
            }
            Verdict::Separable(presupported) => {
                let supports = presupported.support_shells;
                let raft = presupported.raft;
                crate::breadcrumbs::note(&format!(
                    "presupported: {supports} support bodies, raft {raft}, {} model shell(s)",
                    presupported.model_shells
                ));
                let run = Rc::downgrade(self);
                let (leaf, mesh) = (found.leaf, found.mesh);
                self.dialogs.show_separate_supports(
                    context,
                    move || InfoTexts {
                        heading: ctr!("SeparateSupportsDialog", "This file has supports"),
                        message: if raft {
                            ctr!(
                                "SeparateSupportsDialog",
                                "1 model, {n} support and a raft. Separate them? The supports go to an object of their own: Breadslicer does not support them again, and they move with the model.",
                                "1 model, {n} supports and a raft. Separate them? The supports go to an object of their own: Breadslicer does not support them again, and they move with the model.",
                                supports
                            )
                        } else {
                            ctr!(
                                "SeparateSupportsDialog",
                                "1 model and {n} support. Separate them? The supports go to an object of their own: Breadslicer does not support them again, and they move with the model.",
                                "1 model and {n} supports. Separate them? The supports go to an object of their own: Breadslicer does not support them again, and they move with the model.",
                                supports
                            )
                        },
                        details: Default::default(),
                    },
                    move |separate| {
                        let Some(run) = run.upgrade() else {
                            return;
                        };
                        if separate {
                            run.separate(leaf, &mesh, *presupported);
                        } else {
                            run.declined.borrow_mut().push(leaf);
                        }
                    },
                );
            }
        }
    }

    /// Puts the model and its supports in 2 objects where `leaf` stood, in
    /// one undo entry, when `leaf` still shows `mesh`.
    fn separate(
        self: &Rc<Self>,
        leaf: NodeId,
        mesh: &Weak<IndexedMesh>,
        presupported: Presupported,
    ) {
        if self.still_shows(leaf, mesh).is_none() {
            return;
        }
        let has_sets = {
            let sessions = self.document.support_sessions();
            self.document
                .with_scene(|spatial| !sets_of_model(spatial, &sessions, leaf).is_empty())
                .unwrap_or(false)
        };
        if has_sets {
            // Its supports made here would lose their model.
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("SeparateSupportsDialog", "This file has supports"),
                message: ctr!(
                    "SeparateSupportsDialog",
                    "Breadslicer already made supports for this object, so its imported supports are not separated. Undo the supports made here to separate them."
                ),
                details: Default::default(),
            });
            return;
        }
        let meta: Option<SceneMetadata> = self
            .document
            .with_scene(|spatial| spatial.graph().node(leaf).and_then(|n| n.meta.clone()))
            .flatten();
        let parts = vec![(presupported.model, meta), (presupported.supports, None)];
        let slots = self.document.allocate_leaf_slots(parts.len());
        let Some(edit) = self
            .document
            .with_scene(|spatial| split_edit(spatial.graph(), leaf, parts, &slots))
            .flatten()
        else {
            return;
        };
        self.quiet.set(true);
        let applied = self.document.apply_edit(edit.clone());
        self.quiet.set(false);
        match applied {
            Ok(()) => {
                self.document.push_edit(edit);
                for (id, _) in &slots {
                    let mesh = self
                        .document
                        .with_scene(|spatial| spatial.graph().leaf_mesh_arc(*id).cloned())
                        .flatten();
                    if let Some(mesh) = mesh {
                        self.mark_seen(&mesh);
                    }
                }
                let (model, _) = slots[0];
                self.document.set_selection(Selection::One(LeafId(model)));
                self.show_status(ctr!(
                    "SeparateSupportsDialog",
                    "Separated: the model and its imported supports."
                ));
                crate::breadcrumbs::note("presupported: separated");
                self.offer_replace(model, self.name_of(model));
            }
            Err(EditNotApplied::NoSceneLoaded) => {}
            Err(EditNotApplied::Rejected(error)) => self.show_rejected(error.to_string()),
        }
    }

    /// Shows `text` in the status bar in place of its hint for a few
    /// seconds.
    fn show_status(&self, text: SharedString) {
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

    /// Waits for the worker in flight and asks about what it found: for
    /// tests, which have no event loop to run the timer.
    #[cfg(test)]
    pub(crate) fn settle_for_test(self: &Rc<Self>) {
        while self.slot.borrow().is_some() {
            std::thread::sleep(Duration::from_millis(5));
            self.collect();
        }
    }
}

/// What each of `models` is, on the worker: only the ones with supports.
fn look_at(models: Vec<(NodeId, Arc<IndexedMesh>)>) -> Vec<Found> {
    models
        .into_iter()
        .filter_map(|(leaf, mesh)| {
            let verdict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match find_supports(&mesh) {
                    Some(found) => Some(Verdict::Separable(Box::new(found))),
                    None => fused_supports(&mesh).then_some(Verdict::Fused),
                }
            }))
            .ok()
            .flatten()?;
            Some(Found {
                leaf,
                mesh: Arc::downgrade(&mesh),
                verdict,
            })
        })
        .collect()
}

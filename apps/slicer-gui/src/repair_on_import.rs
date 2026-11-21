/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Repair on import (#677): with the setting on, every model that comes in
//! from a file and does not slice is repaired as soon as it lands, by the
//! same run as the Repair button (through Caliper when it is installed,
//! #279), in an Undo step of its own. Models that slice are left alone, and
//! a project's models stay as they were saved.
//!
//! A file that could not be imported as it came, because some of its
//! triangles have no coordinates (the reader leaves them out), is repaired
//! whatever the setting says, and a card that always shows says why.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use sc_mesh_core::IndexedMesh;
use sc_mesh_formats::messages::{LoadMeshWarning, LoadStlWarning};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;
use slint::SharedString;

use crate::error_dialog::{DialogManager, InfoTexts};
use crate::i18n::ctr;
use crate::preferences_bus::PreferencesBus;
use crate::repair_run::RepairRun;
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
use sc_geom::df_supports::is_any_support_mesh;

/// How often the checks running and the models waiting for a busy window
/// are looked at again.
const POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// A model checked off the window's thread: whether it slices, and how many
/// triangles its file left out (none for a file read whole).
struct Checked {
    leaf: NodeId,
    slices: bool,
    left_out: Option<usize>,
}

type CheckSlot = Arc<Mutex<Option<Vec<Checked>>>>;

thread_local! {
    /// The meshes of files the reader left triangles out of, and how many.
    static LEFT_OUT: RefCell<Vec<(Weak<IndexedMesh>, usize)>> = const { RefCell::new(Vec::new()) };
}

/// Notes the models of a file read into `spatial` with `warnings`, when the
/// reader left triangles out of it: they are repaired whatever the setting.
pub(crate) fn note_left_out(spatial: &SceneSpatial, warnings: &[LoadMeshWarning]) {
    let Some(count) = warnings.iter().find_map(|w| match w {
        LoadMeshWarning::Stl(LoadStlWarning::FacetsWithoutCoordinates { count }) => Some(*count),
        _ => None,
    }) else {
        return;
    };
    let meshes: Vec<Arc<IndexedMesh>> = (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter_map(|id| spatial.graph().leaf_mesh_arc(id).cloned())
        .collect();
    LEFT_OUT.with_borrow_mut(|left| {
        left.retain(|(w, _)| w.strong_count() > 0);
        left.extend(meshes.iter().map(|m| (Arc::downgrade(m), count)));
    });
}

/// How many triangles the file of `mesh` left out, taken once.
fn take_left_out(mesh: &Arc<IndexedMesh>) -> Option<usize> {
    LEFT_OUT.with_borrow_mut(|left| {
        let at = left
            .iter()
            .position(|(w, _)| w.as_ptr() == Arc::as_ptr(mesh))?;
        Some(left.remove(at).1)
    })
}

/// Why a model that did not come in whole had to be repaired.
fn repaired_reason(left_out: usize) -> SharedString {
    ctr!(
        "RepairDialog",
        "The file could not be imported as it came: {n} triangle has no coordinates in space. It was left out, and the model repaired.",
        "The file could not be imported as it came: {n} triangles have no coordinates in space. They were left out, and the model repaired.",
        left_out
    )
}

pub(crate) struct RepairOnImport {
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    bus: Rc<PreferencesBus>,
    repair: Rc<RepairRun>,
    /// The meshes already looked at, so each is checked once.
    seen: RefCell<Vec<Weak<IndexedMesh>>>,
    /// Checks still running.
    checks: RefCell<Vec<CheckSlot>>,
    /// Models to repair, waiting for the window to be free, with the
    /// triangles their file left out.
    waiting: RefCell<Vec<(NodeId, Option<usize>)>>,
    poll: slint::Timer,
    subscription: RefCell<Option<SceneSubscription>>,
}

impl RepairOnImport {
    pub(crate) fn install(
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        bus: Rc<PreferencesBus>,
        repair: Rc<RepairRun>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            document,
            dialogs,
            bus,
            repair,
            seen: RefCell::new(Vec::new()),
            checks: RefCell::new(Vec::new()),
            waiting: RefCell::new(Vec::new()),
            poll: slint::Timer::default(),
            subscription: RefCell::new(None),
        });
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
        match event {
            SceneEvent::Installed if self.document.is_project() => {
                // As the person saved it.
                self.waiting.borrow_mut().clear();
                for (_, mesh) in self.models() {
                    self.mark_seen(&mesh);
                }
            }
            SceneEvent::Installed | SceneEvent::Restructured => self.look(),
            SceneEvent::Cleared => self.waiting.borrow_mut().clear(),
            _ => {}
        }
    }

    /// Checks the models not looked at yet, off the window's thread: those
    /// whose file left triangles out always, the rest with the setting on.
    fn look(self: &Rc<Self>) {
        let on = self.bus.snapshot().repair_on_import;
        let new: Vec<(NodeId, Arc<IndexedMesh>, Option<usize>)> = self
            .models()
            .into_iter()
            .filter(|(_, mesh)| !self.is_seen(mesh))
            .map(|(id, mesh)| {
                self.mark_seen(&mesh);
                let left_out = take_left_out(&mesh);
                (id, mesh, left_out)
            })
            .filter(|(_, _, left_out)| on || left_out.is_some())
            .collect();
        if new.is_empty() {
            return;
        }
        let slot: CheckSlot = Arc::new(Mutex::new(None));
        self.checks.borrow_mut().push(slot.clone());
        std::thread::spawn(move || {
            let checked: Vec<Checked> = new
                .into_iter()
                .map(|(leaf, mesh, left_out)| Checked {
                    leaf,
                    slices: crate::mesh_check::mesh_slices(&mesh),
                    left_out,
                })
                .collect();
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(checked);
        });
        if !self.poll.running() {
            let weak = Rc::downgrade(self);
            self.poll.start(slint::TimerMode::Repeated, POLL, move || {
                if let Some(run) = weak.upgrade() {
                    run.tick();
                }
            });
        }
    }

    /// Takes in the checks that finished, and repairs the models waiting
    /// once the window is free (not loading, slicing or repairing).
    fn tick(self: &Rc<Self>) {
        let mut whole_without = 0;
        self.checks.borrow_mut().retain(|slot| {
            match slot.lock().unwrap_or_else(PoisonError::into_inner).take() {
                Some(checked) => {
                    for c in checked {
                        if !c.slices {
                            self.waiting.borrow_mut().push((c.leaf, c.left_out));
                        } else if let Some(n) = c.left_out {
                            whole_without += n;
                        }
                    }
                    false
                }
                None => true,
            }
        });
        if whole_without > 0 {
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("RepairDialog", "Opened without some triangles"),
                message: ctr!(
                    "RepairDialog",
                    "The file could not be imported as it came: {n} triangle has no coordinates in space. It was left out; the rest of the model is whole and slices.",
                    "The file could not be imported as it came: {n} triangles have no coordinates in space. They were left out; the rest of the model is whole and slices.",
                    whole_without
                ),
                details: Default::default(),
            });
        }
        let waiting = self.waiting.borrow().clone();
        if !waiting.is_empty() {
            let leaves: Vec<NodeId> = waiting.iter().map(|(leaf, _)| *leaf).collect();
            let left_out: usize = waiting.iter().filter_map(|(_, n)| *n).sum();
            let reason = (left_out > 0).then(|| repaired_reason(left_out));
            if self.repair.repair_imported(&leaves, reason) {
                crate::breadcrumbs::note(&format!(
                    "repair on import: {} model(s), {left_out} triangle(s) left out",
                    leaves.len()
                ));
                self.waiting.borrow_mut().clear();
            }
        }
        if self.checks.borrow().is_empty() && self.waiting.borrow().is_empty() {
            self.poll.stop();
        }
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

    /// The models of the scene with their meshes: no supports, no empty
    /// meshes, no hollow made here.
    fn models(&self) -> Vec<(NodeId, Arc<IndexedMesh>)> {
        let hollows = self.document.hollows();
        let hollows = hollows.borrow();
        self.document
            .with_scene(|spatial| {
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter_map(|id| {
                        let mesh = spatial.graph().leaf_mesh_arc(id)?;
                        let hollowed = hollows.iter().any(|h| Arc::ptr_eq(&h.hollow, mesh));
                        (!is_any_support_mesh(mesh) && !mesh.faces().is_empty() && !hollowed)
                            .then(|| (id, mesh.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_fixtures::repair_corpus::{RepairExpectation, repair_corpus};
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

    use super::*;
    use crate::MainWindow;
    use crate::preferences_bus::{PreferencesChange, PreferencesSnapshot};
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::viewport::ViewportController;

    /// A broken model of the repair corpus beside a clean cube.
    fn broken_and_clean() -> Scene {
        let broken = repair_corpus()
            .into_iter()
            .find(|f| f.expectation == RepairExpectation::Broken)
            .expect("a broken fixture")
            .mesh();
        assert!(!broken.is_sliceable());
        let mut graph = Scene::new();
        for (mesh, x) in [(broken, 0.0), (unit_cube_mesh(), 40.0)] {
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.0])),
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .expect("inline leaf");
            graph.add_root(leaf).expect("root");
        }
        graph
    }

    /// Whether each model of the scene slices, and its mesh, in walk order.
    fn models(document: &SceneDocument) -> Vec<(bool, Arc<IndexedMesh>)> {
        document
            .with_scene(|spatial| {
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter_map(|id| spatial.graph().leaf_mesh_arc(id).cloned())
                    .map(|mesh| (mesh.is_sliceable(), mesh))
                    .collect()
            })
            .expect("a scene")
    }

    /// Opens `graph` as a model file in a window with the setting `on`, and
    /// runs the window until nothing is left to check or repair. Returns the
    /// models before and after, and the Undo steps.
    fn import(
        graph: Scene,
        on: bool,
        project: bool,
        left_out: Option<usize>,
    ) -> (
        Vec<(bool, Arc<IndexedMesh>)>,
        Vec<(bool, Arc<IndexedMesh>)>,
        usize,
        Option<String>,
    ) {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot {
            repair_on_import: on,
            ..PreferencesSnapshot::for_tests()
        });
        let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
        let document = ctrl.document();
        let dialogs = ctrl.dialogs();
        dialogs.attach(&window);
        let repair = RepairRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
        let run = RepairOnImport::install(document.clone(), dialogs, bus.clone(), repair);
        // The file of the broken model left triangles out; the cube came
        // in whole.
        if let Some(count) = left_out {
            let spatial = SceneSpatial::from_graph(graph.clone());
            let broken = spatial
                .leaf_id_at_index(0)
                .and_then(|id| spatial.graph().leaf_mesh_arc(id).cloned())
                .expect("the broken model");
            LEFT_OUT.with_borrow_mut(|left| left.push((Arc::downgrade(&broken), count)));
        }
        if project {
            document.install_project_graph(graph, PathBuf::from("mini.bread"), Default::default());
        } else {
            document.install_graph(graph, PathBuf::from("mini.stl"));
        }
        let before = models(&document);
        let start = Instant::now();
        loop {
            slint::platform::update_timers_and_animations();
            if window.get_repairing_in_progress() {
                window.invoke_repair_finished();
            }
            let idle = run.checks.borrow().is_empty()
                && run.waiting.borrow().is_empty()
                && !window.get_repairing_in_progress();
            if idle {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "it never settles"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = bus.update(|_| PreferencesChange::RepairOnImport);
        let card = window
            .get_message_open()
            .then(|| window.get_message().heading.to_string());
        (before, models(&document), document.undo_len(), card)
    }

    #[test]
    fn a_model_that_comes_in_broken_is_repaired_and_a_clean_one_left_alone() {
        let (before, after, undo, _card) =
            on_fresh_slint_thread(|| import(broken_and_clean(), true, false, None));
        assert_eq!(
            before.iter().map(|m| m.0).collect::<Vec<_>>(),
            [false, true]
        );
        assert_eq!(after.iter().map(|m| m.0).collect::<Vec<_>>(), [true, true]);
        assert!(
            Arc::ptr_eq(&before[1].1, &after[1].1),
            "the clean cube is not touched"
        );
        assert_eq!(undo, 1, "the repair is 1 Undo step");
    }

    #[test]
    fn with_the_setting_off_nothing_is_repaired() {
        let (before, after, undo, _card) =
            on_fresh_slint_thread(|| import(broken_and_clean(), false, false, None));
        assert!(Arc::ptr_eq(&before[0].1, &after[0].1));
        assert_eq!(undo, 0);
    }

    #[test]
    fn a_project_stays_as_it_was_saved() {
        let (before, after, undo, _card) =
            on_fresh_slint_thread(|| import(broken_and_clean(), true, true, None));
        assert!(Arc::ptr_eq(&before[0].1, &after[0].1));
        assert_eq!(undo, 0);
    }

    /// A file the reader left triangles out of is repaired with the setting
    /// off too, and the card that always shows says why (#677).
    #[test]
    fn a_file_that_could_not_come_in_as_it_was_is_repaired_whatever_the_setting() {
        let (before, after, undo, card) =
            on_fresh_slint_thread(|| import(broken_and_clean(), false, false, Some(2912)));
        assert!(!before[0].0);
        assert!(after[0].0, "repaired");
        assert!(Arc::ptr_eq(&before[1].1, &after[1].1));
        assert_eq!(undo, 1);
        assert_eq!(card.as_deref(), Some("Repaired on import"));
    }
}

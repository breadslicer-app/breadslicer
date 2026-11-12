/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Repair from the card of a refused slice (#407), driven end to end through
//! the window's own callbacks: the Slice press, the card's buttons, the
//! repair card's Cancel and the workers' wake-ups.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::repair_corpus::repair_corpus;
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::SceneMetadata;
use sc_scene::mesh_targets::MeshTarget;
use sc_scene::node::LeafPayload;

use super::*;
use crate::WorkspaceSection;
use crate::df_support_run::DfSupportRun;
use crate::error_dialog::{InfoTexts, MessageKey};
use crate::hollow_run::HollowRun;
use crate::preferences_bus::PreferencesSnapshot;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::ViewportController;
use crate::{AppMessage, MessageAnswer, MessageKind};

/// The longest a worker may take before the test calls it hung.
const PATIENCE: Duration = Duration::from_secs(300);

/// A main window with the Slice and Repair drivers installed and the card
/// layer attached, slicing on a printer and a resin that the test profiles
/// have, in the Slice tab.
struct Window {
    window: MainWindow,
    _ctrl: Rc<ViewportController>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    slice: Rc<SliceRun>,
}

fn window_with(graph: Scene) -> Window {
    let window = MainWindow::new().expect("the headless platform creates a main window");
    let bus = PreferencesBus::new(PreferencesSnapshot {
        default_printer: Some(PrinterId::new("elegoo-saturn-4-ultra").expect("valid id")),
        default_resin: Some(MaterialId::new("standard-405").expect("valid id")),
        ..PreferencesSnapshot::for_tests()
    });
    let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
    let document = ctrl.document();
    document.install_graph(graph, PathBuf::from("mini.bread"));
    let dialogs = ctrl.dialogs();
    dialogs.attach(&window);
    let repair = RepairRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
    let supports = DfSupportRun::install(
        &window,
        bus.clone(),
        document.clone(),
        dialogs.clone(),
        ctrl.clone(),
    );
    let hollow = HollowRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
    let review = ReviewPanel::install(
        &window,
        bus.clone(),
        document.clone(),
        ctrl.clone(),
        supports,
        hollow,
    );
    let slice = SliceRun::install(
        &window,
        bus,
        document.clone(),
        dialogs.clone(),
        review,
        repair,
        ctrl.clone(),
    );
    ctrl.set_section(WorkspaceSection::Slice);
    Window {
        window,
        _ctrl: ctrl,
        document,
        dialogs,
        slice,
    }
}

impl Window {
    /// Presses Slice and waits for the run's wake-up.
    fn press_slice(&self) {
        self.window.invoke_slice_requested();
        self.finish_slice();
    }

    /// Waits for the slice in flight and hands its wake-up to the window.
    fn finish_slice(&self) {
        assert!(self.window.get_slicing_in_progress(), "a slice runs");
        let start = Instant::now();
        while self
            .slice
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
        {
            assert!(start.elapsed() < PATIENCE, "the slice worker hangs");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.window.invoke_slice_finished();
    }

    /// Hands the repair worker's wake-up to the window until the run in
    /// flight is done; an early wake-up is ignored, as in the app.
    fn finish_repair(&self) {
        let start = Instant::now();
        while self.window.get_repairing_in_progress() {
            assert!(start.elapsed() < PATIENCE, "the repair worker hangs");
            std::thread::sleep(Duration::from_millis(5));
            self.window.invoke_repair_finished();
        }
    }

    /// The card on show.
    fn card(&self) -> AppMessage {
        assert!(self.window.get_message_open(), "a card is on show");
        self.window.get_message()
    }

    /// The `Arc` of each mesh the scene stores, in walk order.
    fn meshes(&self) -> Vec<(MeshTarget, Arc<IndexedMesh>)> {
        self.document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                graph
                    .mesh_targets()
                    .into_iter()
                    .filter_map(|t| crate::mesh_check::target_arc(graph, t).map(|m| (t, m)))
                    .collect()
            })
            .expect("a scene")
    }
}

/// A corpus mesh by name.
fn corpus_mesh(name: &str) -> IndexedMesh {
    repair_corpus()
        .into_iter()
        .find(|fixture| fixture.name == name)
        .expect("a corpus fixture")
        .mesh()
}

/// A flat open plate: 2 triangles and no volume, which Repair can't close.
fn open_plate() -> IndexedMesh {
    let vertices = [
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [0.0, 10.0, 0.0],
        [10.0, 10.0, 0.0],
    ]
    .map(Vertex::new)
    .to_vec();
    let faces = [[0, 1, 2], [2, 1, 3]]
        .map(IndexedTriangle::from_raw_indices)
        .to_vec();
    IndexedMesh::new(None, vertices, faces)
}

/// A scene of `broken`, named "Mini", and a clean cube beside it.
fn mini_and_cube(broken: IndexedMesh) -> Scene {
    let mut graph = Scene::new();
    let mini = graph
        .add_leaf(
            Some(SceneMetadata {
                name: Some("Mini".to_owned()),
                comment: None,
            }),
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(broken)),
        )
        .expect("inline leaf");
    graph.add_root(mini).expect("root");
    let cube = graph
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([30.0, 0.0, 0.0])),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline leaf");
    graph.add_root(cube).expect("root");
    graph
}

/// Pins the whole way: the refused slice's card offers Repair; Repair moves
/// to the Transform tab and repairs only the refused object, as 1 undo step,
/// with its report; then it goes back to the Slice tab and slices, without
/// asking to overwrite, and the slice lands.
#[test]
fn repair_from_the_card_repairs_the_refused_object_and_slices_again() {
    on_fresh_slint_thread(|| {
        // Faces turned the wrong way at random, as on Damian's mini.
        let w = window_with(mini_and_cube(corpus_mesh("s08-flipped-scattered-5pct")));
        let before = w.meshes();

        w.press_slice();
        assert!(w.document.slice_params().is_none(), "nothing sliced yet");
        let card = w.card();
        assert_eq!(card.kind, MessageKind::Error);
        assert_eq!(card.heading, "Can't slice “Mini”");
        assert_eq!(
            card.text,
            "“Mini” is not a closed mesh: it has holes, cracks or faces that don't line up. Press Repair to fix it and slice again."
        );
        assert!(
            card.details.contains("InconsistentWindingOrder"),
            "{}",
            card.details
        );
        assert_eq!(card.action, "Repair");

        w.window.invoke_message_answered(MessageAnswer::Confirm);
        assert_eq!(
            w.window.get_workspace_section(),
            WorkspaceSection::Transform
        );
        assert!(w.window.get_repairing_in_progress());
        w.finish_repair();

        // Only the refused object changed, as 1 undo step.
        let after = w.meshes();
        assert_eq!(after.len(), before.len());
        assert!(
            !Arc::ptr_eq(&after[0].1, &before[0].1),
            "the mini is repaired"
        );
        assert!(
            Arc::ptr_eq(&after[1].1, &before[1].1),
            "the cube is untouched"
        );
        assert!(after[0].1.is_sliceable());

        // Back in the Slice tab, slicing again on its own, with the report.
        assert_eq!(w.window.get_workspace_section(), WorkspaceSection::Slice);
        assert!(w.window.get_slicing_in_progress(), "the slice starts again");
        assert_eq!(w.dialogs.info_headings_for_test(), ["Mesh repair"]);
        w.finish_slice();
        assert!(w.document.slice_params().is_some(), "the slice landed");
        assert!(w.window.get_slice_layer_count() > 1);
        assert!(w.dialogs.error_headings_for_test().is_empty());

        assert!(w.document.can_undo());
        w.document.undo().expect("undo");
        assert!(!w.document.can_undo(), "the repair is 1 undo step");
        assert!(Arc::ptr_eq(&w.meshes()[0].1, &before[0].1));
    });
}

/// Pins Cancel: the repair card's Cancel stops the run, the scene keeps every
/// mesh it had, nothing reaches the history, and nothing slices, even when
/// the cancelled worker wakes the window later.
#[test]
fn cancelling_the_repair_leaves_the_scene_and_slices_nothing() {
    on_fresh_slint_thread(|| {
        let w = window_with(mini_and_cube(corpus_mesh("s12-fin-on-edge")));
        let before = w.meshes();
        w.press_slice();
        assert_eq!(w.card().action, "Repair");
        w.window.invoke_message_answered(MessageAnswer::Confirm);
        assert!(w.window.get_repairing_in_progress());

        w.window.invoke_repair_cancel();
        assert!(!w.window.get_repairing_in_progress());
        // The late wake-up of the cancelled worker.
        std::thread::sleep(Duration::from_millis(200));
        w.window.invoke_repair_finished();

        let after = w.meshes();
        for ((_, a), (_, b)) in after.iter().zip(&before) {
            assert!(Arc::ptr_eq(a, b), "a cancelled repair changed a mesh");
        }
        assert!(!w.document.can_undo());
        assert!(!w.window.get_slicing_in_progress(), "nothing slices");
        assert!(w.document.slice_params().is_none());
        assert!(!w.window.get_message_open(), "no card follows a Cancel");
    });
}

/// Pins a mesh Repair can't make sliceable: its report says so, slicing
/// does not start again, and the Transform tab stays, showing what is left.
#[test]
fn a_repair_that_cant_make_it_sliceable_reports_and_slices_nothing() {
    on_fresh_slint_thread(|| {
        let w = window_with(mini_and_cube(open_plate()));
        w.press_slice();
        assert_eq!(w.card().action, "Repair");
        w.window.invoke_message_answered(MessageAnswer::Confirm);
        w.finish_repair();

        assert!(!w.window.get_slicing_in_progress(), "nothing slices");
        assert!(w.document.slice_params().is_none());
        assert_eq!(
            w.window.get_workspace_section(),
            WorkspaceSection::Transform
        );
        let card = w.card();
        assert_eq!(card.kind, MessageKind::Info);
        assert_eq!(card.heading, "Still can't slice");
        assert!(
            card.text
                .starts_with("Repair couldn't fix everything, so slicing didn't start again. "),
            "{}",
            card.text
        );
        assert!(!card.details.is_empty(), "the report's details are there");
        assert!(!card.can_silence, "this report can't be silenced");
        assert!(w.window.get_mesh_needs_repair());
    });
}

/// Pins that a silenced repair report still says when Repair could not make
/// the object sliceable.
#[test]
fn a_silenced_repair_report_still_says_it_cant_slice() {
    on_fresh_slint_thread(|| {
        let w = window_with(mini_and_cube(open_plate()));
        w.dialogs
            .show_notice(MessageKey::MeshRepairSummary, || InfoTexts {
                heading: "Mesh repair".into(),
                message: Default::default(),
                details: Default::default(),
            });
        w.window.set_message_silence(true);
        w.window.invoke_message_answered(MessageAnswer::Close);
        assert!(!w.window.get_message_open());

        w.press_slice();
        w.window.invoke_message_answered(MessageAnswer::Confirm);
        w.finish_repair();
        assert_eq!(w.card().heading, "Still can't slice");
    });
}

/// Pins that the card's Close, Escape and a click outside repair nothing.
#[test]
fn closing_the_card_repairs_nothing() {
    on_fresh_slint_thread(|| {
        let w = window_with(mini_and_cube(corpus_mesh("s12-fin-on-edge")));
        for answer in [MessageAnswer::Close, MessageAnswer::Cancel] {
            w.press_slice();
            assert_eq!(w.card().action, "Repair");
            w.window.invoke_message_answered(answer);
            assert!(!w.window.get_repairing_in_progress());
            assert!(!w.window.get_message_open());
        }
        w.press_slice();
        w.window.invoke_message_outside_clicked();
        assert!(
            w.window.get_message_open(),
            "an error is answered, not clicked away"
        );
        assert_eq!(w.window.get_workspace_section(), WorkspaceSection::Slice);
        assert!(!w.document.can_undo());
    });
}

/// Pins that a slice started by a Repair, refused again, offers no Repair:
/// nothing loops.
#[test]
fn a_slice_started_by_repair_and_refused_again_offers_no_repair() {
    on_fresh_slint_thread(|| {
        let w = window_with(mini_and_cube(open_plate()));
        w.slice.start_with(SliceTrigger::Repair);
        w.finish_slice();
        let card = w.card();
        assert_eq!(card.heading, "Can't slice “Mini”");
        assert_eq!(card.action, "");
        assert!(
            card.text
                .ends_with("Press Repair meshes in the Transform tab, then slice again.")
        );
    });
}

/// Pins that a slice refused only for vertices that are not numbers offers
/// no Repair, which can't fix them, and that other failures offer none.
#[test]
fn only_meshes_repair_can_fix_offer_repair() {
    let leaf = {
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(open_plate())),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        leaf
    };
    let refused = |geometry| {
        SliceFailure::Slice(SliceSceneError::LeavesValidationFailed(vec![
            LeafValidationFailure {
                leaf,
                leaf_name: None,
                geometry,
            },
        ]))
    };
    assert_eq!(
        repairable_leaves(&refused(InvalidLeafGeometry::Manifold(Vec::new()))),
        Some(vec![leaf])
    );
    assert_eq!(
        repairable_leaves(&refused(InvalidLeafGeometry::Faces(Vec::new()))),
        Some(vec![leaf])
    );
    assert_eq!(
        repairable_leaves(&refused(InvalidLeafGeometry::Vertices(Vec::new()))),
        None
    );
    assert_eq!(
        repairable_leaves(&SliceFailure::Slice(SliceSceneError::EmptyScene)),
        None
    );
}

/// Pins that the action of a card runs once, and only from its own button.
#[test]
fn an_error_action_runs_only_from_its_button() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let dialogs = DialogManager::new(&PreferencesBus::new(PreferencesSnapshot::for_tests()));
        dialogs.attach(&window);
        let ran = Rc::new(Cell::new(0));
        let texts = || ErrorTexts {
            heading: "Broken".into(),
            context: None,
            message: Default::default(),
            details: Default::default(),
        };
        for answer in [
            MessageAnswer::Close,
            MessageAnswer::Cancel,
            MessageAnswer::Confirm,
        ] {
            let counter = ran.clone();
            dialogs.show_error_with_action(
                texts,
                || "Fix".into(),
                move || counter.set(counter.get() + 1),
            );
            assert_eq!(window.get_message().action, "Fix");
            window.invoke_message_answered(answer);
        }
        assert_eq!(ran.get(), 1);
        dialogs.show_error(texts);
        assert_eq!(
            window.get_message().action,
            "",
            "a plain error has no action"
        );
    });
}

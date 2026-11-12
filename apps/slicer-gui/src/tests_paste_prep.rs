/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Copy and paste of a prepared model (#113): a hollowed, drained and
//! supported model with a support added by hand, copied and pasted, in the
//! same window and in another, and after it was deleted. The copy prints as
//! the model does, moved; it is one undo step; it saves and opens again
//! with its hollow and its supports editable; editing the model afterwards
//! leaves the copy as it was.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_print_primitives::{MaterialId, PrinterId};
use sc_project_formats::{LoadedContent, load_scene};
use sc_scene::node::LeafPayload;
use sc_scene::{NodeId, Scene};
use sc_sl1cer::contour::{ClosedRing, ContourStack};

use crate::MainWindow;
use crate::WorkspaceSection;
use crate::clipboard::{self, Clipboard, clipboard_from_document_selection};
use crate::df_support_run::{DfSupportRun, HandEdit};
use crate::hollow_persist::{install_hollows, restore_hollows};
use crate::hollow_run::HollowRun;
use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
use crate::print_selection::PrintDefaults;
use crate::scene_document::SceneDocument;
use crate::selection::{LeafId, Selection};
use crate::support_persist::{
    DrawnSet, install_sets, restore_supports, sets_in_force, sets_of_model, support_owners,
};
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::ViewportController;

/// Where the model's middle stands: its underside 5 mm over the plate.
const MODEL_AT: [f64; 3] = [40.0, 40.0, 15.0];

/// A 20 mm cube.
fn cube() -> IndexedMesh {
    let mut cube = sc_mesh_fixtures::unit_cube_mesh();
    for v in &mut cube.vertices {
        *v = Vertex::new(v.as_array().map(|c| c * 20.0));
    }
    cube
}

/// A window's scene with a model in it, and the drivers that prepare it.
struct Prepared {
    _ctrl: Rc<ViewportController>,
    document: Rc<SceneDocument>,
    hollow: Rc<HollowRun>,
    supports: Rc<DfSupportRun>,
    model: NodeId,
}

/// A window whose scene holds the 20 mm cube at `at`.
fn window_with_cube(window: &MainWindow, at: [f64; 3]) -> Prepared {
    let ctrl = Rc::new(ViewportController::new_for_test());
    let document = ctrl.document();
    let mut graph = Scene::new();
    let model = graph
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(at)),
            LeafPayload::Mesh(Arc::new(cube())),
        )
        .unwrap();
    graph.add_root(model).unwrap();
    document.install_graph(graph, PathBuf::from("cube.stl"));
    let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
    let hollow = HollowRun::install(window, document.clone(), ctrl.dialogs(), ctrl.clone());
    let supports =
        DfSupportRun::install(window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
    Prepared {
        _ctrl: ctrl,
        document,
        hollow,
        supports,
        model,
    }
}

/// The model hollowed with a 2 mm wall and a drain on top, then supported,
/// then given one support more by hand.
fn prepared(window: &MainWindow) -> Prepared {
    let p = window_with_cube(window, MODEL_AT);
    p.document.set_selection(Selection::One(LeafId(p.model)));
    window.set_workspace_section(WorkspaceSection::Hollow);
    p.hollow
        .hollow_now(2.0, &[([43.0, 41.0, 25.0], [0.0, 0.0, 1.0])]);
    let hollow = mesh_of(&p.document, p.model);
    let entry = p
        .document
        .hollows()
        .borrow()
        .last()
        .cloned()
        .expect("hollowed");
    assert!(
        Arc::ptr_eq(&entry.hollow, &hollow),
        "the model is the hollow"
    );
    assert_eq!(entry.drains.len(), 1, "drained");
    p.supports.generate_now(Default::default());
    let (edit, refused) = p.supports.add_supports_at(&[[46.0, 34.0, 5.0]]);
    assert!(matches!(edit, HandEdit::Applied), "{edit:?} {refused:?}");
    let set = set_of(&p.document, p.model).expect("a set in force");
    assert!(set.session.hand_edited(), "edited by hand");
    p
}

fn mesh_of(document: &SceneDocument, leaf: NodeId) -> Arc<IndexedMesh> {
    document
        .with_scene(|s| s.graph().leaf_mesh_arc(leaf).cloned())
        .flatten()
        .expect("a leaf with a mesh")
}

fn translation_of(document: &SceneDocument, leaf: NodeId) -> [f64; 3] {
    document
        .with_scene(|s| match s.leaf(leaf)?.world_transform() {
            Transform3D::Identity => Some([0.0; 3]),
            Transform3D::Affine(a) => Some(*a.translation()),
        })
        .flatten()
        .expect("a leaf")
}

fn leaf_count(document: &SceneDocument) -> usize {
    document
        .with_scene(|s| (0..).map_while(|i| s.leaf_id_at_index(i)).count())
        .unwrap_or_default()
}

/// The set in force of `model`.
fn set_of(document: &SceneDocument, model: NodeId) -> Option<DrawnSet> {
    let sessions = document.support_sessions();
    document
        .with_scene(|s| sets_in_force(s, &sessions))?
        .into_iter()
        .find(|d| d.model == model)
}

/// Whether `leaf` shows a hollow the document knows: its original, its
/// drains.
fn hollow_of(document: &SceneDocument, leaf: NodeId) -> Option<(Arc<IndexedMesh>, usize)> {
    let mesh = mesh_of(document, leaf);
    document
        .hollows()
        .borrow()
        .iter()
        .find(|h| Arc::ptr_eq(&h.hollow, &mesh))
        .map(|h| (h.original.clone(), h.drains.len()))
}

/// Copies `leaf` of `document`.
fn copy(document: &SceneDocument, leaf: NodeId) -> Clipboard {
    document.set_selection(Selection::One(LeafId(leaf)));
    clipboard_from_document_selection(document).expect("a model to copy")
}

/// Pastes `clipboard` into `document`, centred near `at` on the plate.
fn paste(document: &SceneDocument, clipboard: &mut Clipboard, at: [f64; 2]) -> NodeId {
    let copy = clipboard::paste(document, clipboard, at, 0.0)
        .expect("the scene takes it")
        .expect("pasted");
    clipboard.bump_cascade();
    copy
}

/// Each leaf's slices of the document's scene, as the Slice driver cuts
/// them.
fn slices(document: &SceneDocument) -> Vec<(NodeId, ContourStack)> {
    let sessions = document.support_sessions();
    let (scene, owners) = document
        .with_scene(|s| (s.graph().clone(), support_owners(s, &sessions)))
        .expect("a scene");
    let (printer, resin) = crate::slice_projection::load_selected_profiles(
        &PrinterId::new("elegoo-saturn-4-ultra").unwrap(),
        &MaterialId::new("standard-405").unwrap(),
    )
    .expect("profiles");
    let params = sc_print_glue::SlicingParams {
        layer_height_request: None,
        first_layer_height: None,
        aa_level: sc_print_primitives::AaLevel::Off,
    };
    let run = sc_print_glue::slice_plate(
        &scene,
        &printer,
        &resin,
        &params,
        &owners,
        &sc_sl1cer::CancelToken::default(),
        |_| {},
    )
    .expect("the plate slices");
    let sc_print_glue::SliceRun::Completed(plate) = run else {
        panic!("not cancelled")
    };
    plate
        .leaves()
        .iter()
        .map(|l| (l.leaf(), l.stack().clone()))
        .collect()
}

/// A ring's box.
fn ring_box(ring: &ClosedRing) -> ([f64; 2], [f64; 2]) {
    ring.vertices()
        .iter()
        .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| {
            (
                [lo[0].min(p.x), lo[1].min(p.y)],
                [hi[0].max(p.x), hi[1].max(p.y)],
            )
        })
}

/// Asserts that `copy` prints as `original` moved by `shift` in the plate
/// plane: the same layers at the same heights, each with as many islands
/// and holes (the cavity, the drain), of the same area, where the move
/// takes them.
fn prints_the_same(original: &ContourStack, copy: &ContourStack, shift: [f64; 2], what: &str) {
    assert_eq!(
        original.layers().len(),
        copy.layers().len(),
        "{what}: as many layers"
    );
    let mut holes = 0;
    for (k, (a, b)) in original.layers().iter().zip(copy.layers()).enumerate() {
        assert!(
            (a.z_mm() - b.z_mm()).abs() < 1e-9,
            "{what}: layer {k} height"
        );
        assert_eq!(
            a.polygons().len(),
            b.polygons().len(),
            "{what}: layer {k} islands"
        );
        let summary = |layer: &sc_sl1cer::contour::ContourLayer| {
            let mut islands: Vec<(f64, usize, [f64; 2], [f64; 2])> = layer
                .polygons()
                .iter()
                .map(|p| {
                    let area = p.outer().signed_area()
                        + p.holes().iter().map(ClosedRing::signed_area).sum::<f64>();
                    let (lo, hi) = ring_box(p.outer());
                    (area, p.holes().len(), lo, hi)
                })
                .collect();
            islands.sort_by(|x, y| x.2[0].total_cmp(&y.2[0]).then(x.2[1].total_cmp(&y.2[1])));
            islands
        };
        for (x, y) in summary(a).iter().zip(&summary(b)) {
            holes += x.1;
            assert_eq!(x.1, y.1, "{what}: layer {k} holes");
            assert!(
                (x.0 - y.0).abs() < 1e-3 * x.0.abs().max(1.0),
                "{what}: layer {k} area {} vs {}",
                x.0,
                y.0
            );
            for c in 0..2 {
                assert!(
                    (x.2[c] + shift[c] - y.2[c]).abs() < 1e-3
                        && (x.3[c] + shift[c] - y.3[c]).abs() < 1e-3,
                    "{what}: layer {k} moved by {shift:?}: {x:?} {y:?}"
                );
            }
        }
    }
    assert!(
        what != "model" || holes > 0,
        "the model's layers show its cavity"
    );
}

fn stack_of(slices: &[(NodeId, ContourStack)], leaf: NodeId) -> &ContourStack {
    &slices
        .iter()
        .find(|(id, _)| *id == leaf)
        .expect("the leaf is sliced")
        .1
}

/// Opens `path` into a fresh document, rebuilding its supports and
/// hollows, as the load worker does.
fn reopen(path: &Path) -> Rc<SceneDocument> {
    let loaded = load_scene(path, None, |_| {}).expect("it opens");
    let LoadedContent::Project(project) = loaded.content else {
        panic!("a .bread opens as a project");
    };
    let mut scene = project.scene;
    let supports = restore_supports(&mut scene, project.supports);
    assert!(supports.failures.is_empty(), "{:?}", supports.failures);
    let hollows = restore_hollows(&scene, project.hollows);
    assert!(hollows.failures.is_empty(), "{:?}", hollows.failures);
    let document = Rc::new(ViewportController::new_for_test()).document();
    document.install_project_graph(scene, path.to_path_buf(), project.payload);
    install_sets(&document, supports.sets);
    install_hollows(&document, hollows.hollows);
    document
}

/// The models of `document` (leaves that are not supports), in row order.
fn models(document: &SceneDocument) -> Vec<NodeId> {
    document
        .with_scene(|s| {
            (0..)
                .map_while(|i| s.leaf_id_at_index(i))
                .filter(|&id| {
                    !s.leaf(id)
                        .is_some_and(|q| sc_geom::df_supports::is_support_mesh(q.mesh()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The copy of a hollowed, drained, supported and hand-edited model is that
/// hollow at the model's height with a set of its own drawn under it, in
/// one undo step; it slices as the model does, moved; it saves and opens
/// again with its hollow and its set editable; and making the model solid
/// afterwards leaves the copy as it was.
#[test]
fn a_pasted_copy_keeps_its_hollow_and_supports_and_prints_the_same() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("copies.bread");
    on_fresh_slint_thread(move || {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let p = prepared(&window);
        let (document, model) = (p.document.clone(), p.model);
        let original_set = set_of(&document, model).expect("the model's set");
        let original_set_mesh = mesh_of(&document, original_set.leaf);
        let leaves_before = leaf_count(&document);

        let mut clipboard = copy(&document, model);
        let copy = paste(&document, &mut clipboard, [110.0, 40.0]);

        // The copy is the model's hollow, at its height, with its drain.
        assert!(Arc::ptr_eq(
            &mesh_of(&document, copy),
            &mesh_of(&document, model)
        ));
        assert_eq!(hollow_of(&document, copy).map(|h| h.1), Some(1));
        let (at, copy_at) = (
            translation_of(&document, model),
            translation_of(&document, copy),
        );
        assert!((copy_at[2] - at[2]).abs() < 1e-12, "at the model's height");
        let shift = [copy_at[0] - at[0], copy_at[1] - at[1]];
        assert!(shift[0] > 30.0, "beside the model: {shift:?}");
        // With a set of its own, in force, hand edit and all.
        let copy_set = set_of(&document, copy).expect("the copy's set is in force");
        assert_ne!(copy_set.leaf, original_set.leaf);
        assert!(copy_set.session.hand_edited(), "the hand edit came along");
        let copy_set_mesh = mesh_of(&document, copy_set.leaf);
        assert!(!Arc::ptr_eq(&copy_set_mesh, &original_set_mesh));
        // Drawn as supports, as the model's are.
        assert!(
            sc_geom::df_supports::is_any_support_mesh(&original_set_mesh),
            "the model's supports read as supports"
        );
        assert!(
            sc_geom::df_supports::is_any_support_mesh(&copy_set_mesh),
            "the copy's supports read as supports: {:?}",
            copy_set_mesh.meta()
        );
        assert_eq!(
            copy_set_mesh.vertices().len(),
            original_set_mesh.vertices().len()
        );
        assert_eq!(leaf_count(&document), leaves_before + 2);
        assert_eq!(
            document.selection_primary_node_id(),
            Some(copy),
            "the copy is selected"
        );

        // It prints as the model does, moved.
        let cut = slices(&document);
        prints_the_same(stack_of(&cut, model), stack_of(&cut, copy), shift, "model");
        prints_the_same(
            stack_of(&cut, original_set.leaf),
            stack_of(&cut, copy_set.leaf),
            shift,
            "supports",
        );

        // One undo step takes it all away; redo brings it back, in force.
        assert!(document.undo().unwrap());
        assert_eq!(leaf_count(&document), leaves_before);
        assert!(set_of(&document, model).is_some());
        assert!(document.redo().unwrap());
        assert_eq!(leaf_count(&document), leaves_before + 2);
        assert!(set_of(&document, copy).is_some(), "in force again");

        // It saves and opens again: the copy still the hollow, drained, and
        // its set editable.
        crate::scene_save::save_project_to(&document, &path, &PrintDefaults::default())
            .expect("the project saves");
        let reopened = reopen(&path);
        let opened = models(&reopened);
        assert_eq!(opened.len(), 2);
        for &m in &opened {
            assert_eq!(hollow_of(&reopened, m).map(|h| h.1), Some(1), "hollow");
            let set = set_of(&reopened, m).expect("an editable set");
            assert!(set.session.hand_edited());
        }
        let again = slices(&reopened);
        prints_the_same(
            stack_of(&again, opened[0]),
            stack_of(&again, opened[1]),
            shift,
            "model",
        );

        // The model made solid again leaves the copy as it was.
        document.set_selection(Selection::One(LeafId(model)));
        p.hollow.restore_now();
        assert!(hollow_of(&document, model).is_none(), "the model is solid");
        assert_eq!(hollow_of(&document, copy).map(|h| h.1), Some(1));
        assert!(set_of(&document, copy).is_some(), "the copy's set holds");
        assert!(document.undo().unwrap());
        assert!(hollow_of(&document, model).is_some(), "undo brings it back");
    });
}

/// Two copies share one mesh: hollowing one again, with another drain,
/// leaves the other as it was, and the copies' drains stay theirs.
#[test]
fn hollowing_one_copy_again_leaves_the_other() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let p = prepared(&window);
        let document = p.document.clone();
        let mut clipboard = copy(&document, p.model);
        let first = paste(&document, &mut clipboard, [110.0, 40.0]);
        let second = paste(&document, &mut clipboard, [110.0, 100.0]);
        let shared = mesh_of(&document, first);
        assert!(Arc::ptr_eq(&shared, &mesh_of(&document, second)));
        // A second drain on the first copy: it is hollowed again, alone.
        let at = translation_of(&document, first);
        document.set_selection(Selection::One(LeafId(first)));
        window.set_workspace_section(WorkspaceSection::Hollow);
        p.hollow.hollow_now(
            2.0,
            &[(
                [at[0] as f32 - 4.0, at[1] as f32 + 3.0, 25.0],
                [0.0, 0.0, 1.0],
            )],
        );
        assert_eq!(hollow_of(&document, first).map(|h| h.1), Some(2));
        assert!(
            Arc::ptr_eq(&mesh_of(&document, second), &shared),
            "untouched"
        );
        assert!(Arc::ptr_eq(&mesh_of(&document, p.model), &shared));
        assert!(set_of(&document, second).is_some());
        assert_eq!(
            document.selection_primary_node_id(),
            Some(first),
            "still selected"
        );
        // Undo puts the first copy back on the shared mesh.
        assert!(document.undo().unwrap());
        assert!(Arc::ptr_eq(&mesh_of(&document, first), &shared));
    });
}

/// A copy pasted in another window, and one pasted after the model was
/// deleted, keep the hollow and the supports as they were copied.
#[test]
fn a_copy_pasted_elsewhere_or_after_a_delete_keeps_its_preparation() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let p = prepared(&window);
        let document = p.document.clone();
        let mut clipboard = copy(&document, p.model);
        let original = hollow_of(&document, p.model).expect("hollow");

        // Another window, its own scene: a plain cube.
        let other_window = MainWindow::new().unwrap();
        let other = window_with_cube(&other_window, [60.0, 60.0, 10.0]);
        let copy = paste(&other.document, &mut clipboard, [130.0, 60.0]);
        let (kept, drains) = hollow_of(&other.document, copy).expect("the hollow came along");
        assert!(Arc::ptr_eq(&kept, &original.0), "with its original");
        assert_eq!(drains, 1);
        let set = set_of(&other.document, copy).expect("a set in force there");
        assert!(set.session.hand_edited());
        assert!(other.document.undo().unwrap());
        assert_eq!(leaf_count(&other.document), 1, "one undo step");

        // Deleted, then pasted back.
        document.set_selection(Selection::One(LeafId(p.model)));
        assert!(document.delete_selected_leaf().unwrap());
        let back = paste(&document, &mut clipboard, [40.0, 40.0]);
        assert_eq!(hollow_of(&document, back).map(|h| h.1), Some(1));
        let set = set_of(&document, back).expect("its set");
        assert_eq!(
            document
                .with_scene(|s| sets_of_model(s, &document.support_sessions(), back))
                .unwrap()
                .len(),
            1
        );
        assert!(set.session.hand_edited());
    });
}

/// A model with neither a hollow made here nor supports pastes as it did:
/// on the plate.
#[test]
fn a_plain_model_still_lands_on_the_plate() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let p = window_with_cube(&window, MODEL_AT);
        let mut clipboard = copy(&p.document, p.model);
        let pasted = paste(&p.document, &mut clipboard, [110.0, 40.0]);
        assert!((translation_of(&p.document, pasted)[2] - 10.0).abs() < 1e-9);
        assert_eq!(leaf_count(&p.document), 2);
    });
}

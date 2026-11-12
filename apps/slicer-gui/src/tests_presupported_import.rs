/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A pre-supported file in a window (#258, #259, #260): the question when it
//! opens, the 2 objects Separate makes in one undo step, the imported
//! supports following the model in the plate plane and making way for
//! supports made here otherwise, the Supports tab's Remove and Replace, and
//! the Remove tool taking off one body.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use sc_geom::df_supports::{is_imported_supports, scene_model_leaves};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::MeshMetadata;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::node::LeafPayload;
use sc_scene::{NodeId, Scene};

use crate::MainWindow;
use crate::error_dialog::DialogManager;
use crate::imported_supports::{body_at, imported_of, without_faces};
use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
use crate::presupported_import::PresupportedImport;
use crate::scene_document::{SceneDocument, SceneEdit};
use crate::slint_ui::{MessageAnswer, MessageKind};
use crate::test_support::slint_thread::on_fresh_slint_thread;

/// A mesh made of closed shells, each on its own vertices.
#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    faces: Vec<IndexedTriangle>,
}

impl Builder {
    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        self.vertices
            .push(Vertex::new([p[0] as f32, p[1] as f32, p[2] as f32]));
        (self.vertices.len() - 1) as u32
    }

    fn cuboid(&mut self, min: [f64; 3], max: [f64; 3]) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ]
        .map(|p| self.vertex(p));
        for [a, b, d] in [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ] {
            self.faces
                .push(IndexedTriangle::from_raw_indices([c[a], c[b], c[d]]));
        }
    }

    /// A closed frustum standing on `(x, y)` from `z0` (radius `r0`) to `z1`
    /// (radius `r1`), with 8 sides.
    fn frustum(&mut self, x: f64, y: f64, z0: f64, z1: f64, r0: f64, r1: f64) {
        let n = 8;
        let ring = |b: &mut Self, z: f64, r: f64| -> Vec<u32> {
            (0..n)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / n as f64;
                    b.vertex([x + r * a.cos(), y + r * a.sin(), z])
                })
                .collect()
        };
        let bottom = ring(self, z0, r0);
        let top = ring(self, z1, r1);
        let cb = self.vertex([x, y, z0]);
        let ct = self.vertex([x, y, z1]);
        for i in 0..n {
            let j = (i + 1) % n;
            for f in [
                [bottom[i], bottom[j], top[j]],
                [bottom[i], top[j], top[i]],
                [cb, bottom[j], bottom[i]],
                [ct, top[i], top[j]],
            ] {
                self.faces.push(IndexedTriangle::from_raw_indices(f));
            }
        }
    }
}

/// Where the 6 supports stand.
const SPOTS: [[f64; 2]; 6] = [
    [-7.0, -7.0],
    [0.0, -7.0],
    [7.0, -7.0],
    [-7.0, 7.0],
    [0.0, 7.0],
    [7.0, 7.0],
];

/// A pre-supported file as makers export them: a box of a model with a
/// horn, on 6 supports (shaft and tip cone) over a raft, every piece a closed
/// shell of its own; and how many faces the model has.
fn presupported_mesh() -> (IndexedMesh, usize) {
    let mut b = Builder::default();
    b.cuboid([-10.0, -10.0, 5.0], [10.0, 10.0, 25.0]);
    b.frustum(0.0, 0.0, 25.0, 30.0, 1.5, 0.2);
    let model_faces = b.faces.len();
    b.cuboid([-15.0, -15.0, 0.0], [15.0, 15.0, 1.0]);
    for [x, y] in SPOTS {
        b.frustum(x, y, 1.0, 4.2, 0.5, 0.5);
        b.frustum(x, y, 4.2, 5.0, 0.5, 0.15);
    }
    let meta = MeshMetadata {
        name: Some("werewolf".to_owned()),
        ..Default::default()
    };
    (
        IndexedMesh::new(Some(meta), b.vertices, b.faces),
        model_faces,
    )
}

/// A window with the pre-supported file open, as a mesh file opens, and
/// the question it brought answered by nobody yet.
struct Opened {
    window: MainWindow,
    document: Rc<SceneDocument>,
    _run: Rc<PresupportedImport>,
    file: Arc<IndexedMesh>,
    model_faces: usize,
}

fn open() -> Opened {
    let window = MainWindow::new().expect("the headless platform creates a window");
    let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
    let dialogs = DialogManager::new(&bus);
    dialogs.attach(&window);
    let document = SceneDocument::new();
    let run = PresupportedImport::install(&window, document.clone(), dialogs);
    let (mesh, model_faces) = presupported_mesh();
    let file = Arc::new(mesh);
    let mut graph = Scene::new();
    let leaf = graph
        .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(file.clone()))
        .expect("leaf");
    graph.add_root(leaf).expect("root");
    document.install_graph(graph, PathBuf::from("/tmp/werewolf.stl"));
    run.settle_for_test();
    Opened {
        window,
        document,
        _run: run,
        file,
        model_faces,
    }
}

/// The scene's leaves, in list order, with their meshes.
fn scene_leaves(document: &SceneDocument) -> Vec<(NodeId, Arc<IndexedMesh>)> {
    document
        .with_scene(|spatial| {
            (0..)
                .map_while(|i| spatial.leaf_id_at_index(i))
                .filter_map(|id| Some((id, spatial.graph().leaf_mesh_arc(id)?.clone())))
                .collect()
        })
        .unwrap_or_default()
}

/// Answers Separate, and returns the model and its imported supports.
fn separate(opened: &Opened) -> (NodeId, NodeId) {
    assert!(opened.window.get_message_open(), "the question is up");
    assert_eq!(
        opened.window.get_message().kind,
        MessageKind::SeparateSupports
    );
    opened
        .window
        .invoke_message_answered(MessageAnswer::Separate);
    assert!(!opened.window.get_message_open(), "nothing more is asked");
    let leaves = scene_leaves(&opened.document);
    assert_eq!(leaves.len(), 2, "the model and its supports");
    assert!(opened.document.supports_wanted().is_none(), "nothing waits");
    (leaves[0].0, leaves[1].0)
}

#[test]
fn replace_gives_the_model_supports_made_here_in_one_undo_step() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let doc = &opened.document;
        // Enter replaces.
        opened
            .window
            .invoke_message_answered(MessageAnswer::Confirm);
        assert!(!opened.window.get_message_open(), "nothing more is asked");
        let leaves = scene_leaves(doc);
        assert_eq!(leaves.len(), 1, "the model alone");
        assert_eq!(leaves[0].1.faces().len(), opened.model_faces);
        let model = leaves[0].0;
        assert_eq!(doc.selection_primary_node_id(), Some(model));
        let (_, wanted) = doc.supports_wanted().expect("the model waits");
        assert_eq!(wanted.ids(), [model]);
        // One undo step brings the file back as it came, and asks nothing.
        assert!(doc.undo().expect("undoes"));
        let back = scene_leaves(doc);
        assert_eq!(back.len(), 1);
        assert!(Arc::ptr_eq(&back[0].1, &opened.file));
        opened._run.settle_for_test();
        assert!(!opened.window.get_message_open());
    });
}

/// Three pre-supported files that come in together get one question
/// (#527): Replace answers for all of them in one undo step, each model
/// waiting for supports made here.
#[test]
fn files_with_supports_that_come_in_together_get_one_question() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a window");
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let dialogs = DialogManager::new(&bus);
        dialogs.attach(&window);
        let document = SceneDocument::new();
        let run = PresupportedImport::install(&window, document.clone(), dialogs.clone());
        let (mesh, model_faces) = presupported_mesh();
        let mut graph = Scene::new();
        for x in [0.0, 40.0, 80.0] {
            let leaf = graph
                .add_leaf(
                    None,
                    at([x, 0.0, 0.0]),
                    LeafPayload::Mesh(Arc::new(mesh.clone())),
                )
                .expect("leaf");
            graph.add_root(leaf).expect("root");
        }
        document.install_graph(graph, PathBuf::from("/tmp/werewolves.stl"));
        run.settle_for_test();
        assert_eq!(dialogs.queued_for_test().len(), 1, "one question");
        let message = window.get_message();
        assert_eq!(message.kind, MessageKind::SeparateSupports);
        assert!(message.text.starts_with("3 models"), "{}", message.text);
        window.invoke_message_answered(MessageAnswer::Confirm);
        assert!(!window.get_message_open(), "nothing more is asked");
        let leaves = scene_leaves(&document);
        assert_eq!(leaves.len(), 3, "the models alone");
        assert!(leaves.iter().all(|(_, m)| m.faces().len() == model_faces));
        let (_, wanted) = document.supports_wanted().expect("the models wait");
        assert_eq!(wanted.ids().len(), 3, "every one");
        assert!(document.undo().expect("undoes"));
        assert_eq!(scene_leaves(&document).len(), 3, "the files back");
        assert!(
            scene_leaves(&document)
                .iter()
                .all(|(_, m)| m.faces().len() > model_faces)
        );
    });
}

/// The question on files with supports (#540) waits while the window loads
/// a file and for a moment after the last model came in, no longer: then it
/// asks at once.
#[test]
fn the_question_waits_for_files_still_loading_and_then_asks_at_once() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a window");
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let dialogs = DialogManager::new(&bus);
        dialogs.attach(&window);
        let document = SceneDocument::new();
        let run = PresupportedImport::install(&window, document.clone(), dialogs.clone());
        let (mesh, _) = presupported_mesh();
        let mut graph = Scene::new();
        for x in [0.0, 40.0] {
            let leaf = graph
                .add_leaf(
                    None,
                    at([x, 0.0, 0.0]),
                    LeafPayload::Mesh(Arc::new(mesh.clone())),
                )
                .expect("leaf");
            graph.add_root(leaf).expect("root");
        }
        // Another file is still loading.
        window.set_loading(true);
        document.install_graph(graph, PathBuf::from("/tmp/werewolves.stl"));
        run.work_for_test();
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(!run.tick_for_test(), "no question while a file loads");
        assert!(dialogs.queued_for_test().is_empty());
        window.set_loading(false);
        assert!(run.tick_for_test(), "then it asks at once");
        assert_eq!(dialogs.queued_for_test().len(), 1, "one question");
        assert!(window.get_message().text.starts_with("2 models"));
    });
}

fn world(document: &SceneDocument, leaf: NodeId) -> Transform3D {
    document
        .with_scene(|spatial| spatial.leaf(leaf).map(|q| q.world_transform()))
        .flatten()
        .expect("the leaf")
}

fn at(t: [f64; 3]) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation(t))
}

fn move_to(document: &SceneDocument, leaf: NodeId, to: Transform3D) {
    let edit = SceneEdit::SetLocalTransform {
        node_id: leaf,
        before: world(document, leaf),
        after: to,
    };
    document.apply_edit(edit.clone()).expect("applies");
    document.push_edit(edit);
}

#[test]
fn a_presupported_file_comes_apart_in_one_undo_step_when_asked() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let message = opened.window.get_message();
        assert_eq!(message.context.as_str(), "werewolf");
        assert!(
            message.text.contains("12 supports and a raft"),
            "{}",
            message.text
        );
        let (model, supports) = separate(&opened);
        let leaves = scene_leaves(&opened.document);
        assert_eq!(leaves[0].1.faces().len(), opened.model_faces);
        assert!(is_imported_supports(&leaves[1].1));
        assert_eq!(
            leaves[0].1.faces().len() + leaves[1].1.faces().len(),
            opened.file.faces().len()
        );
        assert_eq!(opened.document.selection_primary_node_id(), Some(model));
        // The list names them.
        let names = opened
            .document
            .with_scene(|spatial| {
                crate::viewport::object_list::build_object_list_names(spatial.graph())
            })
            .expect("a scene");
        assert_eq!(names, ["werewolf", "Imported supports"]);
        // The engine supports the model alone.
        let models = opened
            .document
            .with_scene(|spatial| scene_model_leaves(spatial.graph(), None).expect("models"))
            .expect("a scene");
        assert_eq!(
            models.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [model]
        );
        let owned = opened
            .document
            .with_scene(|spatial| imported_of(spatial, model, None))
            .expect("a scene");
        assert_eq!(owned, [supports]);

        // One undo step brings the file back as it came, and asks nothing.
        assert!(opened.document.undo().expect("undoes"));
        let back = scene_leaves(&opened.document);
        assert_eq!(back.len(), 1);
        assert!(Arc::ptr_eq(&back[0].1, &opened.file));
        assert!(!opened.window.get_message_open());
        assert!(opened.document.redo().expect("redoes"));
        assert_eq!(scene_leaves(&opened.document).len(), 2);
    });
}

#[test]
fn leave_as_is_keeps_the_file_as_it_came() {
    on_fresh_slint_thread(|| {
        let opened = open();
        opened.window.invoke_message_answered(MessageAnswer::Cancel);
        let leaves = scene_leaves(&opened.document);
        assert_eq!(leaves.len(), 1);
        assert!(Arc::ptr_eq(&leaves[0].1, &opened.file));
        assert!(!opened.window.get_message_open());
        // A new mesh for it, as Repair gives, asks nothing again.
        let (leaf, mesh) = leaves[0].clone();
        let swap = opened
            .document
            .with_scene(|spatial| {
                crate::scene_document::leaf_mesh_swap(
                    spatial.graph(),
                    leaf,
                    Arc::new(mesh.as_ref().clone()),
                )
            })
            .flatten()
            .expect("a swap");
        opened.document.apply_edit(swap.clone()).expect("applies");
        opened.document.push_edit(swap);
        opened._run.settle_for_test();
        assert!(!opened.window.get_message_open());
    });
}

#[test]
fn imported_supports_follow_their_model_and_make_way_for_a_lift() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let (model, supports) = separate(&opened);
        let doc = &opened.document;
        // A move in X/Y takes them along, in the same undo step.
        move_to(doc, model, at([10.0, 5.0, 0.0]));
        assert_eq!(world(doc, supports), at([10.0, 5.0, 0.0]));
        assert!(doc.undo().expect("undoes"));
        assert_eq!(world(doc, supports), Transform3D::Identity);
        assert_eq!(world(doc, model), Transform3D::Identity);
        assert!(doc.redo().expect("redoes"));
        // A turn about the vertical axis too.
        let turned = Transform3D::Affine(AffineTransform3D::from_rotation_z(0.5))
            .compose(&world(doc, model));
        move_to(doc, model, turned);
        assert_eq!(world(doc, supports), world(doc, model));
        // A lift takes them away and leaves the model waiting for supports
        // made here, in the same step.
        let lifted = at([0.0, 0.0, 3.0]).compose(&world(doc, model));
        move_to(doc, model, lifted);
        assert_eq!(scene_leaves(doc).len(), 1, "the imported supports went");
        let (_, wanted) = doc.supports_wanted().expect("the model waits");
        assert_eq!(wanted.ids(), [model]);
        assert!(doc.undo().expect("undoes"));
        assert_eq!(scene_leaves(doc).len(), 2, "back with the lift undone");
    });
}

#[test]
fn the_supports_tab_removes_or_replaces_imported_supports() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let (model, _) = separate(&opened);
        let doc = &opened.document;
        // The raft and the 12 pieces of the supports.
        assert_eq!(opened.window.get_imported_support_bodies(), 13);
        opened.window.invoke_imported_supports_remove_requested();
        assert_eq!(scene_leaves(doc).len(), 1);
        assert_eq!(opened.window.get_imported_support_bodies(), 0);
        assert!(doc.supports_wanted().is_none(), "nothing waits");
        assert!(doc.undo().expect("undoes"));
        assert_eq!(scene_leaves(doc).len(), 2);
        doc.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
            model,
        )));
        opened.window.invoke_imported_supports_replace_requested();
        assert_eq!(scene_leaves(doc).len(), 1);
        let (_, wanted) = doc.supports_wanted().expect("the model waits");
        assert_eq!(wanted.ids(), [model]);
    });
}

#[test]
fn one_body_of_imported_supports_comes_off_alone() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let (model, supports) = separate(&opened);
        let doc = &opened.document;
        // A point on the side of the first shaft, as a pick lands.
        let side = [SPOTS[0][0] + 0.5, SPOTS[0][1], 2.5];
        let (body, mesh) = doc
            .with_scene(|spatial| {
                let body = body_at(spatial, &[supports], side)?;
                Some((body, spatial.graph().leaf_mesh_arc(supports)?.clone()))
            })
            .flatten()
            .expect("the shaft is there");
        // The shaft's 32 faces, nothing of the raft or the other shafts.
        assert_eq!(body.faces.len(), 32);
        let rest = without_faces(&mesh, &body.faces).expect("bodies are left");
        assert_eq!(rest.faces().len(), mesh.faces().len() - 32);
        assert!(is_imported_supports(&rest));
        // On the model, or off every body, there is none.
        let none = doc
            .with_scene(|spatial| {
                body_at(spatial, &[supports], [0.0, 0.0, 15.0])
                    .or_else(|| body_at(spatial, &[supports], [0.0, 0.0, 10.0]))
            })
            .flatten();
        assert!(none.is_none());
        // After a move, the body is found where it went.
        move_to(doc, model, at([20.0, 0.0, 0.0]));
        let moved = doc
            .with_scene(|spatial| body_at(spatial, &[supports], [side[0] + 20.0, side[1], side[2]]))
            .flatten()
            .expect("found where it went");
        assert_eq!(moved.faces, body.faces);
    });
}

fn select(document: &SceneDocument, leaf: NodeId) {
    document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
        leaf,
    )));
}

/// Where the leaf's origin stands in the world.
fn origin(document: &SceneDocument, leaf: NodeId) -> [f32; 3] {
    world(document, leaf)
        .apply_to_vertex(Vertex::new([0.0; 3]))
        .coords()
}

#[test]
fn a_model_deleted_takes_its_imported_supports_along() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let (model, supports) = separate(&opened);
        let doc = &opened.document;
        // The supports by themselves go by themselves.
        select(doc, supports);
        assert!(doc.delete_selected_leaf().expect("deletes"));
        assert_eq!(scene_leaves(doc).len(), 1);
        assert!(doc.undo().expect("undoes"));
        // The model takes them along, in one undo step.
        select(doc, model);
        assert!(doc.delete_selected_leaf().expect("deletes"));
        assert!(scene_leaves(doc).is_empty(), "nothing is left behind");
        assert!(doc.undo().expect("undoes"));
        assert_eq!(scene_leaves(doc).len(), 2);
        let owned = doc
            .with_scene(|spatial| imported_of(spatial, model, None))
            .expect("a scene");
        assert_eq!(owned, [supports], "back with their model");
    });
}

#[test]
fn a_copy_of_the_model_takes_its_imported_supports_along() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let (model, _) = separate(&opened);
        let doc = &opened.document;
        select(doc, model);
        let clipboard =
            crate::clipboard::clipboard_from_document_selection(doc).expect("a clipboard");
        let copy = crate::clipboard::paste(doc, &clipboard, [80.0, 0.0], 0.0)
            .expect("pastes")
            .expect("a copy");
        assert_eq!(scene_leaves(doc).len(), 4);
        let theirs = doc
            .with_scene(|spatial| imported_of(spatial, copy, None))
            .expect("a scene");
        assert_eq!(theirs.len(), 1, "the copy has supports of its own");
        // Under the copy, as the model's are under it: the copy keeps the
        // model's height, so they still hold it.
        let (o_copy, o_set) = (origin(doc, copy), origin(doc, theirs[0]));
        for k in 0..3 {
            assert!((o_copy[k] - o_set[k]).abs() < 1e-4, "{o_copy:?} {o_set:?}");
        }
        assert!(o_copy[0] > 20.0, "away from the model: {o_copy:?}");
        assert!(o_copy[2].abs() < 1e-4);
        // One undo step takes both.
        assert!(doc.undo().expect("undoes"));
        assert_eq!(scene_leaves(doc).len(), 2);
    });
}

#[test]
fn fill_bed_copies_take_the_imported_supports_along() {
    on_fresh_slint_thread(|| {
        let opened = open();
        let (model, _) = separate(&opened);
        let doc = &opened.document;
        select(doc, model);
        let plate = sc_arrange::Plate {
            min: [-25.0, -25.0],
            max: [200.0, 25.0],
            floor: 0.0,
        };
        let options = crate::arrange_run::ArrangeOptions {
            max_copies: 2,
            ..Default::default()
        };
        let (filled, _) = crate::arrange_run::fill_now(doc, &plate, options).expect("filled");
        assert_eq!(filled.copies.len(), 2);
        let leaves = scene_leaves(doc);
        let models: Vec<NodeId> = leaves
            .iter()
            .filter(|(_, mesh)| !is_imported_supports(mesh))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(models.len(), 3);
        assert_eq!(leaves.len(), 6, "each model with its supports");
        for &m in &models {
            let theirs = doc
                .with_scene(|spatial| imported_of(spatial, m, None))
                .expect("a scene");
            assert_eq!(theirs.len(), 1, "each model has its own");
            let (o_model, o_set) = (origin(doc, m), origin(doc, theirs[0]));
            for k in 0..3 {
                assert!(
                    (o_model[k] - o_set[k]).abs() < 1e-3,
                    "{o_model:?} {o_set:?}"
                );
            }
        }
        assert!(doc.undo().expect("undoes"));
        assert_eq!(scene_leaves(doc).len(), 2);
    });
}

/// A pre-supported file already separated: the model and its imported
/// supports (the raft and the 12 pieces of [`presupported_mesh`]), each a
/// root leaf standing where the file had it.
pub(crate) fn separated_scene() -> (Scene, NodeId, NodeId) {
    let mut model = Builder::default();
    model.cuboid([-10.0, -10.0, 5.0], [10.0, 10.0, 25.0]);
    model.frustum(0.0, 0.0, 25.0, 30.0, 1.5, 0.2);
    let mut supports = Builder::default();
    supports.cuboid([-15.0, -15.0, 0.0], [15.0, 15.0, 1.0]);
    for [x, y] in SPOTS {
        supports.frustum(x, y, 1.0, 4.2, 0.5, 0.5);
        supports.frustum(x, y, 4.2, 5.0, 0.5, 0.15);
    }
    let named = |name: &str| {
        Some(MeshMetadata {
            name: Some(name.to_owned()),
            ..Default::default()
        })
    };
    let model = IndexedMesh::new(named("werewolf"), model.vertices, model.faces);
    let supports = IndexedMesh::new(
        named(sc_mesh_core::presupported::IMPORTED_SUPPORTS_NAME),
        supports.vertices,
        supports.faces,
    );
    let mut graph = Scene::new();
    let mut leaf = |mesh: IndexedMesh| {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("leaf");
        graph.add_root(leaf).expect("root");
        leaf
    };
    let (model, supports) = (leaf(model), leaf(supports));
    (graph, model, supports)
}

/// Where the shafts stand on the plate, for other modules' tests.
pub(crate) const SHAFT_SPOTS: [[f64; 2]; 6] = SPOTS;

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Supports and Repair in every order (#477), through the window's own
//! callbacks: a slab whose mesh has a crack the slicer refuses and Repair
//! welds, as on a real mini whose STL needs Repair to slice. Each flow ends
//! with a save and a reopen, which is where supports out of date used to be
//! lost: written as a plain mesh, no longer editable.

use std::path::PathBuf;
use std::time::Instant;

use sc_geom::df_supports::DfSupportOptions;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::SceneMetadata;
use sc_scene::node::LeafPayload;

use super::*;
use crate::MessageAnswer;
use crate::WorkspaceSection;
use crate::df_support_run::DfSupportRun;
use crate::hollow_run::HollowRun;
use crate::preferences_bus::PreferencesSnapshot;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::ViewportController;

/// The longest a worker may take before the test calls it hung.
const PATIENCE: Duration = Duration::from_secs(300);

struct Window {
    window: MainWindow,
    ctrl: Rc<ViewportController>,
    document: Rc<SceneDocument>,
    slice: Rc<SliceRun>,
    supports: Rc<DfSupportRun>,
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
    document.install_graph(graph, PathBuf::from("slab.stl"));
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
        supports.clone(),
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
        ctrl,
        document,
        slice,
        supports,
    }
}

/// What a flow left, as the person meets it.
#[derive(Debug, Default)]
struct Outcome {
    /// The last slice landed.
    sliced: bool,
    /// Support leaves in the scene at the save.
    support_leaves: usize,
    /// Of them, editable now, before the save.
    in_force: usize,
    /// Of them, saved as data: editable again on reopening.
    saved_as_data: usize,
    /// Sets rebuilt on reopening.
    reopened_editable: usize,
    /// Every card heading the flow showed, in order.
    cards: Vec<String>,
}

impl Window {
    fn generate(&self) {
        self.supports.generate_now(DfSupportOptions::default());
        self.close_cards();
    }

    /// Presses Slice, waits for the run and hands its wake-up to the window.
    fn press_slice(&self) {
        self.window.invoke_slice_requested();
        self.finish_slice();
    }

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

    fn finish_repair(&self) {
        let start = Instant::now();
        while self.window.get_repairing_in_progress() {
            assert!(start.elapsed() < PATIENCE, "the repair worker hangs");
            std::thread::sleep(Duration::from_millis(5));
            self.window.invoke_repair_finished();
        }
    }

    /// The Transform tab's Repair button, then back to the Slice tab.
    fn press_repair(&self) {
        self.ctrl.set_section(WorkspaceSection::Transform);
        self.window.invoke_repair_requested();
        assert!(self.window.get_repairing_in_progress(), "Repair runs");
        self.finish_repair();
        self.close_cards();
        self.ctrl.set_section(WorkspaceSection::Slice);
    }

    /// Slice, refused; the card's Repair; the second slice.
    fn slice_and_repair_from_the_card(&self, cards: &mut Vec<String>) {
        self.press_slice();
        let card = self.window.get_message();
        cards.push(card.heading.to_string());
        assert_eq!(card.action, "Repair", "the slice is refused with Repair");
        self.window.invoke_message_answered(MessageAnswer::Confirm);
        self.finish_repair();
        if self.window.get_slicing_in_progress() {
            self.finish_slice();
        }
    }

    /// Notes and closes every card on show or waiting, as the person would.
    fn close_cards(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for _ in 0..20 {
            if !self.window.get_message_open() {
                return seen;
            }
            let card = self.window.get_message();
            seen.push(card.heading.to_string());
            self.window.invoke_message_answered(MessageAnswer::Close);
        }
        panic!("the cards do not close");
    }

    /// Saves, reopens and counts what came back editable.
    fn save_and_reopen(&self, mut cards: Vec<String>) -> Outcome {
        cards.extend(self.close_cards());
        let support_leaves = self
            .document
            .with_scene(|spatial| support_persist::support_leaves(spatial).count())
            .unwrap_or_default();
        let in_force = self
            .document
            .with_scene(|spatial| {
                support_persist::sets_in_force(spatial, &self.document.support_sessions()).len()
            })
            .unwrap_or_default();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("slab.bread");
        crate::scene_save::save_project_to(&self.document, &target, &PrintDefaults::default())
            .expect("the project saves");
        cards.extend(self.close_cards());
        let loaded = sc_project_formats::load_scene(&target, None, |_| {}).unwrap();
        let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
            panic!("a .bread opens as a project");
        };
        let saved_as_data = project.supports.len();
        let mut scene = project.scene;
        let restored = support_persist::restore_supports(&mut scene, project.supports);
        Outcome {
            sliced: self.document.slice_params().is_some(),
            support_leaves,
            in_force,
            saved_as_data,
            reopened_editable: restored.sets.len(),
            cards,
        }
    }
}

/// The slab's defect, as an STL may bring it.
#[derive(Clone, Copy, Debug)]
enum Defect {
    /// A crack along 2 edges of the top: its corner vertex is a copy of
    /// the sides' one. Repair welds it back: the same faces, the same shape.
    Crack,
    /// One of the top's 2 faces missing. Repair fills the hole: a face
    /// more, the same shape, as a repair that collapses or fills faces
    /// changes the count of a real mini.
    Hole,
}

/// A slab 20 × 10 × 2 mm, its underside 9 mm up in the scene, with
/// `defect`, which the slicer refuses and Repair fixes; `None` whole.
fn slab(defect: Option<Defect>) -> IndexedMesh {
    let cube = sc_mesh_fixtures::unit_cube_mesh();
    let mut vertices: Vec<Vertex> = cube
        .vertices()
        .iter()
        .map(|v| {
            let p = v.as_array();
            Vertex::new([(p[0] + 0.5) * 20.0, (p[1] + 0.5) * 10.0, (p[2] + 0.5) * 2.0])
        })
        .collect();
    let mut faces = cube.faces().to_vec();
    match defect {
        None => {}
        Some(Defect::Crack) => {
            vertices.push(vertices[6]);
            // The top's 2 faces take the copy of corner 6.
            for face in &mut faces[2..4] {
                let raw = face.vertices.map(|i| i.get());
                *face = IndexedTriangle::from_raw_indices(raw.map(|i| if i == 6 { 8 } else { i }));
            }
        }
        Some(Defect::Hole) => {
            faces.remove(3);
        }
    }
    IndexedMesh::new(None, vertices, faces)
}

fn at(x: f64) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation([x, 20.0, 9.0]))
}

/// One slab, inline.
fn one(mesh: IndexedMesh) -> Scene {
    let mut graph = Scene::new();
    let leaf = graph
        .add_leaf(
            Some(SceneMetadata {
                name: Some("Slab".to_owned()),
                comment: None,
            }),
            at(10.0),
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("leaf");
    graph.add_root(leaf).expect("root");
    graph
}

/// Two copies of one slab sharing its mesh, as pasted copies do.
fn two_sharing(mesh: IndexedMesh) -> Scene {
    let mut graph = Scene::new();
    let resource = graph.add_mesh_resource(mesh);
    for x in [10.0, 50.0] {
        let leaf = graph
            .add_leaf(None, at(x), LeafPayload::MeshResource(resource))
            .expect("leaf");
        graph.add_root(leaf).expect("root");
    }
    graph
}

/// Every order of Repair, supports and Slice, each to a save and a reopen,
/// for `defect`.
fn every_order(defect: Defect, rows: &mut Vec<(String, Outcome)>) {
    let broken = || slab(Some(defect));
    let mut row =
        |flow: &str, outcome: Outcome| rows.push((format!("{defect:?}: {flow}"), outcome));

    // Repaired first, then supports, then Slice.
    let w = window_with(one(broken()));
    w.press_repair();
    w.generate();
    w.press_slice();
    row("repair, supports, slice", w.save_and_reopen(Vec::new()));

    // Supports first; the slice is refused and its card repairs.
    let w = window_with(one(broken()));
    w.generate();
    let mut cards = Vec::new();
    w.slice_and_repair_from_the_card(&mut cards);
    row("supports, slice, card's Repair", w.save_and_reopen(cards));

    // Supports first, then the Transform tab's Repair button.
    let w = window_with(one(broken()));
    w.generate();
    w.press_repair();
    w.press_slice();
    row(
        "supports, Repair button, slice",
        w.save_and_reopen(Vec::new()),
    );

    // Never repaired: the slice is refused and the card closed.
    let w = window_with(one(broken()));
    w.generate();
    w.press_slice();
    let cards = w.close_cards();
    row(
        "supports, slice refused, no repair",
        w.save_and_reopen(cards),
    );

    // Two copies sharing the mesh, both supported, repaired at the slice.
    let w = window_with(two_sharing(broken()));
    w.generate();
    let mut cards = Vec::new();
    w.slice_and_repair_from_the_card(&mut cards);
    row("2 sharing copies, card's Repair", w.save_and_reopen(cards));

    // Repaired, then the repair undone.
    let w = window_with(one(broken()));
    w.generate();
    w.press_repair();
    w.document.undo().expect("undo");
    row("supports, Repair, Undo", w.save_and_reopen(Vec::new()));

    // Repaired, undone, redone.
    let w = window_with(one(broken()));
    w.generate();
    w.press_repair();
    w.document.undo().expect("undo");
    w.document.redo().expect("redo");
    row(
        "supports, Repair, Undo, Redo",
        w.save_and_reopen(Vec::new()),
    );
}

/// Every order of Repair, supports and Slice, for each defect, and a whole
/// model whose only meshes Repair could touch are the supports'.
#[test]
fn every_order_of_repair_and_supports() {
    on_fresh_slint_thread(|| {
        let mut rows: Vec<(String, Outcome)> = Vec::new();
        every_order(Defect::Crack, &mut rows);
        every_order(Defect::Hole, &mut rows);

        let w = window_with(one(slab(None)));
        w.generate();
        w.press_repair();
        w.press_slice();
        rows.push((
            "whole: supports, Repair button, slice".to_owned(),
            w.save_and_reopen(Vec::new()),
        ));

        for (flow, o) in &rows {
            eprintln!(
                "{flow:<46} sliced {:<5} supports {} in force {} saved as data {} editable on reopening {} cards {:?}",
                o.sliced,
                o.support_leaves,
                o.in_force,
                o.saved_as_data,
                o.reopened_editable,
                o.cards
            );
        }
        for (flow, o) in &rows {
            // Every set stays editable: now, saved, and reopened.
            assert!(o.support_leaves > 0, "{flow}: supports");
            assert_eq!(o.in_force, o.support_leaves, "{flow}: in force");
            assert_eq!(o.saved_as_data, o.support_leaves, "{flow}: saved as data");
            assert_eq!(o.reopened_editable, o.support_leaves, "{flow}: reopened");
            assert!(
                !o.cards.iter().any(|c| c == "Supports out of date"),
                "{flow}: every tip still touches"
            );
            // The Undo flows never press Slice.
            let unrepaired = flow.contains("no repair") || flow.contains("Undo");
            assert_eq!(o.sliced, !unrepaired, "{flow}: sliced");
        }
    });
}

/// Damian's plate (#477), with a real model whose STL needs Repair to slice
/// (`BREADSLICER_REPAIR_MODEL`): 2 copies sharing its mesh, one twice the
/// other's size, supported, the slice refused and repaired from its card,
/// then saved and reopened.
#[test]
#[ignore]
fn a_real_model_keeps_its_supports_through_the_cards_repair() {
    let Ok(path) = std::env::var("BREADSLICER_REPAIR_MODEL") else {
        return;
    };
    on_fresh_slint_thread(move || {
        let loaded = sc_project_formats::load_scene(std::path::Path::new(&path), None, |_| {})
            .expect("the model opens");
        let sc_project_formats::LoadedContent::Mesh(file) = loaded.content else {
            panic!("a mesh file");
        };
        let mut mesh = None;
        file.for_each_expanded_leaf::<sc_scene::SceneError, _>(
            sc_scene::CsgPolicy::Refuse,
            |_, _, m| {
                mesh.get_or_insert_with(|| m.clone());
                Ok(())
            },
        )
        .expect("its leaves");
        let mesh = mesh.expect("a mesh");
        let (lo, hi) =
            mesh.vertices()
                .iter()
                .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                    let p = v.as_array();
                    (
                        std::array::from_fn(|k| lo[k].min(p[k])),
                        std::array::from_fn(|k| hi[k].max(p[k])),
                    )
                });
        let centre = |k: usize| f64::from(lo[k] + hi[k]) * 0.5;
        let mut graph = Scene::new();
        let resource = graph.add_mesh_resource(mesh);
        for (scale, x) in [(1.0, 40.0), (2.0, 110.0)] {
            let place = AffineTransform3D::from_linear_and_translation(
                [[scale, 0.0, 0.0], [0.0, scale, 0.0], [0.0, 0.0, scale]],
                [
                    x - centre(0) * scale,
                    60.0 - centre(1) * scale,
                    5.0 - f64::from(lo[2]) * scale,
                ],
            );
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(place),
                    LeafPayload::MeshResource(resource),
                )
                .expect("leaf");
            graph.add_root(leaf).expect("root");
        }
        let w = window_with(graph);
        let started = Instant::now();
        w.generate();
        eprintln!("generated in {:.1} s", started.elapsed().as_secs_f64());
        let mut cards = Vec::new();
        let started = Instant::now();
        w.slice_and_repair_from_the_card(&mut cards);
        eprintln!(
            "refused, repaired and sliced again in {:.1} s",
            started.elapsed().as_secs_f64()
        );
        let o = w.save_and_reopen(cards);
        eprintln!(
            "sliced {} supports {} in force {} saved as data {} editable on reopening {} cards {:?}",
            o.sliced, o.support_leaves, o.in_force, o.saved_as_data, o.reopened_editable, o.cards
        );
    });
}

/// Analyze islands on a model the slicer refuses (#526): Repair runs on its
/// own and the analysis goes on, with no card to answer and no question at
/// its end; the supports stay editable on the repaired model, so a hand
/// edit goes through at once.
#[test]
fn analyzing_islands_repairs_on_its_own_and_keeps_the_supports() {
    on_fresh_slint_thread(|| {
        for defect in [Defect::Crack, Defect::Hole] {
            let w = window_with(one(slab(Some(defect))));
            w.generate();
            w.ctrl.set_section(WorkspaceSection::Supports);
            w.window.invoke_support_islands_analyze_requested();
            w.finish_slice();
            assert!(
                !w.window.get_message_open(),
                "{defect:?}: no card to answer"
            );
            assert!(
                w.window.get_repairing_in_progress(),
                "{defect:?}: Repair runs on its own"
            );
            w.finish_repair();
            assert!(
                w.window.get_slicing_in_progress(),
                "{defect:?}: the analysis goes on"
            );
            w.finish_slice();
            let cards = w.close_cards();
            assert!(
                cards
                    .iter()
                    .all(|c| !c.contains("have no support") && c != "Supports out of date"),
                "{defect:?}: {cards:?}"
            );
            assert!(w.document.slice_params().is_some(), "{defect:?}: sliced");
            assert_eq!(
                w.window.get_workspace_section(),
                WorkspaceSection::Supports,
                "{defect:?}: back on the Supports tab"
            );
            let in_force = w
                .document
                .with_scene(|spatial| {
                    support_persist::sets_in_force(spatial, &w.document.support_sessions()).len()
                })
                .unwrap_or_default();
            assert_eq!(in_force, 1, "{defect:?}: the supports stay editable");
        }
    });
}

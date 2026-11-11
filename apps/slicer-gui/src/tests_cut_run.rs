/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Headless tests of the Cut tool (#56): the cut is one undo step, the card
//! says how many pieces it leaves before it is taken, a click on a part of
//! the cut takes it or leaves it, and a bounded plane that crosses a part
//! does not cut.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;

use super::*;
use crate::test_support::slint_thread::on_fresh_slint_thread;

/// A 20 mm cube.
fn cube() -> IndexedMesh {
    let mut cube = sc_mesh_fixtures::unit_cube_mesh();
    for v in &mut cube.vertices {
        *v = Vertex::new(v.as_array().map(|c| c * 20.0));
    }
    cube
}

/// Two 20 mm cubes side by side, 10 mm apart, as one model.
fn two_cubes() -> IndexedMesh {
    let one = cube();
    let n = one.vertices.len() as u32;
    let mut vertices = one.vertices.clone();
    vertices.extend(
        one.vertices
            .iter()
            .map(|v| Vertex::new([v.as_array()[0] + 30.0, v.as_array()[1], v.as_array()[2]])),
    );
    let mut faces = one.faces.clone();
    faces.extend(
        one.faces.iter().map(|f| {
            sc_mesh_core::IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + n))
        }),
    );
    IndexedMesh::new(None, vertices, faces)
}

struct Fixture {
    window: MainWindow,
    document: Rc<SceneDocument>,
    run: Rc<CutRun>,
    leaf: NodeId,
}

/// A window with `mesh` 10 mm right of the origin, selected, and the Cut
/// tool installed.
fn fixture(mesh: IndexedMesh) -> Fixture {
    let window = MainWindow::new().expect("the headless platform creates a main window");
    let ctrl = Rc::new(ViewportController::new_for_test());
    let document = ctrl.document();
    let mut graph = Scene::new();
    let leaf = graph
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .unwrap();
    graph.add_root(leaf).unwrap();
    document.install_graph(graph, PathBuf::from("cube.stl"));
    document.set_selection(Selection::One(LeafId(leaf)));
    let run = CutRun::install(&window, document.clone(), ctrl.dialogs(), ctrl);
    Fixture {
        window,
        document,
        run,
        leaf,
    }
}

/// Waits for a worker of the tool and takes its result, as the event loop
/// would.
fn land(run: &CutRun) {
    let start = Instant::now();
    while run
        .slot
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_none()
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the worker hangs"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    run.worked();
}

/// Opens the tool and waits until the model is ready and the cut worked out.
fn opened(f: &Fixture) {
    f.run.toggle();
    assert!(f.window.get_cut_open());
    land(&f.run);
    f.run.on_option(-1, 0.0);
    land(&f.run);
}

fn leaves(f: &Fixture) -> usize {
    f.document
        .with_scene(|spatial| (0..).map_while(|i| spatial.leaf_id_at_index(i)).count())
        .unwrap_or(0)
}

#[test]
fn a_cut_leaves_two_pieces_in_one_undo_step() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        opened(&f);
        assert!(f.window.get_cut_can_cut(), "{}", f.window.get_cut_warning());
        assert!(
            f.window.get_cut_status().contains('2'),
            "the card says it leaves two pieces: {}",
            f.window.get_cut_status()
        );
        assert_eq!(f.window.get_cut_height(), 0.0, "through the middle");
        f.run.apply();
        assert!(!f.window.get_cut_open(), "the tool closes");
        assert_eq!(leaves(&f), 2);
        assert!(
            f.document
                .with_scene(|s| s.graph().node(f.leaf).is_none())
                .unwrap()
        );

        assert!(f.document.undo().unwrap());
        assert_eq!(leaves(&f), 1);
        assert!(
            f.document
                .with_scene(|s| s.graph().node(f.leaf).is_some())
                .unwrap()
        );
        assert!(!f.document.can_undo());
        assert!(f.document.redo().unwrap());
        assert_eq!(leaves(&f), 2);
    });
}

#[test]
fn a_click_on_a_part_of_the_cut_leaves_it_whole() {
    on_fresh_slint_thread(|| {
        let f = fixture(two_cubes());
        opened(&f);
        assert!(f.window.get_cut_status().contains('2'));
        // Straight down onto the left cube's part of the plane (the cubes
        // span x 0 to 20 and 30 to 50, y -10 to 10).
        assert!(f.run.click([10.0, 0.0, 50.0], [0.0, 0.0, -1.0]));
        f.run.on_option(-1, 0.0);
        land(&f.run);
        assert!(f.window.get_cut_can_cut());
        f.run.apply();
        // The right cube cut in two, the left one whole.
        assert_eq!(leaves(&f), 3);
        // A click off every part is not the tool's.
        let g = fixture(cube());
        opened(&g);
        assert!(!g.run.click([100.0, 100.0, 50.0], [0.0, 0.0, -1.0]));
    });
}

#[test]
fn a_bounded_plane_across_a_part_does_not_cut() {
    on_fresh_slint_thread(|| {
        let f = fixture(two_cubes());
        opened(&f);
        f.run.on_option(3, 1.0);
        // 20 mm wide, centred on the model: it crosses both cubes.
        f.run.on_option(4, 20.0);
        assert!(!f.window.get_cut_can_cut());
        assert!(!f.window.get_cut_warning().is_empty());
        // Around the left cube alone.
        f.run.on_option(4, 28.0);
        f.run.on_option(5, 28.0);
        f.run.on_option(6, -15.0);
        f.run.on_option(-1, 0.0);
        land(&f.run);
        assert!(f.window.get_cut_can_cut(), "{}", f.window.get_cut_warning());
        f.run.apply();
        assert_eq!(leaves(&f), 3);
    });
}

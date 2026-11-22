/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Headless tests of the Cut tool (#56): the cut is one undo step, the card
//! says how many pieces it leaves before it is taken, a click on a part of
//! the cut takes it or leaves it, a bounded plane that crosses a part does
//! not cut, and the gizmo's handles move the plane in the scene.

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

/// The volumes of the scene's pieces, lowest piece first.
fn piece_volumes(f: &Fixture) -> Vec<f64> {
    let mut pieces: Vec<(f64, f64)> = f
        .document
        .with_scene(|spatial| {
            (0..)
                .map_while(|i| spatial.leaf_id_at_index(i))
                .filter_map(|id| {
                    let mesh = spatial.graph().leaf_mesh_arc(id)?;
                    let low = mesh
                        .vertices()
                        .iter()
                        .map(|v| v.coords()[2])
                        .fold(f32::INFINITY, f32::min);
                    Some((f64::from(low), bread_cut::volume(mesh)))
                })
                .collect()
        })
        .unwrap_or_default();
    pieces.sort_by(|a, b| a.0.total_cmp(&b.0));
    pieces.into_iter().map(|p| p.1).collect()
}

/// #109, #442: pins put a pin on the part below and a socket in the part
/// above; magnets a pocket in each. The card says so before the cut, and
/// the cut is one Undo step as any.
#[test]
fn a_cut_with_pins_or_magnets_joins_the_parts() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        opened(&f);
        f.run.on_option(8, 1.0);
        assert_eq!(f.window.get_cut_joint_kind(), 1);
        f.run.on_option(-1, 0.0);
        land(&f.run);
        assert!(
            f.window.get_cut_status().contains("Joints added: 1"),
            "{}",
            f.window.get_cut_status()
        );
        f.run.apply();
        assert_eq!(leaves(&f), 2);
        let half = 4000.0;
        let v = piece_volumes(&f);
        assert!(v[0] > half + 15.0, "the pin is on the part below: {v:?}");
        assert!(v[1] < half - 15.0, "the socket is in the part above: {v:?}");
        assert!(f.document.undo().unwrap());
        assert_eq!(leaves(&f), 1);

        // Magnets: a pocket on each side, the 5 x 2 mm preset. The model
        // is ready already: the tool opens on it at once.
        f.document.set_selection(Selection::One(LeafId(f.leaf)));
        f.run.toggle();
        assert!(f.window.get_cut_open());
        f.run.on_option(8, 2.0);
        f.run.on_option(13, 2.0);
        assert_eq!(f.window.get_cut_joint_size(), 5.0);
        assert_eq!(f.window.get_cut_joint_preset(), 2);
        f.run.on_option(-1, 0.0);
        land(&f.run);
        f.run.apply();
        let v = piece_volumes(&f);
        let pocket = std::f64::consts::PI * 2.6 * 2.6 * 2.1;
        for piece in &v {
            assert!((half - pocket - piece).abs() < pocket * 0.05, "{v:?}");
        }
    });
}

/// #440, #441: in Stroke mode a drag across the model cuts it along the
/// stroke, and a loop round a corner cuts that corner off; each a cut as
/// the plane's, one Undo step.
#[test]
fn a_stroke_across_the_model_cuts_it_along_the_stroke() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        opened(&f);
        f.run.on_option(14, 1.0);
        assert!(f.window.get_cut_stroke_mode());
        assert!(!f.window.get_cut_can_cut(), "nothing drawn yet");
        // From above, straight across the cube (x 0 to 20, y -10 to 10)
        // along y = 3.
        assert!(f.run.press(&above(-10.0, 3.0)));
        for k in 1..=10 {
            f.run.drag(&above(-10.0 + f64::from(k) * 4.0, 3.0));
        }
        f.run.release(Some(&above(32.0, 3.0)), true);
        land(&f.run);
        assert!(f.window.get_cut_can_cut(), "{}", f.window.get_cut_warning());
        f.run.apply();
        assert_eq!(leaves(&f), 2);
        let mut v = piece_volumes(&f);
        v.sort_by(f64::total_cmp);
        assert!((v[0] - 20.0 * 20.0 * 7.0).abs() < 1e-2, "{v:?}");
        assert!((v[1] - 20.0 * 20.0 * 13.0).abs() < 1e-2, "{v:?}");
        assert!(f.document.undo().unwrap());
        assert_eq!(leaves(&f), 1);

        f.document.set_selection(Selection::One(LeafId(f.leaf)));
        f.run.toggle();
        f.run.on_option(14, 1.0);
        // From above, a loop round the cube's corner at x 20, y 10: the
        // ring lies on the top face, so the loop cuts off nothing and says
        // so.
        let ring = [
            [14.0, 4.0],
            [26.0, 4.0],
            [26.0, 16.0],
            [14.0, 16.0],
            [14.0, 4.5],
        ];
        assert!(f.run.press(&above(ring[0][0], ring[0][1])));
        for p in &ring[1..] {
            f.run.drag(&above(p[0], p[1]));
        }
        f.run.release(Some(&above(14.0, 4.2)), true);
        land(&f.run);
        assert!(!f.window.get_cut_can_cut());
        assert!(!f.window.get_cut_warning().is_empty());
    });
}

/// #658: a stroke once drawn stays as a surface with handles: a drag on
/// it moves it, a cube moves one of its points, and the cut follows; the
/// stroke keeps few points, each a handle.
#[test]
fn a_drawn_stroke_is_adjusted_before_it_cuts() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        opened(&f);
        f.run.on_option(14, 1.0);
        // Across the cube along y = 3, from above, with many little steps.
        assert!(f.run.press(&above(-10.0, 3.0)));
        for k in 1..=40 {
            f.run.drag(&above(-10.0 + f64::from(k), 3.0));
        }
        f.run.release(Some(&above(32.0, 3.0)), true);
        land(&f.run);
        let world = f
            .run
            .stroke_world
            .borrow()
            .clone()
            .expect("the stroke is kept");
        assert_eq!(world.points.len(), 2, "a straight stroke keeps its ends");
        let (_, gizmo, _) = f.run.drawn_gizmo().expect("its gizmo shows");
        assert_eq!(gizmo.corners().len(), 2);

        // A drag on its line, seen edge on, moves it 5 mm across.
        assert!(f.run.press(&above(26.0, 3.0)));
        f.run.drag(&above(26.0, 6.0));
        f.run.drag(&above(26.0, 8.0));
        f.run.release(Some(&above(26.0, 8.0)), true);
        land(&f.run);
        assert!(f.window.get_cut_can_cut(), "{}", f.window.get_cut_warning());
        let moved = f.run.stroke_world.borrow().clone().unwrap();
        let ends: Vec<[f64; 3]> = moved.points.iter().map(|&p| moved.world(p)).collect();
        for e in &ends {
            assert!((e[1] - 8.0).abs() < 1e-6, "{ends:?}");
        }

        // Its last point dragged to y = -2 tilts the cut across the cube.
        let last = ends[1];
        assert!(f.run.press(&above(last[0], last[1])));
        f.run.drag(&above(last[0], -2.0));
        f.run.release(Some(&above(last[0], -2.0)), true);
        land(&f.run);
        let bent = f.run.stroke_world.borrow().clone().unwrap();
        let end = bent.world(bent.points[1]);
        assert!((end[1] + 2.0).abs() < 1e-6, "{end:?}");

        // Undone by a right click mid drag: back where it was.
        assert!(f.run.press(&above(end[0], end[1])));
        f.run.drag(&above(end[0], 5.0));
        f.run.release(None, true);
        land(&f.run);
        assert_eq!(f.run.stroke_world.borrow().clone().unwrap(), bent);

        // The cut follows the surface: y from 8 at x -10 to -2 at x 32.
        f.run.apply();
        assert_eq!(leaves(&f), 2);
        let line = |x: f64| 8.0 + (x + 10.0) * (-10.0 / 42.0);
        // The cube spans x 0..20, y -10..10: the part above the line.
        let above_line = 20.0 * ((10.0 - line(0.0)) + (10.0 - line(20.0))) / 2.0 * 20.0;
        let mut v = piece_volumes(&f);
        v.sort_by(f64::total_cmp);
        let want = [
            above_line.min(8000.0 - above_line),
            above_line.max(8000.0 - above_line),
        ];
        assert!(
            (v[0] - want[0]).abs() < 1e-2 && (v[1] - want[1]).abs() < 1e-2,
            "{v:?} {want:?}"
        );
    });
}

/// #443: Suggest puts the plane, level, at the model's narrowest place; it
/// cuts nothing.
#[test]
fn suggest_puts_the_plane_at_the_neck() {
    on_fresh_slint_thread(|| {
        // Two 20 mm cubes stacked on a thin 4 mm neck, z 0 to 20, 26 to 46.
        let mut mesh = cube();
        for v in &mut mesh.vertices {
            let p = v.as_array();
            *v = Vertex::new([p[0], p[1], p[2] + 10.0]);
        }
        let mut neck = sc_mesh_fixtures::unit_cube_mesh();
        for v in &mut neck.vertices {
            let p = v.as_array();
            *v = Vertex::new([p[0] * 4.0, p[1] * 4.0, p[2] * 8.0 + 23.0]);
        }
        let mut top = cube();
        for v in &mut top.vertices {
            let p = v.as_array();
            *v = Vertex::new([p[0], p[1], p[2] + 36.0]);
        }
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        for part in [mesh, neck, top] {
            let n = vertices.len() as u32;
            vertices.extend(part.vertices.iter().copied());
            faces.extend(part.faces.iter().map(|f| {
                sc_mesh_core::IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + n))
            }));
        }
        let f = fixture(IndexedMesh::new(None, vertices, faces));
        opened(&f);
        let undo = f.document.undo_len();
        f.run.on_option(15, 0.0);
        let h = f.window.get_cut_height();
        assert!(h > 20.0 && h < 26.0, "at the neck: {h}");
        assert!(
            f.window.get_cut_status().contains("Suggested"),
            "{}",
            f.window.get_cut_status()
        );
        assert_eq!(f.document.undo_len(), undo, "nothing cut");
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

/// A pointer at world `(x, y)` seen from straight above: 10 px a mm, the
/// world origin at (400, 300).
fn above(x: f64, y: f64) -> ScenePointer {
    ScenePointer {
        origin: [x, y, 100.0],
        direction: [0.0, 0.0, -1.0],
        cursor: [400.0 + x * 10.0, 300.0 - y * 10.0],
        project: Box::new(|p| Some([400.0 + p[0] * 10.0, 300.0 - p[1] * 10.0])),
        // The shaft 12 mm, 120 px, as at 100 %.
        scale_at: Box::new(|_| Some(12.0)),
        ring_radius: 0.85,
        arrow_tip: 1.18,
    }
}

/// A pointer at world `(x, z)` seen from the front, looking along +y.
fn front(x: f64, z: f64) -> ScenePointer {
    ScenePointer {
        origin: [x, -100.0, z],
        direction: [0.0, 1.0, 0.0],
        cursor: [400.0 + x * 10.0, 300.0 - z * 10.0],
        project: Box::new(|p| Some([400.0 + p[0] * 10.0, 300.0 - p[2] * 10.0])),
        // The shaft 12 mm, 120 px, as at 100 %.
        scale_at: Box::new(|_| Some(12.0)),
        ring_radius: 0.85,
        arrow_tip: 1.18,
    }
}

/// Presses at `from`, drags to `to` and lets go there.
fn drag(run: &CutRun, from: ScenePointer, to: ScenePointer) {
    assert!(run.press(&from), "the press is on the gizmo");
    run.drag(&to);
    run.release(Some(&to), true);
}

#[test]
fn a_corner_moves_on_the_plane_alone_and_bounds_it() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        opened(&f);
        // The cube spans x 0 to 20 round x 10; not bounded, the plane on
        // show reaches 17 mm round it.
        assert!(!f.run.settings.get().bounded);
        drag(&f.run, above(27.0, 17.0), above(22.0, 12.0));
        let s = f.run.settings.get();
        assert!(s.bounded, "a corner moved bounds the plane");
        assert!(f.window.get_cut_bounded());
        assert_eq!(s.height, 0.0, "the plane stays where it was");
        assert_eq!(s.tilt, [0.0; 2]);
        assert_eq!(s.corners[2], [12.0, 12.0]);
        assert_eq!(s.corners[0], [-17.0, -17.0], "the others stay");
        assert_eq!(s.corners[1], [17.0, -17.0]);
        // A corner may not cross the shape's edges: across the diagonal,
        // it stays where it last was.
        drag(&f.run, above(22.0, 12.0), above(10.0, -25.0));
        assert_eq!(f.run.settings.get().corners[2], [12.0, 12.0]);
        // The shape still lies round the cube: it cuts.
        f.run.on_option(-1, 0.0);
        land(&f.run);
        assert!(f.window.get_cut_can_cut(), "{}", f.window.get_cut_warning());
    });
}

#[test]
fn the_arrow_moves_the_plane_and_the_rings_tilt_it() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        opened(&f);
        // Seen from the front the arrow stands up from the middle.
        drag(&f.run, front(10.0, 4.0), front(10.0, 9.0));
        let s = f.run.settings.get();
        assert!((s.height - 5.0).abs() < 1e-9, "{}", s.height);
        assert!((f64::from(f.window.get_cut_height()) - 5.0).abs() < 1e-5);
        // The ring about y faces the eye from the front: its point on the
        // right dragged 30 degrees up round the middle lifts the plane's
        // right side.
        let r = 12.0 * 0.85;
        let (sin, cos) = 30f64.to_radians().sin_cos();
        drag(
            &f.run,
            front(10.0 + r, 5.0),
            front(10.0 + r * cos, 5.0 + r * sin),
        );
        let s = f.run.settings.get();
        assert!((s.tilt[1] + 30.0).abs() < 1e-6, "{:?}", s.tilt);
        assert!(s.tilt[0].abs() < 1e-9);
        assert!((s.height - 5.0).abs() < 1e-9, "it turns about the middle");
        assert!((f64::from(f.window.get_cut_tilt_y()) + 30.0).abs() < 1e-4);
    });
}

#[test]
fn a_drag_on_the_plane_moves_the_shape_and_a_press_without_one_clicks() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        opened(&f);
        f.run.on_option(3, 1.0);
        // Wide enough that moved 3 mm it still lies round the cube.
        f.run.on_option(4, 30.0);
        f.run.on_option(5, 30.0);
        let before = f.run.settings.get().offset();
        drag(&f.run, above(14.0, 4.0), above(17.0, 6.0));
        let after = f.run.settings.get().offset();
        assert_eq!([after[0] - before[0], after[1] - before[1]], [3.0, 2.0]);
        assert_eq!(f64::from(f.window.get_cut_offset_u()), after[0]);
        // Given up, a drag puts the plane back.
        assert!(f.run.press(&above(14.0, 4.0)));
        f.run.drag(&above(18.0, 0.0));
        f.run.release(None, true);
        assert_eq!(f.run.settings.get().offset(), after);
        // A press on the plane that does not move is a click: it leaves the
        // cube's part of the cut, and nothing is left to cut.
        f.run.on_option(-1, 0.0);
        land(&f.run);
        assert!(f.window.get_cut_can_cut());
        assert!(f.run.press(&above(14.0, 4.0)));
        f.run.release(Some(&above(14.0, 4.0)), false);
        assert!(!f.window.get_cut_can_cut());
        // Off the gizmo and the plane, a press is not the tool's.
        assert!(!f.run.press(&above(80.0, 80.0)));
    });
}

/// A box from `lo` to `hi`, world mm.
fn block(lo: [f32; 3], hi: [f32; 3]) -> IndexedMesh {
    let mut b = sc_mesh_fixtures::unit_cube_mesh();
    for v in &mut b.vertices {
        let u = v.as_array();
        *v = Vertex::new([0, 1, 2].map(|i| lo[i] + (u[i] + 0.5) * (hi[i] - lo[i])));
    }
    b
}

/// The boxes `parts` as one mesh named as imported supports.
fn imported(parts: &[IndexedMesh]) -> IndexedMesh {
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    for part in parts {
        let n = vertices.len() as u32;
        vertices.extend(part.vertices.iter().copied());
        faces.extend(part.faces.iter().map(|f| {
            sc_mesh_core::IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + n))
        }));
    }
    let meta = sc_mesh_core::MeshMetadata {
        name: Some(sc_mesh_core::presupported::IMPORTED_SUPPORTS_NAME.to_owned()),
        header_comment: None,
        declared_facet_normals: None,
        role: sc_mesh_core::MeshRole::from_legacy_name(
            sc_mesh_core::presupported::IMPORTED_SUPPORTS_NAME,
        ),
    };
    IndexedMesh::new(Some(meta), vertices, faces)
}

#[test]
fn imported_supports_of_a_piece_cut_off_go_with_the_cut() {
    on_fresh_slint_thread(|| {
        let f = fixture(cube());
        // The cube spans x 0 to 20, y and z -10 to 10. Two supports stand
        // under it; a column beside it holds its top half alone (#651).
        let supports = imported(&[
            block([4.0, -1.0, -15.0], [6.0, 1.0, -10.0]),
            block([14.0, -1.0, -15.0], [16.0, 1.0, -10.0]),
            block([20.5, -1.0, -15.0], [21.5, 1.0, 9.0]),
        ]);
        let mut graph = f.document.with_scene(|s| s.graph().clone()).unwrap();
        let set = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(supports)),
            )
            .unwrap();
        graph.add_root(set).unwrap();
        f.document.install_graph(graph, PathBuf::from("cube.stl"));
        f.document.set_selection(Selection::One(LeafId(f.leaf)));
        let faces = || {
            f.document
                .with_scene(|s| s.graph().leaf_mesh_arc(set).map(|m| m.faces().len()))
                .flatten()
                .unwrap_or(0)
        };
        assert_eq!(faces(), 36);
        opened(&f);
        assert!(f.window.get_cut_can_cut(), "{}", f.window.get_cut_warning());
        f.run.apply();
        // The two under the bottom half stay; the column goes.
        assert_eq!(faces(), 24, "the column holding the top half went");
        assert!(f.document.undo().unwrap());
        assert_eq!(faces(), 36, "undo brings it back");
    });
}

/// A real pre-supported file (`BREADSLICER_CUT_STL`) split as Separate does
/// and cut through the middle: what the cut takes off with the pieces.
#[test]
#[ignore = "needs a real pre-supported STL"]
fn a_real_presupported_model_cut() {
    let Ok(path) = std::env::var("BREADSLICER_CUT_STL") else {
        return;
    };
    on_fresh_slint_thread(move || {
        let loaded =
            sc_mesh_formats::load_mesh_from_path(std::path::Path::new(&path), None, None).unwrap();
        let leaf = loaded.graph.roots()[0];
        let mesh = loaded.graph.leaf_mesh_arc(leaf).unwrap().clone();
        let split = sc_mesh_core::presupported::find_supports(&mesh).expect("pre-supported");
        let f = fixture(split.model);
        let mut graph = f.document.with_scene(|s| s.graph().clone()).unwrap();
        // The fixture moved the model 10 mm right: the supports too.
        let set = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
                LeafPayload::Mesh(Arc::new(split.supports)),
            )
            .unwrap();
        graph.add_root(set).unwrap();
        f.document.install_graph(graph, PathBuf::from("real.stl"));
        f.document.set_selection(Selection::One(LeafId(f.leaf)));
        let bodies = || {
            f.document
                .with_scene(|s| crate::imported_supports::body_tops(s, &[set]).len())
                .unwrap()
        };
        let owned = f
            .document
            .with_scene(|s| crate::imported_supports::imported_of(s, f.leaf, None))
            .unwrap();
        eprintln!("supports owned by the model: {owned:?} (set {set:?})");
        let before = bodies();
        opened(&f);
        eprintln!("card: {}", f.window.get_cut_status());
        f.run.apply();
        eprintln!(
            "imported bodies {before} -> {}, leaves {}",
            bodies(),
            leaves(&f)
        );
        assert!(bodies() < before, "some supports held the piece cut off");
    });
}

/// #659: the pieces of a model with Breadslicer supports wait for supports
/// of their own in the cut's undo step, as a tilt's model does (#153);
/// Undo brings the model back with its supports and nothing waits.
#[test]
fn cutting_a_supported_model_asks_supports_for_its_pieces() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 18.0])),
                LeafPayload::Mesh(Arc::new(cube())),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        document.install_graph(graph, PathBuf::from("cube.stl"));
        let bus = crate::preferences_bus::PreferencesBus::new(
            crate::preferences_bus::PreferencesSnapshot::for_tests(),
        );
        let supports = crate::support_run::SupportRun::install(
            &window,
            bus,
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        supports.generate_now_for_test();
        assert!(supports.support_leaf().is_some(), "the cube has supports");
        document.set_selection(Selection::One(LeafId(leaf)));
        let run = CutRun::install(&window, document.clone(), ctrl.dialogs(), ctrl);
        let f = Fixture {
            window,
            document,
            run,
            leaf,
        };
        opened(&f);
        f.run.apply();
        let (_, wanted) = f
            .document
            .supports_wanted()
            .expect("the pieces wait for supports");
        assert_eq!(wanted.ids().len(), 2, "both pieces");
        assert!(f.document.undo().unwrap());
        assert!(f.document.supports_wanted().is_none(), "nothing waits");
        assert!(supports.support_leaf().is_some(), "its supports are back");
    });
}

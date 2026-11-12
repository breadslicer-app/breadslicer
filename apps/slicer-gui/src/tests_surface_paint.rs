/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Painting faces in the viewport, through the controller's real callbacks
//! (#183): one stroke is one Undo, nothing made from the geometry goes stale,
//! a stroke in flight never reaches a file half done, and the paint goes with
//! the model through copies, saves, repairs and hollows.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::node::LeafPayload;
use sc_scene::{NodeId, PAINTED, PaintChanges, PaintKind, Scene, SurfaceTags, UNPAINTED};

use crate::camera::{AxisView, OrbitCamera};
use crate::clipboard::{self, Clipboard, clipboard_from_document_selection};
use crate::hollow_run::HollowRun;
use crate::picking::project_world_to_screen;
use crate::print_selection::PrintDefaults;
use crate::scene_document::{SceneDocument, SceneEdit};
use crate::selection::{LeafId, Selection};
use crate::surface_brush::{SurfaceBrush, lost_paint_note};
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::{ScreenPoint, ViewportController};
use crate::{MainWindow, PointerModifiers, WorkspaceSection};

/// A flat sheet, `n` × `n` quads of 1 mm, centred on the origin, looking up.
fn sheet(n: u32) -> IndexedMesh {
    let half = n as f32 / 2.0;
    let mut vertices = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            vertices.push(Vertex::new([x as f32 - half, y as f32 - half, 0.0]));
        }
    }
    let at = |x: u32, y: u32| y * (n + 1) + x;
    let mut faces = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
            faces.push(IndexedTriangle::from_raw_indices([a, b, c]));
            faces.push(IndexedTriangle::from_raw_indices([a, c, d]));
        }
    }
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

struct Painting {
    ctrl: ViewportController,
    document: Rc<SceneDocument>,
    brush: Rc<SurfaceBrush>,
    leaf: NodeId,
    camera: OrbitCamera,
}

/// The sheet in a scene, selected, seen straight from above, the brush on.
fn painting(extra: impl FnOnce(&mut Scene)) -> Painting {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(sheet(40))),
        )
        .expect("a leaf");
    scene.add_root(leaf).expect("a root");
    extra(&mut scene);
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(200.0, 200.0);
    ctrl.set_section(WorkspaceSection::Transform);
    let mut camera = OrbitCamera::new(Vec3::ZERO, 60.0);
    camera.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(camera.clone());
    let document = ctrl.document();
    document.set_selection(Selection::One(LeafId(leaf)));
    let brush = SurfaceBrush::new(document.clone(), PaintKind::Detail);
    brush.set_armed(true, Some(leaf));
    ctrl.set_surface_brush(brush.clone());
    Painting {
        ctrl,
        document,
        brush,
        leaf,
        camera,
    }
}

fn plain() -> PointerModifiers {
    PointerModifiers::default()
}

fn alt() -> PointerModifiers {
    PointerModifiers {
        alt: true,
        ..PointerModifiers::default()
    }
}

fn shift() -> PointerModifiers {
    PointerModifiers {
        shift: true,
        ..PointerModifiers::default()
    }
}

fn at(x: f32, y: f32) -> ScreenPoint {
    ScreenPoint::new(x, y)
}

impl Painting {
    fn painted(&self) -> usize {
        self.document
            .surface_tags(self.leaf, PaintKind::Detail)
            .map_or(0, |t| t.painted_faces())
    }

    /// The pixel over the point of the sheet at `x`, `y`, mm, a little off
    /// the sheet's grid: a ray exactly along a shared edge can miss both faces.
    fn pixel(&self, x: f32, y: f32) -> [f32; 2] {
        let (px, py) = project_world_to_screen(
            &self.camera,
            crate::mesh_render::default_fov_y(),
            200.0,
            200.0,
            Vec3::new(x + 0.13, y + 0.29, 0.0),
        )
        .expect("in front of the camera");
        [px, py]
    }

    /// A stroke between the points of the sheet `from` and `to`, mm, in
    /// `steps` mouse events.
    fn stroke(&self, from: [f32; 2], to: [f32; 2], steps: usize, keys: PointerModifiers) {
        let (from, to) = (self.pixel(from[0], from[1]), self.pixel(to[0], to[1]));
        self.ctrl
            .on_pointer_down(at(from[0], from[1]), keys.clone());
        for i in 1..=steps {
            let s = i as f32 / steps as f32;
            self.ctrl.on_pointer_move(
                at(
                    from[0] + (to[0] - from[0]) * s,
                    from[1] + (to[1] - from[1]) * s,
                ),
                keys.clone(),
            );
        }
        self.ctrl.on_pointer_up(at(to[0], to[1]), keys);
    }
}

#[test]
fn one_stroke_is_one_undo() {
    let p = painting(|_| {});
    p.stroke([0.0, 0.0], [8.0, 0.0], 6, plain());
    let once = p.painted();
    assert!(once > 0);
    assert!(p.document.can_undo());
    assert!(p.document.undo().unwrap());
    assert_eq!(p.painted(), 0);
    assert!(!p.document.can_undo(), "the stroke was one step");
    assert!(p.document.redo().unwrap());
    assert_eq!(p.painted(), once);
}

#[test]
fn painting_leaves_what_was_made_from_the_geometry_alone() {
    let p = painting(|_| {});
    let revision = p.document.scene_revision();
    p.stroke([0.0, 0.0], [6.0, 0.0], 4, plain());
    assert!(p.painted() > 0);
    assert_eq!(p.document.scene_revision(), revision);
    assert!(!p.document.slice_geometry_changed(revision));
    p.document.undo().unwrap();
    assert!(!p.document.slice_geometry_changed(revision));
    p.document.redo().unwrap();
    assert!(!p.document.slice_geometry_changed(revision));
    // A file does change, though.
    assert!(p.document.is_dirty());
}

#[test]
fn a_real_edit_after_a_stroke_still_moves_the_geometry_mark() {
    let p = painting(|_| {});
    p.stroke([0.0, 0.0], [6.0, 0.0], 4, plain());
    let revision = p.document.scene_revision();
    let edit = SceneEdit::SetLocalTransform {
        node_id: p.leaf,
        before: Transform3D::Identity,
        after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
    };
    p.document.apply_edit(edit.clone()).unwrap();
    p.document.push_edit(edit);
    assert!(p.document.slice_geometry_changed(revision));
    p.document.undo().unwrap();
    assert!(!p.document.slice_geometry_changed(revision));
}

#[test]
fn a_long_session_of_strokes_keeps_the_slices_good() {
    let p = painting(|_| {});
    let edit = SceneEdit::SetLocalTransform {
        node_id: p.leaf,
        before: Transform3D::Identity,
        after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
    };
    p.document.apply_edit(edit.clone()).unwrap();
    p.document.push_edit(edit);
    let revision = p.document.scene_revision();
    // More strokes than the history holds: the move falls off its end.
    for i in 0..300u32 {
        let (value, back) = if i % 2 == 0 {
            (PAINTED, UNPAINTED)
        } else {
            (UNPAINTED, PAINTED)
        };
        let edit = SceneEdit::PaintSurface {
            node_id: p.leaf,
            kind: PaintKind::Detail,
            changes: PaintChanges::from_faces(vec![(3, back, value)]),
        };
        p.document.apply_edit(edit.clone()).unwrap();
        p.document.push_edit(edit);
    }
    assert!(!p.document.slice_geometry_changed(revision));
}

#[test]
fn new_supports_join_the_move_they_are_for_not_the_stroke_after_it() {
    let p = painting(|_| {});
    let moved = SceneEdit::SetLocalTransform {
        node_id: p.leaf,
        before: Transform3D::Identity,
        after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
    };
    p.document.apply_edit(moved.clone()).unwrap();
    p.document.push_edit(moved);
    let revision = p.document.scene_revision();
    p.stroke([0.0, 0.0], [6.0, 0.0], 4, plain());
    let painted = p.painted();
    p.document.mark_saved();
    let completion = SceneEdit::SetLocalTransform {
        node_id: p.leaf,
        before: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        after: Transform3D::Affine(AffineTransform3D::from_translation([2.0, 0.0, 0.0])),
    };
    assert!(
        p.document
            .complete_last_edit(revision, completion, &[])
            .unwrap()
    );
    assert!(
        p.document.is_dirty(),
        "the save does not hold the completion"
    );
    assert!(p.document.slice_geometry_changed(revision));
    // The stroke is still its own step, on top.
    p.document.undo().unwrap();
    assert_eq!(p.painted(), 0);
    p.document.redo().unwrap();
    assert_eq!(p.painted(), painted);
}

#[test]
fn a_cancelled_run_cannot_take_back_a_stroke_for_its_edit() {
    let p = painting(|_| {});
    let moved = SceneEdit::SetLocalTransform {
        node_id: p.leaf,
        before: Transform3D::Identity,
        after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
    };
    p.document.apply_edit(moved.clone()).unwrap();
    p.document.push_edit(moved);
    let revision = p.document.scene_revision();
    p.stroke([0.0, 0.0], [6.0, 0.0], 4, plain());
    assert!(!p.document.retract_last_edit(revision).unwrap());
    assert!(p.painted() > 0);
}

#[test]
fn alt_erases_and_shift_fills() {
    let p = painting(|_| {});
    // The sheet is flat: Shift + click fills all of it, in one step.
    p.stroke([0.0, 0.0], [0.0, 0.0], 0, shift());
    let all = p.painted();
    assert_eq!(all, 40 * 40 * 2);
    p.stroke([0.0, 0.0], [6.0, 0.0], 4, alt());
    let erased = p.painted();
    assert!(erased < all);
    // The Erase toggle does what Alt does, and Alt turns it back.
    p.brush.set_erasing(true);
    p.stroke([-10.0, 10.0], [-10.0, 10.0], 0, plain());
    assert!(p.painted() < erased);
    let before = p.painted();
    p.stroke([-10.0, 10.0], [-10.0, 10.0], 0, alt());
    assert!(p.painted() > before);
    p.brush.set_erasing(false);
    // Three strokes and a fill: four steps.
    for _ in 0..4 {
        assert!(p.document.undo().unwrap());
    }
    assert_eq!(p.painted(), 0);
    assert!(!p.document.can_undo());
}

#[test]
fn a_press_with_the_brush_on_selects_nothing_and_moves_nothing() {
    let p = painting(|_| {});
    // Off the model: the brush keeps the press, and the selection stands.
    p.ctrl.on_pointer_down(at(2.0, 2.0), plain());
    p.ctrl.on_pointer_up(at(2.0, 2.0), plain());
    assert_eq!(p.document.selection_primary_node_id(), Some(p.leaf));
    assert_eq!(p.painted(), 0);
    assert!(!p.document.can_undo());
    assert!(!p.document.has_live_preview());
}

#[test]
fn a_drag_that_starts_off_the_model_paints_from_where_it_reaches_it() {
    let p = painting(|_| {});
    // Off the sheet, then over it.
    p.ctrl.on_pointer_down(at(2.0, 2.0), plain());
    assert_eq!(p.painted(), 0);
    let [x, y] = p.pixel(0.0, 0.0);
    p.ctrl.on_pointer_move(at(x, y), plain());
    assert!(p.painted() > 0);
    p.ctrl.on_pointer_up(at(x, y), plain());
    assert!(p.document.can_undo());
    assert!(!p.ctrl.is_mid_gesture());
}

#[test]
fn esc_and_a_right_press_drop_the_stroke_and_leave_nothing_behind() {
    let p = painting(|_| {});
    let revision = p.document.save_stamp();
    p.ctrl.on_pointer_down(at(100.0, 100.0), plain());
    p.ctrl.on_pointer_move(at(120.0, 100.0), plain());
    assert!(p.painted() > 0);
    assert!(p.ctrl.cancel_brush_stroke());
    assert_eq!(p.painted(), 0);
    assert!(!p.document.can_undo());
    p.ctrl.on_pointer_up(at(120.0, 100.0), plain());
    assert_eq!(p.document.save_stamp(), revision);

    p.ctrl.on_pointer_down(at(100.0, 100.0), plain());
    p.ctrl.on_right_pointer_down(100.0, 100.0);
    assert_eq!(p.painted(), 0);
    assert!(!p.document.can_undo());
    assert!(p.ctrl.on_right_pointer_up(100.0, 100.0).is_none());
}

#[test]
fn undo_during_a_stroke_takes_the_stroke_back_and_nothing_older() {
    let p = painting(|_| {});
    p.stroke([-12.0, 0.0], [-10.0, 0.0], 2, plain());
    let first = p.painted();
    let [x, y] = p.pixel(6.0, 0.0);
    p.ctrl.on_pointer_down(at(x, y), plain());
    let [x, y] = p.pixel(12.0, 0.0);
    p.ctrl.on_pointer_move(at(x, y), plain());
    assert!(p.painted() > first);
    assert!(p.document.undo().unwrap());
    assert_eq!(p.painted(), first);
    assert!(!p.brush.stroking());
    assert!(!p.document.has_live_preview());
    p.ctrl.on_pointer_move(at(x + 5.0, y), plain());
    p.ctrl.on_pointer_up(at(x + 5.0, y), plain());
    assert_eq!(p.painted(), first);
}

#[test]
fn a_save_during_a_stroke_closes_it_first_and_autosave_waits() {
    let p = painting(|_| {});
    p.ctrl.on_pointer_down(at(100.0, 100.0), plain());
    p.ctrl.on_pointer_move(at(120.0, 100.0), plain());
    assert!(p.ctrl.is_mid_gesture(), "autosave waits for the stroke");
    let painted = p.painted();
    // What a save does first.
    p.document.end_live_preview();
    assert!(!p.brush.stroking());
    assert_eq!(p.painted(), painted);
    assert!(p.document.can_undo(), "what is on file is in the history");
    p.ctrl.on_pointer_up(at(120.0, 100.0), plain());
    assert!(!p.ctrl.is_mid_gesture());
}

#[test]
fn a_fast_stroke_stays_continuous() {
    let p = painting(|_| {});
    p.brush.set_radius_mm(1.0);
    // One mouse event from one end to the other.
    p.stroke([-18.0, 0.0], [18.0, 0.0], 1, plain());
    let tags = p
        .document
        .surface_tags(p.leaf, PaintKind::Detail)
        .expect("paint");
    let mesh = tags.mesh();
    // Every 2 mm along the line has paint under it.
    for x in -8..=8 {
        let x = x as f32 * 2.0;
        let painted = mesh.faces().iter().zip(tags.values()).any(|(f, v)| {
            let c = f.vertices.map(|i| mesh.vertices()[usize::from(i)].coords());
            let (cx, cy) = (
                (c[0][0] + c[1][0] + c[2][0]) / 3.0,
                (c[0][1] + c[1][1] + c[2][1]) / 3.0,
            );
            *v != UNPAINTED && (cx - x).abs() < 1.0 && cy.abs() < 1.0
        });
        assert!(painted, "a gap at x {x}");
    }
}

#[test]
fn another_model_in_the_way_takes_the_brush_and_supports_do_not() {
    let p = painting(|scene| {
        let lid = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 5.0])),
                LeafPayload::Mesh(Arc::new(sheet(10))),
            )
            .expect("a leaf");
        scene.add_root(lid).expect("a root");
    });
    p.stroke([0.0, 0.0], [0.0, 0.0], 0, plain());
    assert_eq!(p.painted(), 0, "the sheet above covers it");
    p.stroke([-15.0, 15.0], [-15.0, 15.0], 0, plain());
    assert!(p.painted() > 0, "clear of the sheet above");
}

#[test]
fn the_edge_steps_and_clear_are_one_step_each() {
    let p = painting(|_| {});
    p.stroke([0.0, 0.0], [0.0, 0.0], 0, plain());
    let start = p.painted();
    assert!(p.brush.step_edge(p.leaf, true));
    let grown = p.painted();
    assert!(grown > start);
    assert!(p.brush.step_edge(p.leaf, false));
    assert!(p.painted() < grown);
    p.document.undo().unwrap();
    assert_eq!(p.painted(), grown);
    assert!(p.brush.invert(p.leaf));
    assert_eq!(p.painted(), 40 * 40 * 2 - grown);
    assert!(p.brush.clear(p.leaf));
    assert_eq!(p.painted(), 0);
    assert!(!p.brush.clear(p.leaf), "nothing left to clear");
    p.document.undo().unwrap();
    p.document.undo().unwrap();
    assert_eq!(p.painted(), grown);
}

#[test]
fn a_big_stroke_is_stored_as_runs_not_a_snapshot() {
    let p = painting(|_| {});
    p.stroke([0.0, 0.0], [0.0, 0.0], 0, shift());
    let tags = p.document.surface_tags(p.leaf, PaintKind::Detail).unwrap();
    let changes = PaintChanges::between(&vec![UNPAINTED; tags.values().len()], tags.values());
    assert_eq!(changes.runs().len(), 1);
    assert_eq!(changes.face_count(), tags.values().len());
}

/// A 20 mm cube, split fine enough to paint.
fn cube_mesh() -> IndexedMesh {
    let mut cube = sc_mesh_fixtures::unit_cube_mesh();
    for v in &mut cube.vertices {
        *v = Vertex::new(v.as_array().map(|c| c * 20.0));
    }
    cube
}

/// Paints the faces of `leaf` whose every corner is at `z` or above.
fn paint_top(document: &SceneDocument, leaf: NodeId, z: f32) -> usize {
    let (faces, changes) = document
        .with_scene(|spatial| {
            let mesh = spatial.leaf(leaf).unwrap().mesh();
            let changes: Vec<(u32, u8, u8)> = mesh
                .faces()
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.vertices
                        .iter()
                        .all(|&v| mesh.vertices()[usize::from(v)].coords()[2] >= z)
                })
                .map(|(i, _)| (i as u32, UNPAINTED, PAINTED))
                .collect();
            (changes.len(), changes)
        })
        .unwrap();
    let edit = SceneEdit::PaintSurface {
        node_id: leaf,
        kind: PaintKind::Detail,
        changes: PaintChanges::from_faces(changes),
    };
    document.apply_edit(edit.clone()).unwrap();
    document.push_edit(edit);
    faces
}

/// The window's document with the 20 mm cube at the origin.
fn cube_document() -> (Rc<ViewportController>, Rc<SceneDocument>, NodeId) {
    let ctrl = Rc::new(ViewportController::new_for_test());
    let document = ctrl.document();
    let mut graph = Scene::new();
    let leaf = graph
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([40.0, 40.0, 15.0])),
            LeafPayload::Mesh(Arc::new(cube_mesh())),
        )
        .unwrap();
    graph.add_root(leaf).unwrap();
    document.install_graph(graph, PathBuf::from("cube.stl"));
    document.set_selection(Selection::One(LeafId(leaf)));
    (ctrl, document, leaf)
}

#[test]
fn a_pasted_copy_keeps_the_paint_and_then_goes_its_own_way() {
    on_fresh_slint_thread(|| {
        let (_ctrl, document, leaf) = cube_document();
        let faces = paint_top(&document, leaf, 9.0);
        assert!(faces > 0);
        let clip: Clipboard = clipboard_from_document_selection(&document).expect("the model");
        let copy = clipboard::paste(
            &document,
            &clip,
            [120.0, 40.0],
            &crate::build_volume::NO_PRINTER_BUILD_VOLUME,
        )
        .unwrap()
        .expect("pasted");
        let of = |id| {
            document
                .surface_tags(id, PaintKind::Detail)
                .map_or(0, |t| t.painted_faces())
        };
        assert_eq!(of(copy), faces);
        // Painting the copy leaves the model as it was.
        let edit = SceneEdit::PaintSurface {
            node_id: copy,
            kind: PaintKind::Detail,
            changes: PaintChanges::from_faces(
                (0..12u32)
                    .filter(|f| {
                        document
                            .surface_tags(copy, PaintKind::Detail)
                            .is_some_and(|t| t.values()[*f as usize] == UNPAINTED)
                    })
                    .map(|f| (f, UNPAINTED, PAINTED))
                    .collect(),
            ),
        };
        document.apply_edit(edit.clone()).unwrap();
        document.push_edit(edit);
        assert!(of(copy) > faces);
        assert_eq!(of(leaf), faces);
        // One Undo takes the stroke, one the paste.
        document.undo().unwrap();
        document.undo().unwrap();
        assert_eq!(of(leaf), faces);
        assert!(
            document
                .with_scene(|s| s.leaf_ids().contains(&copy))
                .is_some_and(|there| !there)
        );
    });
}

#[test]
fn deleting_a_painted_model_and_undoing_brings_the_paint_back() {
    on_fresh_slint_thread(|| {
        let (_ctrl, document, leaf) = cube_document();
        let faces = paint_top(&document, leaf, 9.0);
        document.delete_selected_leaf().unwrap();
        assert!(document.surface_tags(leaf, PaintKind::Detail).is_none());
        document.undo().unwrap();
        let tags = document
            .surface_tags(leaf, PaintKind::Detail)
            .expect("back");
        assert_eq!(tags.painted_faces(), faces);
    });
}

/// `document`'s scene through a `.bread` file and back.
fn reopened(document: &SceneDocument) -> Scene {
    let payload = document.compose_project_payload(&PrintDefaults::default());
    let bundle =
        crate::scene_save::prepare_save(document, payload, PathBuf::from("painted.bread")).unwrap();
    let bytes = crate::scene_save::encode_bundle(&bundle, |_, _, _| {}).unwrap();
    sc_project_formats::read_bread_project(bytes.as_slice())
        .unwrap()
        .scene
}

#[test]
fn a_3mf_says_it_leaves_the_paint_out_and_a_bread_does_not() {
    on_fresh_slint_thread(|| {
        let (_ctrl, document, leaf) = cube_document();
        let bundle_for = |name: &str| {
            let payload = document.compose_project_payload(&PrintDefaults::default());
            crate::scene_save::prepare_save(&document, payload, PathBuf::from(name)).unwrap()
        };
        assert!(!bundle_for("bare.3mf").leaves_paint_out());
        paint_top(&document, leaf, 9.0);
        assert!(bundle_for("painted.3mf").leaves_paint_out());
        assert!(!bundle_for("painted.bread").leaves_paint_out());
    });
}

#[test]
fn the_paint_survives_a_save_and_an_open() {
    on_fresh_slint_thread(|| {
        let (_ctrl, document, leaf) = cube_document();
        let faces = paint_top(&document, leaf, 9.0);
        let values = document
            .surface_tags(leaf, PaintKind::Detail)
            .unwrap()
            .values()
            .to_vec();
        let scene = reopened(&document);
        let tags: Vec<&SurfaceTags> = scene.surface_tags_iter().map(|(_, _, t)| t).collect();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].painted_faces(), faces);
        assert_eq!(tags[0].values(), values);
    });
}

/// A small painted project at `PAINT_SAMPLE`, for trying it on a build that
/// does not know the paint.
#[test]
fn a_painted_sample_file_is_written_when_asked() {
    let Some(path) = std::env::var_os("PAINT_SAMPLE").map(PathBuf::from) else {
        return;
    };
    on_fresh_slint_thread(move || {
        let (_ctrl, document, leaf) = cube_document();
        paint_top(&document, leaf, 9.0);
        let payload = document.compose_project_payload(&PrintDefaults::default());
        let bundle = crate::scene_save::prepare_save(&document, payload, path.clone()).unwrap();
        let bytes = crate::scene_save::encode_bundle(&bundle, |_, _, _| {}).unwrap();
        std::fs::write(&path, bytes).unwrap();
    });
}

#[test]
fn hollowing_keeps_the_paint_on_the_outside_and_undo_brings_it_back() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (ctrl, document, leaf) = cube_document();
        let hollow = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
        window.set_workspace_section(WorkspaceSection::Hollow);
        let painted = paint_top(&document, leaf, 9.0);
        let before = document.surface_tags(leaf, PaintKind::Detail).unwrap();
        let area = before.painted_area();
        hollow.hollow_now(2.0, &[]);
        let after = document
            .surface_tags(leaf, PaintKind::Detail)
            .expect("the paint came along");
        assert!(!Arc::ptr_eq(after.mesh(), before.mesh()), "hollowed");
        assert!(
            (after.painted_area() - area).abs() < area * 0.02,
            "{} {area}",
            after.painted_area()
        );
        // Only the outside of the top: the cavity under it takes none.
        let mesh = after.mesh();
        for (f, v) in mesh.faces().iter().zip(after.values()) {
            if *v != UNPAINTED {
                let z = mesh.vertices()[usize::from(f.vertices[0])].coords()[2];
                assert!(z > 9.0, "paint on the cavity at z {z}");
            }
        }
        document.undo().unwrap();
        let back = document.surface_tags(leaf, PaintKind::Detail).unwrap();
        assert_eq!(back.painted_faces(), painted);
        assert!(Arc::ptr_eq(back.mesh(), before.mesh()));
    });
}

#[test]
fn a_mesh_swap_that_loses_paint_says_how_much_and_undo_restores_it() {
    on_fresh_slint_thread(|| {
        let (_ctrl, document, leaf) = cube_document();
        paint_top(&document, leaf, 9.0);
        // The model's top goes: a mesh with none of the painted surface.
        let before = document
            .with_scene(|s| s.graph().leaf_mesh_arc(leaf).cloned())
            .flatten()
            .unwrap();
        let mut shorter = (*before).clone();
        for v in &mut shorter.vertices {
            let p = v.coords();
            *v = Vertex::new([p[0], p[1], p[2].min(0.0) - 5.0]);
        }
        let swap = SceneEdit::ReplaceLeafMesh {
            node_id: leaf,
            before: before.clone(),
            after: Arc::new(shorter),
        };
        document.apply_edit(swap.clone()).unwrap();
        document.push_edit(swap);
        let report = document.take_paint_report();
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].area_after, 0.0);
        assert!((report[0].area_before - 400.0).abs() < 1.0, "{report:?}");
        assert!(document.take_paint_report().is_empty(), "said once");
        assert!(document.surface_tags(leaf, PaintKind::Detail).is_none());
        assert!(lost_paint_note(&report).is_some());
        document.undo().unwrap();
        let back = document
            .surface_tags(leaf, PaintKind::Detail)
            .expect("back");
        assert!(Arc::ptr_eq(back.mesh(), &before));
    });
}

#[test]
fn a_mesh_swap_that_keeps_the_paint_says_nothing() {
    let report = [crate::scene_document::PaintCarried {
        leaf: Scene::new()
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(sheet(1))),
            )
            .unwrap(),
        kind: PaintKind::Detail,
        area_before: 100.0,
        area_after: 99.9,
    }];
    assert!(lost_paint_note(&report).is_none());
}

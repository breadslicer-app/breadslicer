/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Models imported into the open scene (#384), through the controller as
//! the menu, the start dialog and a drop reach it: files read on the worker,
//! placed clear of what stands on the plate, added as one undo step.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{LeafPayload, NodeId, Scene};

use crate::build_volume::NO_PRINTER_BUILD_VOLUME;
use crate::mesh_render::DeviceToken;
use crate::mesh_render::mock::{MockRenderer, MockRendererState};
use crate::scene_document::SceneDocument;
use crate::selection::{LeafId, Selection};
use crate::test_support::gpu_devices::shared_test_device;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::ViewportController;

/// The 12 facets of a box `size` mm, its low corner at the origin, as an
/// ASCII STL whose solid names nothing.
fn box_stl(size: [f32; 3]) -> String {
    let [x, y, z] = size;
    let p = |i: usize| -> [f32; 3] {
        [
            if i & 1 == 0 { 0.0 } else { x },
            if i & 2 == 0 { 0.0 } else { y },
            if i & 4 == 0 { 0.0 } else { z },
        ]
    };
    // Outward-wound quads of the cube's corner indices.
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let mut stl = String::from("solid\n");
    for q in quads {
        for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            stl.push_str(" facet normal 0 0 0\n  outer loop\n");
            for i in tri {
                let [a, b, c] = p(i);
                stl.push_str(&format!("   vertex {a} {b} {c}\n"));
            }
            stl.push_str("  endloop\n endfacet\n");
        }
    }
    stl.push_str("endsolid\n");
    stl
}

/// Writes a box `size` mm as `name` in `dir`.
pub(crate) fn write_box(dir: &Path, name: &str, size: [f32; 3]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, box_stl(size)).expect("the STL writes");
    path
}

/// The 8 vertices and 12 triangles of a box `size` mm, as a 3MF mesh.
fn threemf_box(size: [f32; 3]) -> String {
    let [x, y, z] = size;
    let mut vertices = String::new();
    for i in 0..8 {
        let v = [
            if i & 1 == 0 { 0.0 } else { x },
            if i & 2 == 0 { 0.0 } else { y },
            if i & 4 == 0 { 0.0 } else { z },
        ];
        vertices.push_str(&format!(
            "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
            v[0], v[1], v[2]
        ));
    }
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let mut triangles = String::new();
    for q in quads {
        for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            triangles.push_str(&format!(
                "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
                t[0], t[1], t[2]
            ));
        }
    }
    format!("<mesh><vertices>{vertices}</vertices><triangles>{triangles}</triangles></mesh>")
}

/// Writes a 3MF of 2 boxes, named, side by side, as `name` in `dir`.
pub(crate) fn write_two_object_threemf(dir: &Path, name: &str) -> PathBuf {
    let model = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources>
  <object id="1" type="model" name="Knight">{}</object>
  <object id="2" type="model" name="Horse">{}</object>
 </resources>
 <build>
  <item objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 0"/>
  <item objectid="2" transform="1 0 0 0 1 0 0 0 1 12 0 0"/>
 </build>
</model>"#,
        threemf_box([8.0, 8.0, 12.0]),
        threemf_box([6.0, 10.0, 9.0])
    );
    let content_types = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("the file is created");
    let mut archive = zip::write::ZipWriter::new(file);
    for (entry, bytes) in [
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("3D/3dmodel.model", model.as_bytes()),
    ] {
        archive
            .start_file(entry, zip::write::SimpleFileOptions::default())
            .expect("the entry starts");
        archive.write_all(bytes).expect("the entry writes");
    }
    archive.finish().expect("the archive finishes");
    path
}

/// Runs what the window's `scene-load-finished` callback runs until no load
/// or import is in flight.
pub(crate) fn finish_loads(ctrl: &ViewportController) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while ctrl.loader().is_busy() {
        assert!(Instant::now() < deadline, "the load ends within 60 s");
        std::thread::sleep(Duration::from_millis(5));
        ctrl.on_scene_load_finished();
    }
}

/// A box `size` mm standing on the middle of the plate, as an opened file
/// leaves it.
fn scene_with_a_box(document: &SceneDocument, size: [f32; 3]) -> NodeId {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_box(dir.path(), "base.stl", size);
    let loaded = sc_project_formats::load_scene(&path, None, |_| {}).expect("it loads");
    let sc_project_formats::LoadedContent::Mesh(mut scene) = loaded.content else {
        panic!("an STL loads as a mesh");
    };
    sc_print_glue::place_on_plate(&mut scene, &NO_PRINTER_BUILD_VOLUME.volume()).expect("placed");
    let leaf = scene.roots()[0];
    document.install_graph(scene, path);
    leaf
}

/// Every model leaf in row order: its name, world transform and mesh.
pub(crate) fn rows(document: &SceneDocument) -> Vec<(String, Transform3D, usize)> {
    document
        .with_scene(|spatial| {
            let names = crate::viewport::object_names(spatial.graph());
            (0..)
                .map_while(|i| spatial.leaf_id_at_index(i))
                .enumerate()
                .map(|(row, id)| {
                    let mesh = spatial.graph().leaf_mesh_arc(id).unwrap();
                    (
                        names[row].clone(),
                        spatial.leaf(id).unwrap().world_transform(),
                        Arc::as_ptr(mesh) as usize,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The box from above that `leaf` covers: `[min x, min y, max x, max y]`.
pub(crate) fn footprint(document: &SceneDocument, leaf: NodeId) -> [f32; 4] {
    document
        .with_scene(|spatial| {
            let world = spatial.leaf(leaf)?.world_transform();
            let mesh = spatial.graph().leaf_mesh_arc(leaf)?.clone();
            Some(mesh.vertices().iter().fold(
                [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
                |[x0, y0, x1, y1], v| {
                    let p = world.apply_to_vertex(*v);
                    let (x, y) = (p.as_array()[0], p.as_array()[1]);
                    [x0.min(x), y0.min(y), x1.max(x), y1.max(y)]
                },
            ))
        })
        .flatten()
        .expect("a leaf")
}

fn apart(a: [f32; 4], b: [f32; 4]) -> bool {
    a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1]
}

fn on_plate(f: [f32; 4]) -> bool {
    let volume = NO_PRINTER_BUILD_VOLUME;
    let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
    f[0] >= lo.x && f[1] >= lo.y && f[2] <= hi.x && f[3] <= hi.y
}

pub(crate) fn leaves(document: &SceneDocument) -> Vec<NodeId> {
    document
        .with_scene(|spatial| (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect())
        .unwrap_or_default()
}

/// A model imported into an open scene joins it beside what stands there,
/// selected, named after its file; the scene is unsaved; one Undo takes it
/// away and Redo puts it back exactly.
#[test]
fn an_imported_model_joins_the_scene_beside_the_others_as_one_undo_step() {
    on_fresh_slint_thread(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let dragon = write_box(dir.path(), "dragon.stl", [30.0, 20.0, 15.0]);
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let base = scene_with_a_box(&document, [40.0, 30.0, 10.0]);
        let before = rows(&document);
        assert!(!document.is_dirty());

        ctrl.open_models(vec![dragon]);
        finish_loads(&ctrl);

        let now = leaves(&document);
        assert_eq!(now.len(), 2, "the model joined the scene");
        let added = now[1];
        assert_eq!(document.selection(), Selection::One(LeafId(added)));
        assert_eq!(rows(&document)[0], before[0], "the base did not move");
        assert_eq!(rows(&document)[1].0, "dragon");
        assert!(apart(
            footprint(&document, base),
            footprint(&document, added)
        ));
        assert!(on_plate(footprint(&document, added)));
        assert!(document.is_dirty(), "the scene has unsaved changes");
        let after = rows(&document);

        ctrl.on_undo_requested();
        assert_eq!(rows(&document), before, "one Undo takes it away");
        ctrl.on_redo_requested();
        assert_eq!(rows(&document), after, "Redo puts it back as it was");
    });
}

/// Several files picked or dropped together (an STL and a 3MF of 2 objects)
/// join as one undo step: every object of the 3MF comes, and no file
/// overlaps another or what stood there.
#[test]
fn several_files_join_together_as_one_undo_step() {
    on_fresh_slint_thread(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let goblin = write_box(dir.path(), "goblin.stl", [12.0, 12.0, 20.0]);
        let pair = write_two_object_threemf(dir.path(), "pair.3mf");
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let base = scene_with_a_box(&document, [40.0, 30.0, 10.0]);
        let before = rows(&document);

        ctrl.open_models(vec![goblin, pair]);
        finish_loads(&ctrl);

        let now = leaves(&document);
        assert_eq!(now.len(), 4, "the STL and both objects of the 3MF");
        let names: Vec<String> = rows(&document).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["Object 1", "goblin", "Knight", "Horse"]);
        assert_eq!(
            document.selection(),
            Selection::of(now[1..].iter().copied().map(LeafId)),
            "every new model is selected"
        );
        let [base_at, goblin_at, knight_at, horse_at] =
            [base, now[1], now[2], now[3]].map(|leaf| footprint(&document, leaf));
        for (a, b) in [
            (base_at, goblin_at),
            (base_at, knight_at),
            (base_at, horse_at),
            (goblin_at, knight_at),
            (goblin_at, horse_at),
        ] {
            assert!(apart(a, b), "{a:?} overlaps {b:?}");
        }
        // The 3MF's objects keep their places to each other.
        assert!((horse_at[0] - knight_at[0] - 12.0).abs() < 1e-3);

        ctrl.on_undo_requested();
        assert_eq!(rows(&document), before, "one Undo takes every file away");
        assert!(!document.can_undo());
    });
}

/// A file that cannot be read is left out, and the others are added.
#[test]
fn a_file_that_cannot_be_read_leaves_the_others_added() {
    on_fresh_slint_thread(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = write_box(dir.path(), "good.stl", [10.0, 10.0, 10.0]);
        let broken = dir.path().join("broken.obj");
        std::fs::write(&broken, "this is not a model").expect("written");
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        scene_with_a_box(&document, [20.0, 20.0, 10.0]);

        ctrl.open_models(vec![broken, good]);
        finish_loads(&ctrl);

        let names: Vec<String> = rows(&document).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["Object 1", "good"]);
    });
}

/// A model too big for the plate goes beside it; the others stay put.
#[test]
fn a_model_that_does_not_fit_goes_beside_the_plate() {
    on_fresh_slint_thread(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let volume = NO_PRINTER_BUILD_VOLUME;
        let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
        let huge = write_box(dir.path(), "huge.stl", [hi.x - lo.x + 10.0, 10.0, 10.0]);
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        scene_with_a_box(&document, [20.0, 20.0, 10.0]);
        let before = rows(&document);

        ctrl.open_models(vec![huge]);
        finish_loads(&ctrl);

        let now = leaves(&document);
        assert_eq!(now.len(), 2);
        assert_eq!(rows(&document)[0], before[0]);
        assert!(!on_plate(footprint(&document, now[1])));
    });
}

/// A model with no room on the first plate goes on the next (#557), and
/// on a plate added after the last when none has room; nothing that stood
/// there moves, and one Undo takes the model and the plate away.
#[test]
fn a_model_with_no_room_on_the_first_plate_goes_on_a_new_plate() {
    on_fresh_slint_thread(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let volume = NO_PRINTER_BUILD_VOLUME;
        let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
        // A base that leaves no room on the first plate.
        let full = [hi.x - lo.x - 12.0, hi.y - lo.y - 12.0, 10.0];
        let dragon = write_box(dir.path(), "dragon.stl", [20.0, 20.0, 15.0]);
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        scene_with_a_box(&document, full);
        let before = rows(&document);

        ctrl.open_models(vec![dragon]);
        finish_loads(&ctrl);

        assert_eq!(document.plates().count(), 2, "a plate added");
        assert_eq!(rows(&document)[0], before[0], "the base did not move");
        let added = leaves(&document)[1];
        let f = footprint(&document, added);
        let second = crate::plates::offset(&volume, 1);
        let moved = [
            f[0] - second.x,
            f[1] - second.y,
            f[2] - second.x,
            f[3] - second.y,
        ];
        assert!(on_plate(moved), "on the second plate: {f:?}");

        ctrl.on_undo_requested();
        assert_eq!(rows(&document), before, "one Undo takes it away");
        assert_eq!(document.plates().count(), 1, "and the plate");
    });
}

/// Models opened together in a window without a scene open one: the first
/// opens it as any file does, the others join it, all part of the opening
/// (nothing to undo, nothing unsaved).
#[test]
fn models_opened_without_a_scene_open_one_together() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    on_fresh_slint_thread(move || {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = write_box(dir.path(), "first.stl", [10.0, 10.0, 10.0]);
        let second = write_box(dir.path(), "second.stl", [14.0, 8.0, 6.0]);
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.loader().set_gpu(device, DeviceToken::next(), queue);
        let document = ctrl.document();

        ctrl.open_models(vec![first.clone(), second]);
        finish_loads(&ctrl);

        let now = leaves(&document);
        assert_eq!(now.len(), 2, "both models are in the scene");
        assert!(apart(
            footprint(&document, now[0]),
            footprint(&document, now[1])
        ));
        assert_eq!(document.with_source_path(|p| p.to_path_buf()), Some(first));
        assert!(!document.can_undo(), "the opening is not an undo step");
        assert!(!document.is_dirty());
    });
}

/// Models opened together without a scene show together (#391): the window
/// installs one scene holding all of them, built on the worker, and no
/// model shows before the others; nothing is left to upload on the window's
/// thread, and they are all selected.
#[test]
fn models_opened_without_a_scene_arrive_in_one_install() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    on_fresh_slint_thread(move || {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = vec![
            write_box(dir.path(), "a.stl", [10.0, 10.0, 10.0]),
            write_box(dir.path(), "b.stl", [14.0, 8.0, 6.0]),
            write_box(dir.path(), "c.stl", [6.0, 6.0, 20.0]),
        ];
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
        let state = MockRendererState::new_shared();
        ctrl.set_renderer_for_test(Box::new(MockRenderer::new(state.clone())));
        ctrl.loader().set_gpu(device, DeviceToken::next(), queue);
        let document = ctrl.document();

        ctrl.open_models(paths);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut counts = Vec::new();
        while ctrl.loader().is_busy() {
            assert!(Instant::now() < deadline, "the load ends within 60 s");
            std::thread::sleep(Duration::from_millis(5));
            ctrl.on_scene_load_finished();
            ctrl.handle_before_rendering();
            let count = leaves(&document).len();
            if counts.last() != Some(&count) {
                counts.push(count);
            }
        }

        assert_eq!(counts.last(), Some(&3), "every model is in the scene");
        assert!(
            counts.iter().all(|&n| n == 0 || n == 3),
            "no model shows before the others: {counts:?}"
        );
        assert_eq!(state.borrow().install_scene_calls, 1, "one install");
        assert!(
            state.borrow().upload_calls.is_empty(),
            "nothing is uploaded on the window's thread"
        );
        let now = leaves(&document);
        assert_eq!(
            document.selection(),
            Selection::of(now.iter().copied().map(LeafId)),
            "the models opened are selected"
        );
        assert!(!document.can_undo(), "the opening is not an undo step");
        assert!(!document.is_dirty());
    });
}

/// Models opened together without a scene stand where they stood when the
/// first opened the scene alone and the others were imported into it.
#[test]
fn models_opened_without_a_scene_are_placed_as_an_import_places_them() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    on_fresh_slint_thread(move || {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = write_box(dir.path(), "first.stl", [40.0, 30.0, 10.0]);
        let others = vec![
            write_box(dir.path(), "goblin.stl", [12.0, 12.0, 20.0]),
            write_two_object_threemf(dir.path(), "pair.3mf"),
            write_box(dir.path(), "tower.stl", [8.0, 8.0, 30.0]),
        ];
        let together = Rc::new(ViewportController::new_for_test());
        together
            .loader()
            .set_gpu(device.clone(), DeviceToken::next(), queue.clone());
        let mut paths = vec![first.clone()];
        paths.extend(others.iter().cloned());
        together.open_models(paths);
        finish_loads(&together);

        let apart_ctrl = Rc::new(ViewportController::new_for_test());
        apart_ctrl
            .loader()
            .set_gpu(device, DeviceToken::next(), queue);
        apart_ctrl.open_models(vec![first]);
        finish_loads(&apart_ctrl);
        apart_ctrl.open_models(others);
        finish_loads(&apart_ctrl);

        let placed = |document: &SceneDocument| -> Vec<(String, Transform3D)> {
            rows(document)
                .into_iter()
                .map(|(name, at, _)| (name, at))
                .collect()
        };
        let together = placed(&together.document());
        assert_eq!(together.len(), 5);
        assert_eq!(together, placed(&apart_ctrl.document()));
    });
}

/// A first file that cannot be read leaves the scene to the next that can,
/// with the others joined to it; no file read leaves no scene.
#[test]
fn models_opened_without_a_scene_skip_a_file_that_cannot_be_read() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    on_fresh_slint_thread(move || {
        let dir = tempfile::tempdir().expect("tempdir");
        let broken = dir.path().join("broken.obj");
        std::fs::write(&broken, "this is not a model").expect("written");
        let good = write_box(dir.path(), "good.stl", [10.0, 10.0, 10.0]);
        let fine = write_box(dir.path(), "fine.stl", [12.0, 8.0, 10.0]);
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.loader()
            .set_gpu(device.clone(), DeviceToken::next(), queue.clone());
        let document = ctrl.document();

        ctrl.open_models(vec![broken.clone(), good.clone(), fine]);
        finish_loads(&ctrl);

        let names: Vec<String> = rows(&document).into_iter().map(|r| r.0).collect();
        // The scene opened from the good file, its model named after it.
        assert_eq!(names, ["good", "fine"]);
        assert_eq!(document.with_source_path(|p| p.to_path_buf()), Some(good));
        assert!(!document.is_dirty());

        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.loader().set_gpu(device, DeviceToken::next(), queue);
        let other = dir.path().join("other.stl");
        std::fs::write(&other, "not a model either").expect("written");
        ctrl.open_models(vec![broken, other]);
        finish_loads(&ctrl);
        assert!(!ctrl.document().is_loaded(), "no file read, no scene");
    });
}

/// An import moves nothing that stood on the plate, and its Undo leaves the
/// scene exactly as it was.
#[test]
fn an_import_moves_nothing_that_stood_there() {
    on_fresh_slint_thread(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let pair = write_two_object_threemf(dir.path(), "pair.3mf");
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut scene = Scene::new();
        let cube = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        for x in [10.0, 30.0] {
            let leaf = scene
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 10.0, 0.0])),
                    LeafPayload::Mesh(cube.clone()),
                )
                .unwrap();
            scene.add_root(leaf).unwrap();
        }
        document.install_graph(scene, PathBuf::from("cubes.stl"));
        let before = rows(&document);

        ctrl.open_models(vec![pair]);
        finish_loads(&ctrl);
        assert_eq!(leaves(&document).len(), 4);
        assert_eq!(&rows(&document)[..2], &before[..], "nothing moved");
        ctrl.on_undo_requested();
        assert_eq!(rows(&document), before);
    });
}

/// A model that moves while the files are read: they are placed again
/// against the plate as it stands, clear of where it went; the move and the
/// import stay two undo steps.
#[test]
fn files_read_while_the_plate_changes_are_placed_against_it_as_it_stands() {
    on_fresh_slint_thread(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let wide = write_box(dir.path(), "wide.stl", [40.0, 30.0, 10.0]);
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let base = scene_with_a_box(&document, [40.0, 30.0, 10.0]);

        ctrl.open_models(vec![wide]);
        // The base moves over to where the model would have gone, while the
        // file is read.
        let before = document
            .with_scene(|spatial| spatial.leaf(base).map(|q| q.local_transform()))
            .flatten()
            .expect("the base");
        let Transform3D::Affine(at) = before else {
            panic!("the base was placed");
        };
        let moved = Transform3D::Affine(AffineTransform3D::from_translation([
            at.translation()[0] + 45.0,
            at.translation()[1],
            at.translation()[2],
        ]));
        let edit = sc_scene_edit::SceneEdit::SetLocalTransform {
            node_id: base,
            before,
            after: moved,
        };
        document.apply_edit(edit.clone()).expect("the base moves");
        document.push_edit(edit);
        finish_loads(&ctrl);

        let now = leaves(&document);
        assert_eq!(now.len(), 2);
        assert!(apart(
            footprint(&document, base),
            footprint(&document, now[1])
        ));
        ctrl.on_undo_requested();
        assert_eq!(leaves(&document).len(), 1, "Undo takes the import");
        ctrl.on_undo_requested();
        let back = document
            .with_scene(|spatial| spatial.leaf(base).map(|q| q.local_transform()))
            .flatten();
        assert_eq!(back, Some(before), "and then the move");
    });
}

/// An import builds the pick structure of every mesh it adds on its worker
/// (#395), once per mesh, and the scene takes it as it adds the models:
/// the first click on one builds nothing, and finds what a click would have
/// built.
#[test]
fn an_import_brings_the_pick_structure_of_its_meshes_built() {
    use std::sync::atomic::AtomicU32;

    use sc_mesh_spatial::TriangleClusters;

    use crate::load_progress::LoadProgressSink;
    use crate::model_import::{ImportJob, add_to_scene, prepare_models};

    let dir = tempfile::tempdir().expect("tempdir");
    let goblin = write_box(dir.path(), "goblin.stl", [12.0, 12.0, 20.0]);
    let pair = write_two_object_threemf(dir.path(), "pair.3mf");
    let document = SceneDocument::new();
    let base = scene_with_a_box(&document, [40.0, 30.0, 10.0]);
    let volume = NO_PRINTER_BUILD_VOLUME;
    let (standing, stands_on) = crate::arrange_run::standing_bodies(&document, &volume);
    let job = ImportJob {
        paths: vec![goblin, pair],
        standing,
        stands_on,
        plates: crate::arrange_run::every_plate(&volume),
        build_volume: volume.volume(),
        revision: document.scene_revision(),
        recorded: true,
        device: None,
    };
    let models = prepare_models(job, &LoadProgressSink::new(Arc::new(AtomicU32::new(0))));
    assert!(models.failures.is_empty());
    assert_eq!(models.primed.len(), 3, "one per mesh");

    let added = add_to_scene(
        &document,
        &models.files,
        &models.placements,
        &models.primed,
        models.recorded,
    )
    .expect("the models join");
    assert_eq!(added.len(), 3);
    document
        .with_scene(|spatial| {
            for id in &added {
                let query = spatial.leaf(*id).expect("a live leaf");
                let primed = models
                    .primed
                    .iter()
                    .find(|p| std::ptr::eq(p.mesh().as_ref(), query.mesh()))
                    .expect("its mesh was primed");
                let clusters = query.clusters().expect("faces");
                assert!(std::ptr::eq(clusters, primed.clusters().expect("faces")));
                assert_eq!(
                    Some(clusters),
                    TriangleClusters::for_mesh(query.mesh()).as_ref()
                );
            }
            // What stood there kept its own.
            assert!(spatial.leaf(base).expect("the base").clusters().is_some());
        })
        .expect("a scene");
}

/// The distance from above between the boxes `a` and `b` cover, both
/// `[min x, min y, max x, max y]`: negative when they overlap.
pub(crate) fn gap_between(a: [f32; 4], b: [f32; 4]) -> f32 {
    (b[0] - a[2])
        .max(a[0] - b[2])
        .max(b[1] - a[3])
        .max(a[1] - b[3])
}

/// #420: the model of a file a scene opens from is named after the file,
/// as an import names it: alone, and first of several opened together.
#[test]
fn the_model_a_scene_opens_from_is_named_after_its_file() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    on_fresh_slint_thread(move || {
        let dir = tempfile::tempdir().expect("tempdir");
        let archer = write_box(dir.path(), "Aarakocra Archer.stl", [10.0, 10.0, 10.0]);
        let bard = write_box(dir.path(), "Aarakocra Bard 2.stl", [8.0, 12.0, 6.0]);
        let names_after = |paths: Vec<PathBuf>| {
            let ctrl = Rc::new(ViewportController::new_for_test());
            ctrl.loader()
                .set_gpu(device.clone(), DeviceToken::next(), queue.clone());
            ctrl.open_models(paths);
            finish_loads(&ctrl);
            rows(&ctrl.document())
                .into_iter()
                .map(|r| r.0)
                .collect::<Vec<String>>()
        };
        assert_eq!(names_after(vec![archer.clone()]), ["Aarakocra Archer"]);
        assert_eq!(
            names_after(vec![archer, bard]),
            ["Aarakocra Archer", "Aarakocra Bard 2"]
        );
    });
}

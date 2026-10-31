/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Real `.bread` files through open, save and open again (#220), as the load
//! worker and the controller do it. Run with `SC_BREAD_ROUNDTRIP_DIR` set to
//! a folder of `.bread` files; they are only read, the saves go to a
//! temporary folder.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use sc_mesh_core::IndexedMesh;
use sc_project_formats::{LoadedContent, load_scene};

use crate::hollow_persist::{install_hollows, restore_hollows};
use crate::print_selection::PrintDefaults;
use crate::scene_document::SceneDocument;
use crate::support_persist::{install_sets, restore_supports, sets_in_force};
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::ViewportController;

/// A mesh by its sizes and a hash of its exact content.
fn mesh_print(mesh: &IndexedMesh) -> (usize, usize, u64) {
    let mut hasher = DefaultHasher::new();
    for vertex in mesh.vertices() {
        for value in vertex.as_array() {
            value.to_bits().hash(&mut hasher);
        }
    }
    for face in mesh.faces() {
        for index in face.vertices {
            usize::from(index).hash(&mut hasher);
        }
    }
    (mesh.vertices().len(), mesh.faces().len(), hasher.finish())
}

/// What an opened document shows and can edit: every leaf in row order with
/// its world transform and mesh, the editable support sets, and the hollows.
#[derive(Debug, PartialEq)]
struct Opened {
    leaves: Vec<(String, (usize, usize, u64))>,
    editable_sets: usize,
    hollows: Vec<String>,
}

fn opened(document: &SceneDocument) -> Opened {
    let (leaves, editable_sets) = document
        .with_scene(|spatial| {
            let leaves = (0..)
                .map_while(|i| spatial.leaf_id_at_index(i))
                .map(|id| {
                    let world = format!("{:?}", spatial.leaf(id).unwrap().world_transform());
                    let mesh = spatial.graph().leaf_mesh_arc(id).unwrap();
                    (world, mesh_print(mesh))
                })
                .collect::<Vec<_>>();
            let sets = sets_in_force(spatial, &document.support_sessions()).len();
            (leaves, sets)
        })
        .expect("a scene is open");
    let hollows = document
        .hollows()
        .borrow()
        .iter()
        .map(|h| {
            format!(
                "{:?} {:?} {:?} {:?}",
                h.options,
                h.drains,
                mesh_print(&h.original),
                mesh_print(&h.hollow)
            )
        })
        .collect();
    Opened {
        leaves,
        editable_sets,
        hollows,
    }
}

/// Opens `path` into `document`, rebuilding its supports and hollows.
fn open(document: &SceneDocument, path: &Path) {
    let loaded = load_scene(path, None, |_| {}).expect("it opens");
    let LoadedContent::Project(project) = loaded.content else {
        panic!("a .bread opens as a project");
    };
    let mut scene = project.scene;
    let supports = restore_supports(&mut scene, project.supports);
    assert!(supports.failures.is_empty(), "{:?}", supports.failures);
    let hollows = restore_hollows(&scene, project.hollows);
    assert!(hollows.failures.is_empty(), "{:?}", hollows.failures);
    document.install_project_graph(scene, path.to_path_buf(), project.payload);
    install_sets(document, supports.sets);
    install_hollows(document, hollows.hollows);
}

fn reopened(path: &Path) -> (Rc<ViewportController>, Opened) {
    let ctrl = Rc::new(ViewportController::new_for_test());
    open(&ctrl.document(), path);
    let state = opened(&ctrl.document());
    (ctrl, state)
}

fn save(document: &SceneDocument, path: &Path) -> u64 {
    crate::scene_save::save_project_to(document, path, &PrintDefaults::default())
        .expect("the project saves");
    std::fs::metadata(path).unwrap().len()
}

#[test]
#[ignore = "needs SC_BREAD_ROUNDTRIP_DIR"]
fn real_bread_files_reopen_the_same_after_a_save() {
    let Some(dir) = std::env::var_os("SC_BREAD_ROUNDTRIP_DIR").map(PathBuf::from) else {
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "bread"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .bread files in {}", dir.display());
    for file in files {
        let file_for_thread = file.clone();
        let report = on_fresh_slint_thread(move || {
            let file = file_for_thread;
            let out = tempfile::tempdir().unwrap();
            let original = std::fs::metadata(&file).unwrap().len();
            let (ctrl, before) = reopened(&file);

            // Open, save, open: the same scene, supports and hollows.
            let first = out.path().join("first.bread");
            let first_len = save(&ctrl.document(), &first);
            let (ctrl, after) = reopened(&first);
            assert_eq!(after, before, "{} reopens the same", file.display());

            // Save again: nothing piles up.
            let second = out.path().join("second.bread");
            let second_len = save(&ctrl.document(), &second);
            assert_eq!(
                std::fs::read(&second).unwrap(),
                std::fs::read(&first).unwrap(),
                "{} saves the same file twice",
                file.display()
            );
            let (_, again) = reopened(&second);
            assert_eq!(again, before);
            format!(
                "{}: {original} -> {first_len} -> {second_len} bytes, {} leaves, {} editable \
                 support sets, {} hollows",
                file.file_name().unwrap().to_string_lossy(),
                before.leaves.len(),
                before.editable_sets,
                before.hollows.len()
            )
        });
        println!("{report}");
    }
}

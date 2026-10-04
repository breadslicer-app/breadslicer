/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! "Save" / "Save As" support for project files: functions over
//! `&SceneDocument` plus explicit inputs. The wiring layer drives the file
//! dialog, then calls in here to produce the file.
//!
//! A save always writes a 3MF project: the scene plus the project payload. The
//! payload holds the scene's own printer / resin / first-layer choices, with a
//! missing printer or resin filled from the app-wide default at save time, and
//! the embedded profile documents the document holds. Referenced profiles
//! without a held document get embedded copies resolved from the local
//! libraries.

use std::path::{Path, PathBuf};

use anyhow::Result;
use sc_mesh_formats::scene_writer::write_bytes_atomically;
use sc_print_primitives::{MaterialId, PrinterId};
use sc_print_profiles::ProfileLibraries;
use sc_project_formats::{
    EmbedProfiles, PlateSlices, ProjectFileType, ProjectPayload, WriteProgress,
    write_project_with_progress,
};
use sc_scene::Scene;

use crate::project_profiles::complete_embedded_documents;
use crate::save_progress::SavePhase;
use crate::scene_document::{SaveStamp, SceneDocument};
use crate::slice_projection::resolve_current_schedule;

/// Classification of a "Save" command against the document state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveDispatch {
    /// The document is a project with a known location and unsaved changes:
    /// overwrite it in place, no dialog.
    OverwriteProject(PathBuf),
    /// The source is a plain mesh file (or a 3MF without project data): route
    /// to "Save As" so an STL/OBJ/interop file is never silently replaced by a
    /// project.
    NeedsSaveAs,
    /// The project file already matches the document: nothing to write.
    AlreadySaved,
    /// No scene is loaded: nothing to save. The menu item is disabled in that
    /// case; this catches a stale callback racing the close path.
    NoScene,
}

pub(crate) fn decide_save_dispatch(doc: &SceneDocument) -> SaveDispatch {
    if !doc.is_loaded() {
        return SaveDispatch::NoScene;
    }
    if !doc.is_project() {
        return SaveDispatch::NeedsSaveAs;
    }
    if !doc.is_dirty() {
        return SaveDispatch::AlreadySaved;
    }
    let path = doc
        .with_source_path(Path::to_path_buf)
        .expect("a loaded document always has a source path");
    SaveDispatch::OverwriteProject(path)
}

/// Classification of the file-save dialog's outcome for "Save As".
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveTargetDispatch {
    /// The user dismissed the dialog.
    Cancelled,
    /// The picked path names a different format; a project save would not
    /// produce what the name promises.
    WrongExtension(PathBuf),
    /// The path is ready to write (a missing extension gets `.3mf` appended:
    /// there is exactly one project format, so the intent is unambiguous).
    Ready(PathBuf),
}

pub(crate) fn decide_save_target(target: Option<PathBuf>) -> SaveTargetDispatch {
    let Some(target) = target else {
        return SaveTargetDispatch::Cancelled;
    };
    match target.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("3mf") => SaveTargetDispatch::Ready(target),
        Some(_) => SaveTargetDispatch::WrongExtension(target),
        None => SaveTargetDispatch::Ready(target.with_extension("3mf")),
    }
}

/// Build the payload to write when saving `doc` as a project. A document opened
/// from a project keeps that project's payload; `default_printer` and
/// `default_resin` are used only when the document did not come from one. Every
/// referenced profile without an embedded document then gets one, resolved from
/// `libraries`.
pub(crate) fn payload_for_save(
    doc: &SceneDocument,
    default_printer: Option<PrinterId>,
    default_resin: Option<MaterialId>,
    libraries: &ProfileLibraries,
) -> ProjectPayload {
    let mut payload = doc.compose_project_payload(default_printer, default_resin);
    complete_embedded_documents(&mut payload, libraries);
    payload
}

/// Everything a project write needs, captured as owned, `Send` data so the
/// write can run off the UI thread. The scene is a cheap clone of the
/// document's graph (mesh payloads are shared through `Arc`), and the slices,
/// payload, and target are owned. Built on the UI thread by [`prepare_save`];
/// consumed by [`write_bundle`] on a worker thread.
pub(crate) struct SaveBundle {
    scene: Scene,
    payload: ProjectPayload,
    slices: Option<PlateSlices>,
    slice_fingerprint: Option<String>,
    target: PathBuf,
}

/// Snapshots the document's scene and the slices to persist into a `Send`
/// [`SaveBundle`], on the UI thread. Returns `None` when no scene is loaded.
/// The scene clone and the slice capture are taken together under one scene
/// borrow so the persisted slices match the geometry written.
pub(crate) fn prepare_save(
    doc: &SceneDocument,
    payload: ProjectPayload,
    target: PathBuf,
) -> Option<SaveBundle> {
    doc.with_scene(|spatial| {
        let scene = spatial.graph().clone();
        let (slices, slice_fingerprint) = match persist_slices(doc, &payload, spatial.graph()) {
            Some((plate, token)) => (Some(plate), Some(token)),
            None => (None, None),
        };
        SaveBundle {
            scene,
            payload,
            slices,
            slice_fingerprint,
            target,
        }
    })
}

/// Writes a prepared bundle to its target as a 3MF project, atomically,
/// reporting each phase through `progress`. Pure with respect to the document,
/// so it runs on a worker thread; the caller records success on the UI thread.
pub(crate) fn write_bundle(
    bundle: &SaveBundle,
    mut progress: impl FnMut(SavePhase, u64, u64),
) -> Result<()> {
    let mut bytes = Vec::new();
    write_project_with_progress(
        &bundle.scene,
        &bundle.payload,
        EmbedProfiles::Yes,
        bundle.slices.as_ref(),
        bundle.slice_fingerprint.as_deref(),
        ProjectFileType::ThreeMf,
        &mut bytes,
        |report| match report {
            WriteProgress::Serialize { done, total } => progress(SavePhase::Serialize, done, total),
            WriteProgress::Compress { done, total } => progress(SavePhase::Compress, done, total),
        },
    )?;
    progress(SavePhase::Writing, 0, 1);
    write_bytes_atomically(&bundle.target, &bytes)?;
    progress(SavePhase::Writing, 1, 1);
    Ok(())
}

/// Returns the held slices to persist with the project, or `None` when the
/// scene has no in-sync slice result to write. Resolves the current layer
/// schedule from `payload`'s printer and resin and defers the in-sync decision
/// to the document's write guard; a missing profile or a resolution failure
/// persists nothing.
fn persist_slices(
    doc: &SceneDocument,
    payload: &ProjectPayload,
    scene: &Scene,
) -> Option<(PlateSlices, String)> {
    let printer = payload.printer.clone()?;
    let resin = payload.resin.clone()?;
    let schedule =
        resolve_current_schedule(&printer, &resin, doc.selected_first_layer_height(), scene)
            .ok()?;
    doc.slices_for_persist(&schedule)
}

/// Runs a full project save against `target` synchronously: builds the payload,
/// snapshots and writes the bundle, and on success updates the document so the
/// held payload matches the file, "Save" overwrites `target` from now on, and
/// the dirty marker clears. The GUI drives the write off the UI thread instead;
/// this direct composition backs the save round-trip tests.
#[cfg(test)]
pub(crate) fn save_project_to(
    doc: &SceneDocument,
    target: &Path,
    default_printer: Option<PrinterId>,
    default_resin: Option<MaterialId>,
) -> Result<()> {
    let (bundle, payload, stamp) =
        prepare_save_for_target(doc, target.to_path_buf(), default_printer, default_resin)
            .ok_or_else(|| anyhow::anyhow!("No scene loaded"))?;
    write_bundle(&bundle, |_, _, _| {})?;
    doc.record_project_save_for(payload, target.to_path_buf(), stamp);
    Ok(())
}

/// Builds the save payload and snapshots the scene into a `Send` [`SaveBundle`]
/// for `target`. Returns the bundle, the payload it carries, and the document
/// state both were taken from; `None` when no scene is loaded.
///
/// The stamp is captured here so it cannot drift from the snapshot it
/// describes: the caller hands it back once the write completes.
pub(crate) fn prepare_save_for_target(
    doc: &SceneDocument,
    target: PathBuf,
    default_printer: Option<PrinterId>,
    default_resin: Option<MaterialId>,
) -> Option<(SaveBundle, ProjectPayload, SaveStamp)> {
    let payload = payload_for_save(
        doc,
        default_printer,
        default_resin,
        &ProfileLibraries::platform_default(),
    );
    let bundle = prepare_save(doc, payload.clone(), target)?;
    Some((bundle, payload, doc.save_stamp()))
}

#[cfg(test)]
mod scene_save_tests {
    use super::*;

    use std::fs;
    use std::io::Cursor;
    use std::sync::Arc;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};
    use sc_project_formats::{EmbeddedProfile, read_project};
    use sc_scene::{LeafPayload, Scene};

    fn printer_id(raw: &str) -> PrinterId {
        PrinterId::new(raw).expect("test id is a valid slug")
    }

    fn material_id(raw: &str) -> MaterialId {
        MaterialId::new(raw).expect("test id is a valid slug")
    }

    fn unit_triangle() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        }
    }

    fn one_object_graph() -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        g
    }

    fn held_payload() -> ProjectPayload {
        ProjectPayload {
            printer: Some(printer_id("held-printer")),
            resin: Some(material_id("held-resin")),
            first_layer_height: None,
            output_format: None,
            embedded: vec![EmbeddedProfile::Printer {
                id: printer_id("held-printer"),
                bytes: b"kind = \"printer\"".to_vec(),
            }],
        }
    }

    /// Commits one transform edit so the document reads dirty.
    fn make_dirty(doc: &SceneDocument) {
        use sc_mesh_core::transform::AffineTransform3D;

        let node_id = doc
            .with_scene(|s| s.leaf_id_at_index(0))
            .flatten()
            .expect("fixture has one leaf");
        let edit = crate::scene_document::SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        };
        assert!(doc.apply_edit(edit.clone()).is_ok());
        doc.push_edit(edit);
    }

    mod save_dispatch_tests {
        use super::*;
        use std::path::PathBuf;

        #[test]
        fn no_scene_classifies_as_no_scene() {
            let doc = SceneDocument::new();
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NoScene);
        }

        #[test]
        fn a_plain_mesh_source_routes_to_save_as() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
        }

        #[test]
        fn a_dirty_project_document_overwrites_its_source_path() {
            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/tmp/plate.3mf"),
                ProjectPayload::default(),
            );
            make_dirty(&doc);
            assert_eq!(
                decide_save_dispatch(&doc),
                SaveDispatch::OverwriteProject(PathBuf::from("/tmp/plate.3mf")),
            );
        }

        #[test]
        fn a_clean_project_document_writes_nothing() {
            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/tmp/plate.3mf"),
                ProjectPayload::default(),
            );
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::AlreadySaved);
        }

        #[test]
        fn a_clean_plain_mesh_source_still_routes_to_save_as() {
            // No project file exists yet, so "Save" on a clean mesh-sourced
            // scene is still a real action: it creates the project.
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            assert!(!doc.is_dirty());
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
        }

        // A printer / resin / first-layer pick makes the document dirty but
        // does not turn a plain mesh file into a project: "Save" must still go
        // through the dialog instead of overwriting the mesh file with a 3MF.
        #[test]
        fn a_plain_mesh_source_with_a_picked_printer_still_routes_to_save_as() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            assert!(doc.set_selected_printer(Some(printer_id("picked-printer"))));
            assert!(doc.is_dirty());
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
        }

        #[test]
        fn a_plain_mesh_source_with_a_picked_resin_still_routes_to_save_as() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            assert!(doc.set_selected_resin(Some(material_id("picked-resin"))));
            assert!(doc.is_dirty());
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
        }

        #[test]
        fn a_plain_mesh_source_with_a_first_layer_height_still_routes_to_save_as() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            let height = sc_print_primitives::PositiveFiniteMm::new(0.1).expect("valid length");
            assert!(doc.set_selected_first_layer_height(Some(height)));
            assert!(doc.is_dirty());
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
        }
    }

    mod save_target_tests {
        use super::*;
        use std::path::PathBuf;

        #[test]
        fn none_classifies_as_cancelled() {
            assert_eq!(decide_save_target(None), SaveTargetDispatch::Cancelled);
        }

        #[test]
        fn a_threemf_extension_is_ready_as_is() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.3mf"))),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.3mf")),
            );
        }

        #[test]
        fn the_extension_match_is_case_insensitive() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.3MF"))),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.3MF")),
            );
        }

        #[test]
        fn a_missing_extension_gets_threemf_appended() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate"))),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.3mf")),
            );
        }

        #[test]
        fn a_different_extension_is_rejected() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.stl"))),
                SaveTargetDispatch::WrongExtension(PathBuf::from("/tmp/plate.stl")),
            );
        }
    }

    mod payload_for_save_tests {
        use super::*;
        use std::path::PathBuf;

        #[test]
        fn a_held_payload_wins_over_the_globals() {
            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/tmp/plate.3mf"),
                held_payload(),
            );
            let payload = payload_for_save(
                &doc,
                Some(printer_id("global-printer")),
                Some(material_id("global-resin")),
                &ProfileLibraries::new(None, vec![]),
            );
            assert_eq!(payload.printer, Some(printer_id("held-printer")));
            assert_eq!(payload.resin, Some(material_id("held-resin")));
        }

        #[test]
        fn a_document_without_project_data_snapshots_the_globals() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            let payload = payload_for_save(
                &doc,
                Some(printer_id("global-printer")),
                Some(material_id("global-resin")),
                &ProfileLibraries::new(None, vec![]),
            );
            assert_eq!(payload.printer, Some(printer_id("global-printer")));
            assert_eq!(payload.resin, Some(material_id("global-resin")));
            assert!(payload.embedded.is_empty());
        }

        // For each of printer and resin, the panel and the slice run use the
        // scene's own choice when there is one and the app-wide default
        // otherwise. The saved payload must hold those same values.
        #[test]
        fn a_partial_selection_is_completed_from_the_globals() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            assert!(doc.set_selected_printer(Some(printer_id("picked-printer"))));
            let payload = payload_for_save(
                &doc,
                Some(printer_id("global-printer")),
                Some(material_id("global-resin")),
                &ProfileLibraries::new(None, vec![]),
            );
            assert_eq!(payload.printer, Some(printer_id("picked-printer")));
            assert_eq!(payload.resin, Some(material_id("global-resin")));
            assert_eq!(payload.first_layer_height, None);
        }

        #[test]
        fn a_first_layer_height_alone_still_records_both_globals() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            let height = sc_print_primitives::PositiveFiniteMm::new(0.1).expect("valid length");
            assert!(doc.set_selected_first_layer_height(Some(height)));
            let payload = payload_for_save(
                &doc,
                Some(printer_id("global-printer")),
                Some(material_id("global-resin")),
                &ProfileLibraries::new(None, vec![]),
            );
            assert_eq!(payload.printer, Some(printer_id("global-printer")));
            assert_eq!(payload.resin, Some(material_id("global-resin")));
            assert_eq!(payload.first_layer_height, Some(height));
        }

        #[test]
        fn referenced_profiles_get_embedded_from_the_roots() {
            let root = tempfile::tempdir().expect("temp library root");
            fs::create_dir_all(root.path().join("printers")).expect("printers dir");
            fs::create_dir_all(root.path().join("materials/extensions")).expect("materials dirs");
            fs::write(
                root.path().join("printers/global-printer.toml"),
                b"kind = \"printer\"",
            )
            .expect("printer file");
            fs::write(
                root.path().join("materials/global-resin.toml"),
                b"kind = \"material\"",
            )
            .expect("material file");
            fs::write(
                root.path().join("materials/extensions/global-resin.toml"),
                b"kind = \"extension\"",
            )
            .expect("extension file");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            let payload = payload_for_save(
                &doc,
                Some(printer_id("global-printer")),
                Some(material_id("global-resin")),
                &ProfileLibraries::new(Some(root.path().to_path_buf()), vec![]),
            );
            assert_eq!(
                payload.embedded,
                vec![
                    EmbeddedProfile::Printer {
                        id: printer_id("global-printer"),
                        bytes: b"kind = \"printer\"".to_vec(),
                    },
                    EmbeddedProfile::Material {
                        id: material_id("global-resin"),
                        bytes: b"kind = \"material\"".to_vec(),
                    },
                    EmbeddedProfile::MaterialExtension {
                        id: material_id("global-resin"),
                        bytes: b"kind = \"extension\"".to_vec(),
                    },
                ],
            );
        }

        #[test]
        fn held_embedded_documents_are_never_replaced() {
            let root = tempfile::tempdir().expect("temp library root");
            fs::create_dir_all(root.path().join("printers")).expect("printers dir");
            fs::write(
                root.path().join("printers/held-printer.toml"),
                b"kind = \"library-copy\"",
            )
            .expect("printer file");

            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/tmp/plate.3mf"),
                held_payload(),
            );
            let payload = payload_for_save(
                &doc,
                None,
                None,
                &ProfileLibraries::new(Some(root.path().to_path_buf()), vec![]),
            );
            let printer_bytes: Vec<&[u8]> = payload
                .embedded
                .iter()
                .filter_map(|p| match p {
                    EmbeddedProfile::Printer { bytes, .. } => Some(bytes.as_slice()),
                    _ => None,
                })
                .collect();
            assert_eq!(printer_bytes, vec![b"kind = \"printer\"".as_slice()]);
        }
    }

    mod save_round_trip_tests {
        use super::*;

        #[test]
        fn saving_without_a_scene_errors() {
            let doc = SceneDocument::new();
            let dir = tempfile::tempdir().expect("temp dir");
            let target = dir.path().join("plate.3mf");
            assert!(save_project_to(&doc, &target, None, None).is_err());
            assert!(!target.exists());
        }

        #[test]
        fn a_save_writes_a_readable_project_and_updates_the_document() {
            let dir = tempfile::tempdir().expect("temp dir");
            let source = dir.path().join("widget.stl");
            let target = dir.path().join("widget.3mf");

            let doc = SceneDocument::new();
            doc.install_project_graph(one_object_graph(), source, held_payload());

            save_project_to(&doc, &target, None, None).expect("the save succeeds");

            assert_eq!(doc.source_path(), Some(target.clone()));
            assert!(doc.is_project());
            assert!(!doc.is_dirty());
            // A follow-up "Save" has nothing to write.
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::AlreadySaved);

            let bytes = fs::read(&target).expect("the file exists");
            let loaded = read_project(&mut Cursor::new(bytes), ProjectFileType::ThreeMf)
                .expect("the saved project reads back");
            assert_eq!(loaded.payload, Some(held_payload()));
            assert_eq!(loaded.scene.roots().len(), 1);
        }

        #[test]
        fn a_save_as_of_a_picked_plain_mesh_leaves_the_mesh_file_untouched_and_relocates_the_document()
         {
            let dir = tempfile::tempdir().expect("temp dir");
            let source = dir.path().join("widget.stl");
            let target = dir.path().join("widget.3mf");
            let stl_bytes = b"solid widget\nendsolid widget\n".to_vec();
            fs::write(&source, &stl_bytes).expect("the mesh file is written");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), source.clone());
            assert!(doc.set_selected_printer(Some(printer_id("picked-printer"))));
            // The pick alone must not route "Save" onto the mesh file.
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);

            save_project_to(&doc, &target, None, None).expect("the save succeeds");

            assert_eq!(fs::read(&source).expect("the mesh file exists"), stl_bytes);
            assert!(target.exists());
            assert!(doc.is_project());
            assert_eq!(doc.source_path(), Some(target.clone()));
            assert!(!doc.is_dirty());
            // From now on "Save" overwrites the project file in place.
            make_dirty(&doc);
            assert_eq!(
                decide_save_dispatch(&doc),
                SaveDispatch::OverwriteProject(target),
            );
        }

        #[test]
        fn a_second_save_overwrites_the_same_file() {
            let dir = tempfile::tempdir().expect("temp dir");
            let target = dir.path().join("plate.3mf");

            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                target.clone(),
                ProjectPayload::default(),
            );

            save_project_to(&doc, &target, None, None).expect("first save");
            let first = fs::read(&target).expect("first bytes");
            save_project_to(&doc, &target, None, None).expect("second save");
            let second = fs::read(&target).expect("second bytes");
            assert_eq!(first.len(), second.len());
        }

        #[test]
        fn a_snapshot_save_records_the_global_references() {
            let dir = tempfile::tempdir().expect("temp dir");
            let target = dir.path().join("widget.3mf");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), dir.path().join("widget.stl"));

            save_project_to(
                &doc,
                &target,
                Some(printer_id("nonexistent-printer")),
                Some(material_id("nonexistent-resin")),
            )
            .expect("the save succeeds");

            let bytes = fs::read(&target).expect("the file exists");
            let loaded = read_project(&mut Cursor::new(bytes), ProjectFileType::ThreeMf)
                .expect("the saved project reads back");
            let payload = loaded.payload.expect("the payload is present");
            assert_eq!(payload.printer, Some(printer_id("nonexistent-printer")));
            assert_eq!(payload.resin, Some(material_id("nonexistent-resin")));
            // The ids resolve nowhere, so nothing was embedded.
            assert!(payload.embedded.is_empty());
            // The written values become the scene's own choices.
            assert_eq!(
                doc.selected_printer(),
                Some(printer_id("nonexistent-printer"))
            );
            assert_eq!(doc.selected_resin(), Some(material_id("nonexistent-resin")));
        }

        #[test]
        fn slices_persist_when_sliced_on_the_default_profile() {
            // Regression: a freshly loaded mesh has no project payload, so
            // `selected_printer`/`selected_resin` read `None` and slicing runs
            // on the app-default profiles the panel shows. The save must
            // persist the held slices against those same defaults rather than
            // silently dropping them, so a reopened project can show them.
            use crate::slice_projection::resolve_current_schedule;
            use crate::slice_result::{SliceParams, SliceResult};
            use sc_print_glue::{PlateContours, slice_input_fingerprint};
            use sc_print_primitives::AaLevel;
            use sc_sl1cer::contour::{ContourLayer, ContourStack};

            let dir = tempfile::tempdir().expect("temp dir");
            let source = dir.path().join("widget.stl");
            let target = dir.path().join("widget.3mf");

            let printer = printer_id("elegoo-mars-5");
            let resin = material_id("siraya-tech-craft-high-clear");

            let doc = SceneDocument::new();
            // No project data: `selected_printer` stays `None`, exactly the
            // state right after a plain mesh load.
            doc.install_graph(one_object_graph(), source);
            let scene = doc
                .with_scene(|s| s.graph().clone())
                .expect("the scene is installed");

            let schedule = resolve_current_schedule(&printer, &resin, None, &scene)
                .expect("the bundled default profiles resolve a schedule");
            let token = slice_input_fingerprint(&scene, &schedule)
                .expect("the scene has sliceable geometry")
                .to_token();
            let layers: Vec<ContourLayer> = (0..schedule.total_layers())
                .map(|_| ContourLayer::new(0.05, vec![]))
                .collect();
            doc.set_slice_result(SliceResult {
                contours: PlateContours::new(ContourStack::new_unchecked(layers), schedule)
                    .expect("1 layer for each scheduled layer"),
                token,
                params: SliceParams {
                    printer: printer.clone(),
                    resin: resin.clone(),
                    first_layer_height: None,
                    aa_level: AaLevel::Off,
                    geometry: doc.scene_revision(),
                },
            });

            save_project_to(&doc, &target, Some(printer), Some(resin)).expect("the save succeeds");

            let bytes = fs::read(&target).expect("the file exists");
            let loaded = read_project(&mut Cursor::new(bytes), ProjectFileType::ThreeMf)
                .expect("the saved project reads back");
            assert!(
                loaded.slices.is_some(),
                "slices sliced on the default profile must survive the save"
            );
            assert!(loaded.slice_fingerprint.is_some());
        }

        #[test]
        fn slices_persist_when_only_the_printer_was_picked() {
            // The pick leaves the resin to the app default, which is what
            // slicing ran on; the saved project must name both and keep the
            // slices.
            use crate::slice_projection::resolve_current_schedule;
            use crate::slice_result::{SliceParams, SliceResult};
            use sc_print_glue::{PlateContours, slice_input_fingerprint};
            use sc_print_primitives::AaLevel;
            use sc_sl1cer::contour::{ContourLayer, ContourStack};

            let dir = tempfile::tempdir().expect("temp dir");
            let source = dir.path().join("widget.stl");
            let target = dir.path().join("widget.3mf");

            let printer = printer_id("elegoo-mars-5");
            let resin = material_id("siraya-tech-craft-high-clear");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), source);
            assert!(doc.set_selected_printer(Some(printer.clone())));
            let scene = doc
                .with_scene(|s| s.graph().clone())
                .expect("the scene is installed");

            let schedule = resolve_current_schedule(&printer, &resin, None, &scene)
                .expect("the bundled profiles resolve a schedule");
            let token = slice_input_fingerprint(&scene, &schedule)
                .expect("the scene has sliceable geometry")
                .to_token();
            let layers: Vec<ContourLayer> = (0..schedule.total_layers())
                .map(|_| ContourLayer::new(0.05, vec![]))
                .collect();
            doc.set_slice_result(SliceResult {
                contours: PlateContours::new(ContourStack::new_unchecked(layers), schedule)
                    .expect("1 layer for each scheduled layer"),
                token,
                params: SliceParams {
                    printer: printer.clone(),
                    resin: resin.clone(),
                    first_layer_height: None,
                    aa_level: AaLevel::Off,
                    geometry: doc.scene_revision(),
                },
            });

            save_project_to(
                &doc,
                &target,
                Some(printer_id("global-printer")),
                Some(resin.clone()),
            )
            .expect("the save succeeds");

            let bytes = fs::read(&target).expect("the file exists");
            let loaded = read_project(&mut Cursor::new(bytes), ProjectFileType::ThreeMf)
                .expect("the saved project reads back");
            let payload = loaded.payload.expect("the project carries a payload");
            assert_eq!(payload.printer, Some(printer));
            assert_eq!(payload.resin, Some(resin));
            assert!(
                loaded.slices.is_some(),
                "slices sliced on the picked printer and default resin must survive the save"
            );
            assert!(loaded.slice_fingerprint.is_some());
        }
    }

    mod write_bundle_tests {
        use super::*;

        #[test]
        fn a_prepared_bundle_writes_a_readable_project() {
            let dir = tempfile::tempdir().expect("temp dir");
            let target = dir.path().join("plate.3mf");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), dir.path().join("widget.stl"));
            let bundle = prepare_save(&doc, held_payload(), target.clone())
                .expect("a loaded document prepares a bundle");

            write_bundle(&bundle, |_, _, _| {}).expect("the bundle writes");

            let bytes = fs::read(&target).expect("the file exists");
            let loaded = read_project(&mut Cursor::new(bytes), ProjectFileType::ThreeMf)
                .expect("the saved project reads back");
            assert_eq!(loaded.payload, Some(held_payload()));
        }

        #[test]
        fn write_bundle_reports_serialize_then_compress_then_writing() {
            let dir = tempfile::tempdir().expect("temp dir");
            let target = dir.path().join("plate.3mf");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), dir.path().join("widget.stl"));
            let bundle = prepare_save(&doc, held_payload(), target)
                .expect("a loaded document prepares a bundle");

            let mut phases = Vec::new();
            write_bundle(&bundle, |phase, _, _| phases.push(phase)).expect("the bundle writes");

            let first_compress = phases
                .iter()
                .position(|p| *p == SavePhase::Compress)
                .expect("a compress phase is reported");
            let first_writing = phases
                .iter()
                .position(|p| *p == SavePhase::Writing)
                .expect("a writing phase is reported");
            assert!(
                phases[..first_compress]
                    .iter()
                    .all(|p| *p == SavePhase::Serialize),
                "serialize reports come before the first compress",
            );
            assert!(
                first_compress < first_writing,
                "compress comes before writing"
            );
        }

        #[test]
        fn preparing_without_a_scene_yields_no_bundle() {
            let doc = SceneDocument::new();
            assert!(prepare_save(&doc, held_payload(), PathBuf::from("/tmp/plate.3mf")).is_none(),);
        }
    }
}

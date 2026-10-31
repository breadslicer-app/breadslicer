/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! "Save" and "Save As" support for project files: functions over
//! `&SceneDocument` plus explicit inputs. The wiring layer drives the file
//! dialog, then calls in here to produce the file.
//!
//! A save writes a project: the scene plus the project payload. "Save" and "Save
//! As" write Breadslicer's `.bread` (#145), which keeps DragonFruit supports as
//! data so they reopen editable, and never stores slices; "Save As 3MF" writes
//! a 3MF project, with the supports as a mesh and the in-sync slices. The
//! payload holds the scene's own printer, resin, first-layer height and printer
//! file, with a missing printer or resin filled from the app-wide default at
//! save time, and the embedded profile documents that the document holds.
//! Referenced profiles without a held document get embedded copies resolved
//! from the local libraries.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sc_mesh_formats::scene_writer::write_bytes_atomically;
use sc_print_profiles::ProfileLibraries;
use sc_project_formats::{
    EmbedProfiles, HollowDocument, PlateSlices, ProjectFileType, ProjectPayload, SupportDocument,
    WriteProgress, write_bread_project_with_progress, write_project_with_progress,
};
use sc_scene::Scene;

use crate::error_dialog::InfoTexts;
use crate::hollow_persist::hollow_documents;
use crate::i18n::ctr;
use crate::print_selection::PrintDefaults;
use crate::project_profiles::complete_embedded_documents;
use crate::save_progress::SavePhase;
use crate::scene_document::{SaveStamp, SceneDocument};
use crate::slice_projection::resolve_current_schedule;
use crate::support_persist::{SupportsAsMesh, support_documents};

/// The format a project is saved in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectFormat {
    /// Breadslicer's own `.bread`, the default.
    Bread,
    /// A 3MF project, on request.
    ThreeMf,
}

impl ProjectFormat {
    /// The extension a file of this format carries, without the dot.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Bread => "bread",
            Self::ThreeMf => "3mf",
        }
    }

    /// The format a save to `path` writes: 3MF for a `.3mf` path, `.bread`
    /// otherwise.
    pub(crate) fn of_path(path: &Path) -> Self {
        if Self::ThreeMf.names(path) {
            Self::ThreeMf
        } else {
            Self::Bread
        }
    }

    /// Whether `path` carries this format's extension (in any case).
    fn names(self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(self.extension()))
    }
}

/// Classification of a "Save" command against the document state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveDispatch {
    /// The document is a `.bread` project with a known location and unsaved
    /// changes: overwrite it in place, no dialog.
    OverwriteProject(PathBuf),
    /// The source is a plain mesh file, a 3MF without project data, or a 3MF
    /// project: route to "Save As", which writes a `.bread`, so an STL, OBJ,
    /// 3MF or other interop file is never silently replaced.
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
    if !saves_in_place(doc) {
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

/// Whether "Save" writes the document back to its own file: only a `.bread`
/// project's. Anything else, a 3MF project included, is saved through "Save
/// As" (see [`SaveDispatch::NeedsSaveAs`]).
pub(crate) fn saves_in_place(doc: &SceneDocument) -> bool {
    doc.is_project()
        && doc
            .with_source_path(|path| ProjectFormat::Bread.names(path))
            .unwrap_or(false)
}

/// Classification of the file-save dialog's outcome for "Save As".
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveTargetDispatch {
    /// The user dismissed the dialog.
    Cancelled,
    /// The picked path names a different format; a project save would not
    /// produce what the name promises.
    WrongExtension(PathBuf),
    /// The path is ready to write (a missing extension gets the format's
    /// appended: the command named the format, so the intent is unambiguous).
    Ready(PathBuf),
}

/// Classifies the path picked for a save in `format`.
pub(crate) fn decide_save_target(
    target: Option<PathBuf>,
    format: ProjectFormat,
) -> SaveTargetDispatch {
    let Some(target) = target else {
        return SaveTargetDispatch::Cancelled;
    };
    if format.names(&target) {
        return SaveTargetDispatch::Ready(target);
    }
    match target.extension() {
        Some(_) => SaveTargetDispatch::WrongExtension(target),
        None => SaveTargetDispatch::Ready(target.with_extension(format.extension())),
    }
}

/// The refusal of a "Save As" path that names another format than the one
/// asked for ([`SaveTargetDispatch::WrongExtension`]). Its text is in the
/// language in force each time it is formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WrongProjectExtension(pub(crate) ProjectFormat);

impl fmt::Display for WrongProjectExtension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&match self.0 {
            ProjectFormat::Bread => ctr!(
                "ErrorDialog",
                "Project files are saved as .bread files. To save a 3MF project, use Save As 3MF."
            ),
            ProjectFormat::ThreeMf => {
                ctr!("ErrorDialog", "Project files are saved as .3mf packages.")
            }
        })
    }
}

impl std::error::Error for WrongProjectExtension {}

/// Builds the payload to write when saving `doc` as a project: the scene's own
/// choices, with a missing printer or resin filled from `defaults`. Every
/// referenced profile without an embedded document then gets one, resolved from
/// `libraries`.
pub(crate) fn payload_for_save(
    doc: &SceneDocument,
    defaults: &PrintDefaults,
    libraries: &ProfileLibraries,
) -> ProjectPayload {
    let mut payload = doc.compose_project_payload(defaults);
    complete_embedded_documents(&mut payload, libraries);
    payload
}

/// Everything a project write needs, captured as owned, `Send` data so the
/// write can run off the UI thread. The scene is a cheap clone of the
/// document's graph (mesh payloads are shared through `Arc`), and the slices,
/// supports, payload, and target are owned. Built on the UI thread by
/// [`prepare_save`]; consumed by [`write_bundle`] on a worker thread.
pub(crate) struct SaveBundle {
    format: ProjectFormat,
    scene: Scene,
    payload: ProjectPayload,
    /// 3MF only: a `.bread` never stores slices.
    slices: Option<PlateSlices>,
    slice_fingerprint: Option<String>,
    /// `.bread` only: the supports kept as data.
    supports: Vec<SupportDocument>,
    /// `.bread` only: set when the scene's supports go in as a mesh.
    supports_as_mesh: Option<SupportsAsMesh>,
    /// `.bread` only: how the models hollowed here were made (#161).
    hollows: Vec<HollowDocument>,
    target: PathBuf,
}

/// Snapshots the document's scene, and the slices (3MF) or the supports
/// (`.bread`) to persist, into a `Send` [`SaveBundle`], on the UI thread. The
/// format follows the target's extension ([`ProjectFormat::of_path`]). Returns
/// `None` when no scene is loaded. The scene clone and the slice or support
/// capture are taken together under the same scene borrow, so what is
/// persisted matches the geometry written.
pub(crate) fn prepare_save(
    doc: &SceneDocument,
    payload: ProjectPayload,
    target: PathBuf,
) -> Option<SaveBundle> {
    let format = ProjectFormat::of_path(&target);
    doc.with_scene(|spatial| {
        let scene = spatial.graph().clone();
        let (mut slices, mut slice_fingerprint) = (None, None);
        let (mut supports, mut supports_as_mesh) = (Vec::new(), None);
        let mut hollows = Vec::new();
        match format {
            ProjectFormat::ThreeMf => {
                if let Some((plate, token)) = persist_slices(doc, &payload, spatial.graph()) {
                    slices = Some(plate);
                    slice_fingerprint = Some(token);
                }
            }
            ProjectFormat::Bread => {
                let kept = support_documents(doc, spatial);
                supports = kept.documents;
                supports_as_mesh = kept.as_mesh;
                hollows = hollow_documents(doc, spatial);
            }
        }
        SaveBundle {
            format,
            scene,
            payload,
            slices,
            slice_fingerprint,
            supports,
            supports_as_mesh,
            hollows,
            target,
        }
    })
}

impl SaveBundle {
    /// Why the scene's supports go into a `.bread` as a mesh, or `None` when
    /// they are kept as data or there are none.
    pub(crate) fn supports_as_mesh(&self) -> Option<SupportsAsMesh> {
        self.supports_as_mesh
    }
}

/// The notice for a save whose supports went in as a mesh.
pub(crate) fn supports_as_mesh_texts(why: SupportsAsMesh) -> InfoTexts {
    let message = match why {
        SupportsAsMesh::NoSession => ctr!(
            "DfSupportDialog",
            "These supports no longer match the models (or were opened as a mesh), so they were saved as a mesh: they will open, but cannot be edited by hand. Generate them again and save to keep them editable."
        ),
        SupportsAsMesh::NoRecipe => ctr!(
            "DfSupportDialog",
            "These supports cannot be kept as data, so they were saved as a mesh: they will open, but cannot be edited by hand."
        ),
    };
    InfoTexts {
        heading: ctr!("DfSupportDialog", "Supports saved as a mesh"),
        message,
        details: Default::default(),
    }
}

/// Writes a prepared bundle to its target in its format, atomically,
/// reporting each phase through `progress`. Pure with respect to the document,
/// so it runs on a worker thread; the caller records success on the UI thread.
pub(crate) fn write_bundle(
    bundle: &SaveBundle,
    mut progress: impl FnMut(SavePhase, u64, u64),
) -> Result<()> {
    let bytes = encode_bundle(bundle, &mut progress)?;
    progress(SavePhase::Writing, 0, 1);
    write_bytes_atomically(&bundle.target, &bytes)?;
    progress(SavePhase::Writing, 1, 1);
    Ok(())
}

/// The bytes of a prepared bundle in its format, as [`write_bundle`] writes
/// them; also what a diagnostics file carries as the scene's models.
pub(crate) fn encode_bundle(
    bundle: &SaveBundle,
    mut progress: impl FnMut(SavePhase, u64, u64),
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let report = |report| match report {
        WriteProgress::Serialize { done, total } => progress(SavePhase::Serialize, done, total),
        WriteProgress::Compress { done, total } => progress(SavePhase::Compress, done, total),
    };
    match bundle.format {
        ProjectFormat::ThreeMf => write_project_with_progress(
            &bundle.scene,
            &bundle.payload,
            EmbedProfiles::Yes,
            bundle.slices.as_ref(),
            bundle.slice_fingerprint.as_deref(),
            ProjectFileType::ThreeMf,
            &mut bytes,
            report,
        )?,
        ProjectFormat::Bread => write_bread_project_with_progress(
            &bundle.scene,
            &bundle.payload,
            EmbedProfiles::Yes,
            &bundle.supports,
            &bundle.hollows,
            &mut bytes,
            report,
        )?,
    }
    Ok(bytes)
}

/// Returns the held slices to persist with the project, or `None` when the
/// scene has no in-sync slice result to write. Resolves the current layer
/// schedule from `payload`'s printer, resin and first-layer height, and defers
/// the in-sync decision to the document's write guard; a missing profile or a
/// resolution failure persists nothing.
fn persist_slices(
    doc: &SceneDocument,
    payload: &ProjectPayload,
    scene: &Scene,
) -> Option<(PlateSlices, String)> {
    let printer = payload.printer.clone()?;
    let resin = payload.resin.clone()?;
    let schedule =
        resolve_current_schedule(&printer, &resin, payload.first_layer_height, scene).ok()?;
    doc.slices_for_persist(&schedule)
}

/// Runs a full project save against `target` synchronously: builds the payload,
/// snapshots and writes the bundle, and on success updates the document so the
/// held payload matches the file, "Save" overwrites `target` from now on, and
/// the dirty marker clears. The GUI drives the write off the UI thread; this
/// synchronous composition serves the save round-trip tests.
#[cfg(test)]
pub(crate) fn save_project_to(
    doc: &SceneDocument,
    target: &Path,
    defaults: &PrintDefaults,
) -> Result<()> {
    let (bundle, payload, stamp) = prepare_save_for_target(doc, target.to_path_buf(), defaults)
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
    defaults: &PrintDefaults,
) -> Option<(SaveBundle, ProjectPayload, SaveStamp)> {
    // A live preview is not the scene: the file gets what the history has.
    doc.end_live_preview();
    let payload = payload_for_save(doc, defaults, &ProfileLibraries::platform_default());
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
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
    use sc_print_primitives::{MaterialId, PrinterId};
    use sc_project_formats::{EmbeddedProfile, read_project};
    use sc_scene::{LeafPayload, Scene};

    fn printer_id(raw: &str) -> PrinterId {
        PrinterId::new(raw).expect("test id is a valid slug")
    }

    fn material_id(raw: &str) -> MaterialId {
        MaterialId::new(raw).expect("test id is a valid slug")
    }

    fn defaults(printer: PrinterId, resin: MaterialId) -> PrintDefaults {
        PrintDefaults {
            printer: Some(printer),
            resin: Some(resin),
        }
    }

    fn unit_triangle() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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

    /// Commits a transform edit so the document reads dirty.
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
                PathBuf::from("/tmp/plate.bread"),
                ProjectPayload::default(),
            );
            make_dirty(&doc);
            assert_eq!(
                decide_save_dispatch(&doc),
                SaveDispatch::OverwriteProject(PathBuf::from("/tmp/plate.bread")),
            );
        }

        #[test]
        fn a_clean_project_document_writes_nothing() {
            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/tmp/plate.bread"),
                ProjectPayload::default(),
            );
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::AlreadySaved);
        }

        // "Save" writes a `.bread`: a 3MF project, clean or dirty, goes
        // through the dialog rather than being overwritten in another format.
        #[test]
        fn a_3mf_project_document_routes_to_save_as() {
            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/tmp/plate.3mf"),
                ProjectPayload::default(),
            );
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
            make_dirty(&doc);
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
        }

        // A scene read from an autosave or an earlier version (#18) stands
        // for its project: Save goes there, never to the copy it was read
        // from, and only once the scene reads as unsaved.
        #[test]
        fn a_recovered_project_saves_back_to_its_project() {
            use crate::scene_document::DocumentOrigin;

            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/data/autosave/run/window-0-3.bread"),
                ProjectPayload::default(),
            );
            assert!(!doc.is_dirty());
            doc.restore_identity(PathBuf::from("/tmp/hueco.bread"), DocumentOrigin::Project);
            assert!(doc.is_dirty());
            assert_eq!(
                decide_save_dispatch(&doc),
                SaveDispatch::OverwriteProject(PathBuf::from("/tmp/hueco.bread")),
            );
        }

        #[test]
        fn a_recovered_mesh_scene_still_routes_to_save_as() {
            use crate::scene_document::DocumentOrigin;

            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                PathBuf::from("/data/autosave/run/window-0-3.bread"),
                ProjectPayload::default(),
            );
            doc.restore_identity(PathBuf::from("/tmp/widget.stl"), DocumentOrigin::PlainMesh);
            assert!(doc.is_dirty());
            assert_eq!(doc.source_path(), Some(PathBuf::from("/tmp/widget.stl")));
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);
        }

        #[test]
        fn a_recovered_scene_reads_clean_once_saved() {
            use crate::scene_document::DocumentOrigin;

            let dir = tempfile::tempdir().unwrap();
            let project = dir.path().join("hueco.bread");
            let doc = SceneDocument::new();
            doc.install_project_graph(
                one_object_graph(),
                dir.path().join("window-0-3.bread"),
                ProjectPayload::default(),
            );
            doc.restore_identity(project.clone(), DocumentOrigin::Project);
            save_project_to(&doc, &project, &PrintDefaults::default()).expect("saved");
            assert!(!doc.is_dirty());
            assert!(project.exists());
            assert!(!dir.path().join("window-0-3.bread").exists());
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

        // A printer, resin or first-layer pick makes the document dirty, and
        // the source stays a plain mesh file: "Save" still goes through the
        // dialog, so the mesh file keeps its content.
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

        use ProjectFormat::{Bread, ThreeMf};

        #[test]
        fn none_classifies_as_cancelled() {
            assert_eq!(
                decide_save_target(None, Bread),
                SaveTargetDispatch::Cancelled
            );
        }

        #[test]
        fn a_bread_extension_is_ready_as_is() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.bread")), Bread),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.bread")),
            );
        }

        #[test]
        fn a_threemf_extension_is_ready_as_is_for_a_3mf_save() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.3mf")), ThreeMf),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.3mf")),
            );
        }

        #[test]
        fn the_extension_match_is_case_insensitive() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.3MF")), ThreeMf),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.3MF")),
            );
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.Bread")), Bread),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.Bread")),
            );
        }

        #[test]
        fn a_missing_extension_gets_the_format_appended() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate")), Bread),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.bread")),
            );
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate")), ThreeMf),
                SaveTargetDispatch::Ready(PathBuf::from("/tmp/plate.3mf")),
            );
        }

        #[test]
        fn a_different_extension_is_rejected() {
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.stl")), Bread),
                SaveTargetDispatch::WrongExtension(PathBuf::from("/tmp/plate.stl")),
            );
            // Each command writes its own format only.
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.3mf")), Bread),
                SaveTargetDispatch::WrongExtension(PathBuf::from("/tmp/plate.3mf")),
            );
            assert_eq!(
                decide_save_target(Some(PathBuf::from("/tmp/plate.bread")), ThreeMf),
                SaveTargetDispatch::WrongExtension(PathBuf::from("/tmp/plate.bread")),
            );
        }

        #[test]
        fn the_refusal_names_the_format_asked_for() {
            assert_eq!(
                WrongProjectExtension(Bread).to_string(),
                "Project files are saved as .bread files. To save a 3MF project, use Save As 3MF."
            );
            assert_eq!(
                WrongProjectExtension(ThreeMf).to_string(),
                "Project files are saved as .3mf packages."
            );
        }

        #[test]
        fn the_target_extension_picks_the_written_format() {
            assert_eq!(ProjectFormat::of_path(Path::new("/tmp/a.3MF")), ThreeMf);
            assert_eq!(ProjectFormat::of_path(Path::new("/tmp/a.bread")), Bread);
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
                &defaults(printer_id("global-printer"), material_id("global-resin")),
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
                &defaults(printer_id("global-printer"), material_id("global-resin")),
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
                &defaults(printer_id("global-printer"), material_id("global-resin")),
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
                &defaults(printer_id("global-printer"), material_id("global-resin")),
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
                &defaults(printer_id("global-printer"), material_id("global-resin")),
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
                &PrintDefaults::default(),
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
            assert!(save_project_to(&doc, &target, &PrintDefaults::default()).is_err());
            assert!(!target.exists());
        }

        #[test]
        fn a_save_writes_a_readable_project_and_updates_the_document() {
            let dir = tempfile::tempdir().expect("temp dir");
            let source = dir.path().join("widget.stl");
            let target = dir.path().join("widget.3mf");

            let doc = SceneDocument::new();
            doc.install_project_graph(one_object_graph(), source, held_payload());

            save_project_to(&doc, &target, &PrintDefaults::default()).expect("the save succeeds");

            assert_eq!(doc.source_path(), Some(target.clone()));
            assert!(doc.is_project());
            assert!(!doc.is_dirty());
            // "Save" writes a `.bread`, so it goes through Save As rather than
            // overwrite the 3MF.
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);

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
            let target = dir.path().join("widget.bread");
            let stl_bytes = b"solid widget\nendsolid widget\n".to_vec();
            fs::write(&source, &stl_bytes).expect("the mesh file is written");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), source.clone());
            assert!(doc.set_selected_printer(Some(printer_id("picked-printer"))));
            // The pick alone must not route "Save" onto the mesh file.
            assert_eq!(decide_save_dispatch(&doc), SaveDispatch::NeedsSaveAs);

            save_project_to(&doc, &target, &PrintDefaults::default()).expect("the save succeeds");

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
        fn a_bread_save_writes_a_readable_project_and_updates_the_document() {
            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stl"));
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("widget.bread");
            save_project_to(
                &doc,
                &target,
                &defaults(printer_id("global-printer"), material_id("global-resin")),
            )
            .expect("the bread project saves");
            let loaded = read_project(
                &mut fs::File::open(&target).unwrap(),
                ProjectFileType::Bread,
            )
            .expect("the file reads back as a .bread project");
            let payload = loaded.payload.expect("a payload");
            assert_eq!(payload.printer, Some(printer_id("global-printer")));
            assert!(loaded.supports.is_empty(), "no supports in the scene");
            assert!(loaded.slices.is_none());
            assert_eq!(doc.source_path(), Some(target.clone()));
            assert!(doc.is_project());
            assert!(!doc.is_dirty());
            // From here on "Save" overwrites the `.bread` in place.
            make_dirty(&doc);
            assert_eq!(
                decide_save_dispatch(&doc),
                SaveDispatch::OverwriteProject(target)
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

            save_project_to(&doc, &target, &PrintDefaults::default()).expect("first save");
            let first = fs::read(&target).expect("first bytes");
            save_project_to(&doc, &target, &PrintDefaults::default()).expect("second save");
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
                &defaults(
                    printer_id("nonexistent-printer"),
                    material_id("nonexistent-resin"),
                ),
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
            let own = doc.print_selection();
            assert_eq!(own.printer, Some(printer_id("nonexistent-printer")));
            assert_eq!(own.resin, Some(material_id("nonexistent-resin")));
        }

        #[test]
        fn slices_persist_when_sliced_on_the_default_profile() {
            // A freshly loaded mesh has no own printer or resin, so slicing
            // runs on the app-wide defaults that the panel shows. The save
            // persists the held slices against those defaults, so a reopened
            // project can show them.
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
            // No project data: the scene has no own printer, as right after a
            // plain mesh load.
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
                layers: None,
            });

            save_project_to(&doc, &target, &defaults(printer, resin)).expect("the save succeeds");

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
                layers: None,
            });

            save_project_to(
                &doc,
                &target,
                &defaults(printer_id("global-printer"), resin.clone()),
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

        #[test]
        fn slices_persist_when_sliced_with_an_own_first_layer_height() {
            // The resin's layer height is 0.05 mm, so a schedule resolved
            // without the own first-layer height of 0.1 mm differs from the
            // held slice's, and the save would drop the slices.
            use crate::slice_projection::resolve_current_schedule;
            use crate::slice_result::{SliceParams, SliceResult};
            use sc_print_glue::{PlateContours, slice_input_fingerprint};
            use sc_print_primitives::{AaLevel, PositiveFiniteMm};
            use sc_sl1cer::contour::{ContourLayer, ContourStack};

            let dir = tempfile::tempdir().expect("temp dir");
            let source = dir.path().join("widget.stl");
            let target = dir.path().join("widget.3mf");

            let printer = printer_id("elegoo-mars-5");
            let resin = material_id("siraya-tech-craft-high-clear");
            let height = PositiveFiniteMm::new(0.1).expect("valid length");

            let doc = SceneDocument::new();
            doc.install_graph(one_object_graph(), source);
            assert!(doc.set_selected_first_layer_height(Some(height)));
            let scene = doc
                .with_scene(|s| s.graph().clone())
                .expect("the scene is installed");

            let schedule = resolve_current_schedule(&printer, &resin, Some(height), &scene)
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
                    first_layer_height: Some(height),
                    aa_level: AaLevel::Off,
                    geometry: doc.scene_revision(),
                },
                layers: None,
            });

            save_project_to(&doc, &target, &defaults(printer, resin)).expect("the save succeeds");

            let bytes = fs::read(&target).expect("the file exists");
            let loaded = read_project(&mut Cursor::new(bytes), ProjectFileType::ThreeMf)
                .expect("the saved project reads back");
            assert!(
                loaded.slices.is_some(),
                "slices sliced with the own first-layer height must survive the save"
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

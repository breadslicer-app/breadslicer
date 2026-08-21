/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! "Export As" support: pure functions over `&SceneDocument` plus the
//! worker-side write step. The viewport stays out of this module; the
//! wiring layer drives the file dialog and the worker thread, then
//! calls in here to snapshot the scene and produce the file.

use std::path::Path;

use anyhow::Result;
use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::ports::WriteSceneProgress;
use sc_mesh_formats::scene_writer::{self, FlattenWarning};
use sc_scene::Scene;

use crate::export_progress::ExportPhase;
use crate::scene_document::SceneDocument;

/// Fallback extension used when we can't infer a better one from the
/// source path. STL binary is the project's default export format.
const DEFAULT_EXPORT_EXTENSION: &str = "stl";

/// Default filename stem to seed the file-save dialog with: the
/// original load path's stem (e.g. `gear.obj` -> `gear`). `None` when
/// no scene is loaded or the source path has no recognisable stem.
pub(crate) fn default_export_stem(doc: &SceneDocument) -> Option<String> {
    doc.with_source_path(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_owned()))
        .flatten()
}

/// Default extension to seed the dialog with. Matches the source
/// path's extension when it's one of the formats this app can write
/// back out (`.obj`, `.stl`, `.stla`, `.3mf`); otherwise falls back
/// to the project default of binary STL.
///
/// rfd 0.17 does not expose which filter the user actually picked, so
/// the seeded extension is the only signal that tells the user which
/// format they're about to write. Matching the source preserves the
/// "Export As" sensible default of "same format unless I change it".
pub(crate) fn default_export_extension(doc: &SceneDocument) -> &'static str {
    let from_source = doc
        .with_source_path(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
        })
        .flatten();
    match from_source.as_deref() {
        Some("obj") => "obj",
        Some("stla") => "stla",
        // `.stl` and `.stlb` both round-trip via binary STL.
        Some("stl") | Some("stlb") => "stl",
        Some("3mf") => "3mf",
        _ => DEFAULT_EXPORT_EXTENSION,
    }
}

/// Snapshots the document's scene for an off-thread export: one
/// borrow, one clone. The clone shares mesh payloads through `Arc`,
/// so it is cheap relative to the write, and taking it releases the
/// scene borrow so the document stays usable while a worker writes
/// the clone. Returns `None` when no scene is loaded.
pub(crate) fn prepare_export(doc: &SceneDocument) -> Option<Scene> {
    doc.with_scene(|spatial| spatial.graph().clone())
}

/// Writes a snapshotted scene to `target` in `format`, reporting each
/// phase through `progress`. Pure with respect to the document, so it
/// runs on a worker thread; the caller surfaces warnings and errors on
/// the UI thread. Returns the warnings the flatten step produced
/// (currently `MultiObjectMerge` for STL with >1 object).
pub(crate) fn write_export(
    scene: &Scene,
    target: &Path,
    format: WriteFormat,
    mut progress: impl FnMut(ExportPhase, u64, u64),
) -> Result<Vec<FlattenWarning>> {
    scene_writer::write_scene_with_progress(scene, target, format, |report| match report {
        WriteSceneProgress::Flatten { done, total } => progress(ExportPhase::Flatten, done, total),
        WriteSceneProgress::Serialize { done, total } => {
            progress(ExportPhase::Serialize, done, total)
        }
        WriteSceneProgress::Compress { done, total } => {
            progress(ExportPhase::Compress, done, total)
        }
        WriteSceneProgress::Write { done, total } => progress(ExportPhase::Write, done, total),
    })
    .map_err(anyhow::Error::from)
}

/// The synchronous form of a full export against `target`: snapshot
/// plus write in the caller's thread. The GUI drives the write off the
/// UI thread instead; this direct composition backs the format
/// round-trip tests. Errors when no scene is loaded.
#[cfg(test)]
pub(crate) fn export_scene_as(
    doc: &SceneDocument,
    target: &Path,
    format: WriteFormat,
) -> Result<Vec<FlattenWarning>> {
    let scene = prepare_export(doc).ok_or_else(|| anyhow::anyhow!("No scene loaded"))?;
    write_export(&scene, target, format, |_, _, _| {})
}

#[cfg(test)]
mod scene_export_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};
    use sc_scene::{LeafPayload, Scene};

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

    /// Loader-shape one-object graph: a single inline-mesh leaf
    /// under an identity-Group root.
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

    /// Graph with a registered single-triangle subtree, one plain
    /// leaf at the origin, and 2 references to the subtree at
    /// x = +10 / +20: 3 expanded occurrences in total.
    fn graph_with_subtree_references() -> Scene {
        let mut g = Scene::new();
        let sub_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("add subtree leaf");
        let sub_root = g.add_group(None, Transform3D::Identity);
        g.add_child(sub_root, sub_leaf)
            .expect("attach subtree leaf");
        let rid = g
            .register_subtree_resource(sub_root)
            .expect("unparented group registers");
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
        for x in [10.0, 20.0] {
            let reference = g
                .add_reference(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.0])),
                    rid,
                )
                .expect("registered resource resolves");
            g.add_child(root, reference).expect("attach reference");
        }
        g
    }

    /// Loader-shape two-object graph: both leaves attached under a
    /// shared identity-Group root.
    fn two_object_graph() -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let leaf_a = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("add leaf A");
        g.add_child(root, leaf_a).expect("attach leaf A");
        let leaf_b = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("add leaf B");
        g.add_child(root, leaf_b).expect("attach leaf B");
        g
    }

    fn tmp_path(suffix: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "scene_export_test_{}_{}.tmp",
            std::process::id(),
            suffix
        ));
        p
    }

    #[test]
    fn default_export_stem_returns_none_without_a_scene() {
        let doc = SceneDocument::new();
        assert!(default_export_stem(&doc).is_none());
    }

    #[test]
    fn default_export_stem_uses_source_path_stem() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.obj"));
        assert_eq!(default_export_stem(&doc).as_deref(), Some("widget"));
    }

    #[test]
    fn default_export_stem_handles_no_extension() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget"));
        assert_eq!(default_export_stem(&doc).as_deref(), Some("widget"));
    }

    #[test]
    fn default_export_extension_falls_back_without_a_scene() {
        let doc = SceneDocument::new();
        assert_eq!(default_export_extension(&doc), "stl");
    }

    #[test]
    fn default_export_extension_keeps_obj_source() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.obj"));
        assert_eq!(default_export_extension(&doc), "obj");
    }

    #[test]
    fn default_export_extension_keeps_ascii_stl_source() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stla"));
        assert_eq!(default_export_extension(&doc), "stla");
    }

    #[test]
    fn default_export_extension_normalises_stl_variants_to_binary() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.stlb"));
        assert_eq!(default_export_extension(&doc), "stl");

        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.STL"));
        assert_eq!(default_export_extension(&doc), "stl");
    }

    #[test]
    fn default_export_extension_keeps_threemf_source() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.3mf"));
        assert_eq!(default_export_extension(&doc), "3mf");
    }

    #[test]
    fn default_export_extension_falls_back_for_unknown_or_missing_extension() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.ply"));
        assert_eq!(default_export_extension(&doc), "stl");

        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget"));
        assert_eq!(default_export_extension(&doc), "stl");
    }

    #[test]
    fn export_without_a_scene_errors() {
        let doc = SceneDocument::new();
        let target = tmp_path("no_scene");
        let result = export_scene_as(&doc, &target, WriteFormat::StlBin);
        assert!(result.is_err());
    }

    #[test]
    fn prepare_export_returns_none_without_a_scene() {
        let doc = SceneDocument::new();
        assert!(prepare_export(&doc).is_none());
    }

    #[test]
    fn prepare_export_releases_the_scene_borrow() {
        // The snapshot exists so a worker can write it while the
        // document stays usable; holding it must not keep the scene
        // borrowed.
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.obj"));
        let snapshot = prepare_export(&doc).expect("scene is loaded");
        assert!(doc.with_scene(|_| ()).is_some());
        drop(snapshot);
    }

    #[test]
    fn export_snapshot_is_independent_of_later_scene_edits() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/widget.obj"));
        let snapshot = prepare_export(&doc).expect("scene is loaded");

        // Replace the document's scene after the snapshot; the write
        // must still produce the snapshotted single-object scene.
        doc.install_graph(two_object_graph(), PathBuf::from("/tmp/other.obj"));

        let target = tmp_path("snapshot_3mf");
        let warnings = write_export(&snapshot, &target, WriteFormat::ThreeMf, |_, _, _| {})
            .expect("write should succeed");
        assert!(warnings.is_empty());
        let loaded = sc_mesh_formats::load_mesh_from_path(
            &target,
            Some(sc_mesh_formats::filetypes::MeshFileType::_3mf),
            None,
        )
        .expect("exported 3MF must load back");
        assert_eq!(loaded.graph.reachable_leaf_count(), 1);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn write_export_reports_phases_in_order_for_stl() {
        let doc = SceneDocument::new();
        doc.install_graph(two_object_graph(), PathBuf::from("/tmp/multi.obj"));
        let snapshot = prepare_export(&doc).expect("scene is loaded");

        let target = tmp_path("phases_stlb");
        let mut reports: Vec<(ExportPhase, u64, u64)> = Vec::new();
        write_export(
            &snapshot,
            &target,
            WriteFormat::StlBin,
            |phase, done, total| {
                reports.push((phase, done, total));
            },
        )
        .expect("write should succeed");

        assert_eq!(reports.first(), Some(&(ExportPhase::Flatten, 0, 1)));
        assert!(reports.contains(&(ExportPhase::Flatten, 1, 1)));
        assert!(
            reports
                .iter()
                .any(|(p, done, total)| *p == ExportPhase::Serialize
                    && done == total
                    && *total > 0)
        );
        assert_eq!(reports.last(), Some(&(ExportPhase::Write, 1, 1)));
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_single_object_stl_binary_writes_a_non_empty_file() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/single.obj"));
        let target = tmp_path("single_stlb");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::StlBin).expect("write should succeed");
        assert!(warnings.is_empty(), "single-object export emits no warning");
        let bytes = std::fs::read(&target).expect("file should exist");
        // Binary STL: 80-byte header + 4-byte triangle count + n*50.
        assert!(bytes.len() > 80, "binary stl must include a header");
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_single_object_stl_ascii_starts_with_solid_keyword() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/single.obj"));
        let target = tmp_path("single_stla");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::StlAscii).expect("write should succeed");
        assert!(warnings.is_empty());
        let text = std::fs::read_to_string(&target).expect("file should exist");
        assert!(text.starts_with("solid"));
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_single_object_obj_writes_obj_keywords() {
        let doc = SceneDocument::new();
        doc.install_graph(one_object_graph(), PathBuf::from("/tmp/single.obj"));
        let target = tmp_path("single_obj");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::Obj).expect("write should succeed");
        assert!(warnings.is_empty());
        let text = std::fs::read_to_string(&target).expect("file should exist");
        assert!(text.contains("v "), "OBJ file must contain vertex lines");
        assert!(text.contains("f "), "OBJ file must contain face lines");
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_multi_object_threemf_writes_a_loadable_package() {
        let doc = SceneDocument::new();
        doc.install_graph(two_object_graph(), PathBuf::from("/tmp/multi.obj"));
        let target = tmp_path("multi_3mf");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::ThreeMf).expect("write should succeed");
        assert!(warnings.is_empty(), "3MF export preserves everything");
        // The tmp_path helper appends `.tmp`, so extension inference
        // cannot run; name the format explicitly for the load-back.
        let loaded = sc_mesh_formats::load_mesh_from_path(
            &target,
            Some(sc_mesh_formats::filetypes::MeshFileType::_3mf),
            None,
        )
        .expect("exported 3MF must load back");
        assert_eq!(loaded.graph.reachable_leaf_count(), 2);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_multi_object_stl_emits_merge_warning() {
        let doc = SceneDocument::new();
        doc.install_graph(two_object_graph(), PathBuf::from("/tmp/multi.obj"));
        let target = tmp_path("multi_stlb");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::StlBin).expect("write should succeed");
        assert_eq!(
            warnings,
            vec![FlattenWarning::MultiObjectMerge { object_count: 2 }]
        );
        let _ = std::fs::remove_file(&target);
    }

    // Capability pins on write_export, the worker-side write path
    // the GUI drives: it must expand scenes with subtree references.
    // Permanent pins; a refusal or skipping walk below this seam
    // turns them red. They call write_export on a bare Scene rather
    // than going through a SceneDocument because SceneSpatial's
    // cached walks do not expand references and its construction
    // rejects scenes with subtree references in debug builds; the export write
    // path operates on the raw graph and carries no such limit.

    #[test]
    fn export_expands_subtree_references_for_obj() {
        let scene = graph_with_subtree_references();
        let target = tmp_path("referenced_obj");
        let warnings = write_export(&scene, &target, WriteFormat::Obj, |_, _, _| {})
            .expect("write should succeed");
        assert!(warnings.is_empty());
        let text = std::fs::read_to_string(&target).expect("file should exist");
        assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 3);
        assert!(text.contains("v 11 0 0"), "first occurrence missing");
        assert!(text.contains("v 21 0 0"), "second occurrence missing");
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_expands_subtree_references_for_binary_stl() {
        let scene = graph_with_subtree_references();
        let target = tmp_path("referenced_stlb");
        let warnings = write_export(&scene, &target, WriteFormat::StlBin, |_, _, _| {})
            .expect("write should succeed");
        assert_eq!(
            warnings,
            vec![FlattenWarning::MultiObjectMerge { object_count: 3 }]
        );
        let bytes = std::fs::read(&target).expect("file should exist");
        // Binary STL layout: 80-byte header + 4-byte count + 3 * 50.
        assert_eq!(bytes.len(), 80 + 4 + 3 * 50);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_expands_subtree_references_for_ascii_stl() {
        let scene = graph_with_subtree_references();
        let target = tmp_path("referenced_stla");
        let warnings = write_export(&scene, &target, WriteFormat::StlAscii, |_, _, _| {})
            .expect("write should succeed");
        assert_eq!(
            warnings,
            vec![FlattenWarning::MultiObjectMerge { object_count: 3 }]
        );
        let text = std::fs::read_to_string(&target).expect("file should exist");
        assert!(text.starts_with("solid"));
        assert_eq!(text.matches("facet normal").count(), 3);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_expands_subtree_references_for_threemf() {
        // 3MF keeps the sharing native; the expanding
        // reader loads the written package back as all 3
        // occurrences.
        let scene = graph_with_subtree_references();
        let target = tmp_path("referenced_3mf");
        let warnings = write_export(&scene, &target, WriteFormat::ThreeMf, |_, _, _| {})
            .expect("write should succeed");
        assert!(warnings.is_empty(), "3MF export preserves everything");
        let loaded = sc_mesh_formats::load_mesh_from_path(
            &target,
            Some(sc_mesh_formats::filetypes::MeshFileType::_3mf),
            None,
        )
        .expect("exported 3MF must load back");
        assert_eq!(loaded.graph.reachable_leaf_count(), 3);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_multi_object_obj_does_not_emit_merge_warning() {
        // OBJ preserves per-object boundaries, so the flatten step
        // doesn't run and the warning vec is empty regardless of object
        // count.
        let doc = SceneDocument::new();
        doc.install_graph(two_object_graph(), PathBuf::from("/tmp/multi.obj"));
        let target = tmp_path("multi_obj");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::Obj).expect("write should succeed");
        assert!(warnings.is_empty());
        let _ = std::fs::remove_file(&target);
    }
}

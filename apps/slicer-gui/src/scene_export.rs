/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! "Export As" support: pure functions over `&SceneDocument`. The
//! viewport stays out of this module; the wiring layer drives the
//! file dialog, then calls in here to produce the file.

use std::path::Path;

use anyhow::{Result, anyhow};
use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::scene_writer::{self, FlattenWarning};

use crate::scene_document::SceneDocument;

/// Fallback extension used when we can't infer a better one from the
/// source path. STL binary is the project's default export format.
const DEFAULT_EXPORT_EXTENSION: &str = "stl";

/// Default filename stem to seed the file-save dialog with: the
/// original load path's stem (e.g. `gear.obj` → `gear`). `None` when
/// no scene is loaded or the source path has no recognisable stem.
pub(crate) fn default_export_stem(doc: &SceneDocument) -> Option<String> {
    doc.with_source_path(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_owned()))
        .flatten()
}

/// Default extension to seed the dialog with. Matches the source
/// path's extension when it's one of the formats this app can write
/// back out (`.obj`, `.stl`, `.stla`); otherwise falls back to the
/// project default of binary STL.
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
        _ => DEFAULT_EXPORT_EXTENSION,
    }
}

/// Writes the document's current scene out to `target` in `format`.
/// Returns the warnings the flatten step produced (currently
/// `MultiObjectMerge` for STL with >1 object). Errors when no scene is
/// loaded: the menu item should be disabled in that case, but the
/// guard catches a stale callback racing the close path.
pub(crate) fn export_scene_as(
    doc: &SceneDocument,
    target: &Path,
    format: WriteFormat,
) -> Result<Vec<FlattenWarning>> {
    let warnings = doc
        .with_scene(|spatial| -> Result<Vec<FlattenWarning>> {
            scene_writer::write_scene(spatial.graph(), target, format).map_err(anyhow::Error::from)
        })
        .ok_or_else(|| anyhow!("No scene loaded"))??;
    Ok(warnings)
}

#[cfg(test)]
mod scene_export_tests {
    use super::*;
    use std::path::PathBuf;

    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};

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

    fn one_object_scene() -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(unit_triangle()),
            }],
        }
    }

    fn two_object_scene() -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(unit_triangle()),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Affine(AffineTransform3D::from_translation([
                        10.0, 0.0, 0.0,
                    ])),
                    content: SceneObjectContent::Mesh(unit_triangle()),
                },
            ],
        }
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
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget.obj"));
        assert_eq!(default_export_stem(&doc).as_deref(), Some("widget"));
    }

    #[test]
    fn default_export_stem_handles_no_extension() {
        let doc = SceneDocument::new();
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget"));
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
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget.obj"));
        assert_eq!(default_export_extension(&doc), "obj");
    }

    #[test]
    fn default_export_extension_keeps_ascii_stl_source() {
        let doc = SceneDocument::new();
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget.stla"));
        assert_eq!(default_export_extension(&doc), "stla");
    }

    #[test]
    fn default_export_extension_normalises_stl_variants_to_binary() {
        let doc = SceneDocument::new();
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget.stlb"));
        assert_eq!(default_export_extension(&doc), "stl");

        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget.STL"));
        assert_eq!(default_export_extension(&doc), "stl");
    }

    #[test]
    fn default_export_extension_falls_back_for_unknown_or_missing_extension() {
        let doc = SceneDocument::new();
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget.ply"));
        assert_eq!(default_export_extension(&doc), "stl");

        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/widget"));
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
    fn export_single_object_stl_binary_writes_a_non_empty_file() {
        let doc = SceneDocument::new();
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/single.obj"));
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
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/single.obj"));
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
        doc.install_scene(one_object_scene(), PathBuf::from("/tmp/single.obj"));
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
    fn export_multi_object_stl_emits_merge_warning() {
        let doc = SceneDocument::new();
        doc.install_scene(two_object_scene(), PathBuf::from("/tmp/multi.obj"));
        let target = tmp_path("multi_stlb");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::StlBin).expect("write should succeed");
        assert_eq!(
            warnings,
            vec![FlattenWarning::MultiObjectMerge { object_count: 2 }]
        );
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn export_multi_object_obj_does_not_emit_merge_warning() {
        // OBJ preserves per-object boundaries, so the flatten step
        // doesn't run and the warning vec is empty regardless of object
        // count.
        let doc = SceneDocument::new();
        doc.install_scene(two_object_scene(), PathBuf::from("/tmp/multi.obj"));
        let target = tmp_path("multi_obj");
        let warnings =
            export_scene_as(&doc, &target, WriteFormat::Obj).expect("write should succeed");
        assert!(warnings.is_empty());
        let _ = std::fs::remove_file(&target);
    }
}

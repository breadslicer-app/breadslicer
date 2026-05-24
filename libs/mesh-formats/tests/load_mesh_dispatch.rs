/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Integration tests for the top-level `load_mesh_from_*` dispatcher in
 * `sc_mesh_formats`. Complements the format-specific round-trip suites by
 * proving the dispatcher routes each `MeshFileType` to the right reader,
 * propagates warnings, errors on unsupported formats, and infers format
 * from the filesystem path.
 */

use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};

fn triangle_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([0.0, 0.0, 1.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 2],
        )],
    }
}

#[cfg(test)]
mod stl_dispatch_tests {
    use super::*;
    use std::io::Cursor;

    use sc_mesh_formats::filetypes::MeshFileType;
    use sc_mesh_formats::load_mesh_from_reader;
    use sc_mesh_formats::stl::{write_stla, write_stlb};

    #[test]
    fn stl_auto_dispatches_ascii_content() {
        let mut buf: Vec<u8> = Vec::new();
        write_stla(&triangle_mesh(), &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let loaded = load_mesh_from_reader(MeshFileType::Stl, &mut reader).unwrap();

        assert_eq!(loaded.graph.reachable_leaf_count(), 1);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn stl_auto_dispatches_binary_content() {
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&triangle_mesh(), &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let loaded = load_mesh_from_reader(MeshFileType::Stl, &mut reader).unwrap();

        assert_eq!(loaded.graph.reachable_leaf_count(), 1);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn stl_ascii_explicit_kind_works() {
        let mut buf: Vec<u8> = Vec::new();
        write_stla(&triangle_mesh(), &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let loaded = load_mesh_from_reader(MeshFileType::StlAscii, &mut reader).unwrap();
        assert_eq!(loaded.graph.reachable_leaf_count(), 1);
    }

    #[test]
    fn stl_binary_explicit_kind_works() {
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&triangle_mesh(), &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let loaded = load_mesh_from_reader(MeshFileType::StlBin, &mut reader).unwrap();
        assert_eq!(loaded.graph.reachable_leaf_count(), 1);
    }
}

#[cfg(test)]
mod obj_dispatch_tests {
    use std::io::Cursor;

    use sc_mesh_formats::filetypes::MeshFileType;
    use sc_mesh_formats::load_mesh_from_reader;
    use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning, SceneLoadWarning};

    #[test]
    fn obj_propagates_warnings() {
        // A `vt` line triggers `SkippedTextureCoordinates`; proves warnings
        // from the reader survive the dispatcher boundary and land in
        // `LoadedMesh.warnings`.
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0.1 0.2\nf 1 2 3\n";
        let mut reader = Cursor::new(&obj[..]);
        let loaded = load_mesh_from_reader(MeshFileType::Obj, &mut reader).unwrap();

        assert!(
            loaded.graph.reachable_leaf_count() > 0,
            "expected at least one leaf in the loaded graph"
        );
        let has_tex_warning = loaded.warnings.iter().any(|w| {
            matches!(
                w,
                LoadMeshWarning::Obj(LoadObjWarning::SkippedTextureCoordinates { .. })
            )
        });
        assert!(
            has_tex_warning,
            "expected SkippedTextureCoordinates warning, got {:?}",
            loaded.warnings
        );
    }

    #[test]
    fn dispatcher_surfaces_shared_vertices_at_scene_level() {
        // ObjectA and ObjectB both reference the OBJ global vertex 1.
        // `read_obj` no longer emits a per-object warning for that;
        // the dispatcher must run the scene-level detector and produce
        // a single `LoadMeshWarning::Scene(SharedVertices)` carrying
        // both leaves, named when their `o` line carried a name.
        let obj = b"\
o ObjectA
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
o ObjectB
v 1.0 1.0 0.0
f 1 3 4
";
        let mut reader = Cursor::new(&obj[..]);
        let loaded = load_mesh_from_reader(MeshFileType::Obj, &mut reader).unwrap();

        let scene_warning = loaded
            .warnings
            .iter()
            .find_map(|w| match w {
                LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices { leaves }) => Some(leaves),
                _ => None,
            })
            .expect("scene-level SharedVertices warning must be emitted");

        let names: Vec<Option<&str>> = scene_warning
            .iter()
            .map(|leaf| leaf.object_name.as_deref())
            .collect();
        assert!(names.contains(&Some("ObjectA")), "{names:?}");
        assert!(names.contains(&Some("ObjectB")), "{names:?}");
        assert_eq!(scene_warning.len(), 2, "{scene_warning:?}");
    }
}

#[cfg(test)]
mod unsupported_format_tests {
    use std::io::Cursor;

    use sc_mesh_formats::filetypes::MeshFileType;
    use sc_mesh_formats::load_mesh_from_reader;

    #[test]
    fn ply_kind_returns_error() {
        let mut reader = Cursor::new(b"irrelevant".to_vec());
        assert!(load_mesh_from_reader(MeshFileType::Ply, &mut reader).is_err());
    }

    #[test]
    fn amf_kind_returns_error() {
        let mut reader = Cursor::new(b"irrelevant".to_vec());
        assert!(load_mesh_from_reader(MeshFileType::Amf, &mut reader).is_err());
    }
}

#[cfg(test)]
mod path_loading_tests {
    use super::*;

    use sc_mesh_formats::filetypes::MeshFileType;
    use sc_mesh_formats::load_mesh_from_path;
    use sc_mesh_formats::stl::write_stla;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        // `CARGO_TARGET_TMPDIR` is set by Cargo at compile time for integration
        // tests; it's not a runtime environment variable.
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
    }

    #[test]
    fn load_from_path_infers_extension_and_dispatches() {
        let path = tmp_path("load_mesh_dispatch_smoke.stla");
        let mut file = std::fs::File::create(&path).unwrap();
        write_stla(&triangle_mesh(), &mut file).unwrap();
        drop(file);

        let loaded = load_mesh_from_path(&path, None, None).unwrap();
        assert_eq!(loaded.graph.reachable_leaf_count(), 1);
        assert!(loaded.warnings.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn explicit_kind_overrides_unknown_extension() {
        // Extension `.dat` is unknown; auto-infer fails.
        // With an explicit `MeshFileType::StlAscii` the dispatcher still loads.
        let path = tmp_path("load_mesh_dispatch_override.dat");
        let mut file = std::fs::File::create(&path).unwrap();
        write_stla(&triangle_mesh(), &mut file).unwrap();
        drop(file);

        assert!(load_mesh_from_path(&path, None, None).is_err());
        let loaded = load_mesh_from_path(&path, Some(MeshFileType::StlAscii), None).unwrap();
        assert_eq!(loaded.graph.reachable_leaf_count(), 1);

        let _ = std::fs::remove_file(&path);
    }

    fn expect_err(
        result: Result<sc_mesh_formats::LoadedMesh, sc_mesh_formats::error::LoadMeshError>,
    ) -> sc_mesh_formats::error::LoadMeshError {
        match result {
            Ok(_) => panic!("expected an error from load_mesh_from_path"),
            Err(e) => e,
        }
    }

    #[test]
    fn file_size_cap_rejects_oversized_input() {
        // Write a small STL file and ask for a 1-byte cap: the
        // metadata precheck must fire before the parser sees the
        // file. The error must surface both the actual size and the
        // cap so callers can format a user-facing message.
        let path = tmp_path("load_mesh_dispatch_oversize.stla");
        let mut file = std::fs::File::create(&path).unwrap();
        write_stla(&triangle_mesh(), &mut file).unwrap();
        drop(file);

        let err = expect_err(load_mesh_from_path(&path, None, Some(1)));
        let msg = err.to_string();
        assert!(msg.contains("exceeds"), "{msg}");
        assert!(msg.contains("1 byte limit"), "{msg}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_size_cap_accepts_within_limit() {
        // Same file, generous cap: parser runs and returns the mesh.
        let path = tmp_path("load_mesh_dispatch_within_cap.stla");
        let mut file = std::fs::File::create(&path).unwrap();
        write_stla(&triangle_mesh(), &mut file).unwrap();
        drop(file);

        let loaded = load_mesh_from_path(&path, None, Some(u64::MAX)).unwrap();
        assert_eq!(loaded.graph.reachable_leaf_count(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_falls_through_to_open_error() {
        // metadata() failure (missing file) is not surfaced by the
        // size gate; the subsequent File::open produces the more
        // specific I/O error. The same path with no cap behaves
        // identically.
        let path = tmp_path("load_mesh_dispatch_missing.stla");
        let _ = std::fs::remove_file(&path);

        let err_capped = expect_err(load_mesh_from_path(&path, None, Some(1024)));
        let err_uncapped = expect_err(load_mesh_from_path(&path, None, None));
        assert!(!err_capped.to_string().contains("byte limit"));
        assert_eq!(err_capped.to_string(), err_uncapped.to_string());
    }
}

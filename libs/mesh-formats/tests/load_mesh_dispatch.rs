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
        faces: vec![IndexedTriangle {
            normal: Normal::new([0.0, 0.0, 1.0]),
            vertices: [0, 1, 2],
        }],
        local_aabb_cache: ::std::sync::OnceLock::new(),
        clusters_cache: ::std::sync::OnceLock::new(),
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

        assert_eq!(loaded.scene.objects.len(), 1);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn stl_auto_dispatches_binary_content() {
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&triangle_mesh(), &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let loaded = load_mesh_from_reader(MeshFileType::Stl, &mut reader).unwrap();

        assert_eq!(loaded.scene.objects.len(), 1);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn stl_ascii_explicit_kind_works() {
        let mut buf: Vec<u8> = Vec::new();
        write_stla(&triangle_mesh(), &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let loaded = load_mesh_from_reader(MeshFileType::StlAscii, &mut reader).unwrap();
        assert_eq!(loaded.scene.objects.len(), 1);
    }

    #[test]
    fn stl_binary_explicit_kind_works() {
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&triangle_mesh(), &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let loaded = load_mesh_from_reader(MeshFileType::StlBin, &mut reader).unwrap();
        assert_eq!(loaded.scene.objects.len(), 1);
    }
}

#[cfg(test)]
mod obj_dispatch_tests {
    use std::io::Cursor;

    use sc_mesh_formats::filetypes::MeshFileType;
    use sc_mesh_formats::load_mesh_from_reader;
    use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};

    #[test]
    fn obj_propagates_warnings() {
        // A `vt` line triggers `SkippedTextureCoordinates`; proves warnings
        // from the reader survive the dispatcher boundary and land in
        // `LoadedMesh.warnings`.
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0.1 0.2\nf 1 2 3\n";
        let mut reader = Cursor::new(&obj[..]);
        let loaded = load_mesh_from_reader(MeshFileType::Obj, &mut reader).unwrap();

        assert!(
            !loaded.scene.objects.is_empty(),
            "expected at least one object in the loaded scene"
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

        let loaded = load_mesh_from_path(&path, None).unwrap();
        assert_eq!(loaded.scene.objects.len(), 1);
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

        assert!(load_mesh_from_path(&path, None).is_err());
        let loaded = load_mesh_from_path(&path, Some(MeshFileType::StlAscii)).unwrap();
        assert_eq!(loaded.scene.objects.len(), 1);

        let _ = std::fs::remove_file(&path);
    }
}

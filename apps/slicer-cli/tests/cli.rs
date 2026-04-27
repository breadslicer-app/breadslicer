/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * End-to-end tests for the `sc-slicer-cli` binary. These drive the
 * compiled CLI against synthetic and committed fixtures and assert on
 * exit code + stdout/stderr predicates. They cover the orchestration
 * layer (path validation, format dispatch, warning surfacing, target
 * file output) which the per-module unit tests cannot reach.
 */

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_mesh_formats::stl::{write_stla, write_stlb};
use tempfile::TempDir;

const BIN: &str = "sc-slicer-cli";

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at apps/slicer-cli/; two parents up is
    // the workspace root, where examples/ lives.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn examples_path(name: &str) -> PathBuf {
    workspace_root().join("examples").join(name)
}

/// Builds a closed, manifold tetrahedron with consistent winding and
/// computed normals. `diagnose` reports zero issues for this mesh, so
/// it works as a "clean input" fixture for the CLI tests. Same vertex
/// layout as the unit-test tetrahedron in `sc_mesh_core::indexed_mesh`.
fn tetrahedron_mesh(name: Option<&str>) -> IndexedMesh {
    let vertices = vec![
        Vertex::new([1.0, 1.0, 1.0]),
        Vertex::new([1.0, -1.0, -1.0]),
        Vertex::new([-1.0, 1.0, -1.0]),
        Vertex::new([-1.0, -1.0, 1.0]),
    ];
    let face_indices: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];
    let faces = face_indices
        .iter()
        .map(|indices| {
            let tri_verts = [
                vertices[indices[0] as usize],
                vertices[indices[1] as usize],
                vertices[indices[2] as usize],
            ];
            let normal = compute_normal_from_triangle_vertices(&tri_verts).unwrap();
            IndexedTriangle {
                normal,
                vertices: *indices,
            }
        })
        .collect();

    IndexedMesh {
        meta: name.map(|n| MeshMetadata {
            name: Some(n.to_string()),
            ..Default::default()
        }),
        vertices,
        faces,
    }
}

/// Writes a clean ASCII STL tetrahedron at `path`.
fn write_clean_stla(path: &Path) {
    let file = fs::File::create(path).expect("create stla");
    let mut writer = std::io::BufWriter::new(file);
    write_stla(&tetrahedron_mesh(None), &mut writer).expect("write_stla");
}

/// Writes a clean binary STL tetrahedron at `path`.
fn write_clean_stlb(path: &Path) {
    let file = fs::File::create(path).expect("create stlb");
    let mut writer = std::io::BufWriter::new(file);
    write_stlb(&tetrahedron_mesh(None), &mut writer).expect("write_stlb");
}

/// Writes a multi-object scene as ASCII STL by going through `Scene`
/// and the writer-side flatten step. Used to drive the
/// "MultiObjectMerge" warning when this scene is later read and
/// converted back to STL.
fn write_two_object_obj(path: &Path) {
    let a = SceneObject {
        meta: None,
        transform: Transform3D::Identity,
        content: SceneObjectContent::Mesh(tetrahedron_mesh(Some("A"))),
    };
    let b = SceneObject {
        meta: None,
        transform: Transform3D::Identity,
        content: SceneObjectContent::Mesh(tetrahedron_mesh(Some("B"))),
    };
    let scene = Scene {
        meta: None,
        resources: vec![],
        objects: vec![a, b],
    };
    let file = fs::File::create(path).expect("create obj");
    let mut writer = std::io::BufWriter::new(file);
    sc_mesh_formats::obj::write_obj(&scene, &mut writer).expect("write_obj");
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn version_subcommand_prints_a_version_line() {
        Command::cargo_bin(BIN)
            .unwrap()
            .arg("version")
            .assert()
            .success()
            .stdout(predicate::str::contains("Version:"));
    }
}

#[cfg(test)]
mod convert_tests {
    use super::*;

    #[test]
    fn obj_to_stla_round_trips_through_the_binary() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("out.stla");

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                examples_path("double_mesh.obj").to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .success();

        assert!(target.exists());
        let content = fs::read_to_string(&target).unwrap();
        assert!(
            content.starts_with("solid"),
            "ASCII STL must start with `solid`, got: {:?}",
            &content[..content.len().min(60)]
        );
    }

    #[test]
    fn convert_with_explicit_source_format_overrides_extension() {
        // Save an ASCII-STL payload under a `.dat` extension; without
        // --source-format the CLI cannot infer it.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("mystery.dat");
        let target = tmp.path().join("out.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "--source-format",
                "stla",
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .success();
        assert!(target.exists());
    }

    #[test]
    fn convert_unknown_source_extension_without_override_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("mystery.dat");
        let target = tmp.path().join("out.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .failure();
    }

    #[test]
    fn convert_same_format_short_circuits_to_copy() {
        // ASCII STL → ASCII STL just `fs::copy`s the source. The CLI
        // emits a warning on stderr and returns success.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        let target = tmp.path().join("out.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .success()
            .stderr(predicate::str::contains("copying file"));
        assert_eq!(
            fs::read(&source).unwrap(),
            fs::read(&target).unwrap(),
            "same-format conversion must produce a byte-identical copy"
        );
    }

    #[test]
    fn convert_target_equal_to_source_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "-t",
                source.to_str().unwrap(),
            ])
            .assert()
            .failure();
    }

    #[test]
    fn convert_multi_object_source_to_stl_warns_about_merge() {
        // Multi-object scenes can't be written as separate solids in
        // STL; the CLI surfaces a "Merging N objects" warning to stderr
        // and still produces the output file.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("two.obj");
        let target = tmp.path().join("out.stla");
        write_two_object_obj(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .success()
            .stderr(predicate::str::contains("Merging"));
        assert!(target.exists());
    }

    #[test]
    fn convert_source_does_not_exist_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("does_not_exist.stla");
        let target = tmp.path().join("out.stla");

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .failure();
    }

    #[test]
    fn convert_target_is_a_directory_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        write_clean_stla(&source);
        let target_dir = tmp.path().join("out_dir");
        fs::create_dir(&target_dir).unwrap();

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target_dir.to_str().unwrap(),
            ])
            .assert()
            .failure();
    }
}

#[cfg(test)]
mod diagnose_tests {
    use super::*;

    #[test]
    fn diagnose_clean_stla_returns_success() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .success();
    }

    #[test]
    fn diagnose_clean_stlb_returns_success() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stlb");
        write_clean_stlb(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .success();
    }

    #[test]
    fn diagnose_garbage_input_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("garbage.stl");
        fs::write(&source, b"This is not an STL file").unwrap();

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure();
    }

    #[test]
    fn diagnose_source_does_not_exist_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("missing.stl");

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure();
    }

    #[test]
    fn diagnose_with_explicit_source_format_works() {
        // Same payload, unknown extension; --source-format makes it
        // readable.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.dat");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "diagnose",
                "-s",
                source.to_str().unwrap(),
                "--source-format",
                "stla",
            ])
            .assert()
            .success();
    }
}

#[cfg(test)]
mod fix_tests {
    use super::*;

    #[test]
    fn fix_clean_input_reports_no_fixes_and_writes_target() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stla");
        let target = tmp.path().join("fixed.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("No fixes needed"));
        assert!(target.exists());
    }

    #[test]
    fn fix_target_equal_to_source_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                source.to_str().unwrap(),
            ])
            .assert()
            .failure();
    }

    #[test]
    fn fix_unknown_target_extension_errors() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stla");
        let target = tmp.path().join("fixed.dat");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .failure();
    }
}

#[cfg(test)]
mod inspect_and_slice_tests {
    use super::*;

    #[test]
    fn inspect_subcommand_panics_with_todo() {
        // `inspect` is not implemented yet — the binary calls `todo!()`
        // and exits non-zero. Pinning this so a future "no it returns
        // success now" change shows up as a deliberate test update.
        Command::cargo_bin(BIN)
            .unwrap()
            .arg("inspect")
            .assert()
            .failure();
    }

    #[test]
    fn slice_subcommand_panics_with_todo() {
        Command::cargo_bin(BIN)
            .unwrap()
            .arg("slice")
            .assert()
            .failure();
    }
}

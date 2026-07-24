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
use std::sync::Arc;

use assert_cmd::Command;
use predicates::prelude::*;
use sc_3mf::model::{
    BuildItem, MeshData, MeshResolution, ModelDocument, ObjectPayload, ObjectResource, ObjectType,
    ResourceId, ResourceIndex, TriangleRef, Vertex3, write_model,
};
use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{
    IndexedMesh, IndexedTriangle, LeafPayload, MeshMetadata, Normal, Scene, Vertex,
};
use sc_mesh_formats::stl::{write_stla, write_stlb};
use tempfile::TempDir;

const BIN: &str = "sc-slicer-cli";

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Directory of bundled test profiles, pointed at by the
/// `SC_SLICER_ASSETS_DIR` override so `slice` resolves the fake
/// fixture printer and material by id.
fn profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("profiles")
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
            IndexedTriangle::from_raw_indices(normal, *indices)
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

/// Writes a millimetre 3MF package holding the clean tetrahedron at
/// `path`, built through `sc-3mf` so the CLI test exercises the same
/// container layout the reader expects.
fn write_clean_3mf(path: &Path) {
    let mesh = tetrahedron_mesh(None);
    let document = ModelDocument {
        objects: vec![ObjectResource {
            id: ResourceId::new(1).expect("valid id"),
            object_type: ObjectType::Model,
            name: None,
            partnumber: None,
            property_ref: None,
            slice_stack: None,
            mesh_resolution: MeshResolution::default(),
            payload: ObjectPayload::Mesh(MeshData {
                vertices: mesh
                    .vertices
                    .iter()
                    .map(|v| Vertex3 {
                        x: f64::from(v[0]),
                        y: f64::from(v[1]),
                        z: f64::from(v[2]),
                    })
                    .collect(),
                triangles: mesh
                    .faces
                    .iter()
                    .map(|f| TriangleRef {
                        v1: ResourceIndex::new(f.vertices[0].get()).expect("small index"),
                        v2: ResourceIndex::new(f.vertices[1].get()).expect("small index"),
                        v3: ResourceIndex::new(f.vertices[2].get()).expect("small index"),
                    })
                    .collect(),
            }),
        }],
        build: vec![BuildItem {
            object_id: ResourceId::new(1).expect("valid id"),
            transform: None,
            partnumber: None,
        }],
        ..ModelDocument::default()
    };
    let mut model_xml = Vec::new();
    write_model(&document, &mut model_xml).expect("document serializes");
    let mut package = Package::new();
    let part = PartName::new("/3D/3dmodel.model").expect("valid part name");
    package
        .add_part(part, MODEL_PART_CONTENT_TYPE, model_xml)
        .expect("unique part name");
    package.add_root_relationship(Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: "/3D/3dmodel.model".to_string(),
    });
    let file = fs::File::create(path).expect("create 3mf");
    package
        .write_to(std::io::BufWriter::new(file))
        .expect("write 3mf package");
}

/// Tetrahedron with face 0's winding reversed. The other three faces
/// keep their original winding, so the result is internally
/// inconsistent: `validate_manifold` flags every edge of face 0 with
/// `InconsistentWindingOrder`. The stored normal is left as-is so
/// `validate_normal_vectors` flags `ExpectationMismatch` on the same
/// face. All vertex indices remain in range.
fn flipped_winding_tetrahedron_mesh() -> IndexedMesh {
    let mut mesh = tetrahedron_mesh(None);
    mesh.faces[0].vertices.reverse();
    mesh
}

/// Tetrahedron whose face 1 carries a non-unit-magnitude normal
/// (scaled by 2). `validate_normal_vectors` flags `NonUnitNorm`. All
/// vertex indices remain in range.
fn nonunit_normal_tetrahedron_mesh() -> IndexedMesh {
    let mut mesh = tetrahedron_mesh(None);
    let n = mesh.faces[1].normal.coords();
    mesh.faces[1].normal = Normal::new([n[0] * 2.0, n[1] * 2.0, n[2] * 2.0]);
    mesh
}

/// Tetrahedron with three extra collinear vertices appended and an
/// extra face built from them. The face is degenerate (its computed
/// normal is `Err(Degenerate triangle)`) but every face index in the
/// mesh is in range. This is the worst-case input the CLI's `fix`
/// pipeline can encounter through the bundled loaders.
fn tetrahedron_with_extra_collinear_face() -> IndexedMesh {
    let mut mesh = tetrahedron_mesh(None);
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex::new([0.0, 0.0, 0.0]));
    mesh.vertices.push(Vertex::new([1.0, 0.0, 0.0]));
    mesh.vertices.push(Vertex::new([2.0, 0.0, 0.0]));
    mesh.faces.push(IndexedTriangle::from_raw_indices(
        Normal::new([1.0, 0.0, 0.0]),
        [base, base + 1, base + 2],
    ));
    mesh
}

/// Writes the given mesh to `path` as ASCII STL.
fn write_stla_to(path: &Path, mesh: &IndexedMesh) {
    let file = fs::File::create(path).expect("create stla");
    let mut writer = std::io::BufWriter::new(file);
    write_stla(mesh, &mut writer).expect("write_stla");
}

/// A degenerate triangle whose first and third vertex are the same
/// point. STL stores explicit vertices, so the writer emits the same
/// (0, 0, 0) coordinate twice; on read, `as_indexed_mesh` dedups by
/// f32 bit pattern and the face collapses to indices `[0, 1, 0]`,
/// which `validate_faces` flags as a `ZeroLengthEdge`.
fn mesh_with_zero_length_edge() -> IndexedMesh {
    let repeat = Vertex::new([0.0, 0.0, 0.0]);
    IndexedMesh {
        meta: None,
        vertices: vec![repeat, Vertex::new([1.0, 0.0, 0.0]), repeat],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 2],
        )],
    }
}

/// A mesh with exactly one triangle. Every edge appears once and only
/// in one direction; `validate_manifold` flags all three as
/// `BoundaryEdge` because nothing closes them.
fn single_triangle_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 2],
        )],
    }
}

/// Three triangles all sharing the edge `(v0, v1)`. The half-edge
/// `(0, 1)` appears three times forward and zero times reverse, which
/// `validate_manifold` flags as `NonManifoldEdge`.
fn three_triangles_sharing_edge_mesh() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([0.0, 0.0, 0.0]),
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([0.5, 1.0, 0.0]),
        Vertex::new([0.5, -1.0, 0.0]),
        Vertex::new([0.5, 0.0, 1.0]),
    ];
    let faces = [[0, 1, 2], [0, 1, 3], [0, 1, 4]]
        .iter()
        .map(|idx| {
            let tri_verts = [
                vertices[idx[0] as usize],
                vertices[idx[1] as usize],
                vertices[idx[2] as usize],
            ];
            let normal = compute_normal_from_triangle_vertices(&tri_verts).unwrap();
            IndexedTriangle::from_raw_indices(normal, *idx)
        })
        .collect();
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// `assert_cmd` predicate that fails if stderr contains the textual
/// signature of a Rust panic. A Rust panic in a `#[panic = "unwind"]`
/// build prints `thread '…' panicked at …` and exits with code 101 by
/// default; in `#[panic = "abort"]` builds it still emits the same
/// header before aborting. Either way, the substring is reliable.
///
/// Use this in addition to `.success()` / `.failure()` whenever the
/// property we care about is "the binary did not crash": a Rust
/// panic propagating to the CLI is *always* a bug, regardless of what
/// the user typed.
fn no_panic_in_stderr() -> predicates::str::ContainsPredicate {
    // `predicate::str::contains("panicked at")` is what we'll negate
    // at the call site via `.not()`. Returning the bare predicate
    // keeps the call sites self-documenting.
    predicate::str::contains("panicked at")
}

/// Writes a single-object scene as an OBJ file. The OBJ reader wraps
/// object leaves under one identity-Group root, so reading this back
/// yields a leaf that is NOT itself a scene root: the shape the
/// `slice` leaf selection must handle.
fn write_single_object_obj(path: &Path) {
    let mut graph = Scene::new();
    let leaf = graph
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(tetrahedron_mesh(Some("A")))),
        )
        .expect("add leaf");
    graph.add_root(leaf).expect("add root");
    let file = fs::File::create(path).expect("create obj");
    let mut writer = std::io::BufWriter::new(file);
    sc_mesh_formats::obj::write_obj(&graph, &mut writer).expect("write_obj");
}

/// Writes a multi-object scene as an OBJ file. Used to drive the
/// "MultiObjectMerge" warning when this OBJ is later read back and
/// converted to STL.
fn write_two_object_obj(path: &Path) {
    let mut graph = Scene::new();
    for name in ["A", "B"] {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tetrahedron_mesh(Some(name)))),
            )
            .expect("add leaf");
        graph.add_root(leaf).expect("add root");
    }
    let file = fs::File::create(path).expect("create obj");
    let mut writer = std::io::BufWriter::new(file);
    sc_mesh_formats::obj::write_obj(&graph, &mut writer).expect("write_obj");
}

/// Writes an OBJ with a clean tetrahedron ("good") next to a single
/// open triangle ("broken-fin") that fails the manifold check with
/// three boundary edges. Drives the per-leaf validation reporting of
/// the slice subcommand.
fn write_obj_with_broken_object(path: &Path) {
    let mut graph = Scene::new();
    let mut broken = single_triangle_mesh();
    broken.meta = Some(MeshMetadata {
        name: Some("broken-fin".to_string()),
        ..Default::default()
    });
    for mesh in [tetrahedron_mesh(Some("good")), broken] {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        graph.add_root(leaf).expect("add root");
    }
    let file = fs::File::create(path).expect("create obj");
    let mut writer = std::io::BufWriter::new(file);
    sc_mesh_formats::obj::write_obj(&graph, &mut writer).expect("write_obj");
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
mod help_tests {
    use super::*;

    #[test]
    fn help_subcommand_prints_command_list() {
        // The `help` subcommand re-runs the parser with `-h` and prints
        // bpaf's auto-generated help text. Asserting on "Available
        // commands:" and one known subcommand name (`convert`) pins
        // both the section banner and the visibility of registered
        // subcommands.
        Command::cargo_bin(BIN)
            .unwrap()
            .arg("help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Available commands:"))
            .stdout(predicate::str::contains("convert"));
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
                fixture_path("double_mesh.obj").to_str().unwrap(),
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
    fn convert_same_format_parses_and_re_emits_through_the_writer() {
        // Same-format conversion no longer short-circuits to
        // `fs::copy`: it goes through the loader (so the source is
        // validated as a real mesh) and the atomic writer (so the
        // destination is never partially overwritten and never
        // followed through a symlink). The output is a freshly emitted
        // STLA, not a verbatim byte copy.
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
            .success();

        let content = fs::read_to_string(&target).unwrap();
        assert!(
            content.starts_with("solid"),
            "ASCII STL must start with `solid`, got: {:?}",
            &content[..content.len().min(60)]
        );
        // Same fixture geometry has 4 facets; the writer must emit one
        // `facet normal` line per triangle.
        assert_eq!(
            content.matches("facet normal").count(),
            4,
            "round-tripped tetrahedron must keep all 4 facets"
        );
    }

    #[test]
    fn convert_same_format_rejects_garbage_payload_under_known_extension() {
        // A `.stla` extension whose contents do not parse as ASCII STL
        // used to be copied verbatim by the same-format short-circuit,
        // producing a "successfully converted" file full of garbage.
        // The loader-routed path rejects it with a non-zero exit and
        // no target file is created.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        let target = tmp.path().join("out.stla");
        fs::write(&source, b"this is not an STL file at all").unwrap();

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
        assert!(
            !target.exists(),
            "target must not be created when the source fails to parse"
        );
    }

    #[test]
    fn convert_rejects_target_equal_to_source_via_relative_spelling() {
        // Pre-fix: source canonicalised to its absolute path, target
        // stayed as the raw `in.stla`, the equality check missed it,
        // and `fs::copy` truncated the source before reading a byte.
        // Post-fix: validate_targetfile_path canonicalises the parent
        // and re-joins the file name, ensure_distinct_files then sees
        // both paths in the same canonical form and refuses.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        write_clean_stla(&source);
        let original = fs::read(&source).unwrap();

        Command::cargo_bin(BIN)
            .unwrap()
            .current_dir(tmp.path())
            .args(["convert", "-s", source.to_str().unwrap(), "-t", "in.stla"])
            .assert()
            .failure();

        assert_eq!(
            fs::read(&source).unwrap(),
            original,
            "source file must not be touched by a rejected same-file convert"
        );
    }

    // Windows / NTFS is case-insensitive, so `Model.stla` and
    // `model.stla` name the same file. `canonicalize` preserves the
    // on-disk case, so ensure_distinct_files' `source == target` string
    // check does not see the alias (the two spellings differ in case);
    // the file-identity comparison is what catches it. On Unix these are
    // two different files and the convert would succeed, so this is a
    // Windows-only behaviour.
    #[cfg(windows)]
    #[test]
    fn convert_rejects_target_case_variant_of_source() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("Model.stla");
        write_clean_stla(&source);
        let original = fs::read(&source).unwrap();

        Command::cargo_bin(BIN)
            .unwrap()
            .current_dir(tmp.path())
            .args(["convert", "-s", "Model.stla", "-t", "model.stla"])
            .assert()
            .failure()
            // The identity-comparison branch, not the `source == target`
            // string check, must be what rejects: canonicalize preserves
            // the differing case so the strings are not equal.
            .stderr(predicate::str::contains("same underlying file"));

        assert_eq!(
            fs::read(&source).unwrap(),
            original,
            "source must be untouched by a rejected case-variant convert"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn convert_rejects_target_hardlinked_to_source() {
        // Two distinct names for the same inode (Unix) / NTFS file id
        // (Windows): path canonicalisation cannot see they are aliases,
        // but the file-identity comparison in ensure_distinct_files
        // does. The atomic-rename writer would not corrupt the source
        // even without this check, but silently unlinking one of the
        // user's hardlinks is bad UX.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        let target = tmp.path().join("link.stla");
        write_clean_stla(&source);
        std::fs::hard_link(&source, &target).unwrap();
        let original = fs::read(&source).unwrap();

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

        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn convert_target_with_missing_parent_directory_errors_at_validation() {
        // The parent directory has to exist (we need to canonicalise
        // it for the equality check and the writer needs to land the
        // temp file there). Surfacing the failure at validation time
        // gives a path-specific message instead of a mid-write I/O
        // error.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        write_clean_stla(&source);
        let target = tmp.path().join("does_not_exist").join("out.stla");

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

    // -----------------------------------------------------------------
    // Format-pair sweep. The CLI's effective conversion matrix is
    // 5 readable (Stl, StlAscii, StlBin, Obj, _3mf) by 4 writable
    // (StlAscii, StlBin, Obj, ThreeMf). The tests above cover
    // obj->stla (extension inference) and stla->stla (same-format
    // short circuit); the tests below add the remaining round-trip
    // pairs, 3MF as source and target, plus a single PLY-as-source
    // rejection test that also covers AMF since the two share one
    // dispatch arm in `load_mesh_from_reader`.
    // -----------------------------------------------------------------

    #[test]
    fn convert_stla_to_stlb_writes_fixed_size_binary() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        let target = tmp.path().join("out.stlb");
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
            .success();
        // 80-byte header + 4-byte u32 count + 4 * 50-byte triangle rec.
        assert_eq!(fs::metadata(&target).unwrap().len(), 80 + 4 + 4 * 50);
    }

    #[test]
    fn convert_stla_to_obj_writes_vertex_and_face_lines() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        let target = tmp.path().join("out.obj");
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
            .success();
        let content = fs::read_to_string(&target).unwrap();
        assert!(
            content.lines().any(|l| l.starts_with("v ")),
            "OBJ output missing vertex lines"
        );
        assert!(
            content.lines().any(|l| l.starts_with("f ")),
            "OBJ output missing face lines"
        );
    }

    #[test]
    fn convert_stlb_to_stla_writes_ascii_solid_header() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stlb");
        let target = tmp.path().join("out.stla");
        write_clean_stlb(&source);

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
            .success();
        let content = fs::read_to_string(&target).unwrap();
        assert!(
            content.starts_with("solid"),
            "ASCII STL must start with `solid`, got: {:?}",
            &content[..content.len().min(60)]
        );
    }

    #[test]
    fn convert_stlb_to_obj_writes_vertex_and_face_lines() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stlb");
        let target = tmp.path().join("out.obj");
        write_clean_stlb(&source);

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
            .success();
        let content = fs::read_to_string(&target).unwrap();
        assert!(
            content.lines().any(|l| l.starts_with("v ")),
            "OBJ output missing vertex lines"
        );
        assert!(
            content.lines().any(|l| l.starts_with("f ")),
            "OBJ output missing face lines"
        );
    }

    #[test]
    fn convert_obj_to_stlb_writes_well_formed_binary_payload() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("out.stlb");

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                fixture_path("double_mesh.obj").to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .success();
        // Binary STL is fixed-width: 80-byte header + 4-byte u32 count
        // + N * 50-byte triangle records. Asserting the size matches
        // that pattern (rather than a specific N) keeps the test
        // stable against changes in `examples/double_mesh.obj`.
        let size = fs::metadata(&target).unwrap().len();
        assert!(size > 84, "binary STL too short, got {size} bytes");
        assert_eq!(
            (size - 84) % 50,
            0,
            "binary STL payload past the 84-byte preamble must be a multiple of 50 bytes, got {size}"
        );
    }

    #[test]
    fn convert_3mf_to_obj_writes_the_tetrahedron() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.3mf");
        let target = tmp.path().join("out.obj");
        write_clean_3mf(&source);

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
            .success();
        let content = fs::read_to_string(&target).unwrap();
        assert_eq!(content.lines().filter(|l| l.starts_with("v ")).count(), 4);
        assert_eq!(content.lines().filter(|l| l.starts_with("f ")).count(), 4);
    }

    #[test]
    fn convert_stla_to_3mf_and_back_preserves_the_tetrahedron() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("in.stla");
        let packaged = tmp.path().join("mid.3mf");
        let target = tmp.path().join("out.obj");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                source.to_str().unwrap(),
                "-t",
                packaged.to_str().unwrap(),
            ])
            .assert()
            .success();
        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "convert",
                "-s",
                packaged.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .success();
        let content = fs::read_to_string(&target).unwrap();
        assert_eq!(content.lines().filter(|l| l.starts_with("v ")).count(), 4);
        assert_eq!(content.lines().filter(|l| l.starts_with("f ")).count(), 4);
    }

    #[test]
    fn convert_ply_source_errors_with_unsupported_message() {
        // PLY/AMF have no reader implementation; the loader rejects
        // them at dispatch time with "Unsupported mesh file type"
        // before parsing the file body.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("dummy.ply");
        let target = tmp.path().join("out.stla");
        fs::write(&source, b"ply\nformat ascii 1.0\n").unwrap();

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
            .failure()
            .stderr(predicate::str::contains("Unsupported mesh file type"));
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

    // -----------------------------------------------------------------
    // Per-category sweep over `diagnose`'s error variants. Each test
    // builds a minimal mesh that triggers one specific category and
    // asserts the category-specific stderr fragment is surfaced.
    //
    // 8 of the 11 mesh-validator error variants are reachable through
    // the CLI's STL/OBJ loaders. The three unreachable categories are
    // `MeshVertexError::InfiniteCoordinate`,
    // `MeshVertexError::NaNCoordinate`, and
    // `MeshFaceError::OutOfRangeVertexIndex`. The STL and OBJ parsers
    // reject non-finite f32 tokens and out-of-range face indices at
    // parse time, before any mesh ever reaches `validate_*`, so those
    // variants can only fire on meshes constructed directly in code
    // (covered by the unit tests in `sc-mesh-core`).
    // -----------------------------------------------------------------

    #[test]
    fn diagnose_reports_zero_length_edge() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("zle.stla");
        write_stla_to(&source, &mesh_with_zero_length_edge());

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains("zero-length edge"));
    }

    #[test]
    fn diagnose_reports_zero_area_triangle() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("zat.stla");
        write_stla_to(&source, &tetrahedron_with_extra_collinear_face());

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains("is degenerate: zero area"));
    }

    #[test]
    fn diagnose_reports_non_unit_norm() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("nun.stla");
        write_stla_to(&source, &nonunit_normal_tetrahedron_mesh());

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains("non-unit norm"));
    }

    #[test]
    fn diagnose_reports_expectation_mismatch() {
        // The flipped-winding fixture also triggers
        // `InconsistentWindingOrder`; this test only pins that the
        // expectation-mismatch fragment appears alongside it.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("em.stla");
        write_stla_to(&source, &flipped_winding_tetrahedron_mesh());

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "disagrees with the triangle plane",
            ));
    }

    #[test]
    fn diagnose_reports_empty_mesh() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("empty.stla");
        let empty = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        write_stla_to(&source, &empty);

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains("mesh has no faces"));
    }

    #[test]
    fn diagnose_reports_boundary_edge() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("boundary.stla");
        write_stla_to(&source, &single_triangle_mesh());

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains("is shared by only face"));
    }

    #[test]
    fn diagnose_reports_non_manifold_edge() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("nme.stla");
        write_stla_to(&source, &three_triangles_sharing_edge_mesh());

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains("is shared by 3 faces"));
    }

    #[test]
    fn diagnose_reports_inconsistent_winding_order() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("iwo.stla");
        write_stla_to(&source, &flipped_winding_tetrahedron_mesh());

        Command::cargo_bin(BIN)
            .unwrap()
            .args(["diagnose", "-s", source.to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains("windings disagree"));
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
            .stderr(predicate::str::contains("No fixes were applied"));
        assert!(target.exists());
    }

    #[test]
    fn fix_with_default_flags_repairs_flipped_winding() {
        // Default flags: zero-length-edges=true, flipped-faces=true,
        // face-normals=false. The flipped-winding tetrahedron triggers
        // the flipped-face fix path. The test pins that the default
        // configuration (no `--fix-*` flags on the command line) does
        // produce a fix, surfaces it in the "Applied fixes" report,
        // and writes the repaired mesh to the target.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("flipped.stla");
        let target = tmp.path().join("fixed.stla");
        write_stla_to(&source, &flipped_winding_tetrahedron_mesh());

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
            .stderr(predicate::str::contains("Applied fixes"))
            .stderr(predicate::str::contains("Flipped orientation"));
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
    fn fix_rejects_target_equal_to_source_via_relative_spelling() {
        // Same canonicalisation hole as `convert` had: source resolved
        // to its absolute path, target stayed raw, the equality check
        // missed it, and the writer would have rewritten the source.
        // The shared `ensure_distinct_files` helper now closes both.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stla");
        write_clean_stla(&source);
        let original = fs::read(&source).unwrap();

        Command::cargo_bin(BIN)
            .unwrap()
            .current_dir(tmp.path())
            .args(["fix", "-s", source.to_str().unwrap(), "-t", "clean.stla"])
            .assert()
            .failure();

        assert_eq!(fs::read(&source).unwrap(), original);
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

/// CLI-level no-panic contract for the `fix` subcommand.
///
/// `IndexedMesh::validate_normal_vectors` and
/// `IndexedMesh::fix_faces_orientation` both index into
/// `self.vertices` / `self.faces` without bounds checks, on the
/// assumption that an earlier `validate_faces` has guaranteed the
/// indices are in range. In `slicer-cli fix`, that earlier
/// `validate_faces` only runs when `--fix-zero-length-edges=true`
/// (the default). With `--fix-zero-length-edges=false` the gate is
/// bypassed: any mesh produced by the bundled loaders flows directly
/// into `validate_normal_vectors` and then into
/// `fix_faces_orientation`.
///
/// These are **contract tests**, not regression tests. The bundled
/// STL/OBJ loaders dedupe vertices densely, so no file-driven input
/// the CLI accepts today can carry out-of-range face indices, and
/// the unchecked-index assumption holds; the tests are expected to
/// pass against the unmodified library. They serve two purposes:
///
/// 1. Lock the property "the CLI does not panic on any
///    loader-acceptable mesh under any `--fix-*` flag combination",
///    end-to-end through the binary, for clean inputs and for inputs
///    that exercise each fix sub-path (winding flip, normal
///    recompute, degenerate face). A regression in any layer
///    (loader, validate, fix, writer) that lets a panic escape will
///    fail one of these tests.
///
/// 2. Future-proof the unchecked-index assumption: if a new file
///    format, a custom binary loader, or a library refactor ever
///    produces an `IndexedMesh` with out-of-range indices, these
///    tests will surface the resulting library-level panic as a
///    CI-visible CLI abort rather than a silent data crash.
///
/// Tests labelled "_does_not_panic_on_*" assert *only* the absence
/// of a Rust panic in stderr; the exit code (success vs. graceful
/// failure) is left unconstrained where the answer depends on
/// library-level decisions outside this binary's contract.
#[cfg(test)]
mod fix_no_panic_contract_tests {
    use super::*;

    #[test]
    fn fix_with_validate_faces_skipped_does_not_panic_on_clean_input() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stla");
        let target = tmp.path().join("out.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
                "--fix-zero-length-edges=false",
            ])
            .assert()
            .success()
            .stderr(no_panic_in_stderr().not());
    }

    #[test]
    fn fix_with_all_fix_passes_disabled_is_a_clean_no_op() {
        // All three `--fix-*` flags off: `fix_mesh` skips every
        // validate/fix sub-pass. The pipeline must still succeed
        // (the writer runs unconditionally) and must not panic.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("clean.stla");
        let target = tmp.path().join("out.stla");
        write_clean_stla(&source);

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
                "--fix-zero-length-edges=false",
                "--fix-flipped-faces=false",
                "--fix-face-normals=false",
            ])
            .assert()
            .success()
            .stderr(no_panic_in_stderr().not());
        assert!(target.exists());
    }

    #[test]
    fn fix_with_validate_faces_skipped_does_not_panic_on_winding_errors() {
        // Face 0 is wound backwards relative to its neighbours, so
        // `validate_manifold` returns `InconsistentWindingOrder`
        // errors and `validate_normal_vectors` returns an
        // `ExpectationMismatch` for face 0. With `--fix-zero-length-edges=false`,
        // `validate_faces` is skipped: the fix routine receives
        // these errors and exercises the flip + normal-recompute
        // branch end-to-end through the binary.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("flipped.stla");
        let target = tmp.path().join("out.stla");
        write_stla_to(&source, &flipped_winding_tetrahedron_mesh());

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
                "--fix-zero-length-edges=false",
                "--fix-flipped-faces=true",
                "--fix-face-normals=true",
            ])
            .assert()
            .success()
            .stderr(no_panic_in_stderr().not());
        assert!(target.exists());
    }

    #[test]
    fn fix_with_validate_faces_skipped_does_not_panic_on_nonunit_normal() {
        // Face 1's normal is stretched to magnitude 2.
        // `validate_normal_vectors` returns a `NonUnitNorm` error;
        // `fix_faces_orientation` puts the face in the second-pass
        // (normals-only) loop and recomputes the normal. With
        // `validate_faces` skipped, this drives the second-pass path
        // end-to-end through the binary.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("nonunit.stla");
        let target = tmp.path().join("out.stla");
        write_stla_to(&source, &nonunit_normal_tetrahedron_mesh());

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
                "--fix-zero-length-edges=false",
                "--fix-flipped-faces=false",
                "--fix-face-normals=true",
            ])
            .assert()
            .success()
            .stderr(no_panic_in_stderr().not());
        assert!(target.exists());
    }

    #[test]
    fn fix_with_validate_faces_skipped_does_not_panic_on_degenerate_face() {
        // The hardest input the bundled loaders can hand to the fix
        // routine: a mesh where one face is geometrically degenerate
        // (collinear vertices). `validate_normal_vectors` emits
        // `ZeroAreaTriangle`; if `fix_faces_orientation` later tries
        // to recompute that face's normal, the recompute fails. The
        // pipeline must surface that as a clean exit, not a panic.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("degenerate.stla");
        let target = tmp.path().join("out.stla");
        write_stla_to(&source, &tetrahedron_with_extra_collinear_face());

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
                "--fix-zero-length-edges=false",
                "--fix-flipped-faces=true",
                "--fix-face-normals=true",
            ])
            .assert()
            // Either a graceful non-zero exit (the fix routine
            // refuses to recompute the degenerate face's normal) or
            // a success (the routine chose to skip it). Both are
            // acceptable; only a Rust panic is a regression.
            .stderr(no_panic_in_stderr().not());
    }

    #[test]
    fn fix_with_validate_faces_skipped_does_not_panic_on_malformed_input() {
        // Loader rejects the input; the validate/fix pipeline is
        // never reached. Pin that skipping `--fix-zero-length-edges`
        // does not change the failure shape: the binary still exits
        // non-zero with no Rust panic in stderr.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("garbage.stl");
        let target = tmp.path().join("out.stla");
        fs::write(&source, b"This is not an STL file").unwrap();

        Command::cargo_bin(BIN)
            .unwrap()
            .args([
                "fix",
                "-s",
                source.to_str().unwrap(),
                "-t",
                target.to_str().unwrap(),
                "--fix-zero-length-edges=false",
            ])
            .assert()
            .failure()
            .stderr(no_panic_in_stderr().not());
    }
}

#[cfg(test)]
mod argument_parsing_tests {
    use super::*;

    #[test]
    fn convert_without_source_argument_fails_with_usage_hint() {
        // The `--source` argument is mandatory; bpaf reports the
        // missing argument with the expected-token name in stderr.
        Command::cargo_bin(BIN)
            .unwrap()
            .arg("convert")
            .assert()
            .failure()
            .stderr(predicate::str::contains("--source"));
    }

    #[test]
    fn convert_with_invalid_source_format_fails_with_type_parse_error() {
        // `--source-format` parses through `MeshFileType::FromStr`,
        // which returns `InvalidMeshFileType` for any unknown token.
        // bpaf surfaces the error via Display, so stderr carries the
        // "Invalid mesh file format type" string.
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
                "--source-format",
                "xyz",
                "-t",
                target.to_str().unwrap(),
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Invalid mesh file format type"));
    }
}

#[cfg(test)]
mod inspect_and_slice_tests {
    use super::*;

    #[test]
    fn inspect_subcommand_panics_with_todo() {
        // `inspect` is not implemented yet: the binary calls `todo!()`
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

#[cfg(test)]
mod slice_validation_error_tests {
    use super::*;

    /// Single triangle whose first and third vertices share a position
    /// (collapsed edge) at z = 1, with a third vertex at z = 2. The
    /// non-zero z range survives the CLI's auto-placement (which
    /// drops the mesh's lowest point to z = 0) and keeps z_max > 0,
    /// so the validation check fires instead of the "reposition above
    /// z = 0" check.
    fn mesh_with_zero_length_edge_above_z0() -> IndexedMesh {
        let repeat = Vertex::new([0.0, 0.0, 1.0]);
        IndexedMesh {
            meta: None,
            vertices: vec![repeat, Vertex::new([1.0, 0.0, 2.0]), repeat],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        }
    }

    fn run_slice(source: &Path, output: &Path, verbose: bool) -> assert_cmd::assert::Assert {
        let mut cmd = Command::cargo_bin(BIN).unwrap();
        cmd.args([
            "slice",
            "-s",
            source.to_str().unwrap(),
            "-t",
            output.to_str().unwrap(),
            "--target-format",
            "png-per-layer",
            "--printer-profile",
            "slicer-cli-e2e-test",
            "--material-profile",
            "slicer-cli-e2e-test-resin",
        ]);
        cmd.env("SC_SLICER_ASSETS_DIR", profiles_dir());
        if verbose {
            cmd.arg("--verbose");
        }
        cmd.assert()
    }

    #[test]
    fn slice_without_verbose_emits_summary_and_advertises_verbose() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("degenerate.stla");
        let output = tmp.path().join("layers");
        write_stla_to(&source, &mesh_with_zero_length_edge_above_z0());

        run_slice(&source, &output, false)
            .failure()
            .stderr(predicate::str::contains(
                "mesh face validation failed: 1 error(s)",
            ))
            .stderr(predicate::str::contains("1x ZeroLengthEdge"))
            .stderr(predicate::str::contains(
                "re-run with --verbose for the full error list",
            ))
            .stderr(predicate::str::contains("`sc-slicer-cli fix`"))
            .stderr(predicate::str::contains("ZeroLengthEdge (1):").not());
    }

    #[test]
    fn slice_with_verbose_emits_summary_and_grouped_sections_without_verbose_advertisement() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("degenerate.stla");
        let output = tmp.path().join("layers");
        write_stla_to(&source, &mesh_with_zero_length_edge_above_z0());

        run_slice(&source, &output, true)
            .failure()
            .stderr(predicate::str::contains(
                "mesh face validation failed: 1 error(s)",
            ))
            .stderr(predicate::str::contains("ZeroLengthEdge (1):"))
            .stderr(predicate::str::contains(
                "  - face #0 has a zero-length edge: slots v2 and v0 both bound to vertex #0",
            ))
            .stderr(predicate::str::contains(
                "Run `sc-slicer-cli fix` to repair the mesh.",
            ))
            .stderr(predicate::str::contains("--verbose for the full error list").not());
    }
}

#[cfg(test)]
mod slice_auto_place_tests {
    use super::*;

    /// `tetrahedron_mesh` has vertices at +/- 1 in each axis, so its
    /// z range is `[-1, +1]`. Default auto-placement drops the mesh
    /// floor to z = 0, doubling the sliced z range from `[0, +1]` to
    /// `[0, +2]` and the layer count from 5 to 10 at layer-height 0.2.
    fn write_centred_tetrahedron(path: &Path) {
        write_stla_to(path, &tetrahedron_mesh(None));
    }

    fn run_slice(
        source: &Path,
        output_dir: &Path,
        extra_args: &[&str],
    ) -> assert_cmd::assert::Assert {
        let mut cmd = Command::cargo_bin(BIN).unwrap();
        cmd.args([
            "slice",
            "-s",
            source.to_str().unwrap(),
            "-t",
            output_dir.to_str().unwrap(),
            "--target-format",
            "svg-per-layer",
            "--printer-profile",
            "slicer-cli-e2e-test",
            "--material-profile",
            "slicer-cli-e2e-test-resin",
        ]);
        cmd.env("SC_SLICER_ASSETS_DIR", profiles_dir());
        cmd.args(extra_args);
        cmd.assert()
    }

    #[test]
    fn slice_default_auto_places_mesh_floor_at_z0() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("tet.stla");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_centred_tetrahedron(&source);

        run_slice(&source, &output, &[])
            .success()
            .stdout(predicate::str::contains("Sliced 1 model: 10 layers"));
    }

    #[test]
    fn slice_with_no_auto_place_leaves_mesh_at_file_coords() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("tet.stla");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_centred_tetrahedron(&source);

        run_slice(&source, &output, &["--no-auto-place"])
            .success()
            .stdout(predicate::str::contains("Sliced 1 model: 5 layers"));
    }
}

#[cfg(test)]
mod slice_repair_tests {
    use super::*;

    fn run_slice(
        source: &Path,
        output_dir: &Path,
        extra_args: &[&str],
    ) -> assert_cmd::assert::Assert {
        let mut cmd = Command::cargo_bin(BIN).unwrap();
        cmd.args([
            "slice",
            "-s",
            source.to_str().unwrap(),
            "-t",
            output_dir.to_str().unwrap(),
            "--target-format",
            "svg-per-layer",
            "--printer-profile",
            "slicer-cli-e2e-test",
            "--material-profile",
            "slicer-cli-e2e-test-resin",
        ]);
        cmd.env("SC_SLICER_ASSETS_DIR", profiles_dir());
        cmd.args(extra_args);
        cmd.assert()
    }

    /// Tetrahedron with one face's winding reversed: triggers
    /// `InconsistentWindingOrder` (manifold check) and
    /// `ExpectationMismatch` (normal-vector check). The `fix`
    /// subcommand's default flags (`flipped-faces=true`) repair this;
    /// `--repair` on slice should do the same.
    fn write_flipped_winding_tetrahedron(path: &Path) {
        write_stla_to(path, &flipped_winding_tetrahedron_mesh());
    }

    #[test]
    fn slice_without_repair_on_flipped_winding_fixture_fails_validation() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("flipped.stla");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_flipped_winding_tetrahedron(&source);

        run_slice(&source, &output, &[])
            .failure()
            .stderr(predicate::str::contains("InconsistentWindingOrder"));
    }

    #[test]
    fn slice_with_repair_on_flipped_winding_fixture_repairs_and_slices() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("flipped.stla");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_flipped_winding_tetrahedron(&source);

        run_slice(&source, &output, &["--repair"])
            .success()
            .stderr(predicate::str::contains("Applied fixes"))
            .stderr(predicate::str::contains("Flipped orientation"));
    }
}

#[cfg(test)]
mod slice_leaf_selection_tests {
    use super::*;

    fn run_slice(source: &Path, output_dir: &Path) -> assert_cmd::assert::Assert {
        let mut cmd = Command::cargo_bin(BIN).unwrap();
        cmd.args([
            "slice",
            "-s",
            source.to_str().unwrap(),
            "-t",
            output_dir.to_str().unwrap(),
            "--target-format",
            "svg-per-layer",
            "--printer-profile",
            "slicer-cli-e2e-test",
            "--material-profile",
            "slicer-cli-e2e-test-resin",
        ]);
        cmd.env("SC_SLICER_ASSETS_DIR", profiles_dir());
        cmd.assert()
    }

    /// The OBJ reader nests the object leaf under a Group root, so
    /// leaf selection must reach through groups. Same geometry and
    /// auto-placement as `slice_default_auto_places_mesh_floor_at_z0`,
    /// hence the same 10-layer expectation.
    #[test]
    fn slice_single_object_obj_succeeds() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("single.obj");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_single_object_obj(&source);

        run_slice(&source, &output)
            .success()
            .stdout(predicate::str::contains("Sliced 1 model: 10 layers"));
    }

    /// Both tetrahedra share the same footprint, so each of the 10
    /// layers carries one polygon per model: 20 polygons total.
    #[test]
    fn slice_multi_object_obj_slices_every_object() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("two.obj");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_two_object_obj(&source);

        run_slice(&source, &output)
            .success()
            .stdout(predicate::str::contains(
                "Sliced 2 models: 10 layers (20 polygons total)",
            ))
            .stderr(no_panic_in_stderr().not());
        let svg_count = fs::read_dir(&output).unwrap().count();
        assert_eq!(svg_count, 10, "one SVG file per layer");
    }
}

#[cfg(test)]
mod slice_multi_mesh_tests {
    use super::*;

    fn run_slice(
        source: &Path,
        output_dir: &Path,
        format: &str,
        extra_args: &[&str],
    ) -> assert_cmd::assert::Assert {
        let mut cmd = Command::cargo_bin(BIN).unwrap();
        cmd.args([
            "slice",
            "-s",
            source.to_str().unwrap(),
            "-t",
            output_dir.to_str().unwrap(),
            "--target-format",
            format,
            "--printer-profile",
            "slicer-cli-e2e-test",
            "--material-profile",
            "slicer-cli-e2e-test-resin",
        ]);
        cmd.env("SC_SLICER_ASSETS_DIR", profiles_dir());
        cmd.args(extra_args);
        cmd.assert()
    }

    #[test]
    fn multi_object_obj_slices_to_png_layers() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("two.obj");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_two_object_obj(&source);

        run_slice(&source, &output, "png-per-layer", &[])
            .success()
            .stdout(predicate::str::contains("Sliced 2 models: 10 layers"))
            .stderr(no_panic_in_stderr().not());
        assert_eq!(
            fs::read_dir(&output).unwrap().count(),
            10,
            "one PNG file per layer",
        );
    }

    /// Without auto-placement the tetrahedra keep their file
    /// coordinates (`z in [-1, 1]`), so only the upper half is above
    /// the plate: 5 layers, still 2 polygons per layer.
    #[test]
    fn no_auto_place_keeps_the_multi_model_layout() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("two.obj");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_two_object_obj(&source);

        run_slice(&source, &output, "svg-per-layer", &["--no-auto-place"])
            .success()
            .stdout(predicate::str::contains(
                "Sliced 2 models: 5 layers (10 polygons total)",
            ));
    }

    #[test]
    fn validation_failure_names_the_broken_leaf_only() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("broken.obj");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_obj_with_broken_object(&source);

        run_slice(&source, &output, "svg-per-layer", &[])
            .failure()
            .stderr(predicate::str::contains(
                "mesh validation failed for 1 of the scene's leaves:",
            ))
            .stderr(predicate::str::contains("(\"broken-fin\")"))
            .stderr(predicate::str::contains("mesh manifold validation failed"))
            .stderr(predicate::str::contains("BoundaryEdge"))
            .stderr(predicate::str::contains("(\"good\")").not())
            .stderr(predicate::str::contains(
                "re-run with --verbose for the full error list",
            ))
            .stderr(no_panic_in_stderr().not());
    }

    #[test]
    fn verbose_validation_failure_lists_every_boundary_edge() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("broken.obj");
        let output = tmp.path().join("layers");
        fs::create_dir(&output).unwrap();
        write_obj_with_broken_object(&source);

        run_slice(&source, &output, "svg-per-layer", &["--verbose"])
            .failure()
            .stderr(predicate::str::contains("BoundaryEdge (3):"))
            .stderr(predicate::str::contains("(\"broken-fin\"):\nBoundaryEdge"))
            .stderr(predicate::str::contains("--verbose for the full error list").not());
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::messages::{LoadMeshWarning, SceneLoadWarning};
use sc_mesh_formats::scene_writer::write_scene;
use sc_print_primitives::{MaterialId, PrinterId};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, NodeKind};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;

use crate::loader::{
    LoadProgress, LoadSceneError, LoadedScene, SUPPORTED_SCENE_FILE_EXTENSIONS,
    is_supported_scene_file_extension, load_scene,
};
use crate::payload::{EmbedProfiles, ProjectPayload};
use crate::slices::PlateSlices;
use crate::write::write_project;

/// Cube plus the 2 tunnel prisms as 3 translated leaf roots.
fn prism_trio_scene() -> Scene {
    let mut scene = Scene::new();
    let sources = [
        (unit_cube_mesh(), [0.0, 0.0, 0.0]),
        (cuboid_with_z_tunnel(), [5.0, 0.0, 0.0]),
        (cuboid_with_y_tunnel(), [-5.0, 0.0, 0.0]),
    ];
    for (mesh, translation) in sources {
        let transform = if translation == [0.0, 0.0, 0.0] {
            Transform3D::Identity
        } else {
            Transform3D::Affine(AffineTransform3D::from_translation(translation))
        };
        let leaf = scene
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
    }
    scene
}

/// 2 leaves referencing one registered mesh resource, so the
/// shared-vertex check has something to flag.
fn shared_resource_scene() -> Scene {
    let mut scene = Scene::new();
    let resource = scene.add_mesh_resource_arc(Arc::new(unit_cube_mesh()));
    for _ in 0..2 {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::MeshResource(resource),
            )
            .expect("resource leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
    }
    scene
}

fn payload_with_ids() -> ProjectPayload {
    ProjectPayload {
        printer: Some(PrinterId::new("test-printer").expect("valid slug")),
        resin: Some(MaterialId::new("test-resin").expect("valid slug")),
        first_layer_height: None,
        embedded: Vec::new(),
    }
}

/// A CCW triangle as one solid layer plus an empty layer, paired
/// with the top Z of each layer.
fn plate_slices() -> PlateSlices {
    let outer = ClosedRing::new(
        [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)]
            .into_iter()
            .map(|(x, y)| Point2D::new(x, y))
            .collect(),
    );
    let triangle = Polygon::new(outer, vec![]).expect("the outer ring is CCW");
    let solid = ContourLayer::new(0.025, vec![triangle]);
    let void = ContourLayer::new(0.075, vec![]);
    let contours = ContourStack::new(vec![solid, void]).expect("z is strictly increasing");
    PlateSlices {
        contours,
        slab_tops: vec![0.05, 0.10],
    }
}

fn write_mesh_fixture(dir: &Path, name: &str, format: WriteFormat) -> PathBuf {
    let path = dir.join(name);
    write_scene(&prism_trio_scene(), &path, format).expect("mesh fixture writes");
    path
}

fn write_project_fixture(dir: &Path, name: &str, scene: &Scene) -> PathBuf {
    let path = dir.join(name);
    let mut bytes = Vec::new();
    write_project(
        scene,
        &payload_with_ids(),
        EmbedProfiles::No,
        Some(&plate_slices()),
        Some("fingerprint-token"),
        crate::filetypes::ProjectFileType::ThreeMf,
        &mut bytes,
    )
    .expect("project fixture writes");
    std::fs::write(&path, bytes).expect("fixture file writes");
    path
}

fn open(path: &Path) -> Result<LoadedScene, LoadSceneError> {
    load_scene(path, None, |_| {})
}

/// Children of the single group root the readers rebuild loaded
/// content under.
fn root_child_count(scene: &Scene) -> usize {
    let root = scene.node(scene.roots()[0]).expect("root resolves");
    match &root.kind {
        NodeKind::Group { children } => children.len(),
        other => panic!("the root should be a group, got {other:?}"),
    }
}

#[test]
fn an_stl_file_opens_with_empty_project_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_mesh_fixture(dir.path(), "trio.stl", WriteFormat::StlBin);
    let opened = open(&path).expect("the stl opens");
    assert!(
        opened.scene.aabb().expect("aabb resolves").is_some(),
        "the scene carries geometry"
    );
    assert!(opened.payload.is_none());
    assert!(opened.slices.is_none());
    assert!(opened.slice_fingerprint.is_none());
    assert!(opened.project_warnings.is_empty());
}

#[test]
fn an_obj_file_opens_with_three_roots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_mesh_fixture(dir.path(), "trio.obj", WriteFormat::Obj);
    let opened = open(&path).expect("the obj opens");
    assert_eq!(root_child_count(&opened.scene), 3);
    assert!(opened.payload.is_none());
    assert!(opened.project_warnings.is_empty());
}

#[test]
fn a_plain_3mf_opens_with_no_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_mesh_fixture(dir.path(), "trio.3mf", WriteFormat::ThreeMf);
    let opened = open(&path).expect("the plain 3mf opens");
    assert!(
        opened.scene.aabb().expect("aabb resolves").is_some(),
        "the scene carries geometry"
    );
    assert!(opened.payload.is_none());
    assert!(opened.slices.is_none());
    assert!(opened.slice_fingerprint.is_none());
}

#[test]
fn a_project_3mf_carries_payload_slices_and_fingerprint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_project_fixture(dir.path(), "project.3mf", &prism_trio_scene());
    let opened = open(&path).expect("the project opens");
    assert_eq!(root_child_count(&opened.scene), 3);
    let payload = opened.payload.expect("the payload is carried");
    assert_eq!(
        payload.printer,
        Some(PrinterId::new("test-printer").expect("valid slug"))
    );
    let slices = opened.slices.expect("the slices are carried");
    assert_eq!(slices.layers().len(), 2);
    assert_eq!(
        opened.slice_fingerprint.as_deref(),
        Some("fingerprint-token")
    );
}

#[test]
fn an_unknown_extension_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("scene.xyz");
    std::fs::write(&path, b"not a mesh").expect("file writes");
    let err = open(&path).expect_err("nothing dispatches on .xyz");
    assert!(matches!(err, LoadSceneError::UnsupportedExtension { .. }));
}

#[test]
fn an_extension_outside_the_supported_list_is_refused_before_io() {
    // ply and amf infer a mesh type in sc-mesh-formats but have no
    // reader; the dispatch refuses them up front, on the same list
    // the file dialogs advertise. The paths do not exist: reaching
    // I/O would change the error.
    for name in ["scene.ply", "scene.amf"] {
        let err = open(Path::new(name)).expect_err("outside the list");
        assert!(matches!(err, LoadSceneError::UnsupportedExtension { .. }));
    }
}

#[test]
fn the_supported_list_matches_the_dispatch() {
    for ext in SUPPORTED_SCENE_FILE_EXTENSIONS {
        let path = PathBuf::from(format!("scene.{ext}"));
        assert!(
            is_supported_scene_file_extension(&path),
            "{ext} is supported"
        );
        // A supported extension dispatches past the extension gate;
        // the missing file then fails on I/O, never on the gate.
        let err = open(&path).expect_err("the file does not exist");
        assert!(
            !matches!(err, LoadSceneError::UnsupportedExtension { .. }),
            "{ext} must dispatch",
        );
    }
    assert!(!is_supported_scene_file_extension(Path::new("scene.ply")));
    assert!(
        is_supported_scene_file_extension(Path::new("SCENE.STL")),
        "case-insensitive"
    );
}

#[test]
fn an_over_cap_mesh_file_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_mesh_fixture(dir.path(), "trio.stl", WriteFormat::StlBin);
    let size = std::fs::metadata(&path).expect("metadata").len();
    let err = load_scene(&path, Some(size - 1), |_| {}).expect_err("the cap refuses the file");
    assert!(matches!(
        err,
        LoadSceneError::FileTooLarge {
            actual_bytes,
            limit_bytes,
        } if actual_bytes == size && limit_bytes == size - 1
    ));
}

#[test]
fn an_over_cap_project_file_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_project_fixture(dir.path(), "project.3mf", &prism_trio_scene());
    let size = std::fs::metadata(&path).expect("metadata").len();
    let err = load_scene(&path, Some(size - 1), |_| {}).expect_err("the cap refuses the file");
    assert!(matches!(err, LoadSceneError::FileTooLarge { .. }));
}

#[test]
fn a_file_exactly_at_the_cap_opens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_mesh_fixture(dir.path(), "trio.stl", WriteFormat::StlBin);
    let size = std::fs::metadata(&path).expect("metadata").len();
    load_scene(&path, Some(size), |_| {}).expect("at the cap the file opens");
}

#[test]
fn shared_vertices_warn_on_the_mesh_route() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shared.obj");
    write_scene(&shared_resource_scene(), &path, WriteFormat::Obj).expect("obj writes");
    let opened = open(&path).expect("the obj opens");
    assert!(
        opened.mesh_warnings.iter().any(|w| matches!(
            w,
            LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices { .. })
        )),
        "warnings: {:?}",
        opened.mesh_warnings,
    );
}

#[test]
fn shared_vertices_warn_on_the_project_route() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_project_fixture(dir.path(), "shared.3mf", &shared_resource_scene());
    let opened = open(&path).expect("the project opens");
    assert!(
        opened.mesh_warnings.iter().any(|w| matches!(
            w,
            LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices { .. })
        )),
        "warnings: {:?}",
        opened.mesh_warnings,
    );
}

fn last_tick(path: &Path) -> LoadProgress {
    let mut ticks: Vec<LoadProgress> = Vec::new();
    load_scene(path, None, |p| ticks.push(p)).expect("the file opens");
    *ticks.last().expect("at least one tick fires")
}

#[test]
fn the_last_progress_tick_is_complete_on_the_mesh_route() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_mesh_fixture(dir.path(), "trio.stl", WriteFormat::StlBin);
    let tick = last_tick(&path);
    assert!(tick.total_bytes > 0, "the file has bytes");
    assert_eq!(tick.done_bytes, tick.total_bytes);
}

#[test]
fn the_last_progress_tick_is_complete_on_the_project_route() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_project_fixture(dir.path(), "project.3mf", &prism_trio_scene());
    let tick = last_tick(&path);
    assert_eq!(tick.done_bytes, tick.total_bytes);
}

#[test]
fn a_broken_file_with_a_mesh_extension_is_a_mesh_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("broken.obj");
    std::fs::write(&path, b"\x00\x01 not an obj").expect("file writes");
    let err = open(&path).expect_err("garbage does not parse");
    assert!(matches!(err, LoadSceneError::Mesh(_)));
}

#[test]
fn a_broken_file_with_a_project_extension_is_a_project_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("broken.3mf");
    std::fs::write(&path, b"not a zip archive").expect("file writes");
    let err = open(&path).expect_err("garbage does not parse");
    assert!(matches!(err, LoadSceneError::Project(_)));
}

#[test]
fn a_missing_file_is_an_io_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("absent.stl");
    let err = open(&path).expect_err("the file does not exist");
    assert!(matches!(err, LoadSceneError::Io(_)));
}

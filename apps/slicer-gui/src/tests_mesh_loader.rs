/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Behavioural tests for the wgpu-free helpers in
//! `crate::mesh_loader`. The async worker spawned by `spawn_load`
//! requires a real `wgpu::Device` and stays under the software-wgpu
//! integration vehicle (Phase 1.F).

use std::any::Any;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::{LeafPayload, NodeKind};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, MeshMetadata};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::error::LoadMeshError;
use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::scene_writer::write_scene;
use sc_project_formats::{
    EmbedProfiles, ProjectPayload, infer_project_file_type_from_extension, write_project,
};

use crate::build_volume::BuildVolume;
use crate::error_dialog::DialogManager;
use crate::graphics_status::GraphicsUnavailable;
use crate::load_progress::{LoadPhase, LoadProgressSink, PROGRESS_PERMILLE_MAX, decode};
use crate::mesh_loader::{
    LoadDispatch, MAX_MESH_FILE_SIZE_BYTES, check_file_size, decide_load_dispatch,
    ensure_measured_size_within, guard_worker, panic_message, parse_source, place_on_plate,
    start_load_with_ticket,
};
use crate::scene_loader::SceneLoader;

fn unit_triangle_mesh() -> IndexedMesh {
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

fn scene_with_one_triangle_at_origin() -> Scene {
    let mut s = Scene::new();
    let leaf = s
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(unit_triangle_mesh().into()),
        )
        .expect("add_leaf");
    s.add_root(leaf).expect("add_root");
    s
}

fn loader() -> SceneLoader {
    SceneLoader::new_for_test(DialogManager::new())
}

// --- check_file_size --------------------------------------------------

#[test]
fn check_file_size_returns_ok_for_a_file_under_the_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("small.stl");
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(b"hello").expect("write");
    drop(f);
    assert!(check_file_size(&path).is_ok());
}

#[test]
fn check_file_size_returns_err_for_a_file_over_the_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.stl");
    let f = std::fs::File::create(&path).expect("create");
    // Sparse file: set the length without allocating; metadata().len()
    // still reports the logical size.
    f.set_len(MAX_MESH_FILE_SIZE_BYTES + 1).expect("set_len");
    drop(f);
    let result = check_file_size(&path);
    let err = result.expect_err("over-cap file must fail the check");
    let msg = err.to_string();
    assert!(msg.contains("exceeds"), "msg = {msg}");
    assert!(msg.contains("limit"), "msg = {msg}");
}

#[test]
fn check_file_size_returns_ok_for_a_missing_path_so_the_worker_can_report_the_real_error() {
    let path = PathBuf::from("/this/path/does/not/exist/scene.stl");
    assert!(check_file_size(&path).is_ok());
}

// --- place_on_plate ---------------------------------------------------

#[test]
fn place_on_plate_is_a_no_op_on_an_empty_scene() {
    let mut scene = Scene::new();
    let bv = BuildVolume::new(100.0, 60.0, 80.0);
    place_on_plate(&mut scene, bv).expect("place ok");
    assert!(scene.roots().is_empty());
}

#[test]
fn place_on_plate_translates_a_scene_with_geometry() {
    let mut scene = scene_with_one_triangle_at_origin();
    let bv = BuildVolume::new(100.0, 60.0, 80.0);
    let root_id_before = *scene.roots().first().expect("one root");
    let xform_before = scene
        .node(root_id_before)
        .expect("root present")
        .local_transform;
    assert_eq!(xform_before, Transform3D::Identity);
    place_on_plate(&mut scene, bv).expect("place ok");
    let xform_after = scene
        .node(root_id_before)
        .expect("root still present")
        .local_transform;
    assert_ne!(
        xform_after,
        Transform3D::Identity,
        "root must carry an outer translation"
    );
}

// --- start_load_with_ticket ------------------------------------------

#[test]
fn start_load_with_ticket_deposits_an_err_for_an_oversize_file() {
    let l = loader();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.stl");
    let f = std::fs::File::create(&path).expect("create");
    f.set_len(MAX_MESH_FILE_SIZE_BYTES + 1).expect("set_len");
    drop(f);
    let ticket = l.begin_load().expect("begin");
    start_load_with_ticket(
        ticket,
        path.clone(),
        &l,
        BuildVolume::new(100.0, 60.0, 80.0),
    );
    let drained = l
        .drain_deposit_for_test()
        .expect("size-cap bailout must deposit a result");
    let (drained_path, result) = drained;
    assert_eq!(drained_path, path);
    let Err(err) = result else {
        panic!("oversize must deposit an Err");
    };
    assert!(err.to_string().contains("exceeds"), "{err}");
}

#[test]
fn start_load_with_ticket_deposits_an_err_when_no_gpu_snapshot_is_available() {
    let l = loader();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("small.stl");
    std::fs::File::create(&path).expect("create");
    // gpu_snapshot is `None` because no `set_gpu` has been called.
    let ticket = l.begin_load().expect("begin");
    start_load_with_ticket(
        ticket,
        path.clone(),
        &l,
        BuildVolume::new(100.0, 60.0, 80.0),
    );
    let drained = l
        .drain_deposit_for_test()
        .expect("no-gpu bailout must deposit a result");
    let (drained_path, result) = drained;
    assert_eq!(drained_path, path);
    let Err(err) = result else {
        panic!("no-gpu must deposit an Err");
    };
    // No setup has run, so the device may still be on its way and the
    // dialog has to say so rather than name an internal condition.
    assert_eq!(
        err.to_string(),
        GraphicsUnavailable::AwaitingSetup.message(),
        "the bailout must report why there is no device"
    );
}

#[test]
fn start_load_with_ticket_reports_an_unusable_renderer_when_one_was_seen() {
    // Same empty device slot as above, but here a setup already went
    // past carrying an API this build cannot draw with. Waiting will
    // not help, so the user must be told something different.
    let l = loader();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("small.stl");
    std::fs::File::create(&path).expect("create");
    l.note_unsupported_graphics_api("GraphicsAPI::NativeOpenGL".to_string());
    let ticket = l.begin_load().expect("begin");
    start_load_with_ticket(
        ticket,
        path.clone(),
        &l,
        BuildVolume::new(100.0, 60.0, 80.0),
    );
    let (drained_path, result) = l
        .drain_deposit_for_test()
        .expect("no-gpu bailout must deposit a result");
    assert_eq!(drained_path, path);
    let Err(err) = result else {
        panic!("no-gpu must deposit an Err");
    };
    assert_eq!(
        err.to_string(),
        GraphicsUnavailable::UnsupportedApi("GraphicsAPI::NativeOpenGL".to_string()).message()
    );
    assert!(
        err.to_string().contains("GraphicsAPI::NativeOpenGL"),
        "the renderer in use must reach the dialog: {err}"
    );
}

// --- decide_load_dispatch --------------------------------------------

#[test]
fn a_ready_device_starts_the_load() {
    assert_eq!(decide_load_dispatch(None), LoadDispatch::Start);
}

#[test]
fn a_pick_made_before_the_first_paint_is_parked() {
    // The window is cold rather than broken, so the request waits for
    // the device instead of failing in the user's face.
    assert_eq!(
        decide_load_dispatch(Some(GraphicsUnavailable::AwaitingSetup)),
        LoadDispatch::Queue
    );
}

#[test]
fn a_pick_on_an_unusable_renderer_is_reported_not_parked() {
    // Parking this one would swallow the request for the rest of the
    // session, since no later setup is going to arrive.
    let reason = GraphicsUnavailable::UnsupportedApi("GraphicsAPI::NativeOpenGL".to_string());
    assert_eq!(
        decide_load_dispatch(Some(reason.clone())),
        LoadDispatch::Report(reason)
    );
}

#[test]
fn a_pick_on_a_refused_renderer_is_reported_not_parked() {
    let reason = GraphicsUnavailable::RendererRefused("mesh pipeline refused".to_string());
    assert_eq!(
        decide_load_dispatch(Some(reason.clone())),
        LoadDispatch::Report(reason)
    );
}

#[test]
fn the_reported_reason_reaches_the_caller_intact() {
    // The caller turns this into the dialog body, so the API name has
    // to survive the decision rather than be flattened to a generic
    // failure.
    let LoadDispatch::Report(reason) = decide_load_dispatch(Some(
        GraphicsUnavailable::UnsupportedApi("GraphicsAPI::WebGL".to_string()),
    )) else {
        panic!("an unusable renderer must be reported");
    };
    assert!(reason.message().contains("GraphicsAPI::WebGL"));
}

// --- panic_message ---------------------------------------------------

#[test]
fn panic_message_decodes_a_static_str_payload() {
    let payload: Box<dyn Any + Send> = Box::new("static panic msg");
    let s = panic_message(&payload);
    assert_eq!(s, "static panic msg");
}

#[test]
fn panic_message_decodes_a_string_payload() {
    let payload: Box<dyn Any + Send> = Box::new(String::from("owned panic msg"));
    let s = panic_message(&payload);
    assert_eq!(s, "owned panic msg");
}

#[test]
fn panic_message_falls_back_for_an_unknown_payload_type() {
    let payload: Box<dyn Any + Send> = Box::new(42_u32);
    let s = panic_message(&payload);
    assert!(s.contains("unknown panic payload"));
}

#[test]
fn panic_message_decodes_a_boxed_error_send_sync_payload() {
    let inner: Box<dyn std::error::Error + Send + Sync + 'static> =
        Box::new(std::io::Error::other("send-sync error payload"));
    let payload: Box<dyn Any + Send> = Box::new(inner);
    let s = panic_message(&payload);
    assert!(s.contains("send-sync error payload"));
}

#[test]
fn panic_message_decodes_a_boxed_error_send_payload() {
    let inner: Box<dyn std::error::Error + Send + 'static> =
        Box::new(std::io::Error::other("send-only error payload"));
    let payload: Box<dyn Any + Send> = Box::new(inner);
    let s = panic_message(&payload);
    assert!(s.contains("send-only error payload"));
}

// --- parse_source progress -------------------------------------------

fn named(mut mesh: IndexedMesh, name: &str) -> IndexedMesh {
    mesh.set_meta(Some(MeshMetadata {
        name: Some(name.to_string()),
        header_comment: None,
    }));
    mesh
}

/// Loader-shape scene carrying the cube and both tunnel prisms as
/// named leaves with distinct translations.
fn prism_trio_scene() -> Scene {
    let mut scene = Scene::new();
    let root = scene.add_group(None, Transform3D::Identity);
    scene.add_root(root).expect("group root");
    let sources = [
        ("Cube", unit_cube_mesh(), [0.0, 0.0, 0.0]),
        ("ZTunnel", cuboid_with_z_tunnel(), [5.0, 0.0, 0.0]),
        ("YTunnel", cuboid_with_y_tunnel(), [-5.0, 0.0, 0.0]),
    ];
    for (name, mesh, translation) in sources {
        let transform = if translation == [0.0, 0.0, 0.0] {
            Transform3D::Identity
        } else {
            Transform3D::Affine(AffineTransform3D::from_translation(translation))
        };
        let leaf = scene
            .add_leaf(
                None,
                transform,
                LeafPayload::Mesh(std::sync::Arc::new(named(mesh, name))),
            )
            .expect("add leaf");
        scene.add_child(root, leaf).expect("attach leaf");
    }
    scene
}

fn sink_and_cell() -> (LoadProgressSink, Arc<AtomicU32>) {
    let cell = Arc::new(AtomicU32::new(0));
    (LoadProgressSink::new(cell.clone()), cell)
}

fn published(cell: &AtomicU32) -> (LoadPhase, u32) {
    decode(cell.load(Ordering::Relaxed))
}

fn root_child_count(scene: &Scene) -> usize {
    let root = scene.node(scene.roots()[0]).expect("root resolves");
    match &root.kind {
        NodeKind::Group { children } => children.len(),
        other => panic!("the root should be a group, got {other:?}"),
    }
}

#[test]
fn parse_source_obj_ends_the_parse_phase_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("trio.obj");
    let trio = prism_trio_scene();
    write_scene(&trio, &path, WriteFormat::Obj).expect("obj writes");
    let (sink, cell) = sink_and_cell();
    let parsed = parse_source(&path, &sink).expect("obj parses");
    assert_eq!(root_child_count(&parsed.graph), 3);
    assert_eq!(published(&cell), (LoadPhase::Parse, PROGRESS_PERMILLE_MAX));
}

#[test]
fn parse_source_binary_stl_ends_the_parse_phase_complete() {
    // `.stl` goes through the format sniffer, which probes the header
    // and rewinds: the seek path of the progress reader is on this
    // route.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("trio.stl");
    let trio = prism_trio_scene();
    write_scene(&trio, &path, WriteFormat::StlBin).expect("stl writes");
    let (sink, cell) = sink_and_cell();
    let parsed = parse_source(&path, &sink).expect("stl parses");
    assert!(
        parsed.graph.aabb().expect("aabb resolves").is_some(),
        "the flattened scene carries geometry"
    );
    assert_eq!(published(&cell), (LoadPhase::Parse, PROGRESS_PERMILLE_MAX));
}

#[test]
fn parse_source_3mf_project_ends_the_parse_phase_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("trio.3mf");
    let file_type = infer_project_file_type_from_extension(Path::new("trio.3mf"))
        .expect("3mf is a project format");
    let payload = ProjectPayload {
        printer: None,
        resin: None,
        first_layer_height: None,
        embedded: Vec::new(),
    };
    let mut bytes = Vec::new();
    write_project(
        &prism_trio_scene(),
        &payload,
        EmbedProfiles::No,
        None,
        None,
        file_type,
        &mut bytes,
    )
    .expect("project writes");
    std::fs::write(&path, bytes).expect("file writes");
    let (sink, cell) = sink_and_cell();
    let parsed = parse_source(&path, &sink).expect("3mf parses");
    assert_eq!(root_child_count(&parsed.graph), 3);
    assert_eq!(published(&cell), (LoadPhase::Parse, PROGRESS_PERMILLE_MAX));
}

// --- ensure_measured_size_within --------------------------------------

#[test]
fn measured_size_at_the_cap_is_accepted() {
    assert!(ensure_measured_size_within(1024, 1024).is_ok());
}

#[test]
fn measured_size_over_the_cap_is_refused() {
    let err = ensure_measured_size_within(1025, 1024).expect_err("over-cap must fail");
    assert!(matches!(
        err,
        LoadMeshError::FileTooLarge {
            actual_bytes: 1025,
            limit_bytes: 1024,
        }
    ));
}

// --- guard_worker -----------------------------------------------------

#[test]
fn a_worker_panic_becomes_an_error_carrying_the_message() {
    let err = guard_worker::<()>(|| panic!("boom")).expect_err("a panic must become an error");
    assert_eq!(err.to_string(), "The mesh loader panicked: boom");
}

#[test]
fn a_worker_result_passes_through_unchanged() {
    assert_eq!(guard_worker(|| Ok(3)).expect("ok passes through"), 3);
    let err = guard_worker::<()>(|| Err(anyhow::anyhow!("plain failure")))
        .expect_err("err passes through");
    assert_eq!(err.to_string(), "plain failure");
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! End-to-end GPU tests. They run on a wgpu device with the **default-limits
//! profile** (the same profile Slint uses), so they catch regressions that
//! would only show up at runtime, most importantly, any binding type that is
//! gated by a non-default limit (storage buffers in the vertex stage have
//! `max_storage_buffers_per_shader_stage = 0` under default limits and silently
//! broke an earlier draft of the renderer).
//!
//! The tests soft-skip when no adapter is available (CI environments without
//! GPU drivers), printing a warning. On a developer's workstation with a
//! working driver they exercise the full pipeline: shader compilation,
//! bind-group layout validation, per-frame uniform writes, and a real draw call
//! against a tiny scene.

use std::sync::Arc;

use super::DeviceToken;
use super::*;
use crate::build_volume::NO_PRINTER_BUILD_VOLUME;
use crate::gizmo::axis::Axis;
use crate::test_support::gpu_devices::{
    SMALL_BUFFER_LIMIT, SMALL_TEXTURE_LIMIT, destroyed_test_device, logged_test_device,
    shared_one_bind_group_test_device, shared_small_buffer_test_device,
    shared_small_texture_test_device, shared_test_device, shared_tiny_buffer_test_device,
};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_scene::SurfaceTags;
use sc_scene::node::LeafPayload;

/// Builds an empty graph shaped as the loader builds it: an identity group root
/// and no leaves.
fn graph_with_group_root() -> (Scene, NodeId) {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    (g, root)
}

/// Attaches an inline-mesh leaf under `parent` with the given local transform
/// and returns the leaf's [`NodeId`].
fn add_inline_leaf(
    g: &mut Scene,
    parent: NodeId,
    mesh: IndexedMesh,
    transform: Transform3D,
) -> NodeId {
    let leaf = g
        .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
        .expect("add leaf");
    g.add_child(parent, leaf).expect("attach leaf");
    leaf
}

/// Build envelope used by the render tests below. Held as a module-level
/// constant so each test passes the same value to `render` without extra
/// boilerplate.
const TEST_BV: BuildVolume = NO_PRINTER_BUILD_VOLUME;

/// Copies `buffer` back to the CPU (it must carry `COPY_SRC`).
fn read_buffer_back(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh-viewer test buffer readback"),
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mesh-viewer test buffer readback encoder"),
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, buffer.size());
    queue.submit(std::iter::once(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result)
            .expect("readback map_async receiver dropped");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll for readback");
    rx.recv()
        .expect("readback map_async sender dropped")
        .expect("readback map_async failed");
    let data = slice
        .get_mapped_range()
        .expect("readback mapped range")
        .to_vec();
    readback.unmap();
    data
}

/// Returns the [`NodeId`] of the first reachable leaf in pre-order traversal.
/// The renderer's selection key is the leaf's stable id, so the render tests
/// take the id of "the only leaf" from the graph through this helper.
fn first_leaf(graph: &Scene) -> NodeId {
    use sc_scene::node::NodeKind;
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let node = graph.node(id).expect("walked id should be live");
        match &node.kind {
            NodeKind::Leaf(_) => return id,
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Reference { .. } => {}
        }
    }
    panic!("test fixture has no leaves");
}

fn triangle_graph_with_translation(t: AffineTransform3D) -> Scene {
    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    };
    let (mut g, root) = graph_with_group_root();
    add_inline_leaf(&mut g, root, mesh, Transform3D::Affine(t));
    g
}

/// Checks that `Renderer::new` succeeds against the default-limits device. The
/// failure this guards against is a `create_bind_group_layout` validation error
/// coming out of `Renderer::new`.
#[test]
fn renderer_construction_under_default_limits_succeeds() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let _renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
}

#[test]
fn renderer_new_names_the_mesh_stage_on_a_one_bind_group_device() {
    let Some((device, queue)) = shared_one_bind_group_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let Err(err) = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default()) else {
        panic!("a 2-bind-group pipeline must be refused on a 1-bind-group device");
    };
    assert_eq!(err.stage, RendererStage::Mesh);
    let text = err.to_string();
    assert!(text.contains("mesh pipeline"), "{text}");
    assert!(
        text.contains("bind"),
        "the cause names the bind group limit: {text}"
    );
}

#[test]
fn two_group_pipelines_are_refused_on_a_one_bind_group_device_and_one_group_ones_are_not() {
    let Some((device, queue)) = shared_one_bind_group_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    assert!(MeshPipeline::create(&device, &queue).is_err());
    assert!(OutlinePipeline::create(&device).is_ok());
    let gizmo_buffers = GizmoBuffers::create(&device, &queue).expect("buffers need no bind groups");
    assert!(GizmoPipeline::create(&device, &gizmo_buffers).is_err());
}

#[test]
fn renderer_calls_return_device_lost_once_the_flag_is_set() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let loss = DeviceLoss::default();
    let token = DeviceToken::next();
    let mut renderer = Renderer::new(device.clone(), token, queue, loss.clone())
        .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    let built_before = GpuScene::build(&device, token, &graph, &DeviceLoss::default())
        .expect("a scene built before the loss");
    loss.mark("Unknown: test".to_string());

    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);
    assert!(matches!(
        renderer.render(None, TEST_BV, &camera, 64, 64),
        Err(RenderError::DeviceLost)
    ));
    let builds = std::cell::Cell::new(0);
    let mut build = || {
        builds.set(builds.get() + 1);
        Some(CapRaster {
            coverage: vec![255u8; 16],
            width: 4,
            height: 4,
            highlight: Vec::new(),
        })
    };
    assert!(matches!(
        renderer.update_cap(Some(1), &mut build),
        Err(RenderError::DeviceLost)
    ));
    assert_eq!(builds.get(), 0, "no raster is built for a lost device");
    let upload_err = renderer
        .upload_scene(&graph)
        .expect_err("an upload on a lost device is refused");
    assert!(
        upload_err
            .downcast_ref::<scene_upload::DeviceLostDuringUpload>()
            .is_some(),
        "{upload_err}"
    );
    assert!(!renderer.install_scene(built_before));
}

#[test]
fn gpu_scene_build_refuses_typed_on_a_flagged_device() {
    let Some((device, _queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let loss = DeviceLoss::default();
    loss.mark("Unknown: test".to_string());
    let graph = graph_with_one_leaf_of_faces(4);
    let Err(err) = GpuScene::build(&device, DeviceToken::next(), &graph, &loss) else {
        panic!("a flagged device builds nothing");
    };
    assert!(
        err.downcast_ref::<scene_upload::DeviceLostDuringUpload>()
            .is_some(),
        "{err}"
    );
}

#[test]
fn gpu_scene_build_on_a_destroyed_device_is_refused_before_the_map_when_flagged() {
    let Some((device, loss)) = destroyed_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    assert!(loss.is_lost(), "the lost callback marked the flag");
    assert!(loss.reason().is_some_and(|r| r.contains("Destroyed")));
    let graph = graph_with_one_leaf_of_faces(4);
    let Err(err) = GpuScene::build(&device, DeviceToken::next(), &graph, &loss) else {
        panic!("a destroyed device builds nothing");
    };
    assert!(
        err.downcast_ref::<scene_upload::DeviceLostDuringUpload>()
            .is_some(),
        "{err}"
    );
}

#[test]
fn gpu_scene_build_on_a_destroyed_device_turns_the_map_failure_into_an_error_when_unflagged() {
    let Some((device, _flagged)) = destroyed_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    // A clear flag is a loss nobody has been told about yet: the buffer map
    // inside wgpu fails, and that failure must come back as the typed error.
    let unflagged = DeviceLoss::default();
    let graph = graph_with_one_leaf_of_faces(4);
    let Err(err) = GpuScene::build(&device, DeviceToken::next(), &graph, &unflagged) else {
        panic!("a destroyed device builds nothing, flagged or not");
    };
    assert!(
        err.downcast_ref::<scene_upload::DeviceLostDuringUpload>()
            .is_some(),
        "{err}"
    );
}

/// Checks that `GpuScene::build` succeeds against the default-limits device for
/// a non-trivial graph with a non-identity transform.
#[test]
fn gpu_scene_build_succeeds_under_default_limits() {
    let Some((device, _queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
    let gpu = GpuScene::build(&device, DeviceToken::next(), &graph, &DeviceLoss::default())
        .expect("GpuScene::build should succeed");
    // Stride must be at least 1 ModelEntry and a multiple of the device's
    // reported alignment.
    let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
    assert!(gpu.model_stride() >= MODEL_ENTRY_SIZE);
    assert_eq!(gpu.model_stride() % alignment, 0);
}

/// Checks that `build_with_progress` ticks once per object per pass with
/// triangle counts: the single-triangle graph yields 1 flatten tick and 1
/// buffer tick, ending at the doubled total.
#[test]
fn gpu_scene_build_reports_two_pass_triangle_progress() {
    let Some((device, _queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
    let mut ticks: Vec<scene_upload::UploadProgress> = Vec::new();
    GpuScene::build_with_progress(
        &device,
        DeviceToken::next(),
        &graph,
        &DeviceLoss::default(),
        |p| ticks.push(p),
    )
    .expect("GpuScene::build_with_progress should succeed");
    let expected: Vec<scene_upload::UploadProgress> = [(1, 2), (2, 2)]
        .into_iter()
        .map(
            |(processed_triangles, total_triangles)| scene_upload::UploadProgress {
                processed_triangles,
                total_triangles,
            },
        )
        .collect();
    assert_eq!(ticks, expected);
}

/// Builds a graph with 1 leaf whose mesh has `face_count` faces over 3 shared
/// vertices: an index buffer of `12 * face_count` bytes next to a 36-byte
/// vertex buffer.
fn graph_with_one_leaf_of_faces(face_count: usize) -> Scene {
    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: (0..face_count)
            .map(|_| IndexedTriangle::from_raw_indices([0, 1, 2]))
            .collect(),
    };
    let (mut g, root) = graph_with_group_root();
    add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
    g
}

/// Checks that a leaf whose index buffer is over the device's `max_buffer_size`
/// is refused with a typed error.
#[test]
fn gpu_scene_build_refuses_a_leaf_above_the_device_buffer_limit() {
    let Some((device, _queue)) = shared_small_buffer_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    // 100 faces: 1200 index bytes against a 1024-byte limit.
    let graph = graph_with_one_leaf_of_faces(100);
    let err = GpuScene::build(&device, DeviceToken::next(), &graph, &DeviceLoss::default())
        .expect_err("a leaf above the limit is refused");
    let refusal = err
        .downcast_ref::<scene_upload::LeafBufferExceedsDeviceLimit>()
        .expect("the refusal is typed");
    assert_eq!(refusal.buffer_kind, scene_upload::LeafBufferKind::Index);
    assert_eq!(refusal.num_elements, 100);
    assert_eq!(refusal.num_bytes, 1200);
    assert_eq!(refusal.max_buffer_size, SMALL_BUFFER_LIMIT);
}

/// Checks that a leaf whose index buffer is exactly at the limit builds.
#[test]
fn gpu_scene_build_accepts_a_leaf_at_the_device_buffer_limit() {
    let Some((device, _queue)) = shared_small_buffer_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    // 85 faces: 1020 index bytes, padded to 1020 (already 4-aligned).
    let graph = graph_with_one_leaf_of_faces(85);
    GpuScene::build(&device, DeviceToken::next(), &graph, &DeviceLoss::default())
        .expect("a leaf at the limit should build");
}

/// Renders end-to-end: builds the renderer, installs a scene with a
/// non-identity transform and drives a render at a tiny size. The
/// storage-buffer-in-vertex bug surfaced as a runtime panic during
/// `RenderingSetup`'s `Renderer::new`; this test runs that code path against
/// the same limits and would have caught the regression before runtime.
#[test]
fn render_succeeds_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render should produce a texture");
    assert_eq!(texture.size().width, 64);
    assert_eq!(texture.size().height, 64);

    // A second upload replaces the held scene and renders again.
    let graph2 =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 7.0, 0.0]));
    renderer
        .upload_scene(&graph2)
        .expect("a second upload_scene under default limits");
    let bounds2 = renderer
        .scene_bounds()
        .expect("scene bounds follow the new scene");
    assert_ne!(bounds2, bounds);
    renderer
        .render(
            Some(SceneRender {
                graph: &graph2,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render after the second upload should produce a texture");
}

/// Checks that a buffer the device refuses comes back as `Err`, with the wgpu
/// error.
#[test]
fn create_buffer_reporting_returns_the_devices_refusal_without_panicking() {
    let Some((device, _queue)) = shared_small_buffer_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let err = device_ops::create_buffer_reporting(
        &device,
        &wgpu::BufferDescriptor {
            label: Some("over the small limit"),
            size: SMALL_BUFFER_LIMIT + 8,
            usage: wgpu::BufferUsages::INDEX,
            mapped_at_creation: false,
        },
    )
    .expect_err("a buffer above max_buffer_size is refused");
    assert!(matches!(err, wgpu::Error::Validation { .. }));
    assert!(err.to_string().contains("maximum buffer size"), "{err}");
}

/// Checks that within the limit the helper hands back a buffer that can be
/// mapped, written and unmapped like any other.
#[test]
fn create_buffer_reporting_yields_a_usable_buffer_within_the_limit() {
    let Some((device, _queue)) = shared_small_buffer_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let buffer = device_ops::create_buffer_reporting(
        &device,
        &wgpu::BufferDescriptor {
            label: Some("at the small limit"),
            size: SMALL_BUFFER_LIMIT,
            usage: wgpu::BufferUsages::INDEX,
            mapped_at_creation: true,
        },
    )
    .expect("a buffer at max_buffer_size is accepted");
    buffer
        .get_mapped_range_mut(..)
        .expect("a buffer mapped at creation")
        .copy_from_slice(&[7u8; SMALL_BUFFER_LIMIT as usize]);
    buffer.unmap();
    assert_eq!(buffer.size(), SMALL_BUFFER_LIMIT);
}

/// Checks that a device that refuses the gizmo buffers answers with `Err`; the
/// gizmo geometry is uploaded without mapping.
#[test]
fn gizmo_buffers_are_refused_typed_on_a_tiny_buffer_device() {
    let Some((device, queue)) = shared_tiny_buffer_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let err = GizmoBuffers::create(&device, &queue)
        .err()
        .expect("the tiny-buffer device refuses the gizmo geometry");
    assert!(matches!(err, wgpu::Error::Validation { .. }), "{err}");
    assert!(err.to_string().contains("maximum buffer size"), "{err}");
}

/// Checks the same for the plate geometry, which is uploaded at render time.
#[test]
fn plate_buffers_are_refused_typed_on_a_tiny_buffer_device() {
    let Some((device, queue)) = shared_tiny_buffer_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let tab = FrontTab::around_label(4.0, 1.0);
    let err = PlateBuffers::create(&device, &queue, TEST_BV, Some(tab))
        .err()
        .expect("the tiny-buffer device refuses the plate geometry");
    assert!(
        matches!(
            err,
            RenderError::DeviceRefused {
                what: RenderAllocation::PlateGeometry,
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("maximum buffer size"), "{err}");
}

/// Checks that `upload_static_buffer` fills the buffer through the queue: the
/// bytes read back are the bytes handed in.
#[test]
fn upload_static_buffer_round_trips_contents() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let contents: Vec<u8> = (0u8..48).collect();
    let buffer = device_ops::upload_static_buffer(
        &device,
        &queue,
        "round-trip",
        &contents,
        wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
    )
    .expect("a 48-byte buffer under default limits");
    assert_eq!(buffer.size(), 48);
    assert_eq!(read_buffer_back(&device, &queue, &buffer), contents);
}

/// Checks that, with the error logger installed, a device error nothing scopes
/// is logged and the calling thread survives it.
#[test]
fn after_the_error_logger_is_installed_an_unscoped_device_error_no_longer_panics() {
    let Some((device, _queue)) = logged_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    install_device_error_logger(&device);
    let worker = std::thread::spawn(move || {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unscoped, over the small limit"),
            size: SMALL_BUFFER_LIMIT + 8,
            usage: wgpu::BufferUsages::INDEX,
            mapped_at_creation: false,
        });
        buffer.size()
    });
    assert_eq!(
        worker
            .join()
            .expect("the device error must not end the thread"),
        SMALL_BUFFER_LIMIT + 8
    );
}

/// Checks that error scopes are per thread: 2 threads scoping refused
/// allocations at the same time each get their own refusal back and neither
/// trips wgpu's scope-order check.
#[test]
fn concurrent_scoped_allocations_on_2_threads_each_get_their_own_refusal() {
    let Some((device, _queue)) = shared_small_buffer_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let start = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let device = device.clone();
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                (0..500).all(|_| {
                    device_ops::create_buffer_reporting(
                        &device,
                        &wgpu::BufferDescriptor {
                            label: Some("concurrently over the small limit"),
                            size: SMALL_BUFFER_LIMIT + 8,
                            usage: wgpu::BufferUsages::INDEX,
                            mapped_at_creation: false,
                        },
                    )
                    .is_err()
                })
            })
        })
        .collect();
    for worker in workers {
        assert!(
            worker
                .join()
                .expect("scoped allocation must not panic on a worker thread"),
            "every refused allocation must come back as Err"
        );
    }
}

/// Checks that the slice-view cap renders without crashing: once `update_cap`
/// installs a coverage texture, a render with a clip height set still produces
/// a texture, so the cap pipeline and its draw are exercised.
#[test]
fn render_with_a_cap_produces_a_texture() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);

    let mut build = || {
        Some(CapRaster {
            coverage: vec![255u8; 4 * 4],
            width: 4,
            height: 4,
            highlight: Vec::new(),
        })
    };
    renderer
        .update_cap(Some(1), &mut build)
        .expect("a 4 x 4 cap texture is not refused");

    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: Some(1.0),
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render should produce a texture with a cap");
    assert_eq!(texture.size().width, 64);
    assert_eq!(texture.size().height, 64);
}

/// Returns a shared (Device, Queue) pinned to the DX12 backend, built lazily,
/// or `None` when no DX12 adapter or device is available. The default-backend
/// `shared_test_device` may resolve to Vulkan on Windows when a Vulkan ICD is
/// present, so it does not guarantee the DX12 path is exercised.
#[cfg(windows)]
fn shared_dx12_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    static DEVICE: std::sync::OnceLock<Option<(wgpu::Device, wgpu::Queue)>> =
        std::sync::OnceLock::new();
    DEVICE
        .get_or_init(|| {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = pollster::block_on(
                instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
            )
            .ok()?;
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("mesh-viewer DX12 test device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                    experimental_features: wgpu::ExperimentalFeatures::default(),
                    trace: wgpu::Trace::Off,
                }))
                .ok()?;
            Some((device, queue))
        })
        .clone()
}

/// Renders end-to-end on DX12, the backend that Windows production pins (see
/// the GUI's shared-device setup): Slint's Skia renderer rejects a non-DX12
/// adapter on Windows. The other render tests build a default-backend device
/// that may be Vulkan even on Windows, so this test pins DX12 explicitly,
/// proving shader compilation, bind-group validation, and a draw call all work
/// on the backend Windows actually ships. Soft-skips with no DX12 adapter.
#[cfg(windows)]
#[test]
fn render_succeeds_on_dx12_backend() {
    let Some((device, queue)) = shared_dx12_device() else {
        eprintln!("skipping: no DX12 wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene on the DX12 device");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render on DX12 should produce a texture");
    assert_eq!(texture.size().width, 64);
    assert_eq!(texture.size().height, 64);
}

/// Checks the same as `render_succeeds_under_default_limits`, but with a
/// non-`None` selection so the `flags.x` lane carries a 1 through the per-frame
/// uniform write. Catches any default-limits regression in the selection-flag
/// path before runtime.
#[test]
fn render_with_selection_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render with selection should produce a texture");
    assert_eq!(texture.size().width, 64);
}

/// Drives the outline post-pass: a render with a selection binds the
/// stencil-aspect bind group and runs the fullscreen edge-detect. Pins that
/// `RENDER_ATTACHMENT | TEXTURE_BINDING` on a multisampled
/// `Depth24PlusStencil8` is accepted under the default-limits profile, and that
/// the `multisampled: true, sample_type: Uint` stencil binding declared in the
/// layout matches the WGSL `texture_multisampled_2d<u32>` the outline shader
/// expects.
#[test]
fn outline_post_pass_under_default_limits_is_validated() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    // Twice: the first frame creates the MSAA targets and outline bind group;
    // the second exercises the cached path.
    let _ = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render with selection runs the outline pass");
    let _ = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("cached outline bind group survives a second frame");
}

/// Drives the gizmo pass: a render with `Some(GizmoFrame)` binds the gizmo
/// pipeline, runs the per-axis dynamic-offset uniform path and validates the
/// private depth attachment under default limits. 2 consecutive frames send the
/// MSAA-targets cache and the gizmo depth view through the reuse path.
#[test]
fn render_with_gizmo_under_default_limits_is_validated() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let frame = GizmoFrame {
        pivot: center,
        scale: 0.5,
        visibility: GizmoVisibility::DraggingTranslate { axis: Axis::X },
        view_dir: Vec3::new(0.0, 0.0, -1.0),
        hover: None,
        drag_visual: None,
        look: crate::gizmo::look::GizmoLook::default(),
        tool: None,
    };
    let _ = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: Some(frame),
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("first gizmo render should succeed");
    let _ = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: Some(frame),
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("cached gizmo resources survive a second frame");
}

/// Checks that idle visibility renders all 3 handle categories at once, which
/// exercises the translate, rotate and scale draws in a single frame.
#[test]
fn render_with_idle_gizmo_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let frame = GizmoFrame {
        pivot: center,
        scale: 0.5,
        visibility: GizmoVisibility::Idle,
        view_dir: Vec3::new(-0.5, 0.6, -0.62).normalize(),
        hover: None,
        drag_visual: None,
        look: crate::gizmo::look::GizmoLook::default(),
        tool: None,
    };
    let _ = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: Some(frame),
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("idle gizmo render should succeed");
}

/// Checks that rotate-drag visibility renders the dragged ring with the marks of
/// the turn, which pins the ring mesh and the per-frame marks buffer under
/// default limits.
#[test]
fn render_with_rotate_gizmo_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let frame = GizmoFrame {
        pivot: center,
        scale: 0.5,
        visibility: GizmoVisibility::DraggingRotate { axis: Axis::X },
        view_dir: Vec3::new(-0.5, 0.6, -0.62).normalize(),
        hover: None,
        // A turn in progress: the slice and the ticks go through the
        // per-frame marks buffer.
        drag_visual: Some(GizmoDragVisual::Rotate {
            axis: Axis::X,
            start: 0.3,
            sweep: 1.2,
        }),
        look: crate::gizmo::look::GizmoLook::default(),
        tool: None,
    };
    let _ = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: Some(frame),
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("rotate gizmo render should succeed");
}

/// Checks that scale-axis visibility renders the 3 midpoint cubes, which pins
/// the scale-pass wiring under default limits.
#[test]
fn render_with_scale_gizmo_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let scale_axis = GizmoFrame {
        pivot: center,
        scale: 0.5,
        visibility: GizmoVisibility::DraggingScaleAxis { axis: Axis::Y },
        view_dir: Vec3::new(-0.5, 0.6, -0.62).normalize(),
        hover: None,
        drag_visual: None,
        look: crate::gizmo::look::GizmoLook::default(),
        tool: None,
    };
    let _ = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[first_leaf(&graph)],
                gizmo: Some(scale_axis),
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("scale-axis gizmo render should succeed");
}

/// Renders a multi-object scene, which drives 1 draw call per object; the
/// dynamic-offset bind path must keep `model_entry` pointing at the right slot
/// for each draw. Pure smoke test (no read-back), but it exercises the full
/// draw loop with N > 1.
#[test]
fn render_multi_object_scene_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    };
    let (mut graph, root) = graph_with_group_root();
    add_inline_leaf(&mut graph, root, mesh.clone(), Transform3D::Identity);
    add_inline_leaf(
        &mut graph,
        root,
        mesh.clone(),
        Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
    );
    add_inline_leaf(
        &mut graph,
        root,
        mesh,
        Transform3D::Affine(AffineTransform3D::from_translation([0.0, 10.0, 0.0])),
    );
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    renderer.upload_scene(&graph).expect("upload");
    let bounds = renderer.scene_bounds().expect("bounds");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let _texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render multi-object");
}

/// A small closed mesh with a box that is not symmetric about any axis.
fn wedge_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([4.0, 0.0, 0.0]),
            Vertex::new([0.0, 3.0, 0.0]),
            Vertex::new([0.0, 0.0, 2.0]),
        ],
        faces: vec![
            IndexedTriangle::from_raw_indices([0, 2, 1]),
            IndexedTriangle::from_raw_indices([0, 1, 3]),
            IndexedTriangle::from_raw_indices([0, 3, 2]),
            IndexedTriangle::from_raw_indices([1, 2, 3]),
        ],
    }
}

/// Adds a root leaf drawing the resource `rid` at `[x, y, 0]`, as a paste does.
fn paste_resource_leaf(graph: &mut Scene, rid: sc_scene::MeshResourceId, x: f64, y: f64) {
    let leaf = graph
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([x, y, 0.0])),
            LeafPayload::MeshResource(rid),
        )
        .expect("leaf");
    graph.add_root(leaf).expect("root");
}

fn renderer_on(device: &wgpu::Device, queue: &wgpu::Queue) -> Renderer {
    Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device")
}

fn installed_objects(renderer: &Renderer) -> &[scene_upload::ObjectDraw] {
    renderer.scene.as_ref().expect("a scene").gpu.objects()
}

/// Copies of one mesh draw from one set of buffers: a paste uploads nothing of
/// its own (#353).
#[test]
fn copies_of_a_mesh_share_one_set_of_buffers() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut graph = Scene::new();
    let rid = graph.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut graph, rid, 0.0, 0.0);
    paste_resource_leaf(&mut graph, rid, 10.0, 0.0);
    let mut renderer = renderer_on(&device, &queue);
    renderer.upload_scene(&graph).expect("upload");
    let objects = installed_objects(&renderer);
    assert_eq!(objects.len(), 2);
    assert!(objects[0].shares_buffers_with(&objects[1]));
    assert_eq!(objects[1].index_count(), 12);
}

/// Uploading the scene again after a paste keeps the buffers of the mesh it
/// already drew, and a mesh new to the scene gets buffers of its own.
#[test]
fn an_upload_after_a_paste_reuses_the_buffers_already_uploaded() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut graph = Scene::new();
    let rid = graph.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut graph, rid, 0.0, 0.0);
    let mut renderer = renderer_on(&device, &queue);
    renderer.upload_scene(&graph).expect("upload");
    let probe = installed_objects(&renderer)[0].buffers_for_test();

    paste_resource_leaf(&mut graph, rid, 10.0, 0.0);
    let other = graph.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut graph, other, 20.0, 0.0);
    renderer.upload_scene(&graph).expect("upload");
    let objects = installed_objects(&renderer);
    assert_eq!(objects.len(), 3);
    assert!(Arc::ptr_eq(&objects[0].buffers_for_test(), &probe));
    assert!(Arc::ptr_eq(&objects[1].buffers_for_test(), &probe));
    assert!(!objects[2].shares_buffers_with(&objects[0]));
}

/// The buffers of meshes uploaded on a worker for models about to join the
/// scene (#391) are the ones the next upload draws them with; the meshes it
/// then no longer draws are let go.
#[test]
fn an_upload_takes_the_buffers_adopted_for_joining_meshes() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut graph = Scene::new();
    let rid = graph.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut graph, rid, 0.0, 0.0);
    let mut renderer = renderer_on(&device, &queue);
    renderer.upload_scene(&graph).expect("upload");

    // What the worker read and uploaded, with the same mesh.
    let joining = Arc::new(wedge_mesh());
    let mut read = Scene::new();
    let read_rid = read.add_mesh_resource_arc(Arc::clone(&joining));
    paste_resource_leaf(&mut read, read_rid, 0.0, 0.0);
    let built = GpuScene::build(
        &device,
        renderer.device_token(),
        &read,
        &DeviceLoss::default(),
    )
    .expect("built");
    let probe = built.objects()[0].buffers_for_test();
    assert!(renderer.adopt_meshes(built));

    let other = graph.add_mesh_resource_arc(joining);
    paste_resource_leaf(&mut graph, other, 20.0, 0.0);
    renderer.upload_scene(&graph).expect("upload");
    let objects = installed_objects(&renderer);
    assert_eq!(objects.len(), 2);
    assert!(Arc::ptr_eq(&objects[1].buffers_for_test(), &probe));

    // A mesh adopted but never drawn is let go at the next upload.
    let unused = Arc::new(wedge_mesh());
    let mut read = Scene::new();
    let read_rid = read.add_mesh_resource_arc(Arc::clone(&unused));
    paste_resource_leaf(&mut read, read_rid, 0.0, 0.0);
    let built = GpuScene::build(
        &device,
        renderer.device_token(),
        &read,
        &DeviceLoss::default(),
    )
    .expect("built");
    let probe = built.objects()[0].buffers_for_test();
    assert!(renderer.adopt_meshes(built));
    renderer.upload_scene(&graph).expect("upload");
    assert_eq!(Arc::strong_count(&probe), 1);
}

/// Buffers built on another device are not adopted.
#[test]
fn buffers_of_another_device_are_not_adopted() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut read = Scene::new();
    let rid = read.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut read, rid, 0.0, 0.0);
    let mut renderer = renderer_on(&device, &queue);
    let built = GpuScene::build(&device, DeviceToken::next(), &read, &DeviceLoss::default())
        .expect("built");
    assert!(!renderer.adopt_meshes(built));
}

/// The buffers of a mesh the scene no longer draws are released at the next
/// upload, and a mesh drawn again after a replacement is uploaded afresh.
#[test]
fn an_upload_releases_the_buffers_of_meshes_no_longer_drawn() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let old = Arc::new(wedge_mesh());
    let mut graph = Scene::new();
    let rid = graph.add_mesh_resource_arc(Arc::clone(&old));
    paste_resource_leaf(&mut graph, rid, 0.0, 0.0);
    let mut renderer = renderer_on(&device, &queue);
    renderer.upload_scene(&graph).expect("upload");
    let probe = installed_objects(&renderer)[0].buffers_for_test();

    // Replaced as an edit does: the resource now holds another mesh.
    let mut replaced = Scene::new();
    let new_rid = replaced.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut replaced, new_rid, 0.0, 0.0);
    renderer.upload_scene(&replaced).expect("upload");
    let objects = installed_objects(&renderer);
    assert!(!Arc::ptr_eq(&objects[0].buffers_for_test(), &probe));
    // Only the probe here and nothing in the renderer holds the old buffers.
    assert_eq!(Arc::strong_count(&probe), 1);
    let uploaded = renderer.scene.take().expect("a scene").gpu.into_uploaded();
    assert_eq!(uploaded.len(), 1);
}

/// The world boxes of meshes a new upload still draws are kept, and the ones
/// of meshes it no longer draws are forgotten.
#[test]
fn an_upload_keeps_the_world_boxes_of_meshes_still_drawn() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut graph = Scene::new();
    let rid = graph.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut graph, rid, 0.0, 0.0);
    let mut renderer = renderer_on(&device, &queue);
    renderer.upload_scene(&graph).expect("upload");
    let frame = |renderer: &mut Renderer, graph: &Scene| {
        build_model_entries(graph, &[], TEST_BV, &mut renderer.world_boxes, false)
            .expect("entries");
    };
    frame(&mut renderer, &graph);
    assert_eq!(renderer.world_boxes.remembered(), 1);

    paste_resource_leaf(&mut graph, rid, 10.0, 0.0);
    renderer.upload_scene(&graph).expect("upload");
    assert_eq!(renderer.world_boxes.remembered(), 1);
    frame(&mut renderer, &graph);
    assert_eq!(renderer.world_boxes.remembered(), 2);

    let mut other = Scene::new();
    let other_rid = other.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut other, other_rid, 0.0, 0.0);
    renderer.upload_scene(&other).expect("upload");
    assert_eq!(renderer.world_boxes.remembered(), 0);
}

/// A scene uploaded on top of the scene before it draws exactly what a fresh
/// upload of it draws.
#[test]
fn an_upload_on_top_of_the_scene_before_draws_what_a_fresh_upload_draws() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut graph = Scene::new();
    let rid = graph.add_mesh_resource_arc(Arc::new(wedge_mesh()));
    paste_resource_leaf(&mut graph, rid, 0.0, 0.0);
    let mut reused = renderer_on(&device, &queue);
    reused.upload_scene(&graph).expect("upload");
    paste_resource_leaf(&mut graph, rid, 6.0, 0.0);
    paste_resource_leaf(&mut graph, rid, 0.0, 6.0);
    reused.upload_scene(&graph).expect("upload");
    let mut fresh = renderer_on(&device, &queue);
    fresh.upload_scene(&graph).expect("upload");

    assert_eq!(reused.scene_bounds(), fresh.scene_bounds());
    let bounds = fresh.scene_bounds().expect("bounds");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let pixels = |renderer: &mut Renderer| {
        renderer
            .preview_pixels(&graph, TEST_BV, &camera, 64, 64)
            .expect("drawn")
            .expect("a frame")
    };
    let (a, b) = (pixels(&mut reused), pixels(&mut fresh));
    assert!(
        a.rgba.iter().any(|&c| c != a.rgba[0]),
        "the frame shows something"
    );
    assert_eq!(a, b);
}

/// Renders with no scene installed: the plate alone should produce a non-`None`
/// texture. 2 consecutive calls, so the second exercises the cached
/// `plate_buffers` path.
#[test]
fn render_plate_only_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);
    let _ = renderer
        .render(None, TEST_BV, &camera, 64, 64)
        .expect("render is not refused")
        .expect("first plate-only render should succeed");
    let _ = renderer
        .render(None, TEST_BV, &camera, 64, 64)
        .expect("render is not refused")
        .expect("second plate-only render should succeed");
}

/// Checks that a label installed before the frame reaches the plate as a tab
/// and a textured quad over it, and holds across a second frame.
#[test]
fn render_with_a_front_label_under_default_limits() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);

    let mut build = || crate::front_label::rasterise_label("FRONT", Some("Photon Mono M7 Pro"));
    renderer
        .update_front_label(1, &mut build)
        .expect("the label is not refused");
    for _ in 0..2 {
        let _ = renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("render is not refused")
            .expect("a labelled plate should draw");
    }
}

/// Checks that a word that rasterises to nothing leaves the plate bare: no
/// label, and no tab to carry one.
#[test]
fn a_label_with_no_ink_leaves_the_plate_without_a_tab() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);

    let mut build = || crate::front_label::rasterise_label("   ", None);
    renderer
        .update_front_label(1, &mut build)
        .expect("an inkless label is not an error");
    let _ = renderer
        .render(None, TEST_BV, &camera, 64, 64)
        .expect("render is not refused")
        .expect("a bare plate should still draw");
}

#[test]
fn render_clamps_the_target_to_the_device_texture_ceiling() {
    let Some((device, queue)) = shared_small_texture_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the small-texture test device");
    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);
    let texture = renderer
        .render(None, TEST_BV, &camera, 300, 150)
        .expect("render is not refused")
        .expect("a viewport wider than the ceiling still renders");
    assert_eq!(
        (texture.size().width, texture.size().height),
        (SMALL_TEXTURE_LIMIT, SMALL_TEXTURE_LIMIT / 2),
        "the target is clamped to the ceiling with the aspect kept"
    );
}

#[test]
fn msaa_targets_are_refused_typed_on_a_small_texture_device() {
    let Some((device, queue)) = shared_small_texture_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the small-texture test device");
    let over = SMALL_TEXTURE_LIMIT + 44;
    let refusal = MsaaTargets::create(
        &device,
        &renderer.pipelines.outline.bind_group_layout,
        over,
        over,
    )
    .err()
    .expect("attachments over the texture ceiling are refused");
    assert!(matches!(
        refusal,
        RenderError::DeviceRefused {
            what: RenderAllocation::MsaaTargets,
            ..
        }
    ));
}

#[test]
fn colour_targets_are_refused_typed_on_a_small_texture_device() {
    let Some((device, _queue)) = shared_small_texture_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let over = SMALL_TEXTURE_LIMIT + 44;
    let refusal = TargetPool::create(&device, over, over)
        .err()
        .expect("colour targets over the texture ceiling are refused");
    assert!(matches!(
        refusal,
        RenderError::DeviceRefused {
            what: RenderAllocation::ColourTargets,
            ..
        }
    ));
}

#[test]
fn update_cap_reports_a_refused_texture_and_leaves_the_cap_off() {
    let Some((device, queue)) = shared_small_texture_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the small-texture test device");
    let side = SMALL_TEXTURE_LIMIT * 2;
    let builds = std::cell::Cell::new(0);
    let mut build = || {
        builds.set(builds.get() + 1);
        Some(CapRaster {
            coverage: vec![255u8; (side * side) as usize],
            width: side,
            height: side,
            highlight: Vec::new(),
        })
    };

    let refusal = renderer
        .update_cap(Some(1), &mut build)
        .expect_err("a cap texture over the ceiling is refused");
    assert!(matches!(
        refusal,
        RenderError::DeviceRefused {
            what: RenderAllocation::CapTexture,
            ..
        }
    ));
    assert_eq!(builds.get(), 1);

    // The same key is not retried; a new key is.
    renderer
        .update_cap(Some(1), &mut build)
        .expect("a remembered refusal is not an error");
    assert_eq!(builds.get(), 1);
    let _ = renderer.update_cap(Some(2), &mut build);
    assert_eq!(builds.get(), 2);

    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);
    renderer
        .render(None, TEST_BV, &camera, 64, 64)
        .expect("the frame renders without the cap")
        .expect("a 64 x 64 frame produces a texture");
}

/// Checks that a render call against a different `BuildVolume` rebuilds the
/// cached plate buffers so the new envelope reaches the next frame: it renders
/// once at `TEST_BV`, then at a smaller volume, and confirms that both frames
/// succeed and that the cache key follows the latest call.
#[test]
fn render_plate_buffers_rebuild_on_envelope_change() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let camera = OrbitCamera::new(Vec3::ZERO, 100.0);

    let _ = renderer
        .render(None, TEST_BV, &camera, 64, 64)
        .expect("render is not refused")
        .expect("render at default envelope");
    let cached_after_first = renderer
        .pipelines
        .plate_buffers
        .as_ref()
        .map(|b| b.build_volume);
    assert_eq!(cached_after_first, Some(TEST_BV));

    let smaller = BuildVolume::new(50.0, 50.0, 50.0);
    let _ = renderer
        .render(None, smaller, &camera, 64, 64)
        .expect("render is not refused")
        .expect("render at smaller envelope");
    let cached_after_second = renderer
        .pipelines
        .plate_buffers
        .as_ref()
        .map(|b| b.build_volume);
    assert_eq!(cached_after_second, Some(smaller));
}

/// Builds a 1-triangle graph that pokes 1 vertex past the +X envelope wall of
/// `TEST_BV` at x = +96 mm. 2 anchor vertices stay inside the envelope, so the
/// graph's AABB straddles the wall.
fn x_pos_breaching_graph() -> Scene {
    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, -1.0, 50.0]),
            Vertex::new([0.0, 1.0, 50.0]),
            Vertex::new([120.0, 0.0, 50.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    };
    let (mut g, root) = graph_with_group_root();
    add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
    g
}

/// Counts pixels whose RGB signature matches the wall warning stripe. The
/// shader emits the theme's amber `WALL_WARNING_COLOUR = (0.843, 0.655,
/// 0.353)` with `WALL_STRIPE_ALPHA = 0.55`, so on a typical background the
/// visible pixel is warm: red at least green, both well above blue. The
/// thresholds are loose enough to match the stripe over any plausible
/// background (plate, grey mesh, cage) but tight enough that no untinted
/// surface in the scene, all neutral or cool, clears the bar.
fn count_warning_pixels(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|p| {
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            r > 100 && r >= g && (r - b) > 40
        })
        .count()
}

/// Copies an `Rgba8Unorm` 2D texture back to the CPU and returns the pixel
/// bytes in row-major RGBA order. Requires `wgpu::TextureUsages::COPY_SRC` on
/// the texture (the renderer's resolved color target has it). `width * 4` must
/// satisfy `COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes); the function asserts it.
fn read_target_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    const BYTES_PER_PIXEL: u32 = 4;
    let bytes_per_row = width * BYTES_PER_PIXEL;
    assert!(
        bytes_per_row.is_multiple_of(256),
        "test-only helper expects width * 4 to be a 256-byte multiple; got {bytes_per_row}"
    );
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh-viewer test readback"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mesh-viewer test readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result)
            .expect("readback map_async receiver dropped");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll for readback");
    rx.recv()
        .expect("readback map_async sender dropped")
        .expect("readback map_async failed");
    let data = slice
        .get_mapped_range()
        .expect("readback mapped range")
        .to_vec();
    readback.unmap();
    data
}

/// Checks the +X envelope warning stripe end to end: renders the same camera
/// frame twice, once with a mesh that stays inside the envelope and once with a
/// mesh that pokes past +X, reads back both framebuffers, and asserts that the
/// breach render carries visible stripe pixels (warm: red and green over
/// blue) and the safe render does not. The check is a property,
/// independent of the exact stripe geometry, driver-specific blending rounding,
/// and viewport jitter.
#[test]
fn envelope_breach_x_pos_paints_warning_stripe_pixels() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    // 64 * 4 = 256 bytes / row, which already satisfies
    // `COPY_BYTES_PER_ROW_ALIGNMENT` and matches the size used by the other
    // render tests in this module.
    let width = 64u32;
    let height = 64u32;
    // Centre the orbit on the build-volume centre with a radius that
    // comfortably contains the envelope so the +X wall is in frame regardless
    // of which mesh is installed.
    let camera = OrbitCamera::new(Vec3::new(0.0, 0.0, 100.0), 250.0);

    let safe_graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 50.0]));
    renderer
        .upload_scene(&safe_graph)
        .expect("upload safe graph");
    let safe_texture = renderer
        .render(
            Some(SceneRender {
                graph: &safe_graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            width,
            height,
        )
        .expect("render is not refused")
        .expect("safe render produces a texture");
    let safe_pixels = read_target_back(&device, &queue, &safe_texture, width, height);
    let safe_warning = count_warning_pixels(&safe_pixels);

    let breach_graph = x_pos_breaching_graph();
    renderer
        .upload_scene(&breach_graph)
        .expect("upload breaching graph");
    let breach_texture = renderer
        .render(
            Some(SceneRender {
                graph: &breach_graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            width,
            height,
        )
        .expect("render is not refused")
        .expect("breach render produces a texture");
    let breach_pixels = read_target_back(&device, &queue, &breach_texture, width, height);
    let breach_warning = count_warning_pixels(&breach_pixels);

    assert!(
        safe_warning < 5,
        "safe render should be essentially free of warning-colour pixels, got {safe_warning}"
    );
    assert!(
        breach_warning >= 20,
        "breach render should paint visible stripe pixels, got {breach_warning} (safe={safe_warning})"
    );
    assert!(
        breach_warning > safe_warning * 4,
        "breach must dominate safe in warning-colour pixel count: breach={breach_warning}, safe={safe_warning}"
    );
}

/// Checks that `install_scene` with a `GpuScene` built against the same device
/// as the renderer reports success and leaves the renderer with a populated
/// scene (observable through `scene_bounds`).
#[test]
fn install_scene_with_matching_device_succeeds() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    // Renderer and GpuScene must share the same `device_token` for
    // `install_scene` to accept the worker-built scene; mismatched tokens are
    // the device-rotation rejection path covered by the
    // `install_scene_with_mismatched_device_returns_false` integration test.
    let token = DeviceToken::next();
    let mut renderer = Renderer::new(device.clone(), token, queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    let gpu =
        GpuScene::build(&device, token, &graph, &DeviceLoss::default()).expect("gpu scene build");
    let installed = renderer.install_scene(gpu);
    assert!(installed);
    assert!(renderer.scene_bounds().is_some());
}

/// Checks that a live `Scene` that grew an extra leaf after `upload_scene` does
/// not crash `render`: the per-leaf model uploads target a `model_buffer` sized
/// at install time, so writing 1 entry per live leaf would overrun the buffer
/// (the wgpu runtime would panic on the trailing `queue.write_buffer`). The
/// renderer must detect the leaf-count drift and skip the per-frame mesh path
/// until the next install brings the GpuScene back in sync with the graph.
#[test]
fn render_against_grown_graph_does_not_panic() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue,
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let mut graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload single-leaf graph");

    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    };
    let root = graph.roots()[0];
    add_inline_leaf(
        &mut graph,
        root,
        mesh,
        Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
    );

    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);
    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render against the live mutated graph still produces a texture");
    assert_eq!(texture.size().width, 64);
}

/// Checks that `clear_scene` drops the installed scene: later `scene_bounds`
/// reads return `None`.
#[test]
fn clear_scene_drops_the_installed_scene() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer.upload_scene(&graph).expect("upload");
    assert!(renderer.scene_bounds().is_some());
    renderer.clear_scene();
    assert!(renderer.scene_bounds().is_none());
}

/// Checks that a graph whose last leaf was deleted uploads cleanly: there is
/// nothing to mirror, so the renderer ends up holding no scene.
#[test]
fn upload_scene_on_a_leafless_graph_succeeds_and_installs_nothing() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let (graph, _root) = graph_with_group_root();
    renderer
        .upload_scene(&graph)
        .expect("a leafless graph uploads as an empty mirror");
    assert!(renderer.scene_bounds().is_none());
}

/// Runs the deletion of the last object end to end: a 1-object scene, the
/// object deleted, then the upload the deletion triggers. The render that
/// follows must still produce a texture, which proves that the empty case
/// raises no wgpu validation error and that the viewport keeps painting the
/// plate.
#[test]
fn render_after_deleting_the_last_leaf_still_produces_a_texture() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let mut graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
    renderer
        .upload_scene(&graph)
        .expect("upload one-leaf graph");
    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::new(center, radius);

    let leaf = first_leaf(&graph);
    graph.remove_leaf(leaf).expect("delete the only object");
    renderer
        .upload_scene(&graph)
        .expect("the post-delete upload succeeds");
    assert!(renderer.scene_bounds().is_none());

    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            }),
            TEST_BV,
            &camera,
            64,
            64,
        )
        .expect("render is not refused")
        .expect("render after the delete still produces a texture");
    assert_eq!(texture.size().width, 64);
    assert_eq!(texture.size().height, 64);
}

/// Checks that re-uploading a leaf after the scene went empty installs a fresh
/// mirror, so the renderer comes back from the empty case.
#[test]
fn upload_after_an_empty_upload_installs_a_scene_again() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let (mut graph, root) = graph_with_group_root();
    renderer.upload_scene(&graph).expect("empty upload");
    assert!(renderer.scene_bounds().is_none());

    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    };
    add_inline_leaf(&mut graph, root, mesh, Transform3D::Identity);
    renderer.upload_scene(&graph).expect("upload after refill");
    assert!(renderer.scene_bounds().is_some());
}

/// Checks that `invalidate_sizing` drops only the viewport-sized allocations:
/// the installed scene survives (still reports bounds).
#[test]
fn invalidate_sizing_keeps_the_installed_scene() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer.upload_scene(&graph).expect("upload");
    let bounds_before = renderer.scene_bounds().expect("bounds populated");
    renderer.invalidate_sizing();
    let bounds_after = renderer.scene_bounds().expect("bounds preserved");
    assert_eq!(bounds_before, bounds_after);
}

/// Counts pixels that differ from the mesh pass's clear colour quantised to
/// `Rgba8Unorm`. Every surface the renderer draws lands outside the
/// tolerance, but for the plate's rim, which is the background's shade.
fn count_drawn_pixels(rgba: &[u8]) -> usize {
    const BACKGROUND: [i32; 3] = [17, 19, 21];
    const TOLERANCE: i32 = 2;
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| {
            (0..3).any(|channel| (pixel[channel] as i32 - BACKGROUND[channel]).abs() > TOLERANCE)
        })
        .count()
}

/// Returns a camera that holds the whole installed scene, the framing a file
/// preview asks for.
fn camera_framing_the_scene(renderer: &Renderer) -> OrbitCamera {
    let (min, max) = renderer.scene_bounds().expect("a scene is installed");
    let radius = ((max - min).length() * 0.5).max(1.0);
    OrbitCamera::new((min + max) * 0.5, radius)
}

/// Checks that a preview draws only the models: with no models on the plate it
/// has nothing to draw, where the viewport's own frame still shows the plate
/// and its cage.
#[test]
fn a_preview_of_a_scene_without_models_draws_nothing() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    // 64 * 4 = 256 bytes per row, which the readback's copy alignment already
    // satisfies.
    let (width, height) = (64u32, 64u32);
    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);
    let (graph, _) = graph_with_group_root();

    let preview = renderer
        .render_preview(&graph, TEST_BV, &camera, width, height)
        .expect("preview is not refused")
        .expect("a sized preview produces a texture");
    let preview_pixels = read_target_back(&device, &queue, &preview, width, height);
    assert_eq!(count_drawn_pixels(&preview_pixels), 0);

    let workspace = renderer
        .render(None, TEST_BV, &camera, width, height)
        .expect("render is not refused")
        .expect("a sized viewport produces a texture");
    let workspace_pixels = read_target_back(&device, &queue, &workspace, width, height);
    assert!(
        count_drawn_pixels(&workspace_pixels) > 0,
        "the viewport's own frame should still show the plate",
    );
}

#[test]
fn a_preview_draws_the_models() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let (width, height) = (64u32, 64u32);
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 50.0]));
    renderer.upload_scene(&graph).expect("upload the triangle");
    let camera = camera_framing_the_scene(&renderer);

    let preview = renderer
        .render_preview(&graph, TEST_BV, &camera, width, height)
        .expect("preview is not refused")
        .expect("a sized preview produces a texture");

    let pixels = read_target_back(&device, &queue, &preview, width, height);
    assert!(
        count_drawn_pixels(&pixels) > 0,
        "the model should be visible in its own preview",
    );
}

/// Reads back a preview 290 pixels wide, the size a GOO file's large preview
/// fixes: 290 * 4 = 1160 bytes per row, which is not a multiple of the 256-byte
/// copy stride. The readback has to pad and unpad around that, so the pixels
/// have to come back tightly packed anyway.
#[test]
fn a_preview_at_an_unaligned_width_reads_back_tightly_packed() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let (width, height) = (290u32, 290u32);
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 50.0]));
    renderer.upload_scene(&graph).expect("upload the triangle");
    let camera = camera_framing_the_scene(&renderer);

    let frame = renderer
        .preview_pixels(&graph, TEST_BV, &camera, width, height)
        .expect("the preview is not refused")
        .expect("a sized preview produces pixels");

    assert_eq!(frame.width, width);
    assert_eq!(frame.height, height);
    assert_eq!(frame.rgba.len(), (width * height * 4) as usize);
    assert!(
        count_drawn_pixels(&frame.rgba) > 0,
        "the model should be visible in its own preview",
    );
}

/// Checks that the readback drops the padding that the copy stride inserts
/// between rows: padding left at a row boundary would show up as a black band.
#[test]
fn a_read_back_preview_has_no_padding_left_between_its_rows() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let (width, height) = (116u32, 116u32);
    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);
    let (graph, _) = graph_with_group_root();

    let frame = renderer
        .preview_pixels(&graph, TEST_BV, &camera, width, height)
        .expect("the preview is not refused")
        .expect("a sized preview produces pixels");

    // An empty plate previews as background everywhere, and the staging
    // buffer's padding is zeroed, so any padding left in would read as a fully
    // black pixel.
    assert_eq!(frame.rgba.len(), (width * height * 4) as usize);
    assert_eq!(count_drawn_pixels(&frame.rgba), 0);
}

#[test]
fn a_zero_sized_preview_produces_no_pixels() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let (centre, radius) = TEST_BV.framing(None);
    let camera = OrbitCamera::new(centre, radius);
    let (graph, _) = graph_with_group_root();

    let frame = renderer
        .preview_pixels(&graph, TEST_BV, &camera, 0, 290)
        .expect("a zero-sized preview is not an error");

    assert!(frame.is_none());
}

/// A preview overlay is drawn over the scene in its tint: a red (invalid)
/// quad in front of the camera turns pixels red that are not red without it.
#[test]
fn a_preview_overlay_is_drawn_in_its_tint() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new under the test device");
    let graph =
        triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
    renderer.upload_scene(&graph).expect("upload_scene");
    let camera = OrbitCamera::new(Vec3::new(0.0, 0.0, 5.0), 20.0);
    let frame = |renderer: &mut Renderer| {
        let texture = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: &[],
                    gizmo: None,
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("a texture");
        count_warning_pixels(&read_target_back(&device, &queue, &texture, 64, 64))
    };
    let before = frame(&mut renderer);
    // A big quad through the orbit centre, facing every way (no culling).
    let quad: Overlay = vec![Arc::new(OverlayMesh {
        positions: vec![
            [-30.0, -30.0, 5.0],
            [30.0, -30.0, 5.0],
            [30.0, 30.0, 5.0],
            [-30.0, 30.0, 5.0],
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        tint: OverlayTint::Invalid,
    })]
    .into();
    renderer.set_overlay(Some(&quad));
    let with = frame(&mut renderer);
    assert!(with > before + 500, "red pixels {before} -> {with}");
    renderer.set_overlay(None);
    assert_eq!(frame(&mut renderer), before, "cleared");
}

/// A lathed tower (base, waist, ball) about 40 mm tall standing at
/// `centre_xy`, dense enough for a realistic draw: `segments` around.
fn tower_graph(centre_xy: [f32; 2], segments: u32) -> Scene {
    let profile: [(f32, f32); 12] = [
        (0.0, 0.0),
        (13.0, 0.0),
        (13.0, 3.0),
        (10.0, 5.0),
        (7.0, 14.0),
        (6.5, 24.0),
        (8.5, 27.0),
        (9.0, 30.0),
        (7.0, 34.0),
        (5.0, 38.0),
        (2.5, 40.0),
        (0.0, 40.5),
    ];
    let mut vertices = Vec::new();
    for i in 0..segments {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        for &(r, z) in &profile {
            vertices.push(Vertex::new([
                centre_xy[0] + r * a.cos(),
                centre_xy[1] + r * a.sin(),
                z,
            ]));
        }
    }
    let n = profile.len() as u32;
    let mut faces = Vec::new();
    for i in 0..segments {
        let j = (i + 1) % segments;
        for k in 0..n - 1 {
            let (a, b) = (i * n + k, j * n + k);
            faces.push(IndexedTriangle::from_raw_indices([a, b, b + 1]));
            faces.push(IndexedTriangle::from_raw_indices([a, b + 1, a + 1]));
        }
    }
    let mesh = IndexedMesh {
        meta: None,
        vertices,
        faces,
    };
    let (mut g, root) = graph_with_group_root();
    add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
    g
}

/// The scene, camera and gizmo pivot shared by the gizmo picture and timing
/// runs: the tower on the middle of the plate, seen from the front left.
fn gizmo_showcase() -> (Scene, OrbitCamera, Vec3) {
    let lo = TEST_BV.envelope_min_mm();
    let hi = TEST_BV.envelope_max_mm();
    let centre = [(lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5];
    let graph = tower_graph(centre, 96);
    let pivot = Vec3::new(centre[0], centre[1], 20.25);
    let mut camera = OrbitCamera::new(pivot, 26.0);
    camera.yaw = -2.1;
    camera.pitch = 0.42;
    (graph, camera, pivot)
}

fn showcase_frame(
    camera: &OrbitCamera,
    pivot: Vec3,
    height: f32,
    visibility: GizmoVisibility,
    hover: Option<GizmoHover>,
    drag_visual: Option<GizmoDragVisual>,
) -> GizmoFrame {
    showcase_frame_in(
        crate::gizmo::look::GizmoLook::default(),
        camera,
        pivot,
        height,
        visibility,
        hover,
        drag_visual,
    )
}

/// [`showcase_frame`] in the gizmo style, size and tool of `look`.
fn showcase_frame_in(
    look: crate::gizmo::look::GizmoLook,
    camera: &OrbitCamera,
    pivot: Vec3,
    height: f32,
    visibility: GizmoVisibility,
    hover: Option<GizmoHover>,
    drag_visual: Option<GizmoDragVisual>,
) -> GizmoFrame {
    GizmoFrame {
        pivot,
        scale: crate::gizmo::gizmo_scale_sized(camera, pivot, default_fov_y(), height, look.size)
            .expect("pivot in front of the eye"),
        view_dir: (camera.target - camera.eye()).normalize(),
        visibility,
        hover,
        drag_visual,
        look,
        tool: None,
    }
}

/// Draws the gizmo of #208 in its states over a selected model and writes
/// PNGs to `$BREADSLICER_GIZMO_SHOTS` (default: the temp folder). Run by hand:
/// `cargo test -p sc-slicer-gui --lib gizmo_pictures -- --ignored --nocapture`.
#[test]
#[ignore = "writes pictures for a human to look at"]
fn gizmo_pictures() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let dir = std::env::var_os("BREADSLICER_GIZMO_SHOTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).expect("picture folder");
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("Renderer::new");
    let (graph, camera, pivot) = gizmo_showcase();
    renderer.upload_scene(&graph).expect("upload_scene");
    let (w, h) = (768u32, 576u32);
    let inverse_view = camera.view_matrix().inverse();
    let right = inverse_view.transform_vector3(Vec3::X).normalize();
    let up = inverse_view.transform_vector3(Vec3::Y).normalize();
    let shots: [(
        &str,
        GizmoVisibility,
        Option<GizmoHover>,
        Option<GizmoDragVisual>,
    ); 7] = [
        ("1-reposo", GizmoVisibility::Idle, None, None),
        (
            "2-mouse-encima-flecha-x",
            GizmoVisibility::Idle,
            Some(GizmoHover::Translate(Axis::X)),
            None,
        ),
        (
            "3-mouse-encima-anillo-z",
            GizmoVisibility::Idle,
            Some(GizmoHover::Rotate(Axis::Z)),
            None,
        ),
        (
            "4-girando-z-35",
            GizmoVisibility::DraggingRotate { axis: Axis::Z },
            None,
            Some(GizmoDragVisual::Rotate {
                axis: Axis::Z,
                start: -2.0,
                sweep: 35f32.to_radians(),
            }),
        ),
        (
            "5-moviendo-x",
            GizmoVisibility::DraggingTranslate { axis: Axis::X },
            None,
            None,
        ),
        (
            "6-escalando-y",
            GizmoVisibility::DraggingScaleAxis { axis: Axis::Y },
            None,
            None,
        ),
        (
            "7-r-sin-eje-anillo-de-pantalla",
            GizmoVisibility::DraggingViewRotate,
            None,
            Some(GizmoDragVisual::ViewRotate {
                right,
                up,
                start: 0.3,
                sweep: 50f32.to_radians(),
            }),
        ),
    ];
    use crate::gizmo::look::{GizmoLook, GizmoStyle, GizmoTool};
    let focused = |tool| GizmoLook {
        style: GizmoStyle::Focused,
        size: 1.0,
        tool,
    };
    let more: [(&str, GizmoLook, GizmoVisibility, Option<GizmoHover>); 7] = [
        (
            "8-mouse-encima-plano-xy",
            GizmoLook::default(),
            GizmoVisibility::Idle,
            Some(GizmoHover::Plane(Axis::Z)),
        ),
        (
            "9-moviendo-en-plano-xy",
            GizmoLook::default(),
            GizmoVisibility::DraggingPlane { normal: Axis::Z },
            None,
        ),
        (
            "10-enfocado-mover",
            focused(GizmoTool::Move),
            GizmoVisibility::Idle,
            None,
        ),
        (
            "11-enfocado-rotar",
            focused(GizmoTool::Rotate),
            GizmoVisibility::Idle,
            None,
        ),
        (
            "12-enfocado-escalar",
            focused(GizmoTool::Scale),
            GizmoVisibility::Idle,
            None,
        ),
        (
            "13-tamano-150",
            GizmoLook {
                size: 1.5,
                ..GizmoLook::default()
            },
            GizmoVisibility::Idle,
            None,
        ),
        (
            "14-tamano-75",
            GizmoLook {
                size: 0.75,
                ..GizmoLook::default()
            },
            GizmoVisibility::Idle,
            None,
        ),
    ];
    let shots = shots
        .into_iter()
        .map(|(name, v, hv, dv)| (name, GizmoLook::default(), v, hv, dv))
        .chain(
            more.into_iter()
                .map(|(name, look, v, hv)| (name, look, v, hv, None)),
        );
    for (name, look, visibility, hover, drag_visual) in shots {
        let frame = showcase_frame_in(
            look,
            &camera,
            pivot,
            h as f32,
            visibility,
            hover,
            drag_visual,
        );
        let texture = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: &[first_leaf(&graph)],
                    gizmo: Some(frame),
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                w,
                h,
            )
            .expect("render is not refused")
            .expect("a texture");
        let rgba = read_target_back(&device, &queue, &texture, w, h);
        let path = dir.join(format!("gizmo-{name}.png"));
        image::save_buffer(&path, &rgba, w, h, image::ExtendedColorType::Rgba8)
            .expect("write the picture");
        println!("{}", path.display());
    }
}

/// Times full frames of the showcase at 1920 x 1080 without the gizmo, with it
/// at rest, and while rotating (the per-frame marks), waiting for the GPU after
/// each frame. Run by hand:
/// `cargo test -p sc-slicer-gui --lib --release gizmo_frame_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark, run by hand"]
fn gizmo_frame_cost() {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new");
    let (graph, camera, pivot) = gizmo_showcase();
    renderer.upload_scene(&graph).expect("upload_scene");
    let (w, h) = (1920u32, 1080u32);
    let focused = crate::gizmo::look::GizmoLook {
        style: crate::gizmo::look::GizmoStyle::Focused,
        ..crate::gizmo::look::GizmoLook::default()
    };
    let cases: [(&str, Option<GizmoFrame>); 5] = [
        ("sin gizmo", None),
        (
            "gizmo enfocado (mover)",
            Some(showcase_frame_in(
                focused,
                &camera,
                pivot,
                h as f32,
                GizmoVisibility::Idle,
                None,
                None,
            )),
        ),
        (
            "gizmo moviendo en un plano",
            Some(showcase_frame(
                &camera,
                pivot,
                h as f32,
                GizmoVisibility::DraggingPlane { normal: Axis::Z },
                None,
                None,
            )),
        ),
        (
            "gizmo en reposo",
            Some(showcase_frame(
                &camera,
                pivot,
                h as f32,
                GizmoVisibility::Idle,
                Some(GizmoHover::Rotate(Axis::Z)),
                None,
            )),
        ),
        (
            "gizmo girando",
            Some(showcase_frame(
                &camera,
                pivot,
                h as f32,
                GizmoVisibility::DraggingRotate { axis: Axis::Z },
                None,
                Some(GizmoDragVisual::Rotate {
                    axis: Axis::Z,
                    start: 0.0,
                    sweep: 4.0,
                }),
            )),
        ),
    ];
    let mut render = |gizmo: Option<GizmoFrame>| {
        let start = std::time::Instant::now();
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: &[first_leaf(&graph)],
                    gizmo,
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                w,
                h,
            )
            .expect("render is not refused")
            .expect("a texture");
        renderer.wait_for_gpu();
        start.elapsed().as_secs_f64() * 1000.0
    };
    // Three rounds, interleaved, so clock and driver drift hit every case.
    let mut samples: Vec<Vec<f64>> = vec![Vec::new(); cases.len()];
    for _ in 0..3 {
        for (i, (_, gizmo)) in cases.iter().enumerate() {
            for _ in 0..20 {
                render(*gizmo);
            }
            for _ in 0..150 {
                samples[i].push(render(*gizmo));
            }
        }
    }
    for (i, (name, _)) in cases.iter().enumerate() {
        let s = &mut samples[i];
        s.sort_by(|a, b| a.total_cmp(b));
        let mean = s.iter().sum::<f64>() / s.len() as f64;
        let median = s[s.len() / 2];
        let p95 = s[s.len() * 95 / 100];
        println!("{name}: media {mean:.3} ms, mediana {median:.3} ms, p95 {p95:.3} ms");
    }
}

/// A flat square of `n` x `n` quads over the plate, each 4 mm, looking up.
fn paint_sheet(n: u32) -> IndexedMesh {
    let side = n as f32 * 4.0;
    let mut vertices = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            vertices.push(Vertex::new([
                x as f32 * 4.0 - side / 2.0,
                y as f32 * 4.0 - side / 2.0,
                5.0,
            ]));
        }
    }
    let at = |x: u32, y: u32| y * (n + 1) + x;
    let mut faces = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
            faces.push(IndexedTriangle::from_raw_indices([a, b, c]));
            faces.push(IndexedTriangle::from_raw_indices([a, c, d]));
        }
    }
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// A renderer over the sheet, the camera straight above it, with the sheet
/// painted on the faces `painted` picks: the sheet's left half, say.
fn painted_sheet() -> Option<(Renderer, Scene, NodeId, OrbitCamera, SurfaceTags)> {
    let (device, queue) = shared_test_device()?;
    let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
        .expect("Renderer::new under the test device");
    let mesh = Arc::new(paint_sheet(8));
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::clone(&mesh)),
        )
        .unwrap();
    scene.add_root(leaf).unwrap();
    renderer.upload_scene(&scene).expect("upload");
    let mut camera = OrbitCamera::new(Vec3::new(0.0, 0.0, 5.0), 20.0);
    camera.set_axis_view(crate::camera::AxisView::Top);
    // Left half of the sheet: the first four columns of quads, in each row.
    let values = (0..mesh.faces().len())
        .map(|f| if (f / 2) % 8 < 4 { 255 } else { 0 })
        .collect();
    let tags = SurfaceTags::new(mesh, values).unwrap();
    Some((renderer, scene, leaf, camera, tags))
}

fn frame(renderer: &mut Renderer, scene: &Scene, camera: &OrbitCamera) -> FramePixels {
    renderer
        .workspace_pixels_for_test(
            SceneRender {
                graph: scene,
                selection: &[],
                gizmo: None,
                clip_z: None,
            },
            TEST_BV,
            camera,
            128,
            128,
        )
        .expect("render is not refused")
        .expect("a frame")
}

fn pixel(frame: &FramePixels, x: u32, y: u32) -> [u8; 3] {
    let at = ((y * frame.width + x) * 4) as usize;
    [frame.rgba[at], frame.rgba[at + 1], frame.rgba[at + 2]]
}

/// The tint of the tests: red, which nothing else in the frame is.
const TEST_TINT: [f32; 4] = [1.0, 0.0, 0.0, 0.9];

#[test]
fn paint_tints_the_painted_faces_and_only_those() {
    let Some((mut renderer, scene, leaf, camera, tags)) = painted_sheet() else {
        return;
    };
    let bare = frame(&mut renderer, &scene, &camera);
    renderer.set_surface_paint(&[PaintLayer { leaf, tags }], TEST_TINT);
    let painted = frame(&mut renderer, &scene, &camera);
    // Left of the middle is painted, right of it is as it was.
    let left = pixel(&painted, 48, 64);
    let right = pixel(&painted, 80, 64);
    assert!(left[0] > left[1].saturating_add(40), "{left:?}");
    assert_eq!(right, pixel(&bare, 80, 64));
    // Gone again with the layers.
    renderer.set_surface_paint(&[], TEST_TINT);
    assert_eq!(frame(&mut renderer, &scene, &camera).rgba, bare.rgba);
}

#[test]
fn paint_uses_the_objects_vertices_and_one_buffer_per_object() {
    let Some((mut renderer, _scene, leaf, _camera, tags)) = painted_sheet() else {
        return;
    };
    let faces = tags.painted_faces() as u32;
    let layer = PaintLayer {
        leaf,
        tags: tags.clone(),
    };
    renderer.set_surface_paint(std::slice::from_ref(&layer), TEST_TINT);
    assert_eq!(renderer.surface_paint.layer_count(), 1);
    assert_eq!(renderer.surface_paint.index_count(leaf), Some(faces * 3));
    // The same paint again uploads nothing new, and blank paint draws none.
    renderer.set_surface_paint(std::slice::from_ref(&layer), TEST_TINT);
    assert_eq!(renderer.surface_paint.layer_count(), 1);
    let blank = SurfaceTags::blank(tags.mesh().clone());
    renderer.set_surface_paint(&[PaintLayer { leaf, tags: blank }], TEST_TINT);
    assert_eq!(renderer.surface_paint.layer_count(), 0);
}

#[test]
fn paint_stays_out_of_a_picture_of_the_print() {
    let Some((mut renderer, scene, leaf, camera, tags)) = painted_sheet() else {
        return;
    };
    let clean = renderer
        .preview_pixels(&scene, TEST_BV, &camera, 96, 96)
        .expect("drawn")
        .expect("a frame");
    renderer.set_surface_paint(&[PaintLayer { leaf, tags }], TEST_TINT);
    let with_paint = renderer
        .preview_pixels(&scene, TEST_BV, &camera, 96, 96)
        .expect("drawn")
        .expect("a frame");
    assert_eq!(clean.rgba, with_paint.rgba);
}

#[test]
fn the_ring_lies_on_the_surface_at_its_radius() {
    let Some((mut renderer, scene, leaf, camera, _)) = painted_sheet() else {
        return;
    };
    let bare = frame(&mut renderer, &scene, &camera);
    renderer.set_surface_paint(&[], TEST_TINT);
    renderer.set_brush_ring(Some(BrushRing {
        leaf,
        center: [0.0, 0.0, 5.0],
        radius_mm: 6.0,
        color: [1.0, 0.0, 0.0, 1.0],
    }));
    let ringed = frame(&mut renderer, &scene, &camera);
    let changed: Vec<(u32, u32)> = (0..128)
        .flat_map(|y| (0..128).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (a, b) = (pixel(&ringed, x, y), pixel(&bare, x, y));
            i32::from(a[0]) - i32::from(b[0]) > 60
        })
        .collect();
    assert!(!changed.is_empty(), "the ring shows");
    // A circle about the middle: every marked pixel at about the same
    // distance from it.
    let distance =
        |p: &(u32, u32)| (f32::from(p.0 as u16) - 63.5).hypot(f32::from(p.1 as u16) - 63.5);
    let (lo, hi) = changed.iter().fold((f32::MAX, 0.0f32), |(lo, hi), p| {
        (lo.min(distance(p)), hi.max(distance(p)))
    });
    assert!(hi - lo < 4.0, "{lo} {hi}");
    renderer.set_brush_ring(None);
    assert_eq!(frame(&mut renderer, &scene, &camera).rgba, bare.rgba);
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Integration tests that exercise the GUI's `Renderer` end-to-end
//! against a real wgpu device, including the parts the inline
//! `gpu_integration_tests` block in `mesh_render::mod` cannot reach
//! without a second device (the `install_scene` device-mismatch
//! branch) or the Slint texture surface (`slint::Image::try_from`).
//!
//! Tests soft-skip when no wgpu adapter is available, so CI without
//! a software-wgpu provider does not block.
//!
//! Concurrency note: `wgpu::Instance::new` plus `request_adapter`
//! against the same Vulkan ICD has been observed to race under
//! RADV at low test-thread counts. The shared-device helper here
//! initialises a single `(Device, Queue)` pair once via `OnceLock`;
//! every test that needs a "device A" goes through it. The
//! device-mismatch test additionally creates a second instance, but
//! it does so only after `OnceLock` initialisation has completed, so
//! the two instance creations are not concurrent.

use std::sync::Arc;
use std::sync::OnceLock;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};

use sc_slicer_gui::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME;
use sc_slicer_gui::camera::OrbitCamera;
use sc_slicer_gui::mesh_render::{DeviceToken, Renderer, SceneRender, scene_upload::GpuScene};

use slint::wgpu_29::wgpu;

fn shared_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    static DEVICE: OnceLock<Option<(wgpu::Device, wgpu::Queue)>> = OnceLock::new();
    DEVICE
        .get_or_init(|| build_fresh_device("integration-render shared device"))
        .clone()
}

fn build_fresh_device(label: &'static str) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some(label),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some((device, queue))
}

fn triangle_graph(translation: [f64; 3]) -> Scene {
    let mesh = IndexedMesh {
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
    };
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    let leaf = g
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(translation)),
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("add leaf");
    g.add_child(root, leaf).expect("attach leaf");
    g
}

fn first_leaf(graph: &Scene) -> NodeId {
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
            NodeKind::Instance { .. } => {}
        }
    }
    panic!("test fixture has no leaves");
}

/// T22: drive the renderer end-to-end and confirm the produced
/// `wgpu::Texture` is acceptable to `slint::Image::try_from`.
/// Equivalent to the production frame-handoff at the
/// `slint::Image::try_from(texture)` call in
/// `viewport::controller::handle_before_rendering`.
#[test]
fn render_texture_converts_to_slint_image() {
    let Some((device, queue)) = shared_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue);
    let graph = triangle_graph([5.0, 0.0, 0.0]);
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");

    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    // `with_detail_radius(c, r, r)` matches the single-radius
    // `OrbitCamera::new(c, r)` used by the lib's inline tests; that
    // shortcut is `#[cfg(test)]`-only and so invisible to integration
    // tests built as a separate test binary.
    let camera = OrbitCamera::with_detail_radius(center, radius, radius);

    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: Some(first_leaf(&graph)),
                gizmo: None,
                clip_z: None,
            }),
            HARDCODED_DEFAULT_BUILD_VOLUME,
            &camera,
            64,
            64,
        )
        .expect("render must produce a texture");

    let image =
        slint::Image::try_from(texture).expect("slint::Image::try_from must accept the texture");
    assert_eq!(image.size().width, 64);
    assert_eq!(image.size().height, 64);
}

/// The slice clip path renders without crashing: a `SceneRender` with a
/// clip height set still produces a texture, so the mesh shader's clip
/// discard compiles and runs.
#[test]
fn render_with_a_slice_clip_produces_a_texture() {
    let Some((device, queue)) = shared_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue);
    let graph = triangle_graph([5.0, 0.0, 0.0]);
    renderer
        .upload_scene(&graph)
        .expect("upload_scene under default limits");

    let bounds = renderer.scene_bounds().expect("scene bounds populated");
    let center = (bounds.0 + bounds.1) * 0.5;
    let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
    let camera = OrbitCamera::with_detail_radius(center, radius, radius);

    let texture = renderer
        .render(
            Some(SceneRender {
                graph: &graph,
                selection: None,
                gizmo: None,
                clip_z: Some(0.5),
            }),
            HARDCODED_DEFAULT_BUILD_VOLUME,
            &camera,
            64,
            64,
        )
        .expect("render must produce a texture with a clip set");
    assert_eq!(texture.size().width, 64);
    assert_eq!(texture.size().height, 64);
}

/// T23: a `GpuScene` built against device A must not install on a
/// `Renderer` built against device B. The renderer must report
/// `false` (the caller's fallback path in `mesh_loader` then
/// triggers a recovery upload).
#[test]
fn install_scene_with_mismatched_device_returns_false() {
    // Device A: the shared one. Initialise it first so it stops
    // racing with the second instance below.
    let Some((device_a, _queue_a)) = shared_device() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };

    // Device B: a fresh instance + adapter + device. With `device_a`
    // already initialised via OnceLock, the two instance creations
    // are not concurrent and RADV does not race.
    let Some((device_b, queue_b)) = build_fresh_device("integration-render device B") else {
        eprintln!("skipping: could not build a second wgpu device");
        return;
    };

    // Two distinct tokens, one per device. install_scene checks
    // `self.device_token == gpu.device_token()`, so a mismatch must
    // surface even though both tokens are syntactically `u64`s.
    let token_a = DeviceToken::next();
    let token_b = DeviceToken::next();

    let mut renderer = Renderer::new(device_b.clone(), token_b, queue_b);
    let graph = triangle_graph([0.0, 0.0, 0.0]);
    let gpu = GpuScene::build(&device_a, token_a, &graph).expect("GpuScene::build on device A");

    let installed = renderer.install_scene(gpu);
    assert!(
        !installed,
        "renderer was built with device B; a GpuScene from device A must be rejected"
    );
    assert!(
        renderer.scene_bounds().is_none(),
        "no scene should have been attached after a rejected install"
    );
}

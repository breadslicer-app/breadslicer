/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Integration tests that exercise the GUI's `Renderer` end-to-end against real
//! wgpu devices, including the parts the `mesh_render::tests_gpu` module cannot
//! reach without a second device (the `install_scene` device-mismatch branch)
//! or the Slint texture surface (`slint::Image::try_from`).
//!
//! The render tests run once per adapter wgpu reports, so on a machine with
//! several GPUs the renderer is checked against every one of them. Tests
//! soft-skip when no adapter is available, so CI without a software-wgpu
//! provider does not block.
//!
//! Concurrency note: the tests take turns on the GPU. `with_test_gpus`
//! serialises every wgpu call this file makes, opens 1 device per adapter for
//! the length of a single test, and closes them again before the next test gets
//! its turn. 2 crashes, each a SIGSEGV on a few percent of runs, make this
//! necessary. Building a `wgpu::Instance` on 1 thread while another thread has
//! work in flight through an existing device can make the Vulkan loader read
//! through a torn pointer inside `vkSetDebugUtilsObjectNameEXT`. Devices held
//! open on several drivers until the process exits can jump through a dangling
//! dispatch pointer at teardown. No GPU object may outlive the turn that opened
//! it.

use std::sync::Arc;
use std::sync::Mutex;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, NodeId, NodeKind};

use sc_slicer_gui::build_volume::NO_PRINTER_BUILD_VOLUME;
use sc_slicer_gui::camera::OrbitCamera;
use sc_slicer_gui::mesh_render::{
    DeviceLoss, DeviceToken, Renderer, SceneRender, scene_upload::GpuScene,
};

use slint::wgpu_30::wgpu;

/// Backends the sweep covers: everything wgpu was compiled with except the GL
/// fallback, which the GUI does not ship on. `Backends::all()` also
/// names the no-op backend, but that one stays inert unless it is switched on
/// through `BackendOptions`, which this file never does.
const SWEEP_BACKENDS: wgpu::Backends = wgpu::Backends::all().difference(wgpu::Backends::GL);

/// An adapter opened for the sweep, carrying a readable name so a failing
/// assertion says which GPU produced it.
struct TestGpu {
    label: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// The devices a single test runs against.
struct TestGpus {
    /// 1 open device per adapter, in enumeration order.
    sweep: Vec<TestGpu>,
    /// Declared after `sweep` so it is dropped once every device built from it
    /// is already gone.
    instance: wgpu::Instance,
}

/// Grants 1 test at a time the right to call into wgpu.
static GPU_TURN: Mutex<()> = Mutex::new(());

/// Runs `body` against 1 freshly opened device per adapter, with no other test
/// touching the GPU meanwhile, then closes them again. `body` does not run when
/// the machine has no usable adapter.
fn with_test_gpus(body: impl FnOnce(&TestGpus)) {
    // A test that panics poisons the lock. The failure is already being
    // reported on its own thread, and the remaining tests still need their
    // turn, so the poison itself is not interesting here.
    let _turn = GPU_TURN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let gpus = TestGpus::open();
    if gpus.sweep.is_empty() {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    }

    eprintln!("render sweep covers {} adapter(s):", gpus.sweep.len());
    for gpu in &gpus.sweep {
        eprintln!("  {}", gpu.label);
    }

    body(&gpus);
    gpus.close();
}

impl TestGpus {
    fn open() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: SWEEP_BACKENDS,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let sweep = adapters(&instance)
            .into_iter()
            .filter_map(|adapter| {
                let info = adapter.get_info();
                let label = format!("{} on {:?}", info.name, info.backend);
                let (device, queue) = open_device(&adapter, &label)?;
                Some(TestGpu {
                    label,
                    device,
                    queue,
                })
            })
            .collect();
        Self { sweep, instance }
    }

    /// Opens a device independent of every entry in `sweep`, for the
    /// device-mismatch test. It comes from a freshly enumerated `Adapter`
    /// rather than from one the sweep already used: WebGPU gives an adapter a
    /// single device, and wgpu only declines to enforce that.
    fn open_spare_device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        let adapter = adapters(&self.instance).into_iter().next()?;
        open_device(&adapter, "integration-render spare device")
    }

    /// Lets every queue drain before the devices and then the instance are
    /// destroyed, so nothing is still in flight at teardown.
    fn close(self) {
        for gpu in &self.sweep {
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        }
    }
}

fn adapters(instance: &wgpu::Instance) -> Vec<wgpu::Adapter> {
    pollster::block_on(instance.enumerate_adapters(SWEEP_BACKENDS))
}

/// Opens a device with the default-limits profile Slint requests. Returns
/// `None` for an adapter that cannot satisfy it, which leaves that adapter out
/// of the sweep instead of failing the run.
fn open_device(adapter: &wgpu::Adapter, label: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some(label),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()
}

fn triangle_graph(translation: [f64; 3]) -> Scene {
    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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
            NodeKind::Reference { .. } => {}
        }
    }
    panic!("test fixture has no leaves");
}

/// Drives the renderer end-to-end and checks that `slint::Image::try_from`
/// accepts the produced `wgpu::Texture`, as the production frame handoff in
/// `viewport::controller::on_paint` needs.
#[test]
fn render_texture_converts_to_slint_image() {
    with_test_gpus(|gpus| {
        for gpu in &gpus.sweep {
            let mut renderer = Renderer::new(
                gpu.device.clone(),
                DeviceToken::next(),
                gpu.queue.clone(),
                DeviceLoss::default(),
            )
            .expect("Renderer::new under the test device");
            let graph = triangle_graph([5.0, 0.0, 0.0]);
            renderer.upload_scene(&graph).unwrap_or_else(|err| {
                panic!(
                    "upload_scene under default limits on {}: {err:?}",
                    gpu.label
                )
            });

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
                        selection: &[first_leaf(&graph)],
                        gizmo: None,
                        clip_z: None,
                    }),
                    NO_PRINTER_BUILD_VOLUME,
                    &camera,
                    64,
                    64,
                )
                .unwrap_or_else(|err| panic!("render must not be refused on {}: {err}", gpu.label))
                .unwrap_or_else(|| panic!("render must produce a texture on {}", gpu.label));

            let image = slint::Image::try_from(texture).unwrap_or_else(|err| {
                panic!(
                    "slint::Image::try_from must accept the texture from {}: {err:?}",
                    gpu.label
                )
            });
            assert_eq!(image.size().width, 64, "on {}", gpu.label);
            assert_eq!(image.size().height, 64, "on {}", gpu.label);
        }
    });
}

/// Checks that the slice clip path renders without crashing: a `SceneRender`
/// with a clip height set still produces a texture, so the mesh shader's clip
/// discard compiles and runs.
#[test]
fn render_with_a_slice_clip_produces_a_texture() {
    with_test_gpus(|gpus| {
        for gpu in &gpus.sweep {
            let mut renderer = Renderer::new(
                gpu.device.clone(),
                DeviceToken::next(),
                gpu.queue.clone(),
                DeviceLoss::default(),
            )
            .expect("Renderer::new under the test device");
            let graph = triangle_graph([5.0, 0.0, 0.0]);
            renderer.upload_scene(&graph).unwrap_or_else(|err| {
                panic!(
                    "upload_scene under default limits on {}: {err:?}",
                    gpu.label
                )
            });

            let bounds = renderer.scene_bounds().expect("scene bounds populated");
            let center = (bounds.0 + bounds.1) * 0.5;
            let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
            let camera = OrbitCamera::with_detail_radius(center, radius, radius);

            let texture = renderer
                .render(
                    Some(SceneRender {
                        graph: &graph,
                        selection: &[],
                        gizmo: None,
                        clip_z: Some(0.5),
                    }),
                    NO_PRINTER_BUILD_VOLUME,
                    &camera,
                    64,
                    64,
                )
                .unwrap_or_else(|err| panic!("render must not be refused on {}: {err}", gpu.label))
                .unwrap_or_else(|| {
                    panic!(
                        "render must produce a texture with a clip set on {}",
                        gpu.label
                    )
                });
            assert_eq!(texture.size().width, 64, "on {}", gpu.label);
            assert_eq!(texture.size().height, 64, "on {}", gpu.label);
        }
    });
}

/// Checks that a `GpuScene` built against device A does not install on a
/// `Renderer` built against device B: the renderer reports `false`, and its
/// caller then falls back to the recovery upload.
#[test]
fn install_scene_with_mismatched_device_returns_false() {
    with_test_gpus(|gpus| {
        // Device A: the sweep's first device. Device B: a spare one opened for
        // this test. Both belong to this test's turn, so nothing else is
        // calling into wgpu meanwhile.
        let device_a = &gpus.sweep[0].device;
        let Some((device_b, queue_b)) = gpus.open_spare_device() else {
            eprintln!("skipping: could not open a second wgpu device");
            return;
        };

        // 2 distinct tokens, 1 per device. `install_scene` compares
        // `self.core.device_token` with `gpu.device_token()`, so the mismatch
        // makes it return `false`.
        let token_a = DeviceToken::next();
        let token_b = DeviceToken::next();

        let mut renderer = Renderer::new(device_b.clone(), token_b, queue_b, DeviceLoss::default())
            .expect("Renderer::new under the test device");
        let graph = triangle_graph([0.0, 0.0, 0.0]);
        let gpu = GpuScene::build(device_a, token_a, &graph, &DeviceLoss::default())
            .expect("GpuScene::build on device A");

        let installed = renderer.install_scene(gpu);
        assert!(
            !installed,
            "renderer was built with device B; a GpuScene from device A must be rejected"
        );
        assert!(
            renderer.scene_bounds().is_none(),
            "no scene should have been attached after a rejected install"
        );

        // Closed before the turn ends, like every device in the sweep.
        let _ = device_b.poll(wgpu::PollType::wait_indefinitely());
    });
}

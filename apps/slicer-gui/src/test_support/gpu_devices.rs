/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use slint::wgpu_30::wgpu;

use crate::mesh_render::DeviceLoss;

/// `max_buffer_size` of the small-buffer test device.
pub(crate) const SMALL_BUFFER_LIMIT: u64 = 1024;
/// `max_buffer_size` of the tiny-buffer test device: below every static buffer
/// the renderer uploads, so each of them is refused.
const TINY_BUFFER_LIMIT: u64 = 64;
/// `max_texture_dimension_2d` of the small-texture test device.
pub(crate) const SMALL_TEXTURE_LIMIT: u32 = 256;

#[derive(Clone)]
struct TestDevices {
    default_limits: (wgpu::Device, wgpu::Queue),
    small_buffers: (wgpu::Device, wgpu::Queue),
    tiny_buffers: (wgpu::Device, wgpu::Queue),
    small_textures: (wgpu::Device, wgpu::Queue),
    /// `max_bind_groups: 1`: every pipeline with 2 bind groups is refused at
    /// its layout.
    one_bind_group: (wgpu::Device, wgpu::Queue),
    /// Destroyed once per process by [`destroyed_test_device`], for the tests
    /// that need a lost device; no other test may use it.
    doomed: (wgpu::Device, wgpu::Queue),
    /// Gets the uncaptured-error logger installed by the only test that
    /// exercises it; no other test may use it. Small-buffer limits, so an
    /// unscoped oversize buffer is refused.
    logged: (wgpu::Device, wgpu::Queue),
}

/// Builds the test devices once per process, on 1 adapter, 1 request after the
/// other. Returns `None` if no adapter is available or a device request fails;
/// the caller soft-skips so CI without a GPU isn't blocked.
///
/// The GPU tests share this adapter: concurrent `wgpu::Instance::new` plus
/// `request_adapter` calls against the same Vulkan ICD raced and crashed RADV,
/// reliably at `--test-threads=2` and intermittently at higher parallelism.
/// wgpu's Device and Queue are `Send + Sync` and reference-counted, so cloning
/// them out is cheap and concurrent test access is fine.
fn shared_test_devices() -> Option<TestDevices> {
    static DEVICES: std::sync::OnceLock<Option<TestDevices>> = std::sync::OnceLock::new();
    DEVICES
        .get_or_init(|| {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = pollster::block_on(
                instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
            )
            .ok()?;
            let request = |label: &'static str, required_limits: wgpu::Limits| {
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some(label),
                    required_features: wgpu::Features::empty(),
                    required_limits,
                    memory_hints: wgpu::MemoryHints::default(),
                    experimental_features: wgpu::ExperimentalFeatures::default(),
                    trace: wgpu::Trace::Off,
                }))
                .ok()
            };
            // Default limits are the base profile `run()` asks for.
            let default_limits = request("mesh-viewer test device", wgpu::Limits::default())?;
            let small_buffers = request(
                "mesh-viewer small-buffer test device",
                wgpu::Limits {
                    max_buffer_size: SMALL_BUFFER_LIMIT,
                    ..wgpu::Limits::default()
                },
            )?;
            let tiny_buffers = request(
                "mesh-viewer tiny-buffer test device",
                wgpu::Limits {
                    max_buffer_size: TINY_BUFFER_LIMIT,
                    ..wgpu::Limits::default()
                },
            )?;
            let small_textures = request(
                "mesh-viewer small-texture test device",
                wgpu::Limits {
                    max_texture_dimension_2d: SMALL_TEXTURE_LIMIT,
                    ..wgpu::Limits::default()
                },
            )?;
            let one_bind_group = request(
                "mesh-viewer one-bind-group test device",
                wgpu::Limits {
                    max_bind_groups: 1,
                    ..wgpu::Limits::default()
                },
            )?;
            let doomed = request("mesh-viewer doomed test device", wgpu::Limits::default())?;
            let logged = request(
                "mesh-viewer logged test device",
                wgpu::Limits {
                    max_buffer_size: SMALL_BUFFER_LIMIT,
                    ..wgpu::Limits::default()
                },
            )?;
            Some(TestDevices {
                default_limits,
                small_buffers,
                tiny_buffers,
                small_textures,
                one_bind_group,
                doomed,
                logged,
            })
        })
        .clone()
}

/// Returns the default-limits test device.
pub(crate) fn shared_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    shared_test_devices().map(|d| d.default_limits)
}

/// Returns the test device whose `max_buffer_size` is [`SMALL_BUFFER_LIMIT`].
pub(crate) fn shared_small_buffer_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    shared_test_devices().map(|d| d.small_buffers)
}

/// Returns the test device whose `max_buffer_size` is [`TINY_BUFFER_LIMIT`].
pub(crate) fn shared_tiny_buffer_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    shared_test_devices().map(|d| d.tiny_buffers)
}

/// Returns the test device whose `max_texture_dimension_2d` is
/// [`SMALL_TEXTURE_LIMIT`].
pub(crate) fn shared_small_texture_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    shared_test_devices().map(|d| d.small_textures)
}

/// Returns the test device whose `max_bind_groups` is 1.
pub(crate) fn shared_one_bind_group_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    shared_test_devices().map(|d| d.one_bind_group)
}

/// Returns the test device reserved for the error-logger test.
pub(crate) fn logged_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    shared_test_devices().map(|d| d.logged)
}

/// Returns the doomed test device, destroyed once per process with a lost
/// callback that marks the returned flag; every caller sees the same destroyed
/// device and the same marked flag.
pub(crate) fn destroyed_test_device() -> Option<(wgpu::Device, DeviceLoss)> {
    static DESTROYED: std::sync::OnceLock<Option<(wgpu::Device, DeviceLoss)>> =
        std::sync::OnceLock::new();
    DESTROYED
        .get_or_init(|| {
            let (device, _queue) = shared_test_devices()?.doomed;
            let loss = DeviceLoss::default();
            let marker = loss.clone();
            device.set_device_lost_callback(move |reason, message| {
                marker.mark(format!("{reason:?}: {message}"));
            });
            device.destroy();
            // The lost callback fires from the poll that follows destroy().
            let _ = device.poll(wgpu::PollType::Poll);
            Some((device, loss))
        })
        .clone()
}

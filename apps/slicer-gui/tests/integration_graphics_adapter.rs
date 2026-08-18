/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Integration tests for `graphics_adapter` against the real adapters
//! the machine exposes. The ordering policy itself is covered by the
//! pure tests inside the module; what needs real hardware is everything
//! those cannot see: that enumeration finds the adapters, that the
//! ordering survives contact with a driver, that the device the caller
//! gets back can actually be used, and that the environment override
//! reaches a specific adapter.
//!
//! Concurrency note: the same rule as `integration_render.rs` applies
//! here. Every wgpu call this file makes happens while holding
//! `GPU_TURN`, and nothing GPU-shaped outlives the turn that opened it.
//! Building a `wgpu::Instance` on one thread while another thread has
//! work in flight through an existing device has been observed to kill
//! the process with SIGSEGV inside the driver loader.
//!
//! Tests soft-skip when the machine has no usable adapter, so a
//! headless environment without a software wgpu provider does not block
//! the suite.

use std::sync::Mutex;

use sc_slicer_gui::graphics_adapter::{
    AdapterFacts, AdapterSelection, rank_adapters, select_adapter_and_device,
    select_adapter_and_device_with,
};

use slint::wgpu_29::wgpu;

/// The single backend Slint's Skia renderer accepts on this platform,
/// mirroring the selection `run()` makes. Testing the adapter choice on
/// any other backend would exercise a combination the application never
/// asks for.
#[cfg(target_family = "windows")]
const PLATFORM_BACKENDS: wgpu::Backends = wgpu::Backends::DX12;
#[cfg(all(target_family = "unix", not(target_vendor = "apple")))]
const PLATFORM_BACKENDS: wgpu::Backends = wgpu::Backends::VULKAN;
#[cfg(target_vendor = "apple")]
const PLATFORM_BACKENDS: wgpu::Backends = wgpu::Backends::METAL;

/// Grants one test at a time the right to call into wgpu.
static GPU_TURN: Mutex<()> = Mutex::new(());

/// The device profile `run()` asks for. The adapter choice is only
/// meaningful against the same limits the application requests, since
/// those limits are what an adapter can refuse.
fn device_descriptor(label: &str) -> wgpu::DeviceDescriptor<'_> {
    wgpu::DeviceDescriptor {
        label: Some(label),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }
}

/// Runs `body` with exclusive access to the GPU and an instance
/// restricted to the platform backend. `body` receives the instance and
/// the adapters it enumerated, and does not run when the machine has
/// none.
fn with_gpu_turn(body: impl FnOnce(&wgpu::Instance, &[wgpu::Adapter])) {
    // A panicking test poisons the lock. That failure is already being
    // reported on its own thread and the remaining tests still need
    // their turn, so the poison itself is not interesting here.
    let _turn = GPU_TURN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: PLATFORM_BACKENDS,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapters = pollster::block_on(instance.enumerate_adapters(PLATFORM_BACKENDS));
    if adapters.is_empty() {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    }

    eprintln!("adapter selection sees {} adapter(s):", adapters.len());
    for info in adapters.iter().map(|adapter| adapter.get_info()) {
        eprintln!(
            "  {} ({:?}) on {:?}",
            info.name, info.device_type, info.backend
        );
    }

    body(&instance, &adapters);
}

/// Ranked order over `adapters` under `preferred_name`, computed the
/// same way the module does but from the test's own enumeration.
fn ranked_order(infos: &[wgpu::AdapterInfo], preferred_name: Option<&str>) -> Vec<usize> {
    let candidates: Vec<AdapterFacts<'_>> = infos
        .iter()
        .map(|info| AdapterFacts {
            name: &info.name,
            device_type: info.device_type,
        })
        .collect();
    rank_adapters(&candidates, preferred_name)
}

fn infos_of(adapters: &[wgpu::Adapter]) -> Vec<wgpu::AdapterInfo> {
    adapters.iter().map(|adapter| adapter.get_info()).collect()
}

/// Runtime power-management state of a PCI device, read from sysfs.
/// `None` when the platform has no such interface, the adapter did
/// not report a bus id, or the file cannot be read.
///
/// This exists to attribute transient device refusals: laptop
/// discrete GPUs runtime-suspend, and a request that fails while its
/// device reads `suspended` or `resuming` points at a wake failure
/// rather than a refusal of the requested profile.
fn runtime_power_status(pci_bus_id: &str) -> Option<String> {
    if pci_bus_id.is_empty() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let path = format!("/sys/bus/pci/devices/{pci_bus_id}/power/runtime_status");
        std::fs::read_to_string(path)
            .ok()
            .map(|status| status.trim().to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[test]
fn runtime_power_status_is_none_without_a_readable_device() {
    assert_eq!(runtime_power_status(""), None);
    assert_eq!(runtime_power_status("no-such-pci-device"), None);
}

/// True when `adapter` accepts the application's device profile. The
/// device is drained and dropped before returning, so probing an
/// adapter leaves nothing in flight.
///
/// A refusal is printed with its error and the device's power state
/// before the request and at the refusal: this probe has been seen to
/// fail transiently on hardware that accepts a device moments later,
/// and those details are the only way to attribute such a run.
fn can_open_device(adapter: &wgpu::Adapter) -> bool {
    let info = adapter.get_info();
    let power_before = runtime_power_status(&info.device_pci_bus_id);
    let (device, _queue) = match pollster::block_on(
        adapter.request_device(&device_descriptor("graphics-adapter probe device")),
    ) {
        Ok(pair) => pair,
        Err(error) => {
            let power_at_refusal = runtime_power_status(&info.device_pci_bus_id);
            eprintln!(
                "  probe: {} refused a device request: {error} \
                 (power state before: {}, at refusal: {})",
                info.name,
                power_before.as_deref().unwrap_or("unknown"),
                power_at_refusal.as_deref().unwrap_or("unknown"),
            );
            return false;
        }
    };
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    true
}

/// The adapter the module is expected to settle on: the first in ranked
/// order that accepts the device profile. Expressed independently of
/// the selection under test so a wrong choice is visible rather than
/// mirrored.
fn expected_choice(
    adapters: &[wgpu::Adapter],
    infos: &[wgpu::AdapterInfo],
    preferred_name: Option<&str>,
) -> Option<wgpu::AdapterInfo> {
    ranked_order(infos, preferred_name)
        .into_iter()
        .find(|&index| can_open_device(&adapters[index]))
        .map(|index| infos[index].clone())
}

#[test]
fn selection_returns_the_top_ranked_adapter_that_opens_a_device() {
    with_gpu_turn(|instance, adapters| {
        let infos = infos_of(adapters);
        let Some(expected) = expected_choice(adapters, &infos, None) else {
            eprintln!("skipping: no adapter accepts the application device profile");
            return;
        };

        let AdapterSelection {
            adapter, device, ..
        } = select_adapter_and_device(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter selection test"),
        )
        .expect("an adapter that opens a device must be selected");

        assert_eq!(
            adapter.get_info(),
            expected,
            "selection must settle on the top-ranked adapter that opens a device"
        );
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    });
}

#[test]
fn a_software_adapter_is_never_chosen_while_a_gpu_can_serve() {
    // The behaviour that matters most: a machine with a real GPU must
    // not end up rendering through a software adapter, whatever order
    // the platform enumerated them in.
    with_gpu_turn(|instance, adapters| {
        let infos = infos_of(adapters);
        let gpu_available = infos.iter().zip(adapters.iter()).any(|(info, adapter)| {
            matches!(
                info.device_type,
                wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu
            ) && can_open_device(adapter)
        });
        if !gpu_available {
            eprintln!("skipping: this environment exposes no usable hardware GPU");
            return;
        }

        let AdapterSelection {
            adapter, device, ..
        } = select_adapter_and_device(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter software-avoidance test"),
        )
        .expect("a usable GPU is present, so selection must succeed");

        let chosen = adapter.get_info();
        assert_ne!(
            chosen.device_type,
            wgpu::DeviceType::Cpu,
            "picked the software adapter {} while a hardware GPU was usable",
            chosen.name
        );
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    });
}

#[test]
fn a_discrete_gpu_wins_over_the_rest_when_it_can_serve() {
    with_gpu_turn(|instance, adapters| {
        let infos = infos_of(adapters);
        let discrete_usable = infos.iter().zip(adapters.iter()).any(|(info, adapter)| {
            info.device_type == wgpu::DeviceType::DiscreteGpu && can_open_device(adapter)
        });
        if !discrete_usable {
            eprintln!("skipping: this environment exposes no usable discrete GPU");
            return;
        }

        let AdapterSelection {
            adapter, device, ..
        } = select_adapter_and_device(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter discrete-preference test"),
        )
        .expect("a usable discrete GPU is present, so selection must succeed");

        let chosen = adapter.get_info();
        assert_eq!(
            chosen.device_type,
            wgpu::DeviceType::DiscreteGpu,
            "chose {} ({:?}) while a usable discrete GPU was available",
            chosen.name,
            chosen.device_type
        );
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    });
}

#[test]
fn the_selected_device_can_carry_gpu_work() {
    // A selected adapter that cannot run a submission would satisfy
    // every ordering assertion above and still leave the viewport
    // blank, which is the symptom this whole module exists to prevent.
    with_gpu_turn(|instance, _adapters| {
        let Ok(AdapterSelection { device, queue, .. }) = select_adapter_and_device(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter usability test"),
        ) else {
            eprintln!("skipping: no adapter accepts the application device profile");
            return;
        };

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("graphics-adapter usability buffer"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &[7u8; 256]);
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("graphics-adapter usability encoder"),
        });
        queue.submit(Some(encoder.finish()));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the selected device must complete a submission");
    });
}

#[test]
fn the_selected_device_takes_the_adapters_buffer_ceiling() {
    with_gpu_turn(|instance, _adapters| {
        let Ok(AdapterSelection {
            adapter, device, ..
        }) = select_adapter_and_device(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter buffer ceiling test"),
        )
        else {
            eprintln!("skipping: no adapter accepts the application device profile");
            return;
        };

        let limits = device.limits();
        assert_eq!(
            limits.max_buffer_size,
            adapter.limits().max_buffer_size,
            "the device's buffer ceiling must be the adapter's, not the default profile's"
        );
        let expected_rest = wgpu::Limits {
            max_buffer_size: limits.max_buffer_size,
            ..wgpu::Limits::default()
        };
        assert_eq!(
            limits, expected_rest,
            "every other limit stays at the default profile's value"
        );
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    });
}

#[test]
fn an_override_naming_a_present_adapter_selects_that_adapter() {
    with_gpu_turn(|instance, adapters| {
        let infos = infos_of(adapters);
        // Name the last-ranked adapter that can still open a device, so
        // the assertion fails if the override is ignored and the
        // default ordering wins instead.
        let order = ranked_order(&infos, None);
        let Some(target) = order
            .into_iter()
            .rev()
            .find(|&index| can_open_device(&adapters[index]))
            .map(|index| infos[index].clone())
        else {
            eprintln!("skipping: no adapter accepts the application device profile");
            return;
        };

        let AdapterSelection {
            adapter, device, ..
        } = select_adapter_and_device_with(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter override test"),
            Some(&target.name),
        )
        .expect("the named adapter opens a device, so selection must succeed");

        assert_eq!(
            adapter.get_info().name,
            target.name,
            "the override must reach the adapter it names"
        );
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    });
}

#[test]
fn an_override_matching_nothing_falls_back_to_the_normal_choice() {
    // A stale or misspelled variable must not break the launch, which
    // is the failure mode wgpu's own env helper has.
    with_gpu_turn(|instance, adapters| {
        let infos = infos_of(adapters);
        let Some(expected) = expected_choice(adapters, &infos, None) else {
            eprintln!("skipping: no adapter accepts the application device profile");
            return;
        };

        let AdapterSelection {
            adapter, device, ..
        } = select_adapter_and_device_with(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter unmatched-override test"),
            Some("no adapter is called this"),
        )
        .expect("an unmatched override must not prevent selection");

        assert_eq!(
            adapter.get_info(),
            expected,
            "an unmatched override must leave the normal choice untouched"
        );
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    });
}

#[test]
fn selection_reports_an_error_when_no_backend_exposes_an_adapter() {
    // Deterministic on every machine, GPU or not: an empty backend set
    // enumerates nothing, and the caller has to be told rather than
    // handed a device that cannot exist.
    let _turn = GPU_TURN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::empty(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let error = select_adapter_and_device(
        &instance,
        wgpu::Backends::empty(),
        &device_descriptor("graphics-adapter empty-backends test"),
    )
    .expect_err("an empty backend set exposes no adapter");
    assert!(
        error
            .to_string()
            .contains("No graphics adapter is available"),
        "unexpected error message: {error}"
    );
}

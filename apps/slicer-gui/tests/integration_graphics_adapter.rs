/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Integration tests for `graphics_adapter` against the real adapters the
//! machine exposes. The ordering policy itself is covered by the pure tests
//! inside the module; what needs real hardware is everything those cannot see:
//! that enumeration finds the adapters, that the ordering survives contact with
//! a driver, that the device the caller gets back can actually be used, and
//! that the environment override reaches a specific adapter.
//!
//! Concurrency note: the same rule as `integration_render.rs` applies here.
//! Every wgpu call this file makes happens while holding `GPU_TURN`, and
//! nothing GPU-shaped outlives the turn that opened it. Building a
//! `wgpu::Instance` on a thread while another thread has work in flight through
//! an existing device has been observed to kill the process with SIGSEGV inside
//! the driver loader.
//!
//! Tests soft-skip when the machine has no usable adapter, so a headless
//! environment without a software wgpu provider does not block the suite.

use std::fmt::Debug;
use std::sync::Mutex;

use sc_slicer_gui::graphics_adapter::{
    AdapterFacts, AdapterSelection, rank_adapters, select_adapter_and_device,
    select_adapter_and_device_with,
};

use slint::wgpu_30::wgpu;

/// The single backend Slint's Skia renderer accepts on this platform, mirroring
/// the selection `run()` makes. Testing the adapter choice on any other backend
/// would exercise a combination the application never asks for.
#[cfg(target_family = "windows")]
const PLATFORM_BACKENDS: wgpu::Backends = wgpu::Backends::DX12;
#[cfg(all(target_family = "unix", not(target_vendor = "apple")))]
const PLATFORM_BACKENDS: wgpu::Backends = wgpu::Backends::VULKAN;
#[cfg(target_vendor = "apple")]
const PLATFORM_BACKENDS: wgpu::Backends = wgpu::Backends::METAL;

/// Grants 1 test at a time the right to call into wgpu.
static GPU_TURN: Mutex<()> = Mutex::new(());

/// The device profile `run()` asks for. The adapter choice is only meaningful
/// against the same limits the application requests, since those limits are
/// what an adapter can refuse.
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

/// Runs `body` with exclusive access to the GPU and an instance restricted to
/// the platform backend. `body` receives the instance and the adapters it
/// enumerated, and does not run when the machine has none. Returns what `body`
/// returns, or `None` when `body` did not run.
fn with_gpu_turn<T>(body: impl FnOnce(&wgpu::Instance, &[wgpu::Adapter]) -> T) -> Option<T> {
    // A panicking test poisons the lock. That failure is already being reported
    // on its own thread and the remaining tests still need their turn, so the
    // poison itself is not interesting here.
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
        return None;
    }

    eprintln!("adapter selection sees {} adapter(s):", adapters.len());
    for info in adapters.iter().map(|adapter| adapter.get_info()) {
        eprintln!(
            "  {} ({:?}) on {:?}",
            info.name, info.device_type, info.backend
        );
    }

    Some(body(&instance, &adapters))
}

/// Ranked order over `adapters` under `preferred_name`, computed the same way
/// the module does but from the test's own enumeration.
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

/// Runtime power-management state of a PCI device, read from sysfs. `None` when
/// the platform has no such interface, the adapter did not report a bus id, or
/// the file cannot be read.
///
/// This exists to attribute transient device refusals: laptop discrete GPUs
/// runtime-suspend, and a request that fails while its device reads `suspended`
/// or `resuming` points at a wake failure rather than a refusal of the
/// requested profile.
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

/// True when `adapter` accepts the application's device profile. The device is
/// drained and dropped before returning, so probing an adapter leaves nothing
/// in flight.
///
/// A refusal is printed with its error and the device's power state before the
/// request and at the refusal: this probe has been seen to fail transiently on
/// hardware that accepts a device moments later, and those details are the only
/// way to attribute such a run.
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

/// Reports whether `probe` accepts within 2 attempts. A refusal of the first
/// attempt can be transient, so `probe` then runs 1 more time.
fn accepts_within_2_probes(mut probe: impl FnMut() -> bool) -> bool {
    probe() || probe()
}

/// Runs `accepts_within_2_probes` over the scripted `answers`. Returns its
/// answer and the number of probes that it made.
fn run_probe_script(answers: &[bool]) -> (bool, usize) {
    let mut answers = answers.iter().copied();
    let mut probes_made = 0;
    let accepted = accepts_within_2_probes(|| {
        probes_made += 1;
        answers
            .next()
            .expect("the script must hold an answer for each probe")
    });
    (accepted, probes_made)
}

#[test]
fn a_probe_that_accepts_is_not_repeated() {
    assert_eq!(run_probe_script(&[true, false]), (true, 1));
}

#[test]
fn a_refused_probe_is_repeated_1_time() {
    assert_eq!(run_probe_script(&[false, true]), (true, 2));
}

#[test]
fn a_probe_refused_2_times_is_not_repeated_again() {
    assert_eq!(run_probe_script(&[false, false, true]), (false, 2));
}

/// Returns the place in `order` of the adapter that the module is expected to
/// settle on: the first that accepts the device profile. The answer comes from
/// the requests of the test, so it does not depend on the selection under test.
fn expected_choice(adapters: &[wgpu::Adapter], order: &[usize]) -> Option<usize> {
    order
        .iter()
        .position(|&index| can_open_device(&adapters[index]))
}

/// Returns the lines that report the refusals of a selection: 1 for each
/// adapter that it walked past, in order, and 1 for a refusal that `winner`
/// recovered from.
fn refusal_lines<S: Debug, R: Debug>(
    winner: &str,
    skipped: &[S],
    recovery: Option<&R>,
) -> Vec<String> {
    let skipped = skipped
        .iter()
        .map(|skipped| format!("  selection: walked past {skipped:?}"));
    let recovery =
        recovery.map(|recovery| format!("  selection: {winner} recovered from {recovery:?}"));
    skipped.chain(recovery).collect()
}

fn print_refusals(selection: &AdapterSelection) {
    let winner = selection.adapter.get_info().name;
    for line in refusal_lines(&winner, &selection.skipped, selection.recovery.as_ref()) {
        eprintln!("{line}");
    }
}

#[test]
fn a_selection_without_refusals_reports_no_line() {
    let lines = refusal_lines::<&str, &str>("winner", &[], None);
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn each_refusal_of_a_selection_gets_a_line() {
    assert_eq!(
        refusal_lines("winner", &["first", "second"], Some(&"retried")),
        [
            "  selection: walked past \"first\"",
            "  selection: walked past \"second\"",
            "  selection: winner recovered from \"retried\"",
        ]
    );
}

/// What a test requires of the adapter that the selection ends on.
#[derive(Clone, Copy)]
enum Requirement {
    /// The expected adapter, or an adapter that ranks above it. An adapter that
    /// ranks above the expected one refused the probe of the test. When the
    /// selection opens a device on it, it is usable and it ranks better, so the
    /// selection made the right choice.
    ExpectedOrBetterRanked,
    DiscreteGpu,
    /// An adapter with the name of the expected adapter.
    ExpectedName,
}

/// An adapter and its place in the order in which the selection asks the
/// adapters.
struct RankedAdapter<'a> {
    /// 0 for the adapter that the selection asks first.
    rank: usize,
    name: &'a str,
    device_type: wgpu::DeviceType,
}

impl<'a> RankedAdapter<'a> {
    fn new(rank: usize, info: &'a wgpu::AdapterInfo) -> Self {
        Self {
            rank,
            name: &info.name,
            device_type: info.device_type,
        }
    }
}

/// What the selection reports about an adapter that it asked before the adapter
/// that it ended on.
struct Asked<'a> {
    name: &'a str,
    shortfall_predicted: bool,
    refusals: usize,
}

/// The adapter that a selection ended on, and the adapters that it asked before
/// that one, in the order in which it asked them.
struct Selected<'a> {
    winner: RankedAdapter<'a>,
    asked_before: Vec<Asked<'a>>,
}

/// How a selection relates to the adapter that a test expects.
#[derive(Debug)]
enum Comparison {
    /// The selection ended on an adapter that meets the requirement.
    Reached,
    /// The selection ended on another adapter, and it reports exactly 2
    /// refusals and no predicted shortfall for the expected adapter. The
    /// selection asks an adapter 1 more time after a refusal that the
    /// advertised capabilities do not explain, so this is what 2 transient
    /// refusals in a row look like. The text describes the loss.
    LostAfterTwoRefusals(String),
    /// Any other loss of the expected adapter, and a selection error. The text
    /// describes it.
    Lost(String),
}

/// Compares the outcome of a selection with what a test requires. `expected` is
/// an adapter that accepted the probe of the test. The `Err` side of `outcome`
/// is the error text of a selection that failed.
fn compare(
    requirement: Requirement,
    expected: &RankedAdapter<'_>,
    outcome: &Result<Selected<'_>, String>,
) -> Comparison {
    let selected = match outcome {
        Ok(selected) => selected,
        Err(error) => {
            return Comparison::Lost(format!("the selection returned an error: {error}"));
        }
    };
    let winner = &selected.winner;
    let reached = match requirement {
        Requirement::ExpectedOrBetterRanked => winner.rank <= expected.rank,
        Requirement::DiscreteGpu => winner.device_type == wgpu::DeviceType::DiscreteGpu,
        Requirement::ExpectedName => winner.name == expected.name,
    };
    if reached {
        return Comparison::Reached;
    }

    let ended = format!(
        "the selection ended on {} ({:?}), while {} ({:?}) accepted the probe of the test",
        winner.name, winner.device_type, expected.name, expected.device_type
    );
    // The selection asks the adapters in ranked order, so its report about the
    // expected adapter is at the place of that adapter.
    let asked = selected
        .asked_before
        .get(expected.rank)
        .filter(|asked| asked.name == expected.name);
    match asked {
        None => Comparison::Lost(format!(
            "{ended}; the selection reports no request to that adapter"
        )),
        Some(asked) if asked.shortfall_predicted => Comparison::Lost(format!(
            "{ended}; the selection predicted a shortfall for that adapter"
        )),
        Some(Asked { refusals: 2, .. }) => Comparison::LostAfterTwoRefusals(format!(
            "{ended}; that adapter refused 2 requests of the selection, with no predicted \
             shortfall"
        )),
        Some(asked) => Comparison::Lost(format!(
            "{ended}; the selection reports {} refusal(s) by that adapter and no predicted \
             shortfall",
            asked.refusals
        )),
    }
}

const DISCRETE: RankedAdapter<'static> = RankedAdapter {
    rank: 0,
    name: "discrete",
    device_type: wgpu::DeviceType::DiscreteGpu,
};
const INTEGRATED: RankedAdapter<'static> = RankedAdapter {
    rank: 1,
    name: "integrated",
    device_type: wgpu::DeviceType::IntegratedGpu,
};

fn ended_on<'a>(
    winner: RankedAdapter<'a>,
    asked_before: Vec<Asked<'a>>,
) -> Result<Selected<'a>, String> {
    Ok(Selected {
        winner,
        asked_before,
    })
}

/// Builds the report about an adapter that refused `refusals` requests, with no
/// predicted shortfall.
fn refused(name: &str, refusals: usize) -> Asked<'_> {
    Asked {
        name,
        shortfall_predicted: false,
        refusals,
    }
}

#[test]
fn the_expected_adapter_is_reached() {
    let comparison = compare(
        Requirement::ExpectedOrBetterRanked,
        &DISCRETE,
        &ended_on(DISCRETE, vec![]),
    );
    assert!(matches!(comparison, Comparison::Reached), "{comparison:?}");
}

#[test]
fn a_better_ranked_adapter_counts_as_reached() {
    // The discrete adapter refused the probe, so the test expects the
    // integrated one. Then the selection opened a device on the discrete one.
    let comparison = compare(
        Requirement::ExpectedOrBetterRanked,
        &INTEGRATED,
        &ended_on(DISCRETE, vec![]),
    );
    assert!(matches!(comparison, Comparison::Reached), "{comparison:?}");
}

#[test]
fn exactly_2_refusals_without_a_shortfall_call_for_1_more_round() {
    let comparison = compare(
        Requirement::ExpectedOrBetterRanked,
        &DISCRETE,
        &ended_on(INTEGRATED, vec![refused("discrete", 2)]),
    );
    assert!(
        matches!(comparison, Comparison::LostAfterTwoRefusals(_)),
        "{comparison:?}"
    );
}

#[test]
fn a_predicted_shortfall_fails_at_once() {
    let asked = Asked {
        name: "discrete",
        shortfall_predicted: true,
        refusals: 2,
    };
    let comparison = compare(
        Requirement::ExpectedOrBetterRanked,
        &DISCRETE,
        &ended_on(INTEGRATED, vec![asked]),
    );
    assert!(
        matches!(&comparison, Comparison::Lost(reason) if reason.contains("predicted a shortfall")),
        "{comparison:?}"
    );
}

#[test]
fn any_other_number_of_refusals_fails_at_once() {
    for refusals in [0, 1, 3] {
        let comparison = compare(
            Requirement::ExpectedOrBetterRanked,
            &DISCRETE,
            &ended_on(INTEGRATED, vec![refused("discrete", refusals)]),
        );
        assert!(
            matches!(&comparison, Comparison::Lost(reason) if reason.contains("refusal(s)")),
            "{refusals} refusals: {comparison:?}"
        );
    }
}

#[test]
fn an_adapter_that_was_never_asked_fails_at_once() {
    for asked_before in [vec![], vec![refused("another adapter", 2)]] {
        let comparison = compare(
            Requirement::ExpectedOrBetterRanked,
            &DISCRETE,
            &ended_on(INTEGRATED, asked_before),
        );
        assert!(
            matches!(&comparison, Comparison::Lost(reason) if reason.contains("no request")),
            "{comparison:?}"
        );
    }
}

#[test]
fn a_selection_error_fails_at_once() {
    let comparison = compare(
        Requirement::ExpectedOrBetterRanked,
        &DISCRETE,
        &Err("no adapter accepted".to_string()),
    );
    assert!(
        matches!(&comparison, Comparison::Lost(reason) if reason.contains("no adapter accepted")),
        "{comparison:?}"
    );
}

#[test]
fn another_discrete_gpu_meets_the_discrete_requirement() {
    let second_discrete = RankedAdapter {
        rank: 1,
        name: "second discrete",
        device_type: wgpu::DeviceType::DiscreteGpu,
    };
    let comparison = compare(
        Requirement::DiscreteGpu,
        &DISCRETE,
        &ended_on(second_discrete, vec![refused("discrete", 2)]),
    );
    assert!(matches!(comparison, Comparison::Reached), "{comparison:?}");
}

#[test]
fn a_winner_that_is_not_discrete_misses_the_discrete_requirement() {
    let comparison = compare(
        Requirement::DiscreteGpu,
        &DISCRETE,
        &ended_on(INTEGRATED, vec![refused("discrete", 2)]),
    );
    assert!(
        matches!(comparison, Comparison::LostAfterTwoRefusals(_)),
        "{comparison:?}"
    );
}

#[test]
fn an_adapter_with_the_expected_name_meets_the_name_requirement() {
    let twin = |rank| RankedAdapter {
        rank,
        name: "twin",
        device_type: wgpu::DeviceType::DiscreteGpu,
    };
    let comparison = compare(
        Requirement::ExpectedName,
        &twin(0),
        &ended_on(twin(1), vec![refused("twin", 2)]),
    );
    assert!(matches!(comparison, Comparison::Reached), "{comparison:?}");
}

#[test]
fn a_better_ranked_adapter_with_another_name_misses_the_name_requirement() {
    let comparison = compare(
        Requirement::ExpectedName,
        &INTEGRATED,
        &ended_on(DISCRETE, vec![]),
    );
    assert!(
        matches!(&comparison, Comparison::Lost(reason) if reason.contains("no request")),
        "{comparison:?}"
    );
}

/// Runs `round`, and runs it 1 more time when its comparison is
/// `LostAfterTwoRefusals`. Only `Reached` passes round 2. `round` returns
/// `None` when it skipped. The `Err` side holds the reason why the test must
/// fail.
fn run_rounds(mut round: impl FnMut() -> Option<Comparison>) -> Result<(), String> {
    let first = match round() {
        None | Some(Comparison::Reached) => return Ok(()),
        Some(Comparison::Lost(reason)) => return Err(reason),
        Some(Comparison::LostAfterTwoRefusals(reason)) => reason,
    };
    match round() {
        Some(Comparison::Reached) => {
            eprintln!("round 2 reached the expected adapter; round 1: {first}");
            Ok(())
        }
        Some(Comparison::LostAfterTwoRefusals(second) | Comparison::Lost(second)) => {
            Err(format!("round 1: {first}\nround 2: {second}"))
        }
        None => Err(format!("round 1: {first}\nround 2: skipped")),
    }
}

/// Runs `run_rounds` over the rounds of `script`, in order. Returns its result
/// and the number of rounds that it ran.
fn run_script(script: Vec<Option<Comparison>>) -> (Result<(), String>, usize) {
    let mut rounds = script.into_iter();
    let mut rounds_run = 0;
    let result = run_rounds(|| {
        rounds_run += 1;
        rounds
            .next()
            .expect("the script must hold a round for each call")
    });
    (result, rounds_run)
}

fn lost_after_2_refusals(text: &str) -> Option<Comparison> {
    Some(Comparison::LostAfterTwoRefusals(text.to_string()))
}

fn lost(text: &str) -> Option<Comparison> {
    Some(Comparison::Lost(text.to_string()))
}

#[test]
fn a_round_that_reaches_the_expected_adapter_ends_the_test() {
    assert_eq!(run_script(vec![Some(Comparison::Reached)]), (Ok(()), 1));
}

#[test]
fn a_round_that_skips_ends_the_test() {
    assert_eq!(run_script(vec![None]), (Ok(()), 1));
}

#[test]
fn a_loss_fails_without_a_round_2() {
    assert_eq!(
        run_script(vec![lost("cause")]),
        (Err("cause".to_string()), 1)
    );
}

#[test]
fn a_loss_after_2_refusals_passes_when_round_2_reaches_the_expected_adapter() {
    let script = vec![lost_after_2_refusals("cause 1"), Some(Comparison::Reached)];
    assert_eq!(run_script(script), (Ok(()), 2));
}

#[test]
fn a_loss_after_2_refusals_fails_when_round_2_loses_again() {
    let (result, rounds_run) = run_script(vec![lost_after_2_refusals("cause 1"), lost("cause 2")]);
    let reason = result.expect_err("a loss in round 2 must fail the test");
    assert!(
        reason.contains("cause 1") && reason.contains("cause 2"),
        "{reason}"
    );
    assert_eq!(rounds_run, 2);
}

#[test]
fn a_loss_after_2_refusals_in_round_2_fails_without_a_round_3() {
    let (result, rounds_run) = run_script(vec![
        lost_after_2_refusals("cause 1"),
        lost_after_2_refusals("cause 2"),
        Some(Comparison::Reached),
    ]);
    let reason = result.expect_err("a loss in round 2 must fail the test");
    assert!(
        reason.contains("cause 1") && reason.contains("cause 2"),
        "{reason}"
    );
    assert_eq!(rounds_run, 2);
}

#[test]
fn a_loss_after_2_refusals_fails_when_round_2_skips() {
    let (result, rounds_run) = run_script(vec![lost_after_2_refusals("cause 1"), None]);
    let reason = result.expect_err("a round 2 that skips must fail the test");
    assert!(reason.contains("cause 1"), "{reason}");
    assert_eq!(rounds_run, 2);
}

/// Runs `round` in a GPU turn, and 1 more time in a new GPU turn when
/// `run_rounds` asks for it. Panics with `claim` and the reason when the test
/// must fail.
fn assert_in_rounds(
    claim: &str,
    round: impl Fn(&wgpu::Instance, &[wgpu::Adapter]) -> Option<Comparison>,
) {
    if let Err(reason) = run_rounds(|| with_gpu_turn(&round).flatten()) {
        panic!("{claim}: {reason}");
    }
}

/// Compares what the selection returned with the adapter at `expected_rank` of
/// `order`, the order in which the test expects the selection to ask the
/// adapters of `infos`. Drains the device that the selection opened.
///
/// Panics when that device does not belong to the adapter that the selection
/// returned with it.
fn compare_selection(
    requirement: Requirement,
    infos: &[wgpu::AdapterInfo],
    order: &[usize],
    expected_rank: usize,
    selection: anyhow::Result<AdapterSelection>,
) -> Comparison {
    let expected = RankedAdapter::new(expected_rank, &infos[order[expected_rank]]);
    let selection = match selection {
        Ok(selection) => selection,
        Err(error) => return compare(requirement, &expected, &Err(error.to_string())),
    };
    let winner_info = selection.adapter.get_info();
    assert_eq!(
        selection.device.adapter_info(),
        winner_info,
        "the returned device must belong to the returned adapter"
    );
    let _ = selection.device.poll(wgpu::PollType::wait_indefinitely());

    // The place of the winner comes from the enumeration of the test. The
    // length of `skipped` gives the same number only when the selection asks
    // the adapters in the order that the test expects.
    let winner_rank = order
        .iter()
        .position(|&index| infos[index] == winner_info)
        .expect("the selection must end on an adapter that the test enumerated");
    let asked_before = selection
        .skipped
        .iter()
        .map(|skipped| Asked {
            name: &skipped.name,
            shortfall_predicted: skipped.shortfall.is_some(),
            refusals: skipped.errors.len(),
        })
        .collect();
    let selected = Selected {
        winner: RankedAdapter::new(winner_rank, &winner_info),
        asked_before,
    };
    compare(requirement, &expected, &Ok(selected))
}

#[test]
fn selection_returns_the_top_ranked_adapter_that_opens_a_device() {
    assert_in_rounds(
        "selection must settle on the top-ranked adapter that opens a device",
        |instance, adapters| {
            let infos = infos_of(adapters);
            let order = ranked_order(&infos, None);
            let Some(expected) = expected_choice(adapters, &order) else {
                eprintln!("skipping: no adapter accepts the application device profile");
                return None;
            };

            let selection = select_adapter_and_device(
                instance,
                PLATFORM_BACKENDS,
                &device_descriptor("graphics-adapter selection test"),
            )
            .inspect(print_refusals);
            Some(compare_selection(
                Requirement::ExpectedOrBetterRanked,
                &infos,
                &order,
                expected,
                selection,
            ))
        },
    );
}

#[test]
fn a_software_adapter_is_never_chosen_while_a_gpu_can_serve() {
    // The behaviour that matters most: a machine with a real GPU must not end
    // up rendering through a software adapter, whatever order the platform
    // enumerated them in.
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
        .inspect(print_refusals)
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
    assert_in_rounds(
        "selection must settle on a discrete GPU while a discrete GPU is usable",
        |instance, adapters| {
            let infos = infos_of(adapters);
            let order = ranked_order(&infos, None);
            let discrete = |index: usize| infos[index].device_type == wgpu::DeviceType::DiscreteGpu;
            if !order.iter().any(|&index| discrete(index)) {
                eprintln!("skipping: this environment exposes no usable discrete GPU");
                return None;
            }
            let Some(expected) = order.iter().position(|&index| {
                discrete(index) && accepts_within_2_probes(|| can_open_device(&adapters[index]))
            }) else {
                eprintln!("skipping: every discrete GPU of this environment refused 2 probes");
                return None;
            };

            let selection = select_adapter_and_device(
                instance,
                PLATFORM_BACKENDS,
                &device_descriptor("graphics-adapter discrete-preference test"),
            )
            .inspect(print_refusals);
            Some(compare_selection(
                Requirement::DiscreteGpu,
                &infos,
                &order,
                expected,
                selection,
            ))
        },
    );
}

#[test]
fn the_selected_device_can_carry_gpu_work() {
    // A selected adapter that cannot run a submission would satisfy every
    // ordering assertion above and still leave the viewport blank, which is the
    // symptom this whole module exists to prevent.
    with_gpu_turn(|instance, _adapters| {
        let Ok(AdapterSelection { device, queue, .. }) = select_adapter_and_device(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter usability test"),
        )
        .inspect(print_refusals) else {
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
fn the_selected_device_takes_the_adapters_buffer_and_texture_ceilings() {
    with_gpu_turn(|instance, _adapters| {
        let Ok(AdapterSelection {
            adapter, device, ..
        }) = select_adapter_and_device(
            instance,
            PLATFORM_BACKENDS,
            &device_descriptor("graphics-adapter ceilings test"),
        )
        .inspect(print_refusals)
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
        assert_eq!(
            limits.max_texture_dimension_2d,
            adapter.limits().max_texture_dimension_2d,
            "the device's texture ceiling must be the adapter's, not the default profile's"
        );
        let expected_rest = wgpu::Limits {
            max_buffer_size: limits.max_buffer_size,
            max_texture_dimension_2d: limits.max_texture_dimension_2d,
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
    assert_in_rounds(
        "the override must reach the adapter it names",
        |instance, adapters| {
            let infos = infos_of(adapters);
            // Name the last-ranked adapter that can still open a device, so the
            // assertion fails if the override is ignored and the default
            // ordering wins instead.
            let Some(target) = ranked_order(&infos, None)
                .into_iter()
                .rev()
                .find(|&index| can_open_device(&adapters[index]))
            else {
                eprintln!("skipping: no adapter accepts the application device profile");
                return None;
            };
            let name = &infos[target].name;
            let order = ranked_order(&infos, Some(name));
            let expected = order
                .iter()
                .position(|&index| index == target)
                .expect("a ranked order must hold every adapter");

            let selection = select_adapter_and_device_with(
                instance,
                PLATFORM_BACKENDS,
                &device_descriptor("graphics-adapter override test"),
                Some(name),
            )
            .inspect(print_refusals);
            Some(compare_selection(
                Requirement::ExpectedName,
                &infos,
                &order,
                expected,
                selection,
            ))
        },
    );
}

#[test]
fn an_override_matching_nothing_falls_back_to_the_normal_choice() {
    // A stale or misspelled variable must not break the launch, which is the
    // failure mode wgpu's own env helper has.
    assert_in_rounds(
        "an unmatched override must leave the normal choice untouched",
        |instance, adapters| {
            let infos = infos_of(adapters);
            let order = ranked_order(&infos, None);
            let Some(expected) = expected_choice(adapters, &order) else {
                eprintln!("skipping: no adapter accepts the application device profile");
                return None;
            };

            let selection = select_adapter_and_device_with(
                instance,
                PLATFORM_BACKENDS,
                &device_descriptor("graphics-adapter unmatched-override test"),
                Some("no adapter is called this"),
            )
            .inspect(print_refusals);
            Some(compare_selection(
                Requirement::ExpectedOrBetterRanked,
                &infos,
                &order,
                expected,
                selection,
            ))
        },
    );
}

#[test]
fn selection_reports_an_error_when_no_backend_exposes_an_adapter() {
    // Deterministic on every machine, GPU or not: an empty backend set
    // enumerates nothing, and the caller has to be told rather than handed a
    // device that cannot exist.
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

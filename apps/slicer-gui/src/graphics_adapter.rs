/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Picks the wgpu adapter the whole application renders with, and opens
//! a device on it. `run()` builds this pairing once and hands it to
//! Slint through `WGPUConfiguration::Manual`, so the choice made here
//! is the choice every window lives with.
//!
//! Why this is not a plain `Instance::request_adapter` call: that entry
//! point sorts candidates only when given a non-default
//! `PowerPreference`, and the default `PowerPreference::None` applies no
//! ordering at all. The adapter then comes back in raw enumeration
//! order, which on Windows is DXGI index order and elsewhere is whatever
//! the driver stack lists first. A machine with a single GPU has one
//! candidate and always gets it right; a machine with a discrete GPU
//! next to an integrated one has several, and the first one enumerated
//! is not always the one that can drive the window.
//!
//! The ordering is decided here instead: real GPUs before software ones,
//! discrete before integrated, with an environment override for
//! diagnosing a machine we cannot reproduce on. The policy is split from
//! the wgpu calls so it can be tested without a GPU:
//! [`rank_adapters`] decides the order, `open_first_available` walks it,
//! and [`select_adapter_and_device`] is the thin layer that talks to
//! wgpu.

use slint::wgpu_29::wgpu;

/// Environment variable that forces a specific adapter by name. Same
/// name and same case-insensitive substring matching as wgpu's own
/// `initialize_adapter_from_env`, so anyone who already knows that
/// variable can use it here. Unlike wgpu's helper, a name matching
/// nothing falls back to the normal ordering instead of panicking.
pub const ADAPTER_NAME_ENV: &str = "WGPU_ADAPTER_NAME";

/// The part of `wgpu::AdapterInfo` the ordering policy reads. Keeping
/// the policy off the full adapter handle is what lets the ordering
/// tests run on machines with no GPU at all; the selection itself is
/// covered against real devices in
/// `tests/integration_graphics_adapter.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterFacts<'a> {
    pub name: &'a str,
    pub device_type: wgpu::DeviceType,
}

/// Preference bucket for a device type, lower is preferred. A resin
/// slicer draws a build volume and a mesh continuously, so a discrete
/// GPU is the best default. `Cpu` sorts last on purpose: a software
/// adapter can open a device and then fail to present, which is a worse
/// outcome than never choosing it.
fn device_type_rank(device_type: wgpu::DeviceType) -> u8 {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// True when `name` contains `wanted`, ignoring case. Substring rather
/// than equality because adapter names carry vendor decoration nobody
/// wants to type in full ("NVIDIA GeForce RTX 4070 Ti").
fn name_matches(name: &str, wanted: &str) -> bool {
    name.to_lowercase().contains(&wanted.to_lowercase())
}

/// Orders `candidates` best-first, returning indices into the input.
///
/// An explicit `preferred_name` outranks the device type: the override
/// exists to force a specific adapter while diagnosing a machine, so it
/// has to reach an adapter this policy would otherwise put last. A
/// `preferred_name` matching nothing changes nothing, which keeps a
/// stale or misspelled variable from breaking the launch.
///
/// Ordering inside one bucket follows enumeration order, so the
/// platform's own preference still breaks ties between two adapters we
/// consider equally good.
pub fn rank_adapters(candidates: &[AdapterFacts<'_>], preferred_name: Option<&str>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    // `sort_by_key` is stable, which is what preserves enumeration
    // order inside a bucket.
    order.sort_by_key(|&i| {
        let facts = candidates[i];
        let name_rank = match preferred_name {
            Some(wanted) if name_matches(facts.name, wanted) => 0,
            _ => 1,
        };
        (name_rank, device_type_rank(facts.device_type))
    });
    order
}

/// Normalises a raw override value. Blank or whitespace-only counts as
/// unset, so clearing the variable with an empty string behaves the way
/// people expect rather than matching every adapter.
fn parse_adapter_name_override(raw: Option<&str>) -> Option<&str> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

/// Reads the adapter override from the environment.
fn adapter_name_override() -> Option<String> {
    let raw = std::env::var(ADAPTER_NAME_ENV).ok()?;
    parse_adapter_name_override(Some(&raw)).map(str::to_string)
}

/// Refusals collected while walking candidates: each entry pairs the
/// refusing candidate's index with the error it gave.
type Refusals<E> = Vec<(usize, E)>;

/// Walks `order`, calling `open` until one candidate succeeds. Returns
/// the winning index with its value plus the refusals walked past on
/// the way there, or every failure in the order they happened when no
/// candidate succeeds.
///
/// Trying more than one candidate matters because opening a device is
/// the first point where an adapter can refuse the limits we ask for.
/// Giving up on the first refusal would fail the launch while a usable
/// second adapter sat unqueried.
///
/// The refusals are kept on the success path too: a refusal there
/// means a better-ranked adapter was passed over, and silently landing
/// on a lower-ranked one would leave no trace of why.
fn open_first_available<T, E>(
    order: &[usize],
    mut open: impl FnMut(usize) -> Result<T, E>,
) -> Result<(usize, T, Refusals<E>), Refusals<E>> {
    let mut failures = Vec::new();
    for &index in order {
        match open(index) {
            Ok(value) => return Ok((index, value, failures)),
            Err(err) => failures.push((index, err)),
        }
    }
    Err(failures)
}

/// Renders the per-adapter failures into one message, naming each
/// adapter so a bug report says which hardware refused and why.
fn describe_adapter_failures<E: std::fmt::Display>(
    candidates: &[AdapterFacts<'_>],
    failures: &[(usize, E)],
) -> String {
    failures
        .iter()
        .map(|(index, err)| {
            let name = candidates
                .get(*index)
                .map_or("unknown adapter", |facts| facts.name);
            format!("{name}: {err}")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// An adapter the selection walked past: ranked above the winner but
/// refused the device request. The error is kept in its typed form
/// rather than as text so a future recovery policy can distinguish
/// refusal kinds instead of guessing from a message.
#[derive(Debug)]
pub struct SkippedAdapter {
    pub name: String,
    pub error: wgpu::RequestDeviceError,
}

/// What adapter selection settled on: the winning adapter with its
/// open device and queue, plus every better-ranked adapter that
/// refused a device on the way there. A non-empty `skipped` means the
/// session is not running on the adapter the policy would prefer, and
/// the caller should say so rather than let that pass silently.
#[derive(Debug)]
pub struct AdapterSelection {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub skipped: Vec<SkippedAdapter>,
}

/// Enumerates the adapters `backends` exposes, orders them with
/// [`rank_adapters`], and opens a device on the first one that accepts
/// `descriptor`. The adapter override is read from [`ADAPTER_NAME_ENV`].
pub fn select_adapter_and_device(
    instance: &wgpu::Instance,
    backends: wgpu::Backends,
    descriptor: &wgpu::DeviceDescriptor<'_>,
) -> anyhow::Result<AdapterSelection> {
    select_adapter_and_device_with(
        instance,
        backends,
        descriptor,
        adapter_name_override().as_deref(),
    )
}

/// [`select_adapter_and_device`] with the adapter override passed in
/// rather than read from the environment. Separate entry point so tests
/// can drive the override without mutating process-wide state that
/// every other test in the binary shares.
pub fn select_adapter_and_device_with(
    instance: &wgpu::Instance,
    backends: wgpu::Backends,
    descriptor: &wgpu::DeviceDescriptor<'_>,
    preferred_name: Option<&str>,
) -> anyhow::Result<AdapterSelection> {
    let adapters = pollster::block_on(instance.enumerate_adapters(backends));
    let infos: Vec<wgpu::AdapterInfo> = adapters.iter().map(|adapter| adapter.get_info()).collect();
    let candidates: Vec<AdapterFacts<'_>> = infos
        .iter()
        .map(|info| AdapterFacts {
            name: &info.name,
            device_type: info.device_type,
        })
        .collect();
    if candidates.is_empty() {
        anyhow::bail!("No graphics adapter is available for backends {backends:?}");
    }

    let order = rank_adapters(&candidates, preferred_name);
    match open_first_available(&order, |index| {
        pollster::block_on(adapters[index].request_device(descriptor))
    }) {
        Ok((index, (device, queue), refusals)) => Ok(AdapterSelection {
            adapter: adapters[index].clone(),
            device,
            queue,
            skipped: refusals
                .into_iter()
                .map(|(refused_index, error)| SkippedAdapter {
                    name: infos[refused_index].name.clone(),
                    error,
                })
                .collect(),
        }),
        Err(failures) => anyhow::bail!(
            "No graphics adapter accepted a device request. Tried: {}",
            describe_adapter_failures(&candidates, &failures)
        ),
    }
}

#[cfg(test)]
fn facts<'a>(name: &'a str, device_type: wgpu::DeviceType) -> AdapterFacts<'a> {
    AdapterFacts { name, device_type }
}

#[cfg(test)]
mod rank_adapters_tests {
    use super::*;

    fn names<'a>(candidates: &[AdapterFacts<'a>], order: &[usize]) -> Vec<&'a str> {
        order.iter().map(|&i| candidates[i].name).collect()
    }

    #[test]
    fn empty_input_yields_empty_order() {
        assert!(rank_adapters(&[], None).is_empty());
    }

    #[test]
    fn single_candidate_is_returned_as_is() {
        let candidates = [facts("Only Adapter", wgpu::DeviceType::IntegratedGpu)];
        assert_eq!(rank_adapters(&candidates, None), vec![0]);
    }

    #[test]
    fn discrete_gpu_outranks_integrated() {
        // The reported failure came from a machine with exactly this
        // pairing, where enumeration order alone can put the wrong
        // adapter first.
        let candidates = [
            facts("Integrated Graphics", wgpu::DeviceType::IntegratedGpu),
            facts("Discrete Graphics", wgpu::DeviceType::DiscreteGpu),
        ];
        let order = rank_adapters(&candidates, None);
        assert_eq!(
            names(&candidates, &order),
            ["Discrete Graphics", "Integrated Graphics"]
        );
    }

    #[test]
    fn software_adapter_sorts_last() {
        let candidates = [
            facts("Microsoft Basic Render Driver", wgpu::DeviceType::Cpu),
            facts("Some Unknown Device", wgpu::DeviceType::Other),
            facts("Integrated Graphics", wgpu::DeviceType::IntegratedGpu),
        ];
        let order = rank_adapters(&candidates, None);
        assert_eq!(
            names(&candidates, &order),
            [
                "Integrated Graphics",
                "Some Unknown Device",
                "Microsoft Basic Render Driver"
            ]
        );
    }

    #[test]
    fn full_device_type_ordering_is_respected() {
        let candidates = [
            facts("cpu", wgpu::DeviceType::Cpu),
            facts("other", wgpu::DeviceType::Other),
            facts("virtual", wgpu::DeviceType::VirtualGpu),
            facts("integrated", wgpu::DeviceType::IntegratedGpu),
            facts("discrete", wgpu::DeviceType::DiscreteGpu),
        ];
        let order = rank_adapters(&candidates, None);
        assert_eq!(
            names(&candidates, &order),
            ["discrete", "integrated", "virtual", "other", "cpu"]
        );
    }

    #[test]
    fn equal_ranks_keep_enumeration_order() {
        // Two adapters we consider equally good must come back in the
        // order the platform listed them, so the platform's own
        // preference still decides.
        let candidates = [
            facts("First Discrete", wgpu::DeviceType::DiscreteGpu),
            facts("Second Discrete", wgpu::DeviceType::DiscreteGpu),
            facts("Third Discrete", wgpu::DeviceType::DiscreteGpu),
        ];
        let order = rank_adapters(&candidates, None);
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn every_candidate_appears_exactly_once() {
        let candidates = [
            facts("cpu", wgpu::DeviceType::Cpu),
            facts("discrete", wgpu::DeviceType::DiscreteGpu),
            facts("integrated", wgpu::DeviceType::IntegratedGpu),
        ];
        let mut order = rank_adapters(&candidates, Some("cpu"));
        order.sort_unstable();
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn name_override_outranks_device_type() {
        // The override has to reach an adapter the policy would
        // otherwise put last, otherwise it cannot narrow down a
        // machine-specific failure.
        let candidates = [
            facts("Discrete Graphics", wgpu::DeviceType::DiscreteGpu),
            facts("Microsoft Basic Render Driver", wgpu::DeviceType::Cpu),
        ];
        let order = rank_adapters(&candidates, Some("Basic Render"));
        assert_eq!(
            names(&candidates, &order),
            ["Microsoft Basic Render Driver", "Discrete Graphics"]
        );
    }

    #[test]
    fn name_override_matches_case_insensitively_on_a_substring() {
        let candidates = [
            facts("Integrated Graphics", wgpu::DeviceType::IntegratedGpu),
            facts("NVIDIA GeForce RTX 4070 Ti", wgpu::DeviceType::DiscreteGpu),
        ];
        let order = rank_adapters(&candidates, Some("nvidia"));
        assert_eq!(order[0], 1);
    }

    #[test]
    fn several_name_matches_keep_device_type_order_between_them() {
        let candidates = [
            facts("Vendor Software Device", wgpu::DeviceType::Cpu),
            facts("Unrelated Adapter", wgpu::DeviceType::DiscreteGpu),
            facts("Vendor Discrete Device", wgpu::DeviceType::DiscreteGpu),
        ];
        let order = rank_adapters(&candidates, Some("vendor"));
        assert_eq!(
            names(&candidates, &order),
            [
                "Vendor Discrete Device",
                "Vendor Software Device",
                "Unrelated Adapter"
            ]
        );
    }

    #[test]
    fn unmatched_name_override_falls_back_to_device_type_order() {
        // A stale or misspelled variable must not drop candidates or
        // change the outcome; every adapter is still offered.
        let candidates = [
            facts("Integrated Graphics", wgpu::DeviceType::IntegratedGpu),
            facts("Discrete Graphics", wgpu::DeviceType::DiscreteGpu),
        ];
        let order = rank_adapters(&candidates, Some("no such adapter"));
        assert_eq!(
            names(&candidates, &order),
            ["Discrete Graphics", "Integrated Graphics"]
        );
    }
}

#[cfg(test)]
mod name_matches_tests {
    use super::*;

    #[test]
    fn matching_ignores_case_in_both_directions() {
        assert!(name_matches("NVIDIA GeForce RTX 4070 Ti", "geforce"));
        assert!(name_matches("nvidia geforce rtx 4070 ti", "GeForce"));
    }

    #[test]
    fn non_substring_does_not_match() {
        assert!(!name_matches("NVIDIA GeForce RTX 4070 Ti", "radeon"));
    }

    #[test]
    fn empty_wanted_matches_anything() {
        // Not reachable through `parse_adapter_name_override`, which
        // filters blank values out, but the predicate should behave the
        // way `contains` does rather than surprise a future caller.
        assert!(name_matches("Any Adapter", ""));
    }
}

#[cfg(test)]
mod parse_adapter_name_override_tests {
    use super::*;

    #[test]
    fn unset_variable_is_none() {
        assert_eq!(parse_adapter_name_override(None), None);
    }

    #[test]
    fn empty_value_is_none() {
        assert_eq!(parse_adapter_name_override(Some("")), None);
    }

    #[test]
    fn whitespace_only_value_is_none() {
        assert_eq!(parse_adapter_name_override(Some("   \t ")), None);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(
            parse_adapter_name_override(Some("  nvidia  ")),
            Some("nvidia")
        );
    }

    #[test]
    fn plain_value_passes_through() {
        assert_eq!(parse_adapter_name_override(Some("nvidia")), Some("nvidia"));
    }
}

#[cfg(test)]
mod open_first_available_tests {
    use super::*;

    #[test]
    fn first_candidate_wins_without_trying_the_rest() {
        let mut attempts = Vec::new();
        let result = open_first_available(&[2, 0, 1], |index| {
            attempts.push(index);
            Ok::<_, &str>(format!("device {index}"))
        });
        assert_eq!(result, Ok((2, "device 2".to_string(), vec![])));
        assert_eq!(attempts, vec![2], "later candidates must stay unqueried");
    }

    #[test]
    fn a_refusal_falls_through_to_the_next_candidate() {
        let result = open_first_available(&[0, 1, 2], |index| {
            if index == 0 {
                Err("refused the requested limits")
            } else {
                Ok(index)
            }
        });
        assert_eq!(
            result,
            Ok((1, 1, vec![(0, "refused the requested limits")])),
            "the refusal walked past must be carried alongside the winner"
        );
    }

    #[test]
    fn refusals_before_the_winner_are_carried_in_walk_order() {
        let result = open_first_available(&[2, 0, 1], |index| {
            if index == 1 {
                Ok("device")
            } else {
                Err(format!("adapter {index} refused"))
            }
        });
        assert_eq!(
            result,
            Ok((
                1,
                "device",
                vec![
                    (2, "adapter 2 refused".to_string()),
                    (0, "adapter 0 refused".to_string()),
                ]
            ))
        );
    }

    #[test]
    fn every_refusal_is_reported_in_order() {
        let result: Result<(usize, (), _), _> =
            open_first_available(&[1, 0], |index| Err(format!("adapter {index} refused")));
        assert_eq!(
            result,
            Err(vec![
                (1, "adapter 1 refused".to_string()),
                (0, "adapter 0 refused".to_string()),
            ])
        );
    }

    #[test]
    fn empty_order_reports_no_failures() {
        let result: Result<(usize, (), Refusals<&str>), Refusals<&str>> =
            open_first_available(&[], |_| Ok(()));
        assert_eq!(result, Err(vec![]));
    }
}

#[cfg(test)]
mod describe_adapter_failures_tests {
    use super::*;

    #[test]
    fn no_failures_render_as_an_empty_string() {
        let candidates = [facts("Discrete Graphics", wgpu::DeviceType::DiscreteGpu)];
        let failures: [(usize, &str); 0] = [];
        assert_eq!(describe_adapter_failures(&candidates, &failures), "");
    }

    #[test]
    fn each_failure_is_named_after_its_adapter() {
        let candidates = [
            facts("Discrete Graphics", wgpu::DeviceType::DiscreteGpu),
            facts("Integrated Graphics", wgpu::DeviceType::IntegratedGpu),
        ];
        let failures = [(0, "out of memory"), (1, "unsupported limits")];
        assert_eq!(
            describe_adapter_failures(&candidates, &failures),
            "Discrete Graphics: out of memory; Integrated Graphics: unsupported limits"
        );
    }

    #[test]
    fn an_index_with_no_candidate_still_renders() {
        // Unreachable through `select_adapter_and_device`, which only
        // ever reports indices it took from the same slice, but the
        // message must not panic if that ever stops holding.
        let candidates = [facts("Discrete Graphics", wgpu::DeviceType::DiscreteGpu)];
        let failures = [(7, "vanished")];
        assert_eq!(
            describe_adapter_failures(&candidates, &failures),
            "unknown adapter: vanished"
        );
    }
}

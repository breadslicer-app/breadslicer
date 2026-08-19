/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::PrinterId;

use super::calibration_coverage::CalibrationCoverage;
use super::calibration_target::CalibrationTarget;

fn printer_id(raw: &str) -> PrinterId {
    raw.parse().unwrap()
}

fn keys(raw: &[&str]) -> Vec<CalibrationTarget> {
    raw.iter()
        .map(|id| CalibrationTarget::from_printer(&printer_id(id)))
        .collect()
}

#[test]
fn specific_when_the_printer_key_is_present() {
    let keys = keys(&["printer-a", "generic-machine"]);

    assert_eq!(
        CalibrationCoverage::of(&keys, Some(&printer_id("printer-a"))),
        CalibrationCoverage::Specific,
    );
}

#[test]
fn generic_fallback_when_only_the_generic_key_is_present() {
    let keys = keys(&["printer-a", "generic-machine"]);

    assert_eq!(
        CalibrationCoverage::of(&keys, Some(&printer_id("printer-b"))),
        CalibrationCoverage::GenericFallback,
    );
}

#[test]
fn none_without_either_key() {
    let keys = keys(&["printer-a"]);

    assert_eq!(
        CalibrationCoverage::of(&keys, Some(&printer_id("printer-b"))),
        CalibrationCoverage::None,
    );
    assert_eq!(
        CalibrationCoverage::of(&Vec::new(), Some(&printer_id("printer-b"))),
        CalibrationCoverage::None,
    );
}

#[test]
fn without_a_printer_only_the_generic_key_covers() {
    assert_eq!(
        CalibrationCoverage::of(&keys(&["generic-machine"]), None),
        CalibrationCoverage::GenericFallback,
    );
    assert_eq!(
        CalibrationCoverage::of(&keys(&["printer-a"]), None),
        CalibrationCoverage::None,
    );
}

#[test]
fn the_generic_printer_itself_is_specific() {
    let keys = keys(&["generic-machine"]);

    assert_eq!(
        CalibrationCoverage::of(&keys, Some(&printer_id("generic-machine"))),
        CalibrationCoverage::Specific,
    );
}

#[test]
fn keys_from_two_sets_are_read_as_one() {
    // The GUI's union of a base's keys and its extension's.
    let base = keys(&["printer-a"]);
    let extension = keys(&["generic-machine"]);

    assert_eq!(
        CalibrationCoverage::of(
            base.iter().chain(extension.iter()),
            Some(&printer_id("printer-b")),
        ),
        CalibrationCoverage::GenericFallback,
    );
}

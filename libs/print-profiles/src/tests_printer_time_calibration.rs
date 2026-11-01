/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::printer_time_calibration::PrinterTimeCalibration;
use sc_print_primitives::{NonNegativeFiniteSec, TimeMultiplier, TimeOffsetSec};

const FULL_TOML: &str = r#"
print_time_multiplier = 1.322
print_time_offset_sec = -15.0
per_layer_overhead_sec = 1.14
"#;

fn calibration(multiplier: Option<f64>, offset: Option<f64>) -> PrinterTimeCalibration {
    PrinterTimeCalibration {
        print_time_multiplier: multiplier.map(|v| TimeMultiplier::new(v).unwrap()),
        print_time_offset_sec: offset.map(|v| TimeOffsetSec::new(v).unwrap()),
        per_layer_overhead_sec: None,
    }
}

fn per_layer(overhead: f64, offset: Option<f64>) -> PrinterTimeCalibration {
    PrinterTimeCalibration {
        per_layer_overhead_sec: Some(NonNegativeFiniteSec::new(overhead).unwrap()),
        ..calibration(None, offset)
    }
}

#[test]
fn deserialise_parses_every_coefficient() {
    let parsed: PrinterTimeCalibration = toml_edit::de::from_str(FULL_TOML).unwrap();
    assert_eq!(parsed.print_time_multiplier.map(|v| v.value()), Some(1.322));
    assert_eq!(parsed.print_time_offset_sec.map(|v| v.value()), Some(-15.0));
    assert_eq!(parsed.per_layer_overhead_sec.map(|v| v.value()), Some(1.14));
}

#[test]
fn deserialise_empty_section_leaves_every_coefficient_none() {
    let parsed: PrinterTimeCalibration = toml_edit::de::from_str("").unwrap();
    assert!(parsed.print_time_multiplier.is_none());
    assert!(parsed.print_time_offset_sec.is_none());
    assert!(parsed.per_layer_overhead_sec.is_none());
}

#[test]
fn deserialise_rejects_a_negative_per_layer_overhead() {
    let toml = "per_layer_overhead_sec = -1.0
";
    assert!(toml_edit::de::from_str::<PrinterTimeCalibration>(toml).is_err());
}

#[test]
fn deserialise_rejects_a_zero_multiplier() {
    let toml = "print_time_multiplier = 0.0\n";
    assert!(toml_edit::de::from_str::<PrinterTimeCalibration>(toml).is_err());
}

#[test]
fn deserialise_rejects_a_negative_multiplier() {
    let toml = "print_time_multiplier = -1.0\n";
    assert!(toml_edit::de::from_str::<PrinterTimeCalibration>(toml).is_err());
}

#[test]
fn serialise_omits_the_coefficients_nobody_calibrated() {
    let rendered = toml_edit::ser::to_string(&PrinterTimeCalibration::default()).unwrap();
    assert!(rendered.trim().is_empty(), "rendered: {rendered}");
}

#[test]
fn roundtrip_preserves_every_coefficient() {
    let original = PrinterTimeCalibration {
        print_time_multiplier: Some(TimeMultiplier::new(1.322).unwrap()),
        ..per_layer(1.14, Some(-15.0))
    };
    let rendered = toml_edit::ser::to_string(&original).unwrap();
    let parsed: PrinterTimeCalibration = toml_edit::de::from_str(&rendered).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn an_uncalibrated_printer_reports_the_schedule_unchanged() {
    assert_eq!(
        PrinterTimeCalibration::default().corrected_sec(7883.0, 0),
        7883.0
    );
}

#[test]
fn the_multiplier_scales_the_schedule() {
    // The factor the reference file implies, applied to the schedule
    // it was measured against.
    let corrected = calibration(Some(1.322), None).corrected_sec(7883.0, 0);
    assert!((corrected - 10_421.3).abs() < 0.1, "corrected: {corrected}");
}

#[test]
fn the_offset_is_added_after_the_scaling() {
    // 1.5 * 100 + 30, not 1.5 * (100 + 30).
    assert_eq!(
        calibration(Some(1.5), Some(30.0)).corrected_sec(100.0, 0),
        180.0
    );
}

#[test]
fn a_negative_offset_shortens_the_estimate() {
    assert_eq!(calibration(None, Some(-30.0)).corrected_sec(100.0, 0), 70.0);
}

#[test]
fn a_correction_that_would_run_the_clock_backwards_saturates_at_zero() {
    assert_eq!(calibration(None, Some(-500.0)).corrected_sec(100.0, 0), 0.0);
}

#[test]
fn an_empty_schedule_stays_empty_under_the_multiplier_alone() {
    assert_eq!(calibration(Some(2.0), None).corrected_sec(0.0, 0), 0.0);
}

#[test]
fn the_per_layer_overhead_is_added_once_per_layer_and_never_scaled() {
    // 2 * 100 + 1.5 * 10 + 30, not 2 * (100 + 1.5 * 10) + 30.
    let corrected = PrinterTimeCalibration {
        print_time_multiplier: Some(TimeMultiplier::new(2.0).unwrap()),
        ..per_layer(1.5, Some(30.0))
    }
    .corrected_sec(100.0, 10);
    assert_eq!(corrected, 245.0);
}

#[test]
fn an_uncalibrated_printer_ignores_the_layer_count() {
    assert_eq!(
        PrinterTimeCalibration::default().corrected_sec(7883.0, 1261),
        7883.0
    );
}

#[test]
fn a_print_of_no_layers_costs_only_the_offset() {
    assert_eq!(per_layer(1.14, Some(144.0)).corrected_sec(0.0, 0), 144.0);
}

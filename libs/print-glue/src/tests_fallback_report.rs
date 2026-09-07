/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_profiles::{
    EffectiveMaterial, MaterialProfile, PrinterProfile, ResolvedField, ResolvedPrintConfig,
    SceneOverrides,
};

use super::fallback_report::{FallbackKind, fallback_report_lines, fallbacks_used};

const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-mars-5-goo"
manufacturer = "ELEGOO"
family = "Mars"
name = "Mars 5"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 143.43, y_mm = 89.6, z_mm = 150.0 }
pixel_resolution = { x = 4098, y = 2560 }
pixel_size_microns = { x = 35.0, y = 35.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "goo", version = "v3.0" }]
supports_grayscale = true
two_stage_lift = false
has_camera = false

[constraints]
z_step_mm = 0.01
"#;

/// Calibrated for the printer with the 7 values the resolver
/// requires, and nothing else: every optional value falls back.
const BARE_MATERIAL_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }

[per_printer."elegoo-mars-5-goo"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

/// The same resin with every optional value calibrated.
const FULLY_CALIBRATED_MATERIAL_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }

[per_printer."elegoo-mars-5-goo"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
wait_before_cure_sec = 0.4
wait_after_cure_sec = 1.2
wait_after_lift_sec = 2.5
bottom_wait_before_cure_sec = 6.0
bottom_wait_after_cure_sec = 7.0
bottom_wait_after_lift_sec = 8.0
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
bottom_lift_distance_mm = 8.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 90.0
light_pwm = 80.0
bottom_light_pwm = 60.0
"#;

/// The same resin again, naming its waits and duties resin-wide
/// instead of per printer.
const MATERIAL_WITH_DEFAULTS_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }
wait_before_cure_sec = 0.5
light_pwm = 70.0

[per_printer."elegoo-mars-5-goo"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

fn config(material_toml: &str) -> ResolvedPrintConfig {
    let printer: PrinterProfile = toml_edit::de::from_str(PRINTER_TOML).unwrap();
    let material: MaterialProfile = toml_edit::de::from_str(material_toml).unwrap();
    let material = EffectiveMaterial::from(material);
    sc_print_profiles::resolve(&printer, &material, &SceneOverrides::default()).unwrap()
}

fn reported_fields(material_toml: &str) -> Vec<ResolvedField> {
    fallbacks_used(&config(material_toml))
        .into_iter()
        .map(|used| used.field)
        .collect()
}

#[test]
fn a_bare_calibration_falls_back_on_every_optional_value_in_a_fixed_order() {
    assert_eq!(
        reported_fields(BARE_MATERIAL_TOML),
        vec![
            ResolvedField::WaitBeforeCureSec,
            ResolvedField::WaitAfterCureSec,
            ResolvedField::WaitAfterLiftSec,
            ResolvedField::BottomWaitBeforeCureSec,
            ResolvedField::BottomWaitAfterCureSec,
            ResolvedField::BottomWaitAfterLiftSec,
            ResolvedField::BottomLiftDistanceMm,
            ResolvedField::BottomLiftSpeedMmMin,
            ResolvedField::BottomRetractSpeedMmMin,
            ResolvedField::LightPwm,
            ResolvedField::BottomLightPwm,
        ],
    );
}

#[test]
fn a_fully_calibrated_resin_falls_back_on_nothing() {
    assert!(fallbacks_used(&config(FULLY_CALIBRATED_MATERIAL_TOML)).is_empty());
}

#[test]
fn a_value_named_in_the_defaults_block_is_not_a_fallback() {
    let reported = reported_fields(MATERIAL_WITH_DEFAULTS_TOML);
    assert!(
        !reported.contains(&ResolvedField::WaitBeforeCureSec),
        "the defaults block supplied it: {reported:?}",
    );
    assert!(
        !reported.contains(&ResolvedField::LightPwm),
        "the defaults block supplied it: {reported:?}",
    );
    assert!(
        reported.contains(&ResolvedField::WaitAfterCureSec),
        "nothing supplied this one: {reported:?}",
    );
}

#[test]
fn a_pause_nobody_asked_for_resolves_to_zero() {
    let used = fallbacks_used(&config(BARE_MATERIAL_TOML));
    let wait = used
        .iter()
        .find(|used| used.field == ResolvedField::WaitAfterCureSec)
        .expect("an uncalibrated wait is reported");
    assert_eq!(wait.kind, FallbackKind::NoDwell);
    assert_eq!(wait.resolved_to, 0.0);
}

#[test]
fn an_unnamed_duty_resolves_to_full_power() {
    let used = fallbacks_used(&config(BARE_MATERIAL_TOML));
    for field in [ResolvedField::LightPwm, ResolvedField::BottomLightPwm] {
        let duty = used
            .iter()
            .find(|used| used.field == field)
            .expect("an uncalibrated duty is reported");
        assert_eq!(duty.kind, FallbackKind::FullPower);
        assert_eq!(duty.resolved_to, 100.0);
    }
}

#[test]
fn an_uncalibrated_bottom_value_resolves_to_its_normal_counterpart() {
    let used = fallbacks_used(&config(BARE_MATERIAL_TOML));
    let travel = used
        .iter()
        .find(|used| used.field == ResolvedField::BottomLiftDistanceMm)
        .expect("an uncalibrated bottom distance is reported");
    assert_eq!(travel.kind, FallbackKind::NormalCounterpart);
    assert_eq!(travel.resolved_to, 5.0, "the calibrated lift distance");
}

#[test]
fn each_line_names_the_field_the_value_and_what_the_value_means() {
    let rendered: Vec<String> = fallbacks_used(&config(BARE_MATERIAL_TOML))
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        rendered
            .iter()
            .any(|line| line == "wait_after_cure_sec: no calibrated value, using 0 (no pause)"),
        "lines: {rendered:?}",
    );
    assert!(
        rendered
            .iter()
            .any(|line| line == "light_pwm: no calibrated value, using 100 (full power)"),
        "lines: {rendered:?}",
    );
    assert!(
        rendered.iter().any(|line| line
            == "bottom_lift_distance_mm: no calibrated value, using 5 (the normal-layer value)"),
        "lines: {rendered:?}",
    );
}

#[test]
fn an_override_counts_as_a_supplied_value() {
    let printer: PrinterProfile = toml_edit::de::from_str(PRINTER_TOML).unwrap();
    let material: MaterialProfile = toml_edit::de::from_str(BARE_MATERIAL_TOML).unwrap();
    let material = EffectiveMaterial::from(material);
    let overrides = SceneOverrides {
        wait_after_lift_sec: Some(sc_print_primitives::NonNegativeFiniteSec::new(1.5).unwrap()),
        ..Default::default()
    };
    let config = sc_print_profiles::resolve(&printer, &material, &overrides).unwrap();

    let reported: Vec<ResolvedField> = fallbacks_used(&config)
        .into_iter()
        .map(|used| used.field)
        .collect();
    assert!(
        !reported.contains(&ResolvedField::WaitAfterLiftSec),
        "the override supplied it: {reported:?}",
    );
}

#[test]
fn the_report_heads_the_uncalibrated_values_with_their_count() {
    let lines = fallback_report_lines(&config(BARE_MATERIAL_TOML));
    assert_eq!(
        lines.first().map(String::as_str),
        Some("warning: 11 print values are not calibrated for this printer:"),
    );
    assert_eq!(lines.len(), 12, "lines: {lines:?}");
    for line in &lines[1..] {
        assert!(line.starts_with("  "), "line: {line}");
    }
}

#[test]
fn the_report_gives_each_value_a_line_of_its_own() {
    let lines = fallback_report_lines(&config(BARE_MATERIAL_TOML));
    assert!(
        lines.contains(&"  light_pwm: no calibrated value, using 100 (full power)".to_string()),
        "lines: {lines:?}",
    );
}

#[test]
fn a_fully_calibrated_resin_reports_no_lines_at_all() {
    assert!(fallback_report_lines(&config(FULLY_CALIBRATED_MATERIAL_TOML)).is_empty());
}

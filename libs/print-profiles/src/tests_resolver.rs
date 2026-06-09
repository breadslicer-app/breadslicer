/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    LengthRange, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, ScaleCompensation,
    ScaleCompensationPercent, SpeedRangeMmPerMin,
};

use super::material_profile::MaterialProfile;
use super::printer_constraints::PrinterConstraints;
use super::printer_profile::PrinterProfile;
use super::resolve_error::ResolveError;
use super::resolve_warning::{IntentSource, ResolveWarning, SnapReason};
use super::resolved_field::ResolvedField;
use super::resolver::{
    PickedValues, apply_constraints, apply_scene_overrides, pick_and_derive_material,
};
use super::scene_overrides::SceneOverrides;
use super::value_source::ValueSource;

const PRINTER_WITH_LIGHT_INTENSITY: &str = r#"
[identity]
id = "elegoo-saturn-4-ultra"
manufacturer = "ELEGOO"
family = "Saturn"
name = "Saturn 4 Ultra"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
anti_aliasing = "off"
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
light_intensity_mw_per_cm2 = 5.0
"#;

const PRINTER_WITHOUT_LIGHT_INTENSITY: &str = r#"
[identity]
id = "elegoo-saturn-4-ultra"
manufacturer = "ELEGOO"
family = "Saturn"
name = "Saturn 4 Ultra"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
anti_aliasing = "off"
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
"#;

fn printer_with_light_intensity() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_WITH_LIGHT_INTENSITY).unwrap()
}

fn printer_without_light_intensity() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_WITHOUT_LIGHT_INTENSITY).unwrap()
}

fn material(toml: &str) -> MaterialProfile {
    toml_edit::de::from_str(toml).unwrap()
}

#[test]
fn per_printer_calibrated_values_populate_picked_fields() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;
    let (picked, warnings, records) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(2.5));
    assert_eq!(picked.bottom_exposure_sec.map(|v| v.value()), Some(30.0));
    assert_eq!(picked.bottom_layer_count, Some(8));
    assert_eq!(picked.lift_distance_mm.map(|v| v.value()), Some(5.0));
    assert_eq!(picked.lift_speed_mm_min.map(|v| v.value()), Some(120.0));
    assert_eq!(picked.retract_speed_mm_min.map(|v| v.value()), Some(150.0));
    assert!(warnings.is_empty());
    assert_eq!(records.len(), 6);
    assert!(
        records
            .iter()
            .all(|r| r.source == ValueSource::Calibrated && r.prior_value.is_none())
    );
}

#[test]
fn per_printer_wins_over_defaults_for_bottom_layer_count() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]

[defaults]
bottom_layer_count = 6

[per_printer."elegoo-saturn-4-ultra"]
bottom_layer_count = 8
"#;
    let (picked, warnings, _) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.bottom_layer_count, Some(8));
    assert!(
        !warnings.iter().any(|w| matches!(
            w,
            ResolveWarning::DefaultUsed {
                field: ResolvedField::BottomLayerCount
            }
        )),
        "DefaultUsed should not fire when per-printer supplied the value",
    );
}

#[test]
fn defaults_fill_bottom_layer_count_when_per_printer_does_not() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]

[defaults]
bottom_layer_count = 6

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
"#;
    let (picked, warnings, records) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.bottom_layer_count, Some(6));
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::DefaultUsed {
            field: ResolvedField::BottomLayerCount
        }
    )));
    let bottom_record = records
        .iter()
        .find(|r| r.field == ResolvedField::BottomLayerCount)
        .expect("expected a record for bottom_layer_count");
    assert_eq!(bottom_record.source, ValueSource::Default);
    assert_eq!(bottom_record.final_value, 6.0);
}

#[test]
fn defaults_fill_scale_compensation_without_emitting_record() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]

[defaults]
scale_compensation_pct = { x = 0.5, y = 0.5, z = 0.0 }
"#;
    let (picked, warnings, records) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert!(picked.scale_compensation_pct.is_some());
    let scale = picked.scale_compensation_pct.unwrap();
    assert_eq!(scale.x.value(), 0.5);
    assert_eq!(scale.y.value(), 0.5);
    assert_eq!(scale.z.value(), 0.0);
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::DefaultUsed {
            field: ResolvedField::ScaleCompensationPct
        }
    )));
    assert!(
        !records
            .iter()
            .any(|r| r.field == ResolvedField::ScaleCompensationPct),
        "compound fields do not get OverrideRecord entries in v0",
    );
}

#[test]
fn intent_target_layer_height_passes_through_with_derived_record_and_warning() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
"#;
    let (picked, warnings, records) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.layer_height_mm.map(|v| v.value()), Some(0.05));
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::DerivedFromIntent {
            field: ResolvedField::LayerHeightMm,
            source: IntentSource::TargetLayerHeight,
        }
    )));
    let record = records
        .iter()
        .find(|r| r.field == ResolvedField::LayerHeightMm)
        .expect("expected a record for layer_height_mm");
    assert_eq!(record.source, ValueSource::Derived);
    assert_eq!(record.final_value, 0.05);
}

#[test]
fn intent_target_energy_density_derives_normal_exposure() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.0

[defaults]
"#;
    let (picked, warnings, records) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(3.0));
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::DerivedFromIntent {
            field: ResolvedField::NormalExposureSec,
            source: IntentSource::TargetEnergyDensity,
        }
    )));
    let record = records
        .iter()
        .find(|r| r.field == ResolvedField::NormalExposureSec)
        .expect("expected a record for normal_exposure_sec");
    assert_eq!(record.source, ValueSource::Derived);
    assert_eq!(record.final_value, 3.0);
}

#[test]
fn per_printer_normal_exposure_blocks_intent_derivation() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.0

[defaults]

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
"#;
    let (picked, warnings, _) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(2.5));
    assert!(
        !warnings.iter().any(|w| matches!(
            w,
            ResolveWarning::DerivedFromIntent {
                field: ResolvedField::NormalExposureSec,
                ..
            }
        )),
        "calibrated value blocks derivation",
    );
}

#[test]
fn bottom_exposure_is_not_derived_from_energy_density() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.0

[defaults]
"#;
    let (picked, _, _) = pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert!(picked.bottom_exposure_sec.is_none());
}

#[test]
fn normal_exposure_derivation_skips_silently_without_light_intensity() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.0

[defaults]
"#;
    let (picked, warnings, _) =
        pick_and_derive_material(&printer_without_light_intensity(), &material(toml));

    assert!(picked.normal_exposure_sec.is_none());
    assert!(
        !warnings.iter().any(|w| matches!(
            w,
            ResolveWarning::DerivedFromIntent {
                field: ResolvedField::NormalExposureSec,
                ..
            }
        )),
        "no derivation warning when inputs incomplete",
    );
}

#[test]
fn normal_exposure_derivation_skips_silently_without_target_energy_density() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
"#;
    let (picked, _, _) = pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert!(picked.normal_exposure_sec.is_none());
}

#[test]
fn printer_not_in_per_printer_map_falls_through_to_defaults_and_intent_only() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05
target_energy_density_mj_per_cm2 = 15.0

[defaults]
bottom_layer_count = 6

[per_printer."some-other-printer"]
normal_exposure_sec = 2.5
"#;
    let (picked, warnings, _) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.layer_height_mm.map(|v| v.value()), Some(0.05));
    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(3.0));
    assert_eq!(picked.bottom_layer_count, Some(6));
    assert!(picked.bottom_exposure_sec.is_none());
    assert!(picked.lift_distance_mm.is_none());
    assert!(picked.lift_speed_mm_min.is_none());
    assert!(picked.retract_speed_mm_min.is_none());
    assert!(picked.scale_compensation_pct.is_none());
    assert_eq!(
        warnings
            .iter()
            .filter(|w| matches!(w, ResolveWarning::DerivedFromIntent { .. }))
            .count(),
        2,
        "both layer-height and normal-exposure derivations should warn",
    );
}

#[test]
fn empty_material_with_minimal_intent_produces_no_picked_values() {
    let toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]

[defaults]
"#;
    let (picked, warnings, records) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert!(picked.layer_height_mm.is_none());
    assert!(picked.normal_exposure_sec.is_none());
    assert!(picked.bottom_exposure_sec.is_none());
    assert!(picked.bottom_layer_count.is_none());
    assert!(warnings.is_empty());
    assert!(records.is_empty());
}

fn empty_constraints() -> PrinterConstraints {
    PrinterConstraints {
        z_step_mm: None,
        layer_height_range_mm: None,
        lift_speed_range_mm_min: None,
        retract_speed_range_mm_min: None,
        lift_distance_max_mm: None,
        light_intensity_mw_per_cm2: None,
    }
}

#[test]
fn apply_constraints_quantises_layer_height_emitting_warning_above_threshold() {
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.063).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.025).unwrap()),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    let snapped = picked.layer_height_mm.unwrap().value();
    assert!(
        (snapped - 0.075).abs() < 1e-12,
        "expected ~0.075, got {snapped}",
    );
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::Autosnap {
            field: ResolvedField::LayerHeightMm,
            reason: SnapReason::QuantisedToMultiple { .. },
            ..
        }
    )));
}

#[test]
fn apply_constraints_quantises_silently_below_threshold() {
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.0501).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    assert_eq!(picked.layer_height_mm.unwrap().value(), 0.05);
    assert!(warnings.is_empty());
}

#[test]
fn apply_constraints_clamps_layer_height_into_range_first_then_quantises() {
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.20).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.025).unwrap()),
        layer_height_range_mm: Some(
            LengthRange::new(
                PositiveFiniteMm::new(0.02).unwrap(),
                PositiveFiniteMm::new(0.10).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    assert_eq!(picked.layer_height_mm.unwrap().value(), 0.10);
    let snap_kinds: Vec<&SnapReason> = warnings
        .iter()
        .filter_map(|w| match w {
            ResolveWarning::Autosnap { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert!(
        snap_kinds
            .iter()
            .any(|r| matches!(r, SnapReason::ClampedIntoRange { .. })),
        "expected a range-clamp warning",
    );
}

#[test]
fn apply_constraints_rejects_lift_speed_below_minimum() {
    let mut picked = PickedValues {
        lift_speed_mm_min: Some(PositiveFiniteMmPerMin::new(30.0).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        lift_speed_range_mm_min: Some(
            SpeedRangeMmPerMin::new(
                PositiveFiniteMmPerMin::new(60.0).unwrap(),
                PositiveFiniteMmPerMin::new(300.0).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    let err = apply_constraints(&mut picked, &constraints, &mut warnings).unwrap_err();

    assert!(matches!(
        err,
        ResolveError::RejectedBelowMinimum {
            field: ResolvedField::LiftSpeedMmMin,
            value: 30.0,
            minimum: 60.0,
        },
    ));
}

#[test]
fn apply_constraints_snaps_lift_speed_above_max() {
    let mut picked = PickedValues {
        lift_speed_mm_min: Some(PositiveFiniteMmPerMin::new(400.0).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        lift_speed_range_mm_min: Some(
            SpeedRangeMmPerMin::new(
                PositiveFiniteMmPerMin::new(60.0).unwrap(),
                PositiveFiniteMmPerMin::new(300.0).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    assert_eq!(picked.lift_speed_mm_min.unwrap().value(), 300.0);
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::Autosnap {
            field: ResolvedField::LiftSpeedMmMin,
            reason: SnapReason::ClampedAtMaximum { .. },
            ..
        }
    )));
}

#[test]
fn apply_constraints_snaps_lift_distance_above_max() {
    let mut picked = PickedValues {
        lift_distance_mm: Some(PositiveFiniteMm::new(10.0).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        lift_distance_max_mm: Some(PositiveFiniteMm::new(8.0).unwrap()),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    assert_eq!(picked.lift_distance_mm.unwrap().value(), 8.0);
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::Autosnap {
            field: ResolvedField::LiftDistanceMm,
            ..
        }
    )));
}

#[test]
fn apply_constraints_with_no_declared_constraints_is_noop() {
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.063).unwrap()),
        lift_distance_mm: Some(PositiveFiniteMm::new(5.0).unwrap()),
        lift_speed_mm_min: Some(PositiveFiniteMmPerMin::new(120.0).unwrap()),
        retract_speed_mm_min: Some(PositiveFiniteMmPerMin::new(150.0).unwrap()),
        ..PickedValues::default()
    };
    let constraints = empty_constraints();
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    assert_eq!(picked.layer_height_mm.unwrap().value(), 0.063);
    assert_eq!(picked.lift_distance_mm.unwrap().value(), 5.0);
    assert_eq!(picked.lift_speed_mm_min.unwrap().value(), 120.0);
    assert_eq!(picked.retract_speed_mm_min.unwrap().value(), 150.0);
    assert!(warnings.is_empty());
}

#[test]
fn override_overwrites_picked_value_and_emits_override_record() {
    let mut picked = PickedValues {
        normal_exposure_sec: Some(PositiveFiniteSec::new(2.5).unwrap()),
        ..PickedValues::default()
    };
    let overrides = SceneOverrides {
        normal_exposure_sec: Some(PositiveFiniteSec::new(3.0).unwrap()),
        ..SceneOverrides::default()
    };
    let constraints = empty_constraints();
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    apply_scene_overrides(
        &overrides,
        &mut picked,
        &constraints,
        &mut warnings,
        &mut records,
    )
    .unwrap();

    assert_eq!(picked.normal_exposure_sec.unwrap().value(), 3.0);
    let record = records
        .iter()
        .find(|r| r.field == ResolvedField::NormalExposureSec)
        .expect("expected override record for normal_exposure_sec");
    assert_eq!(record.source, ValueSource::Override);
    assert_eq!(record.prior_value, Some(2.5));
    assert_eq!(record.final_value, 3.0);
}

#[test]
fn override_on_field_with_no_prior_value_records_none_for_prior() {
    let mut picked = PickedValues::default();
    let overrides = SceneOverrides {
        bottom_layer_count: Some(8),
        ..SceneOverrides::default()
    };
    let constraints = empty_constraints();
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    apply_scene_overrides(
        &overrides,
        &mut picked,
        &constraints,
        &mut warnings,
        &mut records,
    )
    .unwrap();

    assert_eq!(picked.bottom_layer_count, Some(8));
    let record = records
        .iter()
        .find(|r| r.field == ResolvedField::BottomLayerCount)
        .expect("expected override record for bottom_layer_count");
    assert_eq!(record.prior_value, None);
    assert_eq!(record.final_value, 8.0);
}

#[test]
fn override_below_minimum_returns_rejected_below_minimum_error() {
    let mut picked = PickedValues::default();
    let overrides = SceneOverrides {
        lift_speed_mm_min: Some(PositiveFiniteMmPerMin::new(30.0).unwrap()),
        ..SceneOverrides::default()
    };
    let constraints = PrinterConstraints {
        lift_speed_range_mm_min: Some(
            SpeedRangeMmPerMin::new(
                PositiveFiniteMmPerMin::new(60.0).unwrap(),
                PositiveFiniteMmPerMin::new(300.0).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    let err = apply_scene_overrides(
        &overrides,
        &mut picked,
        &constraints,
        &mut warnings,
        &mut records,
    )
    .unwrap_err();

    assert!(matches!(
        err,
        ResolveError::RejectedBelowMinimum {
            field: ResolvedField::LiftSpeedMmMin,
            value: 30.0,
            minimum: 60.0,
        },
    ));
}

#[test]
fn override_above_maximum_snaps_and_warns() {
    let mut picked = PickedValues::default();
    let overrides = SceneOverrides {
        lift_distance_mm: Some(PositiveFiniteMm::new(10.0).unwrap()),
        ..SceneOverrides::default()
    };
    let constraints = PrinterConstraints {
        lift_distance_max_mm: Some(PositiveFiniteMm::new(8.0).unwrap()),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    apply_scene_overrides(
        &overrides,
        &mut picked,
        &constraints,
        &mut warnings,
        &mut records,
    )
    .unwrap();

    assert_eq!(picked.lift_distance_mm.unwrap().value(), 8.0);
    let record = records
        .iter()
        .find(|r| r.field == ResolvedField::LiftDistanceMm)
        .expect("expected override record for lift_distance_mm");
    assert_eq!(record.source, ValueSource::Override);
    assert_eq!(record.final_value, 10.0);
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::Autosnap {
            field: ResolvedField::LiftDistanceMm,
            ..
        }
    )));
}

#[test]
fn override_on_scale_compensation_sets_value_without_emitting_record() {
    let mut picked = PickedValues::default();
    let scale = ScaleCompensation {
        x: ScaleCompensationPercent::new(1.0).unwrap(),
        y: ScaleCompensationPercent::new(1.0).unwrap(),
        z: ScaleCompensationPercent::new(0.5).unwrap(),
    };
    let overrides = SceneOverrides {
        scale_compensation_pct: Some(scale),
        ..SceneOverrides::default()
    };
    let constraints = empty_constraints();
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    apply_scene_overrides(
        &overrides,
        &mut picked,
        &constraints,
        &mut warnings,
        &mut records,
    )
    .unwrap();

    assert_eq!(picked.scale_compensation_pct.unwrap().x.value(), 1.0);
    assert!(
        !records
            .iter()
            .any(|r| r.field == ResolvedField::ScaleCompensationPct),
        "no record expected for compound override",
    );
}

#[test]
fn empty_scene_overrides_is_noop() {
    let original_picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
        normal_exposure_sec: Some(PositiveFiniteSec::new(2.5).unwrap()),
        bottom_layer_count: Some(6),
        ..PickedValues::default()
    };
    let mut picked = original_picked;
    let overrides = SceneOverrides::default();
    let constraints = empty_constraints();
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    apply_scene_overrides(
        &overrides,
        &mut picked,
        &constraints,
        &mut warnings,
        &mut records,
    )
    .unwrap();

    assert_eq!(picked, original_picked);
    assert!(warnings.is_empty());
    assert!(records.is_empty());
}

#[test]
fn override_on_layer_height_snaps_range_then_quantises() {
    let mut picked = PickedValues::default();
    let overrides = SceneOverrides {
        layer_height_mm: Some(PositiveFiniteMm::new(0.30).unwrap()),
        ..SceneOverrides::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.025).unwrap()),
        layer_height_range_mm: Some(
            LengthRange::new(
                PositiveFiniteMm::new(0.02).unwrap(),
                PositiveFiniteMm::new(0.10).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    apply_scene_overrides(
        &overrides,
        &mut picked,
        &constraints,
        &mut warnings,
        &mut records,
    )
    .unwrap();

    assert_eq!(picked.layer_height_mm.unwrap().value(), 0.10);
    let layer_record = records
        .iter()
        .find(|r| r.field == ResolvedField::LayerHeightMm)
        .expect("expected override record for layer_height_mm");
    assert_eq!(layer_record.final_value, 0.30);
}

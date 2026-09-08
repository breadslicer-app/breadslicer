/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    LengthRange, LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin,
    PositiveFiniteSec, ScaleCompensation, ScaleCompensationPercent, SpeedRangeMmPerMin,
};

use super::effective_material::EffectiveMaterial;
use super::material_profile::MaterialProfile;
use super::printer_constraints::PrinterConstraints;
use super::printer_profile::PrinterProfile;
use super::resolve_error::ResolveError;
use super::resolve_warning::{IntentSource, ResolveWarning, SnapReason};
use super::resolved_field::ResolvedField;
use super::resolved_print_config::ResolvedPrintConfig;
use super::resolver::{
    PickedValues, apply_constraints, apply_scene_overrides, pick_and_derive_material, resolve,
    resolve_layer_height, snap_to_z_step,
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
supports_grayscale = true
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
supports_grayscale = true
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

const PRINTER_WITH_COARSE_Z_STEP: &str = r#"
[identity]
id = "coarse-axis-printer"
manufacturer = "Example"
family = "Coarse"
name = "Coarse Axis"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.05
"#;

const PRINTER_WITH_MOTION_BOUNDS: &str = r#"
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
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
lift_distance_max_mm = 10.0
lift_speed_range_mm_min = { min = 60.0, max = 300.0 }
retract_speed_range_mm_min = { min = 60.0, max = 300.0 }
"#;

fn printer_with_motion_bounds() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_WITH_MOTION_BOUNDS).unwrap()
}

fn printer_with_coarse_z_step() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_WITH_COARSE_Z_STEP).unwrap()
}

const PRINTER_WITH_LAYER_HEIGHT_RANGE: &str = r#"
[identity]
id = "ranged-printer"
manufacturer = "Example"
family = "Ranged"
name = "Ranged"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
layer_height_range_mm = { min = 0.03, max = 0.1 }
"#;

fn printer_with_layer_height_range() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_WITH_LAYER_HEIGHT_RANGE).unwrap()
}

fn material(toml: &str) -> EffectiveMaterial {
    let profile: MaterialProfile = toml_edit::de::from_str(toml).unwrap();
    EffectiveMaterial::from(profile)
}

const GENERIC_MATERIAL: &str = r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer]
"#;

#[test]
fn resolve_layer_height_succeeds_without_exposure_calibration() {
    // A generic material with only an intent layer height and no
    // per-printer exposure. Full `resolve` rejects it (exposure cannot
    // resolve), but the layer height alone resolves, so a scene can still
    // be sliced into geometry.
    let printer = printer_without_light_intensity();
    let mat = material(GENERIC_MATERIAL);

    assert!(matches!(
        resolve(&printer, &mat, &SceneOverrides::default()),
        Err(ResolveError::MissingRequiredField {
            field: ResolvedField::NormalExposureSec,
        }),
    ));
    assert_eq!(
        resolve_layer_height(&printer, &mat, None).unwrap().value(),
        0.05,
    );
}

#[test]
fn resolution_honours_extension_installed_calibration() {
    // The same pairing that fails base-only in the previous test
    // resolves once the extension's calibration pair is composed in.
    let extension_toml = r#"
extension = true

[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.75
bottom_exposure_sec = 35.0
bottom_layer_count = 7
lift_distance_mm = 6.5
lift_speed_mm_min = 70.0
retract_speed_mm_min = 160.0
"#;
    let base: MaterialProfile = toml_edit::de::from_str(GENERIC_MATERIAL).unwrap();
    let extension: MaterialProfile = toml_edit::de::from_str(extension_toml).unwrap();
    let (effective, issues) = EffectiveMaterial::compose(base, Some(extension));
    assert!(issues.is_empty(), "issues: {issues:?}");

    let printer = printer_without_light_intensity();
    let resolved = resolve(&printer, &effective, &SceneOverrides::default()).unwrap();
    let values = &resolved.material_resolved;
    assert_eq!(values.normal_exposure_sec.value(), 2.75);
    assert_eq!(values.bottom_exposure_sec.value(), 35.0);
    assert_eq!(values.bottom_layer_count, 7);
    assert_eq!(values.lift_distance_mm.value(), 6.5);
    assert_eq!(values.lift_speed_mm_min.value(), 70.0);
    assert_eq!(values.retract_speed_mm_min.value(), 160.0);
}

const MATERIAL_WITHOUT_LAYER_HEIGHT: &str = r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer]
"#;

#[test]
fn resolve_layer_height_errors_when_no_layer_height_is_available() {
    assert!(matches!(
        resolve_layer_height(
            &printer_without_light_intensity(),
            &material(MATERIAL_WITHOUT_LAYER_HEIGHT),
            None,
        ),
        Err(ResolveError::MissingRequiredField {
            field: ResolvedField::LayerHeightMm,
        }),
    ));
}

#[test]
fn resolve_layer_height_snaps_a_below_half_step_intent_up_to_one_step() {
    // 0.02 mm against a 0.05 mm z step rounds to 0 multiples, which
    // is not a printable height; the resolved height is the first
    // multiple, never 0.
    let toml = r#"
[identity]
id = "fine-detail"
brand = "Default"
family = "standard"
name = "Fine Detail"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.02

[defaults]

[per_printer]
"#;
    let resolved =
        resolve_layer_height(&printer_with_coarse_z_step(), &material(toml), None).unwrap();
    assert_eq!(resolved.value(), 0.05);
}

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).expect("test lengths are positive finite")
}

#[test]
fn snap_to_z_step_quantises_onto_the_grid() {
    // 0.08 against a 0.05 step: 1.6 multiples rounds to 2, giving 0.1.
    let constraints = printer_with_coarse_z_step().constraints;
    assert_eq!(snap_to_z_step(&constraints, mm(0.08)).value(), 0.1);
}

#[test]
fn snap_to_z_step_keeps_an_on_grid_value_unchanged() {
    let constraints = printer_with_coarse_z_step().constraints;
    assert_eq!(snap_to_z_step(&constraints, mm(0.1)).value(), 0.1);
}

#[test]
fn snap_to_z_step_snaps_below_half_a_step_up_to_one_step() {
    // Same totality as the layer-height chain: 0 multiples is not a
    // printable height, so 0.02 snaps up to 0.05, never to 0.
    let constraints = printer_with_coarse_z_step().constraints;
    assert_eq!(snap_to_z_step(&constraints, mm(0.02)).value(), 0.05);
}

#[test]
fn snap_to_z_step_without_a_declared_step_is_identity() {
    // The ranged printer declares no z step; the helper passes the
    // value through.
    let constraints = printer_with_layer_height_range().constraints;
    assert_eq!(snap_to_z_step(&constraints, mm(0.037)).value(), 0.037);
}

#[test]
fn snap_to_z_step_ignores_the_layer_height_range() {
    // 0.2 sits far outside the declared [0.03, 0.1] range; the helper
    // quantises only, it never clamps into the range.
    let constraints = printer_with_layer_height_range().constraints;
    assert_eq!(snap_to_z_step(&constraints, mm(0.2)).value(), 0.2);
}

#[test]
fn a_requested_layer_height_replaces_the_picked_value() {
    // The material's intent says 0.05; the user asks for 0.03, which
    // satisfies the 0.01 z step and wins.
    let resolved = resolve_layer_height(
        &printer_without_light_intensity(),
        &material(GENERIC_MATERIAL),
        Some(mm(0.03)),
    )
    .unwrap();
    assert_eq!(resolved.value(), 0.03);
}

#[test]
fn a_requested_layer_height_is_snapped_to_the_z_step() {
    // 0.08 against a 0.05 z step: 1.6 multiples rounds to 2, so the
    // requested height snaps to 0.1, through the same policy chain a
    // profile-supplied value runs.
    let resolved = resolve_layer_height(
        &printer_with_coarse_z_step(),
        &material(GENERIC_MATERIAL),
        Some(mm(0.08)),
    )
    .unwrap();
    assert_eq!(resolved.value(), 0.1);
}

#[test]
fn a_requested_height_below_half_a_step_snaps_up_to_one_step() {
    // Mirror of the intent-value totality pin: a request rounding to
    // 0 multiples resolves to the first multiple, never 0.
    let resolved = resolve_layer_height(
        &printer_with_coarse_z_step(),
        &material(GENERIC_MATERIAL),
        Some(mm(0.02)),
    )
    .unwrap();
    assert_eq!(resolved.value(), 0.05);
}

#[test]
fn a_requested_layer_height_is_clamped_into_the_declared_range() {
    // Range [0.03, 0.1], request 0.2: clamps to the range maximum.
    let resolved = resolve_layer_height(
        &printer_with_layer_height_range(),
        &material(GENERIC_MATERIAL),
        Some(mm(0.2)),
    )
    .unwrap();
    assert_eq!(resolved.value(), 0.1);
}

#[test]
fn a_requested_height_resolves_even_without_a_material_layer_height() {
    // With a request present the material's missing layer height is
    // irrelevant, mirroring the full resolver's override arm.
    let resolved = resolve_layer_height(
        &printer_without_light_intensity(),
        &material(MATERIAL_WITHOUT_LAYER_HEIGHT),
        Some(mm(0.05)),
    )
    .unwrap();
    assert_eq!(resolved.value(), 0.05);
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

const GENERIC_PRINTER: &str = r#"
[identity]
id = "generic-machine"
manufacturer = "SoulCrafted"
family = "Generic"
name = "Generic Machine"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
"#;

fn generic_printer() -> PrinterProfile {
    toml_edit::de::from_str(GENERIC_PRINTER).unwrap()
}

const GENERIC_CALIBRATED_MATERIAL: &str = r#"
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

[per_printer."generic-machine"]
normal_exposure_sec = 4.0
bottom_exposure_sec = 40.0
bottom_layer_count = 4
lift_distance_mm = 7.0
lift_speed_mm_min = 60.0
retract_speed_mm_min = 180.0
"#;

fn assert_generic_values(picked: &PickedValues) {
    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(4.0));
    assert_eq!(picked.bottom_exposure_sec.map(|v| v.value()), Some(40.0));
    assert_eq!(picked.bottom_layer_count, Some(4));
    assert_eq!(picked.lift_distance_mm.map(|v| v.value()), Some(7.0));
    assert_eq!(picked.lift_speed_mm_min.map(|v| v.value()), Some(60.0));
    assert_eq!(picked.retract_speed_mm_min.map(|v| v.value()), Some(180.0));
}

fn generic_warning_count(warnings: &[ResolveWarning]) -> usize {
    warnings
        .iter()
        .filter(|w| matches!(w, ResolveWarning::GenericCalibrationUsed))
        .count()
}

#[test]
fn generic_block_applies_to_a_printer_without_its_own_block() {
    let (picked, warnings, records) = pick_and_derive_material(
        &printer_with_light_intensity(),
        &material(GENERIC_CALIBRATED_MATERIAL),
    );

    assert_generic_values(&picked);
    assert_eq!(generic_warning_count(&warnings), 1);
    let calibrated = records
        .iter()
        .filter(|r| r.source == ValueSource::Calibrated)
        .count();
    assert_eq!(calibrated, 6);
}

#[test]
fn own_block_wins_over_the_generic_block() {
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

[per_printer."generic-machine"]
normal_exposure_sec = 4.0
bottom_exposure_sec = 40.0
bottom_layer_count = 4
lift_distance_mm = 7.0
lift_speed_mm_min = 60.0
retract_speed_mm_min = 180.0
"#;
    let (picked, warnings, _) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(2.5));
    assert_eq!(picked.bottom_exposure_sec.map(|v| v.value()), Some(30.0));
    assert_eq!(picked.bottom_layer_count, Some(8));
    assert_eq!(picked.lift_distance_mm.map(|v| v.value()), Some(5.0));
    assert_eq!(picked.lift_speed_mm_min.map(|v| v.value()), Some(120.0));
    assert_eq!(picked.retract_speed_mm_min.map(|v| v.value()), Some(150.0));
    assert_eq!(generic_warning_count(&warnings), 0);
}

#[test]
fn own_block_gaps_do_not_fall_to_the_generic_block() {
    // The fallback is per block: once the printer's own block is
    // picked, the generic block is not consulted for the fields the
    // own block leaves unset.
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

[per_printer."generic-machine"]
lift_speed_mm_min = 60.0
"#;
    let (picked, warnings, _) =
        pick_and_derive_material(&printer_with_light_intensity(), &material(toml));

    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(2.5));
    assert!(picked.lift_speed_mm_min.is_none());
    assert_eq!(generic_warning_count(&warnings), 0);
}

#[test]
fn the_generic_printer_uses_its_block_as_its_own() {
    let (picked, warnings, _) =
        pick_and_derive_material(&generic_printer(), &material(GENERIC_CALIBRATED_MATERIAL));

    assert_generic_values(&picked);
    assert_eq!(generic_warning_count(&warnings), 0);
}

#[test]
fn a_generic_block_added_by_the_extension_applies() {
    let extension_toml = r#"
extension = true

[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."generic-machine"]
normal_exposure_sec = 4.0
bottom_exposure_sec = 40.0
bottom_layer_count = 4
lift_distance_mm = 7.0
lift_speed_mm_min = 60.0
retract_speed_mm_min = 180.0
"#;
    let base: MaterialProfile = toml_edit::de::from_str(GENERIC_MATERIAL).unwrap();
    let extension: MaterialProfile = toml_edit::de::from_str(extension_toml).unwrap();
    let (effective, issues) = EffectiveMaterial::compose(base, Some(extension));
    assert!(issues.is_empty(), "issues: {issues:?}");

    let (picked, warnings, _) =
        pick_and_derive_material(&printer_with_light_intensity(), &effective);

    assert_generic_values(&picked);
    assert_eq!(generic_warning_count(&warnings), 1);
}

#[test]
fn resolve_succeeds_for_a_generic_only_material_on_another_printer() {
    let resolved = resolve(
        &printer_with_light_intensity(),
        &material(GENERIC_CALIBRATED_MATERIAL),
        &SceneOverrides::default(),
    )
    .unwrap();

    let values = &resolved.material_resolved;
    assert_eq!(values.normal_exposure_sec.value(), 4.0);
    assert_eq!(values.bottom_exposure_sec.value(), 40.0);
    assert_eq!(values.bottom_layer_count, 4);
    assert_eq!(values.lift_distance_mm.value(), 7.0);
    assert_eq!(values.lift_speed_mm_min.value(), 60.0);
    assert_eq!(values.retract_speed_mm_min.value(), 180.0);
    assert_eq!(generic_warning_count(&resolved.warnings), 1);
}

#[test]
fn resolve_layer_height_is_unchanged_by_a_generic_block() {
    let resolved = resolve_layer_height(
        &printer_with_light_intensity(),
        &material(GENERIC_CALIBRATED_MATERIAL),
        None,
    )
    .unwrap();

    assert_eq!(resolved.value(), 0.05);
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
fn apply_constraints_snaps_a_below_half_step_layer_height_up_to_one_step() {
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.02).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    assert_eq!(picked.layer_height_mm.unwrap().value(), 0.05);
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
fn apply_constraints_snaps_below_half_step_even_inside_a_declared_range() {
    // The range clamp runs before the quantiser, so a value inside
    // the declared range but below half a step still reaches it; the
    // declared range is no guard against the snap-up.
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.012).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
        layer_height_range_mm: Some(
            LengthRange::new(
                PositiveFiniteMm::new(0.01).unwrap(),
                PositiveFiniteMm::new(0.10).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let mut warnings = Vec::new();
    apply_constraints(&mut picked, &constraints, &mut warnings).unwrap();

    assert_eq!(picked.layer_height_mm.unwrap().value(), 0.05);
}

#[test]
fn apply_constraints_leaves_layer_height_unchanged_under_a_subnormal_step() {
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
        ..PickedValues::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(5e-324).unwrap()),
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

#[test]
fn override_on_layer_height_below_half_a_step_snaps_up_to_one_step() {
    let mut picked = PickedValues {
        layer_height_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
        ..PickedValues::default()
    };
    let overrides = SceneOverrides {
        layer_height_mm: Some(PositiveFiniteMm::new(0.02).unwrap()),
        ..SceneOverrides::default()
    };
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
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

    assert_eq!(picked.layer_height_mm.unwrap().value(), 0.05);
    assert!(warnings.iter().any(|w| matches!(
        w,
        ResolveWarning::Autosnap {
            field: ResolvedField::LayerHeightMm,
            reason: SnapReason::QuantisedToMultiple { .. },
            ..
        }
    )));
}

const FULLY_CALIBRATED_MATERIAL: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.5, y = 0.5, z = 0.0 }

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

#[test]
fn resolve_assembles_fully_populated_config_from_per_printer_block() {
    let printer = printer_with_light_intensity();
    let material = material(FULLY_CALIBRATED_MATERIAL);
    let overrides = SceneOverrides::default();

    let resolved = resolve(&printer, &material, &overrides).unwrap();

    assert_eq!(resolved.material_resolved.layer_height_mm.value(), 0.05);
    assert_eq!(resolved.material_resolved.normal_exposure_sec.value(), 2.5);
    assert_eq!(resolved.material_resolved.bottom_exposure_sec.value(), 30.0);
    assert_eq!(resolved.material_resolved.bottom_layer_count, 8);
    assert_eq!(resolved.material_resolved.lift_distance_mm.value(), 5.0);
    assert_eq!(resolved.material_resolved.lift_speed_mm_min.value(), 120.0);
    assert_eq!(
        resolved.material_resolved.retract_speed_mm_min.value(),
        150.0
    );
    assert_eq!(
        resolved.material_resolved.scale_compensation_pct.z.value(),
        0.0
    );
    assert_eq!(resolved.printer, printer);
}

#[test]
fn resolve_errors_with_missing_required_field_when_no_layer_present() {
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
    let printer = printer_with_light_intensity();
    let material = material(toml);
    let overrides = SceneOverrides::default();

    let err = resolve(&printer, &material, &overrides).unwrap_err();
    assert!(matches!(err, ResolveError::MissingRequiredField { .. }));
}

#[test]
fn resolve_reports_first_missing_field_in_material_resolved_field_order() {
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
    let printer = printer_with_light_intensity();
    let material = material(toml);
    let overrides = SceneOverrides::default();

    let err = resolve(&printer, &material, &overrides).unwrap_err();
    assert!(matches!(
        err,
        ResolveError::MissingRequiredField {
            field: ResolvedField::LayerHeightMm
        },
    ));
}

#[test]
fn resolve_collects_records_from_each_pipeline_layer() {
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
bottom_layer_count = 6
scale_compensation_pct = { x = 0.5, y = 0.5, z = 0.0 }

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;
    let printer = printer_with_light_intensity();
    let material = material(toml);
    let overrides = SceneOverrides {
        lift_distance_mm: Some(PositiveFiniteMm::new(6.0).unwrap()),
        ..SceneOverrides::default()
    };

    let resolved = resolve(&printer, &material, &overrides).unwrap();

    let sources: Vec<ValueSource> = resolved
        .overrides_applied
        .iter()
        .map(|r| r.source)
        .collect();
    assert!(
        sources.contains(&ValueSource::Calibrated),
        "expected at least one calibrated record, got {sources:?}",
    );
    assert!(
        sources.contains(&ValueSource::Default),
        "expected at least one default record, got {sources:?}",
    );
    assert!(
        sources.contains(&ValueSource::Derived),
        "expected at least one derived record, got {sources:?}",
    );
    assert!(
        sources.contains(&ValueSource::Override),
        "expected at least one override record, got {sources:?}",
    );
    assert_eq!(resolved.material_resolved.lift_distance_mm.value(), 6.0);
}

#[test]
fn resolve_propagates_constraint_rejection_from_step_three() {
    let printer_toml = r#"
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
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
lift_speed_range_mm_min = { min = 60.0, max = 300.0 }
"#;
    let material_toml = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.5, y = 0.5, z = 0.0 }

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 30.0
retract_speed_mm_min = 150.0
"#;
    let printer: PrinterProfile = toml_edit::de::from_str(printer_toml).unwrap();
    let mat = material(material_toml);
    let overrides = SceneOverrides::default();

    let err = resolve(&printer, &mat, &overrides).unwrap_err();
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
fn resolve_propagates_constraint_rejection_from_override() {
    let printer_toml = r#"
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
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
lift_speed_range_mm_min = { min = 60.0, max = 300.0 }
"#;
    let printer: PrinterProfile = toml_edit::de::from_str(printer_toml).unwrap();
    let mat = material(FULLY_CALIBRATED_MATERIAL);
    let overrides = SceneOverrides {
        lift_speed_mm_min: Some(PositiveFiniteMmPerMin::new(30.0).unwrap()),
        ..SceneOverrides::default()
    };

    let err = resolve(&printer, &mat, &overrides).unwrap_err();
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
fn resolve_returns_collected_warnings_in_assembled_config() {
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
bottom_layer_count = 6
scale_compensation_pct = { x = 0.5, y = 0.5, z = 0.0 }

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;
    let printer = printer_with_light_intensity();
    let material = material(toml);
    let overrides = SceneOverrides::default();

    let resolved = resolve(&printer, &material, &overrides).unwrap();

    let has_default_used = resolved
        .warnings
        .iter()
        .any(|w| matches!(w, ResolveWarning::DefaultUsed { .. }));
    let has_derived_from_intent = resolved
        .warnings
        .iter()
        .any(|w| matches!(w, ResolveWarning::DerivedFromIntent { .. }));
    assert!(
        has_default_used,
        "expected at least one DefaultUsed warning"
    );
    assert!(
        has_derived_from_intent,
        "expected at least one DerivedFromIntent warning",
    );
}

/// A material that resolves fully against
/// [`printer_without_light_intensity`], taking the wait keys of its
/// defaults block and of its per-printer block from the caller.
fn material_with_waits(defaults_waits: &str, block_waits: &str) -> EffectiveMaterial {
    material(&format!(
        r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5
{defaults_waits}

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.75
bottom_exposure_sec = 35.0
lift_distance_mm = 6.5
lift_speed_mm_min = 70.0
retract_speed_mm_min = 160.0
{block_waits}
"#
    ))
}

fn resolve_with_waits(defaults_waits: &str, block_waits: &str) -> ResolvedPrintConfig {
    resolve(
        &printer_without_light_intensity(),
        &material_with_waits(defaults_waits, block_waits),
        &SceneOverrides::default(),
    )
    .unwrap()
}

fn defaulted(warnings: &[ResolveWarning], field: ResolvedField) -> bool {
    warnings.iter().any(
        |warning| matches!(warning, ResolveWarning::DefaultUsed { field: got } if *got == field),
    )
}

#[test]
fn calibrated_waits_reach_the_resolved_material() {
    let resolved = resolve_with_waits(
        "",
        "wait_before_cure_sec = 0.5\nwait_after_cure_sec = 1.5\nwait_after_lift_sec = 2.0",
    );
    let values = &resolved.material_resolved;

    assert_eq!(values.wait_before_cure_sec.value(), 0.5);
    assert_eq!(values.wait_after_cure_sec.value(), 1.5);
    assert_eq!(values.wait_after_lift_sec.value(), 2.0);
}

#[test]
fn a_wait_missing_from_the_block_falls_back_to_the_material_default() {
    let resolved = resolve_with_waits("wait_after_cure_sec = 1.25", "");

    assert_eq!(resolved.material_resolved.wait_after_cure_sec.value(), 1.25);
    assert!(
        defaulted(&resolved.warnings, ResolvedField::WaitAfterCureSec),
        "warnings: {:?}",
        resolved.warnings,
    );
}

#[test]
fn a_calibrated_wait_outranks_the_material_default() {
    let resolved = resolve_with_waits("wait_after_cure_sec = 1.25", "wait_after_cure_sec = 0.5");

    assert_eq!(resolved.material_resolved.wait_after_cure_sec.value(), 0.5);
    assert!(!defaulted(
        &resolved.warnings,
        ResolvedField::WaitAfterCureSec
    ));
}

#[test]
fn a_wait_nobody_calibrated_resolves_to_no_pause() {
    let resolved = resolve_with_waits("", "");
    let values = &resolved.material_resolved;

    assert_eq!(values.wait_before_cure_sec.value(), 0.0);
    assert_eq!(values.wait_after_cure_sec.value(), 0.0);
    assert_eq!(values.wait_after_lift_sec.value(), 0.0);
    for field in [
        ResolvedField::WaitBeforeCureSec,
        ResolvedField::WaitAfterCureSec,
        ResolvedField::WaitAfterLiftSec,
    ] {
        assert!(
            !defaulted(&resolved.warnings, field),
            "{field} is zero because the resin asked for no pause, not by falling back",
        );
    }
}

/// A resin that names a target energy density and drives the light
/// at `pwm_percent` on the printer with a declared light intensity.
fn material_at_light_pwm(pwm_percent: &str) -> EffectiveMaterial {
    material(&format!(
        r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.0

[defaults]
light_pwm = {pwm_percent}
"#
    ))
}

#[test]
fn a_light_at_full_power_derives_the_exposure_the_intensity_alone_gives() {
    let (picked, _, _) = pick_and_derive_material(
        &printer_with_light_intensity(),
        &material_at_light_pwm("100.0"),
    );

    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(3.0));
}

#[test]
fn halving_the_light_pwm_doubles_the_derived_normal_exposure() {
    let (picked, _, _) = pick_and_derive_material(
        &printer_with_light_intensity(),
        &material_at_light_pwm("50.0"),
    );

    assert_eq!(picked.normal_exposure_sec.map(|v| v.value()), Some(6.0));
}

#[test]
fn a_light_nobody_dialled_down_derives_at_full_power() {
    let (picked, _, _) = pick_and_derive_material(
        &printer_with_light_intensity(),
        &material_at_light_pwm("100.0"),
    );
    let (unset, _, _) = pick_and_derive_material(
        &printer_with_light_intensity(),
        &material(
            r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.0

[defaults]
"#,
        ),
    );

    assert_eq!(unset.normal_exposure_sec, picked.normal_exposure_sec);
}

#[test]
fn a_light_switched_off_leaves_the_exposure_underivable() {
    let (picked, _, _) = pick_and_derive_material(
        &printer_with_light_intensity(),
        &material_at_light_pwm("0.0"),
    );

    assert!(picked.normal_exposure_sec.is_none());
}

#[test]
fn calibrated_bottom_waits_reach_the_resolved_material() {
    let resolved = resolve_with_waits(
        "",
        "bottom_wait_before_cure_sec = 3.0\n\
         bottom_wait_after_cure_sec = 4.0\n\
         bottom_wait_after_lift_sec = 5.0",
    );
    let values = &resolved.material_resolved;

    assert_eq!(values.bottom_wait_before_cure_sec.value(), 3.0);
    assert_eq!(values.bottom_wait_after_cure_sec.value(), 4.0);
    assert_eq!(values.bottom_wait_after_lift_sec.value(), 5.0);
}

#[test]
fn an_uncalibrated_bottom_wait_takes_the_calibrated_normal_one() {
    let resolved = resolve_with_waits("", "wait_after_lift_sec = 2.0");
    let values = &resolved.material_resolved;

    assert_eq!(values.wait_after_lift_sec.value(), 2.0);
    assert_eq!(values.bottom_wait_after_lift_sec.value(), 2.0);
}

#[test]
fn an_uncalibrated_bottom_wait_follows_the_normal_one_down_to_the_material_default() {
    let resolved = resolve_with_waits("wait_after_cure_sec = 1.25", "");

    assert_eq!(
        resolved
            .material_resolved
            .bottom_wait_after_cure_sec
            .value(),
        1.25,
    );
}

#[test]
fn a_bottom_wait_nobody_calibrated_at_all_resolves_to_no_pause() {
    let resolved = resolve_with_waits("", "");
    let values = &resolved.material_resolved;

    assert_eq!(values.bottom_wait_before_cure_sec.value(), 0.0);
    assert_eq!(values.bottom_wait_after_cure_sec.value(), 0.0);
    assert_eq!(values.bottom_wait_after_lift_sec.value(), 0.0);
}

#[test]
fn a_calibrated_bottom_wait_stands_apart_from_the_normal_one() {
    let resolved = resolve_with_waits(
        "",
        "wait_after_cure_sec = 0.5\nbottom_wait_after_cure_sec = 4.0",
    );
    let values = &resolved.material_resolved;

    assert_eq!(values.wait_after_cure_sec.value(), 0.5);
    assert_eq!(values.bottom_wait_after_cure_sec.value(), 4.0);
}

#[test]
fn a_scene_override_replaces_a_calibrated_bottom_wait_and_records_the_prior_value() {
    let overrides = SceneOverrides {
        bottom_wait_before_cure_sec: Some(NonNegativeFiniteSec::new(6.0).unwrap()),
        ..Default::default()
    };
    let resolved = resolve(
        &printer_without_light_intensity(),
        &material_with_waits("", "bottom_wait_before_cure_sec = 3.0"),
        &overrides,
    )
    .unwrap();

    assert_eq!(
        resolved
            .material_resolved
            .bottom_wait_before_cure_sec
            .value(),
        6.0,
    );
    let record = resolved
        .overrides_applied
        .iter()
        .find(|record| {
            record.field == ResolvedField::BottomWaitBeforeCureSec
                && record.source == ValueSource::Override
        })
        .expect("the override should be recorded");
    assert_eq!(record.prior_value, Some(3.0));
    assert_eq!(record.final_value, 6.0);
}

#[test]
fn a_scene_override_on_a_normal_wait_carries_into_an_uncalibrated_bottom_wait() {
    let overrides = SceneOverrides {
        wait_after_lift_sec: Some(NonNegativeFiniteSec::new(3.0).unwrap()),
        ..Default::default()
    };
    let resolved = resolve(
        &printer_without_light_intensity(),
        &material_with_waits("", "wait_after_lift_sec = 2.0"),
        &overrides,
    )
    .unwrap();

    assert_eq!(
        resolved
            .material_resolved
            .bottom_wait_after_lift_sec
            .value(),
        3.0,
    );
}

/// A material that resolves fully against
/// [`printer_without_light_intensity`], taking the bottom-motion keys
/// of its per-printer block from the caller.
fn material_with_bottom_motion(block_motion: &str) -> EffectiveMaterial {
    material(&format!(
        r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.75
bottom_exposure_sec = 35.0
lift_distance_mm = 6.5
lift_speed_mm_min = 70.0
retract_speed_mm_min = 160.0
{block_motion}
"#
    ))
}

#[test]
fn calibrated_bottom_motion_reaches_the_resolved_material() {
    let resolved = resolve(
        &printer_without_light_intensity(),
        &material_with_bottom_motion(
            "bottom_lift_distance_mm = 8.0\n\
             bottom_lift_speed_mm_min = 40.0\n\
             bottom_retract_speed_mm_min = 90.0",
        ),
        &SceneOverrides::default(),
    )
    .unwrap();
    let values = &resolved.material_resolved;

    assert_eq!(values.bottom_lift_distance_mm.value(), 8.0);
    assert_eq!(values.bottom_lift_speed_mm_min.value(), 40.0);
    assert_eq!(values.bottom_retract_speed_mm_min.value(), 90.0);
}

#[test]
fn uncalibrated_bottom_motion_takes_the_normal_layer_travel() {
    let resolved = resolve(
        &printer_without_light_intensity(),
        &material_with_bottom_motion(""),
        &SceneOverrides::default(),
    )
    .unwrap();
    let values = &resolved.material_resolved;

    assert_eq!(values.bottom_lift_distance_mm, values.lift_distance_mm);
    assert_eq!(values.bottom_lift_speed_mm_min, values.lift_speed_mm_min);
    assert_eq!(
        values.bottom_retract_speed_mm_min,
        values.retract_speed_mm_min
    );
}

#[test]
fn a_bottom_lift_speed_below_the_printer_minimum_is_rejected() {
    let error = resolve(
        &printer_with_motion_bounds(),
        &material_with_bottom_motion("bottom_lift_speed_mm_min = 1.0"),
        &SceneOverrides::default(),
    )
    .unwrap_err();

    assert!(
        matches!(
            error,
            ResolveError::RejectedBelowMinimum {
                field: ResolvedField::BottomLiftSpeedMmMin,
                ..
            }
        ),
        "error: {error:?}",
    );
}

#[test]
fn a_bottom_lift_distance_above_the_printer_maximum_snaps_down() {
    let resolved = resolve(
        &printer_with_motion_bounds(),
        &material_with_bottom_motion("bottom_lift_distance_mm = 500.0"),
        &SceneOverrides::default(),
    )
    .unwrap();

    assert_eq!(
        resolved.material_resolved.bottom_lift_distance_mm.value(),
        10.0,
    );
}

/// A material that resolves fully against
/// [`printer_without_light_intensity`], taking the light-PWM keys of
/// its defaults block and of its per-printer block from the caller.
fn material_with_pwm(defaults_pwm: &str, block_pwm: &str) -> EffectiveMaterial {
    material(&format!(
        r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5
{defaults_pwm}

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.75
bottom_exposure_sec = 35.0
lift_distance_mm = 6.5
lift_speed_mm_min = 70.0
retract_speed_mm_min = 160.0
{block_pwm}
"#
    ))
}

fn resolve_with_pwm(defaults_pwm: &str, block_pwm: &str) -> ResolvedPrintConfig {
    resolve(
        &printer_without_light_intensity(),
        &material_with_pwm(defaults_pwm, block_pwm),
        &SceneOverrides::default(),
    )
    .unwrap()
}

#[test]
fn calibrated_light_pwms_reach_the_resolved_material() {
    let resolved = resolve_with_pwm("", "light_pwm = 80.0\nbottom_light_pwm = 60.0");
    let values = &resolved.material_resolved;

    assert_eq!(values.light_pwm.percent(), 80.0);
    assert_eq!(values.bottom_light_pwm.percent(), 60.0);
}

#[test]
fn a_light_pwm_missing_from_the_block_falls_back_to_the_material_default() {
    let resolved = resolve_with_pwm("light_pwm = 70.0", "");

    assert_eq!(resolved.material_resolved.light_pwm.percent(), 70.0);
    assert!(
        defaulted(&resolved.warnings, ResolvedField::LightPwm),
        "warnings: {:?}",
        resolved.warnings,
    );
}

#[test]
fn a_calibrated_light_pwm_outranks_the_material_default() {
    let resolved = resolve_with_pwm("light_pwm = 70.0", "light_pwm = 80.0");

    assert_eq!(resolved.material_resolved.light_pwm.percent(), 80.0);
    assert!(!defaulted(&resolved.warnings, ResolvedField::LightPwm));
}

#[test]
fn a_light_pwm_nobody_calibrated_resolves_to_full_power() {
    let resolved = resolve_with_pwm("", "");
    let values = &resolved.material_resolved;

    assert_eq!(values.light_pwm, LightPwm::FULL);
    assert_eq!(values.bottom_light_pwm, LightPwm::FULL);
    for field in [ResolvedField::LightPwm, ResolvedField::BottomLightPwm] {
        assert!(
            !defaulted(&resolved.warnings, field),
            "{field} is full power because nothing dialled it down, not by falling back",
        );
    }
}

#[test]
fn the_bottom_light_pwm_does_not_follow_the_normal_one() {
    let resolved = resolve_with_pwm("", "light_pwm = 40.0");
    let values = &resolved.material_resolved;

    assert_eq!(values.light_pwm.percent(), 40.0);
    assert_eq!(values.bottom_light_pwm, LightPwm::FULL);
}

#[test]
fn a_scene_override_replaces_a_calibrated_light_pwm_and_records_the_prior_value() {
    let overrides = SceneOverrides {
        light_pwm: Some(LightPwm::new(55.0).unwrap()),
        ..Default::default()
    };
    let resolved = resolve(
        &printer_without_light_intensity(),
        &material_with_pwm("", "light_pwm = 80.0"),
        &overrides,
    )
    .unwrap();

    assert_eq!(resolved.material_resolved.light_pwm.percent(), 55.0);
    let record = resolved
        .overrides_applied
        .iter()
        .find(|record| {
            record.field == ResolvedField::LightPwm && record.source == ValueSource::Override
        })
        .expect("the override should be recorded");
    assert_eq!(record.prior_value, Some(80.0));
    assert_eq!(record.final_value, 55.0);
}

#[test]
fn a_scene_override_replaces_a_calibrated_wait_and_records_the_prior_value() {
    let overrides = SceneOverrides {
        wait_after_lift_sec: Some(NonNegativeFiniteSec::new(3.0).unwrap()),
        ..Default::default()
    };
    let resolved = resolve(
        &printer_without_light_intensity(),
        &material_with_waits("", "wait_after_lift_sec = 2.0"),
        &overrides,
    )
    .unwrap();

    assert_eq!(resolved.material_resolved.wait_after_lift_sec.value(), 3.0);
    let record = resolved
        .overrides_applied
        .iter()
        .find(|record| {
            record.field == ResolvedField::WaitAfterLiftSec
                && record.source == ValueSource::Override
        })
        .expect("the override should be recorded");
    assert_eq!(record.prior_value, Some(2.0));
    assert_eq!(record.final_value, 3.0);
}

/// A resin calibrated for the test printer, with `[physical]` left to
/// the caller so a test can vary just that section.
fn material_with_physical(physical: &str) -> EffectiveMaterial {
    material(&format!(
        r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = {{ x = 0.0, y = 0.0, z = 0.0 }}

{physical}

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.75
bottom_exposure_sec = 35.0
bottom_layer_count = 7
lift_distance_mm = 6.5
lift_speed_mm_min = 70.0
retract_speed_mm_min = 160.0
"#,
    ))
}

fn resolve_with_physical(physical: &str) -> ResolvedPrintConfig {
    resolve(
        &printer_without_light_intensity(),
        &material_with_physical(physical),
        &SceneOverrides::default(),
    )
    .unwrap()
}

#[test]
fn a_resin_stating_no_cured_density_resolves_to_a_typical_photopolymer() {
    let resolved = resolve_with_physical("");
    assert_eq!(
        resolved.material_resolved.cured_density_g_per_cm3.value(),
        1.17,
    );
}

#[test]
fn a_stated_cured_density_resolves_to_itself() {
    let resolved = resolve_with_physical("[physical]\ncured_density_g_per_cm3 = 1.6");
    assert_eq!(
        resolved.material_resolved.cured_density_g_per_cm3.value(),
        1.6,
    );
}

#[test]
fn a_stated_cured_density_is_recorded_as_the_profile_s_own() {
    let resolved = resolve_with_physical("[physical]\ncured_density_g_per_cm3 = 1.15");
    let record = resolved
        .overrides_applied
        .iter()
        .find(|record| record.field == ResolvedField::CuredDensityGPerCm3)
        .expect("the stated density should be recorded");
    assert_eq!(record.final_value, 1.15);
    assert_eq!(record.source, ValueSource::Default);
}

#[test]
fn an_unstated_cured_density_leaves_no_record_behind() {
    let resolved = resolve_with_physical("");
    assert!(
        !resolved
            .overrides_applied
            .iter()
            .any(|record| record.field == ResolvedField::CuredDensityGPerCm3),
        "records: {:?}",
        resolved.overrides_applied,
    );
}

#[test]
fn a_stated_cured_density_raises_no_warning() {
    let resolved = resolve_with_physical("[physical]\ncured_density_g_per_cm3 = 1.15");
    assert!(
        !resolved.warnings.iter().any(|warning| matches!(
            warning,
            ResolveWarning::DefaultUsed {
                field: ResolvedField::CuredDensityGPerCm3
            }
        )),
        "warnings: {:?}",
        resolved.warnings,
    );
}

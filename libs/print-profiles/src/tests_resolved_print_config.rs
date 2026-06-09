/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    MaterialId, NonEmptyString, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
    ScaleCompensation, ScaleCompensationPercent,
};
use semver::Version;

use super::material_identity::MaterialIdentity;
use super::material_resolved::MaterialResolved;
use super::override_record::OverrideRecord;
use super::printer_profile::PrinterProfile;
use super::resolve_warning::ResolveWarning;
use super::resolved_field::ResolvedField;
use super::resolved_print_config::ResolvedPrintConfig;
use super::value_source::ValueSource;

const PRINTER_TOML: &str = r#"
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

fn sample_printer_profile() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_TOML).unwrap()
}

fn sample_material_resolved() -> MaterialResolved {
    MaterialResolved {
        identity: MaterialIdentity {
            id: MaterialId::new("elegoo-resin-grey").unwrap(),
            brand: NonEmptyString::new("Elegoo").unwrap(),
            family: NonEmptyString::new("Standard Resin").unwrap(),
            name: NonEmptyString::new("Standard Grey").unwrap(),
            profile_version: Version::new(1, 0, 0),
        },
        layer_height_mm: PositiveFiniteMm::new(0.05).unwrap(),
        normal_exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
        bottom_exposure_sec: PositiveFiniteSec::new(30.0).unwrap(),
        bottom_layer_count: 6,
        lift_distance_mm: PositiveFiniteMm::new(5.0).unwrap(),
        lift_speed_mm_min: PositiveFiniteMmPerMin::new(120.0).unwrap(),
        retract_speed_mm_min: PositiveFiniteMmPerMin::new(150.0).unwrap(),
        scale_compensation_pct: ScaleCompensation {
            x: ScaleCompensationPercent::new(0.5).unwrap(),
            y: ScaleCompensationPercent::new(0.5).unwrap(),
            z: ScaleCompensationPercent::new(0.0).unwrap(),
        },
    }
}

#[test]
fn constructed_value_with_empty_diagnostics() {
    let config = ResolvedPrintConfig {
        printer: sample_printer_profile(),
        material_resolved: sample_material_resolved(),
        overrides_applied: Vec::new(),
        warnings: Vec::new(),
    };
    assert!(config.overrides_applied.is_empty());
    assert!(config.warnings.is_empty());
    assert_eq!(config.material_resolved.bottom_layer_count, 6);
}

#[test]
fn constructed_value_with_populated_diagnostics() {
    let config = ResolvedPrintConfig {
        printer: sample_printer_profile(),
        material_resolved: sample_material_resolved(),
        overrides_applied: vec![OverrideRecord {
            field: ResolvedField::LayerHeightMm,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: 0.05,
        }],
        warnings: vec![ResolveWarning::DefaultUsed {
            field: ResolvedField::BottomLayerCount,
        }],
    };
    assert_eq!(config.overrides_applied.len(), 1);
    assert_eq!(config.warnings.len(), 1);
}

#[test]
fn equality_is_structural() {
    let a = ResolvedPrintConfig {
        printer: sample_printer_profile(),
        material_resolved: sample_material_resolved(),
        overrides_applied: Vec::new(),
        warnings: Vec::new(),
    };
    let b = ResolvedPrintConfig {
        printer: sample_printer_profile(),
        material_resolved: sample_material_resolved(),
        overrides_applied: Vec::new(),
        warnings: Vec::new(),
    };
    assert_eq!(a, b);

    let mut c = ResolvedPrintConfig {
        printer: sample_printer_profile(),
        material_resolved: sample_material_resolved(),
        overrides_applied: Vec::new(),
        warnings: Vec::new(),
    };
    c.warnings.push(ResolveWarning::DefaultUsed {
        field: ResolvedField::BottomLayerCount,
    });
    assert_ne!(a, c);
}

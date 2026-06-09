/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::material_intent::MaterialIntent;
use sc_print_primitives::{PositiveFiniteMjPerCm2, PositiveFiniteMm};

const FULL_TOML: &str = r#"
target_energy_density_mj_per_cm2 = 15.5
target_layer_height_mm = 0.05
"#;

#[test]
fn deserialise_parses_both_fields() {
    let parsed: MaterialIntent = toml_edit::de::from_str(FULL_TOML).unwrap();
    assert_eq!(
        parsed.target_energy_density_mj_per_cm2.map(|v| v.value()),
        Some(15.5)
    );
    assert_eq!(parsed.target_layer_height_mm.map(|v| v.value()), Some(0.05));
}

#[test]
fn deserialise_empty_section_leaves_both_fields_none() {
    let parsed: MaterialIntent = toml_edit::de::from_str("").unwrap();
    assert!(parsed.target_energy_density_mj_per_cm2.is_none());
    assert!(parsed.target_layer_height_mm.is_none());
}

#[test]
fn deserialise_with_only_energy_density_leaves_layer_height_none() {
    let toml = "target_energy_density_mj_per_cm2 = 18.0\n";
    let parsed: MaterialIntent = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(
        parsed.target_energy_density_mj_per_cm2.map(|v| v.value()),
        Some(18.0)
    );
    assert!(parsed.target_layer_height_mm.is_none());
}

#[test]
fn deserialise_with_only_layer_height_leaves_energy_density_none() {
    let toml = "target_layer_height_mm = 0.10\n";
    let parsed: MaterialIntent = toml_edit::de::from_str(toml).unwrap();
    assert!(parsed.target_energy_density_mj_per_cm2.is_none());
    assert_eq!(parsed.target_layer_height_mm.map(|v| v.value()), Some(0.10));
}

#[test]
fn deserialise_rejects_non_positive_energy_density() {
    let toml = "target_energy_density_mj_per_cm2 = -1.0\n";
    assert!(toml_edit::de::from_str::<MaterialIntent>(toml).is_err());
}

#[test]
fn deserialise_rejects_zero_layer_height() {
    let toml = "target_layer_height_mm = 0.0\n";
    assert!(toml_edit::de::from_str::<MaterialIntent>(toml).is_err());
}

#[test]
fn deserialise_rejects_non_finite_energy_density() {
    let toml = "target_energy_density_mj_per_cm2 = nan\n";
    assert!(toml_edit::de::from_str::<MaterialIntent>(toml).is_err());
}

#[test]
fn roundtrip_with_both_fields_preserves_value() {
    let original: MaterialIntent = toml_edit::de::from_str(FULL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialIntent = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn roundtrip_empty_section_preserves_all_none() {
    let original: MaterialIntent = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialIntent = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
    assert!(roundtripped.target_energy_density_mj_per_cm2.is_none());
    assert!(roundtripped.target_layer_height_mm.is_none());
}

#[test]
fn serialise_omits_each_unset_field() {
    let parsed: MaterialIntent = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    assert!(
        !serialised.contains("target_energy_density_mj_per_cm2"),
        "should omit target_energy_density_mj_per_cm2: {serialised}"
    );
    assert!(
        !serialised.contains("target_layer_height_mm"),
        "should omit target_layer_height_mm: {serialised}"
    );
}

#[test]
fn programmatic_construction_round_trips() {
    let intent = MaterialIntent {
        target_energy_density_mj_per_cm2: Some(PositiveFiniteMjPerCm2::new(18.0).unwrap()),
        target_layer_height_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
    };
    let serialised = toml_edit::ser::to_string(&intent).unwrap();
    let roundtripped: MaterialIntent = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(intent, roundtripped);
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::material_physical::MaterialPhysical;
use sc_print_primitives::DensityGPerCm3;

/// Siraya Tech's published pair for Blu.
const FULL_TOML: &str = r#"
liquid_density_g_per_cm3 = 1.1
cured_density_g_per_cm3 = 1.15
"#;

#[test]
fn deserialise_parses_both_densities() {
    let parsed: MaterialPhysical = toml_edit::de::from_str(FULL_TOML).unwrap();
    assert_eq!(
        parsed.liquid_density_g_per_cm3.map(|v| v.value()),
        Some(1.1)
    );
    assert_eq!(
        parsed.cured_density_g_per_cm3.map(|v| v.value()),
        Some(1.15)
    );
}

#[test]
fn deserialise_empty_section_leaves_both_densities_none() {
    let parsed: MaterialPhysical = toml_edit::de::from_str("").unwrap();
    assert!(parsed.liquid_density_g_per_cm3.is_none());
    assert!(parsed.cured_density_g_per_cm3.is_none());
}

#[test]
fn a_vendor_publishing_only_one_density_leaves_the_other_none() {
    let toml = "liquid_density_g_per_cm3 = 1.13\n";
    let parsed: MaterialPhysical = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(
        parsed.liquid_density_g_per_cm3.map(|v| v.value()),
        Some(1.13)
    );
    assert!(parsed.cured_density_g_per_cm3.is_none());
}

#[test]
fn the_default_section_states_nothing() {
    assert_eq!(
        MaterialPhysical::default(),
        MaterialPhysical {
            liquid_density_g_per_cm3: None,
            cured_density_g_per_cm3: None,
        }
    );
}

#[test]
fn deserialise_rejects_a_zero_density() {
    let toml = "cured_density_g_per_cm3 = 0.0\n";
    assert!(toml_edit::de::from_str::<MaterialPhysical>(toml).is_err());
}

#[test]
fn deserialise_rejects_a_negative_density() {
    let toml = "liquid_density_g_per_cm3 = -1.1\n";
    assert!(toml_edit::de::from_str::<MaterialPhysical>(toml).is_err());
}

#[test]
fn serialise_omits_the_densities_nobody_stated() {
    let rendered = toml_edit::ser::to_string(&MaterialPhysical::default()).unwrap();
    assert!(rendered.trim().is_empty(), "rendered: {rendered}");
}

#[test]
fn roundtrip_preserves_both_densities() {
    let original = MaterialPhysical {
        liquid_density_g_per_cm3: Some(DensityGPerCm3::new(1.1).unwrap()),
        cured_density_g_per_cm3: Some(DensityGPerCm3::new(1.195).unwrap()),
    };
    let rendered = toml_edit::ser::to_string(&original).unwrap();
    let parsed: MaterialPhysical = toml_edit::de::from_str(&rendered).unwrap();
    assert_eq!(parsed, original);
}

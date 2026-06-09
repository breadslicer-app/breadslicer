/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::material_profile::{MaterialProfile, MaterialProfileReservedSection};
use sc_print_primitives::PrinterId;

const MINIMAL_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
name = "Blu"
family = "standard"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.5

[defaults]
bottom_layer_count = 6
"#;

const WITH_PER_PRINTER_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
name = "Blu"
family = "standard"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.5

[defaults]
bottom_layer_count = 6

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
lift_distance_mm = 6.0

[per_printer."elegoo-saturn-4"]
normal_exposure_sec = 2.8
bottom_exposure_sec = 35.0
"#;

#[test]
fn deserialise_parses_minimal_profile_with_empty_per_printer() {
    let parsed: MaterialProfile = toml_edit::de::from_str(MINIMAL_TOML).unwrap();
    assert_eq!(parsed.identity.brand.as_str(), "Siraya Tech");
    assert_eq!(
        parsed
            .intent
            .target_energy_density_mj_per_cm2
            .map(|v| v.value()),
        Some(15.5)
    );
    assert_eq!(parsed.defaults.bottom_layer_count, Some(6));
    assert!(parsed.per_printer.is_empty());
}

#[test]
fn deserialise_parses_profile_with_per_printer_entries() {
    let parsed: MaterialProfile = toml_edit::de::from_str(WITH_PER_PRINTER_TOML).unwrap();
    assert_eq!(parsed.per_printer.len(), 2);
    let saturn_ultra = PrinterId::new("elegoo-saturn-4-ultra").unwrap();
    let entry = parsed.per_printer.get(&saturn_ultra).unwrap();
    assert_eq!(entry.normal_exposure_sec.map(|v| v.value()), Some(2.5));
    assert_eq!(entry.bottom_exposure_sec.map(|v| v.value()), Some(30.0));
    assert_eq!(entry.lift_distance_mm.map(|v| v.value()), Some(6.0));
    let saturn = PrinterId::new("elegoo-saturn-4").unwrap();
    let entry = parsed.per_printer.get(&saturn).unwrap();
    assert_eq!(entry.normal_exposure_sec.map(|v| v.value()), Some(2.8));
}

#[test]
fn deserialise_rejects_per_environment_section() {
    let toml = format!("{MINIMAL_TOML}\n[per_environment.cold]\nbias = 0.1\n");
    let err = toml_edit::de::from_str::<MaterialProfile>(&toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("per_environment"), "msg: {msg}");
    assert!(msg.contains("v0"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_per_session_state_section() {
    let toml = format!("{MINIMAL_TOML}\n[per_session_state.fresh_resin]\nbias = 0.1\n");
    let err = toml_edit::de::from_str::<MaterialProfile>(&toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("per_session_state"), "msg: {msg}");
    assert!(msg.contains("v0"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_per_printer_constraints_as_nested_section() {
    let toml = format!(
        "{MINIMAL_TOML}\n[per_printer.\"elegoo-saturn-4-ultra\"]\nnormal_exposure_sec = 2.5\n\n[per_printer.\"elegoo-saturn-4-ultra\".constraints]\nz_step_mm = 0.01\n",
    );
    let err = toml_edit::de::from_str::<MaterialProfile>(&toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("constraints"), "msg: {msg}");
    assert!(msg.contains("elegoo-saturn-4-ultra"), "msg: {msg}");
    assert!(msg.contains("v0"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_per_printer_constraints_as_inline_table() {
    let toml = format!(
        "{MINIMAL_TOML}\n[per_printer.\"elegoo-saturn-4-ultra\"]\nnormal_exposure_sec = 2.5\nconstraints = {{ z_step_mm = 0.01 }}\n",
    );
    let err = toml_edit::de::from_str::<MaterialProfile>(&toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("constraints"), "msg: {msg}");
    assert!(msg.contains("elegoo-saturn-4-ultra"), "msg: {msg}");
}

#[test]
fn per_printer_constraints_error_variant_carries_printer_id() {
    let toml = format!(
        "{MINIMAL_TOML}\n[per_printer.\"elegoo-saturn-4-ultra\"]\nnormal_exposure_sec = 2.5\n\n[per_printer.\"elegoo-saturn-4-ultra\".constraints]\nz_step_mm = 0.01\n",
    );
    let err = toml_edit::de::from_str::<MaterialProfile>(&toml);
    assert!(err.is_err());
}

#[test]
fn deserialise_rejects_invalid_per_printer_id() {
    let toml = format!("{MINIMAL_TOML}\n[per_printer.\"Bad-Caps\"]\nnormal_exposure_sec = 2.5\n",);
    assert!(toml_edit::de::from_str::<MaterialProfile>(&toml).is_err());
}

#[test]
fn roundtrip_with_per_printer_entries_preserves_value() {
    let original: MaterialProfile = toml_edit::de::from_str(WITH_PER_PRINTER_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialProfile = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn roundtrip_minimal_preserves_value() {
    let original: MaterialProfile = toml_edit::de::from_str(MINIMAL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialProfile = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn serialise_does_not_emit_reserved_sections() {
    let parsed: MaterialProfile = toml_edit::de::from_str(WITH_PER_PRINTER_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    assert!(
        !serialised.contains("per_environment"),
        "should not emit per_environment: {serialised}"
    );
    assert!(
        !serialised.contains("per_session_state"),
        "should not emit per_session_state: {serialised}"
    );
    assert!(
        !serialised.contains("constraints"),
        "should not emit per-printer constraints: {serialised}"
    );
}

#[test]
fn reserved_section_display_messages_are_descriptive() {
    let env = MaterialProfileReservedSection::PerEnvironment;
    assert!(format!("{env}").contains("per_environment"));

    let session = MaterialProfileReservedSection::PerSessionState;
    assert!(format!("{session}").contains("per_session_state"));

    let pp = MaterialProfileReservedSection::PerPrinterConstraints {
        printer_id: PrinterId::new("elegoo-saturn-4-ultra").unwrap(),
    };
    let msg = format!("{pp}");
    assert!(msg.contains("elegoo-saturn-4-ultra"), "msg: {msg}");
    assert!(msg.contains("constraints"), "msg: {msg}");
}

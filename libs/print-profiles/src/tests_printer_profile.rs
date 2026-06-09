/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::printer_profile::PrinterProfile;

const VALID_TOML: &str = r#"
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

#[test]
fn deserialise_parses_all_section_blocks() {
    let parsed: PrinterProfile = toml_edit::de::from_str(VALID_TOML).unwrap();
    assert_eq!(parsed.identity.manufacturer.as_str(), "ELEGOO");
    assert_eq!(parsed.hardware.build_volume().x_mm(), 153.36);
    assert!(parsed.capabilities.two_stage_lift);
    assert_eq!(parsed.constraints.z_step_mm.map(|v| v.value()), Some(0.01));
}

#[test]
fn deserialise_rejects_reserved_instance_section() {
    let toml = format!("{VALID_TOML}\n[instance]\nserial_number = \"ABC-123\"\n");
    let err = toml_edit::de::from_str::<PrinterProfile>(&toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("instance"), "msg: {msg}");
    assert!(msg.contains("v0"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_empty_reserved_instance_section() {
    let toml = format!("{VALID_TOML}\n[instance]\n");
    let err = toml_edit::de::from_str::<PrinterProfile>(&toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("instance"), "msg: {msg}");
}

#[test]
fn roundtrip_preserves_value() {
    let original: PrinterProfile = toml_edit::de::from_str(VALID_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterProfile = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn serialise_does_not_emit_instance_section() {
    let parsed: PrinterProfile = toml_edit::de::from_str(VALID_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    assert!(
        !serialised.contains("[instance]"),
        "should not emit [instance]: {serialised}"
    );
}

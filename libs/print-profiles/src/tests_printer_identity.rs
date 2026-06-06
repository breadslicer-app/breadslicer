/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::printer_identity::PrinterIdentity;
use sc_print_primitives::{NonEmptyString, PrinterId};
use semver::Version;

const VALID_TOML: &str = r#"
id = "elegoo-mars-3-pro"
manufacturer = "Elegoo"
family = "Mars 3"
name = "Mars 3 Pro"
profile_version = "1.0.0"
image_asset_path = "assets/elegoo-mars-3-pro.png"
"#;

const VALID_TOML_NO_IMAGE: &str = r#"
id = "elegoo-mars-3-pro"
manufacturer = "Elegoo"
family = "Mars 3"
name = "Mars 3 Pro"
profile_version = "1.0.0"
"#;

#[test]
fn deserialise_parses_all_fields() {
    let parsed: PrinterIdentity = toml_edit::de::from_str(VALID_TOML).unwrap();
    assert_eq!(parsed.id, PrinterId::new("elegoo-mars-3-pro").unwrap());
    assert_eq!(parsed.manufacturer, NonEmptyString::new("Elegoo").unwrap());
    assert_eq!(parsed.family, NonEmptyString::new("Mars 3").unwrap());
    assert_eq!(parsed.name, NonEmptyString::new("Mars 3 Pro").unwrap());
    assert_eq!(parsed.profile_version, Version::new(1, 0, 0));
    assert_eq!(
        parsed.image_asset_path,
        Some("assets/elegoo-mars-3-pro.png".to_owned())
    );
}

#[test]
fn deserialise_image_asset_path_defaults_to_none_when_missing() {
    let parsed: PrinterIdentity = toml_edit::de::from_str(VALID_TOML_NO_IMAGE).unwrap();
    assert_eq!(parsed.image_asset_path, None);
}

#[test]
fn deserialise_rejects_invalid_id_slug() {
    let toml = r#"
        id = "Elegoo-Mars-3"
        manufacturer = "Elegoo"
        family = "Mars 3"
        name = "Mars 3 Pro"
        profile_version = "1.0.0"
    "#;
    let err = toml_edit::de::from_str::<PrinterIdentity>(toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("invalid"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_empty_manufacturer() {
    let toml = r#"
        id = "elegoo-mars-3-pro"
        manufacturer = ""
        family = "Mars 3"
        name = "Mars 3 Pro"
        profile_version = "1.0.0"
    "#;
    let err = toml_edit::de::from_str::<PrinterIdentity>(toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("empty"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_empty_family() {
    let toml = r#"
        id = "elegoo-mars-3-pro"
        manufacturer = "Elegoo"
        family = ""
        name = "Mars 3 Pro"
        profile_version = "1.0.0"
    "#;
    let err = toml_edit::de::from_str::<PrinterIdentity>(toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("empty"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_two_component_version() {
    let toml = r#"
        id = "elegoo-mars-3-pro"
        manufacturer = "Elegoo"
        family = "Mars 3"
        name = "Mars 3 Pro"
        profile_version = "1.0"
    "#;
    assert!(toml_edit::de::from_str::<PrinterIdentity>(toml).is_err());
}

#[test]
fn deserialise_rejects_garbage_version() {
    let toml = r#"
        id = "elegoo-mars-3-pro"
        manufacturer = "Elegoo"
        family = "Mars 3"
        name = "Mars 3 Pro"
        profile_version = "not-a-version"
    "#;
    assert!(toml_edit::de::from_str::<PrinterIdentity>(toml).is_err());
}

#[test]
fn deserialise_rejects_missing_required_field() {
    let toml = r#"
        id = "elegoo-mars-3-pro"
        manufacturer = "Elegoo"
        family = "Mars 3"
        profile_version = "1.0.0"
    "#;
    assert!(toml_edit::de::from_str::<PrinterIdentity>(toml).is_err());
}

#[test]
fn roundtrip_with_image_asset_path_preserves_value() {
    let original: PrinterIdentity = toml_edit::de::from_str(VALID_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterIdentity = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn roundtrip_without_image_asset_path_preserves_value() {
    let original: PrinterIdentity = toml_edit::de::from_str(VALID_TOML_NO_IMAGE).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterIdentity = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
    assert_eq!(roundtripped.image_asset_path, None);
}

#[test]
fn serialise_omits_image_asset_path_when_none() {
    let parsed: PrinterIdentity = toml_edit::de::from_str(VALID_TOML_NO_IMAGE).unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    assert!(
        !serialised.contains("image_asset_path"),
        "serialised should omit image_asset_path: {serialised}"
    );
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::material_identity::MaterialIdentity;
use sc_print_primitives::{MaterialId, NonEmptyString};
use semver::Version;

const VALID_TOML: &str = r#"
id = "elegoo-resin-grey"
brand = "Elegoo"
family = "Standard Resin"
name = "Standard Grey"
profile_version = "1.0.0"
"#;

#[test]
fn deserialise_parses_all_fields() {
    let parsed: MaterialIdentity = toml_edit::de::from_str(VALID_TOML).unwrap();
    assert_eq!(parsed.id, MaterialId::new("elegoo-resin-grey").unwrap());
    assert_eq!(parsed.brand, NonEmptyString::new("Elegoo").unwrap());
    assert_eq!(
        parsed.family,
        NonEmptyString::new("Standard Resin").unwrap()
    );
    assert_eq!(parsed.name, NonEmptyString::new("Standard Grey").unwrap());
    assert_eq!(parsed.profile_version, Version::new(1, 0, 0));
}

#[test]
fn deserialise_accepts_open_family_string_with_spaces_and_hyphens() {
    let toml = r#"
        id = "exotic-resin-1"
        brand = "Maker Co"
        family = "Photo-curable Acrylic Blend"
        name = "Photoblend Grey"
        profile_version = "0.1.0"
    "#;
    let parsed: MaterialIdentity = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.family.as_str(), "Photo-curable Acrylic Blend");
}

#[test]
fn deserialise_rejects_invalid_id_slug() {
    let toml = r#"
        id = "Elegoo_Resin"
        brand = "Elegoo"
        family = "Standard Resin"
        name = "Standard Grey"
        profile_version = "1.0.0"
    "#;
    let err = toml_edit::de::from_str::<MaterialIdentity>(toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("invalid"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_empty_brand() {
    let toml = r#"
        id = "elegoo-resin-grey"
        brand = ""
        family = "Standard Resin"
        name = "Standard Grey"
        profile_version = "1.0.0"
    "#;
    let err = toml_edit::de::from_str::<MaterialIdentity>(toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("empty"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_empty_family() {
    let toml = r#"
        id = "elegoo-resin-grey"
        brand = "Elegoo"
        family = ""
        name = "Standard Grey"
        profile_version = "1.0.0"
    "#;
    let err = toml_edit::de::from_str::<MaterialIdentity>(toml).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("empty"), "msg: {msg}");
}

#[test]
fn deserialise_rejects_garbage_version() {
    let toml = r#"
        id = "elegoo-resin-grey"
        brand = "Elegoo"
        family = "Standard Resin"
        name = "Standard Grey"
        profile_version = "not-a-version"
    "#;
    assert!(toml_edit::de::from_str::<MaterialIdentity>(toml).is_err());
}

#[test]
fn deserialise_rejects_missing_required_field() {
    let toml = r#"
        id = "elegoo-resin-grey"
        family = "Standard Resin"
        name = "Standard Grey"
        profile_version = "1.0.0"
    "#;
    assert!(toml_edit::de::from_str::<MaterialIdentity>(toml).is_err());
}

#[test]
fn roundtrip_serialise_then_deserialise_preserves_value() {
    let original: MaterialIdentity = toml_edit::de::from_str(VALID_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialIdentity = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

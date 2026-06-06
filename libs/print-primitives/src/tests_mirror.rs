/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::mirror::Mirror;

#[test]
fn mirror_default_has_both_axes_unflipped() {
    let m = Mirror::default();
    assert!(!m.x);
    assert!(!m.y);
}

#[test]
fn mirror_field_access_is_independent_per_axis() {
    let m = Mirror { x: true, y: false };
    assert!(m.x);
    assert!(!m.y);
}

#[cfg(feature = "serde")]
#[test]
fn mirror_deserialises_from_toml() {
    let toml = "x = true\ny = false\n";
    let m: Mirror = toml_edit::de::from_str(toml).unwrap();
    assert!(m.x);
    assert!(!m.y);
}

#[cfg(feature = "serde")]
#[test]
fn mirror_roundtrip_through_toml_preserves_value() {
    let original = Mirror { x: true, y: true };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Mirror = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

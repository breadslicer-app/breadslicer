/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use super::panel_rotation::PanelRotation;

#[test]
fn panel_rotation_defaults_to_no_turn() {
    assert_eq!(PanelRotation::default(), PanelRotation::None);
    assert!(PanelRotation::default().is_none());
}

#[test]
fn only_a_quarter_turn_swaps_the_axes() {
    assert!(!PanelRotation::None.swaps_axes());
    assert!(PanelRotation::Ccw90.swaps_axes());
}

#[test]
fn to_panel_orders_plate_pairs_as_width_and_height() {
    assert_eq!(PanelRotation::None.to_panel((2560, 1620)), (2560, 1620));
    assert_eq!(PanelRotation::Ccw90.to_panel((2560, 1620)), (1620, 2560));
}

#[cfg(feature = "serde")]
#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
struct Holder {
    panel_rotation: PanelRotation,
}

#[cfg(feature = "serde")]
#[test]
fn panel_rotation_reads_its_toml_names() {
    for (text, expected) in [
        ("panel_rotation = \"none\"\n", PanelRotation::None),
        ("panel_rotation = \"ccw90\"\n", PanelRotation::Ccw90),
    ] {
        let holder: Holder = toml_edit::de::from_str(text).unwrap();
        assert_eq!(holder.panel_rotation, expected);
    }
}

#[cfg(feature = "serde")]
#[test]
fn panel_rotation_refuses_an_unknown_name() {
    assert!(toml_edit::de::from_str::<Holder>("panel_rotation = \"cw90\"\n").is_err());
}

#[cfg(feature = "serde")]
#[test]
fn panel_rotation_roundtrips_through_toml() {
    let original = Holder {
        panel_rotation: PanelRotation::Ccw90,
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    assert_eq!(serialised, "panel_rotation = \"ccw90\"\n");
    assert_eq!(
        toml_edit::de::from_str::<Holder>(&serialised).unwrap(),
        original
    );
}

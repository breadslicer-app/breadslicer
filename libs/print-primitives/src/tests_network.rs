/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::network::NetworkProtocol;

#[test]
fn network_protocol_canonical_roundtrips_for_known_variants() {
    let cases = [
        ("elegoo_centauri", NetworkProtocol::ElegooCentauri),
        ("anycubic_homie", NetworkProtocol::AnycubicHomie),
        ("moonraker", NetworkProtocol::Moonraker),
    ];
    for (s, expected) in cases {
        let parsed = NetworkProtocol::from_canonical(s);
        assert_eq!(parsed, expected);
        assert_eq!(parsed.as_canonical(), s);
    }
}

#[test]
fn network_protocol_from_canonical_falls_through_to_unknown() {
    let parsed = NetworkProtocol::from_canonical("future_protocol");
    assert_eq!(
        parsed,
        NetworkProtocol::Unknown("future_protocol".to_owned())
    );
}

#[test]
fn network_protocol_unknown_preserves_inner_string_in_as_canonical() {
    let p = NetworkProtocol::Unknown("future_protocol".to_owned());
    assert_eq!(p.as_canonical(), "future_protocol");
}

#[cfg(feature = "serde")]
#[test]
fn network_protocol_deserialises_canonical_string_to_known_variant() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: NetworkProtocol,
    }
    let toml = "value = \"elegoo_centauri\"\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.value, NetworkProtocol::ElegooCentauri);
}

#[cfg(feature = "serde")]
#[test]
fn network_protocol_deserialises_unknown_string_via_escape_hatch() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: NetworkProtocol,
    }
    let toml = "value = \"future_protocol\"\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(
        parsed.value,
        NetworkProtocol::Unknown("future_protocol".to_owned())
    );
}

#[cfg(feature = "serde")]
#[test]
fn network_protocol_serialises_known_variant_as_canonical_string() {
    #[derive(serde::Serialize)]
    struct Wrap {
        value: NetworkProtocol,
    }
    let toml = toml_edit::ser::to_string(&Wrap {
        value: NetworkProtocol::Moonraker,
    })
    .unwrap();
    assert!(toml.contains("moonraker"), "toml: {toml}");
}

#[cfg(feature = "serde")]
#[test]
fn network_protocol_roundtrip_through_toml_preserves_known_variant() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: NetworkProtocol,
    }
    let original = Wrap {
        value: NetworkProtocol::AnycubicHomie,
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[cfg(feature = "serde")]
#[test]
fn network_protocol_roundtrip_through_toml_preserves_unknown_variant() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: NetworkProtocol,
    }
    let original = Wrap {
        value: NetworkProtocol::Unknown("future_protocol".to_owned()),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

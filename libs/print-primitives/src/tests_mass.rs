/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::mass::{DensityGPerCm3, DensityOutOfRange, MassG, MassOutOfRange};
use super::volume::VolumeMm3;

fn density(value: f64) -> DensityGPerCm3 {
    DensityGPerCm3::new(value).unwrap()
}

fn volume(value: f64) -> VolumeMm3 {
    VolumeMm3::new(value).unwrap()
}

#[test]
fn mass_g_new_accepts_zero() {
    assert_eq!(MassG::new(0.0).unwrap().value(), 0.0);
}

#[test]
fn mass_g_new_accepts_positive_finite_value() {
    assert_eq!(MassG::new(12.871).unwrap().value(), 12.871);
}

#[test]
fn mass_g_new_rejects_negative() {
    let err = MassG::new(-1.0).unwrap_err();
    assert_eq!(err, MassOutOfRange { value: -1.0 });
}

#[test]
fn mass_g_new_rejects_nan() {
    assert!(MassG::new(f64::NAN).is_err());
}

#[test]
fn mass_g_new_rejects_positive_infinity() {
    assert!(MassG::new(f64::INFINITY).is_err());
}

#[test]
fn mass_g_zero_matches_the_constructed_zero() {
    assert_eq!(MassG::ZERO, MassG::new(0.0).unwrap());
}

#[test]
fn mass_out_of_range_display_mentions_unit_and_value() {
    let msg = MassOutOfRange { value: -1.25 }.to_string();
    assert!(msg.contains("-1.25"), "msg: {msg}");
    assert!(msg.contains(" g"), "msg: {msg}");
    assert!(msg.contains("mass"), "msg: {msg}");
}

#[test]
fn mass_g_try_from_f64_round_trips_to_f64() {
    let m: MassG = 12.5.try_into().unwrap();
    let back: f64 = m.into();
    assert_eq!(back, 12.5);
}

#[test]
fn mass_g_display_formats_inner_value() {
    let msg = MassG::new(15.5).unwrap().to_string();
    assert!(msg.contains("15.5"), "msg: {msg}");
}

#[test]
fn density_new_accepts_positive_finite_value() {
    assert_eq!(density(1.1).value(), 1.1);
}

#[test]
fn density_new_rejects_zero() {
    let err = DensityGPerCm3::new(0.0).unwrap_err();
    assert_eq!(err, DensityOutOfRange { value: 0.0 });
}

#[test]
fn density_new_rejects_negative() {
    assert!(DensityGPerCm3::new(-1.1).is_err());
}

#[test]
fn density_new_rejects_nan() {
    assert!(DensityGPerCm3::new(f64::NAN).is_err());
}

#[test]
fn density_new_rejects_positive_infinity() {
    assert!(DensityGPerCm3::new(f64::INFINITY).is_err());
}

#[test]
fn density_out_of_range_display_mentions_unit_and_value() {
    let msg = DensityOutOfRange { value: 0.0 }.to_string();
    assert!(msg.contains('0'), "msg: {msg}");
    assert!(msg.contains("g/cm^3"), "msg: {msg}");
    assert!(msg.contains("density"), "msg: {msg}");
}

#[test]
fn density_try_from_f64_round_trips_to_f64() {
    let d: DensityGPerCm3 = 1.1.try_into().unwrap();
    let back: f64 = d.into();
    assert_eq!(back, 1.1);
}

#[test]
fn density_display_formats_inner_value() {
    let msg = density(1.1).to_string();
    assert!(msg.contains("1.1"), "msg: {msg}");
}

#[test]
fn a_cubic_centimetre_weighs_its_density() {
    // 1000 mm3 is 1 cm3, so the mass in grams is the density.
    let mass = density(1.1).mass_of(volume(1_000.0)).unwrap();
    assert_eq!(mass.value(), 1.1);
}

#[test]
fn the_reference_sample_weighs_what_its_own_file_states() {
    // The vendor's GOO file states 12.871 g over 11701.23 mm3.
    let mass = density(1.1).mass_of(volume(11_701.23)).unwrap();
    assert!((mass.value() - 12.871).abs() < 5e-4, "mass: {mass}");
}

#[test]
fn an_empty_plate_weighs_nothing_at_any_density() {
    assert_eq!(density(1.1).mass_of(VolumeMm3::ZERO).unwrap(), MassG::ZERO);
}

#[test]
fn a_volume_that_overflows_the_product_is_rejected() {
    let vast = volume(f64::MAX);
    assert!(density(1e6).mass_of(vast).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn density_deserialises_from_toml() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: DensityGPerCm3,
    }
    let parsed: Wrap = toml_edit::de::from_str("value = 1.1\n").unwrap();
    assert_eq!(parsed.value.value(), 1.1);
}

#[cfg(feature = "serde")]
#[test]
fn density_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: DensityGPerCm3,
    }
    assert!(toml_edit::de::from_str::<Wrap>("value = 0.0\n").is_err());
}

#[cfg(feature = "serde")]
#[test]
fn density_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: DensityGPerCm3,
    }
    let original = Wrap {
        value: density(1.1),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[cfg(feature = "serde")]
#[test]
fn mass_g_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: MassG,
    }
    let original = Wrap {
        value: MassG::new(12.871).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

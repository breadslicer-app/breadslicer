/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::material_per_printer::MaterialPerPrinter;
use super::second_travel_stage::SecondTravelStage;
use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};

const FULL_TOML: &str = r#"
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 6
wait_before_cure_sec = 0.5
wait_after_cure_sec = 1.0
wait_after_lift_sec = 0.0
bottom_wait_before_cure_sec = 3.0
bottom_wait_after_cure_sec = 4.0
bottom_wait_after_lift_sec = 5.0
lift_distance_mm = 6.0
lift_speed_mm_min = 60.0
retract_speed_mm_min = 150.0
bottom_lift_distance_mm = 8.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 90.0
light_pwm = 80.0
bottom_light_pwm = 100.0

[second_travel]
lift_distance_mm = 3.0
lift_speed_mm_min = 230.0
retract_speed_mm_min = 230.0

[bottom_second_travel]
lift_distance_mm = 4.0
lift_speed_mm_min = 65.0
retract_speed_mm_min = 180.0
"#;

#[test]
fn deserialise_parses_all_fields() {
    let parsed: MaterialPerPrinter = toml_edit::de::from_str(FULL_TOML).unwrap();
    assert_eq!(parsed.normal_exposure_sec.map(|v| v.value()), Some(2.5));
    assert_eq!(parsed.bottom_exposure_sec.map(|v| v.value()), Some(30.0));
    assert_eq!(parsed.bottom_layer_count, Some(6));
    assert_eq!(parsed.wait_before_cure_sec.map(|v| v.value()), Some(0.5));
    assert_eq!(parsed.wait_after_cure_sec.map(|v| v.value()), Some(1.0));
    assert_eq!(parsed.wait_after_lift_sec.map(|v| v.value()), Some(0.0));
    assert_eq!(
        parsed.bottom_wait_before_cure_sec.map(|v| v.value()),
        Some(3.0)
    );
    assert_eq!(
        parsed.bottom_wait_after_cure_sec.map(|v| v.value()),
        Some(4.0)
    );
    assert_eq!(
        parsed.bottom_wait_after_lift_sec.map(|v| v.value()),
        Some(5.0)
    );
    assert_eq!(parsed.lift_distance_mm.map(|v| v.value()), Some(6.0));
    assert_eq!(parsed.lift_speed_mm_min.map(|v| v.value()), Some(60.0));
    assert_eq!(parsed.retract_speed_mm_min.map(|v| v.value()), Some(150.0));
    assert_eq!(parsed.bottom_lift_distance_mm.map(|v| v.value()), Some(8.0));
    assert_eq!(
        parsed.bottom_lift_speed_mm_min.map(|v| v.value()),
        Some(40.0)
    );
    assert_eq!(
        parsed.bottom_retract_speed_mm_min.map(|v| v.value()),
        Some(90.0)
    );
    assert_eq!(parsed.light_pwm.map(|v| v.percent()), Some(80.0));
    assert_eq!(parsed.bottom_light_pwm.map(|v| v.percent()), Some(100.0));
    let second = parsed.second_travel.unwrap();
    assert_eq!(second.lift_distance_mm.value(), 3.0);
    assert_eq!(second.lift_speed_mm_min.value(), 230.0);
    assert_eq!(second.retract_speed_mm_min.value(), 230.0);
    let bottom_second = parsed.bottom_second_travel.unwrap();
    assert_eq!(bottom_second.lift_distance_mm.value(), 4.0);
    assert_eq!(bottom_second.lift_speed_mm_min.value(), 65.0);
    assert_eq!(bottom_second.retract_speed_mm_min.value(), 180.0);
}

#[test]
fn deserialise_empty_section_leaves_all_fields_none() {
    let parsed: MaterialPerPrinter = toml_edit::de::from_str("").unwrap();
    assert!(parsed.normal_exposure_sec.is_none());
    assert!(parsed.bottom_exposure_sec.is_none());
    assert!(parsed.bottom_layer_count.is_none());
    assert!(parsed.wait_before_cure_sec.is_none());
    assert!(parsed.wait_after_cure_sec.is_none());
    assert!(parsed.wait_after_lift_sec.is_none());
    assert!(parsed.bottom_wait_before_cure_sec.is_none());
    assert!(parsed.bottom_wait_after_cure_sec.is_none());
    assert!(parsed.bottom_wait_after_lift_sec.is_none());
    assert!(parsed.lift_distance_mm.is_none());
    assert!(parsed.lift_speed_mm_min.is_none());
    assert!(parsed.retract_speed_mm_min.is_none());
    assert!(parsed.bottom_lift_distance_mm.is_none());
    assert!(parsed.bottom_lift_speed_mm_min.is_none());
    assert!(parsed.bottom_retract_speed_mm_min.is_none());
    assert!(parsed.light_pwm.is_none());
    assert!(parsed.bottom_light_pwm.is_none());
    assert!(parsed.second_travel.is_none());
    assert!(parsed.bottom_second_travel.is_none());
}

#[test]
fn deserialise_rejects_a_negative_wait() {
    let toml = "wait_before_cure_sec = -1.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_parses_the_bottom_waits_apart_from_the_normal_ones() {
    let toml = r#"
wait_before_cure_sec = 0.5
bottom_wait_before_cure_sec = 3.0
bottom_wait_after_cure_sec = 4.0
bottom_wait_after_lift_sec = 5.0
"#;
    let parsed: MaterialPerPrinter = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.wait_before_cure_sec.map(|v| v.value()), Some(0.5));
    assert_eq!(
        parsed.bottom_wait_before_cure_sec.map(|v| v.value()),
        Some(3.0)
    );
    assert_eq!(
        parsed.bottom_wait_after_cure_sec.map(|v| v.value()),
        Some(4.0)
    );
    assert_eq!(
        parsed.bottom_wait_after_lift_sec.map(|v| v.value()),
        Some(5.0)
    );
}

#[test]
fn deserialise_rejects_a_negative_bottom_wait() {
    let toml = "bottom_wait_after_lift_sec = -1.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_parses_the_bottom_motion_apart_from_the_normal_one() {
    let toml = r#"
lift_distance_mm = 6.0
bottom_lift_distance_mm = 8.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 90.0
"#;
    let parsed: MaterialPerPrinter = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.lift_distance_mm.map(|v| v.value()), Some(6.0));
    assert_eq!(parsed.bottom_lift_distance_mm.map(|v| v.value()), Some(8.0));
    assert_eq!(
        parsed.bottom_lift_speed_mm_min.map(|v| v.value()),
        Some(40.0)
    );
    assert_eq!(
        parsed.bottom_retract_speed_mm_min.map(|v| v.value()),
        Some(90.0)
    );
}

#[test]
fn deserialise_rejects_a_non_positive_bottom_lift_speed() {
    let toml = "bottom_lift_speed_mm_min = 0.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_reads_the_two_light_pwm_values_as_percentages() {
    let toml = "light_pwm = 80.0\nbottom_light_pwm = 100.0\n";
    let parsed: MaterialPerPrinter = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.light_pwm.map(|v| v.percent()), Some(80.0));
    assert_eq!(parsed.bottom_light_pwm.map(|v| v.percent()), Some(100.0));
}

#[test]
fn deserialise_rejects_a_light_pwm_beyond_full_power() {
    let toml = "light_pwm = 150.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_rejects_a_negative_light_pwm() {
    let toml = "bottom_light_pwm = -1.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_partial_section_keeps_unset_fields_none() {
    let toml = r#"
normal_exposure_sec = 3.0
lift_distance_mm = 5.0
"#;
    let parsed: MaterialPerPrinter = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.normal_exposure_sec.map(|v| v.value()), Some(3.0));
    assert_eq!(parsed.lift_distance_mm.map(|v| v.value()), Some(5.0));
    assert!(parsed.bottom_exposure_sec.is_none());
    assert!(parsed.bottom_layer_count.is_none());
    assert!(parsed.lift_speed_mm_min.is_none());
    assert!(parsed.retract_speed_mm_min.is_none());
}

#[test]
fn deserialise_accepts_zero_bottom_layer_count() {
    let toml = "bottom_layer_count = 0\n";
    let parsed: MaterialPerPrinter = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.bottom_layer_count, Some(0));
}

#[test]
fn deserialise_rejects_non_positive_normal_exposure() {
    let toml = "normal_exposure_sec = 0.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_rejects_negative_bottom_exposure() {
    let toml = "bottom_exposure_sec = -5.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_rejects_non_finite_lift_distance() {
    let toml = "lift_distance_mm = nan\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_rejects_non_positive_lift_speed() {
    let toml = "lift_speed_mm_min = 0.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_rejects_negative_retract_speed() {
    let toml = "retract_speed_mm_min = -10.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_rejects_negative_bottom_layer_count() {
    let toml = "bottom_layer_count = -1\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn roundtrip_with_all_fields_preserves_value() {
    let original: MaterialPerPrinter = toml_edit::de::from_str(FULL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialPerPrinter = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn roundtrip_empty_section_preserves_all_none() {
    let original: MaterialPerPrinter = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialPerPrinter = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
    assert!(roundtripped.normal_exposure_sec.is_none());
    assert!(roundtripped.lift_speed_mm_min.is_none());
}

#[test]
fn serialise_omits_every_unset_field() {
    let parsed: MaterialPerPrinter = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    for field in [
        "normal_exposure_sec",
        "bottom_exposure_sec",
        "bottom_layer_count",
        "wait_before_cure_sec",
        "wait_after_cure_sec",
        "wait_after_lift_sec",
        "bottom_wait_before_cure_sec",
        "bottom_wait_after_cure_sec",
        "bottom_wait_after_lift_sec",
        "lift_distance_mm",
        "lift_speed_mm_min",
        "retract_speed_mm_min",
        "bottom_lift_distance_mm",
        "bottom_lift_speed_mm_min",
        "bottom_retract_speed_mm_min",
        "light_pwm",
        "bottom_light_pwm",
        "second_travel",
        "bottom_second_travel",
    ] {
        assert!(
            !serialised.contains(field),
            "should omit {field}: {serialised}"
        );
    }
}

#[test]
fn programmatic_construction_round_trips() {
    let per_printer = MaterialPerPrinter {
        normal_exposure_sec: Some(PositiveFiniteSec::new(2.5).unwrap()),
        bottom_exposure_sec: Some(PositiveFiniteSec::new(30.0).unwrap()),
        bottom_layer_count: Some(6),
        wait_before_cure_sec: Some(NonNegativeFiniteSec::new(0.5).unwrap()),
        wait_after_cure_sec: None,
        wait_after_lift_sec: Some(NonNegativeFiniteSec::ZERO),
        bottom_wait_before_cure_sec: Some(NonNegativeFiniteSec::new(3.0).unwrap()),
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: Some(NonNegativeFiniteSec::ZERO),
        lift_distance_mm: Some(PositiveFiniteMm::new(6.0).unwrap()),
        lift_speed_mm_min: None,
        retract_speed_mm_min: Some(PositiveFiniteMmPerMin::new(150.0).unwrap()),
        bottom_lift_distance_mm: Some(PositiveFiniteMm::new(8.0).unwrap()),
        bottom_lift_speed_mm_min: None,
        bottom_retract_speed_mm_min: Some(PositiveFiniteMmPerMin::new(90.0).unwrap()),
        light_pwm: Some(LightPwm::new(80.0).unwrap()),
        bottom_light_pwm: None,
        second_travel: Some(SecondTravelStage {
            lift_distance_mm: PositiveFiniteMm::new(3.0).unwrap(),
            lift_speed_mm_min: PositiveFiniteMmPerMin::new(230.0).unwrap(),
            retract_speed_mm_min: PositiveFiniteMmPerMin::new(230.0).unwrap(),
        }),
        bottom_second_travel: None,
    };
    let serialised = toml_edit::ser::to_string(&per_printer).unwrap();
    let roundtripped: MaterialPerPrinter = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(per_printer, roundtripped);
}

#[test]
fn deserialise_reads_a_second_stage_for_the_bottom_layers_apart_from_the_normal_one() {
    let toml = r#"
lift_distance_mm = 2.0

[second_travel]
lift_distance_mm = 3.0
lift_speed_mm_min = 230.0
retract_speed_mm_min = 230.0

[bottom_second_travel]
lift_distance_mm = 4.0
lift_speed_mm_min = 65.0
retract_speed_mm_min = 180.0
"#;
    let parsed: MaterialPerPrinter = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(
        parsed.second_travel.map(|s| s.lift_distance_mm.value()),
        Some(3.0)
    );
    assert_eq!(
        parsed
            .bottom_second_travel
            .map(|s| s.lift_distance_mm.value()),
        Some(4.0)
    );
}

#[test]
fn deserialise_rejects_a_second_stage_that_states_a_distance_without_its_speeds() {
    let toml = "[second_travel]\nlift_distance_mm = 3.0\n";
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

#[test]
fn deserialise_rejects_a_non_positive_second_lift_distance() {
    let toml = r#"
[second_travel]
lift_distance_mm = 0.0
lift_speed_mm_min = 230.0
retract_speed_mm_min = 230.0
"#;
    assert!(toml_edit::de::from_str::<MaterialPerPrinter>(toml).is_err());
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fs;

use sc_print_profiles::{
    load_material_extension_from_library, load_material_profile_from_library,
    load_material_profile_from_path,
};

use super::*;

#[test]
fn empty_calibration_fields_parse_to_an_all_unset_block() {
    let block = parse_calibration_form(&CalibrationFormInput::default()).unwrap();
    assert_eq!(
        block,
        MaterialPerPrinter {
            normal_exposure_sec: None,
            bottom_exposure_sec: None,
            bottom_layer_count: None,
            lift_distance_mm: None,
            lift_speed_mm_min: None,
            retract_speed_mm_min: None,
        }
    );
}

#[test]
fn filled_calibration_fields_parse_to_typed_values() {
    let input = CalibrationFormInput {
        normal_exposure_sec: "2.5".to_owned(),
        bottom_exposure_sec: " 30 ".to_owned(),
        bottom_layer_count: "6".to_owned(),
        lift_distance_mm: "6.0".to_owned(),
        lift_speed_mm_min: "60".to_owned(),
        retract_speed_mm_min: "150".to_owned(),
    };
    let block = parse_calibration_form(&input).unwrap();
    assert_eq!(block.normal_exposure_sec.map(|v| v.value()), Some(2.5));
    assert_eq!(block.bottom_exposure_sec.map(|v| v.value()), Some(30.0));
    assert_eq!(block.bottom_layer_count, Some(6));
    assert_eq!(block.lift_distance_mm.map(|v| v.value()), Some(6.0));
    assert_eq!(block.lift_speed_mm_min.map(|v| v.value()), Some(60.0));
    assert_eq!(block.retract_speed_mm_min.map(|v| v.value()), Some(150.0));
}

#[test]
fn a_non_numeric_calibration_field_names_itself_in_the_error() {
    let input = CalibrationFormInput {
        normal_exposure_sec: "fast".to_owned(),
        ..CalibrationFormInput::default()
    };
    let err = parse_calibration_form(&input).unwrap_err();
    assert_eq!(
        err,
        InvalidFormValue::NotANumber {
            field: FormField::NormalExposure
        }
    );
    assert!(format!("{err}").contains("Normal exposure"));
    assert_eq!(err.user_message(), "Normal exposure must be a number");
}

#[test]
fn a_non_positive_calibration_field_is_out_of_range() {
    let input = CalibrationFormInput {
        lift_distance_mm: "0".to_owned(),
        ..CalibrationFormInput::default()
    };
    let err = parse_calibration_form(&input).unwrap_err();
    assert_eq!(
        err,
        InvalidFormValue::OutOfRange {
            field: FormField::LiftDistance
        }
    );
    assert_eq!(err.user_message(), "Lift distance is out of range");
}

#[test]
fn a_negative_layer_count_is_not_a_number() {
    let input = CalibrationFormInput {
        bottom_layer_count: "-3".to_owned(),
        ..CalibrationFormInput::default()
    };
    let err = parse_calibration_form(&input).unwrap_err();
    assert_eq!(
        err,
        InvalidFormValue::NotANumber {
            field: FormField::BottomLayerCount
        }
    );
}

#[test]
fn empty_defaults_fields_parse_to_all_unset() {
    let defaults = parse_defaults_form(&DefaultsFormInput::default()).unwrap();
    assert_eq!(defaults.scale_compensation_pct, None);
    assert_eq!(defaults.bottom_layer_count, None);
}

#[test]
fn all_three_scale_axes_parse_together() {
    let input = DefaultsFormInput {
        scale_x_pct: "0.1".to_owned(),
        scale_y_pct: "-0.2".to_owned(),
        scale_z_pct: "0".to_owned(),
        bottom_layer_count: "5".to_owned(),
    };
    let defaults = parse_defaults_form(&input).unwrap();
    let scale = defaults.scale_compensation_pct.unwrap();
    assert_eq!(scale.x.value(), 0.1);
    assert_eq!(scale.y.value(), -0.2);
    assert_eq!(scale.z.value(), 0.0);
    assert_eq!(defaults.bottom_layer_count, Some(5));
}

#[test]
fn a_partially_filled_scale_compensation_is_rejected() {
    let input = DefaultsFormInput {
        scale_x_pct: "0.1".to_owned(),
        ..DefaultsFormInput::default()
    };
    let err = parse_defaults_form(&input).unwrap_err();
    assert_eq!(err, InvalidFormValue::PartialScaleCompensation);
    assert_eq!(
        err.user_message(),
        "Fill all three scale compensation axes, or leave all three empty",
    );
}

#[test]
fn an_out_of_bound_scale_axis_is_rejected() {
    let input = DefaultsFormInput {
        scale_x_pct: "0.1".to_owned(),
        scale_y_pct: "0.1".to_owned(),
        scale_z_pct: "99".to_owned(),
        ..DefaultsFormInput::default()
    };
    assert_eq!(
        parse_defaults_form(&input).unwrap_err(),
        InvalidFormValue::OutOfRange {
            field: FormField::ScaleCompensationZ
        },
    );
}

const PROFILE_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"other\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]

[defaults]
bottom_layer_count = 5

[defaults.scale_compensation_pct]
x = 0.1
y = -0.2
z = 0.0

[per_printer.printer-a]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
lift_distance_mm = 6.0
";

fn parse_profile(toml: &str) -> MaterialProfile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.toml");
    fs::write(&path, toml).unwrap();
    load_material_profile_from_path(&path).unwrap()
}

#[test]
fn calibration_seeding_round_trips_through_the_parser() {
    let profile = parse_profile(PROFILE_TOML);
    let block = profile.per_printer.values().next().unwrap();
    let seeded = calibration_form_from_block(block);
    assert_eq!(seeded.normal_exposure_sec, "2.5");
    assert_eq!(seeded.bottom_exposure_sec, "30");
    assert_eq!(seeded.bottom_layer_count, "");
    let reparsed = parse_calibration_form(&seeded).unwrap();
    assert_eq!(&reparsed, block);
}

#[test]
fn defaults_seeding_round_trips_through_the_parser() {
    let profile = parse_profile(PROFILE_TOML);
    let seeded = defaults_form_from(&profile.defaults);
    assert_eq!(seeded.scale_x_pct, "0.1");
    assert_eq!(seeded.scale_y_pct, "-0.2");
    assert_eq!(seeded.scale_z_pct, "0");
    assert_eq!(seeded.bottom_layer_count, "5");
    let reparsed = parse_defaults_form(&seeded).unwrap();
    assert_eq!(reparsed, profile.defaults);
}

#[test]
fn unset_defaults_seed_to_empty_texts() {
    let seeded = defaults_form_from(&MaterialDefaults {
        scale_compensation_pct: None,
        bottom_layer_count: None,
    });
    assert_eq!(seeded, DefaultsFormInput::default());
}

#[test]
fn saving_a_full_profile_writes_the_materials_file() {
    let dir = tempfile::tempdir().unwrap();
    let profile = parse_profile(PROFILE_TOML);

    save_edited_profile(dir.path(), &profile, EditedProfileKind::Full).unwrap();

    let loaded = load_material_profile_from_library(dir.path(), &profile.identity.id).unwrap();
    assert_eq!(loaded, profile);
    assert!(
        !dir.path()
            .join("materials")
            .join("extensions")
            .join("brand-resin.toml")
            .exists()
    );
}

#[test]
fn saving_an_extension_writes_through_the_extension_writer() {
    let dir = tempfile::tempdir().unwrap();
    let mut profile = parse_profile(PROFILE_TOML);
    profile.extension = true;

    save_edited_profile(dir.path(), &profile, EditedProfileKind::Extension).unwrap();

    let loaded = load_material_extension_from_library(dir.path(), &profile.identity.id).unwrap();
    assert_eq!(loaded, profile);
    assert!(
        !dir.path()
            .join("materials")
            .join("brand-resin.toml")
            .exists()
    );
}

#[test]
fn saving_an_unflagged_profile_as_extension_reports_the_writer_error() {
    let dir = tempfile::tempdir().unwrap();
    let profile = parse_profile(PROFILE_TOML);
    let err = save_edited_profile(dir.path(), &profile, EditedProfileKind::Extension).unwrap_err();
    assert!(err.contains("extension flag"), "err: {err}");
}

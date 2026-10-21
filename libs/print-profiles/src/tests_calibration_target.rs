/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::PrinterId;

use super::calibration_target::CalibrationTarget;
use super::generic_machine::generic_printer_id;
use super::material_profile::MaterialProfile;

fn printer_id(raw: &str) -> PrinterId {
    raw.parse().unwrap()
}

#[test]
fn from_printer_maps_the_generic_id_to_generic() {
    assert_eq!(
        CalibrationTarget::from_printer(generic_printer_id()),
        CalibrationTarget::Generic,
    );
}

#[test]
fn from_printer_keeps_any_other_id_as_printer() {
    let target = CalibrationTarget::from_printer(&printer_id("printer-a"));

    match &target {
        CalibrationTarget::Printer(id) => {
            assert_eq!(id.as_printer_id(), &printer_id("printer-a"));
            assert_eq!(id.as_str(), "printer-a");
        }
        CalibrationTarget::Generic => panic!("expected a printer target"),
    }
    assert_eq!(target.printer_id(), &printer_id("printer-a"));
}

#[test]
fn printer_payload_is_never_the_generic_id() {
    // Both public constructors normalise the generic id, so a
    // `Printer` carrying it cannot be built.
    assert_eq!(
        CalibrationTarget::try_from(String::from("generic-machine")).unwrap(),
        CalibrationTarget::Generic,
    );
    assert_eq!(
        CalibrationTarget::from(printer_id("generic-machine")),
        CalibrationTarget::Generic,
    );
    assert_eq!(
        CalibrationTarget::Generic.printer_id(),
        generic_printer_id()
    );
}

#[test]
fn as_str_and_display_are_the_slug() {
    assert_eq!(CalibrationTarget::Generic.as_str(), "generic-machine");
    assert_eq!(CalibrationTarget::Generic.to_string(), "generic-machine");
    let printer = CalibrationTarget::from_printer(&printer_id("printer-a"));
    assert_eq!(printer.as_str(), "printer-a");
    assert_eq!(printer.to_string(), "printer-a");
}

#[test]
fn an_invalid_key_is_rejected_with_the_id_error() {
    let err = CalibrationTarget::try_from(String::from("Not A Slug")).unwrap_err();
    assert_eq!(err, PrinterId::new("Not A Slug").unwrap_err());
}

#[test]
fn generic_sorts_before_printers() {
    let mut targets = [
        CalibrationTarget::from_printer(&printer_id("aaa-printer")),
        CalibrationTarget::Generic,
        CalibrationTarget::from_printer(&printer_id("zzz-printer")),
    ];
    targets.sort();

    let slugs: Vec<&str> = targets.iter().map(CalibrationTarget::as_str).collect();
    assert_eq!(slugs, ["generic-machine", "aaa-printer", "zzz-printer"]);
}

const MATERIAL_WITH_GENERIC_BLOCK: &str = r#"
[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-a"]
normal_exposure_sec = 2.5

[per_printer."generic-machine"]
normal_exposure_sec = 4.0
"#;

#[test]
fn a_generic_key_round_trips_through_toml() {
    let parsed: MaterialProfile = toml_edit::de::from_str(MATERIAL_WITH_GENERIC_BLOCK).unwrap();

    let keys: Vec<&CalibrationTarget> = parsed.per_printer.keys().collect();
    assert_eq!(
        keys,
        [
            &CalibrationTarget::Generic,
            &CalibrationTarget::from_printer(&printer_id("printer-a")),
        ],
    );
    assert_eq!(
        parsed.per_printer[&CalibrationTarget::Generic]
            .normal_exposure_sec
            .unwrap()
            .value(),
        4.0,
    );

    let written = toml_edit::ser::to_string_pretty(&parsed).unwrap();
    assert!(
        written.contains("[per_printer.generic-machine]"),
        "{written}"
    );
    assert!(written.contains("[per_printer.printer-a]"), "{written}");
    let reparsed: MaterialProfile = toml_edit::de::from_str(&written).unwrap();
    assert_eq!(reparsed, parsed);
}

#[test]
fn a_key_that_is_not_a_slug_fails_to_parse() {
    let toml = MATERIAL_WITH_GENERIC_BLOCK.replace("\"printer-a\"", "\"Printer A\"");
    let err = toml_edit::de::from_str::<MaterialProfile>(&toml).unwrap_err();
    assert!(err.to_string().contains("Printer A"), "{err}");
}

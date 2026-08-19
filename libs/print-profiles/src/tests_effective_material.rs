/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::PrinterId;

use super::calibration_target::CalibrationTarget;
use super::effective_material::{EffectiveMaterial, MaterialExtensionIssue};
use super::material_profile::MaterialProfile;

fn profile(toml: &str) -> MaterialProfile {
    toml_edit::de::from_str(toml).unwrap()
}

fn printer_id(raw: &str) -> PrinterId {
    raw.parse().unwrap()
}

fn target(raw: &str) -> CalibrationTarget {
    CalibrationTarget::from_printer(&printer_id(raw))
}

const BASE: &str = r#"
[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[per_printer."printer-a"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
"#;

const EXTENSION_NEW_PAIRS: &str = r#"
extension = true

[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-b"]
normal_exposure_sec = 3.0
bottom_exposure_sec = 25.0

[per_printer."printer-c"]
normal_exposure_sec = 8.0
"#;

#[test]
fn a_base_profile_alone_keeps_every_section() {
    let base = profile(BASE);
    let effective = EffectiveMaterial::from(base.clone());

    assert_eq!(effective.identity(), &base.identity);
    assert_eq!(effective.intent(), &base.intent);
    assert_eq!(effective.defaults(), &base.defaults);
    assert_eq!(effective.per_printer(), &base.per_printer);
}

#[test]
fn composing_without_an_extension_yields_the_base_and_no_issues() {
    let base = profile(BASE);
    let (effective, issues) = EffectiveMaterial::compose(base.clone(), None);

    assert_eq!(effective, EffectiveMaterial::from(base));
    assert!(issues.is_empty());
}

#[test]
fn composing_adds_the_extensions_new_pairs() {
    let (effective, issues) =
        EffectiveMaterial::compose(profile(BASE), Some(profile(EXTENSION_NEW_PAIRS)));

    assert!(issues.is_empty());
    let ids: Vec<&CalibrationTarget> = effective.per_printer().keys().collect();
    assert_eq!(
        ids,
        vec![
            &target("printer-a"),
            &target("printer-b"),
            &target("printer-c"),
        ],
    );
    let added = &effective.per_printer()[&target("printer-b")];
    assert_eq!(added.normal_exposure_sec.unwrap().value(), 3.0);
    assert_eq!(added.bottom_exposure_sec.unwrap().value(), 25.0);
}

#[test]
fn an_overlapping_pair_is_excluded_and_its_new_siblings_still_land() {
    let extension = r#"
extension = true

[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-a"]
normal_exposure_sec = 9.0

[per_printer."printer-b"]
normal_exposure_sec = 3.0
"#;
    let (effective, issues) = EffectiveMaterial::compose(profile(BASE), Some(profile(extension)));

    assert_eq!(
        issues,
        vec![MaterialExtensionIssue::OverlappingCalibrationPair {
            target: target("printer-a"),
        }],
    );
    // The base's block stays in effect for the overlapped printer.
    let kept = &effective.per_printer()[&target("printer-a")];
    assert_eq!(kept.normal_exposure_sec.unwrap().value(), 2.5);
    // The valid sibling pair still landed.
    let added = &effective.per_printer()[&target("printer-b")];
    assert_eq!(added.normal_exposure_sec.unwrap().value(), 3.0);
}

#[test]
fn an_unflagged_file_is_excluded_whole() {
    let unflagged = r#"
[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-b"]
normal_exposure_sec = 3.0
"#;
    let base = profile(BASE);
    let (effective, issues) = EffectiveMaterial::compose(base.clone(), Some(profile(unflagged)));

    assert_eq!(issues, vec![MaterialExtensionIssue::ExtensionNotFlagged]);
    assert_eq!(effective, EffectiveMaterial::from(base));
}

#[test]
fn an_id_mismatched_file_is_excluded_whole() {
    let mismatched = r#"
extension = true

[identity]
id = "other-resin"
brand = "Test"
family = "standard"
name = "Other Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-b"]
normal_exposure_sec = 3.0
"#;
    let base = profile(BASE);
    let (effective, issues) = EffectiveMaterial::compose(base.clone(), Some(profile(mismatched)));

    assert_eq!(
        issues,
        vec![MaterialExtensionIssue::ExtensionIdentityMismatch {
            base: "test-resin".parse().unwrap(),
            extension: "other-resin".parse().unwrap(),
        }],
    );
    assert_eq!(effective, EffectiveMaterial::from(base));
}

#[test]
fn inherited_intent_values_are_ignored_and_reported() {
    let extension = r#"
extension = true

[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.1

[defaults]

[per_printer."printer-b"]
normal_exposure_sec = 3.0
"#;
    let base = profile(BASE);
    let (effective, issues) = EffectiveMaterial::compose(base.clone(), Some(profile(extension)));

    assert_eq!(
        issues,
        vec![MaterialExtensionIssue::InheritedIntentCarriesValues],
    );
    // The base's intent stays in effect and the valid pair landed.
    assert_eq!(effective.intent(), &base.intent);
    assert_eq!(
        effective.intent().target_layer_height_mm.unwrap().value(),
        0.05,
    );
    assert!(effective.per_printer().contains_key(&target("printer-b")),);
}

#[test]
fn inherited_defaults_values_are_ignored_and_reported() {
    let extension = r#"
extension = true

[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]

[defaults]
bottom_layer_count = 9

[per_printer."printer-b"]
normal_exposure_sec = 3.0
"#;
    let base = profile(BASE);
    let (effective, issues) = EffectiveMaterial::compose(base.clone(), Some(profile(extension)));

    assert_eq!(
        issues,
        vec![MaterialExtensionIssue::InheritedDefaultsCarryValues],
    );
    assert_eq!(effective.defaults(), &base.defaults);
    assert_eq!(effective.defaults().bottom_layer_count, Some(5));
}

#[test]
fn issues_accumulate_in_deterministic_order() {
    let base = r#"
[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]

[per_printer."printer-a"]
normal_exposure_sec = 2.5

[per_printer."printer-c"]
normal_exposure_sec = 4.0
"#;
    let extension = r#"
extension = true

[identity]
id = "test-resin"
brand = "Test"
family = "standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.1

[defaults]
bottom_layer_count = 9

[per_printer."printer-a"]
normal_exposure_sec = 9.0

[per_printer."printer-b"]
normal_exposure_sec = 3.0

[per_printer."printer-c"]
normal_exposure_sec = 9.0
"#;
    let (effective, issues) = EffectiveMaterial::compose(profile(base), Some(profile(extension)));

    assert_eq!(
        issues,
        vec![
            MaterialExtensionIssue::InheritedIntentCarriesValues,
            MaterialExtensionIssue::InheritedDefaultsCarryValues,
            MaterialExtensionIssue::OverlappingCalibrationPair {
                target: target("printer-a"),
            },
            MaterialExtensionIssue::OverlappingCalibrationPair {
                target: target("printer-c"),
            },
        ],
    );
    // Both overlapped pairs keep the base's values; the new pair lands.
    let map = effective.per_printer();
    assert_eq!(
        map[&target("printer-a")]
            .normal_exposure_sec
            .unwrap()
            .value(),
        2.5,
    );
    assert_eq!(
        map[&target("printer-b")]
            .normal_exposure_sec
            .unwrap()
            .value(),
        3.0,
    );
    assert_eq!(
        map[&target("printer-c")]
            .normal_exposure_sec
            .unwrap()
            .value(),
        4.0,
    );
}

#[test]
fn issue_display_names_what_was_dropped_and_why() {
    assert_eq!(
        MaterialExtensionIssue::ExtensionNotFlagged.to_string(),
        "the extension file is not flagged as an extension and was ignored",
    );
    assert_eq!(
        MaterialExtensionIssue::OverlappingCalibrationPair {
            target: target("printer-a"),
        }
        .to_string(),
        "the extension adds a calibration block for printer 'printer-a', which the base profile already covers; the base profile's block stays in effect",
    );
    assert_eq!(
        MaterialExtensionIssue::ExtensionIdentityMismatch {
            base: "test-resin".parse().unwrap(),
            extension: "other-resin".parse().unwrap(),
        }
        .to_string(),
        "the extension file names material 'other-resin' but the base profile is 'test-resin'; the file was ignored",
    );
}

const BASE_WITH_GENERIC_BLOCK: &str = r#"
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
fn calibration_for_prefers_the_own_block() {
    let effective = EffectiveMaterial::from(profile(BASE_WITH_GENERIC_BLOCK));

    let (key, block) = effective.calibration_for(&printer_id("printer-a")).unwrap();
    assert_eq!(key, &target("printer-a"));
    assert_eq!(block.normal_exposure_sec.unwrap().value(), 2.5);
}

#[test]
fn calibration_for_falls_back_to_the_generic_block() {
    let effective = EffectiveMaterial::from(profile(BASE_WITH_GENERIC_BLOCK));

    let (key, block) = effective.calibration_for(&printer_id("printer-b")).unwrap();
    assert_eq!(key, &CalibrationTarget::Generic);
    assert_eq!(block.normal_exposure_sec.unwrap().value(), 4.0);
}

#[test]
fn calibration_for_is_none_without_either_block() {
    let effective = EffectiveMaterial::from(profile(BASE));

    assert!(
        effective
            .calibration_for(&printer_id("printer-b"))
            .is_none()
    );
}

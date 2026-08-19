/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fs;

use sc_print_primitives::PrinterId;
use sc_print_profiles::load_material_profile_from_path;

use super::*;

const SOURCE_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"standard\"
name = \"Resin\"
profile_version = \"1.2.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[per_printer.printer-a]
normal_exposure_sec = 2.5

[per_printer.generic-machine]
normal_exposure_sec = 3.0
";

fn source_profile() -> MaterialProfile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.toml");
    fs::write(&path, SOURCE_TOML).unwrap();
    load_material_profile_from_path(&path).unwrap()
}

fn target(id: &str) -> CalibrationTarget {
    CalibrationTarget::from(PrinterId::new(id).unwrap())
}

fn no_ids() -> BTreeSet<MaterialId> {
    BTreeSet::new()
}

#[test]
fn a_bare_fork_copies_only_the_identity_frame() {
    let source = source_profile();
    let fork = build_fork(
        &source,
        &ForkParts::default(),
        "My Fork",
        "Brand",
        &no_ids(),
    )
    .unwrap();

    assert!(!fork.extension);
    assert_eq!(fork.identity.id.as_str(), "my-fork");
    assert_eq!(fork.identity.name.as_str(), "My Fork");
    assert_eq!(fork.identity.brand, source.identity.brand);
    assert_eq!(fork.identity.family, source.identity.family);
    assert_eq!(
        fork.identity.profile_version,
        source.identity.profile_version
    );
    assert_eq!(fork.intent.target_energy_density_mj_per_cm2, None);
    assert_eq!(fork.intent.target_layer_height_mm, None);
    assert_eq!(fork.defaults.scale_compensation_pct, None);
    assert_eq!(fork.defaults.bottom_layer_count, None);
    assert!(fork.per_printer.is_empty());
}

#[test]
fn every_selected_part_is_copied_verbatim() {
    let source = source_profile();
    let parts = ForkParts {
        intent: true,
        defaults: true,
        blocks: vec![target("printer-a"), CalibrationTarget::Generic],
    };
    let fork = build_fork(&source, &parts, "My Fork", "Brand", &no_ids()).unwrap();

    assert_eq!(fork.intent, source.intent);
    assert_eq!(fork.defaults, source.defaults);
    assert_eq!(fork.per_printer, source.per_printer);
}

#[test]
fn a_block_subset_copies_exactly_those_blocks() {
    let source = source_profile();
    let parts = ForkParts {
        intent: false,
        defaults: true,
        blocks: vec![CalibrationTarget::Generic],
    };
    let fork = build_fork(&source, &parts, "My Fork", "Brand", &no_ids()).unwrap();

    assert_eq!(fork.intent.target_layer_height_mm, None);
    assert_eq!(fork.defaults, source.defaults);
    assert_eq!(fork.per_printer.len(), 1);
    assert_eq!(
        fork.per_printer.get(&CalibrationTarget::Generic),
        source.per_printer.get(&CalibrationTarget::Generic),
    );
}

#[test]
fn the_fork_id_skips_taken_slugs() {
    let source = source_profile();
    let existing: BTreeSet<MaterialId> = [
        MaterialId::new("my-fork").unwrap(),
        MaterialId::new("my-fork-2").unwrap(),
    ]
    .into();
    let fork = build_fork(
        &source,
        &ForkParts::default(),
        "My Fork",
        "Brand",
        &existing,
    )
    .unwrap();
    assert_eq!(fork.identity.id.as_str(), "my-fork-3");
}

#[test]
fn punctuation_and_case_collapse_into_the_slug() {
    let source = source_profile();
    let fork = build_fork(
        &source,
        &ForkParts::default(),
        "  Grey!  (fork) 2 ",
        "Brand",
        &no_ids(),
    )
    .unwrap();
    assert_eq!(fork.identity.id.as_str(), "grey-fork-2");
    assert_eq!(fork.identity.name.as_str(), "Grey!  (fork) 2");
}

#[test]
fn a_whitespace_name_is_rejected() {
    let source = source_profile();
    let err = build_fork(&source, &ForkParts::default(), "   ", "Brand", &no_ids()).unwrap_err();
    assert_eq!(err, ForkError::EmptyName);
    assert_eq!(err.user_message(), "Enter a name for the forked profile");
}

#[test]
fn a_whitespace_brand_is_rejected() {
    let source = source_profile();
    let err = build_fork(&source, &ForkParts::default(), "My Fork", "   ", &no_ids()).unwrap_err();
    assert_eq!(err, ForkError::EmptyBrand);
}

#[test]
fn an_edited_brand_replaces_the_source_brand() {
    let source = source_profile();
    let fork = build_fork(
        &source,
        &ForkParts::default(),
        "My Fork",
        "  Other Brand ",
        &no_ids(),
    )
    .unwrap();
    assert_eq!(fork.identity.brand.as_str(), "Other Brand");
    // The id still derives from the name alone.
    assert_eq!(fork.identity.id.as_str(), "my-fork");
}

#[test]
fn a_name_with_no_slug_characters_has_no_id() {
    let source = source_profile();
    let err = build_fork(&source, &ForkParts::default(), "???", "Brand", &no_ids()).unwrap_err();
    assert_eq!(err, ForkError::NoAvailableId);
}

#[test]
fn forking_an_extension_yields_a_full_profile() {
    let mut source = source_profile();
    source.extension = true;
    let parts = ForkParts {
        intent: false,
        defaults: false,
        blocks: vec![target("printer-a")],
    };
    let fork = build_fork(&source, &parts, "My Fork", "Brand", &no_ids()).unwrap();
    assert!(!fork.extension);
}

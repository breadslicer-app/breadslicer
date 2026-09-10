/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::fs;
use std::path::PathBuf;

use sc_print_primitives::MaterialId;
use sc_print_profiles::{MaterialPhysical, MaterialProfile, load_material_profile_from_path};
use sc_profiles_converter::material_curation::{MaterialCuration, MaterialStatus};
use sc_profiles_converter::source_note::SourceNote;

/// The material profiles we write ourselves, which name no product and
/// so have no data sheet behind them.
const GENERIC_PROFILES: [&str; 2] = ["abs-like-405", "standard-405"];

/// The vendor resins we ship with no density.
const RESINS_WITHOUT_A_FIGURE: [&str; 1] = [
    // Its strontium aluminate pigment is far denser than the base
    // resin, so the rest of the line's figure would be wrong rather
    // than approximate.
    "siraya-tech-craft-glow-in-dark",
];

/// Resins exempted from the curation by their brand, because nobody has
/// gone looking for what that vendor publishes yet.
///
/// TODO: temporary. Drop this exemption, and the filter that reads it,
/// once ELEGOO's published data sheets have been read and their resins
/// carry curated figures or a named reason to have none.
const UNRESEARCHED_ID_PREFIX: &str = "elegoo-";

/// The resins the source still publishes that we decline to ship, each
/// with what its vendor still does with the product.
const DISCARDED: [(&str, MaterialStatus); 4] = [
    // The vendor's discontinued plant-based Easy line, which their
    // dataset still tunes for 21 machines.
    ("siraya-tech-easy-grey", MaterialStatus::Discontinued),
    (
        "siraya-tech-easy-obsidian-black",
        MaterialStatus::Discontinued,
    ),
    // Sold, and their settings hold only for hollow models printed in
    // fast mode, which a profile cannot say.
    (
        "elegoo-rapid-standard-photopolymer-resin-grey-100um",
        MaterialStatus::Current,
    ),
    ("elegoo-standard-resin-grey-100um", MaterialStatus::Current),
];

/// The curated figures this converter ships.
fn curation() -> MaterialCuration {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("materials.toml");
    MaterialCuration::open(&path)
        .unwrap_or_else(|err| panic!("the curation we ship does not open: {err}"))
}

/// Every material profile in the shipped library.
fn bundled_materials() -> Vec<MaterialProfile> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("profiles")
        .join("materials");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("cannot list {}: {err}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no materials found under {}",
        dir.display()
    );
    paths
        .iter()
        .map(|path| {
            load_material_profile_from_path(path)
                .unwrap_or_else(|err| panic!("bundled material failed to load: {err}"))
        })
        .collect()
}

#[test]
fn the_curation_we_ship_covers_every_resin_but_the_ones_with_no_sheet() {
    let curation = curation();

    let mut uncurated: Vec<String> = bundled_materials()
        .iter()
        .filter(|profile| curation.of(&profile.identity.id).is_none())
        .map(|profile| profile.identity.id.as_str().to_owned())
        .filter(|id| !id.starts_with(UNRESEARCHED_ID_PREFIX))
        .collect();
    uncurated.sort();

    let mut expected: Vec<String> = GENERIC_PROFILES
        .iter()
        .chain(RESINS_WITHOUT_A_FIGURE.iter())
        .map(|id| (*id).to_owned())
        .collect();
    expected.sort();
    assert_eq!(uncurated, expected);
}

#[test]
fn every_curated_entry_states_a_figure_and_where_it_came_from() {
    let curation = curation();

    for profile in bundled_materials() {
        let id = &profile.identity.id;
        let Some(entry) = curation.of(id) else {
            continue;
        };

        assert_ne!(
            entry.physical,
            MaterialPhysical::default(),
            "{id} has an entry that states no density"
        );
        assert!(!entry.source.is_empty(), "{id} names no sheet");
        assert!(
            !entry.read_on.is_empty(),
            "{id} does not say when its sheet was read"
        );
    }
}

#[test]
fn the_resins_we_reject_are_discarded_with_the_reason_recorded() {
    let curation = curation();

    for (id, status) in DISCARDED {
        let material = MaterialId::new(id).unwrap();
        let entry = curation
            .of(&material)
            .unwrap_or_else(|| panic!("{id} has no entry, so a conversion would write it"));

        assert!(entry.discarded, "{id} is not discarded");
        assert_eq!(entry.status, status, "{id}");
        assert!(!entry.source.is_empty(), "{id} names no evidence");
    }
}

#[test]
fn no_profile_we_ship_is_one_the_curation_discards() {
    let curation = curation();

    for profile in bundled_materials() {
        assert!(
            !curation.discards(&profile.identity.id),
            "{} is both shipped and discarded",
            profile.identity.id
        );
    }
}

#[test]
fn applying_the_curation_leaves_every_shipped_figure_as_it_is() {
    let curation = curation();

    for mut profile in bundled_materials() {
        let shipped = profile.physical;
        let notes = curation.apply(&mut profile);

        assert_eq!(
            profile.physical, shipped,
            "{} would come out of a conversion with a different density",
            profile.identity.id
        );
        for note in notes {
            assert!(
                matches!(note, SourceNote::NoCurationForMaterial { .. }),
                "{note}"
            );
        }
    }
}

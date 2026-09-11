/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! One resin a source names 2 ways, back in one profile.
//!
//! A source that spells a resin's name differently across its records
//! makes the converter derive 2 ids, and so 2 profiles that each
//! calibrate some of the resin's machines. Nothing in the records tells
//! that apart from a second resin, so our curation says which id the
//! resin is filed under, and this step moves the other profile's
//! machines into it.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;

use sc_print_primitives::MaterialId;
use sc_print_profiles::{CalibrationTarget, MaterialIdentity, MaterialProfile};

use crate::material_curation::MaterialCuration;
use crate::profile::ConvertedProfile;
use crate::source_note::SourceNote;

/// Returned by [`fold`] when 2 profiles our curation calls one resin
/// disagree, so folding one into the other would drop what it states.
#[derive(Debug)]
pub struct FoldError {
    pub material: MaterialId,
    pub into: MaterialId,
    pub conflict: FoldConflict,
}

/// Where 2 profiles filed as one resin disagree.
#[derive(Debug, PartialEq, Eq)]
pub enum FoldConflict {
    /// A field that is not per machine.
    ResinFields,
    /// The calibration of this machine.
    Machine(CalibrationTarget),
}

impl fmt::Display for FoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "our curation files {:?} under {:?}, but ",
            self.material.as_str(),
            self.into.as_str(),
        )?;
        match &self.conflict {
            FoldConflict::ResinFields => {
                f.write_str("the 2 disagree on a field that is not per machine")
            }
            FoldConflict::Machine(machine) => write!(f, "the 2 calibrate {machine} differently"),
        }
    }
}

impl std::error::Error for FoldError {}

/// Fold every material our curation files under another id into that
/// resin's profile, returning what is left and a note per fold.
///
/// A folded profile whose resin the run did not produce takes that
/// resin's id and keeps its own name.
pub fn fold(
    profiles: Vec<ConvertedProfile>,
    curation: Option<&MaterialCuration>,
) -> Result<(Vec<ConvertedProfile>, Vec<SourceNote>), FoldError> {
    let Some(curation) = curation else {
        return Ok((profiles, Vec::new()));
    };
    let mut kept = Vec::with_capacity(profiles.len());
    let mut aliased = Vec::new();
    for profile in profiles {
        match profile {
            ConvertedProfile::Material(material) => {
                match curation.filed_under(&material.identity.id) {
                    Some(into) => aliased.push((material, into.clone())),
                    None => kept.push(ConvertedProfile::Material(material)),
                }
            }
            printer @ ConvertedProfile::Printer(_) => kept.push(printer),
        }
    }
    let mut notes = Vec::with_capacity(aliased.len());
    for (mut material, into) in aliased {
        let alias = material.identity.id.clone();
        let resin = kept.iter_mut().find_map(|profile| match profile {
            ConvertedProfile::Material(resin) if resin.identity.id == into => Some(resin),
            _ => None,
        });
        match resin {
            Some(resin) => fold_into(resin, material).map_err(|conflict| FoldError {
                material: alias.clone(),
                into: into.clone(),
                conflict,
            })?,
            None => {
                material.identity.id = into.clone();
                kept.push(ConvertedProfile::Material(material));
            }
        }
        notes.push(SourceNote::MaterialFolded {
            material: alias,
            into,
        });
    }
    Ok((kept, notes))
}

/// Move every machine `alias` calibrates into `resin`, provided the 2
/// state the same resin and agree on any machine both calibrate.
fn fold_into(resin: &mut MaterialProfile, alias: MaterialProfile) -> Result<(), FoldConflict> {
    if resin_fields(resin, &resin.identity) != resin_fields(&alias, &resin.identity) {
        return Err(FoldConflict::ResinFields);
    }
    for (machine, block) in alias.per_printer {
        match resin.per_printer.entry(machine) {
            Entry::Vacant(slot) => {
                slot.insert(block);
            }
            Entry::Occupied(slot) if *slot.get() == block => {}
            Entry::Occupied(slot) => return Err(FoldConflict::Machine(slot.key().clone())),
        }
    }
    // A block's note comes with it, and 2 different notes on one
    // machine are both kept.
    for (machine, note) in alias.notes {
        match resin.notes.entry(machine) {
            Entry::Vacant(slot) => {
                slot.insert(note);
            }
            Entry::Occupied(mut slot) if *slot.get() != note => {
                let kept = slot.get_mut();
                kept.push('\n');
                kept.push_str(&note);
            }
            Entry::Occupied(_) => {}
        }
    }
    Ok(())
}

/// Strip `profile` down to the fields that are not per machine, under
/// the id and name `identity` carries, so 2 spellings of one resin
/// compare equal.
fn resin_fields(profile: &MaterialProfile, identity: &MaterialIdentity) -> MaterialProfile {
    MaterialProfile {
        identity: MaterialIdentity {
            id: identity.id.clone(),
            name: identity.name.clone(),
            ..profile.identity.clone()
        },
        per_printer: BTreeMap::new(),
        notes: BTreeMap::new(),
        ..profile.clone()
    }
}

#[cfg(test)]
mod material_fold_tests {
    use std::fs;

    use sc_print_primitives::PrinterId;
    use tempfile::tempdir;

    use super::*;

    /// Build a curation filing `testcorp-resin-gr` under `testcorp-resin`.
    fn curation() -> MaterialCuration {
        let dir = tempdir().unwrap();
        let path = dir.path().join("materials.toml");
        fs::write(
            &path,
            "[testcorp-resin]\nsource = \"s\"\nread_on = \"d\"\naliases = [\"testcorp-resin-gr\"]\n",
        )
        .unwrap();
        MaterialCuration::open(&path).unwrap()
    }

    /// Write a resin calibrated on each machine at the exposure given
    /// for it.
    fn resin_toml(id: &str, name: &str, machines: &[(&str, f64)]) -> String {
        let mut text = format!(
            "[identity]\nid = \"{id}\"\nbrand = \"TestCorp\"\nfamily = \"standard\"\nname = \"{name}\"\nprofile_version = \"1.0.0\"\n\n[intent]\ntarget_layer_height_mm = 0.05\n\n[defaults]\n"
        );
        for (machine, exposure) in machines {
            text.push_str(&format!(
                "\n[per_printer.{machine}]\nnormal_exposure_sec = {exposure}\n"
            ));
        }
        text
    }

    fn resin(id: &str, name: &str, machines: &[(&str, f64)]) -> ConvertedProfile {
        ConvertedProfile::Material(
            toml_edit::de::from_str(&resin_toml(id, name, machines)).unwrap(),
        )
    }

    fn materials(profiles: &[ConvertedProfile]) -> Vec<&MaterialProfile> {
        profiles
            .iter()
            .filter_map(|profile| match profile {
                ConvertedProfile::Material(material) => Some(material),
                ConvertedProfile::Printer(_) => None,
            })
            .collect()
    }

    fn machine(id: &str) -> CalibrationTarget {
        CalibrationTarget::from_printer(&PrinterId::new(id).unwrap())
    }

    fn folded_note() -> SourceNote {
        SourceNote::MaterialFolded {
            material: MaterialId::new("testcorp-resin-gr").unwrap(),
            into: MaterialId::new("testcorp-resin").unwrap(),
        }
    }

    #[test]
    fn a_resin_named_2_ways_becomes_one_profile_calibrating_every_machine() {
        let (profiles, notes) = fold(
            vec![
                resin("testcorp-resin", "Resin", &[("testcorp-a", 2.0)]),
                resin("testcorp-resin-gr", "Resin Gr", &[("testcorp-b", 3.0)]),
            ],
            Some(&curation()),
        )
        .unwrap();

        let [resin] = materials(&profiles)[..] else {
            panic!("expected 1 resin, got {}", profiles.len());
        };
        assert_eq!(resin.identity.id.as_str(), "testcorp-resin");
        assert_eq!(resin.identity.name.as_str(), "Resin");
        assert_eq!(
            resin.per_printer.keys().cloned().collect::<Vec<_>>(),
            vec![machine("testcorp-a"), machine("testcorp-b")],
        );
        assert_eq!(notes, vec![folded_note()]);
    }

    #[test]
    fn a_machine_both_calibrate_alike_folds_into_one_block() {
        let (profiles, _) = fold(
            vec![
                resin("testcorp-resin", "Resin", &[("testcorp-a", 2.0)]),
                resin("testcorp-resin-gr", "Resin Gr", &[("testcorp-a", 2.0)]),
            ],
            Some(&curation()),
        )
        .unwrap();

        assert_eq!(materials(&profiles)[0].per_printer.len(), 1);
    }

    /// Fold an alias that calibrates `machines` and notes `noted`.
    fn fold_with_a_noted_alias(
        machines: &[(&str, f64)],
        noted: &str,
    ) -> BTreeMap<CalibrationTarget, String> {
        let mut alias: MaterialProfile =
            toml_edit::de::from_str(&resin_toml("testcorp-resin-gr", "Resin Gr", machines))
                .unwrap();
        alias
            .notes
            .insert(machine(noted), "Works for fast printing.".to_owned());
        let (profiles, _) = fold(
            vec![
                resin("testcorp-resin", "Resin", &[("testcorp-a", 2.0)]),
                ConvertedProfile::Material(alias),
            ],
            Some(&curation()),
        )
        .unwrap();
        materials(&profiles)[0].notes.clone()
    }

    #[test]
    fn a_machine_the_alias_brings_keeps_its_note() {
        let notes = fold_with_a_noted_alias(&[("testcorp-b", 3.0)], "testcorp-b");

        assert_eq!(
            notes.get(&machine("testcorp-b")).map(String::as_str),
            Some("Works for fast printing."),
        );
    }

    #[test]
    fn a_note_on_a_machine_both_calibrate_alike_is_kept() {
        let notes = fold_with_a_noted_alias(&[("testcorp-a", 2.0)], "testcorp-a");

        assert_eq!(
            notes.get(&machine("testcorp-a")).map(String::as_str),
            Some("Works for fast printing."),
        );
    }

    #[test]
    fn different_notes_on_one_machine_are_both_kept() {
        let mut kept: MaterialProfile = toml_edit::de::from_str(&resin_toml(
            "testcorp-resin",
            "Resin",
            &[("testcorp-a", 2.0)],
        ))
        .unwrap();
        kept.notes
            .insert(machine("testcorp-a"), "Keep the vat warm.".to_owned());
        let mut alias: MaterialProfile = toml_edit::de::from_str(&resin_toml(
            "testcorp-resin-gr",
            "Resin Gr",
            &[("testcorp-a", 2.0)],
        ))
        .unwrap();
        alias
            .notes
            .insert(machine("testcorp-a"), "Works for fast printing.".to_owned());

        let (profiles, _) = fold(
            vec![
                ConvertedProfile::Material(kept),
                ConvertedProfile::Material(alias),
            ],
            Some(&curation()),
        )
        .unwrap();

        assert_eq!(
            materials(&profiles)[0]
                .notes
                .get(&machine("testcorp-a"))
                .map(String::as_str),
            Some("Keep the vat warm.\nWorks for fast printing."),
        );
    }

    #[test]
    fn a_machine_the_2_calibrate_differently_stops_the_fold() {
        let err = fold(
            vec![
                resin("testcorp-resin", "Resin", &[("testcorp-a", 2.0)]),
                resin("testcorp-resin-gr", "Resin Gr", &[("testcorp-a", 3.6)]),
            ],
            Some(&curation()),
        )
        .unwrap_err();

        assert_eq!(err.material.as_str(), "testcorp-resin-gr");
        assert_eq!(err.into.as_str(), "testcorp-resin");
        assert_eq!(err.conflict, FoldConflict::Machine(machine("testcorp-a")));
    }

    #[test]
    fn a_disagreement_outside_the_machines_stops_the_fold() {
        let tough = resin_toml("testcorp-resin-gr", "Resin Gr", &[("testcorp-b", 2.0)])
            .replace("standard", "tough");
        let err = fold(
            vec![
                resin("testcorp-resin", "Resin", &[("testcorp-a", 2.0)]),
                ConvertedProfile::Material(toml_edit::de::from_str(&tough).unwrap()),
            ],
            Some(&curation()),
        )
        .unwrap_err();

        assert_eq!(err.conflict, FoldConflict::ResinFields);
    }

    #[test]
    fn an_alias_whose_resin_the_run_did_not_produce_takes_that_resin_s_id() {
        let (profiles, notes) = fold(
            vec![resin(
                "testcorp-resin-gr",
                "Resin Gr",
                &[("testcorp-b", 3.0)],
            )],
            Some(&curation()),
        )
        .unwrap();

        let [resin] = materials(&profiles)[..] else {
            panic!("expected 1 resin, got {}", profiles.len());
        };
        assert_eq!(resin.identity.id.as_str(), "testcorp-resin");
        assert_eq!(resin.identity.name.as_str(), "Resin Gr");
        assert_eq!(notes, vec![folded_note()]);
    }

    #[test]
    fn a_resin_no_entry_lists_as_an_alias_is_left_alone() {
        let other = resin("testcorp-other", "Other", &[("testcorp-a", 2.0)]);

        let (profiles, notes) = fold(vec![other.clone()], Some(&curation())).unwrap();

        assert_eq!(profiles, vec![other]);
        assert_eq!(notes, Vec::new());
    }

    #[test]
    fn without_a_curation_nothing_is_folded() {
        let alias = resin("testcorp-resin-gr", "Resin Gr", &[("testcorp-b", 3.0)]);

        let (profiles, notes) = fold(vec![alias.clone()], None).unwrap();

        assert_eq!(profiles, vec![alias]);
        assert_eq!(notes, Vec::new());
    }

    #[test]
    fn a_fold_error_names_both_ids_and_what_they_disagree_on() {
        let error = |conflict| {
            FoldError {
                material: MaterialId::new("testcorp-resin-gr").unwrap(),
                into: MaterialId::new("testcorp-resin").unwrap(),
                conflict,
            }
            .to_string()
        };

        let on_a_machine = error(FoldConflict::Machine(machine("testcorp-a")));
        assert!(
            on_a_machine.contains("\"testcorp-resin-gr\" under \"testcorp-resin\""),
            "{on_a_machine}"
        );
        assert!(
            on_a_machine.contains("calibrate testcorp-a differently"),
            "{on_a_machine}"
        );
        let on_the_resin = error(FoldConflict::ResinFields);
        assert!(on_the_resin.contains("not per machine"), "{on_the_resin}");
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What we know about a resin that no source states.
//!
//! A source dataset publishes how to print a resin. What the resin
//! weighs comes from its manufacturer's data sheet, and no source
//! carries it, so a conversion that wrote only what it read would leave
//! the field empty and delete whatever a person had looked up. This
//! module reads the file where those figures are recorded, each with
//! the sheet it came from, and applies them to a converted profile.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sc_print_primitives::{DensityGPerCm3, MaterialId};
use sc_print_profiles::{MaterialPhysical, MaterialProfile};
use serde::Deserialize;

use crate::source_note::SourceNote;

/// What the vendor does with a resin.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialStatus {
    /// The vendor still sells it.
    #[default]
    Current,
    /// The vendor has stopped selling it.
    Discontinued,
}

impl MaterialStatus {
    /// Spell this status the way the curated file does.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Discontinued => "discontinued",
        }
    }
}

/// One resin as our own curation describes it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CuratedMaterial {
    /// Where the statements below were published.
    pub source: String,
    /// The day that page was read, so a statement can be re-checked
    /// against a page the vendor has since revised.
    pub read_on: String,
    /// What the vendor does with this resin.
    #[serde(default)]
    pub status: MaterialStatus,
    /// Whether we decline to ship a profile for this resin. Our own
    /// decision, separate from [`Self::status`]: a resin nobody sells
    /// any more can still be worth a profile to whoever holds a bottle.
    #[serde(default)]
    pub discarded: bool,
    /// The `[physical]` section a profile of this resin carries.
    #[serde(default)]
    pub physical: MaterialPhysical,
}

/// Every resin the curated file describes, by the id we give it.
#[derive(Clone, Debug, Default)]
pub struct MaterialCuration {
    entries: BTreeMap<MaterialId, CuratedMaterial>,
}

/// Returned by [`MaterialCuration::open`] when the curated file cannot
/// be read.
#[derive(Debug)]
pub enum MaterialCurationError {
    /// The file could not be opened.
    Read { path: PathBuf, source: io::Error },
    /// Its contents are not the curation this expects. An id that is
    /// not a valid slug, an entry missing its provenance and a key
    /// nothing reads all arrive here.
    Parse {
        path: PathBuf,
        source: toml_edit::de::Error,
    },
}

impl fmt::Display for MaterialCurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(
                f,
                "failed to read the material curation at {}: {source}",
                path.display(),
            ),
            Self::Parse { path, source } => {
                write!(f, "failed to parse {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for MaterialCurationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
        }
    }
}

impl MaterialCuration {
    /// Read the curated file at `path`.
    ///
    /// A file that does not parse fails the run rather than leaving the
    /// curation empty: it is our own record, and a conversion that
    /// treated it as absent would quietly write away every figure in
    /// it.
    pub fn open(path: &Path) -> Result<Self, MaterialCurationError> {
        let text = fs::read_to_string(path).map_err(|source| MaterialCurationError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let entries =
            toml_edit::de::from_str(&text).map_err(|source| MaterialCurationError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self { entries })
    }

    /// What the file states about the resin `id` names, if anything.
    pub fn of(&self, id: &MaterialId) -> Option<&CuratedMaterial> {
        self.entries.get(id)
    }

    /// Whether we decline to ship a profile for the resin `id` names.
    pub fn discards(&self, id: &MaterialId) -> bool {
        self.entries.get(id).is_some_and(|entry| entry.discarded)
    }

    /// Fill the sections our curation owns in `profile`, and report
    /// what that displaced.
    ///
    /// Those sections are the ones no source publishes, and today
    /// `[physical]` is the only one: curating a second is a change to
    /// this function and to nothing else. A resin the file says nothing
    /// about keeps what the source gave it, and the run says so.
    pub fn apply(&self, profile: &mut MaterialProfile) -> Vec<SourceNote> {
        let Some(curated) = self.entries.get(&profile.identity.id) else {
            return vec![SourceNote::NoCurationForMaterial {
                material: profile.identity.id.clone(),
            }];
        };
        curate_physical(
            &profile.identity.id,
            &curated.source,
            &curated.physical,
            &mut profile.physical,
        )
    }
}

/// Put the curated densities into `into`, reporting each one that
/// displaces a figure the source stated.
fn curate_physical(
    material: &MaterialId,
    sheet: &str,
    curated: &MaterialPhysical,
    into: &mut MaterialPhysical,
) -> Vec<SourceNote> {
    [
        (
            "liquid density",
            curated.liquid_density_g_per_cm3,
            &mut into.liquid_density_g_per_cm3,
        ),
        (
            "cured density",
            curated.cured_density_g_per_cm3,
            &mut into.cured_density_g_per_cm3,
        ),
    ]
    .into_iter()
    .filter_map(|(field, ours, slot)| curate_density(material, sheet, field, ours, slot))
    .collect()
}

/// Replace whatever `slot` holds with `ours`, and report a figure that
/// was already there and differed.
fn curate_density(
    material: &MaterialId,
    sheet: &str,
    field: &'static str,
    ours: Option<DensityGPerCm3>,
    slot: &mut Option<DensityGPerCm3>,
) -> Option<SourceNote> {
    let ours = ours?;
    let displaced = slot.replace(ours)?;
    (displaced != ours).then(|| SourceNote::CurationOverridesTheSource {
        material: material.clone(),
        field,
        kept: ours.to_string(),
        source_states: displaced.to_string(),
        sheet: sheet.to_owned(),
    })
}

#[cfg(test)]
mod material_curation_tests {
    use tempfile::tempdir;

    use super::*;

    /// A resin as a source leaves it: no density anywhere.
    const RESIN: &str = "\
[identity]
id = \"testcorp-resin\"
brand = \"TestCorp\"
family = \"standard\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
";

    const CURATED: &str = "\
[testcorp-resin]
source = \"https://testcorp.example/resin-tds\"
read_on = \"2026-09-10\"
physical = { liquid_density_g_per_cm3 = 1.1, cured_density_g_per_cm3 = 1.15 }
";

    fn curation_of(text: &str) -> Result<MaterialCuration, MaterialCurationError> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("materials.toml");
        fs::write(&path, text).unwrap();
        MaterialCuration::open(&path)
    }

    fn resin(text: &str) -> MaterialProfile {
        toml_edit::de::from_str(text).unwrap()
    }

    fn density(value: f64) -> Option<DensityGPerCm3> {
        Some(DensityGPerCm3::new(value).unwrap())
    }

    #[test]
    fn an_entry_is_found_by_the_id_we_give_the_resin() {
        let curation = curation_of(CURATED).unwrap();

        let entry = curation
            .of(&MaterialId::new("testcorp-resin").unwrap())
            .unwrap();

        assert_eq!(entry.source, "https://testcorp.example/resin-tds");
        assert_eq!(entry.read_on, "2026-09-10");
        assert_eq!(entry.physical.liquid_density_g_per_cm3, density(1.1));
    }

    #[test]
    fn an_entry_without_the_sheet_it_came_from_is_refused() {
        let err = curation_of(
            "[testcorp-resin]\nread_on = \"2026-09-10\"\nphysical = { cured_density_g_per_cm3 = 1.2 }\n",
        )
        .unwrap_err();

        assert!(matches!(err, MaterialCurationError::Parse { .. }), "{err}");
        assert!(err.to_string().contains("source"), "{err}");
    }

    #[test]
    fn a_key_nothing_reads_is_refused_rather_than_ignored() {
        let err =
            curation_of("[testcorp-resin]\nsource = \"s\"\nread_on = \"d\"\nviscosity = 60\n")
                .unwrap_err();

        assert!(matches!(err, MaterialCurationError::Parse { .. }), "{err}");
    }

    #[test]
    fn a_key_that_is_not_a_material_id_is_refused() {
        let err = curation_of("[\"Not An Id\"]\nsource = \"s\"\nread_on = \"d\"\n").unwrap_err();

        assert!(matches!(err, MaterialCurationError::Parse { .. }), "{err}");
    }

    #[test]
    fn a_file_that_is_not_there_is_reported_as_such() {
        let dir = tempdir().unwrap();

        let err = MaterialCuration::open(&dir.path().join("absent.toml")).unwrap_err();

        assert!(matches!(err, MaterialCurationError::Read { .. }), "{err}");
    }

    #[test]
    fn a_curated_figure_fills_a_field_the_source_left_empty() {
        let curation = curation_of(CURATED).unwrap();
        let mut profile = resin(RESIN);

        let notes = curation.apply(&mut profile);

        assert_eq!(profile.physical.liquid_density_g_per_cm3, density(1.1));
        assert_eq!(profile.physical.cured_density_g_per_cm3, density(1.15));
        assert_eq!(notes, Vec::new());
    }

    #[test]
    fn our_own_figure_displaces_a_source_that_disagrees_and_says_so() {
        let curation = curation_of(CURATED).unwrap();
        let mut profile = resin(&format!(
            "{RESIN}\n[physical]\nliquid_density_g_per_cm3 = 1.2\n"
        ));

        let notes = curation.apply(&mut profile);

        assert_eq!(profile.physical.liquid_density_g_per_cm3, density(1.1));
        assert_eq!(
            notes,
            vec![SourceNote::CurationOverridesTheSource {
                material: MaterialId::new("testcorp-resin").unwrap(),
                field: "liquid density",
                kept: "1.1".to_owned(),
                source_states: "1.2".to_owned(),
                sheet: "https://testcorp.example/resin-tds".to_owned(),
            }],
        );
    }

    #[test]
    fn a_source_that_agrees_with_us_raises_nothing() {
        let curation = curation_of(CURATED).unwrap();
        let mut profile = resin(&format!(
            "{RESIN}\n[physical]\nliquid_density_g_per_cm3 = 1.1\ncured_density_g_per_cm3 = 1.15\n"
        ));

        let notes = curation.apply(&mut profile);

        assert_eq!(notes, Vec::new());
    }

    #[test]
    fn a_field_we_do_not_curate_keeps_whatever_the_source_gave_it() {
        let curation = curation_of(
            "[testcorp-resin]\nsource = \"s\"\nread_on = \"d\"\nphysical = { cured_density_g_per_cm3 = 1.2 }\n",
        )
        .unwrap();
        let mut profile = resin(&format!(
            "{RESIN}\n[physical]\nliquid_density_g_per_cm3 = 1.05\n"
        ));

        let notes = curation.apply(&mut profile);

        assert_eq!(profile.physical.liquid_density_g_per_cm3, density(1.05));
        assert_eq!(profile.physical.cured_density_g_per_cm3, density(1.2));
        assert_eq!(notes, Vec::new());
    }

    #[test]
    fn a_resin_is_shipped_unless_the_entry_discards_it() {
        let curation = curation_of(CURATED).unwrap();

        let id = MaterialId::new("testcorp-resin").unwrap();
        assert!(!curation.discards(&id));
        assert_eq!(curation.of(&id).unwrap().status, MaterialStatus::Current);
        assert!(!curation.discards(&MaterialId::new("testcorp-absent").unwrap()));
    }

    #[test]
    fn an_entry_can_discard_a_resin_and_say_what_became_of_it() {
        let curation = curation_of(
            "[testcorp-resin]\nsource = \"s\"\nread_on = \"d\"\nstatus = \"discontinued\"\ndiscarded = true\n",
        )
        .unwrap();

        let entry = curation
            .of(&MaterialId::new("testcorp-resin").unwrap())
            .unwrap();

        assert!(entry.discarded);
        assert_eq!(entry.status, MaterialStatus::Discontinued);
        assert_eq!(entry.status.as_str(), "discontinued");
        assert!(curation.discards(&MaterialId::new("testcorp-resin").unwrap()));
    }

    #[test]
    fn a_status_the_vocabulary_does_not_hold_is_refused() {
        let err = curation_of(
            "[testcorp-resin]\nsource = \"s\"\nread_on = \"d\"\nstatus = \"withdrawn\"\n",
        )
        .unwrap_err();

        assert!(matches!(err, MaterialCurationError::Parse { .. }), "{err}");
    }

    #[test]
    fn a_discontinued_resin_is_still_shipped_unless_we_discard_it() {
        let curation = curation_of(
            "[testcorp-resin]\nsource = \"s\"\nread_on = \"d\"\nstatus = \"discontinued\"\n",
        )
        .unwrap();

        assert!(!curation.discards(&MaterialId::new("testcorp-resin").unwrap()));
    }

    #[test]
    fn a_resin_the_file_says_nothing_about_is_reported() {
        let curation = curation_of(CURATED).unwrap();
        let mut profile = resin(&RESIN.replace("testcorp-resin", "testcorp-other"));

        let notes = curation.apply(&mut profile);

        assert_eq!(
            notes,
            vec![SourceNote::NoCurationForMaterial {
                material: MaterialId::new("testcorp-other").unwrap(),
            }],
        );
        assert_eq!(profile.physical, MaterialPhysical::default());
    }
}

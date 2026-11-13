// SPDX-FileCopyrightText: 2026 SynrgStudio
// SPDX-License-Identifier: AGPL-3.0-only

//! Support settings as one file to hand to someone else (#564): the
//! presets of Generate a person made and the sizes the hand tools place,
//! whichever of them were picked. A text file (TOML) under its own
//! extension; the one who opens it picks what to take from it.

use sc_geom::df_supports::{DfSupportOptions, SupportSize};
use serde::{Deserialize, Serialize};

/// The extension of a support settings file.
pub(crate) const EXTENSION: &str = "breadsupports";

const FORMAT: &str = "breadslicer-supports";
const VERSION: u32 = 1;

/// A preset of Generate, with its name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SharedPreset {
    pub(crate) name: String,
    pub(crate) options: DfSupportOptions,
}

/// One thing a file holds, and a person picks or leaves.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Entry {
    Preset(SharedPreset),
    /// The sizes the hand tools place.
    HandSizes(Vec<SupportSize>),
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hand_sizes: Option<Vec<SupportSize>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    presets: Vec<SharedPreset>,
}

/// Why a file was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadError {
    /// Not a support settings file.
    NotOne,
    /// Written by a newer Breadslicer.
    Newer,
    /// One, with nothing in it.
    Empty,
}

/// The text of a file holding `entries`: its presets in their order, and
/// the last hand sizes among them.
pub(crate) fn write(entries: &[Entry]) -> Option<String> {
    let mut file = File {
        format: FORMAT.to_owned(),
        version: VERSION,
        hand_sizes: None,
        presets: Vec::new(),
    };
    for entry in entries {
        match entry {
            Entry::Preset(p) => file.presets.push(p.clone()),
            Entry::HandSizes(sizes) => file.hand_sizes = Some(sizes.clone()),
        }
    }
    toml::to_string_pretty(&file).ok()
}

/// What the file of text `text` holds: the hand sizes first, then its
/// presets, their numbers inside the engine's bounds.
pub(crate) fn read(text: &str) -> Result<Vec<Entry>, ReadError> {
    let file: File = toml::from_str(text).map_err(|_| ReadError::NotOne)?;
    if file.format != FORMAT {
        return Err(ReadError::NotOne);
    }
    if file.version > VERSION {
        return Err(ReadError::Newer);
    }
    let mut entries = Vec::new();
    if let Some(sizes) = file.hand_sizes.filter(|s| !s.is_empty()) {
        entries.push(Entry::HandSizes(
            sizes.into_iter().map(|s| s.clamped()).collect(),
        ));
    }
    entries.extend(
        file.presets
            .into_iter()
            .filter(|p| !p.name.trim().is_empty())
            .map(|p| {
                Entry::Preset(SharedPreset {
                    name: p.name.trim().to_owned(),
                    options: p.options.clamped(),
                })
            }),
    );
    if entries.is_empty() {
        return Err(ReadError::Empty);
    }
    Ok(entries)
}

/// `name`, or `name (2)`, `name (3)`… when a preset of `taken` has it
/// already (whatever the case): a preset taken from a file never replaces
/// one the person has.
pub(crate) fn free_name(name: &str, taken: &[String]) -> String {
    let has = |candidate: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(candidate));
    if !has(name) {
        return name.to_owned();
    }
    (2..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !has(candidate))
        .unwrap_or_else(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_geom::df_supports::zone_presets::{figures_150, minis_30, support_sizes};

    fn preset(name: &str, zones: sc_geom::df_supports::zone_presets::ZonePreset) -> Entry {
        Entry::Preset(SharedPreset {
            name: name.to_owned(),
            options: DfSupportOptions {
                zones: Some(zones),
                ..Default::default()
            }
            .clamped(),
        })
    }

    /// What goes into a file comes back from it: the presets with every
    /// number, and the hand sizes.
    #[test]
    fn a_file_gives_back_what_was_put_in_it() {
        let mut thick = minis_30();
        thick.rules.sizes[1].trunk_diameter_mm = 2.5;
        let mut sizes: Vec<SupportSize> = support_sizes().into_iter().take(4).collect();
        sizes[0].tip_diameter_mm = 0.21;
        let entries = vec![
            Entry::HandSizes(sizes),
            preset("Thick minis", thick),
            preset("Statues of mine", figures_150()),
        ];
        let text = write(&entries).expect("a file");
        assert_eq!(read(&text), Ok(entries.clone()));
        // Only what was picked is in the file.
        let one = write(&entries[2..]).expect("a file");
        assert_eq!(read(&one), Ok(entries[2..].to_vec()));
    }

    #[test]
    fn a_file_that_is_not_one_or_is_newer_is_told_apart() {
        assert_eq!(read("hello"), Err(ReadError::NotOne));
        assert_eq!(
            read("format = \"other\"\nversion = 1\n"),
            Err(ReadError::NotOne)
        );
        assert_eq!(
            read("format = \"breadslicer-supports\"\nversion = 2\n"),
            Err(ReadError::Newer)
        );
        assert_eq!(
            read("format = \"breadslicer-supports\"\nversion = 1\n"),
            Err(ReadError::Empty)
        );
    }

    #[test]
    fn a_name_taken_gets_a_number() {
        let taken = vec!["Minis".to_owned(), "minis (2)".to_owned()];
        assert_eq!(free_name("Other", &taken), "Other");
        assert_eq!(free_name("Minis", &taken), "Minis (3)");
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Which machines a row's model column names.
//!
//! The sheet names a model the way its marketing does, so one cell can
//! cover 2 machines, and one machine can be spelled 2 ways. The mapping
//! is written out rather than derived from the spelling, because a
//! slug computed from "Mars 2 Pro/Mars 2" names no machine at all.

/// Every model the sheet names, against the printers we ship for it.
///
/// A model we ship no profile for maps to nothing, which is what makes
/// its rows reportable rather than silently keyed to an absent id.
const MODELS: [(&str, &[&str]); 21] = [
    ("ELEGOO Jupiter", &["elegoo-jupiter"]),
    ("ELEGOO Jupiter SE", &["elegoo-jupiter-se"]),
    ("ELEGOO Mars", &[]),
    ("ELEGOO Mars 2 Pro/Mars 2", &["elegoo-mars-2-pro"]),
    ("ELEGOO Mars 2 Pro/Mars2", &["elegoo-mars-2-pro"]),
    (
        "ELEGOO Mars 3/Mars 3 Pro",
        &["elegoo-mars-3", "elegoo-mars-3-pro"],
    ),
    ("ELEGOO Mars 4", &["elegoo-mars-4"]),
    ("ELEGOO Mars 4 DLP", &["elegoo-mars-4-dlp"]),
    ("ELEGOO Mars 4 MAX", &["elegoo-mars-4-max"]),
    ("ELEGOO Mars 4 Ultra", &["elegoo-mars-4-ultra"]),
    ("ELEGOO Mars 5", &["elegoo-mars-5"]),
    ("ELEGOO Mars 5 Ultra", &["elegoo-mars-5-ultra"]),
    ("ELEGOO Mars Pro", &[]),
    ("ELEGOO Saturn 2", &["elegoo-saturn-2"]),
    ("ELEGOO Saturn 3", &["elegoo-saturn-3"]),
    ("ELEGOO Saturn 3 Ultra", &["elegoo-saturn-3-ultra"]),
    ("ELEGOO Saturn 4", &["elegoo-saturn-4"]),
    ("ELEGOO Saturn 4 Ultra", &["elegoo-saturn-4-ultra"]),
    ("ELEGOO Saturn 4 Ultra 16K", &["elegoo-saturn-4-ultra-16k"]),
    ("ELEGOO Saturn 8K", &[]),
    ("ELEGOO Saturn/Saturn S", &[]),
];

/// What a model cell names: the machines it covers, and whether the
/// sheet names that model at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machines {
    /// The printers we ship for this model, never empty.
    Ours(&'static [&'static str]),
    /// A model the sheet names and we ship no profile for.
    NotOurs,
    /// A model this converter has never seen, so the sheet has changed.
    Unknown,
}

/// Which printers of ours a model cell covers.
pub fn machines_named_by(model: &str) -> Machines {
    let model = model.trim();
    match MODELS.iter().find(|(name, _)| *name == model) {
        Some((_, [])) => Machines::NotOurs,
        Some((_, ids)) => Machines::Ours(ids),
        None => Machines::Unknown,
    }
}

#[cfg(test)]
mod machines_tests {
    use super::*;

    #[test]
    fn a_model_naming_one_machine_maps_to_it() {
        assert_eq!(
            machines_named_by("ELEGOO Mars 5"),
            Machines::Ours(&["elegoo-mars-5"]),
        );
    }

    #[test]
    fn the_model_naming_2_machines_maps_to_both() {
        assert_eq!(
            machines_named_by("ELEGOO Mars 3/Mars 3 Pro"),
            Machines::Ours(&["elegoo-mars-3", "elegoo-mars-3-pro"]),
        );
    }

    #[test]
    fn both_spellings_of_the_mars_2_pro_reach_the_same_machine() {
        assert_eq!(
            machines_named_by("ELEGOO Mars 2 Pro/Mars 2"),
            machines_named_by("ELEGOO Mars 2 Pro/Mars2"),
        );
    }

    #[test]
    fn a_model_we_ship_no_profile_for_is_told_apart_from_one_we_do_not_know() {
        assert_eq!(machines_named_by("ELEGOO Saturn 8K"), Machines::NotOurs);
        assert_eq!(machines_named_by("ELEGOO Mars 9"), Machines::Unknown);
    }

    #[test]
    fn surrounding_space_does_not_hide_a_model() {
        assert_eq!(
            machines_named_by("  ELEGOO Mars 4 Ultra  "),
            Machines::Ours(&["elegoo-mars-4-ultra"]),
        );
    }

    #[test]
    fn every_machine_named_here_is_a_printer_we_ship() {
        // Guards the map against a printer profile being renamed: a
        // block keyed to an id no printer has cannot resolve.
        let shipped: Vec<String> = std::fs::read_dir(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("assets")
                .join("profiles")
                .join("printers"),
        )
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();

        for (model, ids) in MODELS {
            for id in ids {
                assert!(
                    shipped.iter().any(|name| name == id),
                    "{model} maps to {id}, which we do not ship",
                );
            }
        }
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a reader wants the operator to know about a run.
//!
//! A note is neither a failure nor a written file: it is a fact about
//! what the source said and what the run could do with it, which a
//! person reviewing the converted profiles needs in order to trust
//! them.

use std::fmt;

use sc_print_primitives::{MaterialId, PrinterId};

/// One thing a reader observed while converting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceNote {
    /// No printer library was given, so nothing said which machines
    /// split their travel and no calibration block was split.
    NoPrinterLibrary,
    /// No material curation was given, so nothing filled the fields the
    /// sources do not publish.
    NoMaterialCuration,
    /// The library holds no profile for a machine the source describes,
    /// so what that machine can do is unknown.
    MachineOutsideTheLibrary { printer: PrinterId },
    /// The source and our own profile disagree on whether a machine
    /// splits its travel into 2 stages. Ours is what the conversion
    /// followed.
    TravelSplitDisagrees {
        printer: PrinterId,
        source_splits: bool,
    },
    /// The source describes a machine whose vat tilts, which no profile
    /// of ours can represent.
    TiltingVat { printer: PrinterId },
    /// The source classifies how a machine separates a layer in a way
    /// this converter does not recognise.
    UnrecognisedClassification {
        printer: PrinterId,
        classification: String,
    },
    /// The source states how many bits a machine's panel drives, which
    /// no profile of ours records.
    PanelBitDepth { printer: PrinterId, bits: u32 },
    /// A resin's records spell its name 2 ways. They are one resin, so
    /// one spelling reaches the profile.
    NameSpelledDifferently {
        material: MaterialId,
        kept: String,
        passed_over: String,
    },
    /// The source glues printing advice onto a resin's name for some
    /// machines. It names no resin, and no field of ours records it.
    AdviceInTheName {
        material: MaterialId,
        advice: String,
    },
    /// A resin's records give one machine a different value from
    /// another for a field a profile carries once, so the profile
    /// carries none of them.
    DefaultVariesByMachine {
        material: MaterialId,
        field: &'static str,
    },
    /// Our own curation and the source both state a field, and they
    /// disagree. Ours is what the profile carries, because the source
    /// curates as we do while our figure comes from the manufacturer.
    CurationOverridesTheSource {
        material: MaterialId,
        field: &'static str,
        kept: String,
        source_states: String,
        sheet: String,
    },
    /// Our curation says nothing about a resin the run wrote, so the
    /// profile carries only what the source states.
    NoCurationForMaterial { material: MaterialId },
    /// Our curation rejects a resin the source still publishes, so the
    /// run wrote no profile for it.
    MaterialDiscarded {
        material: MaterialId,
        status: &'static str,
    },
    /// Our curation files a resin the source named another way under
    /// the id it is curated as, so its machines joined that profile.
    MaterialFolded {
        material: MaterialId,
        into: MaterialId,
    },
    /// The source calibrates a machine we ship no profile for.
    RowsForAMachineWeDoNotShip { model: String, rows: usize },
    /// The source names a machine this converter has never seen, so
    /// what it publishes has changed shape.
    RowsForAnUnknownModel { model: String, rows: usize },
    /// The source states settings without saying which resin they are
    /// for.
    RowsNamingNoResin { rows: usize },
    /// The source states no travel for a machine, which every
    /// calibration block needs, so that machine gets none.
    RowsStatingNoTravel { model: String, rows: usize },
    /// The source states a value this converter cannot read.
    RowsWeCannotRead { model: String, rows: usize },
    /// The source states one machine's calibration for one resin twice,
    /// with different values, so the profile carries neither.
    RowsDisagree {
        material: MaterialId,
        printer: PrinterId,
    },
}

impl fmt::Display for SourceNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPrinterLibrary => f.write_str(
                "no printer library was given, so no calibration block carries a second travel stage",
            ),
            Self::NoMaterialCuration => f.write_str(
                "no material curation was given, so no profile carries a figure its source does not publish",
            ),
            Self::MachineOutsideTheLibrary { printer } => write!(
                f,
                "the printer library has no profile for {:?}, so its calibration blocks carry no second travel stage",
                printer.as_str(),
            ),
            Self::TravelSplitDisagrees {
                printer,
                source_splits: true,
            } => write!(
                f,
                "the source splits the travel of {:?} into 2 stages, our own profile does not",
                printer.as_str(),
            ),
            Self::TravelSplitDisagrees {
                printer,
                source_splits: false,
            } => write!(
                f,
                "the source moves the platform of {:?} in one go, our own profile splits its travel",
                printer.as_str(),
            ),
            Self::TiltingVat { printer } => write!(
                f,
                "the source describes {:?} as a machine whose vat tilts, which our profiles cannot represent: what was written for it is a platform that lifts",
                printer.as_str(),
            ),
            Self::UnrecognisedClassification {
                printer,
                classification,
            } => write!(
                f,
                "the source classifies {:?} as {classification:?}, which this converter does not recognise",
                printer.as_str(),
            ),
            Self::PanelBitDepth { printer, bits } => write!(
                f,
                "the source states a {bits}-bit panel for {:?}, which our profiles do not record: their grayscale support comes from the anti-aliasing flag alone",
                printer.as_str(),
            ),
            Self::NameSpelledDifferently {
                material,
                kept,
                passed_over,
            } => write!(
                f,
                "the source spells {:?} both {kept:?} and {passed_over:?}; the profile carries the spelling most of its records use",
                material.as_str(),
            ),
            Self::AdviceInTheName { material, advice } => write!(
                f,
                "the source's name for {:?} carries the advice {advice:?}, which no profile of ours records",
                material.as_str(),
            ),
            Self::DefaultVariesByMachine { material, field } => write!(
                f,
                "the source gives {:?} a different {field} per machine, so its defaults carry none and every calibration block keeps its own",
                material.as_str(),
            ),
            Self::CurationOverridesTheSource {
                material,
                field,
                kept,
                source_states,
                sheet,
            } => write!(
                f,
                "the source states a {field} of {source_states} for {:?}; the profile carries our own {kept}, read from {sheet}",
                material.as_str(),
            ),
            Self::NoCurationForMaterial { material } => write!(
                f,
                "our curation says nothing about {:?}, so it carries only what its source states",
                material.as_str(),
            ),
            Self::MaterialDiscarded { material, status } => write!(
                f,
                "our curation discards {:?} ({status}), so no profile was written for it",
                material.as_str(),
            ),
            Self::MaterialFolded { material, into } => write!(
                f,
                "our curation files {:?} under {:?}, so what it calibrates was written into that profile",
                material.as_str(),
                into.as_str(),
            ),
            Self::RowsForAMachineWeDoNotShip { model, rows } => write!(
                f,
                "{rows} row(s) calibrate {model:?}, a machine we ship no printer profile for",
            ),
            Self::RowsForAnUnknownModel { model, rows } => write!(
                f,
                "{rows} row(s) name {model:?}, a machine this converter does not know: the source has changed shape",
            ),
            Self::RowsNamingNoResin { rows } => write!(
                f,
                "{rows} row(s) state settings without naming the resin they are for",
            ),
            Self::RowsStatingNoTravel { model, rows } => write!(
                f,
                "{rows} row(s) state no travel for {model:?}, which every calibration block needs, so it has none",
            ),
            Self::RowsWeCannotRead { model, rows } => write!(
                f,
                "{rows} row(s) for {model:?} state a value this converter cannot read",
            ),
            Self::RowsDisagree { material, printer } => write!(
                f,
                "the source calibrates {:?} for {:?} twice with different values, so the profile carries neither",
                material.as_str(),
                printer.as_str(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_names_the_machine_it_is_about() {
        let note = SourceNote::MachineOutsideTheLibrary {
            printer: PrinterId::new("elegoo-mars-5").unwrap(),
        };
        let message = note.to_string();
        assert!(message.contains("elegoo-mars-5"), "{message}");
        assert!(message.contains("second travel stage"), "{message}");
    }

    #[test]
    fn the_missing_library_note_says_what_it_cost() {
        let message = SourceNote::NoPrinterLibrary.to_string();
        assert!(message.contains("printer library"), "{message}");
        assert!(message.contains("second travel stage"), "{message}");
    }

    #[test]
    fn a_disagreement_says_which_side_splits_the_travel() {
        let disagreement = |source_splits| {
            SourceNote::TravelSplitDisagrees {
                printer: PrinterId::new("elegoo-mars-3").unwrap(),
                source_splits,
            }
            .to_string()
        };

        let theirs = disagreement(true);
        assert!(theirs.contains("the source splits"), "{theirs}");
        assert!(theirs.contains("elegoo-mars-3"), "{theirs}");

        let ours = disagreement(false);
        assert!(ours.contains("our own profile splits"), "{ours}");
    }

    #[test]
    fn the_tilting_note_says_what_was_written_instead() {
        let message = SourceNote::TiltingVat {
            printer: PrinterId::new("elegoo-saturn-4-ultra").unwrap(),
        }
        .to_string();
        assert!(message.contains("elegoo-saturn-4-ultra"), "{message}");
        assert!(message.contains("vat tilts"), "{message}");
        assert!(message.contains("platform that lifts"), "{message}");
    }

    #[test]
    fn an_unrecognised_classification_quotes_the_word_the_source_used() {
        let message = SourceNote::UnrecognisedClassification {
            printer: PrinterId::new("elegoo-mars-5").unwrap(),
            classification: "peristaltic".to_owned(),
        }
        .to_string();
        assert!(message.contains("\"peristaltic\""), "{message}");
    }

    #[test]
    fn a_second_spelling_note_shows_both_spellings() {
        let message = SourceNote::NameSpelledDifferently {
            material: MaterialId::new("sirayatech-blu-clear-v2-emerald-blue").unwrap(),
            kept: "Blu (Clear V2 / Emerald Blue)".to_owned(),
            passed_over: "Blu (Clear V2/Emerald Blue)".to_owned(),
        }
        .to_string();
        assert!(
            message.contains("Blu (Clear V2 / Emerald Blue)"),
            "{message}"
        );
        assert!(message.contains("Blu (Clear V2/Emerald Blue)"), "{message}");
    }

    #[test]
    fn an_advice_note_carries_the_advice_itself() {
        let message = SourceNote::AdviceInTheName {
            material: MaterialId::new("sirayatech-blu-nylon-black").unwrap(),
            advice: "Use Extra Heavy support".to_owned(),
        }
        .to_string();
        assert!(message.contains("sirayatech-blu-nylon-black"), "{message}");
        assert!(message.contains("Use Extra Heavy support"), "{message}");
    }

    #[test]
    fn a_varying_default_says_where_the_value_still_lives() {
        let message = SourceNote::DefaultVariesByMachine {
            material: MaterialId::new("sirayatech-rebound-55a-black").unwrap(),
            field: "bottom_layer_count",
        }
        .to_string();
        assert!(
            message.contains("sirayatech-rebound-55a-black"),
            "{message}"
        );
        assert!(message.contains("bottom_layer_count"), "{message}");
        assert!(message.contains("calibration block"), "{message}");
    }

    #[test]
    fn a_discarded_resin_note_says_why_it_was_dropped() {
        let message = SourceNote::MaterialDiscarded {
            material: MaterialId::new("sirayatech-easy-grey").unwrap(),
            status: "discontinued",
        }
        .to_string();
        assert!(message.contains("sirayatech-easy-grey"), "{message}");
        assert!(message.contains("discontinued"), "{message}");
        assert!(message.contains("no profile was written"), "{message}");
    }

    #[test]
    fn a_folded_resin_note_names_both_ids() {
        let message = SourceNote::MaterialFolded {
            material: MaterialId::new("siraya-tech-simple-grey-clear-smoky-black").unwrap(),
            into: MaterialId::new("siraya-tech-simple-clear-grey-smoky-black").unwrap(),
        }
        .to_string();
        assert!(
            message.contains("\"siraya-tech-simple-grey-clear-smoky-black\" under \"siraya-tech-simple-clear-grey-smoky-black\""),
            "{message}"
        );
    }

    #[test]
    fn the_bit_depth_note_says_where_our_grayscale_flag_comes_from() {
        let message = SourceNote::PanelBitDepth {
            printer: PrinterId::new("elegoo-jupiter-2").unwrap(),
            bits: 3,
        }
        .to_string();
        assert!(message.contains("3-bit panel"), "{message}");
        assert!(message.contains("anti-aliasing flag"), "{message}");
    }

    #[test]
    fn a_machine_we_do_not_ship_is_named_with_its_row_count() {
        let message = SourceNote::RowsForAMachineWeDoNotShip {
            model: "ELEGOO Saturn 8K".to_owned(),
            rows: 71,
        }
        .to_string();
        assert!(message.contains("71 row(s)"), "{message}");
        assert!(message.contains("ELEGOO Saturn 8K"), "{message}");
    }

    #[test]
    fn an_unknown_model_note_says_the_source_has_changed_shape() {
        let message = SourceNote::RowsForAnUnknownModel {
            model: "ELEGOO Mars 9".to_owned(),
            rows: 4,
        }
        .to_string();
        assert!(message.contains("ELEGOO Mars 9"), "{message}");
        assert!(message.contains("changed shape"), "{message}");
    }

    #[test]
    fn rows_naming_no_resin_are_counted() {
        let message = SourceNote::RowsNamingNoResin { rows: 3 }.to_string();
        assert!(message.contains("3 row(s)"), "{message}");
        assert!(message.contains("without naming the resin"), "{message}");
    }

    #[test]
    fn a_machine_stating_no_travel_is_told_it_gets_no_block() {
        let message = SourceNote::RowsStatingNoTravel {
            model: "ELEGOO Mars 5 Ultra".to_owned(),
            rows: 107,
        }
        .to_string();
        assert!(message.contains("ELEGOO Mars 5 Ultra"), "{message}");
        assert!(message.contains("calibration block"), "{message}");
    }

    #[test]
    fn an_unreadable_value_names_the_machine_whose_rows_hold_it() {
        let message = SourceNote::RowsWeCannotRead {
            model: "ELEGOO Mars 4".to_owned(),
            rows: 1,
        }
        .to_string();
        assert!(message.contains("ELEGOO Mars 4"), "{message}");
        assert!(message.contains("cannot read"), "{message}");
    }

    #[test]
    fn a_conflict_names_both_the_resin_and_the_machine_left_uncalibrated() {
        let message = SourceNote::RowsDisagree {
            material: MaterialId::new("elegoo-standard-resin-translucent-50um").unwrap(),
            printer: PrinterId::new("elegoo-saturn-3").unwrap(),
        }
        .to_string();
        assert!(
            message.contains("elegoo-standard-resin-translucent-50um"),
            "{message}"
        );
        assert!(message.contains("elegoo-saturn-3"), "{message}");
        assert!(message.contains("carries neither"), "{message}");
    }
}

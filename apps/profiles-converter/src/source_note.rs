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

use sc_print_primitives::PrinterId;

/// One thing a reader observed while converting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceNote {
    /// No printer library was given, so nothing said which machines
    /// split their travel and no calibration block was split.
    NoPrinterLibrary,
    /// The library holds no profile for a machine the source describes,
    /// so what that machine can do is unknown.
    MachineOutsideTheLibrary { printer: PrinterId },
}

impl fmt::Display for SourceNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPrinterLibrary => f.write_str(
                "no printer library was given, so no calibration block carries a second travel stage",
            ),
            Self::MachineOutsideTheLibrary { printer } => write!(
                f,
                "the printer library has no profile for {:?}, so its calibration blocks carry no second travel stage",
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
}

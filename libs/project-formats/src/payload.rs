/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Container-neutral project payload types.
//!
//! [`ProjectPayload`] is what a project file carries besides the
//! scene: the printer/resin profile references, plus any embedded
//! profile documents. Embedded documents are opaque bytes here; this
//! crate never parses profile content, it only transports it (the
//! same way profile packages carry raw on-disk TOML bytes).

use sc_print_primitives::{MaterialId, OutputFormatBinding, PositiveFiniteMm, PrinterId};

use crate::messages::ProfileRefWarning;

/// Project-level data stored next to the scene in a project file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectPayload {
    /// The printer profile the project was saved with, if any.
    pub printer: Option<PrinterId>,
    /// The resin (material) profile the project was saved with, if
    /// any.
    pub resin: Option<MaterialId>,
    /// The per-print first-layer height chosen for the project, if
    /// set. `None` means "use the resolved material's layer height".
    pub first_layer_height: Option<PositiveFiniteMm>,
    /// The printer file the project is exported as, spelled the way
    /// the printer profile declares it rather than as a writer
    /// identity, so a project outlives the set of writers a build
    /// happens to carry. `None` until a format is chosen.
    pub output_format: Option<OutputFormatBinding>,
    /// Embedded profile documents carried by the file, in file
    /// order. Held opaquely: re-saving a project writes these bytes
    /// back unchanged.
    pub embedded: Vec<EmbeddedProfile>,
    /// What the person decided about the review's findings (#432): the
    /// list is made again on every slice, and these are what stays.
    pub review_decisions: Vec<ReviewDecision>,
    /// How many build plates the scene lays out (#545). 0 and 1 both mean
    /// one: a project saved before plates came records none.
    pub plates: u32,
}

/// A finding of the review the person chose to ignore, by kind and place, so
/// the next list that finds it again keeps it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDecision {
    /// The finding's kind, by its stable machine name.
    pub check: String,
    /// The first layer it shows in, counted from 0.
    pub first_layer: u64,
    /// The last layer it shows in.
    pub last_layer: u64,
    /// Where it is, in plate micrometres.
    pub focus_um: [i64; 3],
}

impl ProjectPayload {
    /// Whether the payload embeds a printer profile document with
    /// this id.
    pub fn embeds_printer(&self, id: &PrinterId) -> bool {
        self.embedded
            .iter()
            .any(|p| matches!(p, EmbeddedProfile::Printer { id: embedded, .. } if embedded == id))
    }

    /// Whether the payload embeds a material profile document with
    /// this id (full profiles only; extensions do not count).
    pub fn embeds_material(&self, id: &MaterialId) -> bool {
        self.embedded
            .iter()
            .any(|p| matches!(p, EmbeddedProfile::Material { id: embedded, .. } if embedded == id))
    }

    /// Classifies the profile references against what is installed
    /// locally. `printer_installed` / `material_installed` answer
    /// whether a profile with the given id can be resolved outside
    /// this file (user library or bundled assets); the caller owns
    /// that lookup so this stays a pure computation.
    ///
    /// A reference that resolves locally produces nothing. One that
    /// does not resolve produces the embedded-only variant when the
    /// file carries a copy of the document, and the missing variant
    /// when it does not.
    pub fn profile_ref_warnings<P, M>(
        &self,
        printer_installed: P,
        material_installed: M,
    ) -> Vec<ProfileRefWarning>
    where
        P: Fn(&PrinterId) -> bool,
        M: Fn(&MaterialId) -> bool,
    {
        let mut warnings = Vec::new();
        if let Some(printer) = &self.printer
            && !printer_installed(printer)
        {
            warnings.push(if self.embeds_printer(printer) {
                ProfileRefWarning::PrinterEmbeddedOnly {
                    id: printer.clone(),
                }
            } else {
                ProfileRefWarning::PrinterMissing {
                    id: printer.clone(),
                }
            });
        }
        if let Some(resin) = &self.resin
            && !material_installed(resin)
        {
            warnings.push(if self.embeds_material(resin) {
                ProfileRefWarning::ResinEmbeddedOnly { id: resin.clone() }
            } else {
                ProfileRefWarning::ResinMissing { id: resin.clone() }
            });
        }
        warnings
    }
}

/// One embedded profile document: its kind, its id, and the raw TOML
/// bytes exactly as they sat on disk when the project was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbeddedProfile {
    /// A printer profile document.
    Printer { id: PrinterId, bytes: Vec<u8> },
    /// A material (resin) profile document.
    Material { id: MaterialId, bytes: Vec<u8> },
    /// A per-printer material extension document, named after the
    /// material it extends.
    MaterialExtension { id: MaterialId, bytes: Vec<u8> },
}

/// Whether [`crate::write_project`] writes the payload's embedded
/// profile documents into the file or only the references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedProfiles {
    Yes,
    No,
}

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn project_payloads_are_eq() {
        // Sound since PositiveFiniteMm became Eq (NaN and both zeroes
        // are unrepresentable); guards against the derive being
        // dropped.
        fn assert_eq_bound<T: Eq>() {}
        assert_eq_bound::<ProjectPayload>();
    }
}

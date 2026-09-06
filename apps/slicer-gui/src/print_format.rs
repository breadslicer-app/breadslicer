/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Deriving the Slice panel's print-format selector from the printer's
//! declared output formats.
//!
//! A printer profile states which files its machine takes; this build
//! writes only some of them. The dropdown offers the ones it can write
//! and the note beneath it says why each of the rest is missing, so a
//! printer with nothing writable explains itself instead of presenting
//! an empty control.

use sc_print_primitives::{OutputFormatBinding, OutputFormatBindings};
use sc_sla_formats::{CtbVariant, GooVersion, PrinterFileFormat};

use crate::i18n::ctr;

/// One offered print format: the binding it came from, the format
/// that binding names, and the label the dropdown shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrintFormatOption {
    pub binding: OutputFormatBinding,
    pub format: PrinterFileFormat,
    pub label: String,
}

/// The selector's rendered state: the writable options in the order
/// the profile declares them, the index of the selected one (`-1` when
/// there is none), and one line per declared format this build cannot
/// write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrintFormatModel {
    pub options: Vec<PrintFormatOption>,
    pub selected: i32,
    pub refusals: Vec<String>,
}

impl PrintFormatModel {
    /// The state to show while no printer is selected: nothing on
    /// offer, and nothing to explain either.
    pub(crate) fn nothing_declared() -> Self {
        Self {
            options: Vec::new(),
            selected: -1,
            refusals: Vec::new(),
        }
    }
}

/// Builds the selector from a printer's declared output formats.
///
/// `remembered` is the choice to restore. Anything it cannot name
/// (a binding the printer no longer declares, one this build cannot
/// write, or no choice at all) falls back to the printer's first
/// writable binding, so the panel always offers a usable format when
/// one exists.
pub(crate) fn print_format_model(
    declared: &OutputFormatBindings,
    remembered: Option<&OutputFormatBinding>,
) -> PrintFormatModel {
    let mut options = Vec::new();
    let mut refusals = Vec::new();

    for binding in declared.iter() {
        match PrinterFileFormat::from_binding(binding) {
            Ok(format) if format.has_writer() => options.push(PrintFormatOption {
                binding: binding.clone(),
                format,
                label: format_label(format),
            }),
            Ok(_) => refusals.push(refusal(
                binding,
                &ctr!("SlicePanel", "this build cannot write it yet"),
            )),
            Err(unresolved) => refusals.push(refusal(binding, &unresolved.to_string())),
        }
    }

    let remembered_index =
        remembered.and_then(|wanted| options.iter().position(|option| &option.binding == wanted));
    let selected = match remembered_index {
        Some(index) => index as i32,
        None if options.is_empty() => -1,
        None => 0,
    };

    PrintFormatModel {
        options,
        selected,
        refusals,
    }
}

/// The label the dropdown shows: the extension the file is written
/// with, plus the revision, because revisions of one family share an
/// extension and produce different bytes.
fn format_label(format: PrinterFileFormat) -> String {
    match format {
        PrinterFileFormat::Goo(GooVersion::V3_0) => ctr!("SlicePanel", ".goo (V3.0)"),
        PrinterFileFormat::Goo(GooVersion::V5_1) => ctr!("SlicePanel", ".goo (V5.1)"),
        PrinterFileFormat::Ctb(CtbVariant::PlainV5) => ctr!("SlicePanel", ".ctb (v5)"),
        PrinterFileFormat::Ctb(CtbVariant::EncryptedV5) => {
            ctr!("SlicePanel", ".ctb (v5 encrypted)")
        }
    }
    .to_string()
}

/// One line of the panel's note: the binding as the profile spells it,
/// then why it is not in the dropdown.
fn refusal(binding: &OutputFormatBinding, reason: &str) -> String {
    format!("{}: {reason}", binding_text(binding))
}

/// A binding as a printer profile writes it, with the version when it
/// carries one.
fn binding_text(binding: &OutputFormatBinding) -> String {
    match &binding.version {
        Some(version) => format!("{}@{}", binding.format.as_canonical(), version.as_str()),
        None => binding.format.as_canonical().to_owned(),
    }
}

#[cfg(test)]
mod print_format_tests {
    use sc_print_primitives::{FormatKind, NonEmptyString};

    use super::*;

    fn binding(format: FormatKind, version: Option<&str>) -> OutputFormatBinding {
        OutputFormatBinding {
            format,
            version: version.map(|v| NonEmptyString::new(v).expect("test version is non-empty")),
        }
    }

    fn goo_v3_0() -> OutputFormatBinding {
        binding(FormatKind::Goo, Some("v3.0"))
    }

    fn declared(bindings: &[OutputFormatBinding]) -> OutputFormatBindings {
        OutputFormatBindings::try_from_vec(bindings.to_vec()).expect("test list is non-empty")
    }

    #[test]
    fn a_writable_binding_becomes_the_selected_option() {
        let model = print_format_model(&declared(&[goo_v3_0()]), None);

        assert_eq!(model.options.len(), 1);
        assert_eq!(model.options[0].binding, goo_v3_0());
        assert_eq!(
            model.options[0].format,
            PrinterFileFormat::Goo(GooVersion::V3_0),
        );
        assert_eq!(model.selected, 0);
        assert!(model.refusals.is_empty());
    }

    #[test]
    fn a_declared_format_without_a_writer_is_named_in_the_note_not_the_dropdown() {
        let declared = declared(&[binding(FormatKind::Ctb, Some("v5enc")), goo_v3_0()]);
        let model = print_format_model(&declared, None);

        assert_eq!(model.options.len(), 1);
        assert_eq!(model.options[0].binding, goo_v3_0());
        assert_eq!(
            model.refusals,
            vec!["ctb@v5enc: this build cannot write it yet"],
        );
    }

    #[test]
    fn a_binding_naming_no_known_format_is_refused_with_the_reason_it_carries() {
        let model = print_format_model(&declared(&[binding(FormatKind::Sl1, Some("v1"))]), None);

        assert!(model.options.is_empty());
        assert_eq!(model.selected, -1);
        assert_eq!(model.refusals.len(), 1);
        assert!(model.refusals[0].starts_with("sl1@v1: "), "{model:?}");
        assert!(model.refusals[0].contains("unknown format"), "{model:?}");
    }

    #[test]
    fn a_version_less_binding_is_refused_and_reads_without_an_at_sign() {
        let model = print_format_model(&declared(&[binding(FormatKind::Goo, None)]), None);

        assert!(model.options.is_empty());
        assert!(model.refusals[0].starts_with("goo: "), "{model:?}");
    }

    #[test]
    fn a_printer_with_nothing_writable_offers_nothing_and_explains_every_binding() {
        let declared = declared(&[
            binding(FormatKind::Unknown("aff".to_owned()), Some("pws")),
            binding(FormatKind::Goo, Some("v5.1")),
        ]);
        let model = print_format_model(&declared, None);

        assert!(model.options.is_empty());
        assert_eq!(model.selected, -1);
        assert_eq!(model.refusals.len(), 2);
    }

    #[test]
    fn the_remembered_binding_selects_its_own_option() {
        // Two bindings that differ only in how they spell the version,
        // which is the one way a profile can declare two writable
        // formats while GOO V3.0 is the only one with a writer.
        let upper = binding(FormatKind::Goo, Some("V3.0"));
        let declared = declared(&[goo_v3_0(), upper.clone()]);
        let model = print_format_model(&declared, Some(&upper));

        assert_eq!(model.options.len(), 2);
        assert_eq!(model.selected, 1);
    }

    #[test]
    fn a_remembered_binding_without_a_writer_falls_back_to_the_first_writable() {
        let ctb = binding(FormatKind::Ctb, Some("v5"));
        let declared = declared(&[goo_v3_0(), ctb.clone()]);
        let model = print_format_model(&declared, Some(&ctb));

        assert_eq!(model.options.len(), 1);
        assert_eq!(model.selected, 0);
        assert_eq!(model.options[0].binding, goo_v3_0());
    }

    #[test]
    fn a_remembered_binding_the_printer_no_longer_declares_falls_back_to_the_first_writable() {
        let stale = binding(FormatKind::Goo, Some("v5.1"));
        let model = print_format_model(&declared(&[goo_v3_0()]), Some(&stale));

        assert_eq!(model.selected, 0);
        assert_eq!(model.options[0].binding, goo_v3_0());
    }

    #[test]
    fn the_dropdown_keeps_the_order_the_profile_declares() {
        let declared = declared(&[
            binding(FormatKind::Ctb, Some("v5enc")),
            goo_v3_0(),
            binding(FormatKind::Unknown("azf".to_owned()), Some("pm7")),
        ]);
        let model = print_format_model(&declared, None);

        assert_eq!(model.options.len(), 1);
        assert_eq!(
            model.refusals,
            vec![
                "ctb@v5enc: this build cannot write it yet".to_owned(),
                "azf@pm7: unknown format \"azf\"".to_owned(),
            ],
        );
    }

    #[test]
    fn every_known_revision_has_its_own_label() {
        assert_eq!(
            format_label(PrinterFileFormat::Goo(GooVersion::V3_0)),
            ".goo (V3.0)",
        );
        assert_eq!(
            format_label(PrinterFileFormat::Goo(GooVersion::V5_1)),
            ".goo (V5.1)",
        );
        assert_eq!(
            format_label(PrinterFileFormat::Ctb(CtbVariant::PlainV5)),
            ".ctb (v5)",
        );
        assert_eq!(
            format_label(PrinterFileFormat::Ctb(CtbVariant::EncryptedV5)),
            ".ctb (v5 encrypted)",
        );
    }
}

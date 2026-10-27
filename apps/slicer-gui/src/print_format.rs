/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Deriving the Slice panel's print-format selector from the printer's declared
//! output formats.
//!
//! A printer profile lists output formats for its machine; this build writes
//! only some of them. The dropdown offers the ones it can write, and the note
//! beneath it says why each of the rest is missing. A family with 2 or more
//! writable revisions also gets an automatic entry, which writes the highest of
//! them.

use sc_print_primitives::{FormatKind, OutputFormatBinding, OutputFormatBindings};
use sc_sla_formats::{
    AnycubicZipVariant, CtbVariant, CxdlpVersion, GooVersion, PrinterFileFormat, UnwritableBinding,
};

use crate::i18n::ctr;

/// An offered print format: the binding a pick of it saves, the format it
/// writes, and the label the dropdown shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrintFormatOption {
    pub binding: OutputFormatBinding,
    pub format: PrinterFileFormat,
    pub label: String,
}

/// The selector's rendered state: the writable options grouped by family, each
/// family's automatic entry first; the index of the selected one (`-1` when
/// there is none); and 1 line per declared format this build cannot write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrintFormatModel {
    pub options: Vec<PrintFormatOption>,
    pub selected: i32,
    pub refusals: Vec<String>,
}

impl PrintFormatModel {
    /// Returns the state to show while no printer is selected: nothing on
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
/// `remembered` is the choice to restore. When no option has it, a printer that
/// offers 1 family gets that family's first option selected, and a printer that
/// offers 2 families or more gets none, so the user picks.
pub(crate) fn print_format_model(
    declared: &OutputFormatBindings,
    remembered: Option<&OutputFormatBinding>,
) -> PrintFormatModel {
    let mut writable = Vec::new();
    let mut refusals = Vec::new();

    for binding in declared.iter() {
        match PrinterFileFormat::writable_from_binding(binding) {
            Ok(format) => writable.push(PrintFormatOption {
                binding: binding.clone(),
                format,
                label: format_label(format),
            }),
            Err(UnwritableBinding::NoWriter(_)) => refusals.push(refusal(
                binding,
                &ctr!("SlicePanel", "this build cannot write it yet"),
            )),
            Err(UnwritableBinding::Unresolved(unresolved)) => {
                refusals.push(refusal(binding, &unresolved.to_string()));
            }
        }
    }

    let options = grouped_by_family(writable, declared);
    let selected = remembered
        .and_then(|wanted| options.iter().position(|option| &option.binding == wanted))
        .or_else(|| default_selection(&options))
        .map_or(-1, |index| index as i32);

    PrintFormatModel {
        options,
        selected,
        refusals,
    }
}

/// Groups `writable` by family, in the order the families first appear, and
/// puts an automatic entry first in each family whose options name 2 or more
/// revisions. The automatic entry saves the family without a version and writes
/// the revision [`PrinterFileFormat::highest_writable`] picks.
fn grouped_by_family(
    writable: Vec<PrintFormatOption>,
    declared: &OutputFormatBindings,
) -> Vec<PrintFormatOption> {
    let mut families: Vec<FormatKind> = Vec::new();
    for option in &writable {
        if !families.contains(&option.binding.format) {
            families.push(option.binding.format.clone());
        }
    }

    let mut options = Vec::with_capacity(writable.len() + families.len());
    for family in families {
        let entries: Vec<&PrintFormatOption> = writable
            .iter()
            .filter(|option| option.binding.format == family)
            .collect();
        let revisions_differ = entries
            .windows(2)
            .any(|pair| pair[0].format != pair[1].format);
        if revisions_differ
            && let Ok(format) = PrinterFileFormat::highest_writable(&family, declared)
        {
            options.push(PrintFormatOption {
                binding: OutputFormatBinding {
                    format: family,
                    version: None,
                },
                format,
                label: automatic_label(format),
            });
        }
        options.extend(entries.into_iter().cloned());
    }
    options
}

/// Picks the option to preselect when no option has the remembered choice: the
/// first one, when all the options belong to 1 family. That is the family's
/// automatic entry when it has one.
fn default_selection(options: &[PrintFormatOption]) -> Option<usize> {
    let first = options.first()?;
    options
        .iter()
        .all(|option| option.binding.format == first.binding.format)
        .then_some(0)
}

/// Returns the label of a family's automatic entry, from the format it writes.
fn automatic_label(format: PrinterFileFormat) -> String {
    match format {
        PrinterFileFormat::Goo(_) => ctr!("SlicePanel", ".goo (automatic)"),
        PrinterFileFormat::Ctb(_) => ctr!("SlicePanel", ".ctb (automatic)"),
        PrinterFileFormat::Cxdlp(_) => ctr!("SlicePanel", ".cxdlp (automatic)"),
        PrinterFileFormat::AnycubicZip(_) => ctr!("SlicePanel", "Anycubic (automatic)"),
    }
    .to_string()
}

/// Returns the label the dropdown shows: the extension the file is written
/// with, plus the revision, because revisions of a family share an extension
/// and produce different bytes.
fn format_label(format: PrinterFileFormat) -> String {
    match format {
        PrinterFileFormat::Goo(GooVersion::V3_0) => ctr!("SlicePanel", ".goo (V3.0)"),
        PrinterFileFormat::Goo(GooVersion::V5_1) => ctr!("SlicePanel", ".goo (V5.1)"),
        PrinterFileFormat::Ctb(CtbVariant::PlainV5) => ctr!("SlicePanel", ".ctb (v5)"),
        PrinterFileFormat::Ctb(CtbVariant::EncryptedV5) => {
            ctr!("SlicePanel", ".ctb (v5 encrypted)")
        }
        PrinterFileFormat::Cxdlp(CxdlpVersion::V2) => ctr!("SlicePanel", ".cxdlp (v2)"),
        PrinterFileFormat::Cxdlp(CxdlpVersion::V3) => ctr!("SlicePanel", ".cxdlp (v3)"),
        PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pwsz) => ctr!("SlicePanel", ".pwsz"),
        PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pm7) => ctr!("SlicePanel", ".pm7"),
        PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pm7m) => ctr!("SlicePanel", ".pm7m"),
    }
    .to_string()
}

/// Composes a line of the panel's note: the binding, then why it is not in the
/// dropdown.
fn refusal(binding: &OutputFormatBinding, reason: &str) -> String {
    format!("{binding}: {reason}")
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
        let declared = declared(&[binding(FormatKind::Goo, Some("v5.1")), goo_v3_0()]);
        let model = print_format_model(&declared, None);

        assert_eq!(model.options.len(), 1);
        assert_eq!(model.options[0].binding, goo_v3_0());
        assert_eq!(
            model.refusals,
            vec!["goo@v5.1: this build cannot write it yet"],
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
        // 2 bindings that differ only in how they spell the version, so both
        // resolve to the same writable format.
        let upper = binding(FormatKind::Goo, Some("V3.0"));
        let declared = declared(&[goo_v3_0(), upper.clone()]);
        let model = print_format_model(&declared, Some(&upper));

        assert_eq!(model.options.len(), 2);
        assert_eq!(model.selected, 1);
    }

    #[test]
    fn a_remembered_binding_without_a_writer_falls_back_to_the_first_writable() {
        let unwritable = binding(FormatKind::Goo, Some("v5.1"));
        let declared = declared(&[goo_v3_0(), unwritable.clone()]);
        let model = print_format_model(&declared, Some(&unwritable));

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
            binding(FormatKind::Goo, Some("v5.1")),
            goo_v3_0(),
            binding(FormatKind::Unknown("lumen".to_owned()), Some("v1")),
        ]);
        let model = print_format_model(&declared, None);

        assert_eq!(model.options.len(), 1);
        assert_eq!(
            model.refusals,
            vec![
                "goo@v5.1: this build cannot write it yet".to_owned(),
                "lumen@v1: unknown format \"lumen\"".to_owned(),
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
        assert_eq!(
            format_label(PrinterFileFormat::Cxdlp(CxdlpVersion::V3)),
            ".cxdlp (v3)",
        );
        assert_eq!(
            format_label(PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pwsz)),
            ".pwsz",
        );
    }

    fn selected(model: &PrintFormatModel) -> Option<&OutputFormatBinding> {
        let index = usize::try_from(model.selected).ok()?;
        Some(&model.options[index].binding)
    }

    fn bindings(model: &PrintFormatModel) -> Vec<OutputFormatBinding> {
        model
            .options
            .iter()
            .map(|option| option.binding.clone())
            .collect()
    }

    #[test]
    fn a_family_with_2_revisions_offers_its_automatic_entry_first_and_selects_it() {
        let declared = declared(&[
            binding(FormatKind::Ctb, Some("v5")),
            binding(FormatKind::Ctb, Some("v5enc")),
        ]);
        let model = print_format_model(&declared, None);

        assert_eq!(model.options.len(), 3);
        assert_eq!(model.options[0].binding, binding(FormatKind::Ctb, None));
        assert_eq!(
            model.options[0].format,
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5),
        );
        assert_eq!(model.options[0].label, ".ctb (automatic)");
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn a_printer_offering_2_families_preselects_nothing() {
        let declared = declared(&[binding(FormatKind::Ctb, Some("v5enc")), goo_v3_0()]);
        let model = print_format_model(&declared, None);

        assert_eq!(model.options.len(), 2);
        assert_eq!(model.selected, -1);
    }

    #[test]
    fn a_remembered_automatic_choice_restores_the_automatic_entry() {
        let declared = declared(&[
            binding(FormatKind::Ctb, Some("v5")),
            binding(FormatKind::Ctb, Some("v5enc")),
            goo_v3_0(),
        ]);
        let model = print_format_model(&declared, Some(&binding(FormatKind::Ctb, None)));

        assert_eq!(selected(&model), Some(&binding(FormatKind::Ctb, None)));
    }

    #[test]
    fn a_remembered_specific_choice_restores_its_own_entry() {
        let plain = binding(FormatKind::Ctb, Some("v5"));
        let declared = declared(&[
            plain.clone(),
            binding(FormatKind::Ctb, Some("v5enc")),
            goo_v3_0(),
        ]);
        let model = print_format_model(&declared, Some(&plain));

        assert_eq!(selected(&model), Some(&plain));
    }

    #[test]
    fn a_remembered_choice_no_longer_offered_selects_nothing_between_2_families() {
        let declared = declared(&[binding(FormatKind::Ctb, Some("v5enc")), goo_v3_0()]);
        let model = print_format_model(&declared, Some(&binding(FormatKind::Goo, Some("v5.1"))));

        assert_eq!(model.selected, -1);
    }

    #[test]
    fn a_remembered_automatic_choice_falls_back_to_the_only_entry_of_its_family() {
        let plain = binding(FormatKind::Ctb, Some("v5"));
        let model = print_format_model(
            &declared(std::slice::from_ref(&plain)),
            Some(&binding(FormatKind::Ctb, None)),
        );

        assert_eq!(selected(&model), Some(&plain));
    }

    #[test]
    fn a_family_s_entries_follow_its_automatic_entry() {
        let declared = declared(&[
            binding(FormatKind::Ctb, Some("v5")),
            goo_v3_0(),
            binding(FormatKind::Ctb, Some("v5enc")),
        ]);
        let model = print_format_model(&declared, None);

        assert_eq!(
            bindings(&model),
            vec![
                binding(FormatKind::Ctb, None),
                binding(FormatKind::Ctb, Some("v5")),
                binding(FormatKind::Ctb, Some("v5enc")),
                goo_v3_0(),
            ],
        );
    }

    #[test]
    fn spellings_of_the_same_revision_add_no_automatic_entry() {
        let upper = binding(FormatKind::Goo, Some("V3.0"));
        let model = print_format_model(&declared(&[goo_v3_0(), upper.clone()]), None);

        assert_eq!(bindings(&model), vec![goo_v3_0(), upper]);
    }

    #[test]
    fn the_automatic_entries_name_their_family() {
        assert_eq!(
            automatic_label(PrinterFileFormat::Goo(GooVersion::V3_0)),
            ".goo (automatic)",
        );
        assert_eq!(
            automatic_label(PrinterFileFormat::Ctb(CtbVariant::PlainV5)),
            ".ctb (automatic)",
        );
    }
}

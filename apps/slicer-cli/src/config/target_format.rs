/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Discriminator for the `slice --target-format` flag value.
//!
//! The flag spans the two output vocabularies the writer crate keeps
//! apart, and reaches one of its two entry points accordingly:
//! `Format` names the slice outputs `write_slice` produces, and
//! `PrinterFileFormat` names the files `write_printer_file` produces
//! for a machine. Someone naming a format on the command line is
//! choosing between all of them at once, so the parsed value carries
//! whichever vocabulary the word landed in.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use sc_print_primitives::{FormatKind, NonEmptyString, OutputFormatBinding, OutputFormatBindings};
use sc_sla_formats::format::format_from_path;
use sc_sla_formats::{Format, PrinterFileFormat, UnresolvedBinding, printer_file_family_from_path};

/// Every spelling the flag accepts, in help-text order. A family
/// name may also carry the revision a profile would declare, as in
/// `ctb@v5`.
pub(crate) const ACCEPTED_TARGET_FORMATS: &str =
    "svg-per-layer, svg-combined, png-per-layer, 3mf, goo, ctb";

/// Separates a family from the revision a profile spells after it.
const REVISION_SEPARATOR: char = '@';

/// A `--target-format` flag value: a slice output, a printer file, or
/// the family of one whose revision the printer still has to name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TargetFormat {
    Slices(Format),
    PrinterFile(PrinterFileFormat),
    PrinterFamily(FormatKind),
}

impl TargetFormat {
    /// Name the target format an output path's extension asks for.
    ///
    /// A printer file's extension names only its family, since one
    /// extension covers every revision of a container.
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        format_from_path(path)
            .map(Self::Slices)
            .or_else(|| printer_file_family_from_path(path).map(Self::PrinterFamily))
    }

    /// Report whether the target path is a directory to fill rather
    /// than a single file to write.
    pub(crate) fn is_directory_format(&self) -> bool {
        match self {
            Self::Slices(format) => format.is_directory_format(),
            Self::PrinterFile(_) | Self::PrinterFamily(_) => false,
        }
    }
}

impl fmt::Display for TargetFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slices(format) => format.fmt(f),
            Self::PrinterFile(format) => format.fmt(f),
            Self::PrinterFamily(kind) => f.write_str(kind.as_canonical()),
        }
    }
}

/// Resolve a family spelled with its revision, as a profile spells
/// it, into the one format it names.
fn printer_file_from_spelling(family: &str, revision: &str) -> Option<PrinterFileFormat> {
    let binding = OutputFormatBinding {
        format: FormatKind::from_canonical(family),
        version: Some(NonEmptyString::new(revision).ok()?),
    };
    PrinterFileFormat::from_binding(&binding).ok()
}

/// A target format with nothing left to resolve, which is what the
/// writers take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettledTarget {
    Slices(Format),
    PrinterFile(PrinterFileFormat),
}

/// Settle `target` against the outputs `declared` by the printer that
/// will run the file, so a bare family name takes the revision that
/// printer asks for.
///
/// A slice output and a format already spelled with its revision pass
/// through: naming a revision outright is how a caller asks for one
/// the printer does not declare.
///
/// # Errors
///
/// [`UnwritableTarget`] when the printer declares nothing of the
/// requested family, declares it at a revision this build does not
/// name, or names one whose writer has not landed.
pub(crate) fn settle_for_printer(
    target: TargetFormat,
    declared: &OutputFormatBindings,
) -> Result<SettledTarget, UnwritableTarget> {
    let format = match target {
        TargetFormat::Slices(format) => return Ok(SettledTarget::Slices(format)),
        TargetFormat::PrinterFile(format) => format,
        TargetFormat::PrinterFamily(family) => {
            let binding = declared
                .iter()
                .find(|binding| binding.format == family)
                .ok_or_else(|| UnwritableTarget::FamilyNotDeclared {
                    family: family.clone(),
                    declared: declared.iter().map(rendered).collect::<Vec<_>>().join(", "),
                })?;
            PrinterFileFormat::from_binding(binding).map_err(UnwritableTarget::Unresolved)?
        }
    };
    if format.has_writer() {
        Ok(SettledTarget::PrinterFile(format))
    } else {
        Err(UnwritableTarget::NoWriter(format))
    }
}

/// Spell a binding the way a profile declares it.
fn rendered(binding: &OutputFormatBinding) -> String {
    match &binding.version {
        Some(version) => format!(
            "{}{REVISION_SEPARATOR}{version}",
            binding.format.as_canonical(),
        ),
        None => binding.format.as_canonical().to_owned(),
    }
}

/// Returned when the printer and the requested family cannot name a
/// format this build writes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UnwritableTarget {
    FamilyNotDeclared {
        family: FormatKind,
        declared: String,
    },
    Unresolved(UnresolvedBinding),
    NoWriter(PrinterFileFormat),
}

impl fmt::Display for UnwritableTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FamilyNotDeclared { family, declared } => write!(
                f,
                "this printer declares no \"{}\" output; it declares: {declared}",
                family.as_canonical(),
            ),
            Self::Unresolved(err) => write!(
                f,
                "this printer declares an output this build cannot name: {err}"
            ),
            Self::NoWriter(format) => {
                write!(f, "this build has no writer for \"{format}\" yet")
            }
        }
    }
}

impl std::error::Error for UnwritableTarget {}

/// Returned when a flag value names no output this build writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnknownTargetFormat {
    value: String,
}

impl fmt::Display for UnknownTargetFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown target format \"{}\"; expected one of: {ACCEPTED_TARGET_FORMATS}",
            self.value,
        )
    }
}

impl std::error::Error for UnknownTargetFormat {}

impl FromStr for TargetFormat {
    type Err = UnknownTargetFormat;

    /// A bare family name defers the revision to the printer, which
    /// declares one. Spelling the revision after it, as a profile
    /// does in `ctb@v5`, names the format outright and overrides
    /// whatever the printer declares.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(format) = Format::from_str(s) {
            return Ok(Self::Slices(format));
        }
        if let Some((family, revision)) = s.split_once(REVISION_SEPARATOR) {
            return printer_file_from_spelling(family, revision)
                .map(Self::PrinterFile)
                .ok_or_else(|| UnknownTargetFormat {
                    value: s.to_owned(),
                });
        }
        match s {
            "goo" => Ok(Self::PrinterFamily(FormatKind::Goo)),
            "ctb" => Ok(Self::PrinterFamily(FormatKind::Ctb)),
            other => Err(UnknownTargetFormat {
                value: other.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod target_format_tests {
    use std::path::PathBuf;

    use sc_sla_formats::{CtbVariant, GooVersion};

    use super::*;

    fn goo_v3_0() -> TargetFormat {
        TargetFormat::from_str("goo@v3.0").unwrap()
    }

    fn binding(family: &str, version: Option<&str>) -> OutputFormatBinding {
        OutputFormatBinding {
            format: FormatKind::from_canonical(family),
            version: version.map(|v| NonEmptyString::new(v).unwrap()),
        }
    }

    fn declared(bindings: &[OutputFormatBinding]) -> OutputFormatBindings {
        let mut iter = bindings.iter().cloned();
        let mut out = OutputFormatBindings::new(iter.next().expect("at least one binding"));
        for binding in iter {
            out.push(binding);
        }
        out
    }

    #[test]
    fn every_slice_output_name_parses_into_the_slice_vocabulary() {
        for (name, format) in [
            ("svg-per-layer", Format::SvgPerLayer),
            ("svg-combined", Format::SvgCombined),
            ("png-per-layer", Format::PngPerLayer),
            ("3mf", Format::ThreeMf),
        ] {
            assert_eq!(
                TargetFormat::from_str(name),
                Ok(TargetFormat::Slices(format)),
                "{name}",
            );
        }
    }

    #[test]
    fn a_bare_family_name_leaves_the_revision_to_the_printer() {
        assert_eq!(
            TargetFormat::from_str("goo"),
            Ok(TargetFormat::PrinterFamily(FormatKind::Goo)),
        );
        assert_eq!(
            TargetFormat::from_str("ctb"),
            Ok(TargetFormat::PrinterFamily(FormatKind::Ctb)),
        );
    }

    #[test]
    fn a_family_spelled_with_its_revision_names_the_format_outright() {
        for spelling in ["goo@v3.0", "goo@v5.1", "ctb@v5", "ctb@v5enc"] {
            assert!(
                matches!(
                    TargetFormat::from_str(spelling),
                    Ok(TargetFormat::PrinterFile(_)),
                ),
                "{spelling}",
            );
        }
        assert_eq!(goo_v3_0().to_string(), "goo@v3.0");
    }

    #[test]
    fn a_revision_no_container_here_names_is_refused() {
        for spelling in ["goo@v9", "ctb@v1", "sl1@v1", "goo@"] {
            assert!(TargetFormat::from_str(spelling).is_err(), "{spelling}");
        }
    }

    #[test]
    fn parsing_is_case_sensitive() {
        assert!(TargetFormat::from_str("GOO").is_err());
        assert!(TargetFormat::from_str("SVG-Combined").is_err());
    }

    #[test]
    fn a_family_this_build_has_no_container_for_is_refused() {
        for name in ["sl1", "lumen", "pwmx"] {
            assert!(TargetFormat::from_str(name).is_err(), "{name}");
        }
    }

    #[test]
    fn a_bare_family_takes_the_revision_the_printer_declares() {
        let declared = declared(&[binding("ctb", Some("v5"))]);

        assert_eq!(
            settle_for_printer(TargetFormat::from_str("ctb").unwrap(), &declared),
            Ok(SettledTarget::PrinterFile(PrinterFileFormat::Ctb(
                CtbVariant::PlainV5
            ))),
        );
    }

    #[test]
    fn a_printer_declaring_two_families_answers_for_the_one_asked_about() {
        let declared = declared(&[binding("ctb", Some("v5enc")), binding("goo", Some("v3.0"))]);

        assert_eq!(
            settle_for_printer(TargetFormat::from_str("goo").unwrap(), &declared),
            Ok(SettledTarget::PrinterFile(PrinterFileFormat::Goo(
                GooVersion::V3_0
            ))),
        );
    }

    #[test]
    fn a_family_the_printer_does_not_declare_names_what_it_does() {
        let declared = declared(&[binding("goo", Some("v3.0"))]);
        let err = settle_for_printer(TargetFormat::from_str("ctb").unwrap(), &declared)
            .unwrap_err()
            .to_string();

        assert!(err.contains("no \"ctb\""), "{err}");
        assert!(err.contains("goo@v3.0"), "{err}");
    }

    #[test]
    fn a_declared_revision_without_a_writer_is_refused_by_name() {
        let declared = declared(&[binding("ctb", Some("v5enc"))]);
        let err = settle_for_printer(TargetFormat::from_str("ctb").unwrap(), &declared)
            .unwrap_err()
            .to_string();

        assert!(err.contains("ctb@v5enc"), "{err}");
    }

    #[test]
    fn a_declared_revision_this_build_cannot_name_is_refused() {
        let declared = declared(&[binding("ctb", Some("v2"))]);

        assert!(settle_for_printer(TargetFormat::from_str("ctb").unwrap(), &declared).is_err(),);
    }

    #[test]
    fn a_spelled_revision_overrides_what_the_printer_declares() {
        let declared = declared(&[binding("ctb", Some("v5enc"))]);

        assert_eq!(
            settle_for_printer(TargetFormat::from_str("ctb@v5").unwrap(), &declared),
            Ok(SettledTarget::PrinterFile(PrinterFileFormat::Ctb(
                CtbVariant::PlainV5
            ))),
        );
    }

    #[test]
    fn a_slice_output_settles_to_itself_whatever_the_printer_declares() {
        let declared = declared(&[binding("ctb", Some("v5enc"))]);

        assert_eq!(
            settle_for_printer(TargetFormat::Slices(Format::PngPerLayer), &declared),
            Ok(SettledTarget::Slices(Format::PngPerLayer)),
        );
    }

    #[test]
    fn the_failure_names_the_value_and_every_accepted_spelling() {
        let message = TargetFormat::from_str("xyz").unwrap_err().to_string();
        assert!(message.contains("xyz"), "message: {message}");
        for accepted in ACCEPTED_TARGET_FORMATS.split(", ") {
            assert!(message.contains(accepted), "{accepted} in: {message}");
        }
    }

    #[test]
    fn every_accepted_spelling_parses() {
        for accepted in ACCEPTED_TARGET_FORMATS.split(", ") {
            assert!(TargetFormat::from_str(accepted).is_ok(), "{accepted}");
        }
    }

    #[test]
    fn a_slice_output_extension_infers_its_own_vocabulary() {
        assert_eq!(
            TargetFormat::from_path(&PathBuf::from("plate.svg")),
            Some(TargetFormat::Slices(Format::SvgCombined)),
        );
        assert_eq!(
            TargetFormat::from_path(&PathBuf::from("plate.slices.3mf")),
            Some(TargetFormat::Slices(Format::ThreeMf)),
        );
    }

    #[test]
    fn a_printer_file_extension_infers_its_family_and_no_revision() {
        assert_eq!(
            TargetFormat::from_path(&PathBuf::from("plate.goo")),
            Some(TargetFormat::PrinterFamily(FormatKind::Goo)),
        );
        assert_eq!(
            TargetFormat::from_path(&PathBuf::from("plate.ctb")),
            Some(TargetFormat::PrinterFamily(FormatKind::Ctb)),
        );
    }

    #[test]
    fn an_extension_no_writer_claims_infers_nothing() {
        assert_eq!(TargetFormat::from_path(&PathBuf::from("plate.stl")), None);
        assert_eq!(TargetFormat::from_path(&PathBuf::from("plate")), None);
    }

    #[test]
    fn only_the_per_layer_slice_outputs_fill_a_directory() {
        assert!(TargetFormat::Slices(Format::SvgPerLayer).is_directory_format());
        assert!(TargetFormat::Slices(Format::PngPerLayer).is_directory_format());
        assert!(!TargetFormat::Slices(Format::SvgCombined).is_directory_format());
        assert!(!TargetFormat::Slices(Format::ThreeMf).is_directory_format());
        assert!(!goo_v3_0().is_directory_format());
        assert!(!TargetFormat::PrinterFamily(FormatKind::Ctb).is_directory_format());
    }

    #[test]
    fn a_target_renders_the_way_it_was_named() {
        assert_eq!(goo_v3_0().to_string(), "goo@v3.0");
        assert_eq!(
            TargetFormat::PrinterFamily(FormatKind::Ctb).to_string(),
            "ctb",
        );
        assert_eq!(
            TargetFormat::Slices(Format::PngPerLayer).to_string(),
            "png-per-layer",
        );
    }
}

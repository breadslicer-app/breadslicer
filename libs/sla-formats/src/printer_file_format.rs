/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Which printer file a profile's declared format names.
//!
//! A printer profile declares each output format as an [`OutputFormatBinding`]:
//! a format family plus a version string. Both halves are needed to reach a
//! revision. `.goo` at `v5.1` shares an extension with `.goo` at `V3.0` and
//! nothing else, and `ctb` at `v5enc` differs from `ctb` at `v5` in whether the
//! settings block is encrypted.
//!
//! Some formats are named here but written by a writer crate installed at run
//! time (see [`crate::external`]): this crate stays free of their code, and
//! [`PrinterFileFormat::has_writer`] reports them writable once one is
//! installed.

use std::cmp::Ordering;
use std::fmt;
use std::path::Path;

use sc_print_primitives::{FormatKind, NonEmptyString, OutputFormatBinding, OutputFormatBindings};

use crate::ctb::PREVIEW_SIZES as CTB_PREVIEW_SIZES;
use crate::external;
use crate::goo::v3_0::header::PREVIEW_SIZES as GOO_V3_0_PREVIEW_SIZES;
use crate::preview::PreviewSizes;

/// A printer-file format this crate targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrinterFileFormat {
    Goo(GooVersion),
    Ctb(CtbVariant),
    /// Creality's CXDLP container.
    Cxdlp(CxdlpVersion),
    /// Anycubic's ZIP container.
    AnycubicZip(AnycubicZipVariant),
}

/// A revision of Creality's CXDLP container. A later revision compares greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CxdlpVersion {
    V2 = 0,
    V3 = 1,
}

/// A variant of Anycubic's ZIP container. The variants are one layout written
/// for different machines under different extensions, not revisions of each
/// other, so 2 different variants are incomparable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnycubicZipVariant {
    /// `.pwsz`, the Photon Mono M7 Pro.
    Pwsz,
    /// `.pm7`, the Photon Mono M7.
    Pm7,
    /// `.pm7m`, the Photon Mono M7 Max.
    Pm7m,
}

impl AnycubicZipVariant {
    /// Reports the extension, which is also the binding's version string.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pwsz => "pwsz",
            Self::Pm7 => "pm7",
            Self::Pm7m => "pm7m",
        }
    }
}

/// A revision of the Elegoo GOO container. A later revision compares greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GooVersion {
    V3_0 = 0,
    V5_1 = 1,
}

/// A revision of the ChiTuBox CTB container. Version 5 comes in 2 dialects; the
/// encrypted one compares greater than the plain one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CtbVariant {
    PlainV5 = 0,
    EncryptedV5 = 1,
}

impl PartialOrd for PrinterFileFormat {
    /// Compares 2 formats of the same family by revision. Formats of different
    /// families give `None`.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Goo(revision), Self::Goo(other)) => Some(revision.cmp(other)),
            (Self::Ctb(revision), Self::Ctb(other)) => Some(revision.cmp(other)),
            (Self::Cxdlp(revision), Self::Cxdlp(other)) => Some(revision.cmp(other)),
            (Self::AnycubicZip(variant), Self::AnycubicZip(other)) => {
                (variant == other).then_some(Ordering::Equal)
            }
            _ => None,
        }
    }
}

impl fmt::Display for PrinterFileFormat {
    /// Writes the binding that names the format, as in `goo@v3.0`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.binding().fmt(f)
    }
}

/// Returned when a binding does not name a printer file this crate knows.
#[derive(Clone, Debug, PartialEq)]
pub struct UnresolvedBinding {
    pub binding: OutputFormatBinding,
}

impl fmt::Display for UnresolvedBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let family = self.binding.format.as_canonical();
        match (&self.binding.format, &self.binding.version) {
            (
                FormatKind::Goo | FormatKind::Ctb | FormatKind::Cxdlp | FormatKind::Azf,
                Some(version),
            ) => {
                write!(f, "unknown version \"{version}\" for format \"{family}\"",)
            }
            (FormatKind::Goo | FormatKind::Ctb | FormatKind::Cxdlp | FormatKind::Azf, None) => {
                write!(
                    f,
                    "format \"{family}\" needs a version to name a revision, and the binding carries none",
                )
            }
            _ => write!(f, "unknown format \"{family}\""),
        }
    }
}

impl std::error::Error for UnresolvedBinding {}

/// Returned when a binding gives no printer file this build writes.
#[derive(Clone, Debug, PartialEq)]
pub enum UnwritableBinding {
    /// The binding names no printer file this crate knows.
    Unresolved(UnresolvedBinding),
    /// The binding names a format this build has no writer for.
    NoWriter(PrinterFileFormat),
}

impl fmt::Display for UnwritableBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved(err) => err.fmt(f),
            Self::NoWriter(_) => f.write_str("this build has no writer for it yet"),
        }
    }
}

impl std::error::Error for UnwritableBinding {}

/// Returned when a printer lists no binding of a family that this build writes.
#[derive(Clone, Debug, PartialEq)]
pub struct UnwritableFamily {
    /// The family asked for.
    pub family: FormatKind,
    /// Every binding of the family that the printer lists, with the reason this
    /// build cannot write it. Empty when the printer lists none.
    pub refused: Vec<(OutputFormatBinding, UnwritableBinding)>,
}

impl fmt::Display for UnwritableFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let family = self.family.as_canonical();
        write!(f, "this printer declares no \"{family}\" output")?;
        if self.refused.is_empty() {
            return Ok(());
        }
        f.write_str(" this build can write: ")?;
        for (index, (binding, reason)) in self.refused.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{binding} ({reason})")?;
        }
        Ok(())
    }
}

impl std::error::Error for UnwritableFamily {}

impl PrinterFileFormat {
    /// Identifies the printer file a profile's declared binding names.
    ///
    /// Both halves are required: revisions within a family produce different
    /// bytes, so a binding without a version is refused.
    pub fn from_binding(binding: &OutputFormatBinding) -> Result<Self, UnresolvedBinding> {
        let version = binding.version.as_ref().map(|v| v.as_str().to_lowercase());
        match (&binding.format, version.as_deref()) {
            (FormatKind::Goo, Some("v3.0")) => Ok(Self::Goo(GooVersion::V3_0)),
            (FormatKind::Goo, Some("v5.1")) => Ok(Self::Goo(GooVersion::V5_1)),
            (FormatKind::Ctb, Some("v5")) => Ok(Self::Ctb(CtbVariant::PlainV5)),
            (FormatKind::Ctb, Some("v5enc")) => Ok(Self::Ctb(CtbVariant::EncryptedV5)),
            (FormatKind::Cxdlp, Some("v2")) => Ok(Self::Cxdlp(CxdlpVersion::V2)),
            (FormatKind::Cxdlp, Some("v3")) => Ok(Self::Cxdlp(CxdlpVersion::V3)),
            (FormatKind::Azf, Some("pwsz")) => Ok(Self::AnycubicZip(AnycubicZipVariant::Pwsz)),
            (FormatKind::Azf, Some("pm7")) => Ok(Self::AnycubicZip(AnycubicZipVariant::Pm7)),
            (FormatKind::Azf, Some("pm7m")) => Ok(Self::AnycubicZip(AnycubicZipVariant::Pm7m)),
            _ => Err(UnresolvedBinding {
                binding: binding.clone(),
            }),
        }
    }

    /// Returns the binding that names this format. [`Self::from_binding`]
    /// resolves it back to the same format.
    pub fn binding(self) -> OutputFormatBinding {
        let (format, version) = match self {
            Self::Goo(GooVersion::V3_0) => (FormatKind::Goo, "v3.0"),
            Self::Goo(GooVersion::V5_1) => (FormatKind::Goo, "v5.1"),
            Self::Ctb(CtbVariant::PlainV5) => (FormatKind::Ctb, "v5"),
            Self::Ctb(CtbVariant::EncryptedV5) => (FormatKind::Ctb, "v5enc"),
            Self::Cxdlp(CxdlpVersion::V2) => (FormatKind::Cxdlp, "v2"),
            Self::Cxdlp(CxdlpVersion::V3) => (FormatKind::Cxdlp, "v3"),
            Self::AnycubicZip(variant) => (FormatKind::Azf, variant.extension()),
        };
        OutputFormatBinding {
            format,
            version: Some(NonEmptyString::new(version).expect("a version literal is not empty")),
        }
    }

    /// Identifies the printer file `binding` names, when this build writes it.
    pub fn writable_from_binding(binding: &OutputFormatBinding) -> Result<Self, UnwritableBinding> {
        let format = Self::from_binding(binding).map_err(UnwritableBinding::Unresolved)?;
        if format.has_writer() {
            Ok(format)
        } else {
            Err(UnwritableBinding::NoWriter(format))
        }
    }

    /// Picks the format a family name means for a printer: the highest revision
    /// of `family` that `declared` lists and this build writes. Any order of
    /// `declared` gives the same format, except among incomparable variants
    /// (Anycubic ZIP), where the first one listed wins.
    pub fn highest_writable(
        family: &FormatKind,
        declared: &OutputFormatBindings,
    ) -> Result<Self, UnwritableFamily> {
        let mut writable = Vec::new();
        let mut refused = Vec::new();
        for binding in declared.iter().filter(|binding| &binding.format == family) {
            match Self::writable_from_binding(binding) {
                Ok(format) => writable.push(format),
                Err(reason) => refused.push((binding.clone(), reason)),
            }
        }
        writable
            .into_iter()
            .reduce(|highest, format| if format > highest { format } else { highest })
            .ok_or_else(|| UnwritableFamily {
                family: family.clone(),
                refused,
            })
    }

    /// Reports whether this build carries a writer for the format.
    ///
    /// Identifying a revision and encoding it are separate: a profile may
    /// declare a format this crate names but cannot yet write, and callers that
    /// offer a choice need to tell the 2 apart before
    /// [`write_printer_file`](crate::write_printer_file) refuses.
    ///
    /// A format this crate does not encode itself is writable once a writer
    /// for it is installed with
    /// [`install_printer_file_writer`](crate::install_printer_file_writer).
    pub fn has_writer(self) -> bool {
        self.has_builtin_writer() || external::writer_for(self).is_some()
    }

    /// Reports whether this crate's own code encodes the format.
    pub(crate) fn has_builtin_writer(self) -> bool {
        matches!(
            self,
            Self::Goo(GooVersion::V3_0) | Self::Ctb(CtbVariant::PlainV5 | CtbVariant::EncryptedV5)
        )
    }

    /// Reports the sizes this format fixes for the previews it carries, so a
    /// caller renders at a size the writer accepts.
    ///
    /// `None` for a format with no writer in this build: the sizes belong to
    /// the container's layout, which arrives with it.
    ///
    /// GOO fixes its 2 sizes. CTB does not, since each of its preview records
    /// carries its own, so what it reports is the size its writer renders at.
    pub fn preview_sizes(self) -> Option<PreviewSizes> {
        match self {
            Self::Goo(GooVersion::V3_0) => Some(GOO_V3_0_PREVIEW_SIZES),
            Self::Ctb(CtbVariant::PlainV5 | CtbVariant::EncryptedV5) => Some(CTB_PREVIEW_SIZES),
            Self::Goo(GooVersion::V5_1) | Self::Cxdlp(_) | Self::AnycubicZip(_) => {
                external::writer_for(self).and_then(|writer| writer.preview_sizes(self))
            }
        }
    }

    /// Reports the file extension this format is written with, without a
    /// leading dot.
    ///
    /// Some file formats that differ in shape share file extension, so an
    /// extension is not enough to identify a file format.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Goo(_) => "goo",
            Self::Ctb(_) => "ctb",
            Self::Cxdlp(_) => "cxdlp",
            Self::AnycubicZip(variant) => variant.extension(),
        }
    }
}

/// Names the container family an output path's extension asks for.
///
/// An extension never names a revision: `.goo` is shared by V3.0 and V5.1, and
/// `.ctb` by the plain and the encrypted variant. The revision comes from a
/// binding's version string, which a printer declares and a file name does not
/// carry. Anycubic's ZIP container gives each variant its own extension, and
/// every one of them names the family.
pub fn printer_file_family_from_path(path: &Path) -> Option<FormatKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "goo" => Some(FormatKind::Goo),
        "ctb" => Some(FormatKind::Ctb),
        "cxdlp" => Some(FormatKind::Cxdlp),
        "pwsz" | "pm7" | "pm7m" => Some(FormatKind::Azf),
        _ => None,
    }
}

#[cfg(test)]
mod printer_file_format_tests {
    use sc_print_primitives::NonEmptyString;

    use super::*;
    use crate::preview::PreviewSize;

    fn binding(format: FormatKind, version: Option<&str>) -> OutputFormatBinding {
        OutputFormatBinding {
            format,
            version: version.map(|v| NonEmptyString::new(v).unwrap()),
        }
    }

    fn resolve(format: FormatKind, version: Option<&str>) -> Result<PrinterFileFormat, String> {
        PrinterFileFormat::from_binding(&binding(format, version)).map_err(|err| err.to_string())
    }

    fn declared(bindings: &[OutputFormatBinding]) -> OutputFormatBindings {
        OutputFormatBindings::try_from_vec(bindings.to_vec()).expect("test list is non-empty")
    }

    #[test]
    fn goo_without_a_version_is_refused_rather_than_guessed() {
        let err = resolve(FormatKind::Goo, None).unwrap_err();
        assert!(err.contains("needs a version"), "message: {err}");
    }

    #[test]
    fn goo_at_v3_0_is_the_specified_revision() {
        assert_eq!(
            resolve(FormatKind::Goo, Some("v3.0")),
            Ok(PrinterFileFormat::Goo(GooVersion::V3_0)),
        );
    }

    #[test]
    fn goo_at_v5_1_is_its_own_revision() {
        assert_eq!(
            resolve(FormatKind::Goo, Some("v5.1")),
            Ok(PrinterFileFormat::Goo(GooVersion::V5_1)),
        );
    }

    #[test]
    fn version_matching_ignores_case() {
        assert_eq!(
            resolve(FormatKind::Goo, Some("V3.0")),
            Ok(PrinterFileFormat::Goo(GooVersion::V3_0)),
        );
        assert_eq!(
            resolve(FormatKind::Ctb, Some("V5ENC")),
            Ok(PrinterFileFormat::Ctb(CtbVariant::EncryptedV5)),
        );
    }

    #[test]
    fn goo_at_an_unknown_version_is_refused() {
        assert!(resolve(FormatKind::Goo, Some("v9.9")).is_err());
    }

    #[test]
    fn ctb_v5_is_the_plain_variant() {
        assert_eq!(
            resolve(FormatKind::Ctb, Some("v5")),
            Ok(PrinterFileFormat::Ctb(CtbVariant::PlainV5)),
        );
    }

    #[test]
    fn ctb_v5enc_is_the_encrypted_variant() {
        assert_eq!(
            resolve(FormatKind::Ctb, Some("v5enc")),
            Ok(PrinterFileFormat::Ctb(CtbVariant::EncryptedV5)),
        );
    }

    #[test]
    fn ctb_without_a_version_is_refused_rather_than_guessed() {
        let err = resolve(FormatKind::Ctb, None).unwrap_err();
        assert!(err.contains("needs a version"), "message: {err}");
    }

    #[test]
    fn ctb_at_an_unknown_version_is_refused() {
        assert!(resolve(FormatKind::Ctb, Some("v4enc")).is_err());
    }

    #[test]
    fn every_other_format_kind_is_unknown_here() {
        for format in [
            FormatKind::Sl1,
            FormatKind::Pwmx,
            FormatKind::PngPerLayer,
            FormatKind::SvgPerLayer,
            FormatKind::SvgCombined,
            FormatKind::Unknown("lumen".to_owned()),
        ] {
            let canonical = format.as_canonical().to_owned();
            let err = resolve(format, None).unwrap_err();
            assert!(
                err.contains(&canonical),
                "{canonical} should be named in its own failure, got: {err}",
            );
        }
    }

    #[test]
    fn a_version_does_not_rescue_an_unknown_format_kind() {
        assert!(resolve(FormatKind::Sl1, Some("v1")).is_err());
    }

    #[test]
    fn the_failure_carries_the_binding_it_could_not_resolve() {
        let unresolvable = binding(FormatKind::Goo, Some("v9.9"));
        let err = PrinterFileFormat::from_binding(&unresolvable).unwrap_err();
        assert_eq!(err.binding, unresolvable);
    }

    #[test]
    fn an_unknown_version_failure_names_the_version() {
        let err = resolve(FormatKind::Goo, Some("v9.9")).unwrap_err();
        assert!(err.contains("v9.9"), "message: {err}");
    }

    #[test]
    fn every_format_resolves_back_from_its_binding() {
        for format in [
            PrinterFileFormat::Goo(GooVersion::V3_0),
            PrinterFileFormat::Goo(GooVersion::V5_1),
            PrinterFileFormat::Ctb(CtbVariant::PlainV5),
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5),
            PrinterFileFormat::Cxdlp(CxdlpVersion::V2),
            PrinterFileFormat::Cxdlp(CxdlpVersion::V3),
        ] {
            let binding = format.binding();

            assert_eq!(binding.format.as_canonical(), format.extension());
            assert_eq!(
                PrinterFileFormat::from_binding(&binding),
                Ok(format),
                "{binding}",
            );
        }
    }

    #[test]
    fn a_format_displays_as_its_binding() {
        for (format, displayed) in [
            (PrinterFileFormat::Goo(GooVersion::V3_0), "goo@v3.0"),
            (PrinterFileFormat::Goo(GooVersion::V5_1), "goo@v5.1"),
            (PrinterFileFormat::Ctb(CtbVariant::PlainV5), "ctb@v5"),
            (PrinterFileFormat::Ctb(CtbVariant::EncryptedV5), "ctb@v5enc"),
        ] {
            assert_eq!(format.to_string(), displayed);
            assert_eq!(format.binding().to_string(), displayed);
        }
    }

    #[test]
    fn each_printer_file_extension_names_its_family() {
        assert_eq!(
            printer_file_family_from_path(Path::new("plate.goo")),
            Some(FormatKind::Goo),
        );
        assert_eq!(
            printer_file_family_from_path(Path::new("plate.ctb")),
            Some(FormatKind::Ctb),
        );
    }

    #[test]
    fn extension_matching_ignores_case() {
        assert_eq!(
            printer_file_family_from_path(Path::new("PLATE.GOO")),
            Some(FormatKind::Goo),
        );
    }

    #[test]
    fn a_path_carrying_another_extension_names_no_printer_file() {
        assert_eq!(printer_file_family_from_path(Path::new("plate.svg")), None);
        assert_eq!(printer_file_family_from_path(Path::new("plate.3mf")), None);
    }

    #[test]
    fn a_path_without_an_extension_names_no_printer_file() {
        assert_eq!(printer_file_family_from_path(Path::new("plate")), None);
    }

    #[test]
    fn this_build_writes_goo_v3_0_and_both_ctb_v5_variants() {
        for format in [
            PrinterFileFormat::Goo(GooVersion::V3_0),
            PrinterFileFormat::Ctb(CtbVariant::PlainV5),
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5),
        ] {
            assert!(format.has_writer(), "{format}");
        }
        assert!(!PrinterFileFormat::Goo(GooVersion::V5_1).has_writer());
    }

    #[test]
    fn goo_and_both_ctb_variants_carry_their_own_extension() {
        assert_eq!(PrinterFileFormat::Goo(GooVersion::V3_0).extension(), "goo");
        assert_eq!(PrinterFileFormat::Goo(GooVersion::V5_1).extension(), "goo");
        assert_eq!(
            PrinterFileFormat::Ctb(CtbVariant::PlainV5).extension(),
            "ctb"
        );
        assert_eq!(
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5).extension(),
            "ctb",
        );
    }

    #[test]
    fn goo_v3_0_declares_the_preview_sizes_its_header_reserves() {
        let sizes = PrinterFileFormat::Goo(GooVersion::V3_0)
            .preview_sizes()
            .expect("the v3.0 writer carries previews");
        assert_eq!(
            sizes.small,
            PreviewSize {
                width: 116,
                height: 116
            }
        );
        assert_eq!(
            sizes.large,
            PreviewSize {
                width: 290,
                height: 290
            }
        );
    }

    #[test]
    fn plain_ctb_v5_declares_the_sizes_its_writer_renders_at() {
        let sizes = PrinterFileFormat::Ctb(CtbVariant::PlainV5)
            .preview_sizes()
            .expect("the plain v5 writer carries previews");
        assert_eq!(
            sizes.small,
            PreviewSize {
                width: 200,
                height: 125
            }
        );
        assert_eq!(
            sizes.large,
            PreviewSize {
                width: 400,
                height: 300
            }
        );
    }

    #[test]
    fn a_format_without_a_writer_declares_no_preview_sizes() {
        assert_eq!(
            PrinterFileFormat::Goo(GooVersion::V5_1).preview_sizes(),
            None,
        );
    }

    #[test]
    fn both_ctb_variants_render_at_the_same_sizes() {
        assert_eq!(
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5).preview_sizes(),
            PrinterFileFormat::Ctb(CtbVariant::PlainV5).preview_sizes(),
        );
    }

    #[test]
    fn a_binding_this_build_writes_gives_its_format() {
        assert_eq!(
            PrinterFileFormat::writable_from_binding(&binding(FormatKind::Goo, Some("v3.0"))),
            Ok(PrinterFileFormat::Goo(GooVersion::V3_0)),
        );
    }

    #[test]
    fn each_unwritable_binding_carries_its_own_reason() {
        let listed = [
            binding(FormatKind::Goo, Some("v5.1")),
            binding(FormatKind::Ctb, Some("v2")),
        ];
        let reasons: Vec<_> = listed
            .iter()
            .map(PrinterFileFormat::writable_from_binding)
            .collect();

        assert_eq!(
            reasons,
            vec![
                Err(UnwritableBinding::NoWriter(PrinterFileFormat::Goo(
                    GooVersion::V5_1
                ))),
                Err(UnwritableBinding::Unresolved(UnresolvedBinding {
                    binding: listed[1].clone(),
                })),
            ],
        );
    }

    #[test]
    fn a_family_name_takes_the_highest_revision_this_build_writes() {
        let declared = declared(&[
            binding(FormatKind::Goo, Some("v5.1")),
            binding(FormatKind::Goo, Some("v3.0")),
        ]);

        assert_eq!(
            PrinterFileFormat::highest_writable(&FormatKind::Goo, &declared),
            Ok(PrinterFileFormat::Goo(GooVersion::V3_0)),
        );
    }

    #[test]
    fn the_encrypted_ctb_wins_over_the_plain_one_in_either_order() {
        for listed in [["v5", "v5enc"], ["v5enc", "v5"]] {
            let declared = declared(&listed.map(|version| binding(FormatKind::Ctb, Some(version))));

            assert_eq!(
                PrinterFileFormat::highest_writable(&FormatKind::Ctb, &declared),
                Ok(PrinterFileFormat::Ctb(CtbVariant::EncryptedV5)),
                "{listed:?}",
            );
        }
    }

    #[test]
    fn a_family_name_passes_over_the_other_families_listed() {
        let declared = declared(&[
            binding(FormatKind::Ctb, Some("v5enc")),
            binding(FormatKind::Goo, Some("v3.0")),
        ]);

        assert_eq!(
            PrinterFileFormat::highest_writable(&FormatKind::Goo, &declared),
            Ok(PrinterFileFormat::Goo(GooVersion::V3_0)),
        );
    }

    #[test]
    fn a_family_the_printer_does_not_list_refuses_no_binding() {
        let declared = declared(&[binding(FormatKind::Goo, Some("v3.0"))]);

        assert_eq!(
            PrinterFileFormat::highest_writable(&FormatKind::Ctb, &declared),
            Err(UnwritableFamily {
                family: FormatKind::Ctb,
                refused: Vec::new(),
            }),
        );
    }

    #[test]
    fn a_family_with_nothing_writable_refuses_every_listed_binding() {
        let unwritable = binding(FormatKind::Goo, Some("v5.1"));
        let declared = declared(std::slice::from_ref(&unwritable));

        assert_eq!(
            PrinterFileFormat::highest_writable(&FormatKind::Goo, &declared),
            Err(UnwritableFamily {
                family: FormatKind::Goo,
                refused: vec![(
                    unwritable,
                    UnwritableBinding::NoWriter(PrinterFileFormat::Goo(GooVersion::V5_1)),
                )],
            }),
        );
    }

    #[test]
    fn every_anycubic_zip_variant_resolves_back_and_is_its_own_extension() {
        for variant in [
            AnycubicZipVariant::Pwsz,
            AnycubicZipVariant::Pm7,
            AnycubicZipVariant::Pm7m,
        ] {
            let format = PrinterFileFormat::AnycubicZip(variant);
            let binding = format.binding();

            assert_eq!(binding.format, FormatKind::Azf);
            assert_eq!(
                binding.version.as_ref().unwrap().as_str(),
                format.extension()
            );
            assert_eq!(PrinterFileFormat::from_binding(&binding), Ok(format));
            let path = format!("plate.{}", format.extension());
            assert_eq!(
                printer_file_family_from_path(Path::new(&path)),
                Some(FormatKind::Azf),
            );
        }
    }

    #[test]
    fn cxdlp_names_both_revisions_and_v3_is_the_later_one() {
        assert_eq!(
            resolve(FormatKind::Cxdlp, Some("V3")),
            Ok(PrinterFileFormat::Cxdlp(CxdlpVersion::V3)),
        );
        assert!(
            PrinterFileFormat::Cxdlp(CxdlpVersion::V3) > PrinterFileFormat::Cxdlp(CxdlpVersion::V2)
        );
        assert_eq!(
            printer_file_family_from_path(Path::new("plate.cxdlp")),
            Some(FormatKind::Cxdlp),
        );
        assert!(
            resolve(FormatKind::Cxdlp, None)
                .unwrap_err()
                .contains("needs a version")
        );
    }

    #[test]
    fn different_anycubic_zip_variants_are_incomparable() {
        let pwsz = PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pwsz);
        let pm7 = PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pm7);

        assert_eq!(pwsz.partial_cmp(&pm7), None);
        assert_eq!(pwsz.partial_cmp(&pwsz), Some(Ordering::Equal));
    }

    #[test]
    fn goo_v5_1_compares_greater_than_v3_0() {
        assert!(
            PrinterFileFormat::Goo(GooVersion::V5_1) > PrinterFileFormat::Goo(GooVersion::V3_0)
        );
    }

    #[test]
    fn the_encrypted_ctb_compares_greater_than_the_plain_one() {
        assert!(
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5)
                > PrinterFileFormat::Ctb(CtbVariant::PlainV5)
        );
    }

    #[test]
    fn formats_of_different_families_are_incomparable() {
        let goo = PrinterFileFormat::Goo(GooVersion::V3_0);
        let ctb = PrinterFileFormat::Ctb(CtbVariant::PlainV5);

        assert_eq!(goo.partial_cmp(&ctb), None);
        assert_eq!(ctb.partial_cmp(&goo), None);
    }

    #[test]
    fn a_format_compares_equal_to_itself() {
        let format = PrinterFileFormat::Ctb(CtbVariant::EncryptedV5);

        assert_eq!(format.partial_cmp(&format), Some(Ordering::Equal));
    }

    #[test]
    fn an_unlisted_family_says_the_printer_declares_none() {
        let err = UnwritableFamily {
            family: FormatKind::Ctb,
            refused: Vec::new(),
        };

        assert_eq!(err.to_string(), "this printer declares no \"ctb\" output");
    }

    #[test]
    fn an_unwritable_family_names_each_refused_binding_and_its_reason() {
        let unknown = binding(FormatKind::Goo, Some("v9"));
        let err = UnwritableFamily {
            family: FormatKind::Goo,
            refused: vec![
                (
                    binding(FormatKind::Goo, Some("v5.1")),
                    UnwritableBinding::NoWriter(PrinterFileFormat::Goo(GooVersion::V5_1)),
                ),
                (
                    unknown.clone(),
                    UnwritableBinding::Unresolved(UnresolvedBinding { binding: unknown }),
                ),
            ],
        };

        assert_eq!(
            err.to_string(),
            "this printer declares no \"goo\" output this build can write: \
             goo@v5.1 (this build has no writer for it yet), \
             goo@v9 (unknown version \"v9\" for format \"goo\")",
        );
    }
}

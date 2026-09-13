/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Which printer file a profile's declared format names.
//!
//! A printer profile states what it wants as an
//! [`OutputFormatBinding`]: a format family plus a version string.
//! Both halves are needed to reach a revision. `.goo` at `v5.1`
//! shares an extension with `.goo` at `V3.0` and nothing else, and
//! `ctb` at `v5enc` differs from `ctb` at `v5` in whether the
//! settings block is encrypted.

use std::fmt;
use std::path::Path;

use sc_print_primitives::{FormatKind, OutputFormatBinding};

use crate::ctb::PREVIEW_SIZES as CTB_PREVIEW_SIZES;
use crate::goo::v3_0::header::PREVIEW_SIZES as GOO_V3_0_PREVIEW_SIZES;
use crate::preview::PreviewSizes;

/// A printer-file format this crate targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrinterFileFormat {
    Goo(GooVersion),
    Ctb(CtbVariant),
}

/// A revision of the Elegoo GOO container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GooVersion {
    V3_0,
    V5_1,
}

/// A revision of the ChiTuBox CTB container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtbVariant {
    PlainV5,
    EncryptedV5,
}

impl fmt::Display for PrinterFileFormat {
    /// Render the format the way a profile binding spells it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = match self {
            Self::Goo(GooVersion::V3_0) => "v3.0",
            Self::Goo(GooVersion::V5_1) => "v5.1",
            Self::Ctb(CtbVariant::PlainV5) => "v5",
            Self::Ctb(CtbVariant::EncryptedV5) => "v5enc",
        };
        write!(f, "{}@{version}", self.extension())
    }
}

/// Returned when a binding does not name a printer file this crate
/// knows.
#[derive(Clone, Debug, PartialEq)]
pub struct UnresolvedBinding {
    pub binding: OutputFormatBinding,
}

impl fmt::Display for UnresolvedBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let family = self.binding.format.as_canonical();
        match (&self.binding.format, &self.binding.version) {
            (FormatKind::Goo | FormatKind::Ctb, Some(version)) => {
                write!(f, "unknown version \"{version}\" for format \"{family}\"",)
            }
            (FormatKind::Goo | FormatKind::Ctb, None) => write!(
                f,
                "format \"{family}\" needs a version to name a revision, and the binding carries none",
            ),
            _ => write!(f, "unknown format \"{family}\""),
        }
    }
}

impl std::error::Error for UnresolvedBinding {}

impl PrinterFileFormat {
    /// Identify the printer file a profile's declared binding names.
    ///
    /// Both halves are required: revisions within a family produce
    /// different bytes, so an absent version is refused rather than
    /// resolved to one of them.
    pub fn from_binding(binding: &OutputFormatBinding) -> Result<Self, UnresolvedBinding> {
        let version = binding.version.as_ref().map(|v| v.as_str().to_lowercase());
        match (&binding.format, version.as_deref()) {
            (FormatKind::Goo, Some("v3.0")) => Ok(Self::Goo(GooVersion::V3_0)),
            (FormatKind::Goo, Some("v5.1")) => Ok(Self::Goo(GooVersion::V5_1)),
            (FormatKind::Ctb, Some("v5")) => Ok(Self::Ctb(CtbVariant::PlainV5)),
            (FormatKind::Ctb, Some("v5enc")) => Ok(Self::Ctb(CtbVariant::EncryptedV5)),
            _ => Err(UnresolvedBinding {
                binding: binding.clone(),
            }),
        }
    }

    /// Report whether this build carries a writer for the format.
    ///
    /// Identifying a revision and encoding it are separate: a profile
    /// may declare a format this crate names but cannot yet write, and
    /// callers that offer a choice need to tell the two apart before
    /// [`write_printer_file`](crate::write_printer_file) refuses.
    pub fn has_writer(self) -> bool {
        matches!(
            self,
            Self::Goo(GooVersion::V3_0) | Self::Ctb(CtbVariant::PlainV5)
        )
    }

    /// Report the sizes this format fixes for the previews it
    /// carries, so a caller renders at a size the writer accepts.
    ///
    /// `None` for a format with no writer in this build: the sizes
    /// belong to the container's layout, which arrives with it.
    ///
    /// GOO fixes its 2 sizes. CTB does not, since each of its preview
    /// records carries its own, so what it reports is the size its
    /// writer renders at.
    pub fn preview_sizes(self) -> Option<PreviewSizes> {
        match self {
            Self::Goo(GooVersion::V3_0) => Some(GOO_V3_0_PREVIEW_SIZES),
            Self::Ctb(CtbVariant::PlainV5) => Some(CTB_PREVIEW_SIZES),
            Self::Goo(GooVersion::V5_1) | Self::Ctb(CtbVariant::EncryptedV5) => None,
        }
    }

    /// Report the file extension this format is written with, without
    /// a leading dot.
    ///
    /// Some file formats that differ in shape share file extension, so
    /// an extension is not enough to identify a file format.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Goo(_) => "goo",
            Self::Ctb(_) => "ctb",
        }
    }
}

/// Name the container family an output path's extension asks for.
///
/// An extension never names a revision: `.goo` is shared by V3.0 and
/// V5.1, and `.ctb` by the plain and the encrypted variant. The
/// revision comes from a binding's version string, which a printer
/// declares and a file name does not carry.
pub fn printer_file_family_from_path(path: &Path) -> Option<FormatKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "goo" => Some(FormatKind::Goo),
        "ctb" => Some(FormatKind::Ctb),
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
    fn a_rendered_format_resolves_back_from_a_binding() {
        for format in [
            PrinterFileFormat::Goo(GooVersion::V3_0),
            PrinterFileFormat::Goo(GooVersion::V5_1),
            PrinterFileFormat::Ctb(CtbVariant::PlainV5),
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5),
        ] {
            let rendered = format.to_string();
            let (family, version) = rendered.split_once('@').unwrap();
            let kind = match family {
                "goo" => FormatKind::Goo,
                "ctb" => FormatKind::Ctb,
                other => panic!("unexpected family \"{other}\""),
            };

            assert_eq!(family, format.extension());
            assert_eq!(resolve(kind, Some(version)), Ok(format), "{rendered}");
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
    fn this_build_writes_goo_v3_0_and_plain_ctb_v5() {
        assert!(PrinterFileFormat::Goo(GooVersion::V3_0).has_writer());
        assert!(PrinterFileFormat::Ctb(CtbVariant::PlainV5).has_writer());
        for format in [
            PrinterFileFormat::Goo(GooVersion::V5_1),
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5),
        ] {
            assert!(!format.has_writer(), "{format}");
        }
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
        assert_eq!(
            PrinterFileFormat::Ctb(CtbVariant::EncryptedV5).preview_sizes(),
            None,
        );
    }
}

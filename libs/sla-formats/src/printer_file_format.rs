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

/// Name the printer file an output path's extension asks for.
///
/// An extension names a container family, not a revision: `.goo` is
/// shared by V3.0 and V5.1, and `.ctb` by the plain and the encrypted
/// variant. `.goo` therefore resolves only because one GOO revision
/// has a writer here, while `.ctb` resolves to nothing: its two
/// variants are told apart by a binding's version string, which a
/// file name does not carry.
pub fn printer_file_format_from_path(path: &Path) -> Option<PrinterFileFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "goo" => Some(PrinterFileFormat::Goo(GooVersion::V3_0)),
        _ => None,
    }
}

#[cfg(test)]
mod printer_file_format_tests {
    use sc_print_primitives::NonEmptyString;

    use super::*;

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
    fn a_dot_goo_path_names_the_revision_this_build_writes() {
        assert_eq!(
            printer_file_format_from_path(Path::new("plate.goo")),
            Some(PrinterFileFormat::Goo(GooVersion::V3_0)),
        );
    }

    #[test]
    fn extension_matching_ignores_case() {
        assert_eq!(
            printer_file_format_from_path(Path::new("PLATE.GOO")),
            Some(PrinterFileFormat::Goo(GooVersion::V3_0)),
        );
    }

    #[test]
    fn a_dot_ctb_path_names_no_variant() {
        assert_eq!(printer_file_format_from_path(Path::new("plate.ctb")), None);
    }

    #[test]
    fn a_path_carrying_another_extension_names_no_printer_file() {
        assert_eq!(printer_file_format_from_path(Path::new("plate.svg")), None);
        assert_eq!(printer_file_format_from_path(Path::new("plate.3mf")), None);
    }

    #[test]
    fn a_path_without_an_extension_names_no_printer_file() {
        assert_eq!(printer_file_format_from_path(Path::new("plate")), None);
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
}

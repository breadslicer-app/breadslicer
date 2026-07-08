/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Discriminator for the `--printer-profile` / `--material-profile`
//! flag values.
//!
//! A flag value is either a filesystem path to a profile TOML file
//! or a profile id to be looked up in the configured libraries. The
//! two are disjoint by construction: a profile id matches the slug
//! grammar `[a-z0-9][a-z0-9-]*`, which can never start with a path
//! marker. So the value is classified as a path when it begins with
//! a distinctly path-shaped prefix, and is otherwise required to
//! parse as a valid profile id. Anything that is neither is a hard
//! error naming both contracts; the discriminator never guesses.
//!
//! The path-marker predicate is conditionally compiled per platform
//! so that Windows path shapes (drive prefixes, backslashes) are
//! recognised on Windows targets.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use sc_print_primitives::{InvalidProfileId, MaterialId, PrinterId};

/// A `--printer-profile` flag value: an explicit path or a printer
/// id.
#[derive(Clone, Debug)]
pub enum PrinterProfileRef {
    Path(PathBuf),
    Id(PrinterId),
}

/// A `--material-profile` flag value: an explicit path or a material
/// id.
#[derive(Clone, Debug)]
pub enum MaterialProfileRef {
    Path(PathBuf),
    Id(MaterialId),
}

/// Returned when a flag value is neither a path nor a valid profile
/// id.
#[derive(Debug)]
pub struct ProfileRefParseError {
    value: String,
    id_error: InvalidProfileId,
}

impl fmt::Display for ProfileRefParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' is neither a path (a path must start with a marker such as '/' or './') nor a valid profile id ([a-z0-9][a-z0-9-]*): {}",
            self.value, self.id_error,
        )
    }
}

impl std::error::Error for ProfileRefParseError {}

#[cfg(not(windows))]
fn is_path_ish(value: &str) -> bool {
    value.starts_with('/') || value.starts_with("./") || value.starts_with("../")
}

#[cfg(windows)]
fn is_path_ish(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with('\\')
        || value.starts_with("./")
        || value.starts_with(".\\")
        || value.starts_with("../")
        || value.starts_with("..\\")
        || has_drive_prefix(value)
}

#[cfg(windows)]
fn has_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

impl FromStr for PrinterProfileRef {
    type Err = ProfileRefParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if is_path_ish(value) {
            Ok(Self::Path(PathBuf::from(value)))
        } else {
            match PrinterId::new(value) {
                Ok(id) => Ok(Self::Id(id)),
                Err(id_error) => Err(ProfileRefParseError {
                    value: value.to_owned(),
                    id_error,
                }),
            }
        }
    }
}

impl FromStr for MaterialProfileRef {
    type Err = ProfileRefParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if is_path_ish(value) {
            Ok(Self::Path(PathBuf::from(value)))
        } else {
            match MaterialId::new(value) {
                Ok(id) => Ok(Self::Id(id)),
                Err(id_error) => Err(ProfileRefParseError {
                    value: value.to_owned(),
                    id_error,
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_slug_parses_as_a_printer_id() {
        match "elegoo-saturn-4-ultra"
            .parse::<PrinterProfileRef>()
            .unwrap()
        {
            PrinterProfileRef::Id(id) => assert_eq!(id.as_str(), "elegoo-saturn-4-ultra"),
            other => panic!("expected an id, got {other:?}"),
        }
    }

    #[test]
    fn dot_slash_prefix_parses_as_a_path() {
        match "./printers/foo.toml".parse::<PrinterProfileRef>().unwrap() {
            PrinterProfileRef::Path(p) => {
                assert_eq!(p, PathBuf::from("./printers/foo.toml"))
            }
            other => panic!("expected a path, got {other:?}"),
        }
    }

    #[test]
    fn absolute_prefix_parses_as_a_path() {
        match "/etc/profiles/foo.toml"
            .parse::<MaterialProfileRef>()
            .unwrap()
        {
            MaterialProfileRef::Path(p) => assert_eq!(p, PathBuf::from("/etc/profiles/foo.toml")),
            other => panic!("expected a path, got {other:?}"),
        }
    }

    #[test]
    fn parent_prefix_parses_as_a_path() {
        match "../foo.toml".parse::<PrinterProfileRef>().unwrap() {
            PrinterProfileRef::Path(p) => assert_eq!(p, PathBuf::from("../foo.toml")),
            other => panic!("expected a path, got {other:?}"),
        }
    }

    #[test]
    fn neither_path_nor_valid_id_is_rejected() {
        // Uppercase and underscores are outside the id grammar and
        // the string is not path-ish, so it is rejected outright.
        let err = "Foo_Bar".parse::<PrinterProfileRef>().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Foo_Bar"), "msg: {msg}");
        assert!(msg.contains("valid profile id"), "msg: {msg}");
    }

    #[test]
    fn bare_slug_parses_as_a_material_id() {
        match "sirayatech-blu".parse::<MaterialProfileRef>().unwrap() {
            MaterialProfileRef::Id(id) => assert_eq!(id.as_str(), "sirayatech-blu"),
            other => panic!("expected an id, got {other:?}"),
        }
    }
}

// Windows-only path shapes (drive prefixes, backslashes) that the
// `#[cfg(windows)]` `is_path_ish` / `has_drive_prefix` predicate must
// recognise. The shared tests above only use `/`, `./`, `../`, which
// are valid on every platform, so this branch is otherwise unexercised.
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn as_printer_path(value: &str) -> PathBuf {
        match value.parse::<PrinterProfileRef>().unwrap() {
            PrinterProfileRef::Path(p) => p,
            other => panic!("expected a path for {value:?}, got {other:?}"),
        }
    }

    // Drive-letter prefixes with either separator are paths.
    #[test]
    fn drive_letter_prefixes_parse_as_paths() {
        for value in ["C:\\printers\\foo.toml", "C:/printers/foo.toml", "z:\\x"] {
            assert_eq!(as_printer_path(value), PathBuf::from(value));
        }
    }

    // Backslash spellings are paths (leading `\`, `.\`, `..\`, UNC `\\`).
    #[test]
    fn backslash_spellings_parse_as_paths() {
        for value in [
            "\\foo",
            ".\\foo.toml",
            "..\\foo.toml",
            "\\\\server\\share\\p.toml",
        ] {
            assert_eq!(as_printer_path(value), PathBuf::from(value));
        }
    }

    // The forward-slash arm still holds on Windows.
    #[test]
    fn forward_slash_spellings_parse_as_paths_on_windows() {
        for value in ["/etc/x.toml", "./x.toml", "../x.toml"] {
            assert_eq!(as_printer_path(value), PathBuf::from(value));
        }
    }

    // Near-miss drive shapes are NOT paths. A bare letter is a valid
    // id; `c:` (no separator, `has_drive_prefix` needs len >= 3) and
    // `foo:bar` carry a colon outside the slug grammar and are rejected.
    #[test]
    fn near_miss_drive_shapes_are_not_paths() {
        match "c".parse::<PrinterProfileRef>().unwrap() {
            PrinterProfileRef::Id(id) => assert_eq!(id.as_str(), "c"),
            other => panic!("expected an id for \"c\", got {other:?}"),
        }
        assert!(
            "c:".parse::<PrinterProfileRef>().is_err(),
            "\"c:\" is neither path-ish nor a valid id"
        );
        assert!(
            "foo:bar".parse::<PrinterProfileRef>().is_err(),
            "\"foo:bar\" is neither path-ish nor a valid id"
        );
    }

    // The material ref shares is_path_ish; spot-check both shapes.
    #[test]
    fn material_ref_recognises_windows_paths() {
        match "C:\\materials\\blu.toml"
            .parse::<MaterialProfileRef>()
            .unwrap()
        {
            MaterialProfileRef::Path(p) => assert_eq!(p, PathBuf::from("C:\\materials\\blu.toml")),
            other => panic!("expected a path, got {other:?}"),
        }
        match "..\\blu.toml".parse::<MaterialProfileRef>().unwrap() {
            MaterialProfileRef::Path(p) => assert_eq!(p, PathBuf::from("..\\blu.toml")),
            other => panic!("expected a path, got {other:?}"),
        }
    }
}

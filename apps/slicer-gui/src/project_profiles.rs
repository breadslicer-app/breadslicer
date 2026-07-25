/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Resolution of profile documents for project files.
//!
//! Project saves embed the referenced printer/resin profiles as raw
//! TOML bytes, and project opens classify the references against
//! what is installed locally. Both walk the same roots the profile
//! catalog uses: the user library first, then every bundled data
//! directory, first hit wins.

use std::fs;
use std::path::PathBuf;

use sc_print_primitives::{MaterialId, PrinterId};
use sc_print_profiles::{
    material_extension_path_in_library, material_profile_path_in_library,
    printer_profile_path_in_library, user_library_dir,
};
use sc_project_formats::{EmbeddedProfile, ProjectPayload};

/// Profile library roots in precedence order: the user's writable
/// library (when a home directory resolves), then the bundled
/// libraries shipped with the app. Mirrors the catalog's root list.
pub(crate) fn profile_library_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(user) = user_library_dir() {
        roots.push(user);
    }
    for root in sc_app_paths::bundled_data_dirs() {
        roots.push(root.join("profiles"));
    }
    roots
}

/// Whether a printer profile with this id resolves in any root.
pub(crate) fn printer_installed(roots: &[PathBuf], id: &PrinterId) -> bool {
    roots
        .iter()
        .any(|root| printer_profile_path_in_library(root, id).is_file())
}

/// Whether a material profile with this id resolves in any root.
pub(crate) fn material_installed(roots: &[PathBuf], id: &MaterialId) -> bool {
    roots
        .iter()
        .any(|root| material_profile_path_in_library(root, id).is_file())
}

/// Raw bytes of the printer profile document, from the first root
/// that has it.
fn printer_profile_bytes(roots: &[PathBuf], id: &PrinterId) -> Option<Vec<u8>> {
    roots
        .iter()
        .find_map(|root| fs::read(printer_profile_path_in_library(root, id)).ok())
}

/// Raw bytes of the material profile document, from the first root
/// that has it.
fn material_profile_bytes(roots: &[PathBuf], id: &MaterialId) -> Option<Vec<u8>> {
    roots
        .iter()
        .find_map(|root| fs::read(material_profile_path_in_library(root, id)).ok())
}

/// Raw bytes of the material extension document, from the first
/// root that has it.
fn material_extension_bytes(roots: &[PathBuf], id: &MaterialId) -> Option<Vec<u8>> {
    roots
        .iter()
        .find_map(|root| fs::read(material_extension_path_in_library(root, id)).ok())
}

/// Completes `payload` for a save: every referenced profile without
/// a held embedded document gets one resolved from `roots` (raw
/// on-disk bytes), covering the printer, the resin, and the resin's
/// extension when one exists. Held documents are never replaced;
/// they carry the bytes the project was opened with. A reference
/// that resolves nowhere simply stays a plain reference.
pub(crate) fn complete_embedded_documents(payload: &mut ProjectPayload, roots: &[PathBuf]) {
    if let Some(printer) = payload.printer.clone()
        && !payload.embeds_printer(&printer)
        && let Some(bytes) = printer_profile_bytes(roots, &printer)
    {
        payload
            .embedded
            .push(EmbeddedProfile::Printer { id: printer, bytes });
    }
    if let Some(resin) = payload.resin.clone() {
        if !payload.embeds_material(&resin)
            && let Some(bytes) = material_profile_bytes(roots, &resin)
        {
            payload.embedded.push(EmbeddedProfile::Material {
                id: resin.clone(),
                bytes,
            });
        }
        let extension_held = payload
            .embedded
            .iter()
            .any(|p| matches!(p, EmbeddedProfile::MaterialExtension { id, .. } if *id == resin));
        if !extension_held && let Some(bytes) = material_extension_bytes(roots, &resin) {
            payload
                .embedded
                .push(EmbeddedProfile::MaterialExtension { id: resin, bytes });
        }
    }
}

#[cfg(test)]
mod project_profiles_tests {
    use super::*;

    use std::fs;
    use std::path::Path;

    fn printer_id(raw: &str) -> PrinterId {
        PrinterId::new(raw).expect("test id is a valid slug")
    }

    fn material_id(raw: &str) -> MaterialId {
        MaterialId::new(raw).expect("test id is a valid slug")
    }

    fn write_profile(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("profile paths have parents"))
            .expect("profile dir");
        fs::write(path, bytes).expect("profile file");
    }

    #[test]
    fn installed_checks_probe_every_root() {
        let a = tempfile::tempdir().expect("root a");
        let b = tempfile::tempdir().expect("root b");
        write_profile(b.path(), "printers/only-in-b.toml", b"x = 1");
        write_profile(b.path(), "materials/resin-in-b.toml", b"x = 1");
        let roots = vec![a.path().to_path_buf(), b.path().to_path_buf()];

        assert!(printer_installed(&roots, &printer_id("only-in-b")));
        assert!(!printer_installed(&roots, &printer_id("nowhere")));
        assert!(material_installed(&roots, &material_id("resin-in-b")));
        assert!(!material_installed(&roots, &material_id("nowhere")));
    }

    #[test]
    fn the_first_root_wins_on_byte_resolution() {
        let a = tempfile::tempdir().expect("root a");
        let b = tempfile::tempdir().expect("root b");
        write_profile(a.path(), "printers/shared.toml", b"from = \"a\"");
        write_profile(b.path(), "printers/shared.toml", b"from = \"b\"");
        let roots = vec![a.path().to_path_buf(), b.path().to_path_buf()];

        let mut payload = ProjectPayload {
            printer: Some(printer_id("shared")),
            resin: None,
            embedded: Vec::new(),
        };
        complete_embedded_documents(&mut payload, &roots);
        assert_eq!(
            payload.embedded,
            vec![EmbeddedProfile::Printer {
                id: printer_id("shared"),
                bytes: b"from = \"a\"".to_vec(),
            }],
        );
    }

    #[test]
    fn unresolvable_references_stay_plain_references() {
        let root = tempfile::tempdir().expect("root");
        let roots = vec![root.path().to_path_buf()];
        let mut payload = ProjectPayload {
            printer: Some(printer_id("nowhere")),
            resin: Some(material_id("nowhere")),
            embedded: Vec::new(),
        };
        complete_embedded_documents(&mut payload, &roots);
        assert!(payload.embedded.is_empty());
        assert_eq!(payload.printer, Some(printer_id("nowhere")));
        assert_eq!(payload.resin, Some(material_id("nowhere")));
    }

    #[test]
    fn the_resin_extension_rides_along_when_present() {
        let root = tempfile::tempdir().expect("root");
        write_profile(root.path(), "materials/resin.toml", b"kind = \"material\"");
        write_profile(
            root.path(),
            "materials/extensions/resin.toml",
            b"kind = \"extension\"",
        );
        let roots = vec![root.path().to_path_buf()];

        let mut payload = ProjectPayload {
            printer: None,
            resin: Some(material_id("resin")),
            embedded: Vec::new(),
        };
        complete_embedded_documents(&mut payload, &roots);
        assert_eq!(
            payload.embedded,
            vec![
                EmbeddedProfile::Material {
                    id: material_id("resin"),
                    bytes: b"kind = \"material\"".to_vec(),
                },
                EmbeddedProfile::MaterialExtension {
                    id: material_id("resin"),
                    bytes: b"kind = \"extension\"".to_vec(),
                },
            ],
        );
    }

    #[test]
    fn held_documents_survive_and_gaps_are_filled() {
        let root = tempfile::tempdir().expect("root");
        write_profile(
            root.path(),
            "printers/printer.toml",
            b"kind = \"library-printer\"",
        );
        write_profile(
            root.path(),
            "materials/resin.toml",
            b"kind = \"library-material\"",
        );
        let roots = vec![root.path().to_path_buf()];

        let mut payload = ProjectPayload {
            printer: Some(printer_id("printer")),
            resin: Some(material_id("resin")),
            embedded: vec![EmbeddedProfile::Printer {
                id: printer_id("printer"),
                bytes: b"kind = \"held-printer\"".to_vec(),
            }],
        };
        complete_embedded_documents(&mut payload, &roots);
        assert_eq!(
            payload.embedded,
            vec![
                EmbeddedProfile::Printer {
                    id: printer_id("printer"),
                    bytes: b"kind = \"held-printer\"".to_vec(),
                },
                EmbeddedProfile::Material {
                    id: material_id("resin"),
                    bytes: b"kind = \"library-material\"".to_vec(),
                },
            ],
        );
    }
}

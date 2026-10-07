/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Resolution of profile documents for project files.
//!
//! Project saves embed the referenced printer/resin profiles as raw TOML bytes,
//! and project opens classify the references against what is installed locally.
//! Both consult the given [`ProfileLibraries`] roots in their order: first hit
//! wins.

use std::fs;

use sc_print_primitives::{MaterialId, PrinterId};
use sc_print_profiles::{
    ProfileLibraries, material_extension_path_in_library, material_profile_path_in_library,
    printer_profile_path_in_library,
};
use sc_project_formats::{EmbeddedProfile, ProfileRefWarning, ProjectPayload};

/// Reports whether a printer profile with this id resolves in any root.
fn printer_installed(libraries: &ProfileLibraries, id: &PrinterId) -> bool {
    libraries
        .roots()
        .any(|(root, _)| printer_profile_path_in_library(root, id).is_file())
}

/// Reports whether a material profile with this id resolves in any root.
fn material_installed(libraries: &ProfileLibraries, id: &MaterialId) -> bool {
    libraries
        .roots()
        .any(|(root, _)| material_profile_path_in_library(root, id).is_file())
}

/// Classifies the profile references of `payload` against the profiles
/// installed in `libraries`.
pub(crate) fn profile_ref_warnings(
    payload: &ProjectPayload,
    libraries: &ProfileLibraries,
) -> Vec<ProfileRefWarning> {
    payload.profile_ref_warnings(
        |id| printer_installed(libraries, id),
        |id| material_installed(libraries, id),
    )
}

/// Returns the raw bytes of the printer profile document, from the first root
/// that has it.
fn printer_profile_bytes(libraries: &ProfileLibraries, id: &PrinterId) -> Option<Vec<u8>> {
    libraries
        .roots()
        .find_map(|(root, _)| fs::read(printer_profile_path_in_library(root, id)).ok())
}

/// Returns the raw bytes of the material profile document, from the first root
/// that has it.
fn material_profile_bytes(libraries: &ProfileLibraries, id: &MaterialId) -> Option<Vec<u8>> {
    libraries
        .roots()
        .find_map(|(root, _)| fs::read(material_profile_path_in_library(root, id)).ok())
}

/// Returns the raw bytes of the material extension document, from the first
/// root that has it.
fn material_extension_bytes(libraries: &ProfileLibraries, id: &MaterialId) -> Option<Vec<u8>> {
    libraries
        .roots()
        .find_map(|(root, _)| fs::read(material_extension_path_in_library(root, id)).ok())
}

/// Completes `payload` for a save: every referenced profile that has no
/// embedded document in `payload` gets one; this covers the printer, the resin,
/// and the resin's extension when one exists. Embedded documents already in
/// `payload` are left unchanged. A reference that resolves nowhere stays a
/// plain reference.
pub(crate) fn complete_embedded_documents(
    payload: &mut ProjectPayload,
    libraries: &ProfileLibraries,
) {
    if let Some(printer) = payload.printer.clone()
        && !payload.embeds_printer(&printer)
        && let Some(bytes) = printer_profile_bytes(libraries, &printer)
    {
        payload
            .embedded
            .push(EmbeddedProfile::Printer { id: printer, bytes });
    }
    if let Some(resin) = payload.resin.clone() {
        if !payload.embeds_material(&resin)
            && let Some(bytes) = material_profile_bytes(libraries, &resin)
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
        if !extension_held && let Some(bytes) = material_extension_bytes(libraries, &resin) {
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

    /// Libraries with `a` as the user root and `b` bundled, so `a` precedes
    /// `b`.
    fn libraries(a: &Path, b: &Path) -> ProfileLibraries {
        ProfileLibraries::new(Some(a.to_path_buf()), vec![b.to_path_buf()])
    }

    #[test]
    fn installed_checks_probe_every_root() {
        let a = tempfile::tempdir().expect("root a");
        let b = tempfile::tempdir().expect("root b");
        write_profile(b.path(), "printers/only-in-b.toml", b"x = 1");
        write_profile(b.path(), "materials/resin-in-b.toml", b"x = 1");
        let libraries = libraries(a.path(), b.path());

        assert!(printer_installed(&libraries, &printer_id("only-in-b")));
        assert!(!printer_installed(&libraries, &printer_id("nowhere")));
        assert!(material_installed(&libraries, &material_id("resin-in-b")));
        assert!(!material_installed(&libraries, &material_id("nowhere")));
    }

    #[test]
    fn a_reference_classifies_against_the_library_of_its_kind() {
        let root = tempfile::tempdir().expect("root");
        write_profile(root.path(), "printers/printer.toml", b"x = 1");
        let libraries = ProfileLibraries::new(Some(root.path().to_path_buf()), vec![]);
        let payload = ProjectPayload {
            printer: Some(printer_id("printer")),
            resin: Some(material_id("resin")),
            first_layer_height: None,
            output_format: None,
            embedded: Vec::new(),
        };
        assert_eq!(
            profile_ref_warnings(&payload, &libraries),
            vec![ProfileRefWarning::ResinMissing {
                id: material_id("resin"),
            }],
        );
    }

    #[test]
    fn the_first_root_wins_on_byte_resolution() {
        let a = tempfile::tempdir().expect("root a");
        let b = tempfile::tempdir().expect("root b");
        write_profile(a.path(), "printers/shared.toml", b"from = \"a\"");
        write_profile(b.path(), "printers/shared.toml", b"from = \"b\"");
        let libraries = libraries(a.path(), b.path());

        let mut payload = ProjectPayload {
            printer: Some(printer_id("shared")),
            resin: None,
            first_layer_height: None,
            output_format: None,
            embedded: Vec::new(),
        };
        complete_embedded_documents(&mut payload, &libraries);
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
        let libraries = ProfileLibraries::new(Some(root.path().to_path_buf()), vec![]);
        let mut payload = ProjectPayload {
            printer: Some(printer_id("nowhere")),
            resin: Some(material_id("nowhere")),
            first_layer_height: None,
            output_format: None,
            embedded: Vec::new(),
        };
        complete_embedded_documents(&mut payload, &libraries);
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
        let libraries = ProfileLibraries::new(Some(root.path().to_path_buf()), vec![]);

        let mut payload = ProjectPayload {
            printer: None,
            resin: Some(material_id("resin")),
            first_layer_height: None,
            output_format: None,
            embedded: Vec::new(),
        };
        complete_embedded_documents(&mut payload, &libraries);
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
        let libraries = ProfileLibraries::new(Some(root.path().to_path_buf()), vec![]);

        let mut payload = ProjectPayload {
            printer: Some(printer_id("printer")),
            resin: Some(material_id("resin")),
            first_layer_height: None,
            output_format: None,
            embedded: vec![EmbeddedProfile::Printer {
                id: printer_id("printer"),
                bytes: b"kind = \"held-printer\"".to_vec(),
            }],
        };
        complete_embedded_documents(&mut payload, &libraries);
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

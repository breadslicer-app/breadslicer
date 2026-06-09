/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests for the profile-library packaging (export and import).

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use tempfile::TempDir;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

use crate::package::{
    ExportError, ExportManifest, ExportSelection, ImportError, export_package, import_package,
};

const P1_TOML: &str = "# printer one\nlabel = \"p1\"\n";
const P2_TOML: &str = "# printer two\nlabel = \"p2\"\n";
const M1_TOML: &str = "# material one\nlabel = \"m1\"\n";
const M2_TOML: &str = "# material two\nlabel = \"m2\"\n";

/// Build a library with two printers and two materials. The file
/// contents are arbitrary: export copies raw bytes and never parses
/// them, and enumeration keys off the file-name stem.
fn write_library() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("printers")).unwrap();
    fs::create_dir_all(dir.path().join("materials")).unwrap();
    fs::write(dir.path().join("printers").join("p-one.toml"), P1_TOML).unwrap();
    fs::write(dir.path().join("printers").join("p-two.toml"), P2_TOML).unwrap();
    fs::write(dir.path().join("materials").join("m-one.toml"), M1_TOML).unwrap();
    fs::write(dir.path().join("materials").join("m-two.toml"), M2_TOML).unwrap();
    dir
}

fn manifest() -> ExportManifest {
    ExportManifest {
        name: None,
        description: None,
        author: None,
        exported_at: "2026-06-09T22:00:00+02:00".to_owned(),
        sc_slicer_version: "0.0.12".to_owned(),
    }
}

fn export_to_archive(
    library_dir: &Path,
    selection: &ExportSelection,
    manifest_fields: &ExportManifest,
) -> Result<ZipArchive<Cursor<Vec<u8>>>, ExportError> {
    let mut buf = Cursor::new(Vec::new());
    export_package(library_dir, selection, manifest_fields, &mut buf)?;
    Ok(ZipArchive::new(buf).unwrap())
}

fn entry_names(archive: &mut ZipArchive<Cursor<Vec<u8>>>) -> Vec<String> {
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_owned())
        .collect();
    names.sort();
    names
}

fn entry_text(archive: &mut ZipArchive<Cursor<Vec<u8>>>, name: &str) -> String {
    let mut file = archive.by_name(name).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    text
}

#[derive(serde::Deserialize)]
struct ManifestDoc {
    package: PackageDoc,
    contents: ContentsDoc,
}

#[derive(serde::Deserialize)]
struct PackageDoc {
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    exported_at: String,
    sc_slicer_version: String,
}

#[derive(serde::Deserialize)]
struct ContentsDoc {
    printers: Vec<String>,
    materials: Vec<String>,
}

fn parse_manifest(archive: &mut ZipArchive<Cursor<Vec<u8>>>) -> ManifestDoc {
    toml_edit::de::from_str(&entry_text(archive, "manifest.toml")).unwrap()
}

#[test]
fn export_all_writes_manifest_and_every_profile() {
    let dir = write_library();
    let mut archive = export_to_archive(dir.path(), &ExportSelection::All, &manifest()).unwrap();

    assert_eq!(
        entry_names(&mut archive),
        vec![
            "manifest.toml".to_owned(),
            "materials/m-one.toml".to_owned(),
            "materials/m-two.toml".to_owned(),
            "printers/p-one.toml".to_owned(),
            "printers/p-two.toml".to_owned(),
        ],
    );

    let doc = parse_manifest(&mut archive);
    assert_eq!(doc.contents.printers, vec!["p-one", "p-two"]);
    assert_eq!(doc.contents.materials, vec!["m-one", "m-two"]);
}

#[test]
fn exported_profile_bytes_are_the_raw_on_disk_bytes() {
    let dir = write_library();
    let mut archive = export_to_archive(dir.path(), &ExportSelection::All, &manifest()).unwrap();
    assert_eq!(entry_text(&mut archive, "printers/p-one.toml"), P1_TOML);
    assert_eq!(entry_text(&mut archive, "materials/m-two.toml"), M2_TOML);
}

#[test]
fn export_printers_omits_materials() {
    let dir = write_library();
    let mut archive =
        export_to_archive(dir.path(), &ExportSelection::Printers, &manifest()).unwrap();
    assert_eq!(
        entry_names(&mut archive),
        vec![
            "manifest.toml".to_owned(),
            "printers/p-one.toml".to_owned(),
            "printers/p-two.toml".to_owned(),
        ],
    );
    let doc = parse_manifest(&mut archive);
    assert_eq!(doc.contents.printers, vec!["p-one", "p-two"]);
    assert!(doc.contents.materials.is_empty());
}

#[test]
fn export_materials_omits_printers() {
    let dir = write_library();
    let mut archive =
        export_to_archive(dir.path(), &ExportSelection::Materials, &manifest()).unwrap();
    assert_eq!(
        entry_names(&mut archive),
        vec![
            "manifest.toml".to_owned(),
            "materials/m-one.toml".to_owned(),
            "materials/m-two.toml".to_owned(),
        ],
    );
    let doc = parse_manifest(&mut archive);
    assert!(doc.contents.printers.is_empty());
    assert_eq!(doc.contents.materials, vec!["m-one", "m-two"]);
}

#[test]
fn export_by_id_selects_only_named_profiles() {
    let dir = write_library();
    let selection = ExportSelection::Ids(vec!["p-two".to_owned(), "m-one".to_owned()]);
    let mut archive = export_to_archive(dir.path(), &selection, &manifest()).unwrap();
    assert_eq!(
        entry_names(&mut archive),
        vec![
            "manifest.toml".to_owned(),
            "materials/m-one.toml".to_owned(),
            "printers/p-two.toml".to_owned(),
        ],
    );
    let doc = parse_manifest(&mut archive);
    assert_eq!(doc.contents.printers, vec!["p-two"]);
    assert_eq!(doc.contents.materials, vec!["m-one"]);
}

#[test]
fn export_by_id_deduplicates_repeated_ids() {
    let dir = write_library();
    let selection = ExportSelection::Ids(vec!["p-one".to_owned(), "p-one".to_owned()]);
    let mut archive = export_to_archive(dir.path(), &selection, &manifest()).unwrap();
    assert_eq!(
        entry_names(&mut archive),
        vec!["manifest.toml".to_owned(), "printers/p-one.toml".to_owned()],
    );
}

#[test]
fn export_by_id_includes_both_kinds_when_an_id_matches_both() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("printers")).unwrap();
    fs::create_dir_all(dir.path().join("materials")).unwrap();
    fs::write(dir.path().join("printers").join("shared.toml"), P1_TOML).unwrap();
    fs::write(dir.path().join("materials").join("shared.toml"), M1_TOML).unwrap();

    let selection = ExportSelection::Ids(vec!["shared".to_owned()]);
    let mut archive = export_to_archive(dir.path(), &selection, &manifest()).unwrap();
    assert_eq!(
        entry_names(&mut archive),
        vec![
            "manifest.toml".to_owned(),
            "materials/shared.toml".to_owned(),
            "printers/shared.toml".to_owned(),
        ],
    );
}

#[test]
fn export_by_unknown_id_errors_naming_the_id() {
    let dir = write_library();
    let selection = ExportSelection::Ids(vec!["does-not-exist".to_owned()]);
    let err = export_to_archive(dir.path(), &selection, &manifest()).unwrap_err();
    match err {
        ExportError::UnknownId { id } => assert_eq!(id, "does-not-exist"),
        other => panic!("expected UnknownId, got {other:?}"),
    }
}

#[test]
fn export_by_invalid_id_errors_before_touching_the_filesystem() {
    let dir = write_library();
    // Uppercase is outside the slug grammar; an id that could traverse
    // the library (`../...`) is likewise rejected here.
    let selection = ExportSelection::Ids(vec!["../escape".to_owned()]);
    let err = export_to_archive(dir.path(), &selection, &manifest()).unwrap_err();
    assert!(
        matches!(err, ExportError::InvalidId { .. }),
        "expected InvalidId, got {err:?}",
    );
}

#[test]
fn manifest_omits_optional_fields_when_absent() {
    let dir = write_library();
    let mut archive = export_to_archive(dir.path(), &ExportSelection::All, &manifest()).unwrap();
    let text = entry_text(&mut archive, "manifest.toml");
    assert!(
        !text.contains("name ="),
        "manifest leaked an empty name: {text}"
    );
    assert!(
        !text.contains("description ="),
        "manifest leaked an empty description: {text}",
    );
    assert!(
        !text.contains("author ="),
        "manifest leaked an empty author: {text}",
    );

    let doc = parse_manifest(&mut archive);
    assert!(doc.package.name.is_none());
    assert!(doc.package.description.is_none());
    assert!(doc.package.author.is_none());
    assert_eq!(doc.package.exported_at, "2026-06-09T22:00:00+02:00");
    assert_eq!(doc.package.sc_slicer_version, "0.0.12");
}

#[test]
fn manifest_includes_optional_fields_when_present() {
    let dir = write_library();
    let fields = ExportManifest {
        name: Some("My bundle".to_owned()),
        description: Some("A curated set".to_owned()),
        author: Some("andreu".to_owned()),
        exported_at: "2026-06-09T22:00:00+02:00".to_owned(),
        sc_slicer_version: "0.0.12".to_owned(),
    };
    let mut archive = export_to_archive(dir.path(), &ExportSelection::All, &fields).unwrap();
    let doc = parse_manifest(&mut archive);
    assert_eq!(doc.package.name.as_deref(), Some("My bundle"));
    assert_eq!(doc.package.description.as_deref(), Some("A curated set"));
    assert_eq!(doc.package.author.as_deref(), Some("andreu"));
}

#[test]
fn export_empty_library_writes_only_an_empty_manifest() {
    let dir = TempDir::new().unwrap();
    let mut archive = export_to_archive(dir.path(), &ExportSelection::All, &manifest()).unwrap();
    assert_eq!(entry_names(&mut archive), vec!["manifest.toml".to_owned()]);
    let doc = parse_manifest(&mut archive);
    assert!(doc.contents.printers.is_empty());
    assert!(doc.contents.materials.is_empty());
}

/// Export the library at `src` into an in-memory archive ready to feed
/// back into [`import_package`].
fn package_of(src: &Path) -> Cursor<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    export_package(src, &ExportSelection::All, &manifest(), &mut buf).unwrap();
    buf.set_position(0);
    buf
}

fn read_dest(dest: &Path, kind: &str, id: &str) -> String {
    fs::read_to_string(dest.join(kind).join(format!("{id}.toml"))).unwrap()
}

#[test]
fn import_round_trips_an_exported_library_into_an_empty_destination() {
    let src = write_library();
    let package = package_of(src.path());
    let dest = TempDir::new().unwrap();

    import_package(package, dest.path(), false).unwrap();

    assert_eq!(read_dest(dest.path(), "printers", "p-one"), P1_TOML);
    assert_eq!(read_dest(dest.path(), "printers", "p-two"), P2_TOML);
    assert_eq!(read_dest(dest.path(), "materials", "m-one"), M1_TOML);
    assert_eq!(read_dest(dest.path(), "materials", "m-two"), M2_TOML);
    // The courtesy manifest is not installed into the library.
    assert!(!dest.path().join("manifest.toml").exists());
}

#[test]
fn import_without_force_reports_collisions_across_both_kinds_and_writes_nothing() {
    let src = write_library();
    let package = package_of(src.path());

    // Destination already holds one printer and one material that the
    // package also carries, plus distinct content so an overwrite would
    // be observable.
    let dest = TempDir::new().unwrap();
    fs::create_dir_all(dest.path().join("printers")).unwrap();
    fs::create_dir_all(dest.path().join("materials")).unwrap();
    fs::write(dest.path().join("printers").join("p-one.toml"), "OLD").unwrap();
    fs::write(dest.path().join("materials").join("m-two.toml"), "OLD").unwrap();

    let err = import_package(package, dest.path(), false).unwrap_err();
    let ImportError::Collisions(report) = err else {
        panic!("expected Collisions, got {err:?}");
    };
    assert_eq!(report.printers.len(), 1);
    assert_eq!(report.printers[0].id, "p-one");
    assert_eq!(
        report.printers[0].library_path,
        dest.path().join("printers").join("p-one.toml"),
    );
    assert_eq!(report.materials.len(), 1);
    assert_eq!(report.materials[0].id, "m-two");

    // Nothing was written: the existing files keep their old content and
    // no non-colliding profile was installed.
    assert_eq!(read_dest(dest.path(), "printers", "p-one"), "OLD");
    assert_eq!(read_dest(dest.path(), "materials", "m-two"), "OLD");
    assert!(!dest.path().join("printers").join("p-two.toml").exists());
    assert!(!dest.path().join("materials").join("m-one.toml").exists());
}

#[test]
fn import_with_force_overwrites_colliding_profiles() {
    let src = write_library();
    let package = package_of(src.path());

    let dest = TempDir::new().unwrap();
    fs::create_dir_all(dest.path().join("printers")).unwrap();
    fs::write(dest.path().join("printers").join("p-one.toml"), "OLD").unwrap();

    import_package(package, dest.path(), true).unwrap();

    assert_eq!(read_dest(dest.path(), "printers", "p-one"), P1_TOML);
    assert_eq!(read_dest(dest.path(), "printers", "p-two"), P2_TOML);
    assert_eq!(read_dest(dest.path(), "materials", "m-one"), M1_TOML);
}

/// Build a package by hand so we can plant entries an honest export
/// would never produce: a path-traversal name, a nested path, an
/// invalid slug, and one well-formed profile.
fn hostile_package() -> Cursor<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for name in [
            "../escape.toml",
            "printers/../escape.toml",
            "printers/sub/nested.toml",
            "printers/Bad-Slug.toml",
            "manifest.toml",
            "printers/good.toml",
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(b"x = 1\n").unwrap();
        }
        zip.finish().unwrap();
    }
    buf.set_position(0);
    buf
}

#[test]
fn import_ignores_non_conforming_and_traversing_entries() {
    let dest = TempDir::new().unwrap();
    import_package(hostile_package(), dest.path(), false).unwrap();

    // Only the single well-formed entry is installed.
    assert!(dest.path().join("printers").join("good.toml").is_file());
    assert!(!dest.path().join("printers").join("Bad-Slug.toml").exists());
    assert!(!dest.path().join("printers").join("sub").exists());
    assert!(!dest.path().join("manifest.toml").exists());
    // The traversal entry did not land anywhere around the destination.
    assert!(!dest.path().join("escape.toml").exists());
    assert!(!dest.path().parent().unwrap().join("escape.toml").exists());
}

#[test]
fn import_of_an_empty_archive_writes_nothing_and_succeeds() {
    let empty = TempDir::new().unwrap();
    let package = package_of(empty.path());
    let dest = TempDir::new().unwrap();
    import_package(package, dest.path(), false).unwrap();
    assert!(!dest.path().join("printers").exists());
    assert!(!dest.path().join("materials").exists());
}

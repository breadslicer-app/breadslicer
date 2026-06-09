/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use chrono::Local;
use sc_print_profiles::{
    ExportManifest, ExportSelection, export_package, list_material_ids, list_printer_ids,
    load_material_profile_from_library, load_printer_profile_from_library,
};

use crate::config::cmd_options::ListKind;
use crate::config::library_dir::resolve_user_library_dir;

/// Handle `profiles list [printers|materials]`: enumerate the user
/// library and print one tab-separated line per profile.
pub(crate) fn list(kind: ListKind) -> Result<()> {
    let library_dir = resolve_user_library_dir()
        .map_err(|e| anyhow!("could not determine the user profile library directory: {e}"))?;
    let mut out = std::io::stdout();
    list_to(&library_dir, kind, &mut out)
}

/// Write the listing for `kind` from `library_dir` to `out`. Columns
/// are tab-separated: `<id>\t<name>\t<summary>`. Printers are listed
/// before materials.
fn list_to(library_dir: &Path, kind: ListKind, out: &mut dyn Write) -> Result<()> {
    if matches!(kind, ListKind::All | ListKind::Printers) {
        for id in list_printer_ids(library_dir)? {
            let profile = load_printer_profile_from_library(library_dir, &id)
                .map_err(|e| anyhow!("could not load printer profile '{id}': {e}"))?;
            let build_volume = profile.hardware.build_volume();
            let resolution = profile.hardware.pixel_resolution();
            writeln!(
                out,
                "{}\t{}\t{} x {} x {} mm, {} x {} px",
                id,
                profile.identity.name.as_str(),
                build_volume.x_mm(),
                build_volume.y_mm(),
                build_volume.z_mm(),
                resolution.x(),
                resolution.y(),
            )?;
        }
    }
    if matches!(kind, ListKind::All | ListKind::Materials) {
        for id in list_material_ids(library_dir)? {
            let profile = load_material_profile_from_library(library_dir, &id)
                .map_err(|e| anyhow!("could not load material profile '{id}': {e}"))?;
            writeln!(
                out,
                "{}\t{}\t{}, {}",
                id,
                profile.identity.name.as_str(),
                profile.identity.brand.as_str(),
                profile.identity.family.as_str(),
            )?;
        }
    }
    Ok(())
}

/// Handle `profiles export <output.zip> [--printers|--materials|--id ...]`:
/// resolve the selection and package the user library into a zip.
pub(crate) fn export(
    output: OsString,
    printers: bool,
    materials: bool,
    ids: Vec<String>,
) -> Result<()> {
    let selection = build_selection(printers, materials, ids)?;
    let library_dir = resolve_user_library_dir()
        .map_err(|e| anyhow!("could not determine the user profile library directory: {e}"))?;

    let output_path = PathBuf::from(output);
    let file = File::create(&output_path).map_err(|e| {
        anyhow!(
            "could not create the output archive {}: {e}",
            output_path.display(),
        )
    })?;

    let manifest = ExportManifest {
        name: None,
        description: None,
        author: None,
        exported_at: Local::now().to_rfc3339(),
        sc_slicer_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    export_package(&library_dir, &selection, &manifest, file)
        .map_err(|e| anyhow!("could not export the profile package: {e}"))?;

    println!("Exported profiles to {}", output_path.display());
    Ok(())
}

/// Turn the parsed export flags into an [`ExportSelection`]. `--id` is
/// an explicit selection and may not be combined with the kind flags;
/// `--printers` and `--materials` together (or neither) select
/// everything.
fn build_selection(printers: bool, materials: bool, ids: Vec<String>) -> Result<ExportSelection> {
    if !ids.is_empty() {
        if printers || materials {
            return Err(anyhow!(
                "--id cannot be combined with --printers or --materials",
            ));
        }
        return Ok(ExportSelection::Ids(ids));
    }
    Ok(match (printers, materials) {
        (true, false) => ExportSelection::Printers,
        (false, true) => ExportSelection::Materials,
        (false, false) | (true, true) => ExportSelection::All,
    })
}

#[cfg(test)]
mod list_tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    const PRINTER_TOML: &str = r#"
[identity]
id = "test-printer"
manufacturer = "SoulCrafted"
family = "Test"
name = "Test Printer"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 5.0, y_mm = 5.0, z_mm = 50.0 }
pixel_resolution = { x = 500, y = 500 }
pixel_size_microns = { x = 10.0, y = 10.0 }
mirror = { x = false, y = false }

[capabilities]
output_formats = [{ format = "png_per_layer" }]
supports_grayscale = true
two_stage_lift = false
has_camera = false

[constraints]
"#;

    const MATERIAL_TOML: &str = r#"
[identity]
id = "test-resin"
brand = "SoulCrafted"
family = "Standard"
name = "Test Resin"
profile_version = "1.0.0"

[intent]

[defaults]
"#;

    fn write_library() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("printers")).unwrap();
        fs::create_dir_all(dir.path().join("materials")).unwrap();
        fs::write(
            dir.path().join("printers").join("test-printer.toml"),
            PRINTER_TOML,
        )
        .unwrap();
        fs::write(
            dir.path().join("materials").join("test-resin.toml"),
            MATERIAL_TOML,
        )
        .unwrap();
        dir
    }

    fn render(library_dir: &Path, kind: ListKind) -> String {
        let mut buf: Vec<u8> = Vec::new();
        list_to(library_dir, kind, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn list_all_prints_printers_then_materials_tab_separated() {
        let dir = write_library();
        let out = render(dir.path(), ListKind::All);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "expected one printer + one material: {out}");
        assert_eq!(
            lines[0],
            "test-printer\tTest Printer\t5 x 5 x 50 mm, 500 x 500 px",
        );
        assert_eq!(lines[1], "test-resin\tTest Resin\tSoulCrafted, Standard");
    }

    #[test]
    fn list_printers_omits_materials() {
        let dir = write_library();
        let out = render(dir.path(), ListKind::Printers);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("test-printer\t"));
    }

    #[test]
    fn list_materials_omits_printers() {
        let dir = write_library();
        let out = render(dir.path(), ListKind::Materials);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("test-resin\t"));
    }

    #[test]
    fn empty_library_prints_nothing() {
        let dir = TempDir::new().unwrap();
        let out = render(dir.path(), ListKind::All);
        assert!(out.is_empty(), "expected no output, got: {out:?}");
    }
}

#[cfg(test)]
mod build_selection_tests {
    use super::*;

    #[test]
    fn no_flags_selects_everything() {
        assert_eq!(
            build_selection(false, false, Vec::new()).unwrap(),
            ExportSelection::All,
        );
    }

    #[test]
    fn both_kind_flags_select_everything() {
        assert_eq!(
            build_selection(true, true, Vec::new()).unwrap(),
            ExportSelection::All,
        );
    }

    #[test]
    fn printers_flag_selects_printers() {
        assert_eq!(
            build_selection(true, false, Vec::new()).unwrap(),
            ExportSelection::Printers,
        );
    }

    #[test]
    fn materials_flag_selects_materials() {
        assert_eq!(
            build_selection(false, true, Vec::new()).unwrap(),
            ExportSelection::Materials,
        );
    }

    #[test]
    fn ids_select_those_ids() {
        assert_eq!(
            build_selection(false, false, vec!["a".to_owned(), "b".to_owned()]).unwrap(),
            ExportSelection::Ids(vec!["a".to_owned(), "b".to_owned()]),
        );
    }

    #[test]
    fn ids_combined_with_printers_is_an_error() {
        assert!(build_selection(true, false, vec!["a".to_owned()]).is_err());
    }

    #[test]
    fn ids_combined_with_materials_is_an_error() {
        assert!(build_selection(false, true, vec!["a".to_owned()]).is_err());
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! File > Import printer settings (#565): a printer export of Lychee
//! (`.lyp`) or Photon Workshop (`.pwsp`) the person picks becomes one
//! material profile per resin, in their own library, calibrated for the
//! printer of our catalog the file's printer is. Nothing else of the file
//! is kept, and the printer is never copied: its flags are ours.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use sc_print_profiles::import::{
    ImportedPrinter, matching_printer, material_profile, read_printer_settings,
};
use sc_print_profiles::writer::save_material_profile_to_library;
use sc_print_profiles::{PrinterProfile, ProfileLibraries};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, InfoTexts};
use crate::i18n::ctr;
use crate::preferences_bus::{PreferencesBus, PreferencesChange};
use crate::profile_catalog::{ProfileCatalog, refresh_catalog};

/// What an import did, for its card.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ImportReport {
    /// Each resin saved: its name and the printer it is set for.
    pub(crate) imported: Vec<(String, String)>,
    /// The file's printers that no printer of ours matches.
    pub(crate) unmatched: Vec<String>,
    /// Each resin's settings that have no place in our profiles.
    pub(crate) not_imported: Vec<(String, Vec<String>)>,
}

/// The catalog's printers whose plate is the imported one's, loaded.
fn candidates(catalog: &ProfileCatalog, printer: &ImportedPrinter) -> Vec<PrinterProfile> {
    let libraries = ProfileLibraries::platform_default();
    catalog
        .printers()
        .iter()
        .filter(|entry| {
            let (x, y) = (entry.build_volume.x_mm(), entry.build_volume.y_mm());
            (x - printer.plate_mm.0).abs() <= 0.02 * x && (y - printer.plate_mm.1).abs() <= 0.02 * y
        })
        .filter_map(|entry| libraries.load_printer_by_id(&entry.id).ok())
        .collect()
}

/// Imports the file at `path` into the library at `library`.
pub(crate) fn import_file(
    path: &Path,
    catalog: &ProfileCatalog,
    library: &Path,
) -> Result<ImportReport, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let source = if extension.eq_ignore_ascii_case("lyp") {
        "lychee"
    } else {
        "workshop"
    };
    let printers = read_printer_settings(extension, &bytes).map_err(|e| e.to_string())?;
    let mut report = ImportReport::default();
    for printer in &printers {
        let profiles = candidates(catalog, printer);
        let Some(id) = matching_printer(printer, &profiles) else {
            report.unmatched.push(printer.name.clone());
            continue;
        };
        let ours = profiles
            .iter()
            .find(|p| p.identity.id == id)
            .map_or_else(|| id.as_str().to_owned(), |p| p.identity.name.to_string());
        for resin in &printer.resins {
            let Some(profile) = material_profile(resin, &id, source) else {
                continue;
            };
            save_material_profile_to_library(library, &profile).map_err(|e| e.to_string())?;
            let name = format!("{} {}", profile.identity.brand, profile.identity.name);
            if !resin.not_imported.is_empty() {
                report
                    .not_imported
                    .push((name.clone(), resin.not_imported.clone()));
            }
            report.imported.push((name, ours.clone()));
        }
    }
    Ok(report)
}

/// The card for `report`.
fn report_texts(report: &ImportReport) -> InfoTexts {
    let mut details = String::new();
    for (resin, printer) in &report.imported {
        details.push_str(&format!("{resin} → {printer}\n"));
    }
    for printer in &report.unmatched {
        details.push_str(&ctr!(
            "PrinterSettingsImport",
            "{}: no printer of ours has its panel and plate.",
            printer
        ));
        details.push('\n');
    }
    for (resin, settings) in &report.not_imported {
        details.push_str(&ctr!(
            "PrinterSettingsImport",
            "{}: not imported, no place for them here: {}",
            resin,
            settings.join(", ")
        ));
        details.push('\n');
    }
    InfoTexts {
        heading: ctr!("PrinterSettingsImport", "Printer settings imported"),
        message: ctr!(
            "PrinterSettingsImport",
            "One resin was added to your resins.",
            "{n} resins were added to your resins.",
            report.imported.len()
        ),
        details: details.trim_end().into(),
    }
}

/// File > Import printer settings: picks the file, imports it, refreshes
/// the catalog, and says what it did.
pub(crate) fn wire(
    ui: &MainWindow,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    prefs: Rc<PreferencesBus>,
    dialogs: Rc<DialogManager>,
) {
    let weak = ui.as_weak();
    ui.on_import_printer_settings_requested(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let dialog = crate::error_dialog::file_dialog_over(ui.window())
            .set_title(ctr!("FileDialog", "Import printer settings").to_string())
            .add_filter(
                ctr!("FileDialog", "Lychee or Photon Workshop printer settings").to_string(),
                &["lyp", "pwsp"],
            );
        let (catalog, prefs, dialogs) = (catalog.clone(), prefs.clone(), dialogs.clone());
        if let Err(err) = slint::spawn_local(async move {
            let Some(picked) = dialog.pick_file().await else {
                return;
            };
            let Some(library) = ProfileLibraries::platform_default()
                .user_library_dir()
                .map(Path::to_path_buf)
            else {
                return;
            };
            let current = catalog.borrow().clone();
            match import_file(picked.path(), &current, &library) {
                Ok(report) => {
                    refresh_catalog(&catalog);
                    prefs.update(|_| PreferencesChange::Profiles);
                    dialogs.show_info(move || report_texts(&report));
                }
                Err(why) => dialogs.show_info(move || InfoTexts {
                    heading: ctr!("PrinterSettingsImport", "Printer settings not imported"),
                    message: ctr!(
                        "PrinterSettingsImport",
                        "The file was not imported: {}",
                        why.as_str()
                    ),
                    details: Default::default(),
                }),
            }
        }) {
            eprintln!("Failed to spawn the printer settings dialog task: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #565: a Photon Workshop export for the M7 Pro lands as resins of
    /// the user's library set for our M7 Pro, saying what had no place.
    #[test]
    fn a_workshop_export_lands_as_resins_for_our_printer() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("m7.pwsp");
        std::fs::write(
            &file,
            r#"{"printer_param": {
                "machine_type": {"name": "Anycubic Photon Mono M7 Pro", "res_x": 13312,
                                 "res_y": 5120, "print_xsize": 223.642, "print_ysize": 126.48},
                "machine_extern": {"user_resins": [{
                    "property": {"brand_name": "Anycubic", "resin_name": "Standard",
                                 "setting_name": "Normal"},
                    "slicepara": {"exposure_time": 1.8, "bott_time": 20, "bott_layers": 5,
                                  "zthick": 0.05, "zup_height": 6, "zup_speed": 2,
                                  "zdown_speed": 3},
                    "slice_extpara": {"transition_layercount": 10}
                }]}}}"#,
        )
        .unwrap();
        let library = dir.path().join("library");
        let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::platform_default());
        let report = import_file(&file, &catalog, &library).expect("it imports");
        assert_eq!(report.imported.len(), 1, "{report:?}");
        assert!(report.unmatched.is_empty());
        assert_eq!(report.not_imported[0].1, vec!["transition_layercount"]);
        let saved = library
            .join("materials")
            .join("workshop-anycubic-standard-normal.toml");
        let text = std::fs::read_to_string(saved).expect("the resin is saved");
        assert!(
            text.contains("[per_printer.anycubic-photon-mono-m7-pro]"),
            "{text}"
        );
        assert!(text.contains("lift_speed_mm_min = 120.0"), "{text}");
    }

    #[test]
    fn a_file_of_another_kind_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.lyp");
        std::fs::write(&file, b"nothing").unwrap();
        let catalog = ProfileCatalog::from_entries(Vec::new(), Vec::new());
        assert!(import_file(&file, &catalog, dir.path()).is_err());
    }
}

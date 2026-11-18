/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The form that adds a printer by hand, or edits one the user added (#600),
//! which the Preferences window opens.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use sc_print_primitives::PrinterId;
use sc_print_profiles::{
    PrinterProfile, ProfileLibraries, ProfileSource, printer_profile_path_in_library,
    save_printer_profile_to_library, user_library_dir,
};
use sc_sla_formats::PrinterFileFormat;

use crate::i18n::ctr;
use crate::preferences_bus::{PreferencesBus, PreferencesChange};
use crate::print_format::format_label;
use crate::printer_form::{
    PrinterDraft, PrinterFormError, build_printer, draft_of, panel_of, trim_number,
    writable_formats,
};
use crate::profile_catalog::{ProfileCatalog, refresh_catalog};
use crate::profile_transfer::overwrite_confirmed;
use crate::slint_ui::{PreferencesWindow, PrinterDialog};
use crate::window_prefs::WindowPreferences;

use super::preferences_follow::follow_preferences;

/// What a confirm does: add a printer, or save the one the user added.
#[derive(Clone)]
enum Purpose {
    Add,
    Edit(PrinterProfile),
}

/// Opens the form. With `edit`, it opens on `printer`, or the selected one,
/// when the user added it; otherwise it adds a new printer, starting from it.
pub(super) fn open_printer_dialog(
    owner: &slint::Weak<PreferencesWindow>,
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    slot: &Rc<RefCell<Option<PrinterDialog>>>,
    edit: bool,
    printer: Option<&PrinterId>,
) {
    let catalog = catalog_handle.borrow().clone();
    let libraries = ProfileLibraries::platform_default();
    let selected = printer
        .cloned()
        .or_else(|| catalog.effective_default_printer(bus.snapshot().default_printer.as_ref()))
        .and_then(|id| catalog.printer_index(&id).map(|i| (id, i)));
    let purpose = match (edit, &selected) {
        (true, Some((id, index))) if catalog.printers()[*index].source == ProfileSource::User => {
            match libraries.load_printer_by_id(id) {
                Ok(profile) => Purpose::Edit(profile),
                Err(err) => {
                    eprintln!("cannot open printer {id}: {err}");
                    return;
                }
            }
        }
        _ => Purpose::Add,
    };

    let dialog = match PrinterDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the printer dialog: {err}");
            return;
        }
    };
    crate::window_chrome::wire_framed!(&dialog);
    dialog.apply_preferences(&bus.snapshot());

    let formats = writable_formats();
    dialog.set_format_names(ModelRc::new(VecModel::from(
        formats
            .iter()
            .map(|f| SharedString::from(format_label(*f)))
            .collect::<Vec<_>>(),
    )));
    // The printers to start from, after the empty form.
    let starts: Vec<PrinterId> = catalog.printers().iter().map(|e| e.id.clone()).collect();
    let mut names = vec![SharedString::from(ctr!("PrinterDialog", "Empty form"))];
    names.extend(
        catalog
            .printers()
            .iter()
            .map(|e| SharedString::from(format!("{} {}", e.manufacturer, e.name))),
    );
    dialog.set_start_from_names(ModelRc::new(VecModel::from(names)));

    let base: Rc<RefCell<Option<PrinterProfile>>> = Rc::default();
    match &purpose {
        Purpose::Edit(profile) => {
            dialog.set_editing(true);
            fill(&dialog, &draft_of(profile), &formats);
            *base.borrow_mut() = Some(profile.clone());
        }
        Purpose::Add => {
            dialog.set_editing(false);
            let start = selected.as_ref().and_then(|(id, _)| {
                let position = starts.iter().position(|s| s == id)?;
                libraries.load_printer_by_id(id).ok().map(|p| (position, p))
            });
            if let Some((position, profile)) = start {
                dialog.set_start_from_index(position as i32 + 1);
                start_from(&dialog, &profile, &formats);
                *base.borrow_mut() = Some(profile);
            } else {
                fill(&dialog, &PrinterDraft::default(), &formats);
            }
        }
    }
    show_summary(&dialog);

    let error: Rc<RefCell<Option<PrinterFormError>>> = Rc::default();
    follow_preferences(bus, &dialog, {
        let error = error.clone();
        move |dialog| {
            if let Some(err) = error.borrow().as_ref() {
                dialog.set_error_message(err.to_string().into());
            }
            show_summary(dialog);
        }
    });

    let weak = dialog.as_weak();
    dialog.on_fields_edited({
        let weak = weak.clone();
        move || {
            if let Some(d) = weak.upgrade() {
                show_summary(&d);
            }
        }
    });

    dialog.on_start_from_picked({
        let weak = weak.clone();
        let base = base.clone();
        let formats = formats.clone();
        move |index| {
            let Some(d) = weak.upgrade() else {
                return;
            };
            let picked = usize::try_from(index - 1)
                .ok()
                .and_then(|i| starts.get(i))
                .and_then(|id| {
                    ProfileLibraries::platform_default()
                        .load_printer_by_id(id)
                        .ok()
                });
            match picked {
                Some(profile) => {
                    start_from(&d, &profile, &formats);
                    *base.borrow_mut() = Some(profile);
                }
                None => {
                    fill(&d, &PrinterDraft::default(), &formats);
                    *base.borrow_mut() = None;
                }
            }
            d.set_error_message(SharedString::new());
            show_summary(&d);
        }
    });

    dialog.on_confirm_requested({
        let weak = weak.clone();
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let slot = slot.clone();
        let purpose = purpose.clone();
        let error = error.clone();
        let formats = formats.clone();
        move || {
            let Some(d) = weak.upgrade() else {
                return;
            };
            let catalog = catalog_handle.borrow().clone();
            let taken: BTreeSet<PrinterId> =
                catalog.printers().iter().map(|e| e.id.clone()).collect();
            let editing = match &purpose {
                Purpose::Edit(profile) => Some(profile.identity.id.clone()),
                Purpose::Add => None,
            };
            let built = build_printer(
                &draft_from(&d, &formats),
                base.borrow().as_ref(),
                editing.as_ref(),
                &taken,
            );
            let printer = match built {
                Ok(printer) => printer,
                Err(err) => {
                    d.set_error_message(err.to_string().into());
                    *error.borrow_mut() = Some(err);
                    return;
                }
            };
            let saved = user_library_dir()
                .map_err(|err| err.to_string())
                .and_then(|dir| {
                    save_printer_profile_to_library(&dir, &printer).map_err(|e| e.to_string())
                });
            if let Err(message) = saved {
                d.set_error_message(message.into());
                return;
            }
            refresh_catalog(&catalog_handle);
            let id = printer.identity.id.clone();
            bus.update(move |snap| {
                snap.default_printer = Some(id);
                PreferencesChange::DefaultPrinter
            });
            refresh_view();
            let _ = d.hide();
            *slot.borrow_mut() = None;
        }
    });

    dialog.on_delete_requested({
        let weak = weak.clone();
        let owner = owner.clone();
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let slot = slot.clone();
        move || {
            let Purpose::Edit(profile) = &purpose else {
                return;
            };
            spawn_delete(Deletion {
                dialog: weak.clone(),
                owner: owner.clone(),
                bus: bus.clone(),
                catalog_handle: catalog_handle.clone(),
                refresh_view: refresh_view.clone(),
                slot: slot.clone(),
                profile: profile.clone(),
            });
        }
    });

    dialog.on_cancel_requested({
        let weak = weak.clone();
        let slot = slot.clone();
        move || {
            if let Some(d) = weak.upgrade() {
                let _ = d.hide();
            }
            *slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the printer dialog: {err}");
        return;
    }
    super::centre_over_preferences(&dialog);
    *slot.borrow_mut() = Some(dialog);
}

/// What deleting a printer the user added needs.
struct Deletion {
    dialog: slint::Weak<PrinterDialog>,
    owner: slint::Weak<PreferencesWindow>,
    bus: Rc<PreferencesBus>,
    catalog_handle: Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: Rc<dyn Fn()>,
    slot: Rc<RefCell<Option<PrinterDialog>>>,
    profile: PrinterProfile,
}

/// Asks before deleting the printer the user added, then deletes it.
fn spawn_delete(deletion: Deletion) {
    let spawned = slint::spawn_local(async move {
        let Deletion {
            dialog,
            owner,
            bus,
            catalog_handle,
            refresh_view,
            slot,
            profile,
        } = deletion;
        let Ok(user_lib) = user_library_dir() else {
            return;
        };
        let name = format!(
            "{} {}",
            profile.identity.manufacturer.as_str(),
            profile.identity.name.as_str()
        );
        let delete_label = ctr!("PreferencesPage", "Delete").to_string();
        let result = super::message_dialog_of(&owner)
            .set_title(ctr!("PrinterDialog", "Delete printer?").to_string())
            .set_description(
                ctr!(
                    "PrinterDialog",
                    "Deleting {} removes the printer you added. Resins keep their settings for it.",
                    name
                )
                .to_string(),
            )
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                delete_label.clone(),
                ctr!("PreferencesPage", "Cancel").to_string(),
            ))
            .show()
            .await;
        if !overwrite_confirmed(&result, &delete_label) {
            return;
        }
        let path = printer_profile_path_in_library(&user_lib, &profile.identity.id);
        if let Err(err) = std::fs::remove_file(&path) {
            if let Some(d) = dialog.upgrade() {
                d.set_error_message(format!("{}: {err}", path.display()).into());
            }
            return;
        }
        refresh_catalog(&catalog_handle);
        if bus.snapshot().default_printer.as_ref() == Some(&profile.identity.id) {
            bus.update(|snap| {
                snap.default_printer = None;
                PreferencesChange::DefaultPrinter
            });
        }
        refresh_view();
        if let Some(d) = dialog.upgrade() {
            let _ = d.hide();
        }
        *slot.borrow_mut() = None;
    });
    if let Err(err) = spawned {
        eprintln!("failed to spawn the printer delete task: {err}");
    }
}

/// Fills the form to add a copy of `profile`: its numbers, and its name marked
/// as a copy, since a printer of the same name exists.
fn start_from(dialog: &PrinterDialog, profile: &PrinterProfile, formats: &[PrinterFileFormat]) {
    let mut draft = draft_of(profile);
    draft.name = ctr!("PrinterDialog", "{} (copy)", draft.name).to_string();
    fill(dialog, &draft, formats);
}

fn fill(dialog: &PrinterDialog, draft: &PrinterDraft, formats: &[PrinterFileFormat]) {
    dialog.set_manufacturer(draft.manufacturer.clone().into());
    dialog.set_model_name(draft.name.clone().into());
    dialog.set_resolution_x(draft.resolution_x.clone().into());
    dialog.set_resolution_y(draft.resolution_y.clone().into());
    dialog.set_pixel_x(draft.pixel_x_um.clone().into());
    dialog.set_pixel_y(draft.pixel_y_um.clone().into());
    dialog.set_plate_x(draft.plate_x_mm.clone().into());
    dialog.set_plate_y(draft.plate_y_mm.clone().into());
    dialog.set_build_height(draft.height_mm.clone().into());
    dialog.set_mirror_x(draft.mirror_x);
    dialog.set_mirror_y(draft.mirror_y);
    dialog.set_turned(draft.turned);
    // An empty form starts with grey levels on, as nearly every printer has.
    dialog.set_grayscale(draft.grayscale || draft == &PrinterDraft::default());
    let index = draft
        .format
        .and_then(|f| formats.iter().position(|g| *g == f))
        .unwrap_or(0);
    dialog.set_format_index(index as i32);
}

fn draft_from(dialog: &PrinterDialog, formats: &[PrinterFileFormat]) -> PrinterDraft {
    PrinterDraft {
        manufacturer: dialog.get_manufacturer().into(),
        name: dialog.get_model_name().into(),
        resolution_x: dialog.get_resolution_x().into(),
        resolution_y: dialog.get_resolution_y().into(),
        pixel_x_um: dialog.get_pixel_x().into(),
        pixel_y_um: dialog.get_pixel_y().into(),
        plate_x_mm: dialog.get_plate_x().into(),
        plate_y_mm: dialog.get_plate_y().into(),
        height_mm: dialog.get_build_height().into(),
        mirror_x: dialog.get_mirror_x(),
        mirror_y: dialog.get_mirror_y(),
        turned: dialog.get_turned(),
        grayscale: dialog.get_grayscale(),
        format: usize::try_from(dialog.get_format_index())
            .ok()
            .and_then(|i| formats.get(i))
            .copied(),
    }
}

/// Shows what the panel fields make, as they are typed.
fn show_summary(dialog: &PrinterDialog) {
    let draft = draft_from(dialog, &writable_formats());
    let text = match panel_of(&draft) {
        (Some(x), Some(y)) => ctr!(
            "PrinterDialog",
            "Plate {} × {} mm, pixels of {} × {} µm.",
            trim_number(x.plate_mm),
            trim_number(y.plate_mm),
            trim_number(x.pixel_um),
            trim_number(y.pixel_um)
        )
        .to_string(),
        _ => String::new(),
    };
    dialog.set_summary(text.into());
}

#[cfg(test)]
mod printer_dialog_tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use sc_print_profiles::load_printer_profile_from_path;

    use super::*;
    use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};

    fn m7_pro() -> PrinterProfile {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/profiles/printers/anycubic-photon-mono-m7-pro.toml");
        load_printer_profile_from_path(&path).unwrap()
    }

    #[test]
    fn a_form_started_from_a_printer_reads_back_as_that_machine() {
        on_fresh_slint_thread(|| {
            sc_sla_formats_uvtools::install();
            let dialog = PrinterDialog::new().expect("a printer dialog");
            let formats = writable_formats();
            dialog.set_format_names(ModelRc::new(VecModel::from(
                formats
                    .iter()
                    .map(|f| SharedString::from(format_label(*f)))
                    .collect::<Vec<_>>(),
            )));
            let source = m7_pro();
            start_from(&dialog, &source, &formats);
            show_summary(&dialog);
            assert!(dialog.get_model_name().ends_with("(copy)"));
            assert!(
                dialog.get_summary().contains("223.642"),
                "{}",
                dialog.get_summary()
            );
            let built = build_printer(
                &draft_from(&dialog, &formats),
                Some(&source),
                None,
                &BTreeSet::new(),
            )
            .unwrap();
            assert_eq!(built.hardware, source.hardware);
            assert_eq!(
                built.capabilities.output_formats,
                source.capabilities.output_formats
            );

            // Typing a resolution with the pixel size left empty shows the
            // plate it makes.
            dialog.set_pixel_x(SharedString::new());
            dialog.set_plate_x("".into());
            dialog.set_pixel_x("20".into());
            dialog.set_resolution_x("10000".into());
            show_summary(&dialog);
            assert!(
                dialog.get_summary().contains("200 "),
                "{}",
                dialog.get_summary()
            );
        });
    }

    #[test]
    fn the_picture_of_the_form_is_written_when_asked() {
        let Some(dir) = std::env::var_os("BREADSLICER_UI_SHOTS").map(PathBuf::from) else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();
        for language in ["en", "es"] {
            let dir = dir.clone();
            on_fresh_slint_thread(move || {
                sc_sla_formats_uvtools::install();
                let dialog = PrinterDialog::new().expect("a printer dialog");
                slint::select_bundled_translation(language).expect("bundled language");
                dialog.set_theme(crate::ThemePreference::Dark);
                let formats = writable_formats();
                dialog.set_format_names(ModelRc::new(VecModel::from(
                    formats
                        .iter()
                        .map(|f| SharedString::from(format_label(*f)))
                        .collect::<Vec<_>>(),
                )));
                dialog.set_start_from_names(ModelRc::new(VecModel::from(vec![
                    SharedString::from("Empty form"),
                    SharedString::from("Anycubic Photon Mono M7 Pro"),
                ])));
                dialog.set_start_from_index(1);
                start_from(&dialog, &m7_pro(), &formats);
                show_summary(&dialog);
                dialog.show().unwrap();
                render_frames();
                let shot = dialog.window().take_snapshot().unwrap();
                image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
                    .unwrap()
                    .save(dir.join(format!("{language}-printer-add.png")))
                    .unwrap();
            });
        }
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The dialog that forks a resin profile into the user library, which the
//! Preferences window opens.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, PlatformError, SharedString, VecModel};

use sc_print_primitives::MaterialId;
use sc_print_profiles::{CalibrationTarget, MaterialProfile, user_library_dir};

use crate::i18n::ctr;
use crate::material_form::{EditedProfileKind, save_edited_profile};
use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, load_profile_for_view, refresh_catalog};
use crate::profile_forking::{ForkError, ForkParts, build_fork};
use crate::profile_view::{ordered_calibration_targets, profile_display_name};
use crate::slint_ui::{ForkBlock, ForkDialog, ProfileViewDialog};
use crate::window_prefs::WindowPreferences;

use super::preferences_follow::{ErrorLine, follow_preferences};
use super::profile_editor::{EditorNote, open_profile_editor};

/// Opens the fork dialog for the resin at `index`. The source is the resolved
/// profile behind the row (an extension row forks the same resolved base); the
/// offered parts are strictly the source's own blocks. On confirm the fork is
/// written to the user library and the in-place editor opens on it, with a note
/// that the profile is already saved so cancelling there keeps the untouched
/// fork.
pub(super) fn open_fork_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    fork_slot: &Rc<RefCell<Option<ForkDialog>>>,
    editor_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((source, _, _)) = load_profile_for_view(&catalog, index) else {
        return;
    };

    let dialog = match build_fork_dialog(
        bus,
        catalog_handle,
        refresh_view,
        fork_slot,
        editor_slot,
        source,
    ) {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the fork dialog: {err}");
            return;
        }
    };

    let dialog_weak = dialog.as_weak();
    dialog.on_cancel_requested({
        let fork_slot = fork_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *fork_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the fork dialog: {err}");
        return;
    }
    *fork_slot.borrow_mut() = Some(dialog);
}

/// Builds the fork dialog for `source`, with its error line in the language in
/// force while the dialog exists. A confirm that succeeds writes the fork,
/// empties `fork_slot` and opens the editor on the fork in `editor_slot`.
fn build_fork_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    fork_slot: &Rc<RefCell<Option<ForkDialog>>>,
    editor_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    source: MaterialProfile,
) -> Result<ForkDialog, PlatformError> {
    let catalog = catalog_handle.borrow().clone();
    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    let targets =
        ordered_calibration_targets(&source, catalog.printers(), default_printer.as_ref());

    let dialog = ForkDialog::new()?;
    dialog.apply_preferences(&snapshot);
    dialog.set_source_name(profile_display_name(&source.identity).into());
    dialog.set_fork_name(ctr!(
        "ForkDialog",
        "{} (fork)",
        source.identity.name.as_str()
    ));
    dialog.set_fork_brand(source.identity.brand.as_str().into());
    dialog.set_blocks(ModelRc::new(VecModel::from(fork_blocks(&targets))));

    // The checkboxes' state, mirrored from the dialog; every part starts
    // selected, matching the checkboxes' initial state.
    let blocks_checked = Rc::new(RefCell::new(vec![true; targets.len()]));
    dialog.on_block_toggled({
        let blocks_checked = blocks_checked.clone();
        move |index, checked| {
            if let Some(slot) = blocks_checked.borrow_mut().get_mut(index as usize) {
                *slot = checked;
            }
        }
    });

    let error_line: Rc<RefCell<Option<ErrorLine<ForkError>>>> = Rc::default();
    follow_preferences(bus, &dialog, {
        let error_line = error_line.clone();
        move |dialog| {
            if let Some(line) = error_line.borrow().as_ref() {
                dialog.set_error_message(line.text());
            }
        }
    });
    let dialog_weak = dialog.as_weak();

    dialog.on_confirm_requested({
        let fork_slot = fork_slot.clone();
        let editor_slot = editor_slot.clone();
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let catalog = catalog_handle.borrow().clone();
            let existing: BTreeSet<MaterialId> = catalog
                .materials()
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            let parts = ForkParts {
                intent: d.get_copy_intent(),
                defaults: d.get_copy_defaults(),
                blocks: targets
                    .iter()
                    .zip(blocks_checked.borrow().iter())
                    .filter(|(_, checked)| **checked)
                    .map(|((id, _), _)| id.clone())
                    .collect(),
            };
            let fork = match build_fork(
                &source,
                &parts,
                &d.get_fork_name(),
                &d.get_fork_brand(),
                &existing,
            ) {
                Ok(fork) => fork,
                Err(err) => {
                    show_error(&d, &error_line, ErrorLine::Translated(err));
                    return;
                }
            };
            let user_lib = match user_library_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    show_error(
                        &d,
                        &error_line,
                        ErrorLine::Untranslated(err.to_string().into()),
                    );
                    return;
                }
            };
            if let Err(message) = save_edited_profile(&user_lib, &fork, EditedProfileKind::Full) {
                show_error(&d, &error_line, ErrorLine::Untranslated(message.into()));
                return;
            }
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *fork_slot.borrow_mut() = None;

            let catalog = catalog_handle.borrow().clone();
            let Some(fork_index) = catalog.material_index(&fork.identity.id) else {
                return;
            };
            open_profile_editor(
                &bus,
                &catalog_handle,
                &refresh_view,
                &editor_slot,
                fork_index as i32,
                false,
                Some(EditorNote::ForkSaved),
            );
        }
    });

    Ok(dialog)
}

/// The rows of the dialog's block checkboxes, 1 for each of `targets`, in their
/// order.
fn fork_blocks(targets: &[(CalibrationTarget, SharedString)]) -> Vec<ForkBlock> {
    targets
        .iter()
        .map(|(target, label)| match target {
            CalibrationTarget::Generic => ForkBlock {
                generic: true,
                printer_name: SharedString::new(),
            },
            CalibrationTarget::Printer(_) => ForkBlock {
                generic: false,
                printer_name: label.clone(),
            },
        })
        .collect()
}

/// Shows `line` in the fork dialog's error line and keeps it in `kept`, so that
/// a language change writes the line again.
fn show_error(
    dialog: &ForkDialog,
    kept: &RefCell<Option<ErrorLine<ForkError>>>,
    line: ErrorLine<ForkError>,
) {
    dialog.set_error_message(line.text());
    *kept.borrow_mut() = Some(line);
}

#[cfg(test)]
mod fork_blocks_tests {
    use sc_print_primitives::PrinterId;

    use super::*;

    #[test]
    fn the_generic_block_is_a_flag_and_a_printer_block_its_name() {
        let printer = PrinterId::new("printer-a").expect("valid printer id");
        let targets = [
            (
                CalibrationTarget::from_printer(&printer),
                SharedString::from("Printer A"),
            ),
            (CalibrationTarget::Generic, SharedString::from("Generic")),
        ];
        let expected = [
            ForkBlock {
                generic: false,
                printer_name: "Printer A".into(),
            },
            ForkBlock {
                generic: true,
                printer_name: SharedString::new(),
            },
        ];
        assert_eq!(fork_blocks(&targets), expected);
    }
}

#[cfg(test)]
mod language_change_tests {
    use slint::Model;

    use super::*;
    use crate::language::InstalledTranslations;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::ErrorDialog;
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::material_profiles::resin_profile;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::ui_language::UiLanguage;

    /// A fork dialog for the resin profile, built after the UI language follows
    /// `bus`, as in the app.
    pub(super) fn following_fork_dialog(bus: &Rc<PreferencesBus>) -> (UiLanguage, ForkDialog) {
        // In the app, the 1st main window is the 1st component of the thread.
        let first = ErrorDialog::new().expect("the headless platform creates components");
        let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&first), bus);
        let catalog = ProfileCatalog::from_entries(Vec::new(), Vec::new());
        let refresh_view: Rc<dyn Fn()> = Rc::new(|| {});
        let dialog = build_fork_dialog(
            bus,
            &Rc::new(RefCell::new(Rc::new(catalog))),
            &refresh_view,
            &Rc::new(RefCell::new(None)),
            &Rc::new(RefCell::new(None)),
            resin_profile(),
        )
        .expect("the headless platform creates the fork dialog");
        (ui_language, dialog)
    }

    #[test]
    fn a_language_change_pushes_a_fork_error_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_fork_dialog(&bus);
            dialog.set_fork_name(SharedString::new());
            dialog.invoke_confirm_requested();
            dialog.set_error_message("sentinel".into());

            pick_language(&bus, "es");

            let expected = ForkError::EmptyName.user_message().to_string();
            (dialog.get_error_message().to_string(), expected)
        });
        assert_eq!(shown, expected);
    }

    #[test]
    fn an_edited_fork_name_keeps_its_text() {
        let name = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_fork_dialog(&bus);
            dialog.set_fork_name("My resin".into());
            pick_language(&bus, "es");
            dialog.get_fork_name().to_string()
        });
        assert_eq!(name, "My resin");
    }

    #[test]
    fn the_source_blocks_reach_the_dialog_as_data() {
        let blocks = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_fork_dialog(&bus);
            dialog.get_blocks().iter().collect::<Vec<_>>()
        });
        let generic = ForkBlock {
            generic: true,
            printer_name: SharedString::new(),
        };
        assert_eq!(blocks, [generic]);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::ThemePreference;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::test_support::theme_pick::pick_theme;

    #[test]
    fn the_fork_dialog_takes_the_theme_and_follows_its_changes() {
        let themes = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            pick_theme(&bus, ThemePreference::Dark);
            let (_ui_language, dialog) = language_change_tests::following_fork_dialog(&bus);
            let at_open = dialog.get_theme();
            pick_theme(&bus, ThemePreference::Light);
            (at_open, dialog.get_theme())
        });
        assert_eq!(themes, (ThemePreference::Dark, ThemePreference::Light));
    }
}

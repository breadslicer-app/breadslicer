/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The dialog that adds a calibration block to a resin through its extension
//! file, which the Preferences window opens.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, PlatformError, VecModel};

use sc_print_profiles::{EffectiveMaterial, MaterialProfile, user_library_dir};

use crate::material_form::{
    CalibrationFormInput, EditedProfileKind, InvalidFormValue, calibration_form_from_block,
    parse_calibration_form, save_edited_profile,
};
use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, load_profile_for_view, refresh_catalog};
use crate::profile_extending::{build_extension_update, copy_sources, uncovered_targets};
use crate::profile_view::profile_display_name;
use crate::slint_ui::ExtendDialog;
use crate::window_prefs::WindowPreferences;

use super::preferences_follow::{ErrorLine, follow_preferences, relabel_picker};

/// Opens the extend dialog for the resin at `index`: pick one of the targets
/// the resin does not cover yet, enter the calibration values (partial
/// allowed), and write the block into the resin's extension file (created on
/// the first extend, appended to afterwards).
pub(super) fn open_extend_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    extend_slot: &Rc<RefCell<Option<ExtendDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((base, extension, _)) = load_profile_for_view(&catalog, index) else {
        return;
    };

    let dialog = match build_extend_dialog(
        bus,
        catalog_handle,
        refresh_view,
        extend_slot,
        base,
        extension,
    ) {
        Ok(Some(dialog)) => dialog,
        Ok(None) => return,
        Err(err) => {
            eprintln!("failed to create the extend dialog: {err}");
            return;
        }
    };

    let dialog_weak = dialog.as_weak();
    dialog.on_cancel_requested({
        let extend_slot = extend_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *extend_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the extend dialog: {err}");
        return;
    }
    *extend_slot.borrow_mut() = Some(dialog);
}

/// Builds the extend dialog for the resin `base` and its `extension`, with the
/// labels of its pickers and its error line in the language in force while the
/// dialog exists; `None` when the resin covers every target. A save that
/// succeeds writes the block, refreshes the catalog and the view, and empties
/// `extend_slot`.
fn build_extend_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    extend_slot: &Rc<RefCell<Option<ExtendDialog>>>,
    base: MaterialProfile,
    extension: Option<MaterialProfile>,
) -> Result<Option<ExtendDialog>, PlatformError> {
    let catalog = catalog_handle.borrow().clone();
    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    // The dialog writes to the raw files, so `base` and `extension` stay
    // around; the composed view feeds the target complement and the blocks a
    // new one can be copied from.
    let (material, issues) = EffectiveMaterial::compose(base.clone(), extension.clone());
    for issue in &issues {
        eprintln!("warning: {issue}");
    }
    let targets = uncovered_targets(&material, catalog.printers(), default_printer.as_ref());
    if targets.is_empty() {
        return Ok(None);
    }
    let sources = copy_sources(&material, catalog.printers());

    let dialog = ExtendDialog::new()?;
    dialog.apply_preferences(&snapshot);
    dialog.set_source_name(profile_display_name(&base.identity).into());
    dialog.set_target_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_target_index(-1);
    dialog.set_copy_source_labels(ModelRc::new(VecModel::from(
        sources
            .iter()
            .map(|source| source.label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_copy_source_index(-1);

    let error_line: Rc<RefCell<Option<ErrorLine<InvalidFormValue>>>> = Rc::default();
    follow_preferences(bus, &dialog, {
        let error_line = error_line.clone();
        move |dialog| {
            let printers = catalog.printers();
            let targets = uncovered_targets(&material, printers, default_printer.as_ref());
            relabel_picker(
                &dialog.get_target_labels(),
                dialog.get_target_index(),
                targets.into_iter().map(|(_, label)| label).collect(),
                |model| dialog.set_target_labels(model),
            );
            relabel_picker(
                &dialog.get_copy_source_labels(),
                dialog.get_copy_source_index(),
                copy_sources(&material, printers)
                    .into_iter()
                    .map(|source| source.label)
                    .collect(),
                |model| dialog.set_copy_source_labels(model),
            );
            if let Some(line) = error_line.borrow().as_ref() {
                dialog.set_error_message(line.text());
            }
        }
    });
    let dialog_weak = dialog.as_weak();

    dialog.on_copy_requested({
        let dialog_weak = dialog_weak.clone();
        move |picked| {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some(source) = sources.get(picked as usize) else {
                return;
            };
            let seeded = calibration_form_from_block(&source.values);
            d.set_normal_exposure_text(seeded.normal_exposure_sec.into());
            d.set_bottom_exposure_text(seeded.bottom_exposure_sec.into());
            d.set_bottom_count_text(seeded.bottom_layer_count.into());
            d.set_wait_before_cure_text(seeded.wait_before_cure_sec.into());
            d.set_wait_after_cure_text(seeded.wait_after_cure_sec.into());
            d.set_wait_after_lift_text(seeded.wait_after_lift_sec.into());
            d.set_bottom_wait_before_cure_text(seeded.bottom_wait_before_cure_sec.into());
            d.set_bottom_wait_after_cure_text(seeded.bottom_wait_after_cure_sec.into());
            d.set_bottom_wait_after_lift_text(seeded.bottom_wait_after_lift_sec.into());
            d.set_lift_distance_text(seeded.lift_distance_mm.into());
            d.set_lift_speed_text(seeded.lift_speed_mm_min.into());
            d.set_retract_speed_text(seeded.retract_speed_mm_min.into());
            d.set_second_lift_distance_text(seeded.second_lift_distance_mm.into());
            d.set_second_lift_speed_text(seeded.second_lift_speed_mm_min.into());
            d.set_second_retract_speed_text(seeded.second_retract_speed_mm_min.into());
            d.set_bottom_lift_distance_text(seeded.bottom_lift_distance_mm.into());
            d.set_bottom_lift_speed_text(seeded.bottom_lift_speed_mm_min.into());
            d.set_bottom_retract_speed_text(seeded.bottom_retract_speed_mm_min.into());
            d.set_bottom_second_lift_distance_text(seeded.bottom_second_lift_distance_mm.into());
            d.set_bottom_second_lift_speed_text(seeded.bottom_second_lift_speed_mm_min.into());
            d.set_bottom_second_retract_speed_text(
                seeded.bottom_second_retract_speed_mm_min.into(),
            );
            d.set_light_pwm_text(seeded.light_pwm.into());
            d.set_bottom_light_pwm_text(seeded.bottom_light_pwm.into());
        }
    });

    dialog.on_save_requested({
        let extend_slot = extend_slot.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some((target, _)) = targets.get(d.get_target_index() as usize) else {
                return;
            };
            let input = CalibrationFormInput {
                normal_exposure_sec: d.get_normal_exposure_text().to_string(),
                bottom_exposure_sec: d.get_bottom_exposure_text().to_string(),
                bottom_layer_count: d.get_bottom_count_text().to_string(),
                wait_before_cure_sec: d.get_wait_before_cure_text().to_string(),
                wait_after_cure_sec: d.get_wait_after_cure_text().to_string(),
                wait_after_lift_sec: d.get_wait_after_lift_text().to_string(),
                bottom_wait_before_cure_sec: d.get_bottom_wait_before_cure_text().to_string(),
                bottom_wait_after_cure_sec: d.get_bottom_wait_after_cure_text().to_string(),
                bottom_wait_after_lift_sec: d.get_bottom_wait_after_lift_text().to_string(),
                lift_distance_mm: d.get_lift_distance_text().to_string(),
                lift_speed_mm_min: d.get_lift_speed_text().to_string(),
                retract_speed_mm_min: d.get_retract_speed_text().to_string(),
                second_lift_distance_mm: d.get_second_lift_distance_text().to_string(),
                second_lift_speed_mm_min: d.get_second_lift_speed_text().to_string(),
                second_retract_speed_mm_min: d.get_second_retract_speed_text().to_string(),
                bottom_lift_distance_mm: d.get_bottom_lift_distance_text().to_string(),
                bottom_lift_speed_mm_min: d.get_bottom_lift_speed_text().to_string(),
                bottom_retract_speed_mm_min: d.get_bottom_retract_speed_text().to_string(),
                bottom_second_lift_distance_mm: d
                    .get_bottom_second_lift_distance_text()
                    .to_string(),
                bottom_second_lift_speed_mm_min: d.get_bottom_second_lift_speed_text().to_string(),
                bottom_second_retract_speed_mm_min: d
                    .get_bottom_second_retract_speed_text()
                    .to_string(),
                light_pwm: d.get_light_pwm_text().to_string(),
                bottom_light_pwm: d.get_bottom_light_pwm_text().to_string(),
            };
            let block = match parse_calibration_form(&input) {
                Ok(block) => block,
                Err(err) => {
                    show_error(&d, &error_line, ErrorLine::Translated(err));
                    return;
                }
            };
            let update = build_extension_update(&base, extension.as_ref(), target, block);
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
            if let Err(message) =
                save_edited_profile(&user_lib, &update, EditedProfileKind::Extension)
            {
                show_error(&d, &error_line, ErrorLine::Untranslated(message.into()));
                return;
            }
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *extend_slot.borrow_mut() = None;
        }
    });

    Ok(Some(dialog))
}

/// Shows `line` in the extend dialog's error line and keeps it in `kept`, so
/// that a language change writes the line again.
fn show_error(
    dialog: &ExtendDialog,
    kept: &RefCell<Option<ErrorLine<InvalidFormValue>>>,
    line: ErrorLine<InvalidFormValue>,
) {
    dialog.set_error_message(line.text());
    *kept.borrow_mut() = Some(line);
}

#[cfg(test)]
mod target_picker_tests {
    use slint::SharedString;

    use super::*;
    use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};

    #[test]
    fn the_dialog_opens_with_no_target_picked() {
        let picked = on_fresh_slint_thread(|| {
            let dialog = ExtendDialog::new().expect("the headless platform creates the dialog");
            // The order of `open_extend_dialog`: the labels, then no target.
            let labels = ["Printer A", "Generic", "Printer B"].map(SharedString::from);
            dialog.set_target_labels(ModelRc::new(VecModel::from(labels.to_vec())));
            dialog.set_target_index(-1);
            dialog
                .show()
                .expect("the headless platform shows the dialog");
            render_frames();
            dialog.get_target_index()
        });
        assert_eq!(picked, -1);
    }
}

#[cfg(test)]
mod language_change_tests {
    use sc_print_primitives::PrinterId;
    use slint::{Model, SharedString};

    use super::*;
    use crate::i18n::ctr;
    use crate::language::InstalledTranslations;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::ErrorDialog;
    use crate::test_support::catalog_entries::printer_entry;
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::material_profiles::resin_profile;
    use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
    use crate::ui_language::UiLanguage;

    /// A catalog of 1 printer, "Printer A", which the resin profile has no
    /// block for.
    fn catalog_of_printer_a() -> ProfileCatalog {
        let id = PrinterId::new("printer-a").expect("valid printer id");
        let mut printer = printer_entry(id, "Alpha", [120.0, 68.0, 150.0]);
        printer.name = "Printer A".to_owned();
        ProfileCatalog::from_entries(vec![printer], Vec::new())
    }

    /// The extend dialog for the resin profile over `catalog`, built after the
    /// UI language follows `bus`, as in the app.
    fn following_extend_dialog(
        bus: &Rc<PreferencesBus>,
        catalog: ProfileCatalog,
    ) -> (UiLanguage, Option<ExtendDialog>) {
        // In the app, the 1st main window is the 1st component of the thread.
        let first = ErrorDialog::new().expect("the headless platform creates components");
        let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&first), bus);
        let refresh_view: Rc<dyn Fn()> = Rc::new(|| {});
        let dialog = build_extend_dialog(
            bus,
            &Rc::new(RefCell::new(Rc::new(catalog))),
            &refresh_view,
            &Rc::new(RefCell::new(None)),
            resin_profile(),
            None,
        )
        .expect("the headless platform creates the extend dialog");
        (ui_language, dialog)
    }

    pub(super) fn extend_dialog(bus: &Rc<PreferencesBus>) -> (UiLanguage, ExtendDialog) {
        let (ui_language, dialog) = following_extend_dialog(bus, catalog_of_printer_a());
        (ui_language, dialog.expect("printer A is not covered"))
    }

    fn strings(model: ModelRc<SharedString>) -> Vec<String> {
        model.iter().map(|text| text.to_string()).collect()
    }

    fn sentinel_labels() -> ModelRc<SharedString> {
        ModelRc::new(VecModel::from(vec![SharedString::from("sentinel")]))
    }

    #[test]
    fn a_resin_that_covers_every_target_gets_no_dialog() {
        let built = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let empty = ProfileCatalog::from_entries(Vec::new(), Vec::new());
            following_extend_dialog(&bus, empty).1.is_some()
        });
        assert!(!built);
    }

    #[test]
    fn a_language_change_pushes_the_picker_labels_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = extend_dialog(&bus);
            dialog.set_target_labels(sentinel_labels());
            dialog.set_copy_source_labels(sentinel_labels());

            pick_language(&bus, "es");

            let shown = (
                strings(dialog.get_target_labels()),
                strings(dialog.get_copy_source_labels()),
            );
            let generic = ctr!("ExtendDialog", "Generic").to_string();
            (shown, (vec!["Printer A".to_owned()], vec![generic]))
        });
        assert_eq!(shown, expected);
    }

    #[test]
    fn with_nothing_picked_a_language_change_leaves_the_pickers_empty() {
        let picked = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = extend_dialog(&bus);
            dialog
                .show()
                .expect("the headless platform shows the dialog");
            render_frames();
            pick_language(&bus, "es");
            render_frames();
            (dialog.get_target_index(), dialog.get_copy_source_index())
        });
        assert_eq!(picked, (-1, -1));
    }

    #[test]
    fn with_a_target_picked_a_language_change_keeps_it_and_pushes_its_label() {
        let shown = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = extend_dialog(&bus);
            dialog
                .show()
                .expect("the headless platform shows the dialog");
            dialog.set_target_index(0);
            dialog.set_target_labels(sentinel_labels());
            render_frames();
            pick_language(&bus, "es");
            render_frames();
            (
                dialog.get_target_index(),
                strings(dialog.get_target_labels()),
                dialog.get_copy_source_index(),
            )
        });
        assert_eq!(shown, (0, vec!["Printer A".to_owned()], -1));
    }

    #[test]
    fn an_edited_field_keeps_its_text() {
        let text = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = extend_dialog(&bus);
            dialog.set_normal_exposure_text("2.5".into());
            pick_language(&bus, "es");
            dialog.get_normal_exposure_text().to_string()
        });
        assert_eq!(text, "2.5");
    }

    #[test]
    fn a_language_change_pushes_a_form_error_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = extend_dialog(&bus);
            dialog.set_target_index(0);
            dialog.set_normal_exposure_text("abc".into());
            dialog.invoke_save_requested();
            dialog.set_error_message("sentinel".into());

            pick_language(&bus, "es");

            let input = CalibrationFormInput {
                normal_exposure_sec: "abc".to_owned(),
                ..CalibrationFormInput::default()
            };
            let error = parse_calibration_form(&input).expect_err("\"abc\" is not a number");
            (
                dialog.get_error_message().to_string(),
                error.user_message().to_string(),
            )
        });
        assert_eq!(shown, expected);
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
    fn the_extend_dialog_takes_the_theme_and_follows_its_changes() {
        let themes = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            pick_theme(&bus, ThemePreference::Dark);
            let (_ui_language, dialog) = language_change_tests::extend_dialog(&bus);
            let at_open = dialog.get_theme();
            pick_theme(&bus, ThemePreference::Light);
            (at_open, dialog.get_theme())
        });
        assert_eq!(themes, (ThemePreference::Dark, ThemePreference::Light));
    }
}

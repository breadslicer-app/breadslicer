/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The in-place editor of a custom resin profile, which the Preferences window
//! and the fork dialog open.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, PlatformError, SharedString, VecModel};

use sc_print_profiles::{
    MaterialProfile, load_material_extension_from_library, load_material_profile_from_library,
    user_library_dir,
};

use crate::i18n::ctr;
use crate::material_form::{
    CalibrationFormInput, DefaultsFormInput, EditedProfileKind, InvalidFormValue,
    calibration_form_from_block, defaults_form_from, parse_calibration_form, parse_defaults_form,
    save_edited_profile,
};
use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, ProfileSource, refresh_catalog};
use crate::profile_view::{
    identity_section, intent_section, ordered_calibration_targets, profile_display_name,
};
use crate::slint_ui::ProfileViewDialog;
use crate::window_prefs::WindowPreferences;

use super::preferences_follow::{ErrorLine, follow_preferences, relabel_picker};

/// A note above the editor's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditorNote {
    /// The fork dialog wrote the profile to the library before the editor
    /// opened, so closing keeps it.
    ForkSaved,
}

impl EditorNote {
    /// The text of the note, in the language in force.
    fn text(self) -> SharedString {
        match self {
            Self::ForkSaved => ctr!(
                "ProfileViewDialog",
                "The forked profile is already in your library. Close keeps it unchanged.",
            ),
        }
    }
}

/// Opens the in-place editor for a custom row. The extension row edits the
/// extension file; a custom profile row edits the full profile, falling back to
/// the extension file for an orphaned extension entry. The defaults are
/// editable for full profiles only (an extension inherits them from its
/// parent), and 1 calibration pair per session can be picked from the profile's
/// own blocks and edited.
pub(super) fn open_profile_editor(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    dialog_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
    is_extension: bool,
    note: Option<EditorNote>,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some(entry) = catalog.materials().get(index as usize) else {
        return;
    };
    let user_lib = match user_library_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("cannot resolve the user profile library directory: {err}");
            return;
        }
    };

    let (profile, kind) = if is_extension {
        match load_material_extension_from_library(&user_lib, &entry.id) {
            Ok(profile) => (profile, EditedProfileKind::Extension),
            Err(err) => {
                eprintln!("{err}");
                return;
            }
        }
    } else {
        match load_material_profile_from_library(&user_lib, &entry.id) {
            Ok(profile) => (profile, EditedProfileKind::Full),
            Err(full_err) => match load_material_extension_from_library(&user_lib, &entry.id) {
                Ok(orphan_extension) => (orphan_extension, EditedProfileKind::Extension),
                Err(_) => {
                    eprintln!("{full_err}");
                    return;
                }
            },
        }
    };

    let dialog = match build_profile_editor(
        bus,
        catalog_handle,
        refresh_view,
        dialog_slot,
        profile,
        kind,
        note,
    ) {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the profile editor: {err}");
            return;
        }
    };

    let dialog_weak = dialog.as_weak();
    dialog.on_close_requested({
        let dialog_slot = dialog_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *dialog_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the profile editor: {err}");
        return;
    }
    super::centre_over_preferences(&dialog);
    *dialog_slot.borrow_mut() = Some(dialog);
}

/// Builds the editor of `profile`, with the texts that it takes from Rust in
/// the language in force while the editor exists. A save that succeeds
/// refreshes the catalog and the view, and empties `dialog_slot`.
fn build_profile_editor(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    dialog_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    profile: MaterialProfile,
    kind: EditedProfileKind,
    note: Option<EditorNote>,
) -> Result<ProfileViewDialog, PlatformError> {
    let catalog = catalog_handle.borrow().clone();
    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    let targets =
        ordered_calibration_targets(&profile, catalog.printers(), default_printer.as_ref());

    let dialog = ProfileViewDialog::new()?;
    dialog.apply_preferences(&snapshot);
    dialog.set_editable(true);
    dialog.set_defaults_editable(kind == EditedProfileKind::Full);
    if kind == EditedProfileKind::Full {
        let seeded = defaults_form_from(&profile.defaults);
        dialog.set_scale_x_text(seeded.scale_x_pct.into());
        dialog.set_scale_y_text(seeded.scale_y_pct.into());
        dialog.set_scale_z_text(seeded.scale_z_pct.into());
        dialog.set_defaults_bottom_count_text(seeded.bottom_layer_count.into());
        dialog.set_defaults_wait_before_cure_text(seeded.wait_before_cure_sec.into());
        dialog.set_defaults_wait_after_cure_text(seeded.wait_after_cure_sec.into());
        dialog.set_defaults_wait_after_lift_text(seeded.wait_after_lift_sec.into());
        dialog.set_defaults_light_pwm_text(seeded.light_pwm.into());
        dialog.set_defaults_bottom_light_pwm_text(seeded.bottom_light_pwm.into());
    }
    dialog.set_edit_target_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_edit_target_index(-1);

    let profile = Rc::new(RefCell::new(profile));
    let targets = Rc::new(targets);
    let error_line: Rc<RefCell<Option<ErrorLine<InvalidFormValue>>>> = Rc::default();

    let push_texts = {
        let profile = profile.clone();
        let error_line = error_line.clone();
        move |dialog: &ProfileViewDialog| {
            let profile = profile.borrow();
            let display_name = match kind {
                EditedProfileKind::Full => {
                    SharedString::from(profile_display_name(&profile.identity))
                }
                EditedProfileKind::Extension => ctr!(
                    "PreferencesPage",
                    "{} (extended)",
                    profile_display_name(&profile.identity)
                ),
            };
            dialog.set_profile_name(display_name);
            dialog.set_note(note.map(EditorNote::text).unwrap_or_default());

            // Read-only context above the edit cards: identity, plus intent for
            // full profiles (an extension's intent is inherited).
            let mut sections = vec![identity_section(&profile.identity, ProfileSource::User)];
            if kind == EditedProfileKind::Full {
                sections.push(intent_section(&profile.intent));
            }
            dialog.set_sections(ModelRc::new(VecModel::from(sections)));

            let labels =
                ordered_calibration_targets(&profile, catalog.printers(), default_printer.as_ref())
                    .into_iter()
                    .map(|(_, label)| label)
                    .collect();
            relabel_picker(
                &dialog.get_edit_target_labels(),
                dialog.get_edit_target_index(),
                labels,
                |model| dialog.set_edit_target_labels(model),
            );

            if let Some(line) = error_line.borrow().as_ref() {
                dialog.set_error_message(line.text());
            }
        }
    };
    push_texts(&dialog);
    follow_preferences(bus, &dialog, push_texts);

    let dialog_weak = dialog.as_weak();

    dialog.on_edit_target_changed({
        let dialog_weak = dialog_weak.clone();
        let profile = profile.clone();
        let targets = targets.clone();
        move |picked| {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some((id, _)) = targets.get(picked as usize) else {
                return;
            };
            let profile = profile.borrow();
            let Some(block) = profile.per_printer.get(id) else {
                return;
            };
            let seeded = calibration_form_from_block(block);
            d.set_normal_exposure_text(seeded.normal_exposure_sec.into());
            d.set_bottom_exposure_text(seeded.bottom_exposure_sec.into());
            d.set_block_bottom_count_text(seeded.bottom_layer_count.into());
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
        let dialog_slot = dialog_slot.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let mut profile = profile.borrow_mut();
            if kind == EditedProfileKind::Full {
                let input = DefaultsFormInput {
                    scale_x_pct: d.get_scale_x_text().to_string(),
                    scale_y_pct: d.get_scale_y_text().to_string(),
                    scale_z_pct: d.get_scale_z_text().to_string(),
                    bottom_layer_count: d.get_defaults_bottom_count_text().to_string(),
                    wait_before_cure_sec: d.get_defaults_wait_before_cure_text().to_string(),
                    wait_after_cure_sec: d.get_defaults_wait_after_cure_text().to_string(),
                    wait_after_lift_sec: d.get_defaults_wait_after_lift_text().to_string(),
                    light_pwm: d.get_defaults_light_pwm_text().to_string(),
                    bottom_light_pwm: d.get_defaults_bottom_light_pwm_text().to_string(),
                };
                match parse_defaults_form(&input) {
                    Ok(defaults) => profile.defaults = defaults,
                    Err(err) => {
                        show_error(&d, &error_line, ErrorLine::Translated(err));
                        return;
                    }
                }
            }
            let picked = d.get_edit_target_index();
            if picked >= 0 {
                let Some((id, _)) = targets.get(picked as usize) else {
                    return;
                };
                let input = CalibrationFormInput {
                    normal_exposure_sec: d.get_normal_exposure_text().to_string(),
                    bottom_exposure_sec: d.get_bottom_exposure_text().to_string(),
                    bottom_layer_count: d.get_block_bottom_count_text().to_string(),
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
                    bottom_second_lift_speed_mm_min: d
                        .get_bottom_second_lift_speed_text()
                        .to_string(),
                    bottom_second_retract_speed_mm_min: d
                        .get_bottom_second_retract_speed_text()
                        .to_string(),
                    light_pwm: d.get_light_pwm_text().to_string(),
                    bottom_light_pwm: d.get_bottom_light_pwm_text().to_string(),
                };
                match parse_calibration_form(&input) {
                    Ok(block) => {
                        profile.per_printer.insert(id.clone(), block);
                    }
                    Err(err) => {
                        show_error(&d, &error_line, ErrorLine::Translated(err));
                        return;
                    }
                }
            }
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
            if let Err(message) = save_edited_profile(&user_lib, &profile, kind) {
                show_error(&d, &error_line, ErrorLine::Untranslated(message.into()));
                return;
            }
            drop(profile);
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *dialog_slot.borrow_mut() = None;
        }
    });

    Ok(dialog)
}

/// Shows `line` in the editor's error line and keeps it in `kept`, so that a
/// language change writes the line again.
fn show_error(
    dialog: &ProfileViewDialog,
    kept: &RefCell<Option<ErrorLine<InvalidFormValue>>>,
    line: ErrorLine<InvalidFormValue>,
) {
    dialog.set_error_message(line.text());
    *kept.borrow_mut() = Some(line);
}

#[cfg(test)]
mod editor_note_tests {
    use super::*;

    #[test]
    fn the_fork_note_says_that_the_profile_is_saved() {
        assert_eq!(
            EditorNote::ForkSaved.text(),
            "The forked profile is already in your library. Close keeps it unchanged."
        );
    }
}

#[cfg(test)]
mod language_change_tests {
    use slint::Model;

    use super::*;
    use crate::language::InstalledTranslations;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::{AboutWindow, ProfileViewSection};
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::material_profiles::resin_profile;
    use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
    use crate::ui_language::UiLanguage;

    /// An editor of the resin profile as `kind`, with `note`, built after the
    /// UI language follows `bus`, as in the app.
    pub(super) fn following_editor(
        bus: &Rc<PreferencesBus>,
        kind: EditedProfileKind,
        note: Option<EditorNote>,
    ) -> (UiLanguage, ProfileViewDialog) {
        // In the app, the 1st main window is the 1st component of the thread.
        let first = AboutWindow::new().expect("the headless platform creates components");
        let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&first), bus);
        let catalog = ProfileCatalog::from_entries(Vec::new(), Vec::new());
        let refresh_view: Rc<dyn Fn()> = Rc::new(|| {});
        let dialog = build_profile_editor(
            bus,
            &Rc::new(RefCell::new(Rc::new(catalog))),
            &refresh_view,
            &Rc::new(RefCell::new(None)),
            resin_profile(),
            kind,
            note,
        )
        .expect("the headless platform creates the editor");
        (ui_language, dialog)
    }

    fn strings(model: ModelRc<SharedString>) -> Vec<String> {
        model.iter().map(|text| text.to_string()).collect()
    }

    fn sentinel_labels(rows: usize) -> ModelRc<SharedString> {
        ModelRc::new(VecModel::from(vec![SharedString::from("sentinel"); rows]))
    }

    #[test]
    fn a_language_change_pushes_the_texts_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_editor(
                &bus,
                EditedProfileKind::Extension,
                Some(EditorNote::ForkSaved),
            );
            let sentinel_section = ProfileViewSection {
                title: "sentinel".into(),
                ..ProfileViewSection::default()
            };
            dialog.set_profile_name("sentinel".into());
            dialog.set_note("sentinel".into());
            dialog.set_sections(ModelRc::new(VecModel::from(vec![sentinel_section])));
            dialog.set_edit_target_labels(sentinel_labels(1));

            pick_language(&bus, "es");

            let sections = dialog.get_sections();
            let shown = (
                dialog.get_profile_name().to_string(),
                dialog.get_note().to_string(),
                sections
                    .iter()
                    .map(|section| section.title.to_string())
                    .collect::<Vec<_>>(),
                strings(dialog.get_edit_target_labels()),
            );
            let identity = identity_section(&resin_profile().identity, ProfileSource::User);
            let expected = (
                ctr!("PreferencesPage", "{} (extended)", "Brand Resin").to_string(),
                EditorNote::ForkSaved.text().to_string(),
                vec![identity.title.to_string()],
                vec![ctr!("ProfileViewDialog", "Generic").to_string()],
            );
            (shown, expected)
        });
        assert_eq!(shown, expected);
    }

    #[test]
    fn with_nothing_picked_a_language_change_leaves_the_picker_empty() {
        let picked = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_editor(&bus, EditedProfileKind::Full, None);
            dialog
                .show()
                .expect("the headless platform shows the editor");
            render_frames();
            pick_language(&bus, "es");
            render_frames();
            dialog.get_edit_target_index()
        });
        assert_eq!(picked, -1);
    }

    #[test]
    fn with_a_target_picked_a_language_change_keeps_it_and_pushes_its_label() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_editor(&bus, EditedProfileKind::Full, None);
            dialog
                .show()
                .expect("the headless platform shows the editor");
            dialog.set_edit_target_index(0);
            dialog.set_edit_target_labels(sentinel_labels(1));
            render_frames();
            pick_language(&bus, "es");
            render_frames();
            let shown = (
                dialog.get_edit_target_index(),
                strings(dialog.get_edit_target_labels()),
            );
            (
                shown,
                vec![ctr!("ProfileViewDialog", "Generic").to_string()],
            )
        });
        assert_eq!(shown, (0, expected));
    }

    #[test]
    fn an_edited_field_keeps_its_text() {
        let text = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_editor(&bus, EditedProfileKind::Full, None);
            dialog.set_scale_x_text("101".into());
            pick_language(&bus, "es");
            dialog.get_scale_x_text().to_string()
        });
        assert_eq!(text, "101");
    }

    #[test]
    fn a_language_change_pushes_a_form_error_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, dialog) = following_editor(&bus, EditedProfileKind::Full, None);
            dialog.set_scale_x_text("abc".into());
            dialog.invoke_save_requested();
            dialog.set_error_message("sentinel".into());

            pick_language(&bus, "es");

            let input = DefaultsFormInput {
                scale_x_pct: "abc".to_owned(),
                ..DefaultsFormInput::default()
            };
            let error = parse_defaults_form(&input).expect_err("\"abc\" is not a number");
            (
                dialog.get_error_message().to_string(),
                error.user_message().to_string(),
            )
        });
        assert_eq!(shown, expected);
    }

    #[test]
    fn the_error_line_keeps_the_line_it_shows() {
        let (shown, kept) = on_fresh_slint_thread(|| {
            let dialog = ProfileViewDialog::new().expect("the headless platform creates it");
            let kept = RefCell::new(None);
            let form_error = InvalidFormValue::PartialScaleCompensation;
            show_error(&dialog, &kept, ErrorLine::Translated(form_error));
            show_error(&dialog, &kept, ErrorLine::Untranslated("disk full".into()));
            let kept = kept.borrow().as_ref().map(|line| line.text().to_string());
            (dialog.get_error_message().to_string(), kept)
        });
        assert_eq!(shown, "disk full");
        assert_eq!(kept.as_deref(), Some("disk full"));
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
    fn the_editor_takes_the_theme_and_follows_its_changes() {
        let themes = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            pick_theme(&bus, ThemePreference::Dark);
            let (_ui_language, dialog) =
                language_change_tests::following_editor(&bus, EditedProfileKind::Full, None);
            let at_open = dialog.get_theme();
            pick_theme(&bus, ThemePreference::Light);
            (at_open, dialog.get_theme())
        });
        assert_eq!(themes, (ThemePreference::Dark, ThemePreference::Light));
    }
}

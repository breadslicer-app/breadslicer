/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The read-only resin profile viewer, which the Preferences window opens.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, PlatformError, VecModel};

use sc_print_profiles::EffectiveMaterial;

use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, ProfileSource, load_profile_for_view};
use crate::profile_view::{build_profile_view_model, profile_display_name};
use crate::slint_ui::ProfileViewDialog;
use crate::window_prefs::WindowPreferences;

use super::preferences_follow::follow_preferences;

/// Opens the read-only profile viewer for the resin at `index`: the base
/// profile's sections plus its extension's calibration blocks, with the block
/// the default printer would use highlighted.
pub(super) fn open_profile_view_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    dialog_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((profile, extension, source)) = load_profile_for_view(&catalog, index) else {
        return;
    };
    let (material, issues) = EffectiveMaterial::compose(profile, extension);
    for issue in &issues {
        eprintln!("warning: {issue}");
    }

    let dialog = match build_profile_viewer(bus, material, source, catalog) {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the profile viewer: {err}");
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
        eprintln!("failed to show the profile viewer: {err}");
        return;
    }
    *dialog_slot.borrow_mut() = Some(dialog);
}

/// Builds the viewer of `material`, with its sections in the language in force
/// while the viewer exists.
fn build_profile_viewer(
    bus: &Rc<PreferencesBus>,
    material: EffectiveMaterial,
    source: ProfileSource,
    catalog: Rc<ProfileCatalog>,
) -> Result<ProfileViewDialog, PlatformError> {
    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());

    let dialog = ProfileViewDialog::new()?;
    dialog.apply_preferences(&snapshot);
    dialog.set_profile_name(profile_display_name(material.identity()).into());
    let push_sections = move |dialog: &ProfileViewDialog| {
        let sections = build_profile_view_model(
            &material,
            catalog.printers(),
            default_printer.as_ref(),
            source,
        );
        dialog.set_sections(ModelRc::new(VecModel::from(sections)));
    };
    push_sections(&dialog);
    follow_preferences(bus, &dialog, push_sections);
    Ok(dialog)
}

#[cfg(test)]
mod language_change_tests {
    use slint::Model;

    use super::*;
    use crate::language::InstalledTranslations;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::{ErrorDialog, ProfileViewSection};
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::material_profiles::resin_profile;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::ui_language::UiLanguage;

    fn titles(sections: impl IntoIterator<Item = ProfileViewSection>) -> Vec<String> {
        sections
            .into_iter()
            .map(|section| section.title.to_string())
            .collect()
    }

    #[test]
    fn a_language_change_pushes_the_sections_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            // In the app, the 1st main window is the 1st component of the
            // thread.
            let first = ErrorDialog::new().expect("the headless platform creates components");
            let _ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&first), &bus);
            let catalog = Rc::new(ProfileCatalog::from_entries(Vec::new(), Vec::new()));
            let material = EffectiveMaterial::compose(resin_profile(), None).0;
            let dialog = build_profile_viewer(&bus, material, ProfileSource::User, catalog)
                .expect("the headless platform creates the viewer");
            let sentinel = ProfileViewSection {
                title: "sentinel".into(),
                ..ProfileViewSection::default()
            };
            dialog.set_sections(ModelRc::new(VecModel::from(vec![sentinel])));

            pick_language(&bus, "es");

            let material = EffectiveMaterial::compose(resin_profile(), None).0;
            let expected = build_profile_view_model(&material, &[], None, ProfileSource::User);
            (titles(dialog.get_sections().iter()), titles(expected))
        });
        assert_eq!(shown, expected);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::ThemePreference;
    use crate::test_support::material_profiles::resin_profile;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::test_support::theme_pick::pick_theme;

    #[test]
    fn the_viewer_takes_the_theme_and_follows_its_changes() {
        let themes = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            pick_theme(&bus, ThemePreference::Dark);
            let catalog = Rc::new(ProfileCatalog::from_entries(Vec::new(), Vec::new()));
            let material = EffectiveMaterial::compose(resin_profile(), None).0;
            let dialog = build_profile_viewer(&bus, material, ProfileSource::User, catalog)
                .expect("the headless platform creates the viewer");
            let at_open = dialog.get_theme();
            pick_theme(&bus, ThemePreference::Light);
            (at_open, dialog.get_theme())
        });
        assert_eq!(themes, (ThemePreference::Dark, ThemePreference::Light));
    }
}

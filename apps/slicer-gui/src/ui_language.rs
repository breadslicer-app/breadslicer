/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The language that the UI shows: the language preference, in force from the
//! first window on.

use std::rc::Rc;

use crate::language::InstalledTranslations;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};

/// Keeps the UI language equal to the language preference: puts it in force
/// when created, and again on each `Language` change.
pub(crate) struct UiLanguage {
    _subscription: Subscription,
}

impl UiLanguage {
    /// Puts the language of `bus` in force and follows its changes. Create it
    /// before any listener that reads translated text subscribes: the bus calls
    /// its listeners in registration order.
    pub(crate) fn follow(translations: InstalledTranslations, bus: &Rc<PreferencesBus>) -> Self {
        bus.snapshot().language.apply(translations);
        let subscription = bus.subscribe(move |snap, change| {
            if *change == PreferencesChange::Language {
                snap.language.apply(translations);
            }
        });
        Self {
            _subscription: subscription,
        }
    }
}

#[cfg(test)]
mod ui_language_tests {
    use slint::select_bundled_translation;

    use super::*;
    use crate::language::{BundledLanguage, LanguagePreference};
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::{ErrorDialog, ThemePreference};

    /// The source text of `ErrorDialog.heading`.
    const ENGLISH_HEADING: &str = "An error occurred";

    fn bundled(code: &str) -> LanguagePreference {
        LanguagePreference::Bundled(BundledLanguage::from_code(code).expect("bundled language"))
    }

    fn bus_with(language: LanguagePreference) -> Rc<PreferencesBus> {
        PreferencesBus::new(PreferencesSnapshot {
            language,
            ..PreferencesSnapshot::for_tests()
        })
    }

    fn new_dialog() -> ErrorDialog {
        ErrorDialog::new().expect("the headless platform creates components")
    }

    fn heading(dialog: &ErrorDialog) -> String {
        dialog.get_heading().to_string()
    }

    fn follow_from(dialog: &ErrorDialog, bus: &Rc<PreferencesBus>) -> UiLanguage {
        UiLanguage::follow(InstalledTranslations::proven_by(dialog), bus)
    }

    fn change_language(bus: &PreferencesBus, language: LanguagePreference) {
        bus.update(move |snap| {
            snap.language = language;
            PreferencesChange::Language
        });
    }

    /// Returns the heading of the 1st component of a fresh Slint thread, with
    /// `language` stored and followed from that component on.
    fn first_heading_with(language: LanguagePreference) -> String {
        on_fresh_slint_thread(move || {
            let bus = bus_with(language);
            let dialog = new_dialog();
            let _ui_language = follow_from(&dialog, &bus);
            heading(&dialog)
        })
    }

    #[test]
    fn the_stored_language_is_in_force_in_the_first_window() {
        let spanish = first_heading_with(bundled("es"));
        let german = first_heading_with(bundled("de"));
        assert_ne!(spanish, ENGLISH_HEADING);
        assert_ne!(german, ENGLISH_HEADING);
        assert_ne!(spanish, german);
    }

    #[test]
    fn a_window_created_later_shows_the_followed_language() {
        let (first, later) = on_fresh_slint_thread(|| {
            let bus = bus_with(bundled("es"));
            let first = new_dialog();
            let _ui_language = follow_from(&first, &bus);
            (heading(&first), heading(&new_dialog()))
        });
        assert_ne!(first, ENGLISH_HEADING);
        assert_eq!(later, first);
    }

    #[test]
    fn a_language_change_is_put_in_force() {
        let (spanish, changed) = on_fresh_slint_thread(|| {
            let bus = bus_with(bundled("es"));
            let dialog = new_dialog();
            let _ui_language = follow_from(&dialog, &bus);
            let spanish = heading(&dialog);
            change_language(&bus, bundled("de"));
            (spanish, heading(&dialog))
        });
        assert_ne!(changed, spanish);
        assert_eq!(changed, first_heading_with(bundled("de")));
    }

    #[test]
    fn a_change_of_another_preference_keeps_the_language() {
        let (selected, after) = on_fresh_slint_thread(|| {
            let bus = bus_with(bundled("es"));
            let dialog = new_dialog();
            let _ui_language = follow_from(&dialog, &bus);
            select_bundled_translation("de").expect("`de` is bundled");
            let selected = heading(&dialog);
            bus.update(|snap| {
                snap.theme = ThemePreference::Dark;
                PreferencesChange::Theme
            });
            (selected, heading(&dialog))
        });
        assert_eq!(selected, first_heading_with(bundled("de")));
        assert_eq!(after, selected);
    }

    #[test]
    fn the_system_preference_follows_the_os_locale() {
        let (os_reading, before, after) = on_fresh_slint_thread(|| {
            let bus = bus_with(bundled("es"));
            let dialog = new_dialog();
            // Slint selects the language of the OS locale when the thread
            // creates its 1st component.
            let os_reading = heading(&dialog);
            let _ui_language = follow_from(&dialog, &bus);
            if heading(&dialog) == os_reading {
                change_language(&bus, bundled("de"));
            }
            let before = heading(&dialog);
            change_language(&bus, LanguagePreference::System);
            (os_reading, before, heading(&dialog))
        });
        assert_ne!(before, os_reading);
        assert_eq!(after, os_reading);
    }

    #[test]
    fn explicit_english_shows_the_source_strings() {
        let (english, english_selectable) = on_fresh_slint_thread(|| {
            let bus = bus_with(bundled("es"));
            let dialog = new_dialog();
            let _ui_language = follow_from(&dialog, &bus);
            change_language(&bus, bundled("en"));
            // `apply` falls back to the source strings when a code is refused,
            // so the heading alone does not show that Slint takes "en".
            let english_selectable = select_bundled_translation("en").is_ok();
            (heading(&dialog), english_selectable)
        });
        assert_eq!(english, ENGLISH_HEADING);
        assert!(english_selectable);
    }

    #[test]
    fn dropping_the_follower_stops_following() {
        let (spanish, after) = on_fresh_slint_thread(|| {
            let bus = bus_with(bundled("es"));
            let dialog = new_dialog();
            let ui_language = follow_from(&dialog, &bus);
            let spanish = heading(&dialog);
            drop(ui_language);
            change_language(&bus, bundled("de"));
            (spanish, heading(&dialog))
        });
        assert_ne!(spanish, ENGLISH_HEADING);
        assert_eq!(after, spanish);
    }
}

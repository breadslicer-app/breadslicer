/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window preference application. Each window type implements
//! [`WindowPreferences::apply_preferences`] to push a snapshot into
//! its Slint properties. One module holds all three impls so it's
//! easy to audit which preferences each window honours.
//!
//! Preference re-application on change is driven by a
//! `PreferencesBus` subscription held by the window's builder; this
//! module just defines *what* each window does when a snapshot
//! arrives, not *when*.

use crate::language::LanguagePreference;
use crate::preferences_bus::PreferencesSnapshot;
use crate::{AboutWindow, MainWindow, MenuLayout, PreferencesWindow};

pub(crate) trait WindowPreferences {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot);
}

impl WindowPreferences for MainWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
        // Menu layout is a Linux/Windows affordance: macOS renders a
        // global top-of-screen menu bar, so force Bar there and ignore
        // the stored value.
        let menu_layout = if cfg!(target_os = "macos") {
            MenuLayout::Bar
        } else {
            prefs.menu_layout
        };
        self.set_menu_layout(menu_layout);
        // Language: `@tr` bindings throughout the Slint tree
        // re-evaluate automatically after `select_bundled_translation`
        // flips the active locale; nothing to push per-window.
    }
}

impl WindowPreferences for PreferencesWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
        // Rebuild the language combo model against the active
        // language so the "System" label re-translates, then point
        // the combo at the current preference.
        self.set_language_options(LanguagePreference::display_options());
        self.set_current_language_index(prefs.language.combo_index());
        // Printer / resin selection is pushed separately (see
        // `preferences_window::apply_printing`); the catalog-backed
        // models are not part of the plain snapshot.
    }
}

impl WindowPreferences for AboutWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
        // Language: `@tr` bindings re-evaluate automatically.
    }
}

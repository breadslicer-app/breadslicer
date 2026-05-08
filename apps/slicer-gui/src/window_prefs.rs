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

use crate::build_volume::BuildVolume;
use crate::language::LanguagePreference;
use crate::preferences_bus::PreferencesSnapshot;
use crate::{AboutWindow, MainWindow, PreferencesWindow};

/// Render a build-volume axis as the text shown in the preferences
/// LineEdit. Whole millimetres render as plain integers (no trailing
/// `.0`); fractional values keep up to one decimal so the field still
/// reflects what's persisted on disk.
fn format_dim(d: f32) -> slint::SharedString {
    if (d - d.round()).abs() < 1e-3 {
        slint::format!("{}", d.round() as i32)
    } else {
        slint::format!("{:.1}", d)
    }
}

/// Push the build volume's three axes into the preferences window's
/// LineEdit-backed text properties.
fn apply_build_volume(window: &PreferencesWindow, bv: BuildVolume) {
    window.set_build_volume_width_text(format_dim(bv.width_mm()));
    window.set_build_volume_depth_text(format_dim(bv.depth_mm()));
    window.set_build_volume_height_text(format_dim(bv.height_mm()));
}

pub(crate) trait WindowPreferences {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot);
}

impl WindowPreferences for MainWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
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
        apply_build_volume(self, prefs.build_volume);
    }
}

impl WindowPreferences for AboutWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
        // Language: `@tr` bindings re-evaluate automatically.
    }
}

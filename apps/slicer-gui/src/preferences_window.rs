/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use slint::ComponentHandle;

use crate::language::LanguagePreference;
use crate::{PreferencesWindow, ThemePreference};

pub(crate) fn build_preferences_window(
    on_theme_changed: impl Fn(ThemePreference) + 'static,
    on_language_changed: impl Fn(LanguagePreference) + 'static,
) -> Result<PreferencesWindow, slint::PlatformError> {
    let prefs = PreferencesWindow::new()?;

    let prefs_weak = prefs.as_weak();
    prefs.on_close_requested(move || {
        if let Some(w) = prefs_weak.upgrade() {
            let _ = w.hide();
        }
    });

    prefs.on_theme_changed(on_theme_changed);
    prefs.on_language_changed(move |index| {
        on_language_changed(LanguagePreference::from_combo_index(index))
    });

    Ok(prefs)
}

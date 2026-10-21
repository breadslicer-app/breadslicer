/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::preferences_bus::{PreferencesBus, PreferencesChange};
use crate::slint_ui::ThemePreference;

/// Stores `theme` as the theme preference of `bus`, as a pick in the
/// Preferences window does.
pub(crate) fn pick_theme(bus: &PreferencesBus, theme: ThemePreference) {
    bus.update(move |snap| {
        snap.theme = theme;
        PreferencesChange::Theme
    });
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::rc::Rc;

use slint::ComponentHandle;

use crate::PreferencesWindow;
use crate::language::LanguagePreference;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::window_prefs::WindowPreferences;

/// Bundle of a `PreferencesWindow` and its bus subscription. Drop
/// drops the subscription so the listener is removed as soon as the
/// wrapped window is no longer held.
pub(crate) struct WiredPreferencesWindow {
    pub window: PreferencesWindow,
    _subscription: Subscription,
}

pub(crate) fn build_preferences_window(
    bus: &Rc<PreferencesBus>,
) -> Result<WiredPreferencesWindow, slint::PlatformError> {
    let prefs = PreferencesWindow::new()?;
    prefs.apply_preferences(&bus.snapshot());

    let prefs_weak = prefs.as_weak();
    prefs.on_close_requested({
        let prefs_weak = prefs_weak.clone();
        move || {
            if let Some(w) = prefs_weak.upgrade() {
                let _ = w.hide();
            }
        }
    });

    prefs.on_theme_changed({
        let bus = bus.clone();
        move |new_theme| {
            bus.update(|snap| {
                snap.theme = new_theme;
                PreferencesChange::Theme
            });
        }
    });

    prefs.on_language_changed({
        let bus = bus.clone();
        move |index| {
            let new_language = LanguagePreference::from_combo_index(index);
            bus.update(|snap| {
                snap.language = new_language;
                PreferencesChange::Language
            });
        }
    });

    let subscription = bus.subscribe(move |snap, _change| {
        if let Some(w) = prefs_weak.upgrade() {
            w.apply_preferences(snap);
        }
    });

    Ok(WiredPreferencesWindow {
        window: prefs,
        _subscription: subscription,
    })
}

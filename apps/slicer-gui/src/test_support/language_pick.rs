/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::language::{BundledLanguage, LanguagePreference};
use crate::preferences_bus::{PreferencesBus, PreferencesChange};

/// Stores the bundled language `code` as the language preference of `bus`, as a
/// pick in the Preferences window does.
pub(crate) fn pick_language(bus: &PreferencesBus, code: &str) {
    let language =
        LanguagePreference::Bundled(BundledLanguage::from_code(code).expect("a bundled language"));
    bus.update(move |snap| {
        snap.language = language;
        PreferencesChange::Language
    });
}

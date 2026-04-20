/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use slint::ComponentHandle;

use crate::PreferencesWindow;

pub(crate) fn build_preferences_window() -> Result<PreferencesWindow, slint::PlatformError> {
    let prefs = PreferencesWindow::new()?;

    let prefs_weak = prefs.as_weak();
    prefs.on_close_requested(move || {
        if let Some(w) = prefs_weak.upgrade() {
            let _ = w.hide();
        }
    });

    Ok(prefs)
}

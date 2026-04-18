/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use slint::ComponentHandle;

use crate::AboutWindow;

pub(crate) fn build_about_window() -> Result<AboutWindow, slint::PlatformError> {
    let about = AboutWindow::new()?;
    about.set_app_version(env!("CARGO_PKG_VERSION").into());

    let about_weak = about.as_weak();
    about.on_close_requested(move || {
        if let Some(w) = about_weak.upgrade() {
            let _ = w.hide();
        }
    });

    Ok(about)
}

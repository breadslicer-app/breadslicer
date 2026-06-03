/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::rc::Rc;

use slint::ComponentHandle;

use crate::AboutWindow;
use crate::preferences_bus::{PreferencesBus, Subscription};
use crate::window_prefs::WindowPreferences;

/// Bundle of an `AboutWindow` and its preference-bus subscription.
/// Dropping the bundle drops the subscription, which removes the
/// listener from the bus, preventing dangling calls into a
/// no-longer-used window handle.
pub(crate) struct WiredAboutWindow {
    pub window: AboutWindow,
    _subscription: Subscription,
}

pub(crate) fn build_about_window(
    bus: &Rc<PreferencesBus>,
) -> Result<WiredAboutWindow, slint::PlatformError> {
    let about = AboutWindow::new()?;
    about.set_app_version(env!("CARGO_PKG_VERSION").into());
    about.apply_preferences(&bus.snapshot());

    let about_weak = about.as_weak();
    about.on_close_requested({
        let about_weak = about_weak.clone();
        move || {
            if let Some(w) = about_weak.upgrade() {
                let _ = w.hide();
            }
        }
    });

    let subscription = bus.subscribe(move |snap, _change| {
        if let Some(w) = about_weak.upgrade() {
            w.apply_preferences(snap);
        }
    });

    Ok(WiredAboutWindow {
        window: about,
        _subscription: subscription,
    })
}

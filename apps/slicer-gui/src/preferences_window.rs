/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::rc::Rc;

use slint::{ComponentHandle, SharedString};

use crate::PreferencesWindow;
use crate::app_settings::sanitise_dim;
use crate::build_volume::{BuildVolume, HARDCODED_DEFAULT_BUILD_VOLUME};
use crate::language::LanguagePreference;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::window_prefs::WindowPreferences;

#[derive(Clone, Copy)]
enum Axis {
    Width,
    Depth,
    Height,
}

/// Build the closure wired to a single axis's `accepted` callback.
/// Parses the text, clamps via the same `sanitise_dim` the on-disk
/// loader uses, and pushes the new envelope through the bus. On parse
/// failure the snapshot is re-applied to the window so the LineEdit
/// reverts to the previous good value.
fn commit_axis_handler(
    bus: &Rc<PreferencesBus>,
    prefs_weak: slint::Weak<PreferencesWindow>,
    axis: Axis,
) -> impl Fn(SharedString) + 'static {
    let bus = bus.clone();
    move |text| {
        let current = bus.snapshot().build_volume;
        let parsed = text.trim().parse::<f32>().ok();
        let new_bv = match parsed {
            Some(v) => with_axis(current, axis, sanitise_dim(v)),
            None => {
                if let Some(w) = prefs_weak.upgrade() {
                    w.apply_preferences(&bus.snapshot());
                }
                return;
            }
        };
        if new_bv == current {
            // The clamp may have produced the same value the user had
            // before (e.g. "5" -> 10 mm when 10 mm was already set).
            // Re-apply the snapshot so the LineEdit text snaps from
            // whatever the user typed back to the canonical
            // representation.
            if let Some(w) = prefs_weak.upgrade() {
                w.apply_preferences(&bus.snapshot());
            }
            return;
        }
        bus.update(|snap| {
            snap.build_volume = new_bv;
            PreferencesChange::BuildVolume
        });
    }
}

fn with_axis(bv: BuildVolume, axis: Axis, value: f32) -> BuildVolume {
    match axis {
        Axis::Width => BuildVolume::new(value, bv.depth_mm(), bv.height_mm()),
        Axis::Depth => BuildVolume::new(bv.width_mm(), value, bv.height_mm()),
        Axis::Height => BuildVolume::new(bv.width_mm(), bv.depth_mm(), value),
    }
}

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

    prefs.on_build_volume_width_committed(commit_axis_handler(
        bus,
        prefs_weak.clone(),
        Axis::Width,
    ));
    prefs.on_build_volume_depth_committed(commit_axis_handler(
        bus,
        prefs_weak.clone(),
        Axis::Depth,
    ));
    prefs.on_build_volume_height_committed(commit_axis_handler(
        bus,
        prefs_weak.clone(),
        Axis::Height,
    ));

    prefs.on_reset_build_volume_requested({
        let bus = bus.clone();
        move || {
            bus.update(|snap| {
                snap.build_volume = HARDCODED_DEFAULT_BUILD_VOLUME;
                PreferencesChange::BuildVolume
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

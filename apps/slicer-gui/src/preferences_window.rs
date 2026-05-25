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

/// Decision shape for one axis-LineEdit commit attempt. Separates the
/// pure parse-and-clamp logic from the Slint / bus side effects so
/// the parse-failure / clamp-no-op / new-value branches can be unit
/// tested. The original closure now matches on this enum and turns
/// each variant into the appropriate side effect.
#[derive(Debug, PartialEq)]
enum AxisCommit {
    /// Text did not parse as `f32`; the caller should re-apply the
    /// snapshot so the LineEdit reverts to its previous canonical
    /// representation.
    RevertParseError,
    /// Text parsed but clamping produced the same envelope already on
    /// the bus (e.g. typing "5" when 10 mm is the floor); same revert
    /// behaviour as the parse-error case, just a different cause.
    RevertUnchanged,
    /// Text parsed, clamping produced a new envelope; the caller
    /// should push it through the bus.
    Commit(BuildVolume),
}

/// Pure parse-clamp-classify step. Mirrors the on-disk loader's
/// `sanitise_dim` so a value typed into the UI cannot reach the bus
/// in a form the disk persistence would reject on the next launch.
fn parse_and_apply_axis(current: BuildVolume, axis: Axis, text: &str) -> AxisCommit {
    let Some(v) = text.trim().parse::<f32>().ok() else {
        return AxisCommit::RevertParseError;
    };
    let new_bv = with_axis(current, axis, sanitise_dim(v));
    if new_bv == current {
        AxisCommit::RevertUnchanged
    } else {
        AxisCommit::Commit(new_bv)
    }
}

/// Build the closure wired to a single axis's `accepted` callback.
/// Delegates the pure logic to [`parse_and_apply_axis`] and applies
/// the corresponding Slint / bus side effect.
fn commit_axis_handler(
    bus: &Rc<PreferencesBus>,
    prefs_weak: slint::Weak<PreferencesWindow>,
    axis: Axis,
) -> impl Fn(SharedString) + 'static {
    let bus = bus.clone();
    move |text| {
        let current = bus.snapshot().build_volume;
        match parse_and_apply_axis(current, axis, &text) {
            AxisCommit::RevertParseError | AxisCommit::RevertUnchanged => {
                if let Some(w) = prefs_weak.upgrade() {
                    w.apply_preferences(&bus.snapshot());
                }
            }
            AxisCommit::Commit(new_bv) => {
                bus.update(move |snap| {
                    snap.build_volume = new_bv;
                    PreferencesChange::BuildVolume
                });
            }
        }
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
            bus.update(move |snap| {
                snap.theme = new_theme;
                PreferencesChange::Theme
            });
        }
    });

    prefs.on_language_changed({
        let bus = bus.clone();
        move |index| {
            let new_language = LanguagePreference::from_combo_index(index);
            bus.update(move |snap| {
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
            bus.update(move |snap| {
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

#[cfg(test)]
mod with_axis_tests {
    use super::*;

    fn bv() -> BuildVolume {
        BuildVolume::new(100.0, 200.0, 300.0)
    }

    #[test]
    fn width_axis_only_replaces_width_field() {
        let out = with_axis(bv(), Axis::Width, 150.0);
        assert_eq!(out.width_mm(), 150.0);
        assert_eq!(out.depth_mm(), 200.0);
        assert_eq!(out.height_mm(), 300.0);
    }

    #[test]
    fn depth_axis_only_replaces_depth_field() {
        let out = with_axis(bv(), Axis::Depth, 150.0);
        assert_eq!(out.width_mm(), 100.0);
        assert_eq!(out.depth_mm(), 150.0);
        assert_eq!(out.height_mm(), 300.0);
    }

    #[test]
    fn height_axis_only_replaces_height_field() {
        let out = with_axis(bv(), Axis::Height, 150.0);
        assert_eq!(out.width_mm(), 100.0);
        assert_eq!(out.depth_mm(), 200.0);
        assert_eq!(out.height_mm(), 150.0);
    }
}

#[cfg(test)]
mod parse_and_apply_axis_tests {
    use super::*;
    use crate::app_settings::{MAX_DIM_MM, MIN_DIM_MM};

    fn bv() -> BuildVolume {
        BuildVolume::new(100.0, 200.0, 300.0)
    }

    #[test]
    fn non_numeric_text_returns_revert_parse_error() {
        assert_eq!(
            parse_and_apply_axis(bv(), Axis::Width, "abc"),
            AxisCommit::RevertParseError,
        );
    }

    #[test]
    fn empty_text_returns_revert_parse_error() {
        // `"".trim().parse::<f32>()` fails; covers the `.trim()` path.
        assert_eq!(
            parse_and_apply_axis(bv(), Axis::Width, ""),
            AxisCommit::RevertParseError,
        );
        assert_eq!(
            parse_and_apply_axis(bv(), Axis::Width, "   "),
            AxisCommit::RevertParseError,
        );
    }

    #[test]
    fn clamp_to_existing_value_returns_revert_unchanged() {
        // Current width sits at MIN_DIM_MM (10 mm). Typing "5" clamps
        // up to 10, which matches current; no commit happens.
        let current = BuildVolume::new(MIN_DIM_MM, 200.0, 300.0);
        assert_eq!(
            parse_and_apply_axis(current, Axis::Width, "5"),
            AxisCommit::RevertUnchanged,
        );
    }

    #[test]
    fn valid_value_returns_commit_with_only_the_named_axis_changed() {
        let expected = BuildVolume::new(150.0, 200.0, 300.0);
        assert_eq!(
            parse_and_apply_axis(bv(), Axis::Width, "150"),
            AxisCommit::Commit(expected),
        );
    }

    #[test]
    fn above_max_clamps_to_ceiling_then_commits() {
        // 5000 mm is above MAX_DIM_MM (1000); clamped to ceiling and
        // committed because the current value is below the ceiling.
        let expected = BuildVolume::new(MAX_DIM_MM, 200.0, 300.0);
        assert_eq!(
            parse_and_apply_axis(bv(), Axis::Width, "5000"),
            AxisCommit::Commit(expected),
        );
    }

    #[test]
    fn whitespace_around_number_is_accepted() {
        let expected = BuildVolume::new(150.0, 200.0, 300.0);
        assert_eq!(
            parse_and_apply_axis(bv(), Axis::Width, "  150  "),
            AxisCommit::Commit(expected),
        );
    }

    #[test]
    fn each_axis_routes_through_its_own_field() {
        // Same input value drives a different output for each axis.
        let width = parse_and_apply_axis(bv(), Axis::Width, "150");
        let depth = parse_and_apply_axis(bv(), Axis::Depth, "150");
        let height = parse_and_apply_axis(bv(), Axis::Height, "150");
        assert_eq!(
            width,
            AxisCommit::Commit(BuildVolume::new(150.0, 200.0, 300.0))
        );
        assert_eq!(
            depth,
            AxisCommit::Commit(BuildVolume::new(100.0, 150.0, 300.0))
        );
        assert_eq!(
            height,
            AxisCommit::Commit(BuildVolume::new(100.0, 200.0, 150.0))
        );
    }
}

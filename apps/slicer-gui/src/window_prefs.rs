/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window preference application. Each window type implements
//! [`WindowPreferences::apply_preferences`] to push a snapshot into its Slint
//! properties, and [`restore_panels`] pushes the stored panel state into a new
//! main window. 1 module holds every impl and `restore_panels`, so it's easy to
//! audit which preferences each window honours.
//!
//! Preference re-application on change is driven by a `PreferencesBus`
//! subscription held by the window's builder; this module just defines *what*
//! each window does when a snapshot arrives, not *when*.

use crate::aa_level::aa_combo_index;
use crate::language::LanguagePreference;
use crate::preferences_bus::PreferencesSnapshot;
use crate::{
    AboutWindow, ErrorDialog, InfoDialog, MainWindow, MenuLayout, PreferencesWindow,
    ReplaceSceneDialog, SliceOverwriteDialog,
};

pub(crate) trait WindowPreferences {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot);
}

impl WindowPreferences for MainWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
        // Menu layout is a Linux/Windows affordance: macOS renders a global
        // top-of-screen menu bar, so force Bar there and ignore the stored
        // value.
        let menu_layout = if cfg!(target_os = "macos") {
            MenuLayout::Bar
        } else {
            prefs.menu_layout
        };
        self.set_menu_layout(menu_layout);
        // Language: `@tr` bindings throughout the Slint tree re-evaluate
        // automatically after `select_bundled_translation` flips the active
        // locale. The viewport controller and the slice panel push the texts
        // that they build in Rust again themselves.
    }
}

impl WindowPreferences for PreferencesWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
        // Rebuild the language combo model against the active language so the
        // "System" label re-translates, then point the combo at the current
        // preference.
        self.set_language_options(LanguagePreference::display_options());
        self.set_current_language_index(prefs.language.combo_index());
        self.set_current_aa_index(aa_combo_index(prefs.aa_level));
        // Printer / resin selection is pushed separately (see
        // `preferences_window::apply_printing`); the catalog-backed models are
        // not part of the plain snapshot.
    }
}

impl WindowPreferences for AboutWindow {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
        // Language: `@tr` bindings re-evaluate automatically.
    }
}

// The dialogs of a main window. Their texts built in Rust follow the language
// through their manager.

impl WindowPreferences for ErrorDialog {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
    }
}

impl WindowPreferences for InfoDialog {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
    }
}

impl WindowPreferences for ReplaceSceneDialog {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
    }
}

impl WindowPreferences for SliceOverwriteDialog {
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        self.set_theme(prefs.theme);
    }
}

/// Pushes the stored state of the floating panels (object list, Slice control
/// panel, slice view) into a new main window. Call it once, when the window is
/// created: the panels are per-window live state, so `apply_preferences` leaves
/// them out, and a later change in 1 window is saved to disk without moving or
/// hiding the panels in other windows.
pub(crate) fn restore_panels(window: &MainWindow, prefs: &PreferencesSnapshot) {
    // Object list. An offset or size field is pushed only when a previous
    // session saved it; leaving a field untouched keeps the Slint default
    // (top-right anchor, 12px offsets, 220px width, auto height).
    window.set_show_object_list(prefs.object_list_visible);
    window.set_object_list_minimized(prefs.object_list_minimized);
    window.set_object_list_anchor_right(prefs.object_list_anchor_right);
    window.set_object_list_anchor_bottom(prefs.object_list_anchor_bottom);
    if let Some(offset_x) = prefs.object_list_offset_x {
        window.set_object_list_offset_x(offset_x);
    }
    if let Some(offset_y) = prefs.object_list_offset_y {
        window.set_object_list_offset_y(offset_y);
    }
    if let Some(width) = prefs.object_list_width {
        window.set_object_list_width(width);
    }
    if let Some(height) = prefs.object_list_height {
        window.set_object_list_height(height);
    }
    // Slice control panel, same rule (top-left anchor, 12px offsets, 240px
    // width, auto height).
    window.set_slice_panel_anchor_right(prefs.slice_panel_anchor_right);
    window.set_slice_panel_anchor_bottom(prefs.slice_panel_anchor_bottom);
    if let Some(offset_x) = prefs.slice_panel_offset_x {
        window.set_slice_panel_offset_x(offset_x);
    }
    if let Some(offset_y) = prefs.slice_panel_offset_y {
        window.set_slice_panel_offset_y(offset_y);
    }
    if let Some(width) = prefs.slice_panel_width {
        window.set_slice_panel_width(width);
    }
    if let Some(height) = prefs.slice_panel_height {
        window.set_slice_panel_height(height);
    }
    // Slice view, same rule (bottom-left anchor, 12px offsets, 240px width,
    // auto height).
    window.set_show_slice_view(prefs.slice_view_visible);
    window.set_slice_view_minimized(prefs.slice_view_minimized);
    window.set_slice_view_anchor_right(prefs.slice_view_anchor_right);
    window.set_slice_view_anchor_bottom(prefs.slice_view_anchor_bottom);
    if let Some(offset_x) = prefs.slice_view_offset_x {
        window.set_slice_view_offset_x(offset_x);
    }
    if let Some(offset_y) = prefs.slice_view_offset_y {
        window.set_slice_view_offset_y(offset_y);
    }
    if let Some(width) = prefs.slice_view_width {
        window.set_slice_view_width(width);
    }
    if let Some(height) = prefs.slice_view_height {
        window.set_slice_view_height(height);
    }
}

#[cfg(test)]
mod window_prefs_tests {
    use super::*;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    fn new_main_window() -> MainWindow {
        MainWindow::new().expect("the headless platform creates a main window")
    }

    /// Reads offset x, offset y, width and height of the object list, then of
    /// the Slice control panel, then of the slice view.
    fn offsets_and_sizes(window: &MainWindow) -> [f32; 12] {
        [
            window.get_object_list_offset_x(),
            window.get_object_list_offset_y(),
            window.get_object_list_width(),
            window.get_object_list_height(),
            window.get_slice_panel_offset_x(),
            window.get_slice_panel_offset_y(),
            window.get_slice_panel_width(),
            window.get_slice_panel_height(),
            window.get_slice_view_offset_x(),
            window.get_slice_view_offset_y(),
            window.get_slice_view_width(),
            window.get_slice_view_height(),
        ]
    }

    /// Reads the object list flags (visible, minimized, anchor right, anchor
    /// bottom), the Slice control panel flags (anchor right, anchor bottom) and
    /// the slice view flags (visible, minimized, anchor right, anchor bottom).
    fn panel_flags(window: &MainWindow) -> [bool; 10] {
        [
            window.get_show_object_list(),
            window.get_object_list_minimized(),
            window.get_object_list_anchor_right(),
            window.get_object_list_anchor_bottom(),
            window.get_slice_panel_anchor_right(),
            window.get_slice_panel_anchor_bottom(),
            window.get_show_slice_view(),
            window.get_slice_view_minimized(),
            window.get_slice_view_anchor_right(),
            window.get_slice_view_anchor_bottom(),
        ]
    }

    #[test]
    fn restore_panels_pushes_every_stored_offset_and_size() {
        let read = on_fresh_slint_thread(|| {
            let window = new_main_window();
            restore_panels(
                &window,
                &PreferencesSnapshot {
                    object_list_offset_x: Some(31.0),
                    object_list_offset_y: Some(32.0),
                    object_list_width: Some(233.0),
                    object_list_height: Some(234.0),
                    slice_panel_offset_x: Some(41.0),
                    slice_panel_offset_y: Some(42.0),
                    slice_panel_width: Some(243.0),
                    slice_panel_height: Some(244.0),
                    slice_view_offset_x: Some(51.0),
                    slice_view_offset_y: Some(52.0),
                    slice_view_width: Some(253.0),
                    slice_view_height: Some(254.0),
                    ..PreferencesSnapshot::for_tests()
                },
            );
            offsets_and_sizes(&window)
        });
        assert_eq!(
            read,
            [
                31.0, 32.0, 233.0, 234.0, 41.0, 42.0, 243.0, 244.0, 51.0, 52.0, 253.0, 254.0
            ]
        );
    }

    #[test]
    fn restore_panels_keeps_the_slint_default_of_each_unsaved_offset_and_size() {
        let (before, after) = on_fresh_slint_thread(|| {
            let window = new_main_window();
            let before = offsets_and_sizes(&window);
            restore_panels(
                &window,
                &PreferencesSnapshot {
                    object_list_offset_x: None,
                    object_list_offset_y: None,
                    object_list_width: None,
                    object_list_height: None,
                    slice_panel_offset_x: None,
                    slice_panel_offset_y: None,
                    slice_panel_width: None,
                    slice_panel_height: None,
                    slice_view_offset_x: None,
                    slice_view_offset_y: None,
                    slice_view_width: None,
                    slice_view_height: None,
                    ..PreferencesSnapshot::for_tests()
                },
            );
            (before, offsets_and_sizes(&window))
        });
        assert_eq!(after, before);
    }

    #[test]
    fn restore_panels_pushes_the_visibility_anchors_and_minimized_flags() {
        let (before, after) = on_fresh_slint_thread(|| {
            let window = new_main_window();
            let before = panel_flags(&window);
            restore_panels(
                &window,
                &PreferencesSnapshot {
                    object_list_visible: false,
                    object_list_minimized: true,
                    object_list_anchor_right: false,
                    object_list_anchor_bottom: true,
                    slice_panel_anchor_right: true,
                    slice_panel_anchor_bottom: true,
                    slice_view_visible: false,
                    slice_view_minimized: true,
                    slice_view_anchor_right: true,
                    slice_view_anchor_bottom: false,
                    ..PreferencesSnapshot::for_tests()
                },
            );
            (before, panel_flags(&window))
        });
        // The stored flags are the opposite of the Slint defaults, so every
        // flag must change.
        assert_eq!(
            before,
            [
                true, false, true, false, false, false, true, false, false, true
            ]
        );
        assert_eq!(
            after,
            [
                false, true, false, true, true, true, false, true, true, false
            ]
        );
    }
}

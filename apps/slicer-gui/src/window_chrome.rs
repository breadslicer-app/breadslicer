/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The app's own title bar (#507). On Windows the main window drops the
//! system's frame: the top bar carries the caption buttons and is where the
//! window is dragged from, and the window's edges are where it is resized.
//! Elsewhere the system's frame stays.

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;

use crate::MainWindow;

/// Whether this platform gets the app's own title bar.
pub(crate) const ON: bool = cfg!(target_os = "windows");

/// Turns the app's title bar on for `ui` and wires its buttons. Before the
/// window is shown.
pub(crate) fn wire(ui: &MainWindow) {
    if !ON {
        return;
    }
    ui.set_custom_chrome(true);
    let weak = ui.as_weak();
    ui.on_window_drag(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().with_winit_window(|w| {
                let _ = w.drag_window();
            });
        }
    });
    let weak = ui.as_weak();
    ui.on_window_minimize(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_window_maximize_toggle(move || {
        if let Some(ui) = weak.upgrade() {
            let maximized = !ui.window().is_maximized();
            ui.window().set_maximized(maximized);
            ui.set_window_maximized(maximized);
        }
    });
    let weak = ui.as_weak();
    ui.on_window_close(move || {
        if let Some(ui) = weak.upgrade() {
            // The same way as the system's close button: the window's close
            // handler asks about unsaved changes.
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
        }
    });
    let weak = ui.as_weak();
    ui.on_window_resize(move |side| {
        let Some(direction) = direction(side) else {
            return;
        };
        if let Some(ui) = weak.upgrade() {
            ui.window().with_winit_window(|w| {
                let _ = w.drag_resize_window(direction);
            });
        }
    });
}

/// After the window is shown: the shadow a framed window has.
pub(crate) fn after_show(ui: &MainWindow) {
    if !ON {
        return;
    }
    #[cfg(target_os = "windows")]
    ui.window().with_winit_window(|w| {
        use slint::winit_030::winit::platform::windows::WindowExtWindows;
        w.set_undecorated_shadow(true);
    });
    note_size(ui);
}

/// Keeps the maximise button in step with the window: a drag to the top of
/// the screen or a double click maximises it outside the button.
pub(crate) fn note_size(ui: &MainWindow) {
    if ON {
        ui.set_window_maximized(ui.window().is_maximized());
    }
}

/// The edge `side` names, as the window's resize callback numbers them.
fn direction(side: i32) -> Option<ResizeDirection> {
    Some(match side {
        0 => ResizeDirection::North,
        1 => ResizeDirection::South,
        2 => ResizeDirection::East,
        3 => ResizeDirection::West,
        4 => ResizeDirection::NorthEast,
        5 => ResizeDirection::NorthWest,
        6 => ResizeDirection::SouthEast,
        7 => ResizeDirection::SouthWest,
        _ => return None,
    })
}

#[cfg(test)]
mod window_chrome_tests {
    use super::*;

    #[test]
    fn every_edge_names_a_direction_and_nothing_else_does() {
        for side in 0..8 {
            assert!(direction(side).is_some(), "{side}");
        }
        assert!(direction(8).is_none());
        assert!(direction(-1).is_none());
    }
}

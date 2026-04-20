/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::rc::Rc;

use slint::{CloseRequestResponse, ComponentHandle};

use crate::about_window::build_about_window;
use crate::preferences_window::build_preferences_window;
use crate::viewport_controller::ViewportController;
use crate::{AboutWindow, MainWindow, PreferencesWindow, ThemePreference};

/// Owns every piece of state that is app-wide (not tied to a single
/// `MainWindow`):
///
/// - a lazily-created, app-wide `AboutWindow` shared across every main window;
/// - a lazily-created, app-wide `PreferencesWindow` shared across every main
///   window;
/// - strong handles to each live `MainWindow`, so they aren't dropped while
///   the event loop is running.
///
/// Per-window state (scene, camera, GPU renderer, per-window error dialogs)
/// stays inside `ViewportController`, one instance per `MainWindow`. Closures
/// installed on a `MainWindow` that need to reach `AppState` hold a
/// `Weak<AppState>` so the window→closure→AppState→window graph doesn't form
/// a strong-reference cycle.
pub(crate) struct AppState {
    about_window: RefCell<Option<AboutWindow>>,
    preferences_window: RefCell<Option<PreferencesWindow>>,
    main_windows: RefCell<HashMap<u64, MainWindow>>,
    next_window_id: Cell<u64>,
    current_theme: Cell<ThemePreference>,
}

impl AppState {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            about_window: RefCell::new(None),
            preferences_window: RefCell::new(None),
            main_windows: RefCell::new(HashMap::new()),
            next_window_id: Cell::new(0),
            current_theme: Cell::new(ThemePreference::System),
        })
    }

    /// Creates, wires, and shows a new `MainWindow`. Per-window Slint
    /// callbacks (viewport input, rendering notifier, file open/close,
    /// resize debounce) are installed by `crate::wiring::wire_main_window`;
    /// the app-level "About" and "New Window" menu items and the OS
    /// close-request handler are wired here.
    pub fn spawn_main_window(self: &Rc<Self>) -> Result<(), Box<dyn Error>> {
        let ui = MainWindow::new()?;
        ui.set_theme(self.current_theme.get());
        let ctrl = Rc::new(ViewportController::new(&ui));

        crate::wiring::wire_main_window(&ui, ctrl)?;
        self.wire_app_level_menus(&ui);

        let id = self.next_window_id.get();
        self.next_window_id.set(id.wrapping_add(1));

        // On OS-level close: drop our strong `MainWindow` handle so the
        // window's `ViewportController`, `Renderer`, and scene can be
        // released. Without this, hidden windows accumulate for the whole
        // session.
        let app_weak = Rc::downgrade(self);
        ui.window().on_close_requested(move || {
            if let Some(app) = app_weak.upgrade() {
                app.main_windows.borrow_mut().remove(&id);
            }
            CloseRequestResponse::HideWindow
        });

        ui.show()?;
        self.main_windows.borrow_mut().insert(id, ui);
        Ok(())
    }

    fn wire_app_level_menus(self: &Rc<Self>, ui: &MainWindow) {
        // Closures hold `Weak<AppState>` so the MainWindow → closure →
        // AppState → main_windows(MainWindow) chain isn't a strong cycle.
        {
            let app = Rc::downgrade(self);
            ui.on_show_about_requested(move || {
                if let Some(app) = app.upgrade() {
                    app.show_about();
                }
            });
        }
        {
            let app = Rc::downgrade(self);
            ui.on_show_preferences_requested(move || {
                if let Some(app) = app.upgrade() {
                    app.show_preferences();
                }
            });
        }
        {
            let app = Rc::downgrade(self);
            ui.on_new_window_requested(move || {
                let Some(app) = app.upgrade() else {
                    return;
                };
                if let Err(err) = app.spawn_main_window() {
                    eprintln!("Failed to open new window: {err}");
                }
            });
        }
    }

    /// Shows the app-wide About window, creating it on first use and
    /// re-showing the same instance on subsequent calls.
    fn show_about(&self) {
        let mut slot = self.about_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.show();
            return;
        }
        match build_about_window() {
            Ok(about) => {
                if let Err(err) = about.show() {
                    eprintln!("Failed to show About window: {err}");
                    return;
                }
                *slot = Some(about);
            }
            Err(err) => eprintln!("Failed to create About window: {err}"),
        }
    }

    /// Shows the app-wide Preferences window, creating it on first use and
    /// re-showing the same instance on subsequent calls.
    fn show_preferences(self: &Rc<Self>) {
        let mut slot = self.preferences_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.show();
            return;
        }
        let app_weak = Rc::downgrade(self);
        let on_theme_changed = move |t| {
            if let Some(app) = app_weak.upgrade() {
                app.apply_theme(t);
            }
        };
        match build_preferences_window(on_theme_changed) {
            Ok(prefs) => {
                prefs.set_theme(self.current_theme.get());
                if let Err(err) = prefs.show() {
                    eprintln!("Failed to show Preferences window: {err}");
                    return;
                }
                *slot = Some(prefs);
            }
            Err(err) => eprintln!("Failed to create Preferences window: {err}"),
        }
    }

    /// Records the new theme and pushes it to every live window so their
    /// per-tree `Palette.color-scheme` bindings update in lock-step.
    fn apply_theme(&self, new_theme: ThemePreference) {
        self.current_theme.set(new_theme);
        for w in self.main_windows.borrow().values() {
            w.set_theme(new_theme);
        }
        if let Some(prefs) = self.preferences_window.borrow().as_ref() {
            prefs.set_theme(new_theme);
        }
    }
}

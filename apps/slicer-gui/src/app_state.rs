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
use crate::viewport_controller::ViewportController;
use crate::{AboutWindow, MainWindow};

/// Owns every piece of state that is app-wide (not tied to a single
/// `MainWindow`):
///
/// - a lazily-created, app-wide `AboutWindow` shared across every main window;
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
    main_windows: RefCell<HashMap<u64, MainWindow>>,
    next_window_id: Cell<u64>,
}

impl AppState {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            about_window: RefCell::new(None),
            main_windows: RefCell::new(HashMap::new()),
            next_window_id: Cell::new(0),
        })
    }

    /// Creates, wires, and shows a new `MainWindow`. Any per-window Slint
    /// callbacks (viewport input, rendering notifier, file open/close, and
    /// the app-level "About" / "New Window" menu items) get installed here.
    pub fn spawn_main_window(self: &Rc<Self>) -> Result<(), Box<dyn Error>> {
        let ui = MainWindow::new()?;
        let ctrl = Rc::new(ViewportController::new(&ui));

        crate::install_rendering_notifier(&ui, ctrl.clone())?;
        crate::wire_close_scene(&ui, ctrl.clone());
        crate::wire_open_file(&ui, ctrl.clone());
        crate::wire_viewport_resized(&ui, ctrl.clone());
        crate::wire_viewport_input(&ui, ctrl);
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
}

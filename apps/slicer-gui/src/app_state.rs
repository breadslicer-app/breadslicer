/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::rc::Rc;

use slint::{CloseRequestResponse, ComponentHandle};

use crate::MainWindow;
use crate::about_window::{WiredAboutWindow, build_about_window};
use crate::app_settings::AppSettings;
use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::preferences_window::{WiredPreferencesWindow, build_preferences_window};
use crate::scene_document::SceneDocument;
use crate::viewport::ViewportController;
use crate::window_prefs::WindowPreferences;

/// A main window paired with its preference-bus subscription. Drop of
/// this struct drops the subscription, unregistering the window's
/// preference-apply listener. Held by value in `AppState::main_windows`
/// so closing a window via `on_close_requested` cleans up both halves
/// in one step. The `window` field is load-bearing — holding the Slint
/// handle by value keeps the component alive — even though the only
/// accesses to the live window happen through the `Weak<MainWindow>`
/// captured in the subscription's listener.
struct WiredMainWindow {
    #[allow(dead_code)]
    window: MainWindow,
    _subscription: Subscription,
}

/// Owns every piece of state that is app-wide (not tied to a single
/// `MainWindow`):
///
/// - a lazily-created, app-wide `AboutWindow` shared across every main
///   window;
/// - a lazily-created, app-wide `PreferencesWindow` shared across
///   every main window;
/// - the `PreferencesBus`, which owns the canonical theme / language
///   state and notifies every live window on change;
/// - strong handles to each live `MainWindow`, so they aren't dropped
///   while the event loop is running.
///
/// Per-window state (scene, camera, GPU renderer, per-window error
/// dialogs) stays inside `ViewportController`, one instance per
/// `MainWindow`. Closures installed on a `MainWindow` that need to
/// reach `AppState` hold a `Weak<AppState>` so the
/// window→closure→AppState→window graph doesn't form a strong-
/// reference cycle.
pub(crate) struct AppState {
    about_window: RefCell<Option<WiredAboutWindow>>,
    preferences_window: RefCell<Option<WiredPreferencesWindow>>,
    main_windows: RefCell<HashMap<u64, WiredMainWindow>>,
    next_window_id: Cell<u64>,
    prefs: Rc<PreferencesBus>,
    // Bus subscriptions owned by AppState. They run on every
    // `bus.update`; dropping them unregisters. Stored here so they
    // live as long as `AppState`. The settings subscription owns the
    // `AppSettings` it writes to, so it lives and dies with AppState.
    _settings_sub: Subscription,
    _language_apply_sub: Subscription,
}

impl AppState {
    pub fn new() -> Rc<Self> {
        let settings = AppSettings::load();
        let snapshot = PreferencesSnapshot {
            theme: settings.theme(),
            language: settings.language(),
        };
        // Apply the persisted language *once* up front so the very
        // first `@tr` evaluation during window construction uses the
        // right locale. `select_bundled_translation` is a no-op until
        // at least one Slint component exists, but the next
        // successful call after the first window is built will retry;
        // keeping this call here preserves the pre-bus behaviour.
        snapshot.language.apply();

        let prefs = PreferencesBus::new(snapshot);

        // Persistence: on any change, mirror the snapshot into
        // `AppSettings` and save. Captures the `AppSettings` by move
        // so its lifetime is tied to the subscription. Kept ahead of
        // window listeners so a disk-write failure happens before
        // any UI re-render.
        let settings_sub = {
            let settings = RefCell::new(settings);
            prefs.subscribe(move |snap, _change| {
                let mut settings = settings.borrow_mut();
                settings.set_theme(snap.theme);
                settings.set_language(snap.language);
                if let Err(err) = settings.save() {
                    eprintln!("Failed to save settings: {err}");
                }
            })
        };

        // Language side-effect: flip the process-global bundled
        // translation before per-window listeners re-render. The
        // `PreferencesWindow`'s listener rebuilds its language combo
        // against the *active* translation, so this must come first.
        let language_apply_sub = prefs.subscribe(|snap, change| {
            if *change == PreferencesChange::Language {
                snap.language.apply();
            }
        });

        Rc::new(Self {
            about_window: RefCell::new(None),
            preferences_window: RefCell::new(None),
            main_windows: RefCell::new(HashMap::new()),
            next_window_id: Cell::new(0),
            prefs,
            _settings_sub: settings_sub,
            _language_apply_sub: language_apply_sub,
        })
    }

    /// Creates, wires, and shows a new `MainWindow`. Per-window Slint
    /// callbacks (viewport input, rendering notifier, file open/close,
    /// resize debounce) are installed by `crate::wiring::wire_main_window`;
    /// the app-level "About" and "New Window" menu items and the OS
    /// close-request handler are wired here.
    pub fn spawn_main_window(self: &Rc<Self>) -> Result<(), Box<dyn Error>> {
        let ui = MainWindow::new()?;
        ui.apply_preferences(&self.prefs.snapshot());
        let document = SceneDocument::new();
        let ctrl = Rc::new(ViewportController::new(&ui, document));
        ctrl.register_document_listener();

        crate::wiring::wire_main_window(&ui, ctrl)?;
        self.wire_app_level_menus(&ui);

        let id = self.next_window_id.get();
        self.next_window_id.set(id.wrapping_add(1));

        let ui_weak = ui.as_weak();
        let subscription = self.prefs.subscribe(move |snap, _change| {
            if let Some(w) = ui_weak.upgrade() {
                w.apply_preferences(snap);
            }
        });

        // On OS-level close: drop our strong handle so the window's
        // `ViewportController`, `Renderer`, and scene — and the
        // preference subscription — can be released. Without this,
        // hidden windows accumulate for the whole session.
        let app_weak = Rc::downgrade(self);
        ui.window().on_close_requested(move || {
            if let Some(app) = app_weak.upgrade() {
                app.main_windows.borrow_mut().remove(&id);
            }
            CloseRequestResponse::HideWindow
        });

        ui.show()?;
        self.main_windows.borrow_mut().insert(
            id,
            WiredMainWindow {
                window: ui,
                _subscription: subscription,
            },
        );
        Ok(())
    }

    fn wire_app_level_menus(self: &Rc<Self>, ui: &MainWindow) {
        // Closures hold `Weak<AppState>` so the MainWindow → closure
        // → AppState → main_windows(MainWindow) chain isn't a strong
        // cycle.
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
    /// re-showing the same instance on subsequent calls. The window's
    /// bus subscription keeps it up-to-date while hidden, so no extra
    /// re-apply is needed here.
    fn show_about(&self) {
        let mut slot = self.about_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.window.show();
            return;
        }
        match build_about_window(&self.prefs) {
            Ok(wired) => {
                if let Err(err) = wired.window.show() {
                    eprintln!("Failed to show About window: {err}");
                    return;
                }
                *slot = Some(wired);
            }
            Err(err) => eprintln!("Failed to create About window: {err}"),
        }
    }

    /// Shows the app-wide Preferences window, creating it on first
    /// use and re-showing the same instance on subsequent calls. The
    /// window's bus subscription keeps it up-to-date while hidden.
    fn show_preferences(&self) {
        let mut slot = self.preferences_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.window.show();
            return;
        }
        match build_preferences_window(&self.prefs) {
            Ok(wired) => {
                if let Err(err) = wired.window.show() {
                    eprintln!("Failed to show Preferences window: {err}");
                    return;
                }
                *slot = Some(wired);
            }
            Err(err) => eprintln!("Failed to create Preferences window: {err}"),
        }
    }
}

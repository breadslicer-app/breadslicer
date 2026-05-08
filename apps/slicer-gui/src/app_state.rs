/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;

use slint::{CloseRequestResponse, ComponentHandle};

use crate::MainWindow;
use crate::about_window::{WiredAboutWindow, build_about_window};
use crate::app_settings::AppSettings;
use crate::error_dialog::ReplaceSceneChoice;
use crate::i18n::ctr;
use crate::mesh_loader::{
    SUPPORTED_MESH_EXTENSIONS, is_supported_mesh_extension, start_load_with_ticket,
};
use crate::pending_load::PendingLoad;
use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::preferences_window::{WiredPreferencesWindow, build_preferences_window};
use crate::scene_document::SceneDocument;
use crate::scene_export::{default_export_extension, default_export_stem};
use crate::viewport::ViewportController;
use crate::window_prefs::WindowPreferences;

/// A main window paired with its viewport controller and preference-
/// bus subscription. Drop of this struct drops the subscription,
/// unregistering the window's preference-apply listener. Held by value
/// in `AppState::main_windows` so closing a window via
/// `on_close_requested` cleans up all three halves in one step. The
/// `window` field is load-bearing: holding the Slint handle by value
/// keeps the component alive, even though the only accesses to the
/// live window happen through the `Weak<MainWindow>` captured in the
/// subscription's listener. The `controller` is held so external entry
/// points (path-driven loads from argv / drag-and-drop) can resolve
/// the window's `SceneLoader` from the window id.
struct WiredMainWindow {
    #[allow(dead_code)]
    window: MainWindow,
    controller: Rc<ViewportController>,
    // Shared with the controller. Writer side: external entry points
    // park a path here when the GPU device isn't yet available. The
    // controller drains it from `handle_rendering_setup`.
    pending_load: Rc<PendingLoad>,
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
            build_volume: settings.build_volume(),
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
                settings.set_build_volume(snap.build_volume);
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
    pub fn spawn_main_window(self: &Rc<Self>) -> Result<u64, Box<dyn Error>> {
        let ui = MainWindow::new()?;
        let snapshot = self.prefs.snapshot();
        ui.apply_preferences(&snapshot);
        let document = SceneDocument::new();
        let pending_load = Rc::new(PendingLoad::new());
        let ctrl = Rc::new(ViewportController::new(&ui, document, pending_load.clone()));
        ctrl.register_document_listener();
        // Bus listeners only fire on `update`, not on subscribe, so
        // the persisted build volume is pushed imperatively here.
        // Future runtime edits flow in through the subscription below.
        ctrl.set_build_volume(snapshot.build_volume);

        crate::wiring::wire_main_window(&ui, ctrl.clone())?;
        self.wire_app_level_menus(&ui);

        let id = self.next_window_id.get();
        self.next_window_id.set(id.wrapping_add(1));

        let ui_weak = ui.as_weak();
        let ctrl_for_listener = ctrl.clone();
        let subscription = self.prefs.subscribe(move |snap, change| {
            if let Some(w) = ui_weak.upgrade() {
                w.apply_preferences(snap);
            }
            if matches!(change, PreferencesChange::BuildVolume) {
                ctrl_for_listener.set_build_volume(snap.build_volume);
            }
        });

        // On OS-level close: drop our strong handle so the window's
        // `ViewportController`, `Renderer`, scene, and the preference
        // subscription can be released. Without this, hidden windows
        // accumulate for the whole session.
        {
            let app_weak = Rc::downgrade(self);
            ui.window().on_close_requested(move || {
                if let Some(app) = app_weak.upgrade() {
                    app.main_windows.borrow_mut().remove(&id);
                }
                CloseRequestResponse::HideWindow
            });
        }

        // Drag-and-drop: route the winit `DroppedFile` event for this
        // window's surface into the shared `open_path_in_window`
        // entry, and toggle the `drop-hovering` overlay on hover
        // events. Wayland users won't see any of this until winit
        // lands its Wayland DnD support upstream; see the open-with
        // plan for context.
        {
            use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
            let app_weak = Rc::downgrade(self);
            let ui_weak = ui.as_weak();
            ui.window().on_winit_window_event(move |_window, event| {
                match event {
                    winit::event::WindowEvent::HoveredFile(_) => {
                        if let Some(w) = ui_weak.upgrade() {
                            w.set_drop_hovering(true);
                        }
                    }
                    winit::event::WindowEvent::HoveredFileCancelled => {
                        if let Some(w) = ui_weak.upgrade() {
                            w.set_drop_hovering(false);
                        }
                    }
                    winit::event::WindowEvent::DroppedFile(path) => {
                        if let Some(w) = ui_weak.upgrade() {
                            w.set_drop_hovering(false);
                        }
                        if let Some(app) = app_weak.upgrade() {
                            app.open_path_in_window(id, path.clone());
                        }
                    }
                    _ => {}
                }
                EventResult::Propagate
            });
        }

        ui.show()?;
        self.main_windows.borrow_mut().insert(
            id,
            WiredMainWindow {
                window: ui,
                controller: ctrl,
                pending_load,
                _subscription: subscription,
            },
        );
        Ok(id)
    }

    /// Routes an external (non-menu) open request into the right
    /// place. Used by argv at startup, drag-and-drop, future macOS
    /// `openURLs`. Branches:
    ///
    /// 1. Unrecognised window id (window closed since the request was
    ///    queued): silently drop.
    /// 2. Unsupported file extension: surface an error dialog
    ///    (decision #3) and return.
    /// 3. Window already has a loaded scene: surface the
    ///    [`ReplaceSceneDialog`] (decision #4). On Discard, close and
    ///    load the new path; on Export, run the existing "Export As…"
    ///    flow and only on a successful save close + load; on Cancel,
    ///    do nothing.
    /// 4. Otherwise dispatch through [`Self::dispatch_path_to_loader`],
    ///    which queues the path on the window's `PendingLoad` slot if
    ///    the GPU device isn't ready yet, or starts the load
    ///    immediately if it is.
    pub(crate) fn open_path_in_window(self: &Rc<Self>, window_id: u64, path: PathBuf) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };

        if !is_supported_mesh_extension(&path) {
            controller.dialogs().show_error(
                ctr!("ErrorDialog", "Unsupported file type").as_str(),
                Some(ctr!("ErrorDialog", "File: {}", path.display()).to_string()),
                ctr!(
                    "ErrorDialog",
                    "Only the following file types can be opened: {}",
                    SUPPORTED_MESH_EXTENSIONS.join(", ")
                )
                .to_string(),
                String::new(),
            );
            return;
        }

        if !controller.document().is_loaded() {
            self.dispatch_path_to_loader(window_id, path);
            return;
        }

        // Replace-confirm flow. The choice callback fires on the UI
        // thread once the user picks a button. `Weak<AppState>` keeps
        // the closure non-cyclic; if the app is gone by then, every
        // branch silently drops.
        let app_weak = Rc::downgrade(self);
        let source_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        // `can_undo` is true iff the user has applied at least one
        // command on the loaded scene that hasn't been undone: our
        // proxy for "this scene has unsaved changes". If they haven't
        // touched it, exporting would just write a copy of the source
        // file they already have, so the Export button is hidden.
        let can_export = controller.document().can_undo();
        controller
            .dialogs()
            .show_replace_scene_confirm(source_name, can_export, move |choice| match choice {
                ReplaceSceneChoice::Discard => {
                    if let Some(app) = app_weak.upgrade() {
                        app.discard_then_open(window_id, path);
                    }
                }
                ReplaceSceneChoice::Export => {
                    if let Some(app) = app_weak.upgrade() {
                        app.export_then_open(window_id, path);
                    }
                }
                ReplaceSceneChoice::Cancel => {}
            });
    }

    /// Inner queue-or-load step. Called from `open_path_in_window`
    /// after extension validation and the replace-confirm branch, and
    /// from the post-Discard / post-Export callbacks below. Caller is
    /// responsible for ensuring any prior scene has been closed.
    fn dispatch_path_to_loader(&self, window_id: u64, path: PathBuf) {
        let (loader, pending) = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            (wired.controller.loader(), wired.pending_load.clone())
        };
        if loader.gpu_snapshot().is_none() {
            // Pre-first-paint: the controller will dispatch this once
            // `handle_rendering_setup` publishes the device.
            pending.queue(path);
            return;
        }
        let Some(ticket) = loader.begin_load() else {
            // Decision #2: drop onto a busy window is silently ignored.
            return;
        };
        start_load_with_ticket(ticket, path, &loader);
    }

    /// Replace-confirm Discard branch: clear the current scene then
    /// dispatch the new path. Runs synchronously on the UI thread.
    fn discard_then_open(&self, window_id: u64, path: PathBuf) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };
        controller.on_close_scene();
        self.dispatch_path_to_loader(window_id, path);
    }

    /// Replace-confirm Export branch: run the existing "Export As…"
    /// flow and, only on a successful save, close the current scene
    /// and dispatch the new path. Cancellation of the export dialog
    /// or a write failure leaves the current scene intact (the
    /// failure dialog is surfaced by `run_export_as_flow` itself).
    fn export_then_open(self: &Rc<Self>, window_id: u64, path: PathBuf) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };
        let doc = controller.document();
        let dialogs = controller.dialogs();
        let stem = default_export_stem(&doc);
        let ext = default_export_extension(&doc);
        let app_weak = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            let saved = crate::wiring::run_export_as_flow(doc, dialogs, stem, ext).await;
            if !saved {
                return;
            }
            if let Some(app) = app_weak.upgrade() {
                app.discard_then_open(window_id, path);
            }
        }) {
            eprintln!("Failed to spawn Export & Open task: {err}");
        }
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

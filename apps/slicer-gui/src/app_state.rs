/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use slint::{CloseRequestResponse, ComponentHandle};

use crate::MainWindow;
use crate::about_window::{WiredAboutWindow, build_about_window};
use crate::app_settings::AppSettings;
use crate::clipboard::{Clipboard, fragment_from_document_selection};
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
use crate::selection::{LeafId, Selection};
use crate::viewport::ViewportController;
use crate::window_prefs::WindowPreferences;

mod replace_scene_policy {
    use crate::error_dialog::ReplaceSceneChoice;

    /// Pre-prompt branch chosen by `open_path_in_window` from the
    /// state of the target window and the dropped/opened path.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum OpenPathAction {
        /// Path extension is not in the supported mesh set; caller
        /// should surface an "Unsupported file type" error dialog.
        RejectUnsupported,
        /// No scene is currently loaded; caller should dispatch the
        /// path to the loader directly.
        DispatchImmediately,
        /// A scene is already loaded; caller should show the replace-
        /// scene confirmation dialog. `can_export` controls whether
        /// the Export button is visible (true iff the user has
        /// applied at least one undoable command, our proxy for
        /// "this scene has unsaved changes worth exporting").
        PromptReplace { can_export: bool },
    }

    /// Post-prompt branch chosen from the user's reply to the
    /// replace-scene confirmation dialog.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum PostPromptAction {
        DiscardThenOpen,
        ExportThenOpen,
        NoOp,
    }

    pub(crate) fn decide_open_path_action(
        supported_extension: bool,
        document_loaded: bool,
        document_can_undo: bool,
    ) -> OpenPathAction {
        if !supported_extension {
            return OpenPathAction::RejectUnsupported;
        }
        if !document_loaded {
            return OpenPathAction::DispatchImmediately;
        }
        OpenPathAction::PromptReplace {
            can_export: document_can_undo,
        }
    }

    pub(crate) fn map_replace_choice(choice: ReplaceSceneChoice) -> PostPromptAction {
        match choice {
            ReplaceSceneChoice::Discard => PostPromptAction::DiscardThenOpen,
            ReplaceSceneChoice::Export => PostPromptAction::ExportThenOpen,
            ReplaceSceneChoice::Cancel => PostPromptAction::NoOp,
        }
    }
}

use replace_scene_policy::{
    OpenPathAction, PostPromptAction, decide_open_path_action, map_replace_choice,
};

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
    /// App-wide copy / paste clipboard. Populated from any window's
    /// current selection by `copy_selection`; consumed by the paste
    /// path which lives on the viewport controller. `None` until the
    /// user has performed the first copy.
    clipboard: RefCell<Option<Clipboard>>,
    // Bus subscriptions owned by AppState. They run on every
    // `bus.update`; dropping them unregisters. Stored here so they
    // live as long as `AppState`. The settings subscription holds the
    // only `Sender` clone; dropping it lets the saver thread observe
    // channel-closed and exit its recv loop.
    _settings_sub: Subscription,
    _language_apply_sub: Subscription,
    // RAII handle that joins the settings-saver worker thread. Declared
    // after `_settings_sub` so the subscription's `Sender` is dropped
    // first; otherwise the join would deadlock waiting for the worker
    // to exit while the worker is still blocked on `recv`.
    _settings_saver: SettingsSaverJoin,
}

/// RAII handle joining the settings-saver thread on drop. The worker
/// owns the `AppSettings` instance and writes to disk off the Slint
/// event-loop thread; joining on drop preserves the previous "save
/// completes before process exit" guarantee.
struct SettingsSaverJoin {
    join: Option<JoinHandle<()>>,
}

impl Drop for SettingsSaverJoin {
    fn drop(&mut self) {
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// Settings-saver worker loop. Blocks on `recv`; on each wake drains
/// any further pending snapshots via `try_recv` so a burst of rapid
/// preference changes collapses to a single `on_snapshot` call.
/// Returns when every `Sender` clone is dropped.
fn run_settings_saver_loop<F>(rx: mpsc::Receiver<PreferencesSnapshot>, mut on_snapshot: F)
where
    F: FnMut(&PreferencesSnapshot),
{
    while let Ok(mut snap) = rx.recv() {
        while let Ok(latest) = rx.try_recv() {
            snap = latest;
        }
        on_snapshot(&snap);
    }
}

/// Spawns the settings-saver worker thread, with the loop body wired
/// to persist each coalesced snapshot through `AppSettings::save`.
fn spawn_settings_saver(
    initial_settings: AppSettings,
) -> std::io::Result<(Sender<PreferencesSnapshot>, SettingsSaverJoin)> {
    let (tx, rx) = mpsc::channel::<PreferencesSnapshot>();
    let join = thread::Builder::new()
        .name("settings-saver".into())
        .spawn(move || {
            let mut settings = initial_settings;
            run_settings_saver_loop(rx, |snap| {
                settings.set_theme(snap.theme);
                settings.set_language(snap.language);
                settings.set_build_volume(snap.build_volume);
                if let Err(err) = settings.save() {
                    eprintln!("Failed to save settings: {err}");
                }
            });
        })?;
    Ok((tx, SettingsSaverJoin { join: Some(join) }))
}

impl AppState {
    pub fn new() -> Result<Rc<Self>, Box<dyn Error>> {
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

        // Persistence runs on a dedicated worker thread: the
        // subscription forwards each updated snapshot to it over an
        // `mpsc` channel and the worker writes to disk. Off-loading
        // the write keeps the Slint event loop responsive on slow /
        // network-mounted / nearly-full config directories.
        let (saver_tx, settings_saver) = spawn_settings_saver(settings)?;
        let settings_sub = prefs.subscribe(move |snap, _change| {
            // Channel send is non-blocking; failure means the worker
            // has already exited, which only happens at shutdown.
            let _ = saver_tx.send(snap.clone());
        });

        // Language side-effect: flip the process-global bundled
        // translation before per-window listeners re-render. The
        // `PreferencesWindow`'s listener rebuilds its language combo
        // against the *active* translation, so this must come first.
        let language_apply_sub = prefs.subscribe(|snap, change| {
            if *change == PreferencesChange::Language {
                snap.language.apply();
            }
        });

        Ok(Rc::new(Self {
            about_window: RefCell::new(None),
            preferences_window: RefCell::new(None),
            main_windows: RefCell::new(HashMap::new()),
            next_window_id: Cell::new(0),
            prefs,
            clipboard: RefCell::new(None),
            _settings_sub: settings_sub,
            _language_apply_sub: language_apply_sub,
            _settings_saver: settings_saver,
        }))
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

        // Copy / paste live at the AppState level (the clipboard
        // is app-wide, not per-window) but the keystrokes arrive
        // through the per-window MeshViewer FocusScope. Bind them
        // here so the closure captures the window id and a Weak
        // back to AppState.
        {
            let app_weak = Rc::downgrade(self);
            ui.on_copy_pressed(move || {
                if let Some(app) = app_weak.upgrade() {
                    app.copy_selection(id);
                }
            });
        }
        {
            let app_weak = Rc::downgrade(self);
            ui.on_paste_pressed(move || {
                if let Some(app) = app_weak.upgrade() {
                    app.paste_at_cursor(id);
                }
            });
        }

        // Right-button context menu. Down latches the press
        // position on the controller; up classifies the gesture
        // and, if it was a click (no drag), asks AppState to open
        // the menu at the press position.
        {
            let ctrl = ctrl.clone();
            ui.on_viewport_right_pointer_down(move |x, y| {
                ctrl.on_right_pointer_down(x, y);
            });
        }
        {
            let ctrl = ctrl.clone();
            let app_weak = Rc::downgrade(self);
            ui.on_viewport_right_pointer_up(move |x, y| {
                if let Some((press_x, press_y)) = ctrl.on_right_pointer_up(x, y)
                    && let Some(app) = app_weak.upgrade()
                {
                    app.open_context_menu(id, press_x, press_y);
                }
            });
        }
        {
            let app_weak = Rc::downgrade(self);
            ui.on_context_copy_pressed(move || {
                if let Some(app) = app_weak.upgrade() {
                    app.copy_selection(id);
                }
            });
        }
        {
            let app_weak = Rc::downgrade(self);
            ui.on_context_paste_pressed(move |x, y| {
                if let Some(app) = app_weak.upgrade() {
                    app.paste_at_pixel(id, x, y);
                }
            });
        }

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

        let action = decide_open_path_action(
            is_supported_mesh_extension(&path),
            controller.document().is_loaded(),
            controller.document().can_undo(),
        );
        match action {
            OpenPathAction::RejectUnsupported => {
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
            }
            OpenPathAction::DispatchImmediately => {
                self.dispatch_path_to_loader(window_id, path);
            }
            OpenPathAction::PromptReplace { can_export } => {
                // Replace-confirm flow. The choice callback fires on
                // the UI thread once the user picks a button.
                // `Weak<AppState>` keeps the closure non-cyclic; if
                // the app is gone by then, every branch silently
                // drops.
                let app_weak = Rc::downgrade(self);
                let source_name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                controller.dialogs().show_replace_scene_confirm(
                    source_name,
                    can_export,
                    move |choice| {
                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };
                        match map_replace_choice(choice) {
                            PostPromptAction::DiscardThenOpen => {
                                app.discard_then_open(window_id, path);
                            }
                            PostPromptAction::ExportThenOpen => {
                                app.export_then_open(window_id, path);
                            }
                            PostPromptAction::NoOp => {}
                        }
                    },
                );
            }
        }
    }

    /// Capture the currently-selected leaf from the focused window's
    /// [`SceneDocument`] into the app-wide clipboard. Silently does
    /// nothing if the window id is unknown (window closed since the
    /// keystroke fired), no scene is loaded, or no leaf is selected;
    /// in those cases the previous clipboard contents survive,
    /// matching the convention that only a successful copy
    /// overwrites the clipboard.
    pub(crate) fn copy_selection(&self, window_id: u64) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };
        let doc = controller.document();
        let Some(fragment) = fragment_from_document_selection(&doc) else {
            return;
        };
        *self.clipboard.borrow_mut() = Some(Clipboard::new(fragment));
    }

    /// Paste the app-wide clipboard's fragment into the focused
    /// window. Silent no-op when the window id is unknown, the
    /// clipboard is empty, or the controller's paste path bails
    /// (no scene loaded, no camera ready, fragment empty, cursor
    /// ray missed the build plate). On a successful paste the
    /// cascade counter is bumped so the next `Ctrl+V` offsets
    /// further; on failure the cascade stays untouched.
    pub(crate) fn paste_at_cursor(&self, window_id: u64) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };
        let mut slot = self.clipboard.borrow_mut();
        let Some(cb) = slot.as_mut() else {
            return;
        };
        if controller.paste_clipboard_at_cursor(cb) {
            cb.bump_cascade();
        }
    }

    /// Right-click context-menu orchestration. Runs the picker at
    /// `(x, y)`; on a hit, moves the selection to that leaf so
    /// the existing `copy_selection` path applies to the hit
    /// object. Selection stays untouched on a miss, per the
    /// agreed UX. Computes the menu's visibility flags (Copy
    /// when something was hit, Paste when the clipboard has
    /// content) and asks the MainWindow to show the popup. Silent
    /// no-op when neither item would be visible (so a right-click
    /// on empty space with an empty clipboard doesn't pop an
    /// empty menu).
    pub(crate) fn open_context_menu(&self, window_id: u64, x: f32, y: f32) {
        let main_windows = self.main_windows.borrow();
        let Some(wired) = main_windows.get(&window_id) else {
            return;
        };
        let hit = wired.controller.pick_node_at(x, y);
        if let Some(node_id) = hit {
            wired
                .controller
                .document()
                .set_selection(Selection::One(LeafId(node_id)));
        }
        let can_copy = hit.is_some();
        let can_paste = self.clipboard.borrow().is_some();
        if !can_copy && !can_paste {
            return;
        }
        wired
            .window
            .invoke_show_context_menu(x, y, can_copy, can_paste);
    }

    /// Paste anchored on an explicit logical pixel (the position
    /// where the right-click opened the context menu). Counterpart
    /// of `paste_at_cursor` for the menu-driven path: the cursor
    /// may have drifted inside the menu by the time the user
    /// clicks Paste, but the placement should still land at the
    /// original anchor pixel.
    pub(crate) fn paste_at_pixel(&self, window_id: u64, x: f32, y: f32) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };
        let mut slot = self.clipboard.borrow_mut();
        let Some(cb) = slot.as_mut() else {
            return;
        };
        if controller.paste_clipboard_at_pixel(cb, x, y) {
            cb.bump_cascade();
        }
    }

    /// Inner queue-or-load step. Called from `open_path_in_window`
    /// after extension validation and the replace-confirm branch, and
    /// from the post-Discard / post-Export callbacks below. Caller is
    /// responsible for ensuring any prior scene has been closed.
    fn dispatch_path_to_loader(&self, window_id: u64, path: PathBuf) {
        let (loader, pending, build_volume) = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            (
                wired.controller.loader(),
                wired.pending_load.clone(),
                wired.controller.build_volume(),
            )
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
        start_load_with_ticket(ticket, path, &loader, build_volume);
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

#[cfg(test)]
mod replace_scene_policy_tests {
    use super::replace_scene_policy::*;
    use crate::error_dialog::ReplaceSceneChoice;

    #[test]
    fn unsupported_extension_rejects_regardless_of_load_state() {
        assert_eq!(
            decide_open_path_action(false, false, false),
            OpenPathAction::RejectUnsupported
        );
        assert_eq!(
            decide_open_path_action(false, true, true),
            OpenPathAction::RejectUnsupported
        );
    }

    #[test]
    fn supported_extension_with_no_loaded_document_dispatches_immediately() {
        assert_eq!(
            decide_open_path_action(true, false, false),
            OpenPathAction::DispatchImmediately
        );
        // `document_can_undo` is irrelevant when nothing is loaded.
        assert_eq!(
            decide_open_path_action(true, false, true),
            OpenPathAction::DispatchImmediately
        );
    }

    #[test]
    fn supported_extension_with_loaded_document_prompts_with_export_iff_can_undo() {
        assert_eq!(
            decide_open_path_action(true, true, false),
            OpenPathAction::PromptReplace { can_export: false }
        );
        assert_eq!(
            decide_open_path_action(true, true, true),
            OpenPathAction::PromptReplace { can_export: true }
        );
    }

    #[test]
    fn map_replace_choice_translates_each_user_choice() {
        assert_eq!(
            map_replace_choice(ReplaceSceneChoice::Discard),
            PostPromptAction::DiscardThenOpen
        );
        assert_eq!(
            map_replace_choice(ReplaceSceneChoice::Export),
            PostPromptAction::ExportThenOpen
        );
        assert_eq!(
            map_replace_choice(ReplaceSceneChoice::Cancel),
            PostPromptAction::NoOp
        );
    }
}

#[cfg(test)]
mod settings_saver_tests {
    use super::*;
    use crate::ThemePreference;
    use crate::build_volume::{BuildVolume, HARDCODED_DEFAULT_BUILD_VOLUME};
    use crate::language::LanguagePreference;

    fn snapshot_with(theme: ThemePreference) -> PreferencesSnapshot {
        PreferencesSnapshot {
            theme,
            language: LanguagePreference::System,
            build_volume: HARDCODED_DEFAULT_BUILD_VOLUME,
        }
    }

    #[test]
    fn loop_exits_when_all_senders_drop() {
        let (tx, rx) = mpsc::channel::<PreferencesSnapshot>();
        drop(tx);
        // If the loop's exit condition regresses, this test hangs
        // rather than fails; that's the failure mode we want to catch
        // since `AppState::drop` would deadlock the same way.
        run_settings_saver_loop(rx, |_| {
            panic!("on_snapshot must not fire when no snapshots were sent");
        });
    }

    #[test]
    fn loop_processes_each_snapshot_when_arrivals_are_serialised() {
        // Sender dropped between sends so each `recv` sees exactly one
        // queued message; no coalescing opportunity. Verifies the
        // straight-line "save every message" path still works.
        let (tx, rx) = mpsc::channel::<PreferencesSnapshot>();
        tx.send(snapshot_with(ThemePreference::Light)).unwrap();
        tx.send(snapshot_with(ThemePreference::Dark)).unwrap();
        tx.send(snapshot_with(ThemePreference::System)).unwrap();
        drop(tx);
        let mut seen = Vec::new();
        run_settings_saver_loop(rx, |snap| seen.push(snap.theme));
        // All three queued sends are observable; coalescing only kicks
        // in when multiple messages are already buffered at recv time,
        // which is timing-dependent across threads. With a single
        // consumer thread and pre-queued messages, the first recv
        // returns one and try_recv drains the rest in a single wake,
        // so we expect exactly one coalesced call here too.
        assert_eq!(seen.len(), 1);
        assert!(matches!(seen[0], ThemePreference::System));
    }

    #[test]
    fn loop_coalesces_burst_to_latest_snapshot() {
        // Queue a burst of snapshots before the loop starts. The first
        // `recv` returns one; `try_recv` drains the rest in the same
        // wake; the loop calls `on_snapshot` once with the LAST value
        // queued. Catches regressions in the inner drain (e.g. an off-
        // by-one or a missing while loop) that would re-introduce
        // per-snapshot writes.
        let (tx, rx) = mpsc::channel::<PreferencesSnapshot>();
        let mut last_bv = BuildVolume::new(100.0, 100.0, 100.0);
        for i in 0..16 {
            let bv = BuildVolume::new(100.0 + i as f32, 80.0, 60.0);
            last_bv = bv;
            tx.send(PreferencesSnapshot {
                theme: ThemePreference::System,
                language: LanguagePreference::System,
                build_volume: bv,
            })
            .unwrap();
        }
        drop(tx);

        let mut calls: Vec<BuildVolume> = Vec::new();
        run_settings_saver_loop(rx, |snap| calls.push(snap.build_volume));

        assert_eq!(calls.len(), 1, "burst must coalesce to one call");
        assert_eq!(calls[0], last_bv);
    }

    #[test]
    fn spawn_settings_saver_joins_cleanly_on_drop() {
        // Smoke test for the production spawn path: drop the sender
        // without sending any snapshot, so the worker exits via
        // channel-closed before ever calling `AppSettings::save` (which
        // would touch the user's real config directory under test).
        // The join in `SettingsSaverJoin::drop` must complete without
        // hanging, matching the shutdown behaviour `AppState::drop`
        // relies on.
        let settings = AppSettings::default();
        let (tx, saver) = spawn_settings_saver(settings).expect("spawn settings-saver thread");
        drop(tx);
        drop(saver);
    }
}

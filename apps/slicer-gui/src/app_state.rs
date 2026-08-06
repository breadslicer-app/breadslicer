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
    LoadDispatch, SUPPORTED_MESH_EXTENSIONS, decide_load_dispatch, is_supported_mesh_extension,
    start_load_with_ticket,
};
use crate::pending_load::PendingLoad;
use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::preferences_window::{WiredPreferencesWindow, build_preferences_window};
use crate::profile_catalog::ProfileCatalog;
use crate::scene_document::SceneDocument;
use crate::scene_export::{default_export_extension, default_export_stem};
use crate::selection::{LeafId, Selection};
use crate::slice_panel::SlicePanelBridge;
use crate::slice_run::SliceRun;
use crate::viewport::ViewportController;
use crate::window_prefs::WindowPreferences;

mod replace_scene_policy {
    use crate::error_dialog::{KeepChangesOffer, ReplaceSceneChoice};

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
        /// scene confirmation dialog. `offer` controls which "keep
        /// your work" buttons (Save / Export) the dialog shows.
        PromptReplace { offer: KeepChangesOffer },
    }

    /// Post-prompt branch chosen from the user's reply to the
    /// replace-scene confirmation dialog.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum PostPromptAction {
        DiscardThenOpen,
        SaveThenOpen,
        ExportThenOpen,
        NoOp,
    }

    pub(crate) fn decide_open_path_action(
        supported_extension: bool,
        document_loaded: bool,
        document_is_dirty: bool,
        document_is_project: bool,
    ) -> OpenPathAction {
        if !supported_extension {
            return OpenPathAction::RejectUnsupported;
        }
        if !document_loaded {
            return OpenPathAction::DispatchImmediately;
        }
        let offer = if !document_is_dirty {
            KeepChangesOffer::NothingToKeep
        } else if document_is_project {
            KeepChangesOffer::SaveExisting
        } else {
            KeepChangesOffer::SaveAsNew
        };
        OpenPathAction::PromptReplace { offer }
    }

    pub(crate) fn map_replace_choice(choice: ReplaceSceneChoice) -> PostPromptAction {
        match choice {
            ReplaceSceneChoice::Discard => PostPromptAction::DiscardThenOpen,
            ReplaceSceneChoice::Save => PostPromptAction::SaveThenOpen,
            ReplaceSceneChoice::Export => PostPromptAction::ExportThenOpen,
            ReplaceSceneChoice::Cancel => PostPromptAction::NoOp,
        }
    }
}

use replace_scene_policy::{
    OpenPathAction, PostPromptAction, decide_open_path_action, map_replace_choice,
};

mod context_menu_policy {
    /// Visibility flags for the viewport's right-click context menu.
    /// Each flag controls whether the corresponding `MenuItem` is
    /// rendered in `mesh-viewer.slint`; the menu popup itself is
    /// suppressed when none of the flags are set so a right-click on
    /// empty space with an empty clipboard does not pop an empty
    /// menu.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct ContextMenuFlags {
        pub(crate) can_copy: bool,
        pub(crate) can_paste: bool,
        pub(crate) can_delete: bool,
    }

    impl ContextMenuFlags {
        /// True when at least one item would be visible.
        pub(crate) fn any(self) -> bool {
            self.can_copy || self.can_paste || self.can_delete
        }
    }

    /// Map the right-click state to the menu's visibility flags:
    /// - `picked_hit` (whether `pick_node_at` returned a node) drives
    ///   both Copy and Delete: both require an object under the cursor;
    /// - `clipboard_has_content` drives Paste alone.
    pub(crate) fn context_menu_flags(
        picked_hit: bool,
        clipboard_has_content: bool,
    ) -> ContextMenuFlags {
        ContextMenuFlags {
            can_copy: picked_hit,
            can_paste: clipboard_has_content,
            can_delete: picked_hit,
        }
    }
}

use context_menu_policy::context_menu_flags;

/// A main window paired with its viewport controller and preference-
/// bus subscription. Drop of this struct drops the subscription,
/// unregistering the window's preference-apply listener. Held by value
/// in `AppState::main_windows` so closing a window via
/// `on_close_requested` cleans up all three halves in one step. The
/// `window` field must be held even though it is never read: keeping
/// the Slint handle by value keeps the component alive, while the
/// only accesses to the live window happen through the
/// `Weak<MainWindow>` captured in the subscription's listener. The `controller` is held so external entry
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
    /// The Slice control panel's picker bridge. Held so its document
    /// and preferences-bus subscriptions live as long as the window.
    _slice_panel: Rc<SlicePanelBridge>,
    /// The Slice action driver. Held so its Slint callbacks and progress
    /// timer live as long as the window.
    _slice_run: Rc<SliceRun>,
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
/// window->closure->AppState->window graph doesn't form a strong-
/// reference cycle.
pub(crate) struct AppState {
    about_window: RefCell<Option<WiredAboutWindow>>,
    preferences_window: RefCell<Option<WiredPreferencesWindow>>,
    main_windows: RefCell<HashMap<u64, WiredMainWindow>>,
    next_window_id: Cell<u64>,
    prefs: Rc<PreferencesBus>,
    /// The merged printer / material catalog the Preferences pickers
    /// read from. Behind a refreshable handle so a later authoring or
    /// import step can rebuild it without a restart.
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
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

/// Copies every persisted preference out of a live snapshot into the
/// on-disk settings model. The single place the two representations are
/// mapped, so the saver thread and its tests stay in agreement.
fn apply_snapshot_to_settings(settings: &mut AppSettings, snap: &PreferencesSnapshot) {
    settings.set_theme(snap.theme);
    settings.set_menu_layout(snap.menu_layout);
    settings.set_language(snap.language);
    settings.set_default_printer(snap.default_printer.clone());
    settings.set_default_resin(snap.default_resin.clone());
    settings.set_favourite_printers(snap.favourite_printers.clone());
    settings.set_favourite_resins(snap.favourite_resins.clone());
    settings.set_aa_level(snap.aa_level);
    settings.set_object_list_visible(snap.object_list_visible);
    settings.set_object_list_anchor_right(snap.object_list_anchor_right);
    settings.set_object_list_anchor_bottom(snap.object_list_anchor_bottom);
    settings.set_object_list_offset_x(snap.object_list_offset_x);
    settings.set_object_list_offset_y(snap.object_list_offset_y);
    settings.set_object_list_width(snap.object_list_width);
    settings.set_object_list_height(snap.object_list_height);
    settings.set_object_list_minimized(snap.object_list_minimized);
    settings.set_slice_panel_anchor_right(snap.slice_panel_anchor_right);
    settings.set_slice_panel_anchor_bottom(snap.slice_panel_anchor_bottom);
    settings.set_slice_panel_offset_x(snap.slice_panel_offset_x);
    settings.set_slice_panel_offset_y(snap.slice_panel_offset_y);
    settings.set_slice_panel_width(snap.slice_panel_width);
    settings.set_slice_panel_height(snap.slice_panel_height);
    settings.set_slice_view_visible(snap.slice_view_visible);
    settings.set_slice_view_anchor_right(snap.slice_view_anchor_right);
    settings.set_slice_view_anchor_bottom(snap.slice_view_anchor_bottom);
    settings.set_slice_view_offset_x(snap.slice_view_offset_x);
    settings.set_slice_view_offset_y(snap.slice_view_offset_y);
    settings.set_slice_view_width(snap.slice_view_width);
    settings.set_slice_view_height(snap.slice_view_height);
    settings.set_slice_view_minimized(snap.slice_view_minimized);
    settings.set_last_open_dir(snap.last_open_dir.clone());
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
                apply_snapshot_to_settings(&mut settings, snap);
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
        let catalog = Rc::new(RefCell::new(Rc::new(ProfileCatalog::load_default())));
        let default_printer = settings.default_printer();
        let default_resin = settings.default_resin();
        // The envelope is derived from the effective printer: the user's
        // explicit choice when present, otherwise the first-run soft
        // default (Generic Machine, else the first catalog printer). The
        // soft default drives the opening envelope only; `default_printer`
        // stays None so nothing is written to settings until the user
        // picks a printer.
        let build_volume = {
            let catalog = catalog.borrow();
            let effective_printer = catalog.effective_default_printer(default_printer.as_ref());
            catalog.envelope_for_printer(effective_printer.as_ref())
        };
        let snapshot = PreferencesSnapshot {
            theme: settings.theme(),
            menu_layout: settings.menu_layout(),
            language: settings.language(),
            default_printer,
            default_resin,
            favourite_printers: settings.favourite_printers(),
            favourite_resins: settings.favourite_resins(),
            aa_level: settings.aa_level(),
            build_volume,
            object_list_visible: settings.object_list_visible(),
            object_list_anchor_right: settings.object_list_anchor_right(),
            object_list_anchor_bottom: settings.object_list_anchor_bottom(),
            object_list_offset_x: settings.object_list_offset_x(),
            object_list_offset_y: settings.object_list_offset_y(),
            object_list_width: settings.object_list_width(),
            object_list_height: settings.object_list_height(),
            object_list_minimized: settings.object_list_minimized(),
            slice_panel_anchor_right: settings.slice_panel_anchor_right(),
            slice_panel_anchor_bottom: settings.slice_panel_anchor_bottom(),
            slice_panel_offset_x: settings.slice_panel_offset_x(),
            slice_panel_offset_y: settings.slice_panel_offset_y(),
            slice_panel_width: settings.slice_panel_width(),
            slice_panel_height: settings.slice_panel_height(),
            slice_view_visible: settings.slice_view_visible(),
            slice_view_anchor_right: settings.slice_view_anchor_right(),
            slice_view_anchor_bottom: settings.slice_view_anchor_bottom(),
            slice_view_offset_x: settings.slice_view_offset_x(),
            slice_view_offset_y: settings.slice_view_offset_y(),
            slice_view_width: settings.slice_view_width(),
            slice_view_height: settings.slice_view_height(),
            slice_view_minimized: settings.slice_view_minimized(),
            last_open_dir: settings.last_open_dir(),
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
            catalog,
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
        // Static platform flag consumed by the View menu to show the
        // correct fullscreen accelerator (Cmd+Ctrl+F on macOS, F11
        // elsewhere). Slint has no compile-time platform branch, so the
        // choice is made here and pushed into the UI.
        ui.set_is_macos(cfg!(target_os = "macos"));
        let snapshot = self.prefs.snapshot();
        ui.apply_preferences(&snapshot);
        // Object-list panel visibility and geometry are per-window live
        // state: restored once here at creation and deliberately left
        // out of `apply_preferences`, so a later change in one window is
        // saved to disk without moving or hiding the panel in other
        // windows. An offset or size field is pushed only when a previous
        // session saved it; leaving a field untouched keeps the Slint
        // default (top-right anchor, 12px offsets, 220px width, auto
        // height).
        ui.set_show_object_list(snapshot.object_list_visible);
        ui.set_object_list_minimized(snapshot.object_list_minimized);
        ui.set_object_list_anchor_right(snapshot.object_list_anchor_right);
        ui.set_object_list_anchor_bottom(snapshot.object_list_anchor_bottom);
        if let Some(offset_x) = snapshot.object_list_offset_x {
            ui.set_object_list_offset_x(offset_x);
        }
        if let Some(offset_y) = snapshot.object_list_offset_y {
            ui.set_object_list_offset_y(offset_y);
        }
        if let Some(width) = snapshot.object_list_width {
            ui.set_object_list_width(width);
        }
        if let Some(height) = snapshot.object_list_height {
            ui.set_object_list_height(height);
        }
        // Slice control panel geometry: same per-window live semantics as
        // the object-list panel. An offset or size field is pushed only
        // when a previous session saved it; leaving a field untouched
        // keeps the Slint default (top-left anchor, 12px offsets, 240px
        // width, auto height).
        ui.set_slice_panel_anchor_right(snapshot.slice_panel_anchor_right);
        ui.set_slice_panel_anchor_bottom(snapshot.slice_panel_anchor_bottom);
        if let Some(offset_x) = snapshot.slice_panel_offset_x {
            ui.set_slice_panel_offset_x(offset_x);
        }
        if let Some(offset_y) = snapshot.slice_panel_offset_y {
            ui.set_slice_panel_offset_y(offset_y);
        }
        if let Some(width) = snapshot.slice_panel_width {
            ui.set_slice_panel_width(width);
        }
        if let Some(height) = snapshot.slice_panel_height {
            ui.set_slice_panel_height(height);
        }
        // Slice-view panel visibility and geometry: same per-window live
        // semantics as the object-list panel. An offset or size field is
        // pushed only when a previous session saved it; leaving a field
        // untouched keeps the Slint default (bottom-left anchor, 12px
        // offsets, 240px width, auto height).
        ui.set_show_slice_view(snapshot.slice_view_visible);
        ui.set_slice_view_minimized(snapshot.slice_view_minimized);
        ui.set_slice_view_anchor_right(snapshot.slice_view_anchor_right);
        ui.set_slice_view_anchor_bottom(snapshot.slice_view_anchor_bottom);
        if let Some(offset_x) = snapshot.slice_view_offset_x {
            ui.set_slice_view_offset_x(offset_x);
        }
        if let Some(offset_y) = snapshot.slice_view_offset_y {
            ui.set_slice_view_offset_y(offset_y);
        }
        if let Some(width) = snapshot.slice_view_width {
            ui.set_slice_view_width(width);
        }
        if let Some(height) = snapshot.slice_view_height {
            ui.set_slice_view_height(height);
        }
        let document = SceneDocument::new();
        let pending_load = Rc::new(PendingLoad::new());
        let ctrl = Rc::new(ViewportController::new(&ui, document, pending_load.clone()));
        ctrl.register_document_listener();
        // Bus listeners only fire on `update`, not on subscribe, so
        // the persisted build volume is pushed imperatively here.
        // Future runtime edits flow in through the subscription below.
        ctrl.set_build_volume(snapshot.build_volume);

        crate::wiring::wire_main_window(&ui, ctrl.clone(), self.prefs.clone())?;
        let slice_panel = SlicePanelBridge::install(
            &ui,
            self.catalog.clone(),
            self.prefs.clone(),
            ctrl.document(),
        );
        let slice_run = SliceRun::install(&ui, self.prefs.clone(), ctrl.document(), ctrl.dialogs());
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
            if matches!(change, PreferencesChange::DefaultPrinter) {
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
        // The winit window only exists once shown; set the taskbar icon
        // here so the live window matches the executable resource.
        crate::window_icon::apply(ui.window());
        self.main_windows.borrow_mut().insert(
            id,
            WiredMainWindow {
                window: ui,
                controller: ctrl,
                pending_load,
                _subscription: subscription,
                _slice_panel: slice_panel,
                _slice_run: slice_run,
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
    ///    load the new path; on Save, run the Save flow (overwrite
    ///    the project file, or Save As when there is none) and only
    ///    once the document is on disk close + load; on Export, run
    ///    the existing "Export As…" flow and only on a successful
    ///    save close + load; on Cancel, do nothing.
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
            controller.document().is_dirty(),
            controller.document().is_project(),
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
            OpenPathAction::PromptReplace { offer } => {
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
                    offer,
                    move |choice| {
                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };
                        match map_replace_choice(choice) {
                            PostPromptAction::DiscardThenOpen => {
                                app.discard_then_open(window_id, path);
                            }
                            PostPromptAction::SaveThenOpen => {
                                app.save_then_open(window_id, path);
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
        // Copy is a Transform-section tool (it seeds a paste, which
        // mutates the scene); it stays inert in Prepare and Slice.
        if !controller.allows_layout_tools() {
            return;
        }
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
        // The context menu only offers Copy / Paste / Delete, all
        // Transform-section tools; outside Transform there is nothing to
        // show, so a right-click there opens no menu (camera orbit on
        // the right button still works, it runs through a separate
        // gesture path).
        if !wired.controller.allows_layout_tools() {
            return;
        }
        let hit = wired.controller.pick_node_at(x, y);
        if let Some(node_id) = hit {
            wired
                .controller
                .document()
                .set_selection(Selection::One(LeafId(node_id)));
        }
        let flags = context_menu_flags(hit.is_some(), self.clipboard.borrow().is_some());
        if !flags.any() {
            return;
        }
        wired.window.invoke_show_context_menu(
            x,
            y,
            flags.can_copy,
            flags.can_paste,
            flags.can_delete,
        );
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
        if decide_load_dispatch(loader.graphics_unavailable()) == LoadDispatch::Queue {
            // Pre-first-paint: the controller will dispatch this once
            // `handle_rendering_setup` publishes the device.
            pending.queue(path);
            return;
        }
        let Some(ticket) = loader.begin_load() else {
            // Decision #2: drop onto a busy window is silently ignored.
            return;
        };
        // Covers both a ready device and a renderer that will never
        // provide one; `start_load_with_ticket` reads the same graphics
        // state and deposits the reason for the latter, so a path that
        // can never open reports itself instead of waiting forever on a
        // setup that is not coming.
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

    /// Replace-confirm Save branch: run the Save flow (overwrite the
    /// document's project file, or fall back to the Save As flow when
    /// there is none) and, only once the document is safely on disk,
    /// close the current scene and dispatch the new path. Cancellation
    /// of the Save As dialog or a write failure leaves the current
    /// scene intact (the failure dialog is surfaced by `run_save_flow`
    /// itself).
    fn save_then_open(self: &Rc<Self>, window_id: u64, path: PathBuf) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };
        let doc = controller.document();
        let dialogs = controller.dialogs();
        let save_progress = controller.save_progress();
        let snapshot = self.prefs.snapshot();
        let app_weak = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            let saved = crate::wiring::run_save_flow(
                doc,
                dialogs,
                save_progress,
                snapshot.default_printer,
                snapshot.default_resin,
            )
            .await;
            if !saved {
                return;
            }
            if let Some(app) = app_weak.upgrade() {
                app.discard_then_open(window_id, path);
            }
        }) {
            eprintln!("Failed to spawn Save & Open task: {err}");
        }
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
        let export_progress = controller.export_progress();
        let stem = default_export_stem(&doc);
        let ext = default_export_extension(&doc);
        let app_weak = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            let saved =
                crate::wiring::run_export_as_flow(doc, dialogs, export_progress, stem, ext).await;
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
        // Closures hold `Weak<AppState>` so the MainWindow -> closure
        // -> AppState -> main_windows(MainWindow) chain isn't a strong
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
        {
            // Persist and broadcast a menu-layout switch through the bus,
            // like the preferences window does for the theme. Every
            // window re-applies it and the settings saver writes it out.
            let bus = self.prefs.clone();
            ui.on_menu_layout_change_requested(move |layout| {
                bus.update(move |snap| {
                    snap.menu_layout = layout;
                    PreferencesChange::MenuLayout
                });
            });
        }
        {
            // Persist the object-list panel's visibility. Unlike the
            // menu layout this is per-window live state, so the bus write
            // reaches the settings saver but no window re-applies it (see
            // the restore in `spawn_main_window` and `window_prefs`).
            let bus = self.prefs.clone();
            ui.on_object_list_visibility_changed(move |visible| {
                bus.update(move |snap| {
                    snap.object_list_visible = visible;
                    PreferencesChange::ObjectListPanel
                });
            });
        }
        {
            // Persist the panel geometry after a drag, resize, or
            // minimize. Same per-window live handling as the visibility.
            let bus = self.prefs.clone();
            ui.on_object_list_geometry_changed(
                move |anchor_right, anchor_bottom, offset_x, offset_y, width, height, minimized| {
                    // A 0px height means the panel is auto-sizing; store
                    // it as "unset" so it keeps auto-sizing after a
                    // restart.
                    let height = if height > 0.0 { Some(height) } else { None };
                    let current = bus.snapshot();
                    if current.object_list_anchor_right == anchor_right
                        && current.object_list_anchor_bottom == anchor_bottom
                        && current.object_list_offset_x == Some(offset_x)
                        && current.object_list_offset_y == Some(offset_y)
                        && current.object_list_width == Some(width)
                        && current.object_list_height == height
                        && current.object_list_minimized == minimized
                    {
                        // A gesture that ended without changing anything
                        // (for example a plain click on the title bar)
                        // must not trigger a redundant settings write.
                        return;
                    }
                    bus.update(move |snap| {
                        snap.object_list_anchor_right = anchor_right;
                        snap.object_list_anchor_bottom = anchor_bottom;
                        snap.object_list_offset_x = Some(offset_x);
                        snap.object_list_offset_y = Some(offset_y);
                        snap.object_list_width = Some(width);
                        snap.object_list_height = height;
                        snap.object_list_minimized = minimized;
                        PreferencesChange::ObjectListPanel
                    });
                },
            );
        }
        {
            // Persist the Slice control panel geometry after a drag or
            // resize. Per-window live state, like the object-list panel.
            let bus = self.prefs.clone();
            ui.on_slice_panel_geometry_changed(
                move |anchor_right, anchor_bottom, offset_x, offset_y, width, height| {
                    // A 0px height means the panel is auto-sizing; store
                    // it as "unset" so it keeps auto-sizing after a
                    // restart.
                    let height = if height > 0.0 { Some(height) } else { None };
                    let current = bus.snapshot();
                    if current.slice_panel_anchor_right == anchor_right
                        && current.slice_panel_anchor_bottom == anchor_bottom
                        && current.slice_panel_offset_x == Some(offset_x)
                        && current.slice_panel_offset_y == Some(offset_y)
                        && current.slice_panel_width == Some(width)
                        && current.slice_panel_height == height
                    {
                        // A gesture that ended without changing anything
                        // must not trigger a redundant settings write.
                        return;
                    }
                    bus.update(move |snap| {
                        snap.slice_panel_anchor_right = anchor_right;
                        snap.slice_panel_anchor_bottom = anchor_bottom;
                        snap.slice_panel_offset_x = Some(offset_x);
                        snap.slice_panel_offset_y = Some(offset_y);
                        snap.slice_panel_width = Some(width);
                        snap.slice_panel_height = height;
                        PreferencesChange::SliceControlPanel
                    });
                },
            );
        }
        {
            // Persist the slice-view panel's visibility. Per-window live
            // state, like the object-list panel: the bus write reaches
            // the settings saver but no window re-applies it.
            let bus = self.prefs.clone();
            ui.on_slice_view_visibility_changed(move |visible| {
                bus.update(move |snap| {
                    snap.slice_view_visible = visible;
                    PreferencesChange::SliceViewPanel
                });
            });
        }
        {
            // Persist the slice-view panel geometry after a drag, resize,
            // or minimize. Same per-window live handling as the object
            // list.
            let bus = self.prefs.clone();
            ui.on_slice_view_geometry_changed(
                move |anchor_right, anchor_bottom, offset_x, offset_y, width, height, minimized| {
                    // A 0px height means the panel is auto-sizing; store
                    // it as "unset" so it keeps auto-sizing after a
                    // restart.
                    let height = if height > 0.0 { Some(height) } else { None };
                    let current = bus.snapshot();
                    if current.slice_view_anchor_right == anchor_right
                        && current.slice_view_anchor_bottom == anchor_bottom
                        && current.slice_view_offset_x == Some(offset_x)
                        && current.slice_view_offset_y == Some(offset_y)
                        && current.slice_view_width == Some(width)
                        && current.slice_view_height == height
                        && current.slice_view_minimized == minimized
                    {
                        // A gesture that ended without changing anything
                        // must not trigger a redundant settings write.
                        return;
                    }
                    bus.update(move |snap| {
                        snap.slice_view_anchor_right = anchor_right;
                        snap.slice_view_anchor_bottom = anchor_bottom;
                        snap.slice_view_offset_x = Some(offset_x);
                        snap.slice_view_offset_y = Some(offset_y);
                        snap.slice_view_width = Some(width);
                        snap.slice_view_height = height;
                        snap.slice_view_minimized = minimized;
                        PreferencesChange::SliceViewPanel
                    });
                },
            );
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
        match build_preferences_window(&self.prefs, self.catalog.clone()) {
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
    use crate::error_dialog::{KeepChangesOffer, ReplaceSceneChoice};

    #[test]
    fn unsupported_extension_rejects_regardless_of_load_state() {
        assert_eq!(
            decide_open_path_action(false, false, false, false),
            OpenPathAction::RejectUnsupported
        );
        assert_eq!(
            decide_open_path_action(false, true, true, true),
            OpenPathAction::RejectUnsupported
        );
    }

    #[test]
    fn supported_extension_with_no_loaded_document_dispatches_immediately() {
        assert_eq!(
            decide_open_path_action(true, false, false, false),
            OpenPathAction::DispatchImmediately
        );
        // `document_is_dirty` / `document_is_project` are irrelevant
        // when nothing is loaded.
        assert_eq!(
            decide_open_path_action(true, false, true, true),
            OpenPathAction::DispatchImmediately
        );
    }

    #[test]
    fn clean_document_prompts_with_nothing_to_keep() {
        // Whether a project file exists doesn't matter when there
        // are no unsaved changes: nothing would be lost.
        assert_eq!(
            decide_open_path_action(true, true, false, false),
            OpenPathAction::PromptReplace {
                offer: KeepChangesOffer::NothingToKeep
            }
        );
        assert_eq!(
            decide_open_path_action(true, true, false, true),
            OpenPathAction::PromptReplace {
                offer: KeepChangesOffer::NothingToKeep
            }
        );
    }

    #[test]
    fn dirty_project_prompts_with_save_existing() {
        assert_eq!(
            decide_open_path_action(true, true, true, true),
            OpenPathAction::PromptReplace {
                offer: KeepChangesOffer::SaveExisting
            }
        );
    }

    #[test]
    fn dirty_non_project_prompts_with_save_as_new() {
        assert_eq!(
            decide_open_path_action(true, true, true, false),
            OpenPathAction::PromptReplace {
                offer: KeepChangesOffer::SaveAsNew
            }
        );
    }

    #[test]
    fn map_replace_choice_translates_each_user_choice() {
        assert_eq!(
            map_replace_choice(ReplaceSceneChoice::Discard),
            PostPromptAction::DiscardThenOpen
        );
        assert_eq!(
            map_replace_choice(ReplaceSceneChoice::Save),
            PostPromptAction::SaveThenOpen
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
mod context_menu_policy_tests {
    use super::context_menu_policy::*;

    #[test]
    fn pick_hit_with_empty_clipboard_enables_copy_and_delete_only() {
        let f = context_menu_flags(true, false);
        assert_eq!(
            f,
            ContextMenuFlags {
                can_copy: true,
                can_paste: false,
                can_delete: true,
            }
        );
        assert!(f.any());
    }

    #[test]
    fn no_hit_with_filled_clipboard_enables_paste_only() {
        let f = context_menu_flags(false, true);
        assert_eq!(
            f,
            ContextMenuFlags {
                can_copy: false,
                can_paste: true,
                can_delete: false,
            }
        );
        assert!(f.any());
    }

    #[test]
    fn no_hit_with_empty_clipboard_disables_everything() {
        let f = context_menu_flags(false, false);
        assert_eq!(
            f,
            ContextMenuFlags {
                can_copy: false,
                can_paste: false,
                can_delete: false,
            }
        );
        assert!(!f.any());
    }

    #[test]
    fn pick_hit_with_filled_clipboard_enables_all_three() {
        let f = context_menu_flags(true, true);
        assert!(f.can_copy);
        assert!(f.can_paste);
        assert!(f.can_delete);
        assert!(f.any());
    }
}

#[cfg(test)]
mod settings_saver_tests {
    use std::collections::BTreeSet;

    use sc_print_primitives::{MaterialId, PrinterId};

    use super::*;
    use crate::build_volume::{BuildVolume, HARDCODED_DEFAULT_BUILD_VOLUME};
    use crate::language::LanguagePreference;
    use crate::{MenuLayout, ThemePreference};
    use sc_print_primitives::AaLevel;

    fn snapshot_with(theme: ThemePreference) -> PreferencesSnapshot {
        PreferencesSnapshot {
            theme,
            menu_layout: MenuLayout::Bar,
            language: LanguagePreference::System,
            default_printer: None,
            default_resin: None,
            favourite_printers: BTreeSet::new(),
            favourite_resins: BTreeSet::new(),
            aa_level: AaLevel::Off,
            build_volume: HARDCODED_DEFAULT_BUILD_VOLUME,
            object_list_visible: true,
            object_list_anchor_right: true,
            object_list_anchor_bottom: false,
            object_list_offset_x: None,
            object_list_offset_y: None,
            object_list_width: None,
            object_list_height: None,
            object_list_minimized: false,
            slice_panel_anchor_right: false,
            slice_panel_anchor_bottom: false,
            slice_panel_offset_x: None,
            slice_panel_offset_y: None,
            slice_panel_width: None,
            slice_panel_height: None,
            slice_view_visible: true,
            slice_view_anchor_right: false,
            slice_view_anchor_bottom: true,
            slice_view_offset_x: None,
            slice_view_offset_y: None,
            slice_view_width: None,
            slice_view_height: None,
            slice_view_minimized: false,
            last_open_dir: None,
        }
    }

    #[test]
    fn apply_snapshot_copies_favourites_into_settings() {
        let mut snap = snapshot_with(ThemePreference::System);
        let printer = PrinterId::new("anycubic-photon-m3-pm3").unwrap();
        let resin = MaterialId::new("abs-like-405").unwrap();
        snap.favourite_printers.insert(printer.clone());
        snap.favourite_resins.insert(resin.clone());

        let mut settings = AppSettings::default();
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(settings.favourite_printers().contains(&printer));
        assert!(settings.favourite_resins().contains(&resin));
    }

    #[test]
    fn apply_snapshot_copies_aa_level_into_settings() {
        let mut snap = snapshot_with(ThemePreference::System);
        snap.aa_level = AaLevel::X8;

        let mut settings = AppSettings::default();
        apply_snapshot_to_settings(&mut settings, &snap);

        assert_eq!(settings.aa_level(), AaLevel::X8);
    }

    #[test]
    fn apply_snapshot_copies_slice_panel_geometry_into_settings() {
        let mut snap = snapshot_with(ThemePreference::System);
        snap.slice_panel_anchor_right = true;
        snap.slice_panel_anchor_bottom = true;
        snap.slice_panel_offset_x = Some(24.0);
        snap.slice_panel_offset_y = Some(36.0);
        snap.slice_panel_width = Some(300.0);
        snap.slice_panel_height = Some(220.0);

        let mut settings = AppSettings::default();
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(settings.slice_panel_anchor_right());
        assert!(settings.slice_panel_anchor_bottom());
        assert_eq!(settings.slice_panel_offset_x(), Some(24.0));
        assert_eq!(settings.slice_panel_offset_y(), Some(36.0));
        assert_eq!(settings.slice_panel_width(), Some(300.0));
        assert_eq!(settings.slice_panel_height(), Some(220.0));
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
                menu_layout: MenuLayout::Bar,
                language: LanguagePreference::System,
                default_printer: None,
                default_resin: None,
                favourite_printers: BTreeSet::new(),
                favourite_resins: BTreeSet::new(),
                aa_level: AaLevel::Off,
                build_volume: bv,
                object_list_visible: true,
                object_list_anchor_right: true,
                object_list_anchor_bottom: false,
                object_list_offset_x: None,
                object_list_offset_y: None,
                object_list_width: None,
                object_list_height: None,
                object_list_minimized: false,
                slice_panel_anchor_right: false,
                slice_panel_anchor_bottom: false,
                slice_panel_offset_x: None,
                slice_panel_offset_y: None,
                slice_panel_width: None,
                slice_panel_height: None,
                slice_view_visible: true,
                slice_view_anchor_right: false,
                slice_view_anchor_bottom: true,
                slice_view_offset_x: None,
                slice_view_offset_y: None,
                slice_view_width: None,
                slice_view_height: None,
                slice_view_minimized: false,
                last_open_dir: None,
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
    fn saver_mapping_carries_object_list_visibility() {
        // The snapshot-to-settings mapping must copy the panel
        // visibility so a hide survives a save.
        let mut settings = AppSettings::default();
        assert!(settings.object_list_visible());

        let mut snap = snapshot_with(ThemePreference::System);
        snap.object_list_visible = false;
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(!settings.object_list_visible());
    }

    #[test]
    fn saver_mapping_carries_object_list_geometry() {
        // The mapping must copy the panel anchor, offsets, size, and
        // minimized state so a drag or resize survives a save.
        let mut settings = AppSettings::default();

        let mut snap = snapshot_with(ThemePreference::System);
        snap.object_list_anchor_right = false;
        snap.object_list_anchor_bottom = true;
        snap.object_list_offset_x = Some(40.0);
        snap.object_list_offset_y = Some(52.0);
        snap.object_list_width = Some(260.0);
        snap.object_list_height = Some(180.0);
        snap.object_list_minimized = true;
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(!settings.object_list_anchor_right());
        assert!(settings.object_list_anchor_bottom());
        assert_eq!(settings.object_list_offset_x(), Some(40.0));
        assert_eq!(settings.object_list_offset_y(), Some(52.0));
        assert_eq!(settings.object_list_width(), Some(260.0));
        assert_eq!(settings.object_list_height(), Some(180.0));
        assert!(settings.object_list_minimized());
    }

    #[test]
    fn saver_mapping_carries_slice_view_visibility() {
        // The snapshot-to-settings mapping must copy the slice-view
        // panel visibility so a hide survives a save.
        let mut settings = AppSettings::default();
        assert!(settings.slice_view_visible());

        let mut snap = snapshot_with(ThemePreference::System);
        snap.slice_view_visible = false;
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(!settings.slice_view_visible());
    }

    #[test]
    fn saver_mapping_carries_slice_view_geometry() {
        // The mapping must copy the slice-view panel anchor, offsets,
        // size, and minimized state so a drag or resize survives a save.
        let mut settings = AppSettings::default();

        let mut snap = snapshot_with(ThemePreference::System);
        snap.slice_view_anchor_right = true;
        snap.slice_view_anchor_bottom = false;
        snap.slice_view_offset_x = Some(40.0);
        snap.slice_view_offset_y = Some(52.0);
        snap.slice_view_width = Some(260.0);
        snap.slice_view_height = Some(200.0);
        snap.slice_view_minimized = true;
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(settings.slice_view_anchor_right());
        assert!(!settings.slice_view_anchor_bottom());
        assert_eq!(settings.slice_view_offset_x(), Some(40.0));
        assert_eq!(settings.slice_view_offset_y(), Some(52.0));
        assert_eq!(settings.slice_view_width(), Some(260.0));
        assert_eq!(settings.slice_view_height(), Some(200.0));
        assert!(settings.slice_view_minimized());
    }

    #[test]
    fn saver_mapping_carries_last_open_dir() {
        // The mapping must copy the remembered open directory so it
        // survives a save.
        let mut settings = AppSettings::default();
        assert_eq!(settings.last_open_dir(), None);

        let mut snap = snapshot_with(ThemePreference::System);
        snap.last_open_dir = Some(PathBuf::from("/models"));
        apply_snapshot_to_settings(&mut settings, &snap);

        assert_eq!(settings.last_open_dir(), Some(PathBuf::from("/models")));
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

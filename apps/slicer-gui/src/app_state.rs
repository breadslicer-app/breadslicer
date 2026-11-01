/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use slint::winit_030::winit;
use slint::{CloseRequestResponse, ComponentHandle};

use crate::MainWindow;
use crate::about_window::{WiredAboutWindow, build_about_window};
use crate::app_settings::AppSettings;
use crate::app_settings::file::{LoadOutcome, SettingsFile};
use crate::arrange_run::ArrangeRun;
use crate::autosave::{Autosaver, recovery_line};
use crate::autosave_store::{AutosaveStore, OrphanRun};
use crate::clipboard::{Clipboard, fragment_from_document_selection};
use crate::default_envelope::DefaultEnvelope;
use crate::df_support_run::DfSupportRun;
use crate::error_dialog::{ErrorTexts, InfoTexts, RecoveryChoice};
use crate::graphics_status::GraphicsUnavailable;
use crate::hollow_run::HollowRun;
use crate::i18n::ctr;
use crate::language::InstalledTranslations;
use crate::mesh_loader::{
    LoadDispatch, SUPPORTED_MESH_EXTENSIONS, decide_load_dispatch, is_supported_mesh_extension,
    start_load_with_ticket,
};
use crate::mesh_render::DeviceLoss;
use crate::orient_run::OrientRun;
use crate::pending_load::PendingLoad;
use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::preferences_window::{
    WiredPreferencesWindow, build_preferences_window, wire_performance_test,
};
use crate::print_selection::PrintDefaults;
use crate::profile_catalog::{CatalogLoad, ProfileCatalog};
use crate::repair_run::RepairRun;
use crate::review_panel::ReviewPanel;
use crate::scene_document::SceneDocument;
use crate::scene_export::{default_export_extension, default_export_stem};
use crate::selection::{LeafId, Selection};
use crate::slice_panel::SlicePanelBridge;
use crate::slice_run::SliceRun;
use crate::split_bodies::SplitBodies;
use crate::support_gen_run::GenSupportRun;
use crate::ui_language::UiLanguage;
use crate::viewport::ViewportController;
use crate::window_prefs::{WindowPreferences, restore_panels};
use crate::wiring::SaveOutcome;

/// What to do only after the user's unsaved work has been handled.
enum SceneChange {
    Open(PathBuf),
    Clear,
    CloseWindow,
}

mod replace_scene_policy {
    use crate::error_dialog::{KeepChangesOffer, ReplaceSceneChoice};

    /// Pre-prompt branch chosen by `open_path_in_window` from the state of the
    /// target window and the dropped/opened path.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum OpenPathAction {
        /// Path extension is not in the supported mesh set; caller should
        /// surface an "Unsupported file type" error dialog.
        RejectUnsupported,
        /// No scene is currently loaded; caller should dispatch the path to the
        /// loader directly.
        DispatchImmediately,
        /// A scene is already loaded; caller should show the replace-scene
        /// confirmation dialog. `offer` controls which "keep your work" buttons
        /// (Save / Export) the dialog shows.
        PromptReplace { offer: KeepChangesOffer },
    }

    /// Post-prompt branch chosen from the user's reply to the replace-scene
    /// confirmation dialog.
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
        document_saves_in_place: bool,
    ) -> OpenPathAction {
        if !supported_extension {
            return OpenPathAction::RejectUnsupported;
        }
        if !document_loaded {
            return OpenPathAction::DispatchImmediately;
        }
        let offer = if !document_is_dirty {
            KeepChangesOffer::NothingToKeep
        } else if document_saves_in_place {
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
    /// Visibility flags for the viewport's right-click context menu. Each flag
    /// controls whether the corresponding `MenuItem` is rendered in
    /// `mesh-viewer.slint`; the menu popup itself is suppressed when none of
    /// the flags are set, so a right-click on empty space with an empty
    /// clipboard does not pop an empty menu.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct ContextMenuFlags {
        pub(crate) can_copy: bool,
        pub(crate) can_paste: bool,
        pub(crate) can_delete: bool,
    }

    impl ContextMenuFlags {
        /// Reports whether at least 1 item is visible.
        pub(crate) fn any(self) -> bool {
            self.can_copy || self.can_paste || self.can_delete
        }
    }

    /// Maps the right-click state to the menu's visibility flags:
    /// - `picked_hit` (whether `pick_node_at` returned a node) drives both Copy
    ///   and Delete: both require an object under the cursor;
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

/// A step of a file drag over a main window.
#[derive(Debug, PartialEq, Eq)]
enum FileDrag {
    /// A file entered the window.
    Hovered,
    /// The file left the window, or the drag ended without a drop.
    HoverCancelled,
    /// The file was dropped on the window.
    Dropped(PathBuf),
}

impl FileDrag {
    /// Returns the drag step that `event` reports, or `None` for any other
    /// window event.
    fn from_winit(event: &winit::event::WindowEvent) -> Option<Self> {
        match event {
            winit::event::WindowEvent::HoveredFile(_) => Some(Self::Hovered),
            winit::event::WindowEvent::HoveredFileCancelled => Some(Self::HoverCancelled),
            winit::event::WindowEvent::DroppedFile(path) => Some(Self::Dropped(path.clone())),
            _ => None,
        }
    }
}

thread_local! {
    /// The Slint thread's `AppState`, for delivering a device loss raised on
    /// another thread.
    static DEVICE_LOST_TARGET: RefCell<Weak<AppState>> = const { RefCell::new(Weak::new()) };
}

/// A main window with the objects that live as long as it does.
/// `AppState::main_windows` holds it by value, so closing the window through
/// `on_close_requested` drops all of them in 1 step; the drop of
/// `_subscription` unregisters the window's preferences listener. Holding
/// `window` by value keeps the Slint component alive. `AppState` reaches the
/// window's controller and its pending-load slot through the window id.
struct WiredMainWindow {
    window: MainWindow,
    controller: Rc<ViewportController>,
    // Shared with the controller. Writer side: external entry points park a
    // path here when the GPU device isn't yet available. The controller drains
    // it from `handle_rendering_setup`.
    pending_load: Rc<PendingLoad>,
    /// Autosaves the window's scene (#18); `None` without a data folder.
    autosaver: Option<Rc<Autosaver>>,
    _subscription: Subscription,
    _drivers: WindowDrivers,
}

/// The drivers of a main window's Slice control panel and of its Slice, Repair
/// and Generate Support actions.
struct WindowDrivers {
    /// The Slice control panel's picker bridge. Held so its document and
    /// preferences-bus subscriptions live as long as the window.
    _slice_panel: Rc<SlicePanelBridge>,
    /// The Slice action driver. Held so its Slint callbacks and progress timer
    /// live as long as the window.
    _slice_run: Rc<SliceRun>,
    /// The Slice tab's Review card. Held so its callbacks and document
    /// subscription live as long as the window.
    _review_panel: Rc<ReviewPanel>,
    /// The Repair action driver. Held so its Slint callbacks live as long as
    /// the window.
    _repair_run: Rc<RepairRun>,
    _arrange_run: Rc<ArrangeRun>,
    _orient_run: Rc<OrientRun>,
    /// The Tools > "Generate Support (trivial for test)" action driver. Held so
    /// its Slint callbacks live as long as the window.
    _support_gen_run: Rc<GenSupportRun>,
    _df_support_run: Rc<DfSupportRun>,
    _hollow_run: Rc<HollowRun>,
    /// Edit > Split into Bodies and Keep Only This Body. Held so its Slint
    /// callbacks and document subscription live as long as the window.
    _split_bodies: Rc<SplitBodies>,
}

/// Owns the app-wide state:
///
/// - the `AboutWindow` and the `PreferencesWindow`, each created on first use
///   and shared by every main window;
/// - the `PreferencesBus`, which holds the preferences and notifies every live
///   window of a change;
/// - the profile catalog, the default printer's envelope, the clipboard, the
///   device-loss flag and the settings saver;
/// - strong handles to each live `MainWindow`, which keep them alive while the
///   event loop runs.
///
/// Per-window state (scene, camera, GPU renderer, per-window error dialogs)
/// stays inside `ViewportController`, 1 instance per `MainWindow`. Closures
/// installed on a `MainWindow` that need to reach `AppState` hold a
/// `Weak<AppState>`, so the window->closure->AppState->window graph forms no
/// strong-reference cycle.
pub(crate) struct AppState {
    about_window: RefCell<Option<WiredAboutWindow>>,
    preferences_window: RefCell<Option<WiredPreferencesWindow>>,
    main_windows: RefCell<HashMap<u64, WiredMainWindow>>,
    next_window_id: Cell<u64>,
    prefs: Rc<PreferencesBus>,
    /// The merged printer and material catalog that the Preferences window and
    /// the Slice panels read. Behind a shared handle, so the Preferences window
    /// can rebuild it after a change to the user library, without a restart.
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    /// The plate of the app's default printer, which every window shows.
    default_envelope: Rc<DefaultEnvelope>,
    /// App-wide copy / paste clipboard. Populated from any window's current
    /// selection by `copy_selection`; consumed by the paste path which lives on
    /// the viewport controller. `None` until the user has performed the first
    /// copy.
    clipboard: RefCell<Option<Clipboard>>,
    /// The shared device's lost-device flag.
    device_loss: DeviceLoss,
    /// This run's autosaves (#18); `None` without a data folder.
    autosave: Option<Rc<AutosaveStore>>,
    /// Keeps the UI language equal to the preference. Set when the first window
    /// is created: Slint can select a language only once a component exists.
    ui_language: OnceCell<UiLanguage>,
    // Bus subscription owned by AppState. It runs on every `bus.update`;
    // dropping it unregisters. Stored here so it lives as long as `AppState`.
    // It holds the only `Sender` clone of the settings saver; dropping it lets
    // the saver thread observe channel-closed and exit its recv loop.
    _settings_sub: Subscription,
    // RAII handle that joins the settings-saver worker thread. Declared after
    // `_settings_sub` so the subscription's `Sender` is dropped first;
    // otherwise the join would deadlock waiting for the worker to exit while
    // the worker is still blocked on `recv`.
    _settings_saver: SettingsSaverJoin,
}

/// RAII handle joining the settings-saver thread on drop. The worker owns the
/// `AppSettings` instance and writes to disk off the Slint event-loop thread;
/// joining on drop makes the last save complete before the process exits.
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

/// Runs the settings-saver worker loop. Blocks on `recv`; on each wake drains
/// any further pending snapshots via `try_recv`, so a burst of rapid preference
/// changes collapses to a single `on_snapshot` call. Returns when every
/// `Sender` clone is dropped.
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

/// Copies every persisted preference out of a live snapshot into the on-disk
/// settings model. The single place the 2 representations are mapped, so the
/// saver thread and its tests stay in agreement.
fn apply_snapshot_to_settings(settings: &mut AppSettings, snap: &PreferencesSnapshot) {
    settings.set_theme(snap.theme);
    settings.set_menu_layout(snap.menu_layout);
    settings.set_language(snap.language);
    settings.set_default_printer(snap.default_printer.clone());
    settings.set_default_resin(snap.default_resin.clone());
    settings.set_favourite_printers(snap.favourite_printers.clone());
    settings.set_favourite_resins(snap.favourite_resins.clone());
    settings.set_output_format_by_printer(snap.output_format_by_printer.clone());
    settings.set_aa_level(snap.aa_level);
    settings.set_prescan_supports(snap.prescan_supports);
    settings.set_smooth_models(snap.smooth_models);
    settings.set_frame_rate(snap.frame_rate);
    settings.set_gizmo_style(snap.gizmo_style);
    settings.set_gizmo_size(snap.gizmo_size);
    settings.set_orthographic_view(snap.orthographic_view);
    settings.set_silenced_messages(snap.silenced_messages.clone());
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

/// Saves the changes that `snap` makes to `base` into `file`, and returns the
/// base of the next save: the settings with `snap` applied when the save
/// succeeded, `base` when it failed, so that the next save carries the failed
/// change again.
fn save_snapshot(
    file: Option<&SettingsFile>,
    base: AppSettings,
    snap: &PreferencesSnapshot,
) -> AppSettings {
    let mut ours = base.clone();
    apply_snapshot_to_settings(&mut ours, snap);
    let saved = match file {
        Some(file) => file.save_changes(&base, &ours),
        None => Err(std::io::Error::other("no config directory available")),
    };
    match saved {
        Ok(()) => ours,
        Err(err) => {
            eprintln!("Failed to save settings: {err}");
            base
        }
    }
}

/// Spawns the settings-saver worker thread. Its first step replaces an unusable
/// file with the defaults, so that the UI thread never writes; then each
/// coalesced snapshot goes through [`save_snapshot`]. With no file (the
/// platform gives no home directory), each save logs an error.
fn spawn_settings_saver(
    file: Option<SettingsFile>,
    loaded: LoadOutcome,
) -> std::io::Result<(Sender<PreferencesSnapshot>, SettingsSaverJoin)> {
    let (tx, rx) = mpsc::channel::<PreferencesSnapshot>();
    let join = thread::Builder::new()
        .name("settings-saver".into())
        .spawn(move || {
            if let Some(file) = &file {
                file.reset_if_unusable(&loaded);
            }
            let mut base = loaded.settings();
            run_settings_saver_loop(rx, |snap| {
                base = save_snapshot(file.as_ref(), std::mem::take(&mut base), snap);
            });
        })?;
    Ok((tx, SettingsSaverJoin { join: Some(join) }))
}

/// Subscribes `window` to the preferences of `bus`: each change applies them to
/// the window, a `DefaultPrinter` or `Profiles` change also shows the default
/// plate again in `controller` (a saved printer profile can change its size or
/// name, #326), and a `Language` change makes
/// `controller` build its texts again. `envelope` and `UiLanguage` must
/// subscribe to `bus` first, so that the listener reads the new plate and the
/// new language.
/// Shows the default printer's plate in `controller`, with the printer's
/// name on its front tab.
fn show_envelope(controller: &ViewportController, envelope: &DefaultEnvelope) {
    controller.set_build_volume(envelope.current());
    controller.set_plate_printer_name(envelope.printer_name());
}

fn subscribe_window_preferences(
    bus: &Rc<PreferencesBus>,
    window: &MainWindow,
    controller: Rc<ViewportController>,
    envelope: Rc<DefaultEnvelope>,
) -> Subscription {
    let window = window.as_weak();
    bus.subscribe(move |snap, change| {
        if let Some(w) = window.upgrade() {
            w.apply_preferences(snap);
        }
        if matches!(
            change,
            PreferencesChange::DefaultPrinter | PreferencesChange::Profiles
        ) {
            show_envelope(&controller, &envelope);
        }
        if matches!(change, PreferencesChange::Language) {
            controller.refresh_texts();
        }
        if matches!(change, PreferencesChange::SmoothModels) {
            crate::mesh_render::set_smooth_models(snap.smooth_models);
            controller.upload_scene_again();
        }
        if matches!(change, PreferencesChange::FrameRate) {
            crate::frame_rate::set_frame_rate_cap(snap.frame_rate);
        }
        if matches!(change, PreferencesChange::Gizmo) {
            controller.set_gizmo_prefs(snap.gizmo_style, snap.gizmo_size);
        }
    })
}

impl AppState {
    /// The app, with the profile catalog that `catalog` reads.
    pub(crate) fn new(
        device_loss: DeviceLoss,
        catalog: CatalogLoad,
    ) -> Result<Rc<Self>, Box<dyn Error>> {
        let settings_file = SettingsFile::platform_default();
        let loaded = settings_file
            .as_ref()
            .map_or(LoadOutcome::NotRead, SettingsFile::load);
        let settings = loaded.settings();
        crate::startup_timing::mark("settings");
        let catalog = Rc::new(RefCell::new(Rc::new(catalog.finish())));
        crate::startup_timing::mark("profile catalog");
        crate::mesh_render::set_smooth_models(settings.smooth_models());
        crate::frame_rate::set_frame_rate_cap(settings.frame_rate());
        let snapshot = PreferencesSnapshot {
            theme: settings.theme(),
            menu_layout: settings.menu_layout(),
            language: settings.language(),
            default_printer: settings.default_printer(),
            default_resin: settings.default_resin(),
            favourite_printers: settings.favourite_printers(),
            favourite_resins: settings.favourite_resins(),
            output_format_by_printer: settings.output_format_by_printer(),
            aa_level: settings.aa_level(),
            prescan_supports: settings.prescan_supports(),
            smooth_models: settings.smooth_models(),
            frame_rate: settings.frame_rate(),
            gizmo_style: settings.gizmo_style(),
            gizmo_size: settings.gizmo_size(),
            orthographic_view: settings.orthographic_view(),
            // Never stored: the log runs until the app closes and every launch
            // starts with it off (#74).
            diagnostic_log: false,
            silenced_messages: settings.silenced_messages(),
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
        let prefs = PreferencesBus::new(snapshot);

        // Persistence runs on a dedicated worker thread: the subscription
        // forwards each updated snapshot to it over an `mpsc` channel and the
        // worker writes to disk. Off-loading the write keeps the Slint event
        // loop responsive on slow / network-mounted / nearly-full config
        // directories.
        let (saver_tx, settings_saver) = spawn_settings_saver(settings_file, loaded)?;
        let settings_sub = prefs.subscribe(move |snap, change| {
            // A profile change leaves the snapshot as it was: nothing to write.
            if *change == PreferencesChange::Profiles {
                return;
            }
            // Channel send is non-blocking; failure means the worker has
            // already exited, which only happens at shutdown.
            let _ = saver_tx.send(snap.clone());
        });

        let default_envelope = DefaultEnvelope::new(&prefs, catalog.clone());

        let app = Rc::new(Self {
            about_window: RefCell::new(None),
            preferences_window: RefCell::new(None),
            main_windows: RefCell::new(HashMap::new()),
            next_window_id: Cell::new(0),
            prefs,
            catalog,
            default_envelope,
            clipboard: RefCell::new(None),
            device_loss,
            autosave: AutosaveStore::platform_root().map(|root| Rc::new(AutosaveStore::new(root))),
            ui_language: OnceCell::new(),
            _settings_sub: settings_sub,
            _settings_saver: settings_saver,
        });
        DEVICE_LOST_TARGET.with(|target| *target.borrow_mut() = Rc::downgrade(&app));
        crate::startup_timing::mark("app state");
        Ok(app)
    }

    /// Delivers a device loss to this thread's `AppState`, if one exists.
    pub(crate) fn dispatch_device_lost(reason: String) {
        let target = DEVICE_LOST_TARGET.with(|target| target.borrow().upgrade());
        if let Some(app) = target {
            app.on_device_lost(reason);
        }
    }

    /// Takes every window off the lost device (renderer dropped, loads refused
    /// with the reason) and tells the user once per process.
    fn on_device_lost(&self, reason: String) {
        let windows = self.main_windows.borrow();
        for wired in windows.values() {
            wired.controller.handle_rendering_teardown();
            wired.controller.loader().note_device_lost();
        }
        if !self.device_loss.take_report() {
            return;
        }
        if let Some(wired) = windows.values().next() {
            wired.controller.dialogs().show_error(move || ErrorTexts {
                heading: ctr!("ErrorDialog", "The graphics device was lost"),
                context: None,
                message: GraphicsUnavailable::DeviceLost.message().into(),
                details: reason.as_str().into(),
            });
        }
    }

    /// Creates, wires and shows a new `MainWindow`, and returns its id.
    pub(crate) fn spawn_main_window(self: &Rc<Self>) -> Result<u64, Box<dyn Error>> {
        let ui = MainWindow::new()?;
        crate::startup_timing::mark("main window component");
        crate::breadcrumbs::note("window: opened");
        self.on_first_window(&ui);
        crate::startup_timing::mark("translations");
        // Static platform flag consumed by the View menu to show the correct
        // fullscreen accelerator (Cmd+Ctrl+F on macOS, F11 elsewhere). Slint
        // has no compile-time platform branch, so the choice is made here and
        // pushed into the UI.
        ui.set_is_macos(cfg!(target_os = "macos"));
        let snapshot = self.prefs.snapshot();
        ui.apply_preferences(&snapshot);
        restore_panels(&ui, &snapshot);
        let document = SceneDocument::new();
        let pending_load = Rc::new(PendingLoad::new());
        let ctrl = Rc::new(ViewportController::new(
            &ui,
            document,
            pending_load.clone(),
            self.device_loss.clone(),
            &self.prefs,
        ));
        ctrl.register_document_listener();
        crate::startup_timing::mark("viewport controller");
        // Bus listeners run on `update` only, so the window gets the current
        // build volume here, and the subscription below delivers each later
        // change.
        show_envelope(&ctrl, &self.default_envelope);

        let id = self.next_window_id.get();
        let open_app = Rc::downgrade(self);
        crate::wiring::wire_main_window(
            &ui,
            ctrl.clone(),
            self.prefs.clone(),
            Rc::new(move |path| {
                if let Some(app) = open_app.upgrade() {
                    app.open_path_in_window(id, path);
                }
            }),
        )?;
        crate::startup_timing::mark("window wiring");
        let drivers = self.install_window_drivers(&ui, &ctrl);
        crate::startup_timing::mark("window drivers");
        self.wire_app_level_menus(&ui, &ctrl);

        self.next_window_id.set(id.wrapping_add(1));
        let autosaver = self
            .autosave
            .as_ref()
            .map(|store| Autosaver::install(id, store.clone(), &ctrl, self.prefs.clone()));
        self.wire_clipboard(&ui, id);
        self.wire_context_menu(&ui, &ctrl, id);
        let subscription = subscribe_window_preferences(
            &self.prefs,
            &ui,
            ctrl.clone(),
            self.default_envelope.clone(),
        );
        self.wire_window_events(&ui, id);
        crate::startup_timing::mark("window menus, autosave and events");

        ui.show()?;
        crate::startup_timing::mark("window show");
        // The winit window only exists once shown; set the taskbar icon here so
        // the live window matches the executable resource.
        crate::window_icon::apply(ui.window());
        // Open at a share of the monitor, not the 600 x 400 floor.
        crate::window_placement::fit_to_screen(&ui);
        crate::startup_timing::mark("window icon and placement");
        self.main_windows.borrow_mut().insert(
            id,
            WiredMainWindow {
                window: ui,
                controller: ctrl,
                pending_load,
                autosaver,
                _subscription: subscription,
                _drivers: drivers,
            },
        );
        Ok(id)
    }

    /// Puts the language preference in force from `ui` on, at the first call;
    /// later calls do nothing. Slint can select a language only once the thread
    /// has a component.
    fn on_first_window(&self, ui: &MainWindow) {
        self.ui_language
            .get_or_init(|| UiLanguage::follow(InstalledTranslations::proven_by(ui), &self.prefs));
    }

    /// Installs on `ui` the drivers of the Slice control panel and of the
    /// Slice, Repair and Generate Support actions, and wires the printer-file
    /// export to the control panel.
    fn install_window_drivers(
        &self,
        ui: &MainWindow,
        ctrl: &Rc<ViewportController>,
    ) -> WindowDrivers {
        let slice_panel = SlicePanelBridge::install(
            ui,
            self.catalog.clone(),
            self.prefs.clone(),
            ctrl.document(),
        );
        crate::export_print_file::wire_export_print_file(ui, ctrl.clone(), slice_panel.clone());
        let repair_run = RepairRun::install(ui, ctrl.document(), ctrl.dialogs(), ctrl.clone());
        let arrange_run = ArrangeRun::install(ui, ctrl.document(), ctrl.dialogs(), ctrl.clone());
        let support_gen_run =
            GenSupportRun::install(ui, ctrl.document(), ctrl.dialogs(), ctrl.clone());
        let df_support_run = DfSupportRun::install(
            ui,
            self.prefs.clone(),
            ctrl.document(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        let orient_run = OrientRun::install(
            ui,
            ctrl.document(),
            ctrl.dialogs(),
            ctrl.clone(),
            df_support_run.clone(),
        );
        let hollow_run = HollowRun::install(ui, ctrl.document(), ctrl.dialogs(), ctrl.clone());
        // The Review card applies its fixes through the support and hollow
        // drivers, and the Slice driver hands it each slice's findings.
        let review_panel = ReviewPanel::install(
            ui,
            ctrl.document(),
            ctrl.clone(),
            df_support_run.clone(),
            hollow_run.clone(),
        );
        let slice_run = SliceRun::install(
            ui,
            self.prefs.clone(),
            ctrl.document(),
            ctrl.dialogs(),
            review_panel.clone(),
        );
        let split_bodies = SplitBodies::install(ui, ctrl.document(), ctrl.dialogs(), ctrl.clone());
        WindowDrivers {
            _hollow_run: hollow_run,
            _split_bodies: split_bodies,
            _slice_panel: slice_panel,
            _slice_run: slice_run,
            _review_panel: review_panel,
            _repair_run: repair_run,
            _arrange_run: arrange_run,
            _orient_run: orient_run,
            _support_gen_run: support_gen_run,
            _df_support_run: df_support_run,
        }
    }

    /// Binds every copy and paste entry point of window `id` (its keystrokes
    /// and the items of its context menu) to the app-wide clipboard.
    fn wire_clipboard(self: &Rc<Self>, ui: &MainWindow, id: u64) {
        // The clipboard is app-wide, so copy and paste live in AppState, but
        // the keystrokes arrive through each window's MeshViewer FocusScope.
        // The closures bound here capture the window id and a Weak back to
        // AppState.
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
    }

    /// Binds the right button of window `id` to its context menu: a click opens
    /// the menu at the press position, a drag does not.
    fn wire_context_menu(self: &Rc<Self>, ui: &MainWindow, ctrl: &Rc<ViewportController>, id: u64) {
        // Down latches the press position on the controller; up classifies the
        // gesture.
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
    }

    /// Binds the OS events of window `id`: a close request releases the window,
    /// and a file dropped on it opens in it.
    fn wire_window_events(self: &Rc<Self>, ui: &MainWindow, id: u64) {
        {
            let app_weak = Rc::downgrade(self);
            ui.on_close_current_scene(move || {
                if let Some(app) = app_weak.upgrade() {
                    app.request_scene_close(id, false);
                }
            });
        }
        // On OS-level close: drop our strong handle so the window's
        // `ViewportController`, `Renderer`, scene, and the preference
        // subscription can be released. Without this, hidden windows accumulate
        // for the whole session.
        {
            let app_weak = Rc::downgrade(self);
            ui.window().on_close_requested(move || {
                if let Some(app) = app_weak.upgrade() {
                    let dirty = app
                        .main_windows
                        .borrow()
                        .get(&id)
                        .is_some_and(|w| w.controller.document().is_dirty());
                    if dirty {
                        app.request_scene_close(id, true);
                        return CloseRequestResponse::KeepWindowShown;
                    }
                    drop(app.remove_window(id));
                }
                CloseRequestResponse::HideWindow
            });
        }

        // Drag-and-drop: hand the winit file events of this window's surface to
        // `on_file_drag`. winit 0.30 emits these events on Windows, macOS and
        // X11; its Wayland backend emits none.
        {
            use slint::winit_030::{EventResult, WinitWindowAccessor};
            let app_weak = Rc::downgrade(self);
            let ui_weak = ui.as_weak();
            ui.window().on_winit_window_event(move |_window, event| {
                if let Some(drag) = FileDrag::from_winit(event)
                    && let Some(app) = app_weak.upgrade()
                    && let Some(w) = ui_weak.upgrade()
                {
                    app.on_file_drag(&w, id, drag);
                }
                EventResult::Propagate
            });
        }
    }

    /// Applies a step of a file drag to window `id`: the drop overlay shows
    /// while a file hovers it, and a dropped file opens in it.
    fn on_file_drag(self: &Rc<Self>, ui: &MainWindow, id: u64, drag: FileDrag) {
        match drag {
            FileDrag::Hovered => ui.set_drop_hovering(true),
            FileDrag::HoverCancelled => ui.set_drop_hovering(false),
            FileDrag::Dropped(path) => {
                ui.set_drop_hovering(false);
                self.open_path_in_window(id, path);
            }
        }
    }

    /// Routes an external (non-menu) open request into the right place: a path
    /// on `argv` at start-up, a drag-and-drop, or a file that the Finder opens
    /// on macOS. Branches:
    ///
    /// 1. Unrecognised window id (window closed since the request was queued):
    ///    silently drop.
    /// 2. Unsupported file extension: surface an error dialog and return.
    /// 3. Window already has a loaded scene: surface the
    ///    [`ReplaceSceneDialog`]. On Discard, close and load the new path; on
    ///    Save, run the Save flow (overwrite the project file, or Save As when
    ///    there is none) and only once the document is on disk close + load; on
    ///    Export, run the existing "Export As..." flow and only on a successful
    ///    export close + load; on Cancel, do nothing.
    /// 4. Otherwise dispatch through [`Self::dispatch_path_to_loader`], which
    ///    queues the path on the window's `PendingLoad` slot if the GPU device
    ///    isn't ready yet, or starts the load immediately if it is.
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
            crate::scene_save::saves_in_place(&controller.document()),
        );
        match action {
            OpenPathAction::RejectUnsupported => {
                controller.dialogs().show_error(move || ErrorTexts {
                    heading: ctr!("ErrorDialog", "Unsupported file type"),
                    context: Some(ctr!("ErrorDialog", "File: {}", path.display())),
                    message: ctr!(
                        "ErrorDialog",
                        "Only the following file types can be opened: {}",
                        SUPPORTED_MESH_EXTENSIONS.join(", ")
                    ),
                    details: Default::default(),
                });
            }
            OpenPathAction::DispatchImmediately => {
                self.dispatch_path_to_loader(window_id, path);
            }
            OpenPathAction::PromptReplace { offer } => {
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
                                app.save_then_change(window_id, SceneChange::Open(path));
                            }
                            PostPromptAction::ExportThenOpen => {
                                app.export_then_change(window_id, SceneChange::Open(path));
                            }
                            PostPromptAction::NoOp => {}
                        }
                    },
                );
            }
        }
    }

    /// Captures the currently-selected leaf from the focused window's
    /// [`SceneDocument`] into the app-wide clipboard. Silently does nothing if
    /// the window id is unknown (window closed since the keystroke fired), no
    /// scene is loaded, or no leaf is selected; in those cases the previous
    /// clipboard contents survive, matching the convention that only a
    /// successful copy overwrites the clipboard.
    pub(crate) fn copy_selection(&self, window_id: u64) {
        let controller = {
            let main_windows = self.main_windows.borrow();
            let Some(wired) = main_windows.get(&window_id) else {
                return;
            };
            wired.controller.clone()
        };
        // Copy is a Transform-section tool (it seeds a paste, which mutates the
        // scene); it stays inert in every other tab.
        if !controller.allows_layout_tools() {
            return;
        }
        let doc = controller.document();
        let Some(fragment) = fragment_from_document_selection(&doc) else {
            return;
        };
        *self.clipboard.borrow_mut() = Some(Clipboard::new(fragment));
    }

    /// Pastes the app-wide clipboard's fragment into the focused window. Silent
    /// no-op when the window id is unknown, the clipboard is empty, or the
    /// controller's paste path bails (no scene loaded, no camera ready,
    /// fragment empty, cursor ray missed the build plate). On a successful
    /// paste the cascade counter is bumped so the next `Ctrl+V` offsets
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

    /// Opens the context menu for a right-click at `(x, y)`. Runs the picker
    /// there; on a hit, moves the selection to that leaf so the existing
    /// `copy_selection` path applies to the hit object, and on a miss leaves
    /// the selection as it is. Computes the menu's visibility flags (Copy and
    /// Delete when something was hit, Paste when the clipboard has content) and
    /// asks the MainWindow to show the popup. Silent no-op when no item would
    /// be visible (so a right-click on empty space with an empty clipboard pops
    /// no menu).
    pub(crate) fn open_context_menu(&self, window_id: u64, x: f32, y: f32) {
        let main_windows = self.main_windows.borrow();
        let Some(wired) = main_windows.get(&window_id) else {
            return;
        };
        // The context menu only offers Copy / Paste / Delete, all
        // Transform-section tools; outside Transform there is nothing to show,
        // so a right-click there opens no menu (camera orbit on the right
        // button still works, it runs through a separate gesture path).
        if !wired.controller.allows_layout_tools() {
            return;
        }
        let hit = wired.controller.pick_node_at(x, y);
        // A right-click on one of several selected objects keeps them all, so
        // Delete acts on every one of them (#13); on any other object it
        // selects that one.
        if let Some(node_id) = hit {
            let document = wired.controller.document();
            if !document.is_selected(node_id) {
                document.set_selection(Selection::One(LeafId(node_id)));
            }
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

    /// Pastes the clipboard at an explicit logical pixel (the position where
    /// the right-click opened the context menu). Counterpart of
    /// `paste_at_cursor` for the menu-driven path: the cursor may have drifted
    /// inside the menu by the time the user clicks Paste, but the placement
    /// should still land at the original anchor pixel.
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

    /// Queues `path` for the window or starts its load. The caller must close
    /// any prior scene first.
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
            // A drop onto a busy window is silently ignored.
            return;
        };
        // Covers both a ready device and a renderer that will never provide
        // one; `start_load_with_ticket` reads the same graphics state and
        // deposits the reason for the latter, so a path that can never open
        // reports its failure at once.
        start_load_with_ticket(ticket, path, &loader, build_volume);
    }

    /// Clears the current scene, then dispatches the new path: the Discard
    /// choice of the replace prompt. Runs synchronously on the UI thread.
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

    fn finish_scene_change(&self, window_id: u64, change: SceneChange) {
        match change {
            SceneChange::Open(path) => self.discard_then_open(window_id, path),
            SceneChange::Clear => {
                let ctrl = self
                    .main_windows
                    .borrow()
                    .get(&window_id)
                    .map(|w| w.controller.clone());
                if let Some(ctrl) = ctrl {
                    ctrl.on_close_scene();
                }
            }
            SceneChange::CloseWindow => {
                let window = self.remove_window(window_id);
                if let Some(window) = window {
                    let _ = window.window.hide();
                }
            }
        }
    }

    /// Takes window `window_id` out of the app, with its scene's autosave:
    /// the window closes with nothing left to recover.
    fn remove_window(&self, window_id: u64) -> Option<WiredMainWindow> {
        let window = self.main_windows.borrow_mut().remove(&window_id)?;
        crate::breadcrumbs::note("window: closed");
        if let Some(autosaver) = &window.autosaver {
            autosaver.close();
        }
        Some(window)
    }

    /// At start-up: offers back the scenes that earlier runs left with
    /// unsaved changes (#18), asking in window `free`, which also takes the
    /// 1st scene recovered while it is still empty. Without `free` (the start
    /// opened files), it asks in the 1st window and recovers into new ones.
    pub(crate) fn offer_recovery(self: &Rc<Self>, free: Option<u64>) {
        let Some(store) = &self.autosave else {
            return;
        };
        let orphans = store.claim_orphans(std::time::SystemTime::now());
        if orphans.is_empty() {
            return;
        }
        let host = free.or_else(|| self.main_windows.borrow().keys().min().copied());
        let Some(dialogs) = host.and_then(|id| {
            self.main_windows
                .borrow()
                .get(&id)
                .map(|w| w.controller.dialogs())
        }) else {
            return;
        };
        let now = std::time::SystemTime::now();
        let scenes = orphans
            .iter()
            .flat_map(|run| &run.entries)
            .map(|entry| recovery_line(entry, now))
            .collect();
        let app_weak = Rc::downgrade(self);
        dialogs.show_recovery(scenes, move |choice| match choice {
            RecoveryChoice::Recover => {
                if let Some(app) = app_weak.upgrade() {
                    app.recover(orphans, free);
                }
            }
            RecoveryChoice::Discard => orphans.into_iter().for_each(OrphanRun::discard),
            // Dropped: their locks go, and the next start offers them again.
            RecoveryChoice::Later => drop(orphans),
        });
    }

    /// Opens each scene of `orphans` in a window of its own: `free` for the
    /// 1st while it is still empty, new ones after. Each becomes the autosave
    /// of its window, and a run whose scenes all moved is deleted; one that
    /// could not be moved stays, to be offered again.
    fn recover(self: &Rc<Self>, orphans: Vec<OrphanRun>, mut free: Option<u64>) {
        let Some(store) = self.autosave.clone() else {
            return;
        };
        for run in orphans {
            let mut moved = true;
            for entry in &run.entries {
                let window = match free.take().filter(|id| self.is_empty_window(*id)) {
                    Some(id) => id,
                    None => match self.spawn_main_window() {
                        Ok(id) => id,
                        Err(err) => {
                            eprintln!("Failed to open a window for a recovered scene: {err}");
                            moved = false;
                            continue;
                        }
                    },
                };
                match store.adopt(entry, window) {
                    Ok(snapshot) => self.open_path_in_window(window, snapshot),
                    Err(err) => {
                        crate::breadcrumbs::note(&format!("autosave: not adopted: {err:#}"));
                        moved = false;
                        self.open_path_in_window(window, entry.snapshot.clone());
                    }
                }
            }
            if moved {
                run.discard();
            }
        }
    }

    /// Whether window `window_id` shows no scene and loads none.
    fn is_empty_window(&self, window_id: u64) -> bool {
        self.main_windows.borrow().get(&window_id).is_some_and(|w| {
            !w.controller.document().is_loaded() && !w.controller.loader().is_busy()
        })
    }

    /// Ends this run's autosaves, as the app exits: their folder goes unless
    /// a scene was left unsaved in it.
    pub(crate) fn finish_autosave(&self) {
        if let Some(store) = &self.autosave {
            store.finish();
        }
    }

    /// Both File > Close and the OS close button must keep dirty work until a
    /// confirmed discard or a successful, current save/export.
    fn request_scene_close(self: &Rc<Self>, window_id: u64, close_window: bool) {
        let ctrl = self
            .main_windows
            .borrow()
            .get(&window_id)
            .map(|w| w.controller.clone());
        let Some(ctrl) = ctrl else {
            return;
        };
        ctrl.on_modal_transform_finish(false);
        let change = if close_window {
            SceneChange::CloseWindow
        } else {
            SceneChange::Clear
        };
        if !ctrl.document().is_dirty() {
            self.finish_scene_change(window_id, change);
            return;
        }
        let offer = if crate::scene_save::saves_in_place(&ctrl.document()) {
            crate::error_dialog::KeepChangesOffer::SaveExisting
        } else {
            crate::error_dialog::KeepChangesOffer::SaveAsNew
        };
        let app_weak = Rc::downgrade(self);
        ctrl.dialogs()
            .show_close_scene_confirm(offer, move |choice| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                match map_replace_choice(choice) {
                    PostPromptAction::DiscardThenOpen => app.finish_scene_change(window_id, change),
                    PostPromptAction::SaveThenOpen => app.save_then_change(window_id, change),
                    PostPromptAction::ExportThenOpen => app.export_then_change(window_id, change),
                    PostPromptAction::NoOp => {}
                }
            });
    }

    /// Runs the Save flow for the Save choice of the replace prompt: overwrite
    /// the document's project file, or fall back to the Save As flow when there
    /// is none. Only once the document is safely on disk does it close the
    /// current scene and dispatch the new path. A cancelled Save As dialog or a
    /// failed write leaves the current scene intact (`run_save_flow` shows the
    /// failure dialog itself). A write that completed against a document which
    /// moved on in the meantime also leaves the scene intact, and says so: the
    /// user asked to replace what they had saved, and that is not what is on
    /// disk.
    fn save_then_change(self: &Rc<Self>, window_id: u64, change: SceneChange) {
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
        let defaults = PrintDefaults::from(&self.prefs.snapshot());
        let app_weak = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            let outcome =
                crate::wiring::run_save_flow(doc, dialogs.clone(), save_progress, defaults).await;
            match outcome {
                SaveOutcome::Saved => {}
                SaveOutcome::MovedOn => {
                    dialogs.show_info(|| InfoTexts {
                        heading: ctr!("SaveDialog", "Scene not replaced"),
                        message: ctr!(
                            "SaveDialog",
                            "The scene changed while saving, so the file does not hold its current state. Nothing was replaced."
                        ),
                        details: Default::default(),
                    });
                    return;
                }
                SaveOutcome::NotSaved => return,
            }
            if let Some(app) = app_weak.upgrade() {
                app.finish_scene_change(window_id, change);
            }
        }) {
            eprintln!("Failed to spawn Save & Open task: {err}");
        }
    }

    /// Runs the "Export As..." flow for the Export choice of the replace prompt
    /// and, only on a successful export, closes the current scene and
    /// dispatches the new path. A cancelled export dialog or a failed write
    /// leaves the current scene intact (`run_export_as_flow` shows the failure
    /// dialog itself).
    fn export_then_change(self: &Rc<Self>, window_id: u64, change: SceneChange) {
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
                app.finish_scene_change(window_id, change);
            }
        }) {
            eprintln!("Failed to spawn Export & Open task: {err}");
        }
    }

    fn wire_app_level_menus(self: &Rc<Self>, ui: &MainWindow, ctrl: &Rc<ViewportController>) {
        // Closures hold `Weak<AppState>`, so the MainWindow -> closure ->
        // AppState -> main_windows(MainWindow) chain forms no strong cycle.
        ui.on_support_page_requested(|page| open_in_browser(support_page_url(page)));
        {
            let app = Rc::downgrade(self);
            let ui_weak = ui.as_weak();
            ui.on_show_about_requested(move || {
                if let (Some(app), Some(ui)) = (app.upgrade(), ui_weak.upgrade()) {
                    app.show_about(&ui);
                }
            });
        }
        {
            let ui_weak = ui.as_weak();
            let viewport = Rc::downgrade(ctrl);
            let bus = self.prefs.clone();
            ui.on_export_diagnostics_requested(move || {
                if let (Some(ui), Some(ctrl)) = (ui_weak.upgrade(), viewport.upgrade()) {
                    crate::diagnostics::export(&ui, ctrl.document(), ctrl.dialogs(), bus.clone());
                }
            });
        }
        {
            let app = Rc::downgrade(self);
            let ui_weak = ui.as_weak();
            let viewport = Rc::downgrade(ctrl);
            ui.on_show_preferences_requested(move || {
                if let (Some(app), Some(ui)) = (app.upgrade(), ui_weak.upgrade()) {
                    app.show_preferences(&ui, viewport.clone());
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
            // Persist and broadcast a menu-layout switch through the bus, like
            // the preferences window does for the theme. Every window
            // re-applies it and the settings saver writes it out.
            let bus = self.prefs.clone();
            ui.on_menu_layout_change_requested(move |layout| {
                bus.update(move |snap| {
                    snap.menu_layout = layout;
                    PreferencesChange::MenuLayout
                });
            });
        }
        {
            // Persist the object-list panel's visibility. It is per-window live
            // state: the bus write reaches the settings saver, and no window
            // re-applies it (see `window_prefs::restore_panels`).
            let bus = self.prefs.clone();
            ui.on_object_list_visibility_changed(move |visible| {
                bus.update(move |snap| {
                    snap.object_list_visible = visible;
                    PreferencesChange::ObjectListPanel
                });
            });
        }
        {
            // Persist the panel geometry after a drag, resize, or minimize.
            // Same per-window live handling as the visibility.
            let bus = self.prefs.clone();
            ui.on_object_list_geometry_changed(
                move |anchor_right, anchor_bottom, offset_x, offset_y, width, height, minimized| {
                    // A 0px height means the panel is auto-sizing; store it as
                    // "unset" so it keeps auto-sizing after a restart.
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
                        // A gesture that ended without changing anything (for
                        // example a plain click on the title bar) must not
                        // trigger a redundant settings write.
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
            // Persist the Slice control panel geometry after a drag or resize.
            // Per-window live state, like the object-list panel.
            let bus = self.prefs.clone();
            ui.on_slice_panel_geometry_changed(
                move |anchor_right, anchor_bottom, offset_x, offset_y, width, height| {
                    // A 0px height means the panel is auto-sizing; store it as
                    // "unset" so it keeps auto-sizing after a restart.
                    let height = if height > 0.0 { Some(height) } else { None };
                    let current = bus.snapshot();
                    if current.slice_panel_anchor_right == anchor_right
                        && current.slice_panel_anchor_bottom == anchor_bottom
                        && current.slice_panel_offset_x == Some(offset_x)
                        && current.slice_panel_offset_y == Some(offset_y)
                        && current.slice_panel_width == Some(width)
                        && current.slice_panel_height == height
                    {
                        // A gesture that ended without changing anything must
                        // not trigger a redundant settings write.
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
            // Persist the slice-view panel's visibility. Per-window live state,
            // like the object-list panel: the bus write reaches the settings
            // saver but no window re-applies it.
            let bus = self.prefs.clone();
            ui.on_slice_view_visibility_changed(move |visible| {
                bus.update(move |snap| {
                    snap.slice_view_visible = visible;
                    PreferencesChange::SliceViewPanel
                });
            });
        }
        {
            // Persist the slice-view panel geometry after a drag, resize, or
            // minimize. Same per-window live handling as the object list.
            let bus = self.prefs.clone();
            ui.on_slice_view_geometry_changed(
                move |anchor_right, anchor_bottom, offset_x, offset_y, width, height, minimized| {
                    // A 0px height means the panel is auto-sizing; store it as
                    // "unset" so it keeps auto-sizing after a restart.
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
                        // A gesture that ended without changing anything must
                        // not trigger a redundant settings write.
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

    /// Shows the app-wide About window centred over `over`, the window that
    /// asked for it, creating it on first use and re-showing the same instance
    /// on subsequent calls. The window's bus subscription keeps it up-to-date
    /// while hidden, so no extra re-apply is needed here.
    fn show_about(&self, over: &MainWindow) {
        let mut slot = self.about_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.window.show();
            crate::window_placement::centre_over(&existing.window, over);
            return;
        }
        match build_about_window(&self.prefs) {
            Ok(wired) => {
                if let Err(err) = wired.window.show() {
                    eprintln!("Failed to show About window: {err}");
                    return;
                }
                crate::window_placement::centre_over(&wired.window, over);
                *slot = Some(wired);
            }
            Err(err) => eprintln!("Failed to create About window: {err}"),
        }
    }

    /// Shows the app-wide Preferences window centred over `over`, the window
    /// that asked for it, creating it on first use and re-showing the same
    /// instance on subsequent calls. The window's bus subscription keeps it
    /// up-to-date while hidden. Its performance test runs in `viewport`, the
    /// 3D view of `over`.
    fn show_preferences(&self, over: &MainWindow, viewport: Weak<ViewportController>) {
        let mut slot = self.preferences_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            wire_performance_test(&existing.window, viewport);
            let _ = existing.window.show();
            crate::window_placement::centre_over(&existing.window, over);
            return;
        }
        match build_preferences_window(&self.prefs, self.catalog.clone()) {
            Ok(wired) => {
                wire_performance_test(&wired.window, viewport);
                if let Err(err) = wired.window.show() {
                    eprintln!("Failed to show Preferences window: {err}");
                    return;
                }
                crate::window_placement::centre_over(&wired.window, over);
                *slot = Some(wired);
            }
            Err(err) => eprintln!("Failed to create Preferences window: {err}"),
        }
    }
}

/// Where the top bar's support links lead.
fn support_page_url(page: crate::SupportPage) -> &'static str {
    match page {
        crate::SupportPage::Patreon => "https://www.patreon.com/cw/synrgstudio",
        crate::SupportPage::KoFi => "https://ko-fi.com/synrg",
    }
}

/// Opens a web page in the default browser, detached: a failure leaves
/// nothing to report.
fn open_in_browser(url: &str) {
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(url).spawn();
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
        // `document_is_dirty` / `document_saves_in_place` are irrelevant when
        // nothing is loaded.
        assert_eq!(
            decide_open_path_action(true, false, true, true),
            OpenPathAction::DispatchImmediately
        );
    }

    #[test]
    fn clean_document_prompts_with_nothing_to_keep() {
        // Whether a project file exists doesn't matter when there are no
        // unsaved changes: nothing would be lost.
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
    use sc_print_primitives::{MaterialId, PrinterId};

    use super::*;
    use crate::ThemePreference;
    use crate::language::{BundledLanguage, LanguagePreference};
    use sc_print_primitives::AaLevel;

    fn snapshot_with(theme: ThemePreference) -> PreferencesSnapshot {
        PreferencesSnapshot {
            theme,
            ..PreferencesSnapshot::for_tests()
        }
    }

    #[test]
    fn apply_snapshot_copies_favourites_into_settings() {
        let mut snap = snapshot_with(ThemePreference::System);
        let printer = PrinterId::new("anycubic-photon-m3").unwrap();
        let resin = MaterialId::new("abs-like-405").unwrap();
        snap.favourite_printers.insert(printer.clone());
        snap.favourite_resins.insert(resin.clone());

        let mut settings = AppSettings::default();
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(settings.favourite_printers().contains(&printer));
        assert!(settings.favourite_resins().contains(&resin));
    }

    #[test]
    fn apply_snapshot_copies_the_silenced_messages_into_settings() {
        let mut snap = snapshot_with(ThemePreference::System);
        snap.silenced_messages.insert("repair.summary".to_owned());

        let mut settings = AppSettings::default();
        apply_snapshot_to_settings(&mut settings, &snap);

        assert_eq!(settings.silenced_messages(), snap.silenced_messages);
    }

    #[test]
    fn apply_snapshot_copies_the_per_printer_output_format_into_settings() {
        use sc_print_primitives::{FormatKind, NonEmptyString, OutputFormatBinding};

        let mut snap = snapshot_with(ThemePreference::System);
        let printer = PrinterId::new("elegoo-mars-5").unwrap();
        let binding = OutputFormatBinding {
            format: FormatKind::Goo,
            version: Some(NonEmptyString::new("v3.0").unwrap()),
        };
        snap.output_format_by_printer
            .insert(printer.clone(), binding.clone());

        let mut settings = AppSettings::default();
        apply_snapshot_to_settings(&mut settings, &snap);

        assert_eq!(
            settings.output_format_by_printer().get(&printer),
            Some(&binding),
        );
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
        // If the loop's exit condition regresses, this test hangs;
        // `AppState::drop` would deadlock the same way.
        run_settings_saver_loop(rx, |_| {
            panic!("on_snapshot must not fire when no snapshots were sent");
        });
    }

    #[test]
    fn pre_queued_snapshots_coalesce_to_the_last_one() {
        // The 3 snapshots are queued before the loop runs, so the first `recv`
        // returns 1 and `try_recv` drains the other 2 in the same wake: 1 call,
        // with the last snapshot.
        let (tx, rx) = mpsc::channel::<PreferencesSnapshot>();
        tx.send(snapshot_with(ThemePreference::Light)).unwrap();
        tx.send(snapshot_with(ThemePreference::Dark)).unwrap();
        tx.send(snapshot_with(ThemePreference::System)).unwrap();
        drop(tx);
        let mut seen = Vec::new();
        run_settings_saver_loop(rx, |snap| seen.push(snap.theme));
        assert_eq!(seen.len(), 1);
        assert!(matches!(seen[0], ThemePreference::System));
    }

    #[test]
    fn loop_coalesces_burst_to_latest_snapshot() {
        // Queue a burst of snapshots before the loop starts. The first `recv`
        // returns 1; `try_recv` drains the rest in the same wake; the loop
        // calls `on_snapshot` once with the LAST value queued. Catches
        // regressions in the inner drain (for example an off-by-one or a
        // missing while loop) that would write each snapshot.
        let (tx, rx) = mpsc::channel::<PreferencesSnapshot>();
        let mut last_width = None;
        for i in 0..16 {
            let width = Some(100.0 + i as f32);
            last_width = width;
            tx.send(PreferencesSnapshot {
                object_list_width: width,
                ..PreferencesSnapshot::for_tests()
            })
            .unwrap();
        }
        drop(tx);

        let mut calls: Vec<Option<f32>> = Vec::new();
        run_settings_saver_loop(rx, |snap| calls.push(snap.object_list_width));

        assert_eq!(calls.len(), 1, "burst must coalesce to one call");
        assert_eq!(calls[0], last_width);
    }

    #[test]
    fn saver_mapping_carries_object_list_visibility() {
        // The snapshot-to-settings mapping must copy the panel visibility so a
        // hide survives a save.
        let mut settings = AppSettings::default();
        assert!(settings.object_list_visible());

        let mut snap = snapshot_with(ThemePreference::System);
        snap.object_list_visible = false;
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(!settings.object_list_visible());
    }

    #[test]
    fn saver_mapping_carries_object_list_geometry() {
        // The mapping must copy the panel anchor, offsets, size, and minimized
        // state so a drag or resize survives a save.
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
        // The snapshot-to-settings mapping must copy the slice-view panel
        // visibility so a hide survives a save.
        let mut settings = AppSettings::default();
        assert!(settings.slice_view_visible());

        let mut snap = snapshot_with(ThemePreference::System);
        snap.slice_view_visible = false;
        apply_snapshot_to_settings(&mut settings, &snap);

        assert!(!settings.slice_view_visible());
    }

    #[test]
    fn saver_mapping_carries_slice_view_geometry() {
        // The mapping must copy the slice-view panel anchor, offsets, size, and
        // minimized state so a drag or resize survives a save.
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
    fn saver_mapping_carries_the_projection() {
        let mut settings = AppSettings::default();
        let mut snap = snapshot_with(ThemePreference::System);
        snap.orthographic_view = true;
        apply_snapshot_to_settings(&mut settings, &snap);
        assert!(settings.orthographic_view());
    }

    #[test]
    fn saver_mapping_carries_last_open_dir() {
        // The mapping must copy the remembered open directory so it survives a
        // save.
        let mut settings = AppSettings::default();
        assert_eq!(settings.last_open_dir(), None);

        let mut snap = snapshot_with(ThemePreference::System);
        snap.last_open_dir = Some(PathBuf::from("/models"));
        apply_snapshot_to_settings(&mut settings, &snap);

        assert_eq!(settings.last_open_dir(), Some(PathBuf::from("/models")));
    }

    #[test]
    fn spawn_settings_saver_joins_cleanly_on_drop() {
        // Smoke test for the production spawn path: drop the sender without
        // sending any snapshot, so the worker exits via channel-closed. The
        // join in `SettingsSaverJoin::drop` must complete without hanging,
        // matching the shutdown behaviour `AppState::drop` relies on.
        let (tx, saver) =
            spawn_settings_saver(None, LoadOutcome::NotRead).expect("spawn settings-saver thread");
        drop(tx);
        drop(saver);
    }

    #[test]
    fn the_saver_replaces_an_unusable_file_first() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        std::fs::write(&path, "this is not valid TOML").unwrap();
        let file = SettingsFile::at(path.clone());
        let loaded = file.load();

        let (tx, saver) =
            spawn_settings_saver(Some(file), loaded).expect("spawn settings-saver thread");
        drop(tx);
        drop(saver);

        let written = std::fs::read_to_string(&path).unwrap();
        let version = crate::app_settings::APP_SETTINGS_SCHEMA_VERSION;
        assert!(
            written.contains(&format!("version = {version}")),
            "got:\n{written}"
        );
    }

    /// Loads the file at `path` and starts a saver on it, as a new process of
    /// the app does.
    fn start_saver(path: PathBuf) -> (Sender<PreferencesSnapshot>, SettingsSaverJoin) {
        let file = SettingsFile::at(path);
        let loaded = file.load();
        spawn_settings_saver(Some(file), loaded).expect("spawn settings-saver thread")
    }

    #[test]
    fn two_processes_keep_each_others_changes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let es = LanguagePreference::Bundled(BundledLanguage::from_code("es").unwrap());
        let (to_a, saver_a) = start_saver(path.clone());
        let (to_b, saver_b) = start_saver(path.clone());

        let a = PreferencesSnapshot {
            language: es,
            ..PreferencesSnapshot::for_tests()
        };
        to_a.send(a).unwrap();
        drop(to_a);
        drop(saver_a);
        let b = PreferencesSnapshot {
            object_list_visible: false,
            ..PreferencesSnapshot::for_tests()
        };
        to_b.send(b).unwrap();
        drop(to_b);
        drop(saver_b);

        let saved = SettingsFile::at(path).load().settings();
        assert_eq!(saved.language(), es);
        assert!(!saved.object_list_visible());
    }

    #[test]
    fn two_processes_keep_each_others_favourites() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let x = PrinterId::new("printer-x").unwrap();
        let y = PrinterId::new("printer-y").unwrap();
        let (to_a, saver_a) = start_saver(path.clone());
        let (to_b, saver_b) = start_saver(path.clone());

        let mut a = PreferencesSnapshot::for_tests();
        a.favourite_printers.insert(x.clone());
        to_a.send(a).unwrap();
        drop(to_a);
        drop(saver_a);
        let mut b = PreferencesSnapshot::for_tests();
        b.favourite_printers.insert(y.clone());
        to_b.send(b).unwrap();
        drop(to_b);
        drop(saver_b);

        let saved = SettingsFile::at(path).load().settings();
        assert_eq!(
            saved.favourite_printers(),
            std::collections::BTreeSet::from([x, y])
        );
    }

    #[test]
    fn a_good_save_makes_the_snapshot_the_next_base() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = SettingsFile::at(tmp.path().join("settings.toml"));
        let snap = PreferencesSnapshot {
            object_list_visible: false,
            ..PreferencesSnapshot::for_tests()
        };

        let next = save_snapshot(Some(&file), AppSettings::default(), &snap);
        assert!(!next.object_list_visible());
        assert!(!file.load().settings().object_list_visible());
    }

    #[test]
    fn the_save_after_a_failed_one_carries_its_change() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let file = SettingsFile::at(path.clone());
        // A directory that is not empty in the place of the file makes the save
        // fail.
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("blocker"), "").unwrap();
        let es = LanguagePreference::Bundled(BundledLanguage::from_code("es").unwrap());
        let first = PreferencesSnapshot {
            language: es,
            ..PreferencesSnapshot::for_tests()
        };

        let base = save_snapshot(Some(&file), AppSettings::default(), &first);
        assert_eq!(base, AppSettings::default());

        // The file comes back empty, which reads as the defaults. A base that
        // took the failed change would leave `es` out of this save.
        std::fs::remove_dir_all(&path).unwrap();
        std::fs::write(&path, "").unwrap();
        let second = PreferencesSnapshot {
            object_list_visible: false,
            ..first
        };
        save_snapshot(Some(&file), base, &second);

        let saved = file.load().settings();
        assert_eq!(saved.language(), es);
        assert!(!saved.object_list_visible());
    }

    #[test]
    fn with_no_file_the_base_stays() {
        let snap = PreferencesSnapshot {
            object_list_visible: false,
            ..PreferencesSnapshot::for_tests()
        };
        let next = save_snapshot(None, AppSettings::default(), &snap);
        assert_eq!(next, AppSettings::default());
    }
}

#[cfg(test)]
mod subscribe_window_preferences_tests {
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_print_primitives::PrinterId;
    use sc_scene_fixtures::meshes_as_roots;
    use slint::Model;

    use super::*;
    use crate::ThemePreference;
    use crate::build_volume::{BuildVolume, NO_PRINTER_BUILD_VOLUME};
    use crate::test_support::catalog_entries::printer_entry;
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    /// A main window and its controller, subscribed to `bus` after the UI
    /// language, as `spawn_main_window` does. The catalog holds 1 printer, and
    /// the default printer of `bus` is not in it, so the default plate starts
    /// as the placeholder.
    struct SubscribedWindow {
        bus: Rc<PreferencesBus>,
        window: MainWindow,
        controller: Rc<ViewportController>,
        _ui_language: UiLanguage,
        _subscription: Subscription,
    }

    fn listed_printer() -> PrinterId {
        PrinterId::new("listed-printer").expect("valid printer id")
    }

    fn subscribed_window() -> SubscribedWindow {
        let ghost = PrinterId::new("ghost-printer").expect("valid printer id");
        let bus = PreferencesBus::new(PreferencesSnapshot {
            default_printer: Some(ghost),
            ..PreferencesSnapshot::for_tests()
        });
        let catalog = ProfileCatalog::from_entries(
            vec![printer_entry(
                listed_printer(),
                "Alpha",
                [120.0, 68.0, 150.0],
            )],
            Vec::new(),
        );
        let envelope = DefaultEnvelope::new(&bus, Rc::new(RefCell::new(Rc::new(catalog))));
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&window), &bus);
        let controller = Rc::new(ViewportController::new(
            &window,
            SceneDocument::new(),
            Rc::new(PendingLoad::new()),
            DeviceLoss::default(),
            &bus,
        ));
        controller.register_document_listener();
        show_envelope(&controller, &envelope);
        let subscription =
            subscribe_window_preferences(&bus, &window, controller.clone(), envelope);
        SubscribedWindow {
            bus,
            window,
            controller,
            _ui_language: ui_language,
            _subscription: subscription,
        }
    }

    fn first_object_name(window: &MainWindow) -> String {
        window
            .get_object_list()
            .row_data(0)
            .expect("the scene has an object")
            .text
            .to_string()
    }

    #[test]
    fn a_theme_change_reaches_the_window() {
        let (before, after) = on_fresh_slint_thread(|| {
            let subscribed = subscribed_window();
            let before = subscribed.window.get_theme();
            subscribed.bus.update(|snap| {
                snap.theme = ThemePreference::Dark;
                PreferencesChange::Theme
            });
            (before, subscribed.window.get_theme())
        });
        assert_eq!(before, ThemePreference::System);
        assert_eq!(after, ThemePreference::Dark);
    }

    #[test]
    fn a_default_printer_change_sets_the_new_default_plate() {
        let (before, after) = on_fresh_slint_thread(|| {
            let subscribed = subscribed_window();
            let before = subscribed.controller.build_volume();
            subscribed.bus.update(|snap| {
                snap.default_printer = Some(listed_printer());
                PreferencesChange::DefaultPrinter
            });
            (before, subscribed.controller.build_volume())
        });
        assert_eq!(before, NO_PRINTER_BUILD_VOLUME);
        assert_eq!(after, BuildVolume::new(120.0, 68.0, 150.0));
    }

    #[test]
    fn every_other_change_keeps_the_build_volume() {
        for change in [
            PreferencesChange::Theme,
            PreferencesChange::MenuLayout,
            PreferencesChange::Language,
            PreferencesChange::DefaultResin,
            PreferencesChange::Favourites,
            PreferencesChange::OutputFormat,
            PreferencesChange::AaLevel,
            PreferencesChange::PrescanSupports,
            PreferencesChange::SmoothModels,
            PreferencesChange::FrameRate,
            PreferencesChange::Gizmo,
            PreferencesChange::Projection,
            PreferencesChange::DiagnosticLog,
            PreferencesChange::SilencedMessages,
            PreferencesChange::ObjectListPanel,
            PreferencesChange::SliceControlPanel,
            PreferencesChange::SliceViewPanel,
            PreferencesChange::LastOpenDir,
        ] {
            let build_volume = on_fresh_slint_thread(move || {
                let subscribed = subscribed_window();
                // Not the default plate, so a push of the plate would show.
                subscribed
                    .controller
                    .set_build_volume(BuildVolume::new(10.0, 20.0, 30.0));
                subscribed.bus.update(move |_| change);
                subscribed.controller.build_volume()
            });
            assert_eq!(
                build_volume,
                BuildVolume::new(10.0, 20.0, 30.0),
                "{change:?}"
            );
        }
    }

    #[test]
    fn a_language_change_pushes_the_object_names_again() {
        let (english, spanish, expected) = on_fresh_slint_thread(|| {
            let subscribed = subscribed_window();
            subscribed
                .controller
                .install_graph_for_test(meshes_as_roots([unit_cube_mesh()]));
            pick_language(&subscribed.bus, "en");
            let english = first_object_name(&subscribed.window);
            pick_language(&subscribed.bus, "es");
            let expected = ctr!("ObjectList", "Object {}", 1).to_string();
            (english, first_object_name(&subscribed.window), expected)
        });
        assert_eq!(english, "Object 1");
        assert_ne!(spanish, english);
        assert_eq!(spanish, expected);
    }

    #[test]
    fn a_language_change_repaints_the_viewport() {
        let owed = on_fresh_slint_thread(|| {
            let subscribed = subscribed_window();
            subscribed.controller.clear_frame_state_for_test();
            pick_language(&subscribed.bus, "es");
            subscribed.controller.redraw_owed_for_test()
        });
        assert!(owed);
    }

    /// Spanish runs longer than English, and a narrow window wraps it more:
    /// the question before the diagnostic log still holds its buttons (#74).
    #[test]
    fn the_log_question_holds_its_buttons_in_a_narrow_window() {
        let around = on_fresh_slint_thread(|| {
            let subscribed = subscribed_window();
            pick_language(&subscribed.bus, "es");
            let catalog = Rc::new(RefCell::new(Rc::new(ProfileCatalog::from_entries(
                Vec::new(),
                Vec::new(),
            ))));
            let prefs = build_preferences_window(&subscribed.bus, catalog)
                .expect("the headless platform creates the Preferences window");
            prefs
                .window
                .window()
                .set_size(slint::PhysicalSize::new(520, 360));
            prefs.window.show().expect("the headless platform shows it");
            prefs.window.invoke_diagnostic_log_changed(true);
            crate::test_support::message_cards::card_around_primary_button(prefs.window.window())
        });
        assert_eq!(around, (true, true), "(card right of the button, below it)");
    }

    /// The whole way of Preferences > Privacy (#74): the box ticked, the
    /// question answered with Enter in the window, `session.log` started with
    /// the session so far, and the box shown on.
    #[test]
    fn turning_the_log_on_in_preferences_writes_session_log() {
        let _serial = crate::breadcrumbs::serial();
        let dir = tempfile::tempdir().expect("a temporary folder");
        let log = on_fresh_slint_thread(|| {
            crate::breadcrumbs::write_log_into(dir.path());
            crate::breadcrumbs::start(env!("CARGO_PKG_VERSION"), crate::diagnostics::commit());
            let subscribed = subscribed_window();
            let catalog = Rc::new(RefCell::new(Rc::new(ProfileCatalog::from_entries(
                Vec::new(),
                Vec::new(),
            ))));
            let prefs = build_preferences_window(&subscribed.bus, catalog)
                .expect("the headless platform creates the Preferences window");
            prefs.window.show().expect("the headless platform shows it");
            prefs.window.invoke_diagnostic_log_changed(true);
            let window = prefs.window.window();
            for event in [
                slint::platform::WindowEvent::KeyPressed {
                    text: slint::platform::Key::Return.into(),
                },
                slint::platform::WindowEvent::KeyReleased {
                    text: slint::platform::Key::Return.into(),
                },
            ] {
                window.dispatch_event(event);
            }
            crate::breadcrumbs::note("test: after turning it on");
            let log = std::fs::read_to_string(dir.path().join("session.log"));
            let shown_on = prefs.window.get_diagnostic_log();
            // Closes the file, so the folder can go.
            crate::breadcrumbs::set_enabled(false).expect("the trail has started");
            (log, shown_on)
        });
        let (log, shown_on) = log;
        assert!(shown_on);
        let log = log.expect("turning the log on starts session.log");
        assert!(log.contains(" start v"), "{log}");
        assert!(log.contains("test: after turning it on"), "{log}");
    }
}

#[cfg(test)]
mod file_drag_tests {
    use super::*;

    #[test]
    fn a_hovered_file_is_a_hover() {
        let event = winit::event::WindowEvent::HoveredFile(PathBuf::from("/models/cube.stl"));
        assert_eq!(FileDrag::from_winit(&event), Some(FileDrag::Hovered));
    }

    #[test]
    fn a_cancelled_hover_is_a_hover_cancel() {
        let event = winit::event::WindowEvent::HoveredFileCancelled;
        assert_eq!(FileDrag::from_winit(&event), Some(FileDrag::HoverCancelled));
    }

    #[test]
    fn a_dropped_file_keeps_its_path() {
        let event = winit::event::WindowEvent::DroppedFile(PathBuf::from("/models/cube.stl"));
        assert_eq!(
            FileDrag::from_winit(&event),
            Some(FileDrag::Dropped(PathBuf::from("/models/cube.stl")))
        );
    }

    #[test]
    fn another_window_event_is_no_drag() {
        let event = winit::event::WindowEvent::Focused(true);
        assert_eq!(FileDrag::from_winit(&event), None);
    }
}

#[cfg(test)]
mod window_wiring_tests {
    use std::sync::Arc;

    use crate::build_volume::BuildVolume;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_print_primitives::PrinterId;
    use sc_scene_edit::SceneFragment;
    use sc_scene_fixtures::meshes_as_roots;
    use slint::Model;
    use slint::platform::WindowEvent;

    use super::*;
    use crate::language::{BundledLanguage, LanguagePreference};
    use crate::test_support::catalog_entries::printer_entry;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::{MainWindow, WorkspaceSection};

    fn listed_printer() -> PrinterId {
        PrinterId::new("listed-printer").expect("valid printer id")
    }

    /// Builds an `AppState` that reads and writes no file. The preferences
    /// start as `snapshot`; the catalog holds 1 printer.
    fn test_app(snapshot: PreferencesSnapshot) -> Rc<AppState> {
        let prefs = PreferencesBus::new(snapshot);
        let catalog = Rc::new(RefCell::new(Rc::new(ProfileCatalog::from_entries(
            vec![printer_entry(
                listed_printer(),
                "Alpha",
                [120.0, 68.0, 150.0],
            )],
            Vec::new(),
        ))));
        let default_envelope = DefaultEnvelope::new(&prefs, catalog.clone());
        Rc::new(AppState {
            about_window: RefCell::new(None),
            preferences_window: RefCell::new(None),
            main_windows: RefCell::new(HashMap::new()),
            next_window_id: Cell::new(0),
            // These tests save no settings, so the settings listener does
            // nothing.
            _settings_sub: prefs.subscribe(|_, _| {}),
            ui_language: OnceCell::new(),
            prefs,
            catalog,
            default_envelope,
            clipboard: RefCell::new(None),
            device_loss: DeviceLoss::default(),
            // These tests autosave nothing.
            autosave: None,
            _settings_saver: SettingsSaverJoin { join: None },
        })
    }

    /// Creates a main window and its controller as `spawn_main_window` does.
    fn new_window(app: &AppState) -> (MainWindow, Rc<PendingLoad>, Rc<ViewportController>) {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let pending_load = Rc::new(PendingLoad::new());
        let controller = Rc::new(ViewportController::new(
            &window,
            SceneDocument::new(),
            pending_load.clone(),
            app.device_loss.clone(),
            &app.prefs,
        ));
        controller.register_document_listener();
        show_envelope(&controller, &app.default_envelope);
        (window, pending_load, controller)
    }

    /// Adds a main window to `app` and returns its id. The window gets its
    /// drivers and its preference listener, but no other callback, and none of
    /// the parts that need a GPU surface.
    fn add_window(app: &Rc<AppState>) -> u64 {
        let (window, pending_load, controller) = new_window(app);
        let drivers = app.install_window_drivers(&window, &controller);
        let subscription = subscribe_window_preferences(
            &app.prefs,
            &window,
            controller.clone(),
            app.default_envelope.clone(),
        );
        let id = app.next_window_id.get();
        app.next_window_id.set(id + 1);
        app.main_windows.borrow_mut().insert(
            id,
            WiredMainWindow {
                window,
                controller,
                pending_load,
                autosaver: None,
                _subscription: subscription,
                _drivers: drivers,
            },
        );
        id
    }

    fn window_of(app: &AppState, id: u64) -> MainWindow {
        app.main_windows.borrow()[&id].window.clone_strong()
    }

    fn controller_of(app: &AppState, id: u64) -> Rc<ViewportController> {
        app.main_windows.borrow()[&id].controller.clone()
    }

    fn load_cube(controller: &ViewportController) {
        controller.install_graph_for_test(meshes_as_roots([unit_cube_mesh()]));
    }

    /// Loads a cube into window `id`, selects it and opens the Transform
    /// section, where copy and paste work.
    fn load_selected_cube(app: &AppState, id: u64) {
        let controller = controller_of(app, id);
        load_cube(&controller);
        let leaf = controller
            .document()
            .with_scene(|scene| scene.leaf_id_at_index(0))
            .flatten()
            .expect("the cube is a leaf");
        controller
            .document()
            .set_selection(Selection::One(LeafId(leaf)));
        controller.set_section(WorkspaceSection::Transform);
    }

    fn cube_clipboard() -> Clipboard {
        Clipboard::new(SceneFragment::from_leaf_snapshot(
            Arc::new(unit_cube_mesh()),
            Transform3D::Identity,
        ))
    }

    #[test]
    fn the_drivers_list_the_default_printer_in_the_slice_panel() {
        let listed = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot {
                default_printer: Some(listed_printer()),
                ..PreferencesSnapshot::for_tests()
            });
            let (window, _pending_load, controller) = new_window(&app);
            let _drivers = app.install_window_drivers(&window, &controller);
            (
                window.get_slice_printer_options().row_count(),
                window.get_slice_printer_current_index(),
            )
        });
        assert_eq!(listed, (1, 0));
    }

    #[test]
    fn a_slice_press_without_a_printer_says_why() {
        let headings = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let (window, _pending_load, controller) = new_window(&app);
            let _drivers = app.install_window_drivers(&window, &controller);
            window.invoke_slice_requested();
            controller.dialogs().error_headings_for_test()
        });
        assert_eq!(headings, ["Cannot slice"]);
    }

    #[test]
    fn a_repair_press_with_a_scene_starts_a_repair() {
        let (before, after) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let (window, _pending_load, controller) = new_window(&app);
            let _drivers = app.install_window_drivers(&window, &controller);
            load_cube(&controller);
            controller.set_section(WorkspaceSection::Transform);
            let before = window.get_repairing_in_progress();
            window.invoke_repair_requested();
            (before, window.get_repairing_in_progress())
        });
        assert_eq!((before, after), (false, true));
    }

    #[test]
    fn a_support_press_with_a_scene_starts_the_generation() {
        let (before, after) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let (window, _pending_load, controller) = new_window(&app);
            let _drivers = app.install_window_drivers(&window, &controller);
            load_cube(&controller);
            let before = window.get_support_gen_in_progress();
            window.invoke_support_gen_requested();
            (before, window.get_support_gen_in_progress())
        });
        assert_eq!((before, after), (false, true));
    }

    #[test]
    fn support_options_reach_the_tab_clamped_and_presets_apply() {
        let (density, clamped, preset_index, sizing, names) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let (window, _pending_load, controller) = new_window(&app);
            let _drivers = app.install_window_drivers(&window, &controller);
            window.invoke_support_option_changed(1, 20.0);
            let density = window.get_support_density();
            window.invoke_support_option_changed(1, 999.0);
            let clamped = window.get_support_density();
            // The built-in "Detail" preset is the second one.
            window.invoke_support_preset_picked(1);
            (
                density,
                clamped,
                window.get_support_preset_index(),
                window.get_support_sizing_index(),
                window.get_support_preset_names().row_count(),
            )
        });
        assert_eq!(density, 20.0);
        assert_eq!(
            clamped, 30.0,
            "DragonFruit caps the area per support at 30 mm2"
        );
        assert_eq!((preset_index, sizing), (1, 0));
        assert!(names >= 3, "the three built-in presets are listed");
    }

    #[test]
    fn a_dragonfruit_support_press_starts_a_cancelable_run() {
        let (before, held, warned, after, cancelable) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let (window, _pending_load, controller) = new_window(&app);
            let _drivers = app.install_window_drivers(&window, &controller);
            load_cube(&controller);
            let before = window.get_support_gen_in_progress();
            // The cube rests on the plate, in the raft: the press waits for
            // the raft warning's answer (#131).
            window.invoke_df_support_requested();
            let held = window.get_support_gen_in_progress();
            let warned = !window.get_support_raft_clash_message().is_empty();
            window.invoke_support_raft_ignore_requested();
            let after = window.get_support_gen_in_progress();
            let cancelable = window.get_support_gen_cancelable();
            window.invoke_support_gen_cancel_requested();
            (before, held, warned, after, cancelable)
        });
        assert_eq!(
            (before, held, warned, after, cancelable),
            (false, false, true, true, true)
        );
    }

    #[test]
    fn the_copy_keystroke_copies_the_selection_of_its_own_window() {
        let copied = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let empty = add_window(&app);
            let loaded = add_window(&app);
            load_selected_cube(&app, loaded);
            app.wire_clipboard(&window_of(&app, empty), empty);
            app.wire_clipboard(&window_of(&app, loaded), loaded);

            window_of(&app, empty).invoke_copy_pressed();
            let from_empty = app.clipboard.borrow().is_some();
            window_of(&app, loaded).invoke_copy_pressed();
            (from_empty, app.clipboard.borrow().is_some())
        });
        assert_eq!(copied, (false, true));
    }

    #[test]
    fn the_menu_copy_copies_the_selection_of_its_own_window() {
        let copied = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let empty = add_window(&app);
            let loaded = add_window(&app);
            load_selected_cube(&app, loaded);
            app.wire_clipboard(&window_of(&app, empty), empty);
            app.wire_clipboard(&window_of(&app, loaded), loaded);

            window_of(&app, empty).invoke_context_copy_pressed();
            let from_empty = app.clipboard.borrow().is_some();
            window_of(&app, loaded).invoke_context_copy_pressed();
            (from_empty, app.clipboard.borrow().is_some())
        });
        assert_eq!(copied, (false, true));
    }

    /// Returns the number of objects in a window with 1 cube after a paste
    /// through `press`.
    fn objects_after_paste(press: fn(&MainWindow)) -> usize {
        on_fresh_slint_thread(move || {
            let app = test_app(PreferencesSnapshot::for_tests());
            let id = add_window(&app);
            load_selected_cube(&app, id);
            let controller = controller_of(&app, id);
            controller.set_viewport_for_test(800.0, 600.0);
            controller.frame_camera_for_test();
            *app.clipboard.borrow_mut() = Some(cube_clipboard());
            let window = window_of(&app, id);
            app.wire_clipboard(&window, id);
            press(&window);
            controller.reachable_leaf_count_for_test()
        })
    }

    #[test]
    fn the_paste_keystroke_pastes_into_its_window() {
        assert_eq!(
            objects_after_paste(|window| window.invoke_paste_pressed()),
            2
        );
    }

    #[test]
    fn the_menu_paste_pastes_into_its_window() {
        assert_eq!(
            objects_after_paste(|window| window.invoke_context_paste_pressed(400.0, 300.0)),
            2
        );
    }

    /// Returns each call of the window's `show-context-menu` after a right
    /// press at `down` and a release at `up`, in the Transform section with a
    /// filled clipboard and nothing under the cursor.
    fn menus_after_right_button(
        down: (f32, f32),
        up: (f32, f32),
    ) -> Vec<(f32, f32, bool, bool, bool)> {
        on_fresh_slint_thread(move || {
            let app = test_app(PreferencesSnapshot::for_tests());
            let id = add_window(&app);
            let window = window_of(&app, id);
            let controller = controller_of(&app, id);
            controller.set_section(WorkspaceSection::Transform);
            *app.clipboard.borrow_mut() = Some(cube_clipboard());
            let shown = Rc::new(RefCell::new(Vec::new()));
            window.on_show_context_menu({
                let shown = shown.clone();
                move |x, y, copy, paste, delete| {
                    shown.borrow_mut().push((x, y, copy, paste, delete));
                }
            });
            app.wire_context_menu(&window, &controller, id);

            window.invoke_viewport_right_pointer_down(down.0, down.1);
            window.invoke_viewport_right_pointer_up(up.0, up.1);
            shown.take()
        })
    }

    #[test]
    fn a_right_click_shows_the_menu_at_the_press_position() {
        assert_eq!(
            menus_after_right_button((100.0, 80.0), (101.0, 81.0)),
            [(100.0, 80.0, false, true, false)]
        );
    }

    #[test]
    fn a_right_drag_shows_no_menu() {
        assert!(menus_after_right_button((100.0, 80.0), (300.0, 240.0)).is_empty());
    }

    #[test]
    fn a_close_request_releases_only_its_window() {
        let open = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let closed = add_window(&app);
            let _kept = add_window(&app);
            let window = window_of(&app, closed);
            app.wire_window_events(&window, closed);

            window.window().dispatch_event(WindowEvent::CloseRequested);
            app.main_windows
                .borrow()
                .keys()
                .copied()
                .collect::<Vec<u64>>()
        });
        assert_eq!(open, [1]);
    }

    #[test]
    fn a_hovered_file_shows_the_drop_overlay() {
        let hovering = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let id = add_window(&app);
            let window = window_of(&app, id);
            app.on_file_drag(&window, id, FileDrag::Hovered);
            window.get_drop_hovering()
        });
        assert!(hovering);
    }

    #[test]
    fn a_cancelled_hover_hides_the_drop_overlay() {
        let hovering = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let id = add_window(&app);
            let window = window_of(&app, id);
            app.on_file_drag(&window, id, FileDrag::Hovered);
            app.on_file_drag(&window, id, FileDrag::HoverCancelled);
            window.get_drop_hovering()
        });
        assert!(!hovering);
    }

    #[test]
    fn a_dropped_file_hides_the_overlay_and_opens_in_its_window() {
        let (hovering, queued) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot::for_tests());
            let id = add_window(&app);
            let window = window_of(&app, id);
            app.on_file_drag(&window, id, FileDrag::Hovered);
            app.on_file_drag(
                &window,
                id,
                FileDrag::Dropped(PathBuf::from("/models/cube.stl")),
            );
            // The window has no GPU device yet, so the path waits on its
            // pending load.
            let queued = app.main_windows.borrow()[&id].pending_load.take();
            (window.get_drop_hovering(), queued)
        });
        assert!(!hovering);
        assert_eq!(queued, Some(PathBuf::from("/models/cube.stl")));
    }

    fn bundled(code: &str) -> LanguagePreference {
        LanguagePreference::Bundled(BundledLanguage::from_code(code).expect("bundled language"))
    }

    /// Returns the default heading of an error message in a new window, in the
    /// language in force.
    fn error_heading() -> String {
        MainWindow::new()
            .expect("the headless platform creates components")
            .get_message_default_heading()
            .to_string()
    }

    /// Returns the heading of an error dialog created after the first window of
    /// an app whose stored language is `code`.
    fn heading_after_first_window(code: &'static str) -> String {
        on_fresh_slint_thread(move || {
            let app = test_app(PreferencesSnapshot {
                language: bundled(code),
                ..PreferencesSnapshot::for_tests()
            });
            let id = add_window(&app);
            app.on_first_window(&window_of(&app, id));
            error_heading()
        })
    }

    /// A catalog where the listed printer has the plate `plate` and the
    /// manufacturer `name`, as a saved profile leaves it.
    fn saved_catalog(name: &str, plate: [f64; 3]) -> Rc<ProfileCatalog> {
        Rc::new(ProfileCatalog::from_entries(
            vec![printer_entry(listed_printer(), name, plate)],
            Vec::new(),
        ))
    }

    /// Loads 2 cubes into `controller` and deletes 1, so the window has a
    /// scene and an undo step.
    fn load_edited_scene(controller: &ViewportController) {
        controller.install_graph_for_test(meshes_as_roots([unit_cube_mesh(), unit_cube_mesh()]));
        let leaf = controller
            .document()
            .with_scene(|scene| scene.leaf_id_at_index(0))
            .flatten()
            .expect("the cube is a leaf");
        controller
            .document()
            .set_selection(Selection::One(LeafId(leaf)));
        assert!(
            controller
                .document()
                .delete_selected_leaf()
                .expect("the delete applies")
        );
    }

    /// What window `id` shows of the default plate and its scene.
    #[derive(Debug, PartialEq)]
    struct PlateView {
        build_volume: BuildVolume,
        printer_name: Option<String>,
        warning_mask: u32,
        objects: usize,
        can_undo: bool,
    }

    fn plate_view(app: &AppState, id: u64) -> PlateView {
        let controller = controller_of(app, id);
        PlateView {
            build_volume: controller.build_volume(),
            printer_name: controller.plate_printer_name_for_test(),
            warning_mask: controller.envelope_warning_mask_for_test(),
            objects: controller.reachable_leaf_count_for_test(),
            can_undo: controller.document().can_undo(),
        }
    }

    #[test]
    fn saving_the_default_printer_profile_redraws_the_plate_in_every_window() {
        let (before, after) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot {
                default_printer: Some(listed_printer()),
                ..PreferencesSnapshot::for_tests()
            });
            let ids = [add_window(&app), add_window(&app)];
            for id in ids {
                load_edited_scene(&controller_of(&app, id));
            }
            let before = ids.map(|id| plate_view(&app, id));
            // The profile is saved with a plate smaller than the 1 mm cube,
            // and the library rebuild says so on the bus.
            *app.catalog.borrow_mut() = saved_catalog("Beta", [0.5, 0.5, 0.5]);
            app.prefs.update(|_| PreferencesChange::Profiles);
            (before, ids.map(|id| plate_view(&app, id)))
        });
        for (before, after) in before.iter().zip(&after) {
            assert_eq!(before.build_volume, BuildVolume::new(120.0, 68.0, 150.0));
            assert_eq!(before.printer_name.as_deref(), Some("Alpha"));
            assert_eq!(after.build_volume, BuildVolume::new(0.5, 0.5, 0.5));
            assert_eq!(after.printer_name.as_deref(), Some("Beta"));
            // The cube now crosses the side walls too.
            assert_ne!(after.warning_mask, before.warning_mask);
            assert_eq!(
                after.warning_mask & before.warning_mask,
                before.warning_mask
            );
            // The scene and its undo step stay.
            assert_eq!((after.objects, after.can_undo), (1, true));
            assert_eq!((before.objects, before.can_undo), (1, true));
        }
    }

    #[test]
    fn saving_another_printer_profile_keeps_the_plate() {
        let (before, after) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot {
                default_printer: Some(listed_printer()),
                ..PreferencesSnapshot::for_tests()
            });
            let id = add_window(&app);
            load_edited_scene(&controller_of(&app, id));
            let before = plate_view(&app, id);
            let mut entries = app.catalog.borrow().printers().to_vec();
            entries.push(printer_entry(
                PrinterId::new("another-printer").expect("valid printer id"),
                "Gamma",
                [10.0, 10.0, 10.0],
            ));
            *app.catalog.borrow_mut() = Rc::new(ProfileCatalog::from_entries(entries, Vec::new()));
            app.prefs.update(|_| PreferencesChange::Profiles);
            (before, plate_view(&app, id))
        });
        assert_eq!(after, before);
    }

    #[test]
    fn the_first_window_puts_the_stored_language_in_force() {
        let spanish = heading_after_first_window("es");
        let german = heading_after_first_window("de");
        assert_ne!(spanish, "An error occurred");
        assert_ne!(german, "An error occurred");
        assert_ne!(spanish, german);
    }

    #[test]
    fn a_language_change_after_the_first_window_is_put_in_force() {
        let (stored, changed) = on_fresh_slint_thread(|| {
            let app = test_app(PreferencesSnapshot {
                language: bundled("es"),
                ..PreferencesSnapshot::for_tests()
            });
            let id = add_window(&app);
            app.on_first_window(&window_of(&app, id));
            let stored = error_heading();
            app.prefs.update(|snap| {
                snap.language = bundled("de");
                PreferencesChange::Language
            });
            (stored, error_heading())
        });
        assert_ne!(changed, stored);
        assert_eq!(changed, heading_after_first_window("de"));
    }
}

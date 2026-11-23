/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Preferences (#670): a card over the main window, like Printer & resin,
//! filled through the `Prefs` global of that window. Each change goes through
//! the preferences bus, so it is saved at once and reaches every window. The
//! printers and resins themselves are picked and edited in Printer & resin:
//! this card lists the person's own profiles and sends them there.

mod network_printers;

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};

use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use sc_print_primitives::MaterialId;
use sc_print_profiles::{
    ProfileSource, delete_material_extension_from_library, delete_material_profile_from_library,
    printer_profile_path_in_library, user_library_dir,
};

use crate::aa_level::{aa_combo_index, aa_level_from_combo_index};
use crate::app_settings::AppSettings;
use crate::frame_rate::FrameRateCap;
use crate::i18n::ctr;
use crate::language::LanguagePreference;
use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::profile_catalog::{ProfileCatalog, refresh_catalog};
use crate::profile_transfer::{
    ExportScope, ImportOutcome, format_collision_report, overwrite_confirmed, run_export,
    run_import,
};
use crate::viewport::{ProbeResult, ProbeUpdate, ViewportController};
use crate::{MainWindow, MessageAnswer, Prefs, PrefsHit, PrefsProfile};

/// The pages of the card, in the order of its list.
pub(crate) mod page {
    pub(crate) const GENERAL: i32 = 0;
    pub(crate) const LOOK: i32 = 1;
    pub(crate) const MOUSE: i32 = 2;
    pub(crate) const PRINTERS: i32 = 3;
    pub(crate) const NETWORK: i32 = 4;
    pub(crate) const SLICING: i32 = 5;
    pub(crate) const PRIVACY: i32 = 6;
}

/// Where Printer & resin opens when the card sends the person there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetupStart {
    /// The picker, on the printer and resin in use.
    Picker,
    /// The form of a new printer.
    AddPrinter,
    /// The form of the printer at this place of the catalog.
    EditPrinter(usize),
    /// The form of the resin at this place of the catalog.
    EditResin(usize),
}

/// What the card asks of the app: opening Printer & resin, and importing a
/// printer's settings (File > Import printer settings).
pub(crate) struct CardActions {
    pub(crate) open_setup: Rc<dyn Fn(SetupStart)>,
    pub(crate) import_settings: Rc<dyn Fn()>,
}

/// Keeps the card of a window wired: its bus subscription and its network
/// page. Dropping it leaves the card as it is, no longer following.
pub(crate) struct WiredPreferences {
    _subscription: Subscription,
    _network_printers: network_printers::NetworkPrintersPage,
}

/// Opens Preferences over `ui`, on the page it was last on. The performance
/// test draws in `viewport`, the 3D view of `ui`.
pub(crate) fn open_preferences(
    ui: &MainWindow,
    bus: &Rc<PreferencesBus>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    viewport: Weak<ViewportController>,
    actions: CardActions,
) -> WiredPreferences {
    let prefs = ui.global::<Prefs>();
    let snapshot = bus.snapshot();
    apply_preferences(&prefs, &snapshot);
    show_printing(&prefs, &snapshot, &catalog.borrow());
    prefs.set_search(SharedString::new());
    prefs.set_hits(ModelRc::default());
    prefs.set_diagnostic_log_error(SharedString::new());
    prefs.set_log_question_open(false);

    let weak = ui.as_weak();
    let actions = Rc::new(actions);
    wire_general(&prefs, bus, &weak);
    wire_look(&prefs, bus, &weak, viewport);
    wire_printers(&prefs, bus, &catalog, &weak, &actions);
    wire_slicing_and_privacy(&prefs, bus, &weak);

    prefs.on_search_edited({
        let weak = weak.clone();
        move |text| {
            if let Some(ui) = weak.upgrade() {
                let hits = search(&text);
                ui.global::<Prefs>()
                    .set_hits(ModelRc::new(VecModel::from(hits)));
            }
        }
    });
    prefs.on_restore_defaults({
        let bus = bus.clone();
        move |page| restore_defaults(&bus, page)
    });
    prefs.on_close_requested({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Prefs>().set_open(false);
            }
        }
    });

    let subscription = bus.subscribe({
        let weak = weak.clone();
        let catalog = catalog.clone();
        move |snap, change| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let prefs = ui.global::<Prefs>();
            apply_preferences(&prefs, snap);
            if matches!(
                change,
                PreferencesChange::DefaultPrinter
                    | PreferencesChange::DefaultResin
                    | PreferencesChange::Profiles
                    | PreferencesChange::Language
            ) {
                show_printing(&prefs, snap, &catalog.borrow());
            }
            if matches!(change, PreferencesChange::Language) && !prefs.get_search().is_empty() {
                prefs.set_hits(ModelRc::new(VecModel::from(search(&prefs.get_search()))));
            }
        }
    });
    let network_printers = network_printers::wire(ui, bus, catalog);

    prefs.set_open(true);
    crate::breadcrumbs::note("window: preferences");
    WiredPreferences {
        _subscription: subscription,
        _network_printers: network_printers,
    }
}

/// Pushes the plain settings of `snap` into the card.
pub(crate) fn apply_preferences(prefs: &Prefs, snap: &PreferencesSnapshot) {
    prefs.set_theme(snap.theme);
    // The language list again, so "System" follows the language in force.
    prefs.set_language_options(LanguagePreference::display_options());
    prefs.set_current_language_index(snap.language.combo_index());
    prefs.set_current_aa_index(aa_combo_index(snap.aa_level));
    prefs.set_prescan_supports(snap.prescan_supports);
    prefs.set_smooth_models(snap.smooth_models);
    prefs.set_controls_options(ModelRc::new(VecModel::from(
        crate::controls_layout::ControlsLayout::ALL
            .iter()
            .map(|l| SharedString::from(l.label()))
            .collect::<Vec<_>>(),
    )));
    prefs.set_current_controls_index(snap.controls_layout.combo_index());
    prefs.set_controls_summary(snap.controls_layout.summary().into());
    prefs.set_show_start_dialog(snap.show_start_dialog);
    prefs.set_repair_on_import(snap.repair_on_import);
    prefs.set_reload_on_change(snap.reload_on_change);
    if prefs.get_export_name_template().as_str() != snap.export_name_template {
        prefs.set_export_name_template(snap.export_name_template.as_str().into());
    }
    prefs.set_export_name_preview(
        crate::file_name_template::preview(&snap.export_name_template).into(),
    );
    prefs.set_current_frame_rate_index(snap.frame_rate.combo_index());
    prefs.set_current_gizmo_style_index(snap.gizmo_style.combo_index());
    prefs.set_current_gizmo_size_index(crate::gizmo::look::size_combo_index(snap.gizmo_size));
    prefs.set_diagnostic_log(snap.diagnostic_log);
    prefs.set_silenced_message_count(snap.silenced_messages.len() as i32);
}

fn wire_general(prefs: &Prefs, bus: &Rc<PreferencesBus>, _weak: &slint::Weak<MainWindow>) {
    prefs.on_language_changed({
        let bus = bus.clone();
        move |index| {
            let language = LanguagePreference::from_combo_index(index);
            bus.update(move |snap| {
                snap.language = language;
                PreferencesChange::Language
            });
        }
    });
    prefs.on_show_start_dialog_changed({
        let bus = bus.clone();
        move |on| {
            bus.update(move |snap| {
                snap.show_start_dialog = on;
                PreferencesChange::StartDialog
            });
        }
    });
    prefs.on_repair_on_import_changed({
        let bus = bus.clone();
        move |on| {
            bus.update(move |snap| {
                snap.repair_on_import = on;
                PreferencesChange::RepairOnImport
            });
        }
    });
    prefs.on_reload_on_change_changed({
        let bus = bus.clone();
        move |on| {
            bus.update(move |snap| {
                snap.reload_on_change = on;
                PreferencesChange::ReloadOnChange
            });
        }
    });
    prefs.on_export_name_template_edited({
        let bus = bus.clone();
        move |text| {
            let template = text.to_string();
            bus.update(move |snap| {
                snap.export_name_template = template;
                PreferencesChange::ExportNameTemplate
            });
        }
    });
    prefs.on_show_all_messages_again({
        let bus = bus.clone();
        move || crate::error_dialog::show_all_messages_again(&bus)
    });
    prefs.on_controls_selected({
        let bus = bus.clone();
        move |index| {
            bus.update(move |snap| {
                snap.controls_layout =
                    crate::controls_layout::ControlsLayout::from_combo_index(index);
                PreferencesChange::Controls
            });
        }
    });
}

fn wire_look(
    prefs: &Prefs,
    bus: &Rc<PreferencesBus>,
    weak: &slint::Weak<MainWindow>,
    viewport: Weak<ViewportController>,
) {
    prefs.on_theme_changed({
        let bus = bus.clone();
        move |theme| {
            bus.update(move |snap| {
                snap.theme = theme;
                PreferencesChange::Theme
            });
        }
    });
    prefs.on_smooth_models_changed({
        let bus = bus.clone();
        move |on| {
            bus.update(move |snap| {
                snap.smooth_models = on;
                PreferencesChange::SmoothModels
            });
        }
    });
    prefs.on_frame_rate_changed({
        let bus = bus.clone();
        move |index| {
            bus.update(move |snap| {
                snap.frame_rate = FrameRateCap::from_combo_index(index);
                PreferencesChange::FrameRate
            });
        }
    });
    prefs.on_gizmo_style_changed({
        let bus = bus.clone();
        move |index| {
            bus.update(move |snap| {
                snap.gizmo_style = crate::gizmo::look::GizmoStyle::from_combo_index(index);
                PreferencesChange::Gizmo
            });
        }
    });
    prefs.on_gizmo_size_changed({
        let bus = bus.clone();
        move |index| {
            bus.update(move |snap| {
                snap.gizmo_size = crate::gizmo::look::size_from_combo_index(index);
                PreferencesChange::Gizmo
            });
        }
    });
    prefs.on_use_recommended_frame_rate({
        let bus = bus.clone();
        let weak = weak.clone();
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let prefs = ui.global::<Prefs>();
            let Ok(fps) = u32::try_from(prefs.get_probe_recommended_fps()) else {
                return;
            };
            if let Some(cap) = FrameRateCap::fixed(fps) {
                prefs.set_probe_recommended_fps(0);
                bus.update(move |snap| {
                    snap.frame_rate = cap;
                    PreferencesChange::FrameRate
                });
            }
        }
    });
    prefs.on_test_performance({
        let weak = weak.clone();
        move || {
            let Some(viewport) = viewport.upgrade() else {
                return;
            };
            let weak = weak.clone();
            viewport.start_performance_probe(Rc::new(move |update| {
                if let Some(ui) = weak.upgrade() {
                    show_probe_update(&ui.global::<Prefs>(), update);
                }
            }));
        }
    });
}

fn show_probe_update(prefs: &Prefs, update: ProbeUpdate) {
    match update {
        ProbeUpdate::Progress(progress) => {
            prefs.set_probe_running(true);
            prefs.set_probe_progress(progress);
            prefs.set_probe_recommended_fps(0);
            prefs.set_probe_message(ctr!(
                "AppearancePreferences",
                "Testing… watch the 3D view; it takes about 10 seconds."
            ));
        }
        ProbeUpdate::Finished(result) => {
            prefs.set_probe_running(false);
            prefs.set_probe_recommended_fps(i32::try_from(result.recommended_fps).unwrap_or(0));
            prefs.set_probe_message(probe_result_text(&result));
        }
        ProbeUpdate::Failed => {
            prefs.set_probe_running(false);
            prefs.set_probe_recommended_fps(0);
            prefs.set_probe_message(ctr!(
                "AppearancePreferences",
                "The test could not run. Make sure the 3D view is visible and try again."
            ));
        }
    }
}

fn probe_result_text(result: &ProbeResult) -> SharedString {
    let sustained = result.sustained_fps();
    match result.refresh_hz {
        Some(hz) => ctr!(
            "AppearancePreferences",
            "Your computer draws a heavy scene at about {} FPS and your monitor shows {} Hz. Recommended: {} FPS.",
            sustained,
            hz,
            result.recommended_fps
        ),
        None => ctr!(
            "AppearancePreferences",
            "Your computer draws a heavy scene at about {} FPS. Recommended: {} FPS.",
            sustained,
            result.recommended_fps
        ),
    }
}

fn wire_slicing_and_privacy(
    prefs: &Prefs,
    bus: &Rc<PreferencesBus>,
    weak: &slint::Weak<MainWindow>,
) {
    prefs.on_aa_level_changed({
        let bus = bus.clone();
        move |index| {
            let level = aa_level_from_combo_index(index);
            bus.update(move |snap| {
                snap.aa_level = level;
                PreferencesChange::AaLevel
            });
        }
    });
    prefs.on_prescan_supports_changed({
        let bus = bus.clone();
        move |on| {
            bus.update(move |snap| {
                snap.prescan_supports = on;
                PreferencesChange::PrescanSupports
            });
        }
    });
    prefs.on_diagnostic_log_changed({
        let bus = bus.clone();
        let weak = weak.clone();
        move |on| {
            if let Some(ui) = weak.upgrade() {
                diagnostic_log_toggled(&ui.global::<Prefs>(), &bus, on);
            }
        }
    });
    prefs.on_log_question_answered({
        let bus = bus.clone();
        let weak = weak.clone();
        move |answer| {
            if let Some(ui) = weak.upgrade() {
                log_question_answered(&ui.global::<Prefs>(), &bus, answer);
            }
        }
    });
    prefs.on_open_logs_folder(|| {
        if let Some(dir) = crate::breadcrumbs::logs_dir() {
            crate::app_state::open_in_browser(&dir.to_string_lossy());
        }
    });
}

/// The diagnostic log was ticked (`on`) or unticked in Privacy. Ticking only
/// asks: the box stays clear and the log off until the person confirms (#74).
/// Unticking stops it at once.
fn diagnostic_log_toggled(prefs: &Prefs, bus: &PreferencesBus, on: bool) {
    prefs.set_diagnostic_log_error(SharedString::new());
    if on {
        prefs.set_diagnostic_log(false);
        prefs.set_log_question_serial(prefs.get_log_question_serial().wrapping_add(1));
        prefs.set_log_question_open(true);
    } else {
        crate::breadcrumbs::note("diagnostic log: off");
        // Off can't fail in a way worth telling: the log stops either way.
        let _ = crate::breadcrumbs::set_enabled(false);
        bus.update(|snap| {
            snap.diagnostic_log = false;
            PreferencesChange::DiagnosticLog
        });
    }
}

/// Closes the question before the diagnostic log. Turn on starts the log
/// here, and the box shows it on only once `session.log` has started; when it
/// can't, the box stays clear and the page says why, never failing quietly.
fn log_question_answered(prefs: &Prefs, bus: &PreferencesBus, answer: MessageAnswer) {
    prefs.set_log_question_open(false);
    if answer != MessageAnswer::Confirm {
        return;
    }
    match crate::breadcrumbs::set_enabled(true) {
        Ok(()) => {
            crate::breadcrumbs::note("diagnostic log: on");
            bus.update(|snap| {
                snap.diagnostic_log = true;
                PreferencesChange::DiagnosticLog
            });
        }
        Err(err) => {
            crate::breadcrumbs::note(&format!("diagnostic log: could not start: {err}"));
            prefs.set_diagnostic_log(false);
            prefs.set_diagnostic_log_error(ctr!(
                "PrivacyPreferences",
                "The diagnostic log could not start: {}",
                err
            ));
        }
    }
}

/// Puts the settings of `page` back to what Breadslicer ships with. Each one
/// that changes goes through the bus, as if picked by hand.
fn restore_defaults(bus: &PreferencesBus, page: i32) {
    let shipped = AppSettings::default;
    let now = bus.snapshot();
    match page {
        page::GENERAL => {
            if now.language != shipped().language() {
                bus.update(move |snap| {
                    snap.language = shipped().language();
                    PreferencesChange::Language
                });
            }
            if now.repair_on_import != shipped().repair_on_import() {
                bus.update(move |snap| {
                    snap.repair_on_import = shipped().repair_on_import();
                    PreferencesChange::RepairOnImport
                });
            }
            if now.reload_on_change != shipped().reload_on_change() {
                bus.update(move |snap| {
                    snap.reload_on_change = shipped().reload_on_change();
                    PreferencesChange::ReloadOnChange
                });
            }
            if now.show_start_dialog != shipped().show_start_dialog() {
                bus.update(move |snap| {
                    snap.show_start_dialog = shipped().show_start_dialog();
                    PreferencesChange::StartDialog
                });
            }
            if now.export_name_template != shipped().export_name_template() {
                bus.update(move |snap| {
                    snap.export_name_template = shipped().export_name_template();
                    PreferencesChange::ExportNameTemplate
                });
            }
        }
        page::LOOK => {
            if now.theme != shipped().theme() {
                bus.update(move |snap| {
                    snap.theme = shipped().theme();
                    PreferencesChange::Theme
                });
            }
            if now.smooth_models != shipped().smooth_models() {
                bus.update(move |snap| {
                    snap.smooth_models = shipped().smooth_models();
                    PreferencesChange::SmoothModels
                });
            }
            if now.frame_rate != shipped().frame_rate() {
                bus.update(move |snap| {
                    snap.frame_rate = shipped().frame_rate();
                    PreferencesChange::FrameRate
                });
            }
            if now.gizmo_style != shipped().gizmo_style()
                || now.gizmo_size != shipped().gizmo_size()
            {
                bus.update(move |snap| {
                    snap.gizmo_style = shipped().gizmo_style();
                    snap.gizmo_size = shipped().gizmo_size();
                    PreferencesChange::Gizmo
                });
            }
        }
        page::MOUSE => {
            if now.controls_layout != shipped().controls_layout() {
                bus.update(move |snap| {
                    snap.controls_layout = shipped().controls_layout();
                    PreferencesChange::Controls
                });
            }
        }
        page::SLICING => {
            if now.aa_level != shipped().aa_level() {
                bus.update(move |snap| {
                    snap.aa_level = shipped().aa_level();
                    PreferencesChange::AaLevel
                });
            }
            if now.prescan_supports != shipped().prescan_supports() {
                bus.update(move |snap| {
                    snap.prescan_supports = shipped().prescan_supports();
                    PreferencesChange::PrescanSupports
                });
            }
        }
        _ => {}
    }
}

/// The settings the search box finds: each one's name, as its row says it,
/// and its page.
fn searchable() -> Vec<(SharedString, i32)> {
    use page::*;
    vec![
        (ctr!("AppearancePreferences", "Language"), GENERAL),
        (ctr!("PreferencesLayer", "Start card"), GENERAL),
        (ctr!("PreferencesLayer", "Repair on import"), GENERAL),
        (ctr!("PreferencesLayer", "Exported file name"), GENERAL),
        (ctr!("PreferencesLayer", "Messages you silenced"), GENERAL),
        (ctr!("AppearancePreferences", "Theme"), LOOK),
        (
            ctr!("AppearancePreferences", "Smooth shading on models"),
            LOOK,
        ),
        (ctr!("AppearancePreferences", "Frame rate"), LOOK),
        (ctr!("AppearancePreferences", "Test performance"), LOOK),
        (ctr!("AppearancePreferences", "Gizmo style"), LOOK),
        (ctr!("AppearancePreferences", "Gizmo size"), LOOK),
        (ctr!("AppearancePreferences", "Mouse controls"), MOUSE),
        (ctr!("PreferencesLayer", "Every shortcut"), MOUSE),
        (
            ctr!("PreferencesLayer", "Printer and resin in use"),
            PRINTERS,
        ),
        (ctr!("PreferencesLayer", "Your profiles"), PRINTERS),
        (ctr!("PreferencesLayer", "Add a printer by hand"), PRINTERS),
        (
            ctr!("PreferencesLayer", "Import a profiles package"),
            PRINTERS,
        ),
        (
            ctr!("PreferencesLayer", "Import printer settings"),
            PRINTERS,
        ),
        (ctr!("PreferencesLayer", "Export your profiles"), PRINTERS),
        (ctr!("PreferencesMenu", "Network printers"), NETWORK),
        (ctr!("SlicingPreferences", "Anti-aliasing"), SLICING),
        (
            ctr!("SlicingPreferences", "Prepare supports in the background"),
            SLICING,
        ),
        (
            ctr!(
                "PrivacyPreferences",
                "Save a diagnostic log until Breadslicer closes"
            ),
            PRIVACY,
        ),
        (ctr!("PrivacyPreferences", "Open logs folder"), PRIVACY),
    ]
}

fn page_name(page: i32) -> SharedString {
    match page {
        page::GENERAL => ctr!("PreferencesLayer", "General"),
        page::LOOK => ctr!("PreferencesLayer", "Look and feel"),
        page::MOUSE => ctr!("PreferencesLayer", "Mouse and keys"),
        page::PRINTERS => ctr!("PreferencesLayer", "Printers and resins"),
        page::NETWORK => ctr!("PreferencesMenu", "Network printers"),
        page::SLICING => ctr!("PreferencesMenu", "Slicing"),
        _ => ctr!("PreferencesMenu", "Privacy"),
    }
}

/// The settings whose name, or whose page's, holds every word of `text`, in
/// the language in force.
fn search(text: &str) -> Vec<PrefsHit> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    searchable()
        .into_iter()
        .filter_map(|(label, page)| {
            let name = page_name(page);
            let haystack = format!("{} {}", label.to_lowercase(), name.to_lowercase());
            words
                .iter()
                .all(|w| haystack.contains(w.as_str()))
                .then_some(PrefsHit {
                    label,
                    page_name: name,
                    page,
                })
        })
        .collect()
}

fn wire_printers(
    prefs: &Prefs,
    bus: &Rc<PreferencesBus>,
    catalog: &Rc<RefCell<Rc<ProfileCatalog>>>,
    weak: &slint::Weak<MainWindow>,
    actions: &Rc<CardActions>,
) {
    // Printer & resin opens over the card's window; the card goes.
    let to_setup = {
        let weak = weak.clone();
        let actions = actions.clone();
        move |start: SetupStart| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Prefs>().set_open(false);
            }
            (actions.open_setup)(start);
        }
    };
    prefs.on_change_in_use({
        let to_setup = to_setup.clone();
        move || to_setup(SetupStart::Picker)
    });
    prefs.on_add_printer_requested({
        let to_setup = to_setup.clone();
        move || to_setup(SetupStart::AddPrinter)
    });
    prefs.on_edit_profile({
        let to_setup = to_setup.clone();
        move |profile| {
            let Ok(at) = usize::try_from(profile.index) else {
                return;
            };
            to_setup(if profile.printer {
                SetupStart::EditPrinter(at)
            } else {
                SetupStart::EditResin(at)
            });
        }
    });
    // Profiles changed in a library: every view of them reads them again.
    let refresh: Rc<dyn Fn()> = {
        let bus = bus.clone();
        Rc::new(move || bus.update(|_| PreferencesChange::Profiles))
    };
    prefs.on_delete_profile({
        let weak = weak.clone();
        let bus = bus.clone();
        let catalog = catalog.clone();
        let refresh = refresh.clone();
        move |profile| {
            let Ok(at) = usize::try_from(profile.index) else {
                return;
            };
            let deletion = Deletion {
                owner: weak.clone(),
                bus: bus.clone(),
                catalog: catalog.clone(),
                refresh: refresh.clone(),
                at,
            };
            if profile.printer {
                spawn_delete_printer(deletion);
            } else {
                spawn_delete_resin(deletion, profile.extension);
            }
        }
    });
    prefs.on_import_package_requested({
        let weak = weak.clone();
        let catalog = catalog.clone();
        let refresh = refresh.clone();
        move || spawn_import(weak.clone(), catalog.clone(), refresh.clone())
    });
    prefs.on_import_settings_requested({
        let weak = weak.clone();
        let actions = actions.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Prefs>().set_open(false);
            }
            (actions.import_settings)();
        }
    });
    prefs.on_export_requested({
        let weak = weak.clone();
        move || spawn_export(weak.clone(), ExportScope::All)
    });
}

/// The printer and resin in use, and the person's own profiles.
fn show_printing(prefs: &Prefs, snap: &PreferencesSnapshot, catalog: &ProfileCatalog) {
    let printer = catalog
        .effective_default_printer(snap.default_printer.as_ref())
        .and_then(|id| catalog.printer_index(&id))
        .and_then(|at| catalog.printers().get(at));
    match printer {
        Some(p) => {
            prefs.set_in_use_printer(format!("{} {}", p.manufacturer, p.name).trim().into());
            let image = p.image_path.as_deref().and_then(decode_thumbnail);
            prefs.set_has_in_use_image(image.is_some());
            prefs.set_in_use_image(image.map(Image::from_rgba8).unwrap_or_default());
        }
        None => {
            prefs.set_in_use_printer(ctr!("PreferencesLayer", "No printer chosen"));
            prefs.set_has_in_use_image(false);
            prefs.set_in_use_image(Image::default());
        }
    }
    let resin = snap
        .default_resin
        .as_ref()
        .and_then(|id| catalog.material_index(id))
        .and_then(|at| catalog.materials().get(at));
    prefs.set_in_use_detail(match resin {
        Some(r) => format!("{} {}", r.brand, r.name).into(),
        None => ctr!("PreferencesLayer", "No resin chosen"),
    });
    prefs.set_printer_count(catalog.printers().len() as i32);
    prefs.set_profiles(ModelRc::new(VecModel::from(own_profiles(catalog))));
}

/// The printers and resins of the person's own library, printers first, and
/// the resins they gave settings for more printers.
fn own_profiles(catalog: &ProfileCatalog) -> Vec<PrefsProfile> {
    let printers = catalog
        .printers()
        .iter()
        .enumerate()
        .filter(|(_, p)| p.source == ProfileSource::User)
        .map(|(at, p)| PrefsProfile {
            name: format!("{} {}", p.manufacturer, p.name).into(),
            detail: ctr!("PreferencesLayer", "Printer"),
            printer: true,
            extension: false,
            index: at as i32,
        });
    let resins = catalog
        .materials()
        .iter()
        .enumerate()
        .filter(|(_, m)| m.source == ProfileSource::User)
        .map(|(at, m)| PrefsProfile {
            name: format!("{} {}", m.brand, m.name).into(),
            detail: ctr!("PreferencesLayer", "Resin"),
            printer: false,
            extension: false,
            index: at as i32,
        });
    let extensions = catalog.extensions().iter().filter_map(|e| {
        let at = catalog.material_index(&e.id)?;
        let m = catalog.materials().get(at)?;
        Some(PrefsProfile {
            name: format!("{} {}", m.brand, m.name).into(),
            detail: ctr!(
                "PreferencesLayer",
                "Resin, your settings for {n} more printer",
                "Resin, your settings for {n} more printers",
                e.printers.len()
            ),
            printer: false,
            extension: true,
            index: at as i32,
        })
    });
    printers.chain(resins).chain(extensions).collect()
}

/// Decodes a printer's thumbnail; `None` on any read or decode failure, and
/// the card keeps its blank place.
fn decode_thumbnail(path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let rgba = image::open(path).ok()?.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba.as_raw());
    Some(buffer)
}

/// A native file dialog over the card's window.
fn file_dialog_of(owner: &slint::Weak<MainWindow>) -> rfd::AsyncFileDialog {
    owner.upgrade().map_or_else(rfd::AsyncFileDialog::new, |w| {
        crate::error_dialog::file_dialog_over(w.window())
    })
}

/// A native message box over the card's window, like [`file_dialog_of`].
fn message_dialog_of(owner: &slint::Weak<MainWindow>) -> rfd::AsyncMessageDialog {
    owner
        .upgrade()
        .map_or_else(rfd::AsyncMessageDialog::new, |w| {
            crate::error_dialog::message_dialog_over(w.window())
        })
}

/// Picks a `.zip` and packs the profiles of `scope` from the user library
/// into it. Failures go to stderr; the library is only read.
fn spawn_export(owner: slint::Weak<MainWindow>, scope: ExportScope) {
    if let Err(err) = slint::spawn_local(async move {
        let Ok(library_dir) = user_library_dir() else {
            eprintln!("cannot resolve the user profile library directory");
            return;
        };
        let Some(handle) = file_dialog_of(&owner)
            .set_file_name(scope.default_file_name())
            .add_filter(ctr!("FileDialog", "Profile package").to_string(), &["zip"])
            .save_file()
            .await
        else {
            return;
        };
        if let Err(err) = run_export(&library_dir, &scope.selection(), handle.path()) {
            eprintln!("{err}");
        }
    }) {
        eprintln!("failed to spawn export task: {err}");
    }
}

/// Picks a `.zip`, installs it into the user library and, on a collision,
/// offers to overwrite. On success the catalog is read again.
fn spawn_import(
    owner: slint::Weak<MainWindow>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh: Rc<dyn Fn()>,
) {
    if let Err(err) = slint::spawn_local(async move {
        let Ok(library_dir) = user_library_dir() else {
            eprintln!("cannot resolve the user profile library directory");
            return;
        };
        let Some(handle) = file_dialog_of(&owner)
            .add_filter(ctr!("FileDialog", "Profile package").to_string(), &["zip"])
            .pick_file()
            .await
        else {
            return;
        };
        let src = handle.path().to_path_buf();
        let outcome = match run_import(&library_dir, &src, false) {
            Ok(outcome) => outcome,
            Err(err) => {
                eprintln!("{err}");
                return;
            }
        };
        match outcome {
            ImportOutcome::Installed => {
                refresh_catalog(&catalog);
                refresh();
            }
            ImportOutcome::Collisions(report) => {
                let overwrite = ctr!("PreferencesPage", "Overwrite").to_string();
                let result = message_dialog_of(&owner)
                    .set_title(ctr!("PreferencesPage", "Overwrite existing profiles?").to_string())
                    .set_description(format_collision_report(&report))
                    .set_buttons(rfd::MessageButtons::OkCancelCustom(
                        overwrite.clone(),
                        ctr!("PreferencesPage", "Cancel").to_string(),
                    ))
                    .show()
                    .await;
                if overwrite_confirmed(&result, &overwrite) {
                    match run_import(&library_dir, &src, true) {
                        Ok(_) => {
                            refresh_catalog(&catalog);
                            refresh();
                        }
                        Err(err) => eprintln!("{err}"),
                    }
                }
            }
        }
    }) {
        eprintln!("failed to spawn import task: {err}");
    }
}

/// What deleting one of the person's profiles needs.
struct Deletion {
    owner: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh: Rc<dyn Fn()>,
    /// Its place in the catalog's printers or resins.
    at: usize,
}

/// Asks, then deletes `label`'s confirmation; true once the person agreed.
async fn confirmed(owner: &slint::Weak<MainWindow>, title: String, description: String) -> bool {
    let delete_label = ctr!("PreferencesPage", "Delete").to_string();
    let result = message_dialog_of(owner)
        .set_title(title)
        .set_description(description)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            delete_label.clone(),
            ctr!("PreferencesPage", "Cancel").to_string(),
        ))
        .show()
        .await;
    overwrite_confirmed(&result, &delete_label)
}

/// Asks before deleting a printer the person added, then deletes its file.
/// The printer in use that goes is no longer in use.
fn spawn_delete_printer(deletion: Deletion) {
    let Some((id, name)) = deletion
        .catalog
        .borrow()
        .printers()
        .get(deletion.at)
        .map(|p| (p.id.clone(), format!("{} {}", p.manufacturer, p.name)))
    else {
        return;
    };
    if let Err(err) = slint::spawn_local(async move {
        let Ok(user_lib) = user_library_dir() else {
            return;
        };
        if !confirmed(
            &deletion.owner,
            ctr!("PrinterDialog", "Delete printer?").to_string(),
            ctr!(
                "PrinterDialog",
                "Deleting {} removes the printer you added. Resins keep their settings for it.",
                name
            )
            .to_string(),
        )
        .await
        {
            return;
        }
        if let Err(err) = std::fs::remove_file(printer_profile_path_in_library(&user_lib, &id)) {
            eprintln!("{err}");
            return;
        }
        refresh_catalog(&deletion.catalog);
        if deletion.bus.snapshot().default_printer.as_ref() == Some(&id) {
            deletion.bus.update(|snap| {
                snap.default_printer = None;
                PreferencesChange::DefaultPrinter
            });
        }
        (deletion.refresh)();
    }) {
        eprintln!("failed to spawn the printer delete task: {err}");
    }
}

/// Asks before deleting a resin the person made (`extension` false) or the
/// settings they gave a resin for more printers, then deletes the file. A
/// resin in use that no longer resolves is no longer in use.
fn spawn_delete_resin(deletion: Deletion, extension: bool) {
    let Some((id, name)) = deletion
        .catalog
        .borrow()
        .materials()
        .get(deletion.at)
        .map(|m| (m.id.clone(), format!("{} {}", m.brand, m.name)))
    else {
        return;
    };
    if let Err(err) = slint::spawn_local(async move {
        let Ok(user_lib) = user_library_dir() else {
            return;
        };
        let description = if extension {
            ctr!(
                "PreferencesPage",
                "Deleting the extension of {} removes every calibration pair it adds.",
                name,
            )
        } else {
            ctr!(
                "PreferencesPage",
                "Deleting {} removes the custom profile file.",
                name,
            )
        };
        if !confirmed(
            &deletion.owner,
            ctr!("PreferencesPage", "Delete profile?").to_string(),
            description.to_string(),
        )
        .await
        {
            return;
        }
        let deleted = if extension {
            delete_material_extension_from_library(&user_lib, &id)
        } else {
            delete_material_profile_from_library(&user_lib, &id)
        };
        if let Err(err) = deleted {
            eprintln!("{err}");
            return;
        }
        refresh_catalog(&deletion.catalog);
        let catalog = deletion.catalog.borrow().clone();
        if default_resin_orphaned(
            &catalog,
            deletion.bus.snapshot().default_resin.as_ref(),
            &id,
        ) {
            deletion.bus.update(|snap| {
                snap.default_resin = None;
                PreferencesChange::DefaultResin
            });
        }
        (deletion.refresh)();
    }) {
        eprintln!("failed to spawn delete task: {err}");
    }
}

/// Whether the deleted id was the resin in use and no longer resolves.
/// Deleting the person's copy of a resin that ships keeps it in use: the copy
/// that ships still resolves it.
fn default_resin_orphaned(
    catalog: &ProfileCatalog,
    default_resin: Option<&MaterialId>,
    deleted: &MaterialId,
) -> bool {
    default_resin == Some(deleted) && catalog.material_index(deleted).is_none()
}

#[cfg(test)]
mod tests;

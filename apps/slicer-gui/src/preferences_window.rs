/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer,
    TimerMode, VecModel,
};

use sc_print_primitives::{MaterialId, PrinterId};
use sc_print_profiles::{
    CalibrationTarget, EffectiveMaterial, MaterialProfile, ProfileLibraries,
    delete_material_extension_from_library, delete_material_profile_from_library,
    load_material_extension_from_library, load_material_profile_from_library, user_library_dir,
};

use crate::aa_level::aa_level_from_combo_index;
use crate::i18n::ctr;
use crate::language::LanguagePreference;
use crate::material_form::{
    CalibrationFormInput, DefaultsFormInput, EditedProfileKind, calibration_form_from_block,
    defaults_form_from, parse_calibration_form, parse_defaults_form, save_edited_profile,
};
use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::preferences_filters::{
    PrinterCardData, ResinFilterData, apply_thumbnails_to_visible, build_brand_tabs,
    build_card_data, build_manufacturer_tabs, build_resin_filter_data, selected_brand,
    selected_manufacturer, set_visible_cards, set_visible_resin_rows, tab_index_for_label,
    tab_label, visible_material_mask,
};
use crate::profile_catalog::{
    MaterialCatalogEntry, MaterialExtensionEntry, PrinterCatalogEntry, ProfileCatalog,
    ProfileSource, has_active_matcher,
};
use crate::profile_extending::{build_extension_update, copy_sources, uncovered_targets};
use crate::profile_forking::{ForkParts, build_fork};
use crate::profile_transfer::{
    ExportScope, ImportOutcome, format_collision_report, overwrite_confirmed, run_export,
    run_import,
};
use crate::profile_view::{
    build_profile_view_model, identity_section, intent_section, ordered_calibration_targets,
    profile_display_name,
};
use crate::window_prefs::WindowPreferences;
use crate::{
    ExtendDialog, ForkDialog, PillKind, PreferencesWindow, ProfileViewDialog, ResinPill, ResinRow,
};

/// View-local state of the resin list: every derived row in tier order, the
/// filter data for the materials those rows index into, the tab labels, and the
/// active filter. Never reaches the bus.
///
/// `rows` and `filter_data` are rebuilt together from the same catalog, so a
/// row's `id_index` always indexes `filter_data`.
struct ResinView {
    rows: Vec<ResinRow>,
    filter_data: Vec<ResinFilterData>,
    tabs: Vec<String>,
    current_tab: i32,
    search_lower: String,
}

/// Bundle of a `PreferencesWindow` and its bus subscription. Drop drops the
/// subscription so the listener is removed as soon as the wrapped window is no
/// longer held.
pub(crate) struct WiredPreferencesWindow {
    pub window: PreferencesWindow,
    _subscription: Subscription,
    // Keeps the thumbnail-loading poll timer alive for the window's lifetime;
    // dropping it (when the window closes) stops the poll.
    _thumbnail_timer: Rc<Timer>,
}

pub(crate) fn build_preferences_window(
    bus: &Rc<PreferencesBus>,
    catalog_handle: Rc<RefCell<Rc<ProfileCatalog>>>,
) -> Result<WiredPreferencesWindow, slint::PlatformError> {
    let prefs = PreferencesWindow::new()?;
    let catalog = catalog_handle.borrow().clone();

    // The gallery tiles and the manufacturer tab bar are derived from the
    // catalog, held behind interior mutability so `refresh_view` can rebuild
    // them in place after each rebuild of the catalog.
    let all_cards = Rc::new(RefCell::new(build_card_data(
        catalog.printers(),
        &bus.snapshot().favourite_printers,
    )));
    let tabs = Rc::new(RefCell::new(build_manufacturer_tabs(&catalog)));
    // View-local filter state: which manufacturer tab and search text narrow
    // the visible tiles. Never reaches the bus.
    let current_tab = Rc::new(Cell::new(0i32));
    let search_lower = Rc::new(RefCell::new(String::new()));

    let snapshot = bus.snapshot();
    prefs.apply_preferences(&snapshot);

    // Start on the brand tab of the default resin, the way the gallery starts
    // on the manufacturer of the default printer. Resolved before the first
    // derivation, which pushes through this filter.
    let brand_tabs = build_brand_tabs(&catalog);
    let initial_brand_tab = tab_index_for_label(
        &brand_tabs,
        selected_brand(catalog.materials(), snapshot.default_resin.as_ref()).as_deref(),
    );
    prefs.set_current_brand_index(initial_brand_tab);
    prefs.set_resin_brand_tabs(string_model(&brand_tabs));
    let resin_view = Rc::new(RefCell::new(ResinView {
        rows: Vec::new(),
        filter_data: Vec::new(),
        tabs: brand_tabs,
        current_tab: initial_brand_tab,
        search_lower: String::new(),
    }));

    // Selection first: the visible-cards pass below reads the selected printer
    // index back from the window.
    apply_printing(&prefs, &snapshot, &catalog, &resin_view);

    // Start on the manufacturer tab of the app's default printer, so that
    // printer is in view when the section first opens.
    let initial_tab = tab_index_for_label(
        &tabs.borrow(),
        selected_manufacturer(&catalog, &snapshot).as_deref(),
    );
    current_tab.set(initial_tab);
    prefs.set_current_manufacturer_index(initial_tab);

    prefs.set_manufacturer_tabs(string_model(&tabs.borrow()));
    set_visible_cards(
        &prefs,
        &all_cards.borrow(),
        tab_label(&tabs.borrow(), initial_tab),
        "",
    );

    let prefs_weak = prefs.as_weak();

    // Thumbnails decode on a worker thread and land in the gallery once ready;
    // this timer polls for the finished batch. Kept alive by the returned
    // `WiredPreferencesWindow`.
    let thumbnail_timer = Rc::new(Timer::default());
    start_thumbnail_load(
        &thumbnail_timer,
        &prefs,
        all_cards.clone(),
        thumbnail_paths(&catalog),
    );

    // Rebuild every catalog-derived view (gallery, tabs, resin list) from the
    // current catalog handle, preserving the active filter and selection.
    // Called after each rebuild of the catalog.
    let refresh_view: Rc<dyn Fn()> = {
        let prefs_weak = prefs_weak.clone();
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let all_cards = all_cards.clone();
        let tabs = tabs.clone();
        let current_tab = current_tab.clone();
        let search_lower = search_lower.clone();
        let thumbnail_timer = thumbnail_timer.clone();
        let resin_view = resin_view.clone();
        Rc::new(move || {
            let Some(w) = prefs_weak.upgrade() else {
                return;
            };
            let catalog = catalog_handle.borrow().clone();
            *all_cards.borrow_mut() =
                build_card_data(catalog.printers(), &bus.snapshot().favourite_printers);
            *tabs.borrow_mut() = build_manufacturer_tabs(&catalog);
            // An import can add a brand, which shifts every tab index after it,
            // so the active tab is re-resolved by its label.
            {
                let mut view = resin_view.borrow_mut();
                let active = tab_label(&view.tabs, view.current_tab).map(str::to_owned);
                view.tabs = build_brand_tabs(&catalog);
                view.current_tab = tab_index_for_label(&view.tabs, active.as_deref());
                w.set_current_brand_index(view.current_tab);
                w.set_resin_brand_tabs(string_model(&view.tabs));
            }
            // Selection first: catalog indices may have shifted, and the
            // visible-cards pass reads the selected printer index back from the
            // window.
            apply_printing(&w, &bus.snapshot(), &catalog, &resin_view);
            w.set_manufacturer_tabs(string_model(&tabs.borrow()));
            set_visible_cards(
                &w,
                &all_cards.borrow(),
                tab_label(&tabs.borrow(), current_tab.get()),
                &search_lower.borrow(),
            );
            start_thumbnail_load(
                &thumbnail_timer,
                &w,
                all_cards.clone(),
                thumbnail_paths(&catalog),
            );
        })
    };

    prefs.on_close_requested({
        let prefs_weak = prefs_weak.clone();
        move || {
            if let Some(w) = prefs_weak.upgrade() {
                let _ = w.hide();
            }
        }
    });

    prefs.on_theme_changed({
        let bus = bus.clone();
        move |new_theme| {
            bus.update(move |snap| {
                snap.theme = new_theme;
                PreferencesChange::Theme
            });
        }
    });

    prefs.on_language_changed({
        let bus = bus.clone();
        move |index| {
            let new_language = LanguagePreference::from_combo_index(index);
            bus.update(move |snap| {
                snap.language = new_language;
                PreferencesChange::Language
            });
        }
    });

    prefs.on_aa_level_changed({
        let bus = bus.clone();
        move |index| {
            let new_level = aa_level_from_combo_index(index);
            bus.update(move |snap| {
                snap.aa_level = new_level;
                PreferencesChange::AaLevel
            });
        }
    });

    prefs.on_manufacturer_tab_changed({
        let prefs_weak = prefs_weak.clone();
        let all_cards = all_cards.clone();
        let tabs = tabs.clone();
        let current_tab = current_tab.clone();
        let search_lower = search_lower.clone();
        move |index| {
            current_tab.set(index);
            if let Some(w) = prefs_weak.upgrade() {
                set_visible_cards(
                    &w,
                    &all_cards.borrow(),
                    tab_label(&tabs.borrow(), index),
                    &search_lower.borrow(),
                );
            }
        }
    });

    prefs.on_printer_search_changed({
        let prefs_weak = prefs_weak.clone();
        let all_cards = all_cards.clone();
        let tabs = tabs.clone();
        let current_tab = current_tab.clone();
        let search_lower = search_lower.clone();
        move |text| {
            *search_lower.borrow_mut() = text.to_lowercase();
            if let Some(w) = prefs_weak.upgrade() {
                set_visible_cards(
                    &w,
                    &all_cards.borrow(),
                    tab_label(&tabs.borrow(), current_tab.get()),
                    &search_lower.borrow(),
                );
            }
        }
    });

    prefs.on_brand_tab_changed({
        let prefs_weak = prefs_weak.clone();
        let resin_view = resin_view.clone();
        move |index| {
            resin_view.borrow_mut().current_tab = index;
            if let Some(w) = prefs_weak.upgrade() {
                push_visible_resins(&w, &resin_view.borrow());
            }
        }
    });

    prefs.on_resin_search_changed({
        let prefs_weak = prefs_weak.clone();
        let resin_view = resin_view.clone();
        move |text| {
            resin_view.borrow_mut().search_lower = text.to_lowercase();
            if let Some(w) = prefs_weak.upgrade() {
                // Without this the box would come back empty over a still
                // filtered list when the section is rebuilt on a sidebar
                // switch.
                w.set_resin_search_text(text);
                push_visible_resins(&w, &resin_view.borrow());
            }
        }
    });

    prefs.on_printer_selected({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        move |index| {
            let catalog = catalog_handle.borrow().clone();
            let Some(entry) = catalog.printers().get(index as usize) else {
                return;
            };
            let id = entry.id.clone();
            bus.update(move |snap| {
                snap.default_printer = Some(id);
                PreferencesChange::DefaultPrinter
            });
        }
    });

    prefs.on_resin_selected({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        move |index| {
            let catalog = catalog_handle.borrow().clone();
            let Some(entry) = catalog.materials().get(index as usize) else {
                return;
            };
            let id = entry.id.clone();
            bus.update(move |snap| {
                snap.default_resin = Some(id);
                PreferencesChange::DefaultResin
            });
        }
    });

    prefs.on_printer_favourite_toggled({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let all_cards = all_cards.clone();
        let prefs_weak = prefs_weak.clone();
        move |index| {
            let catalog = catalog_handle.borrow().clone();
            let Some(entry) = catalog.printers().get(index as usize) else {
                return;
            };
            let id = entry.id.clone();
            let make_favourite = !bus.snapshot().favourite_printers.contains(&id);
            bus.update(move |snap| {
                if make_favourite {
                    snap.favourite_printers.insert(id);
                } else {
                    snap.favourite_printers.remove(&id);
                }
                PreferencesChange::Favourites
            });
            // Flip the star in place: the bus subscription re-applies the resin
            // view but not the gallery, and rebuilding the cards would drop the
            // async-loaded thumbnails, so update that card directly (keeps its
            // thumbnail and the scroll position).
            if let Some(data) = all_cards.borrow_mut().get_mut(index as usize) {
                data.card.is_favourite = make_favourite;
            }
            if let Some(w) = prefs_weak.upgrade() {
                let cards = w.get_printer_cards();
                for row in 0..cards.row_count() {
                    if let Some(mut card) = cards.row_data(row)
                        && card.id_index == index
                    {
                        card.is_favourite = make_favourite;
                        cards.set_row_data(row, card);
                    }
                }
            }
        }
    });

    prefs.on_resin_favourite_toggled({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        move |index| {
            let catalog = catalog_handle.borrow().clone();
            let Some(entry) = catalog.materials().get(index as usize) else {
                return;
            };
            let id = entry.id.clone();
            let make_favourite = !bus.snapshot().favourite_resins.contains(&id);
            bus.update(move |snap| {
                if make_favourite {
                    snap.favourite_resins.insert(id);
                } else {
                    snap.favourite_resins.remove(&id);
                }
                PreferencesChange::Favourites
            });
            // The bus subscription re-runs `apply_printing`, which rebuilds the
            // resin rows against the updated favourites, so the row's star
            // refreshes without extra work here.
        }
    });

    prefs.on_import_requested({
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        move || spawn_import(catalog_handle.clone(), refresh_view.clone())
    });
    prefs.on_export_printers_requested(move || spawn_export(ExportScope::Printers));
    prefs.on_export_materials_requested(move || spawn_export(ExportScope::Materials));

    // Holds the profile viewer or editor while one is open so it is not dropped
    // (and closed) the moment its opener returns. Opening another profile
    // replaces (and closes) the previous dialog.
    let profile_dialog: Rc<RefCell<Option<ProfileViewDialog>>> = Rc::new(RefCell::new(None));
    prefs.on_view_requested({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let profile_dialog = profile_dialog.clone();
        move |index| {
            open_profile_view_dialog(&bus, &catalog_handle, &profile_dialog, index);
        }
    });

    prefs.on_edit_requested({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let profile_dialog = profile_dialog.clone();
        move |index, is_extension| {
            open_profile_editor(
                &bus,
                &catalog_handle,
                &refresh_view,
                &profile_dialog,
                index,
                is_extension,
                None,
            );
        }
    });

    prefs.on_delete_requested({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        move |index, is_extension| {
            spawn_delete_flow(
                bus.clone(),
                catalog_handle.clone(),
                refresh_view.clone(),
                index,
                is_extension,
            );
        }
    });

    // Holds the fork dialog while one is open, like `profile_dialog` does for
    // the viewer / editor.
    let fork_dialog: Rc<RefCell<Option<ForkDialog>>> = Rc::new(RefCell::new(None));
    prefs.on_fork_requested({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let fork_dialog = fork_dialog.clone();
        let profile_dialog = profile_dialog.clone();
        move |index| {
            open_fork_dialog(
                &bus,
                &catalog_handle,
                &refresh_view,
                &fork_dialog,
                &profile_dialog,
                index,
            );
        }
    });

    // Holds the extend dialog while one is open, like `fork_dialog`.
    let extend_dialog: Rc<RefCell<Option<ExtendDialog>>> = Rc::new(RefCell::new(None));
    prefs.on_extend_requested({
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let extend_dialog = extend_dialog.clone();
        move |index| {
            open_extend_dialog(&bus, &catalog_handle, &refresh_view, &extend_dialog, index);
        }
    });

    let subscription = bus.subscribe({
        let prefs_weak = prefs_weak.clone();
        let catalog_handle = catalog_handle.clone();
        let resin_view = resin_view.clone();
        move |snap, _change| {
            if let Some(w) = prefs_weak.upgrade() {
                w.apply_preferences(snap);
                let catalog = catalog_handle.borrow().clone();
                apply_printing(&w, snap, &catalog, &resin_view);
            }
        }
    });

    Ok(WiredPreferencesWindow {
        window: prefs,
        _subscription: subscription,
        _thumbnail_timer: thumbnail_timer,
    })
}

fn string_model(values: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|s| SharedString::from(s.as_str()))
            .collect::<Vec<_>>(),
    ))
}

fn refresh_catalog(handle: &Rc<RefCell<Rc<ProfileCatalog>>>) {
    *handle.borrow_mut() = Rc::new(ProfileCatalog::from_libraries(
        &ProfileLibraries::platform_default(),
    ));
}

/// Runs the export flow for `scope`: pick a `.zip` destination, then package
/// the matching profiles from the user library. Failures are reported on
/// stderr; the library is read-only here.
fn spawn_export(scope: ExportScope) {
    if let Err(err) = slint::spawn_local(async move {
        let Ok(library_dir) = user_library_dir() else {
            eprintln!("cannot resolve the user profile library directory");
            return;
        };
        let Some(handle) = rfd::AsyncFileDialog::new()
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

/// Runs the import flow: pick a `.zip`, install it into the user library, and
/// on a collision offer to overwrite. On success the catalog is rebuilt and the
/// window's views refreshed so imported profiles appear immediately.
fn spawn_import(catalog_handle: Rc<RefCell<Rc<ProfileCatalog>>>, refresh_view: Rc<dyn Fn()>) {
    if let Err(err) = slint::spawn_local(async move {
        let Ok(library_dir) = user_library_dir() else {
            eprintln!("cannot resolve the user profile library directory");
            return;
        };
        let Some(handle) = rfd::AsyncFileDialog::new()
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
                refresh_catalog(&catalog_handle);
                refresh_view();
            }
            ImportOutcome::Collisions(report) => {
                let overwrite = ctr!("PreferencesPage", "Overwrite").to_string();
                let result = rfd::AsyncMessageDialog::new()
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
                            refresh_catalog(&catalog_handle);
                            refresh_view();
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

/// Loads the material profile behind a catalog entry. Regular entries resolve
/// through the by-id cascade (user library shadowing the bundled roots); an id
/// whose only file is an extension (its base profile is gone) falls back to the
/// extension file itself so the data stays viewable.
fn load_profile_for_view(
    catalog: &ProfileCatalog,
    index: i32,
) -> Option<(MaterialProfile, Option<MaterialProfile>, ProfileSource)> {
    let entry = catalog.materials().get(index as usize)?;
    let libraries = ProfileLibraries::platform_default();

    let profile = match libraries.load_material_by_id(&entry.id) {
        Ok(profile) => profile,
        Err(base_err) => {
            let orphan = libraries.user_library_dir().and_then(|user_lib| {
                load_material_extension_from_library(user_lib, &entry.id).ok()
            });
            match orphan {
                Some(orphan_extension) => orphan_extension,
                None => {
                    eprintln!("{base_err}");
                    return None;
                }
            }
        }
    };

    let extension = catalog.extension_for(&entry.id).and_then(|_| {
        let user_lib = libraries.user_library_dir()?;
        match load_material_extension_from_library(user_lib, &entry.id) {
            Ok(extension) => Some(extension),
            Err(err) => {
                eprintln!("{err}");
                None
            }
        }
    });

    Some((profile, extension, entry.source))
}

/// Opens the read-only profile viewer for the resin at `index`: the base
/// profile's sections plus its extension's calibration blocks, with the block
/// the default printer would use highlighted.
fn open_profile_view_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    dialog_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((profile, extension, source)) = load_profile_for_view(&catalog, index) else {
        return;
    };
    let (material, issues) = EffectiveMaterial::compose(profile, extension);
    for issue in &issues {
        eprintln!("warning: {issue}");
    }

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    let sections = build_profile_view_model(
        &material,
        catalog.printers(),
        default_printer.as_ref(),
        source,
    );

    let dialog = match ProfileViewDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the profile viewer: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    dialog.set_profile_name(profile_display_name(material.identity()).into());
    dialog.set_sections(ModelRc::new(VecModel::from(sections)));

    let dialog_weak = dialog.as_weak();
    dialog.on_close_requested({
        let dialog_slot = dialog_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *dialog_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the profile viewer: {err}");
        return;
    }
    *dialog_slot.borrow_mut() = Some(dialog);
}

/// Opens the in-place editor for a custom row. The extension row edits the
/// extension file; a custom profile row edits the full profile, falling back to
/// the extension file for an orphaned extension entry. The defaults are
/// editable for full profiles only (an extension inherits them from its
/// parent), and 1 calibration pair per session can be picked from the profile's
/// own blocks and edited.
fn open_profile_editor(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    dialog_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
    is_extension: bool,
    note: Option<SharedString>,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some(entry) = catalog.materials().get(index as usize) else {
        return;
    };
    let user_lib = match user_library_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("cannot resolve the user profile library directory: {err}");
            return;
        }
    };

    let (profile, kind) = if is_extension {
        match load_material_extension_from_library(&user_lib, &entry.id) {
            Ok(profile) => (profile, EditedProfileKind::Extension),
            Err(err) => {
                eprintln!("{err}");
                return;
            }
        }
    } else {
        match load_material_profile_from_library(&user_lib, &entry.id) {
            Ok(profile) => (profile, EditedProfileKind::Full),
            Err(full_err) => match load_material_extension_from_library(&user_lib, &entry.id) {
                Ok(orphan_extension) => (orphan_extension, EditedProfileKind::Extension),
                Err(_) => {
                    eprintln!("{full_err}");
                    return;
                }
            },
        }
    };

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());

    // Read-only context above the edit cards: identity, plus intent for full
    // profiles (an extension's intent is inherited).
    let mut sections = vec![identity_section(&profile.identity, ProfileSource::User)];
    if kind == EditedProfileKind::Full {
        sections.push(intent_section(&profile.intent));
    }

    let targets =
        ordered_calibration_targets(&profile, catalog.printers(), default_printer.as_ref());

    let dialog = match ProfileViewDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the profile editor: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    let display_name = match kind {
        EditedProfileKind::Full => SharedString::from(profile_display_name(&profile.identity)),
        EditedProfileKind::Extension => ctr!(
            "PreferencesPage",
            "{} (extended)",
            profile_display_name(&profile.identity)
        ),
    };
    dialog.set_profile_name(display_name);
    dialog.set_note(note.unwrap_or_default());
    dialog.set_sections(ModelRc::new(VecModel::from(sections)));
    dialog.set_editable(true);
    dialog.set_defaults_editable(kind == EditedProfileKind::Full);
    if kind == EditedProfileKind::Full {
        let seeded = defaults_form_from(&profile.defaults);
        dialog.set_scale_x_text(seeded.scale_x_pct.into());
        dialog.set_scale_y_text(seeded.scale_y_pct.into());
        dialog.set_scale_z_text(seeded.scale_z_pct.into());
        dialog.set_defaults_bottom_count_text(seeded.bottom_layer_count.into());
        dialog.set_defaults_wait_before_cure_text(seeded.wait_before_cure_sec.into());
        dialog.set_defaults_wait_after_cure_text(seeded.wait_after_cure_sec.into());
        dialog.set_defaults_wait_after_lift_text(seeded.wait_after_lift_sec.into());
        dialog.set_defaults_light_pwm_text(seeded.light_pwm.into());
        dialog.set_defaults_bottom_light_pwm_text(seeded.bottom_light_pwm.into());
    }
    dialog.set_edit_target_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_edit_target_index(-1);

    let profile = Rc::new(RefCell::new(profile));
    let targets = Rc::new(targets);
    let dialog_weak = dialog.as_weak();

    dialog.on_edit_target_changed({
        let dialog_weak = dialog_weak.clone();
        let profile = profile.clone();
        let targets = targets.clone();
        move |picked| {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some((id, _)) = targets.get(picked as usize) else {
                return;
            };
            let profile = profile.borrow();
            let Some(block) = profile.per_printer.get(id) else {
                return;
            };
            let seeded = calibration_form_from_block(block);
            d.set_normal_exposure_text(seeded.normal_exposure_sec.into());
            d.set_bottom_exposure_text(seeded.bottom_exposure_sec.into());
            d.set_block_bottom_count_text(seeded.bottom_layer_count.into());
            d.set_wait_before_cure_text(seeded.wait_before_cure_sec.into());
            d.set_wait_after_cure_text(seeded.wait_after_cure_sec.into());
            d.set_wait_after_lift_text(seeded.wait_after_lift_sec.into());
            d.set_bottom_wait_before_cure_text(seeded.bottom_wait_before_cure_sec.into());
            d.set_bottom_wait_after_cure_text(seeded.bottom_wait_after_cure_sec.into());
            d.set_bottom_wait_after_lift_text(seeded.bottom_wait_after_lift_sec.into());
            d.set_lift_distance_text(seeded.lift_distance_mm.into());
            d.set_lift_speed_text(seeded.lift_speed_mm_min.into());
            d.set_retract_speed_text(seeded.retract_speed_mm_min.into());
            d.set_second_lift_distance_text(seeded.second_lift_distance_mm.into());
            d.set_second_lift_speed_text(seeded.second_lift_speed_mm_min.into());
            d.set_second_retract_speed_text(seeded.second_retract_speed_mm_min.into());
            d.set_bottom_lift_distance_text(seeded.bottom_lift_distance_mm.into());
            d.set_bottom_lift_speed_text(seeded.bottom_lift_speed_mm_min.into());
            d.set_bottom_retract_speed_text(seeded.bottom_retract_speed_mm_min.into());
            d.set_bottom_second_lift_distance_text(seeded.bottom_second_lift_distance_mm.into());
            d.set_bottom_second_lift_speed_text(seeded.bottom_second_lift_speed_mm_min.into());
            d.set_bottom_second_retract_speed_text(
                seeded.bottom_second_retract_speed_mm_min.into(),
            );
            d.set_light_pwm_text(seeded.light_pwm.into());
            d.set_bottom_light_pwm_text(seeded.bottom_light_pwm.into());
        }
    });

    dialog.on_save_requested({
        let dialog_weak = dialog_weak.clone();
        let dialog_slot = dialog_slot.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let profile = profile.clone();
        let targets = targets.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let mut profile = profile.borrow_mut();
            if kind == EditedProfileKind::Full {
                let input = DefaultsFormInput {
                    scale_x_pct: d.get_scale_x_text().to_string(),
                    scale_y_pct: d.get_scale_y_text().to_string(),
                    scale_z_pct: d.get_scale_z_text().to_string(),
                    bottom_layer_count: d.get_defaults_bottom_count_text().to_string(),
                    wait_before_cure_sec: d.get_defaults_wait_before_cure_text().to_string(),
                    wait_after_cure_sec: d.get_defaults_wait_after_cure_text().to_string(),
                    wait_after_lift_sec: d.get_defaults_wait_after_lift_text().to_string(),
                    light_pwm: d.get_defaults_light_pwm_text().to_string(),
                    bottom_light_pwm: d.get_defaults_bottom_light_pwm_text().to_string(),
                };
                match parse_defaults_form(&input) {
                    Ok(defaults) => profile.defaults = defaults,
                    Err(err) => {
                        d.set_error_message(err.user_message());
                        return;
                    }
                }
            }
            let picked = d.get_edit_target_index();
            if picked >= 0 {
                let Some((id, _)) = targets.get(picked as usize) else {
                    return;
                };
                let input = CalibrationFormInput {
                    normal_exposure_sec: d.get_normal_exposure_text().to_string(),
                    bottom_exposure_sec: d.get_bottom_exposure_text().to_string(),
                    bottom_layer_count: d.get_block_bottom_count_text().to_string(),
                    wait_before_cure_sec: d.get_wait_before_cure_text().to_string(),
                    wait_after_cure_sec: d.get_wait_after_cure_text().to_string(),
                    wait_after_lift_sec: d.get_wait_after_lift_text().to_string(),
                    bottom_wait_before_cure_sec: d.get_bottom_wait_before_cure_text().to_string(),
                    bottom_wait_after_cure_sec: d.get_bottom_wait_after_cure_text().to_string(),
                    bottom_wait_after_lift_sec: d.get_bottom_wait_after_lift_text().to_string(),
                    lift_distance_mm: d.get_lift_distance_text().to_string(),
                    lift_speed_mm_min: d.get_lift_speed_text().to_string(),
                    retract_speed_mm_min: d.get_retract_speed_text().to_string(),
                    second_lift_distance_mm: d.get_second_lift_distance_text().to_string(),
                    second_lift_speed_mm_min: d.get_second_lift_speed_text().to_string(),
                    second_retract_speed_mm_min: d.get_second_retract_speed_text().to_string(),
                    bottom_lift_distance_mm: d.get_bottom_lift_distance_text().to_string(),
                    bottom_lift_speed_mm_min: d.get_bottom_lift_speed_text().to_string(),
                    bottom_retract_speed_mm_min: d.get_bottom_retract_speed_text().to_string(),
                    bottom_second_lift_distance_mm: d
                        .get_bottom_second_lift_distance_text()
                        .to_string(),
                    bottom_second_lift_speed_mm_min: d
                        .get_bottom_second_lift_speed_text()
                        .to_string(),
                    bottom_second_retract_speed_mm_min: d
                        .get_bottom_second_retract_speed_text()
                        .to_string(),
                    light_pwm: d.get_light_pwm_text().to_string(),
                    bottom_light_pwm: d.get_bottom_light_pwm_text().to_string(),
                };
                match parse_calibration_form(&input) {
                    Ok(block) => {
                        profile.per_printer.insert(id.clone(), block);
                    }
                    Err(err) => {
                        d.set_error_message(err.user_message());
                        return;
                    }
                }
            }
            let user_lib = match user_library_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    d.set_error_message(err.to_string().into());
                    return;
                }
            };
            if let Err(message) = save_edited_profile(&user_lib, &profile, kind) {
                d.set_error_message(message.into());
                return;
            }
            drop(profile);
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *dialog_slot.borrow_mut() = None;
        }
    });

    dialog.on_close_requested({
        let dialog_slot = dialog_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *dialog_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the profile editor: {err}");
        return;
    }
    *dialog_slot.borrow_mut() = Some(dialog);
}

/// Opens the fork dialog for the resin at `index`. The source is the resolved
/// profile behind the row (an extension row forks the same resolved base); the
/// offered parts are strictly the source's own blocks. On confirm the fork is
/// written to the user library and the in-place editor opens on it, with a note
/// that the profile is already saved so cancelling there keeps the untouched
/// fork.
fn open_fork_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    fork_slot: &Rc<RefCell<Option<ForkDialog>>>,
    editor_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((source, _, _)) = load_profile_for_view(&catalog, index) else {
        return;
    };

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    let targets =
        ordered_calibration_targets(&source, catalog.printers(), default_printer.as_ref());

    let dialog = match ForkDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the fork dialog: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    dialog.set_source_name(profile_display_name(&source.identity).into());
    dialog.set_fork_name(ctr!(
        "ForkDialog",
        "{} (fork)",
        source.identity.name.as_str()
    ));
    dialog.set_fork_brand(source.identity.brand.as_str().into());
    dialog.set_block_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));

    // The checkboxes' state, mirrored from the dialog; every part starts
    // selected, matching the checkboxes' initial state.
    let blocks_checked = Rc::new(RefCell::new(vec![true; targets.len()]));
    dialog.on_block_toggled({
        let blocks_checked = blocks_checked.clone();
        move |index, checked| {
            if let Some(slot) = blocks_checked.borrow_mut().get_mut(index as usize) {
                *slot = checked;
            }
        }
    });

    let source = Rc::new(source);
    let targets = Rc::new(targets);
    let dialog_weak = dialog.as_weak();

    dialog.on_confirm_requested({
        let dialog_weak = dialog_weak.clone();
        let fork_slot = fork_slot.clone();
        let editor_slot = editor_slot.clone();
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let source = source.clone();
        let targets = targets.clone();
        let blocks_checked = blocks_checked.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let catalog = catalog_handle.borrow().clone();
            let existing: BTreeSet<MaterialId> = catalog
                .materials()
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            let parts = ForkParts {
                intent: d.get_copy_intent(),
                defaults: d.get_copy_defaults(),
                blocks: targets
                    .iter()
                    .zip(blocks_checked.borrow().iter())
                    .filter(|(_, checked)| **checked)
                    .map(|((id, _), _)| id.clone())
                    .collect(),
            };
            let fork = match build_fork(
                &source,
                &parts,
                &d.get_fork_name(),
                &d.get_fork_brand(),
                &existing,
            ) {
                Ok(fork) => fork,
                Err(err) => {
                    d.set_error_message(err.user_message());
                    return;
                }
            };
            let user_lib = match user_library_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    d.set_error_message(err.to_string().into());
                    return;
                }
            };
            if let Err(message) = save_edited_profile(&user_lib, &fork, EditedProfileKind::Full) {
                d.set_error_message(message.into());
                return;
            }
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *fork_slot.borrow_mut() = None;

            let catalog = catalog_handle.borrow().clone();
            let Some(fork_index) = catalog.material_index(&fork.identity.id) else {
                return;
            };
            open_profile_editor(
                &bus,
                &catalog_handle,
                &refresh_view,
                &editor_slot,
                fork_index as i32,
                false,
                Some(ctr!(
                    "ProfileViewDialog",
                    "The forked profile is already in your library. Close keeps it unchanged.",
                )),
            );
        }
    });

    dialog.on_cancel_requested({
        let fork_slot = fork_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *fork_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the fork dialog: {err}");
        return;
    }
    *fork_slot.borrow_mut() = Some(dialog);
}

/// Opens the extend dialog for the resin at `index`: pick one of the targets
/// the resin does not cover yet, enter the calibration values (partial
/// allowed), and write the block into the resin's extension file (created on
/// the first extend, appended to afterwards).
fn open_extend_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    extend_slot: &Rc<RefCell<Option<ExtendDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((base, extension, _)) = load_profile_for_view(&catalog, index) else {
        return;
    };

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    // The dialog writes to the raw files, so `base` and `extension` stay
    // around; the composed view feeds the target complement and the blocks a
    // new one can be copied from.
    let (material, issues) = EffectiveMaterial::compose(base.clone(), extension.clone());
    for issue in &issues {
        eprintln!("warning: {issue}");
    }
    let targets = uncovered_targets(&material, catalog.printers(), default_printer.as_ref());
    if targets.is_empty() {
        return;
    }
    let sources = copy_sources(&material, catalog.printers());

    let dialog = match ExtendDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the extend dialog: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    dialog.set_source_name(profile_display_name(&base.identity).into());
    dialog.set_target_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_target_index(-1);
    dialog.set_copy_source_labels(ModelRc::new(VecModel::from(
        sources
            .iter()
            .map(|source| source.label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_copy_source_index(-1);

    let base = Rc::new(base);
    let extension = Rc::new(extension);
    let targets = Rc::new(targets);
    let sources = Rc::new(sources);
    let dialog_weak = dialog.as_weak();

    dialog.on_copy_requested({
        let dialog_weak = dialog_weak.clone();
        let sources = sources.clone();
        move |picked| {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some(source) = sources.get(picked as usize) else {
                return;
            };
            let seeded = calibration_form_from_block(&source.values);
            d.set_normal_exposure_text(seeded.normal_exposure_sec.into());
            d.set_bottom_exposure_text(seeded.bottom_exposure_sec.into());
            d.set_bottom_count_text(seeded.bottom_layer_count.into());
            d.set_wait_before_cure_text(seeded.wait_before_cure_sec.into());
            d.set_wait_after_cure_text(seeded.wait_after_cure_sec.into());
            d.set_wait_after_lift_text(seeded.wait_after_lift_sec.into());
            d.set_bottom_wait_before_cure_text(seeded.bottom_wait_before_cure_sec.into());
            d.set_bottom_wait_after_cure_text(seeded.bottom_wait_after_cure_sec.into());
            d.set_bottom_wait_after_lift_text(seeded.bottom_wait_after_lift_sec.into());
            d.set_lift_distance_text(seeded.lift_distance_mm.into());
            d.set_lift_speed_text(seeded.lift_speed_mm_min.into());
            d.set_retract_speed_text(seeded.retract_speed_mm_min.into());
            d.set_second_lift_distance_text(seeded.second_lift_distance_mm.into());
            d.set_second_lift_speed_text(seeded.second_lift_speed_mm_min.into());
            d.set_second_retract_speed_text(seeded.second_retract_speed_mm_min.into());
            d.set_bottom_lift_distance_text(seeded.bottom_lift_distance_mm.into());
            d.set_bottom_lift_speed_text(seeded.bottom_lift_speed_mm_min.into());
            d.set_bottom_retract_speed_text(seeded.bottom_retract_speed_mm_min.into());
            d.set_bottom_second_lift_distance_text(seeded.bottom_second_lift_distance_mm.into());
            d.set_bottom_second_lift_speed_text(seeded.bottom_second_lift_speed_mm_min.into());
            d.set_bottom_second_retract_speed_text(
                seeded.bottom_second_retract_speed_mm_min.into(),
            );
            d.set_light_pwm_text(seeded.light_pwm.into());
            d.set_bottom_light_pwm_text(seeded.bottom_light_pwm.into());
        }
    });

    dialog.on_save_requested({
        let dialog_weak = dialog_weak.clone();
        let extend_slot = extend_slot.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let base = base.clone();
        let extension = extension.clone();
        let targets = targets.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some((target, _)) = targets.get(d.get_target_index() as usize) else {
                return;
            };
            let input = CalibrationFormInput {
                normal_exposure_sec: d.get_normal_exposure_text().to_string(),
                bottom_exposure_sec: d.get_bottom_exposure_text().to_string(),
                bottom_layer_count: d.get_bottom_count_text().to_string(),
                wait_before_cure_sec: d.get_wait_before_cure_text().to_string(),
                wait_after_cure_sec: d.get_wait_after_cure_text().to_string(),
                wait_after_lift_sec: d.get_wait_after_lift_text().to_string(),
                bottom_wait_before_cure_sec: d.get_bottom_wait_before_cure_text().to_string(),
                bottom_wait_after_cure_sec: d.get_bottom_wait_after_cure_text().to_string(),
                bottom_wait_after_lift_sec: d.get_bottom_wait_after_lift_text().to_string(),
                lift_distance_mm: d.get_lift_distance_text().to_string(),
                lift_speed_mm_min: d.get_lift_speed_text().to_string(),
                retract_speed_mm_min: d.get_retract_speed_text().to_string(),
                second_lift_distance_mm: d.get_second_lift_distance_text().to_string(),
                second_lift_speed_mm_min: d.get_second_lift_speed_text().to_string(),
                second_retract_speed_mm_min: d.get_second_retract_speed_text().to_string(),
                bottom_lift_distance_mm: d.get_bottom_lift_distance_text().to_string(),
                bottom_lift_speed_mm_min: d.get_bottom_lift_speed_text().to_string(),
                bottom_retract_speed_mm_min: d.get_bottom_retract_speed_text().to_string(),
                bottom_second_lift_distance_mm: d
                    .get_bottom_second_lift_distance_text()
                    .to_string(),
                bottom_second_lift_speed_mm_min: d.get_bottom_second_lift_speed_text().to_string(),
                bottom_second_retract_speed_mm_min: d
                    .get_bottom_second_retract_speed_text()
                    .to_string(),
                light_pwm: d.get_light_pwm_text().to_string(),
                bottom_light_pwm: d.get_bottom_light_pwm_text().to_string(),
            };
            let block = match parse_calibration_form(&input) {
                Ok(block) => block,
                Err(err) => {
                    d.set_error_message(err.user_message());
                    return;
                }
            };
            let update = build_extension_update(&base, (*extension).as_ref(), target, block);
            let user_lib = match user_library_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    d.set_error_message(err.to_string().into());
                    return;
                }
            };
            if let Err(message) =
                save_edited_profile(&user_lib, &update, EditedProfileKind::Extension)
            {
                d.set_error_message(message.into());
                return;
            }
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *extend_slot.borrow_mut() = None;
        }
    });

    dialog.on_cancel_requested({
        let extend_slot = extend_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *extend_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the extend dialog: {err}");
        return;
    }
    *extend_slot.borrow_mut() = Some(dialog);
}

/// Reports whether the just-deleted id was the default resin and no longer
/// resolves to any profile. Deleting a user override of a bundled id keeps the
/// default: the bundled copy still resolves it.
fn default_resin_orphaned(
    catalog: &ProfileCatalog,
    default_resin: Option<&MaterialId>,
    deleted: &MaterialId,
) -> bool {
    default_resin == Some(deleted) && catalog.material_index(deleted).is_none()
}

/// Runs the delete flow for a custom row: confirm, remove the file (the full
/// profile's, or the extension file with every pair it adds), then refresh.
/// When the deleted id was the default resin and nothing else resolves it any
/// more, the default is unset.
fn spawn_delete_flow(
    bus: Rc<PreferencesBus>,
    catalog_handle: Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: Rc<dyn Fn()>,
    index: i32,
    is_extension: bool,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some(entry) = catalog.materials().get(index as usize) else {
        return;
    };
    let id = entry.id.clone();
    let display_name = format!("{} {}", entry.brand, entry.name);

    if let Err(err) = slint::spawn_local(async move {
        let Ok(user_lib) = user_library_dir() else {
            eprintln!("cannot resolve the user profile library directory");
            return;
        };
        let description = if is_extension {
            ctr!(
                "PreferencesPage",
                "Deleting the extension of {} removes every calibration pair it adds.",
                display_name,
            )
            .to_string()
        } else {
            ctr!(
                "PreferencesPage",
                "Deleting {} removes the custom profile file.",
                display_name,
            )
            .to_string()
        };
        let delete_label = ctr!("PreferencesPage", "Delete").to_string();
        let result = rfd::AsyncMessageDialog::new()
            .set_title(ctr!("PreferencesPage", "Delete profile?").to_string())
            .set_description(description)
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                delete_label.clone(),
                ctr!("PreferencesPage", "Cancel").to_string(),
            ))
            .show()
            .await;
        if !overwrite_confirmed(&result, &delete_label) {
            return;
        }
        let deleted = if is_extension {
            delete_material_extension_from_library(&user_lib, &id)
        } else {
            delete_material_profile_from_library(&user_lib, &id)
        };
        if let Err(err) = deleted {
            eprintln!("{err}");
            return;
        }
        refresh_catalog(&catalog_handle);
        let catalog = catalog_handle.borrow().clone();
        if default_resin_orphaned(&catalog, bus.snapshot().default_resin.as_ref(), &id) {
            bus.update(|snap| {
                snap.default_resin = None;
                PreferencesChange::DefaultResin
            });
        }
        refresh_view();
    }) {
        eprintln!("failed to spawn delete task: {err}");
    }
}

/// Pushes the current selection (printer / resin indices, the derived resin
/// ordering, and whether a printer is chosen) into the window. The manufacturer
/// and search filters, which are view-local, stay as they are.
fn apply_printing(
    window: &PreferencesWindow,
    snap: &PreferencesSnapshot,
    catalog: &ProfileCatalog,
    resin: &Rc<RefCell<ResinView>>,
) {
    // The gallery highlights, the calibration affordances, and the per-printer
    // resin ordering all track the effective printer: the persisted choice, or
    // the first-run soft default until the user picks one.
    let effective_printer = catalog.effective_default_printer(snap.default_printer.as_ref());
    let printer_index = effective_printer
        .as_ref()
        .and_then(|id| catalog.printer_index(id))
        .map(|i| i as i32)
        .unwrap_or(-1);
    window.set_selected_printer_index(printer_index);

    let resin_index = snap
        .default_resin
        .as_ref()
        .and_then(|id| catalog.material_index(id))
        .map(|i| i as i32)
        .unwrap_or(-1);
    window.set_selected_resin_index(resin_index);

    {
        let mut view = resin.borrow_mut();
        view.rows = derive_resin_view(
            catalog.materials(),
            catalog.extensions(),
            catalog.printers(),
            effective_printer.as_ref(),
            &snap.favourite_resins,
        );
        view.filter_data = build_resin_filter_data(catalog.materials());
    }
    push_visible_resins(window, &resin.borrow());
}

/// Pushes the rows the active brand tab and search text keep.
fn push_visible_resins(window: &PreferencesWindow, view: &ResinView) {
    let mask = visible_material_mask(
        &view.filter_data,
        tab_label(&view.tabs, view.current_tab),
        &view.search_lower,
    );
    set_visible_resin_rows(window, &view.rows, &mask);
}

/// Returns the `(card index, thumbnail path)` pairs that need decoding. Indices
/// match `build_card_data`'s one-to-one printer ordering.
fn thumbnail_paths(catalog: &ProfileCatalog) -> Vec<(usize, PathBuf)> {
    catalog
        .printers()
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| entry.image_path.clone().map(|path| (i, path)))
        .collect()
}

/// Decodes a thumbnail into an RGBA pixel buffer. It runs on a worker thread,
/// so it returns a `SharedPixelBuffer`, which is `Send`. Returns `None` on any
/// read or decode failure; the card then keeps its blank placeholder.
fn decode_thumbnail(path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let rgba = image::open(path)
        .map_err(|err| {
            eprintln!(
                "warning: failed to load printer thumbnail {}: {err}",
                path.display()
            )
        })
        .ok()?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba.as_raw());
    Some(buffer)
}

/// Decodes the printer thumbnails on a worker thread and drops them into the
/// gallery once ready, so opening Preferences never blocks the UI thread. The
/// decoded images land as in-place row updates on the visible model (see
/// `apply_thumbnails_to_visible`), which keeps the gallery's scroll position;
/// they still arrive as a single batch so the timer fires once.
fn start_thumbnail_load(
    timer: &Rc<Timer>,
    window: &PreferencesWindow,
    all_cards: Rc<RefCell<Vec<PrinterCardData>>>,
    paths: Vec<(usize, PathBuf)>,
) {
    timer.stop();
    if paths.is_empty() {
        return;
    }

    let (sender, receiver) = mpsc::channel::<Vec<(usize, SharedPixelBuffer<Rgba8Pixel>)>>();
    std::thread::spawn(move || {
        let decoded = paths
            .into_iter()
            .filter_map(|(index, path)| decode_thumbnail(&path).map(|buffer| (index, buffer)))
            .collect();
        let _ = sender.send(decoded);
    });

    let window_weak = window.as_weak();
    let timer_weak = Rc::downgrade(timer);
    timer.start(TimerMode::Repeated, Duration::from_millis(30), move || {
        let decoded = match receiver.try_recv() {
            Ok(decoded) => decoded,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                // Worker vanished without sending; nothing to apply.
                if let Some(timer) = timer_weak.upgrade() {
                    timer.stop();
                }
                return;
            }
        };

        if let Some(window) = window_weak.upgrade() {
            let mut updated = BTreeSet::new();
            {
                let mut cards = all_cards.borrow_mut();
                for (index, buffer) in decoded {
                    if let Some(card) = cards.get_mut(index) {
                        card.card.thumbnail = Image::from_rgba8(buffer);
                        card.card.has_thumbnail = true;
                        updated.insert(index as i32);
                    }
                }
            }
            apply_thumbnails_to_visible(&window.get_printer_cards(), &all_cards.borrow(), &updated);
        }

        if let Some(timer) = timer_weak.upgrade() {
            timer.stop();
        }
    });
}

/// Derives the resin rows for the list, in 3 tiers: profiles whose block (the
/// base profile's own or its extension's) matches the default printer or
/// generic; then profiles carrying calibration data that does not apply to it;
/// then profiles with no calibration data at all. The catalog's `(brand, name)`
/// order is kept within each tier. An extension renders as its own row right
/// below its parent, sharing the parent's catalog index.
fn derive_resin_view(
    materials: &[MaterialCatalogEntry],
    extensions: &[MaterialExtensionEntry],
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
    favourite_resins: &BTreeSet<MaterialId>,
) -> Vec<ResinRow> {
    let mut matched_rows = Vec::new();
    let mut unmatched_rows = Vec::new();
    let mut dataless_rows = Vec::new();
    for (i, entry) in materials.iter().enumerate() {
        let extension = extensions.iter().find(|ext| ext.id == entry.id);
        // The red no-data marker reflects the whole resin: a base profile whose
        // extension carries blocks is not data-less.
        let union_empty = entry.calibrated_printers.is_empty()
            && extension.is_none_or(|ext| ext.printers.is_empty());
        let target = if has_active_matcher(entry, extension, default_printer) {
            &mut matched_rows
        } else if union_empty {
            &mut dataless_rows
        } else {
            &mut unmatched_rows
        };

        let can_extend = has_uncovered_target(entry, extension, printers);
        // Favourites are keyed by material id, so the base row and its
        // extension row share the same star.
        let is_favourite = favourite_resins.contains(&entry.id);
        target.push(ResinRow {
            name: SharedString::from(entry.name.as_str()),
            brand: SharedString::from(entry.brand.as_str()),
            family: SharedString::from(entry.family.as_str()),
            pills: ModelRc::new(VecModel::from(row_pill_list(
                entry,
                extension,
                printers,
                default_printer,
                false,
            ))),
            is_custom: entry.source == ProfileSource::User,
            is_favourite,
            is_extension: false,
            can_extend,
            id_index: i as i32,
        });

        if extension.is_some() {
            target.push(ResinRow {
                name: ctr!("PreferencesPage", "{} (extended)", entry.name),
                brand: SharedString::from(entry.brand.as_str()),
                family: SharedString::from(entry.family.as_str()),
                pills: ModelRc::new(VecModel::from(row_pill_list(
                    entry,
                    extension,
                    printers,
                    default_printer,
                    true,
                ))),
                is_custom: true,
                is_favourite,
                is_extension: true,
                can_extend,
                id_index: i as i32,
            });
        }
    }
    matched_rows.extend(unmatched_rows);
    matched_rows.extend(dataless_rows);
    matched_rows
}

/// Returns the unchunked pills of a row: the base profile's calibration tags
/// (or the red no-data marker when base and extension are both empty), or, for
/// an extension row, the extension's own tags.
fn row_pill_list(
    entry: &MaterialCatalogEntry,
    extension: Option<&MaterialExtensionEntry>,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
    is_extension_row: bool,
) -> Vec<ResinPill> {
    if is_extension_row {
        return extension
            .map(|ext| build_pills(&ext.printers, printers, default_printer))
            .unwrap_or_default();
    }
    let union_empty =
        entry.calibrated_printers.is_empty() && extension.is_none_or(|ext| ext.printers.is_empty());
    if union_empty {
        vec![no_data_pill(default_printer)]
    } else {
        build_pills(&entry.calibrated_printers, printers, default_printer)
    }
}

/// Reports whether some printer in the catalog has no calibration block in
/// either the base profile or its extension, i.e. whether the extend flow has a
/// target to offer.
fn has_uncovered_target(
    entry: &MaterialCatalogEntry,
    extension: Option<&MaterialExtensionEntry>,
    printers: &[PrinterCatalogEntry],
) -> bool {
    printers.iter().any(|printer| {
        let target = CalibrationTarget::from_printer(&printer.id);
        !entry.calibrated_printers.contains(&target)
            && extension.is_none_or(|ext| !ext.printers.contains(&target))
    })
}

/// Returns the display label of a calibration tag: the printer's name, or its
/// id when the printer is not in the catalog.
fn printer_pill_label(printers: &[PrinterCatalogEntry], id: &PrinterId) -> SharedString {
    printers
        .iter()
        .find(|printer| &printer.id == id)
        .map(|printer| SharedString::from(printer.name.as_str()))
        .unwrap_or_else(|| SharedString::from(id.as_str()))
}

/// Builds 1 pill per calibration block, ordered: the default printer's tag
/// first, the generic tag next, the rest by label. Kinds follow the tag colors:
/// green for the default printer's tag, yellow/orange for generic, gray
/// otherwise; with no default printer everything is gray (the whole list
/// renders dim).
fn build_pills(
    calibrated: &BTreeSet<CalibrationTarget>,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
) -> Vec<ResinPill> {
    let default_target = default_printer.map(CalibrationTarget::from_printer);
    let mut default_pill = None;
    let mut generic_pill = None;
    let mut other_pills = Vec::new();
    for target in calibrated {
        let is_default = default_target.as_ref() == Some(target);
        let kind = if default_printer.is_none() {
            PillKind::Other
        } else if is_default {
            PillKind::OurPrinter
        } else {
            match target {
                CalibrationTarget::Generic => PillKind::Generic,
                CalibrationTarget::Printer(_) => PillKind::Other,
            }
        };
        let label = match target {
            CalibrationTarget::Generic => ctr!("PreferencesPage", "Generic"),
            CalibrationTarget::Printer(id) => printer_pill_label(printers, id.as_printer_id()),
        };
        let pill = ResinPill { label, kind };
        if is_default {
            default_pill = Some(pill);
        } else {
            match target {
                CalibrationTarget::Generic => generic_pill = Some(pill),
                CalibrationTarget::Printer(_) => other_pills.push(pill),
            }
        }
    }
    other_pills.sort_by(|a, b| a.label.cmp(&b.label));
    default_pill
        .into_iter()
        .chain(generic_pill)
        .chain(other_pills)
        .collect()
}

/// Returns the salient marker for a profile without any calibration data. It
/// renders gray like everything else when no default printer is configured.
fn no_data_pill(default_printer: Option<&PrinterId>) -> ResinPill {
    ResinPill {
        label: ctr!("PreferencesPage", "No calibration data"),
        kind: if default_printer.is_some() {
            PillKind::NoData
        } else {
            PillKind::Other
        },
    }
}

#[cfg(test)]
mod derive_resin_view_tests {
    use std::collections::BTreeSet;

    use sc_print_primitives::{
        BuildVolume, FormatKind, MaterialId, NonEmptyString, OutputFormatBinding,
        OutputFormatBindings,
    };

    use super::*;

    fn material(
        id: &str,
        brand: &str,
        name: &str,
        calibrated_for: &[&str],
    ) -> MaterialCatalogEntry {
        MaterialCatalogEntry {
            id: MaterialId::new(id).unwrap(),
            brand: brand.to_owned(),
            name: name.to_owned(),
            family: "other".to_owned(),
            calibrated_printers: calibrated_for
                .iter()
                .map(|p| CalibrationTarget::from(PrinterId::new(p).unwrap()))
                .collect::<BTreeSet<_>>(),
            source: ProfileSource::Bundled,
        }
    }

    fn extension(id: &str, printers: &[&str]) -> MaterialExtensionEntry {
        MaterialExtensionEntry {
            id: MaterialId::new(id).unwrap(),
            printers: printers
                .iter()
                .map(|p| CalibrationTarget::from(PrinterId::new(p).unwrap()))
                .collect(),
        }
    }

    fn printer_entry(id: &str, name: &str) -> PrinterCatalogEntry {
        PrinterCatalogEntry {
            id: PrinterId::new(id).unwrap(),
            manufacturer: "Maker".to_owned(),
            name: name.to_owned(),
            build_volume: BuildVolume::new(100.0, 60.0, 80.0).unwrap(),
            output_formats: OutputFormatBindings::new(OutputFormatBinding {
                format: FormatKind::Goo,
                version: Some(NonEmptyString::new("v3.0").unwrap()),
            }),
            image_path: None,
            source: ProfileSource::Bundled,
        }
    }

    fn printer(id: &str) -> PrinterId {
        PrinterId::new(id).unwrap()
    }

    /// Lists a row's pills as (label, kind) pairs for assertions.
    fn row_pills(row: &ResinRow) -> Vec<(String, PillKind)> {
        row.pills
            .iter()
            .map(|p| (p.label.to_string(), p.kind))
            .collect()
    }

    #[test]
    fn matched_group_comes_first_keeping_brand_name_order_within_groups() {
        let materials = vec![
            material("a-resin", "Alpha", "A", &[]),
            material("b-resin", "Beta", "B", &["printer-a"]),
            material("c-resin", "Gamma", "C", &[]),
            material("d-resin", "Delta", "D", &["generic-machine"]),
        ];
        let rows = derive_resin_view(
            &materials,
            &[],
            &[],
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        // Matched (b: specific, d: generic fallback) first in base order, then
        // the rest.
        let ids: Vec<i32> = rows.iter().map(|r| r.id_index).collect();
        assert_eq!(ids, vec![1, 3, 0, 2]);
    }

    #[test]
    fn dataless_resins_sink_below_unmatched_calibrated_ones() {
        // The default printer matches nothing: no resin carries a block for it
        // and none has a generic block. Resins with some calibration data must
        // still rank above the data-less ones, whatever the brand order says.
        let materials = vec![
            material("abs-like", "Default", "ABS-like", &[]),
            material("standard", "Default", "Standard", &[]),
            material("blu", "Siraya Tech", "Blu", &["saturn-4"]),
            material("build", "Siraya Tech", "Build", &["saturn-4"]),
        ];
        let rows = derive_resin_view(
            &materials,
            &[],
            &[],
            Some(&printer("mars-5")),
            &BTreeSet::new(),
        );
        let ids: Vec<i32> = rows.iter().map(|r| r.id_index).collect();
        assert_eq!(ids, vec![2, 3, 0, 1]);
    }

    #[test]
    fn a_dataless_parent_with_a_calibrated_extension_is_not_dataless() {
        // The extension's blocks count for the whole resin: the pair of rows
        // ranks in the middle tier.
        let materials = vec![
            material("bare", "Alpha", "Bare", &[]),
            material("empty", "Zeta", "Empty", &[]),
        ];
        let extensions = vec![extension("empty", &["printer-b"])];
        let rows = derive_resin_view(
            &materials,
            &extensions,
            &[],
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        let ids: Vec<i32> = rows.iter().map(|r| r.id_index).collect();
        // "empty" (+ its extension row) leads despite sorting after "bare" by
        // brand.
        assert_eq!(ids, vec![1, 1, 0]);
        assert!(rows[1].is_extension);
    }

    #[test]
    fn an_extension_match_pulls_the_parent_into_the_matched_group() {
        let materials = vec![
            material("a-resin", "Alpha", "A", &["printer-b"]),
            material("z-resin", "Zeta", "Z", &["printer-b"]),
        ];
        let extensions = vec![extension("z-resin", &["printer-a"])];
        let rows = derive_resin_view(
            &materials,
            &extensions,
            &[],
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        // z-resin's extension covers our printer: parent + extension rows lead;
        // a-resin (unmatched) follows.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id_index, 1);
        assert!(!rows[0].is_extension);
        assert_eq!(rows[1].id_index, 1);
        assert!(rows[1].is_extension);
        assert_eq!(rows[2].id_index, 0);
    }

    #[test]
    fn extension_rows_sit_below_their_parent_with_a_derived_name() {
        let materials = vec![material("a-resin", "Alpha", "Standard", &["printer-a"])];
        let extensions = vec![extension("a-resin", &["printer-b"])];
        let printers = vec![
            printer_entry("printer-a", "Printer A"),
            printer_entry("printer-b", "Printer B"),
        ];
        let rows = derive_resin_view(
            &materials,
            &extensions,
            &printers,
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        assert_eq!(rows.len(), 2);
        let ext_row = &rows[1];
        assert!(ext_row.is_extension);
        assert!(ext_row.is_custom);
        assert_eq!(ext_row.name.as_str(), "Standard (extended)");
        assert_eq!(ext_row.brand.as_str(), "Alpha");
        // The extension row carries its own pills only.
        assert_eq!(
            row_pills(ext_row),
            vec![("Printer B".to_owned(), PillKind::Other)]
        );
    }

    #[test]
    fn pill_kinds_and_order_follow_the_default_printer_and_generic() {
        let materials = vec![material(
            "a-resin",
            "Alpha",
            "A",
            &["printer-z", "printer-a", "generic-machine", "printer-b"],
        )];
        let printers = vec![
            printer_entry("printer-a", "Printer A"),
            printer_entry("printer-b", "Printer B"),
            printer_entry("printer-z", "Printer Z"),
            printer_entry("generic-machine", "Generic Machine"),
        ];
        let rows = derive_resin_view(
            &materials,
            &[],
            &printers,
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        // Default printer first (green), generic next (yellow), the rest by
        // label (gray).
        assert_eq!(
            row_pills(&rows[0]),
            vec![
                ("Printer A".to_owned(), PillKind::OurPrinter),
                ("Generic".to_owned(), PillKind::Generic),
                ("Printer B".to_owned(), PillKind::Other),
                ("Printer Z".to_owned(), PillKind::Other),
            ]
        );
    }

    #[test]
    fn the_generic_default_printer_gets_the_our_printer_kind() {
        let materials = vec![material("a-resin", "Alpha", "A", &["generic-machine"])];
        let printers = vec![printer_entry("generic-machine", "Generic Machine")];
        let rows = derive_resin_view(
            &materials,
            &[],
            &printers,
            Some(&printer("generic-machine")),
            &BTreeSet::new(),
        );
        assert_eq!(
            row_pills(&rows[0]),
            vec![("Generic".to_owned(), PillKind::OurPrinter)]
        );
    }

    #[test]
    fn a_pill_falls_back_to_the_id_when_the_printer_is_unknown() {
        let materials = vec![material("a-resin", "Alpha", "A", &["gone-printer"])];
        let rows = derive_resin_view(
            &materials,
            &[],
            &[],
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        assert_eq!(
            row_pills(&rows[0]),
            vec![("gone-printer".to_owned(), PillKind::Other)]
        );
    }

    #[test]
    fn zero_block_materials_get_the_red_no_data_pill() {
        let materials = vec![material("a-resin", "Alpha", "A", &[])];
        let rows = derive_resin_view(
            &materials,
            &[],
            &[],
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        let pills = row_pills(&rows[0]);
        assert_eq!(pills.len(), 1);
        assert_eq!(pills[0].1, PillKind::NoData);
    }

    #[test]
    fn a_covering_extension_suppresses_the_parents_no_data_pill() {
        let materials = vec![material("a-resin", "Alpha", "A", &[])];
        let extensions = vec![extension("a-resin", &["printer-b"])];
        let rows = derive_resin_view(
            &materials,
            &extensions,
            &[],
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        // The resin has data (in the extension row); the parent row shows
        // neither pills nor the red marker.
        assert!(row_pills(&rows[0]).is_empty());
        assert_eq!(row_pills(&rows[1]).len(), 1);
    }

    #[test]
    fn without_a_default_printer_every_pill_is_dim() {
        let materials = vec![
            material("a-resin", "Alpha", "A", &["printer-a", "generic-machine"]),
            material("b-resin", "Beta", "B", &[]),
        ];
        let rows = derive_resin_view(&materials, &[], &[], None, &BTreeSet::new());
        for (_, kind) in rows.iter().flat_map(row_pills) {
            assert_eq!(kind, PillKind::Other);
        }
    }

    #[test]
    fn custom_flag_reflects_the_source() {
        let mut entry = material("a-resin", "Alpha", "A", &[]);
        entry.source = ProfileSource::User;
        let rows = derive_resin_view(&[entry], &[], &[], None, &BTreeSet::new());
        assert!(rows[0].is_custom);
        assert!(!rows[0].is_extension);
    }

    #[test]
    fn can_extend_reflects_uncovered_targets() {
        let materials = vec![material("a-resin", "Alpha", "A", &["printer-a"])];
        let extensions = vec![extension("a-resin", &["printer-b"])];
        let all_covered = vec![
            printer_entry("printer-a", "Printer A"),
            printer_entry("printer-b", "Printer B"),
        ];
        let rows = derive_resin_view(
            &materials,
            &extensions,
            &all_covered,
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        // Base plus extension cover every catalog printer: nothing left to
        // extend, on either row.
        assert!(!rows[0].can_extend);
        assert!(!rows[1].can_extend);

        let with_uncovered = vec![
            printer_entry("printer-a", "Printer A"),
            printer_entry("printer-b", "Printer B"),
            printer_entry("printer-c", "Printer C"),
        ];
        let rows = derive_resin_view(
            &materials,
            &extensions,
            &with_uncovered,
            Some(&printer("printer-a")),
            &BTreeSet::new(),
        );
        assert!(rows[0].can_extend);
        assert!(rows[1].can_extend);
    }

    #[test]
    fn favourite_material_marks_both_its_base_and_extension_rows() {
        let materials = vec![
            material("fav-resin", "Alpha", "Fav", &[]),
            material("plain-resin", "Beta", "Plain", &[]),
        ];
        let extensions = vec![extension("fav-resin", &["printer-a"])];
        let printers = vec![printer_entry("printer-a", "A")];
        let favourites: BTreeSet<MaterialId> = [MaterialId::new("fav-resin").unwrap()]
            .into_iter()
            .collect();

        let rows = derive_resin_view(&materials, &extensions, &printers, None, &favourites);

        // fav-resin is material index 0 and produces a base + an extension row,
        // both favourite; plain-resin (index 1) is not.
        assert_eq!(rows.iter().filter(|r| r.is_favourite).count(), 2);
        assert!(
            rows.iter()
                .filter(|r| r.id_index == 0)
                .all(|r| r.is_favourite)
        );
        assert!(
            rows.iter()
                .filter(|r| r.id_index == 1)
                .all(|r| !r.is_favourite)
        );
    }
}

#[cfg(test)]
mod build_card_data_tests {
    use std::collections::BTreeSet;

    use sc_print_primitives::{
        BuildVolume, FormatKind, NonEmptyString, OutputFormatBinding, OutputFormatBindings,
        PrinterId,
    };

    use super::*;

    fn printer_entry(id: &str) -> PrinterCatalogEntry {
        PrinterCatalogEntry {
            id: PrinterId::new(id).unwrap(),
            manufacturer: "Maker".to_owned(),
            name: id.to_owned(),
            build_volume: BuildVolume::new(100.0, 60.0, 80.0).unwrap(),
            output_formats: OutputFormatBindings::new(OutputFormatBinding {
                format: FormatKind::Goo,
                version: Some(NonEmptyString::new("v3.0").unwrap()),
            }),
            image_path: None,
            source: ProfileSource::Bundled,
        }
    }

    #[test]
    fn is_favourite_is_set_from_the_favourite_set() {
        let printers = vec![printer_entry("printer-a"), printer_entry("printer-b")];
        let favourites: BTreeSet<PrinterId> =
            [PrinterId::new("printer-b").unwrap()].into_iter().collect();

        let cards = build_card_data(&printers, &favourites);

        assert_eq!(cards.len(), 2);
        assert!(!cards[0].card.is_favourite, "printer-a is not a favourite");
        assert!(cards[1].card.is_favourite, "printer-b is a favourite");
    }

    #[test]
    fn no_favourites_leaves_every_card_unmarked() {
        let printers = vec![printer_entry("printer-a")];
        let cards = build_card_data(&printers, &BTreeSet::new());
        assert!(!cards[0].card.is_favourite);
    }
}

#[cfg(test)]
mod delete_flow_tests {
    use std::fs;

    use super::*;

    const MATERIAL_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"other\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]

[defaults]
";

    fn material_id(id: &str) -> MaterialId {
        MaterialId::new(id).unwrap()
    }

    fn write_material(root: &Path, id: &str) {
        let materials = root.join("materials");
        fs::create_dir_all(&materials).unwrap();
        let toml = MATERIAL_TOML.replace("brand-resin", id);
        fs::write(materials.join(format!("{id}.toml")), toml).unwrap();
    }

    #[test]
    fn the_default_is_orphaned_when_nothing_resolves_the_deleted_id() {
        let bundled = tempfile::tempdir().unwrap();
        let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::new(
            None,
            vec![bundled.path().to_owned()],
        ));
        let deleted = material_id("gone-resin");
        assert!(default_resin_orphaned(&catalog, Some(&deleted), &deleted));
    }

    #[test]
    fn the_default_survives_when_a_bundled_copy_still_resolves_the_id() {
        // Deleting the user override of a bundled id: the bundled copy still
        // resolves it after the catalog refresh.
        let bundled = tempfile::tempdir().unwrap();
        write_material(bundled.path(), "shared-resin");
        let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::new(
            None,
            vec![bundled.path().to_owned()],
        ));
        let deleted = material_id("shared-resin");
        assert!(!default_resin_orphaned(&catalog, Some(&deleted), &deleted));
    }

    #[test]
    fn an_unrelated_or_absent_default_is_never_unset() {
        let bundled = tempfile::tempdir().unwrap();
        let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::new(
            None,
            vec![bundled.path().to_owned()],
        ));
        let deleted = material_id("gone-resin");
        let other = material_id("other-resin");
        assert!(!default_resin_orphaned(&catalog, Some(&other), &deleted));
        assert!(!default_resin_orphaned(&catalog, None, &deleted));
    }
}

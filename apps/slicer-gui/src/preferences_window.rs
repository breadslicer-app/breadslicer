/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::i18n::ctr;
use crate::language::LanguagePreference;
use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::profile_catalog::{MaterialCatalogEntry, ProfileCatalog, ProfileSource};
use crate::window_prefs::WindowPreferences;
use crate::{PreferencesWindow, PrinterCard, ResinRow};

/// A printer tile plus the fields the manufacturer / search filter
/// needs but the Slint tile does not carry. Held once per session; the
/// visible `printer-cards` model is a filtered projection of these.
struct PrinterCardData {
    manufacturer: String,
    name_lower: String,
    card: PrinterCard,
}

/// Bundle of a `PreferencesWindow` and its bus subscription. Drop
/// drops the subscription so the listener is removed as soon as the
/// wrapped window is no longer held.
pub(crate) struct WiredPreferencesWindow {
    pub window: PreferencesWindow,
    _subscription: Subscription,
}

pub(crate) fn build_preferences_window(
    bus: &Rc<PreferencesBus>,
    catalog_handle: Rc<RefCell<Rc<ProfileCatalog>>>,
) -> Result<WiredPreferencesWindow, slint::PlatformError> {
    let prefs = PreferencesWindow::new()?;
    let catalog = catalog_handle.borrow().clone();

    // Static-per-session: the tiles and the manufacturer tab bar are
    // built once from the catalog. Thumbnails are decoded here.
    let all_cards = Rc::new(build_card_data(&catalog));
    let tabs = Rc::new(build_manufacturer_tabs(&catalog));
    prefs.set_manufacturer_tabs(ModelRc::new(VecModel::from(
        tabs.iter()
            .map(|s| SharedString::from(s.as_str()))
            .collect::<Vec<_>>(),
    )));
    set_visible_cards(&prefs, &all_cards, tab_manufacturer(&tabs, 0), "");

    // View-local filter state: which manufacturer tab and search text
    // narrow the visible tiles. Never reaches the bus.
    let current_tab = Rc::new(Cell::new(0i32));
    let search_lower = Rc::new(RefCell::new(String::new()));

    let snapshot = bus.snapshot();
    prefs.apply_preferences(&snapshot);
    apply_printing(&prefs, &snapshot, &catalog);

    let prefs_weak = prefs.as_weak();
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
                    &all_cards,
                    tab_manufacturer(&tabs, index),
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
                    &all_cards,
                    tab_manufacturer(&tabs, current_tab.get()),
                    &search_lower.borrow(),
                );
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
            let envelope = catalog.envelope_for_printer(Some(&id));
            bus.update(move |snap| {
                snap.default_printer = Some(id);
                snap.build_volume = envelope;
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

    // `create-calibration-requested`, `import-requested`, and the two
    // `export-*-requested` callbacks are declared and forwarded by the
    // pages; their handlers land in later commits (authoring, import /
    // export). Left unhandled here, they are no-ops.

    let subscription = bus.subscribe({
        let prefs_weak = prefs_weak.clone();
        let catalog_handle = catalog_handle.clone();
        move |snap, _change| {
            if let Some(w) = prefs_weak.upgrade() {
                w.apply_preferences(snap);
                let catalog = catalog_handle.borrow().clone();
                apply_printing(&w, snap, &catalog);
            }
        }
    });

    Ok(WiredPreferencesWindow {
        window: prefs,
        _subscription: subscription,
    })
}

/// Push the current selection (printer / resin indices, the derived
/// resin ordering, and whether a printer is chosen) into the window.
/// Does not touch the manufacturer / search filter, which is view-local.
fn apply_printing(
    window: &PreferencesWindow,
    snap: &PreferencesSnapshot,
    catalog: &ProfileCatalog,
) {
    let printer_index = snap
        .default_printer
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

    window.set_has_selected_printer(snap.default_printer.is_some());

    let rows = derive_resin_rows(catalog.materials(), snap.default_printer.as_ref());
    window.set_resin_rows(ModelRc::new(VecModel::from(rows)));
}

fn build_card_data(catalog: &ProfileCatalog) -> Vec<PrinterCardData> {
    catalog
        .printers()
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let (thumbnail, has_thumbnail) = match &entry.image_path {
                Some(path) => match Image::load_from_path(path.as_path()) {
                    Ok(image) => (image, true),
                    Err(err) => {
                        eprintln!(
                            "warning: failed to load printer thumbnail {}: {err}",
                            path.display()
                        );
                        (Image::default(), false)
                    }
                },
                None => (Image::default(), false),
            };
            PrinterCardData {
                manufacturer: entry.manufacturer.clone(),
                name_lower: entry.name.to_lowercase(),
                card: PrinterCard {
                    name: SharedString::from(entry.name.as_str()),
                    thumbnail,
                    has_thumbnail,
                    is_custom: entry.source == ProfileSource::User,
                    id_index: i as i32,
                },
            }
        })
        .collect()
}

/// The manufacturer tab labels: a translated "All" entry followed by
/// the catalog's distinct manufacturers.
fn build_manufacturer_tabs(catalog: &ProfileCatalog) -> Vec<String> {
    let mut tabs = Vec::with_capacity(catalog.manufacturers().len() + 1);
    tabs.push(ctr!("PreferencesPage", "All").to_string());
    tabs.extend(catalog.manufacturers());
    tabs
}

/// The manufacturer a tab index narrows to, or `None` for the leading
/// "All" tab (index 0).
fn tab_manufacturer(tabs: &[String], index: i32) -> Option<&str> {
    if index <= 0 {
        return None;
    }
    tabs.get(index as usize).map(|s| s.as_str())
}

/// Whether a card is visible under the current manufacturer tab and
/// search text. `tab_manufacturer` is `None` for the "All" tab;
/// `search_lower` is the already-lowercased search text.
fn card_visible(
    manufacturer: &str,
    name_lower: &str,
    tab_manufacturer: Option<&str>,
    search_lower: &str,
) -> bool {
    let manufacturer_ok = tab_manufacturer.is_none_or(|m| m == manufacturer);
    let search_ok = search_lower.is_empty() || name_lower.contains(search_lower);
    manufacturer_ok && search_ok
}

fn set_visible_cards(
    window: &PreferencesWindow,
    all: &[PrinterCardData],
    tab_manufacturer: Option<&str>,
    search_lower: &str,
) {
    let cards: Vec<PrinterCard> = all
        .iter()
        .filter(|c| {
            card_visible(
                &c.manufacturer,
                &c.name_lower,
                tab_manufacturer,
                search_lower,
            )
        })
        .map(|c| c.card.clone())
        .collect();
    window.set_printer_cards(ModelRc::new(VecModel::from(cards)));
}

/// Resin rows for the list, ordered calibrated-first relative to the
/// selected printer (uncalibrated last). With no printer selected,
/// nothing is calibrated and the base catalog order is preserved. Each
/// row keeps its catalog index in `id-index`.
fn derive_resin_rows(
    materials: &[MaterialCatalogEntry],
    selected_printer: Option<&sc_print_primitives::PrinterId>,
) -> Vec<ResinRow> {
    let mut calibrated_rows = Vec::new();
    let mut uncalibrated_rows = Vec::new();
    for (i, entry) in materials.iter().enumerate() {
        let calibrated = selected_printer.is_some_and(|p| entry.calibrated_printers.contains(p));
        let row = ResinRow {
            name: SharedString::from(entry.name.as_str()),
            brand: SharedString::from(entry.brand.as_str()),
            family: SharedString::from(entry.family.as_str()),
            calibrated,
            is_custom: entry.source == ProfileSource::User,
            id_index: i as i32,
        };
        if calibrated {
            calibrated_rows.push(row);
        } else {
            uncalibrated_rows.push(row);
        }
    }
    calibrated_rows.extend(uncalibrated_rows);
    calibrated_rows
}

#[cfg(test)]
mod card_visible_tests {
    use super::*;

    #[test]
    fn all_tab_with_empty_search_shows_every_card() {
        assert!(card_visible("Anycubic", "photon mono 4", None, ""));
        assert!(card_visible("Elegoo", "mars 5 ultra", None, ""));
    }

    #[test]
    fn manufacturer_tab_narrows_to_that_manufacturer() {
        assert!(card_visible(
            "Anycubic",
            "photon mono 4",
            Some("Anycubic"),
            ""
        ));
        assert!(!card_visible(
            "Elegoo",
            "mars 5 ultra",
            Some("Anycubic"),
            ""
        ));
    }

    #[test]
    fn search_matches_on_name_substring_case_insensitively() {
        assert!(card_visible("Anycubic", "photon mono 4", None, "mono"));
        assert!(!card_visible("Anycubic", "photon mono 4", None, "saturn"));
    }

    #[test]
    fn manufacturer_tab_and_search_compose() {
        assert!(card_visible(
            "Elegoo",
            "mars 5 ultra",
            Some("Elegoo"),
            "mars"
        ));
        // Right model, wrong manufacturer tab.
        assert!(!card_visible(
            "Anycubic",
            "mars 5 ultra",
            Some("Elegoo"),
            "mars"
        ));
        // Right manufacturer, wrong search.
        assert!(!card_visible(
            "Elegoo",
            "mars 5 ultra",
            Some("Elegoo"),
            "photon"
        ));
    }

    #[test]
    fn tab_manufacturer_maps_index_zero_to_all() {
        let tabs = vec!["All".to_owned(), "Anycubic".to_owned(), "Elegoo".to_owned()];
        assert_eq!(tab_manufacturer(&tabs, 0), None);
        assert_eq!(tab_manufacturer(&tabs, 1), Some("Anycubic"));
        assert_eq!(tab_manufacturer(&tabs, 2), Some("Elegoo"));
        assert_eq!(tab_manufacturer(&tabs, 9), None);
    }
}

#[cfg(test)]
mod derive_resin_rows_tests {
    use std::collections::BTreeSet;

    use sc_print_primitives::{MaterialId, PrinterId};

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
                .map(|p| PrinterId::new(p).unwrap())
                .collect::<BTreeSet<_>>(),
            source: ProfileSource::Bundled,
        }
    }

    fn printer(id: &str) -> PrinterId {
        PrinterId::new(id).unwrap()
    }

    #[test]
    fn no_selected_printer_keeps_base_order_and_no_calibration() {
        let materials = vec![
            material("a-resin", "Alpha", "A", &["printer-a"]),
            material("b-resin", "Beta", "B", &[]),
        ];
        let rows = derive_resin_rows(&materials, None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id_index, 0);
        assert_eq!(rows[1].id_index, 1);
        assert!(!rows[0].calibrated);
        assert!(!rows[1].calibrated);
    }

    #[test]
    fn calibrated_rows_are_ordered_first() {
        let materials = vec![
            material("a-resin", "Alpha", "A", &[]),
            material("b-resin", "Beta", "B", &["printer-a"]),
            material("c-resin", "Gamma", "C", &[]),
            material("d-resin", "Delta", "D", &["printer-a"]),
        ];
        let rows = derive_resin_rows(&materials, Some(&printer("printer-a")));
        // Calibrated (b, d) first in base order, then uncalibrated (a, c).
        let ids: Vec<i32> = rows.iter().map(|r| r.id_index).collect();
        assert_eq!(ids, vec![1, 3, 0, 2]);
        assert!(rows[0].calibrated);
        assert!(rows[1].calibrated);
        assert!(!rows[2].calibrated);
        assert!(!rows[3].calibrated);
    }

    #[test]
    fn custom_flag_reflects_the_source() {
        let mut entry = material("a-resin", "Alpha", "A", &[]);
        entry.source = ProfileSource::User;
        let rows = derive_resin_rows(&[entry], None);
        assert!(rows[0].is_custom);
    }
}

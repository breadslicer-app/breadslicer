/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Narrowing a Preferences catalog section to what its tab bar and
//! search box select.

use std::collections::BTreeSet;

use slint::{Image, Model, ModelRc, SharedString, VecModel};

use sc_print_primitives::PrinterId;

use crate::i18n::ctr;
use crate::preferences_bus::PreferencesSnapshot;
use crate::profile_catalog::{PrinterCatalogEntry, ProfileCatalog, ProfileSource};
use crate::{PreferencesWindow, PrinterCard};

/// A printer tile plus the fields the manufacturer / search filter
/// needs but the Slint tile does not carry. Held once per session; the
/// visible `printer-cards` model is a filtered projection of these.
pub(crate) struct PrinterCardData {
    pub(crate) manufacturer: String,
    pub(crate) name_lower: String,
    pub(crate) card: PrinterCard,
}

pub(crate) fn build_card_data(
    printers: &[PrinterCatalogEntry],
    favourite_printers: &BTreeSet<PrinterId>,
) -> Vec<PrinterCardData> {
    printers
        .iter()
        .enumerate()
        .map(|(i, entry)| PrinterCardData {
            manufacturer: entry.manufacturer.clone(),
            name_lower: entry.name.to_lowercase(),
            card: PrinterCard {
                // Thumbnails start blank and are filled in asynchronously
                // by `start_thumbnail_load` so opening the window never
                // blocks the UI thread on image decoding.
                name: SharedString::from(entry.name.as_str()),
                thumbnail: Image::default(),
                has_thumbnail: false,
                is_custom: entry.source == ProfileSource::User,
                is_favourite: favourite_printers.contains(&entry.id),
                id_index: i as i32,
            },
        })
        .collect()
}

/// The manufacturer tab labels: a translated "All" entry followed by
/// the catalog's distinct manufacturers.
pub(crate) fn build_manufacturer_tabs(catalog: &ProfileCatalog) -> Vec<String> {
    let mut tabs = Vec::with_capacity(catalog.manufacturers().len() + 1);
    tabs.push(ctr!("PreferencesPage", "All").to_string());
    tabs.extend(catalog.manufacturers());
    tabs
}

/// The manufacturer of the printer selected in `snap` (falling back to
/// the first-run default when nothing is persisted yet), or `None`
/// when the catalog has no such printer.
pub(crate) fn selected_manufacturer(
    catalog: &ProfileCatalog,
    snap: &PreferencesSnapshot,
) -> Option<String> {
    let id = catalog.effective_default_printer(snap.default_printer.as_ref())?;
    let index = catalog.printer_index(&id)?;
    Some(catalog.printers()[index].manufacturer.clone())
}

/// The tab index carrying `label`, or 0 (the "All" tab) when `label`
/// is `None` or not among the tabs. The leading "All" label is skipped
/// so a value spelled like it cannot match the wrong tab.
pub(crate) fn tab_index_for_label(tabs: &[String], label: Option<&str>) -> i32 {
    label
        .and_then(|wanted| tabs.iter().skip(1).position(|tab| tab == wanted))
        .map_or(0, |position| (position + 1) as i32)
}

/// The value a tab index narrows to, or `None` for the leading "All"
/// tab (index 0).
pub(crate) fn tab_label(tabs: &[String], index: i32) -> Option<&str> {
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

/// Position of the printer with catalog index `selected_id_index`
/// within `cards`, or -1 when the filter hides it. The gallery needs
/// this to scroll the selected tile into view: tile position within
/// the filtered model, not the catalog index, determines its row.
fn selected_visible_position(cards: &[PrinterCard], selected_id_index: i32) -> i32 {
    cards
        .iter()
        .position(|card| card.id_index == selected_id_index)
        .map_or(-1, |position| position as i32)
}

/// Pushes freshly decoded thumbnails into the visible gallery model by
/// replacing only the rows whose printer (identified by `id-index`) is
/// in `updated`. Row-level updates keep the gallery's scroll position;
/// replacing the whole model would reset it.
pub(crate) fn apply_thumbnails_to_visible(
    visible: &ModelRc<PrinterCard>,
    all: &[PrinterCardData],
    updated: &BTreeSet<i32>,
) {
    for row in 0..visible.row_count() {
        let Some(card) = visible.row_data(row) else {
            continue;
        };
        if updated.contains(&card.id_index) {
            visible.set_row_data(row, all[card.id_index as usize].card.clone());
        }
    }
}

pub(crate) fn set_visible_cards(
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
    window.set_selected_visible_printer_index(selected_visible_position(
        &cards,
        window.get_selected_printer_index(),
    ));
    window.set_printer_cards(ModelRc::new(VecModel::from(cards)));
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
    fn tab_label_maps_index_zero_to_all() {
        let tabs = vec!["All".to_owned(), "Anycubic".to_owned(), "Elegoo".to_owned()];
        assert_eq!(tab_label(&tabs, 0), None);
        assert_eq!(tab_label(&tabs, 1), Some("Anycubic"));
        assert_eq!(tab_label(&tabs, 2), Some("Elegoo"));
        assert_eq!(tab_label(&tabs, 9), None);
    }
}

#[cfg(test)]
mod initial_view_tests {
    use super::*;

    fn tabs() -> Vec<String> {
        vec!["All".to_owned(), "Anycubic".to_owned(), "Elegoo".to_owned()]
    }

    #[test]
    fn tab_index_for_label_finds_the_matching_tab() {
        assert_eq!(tab_index_for_label(&tabs(), Some("Anycubic")), 1);
        assert_eq!(tab_index_for_label(&tabs(), Some("Elegoo")), 2);
    }

    #[test]
    fn tab_index_for_label_falls_back_to_all() {
        assert_eq!(tab_index_for_label(&tabs(), None), 0);
        assert_eq!(tab_index_for_label(&tabs(), Some("Phrozen")), 0);
    }

    #[test]
    fn tab_index_for_label_never_matches_the_all_label() {
        // A manufacturer sharing the localized "All" label must not
        // resolve to the unfiltered tab.
        assert_eq!(tab_index_for_label(&tabs(), Some("All")), 0);
    }

    fn card(id_index: i32) -> PrinterCard {
        PrinterCard {
            id_index,
            ..PrinterCard::default()
        }
    }

    #[test]
    fn selected_visible_position_maps_catalog_index_to_filtered_position() {
        let cards = vec![card(4), card(7), card(9)];
        assert_eq!(selected_visible_position(&cards, 4), 0);
        assert_eq!(selected_visible_position(&cards, 9), 2);
    }

    #[test]
    fn selected_visible_position_is_negative_when_hidden_or_unselected() {
        let cards = vec![card(4), card(7)];
        assert_eq!(selected_visible_position(&cards, 5), -1);
        assert_eq!(selected_visible_position(&cards, -1), -1);
        assert_eq!(selected_visible_position(&[], 0), -1);
    }
}

#[cfg(test)]
mod apply_thumbnails_tests {
    use super::*;

    fn card_data(id_index: i32, has_thumbnail: bool) -> PrinterCardData {
        PrinterCardData {
            manufacturer: String::new(),
            name_lower: String::new(),
            card: PrinterCard {
                id_index,
                has_thumbnail,
                ..PrinterCard::default()
            },
        }
    }

    #[test]
    fn only_the_updated_visible_rows_are_replaced() {
        let all = vec![card_data(0, true), card_data(1, false), card_data(2, true)];
        // Card 0 is filtered out of the visible model; its stale
        // pre-update clones are what the model still holds.
        let visible: ModelRc<PrinterCard> = ModelRc::new(VecModel::from(vec![
            PrinterCard {
                id_index: 1,
                ..PrinterCard::default()
            },
            PrinterCard {
                id_index: 2,
                ..PrinterCard::default()
            },
        ]));

        let updated = BTreeSet::from([0, 2]);
        apply_thumbnails_to_visible(&visible, &all, &updated);

        assert!(!visible.row_data(0).unwrap().has_thumbnail);
        assert!(visible.row_data(1).unwrap().has_thumbnail);
    }
}

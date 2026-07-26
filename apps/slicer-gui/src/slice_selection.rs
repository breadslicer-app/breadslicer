/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Deriving the Slice-section printer/resin picker lists.
//!
//! The Slice panel never shows the whole catalog: its list is the
//! union of the global favourites, the globally-configured default,
//! and the scene's current choice, so the currently-selected profile
//! is always present and selectable. The pure logic here builds that
//! list from `(id, label)` candidates, orders it, and resolves which
//! entry the panel prefills (the scene's choice, else the global
//! default).

use std::collections::BTreeSet;

use sc_print_primitives::{MaterialId, PositiveFiniteMm, PrinterId};

use crate::profile_catalog::{MaterialCatalogEntry, PrinterCatalogEntry};

/// One entry in a picker: the profile id (the stable handle carried
/// back through the pick callback) and the label shown in the
/// dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickerOption<Id> {
    pub id: Id,
    pub label: String,
}

/// A picker's rendered state: the ordered options and the index of
/// the prefilled selection, or `-1` when nothing is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickerModel<Id> {
    pub options: Vec<PickerOption<Id>>,
    pub selected: i32,
}

/// Builds a picker list from `(id, label)` candidates. Keeps only ids
/// that are a favourite, the global `default`, or the `current`
/// choice; orders them case-insensitively by label (id as tiebreaker
/// for determinism); and selects the prefill, which is the `current`
/// choice when set, otherwise the global `default`. The selection is
/// `-1` when the prefill is absent or is an id the candidates do not
/// carry.
///
/// Candidates are assumed unique by id (the catalog guarantees it).
/// A wanted id the candidates do not carry (an unknown favourite,
/// default, or stale scene choice) is silently dropped: without a
/// catalog entry there is no label to render.
pub(crate) fn picker_model<Id>(
    candidates: &[(Id, String)],
    favourites: &BTreeSet<Id>,
    default: Option<&Id>,
    current: Option<&Id>,
) -> PickerModel<Id>
where
    Id: Ord + Clone,
{
    let mut wanted: BTreeSet<Id> = favourites.clone();
    if let Some(default) = default {
        wanted.insert(default.clone());
    }
    if let Some(current) = current {
        wanted.insert(current.clone());
    }

    let mut options: Vec<PickerOption<Id>> = candidates
        .iter()
        .filter(|(id, _)| wanted.contains(id))
        .map(|(id, label)| PickerOption {
            id: id.clone(),
            label: label.clone(),
        })
        .collect();
    options.sort_by(|a, b| {
        a.label
            .to_lowercase()
            .cmp(&b.label.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    let prefill = current.or(default);
    let selected = prefill
        .and_then(|prefill| options.iter().position(|option| &option.id == prefill))
        .map_or(-1, |index| index as i32);

    PickerModel { options, selected }
}

/// The label shown for a printer: `"Manufacturer Name"`, or just the
/// name when the manufacturer is blank.
fn printer_label(manufacturer: &str, name: &str) -> String {
    if manufacturer.is_empty() {
        name.to_string()
    } else {
        format!("{manufacturer} {name}")
    }
}

/// The label shown for a resin: `"Brand Name"`, or just the name when
/// the brand is blank.
fn resin_label(brand: &str, name: &str) -> String {
    if brand.is_empty() {
        name.to_string()
    } else {
        format!("{brand} {name}")
    }
}

/// The printer candidates for [`picker_model`], one `(id, label)` per
/// catalog entry.
pub(crate) fn printer_candidates(entries: &[PrinterCatalogEntry]) -> Vec<(PrinterId, String)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.id.clone(),
                printer_label(&entry.manufacturer, &entry.name),
            )
        })
        .collect()
}

/// The resin candidates for [`picker_model`], one `(id, label)` per
/// catalog entry.
pub(crate) fn resin_candidates(entries: &[MaterialCatalogEntry]) -> Vec<(MaterialId, String)> {
    entries
        .iter()
        .map(|entry| (entry.id.clone(), resin_label(&entry.brand, &entry.name)))
        .collect()
}

/// The outcome of parsing the Slice panel's first-layer-height field.
/// Distinguishes an empty field (clear the per-print override) from a
/// non-empty but unparseable one (leave the current value untouched).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FirstLayerHeightInput {
    /// The field is empty: drop any per-print first-layer height.
    Clear,
    /// A valid positive, finite millimetre value.
    Set(PositiveFiniteMm),
    /// Non-empty but not a positive finite number: ignore the edit.
    Invalid,
}

/// Parses the first-layer-height field text. Surrounding whitespace is
/// ignored; an empty field clears the override, a positive finite
/// number sets it, and anything else is
/// [`FirstLayerHeightInput::Invalid`].
pub(crate) fn parse_first_layer_height(text: &str) -> FirstLayerHeightInput {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return FirstLayerHeightInput::Clear;
    }
    match trimmed.parse::<f64>() {
        Ok(value) => match PositiveFiniteMm::new(value) {
            Ok(mm) => FirstLayerHeightInput::Set(mm),
            Err(_) => FirstLayerHeightInput::Invalid,
        },
        Err(_) => FirstLayerHeightInput::Invalid,
    }
}

/// Formats a first-layer height for the panel field: the bare
/// millimetre number, or an empty string when no per-print height is
/// set.
pub(crate) fn format_first_layer_height(height: Option<PositiveFiniteMm>) -> String {
    height.map_or(String::new(), |mm| mm.value().to_string())
}

#[cfg(test)]
mod picker_model_tests {
    use super::*;

    fn id(value: &str) -> String {
        value.to_string()
    }

    fn candidates(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, label)| ((*id).to_string(), (*label).to_string()))
            .collect()
    }

    fn favourites(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn only_favourites_are_listed_when_there_is_no_default_or_current() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie")]);
        let model = picker_model(&cands, &favourites(&["a", "c"]), None, None);

        let ids: Vec<_> = model.options.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids, vec![id("a"), id("c")]);
        assert_eq!(model.selected, -1);
    }

    #[test]
    fn the_default_and_current_join_the_list_even_when_not_favourites() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie")]);
        let model = picker_model(&cands, &favourites(&["a"]), Some(&id("b")), Some(&id("c")));

        let ids: Vec<_> = model.options.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids, vec![id("a"), id("b"), id("c")]);
    }

    #[test]
    fn an_id_that_is_favourite_and_selected_appears_once() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo")]);
        let model = picker_model(&cands, &favourites(&["a"]), Some(&id("a")), Some(&id("a")));

        assert_eq!(model.options.len(), 1);
        assert_eq!(model.options[0].id, id("a"));
    }

    #[test]
    fn options_are_ordered_case_insensitively_by_label() {
        let cands = candidates(&[("z", "alpha"), ("y", "Bravo"), ("x", "Charlie")]);
        let model = picker_model(&cands, &favourites(&["x", "y", "z"]), None, None);

        let labels: Vec<_> = model.options.iter().map(|o| o.label.clone()).collect();
        assert_eq!(labels, vec!["alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn a_wanted_id_the_catalog_does_not_carry_is_dropped() {
        let cands = candidates(&[("a", "Alpha")]);
        let model = picker_model(
            &cands,
            &favourites(&["a", "ghost"]),
            Some(&id("gone")),
            None,
        );

        let ids: Vec<_> = model.options.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids, vec![id("a")]);
        assert_eq!(model.selected, -1);
    }

    #[test]
    fn the_current_choice_is_selected_over_the_default() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo")]);
        let model = picker_model(&cands, &favourites(&[]), Some(&id("a")), Some(&id("b")));

        let selected = &model.options[model.selected as usize];
        assert_eq!(selected.id, id("b"));
    }

    #[test]
    fn the_default_is_selected_when_there_is_no_current_choice() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo")]);
        let model = picker_model(&cands, &favourites(&["a", "b"]), Some(&id("a")), None);

        let selected = &model.options[model.selected as usize];
        assert_eq!(selected.id, id("a"));
    }

    #[test]
    fn an_empty_catalog_yields_an_empty_unselected_model() {
        let model = picker_model::<String>(&[], &favourites(&["a"]), Some(&id("a")), None);
        assert!(model.options.is_empty());
        assert_eq!(model.selected, -1);
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn printer_label_joins_manufacturer_and_name() {
        assert_eq!(printer_label("Anycubic", "Photon"), "Anycubic Photon");
    }

    #[test]
    fn printer_label_falls_back_to_name_without_a_manufacturer() {
        assert_eq!(printer_label("", "Photon"), "Photon");
    }

    #[test]
    fn resin_label_joins_brand_and_name() {
        assert_eq!(resin_label("Elegoo", "Standard"), "Elegoo Standard");
    }

    #[test]
    fn resin_label_falls_back_to_name_without_a_brand() {
        assert_eq!(resin_label("", "Standard"), "Standard");
    }
}

#[cfg(test)]
mod first_layer_height_tests {
    use super::*;

    fn mm(value: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(value).expect("valid length")
    }

    #[test]
    fn an_empty_field_clears_the_override() {
        assert_eq!(parse_first_layer_height(""), FirstLayerHeightInput::Clear);
        assert_eq!(
            parse_first_layer_height("   "),
            FirstLayerHeightInput::Clear,
        );
    }

    #[test]
    fn a_positive_number_is_parsed_ignoring_surrounding_whitespace() {
        assert_eq!(
            parse_first_layer_height("  0.05 "),
            FirstLayerHeightInput::Set(mm(0.05)),
        );
    }

    #[test]
    fn zero_and_negative_values_are_invalid() {
        assert_eq!(
            parse_first_layer_height("0"),
            FirstLayerHeightInput::Invalid
        );
        assert_eq!(
            parse_first_layer_height("-0.1"),
            FirstLayerHeightInput::Invalid,
        );
    }

    #[test]
    fn non_numeric_text_is_invalid() {
        assert_eq!(
            parse_first_layer_height("abc"),
            FirstLayerHeightInput::Invalid,
        );
        assert_eq!(
            parse_first_layer_height("0.0."),
            FirstLayerHeightInput::Invalid,
        );
    }

    #[test]
    fn an_absent_height_formats_as_empty() {
        assert_eq!(format_first_layer_height(None), "");
    }

    #[test]
    fn a_height_round_trips_through_format_and_parse() {
        let text = format_first_layer_height(Some(mm(0.05)));
        assert_eq!(text, "0.05");
        assert_eq!(
            parse_first_layer_height(&text),
            FirstLayerHeightInput::Set(mm(0.05)),
        );
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Slice panel's view: the printer and resin pickers, the format dropdown
//! and the first-layer-height field, derived from the scene's own choices, the
//! app-wide preferences and the catalog.

use std::collections::{BTreeMap, BTreeSet};

use sc_print_primitives::{MaterialId, OutputFormatBinding, PositiveFiniteMm, PrinterId};

use crate::print_format::{PrintFormatModel, print_format_model};
use crate::print_selection::{OwnOrDefault, PrintDefaults, PrintSelection};
use crate::profile_catalog::{MaterialCatalogEntry, PrinterCatalogEntry, ProfileCatalog};

/// An entry in a picker: the profile id, which the pick callback carries back,
/// and the label that the dropdown shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickerOption<Id> {
    pub id: Id,
    pub label: String,
}

/// A picker's rendered state: the ordered options and the index of the selected
/// one, or `-1` when nothing is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickerModel<Id> {
    pub options: Vec<PickerOption<Id>>,
    pub selected: i32,
}

impl<Id> PickerModel<Id> {
    /// Returns the selected option, or `None` when nothing is selected.
    pub(crate) fn selected_option(&self) -> Option<&PickerOption<Id>> {
        usize::try_from(self.selected)
            .ok()
            .and_then(|index| self.options.get(index))
    }
}

/// What the Slice panel shows for a scene. The format dropdown belongs to the
/// printer that the printer picker selects.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SlicePanelView {
    printer: PickerModel<PrinterId>,
    resin: PickerModel<MaterialId>,
    print_format: PrintFormatModel,
    first_layer_height: Option<PositiveFiniteMm>,
}

impl SlicePanelView {
    pub(crate) fn printer(&self) -> &PickerModel<PrinterId> {
        &self.printer
    }

    pub(crate) fn resin(&self) -> &PickerModel<MaterialId> {
        &self.resin
    }

    pub(crate) fn print_format(&self) -> &PrintFormatModel {
        &self.print_format
    }

    pub(crate) fn first_layer_height(&self) -> Option<PositiveFiniteMm> {
        self.first_layer_height
    }
}

/// Builds what the Slice panel shows for a scene. Each picker selects the
/// scene's own choice, else the default, and selects nothing for an id that the
/// catalog does not hold. The format dropdown offers the formats of the
/// selected printer, and restores the scene's own binding, else the binding
/// remembered for the printer in force.
pub(crate) fn slice_panel_view(
    own: &PrintSelection,
    defaults: &PrintDefaults,
    remembered_formats: &BTreeMap<PrinterId, OutputFormatBinding>,
    favourite_printers: &BTreeSet<PrinterId>,
    favourite_resins: &BTreeSet<MaterialId>,
    catalog: &ProfileCatalog,
) -> SlicePanelView {
    let selection = own.in_force(defaults);
    let printer = picker_model(
        &printer_candidates(catalog.printers()),
        favourite_printers,
        selection.printer(),
    );
    let resin = picker_model(
        &resin_candidates(catalog.materials()),
        favourite_resins,
        selection.resin(),
    );

    let shown_printer = printer.selected_option().map(|option| &option.id);
    let declared = shown_printer
        .and_then(|id| catalog.printer_index(id))
        .map(|index| &catalog.printers()[index].output_formats);
    let print_format = declared.map_or_else(PrintFormatModel::nothing_declared, |declared| {
        print_format_model(declared, selection.format_to_restore(remembered_formats))
    });

    SlicePanelView {
        printer,
        resin,
        print_format,
        first_layer_height: selection.first_layer_height(),
    }
}

/// Builds a picker list from `(id, label)` candidates. It keeps only the ids
/// that are a favourite, the app-wide default or the scene's own choice; orders
/// them by label without case, with the id as tiebreak; and selects the choice
/// in force. The selection is `-1` when no choice is in force, or when the
/// candidates do not carry it.
///
/// The candidates must be unique by id, as the catalog guarantees. A wanted id
/// that the candidates do not carry (an unknown favourite, default or scene
/// choice) is dropped: without a catalog entry, there is no label to show.
fn picker_model<Id>(
    candidates: &[(Id, String)],
    favourites: &BTreeSet<Id>,
    choice: &OwnOrDefault<Id>,
) -> PickerModel<Id>
where
    Id: Ord + Clone,
{
    let mut wanted: BTreeSet<Id> = favourites.clone();
    if let Some(default) = &choice.default {
        wanted.insert(default.clone());
    }
    if let Some(own) = &choice.own {
        wanted.insert(own.clone());
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

    let selected = choice
        .in_force()
        .and_then(|in_force| options.iter().position(|option| &option.id == in_force))
        .map_or(-1, |index| index as i32);

    PickerModel { options, selected }
}

/// Returns the label of a printer: `"Manufacturer Name"`, or the name alone
/// when the manufacturer is blank.
fn printer_label(manufacturer: &str, name: &str) -> String {
    if manufacturer.is_empty() {
        name.to_string()
    } else {
        format!("{manufacturer} {name}")
    }
}

/// Returns the label of a resin: `"Brand Name"`, or the name alone when the
/// brand is blank.
fn resin_label(brand: &str, name: &str) -> String {
    if brand.is_empty() {
        name.to_string()
    } else {
        format!("{brand} {name}")
    }
}

/// Returns the printer candidates for [`picker_model`]: 1 `(id, label)` per
/// catalog entry.
fn printer_candidates(entries: &[PrinterCatalogEntry]) -> Vec<(PrinterId, String)> {
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

/// Returns the resin candidates for [`picker_model`]: 1 `(id, label)` per
/// catalog entry.
fn resin_candidates(entries: &[MaterialCatalogEntry]) -> Vec<(MaterialId, String)> {
    entries
        .iter()
        .map(|entry| (entry.id.clone(), resin_label(&entry.brand, &entry.name)))
        .collect()
}

/// The outcome of parsing the Slice panel's first-layer-height field.
/// Distinguishes an empty field (clear the per-print override) from a non-empty
/// but unparseable one (leave the current value untouched).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FirstLayerHeightInput {
    /// The field is empty: drop any per-print first-layer height.
    Clear,
    /// A valid positive, finite millimetre value.
    Set(PositiveFiniteMm),
    /// Non-empty but not a positive finite number: ignore the edit.
    Invalid,
}

/// Parses the first-layer-height field text. Surrounding whitespace is ignored;
/// an empty field clears the override, a positive finite number sets it, and
/// anything else is [`FirstLayerHeightInput::Invalid`].
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

/// Formats a first-layer height for the panel field: the bare millimetre
/// number, or an empty string when no per-print height is set.
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

    fn choice(own: Option<&str>, default: Option<&str>) -> OwnOrDefault<String> {
        OwnOrDefault {
            own: own.map(id),
            default: default.map(id),
        }
    }

    #[test]
    fn only_favourites_are_listed_when_there_is_no_default_or_current() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie")]);
        let model = picker_model(&cands, &favourites(&["a", "c"]), &choice(None, None));

        let ids: Vec<_> = model.options.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids, vec![id("a"), id("c")]);
        assert_eq!(model.selected, -1);
    }

    #[test]
    fn the_default_and_current_join_the_list_even_when_not_favourites() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie")]);
        let model = picker_model(&cands, &favourites(&["a"]), &choice(Some("c"), Some("b")));

        let ids: Vec<_> = model.options.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids, vec![id("a"), id("b"), id("c")]);
    }

    #[test]
    fn an_id_that_is_favourite_and_selected_appears_once() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo")]);
        let model = picker_model(&cands, &favourites(&["a"]), &choice(Some("a"), Some("a")));

        assert_eq!(model.options.len(), 1);
        assert_eq!(model.options[0].id, id("a"));
    }

    #[test]
    fn options_are_ordered_case_insensitively_by_label() {
        let cands = candidates(&[("z", "alpha"), ("y", "Bravo"), ("x", "Charlie")]);
        let model = picker_model(&cands, &favourites(&["x", "y", "z"]), &choice(None, None));

        let labels: Vec<_> = model.options.iter().map(|o| o.label.clone()).collect();
        assert_eq!(labels, vec!["alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn a_wanted_id_the_catalog_does_not_carry_is_dropped() {
        let cands = candidates(&[("a", "Alpha")]);
        let model = picker_model(
            &cands,
            &favourites(&["a", "ghost"]),
            &choice(None, Some("gone")),
        );

        let ids: Vec<_> = model.options.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids, vec![id("a")]);
        assert_eq!(model.selected, -1);
    }

    #[test]
    fn the_current_choice_is_selected_over_the_default() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo")]);
        let model = picker_model(&cands, &favourites(&[]), &choice(Some("b"), Some("a")));

        let selected = &model.options[model.selected as usize];
        assert_eq!(selected.id, id("b"));
    }

    #[test]
    fn the_default_is_selected_when_there_is_no_current_choice() {
        let cands = candidates(&[("a", "Alpha"), ("b", "Bravo")]);
        let model = picker_model(&cands, &favourites(&["a", "b"]), &choice(None, Some("a")));

        let selected = &model.options[model.selected as usize];
        assert_eq!(selected.id, id("a"));
    }

    #[test]
    fn an_empty_catalog_yields_an_empty_unselected_model() {
        let model = picker_model::<String>(&[], &favourites(&["a"]), &choice(None, Some("a")));
        assert!(model.options.is_empty());
        assert_eq!(model.selected, -1);
    }
}

#[cfg(test)]
mod selected_option_tests {
    use super::*;

    fn model(selected: i32) -> PickerModel<&'static str> {
        PickerModel {
            options: vec![
                PickerOption {
                    id: "a",
                    label: "Alpha".to_owned(),
                },
                PickerOption {
                    id: "b",
                    label: "Bravo".to_owned(),
                },
            ],
            selected,
        }
    }

    #[test]
    fn an_index_in_range_gives_its_option() {
        assert_eq!(
            model(1).selected_option().map(|option| option.id),
            Some("b")
        );
    }

    #[test]
    fn no_selection_gives_no_option() {
        assert_eq!(model(-1).selected_option(), None);
    }

    #[test]
    fn an_index_past_the_end_gives_no_option() {
        assert_eq!(model(2).selected_option(), None);
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

#[cfg(test)]
mod slice_panel_view_tests {
    use sc_print_primitives::{BuildVolume, FormatKind, NonEmptyString, OutputFormatBindings};

    use super::*;
    use crate::profile_catalog::ProfileSource;

    fn printer_id(id: &str) -> PrinterId {
        PrinterId::new(id).expect("valid printer id")
    }

    fn resin_id(id: &str) -> MaterialId {
        MaterialId::new(id).expect("valid resin id")
    }

    /// Declares 2 revisions of 1 family, so its dropdown preselects the
    /// automatic entry of that family.
    fn ctb_printer() -> PrinterId {
        printer_id("ctb-printer")
    }

    /// Declares 2 families, so its dropdown preselects nothing.
    fn two_family_printer() -> PrinterId {
        printer_id("two-family-printer")
    }

    fn ghost_printer() -> PrinterId {
        printer_id("ghost-printer")
    }

    fn resin_a() -> MaterialId {
        resin_id("resin-a")
    }

    fn resin_b() -> MaterialId {
        resin_id("resin-b")
    }

    fn ghost_resin() -> MaterialId {
        resin_id("ghost-resin")
    }

    fn binding(format: FormatKind, version: Option<&str>) -> OutputFormatBinding {
        OutputFormatBinding {
            format,
            version: version.map(|v| NonEmptyString::new(v).expect("test version is non-empty")),
        }
    }

    fn ctb_v5() -> OutputFormatBinding {
        binding(FormatKind::Ctb, Some("v5"))
    }

    fn ctb_v5enc() -> OutputFormatBinding {
        binding(FormatKind::Ctb, Some("v5enc"))
    }

    fn goo_v3_0() -> OutputFormatBinding {
        binding(FormatKind::Goo, Some("v3.0"))
    }

    fn mm(value: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(value).expect("valid length")
    }

    fn printer_entry(
        id: PrinterId,
        name: &str,
        formats: Vec<OutputFormatBinding>,
    ) -> PrinterCatalogEntry {
        PrinterCatalogEntry {
            id,
            manufacturer: String::new(),
            name: name.to_owned(),
            build_volume: BuildVolume::new(100.0, 60.0, 150.0).expect("valid build volume"),
            output_formats: OutputFormatBindings::try_from_vec(formats)
                .expect("test list is non-empty"),
            image_path: None,
            source: ProfileSource::Bundled,
        }
    }

    fn resin_entry(id: MaterialId, name: &str) -> MaterialCatalogEntry {
        MaterialCatalogEntry {
            id,
            brand: String::new(),
            name: name.to_owned(),
            family: String::new(),
            calibrated_printers: BTreeSet::new(),
            source: ProfileSource::Bundled,
        }
    }

    fn catalog() -> ProfileCatalog {
        ProfileCatalog::from_entries(
            vec![
                printer_entry(ctb_printer(), "Alpha", vec![ctb_v5(), ctb_v5enc()]),
                printer_entry(two_family_printer(), "Bravo", vec![goo_v3_0(), ctb_v5enc()]),
            ],
            vec![
                resin_entry(resin_a(), "Alpha Resin"),
                resin_entry(resin_b(), "Bravo Resin"),
            ],
        )
    }

    #[derive(Default)]
    struct Inputs {
        own: PrintSelection,
        defaults: PrintDefaults,
        remembered: BTreeMap<PrinterId, OutputFormatBinding>,
        favourite_printers: BTreeSet<PrinterId>,
        favourite_resins: BTreeSet<MaterialId>,
    }

    impl Inputs {
        fn view(&self) -> SlicePanelView {
            slice_panel_view(
                &self.own,
                &self.defaults,
                &self.remembered,
                &self.favourite_printers,
                &self.favourite_resins,
                &catalog(),
            )
        }
    }

    fn own_printer(printer: PrinterId) -> PrintSelection {
        PrintSelection {
            printer: Some(printer),
            ..PrintSelection::default()
        }
    }

    fn default_printer(printer: PrinterId) -> PrintDefaults {
        PrintDefaults {
            printer: Some(printer),
            resin: None,
        }
    }

    fn default_resin(resin: MaterialId) -> PrintDefaults {
        PrintDefaults {
            printer: None,
            resin: Some(resin),
        }
    }

    fn option_ids<Id: Clone>(model: &PickerModel<Id>) -> Vec<Id> {
        model
            .options
            .iter()
            .map(|option| option.id.clone())
            .collect()
    }

    fn selected_id<Id>(model: &PickerModel<Id>) -> Option<&Id> {
        model.selected_option().map(|option| &option.id)
    }

    fn offered(model: &PrintFormatModel) -> Vec<OutputFormatBinding> {
        model
            .options
            .iter()
            .map(|option| option.binding.clone())
            .collect()
    }

    fn selected_binding(model: &PrintFormatModel) -> Option<&OutputFormatBinding> {
        let index = usize::try_from(model.selected).ok()?;
        model.options.get(index).map(|option| &option.binding)
    }

    #[test]
    fn an_installed_own_printer_is_selected_and_the_default_is_listed() {
        let view = Inputs {
            own: own_printer(ctb_printer()),
            defaults: default_printer(two_family_printer()),
            ..Inputs::default()
        }
        .view();

        assert_eq!(
            option_ids(view.printer()),
            vec![ctb_printer(), two_family_printer()],
        );
        assert_eq!(selected_id(view.printer()), Some(&ctb_printer()));
    }

    #[test]
    fn without_an_own_printer_the_default_is_selected_with_its_formats() {
        let view = Inputs {
            defaults: default_printer(two_family_printer()),
            ..Inputs::default()
        }
        .view();

        assert_eq!(selected_id(view.printer()), Some(&two_family_printer()));
        assert_eq!(offered(view.print_format()), vec![goo_v3_0(), ctb_v5enc()]);
    }

    #[test]
    fn an_own_printer_the_catalog_lacks_gives_no_selection_and_no_formats() {
        let view = Inputs {
            own: own_printer(ghost_printer()),
            defaults: default_printer(ctb_printer()),
            remembered: BTreeMap::from([(ctb_printer(), ctb_v5())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(option_ids(view.printer()), vec![ctb_printer()]);
        assert_eq!(view.printer().selected, -1);
        assert_eq!(view.print_format(), &PrintFormatModel::nothing_declared());
    }

    #[test]
    fn a_format_remembered_for_an_own_printer_the_catalog_lacks_is_not_offered() {
        let view = Inputs {
            own: own_printer(ghost_printer()),
            remembered: BTreeMap::from([(ghost_printer(), ctb_v5())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(view.print_format(), &PrintFormatModel::nothing_declared());
    }

    #[test]
    fn an_own_format_the_printer_offers_is_selected_over_the_remembered_one() {
        let view = Inputs {
            own: PrintSelection {
                printer: Some(ctb_printer()),
                output_format: Some(ctb_v5()),
                ..PrintSelection::default()
            },
            remembered: BTreeMap::from([(ctb_printer(), ctb_v5enc())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(selected_binding(view.print_format()), Some(&ctb_v5()));
    }

    #[test]
    fn without_an_own_format_the_format_remembered_for_the_own_printer_is_selected() {
        let view = Inputs {
            own: own_printer(ctb_printer()),
            remembered: BTreeMap::from([(ctb_printer(), ctb_v5())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(selected_binding(view.print_format()), Some(&ctb_v5()));
    }

    #[test]
    fn without_an_own_printer_the_format_remembered_for_the_default_is_selected() {
        let view = Inputs {
            defaults: default_printer(ctb_printer()),
            remembered: BTreeMap::from([(ctb_printer(), ctb_v5())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(selected_binding(view.print_format()), Some(&ctb_v5()));
    }

    #[test]
    fn an_own_format_a_1_family_printer_lacks_hides_the_remembered_one() {
        let view = Inputs {
            own: PrintSelection {
                printer: Some(ctb_printer()),
                output_format: Some(goo_v3_0()),
                ..PrintSelection::default()
            },
            remembered: BTreeMap::from([(ctb_printer(), ctb_v5())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(
            selected_binding(view.print_format()),
            Some(&binding(FormatKind::Ctb, None)),
        );
    }

    #[test]
    fn an_own_format_a_2_family_printer_lacks_hides_the_remembered_one() {
        let view = Inputs {
            own: PrintSelection {
                printer: Some(two_family_printer()),
                output_format: Some(ctb_v5()),
                ..PrintSelection::default()
            },
            remembered: BTreeMap::from([(two_family_printer(), goo_v3_0())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(view.print_format().selected, -1);
    }

    #[test]
    fn a_format_remembered_for_the_default_printer_is_not_used_for_an_own_printer() {
        // The own printer offers that format too.
        let view = Inputs {
            own: own_printer(two_family_printer()),
            defaults: default_printer(ctb_printer()),
            remembered: BTreeMap::from([(ctb_printer(), ctb_v5enc())]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(view.print_format().selected, -1);
    }

    #[test]
    fn with_no_printer_and_no_resin_the_favourites_are_listed_and_nothing_is_selected() {
        let view = Inputs {
            favourite_printers: BTreeSet::from([ctb_printer()]),
            favourite_resins: BTreeSet::from([resin_a()]),
            ..Inputs::default()
        }
        .view();

        assert_eq!(option_ids(view.printer()), vec![ctb_printer()]);
        assert_eq!(view.printer().selected, -1);
        assert_eq!(option_ids(view.resin()), vec![resin_a()]);
        assert_eq!(view.resin().selected, -1);
        assert_eq!(view.print_format(), &PrintFormatModel::nothing_declared());
    }

    #[test]
    fn the_first_layer_height_is_the_own_value() {
        let own = PrintSelection {
            first_layer_height: Some(mm(0.05)),
            ..PrintSelection::default()
        };
        let view = Inputs {
            own,
            ..Inputs::default()
        }
        .view();

        assert_eq!(view.first_layer_height(), Some(mm(0.05)));
        assert_eq!(Inputs::default().view().first_layer_height(), None);
    }

    #[test]
    fn a_default_printer_the_catalog_lacks_gives_no_selection_and_no_formats() {
        let view = Inputs {
            defaults: default_printer(ghost_printer()),
            ..Inputs::default()
        }
        .view();

        assert_eq!(view.printer().selected, -1);
        assert_eq!(view.print_format(), &PrintFormatModel::nothing_declared());
    }

    #[test]
    fn an_installed_own_resin_is_selected_and_the_default_resin_is_listed() {
        let view = Inputs {
            own: PrintSelection {
                resin: Some(resin_a()),
                ..PrintSelection::default()
            },
            defaults: default_resin(resin_b()),
            ..Inputs::default()
        }
        .view();

        assert_eq!(option_ids(view.resin()), vec![resin_a(), resin_b()]);
        assert_eq!(selected_id(view.resin()), Some(&resin_a()));
    }

    #[test]
    fn without_an_own_resin_the_default_resin_is_selected() {
        let view = Inputs {
            defaults: default_resin(resin_b()),
            ..Inputs::default()
        }
        .view();

        assert_eq!(selected_id(view.resin()), Some(&resin_b()));
    }

    #[test]
    fn an_own_resin_the_catalog_lacks_gives_no_selection() {
        let view = Inputs {
            own: PrintSelection {
                resin: Some(ghost_resin()),
                ..PrintSelection::default()
            },
            defaults: default_resin(resin_a()),
            ..Inputs::default()
        }
        .view();

        assert_eq!(option_ids(view.resin()), vec![resin_a()]);
        assert_eq!(view.resin().selected, -1);
    }

    #[test]
    fn a_default_resin_the_catalog_lacks_gives_no_selection() {
        let view = Inputs {
            defaults: default_resin(ghost_resin()),
            ..Inputs::default()
        }
        .view();

        assert_eq!(view.resin().selected, -1);
    }
}

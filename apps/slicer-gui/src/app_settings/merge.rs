/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The merge of the settings that 1 process changed into the settings file as
//! other processes may have left it.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    AppSettings, Appearance, ObjectListSettings, SliceControlPanelSettings, SliceViewSettings,
};

/// Returns `theirs`, the file as it is now, with the changes from `base` to
/// `ours`: a value that `ours` changed takes the value of `ours`, any other
/// keeps the value of `theirs`. Sets and maps merge per element and per key.
/// `theirs` must be a usable file, so all 3 have the current schema version.
pub(super) fn merge(base: &AppSettings, ours: &AppSettings, theirs: AppSettings) -> AppSettings {
    let AppSettings {
        version,
        appearance,
        default_printer,
        default_resin,
        favourite_printers,
        favourite_resins,
        output_format_by_printer,
        aa_level,
        prescan_supports,
        smooth_models,
        frame_rate,
        gizmo_style,
        gizmo_size,
        orthographic_view,
        silenced_messages,
        object_list,
        slice_panel,
        slice_view,
        last_open_dir,
    } = theirs;
    AppSettings {
        version,
        appearance: merge_appearance(&base.appearance, &ours.appearance, appearance),
        default_printer: pick(
            &base.default_printer,
            &ours.default_printer,
            default_printer,
        ),
        default_resin: pick(&base.default_resin, &ours.default_resin, default_resin),
        favourite_printers: merge_set(
            &base.favourite_printers,
            &ours.favourite_printers,
            favourite_printers,
        ),
        favourite_resins: merge_set(
            &base.favourite_resins,
            &ours.favourite_resins,
            favourite_resins,
        ),
        output_format_by_printer: merge_map(
            &base.output_format_by_printer,
            &ours.output_format_by_printer,
            output_format_by_printer,
        ),
        aa_level: pick(&base.aa_level, &ours.aa_level, aa_level),
        prescan_supports: pick(
            &base.prescan_supports,
            &ours.prescan_supports,
            prescan_supports,
        ),
        smooth_models: pick(&base.smooth_models, &ours.smooth_models, smooth_models),
        frame_rate: pick(&base.frame_rate, &ours.frame_rate, frame_rate),
        gizmo_style: pick(&base.gizmo_style, &ours.gizmo_style, gizmo_style),
        gizmo_size: pick(&base.gizmo_size, &ours.gizmo_size, gizmo_size),
        orthographic_view: pick(
            &base.orthographic_view,
            &ours.orthographic_view,
            orthographic_view,
        ),
        silenced_messages: merge_set(
            &base.silenced_messages,
            &ours.silenced_messages,
            silenced_messages,
        ),
        object_list: merge_object_list(&base.object_list, &ours.object_list, object_list),
        slice_panel: merge_slice_panel(&base.slice_panel, &ours.slice_panel, slice_panel),
        slice_view: merge_slice_view(&base.slice_view, &ours.slice_view, slice_view),
        last_open_dir: pick(&base.last_open_dir, &ours.last_open_dir, last_open_dir),
    }
}

fn merge_appearance(base: &Appearance, ours: &Appearance, theirs: Appearance) -> Appearance {
    let Appearance {
        theme,
        language,
        menu_layout,
    } = theirs;
    Appearance {
        theme: pick(&base.theme, &ours.theme, theme),
        language: pick(&base.language, &ours.language, language),
        menu_layout: pick(&base.menu_layout, &ours.menu_layout, menu_layout),
    }
}

fn merge_object_list(
    base: &ObjectListSettings,
    ours: &ObjectListSettings,
    theirs: ObjectListSettings,
) -> ObjectListSettings {
    let ObjectListSettings {
        visible,
        anchor_right,
        anchor_bottom,
        offset_x,
        offset_y,
        width,
        height,
        minimized,
    } = theirs;
    ObjectListSettings {
        visible: pick(&base.visible, &ours.visible, visible),
        anchor_right: pick(&base.anchor_right, &ours.anchor_right, anchor_right),
        anchor_bottom: pick(&base.anchor_bottom, &ours.anchor_bottom, anchor_bottom),
        offset_x: pick(&base.offset_x, &ours.offset_x, offset_x),
        offset_y: pick(&base.offset_y, &ours.offset_y, offset_y),
        width: pick(&base.width, &ours.width, width),
        height: pick(&base.height, &ours.height, height),
        minimized: pick(&base.minimized, &ours.minimized, minimized),
    }
}

fn merge_slice_panel(
    base: &SliceControlPanelSettings,
    ours: &SliceControlPanelSettings,
    theirs: SliceControlPanelSettings,
) -> SliceControlPanelSettings {
    let SliceControlPanelSettings {
        anchor_right,
        anchor_bottom,
        offset_x,
        offset_y,
        width,
        height,
    } = theirs;
    SliceControlPanelSettings {
        anchor_right: pick(&base.anchor_right, &ours.anchor_right, anchor_right),
        anchor_bottom: pick(&base.anchor_bottom, &ours.anchor_bottom, anchor_bottom),
        offset_x: pick(&base.offset_x, &ours.offset_x, offset_x),
        offset_y: pick(&base.offset_y, &ours.offset_y, offset_y),
        width: pick(&base.width, &ours.width, width),
        height: pick(&base.height, &ours.height, height),
    }
}

fn merge_slice_view(
    base: &SliceViewSettings,
    ours: &SliceViewSettings,
    theirs: SliceViewSettings,
) -> SliceViewSettings {
    let SliceViewSettings {
        visible,
        anchor_right,
        anchor_bottom,
        offset_x,
        offset_y,
        width,
        height,
        minimized,
    } = theirs;
    SliceViewSettings {
        visible: pick(&base.visible, &ours.visible, visible),
        anchor_right: pick(&base.anchor_right, &ours.anchor_right, anchor_right),
        anchor_bottom: pick(&base.anchor_bottom, &ours.anchor_bottom, anchor_bottom),
        offset_x: pick(&base.offset_x, &ours.offset_x, offset_x),
        offset_y: pick(&base.offset_y, &ours.offset_y, offset_y),
        width: pick(&base.width, &ours.width, width),
        height: pick(&base.height, &ours.height, height),
        minimized: pick(&base.minimized, &ours.minimized, minimized),
    }
}

/// Returns `ours` when it differs from `base`, else `theirs`.
fn pick<T: Clone + PartialEq>(base: &T, ours: &T, theirs: T) -> T {
    if ours == base { theirs } else { ours.clone() }
}

/// Returns `theirs` with the elements that `ours` added to `base`, and without
/// the ones that `ours` removed.
fn merge_set<T: Clone + Ord>(
    base: &BTreeSet<T>,
    ours: &BTreeSet<T>,
    mut theirs: BTreeSet<T>,
) -> BTreeSet<T> {
    theirs.extend(ours.difference(base).cloned());
    for removed in base.difference(ours) {
        theirs.remove(removed);
    }
    theirs
}

/// Returns `theirs` with the entries that `ours` added to `base` or changed,
/// and without the keys that `ours` removed.
fn merge_map<K: Clone + Ord, V: Clone + PartialEq>(
    base: &BTreeMap<K, V>,
    ours: &BTreeMap<K, V>,
    mut theirs: BTreeMap<K, V>,
) -> BTreeMap<K, V> {
    for (key, value) in ours {
        if base.get(key) != Some(value) {
            theirs.insert(key.clone(), value.clone());
        }
    }
    for key in base.keys() {
        if !ours.contains_key(key) {
            theirs.remove(key);
        }
    }
    theirs
}

#[cfg(test)]
mod merge_tests {
    use sc_print_primitives::{AaLevel, FormatKind, OutputFormatBinding, PrinterId};

    use super::*;
    use crate::ThemePreference;
    use crate::app_settings::{LanguageCode, LanguageSetting};

    fn printer(id: &str) -> PrinterId {
        PrinterId::new(id).unwrap()
    }

    fn printers(ids: &[&str]) -> BTreeSet<PrinterId> {
        ids.iter().map(|id| printer(id)).collect()
    }

    fn formats(entries: &[(&str, FormatKind)]) -> BTreeMap<PrinterId, OutputFormatBinding> {
        entries
            .iter()
            .map(|(id, format)| {
                let binding = OutputFormatBinding {
                    format: format.clone(),
                    version: None,
                };
                (printer(id), binding)
            })
            .collect()
    }

    #[test]
    fn nothing_changed_by_us_gives_theirs() {
        let base = AppSettings::default();
        let mut theirs = AppSettings::default();
        theirs.set_theme(ThemePreference::Dark);
        theirs.set_favourite_printers(printers(&["printer-y"]));

        assert_eq!(merge(&base, &base.clone(), theirs.clone()), theirs);
    }

    #[test]
    fn a_value_that_we_changed_takes_ours() {
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);

        let merged = merge(&base, &ours, AppSettings::default());
        assert!(matches!(merged.theme(), ThemePreference::Dark));
    }

    #[test]
    fn a_value_that_only_they_changed_keeps_theirs() {
        let base = AppSettings::default();
        let mut theirs = AppSettings::default();
        theirs.set_aa_level(AaLevel::X4);

        let merged = merge(&base, &base.clone(), theirs);
        assert_eq!(merged.aa_level(), AaLevel::X4);
    }

    #[test]
    fn a_value_that_both_changed_takes_ours() {
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);
        let mut theirs = AppSettings::default();
        theirs.set_theme(ThemePreference::Light);

        let merged = merge(&base, &ours, theirs);
        assert!(matches!(merged.theme(), ThemePreference::Dark));
    }

    #[test]
    fn an_element_added_by_each_side_stays() {
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_favourite_printers(printers(&["printer-x"]));
        let mut theirs = AppSettings::default();
        theirs.set_favourite_printers(printers(&["printer-y"]));

        let merged = merge(&base, &ours, theirs);
        assert_eq!(
            merged.favourite_printers(),
            printers(&["printer-x", "printer-y"])
        );
    }

    #[test]
    fn showing_all_messages_again_keeps_a_message_silenced_meanwhile_elsewhere() {
        let mut base = AppSettings::default();
        base.set_silenced_messages(["repair.summary".to_owned()].into_iter().collect());
        let mut ours = base.clone();
        ours.set_silenced_messages(BTreeSet::new());
        let mut theirs = base.clone();
        theirs.set_silenced_messages(
            ["repair.summary".to_owned(), "supports.generated".to_owned()]
                .into_iter()
                .collect(),
        );

        let merged = merge(&base, &ours, theirs);
        assert_eq!(
            merged.silenced_messages(),
            ["supports.generated".to_owned()].into_iter().collect()
        );
    }

    #[test]
    fn an_element_that_we_removed_goes() {
        let mut base = AppSettings::default();
        base.set_favourite_printers(printers(&["printer-x", "printer-y"]));
        let mut ours = base.clone();
        ours.set_favourite_printers(printers(&["printer-y"]));
        let mut theirs = AppSettings::default();
        theirs.set_favourite_printers(printers(&["printer-x", "printer-y", "printer-z"]));

        let merged = merge(&base, &ours, theirs);
        assert_eq!(
            merged.favourite_printers(),
            printers(&["printer-y", "printer-z"])
        );
    }

    #[test]
    fn a_key_that_we_added_or_changed_takes_ours() {
        let mut base = AppSettings::default();
        base.set_output_format_by_printer(formats(&[("printer-a", FormatKind::Ctb)]));
        let mut ours = base.clone();
        ours.set_output_format_by_printer(formats(&[
            ("printer-a", FormatKind::Goo),
            ("printer-b", FormatKind::Sl1),
        ]));
        let mut theirs = AppSettings::default();
        theirs.set_output_format_by_printer(formats(&[
            ("printer-a", FormatKind::Ctb),
            ("printer-c", FormatKind::Pwmx),
        ]));

        let merged = merge(&base, &ours, theirs);
        assert_eq!(
            merged.output_format_by_printer(),
            formats(&[
                ("printer-a", FormatKind::Goo),
                ("printer-b", FormatKind::Sl1),
                ("printer-c", FormatKind::Pwmx),
            ])
        );
    }

    #[test]
    fn a_key_that_we_removed_goes() {
        let mut base = AppSettings::default();
        base.set_output_format_by_printer(formats(&[
            ("printer-a", FormatKind::Ctb),
            ("printer-b", FormatKind::Sl1),
        ]));
        let mut ours = base.clone();
        ours.set_output_format_by_printer(formats(&[("printer-b", FormatKind::Sl1)]));
        let mut theirs = AppSettings::default();
        theirs.set_output_format_by_printer(formats(&[
            ("printer-a", FormatKind::Ctb),
            ("printer-b", FormatKind::Goo),
        ]));

        let merged = merge(&base, &ours, theirs);
        assert_eq!(
            merged.output_format_by_printer(),
            formats(&[("printer-b", FormatKind::Goo)])
        );
    }

    #[test]
    fn an_unbundled_language_stays_when_we_did_not_change_the_language() {
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);
        let mut theirs = AppSettings::default();
        let xx = LanguageCode::new("xx".to_owned()).unwrap();
        theirs.appearance.language = LanguageSetting::Code(xx.clone());

        let merged = merge(&base, &ours, theirs);
        assert_eq!(merged.appearance.language, LanguageSetting::Code(xx));
        assert!(matches!(merged.theme(), ThemePreference::Dark));
    }

    #[test]
    fn two_fields_of_one_panel_keep_both_changes() {
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_object_list_offset_x(Some(48.0));
        let mut theirs = AppSettings::default();
        theirs.set_object_list_width(Some(300.0));

        let merged = merge(&base, &ours, theirs);
        assert_eq!(merged.object_list_offset_x(), Some(48.0));
        assert_eq!(merged.object_list_width(), Some(300.0));
    }
}

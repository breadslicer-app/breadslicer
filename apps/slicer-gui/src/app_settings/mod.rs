/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The application settings that persist between sessions, and their layout in
//! the settings file, which [`file`] reads and writes. Pre-alpha policy: a
//! schema change bumps [`APP_SETTINGS_SCHEMA_VERSION`], with no migration code.

pub(crate) mod file;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use sc_print_primitives::{AaLevel, MaterialId, OutputFormatBinding, PrinterId};
use serde::{Deserialize, Serialize};

use crate::language::LanguagePreference;
use crate::{MenuLayout, ThemePreference};

/// On-disk representation of `ThemePreference`. Separate type because the
/// Slint-generated `ThemePreference` isn't serde-aware; we also want lowercase
/// variants for a friendlier TOML surface.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum ThemeSetting {
    #[default]
    System,
    Light,
    Dark,
}

impl From<ThemePreference> for ThemeSetting {
    fn from(value: ThemePreference) -> Self {
        match value {
            ThemePreference::Light => ThemeSetting::Light,
            ThemePreference::Dark => ThemeSetting::Dark,
            _ => ThemeSetting::System,
        }
    }
}

impl From<ThemeSetting> for ThemePreference {
    fn from(value: ThemeSetting) -> Self {
        match value {
            ThemeSetting::Light => ThemePreference::Light,
            ThemeSetting::Dark => ThemePreference::Dark,
            ThemeSetting::System => ThemePreference::System,
        }
    }
}

/// On-disk representation of `MenuLayout`. Separate type because the
/// Slint-generated `MenuLayout` isn't serde-aware; lowercase variants give a
/// friendlier TOML surface.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MenuLayoutSetting {
    #[default]
    Bar,
    Compact,
}

impl MenuLayoutSetting {
    /// `Bar` is the default; skipped when serializing so a fresh config stays
    /// minimal.
    fn is_bar(&self) -> bool {
        matches!(self, MenuLayoutSetting::Bar)
    }
}

impl From<MenuLayout> for MenuLayoutSetting {
    fn from(value: MenuLayout) -> Self {
        match value {
            MenuLayout::Bar => MenuLayoutSetting::Bar,
            MenuLayout::Compact => MenuLayoutSetting::Compact,
        }
    }
}

impl From<MenuLayoutSetting> for MenuLayout {
    fn from(value: MenuLayoutSetting) -> Self {
        match value {
            MenuLayoutSetting::Bar => MenuLayout::Bar,
            MenuLayoutSetting::Compact => MenuLayout::Compact,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Appearance {
    #[serde(default)]
    theme: ThemeSetting,
    /// Omitted in TOML when the user is on `System` (the default) so a fresh
    /// config stays minimal.
    #[serde(default, skip_serializing_if = "LanguagePreference::is_system")]
    language: LanguagePreference,
    /// Omitted in TOML when the user is on `Bar` (the default).
    #[serde(default, skip_serializing_if = "MenuLayoutSetting::is_bar")]
    menu_layout: MenuLayoutSetting,
}

/// On-disk state of the floating object-list panel. Serde fills any missing
/// field from `Default`, and the whole section is omitted while it holds its
/// default (see the `object_list` field on `AppSettings`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub(crate) struct ObjectListSettings {
    /// Whether the panel is shown.
    visible: bool,
    /// Which window edges the panel hugs; the default is the top-right corner.
    anchor_right: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    anchor_bottom: bool,
    /// Distance from the anchored edges to the matching panel edges, in logical
    /// pixels. Absent until the user drags the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset_y: Option<f32>,
    /// Panel size in logical pixels. Absent until the user resizes the panel;
    /// `height` absent means auto-sizing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<f32>,
    /// Whether the panel is collapsed to its title bar.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    minimized: bool,
}

impl Default for ObjectListSettings {
    fn default() -> Self {
        Self {
            visible: true,
            anchor_right: true,
            anchor_bottom: false,
            offset_x: None,
            offset_y: None,
            width: None,
            height: None,
            minimized: false,
        }
    }
}

impl ObjectListSettings {
    /// True when every field holds its default, so the whole section can be
    /// omitted from the serialized TOML.
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// On-disk position and size of the floating Slice control panel. Serde fills
/// any missing field from `Default`, and the whole section is omitted while it
/// holds its default (see the `slice_panel` field on `AppSettings`). The panel
/// has no visibility or minimized state: it is section-gated, not
/// toggle-hidden, so only geometry is stored.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub(crate) struct SliceControlPanelSettings {
    /// Which window edges the panel hugs; the default is the top-left corner
    /// (both false).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    anchor_right: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    anchor_bottom: bool,
    /// Distance from the anchored edges to the matching panel edges, in logical
    /// pixels. Absent until the user drags the panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    offset_x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset_y: Option<f32>,
    /// Panel size in logical pixels. Absent until the user resizes the panel;
    /// `height` absent means auto-sizing to the content.
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<f32>,
}

impl SliceControlPanelSettings {
    /// True when every field holds its default, so the whole section can be
    /// omitted from the serialized TOML.
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// On-disk state of the floating slice-view panel. Serde fills any missing
/// field from `Default`, and the whole section is omitted while it holds its
/// default (see the `slice_view` field on `AppSettings`). Unlike the Slice
/// control panel this panel is toggle-hidden as well as section-gated, so it
/// carries a visibility and minimized state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub(crate) struct SliceViewSettings {
    /// Whether the panel is shown.
    visible: bool,
    /// Which window edges the panel hugs; the default is the bottom-left corner
    /// (`anchor_right` false, `anchor_bottom` true). Both are always serialized
    /// when the section is present so a non-default bottom anchor round-trips
    /// regardless of value.
    anchor_right: bool,
    anchor_bottom: bool,
    /// Distance from the anchored edges to the matching panel edges, in logical
    /// pixels. Absent until the user drags the panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    offset_x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset_y: Option<f32>,
    /// Panel size in logical pixels. Absent until the user resizes the panel;
    /// `height` absent means auto-sizing.
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<f32>,
    /// Whether the panel is collapsed to its title bar.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    minimized: bool,
}

impl Default for SliceViewSettings {
    fn default() -> Self {
        Self {
            visible: true,
            anchor_right: false,
            anchor_bottom: true,
            offset_x: None,
            offset_y: None,
            width: None,
            height: None,
            minimized: false,
        }
    }
}

impl SliceViewSettings {
    /// True when every field holds its default, so the whole section can be
    /// omitted from the serialized TOML.
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Current schema version of the on-disk settings file. Bump this when the
/// structure of `AppSettings` (or one of its nested settings types) changes in
/// a way that an older or newer build cannot read. On load, a file whose
/// version differs from this constant is replaced with a fresh defaults file;
/// no migration code is kept.
pub(crate) const APP_SETTINGS_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    APP_SETTINGS_SCHEMA_VERSION
}

/// The anti-aliasing default (`Off`) as a free function, so the `aa_level`
/// field can use `#[serde(default = ...)]` without the primitive `AaLevel`
/// needing a `Default` impl.
fn default_aa_level() -> AaLevel {
    AaLevel::Off
}

/// True when the anti-aliasing level is the default (`Off`); the field is then
/// omitted from the serialized TOML so a fresh config stays minimal.
fn is_default_aa_level(level: &AaLevel) -> bool {
    *level == AaLevel::Off
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AppSettings {
    /// On-disk schema version. Files predating this field are read with the
    /// current version filled in by serde (treated as compatible); once
    /// written, the field is always present.
    #[serde(default = "default_schema_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub appearance: Appearance,
    /// The app-wide default printer, by id. Omitted in TOML until the user
    /// picks one; the viewport envelope is derived from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_printer: Option<PrinterId>,
    /// The app-wide default resin, by id. Omitted in TOML until the user picks
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_resin: Option<MaterialId>,
    /// The user's favourite printers, by id. A subset of the installed printers
    /// surfaced in the Slice-section picker. Omitted in TOML while empty.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    favourite_printers: BTreeSet<PrinterId>,
    /// The user's favourite resins, by id. Omitted in TOML while empty.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    favourite_resins: BTreeSet<MaterialId>,
    /// The printer file last chosen for each printer, so switching printers
    /// restores that machine's own choice instead of carrying another one's.
    /// Holds the binding as the printer profile spells it; an entry naming a
    /// format the profile no longer declares is ignored rather than pruned.
    /// Omitted in TOML while empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    output_format_by_printer: BTreeMap<PrinterId, OutputFormatBinding>,
    /// The app-wide anti-aliasing level applied when rasterizing slices.
    /// Omitted in TOML while it holds the default (`Off`).
    #[serde(
        default = "default_aa_level",
        skip_serializing_if = "is_default_aa_level"
    )]
    aa_level: AaLevel,
    /// Floating object-list panel state. Omitted in TOML while every field
    /// holds its default.
    #[serde(default, skip_serializing_if = "ObjectListSettings::is_default")]
    object_list: ObjectListSettings,
    /// Floating Slice control panel position and size. Omitted in TOML while
    /// every field holds its default.
    #[serde(default, skip_serializing_if = "SliceControlPanelSettings::is_default")]
    slice_panel: SliceControlPanelSettings,
    /// Floating slice-view panel state. Omitted in TOML while every field holds
    /// its default.
    #[serde(default, skip_serializing_if = "SliceViewSettings::is_default")]
    slice_view: SliceViewSettings,
    /// Directory the file-open dialog starts in: the folder the last opened
    /// file lived in. Omitted in TOML until the user opens a file; the dialog
    /// then falls back to the home directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_open_dir: Option<PathBuf>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: APP_SETTINGS_SCHEMA_VERSION,
            appearance: Appearance::default(),
            default_printer: None,
            default_resin: None,
            favourite_printers: BTreeSet::new(),
            favourite_resins: BTreeSet::new(),
            output_format_by_printer: BTreeMap::new(),
            aa_level: default_aa_level(),
            object_list: ObjectListSettings::default(),
            slice_panel: SliceControlPanelSettings::default(),
            slice_view: SliceViewSettings::default(),
            last_open_dir: None,
        }
    }
}

impl AppSettings {
    pub(crate) fn theme(&self) -> ThemePreference {
        self.appearance.theme.into()
    }

    pub(crate) fn set_theme(&mut self, theme: ThemePreference) {
        self.appearance.theme = theme.into();
    }

    pub(crate) fn language(&self) -> LanguagePreference {
        self.appearance.language
    }

    pub(crate) fn set_language(&mut self, language: LanguagePreference) {
        self.appearance.language = language;
    }

    pub(crate) fn menu_layout(&self) -> MenuLayout {
        self.appearance.menu_layout.into()
    }

    pub(crate) fn set_menu_layout(&mut self, menu_layout: MenuLayout) {
        self.appearance.menu_layout = menu_layout.into();
    }

    pub(crate) fn default_printer(&self) -> Option<PrinterId> {
        self.default_printer.clone()
    }

    pub(crate) fn set_default_printer(&mut self, id: Option<PrinterId>) {
        self.default_printer = id;
    }

    pub(crate) fn default_resin(&self) -> Option<MaterialId> {
        self.default_resin.clone()
    }

    pub(crate) fn set_default_resin(&mut self, id: Option<MaterialId>) {
        self.default_resin = id;
    }

    pub(crate) fn favourite_printers(&self) -> BTreeSet<PrinterId> {
        self.favourite_printers.clone()
    }

    pub(crate) fn set_favourite_printers(&mut self, ids: BTreeSet<PrinterId>) {
        self.favourite_printers = ids;
    }

    pub(crate) fn favourite_resins(&self) -> BTreeSet<MaterialId> {
        self.favourite_resins.clone()
    }

    pub(crate) fn set_favourite_resins(&mut self, ids: BTreeSet<MaterialId>) {
        self.favourite_resins = ids;
    }

    pub(crate) fn output_format_by_printer(&self) -> BTreeMap<PrinterId, OutputFormatBinding> {
        self.output_format_by_printer.clone()
    }

    pub(crate) fn set_output_format_by_printer(
        &mut self,
        formats: BTreeMap<PrinterId, OutputFormatBinding>,
    ) {
        self.output_format_by_printer = formats;
    }

    pub(crate) fn aa_level(&self) -> AaLevel {
        self.aa_level
    }

    pub(crate) fn set_aa_level(&mut self, level: AaLevel) {
        self.aa_level = level;
    }

    pub(crate) fn object_list_visible(&self) -> bool {
        self.object_list.visible
    }

    pub(crate) fn set_object_list_visible(&mut self, visible: bool) {
        self.object_list.visible = visible;
    }

    pub(crate) fn object_list_anchor_right(&self) -> bool {
        self.object_list.anchor_right
    }

    pub(crate) fn set_object_list_anchor_right(&mut self, anchor_right: bool) {
        self.object_list.anchor_right = anchor_right;
    }

    pub(crate) fn object_list_anchor_bottom(&self) -> bool {
        self.object_list.anchor_bottom
    }

    pub(crate) fn set_object_list_anchor_bottom(&mut self, anchor_bottom: bool) {
        self.object_list.anchor_bottom = anchor_bottom;
    }

    pub(crate) fn object_list_offset_x(&self) -> Option<f32> {
        self.object_list.offset_x
    }

    pub(crate) fn set_object_list_offset_x(&mut self, offset_x: Option<f32>) {
        self.object_list.offset_x = offset_x;
    }

    pub(crate) fn object_list_offset_y(&self) -> Option<f32> {
        self.object_list.offset_y
    }

    pub(crate) fn set_object_list_offset_y(&mut self, offset_y: Option<f32>) {
        self.object_list.offset_y = offset_y;
    }

    pub(crate) fn object_list_width(&self) -> Option<f32> {
        self.object_list.width
    }

    pub(crate) fn set_object_list_width(&mut self, width: Option<f32>) {
        self.object_list.width = width;
    }

    pub(crate) fn object_list_height(&self) -> Option<f32> {
        self.object_list.height
    }

    pub(crate) fn set_object_list_height(&mut self, height: Option<f32>) {
        self.object_list.height = height;
    }

    pub(crate) fn object_list_minimized(&self) -> bool {
        self.object_list.minimized
    }

    pub(crate) fn set_object_list_minimized(&mut self, minimized: bool) {
        self.object_list.minimized = minimized;
    }

    pub(crate) fn slice_panel_anchor_right(&self) -> bool {
        self.slice_panel.anchor_right
    }

    pub(crate) fn set_slice_panel_anchor_right(&mut self, anchor_right: bool) {
        self.slice_panel.anchor_right = anchor_right;
    }

    pub(crate) fn slice_panel_anchor_bottom(&self) -> bool {
        self.slice_panel.anchor_bottom
    }

    pub(crate) fn set_slice_panel_anchor_bottom(&mut self, anchor_bottom: bool) {
        self.slice_panel.anchor_bottom = anchor_bottom;
    }

    pub(crate) fn slice_panel_offset_x(&self) -> Option<f32> {
        self.slice_panel.offset_x
    }

    pub(crate) fn set_slice_panel_offset_x(&mut self, offset_x: Option<f32>) {
        self.slice_panel.offset_x = offset_x;
    }

    pub(crate) fn slice_panel_offset_y(&self) -> Option<f32> {
        self.slice_panel.offset_y
    }

    pub(crate) fn set_slice_panel_offset_y(&mut self, offset_y: Option<f32>) {
        self.slice_panel.offset_y = offset_y;
    }

    pub(crate) fn slice_panel_width(&self) -> Option<f32> {
        self.slice_panel.width
    }

    pub(crate) fn set_slice_panel_width(&mut self, width: Option<f32>) {
        self.slice_panel.width = width;
    }

    pub(crate) fn slice_panel_height(&self) -> Option<f32> {
        self.slice_panel.height
    }

    pub(crate) fn set_slice_panel_height(&mut self, height: Option<f32>) {
        self.slice_panel.height = height;
    }

    pub(crate) fn slice_view_visible(&self) -> bool {
        self.slice_view.visible
    }

    pub(crate) fn set_slice_view_visible(&mut self, visible: bool) {
        self.slice_view.visible = visible;
    }

    pub(crate) fn slice_view_anchor_right(&self) -> bool {
        self.slice_view.anchor_right
    }

    pub(crate) fn set_slice_view_anchor_right(&mut self, anchor_right: bool) {
        self.slice_view.anchor_right = anchor_right;
    }

    pub(crate) fn slice_view_anchor_bottom(&self) -> bool {
        self.slice_view.anchor_bottom
    }

    pub(crate) fn set_slice_view_anchor_bottom(&mut self, anchor_bottom: bool) {
        self.slice_view.anchor_bottom = anchor_bottom;
    }

    pub(crate) fn slice_view_offset_x(&self) -> Option<f32> {
        self.slice_view.offset_x
    }

    pub(crate) fn set_slice_view_offset_x(&mut self, offset_x: Option<f32>) {
        self.slice_view.offset_x = offset_x;
    }

    pub(crate) fn slice_view_offset_y(&self) -> Option<f32> {
        self.slice_view.offset_y
    }

    pub(crate) fn set_slice_view_offset_y(&mut self, offset_y: Option<f32>) {
        self.slice_view.offset_y = offset_y;
    }

    pub(crate) fn slice_view_width(&self) -> Option<f32> {
        self.slice_view.width
    }

    pub(crate) fn set_slice_view_width(&mut self, width: Option<f32>) {
        self.slice_view.width = width;
    }

    pub(crate) fn slice_view_height(&self) -> Option<f32> {
        self.slice_view.height
    }

    pub(crate) fn set_slice_view_height(&mut self, height: Option<f32>) {
        self.slice_view.height = height;
    }

    pub(crate) fn slice_view_minimized(&self) -> bool {
        self.slice_view.minimized
    }

    pub(crate) fn set_slice_view_minimized(&mut self, minimized: bool) {
        self.slice_view.minimized = minimized;
    }

    pub(crate) fn last_open_dir(&self) -> Option<PathBuf> {
        self.last_open_dir.clone()
    }

    pub(crate) fn set_last_open_dir(&mut self, dir: Option<PathBuf>) {
        self.last_open_dir = dir;
    }
}

#[cfg(test)]
mod app_settings_tests {
    use super::*;
    use crate::{MenuLayout, ThemePreference};

    #[test]
    fn defaults_are_system_theme_and_system_language() {
        let s = AppSettings::default();
        // Discriminant comparison only: ThemePreference may carry
        // platform-specific extra variants we don't construct here.
        assert!(matches!(s.theme(), ThemePreference::System));
        assert!(s.language().is_system());
    }

    #[test]
    fn default_menu_layout_is_bar() {
        assert_eq!(AppSettings::default().menu_layout(), MenuLayout::Bar);
    }

    #[test]
    fn default_aa_level_is_off() {
        assert_eq!(AppSettings::default().aa_level(), AaLevel::Off);
    }

    #[test]
    fn default_printer_and_resin_are_none_by_default() {
        let s = AppSettings::default();
        assert_eq!(s.default_printer(), None);
        assert_eq!(s.default_resin(), None);
    }

    #[test]
    fn favourites_are_empty_by_default() {
        let s = AppSettings::default();
        assert!(s.favourite_printers().is_empty());
        assert!(s.favourite_resins().is_empty());
    }

    #[test]
    fn default_object_list_is_visible() {
        assert!(AppSettings::default().object_list_visible());
    }

    #[test]
    fn default_object_list_geometry_is_unset() {
        let s = AppSettings::default();
        assert!(s.object_list_anchor_right());
        assert!(!s.object_list_anchor_bottom());
        assert_eq!(s.object_list_offset_x(), None);
        assert_eq!(s.object_list_offset_y(), None);
        assert_eq!(s.object_list_width(), None);
        assert_eq!(s.object_list_height(), None);
        assert!(!s.object_list_minimized());
    }

    #[test]
    fn default_slice_panel_geometry_is_unset() {
        let s = AppSettings::default();
        assert!(!s.slice_panel_anchor_right());
        assert!(!s.slice_panel_anchor_bottom());
        assert_eq!(s.slice_panel_offset_x(), None);
        assert_eq!(s.slice_panel_offset_y(), None);
        assert_eq!(s.slice_panel_width(), None);
        assert_eq!(s.slice_panel_height(), None);
    }

    #[test]
    fn default_slice_view_is_visible() {
        assert!(AppSettings::default().slice_view_visible());
    }

    #[test]
    fn default_slice_view_geometry_is_bottom_left() {
        let s = AppSettings::default();
        assert!(!s.slice_view_anchor_right());
        assert!(s.slice_view_anchor_bottom());
        assert_eq!(s.slice_view_offset_x(), None);
        assert_eq!(s.slice_view_offset_y(), None);
        assert_eq!(s.slice_view_width(), None);
        assert_eq!(s.slice_view_height(), None);
        assert!(!s.slice_view_minimized());
    }

    #[test]
    fn default_last_open_dir_is_none() {
        assert_eq!(AppSettings::default().last_open_dir(), None);
    }

    #[test]
    fn default_carries_current_schema_version() {
        assert_eq!(AppSettings::default().version, APP_SETTINGS_SCHEMA_VERSION);
    }
}

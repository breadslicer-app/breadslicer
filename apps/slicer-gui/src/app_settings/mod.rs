/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The application settings that persist between sessions, and their layout in
//! the settings file, which [`file`] reads and writes. Pre-alpha policy: a
//! schema change bumps [`APP_SETTINGS_SCHEMA_VERSION`], with no migration code.

pub(crate) mod file;
mod merge;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use newtypes::newtype_validated;
use sc_print_primitives::{AaLevel, MaterialId, OutputFormatBinding, PrinterId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::frame_rate::FrameRateCap;
use crate::language::{BundledLanguage, LanguagePreference};
use crate::{MenuLayout, ThemePreference};

/// On-disk representation of `ThemePreference`. Separate type because the
/// Slint-generated `ThemePreference` isn't serde-aware; we also want lowercase
/// variants for a friendlier TOML surface.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
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

// A language code as the settings file stores it, such as `es`, `pt_BR` or
// `sr@latin`. This build may not bundle it.
newtype_validated!(LanguageCode, MalformedLanguageCode, String, is_language_code; Eq);

/// Returns whether `code` can be stored as a language code: not empty, only
/// ASCII letters, digits, `-`, `_` and `@` (the characters of a locale code),
/// and not `system`, which the settings file uses for the system language.
fn is_language_code(code: &str) -> bool {
    !code.is_empty()
        && code != "system"
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'@'))
}

impl LanguageCode {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<BundledLanguage> for LanguageCode {
    fn from(language: BundledLanguage) -> Self {
        // Every bundled code passes the check, as
        // `every_bundled_code_is_a_language_code` shows.
        Self(language.code().to_owned())
    }
}

/// The language that the settings file stores: the system language, or a
/// language code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum LanguageSetting {
    #[default]
    System,
    Code(LanguageCode),
}

impl LanguageSetting {
    fn is_system(&self) -> bool {
        matches!(self, Self::System)
    }

    /// Returns the preference that this setting puts in force: a code that this
    /// build does not bundle gives the system language.
    fn preference(&self) -> LanguagePreference {
        match self {
            Self::System => LanguagePreference::System,
            Self::Code(code) => BundledLanguage::from_code(code.as_str())
                .map_or(LanguagePreference::System, LanguagePreference::Bundled),
        }
    }
}

// Wire format: `"system"` for the system language, the code otherwise.
impl Serialize for LanguageSetting {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::System => "system".serialize(ser),
            Self::Code(code) => code.as_str().serialize(ser),
        }
    }
}

impl<'de> Deserialize<'de> for LanguageSetting {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let stored = String::deserialize(de)?;
        if stored == "system" {
            return Ok(Self::System);
        }
        match LanguageCode::new(stored.clone()) {
            Ok(code) => Ok(Self::Code(code)),
            Err(MalformedLanguageCode) => {
                // Only a hand edit writes a malformed value, and the next save
                // drops it.
                eprintln!(
                    "Stored language {stored:?} is not a language code; using the system language"
                );
                Ok(Self::System)
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub(crate) struct Appearance {
    #[serde(default)]
    theme: ThemeSetting,
    /// Omitted in TOML when the user is on `System` (the default) so a fresh
    /// config stays minimal.
    #[serde(default, skip_serializing_if = "LanguageSetting::is_system")]
    language: LanguageSetting,
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

/// The background prescan is off by default: it waits for the Supports tab
/// (BS-67), which knows when to start it.
fn default_prescan_supports() -> bool {
    false
}

fn is_default_prescan_supports(on: &bool) -> bool {
    !*on
}

/// Models keep their facets by default; supports always shade smooth.
fn default_smooth_models() -> bool {
    false
}

fn is_default_smooth_models(on: &bool) -> bool {
    !*on
}

fn is_default_frame_rate(cap: &FrameRateCap) -> bool {
    *cap == FrameRateCap::default()
}

/// The diagnostic log is off by default: people turn it on to report a
/// problem.
fn default_diagnostic_log() -> bool {
    false
}

fn is_default_diagnostic_log(on: &bool) -> bool {
    !*on
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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
    /// Whether the support scans run in the background while the scene sits
    /// still, so Generate Supports starts from them. Omitted in TOML while off
    /// (the default).
    #[serde(
        default = "default_prescan_supports",
        skip_serializing_if = "is_default_prescan_supports"
    )]
    prescan_supports: bool,
    /// Whether models shade smooth in the viewport (supports always do).
    /// Omitted in TOML while off (the default).
    #[serde(
        default = "default_smooth_models",
        skip_serializing_if = "is_default_smooth_models"
    )]
    smooth_models: bool,
    /// The viewport's frame-rate cap. Omitted in TOML while it holds the
    /// default (`Automatic`).
    #[serde(default, skip_serializing_if = "is_default_frame_rate")]
    frame_rate: FrameRateCap,
    /// Whether the diagnostic log writes the session's events to disk.
    /// Omitted in TOML while off (the default).
    #[serde(
        default = "default_diagnostic_log",
        skip_serializing_if = "is_default_diagnostic_log"
    )]
    diagnostic_log: bool,
    /// The app messages the user silenced with "Don't show again", by key
    /// (see `error_dialog::MessageKey`). Omitted in TOML while empty.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    silenced_messages: BTreeSet<String>,
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
            prescan_supports: default_prescan_supports(),
            smooth_models: default_smooth_models(),
            frame_rate: FrameRateCap::default(),
            diagnostic_log: default_diagnostic_log(),
            silenced_messages: BTreeSet::new(),
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
        self.appearance.language.preference()
    }

    /// Stores `language`. A stored code that this build does not bundle stays
    /// while `language` is the system language, so a save of another change
    /// keeps the code that the user chose.
    pub(crate) fn set_language(&mut self, language: LanguagePreference) {
        if self.language() == language {
            return;
        }
        self.appearance.language = match language {
            LanguagePreference::System => LanguageSetting::System,
            LanguagePreference::Bundled(bundled) => LanguageSetting::Code(bundled.into()),
        };
    }

    /// Returns the stored language code when this build does not bundle it.
    fn unbundled_language(&self) -> Option<&LanguageCode> {
        match &self.appearance.language {
            LanguageSetting::Code(code) if self.language() == LanguagePreference::System => {
                Some(code)
            }
            _ => None,
        }
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

    pub(crate) fn prescan_supports(&self) -> bool {
        self.prescan_supports
    }

    pub(crate) fn set_prescan_supports(&mut self, on: bool) {
        self.prescan_supports = on;
    }

    pub(crate) fn smooth_models(&self) -> bool {
        self.smooth_models
    }

    pub(crate) fn set_smooth_models(&mut self, on: bool) {
        self.smooth_models = on;
    }

    pub(crate) fn frame_rate(&self) -> FrameRateCap {
        self.frame_rate
    }

    pub(crate) fn set_frame_rate(&mut self, cap: FrameRateCap) {
        self.frame_rate = cap;
    }

    pub(crate) fn diagnostic_log(&self) -> bool {
        self.diagnostic_log
    }

    pub(crate) fn set_diagnostic_log(&mut self, on: bool) {
        self.diagnostic_log = on;
    }

    pub(crate) fn silenced_messages(&self) -> BTreeSet<String> {
        self.silenced_messages.clone()
    }

    pub(crate) fn set_silenced_messages(&mut self, keys: BTreeSet<String>) {
        self.silenced_messages = keys;
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
        assert_eq!(s.language(), LanguagePreference::System);
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

#[cfg(test)]
mod language_setting_tests {
    use super::*;

    /// The `language` key on its own, as the settings file reads and writes it.
    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Stored {
        language: LanguageSetting,
    }

    fn code(value: &str) -> LanguageCode {
        LanguageCode::new(value.to_owned()).expect("a language code")
    }

    fn bundled(value: &str) -> LanguagePreference {
        LanguagePreference::Bundled(BundledLanguage::from_code(value).expect("bundled language"))
    }

    /// Returns default settings that store `language`.
    fn settings_with(language: LanguageSetting) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.appearance.language = language;
        settings
    }

    #[test]
    fn locale_codes_are_language_codes() {
        for value in ["es", "pt_BR", "sr@latin", "xx"] {
            assert_eq!(code(value).as_str(), value);
        }
    }

    #[test]
    fn other_values_are_not_language_codes() {
        for value in ["", "system", "e s", "ü"] {
            assert_eq!(
                LanguageCode::new(value.to_owned()),
                Err(MalformedLanguageCode),
                "{value:?}"
            );
        }
    }

    #[test]
    fn every_bundled_code_is_a_language_code() {
        for language in BundledLanguage::ALL {
            assert_eq!(
                LanguageCode::new(language.code().to_owned()),
                Ok(LanguageCode::from(*language))
            );
        }
    }

    #[test]
    fn system_is_read_as_the_system_language() {
        let stored: Stored = toml::from_str("language = \"system\"").unwrap();
        assert_eq!(stored.language, LanguageSetting::System);
    }

    #[test]
    fn a_code_is_read_as_that_code() {
        let stored: Stored = toml::from_str("language = \"xx\"").unwrap();
        assert_eq!(stored.language, LanguageSetting::Code(code("xx")));
    }

    #[test]
    fn a_malformed_value_is_read_as_the_system_language() {
        let stored: Stored = toml::from_str("language = \"\"").unwrap();
        assert_eq!(stored.language, LanguageSetting::System);
    }

    #[test]
    fn a_value_that_is_not_a_string_does_not_parse() {
        assert!(toml::from_str::<Stored>("language = 5").is_err());
    }

    #[test]
    fn the_system_language_is_written_as_system() {
        let stored = Stored {
            language: LanguageSetting::System,
        };
        assert_eq!(toml::to_string(&stored).unwrap(), "language = \"system\"\n");
    }

    #[test]
    fn a_code_is_written_as_itself() {
        let stored = Stored {
            language: LanguageSetting::Code(code("xx")),
        };
        assert_eq!(toml::to_string(&stored).unwrap(), "language = \"xx\"\n");
    }

    #[test]
    fn a_bundled_code_gives_its_language() {
        let settings = settings_with(LanguageSetting::Code(code("es")));
        assert_eq!(settings.language(), bundled("es"));
    }

    #[test]
    fn an_unbundled_code_gives_the_system_language() {
        let settings = settings_with(LanguageSetting::Code(code("xx")));
        assert_eq!(settings.language(), LanguagePreference::System);
    }

    #[test]
    fn the_system_setting_gives_the_system_language() {
        let settings = settings_with(LanguageSetting::System);
        assert_eq!(settings.language(), LanguagePreference::System);
    }

    #[test]
    fn the_system_language_keeps_an_unbundled_code() {
        let mut settings = settings_with(LanguageSetting::Code(code("xx")));
        settings.set_language(LanguagePreference::System);
        assert_eq!(
            settings.appearance.language,
            LanguageSetting::Code(code("xx"))
        );
    }

    #[test]
    fn picking_a_language_replaces_an_unbundled_code() {
        let mut settings = settings_with(LanguageSetting::Code(code("xx")));
        settings.set_language(bundled("de"));
        assert_eq!(
            settings.appearance.language,
            LanguageSetting::Code(code("de"))
        );
    }

    #[test]
    fn the_system_language_replaces_a_bundled_code() {
        let mut settings = settings_with(LanguageSetting::Code(code("es")));
        settings.set_language(LanguagePreference::System);
        assert_eq!(settings.appearance.language, LanguageSetting::System);
    }

    #[test]
    fn only_an_unbundled_code_is_reported() {
        let unbundled = settings_with(LanguageSetting::Code(code("xx")));
        assert_eq!(unbundled.unbundled_language(), Some(&code("xx")));
        let with_bundled = settings_with(LanguageSetting::Code(code("es")));
        assert_eq!(with_bundled.unbundled_language(), None);
        let system = settings_with(LanguageSetting::System);
        assert_eq!(system.unbundled_language(), None);
    }
}

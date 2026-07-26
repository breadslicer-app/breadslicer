/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! On-disk application settings. Loaded once at startup and rewritten on
//! every user-driven change. Silent fallback to defaults on I/O errors so
//! first-run cases degrade gracefully; a parse failure or schema-version
//! mismatch additionally overwrites the on-disk file with fresh defaults
//! (pre-alpha policy: bump [`APP_SETTINGS_SCHEMA_VERSION`] when the schema
//! changes, no migration code).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sc_print_primitives::{MaterialId, PrinterId};
use serde::{Deserialize, Serialize};

use crate::language::LanguagePreference;
use crate::{MenuLayout, ThemePreference};

const SETTINGS_FILENAME: &str = "settings.toml";

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
/// Slint-generated `MenuLayout` isn't serde-aware; lowercase variants
/// give a friendlier TOML surface.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MenuLayoutSetting {
    #[default]
    Bar,
    Compact,
}

impl MenuLayoutSetting {
    /// `Bar` is the default; skipped when serializing so a fresh config
    /// stays minimal.
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
    /// Omitted in TOML when the user is on `System` (the default) so a
    /// fresh config stays minimal.
    #[serde(default, skip_serializing_if = "LanguagePreference::is_system")]
    language: LanguagePreference,
    /// Omitted in TOML when the user is on `Bar` (the default).
    #[serde(default, skip_serializing_if = "MenuLayoutSetting::is_bar")]
    menu_layout: MenuLayoutSetting,
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// On-disk state of the floating object-list panel. Every field is
/// optional and omitted when it holds its default, so a fresh config
/// carries no `[object_list]` section at all.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct ObjectListSettings {
    /// Whether the panel is shown. Defaults to visible; omitted in TOML
    /// while it holds that default.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    visible: bool,
    /// Which window edges the panel hugs. The default hugs the top-right
    /// corner, so a right anchor and a top (non-bottom) anchor are
    /// omitted from TOML.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    anchor_right: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    anchor_bottom: bool,
    /// Distance from the anchored edges to the matching panel edges, in
    /// logical pixels. Absent until the user drags the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset_y: Option<f32>,
    /// Panel size in logical pixels. Absent until the user resizes the
    /// panel; `height` absent means auto-sizing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<f32>,
    /// Whether the panel is collapsed to its title bar. Omitted while
    /// expanded (the default).
    #[serde(default, skip_serializing_if = "is_false")]
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
    /// True when every field holds its default, so the whole section can
    /// be omitted from the serialized TOML.
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Current schema version of the on-disk settings file. Bump this when
/// the structure of `AppSettings` (or one of its nested settings types)
/// changes in a way that an older or newer build cannot read. On load,
/// a file whose version differs from this constant is replaced with a
/// fresh defaults file; no migration code is kept.
pub(crate) const APP_SETTINGS_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    APP_SETTINGS_SCHEMA_VERSION
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AppSettings {
    /// On-disk schema version. Files predating this field are read with
    /// the current version filled in by serde (treated as compatible);
    /// once written, the field is always present.
    #[serde(default = "default_schema_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub appearance: Appearance,
    /// The app-wide default printer, by id. Omitted in TOML until the
    /// user picks one; the viewport envelope is derived from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_printer: Option<PrinterId>,
    /// The app-wide default resin, by id. Omitted in TOML until the
    /// user picks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_resin: Option<MaterialId>,
    /// Floating object-list panel state. Omitted in TOML while every
    /// field holds its default.
    #[serde(default, skip_serializing_if = "ObjectListSettings::is_default")]
    object_list: ObjectListSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: APP_SETTINGS_SCHEMA_VERSION,
            appearance: Appearance::default(),
            default_printer: None,
            default_resin: None,
            object_list: ObjectListSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        Self::load_from_path(&path)
    }

    /// Same fallback semantics as `load`, but the caller chooses the
    /// file path. Used by tests; the production entry point
    /// `load` resolves the user's config path and forwards. A parse
    /// failure or schema-version mismatch resets to defaults and
    /// overwrites the on-disk file; an I/O failure (other than
    /// `NotFound`) only resets in memory.
    pub(crate) fn load_from_path(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match toml::from_str::<AppSettings>(&content) {
                Ok(loaded) if loaded.version == APP_SETTINGS_SCHEMA_VERSION => loaded,
                Ok(loaded) => {
                    eprintln!(
                        "Settings schema version in {} is {} (current is {}); resetting to defaults",
                        path.display(),
                        loaded.version,
                        APP_SETTINGS_SCHEMA_VERSION,
                    );
                    Self::reset_on_disk(path)
                }
                Err(err) => {
                    eprintln!(
                        "Failed to parse {}: {err}; resetting to defaults",
                        path.display(),
                    );
                    Self::reset_on_disk(path)
                }
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!("Failed to read {}: {err}", path.display());
                Self::default()
            }
        }
    }

    fn reset_on_disk(path: &Path) -> Self {
        let fresh = Self::default();
        if let Err(err) = fresh.save_to_path(path) {
            eprintln!(
                "Failed to overwrite {} with fresh defaults: {err}",
                path.display(),
            );
        }
        fresh
    }

    pub fn save(&self) -> io::Result<()> {
        let path =
            settings_path().ok_or_else(|| io::Error::other("no config directory available"))?;
        self.save_to_path(&path)
    }

    /// Same `create_dir_all` + atomic-write semantics as `save`, but
    /// the caller chooses the file path. Used by tests.
    pub(crate) fn save_to_path(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).map_err(io::Error::other)?;
        write_atomically(path, content.as_bytes())
    }

    pub fn theme(&self) -> ThemePreference {
        self.appearance.theme.into()
    }

    pub fn set_theme(&mut self, theme: ThemePreference) {
        self.appearance.theme = theme.into();
    }

    pub fn language(&self) -> LanguagePreference {
        self.appearance.language
    }

    pub fn set_language(&mut self, language: LanguagePreference) {
        self.appearance.language = language;
    }

    pub fn menu_layout(&self) -> MenuLayout {
        self.appearance.menu_layout.into()
    }

    pub fn set_menu_layout(&mut self, menu_layout: MenuLayout) {
        self.appearance.menu_layout = menu_layout.into();
    }

    pub fn default_printer(&self) -> Option<PrinterId> {
        self.default_printer.clone()
    }

    pub fn set_default_printer(&mut self, id: Option<PrinterId>) {
        self.default_printer = id;
    }

    pub fn default_resin(&self) -> Option<MaterialId> {
        self.default_resin.clone()
    }

    pub fn set_default_resin(&mut self, id: Option<MaterialId>) {
        self.default_resin = id;
    }

    pub fn object_list_visible(&self) -> bool {
        self.object_list.visible
    }

    pub fn set_object_list_visible(&mut self, visible: bool) {
        self.object_list.visible = visible;
    }

    pub fn object_list_anchor_right(&self) -> bool {
        self.object_list.anchor_right
    }

    pub fn set_object_list_anchor_right(&mut self, anchor_right: bool) {
        self.object_list.anchor_right = anchor_right;
    }

    pub fn object_list_anchor_bottom(&self) -> bool {
        self.object_list.anchor_bottom
    }

    pub fn set_object_list_anchor_bottom(&mut self, anchor_bottom: bool) {
        self.object_list.anchor_bottom = anchor_bottom;
    }

    pub fn object_list_offset_x(&self) -> Option<f32> {
        self.object_list.offset_x
    }

    pub fn set_object_list_offset_x(&mut self, offset_x: Option<f32>) {
        self.object_list.offset_x = offset_x;
    }

    pub fn object_list_offset_y(&self) -> Option<f32> {
        self.object_list.offset_y
    }

    pub fn set_object_list_offset_y(&mut self, offset_y: Option<f32>) {
        self.object_list.offset_y = offset_y;
    }

    pub fn object_list_width(&self) -> Option<f32> {
        self.object_list.width
    }

    pub fn set_object_list_width(&mut self, width: Option<f32>) {
        self.object_list.width = width;
    }

    pub fn object_list_height(&self) -> Option<f32> {
        self.object_list.height
    }

    pub fn set_object_list_height(&mut self, height: Option<f32>) {
        self.object_list.height = height;
    }

    pub fn object_list_minimized(&self) -> bool {
        self.object_list.minimized
    }

    pub fn set_object_list_minimized(&mut self, minimized: bool) {
        self.object_list.minimized = minimized;
    }
}

fn settings_path() -> Option<PathBuf> {
    sc_app_paths::project_dirs().map(|d| d.config_dir().join(SETTINGS_FILENAME))
}

/// Process-wide counter that disambiguates concurrent atomic writes
/// from inside the same process (parallel test threads in particular).
static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_temp_path(target: &Path) -> io::Result<PathBuf> {
    let file_name = target
        .file_name()
        .ok_or_else(|| io::Error::other("settings path has no file name"))?
        .to_string_lossy()
        .into_owned();
    let parent = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let pid = std::process::id();
    let token = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(".{file_name}.tmp.{pid}.{token}")))
}

/// Writes `contents` to `target` atomically: opens a freshly named temp
/// file next to the target with `create_new(true)`, writes + flushes +
/// fsyncs it, then renames over the target. On any failure the temp
/// file is removed and the previous target (if any) is left intact, so
/// a crash mid-write cannot truncate the user's settings.
fn write_atomically(target: &Path, contents: &[u8]) -> io::Result<()> {
    let temp_path = atomic_temp_path(target)?;

    let inner = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(contents)?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })();

    match inner {
        Ok(()) => match fs::rename(&temp_path, target) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&temp_path);
                Err(err)
            }
        },
        Err(err) => {
            let _ = fs::remove_file(&temp_path);
            Err(err)
        }
    }
}

#[cfg(test)]
mod app_settings_tests {
    use super::*;
    use crate::language::LanguagePreference;
    use crate::{MenuLayout, ThemePreference};
    use tempfile::TempDir;

    #[test]
    fn defaults_are_system_theme_and_system_language() {
        let s = AppSettings::default();
        // Discriminant comparison only: ThemePreference may carry
        // platform-specific extra variants we don't construct here.
        assert!(matches!(s.theme(), ThemePreference::System));
        assert!(s.language().is_system());
    }

    #[test]
    fn missing_file_loads_defaults() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("does_not_exist.toml");
        let s = AppSettings::load_from_path(&path);
        assert!(matches!(s.theme(), ThemePreference::System));
        assert!(s.language().is_system());
    }

    #[test]
    fn malformed_toml_resets_and_overwrites_file() {
        // A parse failure is intentionally non-fatal: first-run / hand-
        // edited typo cases must not refuse to launch the GUI. The
        // garbled file is replaced with a fresh defaults file so the
        // next run starts from a known-good state.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(&path, b"this is not valid TOML").unwrap();
        let s = AppSettings::load_from_path(&path);
        assert!(matches!(s.theme(), ThemePreference::System));
        assert_eq!(s.version, APP_SETTINGS_SCHEMA_VERSION);
        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains(&format!("version = {APP_SETTINGS_SCHEMA_VERSION}")),
            "expected garbled file to be replaced with defaults, got:\n{on_disk}"
        );
    }

    #[test]
    fn save_then_load_round_trips_theme_and_language() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("settings.toml");

        let mut s = AppSettings::default();
        s.set_theme(ThemePreference::Dark);
        s.save_to_path(&path).unwrap();

        // The save call must create parent directories: the path
        // above includes a `nested/` segment that doesn't exist yet.
        assert!(path.exists());

        let loaded = AppSettings::load_from_path(&path);
        assert!(matches!(loaded.theme(), ThemePreference::Dark));
        assert!(loaded.language().is_system());
    }

    #[test]
    fn saved_toml_omits_default_language() {
        // System (the default) is `skip_serializing_if`; a fresh config
        // shouldn't carry a redundant `language = "system"` line.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let s = AppSettings::default();
        s.save_to_path(&path).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            !written.contains("language ="),
            "default settings should not serialise the language key, got:\n{written}"
        );
    }

    #[test]
    fn explicit_language_round_trips() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        let bundled = crate::language::BundledLanguage::from_code("es")
            .expect("`es` must be bundled in i18n/");
        s.set_language(LanguagePreference::Bundled(bundled));
        s.save_to_path(&path).unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.language(), LanguagePreference::Bundled(bundled));

        // The output must mention `language = "es"` so an external tool
        // (or hand-edit) can find it.
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("language = \"es\""), "got:\n{written}");
    }

    #[test]
    fn light_theme_round_trips() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        s.set_theme(ThemePreference::Light);
        s.save_to_path(&path).unwrap();
        let loaded = AppSettings::load_from_path(&path);
        assert!(matches!(loaded.theme(), ThemePreference::Light));
    }

    #[test]
    fn default_menu_layout_is_bar() {
        assert_eq!(AppSettings::default().menu_layout(), MenuLayout::Bar);
    }

    #[test]
    fn compact_menu_layout_round_trips() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        s.set_menu_layout(MenuLayout::Compact);
        s.save_to_path(&path).unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.menu_layout(), MenuLayout::Compact);

        // Written as a plain lowercase string so an external tool or a
        // hand-edit can find it.
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("menu_layout = \"compact\""),
            "got:\n{written}"
        );
    }

    #[test]
    fn saved_toml_omits_default_menu_layout() {
        // Bar (the default) is `skip_serializing_if`; a fresh config
        // shouldn't carry a redundant `menu_layout = "bar"` line.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        AppSettings::default().save_to_path(&path).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            !written.contains("menu_layout"),
            "default settings should not serialise the menu layout key, got:\n{written}"
        );
    }

    #[test]
    fn default_printer_and_resin_are_none_by_default() {
        let s = AppSettings::default();
        assert_eq!(s.default_printer(), None);
        assert_eq!(s.default_resin(), None);
    }

    #[test]
    fn explicit_default_printer_and_resin_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        let printer = PrinterId::new("anycubic-photon-m3-pm3").unwrap();
        let resin = MaterialId::new("abs-like-405").unwrap();
        s.set_default_printer(Some(printer.clone()));
        s.set_default_resin(Some(resin.clone()));
        s.save_to_path(&path).unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.default_printer(), Some(printer));
        assert_eq!(loaded.default_resin(), Some(resin));

        // The ids are written as plain strings so an external tool or a
        // hand-edit can find them.
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("default_printer = \"anycubic-photon-m3-pm3\""),
            "got:\n{written}"
        );
        assert!(
            written.contains("default_resin = \"abs-like-405\""),
            "got:\n{written}"
        );
    }

    #[test]
    fn saved_toml_omits_unset_default_printer_and_resin() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let s = AppSettings::default();
        s.save_to_path(&path).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            !written.contains("default_printer"),
            "unset default printer should not serialise, got:\n{written}"
        );
        assert!(
            !written.contains("default_resin"),
            "unset default resin should not serialise, got:\n{written}"
        );
    }

    #[test]
    fn default_object_list_is_visible() {
        assert!(AppSettings::default().object_list_visible());
    }

    #[test]
    fn hidden_object_list_round_trips() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        s.set_object_list_visible(false);
        s.save_to_path(&path).unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert!(!loaded.object_list_visible());

        // Written under a dedicated section so an external tool or a
        // hand-edit can find it.
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("[object_list]"), "got:\n{written}");
        assert!(written.contains("visible = false"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_default_object_list() {
        // Visible (the default) is skipped, and with no other field set
        // the whole section is omitted so a fresh config stays minimal.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        AppSettings::default().save_to_path(&path).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            !written.contains("object_list"),
            "default settings should not serialise the object-list section, got:\n{written}"
        );
    }

    #[test]
    fn missing_object_list_section_loads_visible() {
        // A settings file predating the object-list section loads with
        // the panel visible (its default).
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(&path, "[appearance]\ntheme = \"dark\"\n").unwrap();
        let loaded = AppSettings::load_from_path(&path);
        assert!(loaded.object_list_visible());
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
    fn object_list_geometry_round_trips() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        s.set_object_list_anchor_right(false);
        s.set_object_list_anchor_bottom(true);
        s.set_object_list_offset_x(Some(48.0));
        s.set_object_list_offset_y(Some(64.0));
        s.set_object_list_width(Some(280.0));
        s.set_object_list_height(Some(200.0));
        s.set_object_list_minimized(true);
        s.save_to_path(&path).unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert!(!loaded.object_list_anchor_right());
        assert!(loaded.object_list_anchor_bottom());
        assert_eq!(loaded.object_list_offset_x(), Some(48.0));
        assert_eq!(loaded.object_list_offset_y(), Some(64.0));
        assert_eq!(loaded.object_list_width(), Some(280.0));
        assert_eq!(loaded.object_list_height(), Some(200.0));
        assert!(loaded.object_list_minimized());

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("[object_list]"), "got:\n{written}");
        assert!(written.contains("anchor_right = false"), "got:\n{written}");
        assert!(written.contains("anchor_bottom = true"), "got:\n{written}");
        assert!(written.contains("minimized = true"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_unset_object_list_geometry() {
        // Default anchors and auto-sizing stay absent: only the explicit
        // minimized flag lands in the file.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        s.set_object_list_minimized(true);
        s.save_to_path(&path).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("anchor_right"), "got:\n{written}");
        assert!(!written.contains("anchor_bottom"), "got:\n{written}");
        assert!(!written.contains("offset_x"), "got:\n{written}");
        assert!(!written.contains("width ="), "got:\n{written}");
        assert!(!written.contains("height ="), "got:\n{written}");
    }

    #[test]
    fn default_carries_current_schema_version() {
        assert_eq!(AppSettings::default().version, APP_SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn saved_toml_includes_schema_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        AppSettings::default().save_to_path(&path).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.contains(&format!("version = {APP_SETTINGS_SCHEMA_VERSION}")),
            "saved file must stamp the current schema version, got:\n{written}"
        );
    }

    #[test]
    fn save_then_load_preserves_schema_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        AppSettings::default().save_to_path(&path).unwrap();
        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.version, APP_SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn missing_version_field_loads_as_current() {
        // Files written before this field existed do not carry a
        // `version` key. Serde's default fills in the current schema
        // version so the legacy file loads without triggering a reset.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(&path, "[appearance]\ntheme = \"dark\"\n").unwrap();
        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.version, APP_SETTINGS_SCHEMA_VERSION);
        assert!(matches!(loaded.theme(), ThemePreference::Dark));
        // The legacy file is preserved verbatim (no rewrite happens
        // until the user makes a change that triggers a save).
        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("version ="), "got:\n{on_disk}");
    }

    #[test]
    fn future_schema_version_resets_and_overwrites_file() {
        // A settings file from a hypothetical future build (version
        // ahead of the current one) is not migrated; the file is
        // replaced with a fresh defaults file stamped at the current
        // schema version.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let future = APP_SETTINGS_SCHEMA_VERSION + 999;
        fs::write(
            &path,
            format!("version = {future}\n[appearance]\ntheme = \"dark\"\n"),
        )
        .unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert!(matches!(loaded.theme(), ThemePreference::System));
        assert_eq!(loaded.version, APP_SETTINGS_SCHEMA_VERSION);

        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains(&format!("version = {APP_SETTINGS_SCHEMA_VERSION}")),
            "expected file to be replaced with current version, got:\n{on_disk}"
        );
        assert!(
            !on_disk.contains(&format!("version = {future}")),
            "stale future version still present, got:\n{on_disk}"
        );
    }

    #[test]
    fn past_schema_version_resets_and_overwrites_file() {
        // A settings file with a version below the current schema is
        // also smashed; no migration code is kept in pre-alpha. `0`
        // stands in for "any version older than the first real
        // schema", which is the only direction below the current one.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(&path, "version = 0\n").unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.version, APP_SETTINGS_SCHEMA_VERSION);

        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains(&format!("version = {APP_SETTINGS_SCHEMA_VERSION}")),
            "expected file to be replaced with current version, got:\n{on_disk}"
        );
    }

    #[test]
    fn save_to_path_leaves_no_temp_file_on_success() {
        // The atomic-write helper places a `.settings.toml.tmp.<pid>.<n>`
        // file next to the target and renames it into place. A successful
        // save must leave exactly the target behind, with no leftover
        // temp file polluting the config directory across repeated saves.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");

        let s = AppSettings::default();
        s.save_to_path(&path).unwrap();
        s.save_to_path(&path).unwrap();
        s.save_to_path(&path).unwrap();

        let entries: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from("settings.toml")],
            "expected only the target file, got {entries:?}",
        );
    }
}

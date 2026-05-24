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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::ThemePreference;
use crate::build_volume::{BuildVolume, HARDCODED_DEFAULT_BUILD_VOLUME};
use crate::language::LanguagePreference;

// Linux's config_dir = $XDG_CONFIG_HOME/<application>. Bare "slicer" would
// collide with unrelated tools, so use the fully-namespaced name on Linux.
// macOS keeps the reverse-DNS bundle id (it.soulcrafted.slicer) and Windows
// keeps the Organization\Application layout (soulcrafted\slicer\config),
// so only the Linux path needs the adjustment.
#[cfg(target_os = "linux")]
const APPLICATION: &str = "soulcrafted-slicer";
#[cfg(not(target_os = "linux"))]
const APPLICATION: &str = "slicer";

const QUALIFIER: &str = "it";
const ORGANIZATION: &str = "soulcrafted";
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Appearance {
    #[serde(default)]
    theme: ThemeSetting,
    /// Omitted in TOML when the user is on `System` (the default) so a
    /// fresh config stays minimal.
    #[serde(default, skip_serializing_if = "LanguagePreference::is_system")]
    language: LanguagePreference,
}

/// Lower bound for any build-volume axis (mm). Below this, the camera
/// can't reasonably frame the plate and translate-clamp arithmetic
/// loses precision; on-disk values below the bound are clamped on
/// load.
pub(crate) const MIN_DIM_MM: f32 = 10.0;
/// Upper bound for any build-volume axis (mm). An ample envelope for
/// prosumer-class SLA printers; on-disk values above the bound are
/// clamped on load to keep the camera framing radius finite.
pub(crate) const MAX_DIM_MM: f32 = 1000.0;

/// On-disk representation of `BuildVolume`. Stored as three numeric
/// fields rather than the type itself so a hand-edited or out-of-range
/// value loads as a clamped default instead of failing the whole file
/// parse.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct BuildVolumeSetting {
    width_mm: f32,
    depth_mm: f32,
    height_mm: f32,
}

impl Default for BuildVolumeSetting {
    fn default() -> Self {
        BuildVolumeSetting::from(HARDCODED_DEFAULT_BUILD_VOLUME)
    }
}

impl From<BuildVolume> for BuildVolumeSetting {
    fn from(value: BuildVolume) -> Self {
        Self {
            width_mm: value.width_mm(),
            depth_mm: value.depth_mm(),
            height_mm: value.height_mm(),
        }
    }
}

impl From<BuildVolumeSetting> for BuildVolume {
    fn from(value: BuildVolumeSetting) -> Self {
        BuildVolume::new(
            sanitise_dim(value.width_mm),
            sanitise_dim(value.depth_mm),
            sanitise_dim(value.height_mm),
        )
    }
}

/// Coerce a dimension into a valid axis length: NaN / non-finite
/// values fall back to the minimum bound, then the value is clamped
/// into `[MIN_DIM_MM, MAX_DIM_MM]`. Used by both the on-disk loader
/// and the preferences-UI commit handler so all entry points share
/// one normalisation rule.
pub(crate) fn sanitise_dim(d: f32) -> f32 {
    if !d.is_finite() {
        return MIN_DIM_MM;
    }
    d.clamp(MIN_DIM_MM, MAX_DIM_MM)
}

fn build_volume_is_default(bv: &BuildVolumeSetting) -> bool {
    *bv == BuildVolumeSetting::default()
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
    /// Omitted in TOML when the user has the hardcoded default, so a
    /// fresh config stays minimal.
    #[serde(default, skip_serializing_if = "build_volume_is_default")]
    build_volume: BuildVolumeSetting,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: APP_SETTINGS_SCHEMA_VERSION,
            appearance: Appearance::default(),
            build_volume: BuildVolumeSetting::default(),
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
        fs::write(path, content)
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

    pub fn build_volume(&self) -> BuildVolume {
        self.build_volume.into()
    }

    pub fn set_build_volume(&mut self, bv: BuildVolume) {
        self.build_volume = bv.into();
    }
}

fn settings_path() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|d| d.config_dir().join(SETTINGS_FILENAME))
}

#[cfg(test)]
mod app_settings_tests {
    use super::*;
    use crate::ThemePreference;
    use crate::language::LanguagePreference;
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
    fn default_build_volume_matches_hardcoded_constant() {
        let s = AppSettings::default();
        let bv = s.build_volume();
        assert_eq!(bv, HARDCODED_DEFAULT_BUILD_VOLUME);
    }

    #[test]
    fn explicit_build_volume_round_trips() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut s = AppSettings::default();
        s.set_build_volume(BuildVolume::new(150.0, 80.0, 160.0));
        s.save_to_path(&path).unwrap();

        let loaded = AppSettings::load_from_path(&path);
        let bv = loaded.build_volume();
        assert_eq!(bv, BuildVolume::new(150.0, 80.0, 160.0));
    }

    #[test]
    fn saved_toml_omits_default_build_volume() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let s = AppSettings::default();
        s.save_to_path(&path).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            !written.contains("[build_volume]"),
            "default settings should not serialise the build_volume table, got:\n{written}"
        );
    }

    #[test]
    fn out_of_range_build_volume_clamps_on_load() {
        // 5 mm is below MIN_DIM_MM (10), 5000 mm is above MAX_DIM_MM (1000).
        // Both axes should be clamped into the allowed band.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(
            &path,
            "[build_volume]\nwidth_mm = 5.0\ndepth_mm = 50.0\nheight_mm = 5000.0\n",
        )
        .unwrap();

        let loaded = AppSettings::load_from_path(&path);
        let bv = loaded.build_volume();
        assert_eq!(bv, BuildVolume::new(MIN_DIM_MM, 50.0, MAX_DIM_MM));
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
    fn non_finite_build_volume_falls_back_to_min() {
        // NaN / inf in the file shouldn't crash; they collapse to the
        // minimum bound so the GUI can still launch and the user sees
        // a small but legal envelope rather than an undefined render.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(
            &path,
            "[build_volume]\nwidth_mm = nan\ndepth_mm = inf\nheight_mm = 80.0\n",
        )
        .unwrap();

        let loaded = AppSettings::load_from_path(&path);
        let bv = loaded.build_volume();
        assert_eq!(bv, BuildVolume::new(MIN_DIM_MM, MIN_DIM_MM, 80.0));
    }
}

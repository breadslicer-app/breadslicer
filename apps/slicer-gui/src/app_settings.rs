/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! On-disk application settings. Loaded once at startup and rewritten on
//! every user-driven change. Silent fallback to defaults on any I/O or parse
//! failure so first-run and corrupted-file cases both degrade gracefully.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::ThemePreference;
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct AppSettings {
    #[serde(default)]
    pub appearance: Appearance,
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
    /// `load` resolves the user's config path and forwards.
    pub(crate) fn load_from_path(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => toml::from_str::<AppSettings>(&content).unwrap_or_else(|err| {
                eprintln!("Failed to parse {}: {err}", path.display());
                Self::default()
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!("Failed to read {}: {err}", path.display());
                Self::default()
            }
        }
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
    fn malformed_toml_logs_and_loads_defaults() {
        // A parse failure is intentionally non-fatal — first-run / hand-
        // edited typo cases must not refuse to launch the GUI.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(&path, b"this is not valid TOML").unwrap();
        let s = AppSettings::load_from_path(&path);
        assert!(matches!(s.theme(), ThemePreference::System));
    }

    #[test]
    fn save_then_load_round_trips_theme_and_language() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("settings.toml");

        let mut s = AppSettings::default();
        s.set_theme(ThemePreference::Dark);
        s.save_to_path(&path).unwrap();

        // The save call must create parent directories — the path
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
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! On-disk application settings. Loaded once at startup and rewritten on
//! every user-driven change. Silent fallback to defaults on any I/O or parse
//! failure so first-run and corrupted-file cases both degrade gracefully.

use std::fs;
use std::io;
use std::path::PathBuf;

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
        match fs::read_to_string(&path) {
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
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(&path, content)
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

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The file that keeps the application settings between sessions. It is read
//! once at startup and written again on every user-driven change. Each process
//! merges its own changes into the file under a lock, so that 2 processes keep
//! the changes of each other.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::merge::merge;
use super::{APP_SETTINGS_SCHEMA_VERSION, AppSettings};

const SETTINGS_FILENAME: &str = "settings.toml";

/// The file that keeps the application settings.
pub(crate) struct SettingsFile {
    path: PathBuf,
}

impl SettingsFile {
    /// Returns the settings file of the current user, in the configuration
    /// directory of the app, or `None` when the platform gives no home
    /// directory.
    pub(crate) fn platform_default() -> Option<Self> {
        sc_app_paths::project_dirs().map(|dirs| Self {
            path: dirs.config_dir().join(SETTINGS_FILENAME),
        })
    }

    /// Returns the settings file at `path`.
    #[cfg(test)]
    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the settings. It never writes, so the UI thread can call it: a
    /// file that does not parse or has another schema version gives
    /// [`LoadOutcome::Unusable`], which [`Self::reset_if_unusable`] then
    /// replaces.
    pub(crate) fn load(&self) -> LoadOutcome {
        match self.read() {
            OnDisk::Usable(settings) => {
                if let Some(code) = settings.unbundled_language() {
                    eprintln!(
                        "Stored language {:?} is not bundled in this build; using the system language",
                        code.as_str(),
                    );
                }
                LoadOutcome::Read(settings)
            }
            OnDisk::Missing => LoadOutcome::NotRead,
            OnDisk::Unreadable(err) => {
                eprintln!("Failed to read {}: {err}", self.path.display());
                LoadOutcome::NotRead
            }
            OnDisk::Unparseable(err) => {
                eprintln!(
                    "Failed to parse {}: {err}; resetting to defaults",
                    self.path.display(),
                );
                LoadOutcome::Unusable
            }
            OnDisk::OtherVersion(version) => {
                eprintln!(
                    "Settings schema version in {} is {} (current is {}); resetting to defaults",
                    self.path.display(),
                    version,
                    APP_SETTINGS_SCHEMA_VERSION,
                );
                LoadOutcome::Unusable
            }
        }
    }

    fn read(&self) -> OnDisk {
        match fs::read_to_string(&self.path) {
            Ok(content) => match toml::from_str::<AppSettings>(&content) {
                Ok(settings) if settings.version == APP_SETTINGS_SCHEMA_VERSION => {
                    OnDisk::Usable(Box::new(settings))
                }
                Ok(settings) => OnDisk::OtherVersion(settings.version),
                Err(err) => OnDisk::Unparseable(err),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => OnDisk::Missing,
            Err(err) => OnDisk::Unreadable(err),
        }
    }

    /// Replaces the file with the defaults when `loaded` is
    /// [`LoadOutcome::Unusable`] and the file is still not usable, so that the
    /// next run starts from a good file.
    pub(crate) fn reset_if_unusable(&self, loaded: &LoadOutcome) {
        if !matches!(loaded, LoadOutcome::Unusable) {
            return;
        }
        let defaults = AppSettings::default();
        if let Err(err) = self.save_changes(&defaults, &defaults) {
            eprintln!(
                "Failed to overwrite {} with fresh defaults: {err}",
                self.path.display(),
            );
        }
    }

    /// Writes the changes from `base` to `ours` into the file as it is now, so
    /// that the changes that other processes saved meanwhile stay. A file that
    /// is missing, cannot be read or is not usable gets `ours`. Nothing is
    /// written when the file already holds the result. The read and the write
    /// run under the lock of the file; where the system cannot lock, the save
    /// logs that and goes on without the lock.
    pub(crate) fn save_changes(&self, base: &AppSettings, ours: &AppSettings) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = self.lock();
        let merged = match self.read() {
            OnDisk::Usable(theirs) => {
                let merged = merge(base, ours, theirs.as_ref().clone());
                if merged == *theirs {
                    return Ok(());
                }
                merged
            }
            OnDisk::Missing
            | OnDisk::Unreadable(_)
            | OnDisk::Unparseable(_)
            | OnDisk::OtherVersion(_) => ours.clone(),
        };
        self.save(&merged)
    }

    /// Takes the lock that every process holds while it reads and writes the
    /// file, or logs why it cannot and returns `None`.
    fn lock(&self) -> Option<FileLock> {
        match lock_path(&self.path).and_then(|path| FileLock::take(&path)) {
            Ok(lock) => Some(lock),
            Err(err) => {
                eprintln!(
                    "Cannot lock {}: {err}; saving without the lock",
                    self.path.display(),
                );
                None
            }
        }
    }

    /// Writes `settings`, and creates the directory of the file when it is
    /// missing. The write is atomic: when it fails, the previous file stays.
    fn save(&self, settings: &AppSettings) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(settings).map_err(io::Error::other)?;
        write_atomically(&self.path, content.as_bytes())
    }
}

/// What [`SettingsFile::load`] found in the file.
#[derive(Debug)]
#[must_use]
pub(crate) enum LoadOutcome {
    /// The settings that the file holds.
    Read(Box<AppSettings>),
    /// No file, or a file that cannot be read. It stays as it is.
    NotRead,
    /// A file that does not parse or has another schema version.
    /// [`SettingsFile::reset_if_unusable`] replaces it with the defaults.
    Unusable,
}

impl LoadOutcome {
    /// Returns the settings to start with: the ones read, or the defaults.
    pub(crate) fn settings(&self) -> AppSettings {
        match self {
            Self::Read(settings) => settings.as_ref().clone(),
            Self::NotRead | Self::Unusable => AppSettings::default(),
        }
    }
}

/// The settings file as it is on disk.
enum OnDisk {
    Usable(Box<AppSettings>),
    Missing,
    Unreadable(io::Error),
    Unparseable(toml::de::Error),
    OtherVersion(u32),
}

/// The lock of a settings file, held until dropped.
struct FileLock {
    _file: File,
}

impl FileLock {
    fn take(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}

/// Returns the path of the lock file of `target`: a hidden file next to it,
/// which stays after the save.
fn lock_path(target: &Path) -> io::Result<PathBuf> {
    let file_name = target
        .file_name()
        .ok_or_else(|| io::Error::other("settings path has no file name"))?
        .to_string_lossy();
    Ok(target.with_file_name(format!(".{file_name}.lock")))
}

/// Process-wide counter that disambiguates concurrent atomic writes from inside
/// the same process (parallel test threads in particular).
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

/// Writes `contents` to `target` atomically: opens a freshly named temp file
/// next to the target with `create_new(true)`, writes + flushes + fsyncs it,
/// then renames over the target. On any failure the temp file is removed and
/// the previous target (if any) is left intact, so a crash mid-write cannot
/// truncate the user's settings.
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
mod settings_file_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use sc_print_primitives::{AaLevel, MaterialId, PrinterId};
    use tempfile::TempDir;

    use super::*;
    use crate::language::LanguagePreference;
    use crate::{MenuLayout, ThemePreference};

    /// Returns the file `settings.toml` in `dir`.
    fn settings_file_in(dir: &TempDir) -> SettingsFile {
        SettingsFile {
            path: dir.path().join("settings.toml"),
        }
    }

    #[test]
    fn missing_file_loads_defaults() {
        let tmp = TempDir::new().unwrap();
        let file = SettingsFile {
            path: tmp.path().join("does_not_exist.toml"),
        };
        let loaded = file.load();
        assert!(matches!(loaded, LoadOutcome::NotRead), "got {loaded:?}");
        let s = loaded.settings();
        assert!(matches!(s.theme(), ThemePreference::System));
        assert_eq!(s.language(), LanguagePreference::System);
    }

    #[test]
    fn malformed_toml_resets_and_overwrites_file() {
        // A parse failure is intentionally non-fatal: first-run / hand-edited
        // typo cases must not refuse to launch the GUI. The garbled file is
        // replaced with a fresh defaults file so the next run starts from a
        // known-good state.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, b"this is not valid TOML").unwrap();
        let loaded = file.load();
        assert!(matches!(loaded, LoadOutcome::Unusable), "got {loaded:?}");
        assert_eq!(fs::read(&file.path).unwrap(), b"this is not valid TOML");
        let s = loaded.settings();
        assert!(matches!(s.theme(), ThemePreference::System));
        assert_eq!(s.version, APP_SETTINGS_SCHEMA_VERSION);

        file.reset_if_unusable(&loaded);
        let on_disk = fs::read_to_string(&file.path).unwrap();
        assert!(
            on_disk.contains(&format!("version = {APP_SETTINGS_SCHEMA_VERSION}")),
            "expected garbled file to be replaced with defaults, got:\n{on_disk}"
        );
    }

    #[test]
    fn save_then_load_round_trips_theme_and_language() {
        let tmp = TempDir::new().unwrap();
        let file = SettingsFile {
            path: tmp.path().join("nested").join("settings.toml"),
        };

        let mut s = AppSettings::default();
        s.set_theme(ThemePreference::Dark);
        file.save(&s).unwrap();

        // The save call must create parent directories: the path above includes
        // a `nested/` segment that doesn't exist yet.
        assert!(file.path.exists());

        let loaded = file.load().settings();
        assert!(matches!(loaded.theme(), ThemePreference::Dark));
        assert_eq!(loaded.language(), LanguagePreference::System);
    }

    #[test]
    fn saved_toml_omits_default_language() {
        // System (the default) is `skip_serializing_if`; a fresh config
        // shouldn't carry a redundant `language = "system"` line.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let s = AppSettings::default();
        file.save(&s).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            !written.contains("language ="),
            "default settings should not serialise the language key, got:\n{written}"
        );
    }

    #[test]
    fn explicit_language_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        let bundled = crate::language::BundledLanguage::from_code("es")
            .expect("`es` must be bundled in i18n/");
        s.set_language(LanguagePreference::Bundled(bundled));
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert_eq!(loaded.language(), LanguagePreference::Bundled(bundled));

        // The output must mention `language = "es"` so an external tool (or
        // hand-edit) can find it.
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("language = \"es\""), "got:\n{written}");
    }

    #[test]
    fn an_unbundled_language_keeps_the_other_settings() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(
            &file.path,
            "default_printer = \"elegoo-mars-5\"\n[appearance]\nlanguage = \"xx\"\n",
        )
        .unwrap();

        let loaded = file.load().settings();
        assert_eq!(
            loaded.default_printer(),
            Some(PrinterId::new("elegoo-mars-5").unwrap())
        );
        assert_eq!(loaded.language(), LanguagePreference::System);
    }

    #[test]
    fn an_unbundled_language_stays_on_disk_after_another_change() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "[appearance]\nlanguage = \"xx\"\n").unwrap();

        let mut s = file.load().settings();
        // The saver copies every preference of the snapshot, which holds the
        // system language for an unbundled code.
        s.set_language(LanguagePreference::System);
        s.set_theme(ThemePreference::Dark);
        file.save(&s).unwrap();

        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("language = \"xx\""), "got:\n{written}");
        assert!(written.contains("theme = \"dark\""), "got:\n{written}");
    }

    #[test]
    fn a_malformed_language_keeps_the_other_settings() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(
            &file.path,
            "default_printer = \"elegoo-mars-5\"\n[appearance]\nlanguage = \"\"\n",
        )
        .unwrap();

        let loaded = file.load().settings();
        assert_eq!(
            loaded.default_printer(),
            Some(PrinterId::new("elegoo-mars-5").unwrap())
        );
        assert_eq!(loaded.language(), LanguagePreference::System);
    }

    #[test]
    fn light_theme_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_theme(ThemePreference::Light);
        file.save(&s).unwrap();
        let loaded = file.load().settings();
        assert!(matches!(loaded.theme(), ThemePreference::Light));
    }

    #[test]
    fn compact_menu_layout_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_menu_layout(MenuLayout::Compact);
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert_eq!(loaded.menu_layout(), MenuLayout::Compact);

        // Written as a plain lowercase string so an external tool or a
        // hand-edit can find it.
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            written.contains("menu_layout = \"compact\""),
            "got:\n{written}"
        );
    }

    #[test]
    fn saved_toml_omits_default_menu_layout() {
        // Bar (the default) is `skip_serializing_if`; a fresh config shouldn't
        // carry a redundant `menu_layout = "bar"` line.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            !written.contains("menu_layout"),
            "default settings should not serialise the menu layout key, got:\n{written}"
        );
    }

    #[test]
    fn explicit_aa_level_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_aa_level(AaLevel::X4);
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert_eq!(loaded.aa_level(), AaLevel::X4);

        // Written as a plain lowercase string so an external tool or a
        // hand-edit can find it.
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("aa_level = \"x4\""), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_default_aa_level() {
        // Off (the default) is `skip_serializing_if`; a fresh config shouldn't
        // carry a redundant `aa_level = "off"` line.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            !written.contains("aa_level"),
            "default settings should not serialise the AA level key, got:\n{written}"
        );
    }

    #[test]
    fn explicit_default_printer_and_resin_round_trip() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        let printer = PrinterId::new("anycubic-photon-m3").unwrap();
        let resin = MaterialId::new("abs-like-405").unwrap();
        s.set_default_printer(Some(printer.clone()));
        s.set_default_resin(Some(resin.clone()));
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert_eq!(loaded.default_printer(), Some(printer));
        assert_eq!(loaded.default_resin(), Some(resin));

        // The ids are written as plain strings so an external tool or a
        // hand-edit can find them.
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            written.contains("default_printer = \"anycubic-photon-m3\""),
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
        let file = settings_file_in(&tmp);
        let s = AppSettings::default();
        file.save(&s).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
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
    fn favourite_printers_and_resins_round_trip() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        let p1 = PrinterId::new("anycubic-photon-m3").unwrap();
        let p2 = PrinterId::new("elegoo-mars-4-ultra").unwrap();
        let r1 = MaterialId::new("abs-like-405").unwrap();
        let printers: BTreeSet<PrinterId> = [p1, p2].into_iter().collect();
        let resins: BTreeSet<MaterialId> = [r1].into_iter().collect();
        s.set_favourite_printers(printers.clone());
        s.set_favourite_resins(resins.clone());
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert_eq!(loaded.favourite_printers(), printers);
        assert_eq!(loaded.favourite_resins(), resins);

        // Written as plain string arrays for external tooling / hand-edits.
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            written.contains("favourite_printers = ["),
            "got:\n{written}"
        );
        assert!(written.contains("elegoo-mars-4-ultra"), "got:\n{written}");
        assert!(written.contains("abs-like-405"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_empty_favourites() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            !written.contains("favourite_printers"),
            "empty favourites should not serialise, got:\n{written}"
        );
        assert!(
            !written.contains("favourite_resins"),
            "empty favourites should not serialise, got:\n{written}"
        );
    }

    #[test]
    fn settings_without_favourites_keys_load_empty() {
        // A pre-favourites config (no favourite keys, no version) must load
        // with empty favourites and stay on the current schema version rather
        // than reset to defaults.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "default_printer = \"anycubic-photon-m3\"\n").unwrap();

        let loaded = file.load().settings();
        assert!(loaded.favourite_printers().is_empty());
        assert!(loaded.favourite_resins().is_empty());
        assert_eq!(loaded.version, APP_SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            loaded.default_printer(),
            Some(PrinterId::new("anycubic-photon-m3").unwrap())
        );
    }

    #[test]
    fn the_per_printer_output_format_round_trips() {
        use sc_print_primitives::{FormatKind, NonEmptyString, OutputFormatBinding};

        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let printer = PrinterId::new("elegoo-mars-5").unwrap();
        let chosen: BTreeMap<PrinterId, OutputFormatBinding> = [(
            printer.clone(),
            OutputFormatBinding {
                format: FormatKind::Goo,
                version: Some(NonEmptyString::new("v3.0").unwrap()),
            },
        )]
        .into_iter()
        .collect();

        let mut s = AppSettings::default();
        s.set_output_format_by_printer(chosen.clone());
        file.save(&s).unwrap();

        assert_eq!(file.load().settings().output_format_by_printer(), chosen);

        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            written.contains("[output_format_by_printer.elegoo-mars-5]"),
            "got:\n{written}",
        );
    }

    #[test]
    fn saved_toml_omits_an_empty_per_printer_output_format_map() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();

        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            !written.contains("output_format_by_printer"),
            "an empty map should not serialise, got:\n{written}",
        );
    }

    #[test]
    fn hidden_object_list_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_object_list_visible(false);
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert!(!loaded.object_list_visible());

        // Written under a dedicated section so an external tool or a hand-edit
        // can find it.
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("[object_list]"), "got:\n{written}");
        assert!(written.contains("visible = false"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_default_object_list() {
        // Visible (the default) is skipped, and with no other field set the
        // whole section is omitted so a fresh config stays minimal.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            !written.contains("object_list"),
            "default settings should not serialise the object-list section, got:\n{written}"
        );
    }

    #[test]
    fn missing_object_list_section_loads_visible() {
        // A settings file predating the object-list section loads with the
        // panel visible (its default).
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "[appearance]\ntheme = \"dark\"\n").unwrap();
        let loaded = file.load().settings();
        assert!(loaded.object_list_visible());
    }

    #[test]
    fn object_list_geometry_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_object_list_anchor_right(false);
        s.set_object_list_anchor_bottom(true);
        s.set_object_list_offset_x(Some(48.0));
        s.set_object_list_offset_y(Some(64.0));
        s.set_object_list_width(Some(280.0));
        s.set_object_list_height(Some(200.0));
        s.set_object_list_minimized(true);
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert!(!loaded.object_list_anchor_right());
        assert!(loaded.object_list_anchor_bottom());
        assert_eq!(loaded.object_list_offset_x(), Some(48.0));
        assert_eq!(loaded.object_list_offset_y(), Some(64.0));
        assert_eq!(loaded.object_list_width(), Some(280.0));
        assert_eq!(loaded.object_list_height(), Some(200.0));
        assert!(loaded.object_list_minimized());

        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("[object_list]"), "got:\n{written}");
        assert!(written.contains("anchor_right = false"), "got:\n{written}");
        assert!(written.contains("anchor_bottom = true"), "got:\n{written}");
        assert!(written.contains("minimized = true"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_unset_object_list_geometry() {
        // Unset offsets and size stay absent from the section; an unset height
        // keeps the panel auto-sizing on the next launch.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_object_list_minimized(true);
        file.save(&s).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(!written.contains("offset_x"), "got:\n{written}");
        assert!(!written.contains("offset_y"), "got:\n{written}");
        assert!(!written.contains("width ="), "got:\n{written}");
        assert!(!written.contains("height ="), "got:\n{written}");
    }

    #[test]
    fn slice_panel_geometry_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_slice_panel_anchor_right(true);
        s.set_slice_panel_anchor_bottom(true);
        s.set_slice_panel_offset_x(Some(24.0));
        s.set_slice_panel_offset_y(Some(36.0));
        s.set_slice_panel_width(Some(300.0));
        s.set_slice_panel_height(Some(220.0));
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert!(loaded.slice_panel_anchor_right());
        assert!(loaded.slice_panel_anchor_bottom());
        assert_eq!(loaded.slice_panel_offset_x(), Some(24.0));
        assert_eq!(loaded.slice_panel_offset_y(), Some(36.0));
        assert_eq!(loaded.slice_panel_width(), Some(300.0));
        assert_eq!(loaded.slice_panel_height(), Some(220.0));

        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("[slice_panel]"), "got:\n{written}");
        assert!(written.contains("anchor_right = true"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_default_slice_panel() {
        // A panel left at its default position and size writes no section at
        // all.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(!written.contains("[slice_panel]"), "got:\n{written}");
    }

    #[test]
    fn hidden_slice_view_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_slice_view_visible(false);
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert!(!loaded.slice_view_visible());

        // Written under a dedicated section so an external tool or a hand-edit
        // can find it.
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("[slice_view]"), "got:\n{written}");
        assert!(written.contains("visible = false"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_omits_default_slice_view() {
        // Visible (the default) with default geometry writes no section so a
        // fresh config stays minimal.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            !written.contains("slice_view"),
            "default settings should not serialise the slice-view section, got:\n{written}"
        );
    }

    #[test]
    fn missing_slice_view_section_loads_visible() {
        // A settings file predating the slice-view section loads with the panel
        // visible (its default).
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "[appearance]\ntheme = \"dark\"\n").unwrap();
        let loaded = file.load().settings();
        assert!(loaded.slice_view_visible());
    }

    #[test]
    fn slice_view_geometry_round_trips() {
        // Move the panel to the top-right corner (both anchors flipped from
        // their defaults) so the round trip exercises an `anchor_bottom =
        // false` that differs from the default `true`.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_slice_view_anchor_right(true);
        s.set_slice_view_anchor_bottom(false);
        s.set_slice_view_offset_x(Some(48.0));
        s.set_slice_view_offset_y(Some(64.0));
        s.set_slice_view_width(Some(300.0));
        s.set_slice_view_height(Some(240.0));
        s.set_slice_view_minimized(true);
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert!(loaded.slice_view_anchor_right());
        assert!(!loaded.slice_view_anchor_bottom());
        assert_eq!(loaded.slice_view_offset_x(), Some(48.0));
        assert_eq!(loaded.slice_view_offset_y(), Some(64.0));
        assert_eq!(loaded.slice_view_width(), Some(300.0));
        assert_eq!(loaded.slice_view_height(), Some(240.0));
        assert!(loaded.slice_view_minimized());

        let written = fs::read_to_string(&file.path).unwrap();
        assert!(written.contains("[slice_view]"), "got:\n{written}");
        assert!(written.contains("anchor_right = true"), "got:\n{written}");
        assert!(written.contains("anchor_bottom = false"), "got:\n{written}");
        assert!(written.contains("minimized = true"), "got:\n{written}");
    }

    #[test]
    fn last_open_dir_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_last_open_dir(Some(PathBuf::from("/home/user/models")));
        file.save(&s).unwrap();

        let loaded = file.load().settings();
        assert_eq!(
            loaded.last_open_dir(),
            Some(PathBuf::from("/home/user/models"))
        );

        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            written.contains("last_open_dir = \"/home/user/models\""),
            "got:\n{written}"
        );
    }

    #[test]
    fn saved_toml_omits_unset_last_open_dir() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(!written.contains("last_open_dir"), "got:\n{written}");
    }

    #[test]
    fn saved_toml_includes_schema_version() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let written = fs::read_to_string(&file.path).unwrap();
        assert!(
            written.contains(&format!("version = {APP_SETTINGS_SCHEMA_VERSION}")),
            "saved file must stamp the current schema version, got:\n{written}"
        );
    }

    #[test]
    fn save_then_load_preserves_schema_version() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        file.save(&AppSettings::default()).unwrap();
        let loaded = file.load().settings();
        assert_eq!(loaded.version, APP_SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn missing_version_field_loads_as_current() {
        // Files written before this field existed do not carry a `version` key.
        // Serde's default fills in the current schema version so the legacy
        // file loads without triggering a reset.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "[appearance]\ntheme = \"dark\"\n").unwrap();
        let loaded = file.load().settings();
        assert_eq!(loaded.version, APP_SETTINGS_SCHEMA_VERSION);
        assert!(matches!(loaded.theme(), ThemePreference::Dark));
        // The legacy file is preserved verbatim (no rewrite happens until the
        // user makes a change that triggers a save).
        let on_disk = fs::read_to_string(&file.path).unwrap();
        assert!(!on_disk.contains("version ="), "got:\n{on_disk}");
    }

    #[test]
    fn future_schema_version_resets_and_overwrites_file() {
        // A settings file from a hypothetical future build (version ahead of
        // the current one) is not migrated; the file is replaced with a fresh
        // defaults file stamped at the current schema version.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let future = APP_SETTINGS_SCHEMA_VERSION + 999;
        fs::write(
            &file.path,
            format!("version = {future}\n[appearance]\ntheme = \"dark\"\n"),
        )
        .unwrap();

        let loaded = file.load();
        assert!(matches!(loaded, LoadOutcome::Unusable), "got {loaded:?}");
        let before_reset = fs::read_to_string(&file.path).unwrap();
        assert!(before_reset.contains(&format!("version = {future}")));
        let s = loaded.settings();
        assert!(matches!(s.theme(), ThemePreference::System));
        assert_eq!(s.version, APP_SETTINGS_SCHEMA_VERSION);

        file.reset_if_unusable(&loaded);
        let on_disk = fs::read_to_string(&file.path).unwrap();
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
        // A settings file with a version below the current schema is also
        // smashed; no migration code is kept in pre-alpha. `0` stands in for
        // "any version older than the first real schema", which is the only
        // direction below the current one.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "version = 0\n").unwrap();

        let loaded = file.load();
        assert!(matches!(loaded, LoadOutcome::Unusable), "got {loaded:?}");
        assert_eq!(fs::read_to_string(&file.path).unwrap(), "version = 0\n");
        assert_eq!(loaded.settings().version, APP_SETTINGS_SCHEMA_VERSION);

        file.reset_if_unusable(&loaded);
        let on_disk = fs::read_to_string(&file.path).unwrap();
        assert!(
            on_disk.contains(&format!("version = {APP_SETTINGS_SCHEMA_VERSION}")),
            "expected file to be replaced with current version, got:\n{on_disk}"
        );
    }

    #[test]
    fn a_save_leaves_no_temp_file() {
        // The atomic-write helper places a `.settings.toml.tmp.<pid>.<n>` file
        // next to the target and renames it into place. A successful save must
        // leave exactly the target behind, with no leftover temp file polluting
        // the config directory across repeated saves.
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);

        let s = AppSettings::default();
        file.save(&s).unwrap();
        file.save(&s).unwrap();
        file.save(&s).unwrap();

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

    #[test]
    fn the_platform_file_is_settings_toml_in_the_config_directory() {
        let expected =
            sc_app_paths::project_dirs().map(|dirs| dirs.config_dir().join("settings.toml"));
        assert_eq!(
            SettingsFile::platform_default().map(|file| file.path),
            expected
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_loads_defaults_and_stays_as_it_is() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        // The last byte is not UTF-8, so the read fails before the parse.
        let bytes = b"[appearance]\ntheme = \"dark\"\n\xff";
        fs::write(&file.path, bytes).unwrap();

        let loaded = file.load();
        assert!(matches!(loaded, LoadOutcome::NotRead), "got {loaded:?}");
        assert!(matches!(loaded.settings().theme(), ThemePreference::System));
        file.reset_if_unusable(&loaded);
        assert_eq!(fs::read(&file.path).unwrap(), bytes);
    }

    #[test]
    fn a_good_file_is_read_and_kept() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut s = AppSettings::default();
        s.set_theme(ThemePreference::Dark);
        file.save(&s).unwrap();
        let before = fs::read(&file.path).unwrap();

        let loaded = file.load();
        let LoadOutcome::Read(read) = &loaded else {
            panic!("got {loaded:?}");
        };
        assert!(matches!(read.theme(), ThemePreference::Dark));
        assert!(matches!(loaded.settings().theme(), ThemePreference::Dark));
        file.reset_if_unusable(&loaded);
        assert_eq!(fs::read(&file.path).unwrap(), before);
    }

    #[test]
    fn a_failed_rename_keeps_the_target_and_leaves_no_temp_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("settings.toml");
        // A file cannot replace a directory, so the rename fails.
        fs::create_dir(&target).unwrap();
        fs::write(target.join("kept"), b"kept").unwrap();

        assert!(write_atomically(&target, b"new").is_err());
        assert_eq!(fs::read(target.join("kept")).unwrap(), b"kept");
        let entries: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("settings.toml")]);
    }

    #[test]
    fn a_write_into_a_missing_directory_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("missing").join("settings.toml");

        let err = write_atomically(&target, b"new").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_temp_path_is_a_hidden_sibling_of_the_target() {
        let temp = atomic_temp_path(Path::new("dir/settings.toml")).unwrap();
        assert_eq!(temp.parent(), Some(Path::new("dir")));
        let name = temp.file_name().unwrap().to_string_lossy().into_owned();
        let prefix = format!(".settings.toml.tmp.{}.", std::process::id());
        assert!(name.starts_with(&prefix), "got {name}");
    }

    #[test]
    fn each_temp_path_of_a_target_is_new() {
        let target = Path::new("dir/settings.toml");
        assert_ne!(
            atomic_temp_path(target).unwrap(),
            atomic_temp_path(target).unwrap()
        );
    }

    #[test]
    fn a_bare_file_name_gets_a_temp_path_in_the_current_directory() {
        let temp = atomic_temp_path(Path::new("settings.toml")).unwrap();
        assert_eq!(temp.parent(), Some(Path::new(".")));
    }

    #[test]
    fn a_target_with_no_file_name_has_no_temp_path() {
        assert!(atomic_temp_path(Path::new("/")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_save_that_changes_nothing_does_not_write() {
        use std::os::unix::fs::MetadataExt;

        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let mut theirs = AppSettings::default();
        theirs.set_theme(ThemePreference::Dark);
        file.save(&theirs).unwrap();
        let before = fs::metadata(&file.path).unwrap();

        let base = AppSettings::default();
        file.save_changes(&base, &base).unwrap();
        // The file already holds the change of `ours`.
        file.save_changes(&base, &theirs).unwrap();

        let after = fs::metadata(&file.path).unwrap();
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    }

    #[test]
    fn a_change_of_another_process_stays() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let base = AppSettings::default();
        let mut theirs = base.clone();
        theirs.set_theme(ThemePreference::Dark);
        file.save(&theirs).unwrap();
        let mut ours = base.clone();
        ours.set_aa_level(AaLevel::X4);

        file.save_changes(&base, &ours).unwrap();

        let saved = file.load().settings();
        assert!(matches!(saved.theme(), ThemePreference::Dark));
        assert_eq!(saved.aa_level(), AaLevel::X4);
    }

    #[test]
    fn an_unusable_file_is_replaced_by_ours() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "this is not valid TOML").unwrap();
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);

        file.save_changes(&base, &ours).unwrap();
        assert_eq!(file.load().settings(), ours);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_replaced_by_ours() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, b"[appearance]\ntheme = \"light\"\n\xff").unwrap();
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);

        file.save_changes(&base, &ours).unwrap();
        assert_eq!(file.load().settings(), ours);
    }

    #[test]
    fn a_missing_file_is_created_with_ours() {
        let tmp = TempDir::new().unwrap();
        let file = SettingsFile {
            path: tmp.path().join("nested").join("settings.toml"),
        };
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);

        file.save_changes(&base, &ours).unwrap();
        assert_eq!(file.load().settings(), ours);
    }

    #[test]
    fn the_lock_file_stays_beside_the_settings() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);

        file.save_changes(&base, &ours).unwrap();

        let mut entries: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            [
                std::ffi::OsString::from(".settings.toml.lock"),
                std::ffi::OsString::from("settings.toml"),
            ]
        );
    }

    #[test]
    fn a_lock_that_cannot_be_taken_still_saves() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        // A directory cannot be opened for writing, so the lock fails.
        fs::create_dir(tmp.path().join(".settings.toml.lock")).unwrap();
        let base = AppSettings::default();
        let mut ours = base.clone();
        ours.set_theme(ThemePreference::Dark);

        file.save_changes(&base, &ours).unwrap();
        assert_eq!(file.load().settings(), ours);
    }

    /// Saves 20 changes to the file at `path` one by one, as a saver does:
    /// `change(settings, i)` makes change `i` on the base of the save before.
    fn save_20_changes(path: PathBuf, change: impl Fn(&mut AppSettings, usize)) {
        let file = SettingsFile { path };
        let mut base = AppSettings::default();
        for i in 0..20 {
            let mut ours = base.clone();
            change(&mut ours, i);
            file.save_changes(&base, &ours).unwrap();
            base = ours;
        }
    }

    #[test]
    fn concurrent_savers_keep_every_change() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let printers = {
            let path = path.clone();
            std::thread::spawn(move || {
                save_20_changes(path, |settings, i| {
                    let mut ids = settings.favourite_printers();
                    ids.insert(PrinterId::new(&format!("printer-{i}")).unwrap());
                    settings.set_favourite_printers(ids);
                });
            })
        };
        let resins = {
            let path = path.clone();
            std::thread::spawn(move || {
                save_20_changes(path, |settings, i| {
                    let mut ids = settings.favourite_resins();
                    ids.insert(MaterialId::new(&format!("resin-{i}")).unwrap());
                    settings.set_favourite_resins(ids);
                });
            })
        };
        printers.join().unwrap();
        resins.join().unwrap();

        let saved = SettingsFile { path }.load().settings();
        assert_eq!(saved.favourite_printers().len(), 20);
        assert_eq!(saved.favourite_resins().len(), 20);
    }

    #[test]
    fn the_reset_leaves_a_file_that_became_usable() {
        let tmp = TempDir::new().unwrap();
        let file = settings_file_in(&tmp);
        fs::write(&file.path, "this is not valid TOML").unwrap();
        let loaded = file.load();
        // Another process writes a good file before the reset.
        let mut theirs = AppSettings::default();
        theirs.set_theme(ThemePreference::Dark);
        file.save(&theirs).unwrap();

        file.reset_if_unusable(&loaded);
        assert_eq!(file.load().settings(), theirs);
    }

    #[test]
    fn the_lock_path_is_a_hidden_sibling_of_the_target() {
        assert_eq!(
            lock_path(Path::new("dir/settings.toml")).unwrap(),
            Path::new("dir/.settings.toml.lock")
        );
    }

    #[test]
    fn a_target_with_no_file_name_has_no_lock_path() {
        assert!(lock_path(Path::new("/")).is_err());
    }
}

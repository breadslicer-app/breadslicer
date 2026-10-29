/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Help > Export Diagnostics (#74): a `.zip` the person saves and attaches to
//! a bug report, with the logs, this session's events, the build, the
//! machine, the preferences without folders and the scene in numbers. The
//! scene's models go in only when the person ticks them.
//!
//! Nothing here sends anything anywhere: the file is written where the
//! person chooses, and only reaches anyone if they send it themselves.

mod bundle;
mod scene;
pub(crate) mod scrub;
pub(crate) mod system;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_channel::oneshot;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts, format_anyhow_chain};
use crate::i18n::ctr;
use crate::preferences_bus::PreferencesBus;
use crate::print_selection::PrintDefaults;
use crate::scene_document::SceneDocument;
use crate::slint_ui::MainWindow;
use bundle::Contents;
use scrub::Scrubber;
use system::ScreenFacts;

/// The commit this build was made from, as the build set it in
/// `BREADSLICER_COMMIT`.
pub(crate) fn commit() -> &'static str {
    option_env!("BREADSLICER_COMMIT")
        .filter(|commit| !commit.is_empty())
        .unwrap_or("not recorded")
}

/// Shows what a diagnostics file holds over `ui` and, on Save, asks where
/// and writes it.
pub(crate) fn export(
    ui: &MainWindow,
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    prefs: Rc<PreferencesBus>,
) {
    crate::breadcrumbs::note("diagnostics: export asked");
    let ui = ui.as_weak();
    let shown = dialogs.clone();
    shown.show_diagnostics_export(doc.is_loaded(), move |include_models| {
        if let Err(err) = slint::spawn_local(async move {
            save(ui, doc, dialogs, prefs, include_models).await;
        }) {
            eprintln!("Failed to spawn the diagnostics export: {err}");
        }
    });
}

/// Asks where to save the file, gathers what goes in on the UI thread and
/// writes it on a worker, then says how it went.
async fn save(
    ui: slint::Weak<MainWindow>,
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    prefs: Rc<PreferencesBus>,
    include_models: bool,
) {
    let (file_stamp, exported) = utc_stamps(SystemTime::now());
    let target = dialogs
        .file_dialog()
        .add_filter(ctr!("FileDialog", "ZIP files").to_string(), &["zip"])
        .set_file_name(format!("breadslicer-diagnostics-{file_stamp}.zip"))
        .save_file()
        .await
        .map(|handle| with_zip_extension(handle.path()));
    let Some(target) = target else {
        crate::breadcrumbs::note("diagnostics: export cancelled");
        return;
    };
    let snapshot = prefs.snapshot();
    let screens = ui
        .upgrade()
        .map_or_else(ScreenFacts::default, |ui| ScreenFacts::of(ui.window()));
    let language = format!(
        "{:?} (system {})",
        snapshot.language,
        sys_locale::get_locale().unwrap_or_else(|| "unknown".to_owned())
    );
    let models = include_models
        .then(|| {
            let defaults = PrintDefaults::from(&snapshot);
            crate::scene_save::prepare_save_for_target(&doc, "scene.bread".into(), &defaults)
        })
        .flatten()
        .map(|(bundle, _, _)| bundle);
    crate::breadcrumbs::started("diagnostics");
    let contents = Contents {
        system: system::report(&exported, &language, &screens, snapshot.diagnostic_log),
        scene: scene::report(&doc),
        events: crate::breadcrumbs::recent(),
        logs_dir: crate::breadcrumbs::logs_dir(),
        settings_file: crate::app_settings::file::SettingsFile::platform_default()
            .map(|file| file.path().to_path_buf()),
        models,
    };
    let (tx, rx) = oneshot::channel();
    let written_to = target.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bundle::write(&contents, Scrubber::of_this_user(), &written_to)
        }))
        .unwrap_or_else(|payload| {
            Err(anyhow::anyhow!(crate::mesh_loader::panic_message(&payload)))
        });
        let _ = slint::invoke_from_event_loop(move || {
            let _ = tx.send(result);
        });
    });
    let Ok(result) = rx.await else {
        return;
    };
    crate::breadcrumbs::finished("diagnostics", &crate::breadcrumbs::outcome(&result));
    match result {
        Ok(names) => {
            let file_name = file_name(&target);
            let details = names.join("\n");
            dialogs.show_info(move || saved_texts(&file_name, &details));
        }
        Err(err) => {
            let file_name = file_name(&target);
            dialogs.show_error(move || ErrorTexts {
                heading: ctr!("DiagnosticsDialog", "Could not export the diagnostics"),
                context: Some(ctr!("ErrorDialog", "File: {}", file_name)),
                message: err.to_string().into(),
                details: format_anyhow_chain(&err).into(),
            });
        }
    }
}

/// The notice of a diagnostics file saved as `file_name`, which shows the
/// entries in `details`.
fn saved_texts(file_name: &str, details: &str) -> InfoTexts {
    InfoTexts {
        heading: ctr!("DiagnosticsDialog", "Diagnostics saved"),
        message: ctr!(
            "DiagnosticsDialog",
            "Attach {} to your bug report. Nothing was sent: Breadslicer never sends us anything by itself.",
            file_name
        ),
        details: details.into(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// `path`, ending in `.zip`.
fn with_zip_extension(path: &Path) -> PathBuf {
    let is_zip = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    if is_zip {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_owned();
        name.push(".zip");
        PathBuf::from(name)
    }
}

/// `now` in UTC, for the file name (`20261029-2114`) and for reading
/// (`2026-10-29 21:14`).
fn utc_stamps(now: SystemTime) -> (String, String) {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
    let (hour, minute) = (rest / 3600, rest % 3600 / 60);
    (
        format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}"),
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}"),
    )
}

/// The date of the day `days` after 1970-01-01, in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    #[test]
    fn days_become_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn the_stamps_read_the_time_in_utc() {
        let now = UNIX_EPOCH + Duration::from_secs(19_723 * 86_400 + 21 * 3600 + 14 * 60 + 59);
        assert_eq!(
            utc_stamps(now),
            ("20240101-2114".to_owned(), "2024-01-01 21:14".to_owned())
        );
    }

    #[test]
    fn the_file_always_ends_in_zip() {
        assert_eq!(
            with_zip_extension(Path::new("a/report")),
            PathBuf::from("a/report.zip")
        );
        assert_eq!(
            with_zip_extension(Path::new("a/report.ZIP")),
            PathBuf::from("a/report.ZIP")
        );
        assert_eq!(
            with_zip_extension(Path::new("a/report.v2")),
            PathBuf::from("a/report.v2.zip")
        );
    }

    #[test]
    fn a_build_without_a_commit_says_so() {
        assert!(!commit().is_empty());
    }
}

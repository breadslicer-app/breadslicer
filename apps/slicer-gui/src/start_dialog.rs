/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The start dialog (#381): what the app offers when it opens with no file to
//! open. Reopen the last file, open a recent one, open a project, import a
//! model, or start an empty scene.
//!
//! It never stands in the way: it shows only after the recovery question
//! (#18) has been answered, never when the app opened with a file, and only
//! once the window is on screen. Whether the recent files are still there,
//! and their thumbnails, are read on a worker after that, so the launch does
//! not wait for a slow or missing drive (#359).
//!
//! It comes back when the person empties the scene, deleting its last object
//! or closing it (#397), unless they turned it off.
//!
//! The dialog itself (`ui/start-dialog.slint`) hides while a file loads and
//! closes for good once the window holds a scene or objects again, whichever
//! way they got there (an undo of that delete too).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use futures_channel::oneshot;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::error_dialog::RecoveryChoice;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::recent_files::{self, RecentFile};
use crate::{MainWindow, RecentFileRow};

/// How the recovery question (#18) went at this launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// There was nothing to recover, so nothing was asked.
    NothingAsked,
    /// The person answered it.
    Answered(RecoveryChoice),
}

/// Whether the start dialog shows at this launch: not when the app opened with
/// a file, not when the person turned it off, and not when the recovery put the
/// scenes back.
pub(crate) fn shows_at_launch(
    opened_with_files: bool,
    show_at_startup: bool,
    recovery: Recovery,
) -> bool {
    !opened_with_files && show_at_startup && recovery != Recovery::Answered(RecoveryChoice::Recover)
}

/// Whether the start dialog shows again once the person emptied the scene
/// (#397): it is the same dialog, so "Don't show at startup" keeps it away
/// here too, and it never covers a job still running or a message.
pub(crate) fn shows_when_emptied(
    show_at_startup: bool,
    job_running: bool,
    message_open: bool,
) -> bool {
    show_at_startup && !job_running && !message_open
}

/// Whether `window` runs a job the start dialog must not cover: a load, a
/// save, a slice, a hollow, supports, a repair or an orientation. Fix all
/// runs through these.
pub(crate) fn runs_a_job(window: &MainWindow) -> bool {
    window.get_loading()
        || window.get_load_in_progress()
        || window.get_saving_in_progress()
        || window.get_slicing_in_progress()
        || window.get_hollow_running()
        || window.get_support_gen_in_progress()
        || window.get_repairing_in_progress()
        || window.get_orient_running()
}

/// The largest thumbnail read, a side in pixels (the `.bread` spec's limit).
const MAX_THUMBNAIL_SIDE: u32 = 512;

/// What the worker found for 1 recent file.
struct Scan {
    path: PathBuf,
    missing: bool,
    thumbnail: Option<SharedPixelBuffer<Rgba8Pixel>>,
}

/// What the dialog knows of 1 recent file once the worker has looked.
struct Seen {
    path: PathBuf,
    missing: bool,
    thumbnail: Option<Image>,
}

/// The start dialog of 1 main window: fills its list from the preferences and
/// acts on what the person picks there.
pub(crate) struct StartDialog {
    window: slint::Weak<MainWindow>,
    prefs: Rc<PreferencesBus>,
    /// What the worker found; empty until it reports.
    seen: RefCell<Vec<Seen>>,
    /// The files of the list as shown, in its order.
    shown: RefCell<Vec<PathBuf>>,
    /// Opens a file in the window, asking first about a scene there.
    open: Rc<dyn Fn(PathBuf)>,
    subscription: RefCell<Option<Subscription>>,
}

impl StartDialog {
    /// Binds the start dialog of `window` to `prefs`, opening the files picked
    /// through `open`. The dialog stays closed until [`Self::show`].
    pub(crate) fn install(
        window: &MainWindow,
        prefs: &Rc<PreferencesBus>,
        open: Rc<dyn Fn(PathBuf)>,
    ) -> Rc<Self> {
        let dialog = Rc::new(Self {
            window: window.as_weak(),
            prefs: prefs.clone(),
            seen: RefCell::new(Vec::new()),
            shown: RefCell::new(Vec::new()),
            open,
            subscription: RefCell::new(None),
        });
        let weak = Rc::downgrade(&dialog);
        *dialog.subscription.borrow_mut() = Some(prefs.subscribe(move |_, change| {
            if matches!(
                change,
                PreferencesChange::RecentFiles | PreferencesChange::Language
            ) && let Some(dialog) = weak.upgrade()
            {
                dialog.refresh();
            }
        }));
        dialog.wire(window);
        window.set_start_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
        dialog
    }

    fn wire(self: &Rc<Self>, window: &MainWindow) {
        let weak = Rc::downgrade(self);
        let with = move |act: &dyn Fn(&Rc<Self>)| {
            if let Some(dialog) = weak.upgrade() {
                act(&dialog);
            }
        };
        {
            let with = with.clone();
            window.on_start_open_recent(move |row| with(&|d| d.open_row(row)));
        }
        {
            let with = with.clone();
            window.on_start_reopen_last(move || with(&|d| d.open_row(0)));
        }
        {
            let with = with.clone();
            window.on_start_remove_recent(move |row| with(&|d| d.remove_row(row)));
        }
        window.on_start_dont_show_changed(move |dont_show| {
            with(&|d| {
                d.prefs.update(move |snap| {
                    snap.show_start_dialog = !dont_show;
                    PreferencesChange::StartDialog
                });
            });
        });
    }

    /// Opens the dialog with the list as the preferences hold it, and starts
    /// the worker that checks the files and reads their thumbnails.
    pub(crate) fn show(self: &Rc<Self>) {
        self.refresh();
        if let Some(window) = self.window.upgrade() {
            window.set_start_dont_show(false);
            window.set_start_open(true);
        }
        crate::breadcrumbs::note(&format!(
            "start: dialog shown with {} recent file(s)",
            self.shown.borrow().len()
        ));
        self.scan();
    }

    /// Checks the listed files on a worker, then shows what it found.
    fn scan(self: &Rc<Self>) {
        let paths: Vec<PathBuf> = self.shown.borrow().clone();
        if paths.is_empty() {
            return;
        }
        let (tx, rx) = oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("recent-files".into())
            .spawn(move || {
                let found: Vec<Scan> = paths.into_iter().map(scan_file).collect();
                let _ = slint::invoke_from_event_loop(move || {
                    let _ = tx.send(found);
                });
            });
        if let Err(err) = spawned {
            eprintln!("Failed to start the recent-files worker: {err}");
            return;
        }
        let weak = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            if let Ok(found) = rx.await
                && let Some(dialog) = weak.upgrade()
            {
                dialog.take_scan(found);
            }
        }) {
            eprintln!("Failed to wait for the recent-files worker: {err}");
        }
    }

    /// Shows what a worker would have found: per file, its path, whether it
    /// is gone and its thumbnail.
    #[cfg(test)]
    pub(crate) fn take_scan_for_test(
        &self,
        found: Vec<(PathBuf, bool, Option<SharedPixelBuffer<Rgba8Pixel>>)>,
    ) {
        self.take_scan(
            found
                .into_iter()
                .map(|(path, missing, thumbnail)| Scan {
                    path,
                    missing,
                    thumbnail,
                })
                .collect(),
        );
    }

    fn take_scan(&self, found: Vec<Scan>) {
        *self.seen.borrow_mut() = found
            .into_iter()
            .map(|scan| Seen {
                path: scan.path,
                missing: scan.missing,
                thumbnail: scan.thumbnail.map(Image::from_rgba8),
            })
            .collect();
        self.refresh();
    }

    /// Pushes the list to the window again, from the preferences and what the
    /// worker found.
    fn refresh(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let files = self.prefs.snapshot().recent_files;
        let home = sc_app_paths::home_dir();
        let now = SystemTime::now();
        let seen = self.seen.borrow();
        let rows: Vec<RecentFileRow> = files
            .iter()
            .map(|file| {
                row(
                    file,
                    seen.iter().find(|s| same(&s.path, &file.path)),
                    home.as_deref(),
                    now,
                )
            })
            .collect();
        let last = rows
            .first()
            .filter(|row| !row.missing)
            .map(|row| row.name.clone())
            .unwrap_or_default();
        *self.shown.borrow_mut() = files.into_iter().map(|file| file.path).collect();
        window.set_start_recent(ModelRc::new(VecModel::from(rows)));
        window.set_start_last_name(last);
    }

    /// Opens the file of row `row`, unless the worker found it missing.
    fn open_row(&self, row: i32) {
        let Some(path) = self.path_of(row) else {
            return;
        };
        if self.is_missing(&path) {
            return;
        }
        crate::breadcrumbs::note("start: recent file opened");
        (self.open)(path);
    }

    /// Takes the file of row `row` off the list.
    fn remove_row(&self, row: i32) {
        if let Some(path) = self.path_of(row) {
            recent_files::remove(&self.prefs, path);
        }
    }

    fn path_of(&self, row: i32) -> Option<PathBuf> {
        let row = usize::try_from(row).ok()?;
        self.shown.borrow().get(row).cloned()
    }

    fn is_missing(&self, path: &Path) -> bool {
        self.seen
            .borrow()
            .iter()
            .any(|seen| same(&seen.path, path) && seen.missing)
    }
}

fn same(a: &Path, b: &Path) -> bool {
    recent_files::same_path(a, b)
}

/// The row that the list shows for `file`, with what the worker found, if it
/// has reported.
fn row(
    file: &RecentFile,
    seen: Option<&Seen>,
    home: Option<&Path>,
    now: SystemTime,
) -> RecentFileRow {
    let path = &file.path;
    let thumbnail = seen.and_then(|seen| seen.thumbnail.clone());
    RecentFileRow {
        name: recent_files::name_label(path).into(),
        folder: recent_files::folder_label(path, home).into(),
        when: recent_files::when_label(file.opened, now).into(),
        kind: recent_files::kind_label(path).into(),
        project: recent_files::is_project(path),
        missing: seen.is_some_and(|seen| seen.missing),
        has_thumbnail: thumbnail.is_some(),
        thumbnail: thumbnail.unwrap_or_default(),
    }
}

/// Whether the file at `path` is still there, and its thumbnail when it is a
/// project that has one.
fn scan_file(path: PathBuf) -> Scan {
    let missing = !path.is_file();
    let thumbnail = if !missing && recent_files::is_project(&path) {
        read_thumbnail(&path)
    } else {
        None
    };
    Scan {
        path,
        missing,
        thumbnail,
    }
}

/// The thumbnail of the `.bread` file at `path`, decoded; `None` when it has
/// none, or one that is not a PNG this reads or is larger than the spec
/// allows. Reads the first chunk, where writers put the thumbnail, never the meshes.
pub(crate) fn read_thumbnail(path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let png = sc_project_formats::read_bread_thumbnail(file)?;
    decode_thumbnail(&png)
}

/// Decodes a thumbnail's PNG, checking its size in the header first.
fn decode_thumbnail(png: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    use image::ImageDecoder;

    let decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(png)).ok()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > MAX_THUMBNAIL_SIDE || height > MAX_THUMBNAIL_SIDE {
        return None;
    }
    let rgba = image::DynamicImage::from_decoder(decoder)
        .ok()?
        .into_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::preferences_bus::PreferencesSnapshot;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use slint::Model;

    #[test]
    fn a_launch_with_a_file_skips_the_dialog() {
        assert!(!shows_at_launch(true, true, Recovery::NothingAsked));
        assert!(shows_at_launch(false, true, Recovery::NothingAsked));
    }

    #[test]
    fn the_dialog_turned_off_stays_off() {
        assert!(!shows_at_launch(false, false, Recovery::NothingAsked));
        assert!(!shows_at_launch(
            false,
            false,
            Recovery::Answered(RecoveryChoice::Discard)
        ));
    }

    #[test]
    fn an_emptied_scene_shows_the_dialog_unless_turned_off_busy_or_covered() {
        assert!(shows_when_emptied(true, false, false));
        assert!(!shows_when_emptied(false, false, false));
        assert!(!shows_when_emptied(true, true, false));
        assert!(!shows_when_emptied(true, false, true));
    }

    #[test]
    fn scenes_put_back_by_the_recovery_skip_the_dialog() {
        assert!(!shows_at_launch(
            false,
            true,
            Recovery::Answered(RecoveryChoice::Recover)
        ));
        for choice in [RecoveryChoice::Discard, RecoveryChoice::Later] {
            assert!(shows_at_launch(false, true, Recovery::Answered(choice)));
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(width, height, image::Rgba([215, 167, 90, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("the PNG encodes");
        bytes
    }

    #[test]
    fn a_thumbnail_decodes_and_an_oversized_one_is_none() {
        let small = decode_thumbnail(&png(8, 6)).expect("a small PNG decodes");
        assert_eq!((small.width(), small.height()), (8, 6));
        assert!(decode_thumbnail(&png(513, 4)).is_none());
        assert!(decode_thumbnail(b"not a png").is_none());
    }

    #[test]
    fn a_project_s_thumbnail_is_read_from_its_file() {
        use bread_format::{BreadWriter, ChunkTag, THUMBNAIL_TAG};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("with-thumbnail.bread");
        let mut writer = BreadWriter::new(Vec::new()).unwrap();
        writer.chunk(THUMBNAIL_TAG, 1, &png(16, 16)).unwrap();
        writer.chunk(ChunkTag(*b"SCNE"), 1, &[7; 4096]).unwrap();
        std::fs::write(&path, writer.finish().unwrap()).unwrap();
        let thumbnail = read_thumbnail(&path).expect("the thumbnail reads");
        assert_eq!((thumbnail.width(), thumbnail.height()), (16, 16));

        let scan = scan_file(path);
        assert!(!scan.missing);
        assert!(scan.thumbnail.is_some());
        let gone = scan_file(dir.path().join("gone.bread"));
        assert!(gone.missing);
        assert!(gone.thumbnail.is_none());
    }

    fn snapshot_with(files: &[(&str, u64)]) -> PreferencesSnapshot {
        let mut snap = PreferencesSnapshot::for_tests();
        snap.recent_files = files
            .iter()
            .map(|(path, opened)| RecentFile {
                path: PathBuf::from(path),
                opened: *opened,
            })
            .collect();
        snap
    }

    /// The names the window's list shows.
    fn names(window: &MainWindow) -> Vec<String> {
        let rows = window.get_start_recent();
        (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .map(|row| row.name.to_string())
            .collect()
    }

    #[test]
    fn showing_fills_the_list_and_offers_the_newest_to_reopen() {
        let (open, names, last) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let bus =
                PreferencesBus::new(snapshot_with(&[("/m/hueco.bread", 9), ("/m/cubo.stl", 5)]));
            let dialog = StartDialog::install(&window, &bus, Rc::new(|_| {}));
            dialog.show();
            (
                window.get_start_open(),
                names(&window),
                window.get_start_last_name().to_string(),
            )
        });
        assert!(open);
        assert_eq!(names, ["hueco.bread", "cubo.stl"]);
        assert_eq!(last, "hueco.bread");
    }

    #[test]
    fn a_row_opens_its_file_and_the_x_takes_it_off_the_list() {
        let (opened, names_after) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let bus = PreferencesBus::new(snapshot_with(&[("/m/a.stl", 9), ("/m/b.stl", 5)]));
            let opened = Rc::new(RefCell::new(Vec::new()));
            let sink = opened.clone();
            let dialog = StartDialog::install(
                &window,
                &bus,
                Rc::new(move |path| sink.borrow_mut().push(path)),
            );
            dialog.show();
            window.invoke_start_open_recent(1);
            window.invoke_start_reopen_last();
            window.invoke_start_remove_recent(0);
            let opened = opened.borrow().clone();
            (opened, names(&window))
        });
        assert_eq!(
            opened,
            [PathBuf::from("/m/b.stl"), PathBuf::from("/m/a.stl")]
        );
        assert_eq!(names_after, ["b.stl"]);
    }

    #[test]
    fn a_missing_file_shows_faded_and_does_not_open() {
        let (missing, opened, last) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let bus = PreferencesBus::new(snapshot_with(&[("/gone/a.stl", 9)]));
            let opened = Rc::new(RefCell::new(0));
            let sink = opened.clone();
            let dialog =
                StartDialog::install(&window, &bus, Rc::new(move |_| *sink.borrow_mut() += 1));
            dialog.show();
            dialog.take_scan(vec![Scan {
                path: PathBuf::from("/gone/a.stl"),
                missing: true,
                thumbnail: None,
            }]);
            window.invoke_start_open_recent(0);
            window.invoke_start_reopen_last();
            let row = window.get_start_recent().row_data(0).unwrap();
            (
                row.missing,
                *opened.borrow(),
                window.get_start_last_name().to_string(),
            )
        });
        assert!(missing);
        assert_eq!(opened, 0);
        assert_eq!(last, "", "a missing file is not offered to reopen");
    }

    #[test]
    fn the_checkbox_turns_the_dialog_off_in_the_preferences() {
        let shows = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialog = StartDialog::install(&window, &bus, Rc::new(|_| {}));
            dialog.show();
            window.invoke_start_dont_show_changed(true);
            let off = bus.snapshot().show_start_dialog;
            window.invoke_start_dont_show_changed(false);
            (off, bus.snapshot().show_start_dialog)
        });
        assert_eq!(shows, (false, true));
    }
}

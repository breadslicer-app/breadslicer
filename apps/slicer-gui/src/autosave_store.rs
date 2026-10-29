/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Where the autosaves of a run live, and how a later run finds them again
//! (#18).
//!
//! A run that autosaves owns a folder under the autosave root
//! (`<data>/autosave/<run>`) and holds the folder's `lock` file locked for as
//! long as it runs. Each window with unsaved changes keeps 1 entry there: a
//! `.bread` snapshot of its scene and a manifest that names the snapshot and
//! the file the scene stands for. An entry goes away when its scene is saved,
//! discarded or closed, and the folder when the run ends with no entry left.
//! Entries in a folder that nobody holds locked belong to a run that ended
//! without closing its scenes: the next start offers them back.
//!
//! A snapshot is written whole under a new name before the manifest names it,
//! and the manifest is replaced atomically, so an interruption at any point
//! leaves a complete entry: the new one or the one before.

use std::cell::{Cell, RefCell};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use sc_mesh_formats::scene_writer::write_bytes_atomically;
use serde::{Deserialize, Serialize};

/// The file a run holds locked while it runs.
const LOCK_FILE: &str = "lock";

/// The manifest format this build writes and reads. A manifest of another
/// format is ignored, never misread.
pub(crate) const MANIFEST_FORMAT: u32 = 1;

/// Entries left by a run that ended this long ago are deleted at the next
/// start instead of offered.
pub(crate) const ORPHAN_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What an autosave stands for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) format: u32,
    /// The snapshot's file name, in the manifest's folder.
    pub(crate) snapshot: String,
    /// The file the scene was opened from or last saved to.
    pub(crate) source: PathBuf,
    /// Whether that file is a project, which Save may write back, or a mesh.
    pub(crate) project: bool,
    /// When the snapshot was written, in seconds since the Unix epoch.
    pub(crate) saved_at: u64,
}

fn manifest_name(window: u64) -> String {
    format!("window-{window}.toml")
}

fn snapshot_name(window: u64, seq: u64) -> String {
    format!("window-{window}-{seq}.bread")
}

/// The window a snapshot file name belongs to.
fn snapshot_window(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("window-")?.strip_suffix(".bread")?;
    let (window, seq) = rest.split_once('-')?;
    seq.parse::<u64>().ok()?;
    window.parse().ok()
}

/// Seconds since the Unix epoch.
pub(crate) fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn read_manifest(path: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(path).ok()?;
    let manifest: Manifest = toml::from_str(&text).ok()?;
    (manifest.format == MANIFEST_FORMAT).then_some(manifest)
}

/// Opens the lock file of the run folder `dir` and locks it. Fails with
/// [`io::ErrorKind::WouldBlock`] while another run holds it.
fn lock_run(dir: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

/// Removes the snapshots of `window` in `dir`, all but `keep`.
fn remove_snapshots(dir: &Path, window: u64, keep: Option<&str>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if snapshot_window(name) == Some(window) && Some(name) != keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The complete entries of the run folder `dir`, oldest first.
fn read_entries(dir: &Path) -> Vec<OrphanEntry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<OrphanEntry> = read
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("window-") && name.ends_with(".toml"))
        })
        .filter_map(|entry| read_manifest(&entry.path()))
        .filter(|manifest| snapshot_window(&manifest.snapshot).is_some())
        .map(|manifest| OrphanEntry {
            snapshot: dir.join(&manifest.snapshot),
            manifest,
        })
        .filter(|entry| entry.snapshot.is_file())
        .collect();
    entries.sort_by_key(|entry| entry.manifest.saved_at);
    entries
}

/// The manifest of the autosave at `path`, when `path` is a snapshot in a run
/// folder under `root`.
pub(crate) fn manifest_of(root: &Path, path: &Path) -> Option<Manifest> {
    let dir = path.parent()?;
    if dir.parent()? != root {
        return None;
    }
    let window = snapshot_window(path.file_name()?.to_str()?)?;
    read_manifest(&dir.join(manifest_name(window)))
}

/// The folder of this run, and its lock.
struct Run {
    dir: PathBuf,
    lock: File,
}

/// The autosaves of this run, and the way to those of runs before it.
pub(crate) struct AutosaveStore {
    root: PathBuf,
    /// Made, and locked, at the 1st autosave: a run that never autosaves
    /// leaves nothing behind.
    run: RefCell<Option<Run>>,
    /// Numbers the snapshots of this run, so a new one never takes the name
    /// of the one its manifest still names.
    next_seq: Cell<u64>,
}

impl AutosaveStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            run: RefCell::new(None),
            next_seq: Cell::new(0),
        }
    }

    /// The autosave root in the app's local data folder. Tests never touch
    /// it.
    pub(crate) fn platform_root() -> Option<PathBuf> {
        if cfg!(test) {
            return None;
        }
        Some(
            sc_app_paths::project_dirs()?
                .data_local_dir()
                .join("autosave"),
        )
    }

    /// The folder of this run, made and locked at the 1st call.
    fn run_dir(&self) -> io::Result<PathBuf> {
        if let Some(run) = self.run.borrow().as_ref() {
            return Ok(run.dir.clone());
        }
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let name = format!("{started}-{}", std::process::id());
        fs::create_dir_all(&self.root)?;
        // A folder of its own, even for 2 runs started in the same instant.
        let mut dir = self.root.join(&name);
        let mut again = 1;
        loop {
            match fs::create_dir(&dir) {
                Ok(()) => break,
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    again += 1;
                    dir = self.root.join(format!("{name}-{again}"));
                }
                Err(err) => return Err(err),
            }
        }
        let lock = lock_run(&dir)?;
        *self.run.borrow_mut() = Some(Run {
            dir: dir.clone(),
            lock,
        });
        Ok(dir)
    }

    /// Where the next snapshot of `window` goes.
    pub(crate) fn next_slot(&self, window: u64) -> io::Result<Slot> {
        let dir = self.run_dir()?;
        let seq = self.next_seq.get();
        self.next_seq.set(seq + 1);
        Ok(Slot {
            dir,
            window,
            name: snapshot_name(window, seq),
        })
    }

    /// Removes the entry of `window`, if this run has one: the manifest first,
    /// so no manifest ever names a missing snapshot.
    pub(crate) fn remove_entry(&self, window: u64) {
        let Some(dir) = self.run.borrow().as_ref().map(|run| run.dir.clone()) else {
            return;
        };
        let manifest = dir.join(manifest_name(window));
        if let Err(err) = fs::remove_file(&manifest)
            && err.kind() != io::ErrorKind::NotFound
        {
            crate::breadcrumbs::note(&format!("autosave: entry not removed: {err}"));
            return;
        }
        remove_snapshots(&dir, window, None);
    }

    /// Makes `entry`, left by an earlier run, the entry of `window` in this
    /// run, and returns where its snapshot now is.
    pub(crate) fn adopt(&self, entry: &OrphanEntry, window: u64) -> Result<PathBuf> {
        let slot = self.next_slot(window)?;
        let snapshot = slot.path();
        fs::rename(&entry.snapshot, &snapshot)?;
        slot.publish(&Manifest {
            snapshot: slot.name.clone(),
            ..entry.manifest.clone()
        })?;
        Ok(snapshot)
    }

    /// Ends this run: releases its lock and removes its folder, unless an
    /// entry is still there (a window that never got to close its scene),
    /// which the next start then offers back.
    pub(crate) fn finish(&self) {
        let Some(Run { dir, lock }) = self.run.borrow_mut().take() else {
            return;
        };
        drop(lock);
        if read_entries(&dir).is_empty() {
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// Claims the runs under the root that ended without closing their
    /// scenes: their folders are locked by no one. Runs whose newest entry is
    /// older than [`ORPHAN_MAX_AGE`] at `now`, or with no entry left, are
    /// deleted on the way. The claimed runs stay locked until each is
    /// recovered, discarded or dropped.
    pub(crate) fn claim_orphans(&self, now: SystemTime) -> Vec<OrphanRun> {
        let own = self.run.borrow().as_ref().map(|run| run.dir.clone());
        let Ok(read) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let now = unix_seconds(now);
        let mut orphans = Vec::new();
        for entry in read.flatten() {
            let dir = entry.path();
            if !dir.is_dir() || own.as_ref() == Some(&dir) {
                continue;
            }
            // Locked: another run of the app, still running.
            let Ok(lock) = lock_run(&dir) else {
                continue;
            };
            let entries = read_entries(&dir);
            let run = OrphanRun { dir, lock, entries };
            let newest = run.entries.iter().map(|e| e.manifest.saved_at).max();
            let stale =
                newest.is_none_or(|saved| now.saturating_sub(saved) > ORPHAN_MAX_AGE.as_secs());
            if stale {
                run.discard();
            } else {
                orphans.push(run);
            }
        }
        orphans.sort_by_key(|run| run.entries.first().map(|e| e.manifest.saved_at));
        orphans
    }
}

/// Where 1 snapshot of a window goes. `Send`: the write runs on a worker.
#[derive(Debug)]
pub(crate) struct Slot {
    dir: PathBuf,
    window: u64,
    name: String,
}

impl Slot {
    pub(crate) fn path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Names the snapshot, once written, in the window's manifest, then
    /// removes the window's older snapshots.
    pub(crate) fn publish(&self, manifest: &Manifest) -> Result<()> {
        let text = toml::to_string(manifest)?;
        write_bytes_atomically(&self.dir.join(manifest_name(self.window)), text.as_bytes())?;
        remove_snapshots(&self.dir, self.window, Some(&self.name));
        Ok(())
    }

    /// Takes back a snapshot that turned out not to be wanted, with its
    /// manifest if it was published.
    pub(crate) fn withdraw(&self) {
        let manifest = self.dir.join(manifest_name(self.window));
        if read_manifest(&manifest).is_some_and(|m| m.snapshot == self.name) {
            let _ = fs::remove_file(manifest);
        }
        let _ = fs::remove_file(self.path());
    }
}

/// A run that ended without closing its scenes, claimed by this one.
#[derive(Debug)]
pub(crate) struct OrphanRun {
    dir: PathBuf,
    lock: File,
    /// Its scenes, oldest first.
    pub(crate) entries: Vec<OrphanEntry>,
}

impl OrphanRun {
    /// Deletes the run's folder with whatever it still holds.
    pub(crate) fn discard(self) {
        let Self { dir, lock, .. } = self;
        drop(lock);
        if let Err(err) = fs::remove_dir_all(&dir) {
            crate::breadcrumbs::note(&format!("autosave: {} not removed: {err}", dir.display()));
        }
    }
}

/// 1 scene of an [`OrphanRun`].
#[derive(Clone, Debug)]
pub(crate) struct OrphanEntry {
    pub(crate) snapshot: PathBuf,
    pub(crate) manifest: Manifest,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(NOW)
    }

    /// Writes an entry for `window` the way the autosaver does.
    fn write_entry(store: &AutosaveStore, window: u64, source: &str, saved_at: u64) -> Slot {
        let slot = store.next_slot(window).expect("a slot");
        fs::write(slot.path(), b"snapshot").expect("the snapshot is written");
        slot.publish(&Manifest {
            format: MANIFEST_FORMAT,
            snapshot: slot.name().to_owned(),
            source: PathBuf::from(source),
            project: true,
            saved_at,
        })
        .expect("the manifest is written");
        slot
    }

    fn files_of(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("the folder exists")
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn snapshot_names_tell_their_window() {
        assert_eq!(snapshot_window("window-3-17.bread"), Some(3));
        assert_eq!(snapshot_window("window-12-0.bread"), Some(12));
        assert_eq!(snapshot_window("window-3.toml"), None);
        assert_eq!(snapshot_window("window-3-x.bread"), None);
        assert_eq!(snapshot_window("hueco.bread"), None);
    }

    #[test]
    fn a_run_that_never_autosaves_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().join("autosave"));
        store.remove_entry(0);
        store.finish();
        assert!(!root.path().join("autosave").exists());
    }

    #[test]
    fn a_new_snapshot_replaces_the_window_s_older_one() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().to_path_buf());
        write_entry(&store, 1, "a.bread", NOW);
        let newer = write_entry(&store, 1, "a.bread", NOW + 120);
        let dir = newer.dir.clone();
        assert_eq!(
            files_of(&dir),
            vec!["lock", "window-1-1.bread", "window-1.toml"]
        );
        let manifest = read_manifest(&dir.join("window-1.toml")).unwrap();
        assert_eq!(manifest.snapshot, "window-1-1.bread");
    }

    #[test]
    fn removing_an_entry_keeps_the_other_windows() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().to_path_buf());
        write_entry(&store, 1, "a.bread", NOW);
        let other = write_entry(&store, 12, "b.bread", NOW);
        store.remove_entry(1);
        assert_eq!(
            files_of(&other.dir),
            vec!["lock", "window-12-1.bread", "window-12.toml"]
        );
    }

    #[test]
    fn withdrawing_takes_back_only_what_the_slot_wrote() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().to_path_buf());
        let kept = write_entry(&store, 1, "a.bread", NOW);
        let unpublished = store.next_slot(1).unwrap();
        fs::write(unpublished.path(), b"late").unwrap();
        unpublished.withdraw();
        assert_eq!(
            files_of(&kept.dir),
            vec!["lock", "window-1-0.bread", "window-1.toml"]
        );
        let published = write_entry(&store, 1, "a.bread", NOW + 1);
        published.withdraw();
        assert_eq!(files_of(&kept.dir), vec!["lock"]);
    }

    #[test]
    fn a_run_that_ended_without_closing_its_scenes_is_offered_back() {
        let root = tempfile::tempdir().unwrap();
        let crashed = AutosaveStore::new(root.path().to_path_buf());
        write_entry(&crashed, 1, "C:/models/hueco.bread", NOW - 60);
        write_entry(&crashed, 2, "C:/models/cubo.stl", NOW - 30);
        // The run ends without `finish`: its lock goes with it.
        drop(crashed);

        let next = AutosaveStore::new(root.path().to_path_buf());
        let orphans = next.claim_orphans(now());
        assert_eq!(orphans.len(), 1);
        let sources: Vec<_> = orphans[0]
            .entries
            .iter()
            .map(|e| e.manifest.source.clone())
            .collect();
        assert_eq!(
            sources,
            vec![
                PathBuf::from("C:/models/hueco.bread"),
                PathBuf::from("C:/models/cubo.stl")
            ]
        );
        // Claimed: a 3rd run starting now does not get them too.
        let third = AutosaveStore::new(root.path().to_path_buf());
        assert!(third.claim_orphans(now()).is_empty());
    }

    #[test]
    fn a_running_run_is_never_claimed() {
        let root = tempfile::tempdir().unwrap();
        let running = AutosaveStore::new(root.path().to_path_buf());
        write_entry(&running, 1, "a.bread", NOW);
        let other = AutosaveStore::new(root.path().to_path_buf());
        assert!(other.claim_orphans(now()).is_empty());
        drop(running);
    }

    #[test]
    fn its_own_run_is_not_an_orphan() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().to_path_buf());
        write_entry(&store, 1, "a.bread", NOW);
        assert!(store.claim_orphans(now()).is_empty());
    }

    #[test]
    fn stale_and_empty_runs_are_deleted_not_offered() {
        let root = tempfile::tempdir().unwrap();
        let old = AutosaveStore::new(root.path().to_path_buf());
        write_entry(&old, 1, "a.bread", NOW - ORPHAN_MAX_AGE.as_secs() - 1);
        let old_dir = old.run_dir().unwrap();
        drop(old);
        let empty = AutosaveStore::new(root.path().to_path_buf());
        let slot = write_entry(&empty, 1, "b.bread", NOW);
        empty.remove_entry(1);
        drop(empty);

        let next = AutosaveStore::new(root.path().to_path_buf());
        assert!(next.claim_orphans(now()).is_empty());
        assert!(!old_dir.exists());
        assert!(!slot.dir.exists());
    }

    #[test]
    fn adopting_moves_the_entry_into_this_run() {
        let root = tempfile::tempdir().unwrap();
        let crashed = AutosaveStore::new(root.path().to_path_buf());
        write_entry(&crashed, 4, "C:/models/hueco.bread", NOW);
        drop(crashed);

        let next = AutosaveStore::new(root.path().to_path_buf());
        let orphan = next.claim_orphans(now()).pop().unwrap();
        let adopted = next.adopt(&orphan.entries[0], 1).expect("adopted");
        orphan.discard();

        assert_eq!(fs::read(&adopted).unwrap(), b"snapshot");
        let manifest = manifest_of(root.path(), &adopted).expect("a manifest");
        assert_eq!(manifest.source, PathBuf::from("C:/models/hueco.bread"));
        assert_eq!(
            manifest.snapshot,
            adopted.file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn finishing_removes_the_run_unless_a_scene_is_left() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().to_path_buf());
        let slot = write_entry(&store, 1, "a.bread", NOW);
        store.remove_entry(1);
        store.finish();
        assert!(!slot.dir.exists());

        let store = AutosaveStore::new(root.path().to_path_buf());
        let slot = write_entry(&store, 1, "a.bread", NOW);
        store.finish();
        assert!(slot.path().exists());
        let next = AutosaveStore::new(root.path().to_path_buf());
        assert_eq!(next.claim_orphans(now()).len(), 1);
    }

    #[test]
    fn manifests_are_found_only_for_snapshots_under_the_root() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().to_path_buf());
        let slot = write_entry(&store, 1, "a.bread", NOW);
        assert!(manifest_of(root.path(), &slot.path()).is_some());
        let elsewhere = tempfile::tempdir().unwrap();
        assert!(manifest_of(elsewhere.path(), &slot.path()).is_none());
        assert!(manifest_of(root.path(), &root.path().join("a.bread")).is_none());
    }

    #[test]
    fn a_manifest_of_another_format_is_ignored() {
        let root = tempfile::tempdir().unwrap();
        let store = AutosaveStore::new(root.path().to_path_buf());
        let slot = store.next_slot(1).unwrap();
        fs::write(slot.path(), b"snapshot").unwrap();
        slot.publish(&Manifest {
            format: MANIFEST_FORMAT + 1,
            snapshot: slot.name().to_owned(),
            source: PathBuf::from("a.bread"),
            project: true,
            saved_at: NOW,
        })
        .unwrap();
        assert!(manifest_of(root.path(), &slot.path()).is_none());
        drop(store);
        let next = AutosaveStore::new(root.path().to_path_buf());
        assert!(next.claim_orphans(now()).is_empty());
    }
}

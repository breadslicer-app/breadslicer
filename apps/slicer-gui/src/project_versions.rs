/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Earlier versions of each project (#18). A save over an existing `.bread`
//! first keeps the file it is about to replace, in the app's local data folder
//! (never beside the project): `<data>/versions/<project>/<time>.bread`, named
//! after the time that file was saved. Each project keeps its newest
//! [`KEEP_PER_PROJECT`] versions, and all projects together at most
//! [`MAX_TOTAL_BYTES`]; the oldest go first.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use sc_mesh_formats::scene_writer::write_bytes_atomically;
use serde::{Deserialize, Serialize};

/// Versions kept for each project.
pub(crate) const KEEP_PER_PROJECT: usize = 10;

/// The most all versions together may take.
pub(crate) const MAX_TOTAL_BYTES: u64 = 1 << 30;

/// The file, in each project's folder, that says which project it is.
const PROJECT_FILE: &str = "project.toml";

const RECORD_FORMAT: u32 = 1;

/// Which project a folder of versions belongs to.
#[derive(Debug, Serialize, Deserialize)]
struct ProjectRecord {
    format: u32,
    source: PathBuf,
}

/// 1 version on disk.
#[derive(Debug)]
struct Version {
    path: PathBuf,
    modified: SystemTime,
    size: u64,
}

fn is_bread(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("bread"))
}

/// The versions in the project folder `dir`, oldest first.
fn versions_in(dir: &Path) -> Vec<Version> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut versions: Vec<Version> = read
        .flatten()
        .filter(|entry| is_bread(&entry.path()))
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
            Some(Version {
                path: entry.path(),
                modified: meta.modified().ok()?,
                size: meta.len(),
            })
        })
        .collect();
    versions.sort_by(|a, b| (a.modified, &a.path).cmp(&(b.modified, &b.path)));
    versions
}

/// A stable 64-bit key for `path` (FNV-1a), so 2 projects of the same name in
/// different folders keep their versions apart. Windows paths compare without
/// case.
fn path_key(path: &Path) -> u64 {
    let text = path.to_string_lossy();
    let text = if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.into_owned()
    };
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The project's name, as a folder name can carry it.
fn readable_stem(path: &Path) -> String {
    let stem: String = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let stem = stem.trim().to_owned();
    if stem.is_empty() {
        "project".to_owned()
    } else {
        stem
    }
}

/// A free file name in `dir` for a version saved at `saved`: its local date
/// and time, with ` (2)`, ` (3)`... when that is taken.
fn free_name(dir: &Path, saved: SystemTime) -> PathBuf {
    let stamp = chrono::DateTime::<chrono::Local>::from(saved)
        .format("%Y-%m-%d %H-%M-%S")
        .to_string();
    let mut path = dir.join(format!("{stamp}.bread"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{stamp} ({n}).bread"));
    }
    path
}

/// The versions of every project. `Send`: a save keeps the version on its
/// worker.
#[derive(Clone, Debug)]
pub(crate) struct ProjectVersions {
    root: PathBuf,
    keep: usize,
    max_total: u64,
}

impl ProjectVersions {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            keep: KEEP_PER_PROJECT,
            max_total: MAX_TOTAL_BYTES,
        }
    }

    /// The versions in the app's local data folder. Tests never touch it.
    pub(crate) fn platform_default() -> Option<Self> {
        if cfg!(test) {
            return None;
        }
        Some(Self::new(
            sc_app_paths::project_dirs()?
                .data_local_dir()
                .join("versions"),
        ))
    }

    /// The folder for the versions of `project`.
    pub(crate) fn dir_for(&self, project: &Path) -> PathBuf {
        let full = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
        self.root
            .join(format!("{}-{:016x}", readable_stem(&full), path_key(&full)))
    }

    /// The folder of `project`'s versions, when it holds any.
    pub(crate) fn existing_dir_for(&self, project: &Path) -> Option<PathBuf> {
        let dir = self.dir_for(project);
        (!versions_in(&dir).is_empty()).then_some(dir)
    }

    /// The project that `version` is a version of, when it is a file in 1 of
    /// the project folders.
    pub(crate) fn project_of(&self, version: &Path) -> Option<PathBuf> {
        let dir = version.parent()?;
        if dir.parent()? != self.root || !is_bread(version) {
            return None;
        }
        let text = fs::read_to_string(dir.join(PROJECT_FILE)).ok()?;
        let record: ProjectRecord = toml::from_str(&text).ok()?;
        (record.format == RECORD_FORMAT).then_some(record.source)
    }

    /// Keeps the `.bread` at `target`, which a save is about to replace, as a
    /// version of it. Does nothing for a file that does not exist yet, for any
    /// other format, or when the newest version kept is the same file. Returns
    /// the version written.
    pub(crate) fn keep_previous(&self, target: &Path) -> Result<Option<PathBuf>> {
        if !is_bread(target) || !target.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(target)?;
        let saved = fs::metadata(target)?
            .modified()
            .unwrap_or_else(|_| SystemTime::now());
        let dir = self.dir_for(target);
        fs::create_dir_all(&dir)?;
        let full = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
        let record = toml::to_string(&ProjectRecord {
            format: RECORD_FORMAT,
            source: full,
        })?;
        write_bytes_atomically(&dir.join(PROJECT_FILE), record.as_bytes())?;
        let newest = versions_in(&dir).pop();
        if newest.is_some_and(|v| {
            v.size == bytes.len() as u64 && fs::read(&v.path).is_ok_and(|b| b == bytes)
        }) {
            return Ok(None);
        }
        let path = free_name(&dir, saved);
        write_bytes_atomically(&path, &bytes)?;
        // The time the version was saved, so versions sort by it.
        File::options()
            .write(true)
            .open(&path)?
            .set_modified(saved)?;
        self.prune(&dir, &path);
        Ok(Some(path))
    }

    /// Keeps the newest versions of the project in `dir`, then removes the
    /// oldest versions of any project until all fit the total; never
    /// `written`.
    fn prune(&self, dir: &Path, written: &Path) {
        let versions = versions_in(dir);
        let excess = versions.len().saturating_sub(self.keep);
        for version in versions.iter().take(excess) {
            if version.path != written {
                remove(&version.path);
            }
        }

        let Ok(read) = fs::read_dir(&self.root) else {
            return;
        };
        let projects: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        let mut all: Vec<Version> = projects.iter().flat_map(|p| versions_in(p)).collect();
        all.sort_by(|a, b| (a.modified, &a.path).cmp(&(b.modified, &b.path)));
        let mut total: u64 = all.iter().map(|v| v.size).sum();
        for version in all {
            if total <= self.max_total {
                break;
            }
            if version.path != written {
                remove(&version.path);
                total = total.saturating_sub(version.size);
            }
        }
        for project in projects {
            if versions_in(&project).is_empty() {
                let _ = fs::remove_dir_all(project);
            }
        }
    }
}

fn remove(path: &Path) {
    if let Err(err) = fs::remove_file(path)
        && err.kind() != io::ErrorKind::NotFound
    {
        crate::breadcrumbs::note(&format!("versions: {} not removed: {err}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
    }

    /// Saves `bytes` to `path` as if at `saved`.
    fn save(path: &Path, bytes: &[u8], saved: SystemTime) {
        fs::write(path, bytes).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(saved)
            .unwrap();
    }

    fn kept_bytes(versions: &ProjectVersions, project: &Path) -> Vec<Vec<u8>> {
        versions_in(&versions.dir_for(project))
            .iter()
            .map(|v| fs::read(&v.path).unwrap())
            .collect()
    }

    #[test]
    fn a_save_over_a_project_keeps_the_file_it_replaces() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let versions = ProjectVersions::new(data.path().to_path_buf());
        let project = work.path().join("hueco.bread");
        save(&project, b"first", at(0));

        let kept = versions
            .keep_previous(&project)
            .unwrap()
            .expect("a version");
        assert_eq!(fs::read(&kept).unwrap(), b"first");
        assert_eq!(
            versions.project_of(&kept),
            Some(std::path::absolute(&project).unwrap())
        );
        assert_eq!(
            versions.existing_dir_for(&project),
            Some(versions.dir_for(&project))
        );
        let name = kept.file_name().unwrap().to_str().unwrap();
        assert!(name.ends_with(".bread") && name.starts_with("20"), "{name}");
    }

    #[test]
    fn nothing_is_kept_for_a_new_file_or_another_format() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let versions = ProjectVersions::new(data.path().to_path_buf());
        let new = work.path().join("new.bread");
        assert_eq!(versions.keep_previous(&new).unwrap(), None);
        let other = work.path().join("plate.3mf");
        save(&other, b"3mf", at(0));
        assert_eq!(versions.keep_previous(&other).unwrap(), None);
        assert_eq!(versions.existing_dir_for(&new), None);
        assert_eq!(fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_same_file_twice_is_kept_once() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let versions = ProjectVersions::new(data.path().to_path_buf());
        let project = work.path().join("hueco.bread");
        save(&project, b"same", at(0));
        assert!(versions.keep_previous(&project).unwrap().is_some());
        assert_eq!(versions.keep_previous(&project).unwrap(), None);
        assert_eq!(kept_bytes(&versions, &project), vec![b"same".to_vec()]);
    }

    #[test]
    fn two_saves_in_the_same_second_keep_both() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let versions = ProjectVersions::new(data.path().to_path_buf());
        let project = work.path().join("hueco.bread");
        save(&project, b"one", at(0));
        versions.keep_previous(&project).unwrap();
        save(&project, b"two", at(0));
        versions.keep_previous(&project).unwrap();
        assert_eq!(kept_bytes(&versions, &project).len(), 2);
    }

    #[test]
    fn each_project_keeps_its_newest_versions() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut versions = ProjectVersions::new(data.path().to_path_buf());
        versions.keep = 3;
        let project = work.path().join("hueco.bread");
        for i in 0..5u8 {
            save(&project, &[i], at(u64::from(i) * 60));
            versions.keep_previous(&project).unwrap();
        }
        assert_eq!(
            kept_bytes(&versions, &project),
            vec![vec![2], vec![3], vec![4]]
        );
    }

    #[test]
    fn the_total_drops_the_oldest_of_any_project_never_the_newest() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut versions = ProjectVersions::new(data.path().to_path_buf());
        versions.max_total = 10;
        let a = work.path().join("a.bread");
        let b = work.path().join("b.bread");
        save(&a, b"aaaa", at(0));
        versions.keep_previous(&a).unwrap();
        save(&b, b"bbbb", at(60));
        versions.keep_previous(&b).unwrap();
        save(&a, b"AAAA", at(120));
        versions.keep_previous(&a).unwrap();
        // 12 bytes over a 10-byte total: the oldest, a's first, goes.
        assert_eq!(kept_bytes(&versions, &a), vec![b"AAAA".to_vec()]);
        assert_eq!(kept_bytes(&versions, &b), vec![b"bbbb".to_vec()]);

        // A version bigger than the total still keeps itself, alone.
        save(&b, b"0123456789ABC", at(180));
        versions.keep_previous(&b).unwrap();
        assert_eq!(kept_bytes(&versions, &b), vec![b"0123456789ABC".to_vec()]);
        assert!(kept_bytes(&versions, &a).is_empty());
        assert!(!versions.dir_for(&a).exists());
    }

    #[test]
    fn projects_of_the_same_name_keep_apart() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let versions = ProjectVersions::new(data.path().to_path_buf());
        fs::create_dir(work.path().join("one")).unwrap();
        fs::create_dir(work.path().join("two")).unwrap();
        let first = work.path().join("one/hueco.bread");
        let second = work.path().join("two/hueco.bread");
        assert_ne!(versions.dir_for(&first), versions.dir_for(&second));
        save(&first, b"one", at(0));
        save(&second, b"two", at(0));
        versions.keep_previous(&first).unwrap();
        versions.keep_previous(&second).unwrap();
        assert_eq!(kept_bytes(&versions, &first), vec![b"one".to_vec()]);
        assert_eq!(kept_bytes(&versions, &second), vec![b"two".to_vec()]);
    }

    #[test]
    fn only_files_in_a_project_folder_are_versions() {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let versions = ProjectVersions::new(data.path().to_path_buf());
        let project = work.path().join("hueco.bread");
        assert_eq!(versions.project_of(&project), None);
        assert_eq!(versions.project_of(&data.path().join("loose.bread")), None);
    }

    #[test]
    fn folder_names_keep_only_safe_characters() {
        assert_eq!(
            readable_stem(Path::new("C:/a/Wolverine v2.bread")),
            "Wolverine v2"
        );
        assert_eq!(readable_stem(Path::new("C:/a/hueco:*?.bread")), "hueco___");
        assert_eq!(readable_stem(Path::new("C:/a/   .bread")), "project");
    }
}

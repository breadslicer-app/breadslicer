/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The diagnostics file: a `.zip` the person saves and sends with a report.
//! Every text in it goes through [`Scrubber`]; the scene's models go in only
//! when the person asks for them.

use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use zip::write::{SimpleFileOptions, ZipWriter};

use super::scrub::Scrubber;
use crate::scene_save::SaveBundle;

/// The most of each log that goes in: its last 5 MiB, where the problem is.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// The keys of the settings file that name a folder or a file, or hold a list
/// of files (the recent files, #381), dropped at any depth.
const PATH_KEY_ENDINGS: [&str; 4] = ["_dir", "_path", "_file", "_files"];

/// What goes into a diagnostics file, gathered on the UI thread and written
/// on a worker.
pub(crate) struct Contents {
    /// `system.txt`.
    pub(crate) system: String,
    /// `scene.txt`.
    pub(crate) scene: String,
    /// This session's events kept in memory, for `events.txt`.
    pub(crate) events: Vec<String>,
    /// The app's logs folder: each file in it goes under `logs/`.
    pub(crate) logs_dir: Option<PathBuf>,
    /// The settings file, which goes in without its folders.
    pub(crate) settings_file: Option<PathBuf>,
    /// The scene as a project, when the person asked for the models.
    pub(crate) models: Option<SaveBundle>,
}

impl std::fmt::Debug for Contents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contents")
            .field("events", &self.events.len())
            .field("logs_dir", &self.logs_dir)
            .field("models", &self.models.is_some())
            .finish_non_exhaustive()
    }
}

/// Builds the diagnostics file in memory. Returns its bytes and the names
/// of the entries, in order.
pub(crate) fn build(contents: &Contents, scrubber: &Scrubber) -> Result<(Vec<u8>, Vec<String>)> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut names = Vec::new();
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, bytes: &[u8]| -> Result<()> {
        zip.start_file(name, options)?;
        zip.write_all(bytes)?;
        names.push(name.to_owned());
        Ok(())
    };
    add("system.txt", scrubber.scrub(&contents.system).as_bytes())?;
    add("scene.txt", scrubber.scrub(&contents.scene).as_bytes())?;
    let mut events = contents.events.join("\n");
    events.push('\n');
    add("events.txt", scrubber.scrub(&events).as_bytes())?;
    if let Some(dir) = &contents.logs_dir {
        for (name, text) in read_logs(dir) {
            add(&format!("logs/{name}"), scrubber.scrub(&text).as_bytes())?;
        }
    }
    if let Some(text) = contents
        .settings_file
        .as_deref()
        .and_then(settings_without_paths)
    {
        add("settings.toml", scrubber.scrub(&text).as_bytes())?;
    }
    if let Some(models) = &contents.models {
        let bytes = crate::scene_save::encode_bundle(models, |_, _, _| {})
            .context("Could not put the scene's models in")?;
        add("scene.bread", &bytes)?;
    }
    let bytes = zip.finish()?.into_inner();
    Ok((bytes, names))
}

/// Builds the diagnostics file and writes it to `target`, atomically.
/// Returns the names of its entries.
pub(crate) fn write(
    contents: &Contents,
    scrubber: &Scrubber,
    target: &Path,
) -> Result<Vec<String>> {
    let (bytes, names) = build(contents, scrubber)?;
    sc_mesh_formats::scene_writer::write_bytes_atomically(target, &bytes)?;
    Ok(names)
}

/// Each file of the logs folder, by name, with the text of its last
/// [`MAX_LOG_BYTES`].
fn read_logs(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut logs: Vec<(String, String)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((name, read_tail(&entry.path())?))
        })
        .collect();
    logs.sort();
    logs
}

/// The last [`MAX_LOG_BYTES`] of the file at `path`, as text.
fn read_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let skipped = len.saturating_sub(MAX_LOG_BYTES);
    file.seek(SeekFrom::Start(skipped)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if skipped > 0 {
        format!("[the first {skipped} bytes are left out]\n{text}")
    } else {
        text
    })
}

/// The settings file at `path` without the keys that name a folder or a
/// file; `None` when there is none. A file that does not parse goes in as
/// it is, to be scrubbed like the rest.
fn settings_without_paths(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let Ok(mut table) = text.parse::<toml::Table>() else {
        return Some(format!("# this settings file does not parse\n{text}"));
    };
    drop_path_keys(&mut table);
    toml::to_string(&table).ok()
}

fn drop_path_keys(table: &mut toml::Table) {
    table.retain(|key, _| !PATH_KEY_ENDINGS.iter().any(|end| key.ends_with(end)));
    for (_, value) in table.iter_mut() {
        drop_path_keys_in(value);
    }
}

/// [`drop_path_keys`] in the tables `value` is or holds, in arrays too.
fn drop_path_keys_in(value: &mut toml::Value) {
    match value {
        toml::Value::Table(inner) => drop_path_keys(inner),
        toml::Value::Array(items) => items.iter_mut().for_each(drop_path_keys_in),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Read;

    fn dana() -> Scrubber {
        Scrubber::new(Some(Path::new(r"C:\Users\Dana")), Some("Dana"))
    }

    fn entries(bytes: Vec<u8>) -> Vec<(String, Vec<u8>)> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut file = zip.by_index(i).unwrap();
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).unwrap();
                (file.name().to_owned(), bytes)
            })
            .collect()
    }

    fn contents_in(dir: &Path) -> Contents {
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("crash.log"),
            "=== panic at unix 1 (v0.1.0): boom at C:\\Users\\Dana\\src\\x.rs\n--- last events\n",
        )
        .unwrap();
        std::fs::write(logs.join("session.log"), "  1.000s load: started (.stl)\n").unwrap();
        let settings = dir.join("settings.toml");
        std::fs::write(
            &settings,
            "version = 1\nlast_open_dir = 'C:\\Users\\Dana\\Desktop'\n\n[appearance]\ntheme = \"dark\"\n",
        )
        .unwrap();
        Contents {
            system: "version: 0.1.0\n".to_owned(),
            scene: "models: 1\n".to_owned(),
            events: vec![
                "  0.001s start v0.1.0".to_owned(),
                "  2.000s tab: Supports".to_owned(),
            ],
            logs_dir: Some(logs),
            settings_file: Some(settings),
            models: None,
        }
    }

    #[test]
    fn the_file_holds_the_logs_the_events_the_system_and_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let (bytes, names) = build(&contents_in(dir.path()), &dana()).unwrap();
        assert_eq!(
            names,
            [
                "system.txt",
                "scene.txt",
                "events.txt",
                "logs/crash.log",
                "logs/session.log",
                "settings.toml",
            ]
        );
        let entries = entries(bytes);
        let text = |name: &str| {
            let (_, bytes) = entries.iter().find(|(n, _)| n == name).unwrap();
            String::from_utf8(bytes.clone()).unwrap()
        };
        assert!(text("events.txt").ends_with("tab: Supports\n"));
        assert!(text("logs/crash.log").contains(r"boom at ~\src\x.rs"));
        let settings = text("settings.toml");
        assert!(settings.contains("theme = \"dark\""), "{settings}");
        assert!(!settings.contains("last_open_dir"), "{settings}");
    }

    #[test]
    fn nothing_in_the_file_names_the_person_and_no_models_go_in_unasked() {
        let dir = tempfile::tempdir().unwrap();
        let mut contents = contents_in(dir.path());
        contents
            .events
            .push(r"  3.000s save: failed: cannot write C:\Users\Dana\x.bread".to_owned());
        contents.system.push_str("user: dana\n");
        let (bytes, names) = build(&contents, &dana()).unwrap();
        assert!(!names.iter().any(|n| n.ends_with(".bread")));
        for (name, bytes) in entries(bytes) {
            let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
            assert!(!text.contains("dana"), "{name} names the person: {text}");
        }
    }

    #[test]
    fn a_long_log_goes_in_by_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        let mut text = "x".repeat(MAX_LOG_BYTES as usize);
        text.push_str("the end\n");
        std::fs::write(&path, &text).unwrap();
        let tail = read_tail(&path).unwrap();
        assert!(tail.starts_with("[the first 8 bytes are left out]\n"));
        assert!(tail.ends_with("the end\n"));
    }

    #[test]
    fn the_recent_files_never_reach_the_diagnostics_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut contents = contents_in(dir.path());
        let settings = dir.path().join("settings.toml");
        std::fs::write(
            &settings,
            "version = 1\nshow_start_dialog = false\n\n[[recent_files]]\npath = 'D:\\Clientes\\Dana\\dragon.bread'\nopened = 1800000000\n\n[[recent_files]]\npath = '/home/dana/minis/hueco.stl'\nopened = 1799999000\n\n[appearance]\ntheme = \"dark\"\n",
        )
        .unwrap();
        contents.settings_file = Some(settings);
        let (bytes, _) = build(&contents, &dana()).unwrap();
        let (_, settings) = entries(bytes)
            .into_iter()
            .find(|(name, _)| name == "settings.toml")
            .unwrap();
        let settings = String::from_utf8(settings).unwrap();
        assert!(settings.contains("theme = \"dark\""), "{settings}");
        assert!(settings.contains("show_start_dialog = false"), "{settings}");
        for leak in [
            "recent_files",
            "dragon",
            "hueco",
            "Clientes",
            "minis",
            "1800000000",
        ] {
            assert!(!settings.contains(leak), "{leak} in {settings}");
        }
    }

    #[test]
    fn a_path_key_inside_a_list_of_tables_is_dropped() {
        let mut table: toml::Table = "[[panels]]\nlog_path = 'y'\nwidth = 2\n".parse().unwrap();
        drop_path_keys(&mut table);
        let text = toml::to_string(&table).unwrap();
        assert!(
            !text.contains("log_path") && text.contains("width = 2"),
            "{text}"
        );
    }

    #[test]
    fn a_folder_key_is_dropped_at_any_depth() {
        let mut table: toml::Table = "a_dir = 'x'\nkeep = 1\n[panel]\nlog_path = 'y'\nwidth = 2\n"
            .parse()
            .unwrap();
        drop_path_keys(&mut table);
        let text = toml::to_string(&table).unwrap();
        assert!(
            !text.contains("a_dir") && !text.contains("log_path"),
            "{text}"
        );
        assert!(
            text.contains("keep = 1") && text.contains("width = 2"),
            "{text}"
        );
    }
}

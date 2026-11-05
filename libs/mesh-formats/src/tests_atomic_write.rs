/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A write through [`write_atomically`] either replaces the target whole or
//! leaves it as it was, with nothing left behind.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use tempfile::tempdir;

use crate::error::WriteSceneError;
use crate::scene_writer::write_atomically;

fn entries_of(folder: &Path) -> Vec<String> {
    fs::read_dir(folder)
        .expect("the folder reads")
        .map(|entry| {
            entry
                .expect("the entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn a_write_replaces_the_target_with_all_it_wrote() {
    let folder = tempdir().expect("a folder");
    let target = folder.path().join("plate.3mf");
    fs::write(&target, b"before").expect("the old file");
    write_atomically::<WriteSceneError, _>(&target, |file| {
        file.write_all(b"after")?;
        file.write_all(b" and more")?;
        Ok(())
    })
    .expect("the write ends");
    assert_eq!(fs::read(&target).expect("the file"), b"after and more");
    assert_eq!(entries_of(folder.path()), ["plate.3mf"]);
}

#[test]
fn a_failure_halfway_keeps_the_target_and_leaves_no_temp_file() {
    let folder = tempdir().expect("a folder");
    let target = folder.path().join("plate.3mf");
    fs::write(&target, b"before").expect("the old file");
    let result = write_atomically::<WriteSceneError, _>(&target, |file| {
        file.write_all(&vec![7; 4 << 20])?;
        Err(io::Error::from(io::ErrorKind::StorageFull).into())
    });
    assert!(matches!(result, Err(WriteSceneError::Io(_))), "{result:?}");
    assert_eq!(fs::read(&target).expect("the file"), b"before");
    assert_eq!(entries_of(folder.path()), ["plate.3mf"]);
}

#[test]
fn a_panic_halfway_keeps_the_target_and_leaves_no_temp_file() {
    let folder = tempdir().expect("a folder");
    let target = folder.path().join("plate.3mf");
    fs::write(&target, b"before").expect("the old file");
    let wrote = AtomicBool::new(false);
    let outcome = std::panic::catch_unwind(|| {
        let _ = write_atomically::<WriteSceneError, _>(&target, |file| {
            file.write_all(b"half")?;
            wrote.store(true, Ordering::Relaxed);
            panic!("the worker panics");
        });
    });
    assert!(outcome.is_err());
    assert!(wrote.load(Ordering::Relaxed));
    assert_eq!(fs::read(&target).expect("the file"), b"before");
    assert_eq!(entries_of(folder.path()), ["plate.3mf"]);
}

#[test]
fn a_target_in_a_missing_folder_is_an_error_and_nothing_is_called() {
    let folder = tempdir().expect("a folder");
    let target = folder.path().join("gone").join("plate.3mf");
    let result = write_atomically::<WriteSceneError, _>(&target, |_| {
        panic!("there is no file to write to");
    });
    assert!(
        matches!(result, Err(WriteSceneError::CreateTempFile { .. })),
        "{result:?}"
    );
}

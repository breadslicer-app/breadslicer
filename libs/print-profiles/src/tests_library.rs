/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::library::{UserLibraryDirError, user_library_dir};

#[test]
fn user_library_dir_returns_a_profile_rooted_path_when_home_is_available() {
    match user_library_dir() {
        Ok(path) => {
            let last = path.file_name().unwrap().to_string_lossy().into_owned();
            assert_eq!(
                last, "profiles",
                "tail should be the profiles subdir: {path:?}"
            );
            let full = path.to_string_lossy().into_owned();
            assert!(
                full.contains("slicer") || full.contains("soulcrafted"),
                "path should embed the app or org identifier: {full}"
            );
        }
        Err(UserLibraryDirError::HomeDirUnavailable) => {
            // No home directory in the test environment; tolerated so
            // the suite stays green in containerised runs without HOME.
        }
    }
}

#[test]
fn user_library_dir_error_display_mentions_the_failure_mode() {
    let msg = format!("{}", UserLibraryDirError::HomeDirUnavailable);
    assert!(
        msg.contains("home directory") || msg.contains("data directory"),
        "msg should describe the failure mode: {msg}"
    );
}

#[test]
fn user_library_dir_error_implements_error_trait() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<UserLibraryDirError>();
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` coverage for [`InferFileTypeError`].
//! Migrated from an inline `#[cfg(test)] mod infer_file_type_error_tests`
//! block in `error.rs` for uniformity with the other typed-error
//! test files under `tests_*_error.rs`.

use std::error::Error;
use std::path::PathBuf;

use crate::error::InferFileTypeError;

#[test]
fn unknown_extension_display_names_the_path() {
    let err = InferFileTypeError::UnknownExtension {
        path: PathBuf::from("/tmp/foo.xyz"),
    };
    let s = err.to_string();
    assert!(s.contains("Unable to infer file format"));
    assert!(s.contains("/tmp/foo.xyz"));
}

#[test]
fn unknown_extension_has_no_source() {
    let err = InferFileTypeError::UnknownExtension {
        path: PathBuf::from("/tmp/foo.xyz"),
    };
    assert!(err.source().is_none());
}

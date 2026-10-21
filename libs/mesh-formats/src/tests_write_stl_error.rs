/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` coverage for [`WriteStlError`].
//! Migrated from an inline `#[cfg(test)] mod write_stl_error_tests`
//! block in `error.rs` for uniformity with the other typed-error
//! test files under `tests_*_error.rs`.

use std::error::Error;
use std::io;

use crate::error::WriteStlError;

#[test]
fn io_display_includes_marker_and_inner_message() {
    let err = WriteStlError::Io(io::Error::other("disk full"));
    let s = err.to_string();
    assert!(s.contains("STL writer I/O error"));
    assert!(s.contains("disk full"));
}

#[test]
fn io_source_returns_the_wrapped_io_error() {
    let err = WriteStlError::Io(io::Error::other("disk full"));
    let source = err.source().expect("Io variant must expose a source");
    assert!(source.to_string().contains("disk full"));
}

#[test]
fn face_count_exceeds_u32_display_names_the_count_and_cap() {
    let err = WriteStlError::FaceCountExceedsU32 {
        count: 5_000_000_000,
    };
    let s = err.to_string();
    assert!(s.contains("5000000000"));
    assert!(s.contains("u32::MAX"));
}

#[test]
fn face_count_exceeds_u32_has_no_source() {
    let err = WriteStlError::FaceCountExceedsU32 { count: 42 };
    assert!(err.source().is_none());
}

#[test]
fn from_io_error_routes_to_io_variant() {
    let err: WriteStlError = io::Error::other("flush failed").into();
    assert!(matches!(err, WriteStlError::Io(_)));
}

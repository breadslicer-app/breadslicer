/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` / `From` coverage for [`LoadMeshError`].
//! One `#[test]` per variant for the Display + source pair, one per
//! `From` impl.

use std::error::Error;
use std::io;
use std::path::PathBuf;

use crate::error::{InferFileTypeError, LoadMeshError, LoadObjError, LoadStlError};
use crate::filetypes::MeshFileType;

#[test]
fn io_variant_display_and_source() {
    let err = LoadMeshError::Io(io::Error::other("disk full"));
    let s = err.to_string();
    assert!(s.contains("I/O error while loading mesh"));
    assert!(s.contains("disk full"));
    assert!(err.source().is_some());
}

#[test]
fn file_too_large_variant_display_and_source() {
    let err = LoadMeshError::FileTooLarge {
        actual_bytes: 100,
        limit_bytes: 50,
    };
    let s = err.to_string();
    assert!(s.contains("100"));
    assert!(s.contains("50"));
    assert!(err.source().is_none());
}

#[test]
fn infer_type_variant_display_and_source() {
    let inner = InferFileTypeError::UnknownExtension {
        path: PathBuf::from("/tmp/foo.xyz"),
    };
    let err = LoadMeshError::InferType(inner);
    let s = err.to_string();
    assert!(s.contains("/tmp/foo.xyz"));
    assert!(err.source().is_some());
}

#[test]
fn unsupported_format_variant_display_and_source() {
    let err = LoadMeshError::UnsupportedFormat(MeshFileType::Ply);
    let s = err.to_string();
    assert!(s.contains("Unsupported"));
    assert!(s.contains("Ply"));
    assert!(err.source().is_none());
}

#[test]
fn stl_variant_display_and_source() {
    let err = LoadMeshError::Stl(LoadStlError::AsciiHeaderMissing);
    let s = err.to_string();
    assert!(s.contains("solid"));
    assert!(err.source().is_some());
}

#[test]
fn obj_variant_display_and_source() {
    let err = LoadMeshError::Obj(LoadObjError::VertexWZero { line: 7 });
    let s = err.to_string();
    assert!(s.contains("line 7"));
    assert!(err.source().is_some());
}

#[test]
fn from_io_error_routes_to_io_variant() {
    let err: LoadMeshError = io::Error::other("disk full").into();
    assert!(matches!(err, LoadMeshError::Io(_)));
}

#[test]
fn from_infer_file_type_error_routes_to_infer_type_variant() {
    let inner = InferFileTypeError::UnknownExtension {
        path: PathBuf::from("/tmp/foo.xyz"),
    };
    let err: LoadMeshError = inner.into();
    assert!(matches!(err, LoadMeshError::InferType(_)));
}

#[test]
fn from_load_stl_error_routes_to_stl_variant() {
    let err: LoadMeshError = LoadStlError::AsciiHeaderMissing.into();
    assert!(matches!(err, LoadMeshError::Stl(_)));
}

#[test]
fn from_load_obj_error_routes_to_obj_variant() {
    let err: LoadMeshError = LoadObjError::VertexWZero { line: 1 }.into();
    assert!(matches!(err, LoadMeshError::Obj(_)));
}

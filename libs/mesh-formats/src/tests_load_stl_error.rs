/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` / `From` coverage for [`LoadStlError`].
//! One `#[test]` per variant; one for the single `From<io::Error>`
//! impl.

use std::error::Error;
use std::io;

use crate::error::LoadStlError;

fn parse_float_error() -> std::num::ParseFloatError {
    "abc".parse::<f32>().unwrap_err()
}

fn utf8_error() -> std::str::Utf8Error {
    // Build at runtime so the `invalid_from_utf8` lint can't see
    // through the literal.
    let bytes: Vec<u8> = vec![0xFF, 0xFE];
    std::str::from_utf8(&bytes).unwrap_err()
}

#[test]
fn io_variant_display_and_source() {
    let err = LoadStlError::Io(io::Error::other("read failed"));
    let s = err.to_string();
    assert!(s.contains("STL I/O error"));
    assert!(s.contains("read failed"));
    assert!(err.source().is_some());
}

#[test]
fn truncated_variant_display_and_source() {
    let err = LoadStlError::Truncated { what: "header" };
    let s = err.to_string();
    assert!(s.contains("truncated"));
    assert!(s.contains("header"));
    assert!(err.source().is_none());
}

#[test]
fn declared_triangle_count_exceeds_cap_variant_display_and_source() {
    let err = LoadStlError::DeclaredTriangleCountExceedsCap {
        declared: 1_000_000_000,
        cap: 200_000_000,
    };
    let s = err.to_string();
    assert!(s.contains("1000000000"));
    assert!(s.contains("200000000"));
    assert!(err.source().is_none());
}

#[test]
fn header_not_ascii_variant_display_and_source() {
    let err = LoadStlError::HeaderNotAscii;
    let s = err.to_string();
    assert!(s.contains("not ASCII"));
    assert!(err.source().is_none());
}

#[test]
fn header_not_nul_terminated_variant_display_and_source() {
    let err = LoadStlError::HeaderNotNulTerminated;
    let s = err.to_string();
    assert!(s.contains("not NUL-terminated"));
    assert!(err.source().is_none());
}

#[test]
fn header_not_utf8_variant_display_and_source() {
    let err = LoadStlError::HeaderNotUtf8(utf8_error());
    let s = err.to_string();
    assert!(s.contains("not valid UTF-8"));
    assert!(err.source().is_some());
}

#[test]
fn ascii_header_missing_variant_display_and_source() {
    let err = LoadStlError::AsciiHeaderMissing;
    let s = err.to_string();
    assert!(s.contains("solid"));
    assert!(err.source().is_none());
}

#[test]
fn ascii_empty_input_variant_display_and_source() {
    let err = LoadStlError::AsciiEmptyInput;
    let s = err.to_string();
    assert!(s.contains("empty"));
    assert!(err.source().is_none());
}

#[test]
fn ascii_invalid_facet_header_variant_display_and_source() {
    let err = LoadStlError::AsciiInvalidFacetHeader {
        tokens: vec!["bogus".to_string(), "facet".to_string()],
    };
    let s = err.to_string();
    assert!(s.contains("invalid facet header"));
    assert!(s.contains("bogus"));
    assert!(err.source().is_none());
}

#[test]
fn ascii_invalid_vertex_line_variant_display_and_source() {
    let err = LoadStlError::AsciiInvalidVertexLine {
        tokens: vec!["vertex".to_string(), "nan".to_string()],
    };
    let s = err.to_string();
    assert!(s.contains("vertex"));
    assert!(s.contains("nan"));
    assert!(err.source().is_none());
}

#[test]
fn ascii_unexpected_variant_display_and_source() {
    let err = LoadStlError::AsciiUnexpected {
        expected: "endloop",
        got: vec!["endsolid".to_string()],
    };
    let s = err.to_string();
    assert!(s.contains("endloop"));
    assert!(s.contains("endsolid"));
    assert!(err.source().is_none());
}

#[test]
fn ascii_eof_while_expecting_variant_display_and_source() {
    let err = LoadStlError::AsciiEofWhileExpecting { what: "endsolid" };
    let s = err.to_string();
    assert!(s.contains("EOF"));
    assert!(s.contains("endsolid"));
    assert!(err.source().is_none());
}

#[test]
fn invalid_float_variant_display_and_source() {
    let err = LoadStlError::InvalidFloat {
        token: "not_a_float".to_string(),
        source: parse_float_error(),
    };
    let s = err.to_string();
    assert!(s.contains("invalid float"));
    assert!(s.contains("not_a_float"));
    assert!(err.source().is_some());
}

#[test]
fn non_finite_float_variant_display_and_source() {
    let err = LoadStlError::NonFiniteFloat {
        what: "vertex coord",
        value: f32::NAN,
    };
    let s = err.to_string();
    assert!(s.contains("vertex coord"));
    assert!(s.contains("NaN") || s.contains("Nan"));
    assert!(err.source().is_none());
}

#[test]
fn from_io_error_routes_to_io_variant() {
    let err: LoadStlError = io::Error::other("read failed").into();
    assert!(matches!(err, LoadStlError::Io(_)));
}

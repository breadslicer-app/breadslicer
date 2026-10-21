/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` / `From` coverage for [`LoadObjError`].
//! One `#[test]` per variant; `FaceIndexOutOfRange` has two
//! display branches (positive vs negative index) covered by two
//! tests.

use std::error::Error;
use std::io;

use crate::error::LoadObjError;

fn parse_float_error() -> std::num::ParseFloatError {
    "abc".parse::<f32>().unwrap_err()
}

fn parse_int_error() -> std::num::ParseIntError {
    "abc".parse::<u32>().unwrap_err()
}

#[test]
fn io_variant_display_and_source() {
    let err = LoadObjError::Io(io::Error::other("read failed"));
    let s = err.to_string();
    assert!(s.contains("OBJ I/O error"));
    assert!(s.contains("read failed"));
    assert!(err.source().is_some());
}

#[test]
fn invalid_float_variant_display_and_source() {
    let err = LoadObjError::InvalidFloat {
        line: 5,
        token: "not_a_float".to_string(),
        source: parse_float_error(),
    };
    let s = err.to_string();
    assert!(s.contains("line 5"));
    assert!(s.contains("invalid float"));
    assert!(s.contains("not_a_float"));
    assert!(err.source().is_some());
}

#[test]
fn non_finite_float_variant_display_and_source() {
    let err = LoadObjError::NonFiniteFloat {
        line: 7,
        value: f32::NAN,
    };
    let s = err.to_string();
    assert!(s.contains("line 7"));
    assert!(s.contains("finite float"));
    assert!(err.source().is_none());
}

#[test]
fn vertex_coord_count_invalid_variant_display_and_source() {
    let err = LoadObjError::VertexCoordCountInvalid { line: 9, got: 5 };
    let s = err.to_string();
    assert!(s.contains("line 9"));
    assert!(s.contains("3 or 4"));
    assert!(s.contains("got 5"));
    assert!(err.source().is_none());
}

#[test]
fn vertex_w_zero_variant_display_and_source() {
    let err = LoadObjError::VertexWZero { line: 11 };
    let s = err.to_string();
    assert!(s.contains("line 11"));
    assert!(s.contains("w coordinate"));
    assert!(err.source().is_none());
}

#[test]
fn vertex_count_exceeds_limit_variant_display_and_source() {
    let err = LoadObjError::VertexCountExceedsLimit {
        line: 13,
        limit: 200_000_000,
    };
    let s = err.to_string();
    assert!(s.contains("line 13"));
    assert!(s.contains("200000000"));
    assert!(err.source().is_none());
}

#[test]
fn out_of_memory_vertices_variant_display_and_source() {
    let err = LoadObjError::OutOfMemoryVertices { line: 15 };
    let s = err.to_string();
    assert!(s.contains("line 15"));
    assert!(s.contains("out of memory"));
    assert!(s.contains("vertices"));
    assert!(err.source().is_none());
}

#[test]
fn out_of_memory_faces_variant_display_and_source() {
    let err = LoadObjError::OutOfMemoryFaces { line: 17 };
    let s = err.to_string();
    assert!(s.contains("line 17"));
    assert!(s.contains("out of memory"));
    assert!(s.contains("triangles") || s.contains("face"));
    assert!(err.source().is_none());
}

#[test]
fn face_too_few_vertices_variant_display_and_source() {
    let err = LoadObjError::FaceTooFewVertices { line: 19, got: 2 };
    let s = err.to_string();
    assert!(s.contains("line 19"));
    assert!(s.contains("at least 3"));
    assert!(s.contains("got 2"));
    assert!(err.source().is_none());
}

#[test]
fn face_exceeds_token_limit_variant_display_and_source() {
    let err = LoadObjError::FaceExceedsTokenLimit {
        line: 21,
        limit: 64,
    };
    let s = err.to_string();
    assert!(s.contains("line 21"));
    assert!(s.contains("64"));
    assert!(err.source().is_none());
}

#[test]
fn face_index_zero_variant_display_and_source() {
    let err = LoadObjError::FaceIndexZero { line: 23 };
    let s = err.to_string();
    assert!(s.contains("line 23"));
    assert!(s.contains("must not be zero"));
    assert!(err.source().is_none());
}

#[test]
fn invalid_face_index_variant_display_and_source() {
    let err = LoadObjError::InvalidFaceIndex {
        line: 25,
        token: "x".to_string(),
        source: parse_int_error(),
    };
    let s = err.to_string();
    assert!(s.contains("line 25"));
    assert!(s.contains("invalid face index"));
    assert!(err.source().is_some());
}

#[test]
fn negative_face_index_out_of_range_variant_display_and_source() {
    let err = LoadObjError::NegativeFaceIndexOutOfRange {
        line: 27,
        index: -50,
    };
    let s = err.to_string();
    assert!(s.contains("line 27"));
    assert!(s.contains("-50"));
    assert!(s.contains("negative"));
    assert!(err.source().is_none());
}

#[test]
fn face_index_out_of_range_positive_branch_display_and_source() {
    let err = LoadObjError::FaceIndexOutOfRange {
        line: 29,
        index: 100,
        vertex_count: 10,
    };
    let s = err.to_string();
    assert!(s.contains("line 29"));
    assert!(s.contains("100"));
    assert!(s.contains("10 vertices"));
    assert!(err.source().is_none());
}

#[test]
fn face_index_out_of_range_negative_branch_display_and_source() {
    let err = LoadObjError::FaceIndexOutOfRange {
        line: 31,
        index: -100,
        vertex_count: 10,
    };
    let s = err.to_string();
    assert!(s.contains("line 31"));
    assert!(s.contains("-100"));
    assert!(s.contains("negative"));
    assert!(err.source().is_none());
}

#[test]
fn unrecognized_directive_variant_display_and_source() {
    let err = LoadObjError::UnrecognizedDirective {
        line: 33,
        directive: "weird".to_string(),
    };
    let s = err.to_string();
    assert!(s.contains("line 33"));
    assert!(s.contains("weird"));
    assert!(err.source().is_none());
}

#[test]
fn from_io_error_routes_to_io_variant() {
    let err: LoadObjError = io::Error::other("read failed").into();
    assert!(matches!(err, LoadObjError::Io(_)));
}

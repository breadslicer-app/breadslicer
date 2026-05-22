/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Coverage for the four `show_*` helpers in
//! `commands::diagnose`. Each helper formats a slice of one error
//! enum into the supplied writer; the tests assert every variant
//! contributes a recognisable line so a future refactor that drops a
//! case can be caught.

use sc_mesh_core::indexed_mesh::{
    MeshFaceError, MeshManifoldError, MeshNormalVectorError, MeshVertexError,
};
use sc_mesh_core::primitives::{CoordAxis, FaceIndex, TriVertexSlot, VertexIndex};

use crate::commands::diagnose::{
    show_face_errors, show_manifold_errors, show_normal_vector_errors, show_vertex_errors,
};

fn render<F: FnOnce(&mut Vec<u8>)>(write: F) -> String {
    let mut buf = Vec::new();
    write(&mut buf);
    String::from_utf8(buf).expect("show_* helpers emit valid UTF-8")
}

#[test]
fn show_vertex_errors_covers_every_variant() {
    let errors = vec![
        MeshVertexError::InfiniteCoordinate {
            vertex_idx: 7,
            coord: CoordAxis::X,
        },
        MeshVertexError::NaNCoordinate {
            vertex_idx: 8,
            coord: CoordAxis::Y,
        },
    ];
    let out = render(|w| show_vertex_errors(&errors, w));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "header + 2 entries; got: {out}");
    assert!(lines[0].contains("Vertex errors (2)"));
    assert!(lines[1].contains("ID 7") && lines[1].contains("infinite") && lines[1].contains("'X'"));
    assert!(lines[2].contains("ID 8") && lines[2].contains("NaN") && lines[2].contains("'Y'"));
}

#[test]
fn show_face_errors_covers_every_variant() {
    let errors = vec![
        MeshFaceError::ZeroLengthEdge {
            face_idx: FaceIndex::from(3_usize),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V2,
            vertex_idx: VertexIndex::from(11_u32),
        },
        MeshFaceError::OutOfRangeVertexIndex {
            face_idx: FaceIndex::from(4_usize),
            vertex_pos: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(999_u32),
        },
    ];
    let out = render(|w| show_face_errors(&errors, w));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "header + 2 entries; got: {out}");
    assert!(lines[0].contains("Face errors (2)"));
    assert!(
        lines[1].contains("ID 3")
            && lines[1].contains("v0")
            && lines[1].contains("v2")
            && lines[1].contains("vertex id = 11"),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].contains("ID 4")
            && lines[2].contains("(v1)")
            && lines[2].contains("out of range index 999"),
        "{}",
        lines[2]
    );
}

#[test]
fn show_normal_vector_errors_covers_every_variant() {
    let errors = vec![
        MeshNormalVectorError::ZeroAreaTriangle {
            face_idx: FaceIndex::from(1_usize),
        },
        MeshNormalVectorError::NonUnitNorm {
            face_idx: FaceIndex::from(2_usize),
            norm: 1.25,
        },
        MeshNormalVectorError::ExpectationMismatch {
            face_idx: FaceIndex::from(3_usize),
        },
    ];
    let out = render(|w| show_normal_vector_errors(&errors, w));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "header + 3 entries; got: {out}");
    assert!(lines[0].contains("Normal vectors' errors (3)"));
    assert!(lines[1].contains("ID 1") && lines[1].contains("area equal to 0"));
    assert!(lines[2].contains("ID 2") && lines[2].contains("non-unit norm: 1.25"));
    assert!(lines[3].contains("ID 3") && lines[3].contains("does not match"));
}

#[test]
fn show_manifold_errors_covers_every_variant() {
    let errors = vec![
        MeshManifoldError::EmptyMesh,
        MeshManifoldError::BoundaryEdge {
            edge: (VertexIndex::from(2_u32), VertexIndex::from(5_u32)),
            face_idx: FaceIndex::from(9_usize),
        },
        MeshManifoldError::NonManifoldEdge {
            edge: (VertexIndex::from(1_u32), VertexIndex::from(4_u32)),
            face_indices: vec![
                FaceIndex::from(10_usize),
                FaceIndex::from(11_usize),
                FaceIndex::from(12_usize),
            ],
        },
        MeshManifoldError::InconsistentWindingOrder {
            edge: (VertexIndex::from(3_u32), VertexIndex::from(8_u32)),
            face_indices: [FaceIndex::from(20_usize), FaceIndex::from(21_usize)],
        },
    ];
    let out = render(|w| show_manifold_errors(&errors, w));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 5, "header + 4 entries; got: {out}");
    assert!(lines[0].contains("Manifold errors:"));
    assert!(lines[1].contains("empty") && lines[1].contains("no faces"));
    assert!(
        lines[2].contains("ID 9")
            && lines[2].contains("(2, 5)")
            && lines[2].contains("boundary edge")
    );
    assert!(
        lines[3].contains("(1, 4)") && lines[3].contains("more than 3 faces"),
        "{}",
        lines[3]
    );
    assert!(
        lines[4].contains("(3, 8)")
            && lines[4].contains("(20 and 21)")
            && lines[4].contains("inconsistent"),
        "{}",
        lines[4]
    );
}

#[test]
fn empty_input_only_writes_header() {
    let vertex_out = render(|w| show_vertex_errors(&[], w));
    assert_eq!(vertex_out.trim(), "Vertex errors (0):");

    let face_out = render(|w| show_face_errors(&[], w));
    assert_eq!(face_out.trim(), "Face errors (0):");

    let normal_out = render(|w| show_normal_vector_errors(&[], w));
    assert_eq!(normal_out.trim(), "Normal vectors' errors (0):");

    let manifold_out = render(|w| show_manifold_errors(&[], w));
    assert_eq!(manifold_out.trim(), "Manifold errors:");
}

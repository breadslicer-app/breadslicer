/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Coverage for the four `show_*` helpers in
//! `commands::diagnose`. Each helper emits a header line with the
//! variant count followed by one `\t- {error}\n` line per entry,
//! using each error type's canonical `Display` impl. The tests pin
//! the header shape and one body line per variant so a future
//! refactor that drops a case can be caught.

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
    assert_eq!(lines[0], "Vertex errors (2):");
    assert_eq!(lines[1], "\t- vertex #7 has an infinite X coordinate");
    assert_eq!(lines[2], "\t- vertex #8 has a NaN Y coordinate");
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
    assert_eq!(lines[0], "Face errors (2):");
    assert_eq!(
        lines[1],
        "\t- face #3 has a zero-length edge: slots v0 and v2 both bound to vertex #11",
    );
    assert_eq!(
        lines[2],
        "\t- face #4 slot v1 references vertex #999, which is out of range",
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
    assert_eq!(lines[0], "Normal vector errors (3):");
    assert_eq!(lines[1], "\t- face #1 is degenerate: zero area");
    assert_eq!(
        lines[2],
        "\t- face #2 has a non-unit normal vector (norm = 1.25)",
    );
    assert_eq!(
        lines[3],
        "\t- face #3 declares a normal vector that disagrees with the triangle plane",
    );
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
    assert_eq!(lines[0], "Manifold errors (4):");
    assert_eq!(lines[1], "\t- mesh has no faces");
    assert_eq!(
        lines[2],
        "\t- edge (#2, #5) is shared by only face #9 (a manifold edge must be shared by exactly 2 faces)",
    );
    assert_eq!(
        lines[3],
        "\t- edge (#1, #4) is shared by 3 faces (a manifold edge must be shared by exactly 2)",
    );
    assert_eq!(
        lines[4],
        "\t- edge (#3, #8) is traversed in the same direction by faces #20 and #21 (windings disagree)",
    );
}

#[test]
fn empty_input_only_writes_header() {
    let vertex_out = render(|w| show_vertex_errors(&[], w));
    assert_eq!(vertex_out.trim(), "Vertex errors (0):");

    let face_out = render(|w| show_face_errors(&[], w));
    assert_eq!(face_out.trim(), "Face errors (0):");

    let normal_out = render(|w| show_normal_vector_errors(&[], w));
    assert_eq!(normal_out.trim(), "Normal vector errors (0):");

    let manifold_out = render(|w| show_manifold_errors(&[], w));
    assert_eq!(manifold_out.trim(), "Manifold errors (0):");
}

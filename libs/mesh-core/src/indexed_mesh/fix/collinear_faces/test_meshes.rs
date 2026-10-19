/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Small closed meshes with collinear faces, for the tests of the pass. The
//! names are those of the pass: the collinear face is (R, P, Q) with its long
//! edge P-Q, A is the face across R-P (3rd vertex X), B the face across Q-R
//! (3rd vertex Y), and C the face across P-Q (3rd vertex S).

use crate::indexed_mesh::mesh::IndexedMesh;
use crate::primitives::{IndexedTriangle, Vertex};

/// The points [P, Q, R, X, S] of [`split_tetrahedron`] for a tetrahedron with R
/// at the midpoint of P and Q.
pub(in crate::indexed_mesh) const SPLIT_TETRAHEDRON: [[f32; 3]; 5] = [
    [1.0, 1.0, 1.0],
    [1.0, -1.0, -1.0],
    [1.0, 0.0, 0.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
];

/// The points [P, Q, R, S, X, Y] of [`fan_pyramid`] for a pyramid on the square
/// (0, 0, 0), (2, 0, 0), (2, 2, 0), (0, 2, 0) with its apex S at (1, 1, 1), and
/// R at the midpoint of P and Q.
pub(in crate::indexed_mesh) const SQUARE_FAN_PYRAMID: [[f32; 3]; 6] = [
    [2.0, 0.0, 0.0],
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 1.0],
    [2.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
];

/// Returns a mesh where A and B share X, on the points [P, Q, R, X, S]: the
/// faces (P, Q, R), A (P, R, X), B (R, Q, X), C (P, S, Q), (P, X, S) and (Q, S,
/// X). The merge leaves the tetrahedron P, Q, X, S.
pub(in crate::indexed_mesh) fn split_tetrahedron(points: [[f32; 3]; 5]) -> IndexedMesh {
    mesh(
        &points,
        &[
            [0, 1, 2],
            [0, 2, 3],
            [2, 1, 3],
            [0, 4, 1],
            [0, 3, 4],
            [1, 4, 3],
        ],
    )
}

/// Returns a mesh where A and B do not share their 3rd vertex, on the points
/// [P, Q, R, S, X, Y]: the faces (P, Q, R), A (R, X, P), B (R, Q, Y), C (Q, P,
/// S), (R, Y, X), (P, X, S), (X, Y, S) and (Y, Q, S). R-S is not an edge.
pub(in crate::indexed_mesh) fn fan_pyramid(points: [[f32; 3]; 6]) -> IndexedMesh {
    mesh(
        &points,
        &[
            [0, 1, 2],
            [2, 4, 0],
            [2, 1, 5],
            [1, 0, 3],
            [2, 5, 4],
            [0, 4, 3],
            [4, 5, 3],
            [5, 1, 3],
        ],
    )
}

/// Returns a closed tetrahedron in the plane z = 0, on the points [P, Q, R, X]:
/// the faces (R, P, Q), (P, R, X) and (R, Q, X) facing 1 way, and (Q, P, X) the
/// other. A, B and C all have the 3rd vertex X.
pub(in crate::indexed_mesh) fn flat_tetrahedron() -> IndexedMesh {
    mesh(
        &[
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
        &[[2, 0, 1], [0, 2, 3], [2, 1, 3], [1, 0, 3]],
    )
}

/// Returns the closed pair (R, P, Q), (Q, P, R) on the points [P, Q, R]: each
/// face is A, B and C of the other.
pub(in crate::indexed_mesh) fn collinear_pair() -> IndexedMesh {
    mesh(
        &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        &[[2, 0, 1], [1, 0, 2]],
    )
}

/// Returns a mesh where A and B do not share their 3rd vertex and S = X, on the
/// points [P, Q, R, X, Y]: the faces (P, Q, R), A (R, X, P), B (R, Q, Y), C (P,
/// X, Q), (R, Y, X) and (Y, Q, X). R-S is already an edge, of A.
pub(in crate::indexed_mesh) fn blocked_split_tetrahedron() -> IndexedMesh {
    mesh(
        &[
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [0.0, 0.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
        &[
            [0, 1, 2],
            [2, 3, 0],
            [2, 1, 4],
            [0, 3, 1],
            [2, 4, 3],
            [4, 1, 3],
        ],
    )
}

/// Returns [`split_tetrahedron`] with R 2^-20 of the way from P to Q, and A
/// stored as (X, P, R). A is thin: its sine at X is about 8e-7, so it has zero
/// area too, and its long edge X-P makes it a candidate of the pass.
pub(in crate::indexed_mesh) fn split_tetrahedron_with_a_thin_neighbour() -> IndexedMesh {
    let mut points = SPLIT_TETRAHEDRON;
    let near_p = 1.0 - 2.0_f32.powi(-19);
    points[2] = [1.0, near_p, near_p];
    let mut mesh = split_tetrahedron(points);
    mesh.faces[1] = IndexedTriangle::from_raw_indices([3, 0, 2]);
    mesh
}

/// Returns a pyramid on the square P (0), Q (1), Y (3), X (2) with its apex S
/// (4), where R1 (5) and R2 (6) split the edge P-Q in 3. The collinear face 8,
/// (P, R2, R1), has its long edge P-R2 on the collinear face 9, (P, Q, R2).
pub(in crate::indexed_mesh) fn chained_pyramid() -> IndexedMesh {
    mesh(
        &[
            [0.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [0.0, -3.0, 0.0],
            [3.0, -3.0, 0.0],
            [1.5, -1.5, 1.5],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
        ],
        &[
            [0, 5, 2],
            [5, 3, 2],
            [5, 6, 3],
            [6, 1, 3],
            [1, 0, 4],
            [3, 1, 4],
            [2, 3, 4],
            [0, 2, 4],
            [0, 6, 5],
            [0, 1, 6],
        ],
    )
}

fn mesh(points: &[[f32; 3]], faces: &[[u32; 3]]) -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: points.iter().copied().map(Vertex::new).collect(),
        faces: faces
            .iter()
            .copied()
            .map(IndexedTriangle::from_raw_indices)
            .collect(),
    }
}

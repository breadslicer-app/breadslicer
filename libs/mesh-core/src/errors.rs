/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

#[derive(Debug)]
pub enum MeshVertexError {
    InfiniteCoordinate { vertex_idx: usize, coord: u8 },
    NaNCoordinate { vertex_idx: usize, coord: u8 },
}

#[derive(Debug)]
pub enum MeshNormalVectorError {
    ZeroAreaTriangle { face_idx: usize },
    NonUnitNorm { face_idx: usize, norm: f32 },
    ExpectationMismatch { face_idx: usize },
}

#[derive(Debug)]
pub enum MeshFaceError {
    OutOfRangeVertexIndex {
        /// Index for the problematic face
        face_idx: usize,
        /// 0, 1, or 2. Indicates which vertex index of the triangle is wrong
        vertex_pos: u8,
        /// Value of the problematic vertex index
        vertex_idx: usize,
    },
    ZeroLengthEdge {
        /// Index for the problematic face
        face_idx: usize,
        /// Position of one of the "first" vertex of the collapsed edge
        vertex_pos_a: u8,
        /// Position of one of the "second" vertex of the collapsed edge
        vertex_pos_b: u8,
        /// Value of the problematic vertex index
        vertex_idx: usize,
    },
}

#[derive(Debug)]
pub enum MeshManifoldError {
    EmptyMesh,
    /// An edge that belongs to only one face (the mesh has a boundary).
    BoundaryEdge {
        edge: (u32, u32),
        face_idx: usize,
    },
    /// An edge shared by more than two faces.
    NonManifoldEdge {
        edge: (u32, u32),
        face_indices: Vec<usize>,
    },
    /// Two faces share an edge but traverse it in the same direction,
    /// meaning their normals are inconsistently oriented.
    InconsistentWindingOrder {
        edge: (u32, u32),
        face_indices: [usize; 2],
    },
}

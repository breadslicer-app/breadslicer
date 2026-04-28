/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use newtypes::{newtype_from_only, newtype_ord};

use crate::primitives::Normal;

/// Which coordinate of a 3D point a diagnostic refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CoordAxis {
    X = 0,
    Y = 1,
    Z = 2,
}

impl CoordAxis {
    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    /// 0/1/2 index into a `[T; 3]` coordinate array.
    #[inline]
    pub const fn as_index(self) -> usize {
        self as usize
    }
}

/// Which of a triangle's three corners a diagnostic refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TriVertexSlot {
    V0 = 0,
    V1 = 1,
    V2 = 2,
}

impl TriVertexSlot {
    pub const ALL: [Self; 3] = [Self::V0, Self::V1, Self::V2];

    /// 0/1/2 index into a triangle's `vertices: [u32; 3]` array.
    #[inline]
    pub const fn as_index(self) -> usize {
        self as usize
    }
}

// Index into `IndexedMesh::vertices`. `newtype_ord!` emits the type as
// `pub struct VertexIndex(u32);` `#[repr(transparent)]` plus
// Debug/Clone/Copy/PartialEq/Eq/Hash/PartialOrd/Ord/Into<u32>;
// `newtype_from_only!` adds `From<u32>` (no `FromStr`).
newtype_ord!(VertexIndex, u32);
newtype_from_only!(VertexIndex, u32);

impl VertexIndex {
    /// The raw `u32` index into `IndexedMesh::vertices`.
    #[inline]
    pub fn get(self) -> u32 {
        self.into()
    }
}

// Index into `IndexedMesh::faces`. Same shape as `VertexIndex`.
newtype_ord!(FaceIndex, usize);
newtype_from_only!(FaceIndex, usize);

impl FaceIndex {
    /// The raw `usize` index into `IndexedMesh::faces`.
    #[inline]
    pub fn get(self) -> usize {
        self.into()
    }
}

/// An undirected edge between two vertices, identified by their indices.
///
/// The two endpoints are not ordered: callers that need a canonical form
/// can sort with `Edge::canonical`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub a: VertexIndex,
    pub b: VertexIndex,
}

impl Edge {
    #[inline]
    pub const fn new(a: VertexIndex, b: VertexIndex) -> Self {
        Self { a, b }
    }

    /// Returns the edge with `a <= b`. Use as a canonical undirected key.
    #[inline]
    pub fn canonical(self) -> Self {
        if self.a <= self.b {
            self
        } else {
            Self {
                a: self.b,
                b: self.a,
            }
        }
    }
}

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
pub enum MeshFix {
    /// A face was removed because it had a zero-length edge
    RemovedZeroLengthEdgeFace { face_idx: usize },
    /// A face's winding order was flipped (vertices reversed)
    FlippedFaceOrientation { face_idx: usize },
    /// A face's normal vector was recomputed from its vertices
    RecomputedNormal {
        face_idx: usize,
        old_normal: Normal,
        new_normal: Normal,
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

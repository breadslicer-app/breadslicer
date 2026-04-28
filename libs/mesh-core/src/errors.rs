/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt;

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

impl fmt::Display for CoordAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        })
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

impl fmt::Display for TriVertexSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::V0 => "v0",
            Self::V1 => "v1",
            Self::V2 => "v2",
        })
    }
}

// Index into `IndexedMesh::vertices`.
newtype_ord!(VertexIndex, u32);
newtype_from_only!(VertexIndex, u32);

impl VertexIndex {
    /// The raw `u32` index into `IndexedMesh::vertices`.
    #[inline]
    pub fn get(self) -> u32 {
        self.into()
    }
}

impl fmt::Display for VertexIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
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

impl fmt::Display for FaceIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshVertexError {
    InfiniteCoordinate { vertex_idx: usize, coord: CoordAxis },
    NaNCoordinate { vertex_idx: usize, coord: CoordAxis },
}

#[derive(Clone, Debug, PartialEq)]
pub enum MeshNormalVectorError {
    ZeroAreaTriangle { face_idx: FaceIndex },
    NonUnitNorm { face_idx: FaceIndex, norm: f32 },
    ExpectationMismatch { face_idx: FaceIndex },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshFaceError {
    OutOfRangeVertexIndex {
        /// Index for the problematic face
        face_idx: FaceIndex,
        /// Which corner of the triangle holds the offending index.
        vertex_pos: TriVertexSlot,
        /// Value of the problematic vertex index, as found in the face.
        vertex_idx: VertexIndex,
    },
    ZeroLengthEdge {
        /// Index for the problematic face
        face_idx: FaceIndex,
        /// One of the two corners that share the same vertex.
        vertex_pos_a: TriVertexSlot,
        /// The other corner that shares the same vertex.
        vertex_pos_b: TriVertexSlot,
        /// The shared vertex index.
        vertex_idx: VertexIndex,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum MeshFix {
    /// A face was removed because it had a zero-length edge
    RemovedZeroLengthEdgeFace { face_idx: FaceIndex },
    /// A face's winding order was flipped (vertices reversed)
    FlippedFaceOrientation { face_idx: FaceIndex },
    /// A face's normal vector was recomputed from its vertices
    RecomputedNormal {
        face_idx: FaceIndex,
        old_normal: Normal,
        new_normal: Normal,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshManifoldError {
    EmptyMesh,
    /// An edge that belongs to only one face (the mesh has a boundary).
    BoundaryEdge {
        edge: (u32, u32),
        face_idx: FaceIndex,
    },
    /// An edge shared by more than two faces.
    NonManifoldEdge {
        edge: (u32, u32),
        face_indices: Vec<FaceIndex>,
    },
    /// Two faces share an edge but traverse it in the same direction,
    /// meaning their normals are inconsistently oriented.
    InconsistentWindingOrder {
        edge: (u32, u32),
        face_indices: [FaceIndex; 2],
    },
}

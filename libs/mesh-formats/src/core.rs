/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

#[derive(Default, Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Vector3D<F: Copy + PartialEq + PartialOrd>(pub [F; 3]);

impl<F: Copy + PartialEq + PartialOrd> From<Vector3D<F>> for [F; 3] {
    fn from(v: Vector3D<F>) -> Self {
        v.0
    }
}

impl<F: Copy + PartialEq + PartialOrd> std::ops::Index<usize> for Vector3D<F> {
    type Output = F;
    fn index(&self, i: usize) -> &Self::Output {
        assert!(i < 3); // TODO: Replace with debug_assert once we have proper tests in place
        &self.0[i]
    }
}

pub type Vertex = Vector3D<f32>;
pub type Normal = Vector3D<f32>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub normal: Normal,
    pub vertices: [Vertex; 3],
}

/// Struct that represents a triangle in the context of a mesh, in which its
/// vertices are indexed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IndexedTriangle {
    pub normal: Normal,
    pub vertices: [u32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedMesh {
    vertices: Vec<Vertex>, // TODO: Make it generic (don't rush it)
    faces: Vec<IndexedTriangle>,
}

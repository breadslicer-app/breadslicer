/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

trait With3dVectorProps<F> {
    fn squared_norm(&self) -> F;
    fn norm(&self) -> F;
    fn diff(&self, other: &[F; 3]) -> [F; 3];
    fn dot_product(&self, other: &[F; 3]) -> F;
    fn cross_product(&self, other: &[F; 3]) -> [F; 3];
}

#[derive(Default, Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Vector3D<F: Copy + PartialEq + PartialOrd>(pub [F; 3]);

impl<F: Copy + PartialEq + PartialOrd> Vector3D<F> {
    pub const fn new(v: [F; 3]) -> Self {
        Self(v)
    }
}

impl With3dVectorProps<f32> for [f32; 3] {
    fn squared_norm(&self) -> f32 {
        self[0].mul_add(self[0], self[1].mul_add(self[1], self[2] * self[2]))
    }

    fn norm(&self) -> f32 {
        self[0]
            .mul_add(self[0], self[1].mul_add(self[1], self[2] * self[2]))
            .sqrt()
    }

    fn diff(&self, other: &[f32; 3]) -> [f32; 3] {
        [self[0] - other[0], self[1] - other[1], self[2] - other[2]]
    }

    fn dot_product(&self, e2: &[f32; 3]) -> f32 {
        self[0].mul_add(e2[0], self[1].mul_add(e2[1], self[2] * e2[2]))
    }

    fn cross_product(&self, e2: &[f32; 3]) -> [f32; 3] {
        let e1 = self;
        [
            e1[1].mul_add(e2[2], -e1[2] * e2[1]),
            e1[2].mul_add(e2[0], -e1[0] * e2[2]),
            e1[0].mul_add(e2[1], -e1[1] * e2[0]),
        ]
    }
}

impl With3dVectorProps<f64> for [f64; 3] {
    fn squared_norm(&self) -> f64 {
        self[0].mul_add(self[0], self[1].mul_add(self[1], self[2] * self[2]))
    }

    fn norm(&self) -> f64 {
        self[0]
            .mul_add(self[0], self[1].mul_add(self[1], self[2] * self[2]))
            .sqrt()
    }

    fn diff(&self, other: &[f64; 3]) -> [f64; 3] {
        [self[0] - other[0], self[1] - other[1], self[2] - other[2]]
    }

    fn dot_product(&self, e2: &[f64; 3]) -> f64 {
        self[0].mul_add(e2[0], self[1].mul_add(e2[1], self[2] * e2[2]))
    }

    fn cross_product(&self, e2: &[f64; 3]) -> [f64; 3] {
        let e1 = self;
        [
            e1[1].mul_add(e2[2], -e1[2] * e2[1]),
            e1[2].mul_add(e2[0], -e1[0] * e2[2]),
            e1[0].mul_add(e2[1], -e1[1] * e2[0]),
        ]
    }
}

impl Vector3D<f32> {
    pub fn squared_norm(&self) -> f32 {
        self.0.squared_norm()
    }

    pub fn norm(&self) -> f32 {
        self.0.norm()
    }

    pub fn diff(&self, other: &Vector3D<f32>) -> Vector3D<f32> {
        Vector3D(self.0.diff(&other.0))
    }

    pub fn dot_product(&self, other: &Vector3D<f32>) -> f32 {
        self.0.dot_product(&other.0)
    }

    pub fn cross_product(&self, other: &Vector3D<f32>) -> Vector3D<f32> {
        Vector3D(self.0.cross_product(&other.0))
    }
}

impl Vector3D<f64> {
    pub fn squared_norm(&self) -> f64 {
        self.0.squared_norm()
    }

    pub fn norm(&self) -> f64 {
        self.0.norm()
    }

    pub fn diff(&self, other: &Vector3D<f64>) -> Vector3D<f64> {
        Vector3D(self.0.diff(&other.0))
    }

    pub fn dot_product(&self, other: &Vector3D<f64>) -> f64 {
        self.0.dot_product(&other.0)
    }

    pub fn cross_product(&self, other: &Vector3D<f64>) -> Vector3D<f64> {
        Vector3D(self.0.cross_product(&other.0))
    }
}

impl From<Vector3D<f32>> for Vector3D<f64> {
    fn from(v: Vector3D<f32>) -> Self {
        Vector3D([v[0] as f64, v[1] as f64, v[2] as f64])
    }
}

impl From<Vector3D<f64>> for Vector3D<f32> {
    fn from(v: Vector3D<f64>) -> Self {
        Vector3D([v[0] as f32, v[1] as f32, v[2] as f32])
    }
}

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

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

#[derive(Default, Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Vector3D<F: Copy + PartialEq + PartialOrd>(pub [F; 3]);

impl<F: Copy + PartialEq + PartialOrd> Vector3D<F> {
    pub const fn new(v: [F; 3]) -> Self {
        Self(v)
    }
}

macro_rules! impl_vector3d_ops {
    ($f:ty) => {
        impl Vector3D<$f> {
            pub fn squared_norm(&self) -> $f {
                let s = &self.0;
                s[0].mul_add(s[0], s[1].mul_add(s[1], s[2] * s[2]))
            }

            pub fn norm(&self) -> $f {
                self.squared_norm().sqrt()
            }

            pub fn diff(&self, other: &Vector3D<$f>) -> Vector3D<$f> {
                Vector3D([
                    self.0[0] - other.0[0],
                    self.0[1] - other.0[1],
                    self.0[2] - other.0[2],
                ])
            }

            pub fn dot_product(&self, other: &Vector3D<$f>) -> $f {
                self.0[0].mul_add(
                    other.0[0],
                    self.0[1].mul_add(other.0[1], self.0[2] * other.0[2]),
                )
            }

            pub fn cross_product(&self, other: &Vector3D<$f>) -> Vector3D<$f> {
                let (e1, e2) = (&self.0, &other.0);
                Vector3D([
                    e1[1].mul_add(e2[2], -e1[2] * e2[1]),
                    e1[2].mul_add(e2[0], -e1[0] * e2[2]),
                    e1[0].mul_add(e2[1], -e1[1] * e2[0]),
                ])
            }
        }
    };
}

impl_vector3d_ops!(f32);
impl_vector3d_ops!(f64);

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

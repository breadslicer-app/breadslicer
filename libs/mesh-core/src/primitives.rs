/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use newtypes::newtype_array;

use crate::errors::VertexIndex;

#[derive(Default, Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Vector3D<F: Copy + PartialEq + PartialOrd>(pub(crate) [F; 3]);

impl<F: Copy + PartialEq + PartialOrd> Vector3D<F> {
    pub const fn new(v: [F; 3]) -> Self {
        Self(v)
    }

    /// Components as a fixed-size array, by value.
    pub fn coords(&self) -> [F; 3] {
        self.0
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

// Distinct newtypes around `[f32; 3]` so a `Normal` cannot silently
// flow through `apply_to_vertex` and vice versa. `newtype_array!` emits
// `pub struct $name([f32; 3])` `#[repr(transparent)]`, plus Debug, Clone,
// Copy, PartialEq, Index<I> (single index + ranges), AsRef<[f32]>,
// AsMut<[f32]>, IntoIterator for `Self` / `&Self` / `&mut Self`,
// From<Self> for [f32; 3], and inherent `len`, `is_empty`, `iter`,
// `iter_mut`, `as_array`, `as_array_mut`. We add only the constructors
// and the few math accessors below.
newtype_array!(Vertex, f32, 3);
newtype_array!(Normal, f32, 3);

impl Vertex {
    /// Construct from the three coordinates.
    #[inline]
    pub const fn new(coords: [f32; 3]) -> Self {
        Self(coords)
    }

    /// Coordinates as a fixed-size array, by value.
    #[inline]
    pub fn coords(self) -> [f32; 3] {
        self.into()
    }

    /// Widen to f64 for cancellation-safe math (used in geometry/transform
    /// hot paths where f32 differences lose precision).
    #[inline]
    pub fn to_f64(self) -> Vector3D<f64> {
        Vector3D::new(self.coords()).into()
    }
}

impl Normal {
    /// Construct from the three components.
    #[inline]
    pub const fn new(components: [f32; 3]) -> Self {
        Self(components)
    }

    /// Components as a fixed-size array, by value.
    #[inline]
    pub fn coords(self) -> [f32; 3] {
        self.into()
    }

    /// Widen to f64 for cancellation-safe math.
    #[inline]
    pub fn to_f64(self) -> Vector3D<f64> {
        Vector3D::new(self.coords()).into()
    }

    /// Reverse direction (component-wise negation). Used when a face's
    /// winding is flipped without recomputing from vertex positions.
    #[inline]
    pub fn negate(self) -> Self {
        Self::new(self.coords().map(|c| -c))
    }

    /// Magnitude (Euclidean length). For a well-formed unit normal this
    /// is ≈ 1.0 within f32 precision; deviations larger than
    /// [`UNIT_NORM_TOLERANCE`](crate::geometry::UNIT_NORM_TOLERANCE) are
    /// flagged by `validate_normal_vectors`.
    #[inline]
    pub fn norm(self) -> f32 {
        self.iter().map(|c| c * c).sum::<f32>().sqrt()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub(crate) normal: Normal,
    pub(crate) vertices: [Vertex; 3],
}

impl Triangle {
    pub const fn new(normal: Normal, vertices: [Vertex; 3]) -> Self {
        Self { normal, vertices }
    }

    pub fn normal(&self) -> Normal {
        self.normal
    }

    pub fn vertices(&self) -> [Vertex; 3] {
        self.vertices
    }
}

/// Struct that represents a triangle in the context of a mesh, in which its
/// vertices are indexed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IndexedTriangle {
    pub normal: Normal,
    pub vertices: [VertexIndex; 3],
}

impl IndexedTriangle {
    /// Builds an `IndexedTriangle` from a normal and three raw `u32`
    /// vertex indices. Used at format-loader and test boundaries where
    /// the indices arrive as raw integers; production code that already
    /// holds typed `VertexIndex` values should construct the struct
    /// directly.
    #[inline]
    pub fn from_raw_indices(normal: Normal, vertices: [u32; 3]) -> Self {
        Self {
            normal,
            vertices: [
                VertexIndex::from(vertices[0]),
                VertexIndex::from(vertices[1]),
                VertexIndex::from(vertices[2]),
            ],
        }
    }
}

#[cfg(test)]
mod vector3d_constructor_tests {
    use super::Vector3D;

    #[test]
    fn new_stores_components_in_order() {
        let v = Vector3D::new([1.0_f32, 2.0, 3.0]);
        assert_eq!(v.0, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn new_is_const() {
        const V: Vector3D<f32> = Vector3D::new([4.0, 5.0, 6.0]);
        assert_eq!(V.0, [4.0, 5.0, 6.0]);
    }

    #[test]
    fn array_into_vector3d_round_trip() {
        let arr: [f32; 3] = [1.5, -2.5, 0.0];
        let v = Vector3D::new(arr);
        let back: [f32; 3] = v.into();
        assert_eq!(back, arr);
    }
}

#[cfg(test)]
mod vector3d_conversion_tests {
    use super::Vector3D;

    #[test]
    fn f32_to_f64_widens_components() {
        let v32: Vector3D<f32> = Vector3D::new([1.5, -2.5, 0.0]);
        let v64: Vector3D<f64> = v32.into();
        assert_eq!(v64.0, [1.5_f64, -2.5_f64, 0.0_f64]);
    }

    #[test]
    fn f64_to_f32_truncates_components() {
        // 0.1 + 0.2 in f64 cannot be represented exactly in f32; the
        // conversion is a plain `as f32` cast, so this checks that the
        // component-wise cast is what's happening (no rounding mode tweak).
        let v64: Vector3D<f64> = Vector3D::new([0.1 + 0.2, 1.0e30, -1.0e-30]);
        let v32: Vector3D<f32> = v64.into();
        assert_eq!(
            v32.0,
            [(0.1 + 0.2) as f32, 1.0e30_f64 as f32, -1.0e-30_f64 as f32]
        );
    }

    #[test]
    fn f32_to_f64_to_f32_is_identity_for_normal_finite() {
        // Casting f32 -> f64 -> f32 is lossless for any non-NaN finite f32:
        // f64 can exactly represent every f32, and the back cast picks the
        // exact same value.
        for &c in &[0.0_f32, 1.0, -1.0, 1.234e10, -7.5e-12, f32::MIN, f32::MAX] {
            let v32: Vector3D<f32> = Vector3D::new([c, c, c]);
            let v64: Vector3D<f64> = v32.into();
            let back: Vector3D<f32> = v64.into();
            assert_eq!(v32.0, back.0, "round-trip failed for {c}");
        }
    }
}

#[cfg(test)]
mod vector3d_index_tests {
    use super::Vector3D;

    #[test]
    fn index_returns_each_component() {
        let v = Vector3D::new([10.0_f32, 20.0, 30.0]);
        assert_eq!(v[0], 10.0);
        assert_eq!(v[1], 20.0);
        assert_eq!(v[2], 30.0);
    }

    #[test]
    #[should_panic]
    fn index_out_of_bounds_panics() {
        let v = Vector3D::new([1.0_f32, 2.0, 3.0]);
        let _ = v[3];
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt;

use newtypes::{newtype_array, newtype_array_readonly, newtype_from_only, newtype_ord};

mod canonical_coords;

pub use canonical_coords::CanonicalCoords;

/// Which coordinate of a 3D point is being referred to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CoordAxis {
    X = 0,
    Y = 1,
    Z = 2,
}

impl CoordAxis {
    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    /// Returns the index of the axis in a `[T; 3]` coordinate array: 0, 1 or 2.
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

#[derive(Default, Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Vector3D<F: Copy + PartialEq + PartialOrd>(pub(crate) [F; 3]);

impl<F: Copy + PartialEq + PartialOrd> Vector3D<F> {
    pub const fn new(v: [F; 3]) -> Self {
        Self(v)
    }

    /// Returns the components as a fixed-size array, by value.
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
        assert!(i < 3);
        &self.0[i]
    }
}

// Distinct newtypes around `[f32; 3]` so a `Normal` cannot silently flow
// through `apply_to_vertex` and vice versa. For `Vertex`, `newtype_array!`
// emits `pub struct Vertex([f32; 3])` with `#[repr(transparent)]`, plus Debug,
// Clone, Copy, PartialEq, Index<I> and IndexMut<I> (single index and ranges),
// AsRef<[f32]>, AsMut<[f32]>, IntoIterator for `Self`, `&Self` and `&mut Self`,
// From<Self> for [f32; 3], and inherent `len`, `is_empty`, `iter`, `iter_mut`,
// `as_array` and `as_array_mut`. `newtype_array_readonly!` emits the same for
// `Normal`, without the mutable items.
newtype_array!(Vertex, f32, 3);
newtype_array_readonly!(Normal, f32, 3);

impl Vertex {
    /// Constructs a vertex from its 3 coordinates.
    #[inline]
    pub const fn new(coords: [f32; 3]) -> Self {
        Self(coords)
    }

    /// Returns the coordinates as a fixed-size array, by value.
    #[inline]
    pub fn coords(self) -> [f32; 3] {
        self.into()
    }

    /// Widens the coordinates to f64 for cancellation-safe math, where
    /// differences of f32 values lose precision.
    #[inline]
    pub fn to_f64(self) -> Vector3D<f64> {
        Vector3D::new(self.coords()).into()
    }

    /// Returns the coordinates in a form that is equal for coincident vertices,
    /// or `None` when a coordinate is NaN or infinite: such a vertex coincides
    /// with no other.
    #[inline]
    pub fn canonical_coords(self) -> Option<CanonicalCoords> {
        CanonicalCoords::from_coords(self.coords())
    }
}

impl Normal {
    /// Builds a normal from `components`, which the caller guarantees to have
    /// unit length. Outside mesh-core, `compute_normal_from_triangle_vertices`
    /// is the only way to get a normal.
    #[inline]
    pub(crate) const fn from_unit_unchecked(components: [f32; 3]) -> Self {
        Self(components)
    }

    /// Returns the components as a fixed-size array, by value.
    #[inline]
    pub fn coords(self) -> [f32; 3] {
        self.into()
    }

    /// Widens the components to f64 for cancellation-safe math.
    #[inline]
    pub fn to_f64(self) -> Vector3D<f64> {
        Vector3D::new(self.coords()).into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub(crate) vertices: [Vertex; 3],
}

impl Triangle {
    pub const fn new(vertices: [Vertex; 3]) -> Self {
        Self { vertices }
    }

    pub fn vertices(&self) -> [Vertex; 3] {
        self.vertices
    }
}

// Index into `IndexedMesh::vertices`.
newtype_ord!(VertexIndex, u32);
newtype_from_only!(VertexIndex, u32);

impl VertexIndex {
    /// Returns the raw `u32` index into `IndexedMesh::vertices`.
    #[inline]
    pub fn get(self) -> u32 {
        self.into()
    }

    /// Adds an integer offset to the index.
    #[inline]
    pub fn offset(self, by: u32) -> Self {
        Self::from(u32::from(self) + by)
    }
}

impl From<VertexIndex> for usize {
    #[inline]
    fn from(v: VertexIndex) -> Self {
        u32::from(v) as usize
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
    /// Returns the raw `usize` index into `IndexedMesh::faces`.
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

/// Which of a triangle's 3 corners is being referred to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TriVertexSlot {
    V0 = 0,
    V1 = 1,
    V2 = 2,
}

impl TriVertexSlot {
    pub const ALL: [Self; 3] = [Self::V0, Self::V1, Self::V2];

    /// Returns the corner's index in a triangle's `vertices` array: 0, 1 or 2.
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

/// Struct that represents a triangle in the context of a mesh, in which its
/// vertices are indexed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IndexedTriangle {
    pub vertices: [VertexIndex; 3],
}

impl IndexedTriangle {
    /// Builds an `IndexedTriangle` from 3 raw `u32` vertex indices.
    #[inline]
    pub fn from_raw_indices(vertices: [u32; 3]) -> Self {
        Self {
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
        // 0.1 + 0.2 in f64 cannot be represented exactly in f32; the conversion
        // is a plain `as f32` cast, so this checks that the conversion is a
        // component-wise cast (no rounding mode tweak).
        let v64: Vector3D<f64> = Vector3D::new([0.1 + 0.2, 1.0e30, -1.0e-30]);
        let v32: Vector3D<f32> = v64.into();
        assert_eq!(
            v32.0,
            [(0.1 + 0.2) as f32, 1.0e30_f64 as f32, -1.0e-30_f64 as f32]
        );
    }

    #[test]
    fn f32_to_f64_to_f32_is_identity_for_normal_finite() {
        // Casting f32 -> f64 -> f32 is lossless for any non-NaN finite f32: f64
        // can exactly represent every f32, and the back cast picks the exact
        // same value.
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

#[cfg(test)]
mod vertex_canonical_coords_tests {
    use super::{CanonicalCoords, Vertex};

    #[test]
    fn canonical_coords_gives_what_from_coords_gives() {
        for coords in [[1.0, -0.0, 2.5], [f32::NAN, 0.0, 0.0]] {
            assert_eq!(
                Vertex::new(coords).canonical_coords(),
                CanonicalCoords::from_coords(coords)
            );
        }
    }
}

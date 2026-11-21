/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A bounding volume hierarchy over one mesh's triangles (#683): rays (every
//! hit, or the first), the closest point to a point, and every triangle a
//! ball reaches.
//!
//! The tree is built with a binned surface-area heuristic. Triangles keep
//! their own numbers: a face index here is the triangle's place in the
//! geometry it was built from, so what was computed on that geometry (an
//! overhang region's faces) names the same triangles. Bounds are `f32`, the
//! queries `f64`.

mod build;
mod closest_point_to_point;
mod faces_within;
mod raycast;

pub use build::MeshBvhOptions;
pub use closest_point_to_point::ClosestPointTarget;
pub use raycast::{BvhMesh, Face, Intersection, Side};

use crate::three::{Triangle, Vec3};

/// One node: its bounds, then a leaf range of [`MeshBvh::order`] or the
/// index of its right child (the left child is the next node).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Node {
    /// `[minx, miny, minz, maxx, maxy, maxz]`
    pub bounds: [f32; 6],
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// Slots `offset .. offset + count` of the triangle order.
    Leaf { offset: u32, count: u32 },
    /// The right child; `split_axis` is the axis the triangles were split on.
    Inner { right: usize, split_axis: u32 },
}

/// The tree over one geometry: its positions, its index, the triangles in
/// tree order and the nodes.
#[derive(Debug, Clone)]
pub struct MeshBvh {
    positions: Vec<[f32; 3]>,
    /// Three vertex indices per triangle, in the geometry's own order.
    index: Vec<u32>,
    /// The triangles as the leaves hold them.
    order: Vec<u32>,
    nodes: Vec<Node>,
    /// Each triangle's bounds `[min xyz, max xyz]`, by triangle.
    triangle_bounds: Vec<[f32; 6]>,
    /// Whether the geometry came without an index: a triangle soup.
    soup: bool,
}

impl MeshBvh {
    /// What the tree holds on the heap, in bytes.
    pub fn heap_bytes(&self) -> usize {
        self.positions.capacity() * size_of::<[f32; 3]>()
            + self.index.capacity() * size_of::<u32>()
            + self.order.capacity() * size_of::<u32>()
            + self.nodes.capacity() * size_of::<Node>()
            + self.triangle_bounds.capacity() * size_of::<[f32; 6]>()
    }

    /// The tree over `positions`, three vertices per triangle by `index`, or
    /// in order when `index` is `None` (a triangle soup).
    pub fn new(positions: Vec<[f32; 3]>, index: Option<Vec<u32>>) -> Self {
        Self::with_options(positions, index, MeshBvhOptions::default())
    }

    pub fn with_options(
        positions: Vec<[f32; 3]>,
        index: Option<Vec<u32>>,
        options: MeshBvhOptions,
    ) -> Self {
        let soup = index.is_none();
        let index = index.unwrap_or_else(|| (0..positions.len() as u32).collect());
        assert!(
            index.len().is_multiple_of(3),
            "MeshBvh: index count {} is not a whole number of triangles",
            index.len()
        );
        let triangle_bounds: Vec<[f32; 6]> = index
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
                let mut bounds = [a[0], a[1], a[2], a[0], a[1], a[2]];
                for v in [b, c] {
                    for k in 0..3 {
                        bounds[k] = bounds[k].min(v[k]);
                        bounds[k + 3] = bounds[k + 3].max(v[k]);
                    }
                }
                bounds
            })
            .collect();
        let (order, nodes) = build::build(&triangle_bounds, options);
        MeshBvh {
            positions,
            index,
            order,
            nodes,
            triangle_bounds,
            soup,
        }
    }

    /// The face each triangle of the soup the tree was built from is: itself,
    /// since faces keep their numbers. `None` for a geometry that came with its
    /// own index.
    pub fn faces_of_soup(&self) -> Option<Vec<u32>> {
        self.soup
            .then(|| (0..self.triangle_count() as u32).collect())
    }

    /// Triangles in the geometry.
    pub fn triangle_count(&self) -> usize {
        self.index.len() / 3
    }

    /// The vertex positions.
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }

    /// The index, three entries per triangle.
    pub fn index(&self) -> &[u32] {
        &self.index
    }

    /// Vertex `i`.
    #[inline]
    pub fn vertex(&self, i: u32) -> Vec3 {
        let [x, y, z] = self.positions[i as usize];
        Vec3::new(f64::from(x), f64::from(y), f64::from(z))
    }

    /// The vertex indices of triangle `face`.
    #[inline]
    pub fn face(&self, face: usize) -> [u32; 3] {
        [
            self.index[face * 3],
            self.index[face * 3 + 1],
            self.index[face * 3 + 2],
        ]
    }

    /// Triangle `face`.
    #[inline]
    pub fn triangle(&self, face: usize) -> Triangle {
        let [i0, i1, i2] = self.face(face);
        Triangle::new(self.vertex(i0), self.vertex(i1), self.vertex(i2))
    }

    /// The triangles a leaf holds.
    #[inline]
    pub(crate) fn leaf(&self, offset: u32, count: u32) -> impl Iterator<Item = usize> + '_ {
        self.order[offset as usize..(offset + count) as usize]
            .iter()
            .map(|&t| t as usize)
    }
}

/// The squared distance from `point` to a box.
#[inline]
pub(crate) fn box_distance_sq(bounds: &[f32; 6], point: Vec3) -> f64 {
    let min = Vec3::new(
        f64::from(bounds[0]),
        f64::from(bounds[1]),
        f64::from(bounds[2]),
    );
    let max = Vec3::new(
        f64::from(bounds[3]),
        f64::from(bounds[4]),
        f64::from(bounds[5]),
    );
    point.clamp(min, max).distance_to_squared(point)
}

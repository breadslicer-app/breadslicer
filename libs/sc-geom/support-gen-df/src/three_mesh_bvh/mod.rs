//! three-mesh-bvh 0.8.3 (MIT, Garrett Johnson), the part DragonFruit's support
//! code relies on: `MeshBVH` built with DragonFruit's options (the library
//! defaults: `CENTER` split, `maxLeafTris` 10, `maxDepth` 40, not indirect),
//! `closestPointToPoint`, `raycast` and `raycastFirst`.
//!
//! Ported, not wrapped, because the answers depend on the tree: the build
//! reorders the triangle index in place, a face index refers to that reordered
//! order, and when two triangles are equally close the traversal order decides
//! which one reports — and so which face normal signs a distance. Node and
//! triangle bounds are `Float32Array`s in three-mesh-bvh and are `f32` here.
//!
//! Sources: `src/core/build/{buildTree,computeBoundsUtils,splitUtils,sortUtils}`,
//! `src/core/cast/{shapecast,closestPointToPoint}.js`, `src/core/MeshBVH.js`.
//! See `NOTICE`.

mod build;
mod closest_point_to_point;
mod faces_within;
mod raycast;

pub use build::MeshBvhOptions;
pub use closest_point_to_point::ClosestPointTarget;
pub use raycast::{BvhMesh, Face, Intersection, Side};

use crate::three::{Triangle, Vec3};

/// One packed BVH node: six `f32` bounds, then either a leaf range or a split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Node {
    /// `[minx, miny, minz, maxx, maxy, maxz]`
    pub bounds: [f32; 6],
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// Triangles `offset .. offset + count` of the reordered index. The count is
    /// a `uint16` in the packed buffer.
    Leaf { offset: u32, count: u16 },
    /// The left child is the next node; the right child is at `right`.
    Inner { right: usize, split_axis: u32 },
}

/// `MeshBVH` over one geometry: its positions, its (reordered) index and the
/// packed tree. A geometry without an index gets the sequential one
/// (`ensureIndex`), so a triangle soup and an indexed mesh with the same
/// triangles in the same order build the same tree.
#[derive(Debug, Clone)]
pub struct MeshBvh {
    positions: Vec<[f32; 3]>,
    /// Three vertex indices per triangle, in BVH order.
    index: Vec<u32>,
    /// One tree per geometry group range; DragonFruit's meshes have one.
    roots: Vec<Vec<Node>>,
    /// Each triangle's bounds `[min xyz, max xyz]`, in BVH order, for the
    /// closest-point query's per-triangle pre-check.
    triangle_bounds: Vec<[f32; 6]>,
    /// Whether the geometry came without an index: a triangle soup, whose
    /// triangle `t` holds the vertices `3t..3t + 3`.
    soup: bool,
}

impl MeshBvh {
    /// What the tree holds on the heap, in bytes: its vertices, index,
    /// nodes and triangle bounds.
    pub fn heap_bytes(&self) -> usize {
        self.positions.capacity() * std::mem::size_of::<[f32; 3]>()
            + self.index.capacity() * std::mem::size_of::<u32>()
            + self
                .roots
                .iter()
                .map(|r| r.capacity() * std::mem::size_of::<Node>())
                .sum::<usize>()
            + self.triangle_bounds.capacity() * std::mem::size_of::<[f32; 6]>()
    }

    /// `new MeshBVH(geometry)` with DragonFruit's options.
    ///
    /// `positions` is the position attribute, `index` the index attribute
    /// (`None` for a non-indexed geometry such as `STLLoader` produces).
    pub fn new(positions: Vec<[f32; 3]>, index: Option<Vec<u32>>) -> Self {
        Self::with_options(positions, index, MeshBvhOptions::default())
    }

    pub fn with_options(
        positions: Vec<[f32; 3]>,
        index: Option<Vec<u32>>,
        options: MeshBvhOptions,
    ) -> Self {
        // ensureIndex
        let soup = index.is_none();
        let mut index = index.unwrap_or_else(|| (0..positions.len() as u32).collect());
        assert!(
            index.len().is_multiple_of(3),
            "MeshBVH: index count {} is not a whole number of triangles",
            index.len()
        );
        let roots = build::build_packed_tree(&positions, &mut index, options);
        let triangle_bounds = index
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
        MeshBvh {
            positions,
            index,
            roots,
            triangle_bounds,
            soup,
        }
    }

    /// SoulCrafted (#500): the face of this tree (in BVH order) that each
    /// triangle of the soup it was built from became. The build reorders the
    /// triangles, so where a triangle stood in the soup says nothing of its
    /// face, and whatever was computed on the soup itself (the overhang
    /// scan's regions) names triangles by that place. `None` for a geometry
    /// that came with its own index, whose build does not keep where a
    /// triangle stood.
    pub fn faces_of_soup(&self) -> Option<Vec<u32>> {
        if !self.soup {
            return None;
        }
        let mut faces = vec![0u32; self.triangle_count()];
        for (face, corners) in self.index.as_chunks::<3>().0.iter().enumerate() {
            faces[corners[0] as usize / 3] = face as u32;
        }
        Some(faces)
    }

    /// Triangles in the geometry.
    pub fn triangle_count(&self) -> usize {
        self.index.len() / 3
    }

    /// The position attribute.
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }

    /// The reordered index attribute, three entries per triangle.
    pub fn index(&self) -> &[u32] {
        &self.index
    }

    /// Vertex `i` of the position attribute, as `fromBufferAttribute` reads it.
    #[inline]
    pub fn vertex(&self, i: u32) -> Vec3 {
        let [x, y, z] = self.positions[i as usize];
        Vec3::new(f64::from(x), f64::from(y), f64::from(z))
    }

    /// The vertex indices of triangle `face` (in BVH order).
    #[inline]
    pub fn face(&self, face: usize) -> [u32; 3] {
        [
            self.index[face * 3],
            self.index[face * 3 + 1],
            self.index[face * 3 + 2],
        ]
    }

    /// `setTriangle(triangle, face * 3, index, pos)`.
    #[inline]
    pub fn triangle(&self, face: usize) -> Triangle {
        let [i0, i1, i2] = self.face(face);
        Triangle::new(self.vertex(i0), self.vertex(i1), self.vertex(i2))
    }
}

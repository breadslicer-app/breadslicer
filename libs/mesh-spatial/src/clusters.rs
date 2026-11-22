/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Object-space cluster AABBs over an [`IndexedMesh`]'s face array.
//!
//! Faces are grouped by where they lie into clusters of
//! [`TriangleClusters::CLUSTER_SIZE`] (their centres split at the median
//! along the widest axis, again and again); each cluster gets one [`Aabb`]
//! covering the union of its triangle vertices. Grouping by the order of
//! [`IndexedMesh::faces`] instead gave a file whose faces come scattered
//! boxes as big as a sixth of the model, and every ray tested hundreds of
//! them.
//! Spatial-query consumers (the picker today, slicing later)
//! slab-test the cluster AABBs first and only iterate triangles
//! inside surviving clusters.
//!
//! A tree over the cluster boxes (#3) lets a query walk down to the
//! clusters it reaches, nearest first, instead of testing every box: a
//! ray through a model of two million triangles tested 30 000 boxes.
//!
//! The clustering is a pure function of the mesh's faces and vertex
//! positions. Cache lifetime is managed
//! by the surrounding [`crate::MeshSpatial`] / [`crate::SceneSpatial`]
//! layer; the type itself has no notion of staleness.

use sc_mesh_core::{Aabb, IndexedMesh, IndexedTriangle, Vertex};

/// Object-space AABBs over fixed-size runs of an [`IndexedMesh`]'s
/// faces. See the module docs for the build contract.
#[derive(Clone, Debug, PartialEq)]
pub struct TriangleClusters {
    face_count: u32,
    /// Face indices, cluster by cluster: cluster `i` holds
    /// `faces[i * CLUSTER_SIZE ..]`, up to `CLUSTER_SIZE` of them.
    faces: Vec<u32>,
    aabbs: Vec<Aabb>,
    /// The tree over `aabbs`, root first.
    nodes: Vec<TreeNode>,
    /// Cluster indices in the order the tree's leaves cover them.
    order: Vec<u32>,
}

/// A node of the tree over the cluster boxes: its box, and either the run
/// `order[first .. first + count]` of clusters (a leaf, `count > 0`) or
/// its children, the next node and `right`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TreeNode {
    aabb: Aabb,
    first: u32,
    count: u32,
    right: u32,
}

/// The most clusters a leaf of the tree holds.
const LEAF_CLUSTERS: usize = 4;

impl TriangleClusters {
    /// Number of faces packed into one cluster. Balanced between
    /// memory (one [`Aabb`] per cluster ≈ 24 B) and query speedup (a
    /// hit cluster pays a linear MT walk over up to this many
    /// faces).
    pub const CLUSTER_SIZE: u32 = 64;

    /// Build clusters from the vertex / face arrays of a mesh.
    /// Returns `None` when `faces` is empty (no clusters to emit) or
    /// when any face references an out-of-range vertex index;
    /// corrupt-mesh callers (e.g. the picker) treat this the same as
    /// "miss" rather than surfacing the error. For an in-range mesh,
    /// the result has `ceil(faces.len() / CLUSTER_SIZE)` clusters, every
    /// one full but the last, and holds every face once.
    pub fn build(vertices: &[Vertex], faces: &[IndexedTriangle]) -> Option<Self> {
        if faces.is_empty() {
            return None;
        }
        let mut centres = Vec::with_capacity(faces.len());
        for face in faces {
            let mut c = [0.0f32; 3];
            for vidx in face.vertices {
                let v = vertices.get(usize::from(vidx))?.coords();
                for k in 0..3 {
                    c[k] += v[k];
                }
            }
            centres.push(c);
        }
        let cluster_size = Self::CLUSTER_SIZE as usize;
        let mut order: Vec<u32> = (0..faces.len() as u32).collect();
        split_into_clusters(&centres, &mut order, cluster_size);
        let mut aabbs = Vec::with_capacity(faces.len().div_ceil(cluster_size));
        for chunk in order.chunks(cluster_size) {
            let mut aabb = Aabb::empty();
            for &f in chunk {
                for vidx in faces[f as usize].vertices {
                    aabb.extend(vertices[usize::from(vidx)]);
                }
            }
            aabbs.push(aabb);
        }
        let face_order = order;
        let mut order: Vec<u32> = (0..aabbs.len() as u32).collect();
        let mut nodes = Vec::with_capacity(2 * aabbs.len().div_ceil(LEAF_CLUSTERS));
        build_tree(&aabbs, &mut order, 0, &mut nodes);
        Some(Self {
            face_count: faces.len() as u32,
            faces: face_order,
            aabbs,
            nodes,
            order,
        })
    }

    /// Walks the clusters a query reaches, nearest first. `enter` says how
    /// far along the query a box starts (`None`: not reached at all);
    /// `visit` gets a cluster's face indices and the nearest hit so far,
    /// and returns a nearer one when it finds it. Boxes that start beyond
    /// the nearest hit are left out.
    pub fn walk_nearest(
        &self,
        enter: impl Fn(&Aabb) -> Option<f32>,
        mut visit: impl FnMut(&[u32], f32) -> Option<f32>,
    ) {
        let mut best = f32::INFINITY;
        let Some(root) = self.nodes.first() else {
            return;
        };
        let Some(t) = enter(&root.aabb) else {
            return;
        };
        let mut stack: Vec<(u32, f32)> = vec![(0, t)];
        while let Some((index, t)) = stack.pop() {
            if t > best {
                continue;
            }
            let node = self.nodes[index as usize];
            if node.count > 0 {
                for &cluster in &self.order[node.first as usize..(node.first + node.count) as usize]
                {
                    if enter(&self.aabbs[cluster as usize]).is_none_or(|t| t > best) {
                        continue;
                    }
                    if let Some(hit) = visit(self.cluster_faces(cluster as usize), best)
                        && hit < best
                    {
                        best = hit;
                    }
                }
                continue;
            }
            let (left, right) = (index + 1, node.right);
            let near = |i: u32| enter(&self.nodes[i as usize].aabb).map(|t| (i, t));
            match (near(left), near(right)) {
                (Some(a), Some(b)) => {
                    // The nearer is walked first: pushed last.
                    let (first, second) = if a.1 <= b.1 { (a, b) } else { (b, a) };
                    stack.push(second);
                    stack.push(first);
                }
                (Some(only), None) | (None, Some(only)) => stack.push(only),
                (None, None) => {}
            }
        }
    }

    /// Convenience builder over an entire [`IndexedMesh`].
    pub fn for_mesh(mesh: &IndexedMesh) -> Option<Self> {
        Self::build(&mesh.vertices, &mesh.faces)
    }

    /// Number of clusters. Always ≥ 1 for a `Some` build result and
    /// ≤ `ceil(face_count / CLUSTER_SIZE)`.
    pub fn len(&self) -> usize {
        self.aabbs.len()
    }

    /// Report whether the clustering holds no clusters. A
    /// constructed value has at least 1, so the answer is always
    /// `false`.
    pub fn is_empty(&self) -> bool {
        self.aabbs.is_empty()
    }

    /// Total number of faces covered by the clustering. Equal to the
    /// `faces.len()` value passed at build time.
    pub fn face_count(&self) -> u32 {
        self.face_count
    }

    /// AABB of the cluster at index `i`. Panics if `i >= self.len()`.
    pub fn aabb(&self, i: usize) -> Aabb {
        self.aabbs[i]
    }

    /// The face indices of cluster `i` (into the `faces` it was built
    /// from). Panics if `i >= self.len()`.
    pub fn cluster_faces(&self, i: usize) -> &[u32] {
        let size = Self::CLUSTER_SIZE as usize;
        &self.faces[i * size..((i + 1) * size).min(self.faces.len())]
    }

    /// Iterate `(face indices, aabb)` pairs in cluster order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u32], Aabb)> + '_ {
        self.aabbs
            .iter()
            .enumerate()
            .map(move |(i, aabb)| (self.cluster_faces(i), *aabb))
    }
}

/// Orders `order` so each run of `size` faces lies together: the faces'
/// `centres` are split at the median along their widest axis, at a multiple
/// of `size` so every cluster but the last is full, until a run fits.
fn split_into_clusters(centres: &[[f32; 3]], order: &mut [u32], size: usize) {
    if order.len() <= size {
        return;
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for &f in order.iter() {
        let c = centres[f as usize];
        for k in 0..3 {
            lo[k] = lo[k].min(c[k]);
            hi[k] = hi[k].max(c[k]);
        }
    }
    let axis = (0..3)
        .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
        .unwrap_or(0);
    let mid = order.len().div_ceil(size) / 2 * size;
    order.select_nth_unstable_by(mid, |&a, &b| {
        centres[a as usize][axis].total_cmp(&centres[b as usize][axis])
    });
    let (left, right) = order.split_at_mut(mid);
    split_into_clusters(centres, left, size);
    split_into_clusters(centres, right, size);
}

/// Builds the tree over `aabbs[order[..]]` into `nodes`, splitting each
/// run at the median of its box centres along their widest axis.
fn build_tree(aabbs: &[Aabb], order: &mut [u32], first: u32, nodes: &mut Vec<TreeNode>) -> u32 {
    let index = nodes.len() as u32;
    let mut aabb = Aabb::empty();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for &c in order.iter() {
        let b = aabbs[c as usize];
        aabb.extend(b.min());
        aabb.extend(b.max());
        let centre = centre_of(&b);
        for k in 0..3 {
            lo[k] = lo[k].min(centre[k]);
            hi[k] = hi[k].max(centre[k]);
        }
    }
    nodes.push(TreeNode {
        aabb,
        first,
        count: 0,
        right: 0,
    });
    if order.len() <= LEAF_CLUSTERS {
        nodes[index as usize].count = order.len() as u32;
        return index;
    }
    let axis = (0..3)
        .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
        .unwrap_or(0);
    let mid = order.len() / 2;
    order.select_nth_unstable_by(mid, |&a, &b| {
        centre_of(&aabbs[a as usize])[axis].total_cmp(&centre_of(&aabbs[b as usize])[axis])
    });
    let (left, right) = order.split_at_mut(mid);
    build_tree(aabbs, left, first, nodes);
    let right_index = build_tree(aabbs, right, first + mid as u32, nodes);
    nodes[index as usize].right = right_index;
    index
}

fn centre_of(b: &Aabb) -> [f32; 3] {
    let (lo, hi) = (b.min().coords(), b.max().coords());
    [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ]
}

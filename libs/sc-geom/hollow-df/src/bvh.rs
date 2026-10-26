//! Axis-aligned bounding volume hierarchy over the triangles of an
//! [`IndexedMesh`].
//!
//! Ported from DragonFruit `rust/dragonfruit-mesh-core/src/bvh.rs`: the flat,
//! leaf-batched build and the ray hit count the parity tests of the hollowing
//! engine use, plus [`Bvh::ray_hit_distances`] for automatic drain depth.

use rayon::prelude::*;

use crate::mesh::{Aabb, IndexedMesh, Vec3};

/// Triangles per leaf (DragonFruit measured 8 best over 1, 2, 4, 16).
const LEAF_FACES: usize = 8;
/// Marks [`FlatNode::b`] as a leaf's face count rather than a right child.
const LEAF_FLAG: u32 = 1 << 31;
/// Traversal stack depth; covers any mesh a `u32` face index can address.
const STACK_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug)]
struct FlatNode {
    min: [f32; 3],
    max: [f32; 3],
    /// Internal: left child. Leaf: first index into `faces`.
    a: u32,
    /// Internal: right child. Leaf: `LEAF_FLAG | face count`.
    b: u32,
}

impl FlatNode {
    #[inline]
    fn is_leaf(&self) -> bool {
        self.b & LEAF_FLAG != 0
    }

    #[inline]
    fn face_count(&self) -> usize {
        (self.b & !LEAF_FLAG) as usize
    }
}

pub struct Bvh {
    nodes: Vec<FlatNode>,
    /// Triangle indices, grouped by leaf.
    faces: Vec<u32>,
    root: u32,
}

impl Bvh {
    pub fn build(mesh: &IndexedMesh) -> Self {
        let count = mesh.triangles.len();
        let mut prims: Vec<(u32, Aabb, Vec3)> = mesh
            .triangles
            .par_iter()
            .enumerate()
            .map(|(i, tri)| {
                let a = mesh.positions[tri[0] as usize];
                let b = mesh.positions[tri[1] as usize];
                let c = mesh.positions[tri[2] as usize];
                let mut bb = Aabb::empty();
                bb.expand(a);
                bb.expand(b);
                bb.expand(c);
                let centroid = a.add(b).add(c).scale(1.0 / 3.0);
                (i as u32, bb, centroid)
            })
            .collect();

        let mut nodes = Vec::with_capacity(count / LEAF_FACES * 2 + 2);
        let mut faces = vec![0u32; count];
        let mut written = 0usize;
        let root = if count == 0 {
            0
        } else {
            build_rec(&mut nodes, &mut faces, &mut written, &mut prims)
        };
        Self { nodes, faces, root }
    }

    /// How many triangles the ray from `origin` along `dir` crosses.
    pub fn ray_hit_count(&self, mesh: &IndexedMesh, origin: Vec3, dir: Vec3) -> u32 {
        let mut count = 0u32;
        self.traverse(mesh, origin, dir, |_| count += 1);
        count
    }

    /// Distances along `dir` (in units of `dir`'s length) of every triangle
    /// the ray crosses, sorted ascending.
    pub fn ray_hit_distances(&self, mesh: &IndexedMesh, origin: Vec3, dir: Vec3) -> Vec<f32> {
        let mut hits = Vec::new();
        self.traverse(mesh, origin, dir, |t| hits.push(t));
        hits.sort_by(f32::total_cmp);
        hits
    }

    fn traverse(&self, mesh: &IndexedMesh, origin: Vec3, dir: Vec3, mut hit: impl FnMut(f32)) {
        if self.nodes.is_empty() {
            return;
        }
        let inv_dir = inverse_direction(dir);
        let mut stack = [0u32; STACK_DEPTH];
        let mut depth = 1usize;
        stack[0] = self.root;
        while depth > 0 {
            depth -= 1;
            let node = self.nodes[stack[depth] as usize];
            if node_entry(origin, inv_dir, &node, f32::INFINITY).is_infinite() {
                continue;
            }
            if node.is_leaf() {
                for k in 0..node.face_count() {
                    let face = self.faces[node.a as usize + k];
                    let [a, b, c] = mesh.tri_positions(face);
                    if let Some(t) = ray_tri(origin, dir, a, b, c) {
                        hit(t);
                    }
                }
            } else {
                stack[depth] = node.a;
                stack[depth + 1] = node.b;
                depth += 2;
            }
        }
    }
}

/// Recursively split `prims` into nodes, appending leaves to `nodes` and their
/// face indices to `faces`. `written` is the next free slot in `faces`.
fn build_rec(
    nodes: &mut Vec<FlatNode>,
    faces: &mut [u32],
    written: &mut usize,
    prims: &mut [(u32, Aabb, Vec3)],
) -> u32 {
    let mut bbox = Aabb::empty();
    for &(_, bb, _) in prims.iter() {
        bbox.union(&bb);
    }

    if prims.len() <= LEAF_FACES {
        let first = *written as u32;
        for &(face, _, _) in prims.iter() {
            faces[*written] = face;
            *written += 1;
        }
        let idx = nodes.len() as u32;
        nodes.push(FlatNode {
            min: bbox.min.to_array(),
            max: bbox.max.to_array(),
            a: first,
            b: LEAF_FLAG | prims.len() as u32,
        });
        return idx;
    }

    // Split along the axis with the largest centroid spread, at the median.
    let mut cmin = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut cmax = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &(_, _, c) in prims.iter() {
        cmin = cmin.min(c);
        cmax = cmax.max(c);
    }
    let ext = cmax.sub(cmin);
    let axis = if ext.x >= ext.y && ext.x >= ext.z {
        0
    } else if ext.y >= ext.z {
        1
    } else {
        2
    };
    let mid = prims.len() / 2;
    prims.select_nth_unstable_by(mid, |a, b| {
        let (av, bv) = match axis {
            0 => (a.2.x, b.2.x),
            1 => (a.2.y, b.2.y),
            _ => (a.2.z, b.2.z),
        };
        av.total_cmp(&bv)
    });
    let (left_slice, right_slice) = prims.split_at_mut(mid);

    let left = build_rec(nodes, faces, written, left_slice);
    let right = build_rec(nodes, faces, written, right_slice);

    let idx = nodes.len() as u32;
    nodes.push(FlatNode {
        min: bbox.min.to_array(),
        max: bbox.max.to_array(),
        a: left,
        b: right,
    });
    idx
}

#[inline]
fn inverse_direction(dir: Vec3) -> Vec3 {
    let axis = |v: f32| {
        if v.abs() > 1e-20 {
            1.0 / v
        } else {
            f32::INFINITY
        }
    };
    Vec3::new(axis(dir.x), axis(dir.y), axis(dir.z))
}

/// Distance along the ray at which it enters `node`, or infinity if it misses
/// or enters beyond `max_distance`.
#[inline]
fn node_entry(origin: Vec3, inv_dir: Vec3, node: &FlatNode, max_distance: f32) -> f32 {
    let t1 = (node.min[0] - origin.x) * inv_dir.x;
    let t2 = (node.max[0] - origin.x) * inv_dir.x;
    let t3 = (node.min[1] - origin.y) * inv_dir.y;
    let t4 = (node.max[1] - origin.y) * inv_dir.y;
    let t5 = (node.min[2] - origin.z) * inv_dir.z;
    let t6 = (node.max[2] - origin.z) * inv_dir.z;
    let tmin = t1.min(t2).max(t3.min(t4)).max(t5.min(t6)).max(0.0);
    let tmax = t1.max(t2).min(t3.max(t4)).min(t5.max(t6));
    if tmax < tmin || tmin > max_distance {
        f32::INFINITY
    } else {
        tmin
    }
}

/// Möller–Trumbore triangle intersection; `t >= 0` if hit in front.
pub fn ray_tri(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    const EPS: f32 = 1e-8;
    let e1 = b.sub(a);
    let e2 = c.sub(a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < EPS {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin.sub(a);
    let u = s.dot(p) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv_det;
    if t >= 0.0 { Some(t) } else { None }
}

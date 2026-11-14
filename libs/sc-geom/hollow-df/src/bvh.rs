//! Axis-aligned bounding volume hierarchy over the triangles of an
//! [`IndexedMesh`].
//!
//! Ported from DragonFruit `rust/dragonfruit-mesh-core/src/bvh.rs`: the flat,
//! leaf-batched build and the ray hit count the parity tests of the hollowing
//! engine use, plus [`Bvh::ray_hit_distances`] for automatic drain depth and
//! [`Bvh::nearest_distance`] for the exact wall distance.

use rayon::prelude::*;

use crate::hollowing::point_triangle_distance;
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

#[derive(Clone)]
pub struct Bvh {
    nodes: Vec<FlatNode>,
    /// Triangle indices, grouped by leaf.
    faces: Vec<u32>,
    root: u32,
}

impl std::fmt::Debug for Bvh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bvh")
            .field("nodes", &self.nodes.len())
            .field("faces", &self.faces.len())
            .finish()
    }
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

        if count == 0 {
            return Self {
                nodes: Vec::new(),
                faces: Vec::new(),
                root: 0,
            };
        }
        let (nodes, faces) = build_halves(&mut prims, PARALLEL_LEVELS);
        let root = nodes.len() as u32 - 1;
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

    /// [`Bvh::ray_hit_distances`] for a closed mesh, without gaps: a ray
    /// through an edge or a vertex hits every triangle there (so the same
    /// distance can come more than once) instead of slipping between them,
    /// which the exact test can do by rounding.
    pub fn ray_hit_distances_closed(
        &self,
        mesh: &IndexedMesh,
        origin: Vec3,
        dir: Vec3,
    ) -> Vec<f32> {
        let mut hits = Vec::new();
        self.traverse_with(mesh, origin, dir, ray_tri_closed, |t| hits.push(t));
        hits.sort_by(f32::total_cmp);
        hits
    }

    /// Distance from `p` to the closest triangle of `mesh`, if one is closer
    /// than `max_distance`. Nearer boxes are visited first, and boxes no
    /// closer than the best triangle so far are skipped.
    pub fn nearest_distance(&self, mesh: &IndexedMesh, p: Vec3, max_distance: f32) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = max_distance * max_distance;
        let mut found = false;
        let mut stack = [0u32; STACK_DEPTH];
        let mut depth = 1usize;
        stack[0] = self.root;
        while depth > 0 {
            depth -= 1;
            let node = &self.nodes[stack[depth] as usize];
            if box_distance_sq(node, p) >= best {
                continue;
            }
            if node.is_leaf() {
                for k in 0..node.face_count() {
                    let [a, b, c] = mesh.tri_positions(self.faces[node.a as usize + k]);
                    let d = point_triangle_distance(p, a, b, c);
                    if d * d < best {
                        best = d * d;
                        found = true;
                    }
                }
            } else {
                let near_a = box_distance_sq(&self.nodes[node.a as usize], p);
                let near_b = box_distance_sq(&self.nodes[node.b as usize], p);
                // The farther child goes first on the stack, so the nearer
                // is visited first and tightens `best` for the other.
                let (far, near) = if near_a <= near_b {
                    ((node.b, near_b), (node.a, near_a))
                } else {
                    ((node.a, near_a), (node.b, near_b))
                };
                for (child, d2) in [far, near] {
                    if d2 < best {
                        stack[depth] = child;
                        depth += 1;
                    }
                }
            }
        }
        found.then(|| best.sqrt())
    }

    fn traverse(&self, mesh: &IndexedMesh, origin: Vec3, dir: Vec3, hit: impl FnMut(f32)) {
        self.traverse_with(mesh, origin, dir, ray_tri, hit);
    }

    fn traverse_with(
        &self,
        mesh: &IndexedMesh,
        origin: Vec3,
        dir: Vec3,
        test: impl Fn(Vec3, Vec3, Vec3, Vec3, Vec3) -> Option<f32>,
        mut hit: impl FnMut(f32),
    ) {
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
                    if let Some(t) = test(origin, dir, a, b, c) {
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
/// How many levels of the tree have their halves built side by side.
const PARALLEL_LEVELS: u32 = 6;
/// Fewer triangles than this are built on one core.
const PARALLEL_MIN_FACES: usize = 1 << 15;

/// The nodes and faces [`build_rec`] lays out for `prims`, the top
/// `levels` levels' halves built on separate cores (#451): each half on
/// its own, then the left's nodes and faces, the right's after them and
/// the parent last, as one core builds them, so the tree is the same.
fn build_halves(prims: &mut [(u32, Aabb, Vec3)], levels: u32) -> (Vec<FlatNode>, Vec<u32>) {
    if levels == 0 || prims.len() < PARALLEL_MIN_FACES {
        let mut nodes = Vec::with_capacity(prims.len() / LEAF_FACES * 2 + 2);
        let mut faces = vec![0u32; prims.len()];
        let mut written = 0usize;
        build_rec(&mut nodes, &mut faces, &mut written, prims);
        return (nodes, faces);
    }
    let bbox = bounds_of(prims);
    let mid = split_at_median(prims);
    let (left_slice, right_slice) = prims.split_at_mut(mid);
    let ((mut nodes, mut faces), (right_nodes, right_faces)) = rayon::join(
        || build_halves(left_slice, levels - 1),
        || build_halves(right_slice, levels - 1),
    );
    let left = nodes.len() as u32 - 1;
    let (node_shift, face_shift) = (nodes.len() as u32, faces.len() as u32);
    nodes.extend(right_nodes.into_iter().map(|mut node| {
        if node.is_leaf() {
            node.a += face_shift;
        } else {
            node.a += node_shift;
            node.b += node_shift;
        }
        node
    }));
    faces.extend(right_faces);
    let right = nodes.len() as u32 - 1;
    nodes.push(FlatNode {
        min: bbox.min.to_array(),
        max: bbox.max.to_array(),
        a: left,
        b: right,
    });
    (nodes, faces)
}

/// The box of `prims`.
fn bounds_of(prims: &[(u32, Aabb, Vec3)]) -> Aabb {
    let mut bbox = Aabb::empty();
    for &(_, bb, _) in prims {
        bbox.union(&bb);
    }
    bbox
}

/// Splits `prims` along the axis their centroids spread most, at the
/// median, and returns where the right half starts.
fn split_at_median(prims: &mut [(u32, Aabb, Vec3)]) -> usize {
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
    mid
}

fn build_rec(
    nodes: &mut Vec<FlatNode>,
    faces: &mut [u32],
    written: &mut usize,
    prims: &mut [(u32, Aabb, Vec3)],
) -> u32 {
    let bbox = bounds_of(prims);

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
    let mid = split_at_median(prims);
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

/// Squared distance from `p` to `node`'s box (0 inside it).
#[inline]
fn box_distance_sq(node: &FlatNode, p: Vec3) -> f32 {
    p.to_array()
        .iter()
        .zip(node.min.iter().zip(&node.max))
        .map(|(&p, (&min, &max))| {
            let v = (min - p).max(p - max).max(0.0);
            v * v
        })
        .sum()
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
    // A ray parallel to a slab is in it or misses it: with the inverse at
    // infinity, `(bound - origin) * inv` would be NaN for an origin on the
    // bound, and the box would be skipped (a ray along an axis through a
    // box face slipped past every triangle in it, #315).
    let slab = |min: f32, max: f32, o: f32, inv: f32| -> (f32, f32) {
        if inv.is_infinite() {
            if (min..=max).contains(&o) {
                (f32::NEG_INFINITY, f32::INFINITY)
            } else {
                (f32::INFINITY, f32::NEG_INFINITY)
            }
        } else {
            let (a, b) = ((min - o) * inv, (max - o) * inv);
            (a.min(b), a.max(b))
        }
    };
    let (x0, x1) = slab(node.min[0], node.max[0], origin.x, inv_dir.x);
    let (y0, y1) = slab(node.min[1], node.max[1], origin.y, inv_dir.y);
    let (z0, z1) = slab(node.min[2], node.max[2], origin.z, inv_dir.z);
    let tmin = x0.max(y0).max(z0).max(0.0);
    let tmax = x1.min(y1).min(z1);
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

/// [`ray_tri`] with the triangle grown by a hair in barycentric terms
/// (`1e-4` of its size), so a ray through a shared edge or vertex hits the
/// triangles on both sides rather than slipping between them, and tiny
/// triangles counted.
pub fn ray_tri_closed(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    const GROW: f32 = 1e-4;
    let e1 = b.sub(a);
    let e2 = c.sub(a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    // Relative to the triangle's size: a tiny triangle (a contour through a
    // grid corner makes them) is still hit; one edge-on to the ray is not.
    if det.abs() <= 1e-7 * e1.dot(e1).sqrt() * p.dot(p).sqrt() {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin.sub(a);
    let u = s.dot(p) * inv_det;
    if !(-GROW..=1.0 + GROW).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv_det;
    if v < -GROW || u + v > 1.0 + GROW {
        return None;
    }
    let t = e2.dot(q) * inv_det;
    if t >= 0.0 { Some(t) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::icosphere;

    /// The tree built on several cores is the one a single core builds
    /// (#451): the same nodes, in the same places, over the same faces.
    #[test]
    fn the_tree_built_on_every_core_is_the_one_built_on_one() {
        let mesh = icosphere(10.0, 7);
        assert!(mesh.triangles.len() > 4 * PARALLEL_MIN_FACES);
        let prims = |mesh: &IndexedMesh| -> Vec<(u32, Aabb, Vec3)> {
            mesh.triangles
                .iter()
                .enumerate()
                .map(|(i, tri)| {
                    let [a, b, c] = tri.map(|v| mesh.positions[v as usize]);
                    let mut bb = Aabb::empty();
                    bb.expand(a);
                    bb.expand(b);
                    bb.expand(c);
                    (i as u32, bb, a.add(b).add(c).scale(1.0 / 3.0))
                })
                .collect()
        };
        let (one_nodes, one_faces) = build_halves(&mut prims(&mesh), 0);
        let (nodes, faces) = build_halves(&mut prims(&mesh), PARALLEL_LEVELS);
        assert_eq!(faces, one_faces);
        assert_eq!(nodes.len(), one_nodes.len());
        for (a, b) in nodes.iter().zip(&one_nodes) {
            assert_eq!((a.min, a.max, a.a, a.b), (b.min, b.max, b.a, b.b));
        }
        let built = Bvh::build(&mesh);
        assert_eq!(built.root as usize, one_nodes.len() - 1);
    }
}

//! A tree of boxes over the triangles of an [`IndexedMesh`], for the rays
//! and the nearest-surface questions the hollow asks of a mesh: whether a
//! point is inside, where a drain meets the cavity, how thick a wall is.
//!
//! Built by the surface area heuristic over binned centroids: each split
//! takes the plane, out of a few per axis, that makes the two halves'
//! surface times triangles smallest. Large halves build on separate cores;
//! the tree is laid out depth first afterwards, so it is the same every run.

use rayon::prelude::*;

use crate::mesh::{IndexedMesh, Vec3};

/// Most triangles a leaf holds.
const LEAF_FACES: usize = 8;
/// Candidate planes per axis.
const BINS: usize = 12;
/// Halves of fewer triangles than this build on the core that split them.
const PARALLEL_FACES: usize = 1 << 14;

#[derive(Clone, Copy, Debug)]
struct Bounds {
    min: [f32; 3],
    max: [f32; 3],
}

impl Bounds {
    const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    fn grow(&mut self, o: &Bounds) {
        for k in 0..3 {
            self.min[k] = self.min[k].min(o.min[k]);
            self.max[k] = self.max[k].max(o.max[k]);
        }
    }

    fn area(&self) -> f32 {
        let d = [0, 1, 2].map(|k| (self.max[k] - self.min[k]).max(0.0));
        d[0] * d[1] + d[1] * d[2] + d[2] * d[0]
    }

    /// Squared distance from `p` to the box, 0 inside.
    fn distance_sq(&self, p: Vec3) -> f32 {
        let p = p.to_array();
        (0..3)
            .map(|k| {
                let d = (self.min[k] - p[k]).max(p[k] - self.max[k]).max(0.0);
                d * d
            })
            .sum()
    }

    /// Whether the ray `origin + t·dir`, `t ≥ 0`, passes through the box
    /// (`inverse` is `1 / dir`, infinite along an axis `dir` keeps still).
    fn crossed(&self, origin: [f32; 3], inverse: [f32; 3]) -> bool {
        let (mut near, mut far) = (0.0f32, f32::INFINITY);
        for k in 0..3 {
            if inverse[k].is_infinite() {
                // Parallel to this axis's faces: through the box's slab or not.
                if origin[k] < self.min[k] || origin[k] > self.max[k] {
                    return false;
                }
                continue;
            }
            let a = (self.min[k] - origin[k]) * inverse[k];
            let b = (self.max[k] - origin[k]) * inverse[k];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
        near <= far
    }
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Bounds,
    /// A leaf's first slot in `faces`, or an inner node's second child (its
    /// first is the next node).
    at: u32,
    /// Triangles of a leaf; 0 for an inner node.
    count: u32,
}

/// The tree, built by [`Bvh::build`] and asked about with the same mesh.
#[derive(Clone)]
pub struct Bvh {
    nodes: Vec<Node>,
    faces: Vec<u32>,
}

impl std::fmt::Debug for Bvh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bvh")
            .field("nodes", &self.nodes.len())
            .field("faces", &self.faces.len())
            .finish()
    }
}

/// A triangle while the tree is built: its number, box and centroid.
#[derive(Clone, Copy)]
struct Item {
    face: u32,
    bounds: Bounds,
    centre: [f32; 3],
}

enum Built {
    Leaf(Bounds, Vec<u32>),
    Split(Bounds, Box<Built>, Box<Built>),
}

fn bounds_of(items: &[Item]) -> Bounds {
    let mut b = Bounds::EMPTY;
    items.iter().for_each(|i| b.grow(&i.bounds));
    b
}

/// Where to split `items` (already bounded by `whole`): the axis, the
/// lowest centroid and span along it, and the first bin of the right half;
/// `None` when no split costs less than a leaf.
fn best_split(items: &[Item], whole: &Bounds) -> Option<(usize, f32, f32, usize)> {
    let mut cmin = [f32::INFINITY; 3];
    let mut cmax = [f32::NEG_INFINITY; 3];
    for i in items {
        for k in 0..3 {
            cmin[k] = cmin[k].min(i.centre[k]);
            cmax[k] = cmax[k].max(i.centre[k]);
        }
    }
    let mut best: Option<(f32, usize, usize)> = None;
    let bin = |i: &Item, axis: usize, span: f32| {
        (((i.centre[axis] - cmin[axis]) / span * BINS as f32) as usize).min(BINS - 1)
    };
    for axis in 0..3 {
        let span = cmax[axis] - cmin[axis];
        if span <= 0.0 {
            continue;
        }
        let mut bins = [(Bounds::EMPTY, 0usize); BINS];
        for i in items {
            let b = &mut bins[bin(i, axis, span)];
            b.0.grow(&i.bounds);
            b.1 += 1;
        }
        // Cost of every plane between bins: left sums from below, right from above.
        let mut right = [(0.0f32, 0usize); BINS];
        let (mut acc, mut n) = (Bounds::EMPTY, 0);
        for b in (1..BINS).rev() {
            acc.grow(&bins[b].0);
            n += bins[b].1;
            right[b] = (acc.area(), n);
        }
        let (mut acc, mut n) = (Bounds::EMPTY, 0);
        for b in 1..BINS {
            acc.grow(&bins[b - 1].0);
            n += bins[b - 1].1;
            let (right_area, right_n) = right[b];
            if n == 0 || right_n == 0 {
                continue;
            }
            let cost = acc.area() * n as f32 + right_area * right_n as f32;
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, axis, b));
            }
        }
    }
    let leaf_cost = whole.area() * items.len() as f32;
    best.filter(|&(cost, _, _)| items.len() > LEAF_FACES || cost < leaf_cost)
        .map(|(_, axis, first_right)| (axis, cmin[axis], cmax[axis] - cmin[axis], first_right))
}

fn build(items: &mut [Item]) -> Built {
    let whole = bounds_of(items);
    let split = (items.len() > LEAF_FACES)
        .then(|| best_split(items, &whole))
        .flatten();
    let Some((axis, low, span, first_right)) = split else {
        if items.len() > LEAF_FACES {
            // Every centroid in one place: halve by count.
            let (a, b) = items.split_at_mut(items.len() / 2);
            let (a, b) = (build(a), build(b));
            return Built::Split(whole, Box::new(a), Box::new(b));
        }
        return Built::Leaf(whole, items.iter().map(|i| i.face).collect());
    };
    // The items of the left bins first, in one pass.
    let goes_left = |i: &Item| {
        (((i.centre[axis] - low) / span * BINS as f32) as usize).min(BINS - 1) < first_right
    };
    let mut left = 0;
    for k in 0..items.len() {
        if goes_left(&items[k]) {
            items.swap(left, k);
            left += 1;
        }
    }
    let (a, b) = items.split_at_mut(left);
    let (a, b) = if a.len() + b.len() >= PARALLEL_FACES {
        rayon::join(|| build(a), || build(b))
    } else {
        (build(a), build(b))
    };
    Built::Split(whole, Box::new(a), Box::new(b))
}

impl Bvh {
    pub fn build(mesh: &IndexedMesh) -> Self {
        let mut items: Vec<Item> = mesh
            .triangles
            .par_iter()
            .enumerate()
            .map(|(face, t)| {
                let [a, b, c] = t.map(|i| mesh.positions[i as usize].to_array());
                let bounds = Bounds {
                    min: [0, 1, 2].map(|k| a[k].min(b[k]).min(c[k])),
                    max: [0, 1, 2].map(|k| a[k].max(b[k]).max(c[k])),
                };
                let centre = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0);
                Item {
                    face: face as u32,
                    bounds,
                    centre,
                }
            })
            .collect();
        let mut tree = Self {
            nodes: Vec::new(),
            faces: Vec::with_capacity(items.len()),
        };
        if !items.is_empty() {
            tree.lay_out(build(&mut items));
        }
        tree
    }

    /// Appends `built` depth first: an inner node, its first child right
    /// after it, its second child where `at` says.
    fn lay_out(&mut self, built: Built) {
        match built {
            Built::Leaf(bounds, faces) => {
                self.nodes.push(Node {
                    bounds,
                    at: self.faces.len() as u32,
                    count: faces.len() as u32,
                });
                self.faces.extend(faces);
            }
            Built::Split(bounds, a, b) => {
                let me = self.nodes.len();
                self.nodes.push(Node {
                    bounds,
                    at: 0,
                    count: 0,
                });
                self.lay_out(*a);
                self.nodes[me].at = self.nodes.len() as u32;
                self.lay_out(*b);
            }
        }
    }

    /// Calls `hit(t, face)` for every triangle `test` says the ray crosses.
    fn cast(
        &self,
        mesh: &IndexedMesh,
        origin: Vec3,
        dir: Vec3,
        test: impl Fn(Vec3, Vec3, Vec3, Vec3, Vec3) -> Option<f32>,
        mut hit: impl FnMut(f32, u32),
    ) {
        if self.nodes.is_empty() {
            return;
        }
        let o = origin.to_array();
        let inverse = dir.to_array().map(|d| 1.0 / d);
        let mut stack = vec![0u32];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n as usize];
            if !node.bounds.crossed(o, inverse) {
                continue;
            }
            if node.count == 0 {
                stack.push(node.at);
                stack.push(n + 1);
                continue;
            }
            for &face in &self.faces[node.at as usize..(node.at + node.count) as usize] {
                let [a, b, c] = mesh.tri_positions(face);
                if let Some(t) = test(origin, dir, a, b, c) {
                    hit(t, face);
                }
            }
        }
    }

    /// How many triangles the ray from `origin` along `dir` crosses.
    pub fn ray_hit_count(&self, mesh: &IndexedMesh, origin: Vec3, dir: Vec3) -> u32 {
        let mut count = 0;
        self.cast(mesh, origin, dir, ray_tri, |_, _| count += 1);
        count
    }

    /// How far along `dir` (in its lengths) the ray crosses each triangle,
    /// nearest first.
    pub fn ray_hit_distances(&self, mesh: &IndexedMesh, origin: Vec3, dir: Vec3) -> Vec<f32> {
        let mut hits = Vec::new();
        self.cast(mesh, origin, dir, ray_tri, |t, _| hits.push(t));
        hits.sort_by(f32::total_cmp);
        hits
    }

    /// [`Bvh::ray_hit_distances`] for a closed mesh, with no gaps: a ray
    /// through an edge or a corner hits every triangle there (the same
    /// distance can come more than once) rather than slipping between them.
    pub fn ray_hit_distances_closed(
        &self,
        mesh: &IndexedMesh,
        origin: Vec3,
        dir: Vec3,
    ) -> Vec<f32> {
        let mut hits = Vec::new();
        self.cast(mesh, origin, dir, ray_tri_closed, |t, _| hits.push(t));
        hits.sort_by(f32::total_cmp);
        hits
    }

    /// [`Bvh::ray_hit_distances_closed`] with the triangle each distance is
    /// on.
    pub fn ray_hits_closed(&self, mesh: &IndexedMesh, origin: Vec3, dir: Vec3) -> Vec<(f32, u32)> {
        let mut hits = Vec::new();
        self.cast(mesh, origin, dir, ray_tri_closed, |t, f| hits.push((t, f)));
        hits.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        hits
    }

    /// The distance from `p` to the nearest triangle, when one is closer than
    /// `max_distance`. The nearer child is opened first; boxes no nearer
    /// than the best triangle found are passed over.
    pub fn nearest_distance(&self, mesh: &IndexedMesh, p: Vec3, max_distance: f32) -> Option<f32> {
        let mut best = max_distance * max_distance;
        let mut found = false;
        let mut stack: Vec<(u32, f32)> = Vec::new();
        if let Some(root) = self.nodes.first() {
            stack.push((0, root.bounds.distance_sq(p)));
        }
        while let Some((n, d2)) = stack.pop() {
            if d2 >= best {
                continue;
            }
            let node = &self.nodes[n as usize];
            if node.count == 0 {
                let kids =
                    [n + 1, node.at].map(|k| (k, self.nodes[k as usize].bounds.distance_sq(p)));
                let (near, far) = if kids[0].1 <= kids[1].1 {
                    (kids[0], kids[1])
                } else {
                    (kids[1], kids[0])
                };
                stack.push(far);
                stack.push(near);
                continue;
            }
            for &face in &self.faces[node.at as usize..(node.at + node.count) as usize] {
                let [a, b, c] = mesh.tri_positions(face);
                let d = point_triangle_distance(p, a, b, c);
                if d * d < best {
                    best = d * d;
                    found = true;
                }
            }
        }
        found.then(|| best.sqrt())
    }
}

/// Whether `p` is inside the closed `mesh`: an odd number of its triangles
/// lie along a ray from `p` in a fixed slanted direction (slanted so it
/// does not run along the grid's planes), started a fifth of `nudge`
/// along it, off any surface `p` sits on.
pub fn point_inside_mesh_parity(mesh: &IndexedMesh, bvh: &Bvh, p: Vec3, nudge: f32) -> bool {
    let dir = Vec3::new(0.802, 0.457, 0.384);
    bvh.ray_hit_count(mesh, p + dir * (nudge * 0.2), dir) % 2 == 1
}

/// Where the ray `origin + t·dir` crosses the triangle `abc`, as `t ≥ 0`,
/// by its barycentric coordinates (Cramer's rule on the triangle's edges).
/// A ray in the triangle's plane misses.
pub fn ray_tri(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    crossing(origin, dir, a, b, c, 0.0, |det, _, _| det.abs() < 1e-8)
}

/// [`ray_tri`] with the triangle grown by `1e-4` of itself in barycentric
/// terms, so a ray through a shared edge or corner hits the triangles on
/// both sides instead of slipping between them, and tiny triangles count.
/// Edge-on is judged relative to the triangle's size.
pub fn ray_tri_closed(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    crossing(origin, dir, a, b, c, 1e-4, |det, e1, side| {
        det.abs() <= 1e-7 * e1.length() * side.length()
    })
}

fn crossing(
    origin: Vec3,
    dir: Vec3,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    grow: f32,
    edge_on: impl Fn(f32, Vec3, Vec3) -> bool,
) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let side = dir.cross(e2);
    let det = e1.dot(side);
    if edge_on(det, e1, side) {
        return None;
    }
    let to = origin - a;
    let u = to.dot(side) / det;
    if u < -grow || u > 1.0 + grow {
        return None;
    }
    let up = to.cross(e1);
    let v = dir.dot(up) / det;
    if v < -grow || u + v > 1.0 + grow {
        return None;
    }
    let t = e2.dot(up) / det;
    (t >= 0.0).then_some(t)
}

/// The distance from `p` to the triangle `abc`: to its plane when `p`'s
/// foot falls inside it, else to the nearest of its three sides.
pub fn point_triangle_distance(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let normal = (b - a).cross(c - a);
    let n2 = normal.dot(normal);
    if n2 > 0.0 {
        // The foot's barycentric weights: each the area of the triangle it
        // makes with the opposite side, over the whole.
        let foot = p - normal * ((p - a).dot(normal) / n2);
        let wa = (b - foot).cross(c - foot).dot(normal);
        let wb = (c - foot).cross(a - foot).dot(normal);
        let wc = (a - foot).cross(b - foot).dot(normal);
        if wa >= 0.0 && wb >= 0.0 && wc >= 0.0 {
            return (p - foot).length();
        }
    }
    segment_distance(p, a, b)
        .min(segment_distance(p, b, c))
        .min(segment_distance(p, c, a))
}

fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::icosphere;

    /// The tree is the same every time, and answers like asking every
    /// triangle.
    #[test]
    fn the_tree_answers_like_every_triangle_and_the_same_each_build() {
        let mesh = icosphere(10.0, 6);
        let tree = Bvh::build(&mesh);
        let again = Bvh::build(&mesh);
        assert_eq!(tree.faces, again.faces);
        let mut faces = tree.faces.clone();
        faces.sort_unstable();
        assert_eq!(faces, (0..mesh.triangles.len() as u32).collect::<Vec<_>>());
        for k in 0..40 {
            let f = k as f32;
            let p = Vec3::new((f * 0.7).sin() * 14.0, (f * 1.3).cos() * 9.0, f * 0.4 - 8.0);
            let brute = (0..mesh.triangles.len() as u32)
                .map(|t| {
                    let [a, b, c] = mesh.tri_positions(t);
                    point_triangle_distance(p, a, b, c)
                })
                .fold(f32::INFINITY, f32::min);
            assert_eq!(tree.nearest_distance(&mesh, p, 100.0), Some(brute));
            let dir = Vec3::new(0.893, 0.372, 0.254);
            let brute_hits = (0..mesh.triangles.len() as u32)
                .filter(|&t| {
                    let [a, b, c] = mesh.tri_positions(t);
                    ray_tri(p, dir, a, b, c).is_some()
                })
                .count() as u32;
            assert_eq!(tree.ray_hit_count(&mesh, p, dir), brute_hits);
        }
    }

    #[test]
    fn a_point_is_as_far_from_a_triangle_as_from_its_nearest_part() {
        let (a, b, c) = (
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(0.0, 4.0, 0.0),
        );
        let d = |p: Vec3| point_triangle_distance(p, a, b, c);
        assert_eq!(d(Vec3::new(1.0, 1.0, 3.0)), 3.0);
        assert_eq!(d(Vec3::new(-3.0, -4.0, 0.0)), 5.0);
        assert_eq!(d(Vec3::new(2.0, -2.0, 0.0)), 2.0);
        assert!((d(Vec3::new(3.0, 3.0, 0.0)) - 2.0f32.sqrt()).abs() < 1e-6);
    }
}

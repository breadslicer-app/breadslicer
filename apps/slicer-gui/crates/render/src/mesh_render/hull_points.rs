/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The few vertices of a mesh that decide its world box under any affine
//! transform: those on or right next to its convex hull.
//!
//! The box of an affine image of a point set is the box of the image of its
//! hull, so a dragged model's box can be computed from a few thousand points
//! instead of every vertex. The hull is built once per mesh on a worker thread;
//! until it is ready, and whenever it cannot vouch for a transform, the caller
//! walks every vertex as before.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock, Weak};

use glam::{DVec3, Mat4, Vec3};
use sc_mesh_core::Aabb;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::Vertex;

/// Meshes with fewer vertices than this are walked whole: a full walk costs a
/// few microseconds, less than looking after a hull.
const MIN_VERTICES: usize = 4096;

/// Bound on the rounding error of one coordinate of `Mat4::transform_point3`,
/// relative to the sum of the magnitudes of its 4 terms: a 4-term dot product
/// rounds by less than 5 units of `f32` rounding (2^-24); this is 8.
const TRANSFORM_ROUNDING: f64 = 4.0 * f32::EPSILON as f64;

/// The vertices of a mesh that lie on or near its convex hull, with the
/// guarantee that every other vertex sits at least `margin` inside their hull.
#[derive(Debug)]
pub(super) struct HullPoints {
    points: Vec<Vec3>,
    margin: f64,
    /// The largest magnitude of each coordinate over every vertex.
    reach: [f64; 3],
}

impl HullPoints {
    /// Returns the hull points of `vertices`, or `None` when the mesh is flat,
    /// has non-finite coordinates, or keeps so many points that a full walk is
    /// about as fast.
    pub(super) fn of(vertices: &[Vertex]) -> Option<Self> {
        let points: Vec<DVec3> = vertices
            .iter()
            .map(|v| DVec3::from(v.coords().map(f64::from)))
            .collect();
        if !points.iter().all(|p| p.is_finite()) {
            return None;
        }
        let mut reach = [0.0f64; 3];
        for p in &points {
            for (r, c) in reach.iter_mut().zip(p.to_array()) {
                *r = r.max(c.abs());
            }
        }
        let scale = reach[0].max(reach[1]).max(reach[2]);
        if scale <= 0.0 {
            return None;
        }
        let planes = convex_hull_planes(&points, scale)?;
        // Every vertex within `depth` of a hull plane is kept. With
        // millimetres, about 0.01 mm for a model a few centimetres across:
        // thin enough to drop almost every vertex of a sculpted model, and
        // wide enough to cover the `f32` rounding of a transform that moves
        // the model metres away (see `world_box`).
        let depth = 1e-5 * (scale + 1000.0);
        let kept = near_planes(&points, &planes, depth);
        if kept.len() * 2 > vertices.len() {
            return None;
        }
        Some(Self {
            points: kept
                .into_iter()
                .map(|i| Vec3::from(vertices[i as usize].coords()))
                .collect(),
            // Half the filter depth: the other half absorbs the `f64`
            // rounding of the planes and the hull's convexity tolerance,
            // both under a millionth of it.
            margin: depth / 2.0,
            reach,
        })
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.points.len()
    }

    /// Returns the box of every vertex of the mesh after `transform_mat`,
    /// computed from the hull points alone, or `None` when the `f32` rounding
    /// of this transform could let a dropped vertex win.
    ///
    /// A dropped vertex lies at least `margin` inside the hull of the kept
    /// ones, so along each output axis its exact coordinate trails the best
    /// kept one by at least `margin` times the length of that row of the
    /// matrix. When that gap exceeds twice the rounding of one transformed
    /// coordinate, the rounded coordinate of a dropped vertex can never beat
    /// the rounded coordinate of the best kept one, and the box equals the
    /// one a walk over every vertex returns.
    pub(super) fn world_box(&self, transform_mat: &Mat4) -> Option<Aabb> {
        for axis in 0..3 {
            // The linear part of this output coordinate, then its shift.
            let row = transform_mat.row(axis).as_dvec4();
            let linear = row.truncate();
            let magnitude = linear.abs().dot(DVec3::from(self.reach)) + row.w.abs();
            let rounding = TRANSFORM_ROUNDING * magnitude + f64::from(f32::MIN_POSITIVE);
            // False for a NaN in the matrix too.
            let vouches = self.margin * linear.length() > 2.0 * rounding;
            if !vouches {
                return None;
            }
        }
        let mut aabb = Aabb::empty();
        for p in &self.points {
            let q = transform_mat.transform_point3(*p);
            aabb.extend(Vertex::new([q.x, q.y, q.z]));
        }
        Some(aabb)
    }
}

/// The hull points of each large mesh the viewport draws, computed on a worker
/// thread the first time the mesh is seen.
///
/// An entry is keyed by the address of the mesh and holds a [`Weak`] to it.
/// While that `Weak` lives, the allocation cannot be reused by another mesh,
/// and the mesh cannot change in place: `Arc::get_mut` refuses and
/// `Arc::make_mut` moves the mesh to a new address. So a live entry always
/// describes the vertices the mesh has now.
#[derive(Debug, Default)]
pub(super) struct HullPointsCache {
    entries: HashMap<usize, CacheEntry>,
    jobs: Option<Sender<Job>>,
}

#[derive(Debug)]
struct CacheEntry {
    mesh: Weak<IndexedMesh>,
    hull: Arc<OnceLock<Option<HullPoints>>>,
}

#[derive(Debug)]
struct Job {
    mesh: Weak<IndexedMesh>,
    hull: Arc<OnceLock<Option<HullPoints>>>,
}

impl HullPointsCache {
    /// Returns the hull points of `mesh` once they are ready, and asks the
    /// worker for them the first time `mesh` is seen. `None` for a small mesh,
    /// a mesh whose hull is still being built, and a mesh that has none.
    pub(super) fn get(&mut self, mesh: &Arc<IndexedMesh>) -> Option<&HullPoints> {
        if mesh.vertices().len() < MIN_VERTICES {
            return None;
        }
        let entry = match self.entries.entry(Arc::as_ptr(mesh) as usize) {
            Entry::Occupied(slot) if slot.get().mesh.strong_count() > 0 => slot.into_mut(),
            Entry::Occupied(mut slot) => {
                slot.insert(Self::request(&mut self.jobs, mesh));
                slot.into_mut()
            }
            Entry::Vacant(slot) => slot.insert(Self::request(&mut self.jobs, mesh)),
        };
        entry.hull.get().and_then(Option::as_ref)
    }

    /// Drops the entries of meshes that no longer exist.
    pub(super) fn prune(&mut self) {
        self.entries.retain(|_, e| e.mesh.strong_count() > 0);
    }

    fn request(jobs: &mut Option<Sender<Job>>, mesh: &Arc<IndexedMesh>) -> CacheEntry {
        let entry = CacheEntry {
            mesh: Arc::downgrade(mesh),
            hull: Arc::new(OnceLock::new()),
        };
        let job = Job {
            mesh: entry.mesh.clone(),
            hull: entry.hull.clone(),
        };
        let sent = jobs
            .get_or_insert_with(spawn_worker)
            .send(job)
            .map_err(|failed| failed.0);
        if let Err(job) = sent {
            // No worker: this mesh keeps the full walk.
            let _ = job.hull.set(None);
        }
        entry
    }
}

/// Starts the thread that builds hull points, one mesh at a time. It holds a
/// mesh only while it works on it, and ends when the cache is dropped.
fn spawn_worker() -> Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new()
        .name("viewport-hull".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                let hull = job
                    .mesh
                    .upgrade()
                    .and_then(|mesh| HullPoints::of(mesh.vertices()));
                let _ = job.hull.set(hull);
            }
        });
    if let Err(err) = spawned {
        // The receiver is gone with the closure, so every request reports
        // "no hull" and the viewport walks every vertex as before.
        eprintln!("Failed to spawn the viewport hull worker: {err}");
    }
    tx
}

/// A hull facet: outward unit normal `n`, offset `c` (points with
/// `n·p > c` are outside), vertices counter-clockwise from outside, and the
/// facet across each edge `(v[k], v[k + 1])`.
#[derive(Debug)]
struct Facet {
    v: [u32; 3],
    n: DVec3,
    c: f64,
    across: [u32; 3],
    outside: Vec<u32>,
    alive: bool,
}

impl Facet {
    fn new(points: &[DVec3], v: [u32; 3]) -> Option<Self> {
        let [a, b, c] = v.map(|i| points[i as usize]);
        let n = (b - a).cross(c - a);
        let length = n.length();
        if !(length > 0.0 && length.is_finite()) {
            return None;
        }
        let n = n / length;
        Some(Self {
            v,
            n,
            c: (n.dot(a) + n.dot(b) + n.dot(c)) / 3.0,
            across: [u32::MAX; 3],
            outside: Vec::new(),
            alive: true,
        })
    }

    fn height(&self, p: DVec3) -> f64 {
        self.n.dot(p) - self.c
    }

    fn edge(&self, k: usize) -> (u32, u32) {
        (self.v[k], self.v[(k + 1) % 3])
    }
}

/// Builds the convex hull of `points` with Quickhull (Barber, Dobkin and
/// Huhdanpaa, 1996) and returns its facet planes, or `None` for a flat or
/// degenerate set, or a hull that fails its closing checks (closed, sphere-like
/// and convex). `scale` is the largest coordinate magnitude.
fn convex_hull_planes(points: &[DVec3], scale: f64) -> Option<Vec<(DVec3, f64)>> {
    // A point counts as outside a facet when it is more than `eps` above it.
    let eps = 1e-10 * scale;
    let mut facets = initial_simplex(points, eps)?;
    for i in 0..points.len() as u32 {
        let p = points[i as usize];
        if let Some(f) = facets.iter_mut().find(|f| f.height(p) > eps) {
            f.outside.push(i);
        }
    }
    let mut pending: Vec<u32> = (0..4).collect();
    let mut visible_mark = vec![0u32; 4];
    let mut hidden_mark = vec![0u32; 4];
    let mut round = 0u32;
    let mut eyes = 0usize;
    while let Some(f) = pending.pop() {
        let facet = &facets[f as usize];
        if !facet.alive || facet.outside.is_empty() {
            continue;
        }
        // A hull with this many vertices keeps too many points to pay off.
        eyes += 1;
        if eyes * 2 > points.len() {
            return None;
        }
        let eye_index = *facet.outside.iter().max_by(|&&a, &&b| {
            facet
                .height(points[a as usize])
                .total_cmp(&facet.height(points[b as usize]))
        })?;
        let eye = points[eye_index as usize];

        // The facets the eye sees, and the horizon around them: each edge of
        // a visible facet whose neighbour is hidden, as oriented in the
        // visible one.
        round += 1;
        let mut visible = Vec::new();
        let mut horizon: Vec<(u32, u32, u32)> = Vec::new();
        let mut stack = vec![f];
        visible_mark[f as usize] = round;
        while let Some(h) = stack.pop() {
            visible.push(h);
            for k in 0..3 {
                let g = facets[h as usize].across[k];
                if visible_mark[g as usize] == round {
                    continue;
                }
                if hidden_mark[g as usize] != round && facets[g as usize].height(eye) > eps {
                    visible_mark[g as usize] = round;
                    stack.push(g);
                } else {
                    hidden_mark[g as usize] = round;
                    let (a, b) = facets[h as usize].edge(k);
                    horizon.push((a, b, g));
                }
            }
        }

        // The horizon must be one simple loop, or the new cone would not
        // close the surface.
        let starts: HashMap<u32, usize> = horizon
            .iter()
            .enumerate()
            .map(|(i, &(a, _, _))| (a, i))
            .collect();
        if starts.len() != horizon.len() || !is_single_loop(&horizon, &starts) {
            return None;
        }

        // Cone the horizon to the eye.
        let first = facets.len() as u32;
        for &(a, b, hidden) in &horizon {
            let new = facets.len() as u32;
            let mut facet = Facet::new(points, [a, b, eye_index])?;
            facet.across[0] = hidden;
            let back = &mut facets[hidden as usize];
            let k = (0..3).find(|&k| back.edge(k) == (b, a))?;
            back.across[k] = new;
            facets.push(facet);
        }
        // In a single loop, the edge that ends at a is the one before the edge
        // that starts at a.
        let mut ends = vec![0u32; horizon.len()];
        for (i, &(_, b, _)) in horizon.iter().enumerate() {
            ends[starts[&b]] = first + i as u32;
        }
        for (i, &(_, b, _)) in horizon.iter().enumerate() {
            let new = &mut facets[first as usize + i];
            // Across (b, eye) is the cone facet that starts at b; across
            // (eye, a) is the one that ends at a.
            new.across[1] = first + starts[&b] as u32;
            new.across[2] = ends[i];
        }
        visible_mark.resize(facets.len(), 0);
        hidden_mark.resize(facets.len(), 0);

        // Hand the visible facets' outside points to the cone.
        for &h in &visible {
            let orphans = std::mem::take(&mut facets[h as usize].outside);
            facets[h as usize].alive = false;
            for i in orphans {
                if i == eye_index {
                    continue;
                }
                let p = points[i as usize];
                if let Some(new) = facets[first as usize..]
                    .iter_mut()
                    .find(|f| f.height(p) > eps)
                {
                    new.outside.push(i);
                }
            }
        }
        pending.extend(
            (first..facets.len() as u32).filter(|&n| !facets[n as usize].outside.is_empty()),
        );
    }
    check_hull(points, &facets, scale)?;
    Some(
        facets
            .iter()
            .filter(|f| f.alive)
            .map(|f| (f.n, f.c))
            .collect(),
    )
}

/// Whether following each horizon edge to the one that starts where it ends
/// visits every edge before coming back.
fn is_single_loop(horizon: &[(u32, u32, u32)], starts: &HashMap<u32, usize>) -> bool {
    let mut at = 0;
    for step in 1..=horizon.len() {
        let Some(&next) = starts.get(&horizon[at].1) else {
            return false;
        };
        at = next;
        if at == 0 {
            return step == horizon.len();
        }
    }
    false
}

/// The starting tetrahedron: the two farthest apart of the 6 axis extremes,
/// the point farthest from their line, and the point farthest from the plane
/// of those three. Facets are oriented outward and linked.
fn initial_simplex(points: &[DVec3], eps: f64) -> Option<Vec<Facet>> {
    let mut extremes = [0usize; 6];
    for (i, p) in points.iter().enumerate() {
        for axis in 0..3 {
            if p[axis] < points[extremes[2 * axis]][axis] {
                extremes[2 * axis] = i;
            }
            if p[axis] > points[extremes[2 * axis + 1]][axis] {
                extremes[2 * axis + 1] = i;
            }
        }
    }
    let mut pair = (extremes[0], extremes[1]);
    for &i in &extremes {
        for &j in &extremes {
            if points[i].distance_squared(points[j])
                > points[pair.0].distance_squared(points[pair.1])
            {
                pair = (i, j);
            }
        }
    }
    let (a, b) = pair;
    let along = (points[b] - points[a]).normalize_or_zero();
    let off_line = |p: DVec3| {
        let d = p - points[a];
        (d - along * d.dot(along)).length()
    };
    let c =
        (0..points.len()).max_by(|&i, &j| off_line(points[i]).total_cmp(&off_line(points[j])))?;
    if off_line(points[c]) <= eps {
        return None;
    }
    let normal = (points[b] - points[a])
        .cross(points[c] - points[a])
        .normalize_or_zero();
    let off_plane = |p: DVec3| normal.dot(p - points[a]).abs();
    let d =
        (0..points.len()).max_by(|&i, &j| off_plane(points[i]).total_cmp(&off_plane(points[j])))?;
    if off_plane(points[d]) <= eps {
        return None;
    }
    let [a, b, c, d] = [a, b, c, d].map(|i| i as u32);
    let mut facets = Vec::with_capacity(4);
    for (tri, apex) in [
        ([a, b, c], d),
        ([a, b, d], c),
        ([a, c, d], b),
        ([b, c, d], a),
    ] {
        let mut facet = Facet::new(points, tri)?;
        if facet.height(points[apex as usize]) > 0.0 {
            facet = Facet::new(points, [tri[0], tri[2], tri[1]])?;
        }
        facets.push(facet);
    }
    for f in 0..4 {
        for k in 0..3 {
            let (p, q) = facets[f].edge(k);
            let g = (0..4).find(|&g| (0..3).any(|m| facets[g].edge(m) == (q, p)))?;
            facets[f].across[k] = g as u32;
        }
    }
    Some(facets)
}

/// Checks that the live facets close a surface around the hull: each edge is
/// shared by exactly 2 facets that point at each other, the surface is a
/// sphere (V - E + F = 2) and it bends outward at every edge.
fn check_hull(points: &[DVec3], facets: &[Facet], scale: f64) -> Option<()> {
    let tolerance = 1e-8 * scale;
    let mut live = 0usize;
    let mut corners = Vec::new();
    for (f, facet) in facets.iter().enumerate().filter(|(_, f)| f.alive) {
        live += 1;
        corners.extend(facet.v);
        for k in 0..3 {
            let g = facets.get(facet.across[k] as usize)?;
            let (a, b) = facet.edge(k);
            let m = (0..3).find(|&m| g.edge(m) == (b, a))?;
            if !g.alive || g.across[m] as usize != f {
                return None;
            }
            let opposite = points[g.v[(m + 2) % 3] as usize];
            if facet.height(opposite) > tolerance {
                return None;
            }
        }
    }
    corners.sort_unstable();
    corners.dedup();
    (corners.len() + live == live * 3 / 2 + 2).then_some(())
}

/// Returns, in increasing order, the indices of the points within `depth` of
/// some plane or outside it. The search splits the points in halves and drops
/// every plane that is farther than `depth` from a half's box, so the deep
/// inside of a mesh is discarded without testing its points one by one.
fn near_planes(points: &[DVec3], planes: &[(DVec3, f64)], depth: f64) -> Vec<u32> {
    let mut order: Vec<u32> = (0..points.len() as u32).collect();
    let all: Vec<u32> = (0..planes.len() as u32).collect();
    let mut kept = Vec::new();
    keep_near(points, planes, depth, &mut order, &all, &mut kept);
    kept.sort_unstable();
    kept
}

fn keep_near(
    points: &[DVec3],
    planes: &[(DVec3, f64)],
    depth: f64,
    order: &mut [u32],
    candidates: &[u32],
    kept: &mut Vec<u32>,
) {
    let (mut lo, mut hi) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
    for &i in order.iter() {
        lo = lo.min(points[i as usize]);
        hi = hi.max(points[i as usize]);
    }
    let centre = (lo + hi) / 2.0;
    let half = (hi - lo) / 2.0;
    // The lowest depth below plane `(n, c)` over the box is reached at a
    // corner: c - n·centre - |n|·half.
    let near: Vec<u32> = candidates
        .iter()
        .copied()
        .filter(|&f| {
            let (n, c) = planes[f as usize];
            c - n.dot(centre) - n.abs().dot(half) < depth
        })
        .collect();
    if near.is_empty() {
        return;
    }
    if order.len() <= 32 {
        kept.extend(order.iter().copied().filter(|&i| {
            let p = points[i as usize];
            near.iter().any(|&f| {
                let (n, c) = planes[f as usize];
                c - n.dot(p) < depth
            })
        }));
        return;
    }
    let extent = hi - lo;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let mid = order.len() / 2;
    order.select_nth_unstable_by(mid, |&a, &b| {
        points[a as usize][axis].total_cmp(&points[b as usize][axis])
    });
    let (left, right) = order.split_at_mut(mid);
    keep_near(points, planes, depth, left, &near, kept);
    keep_near(points, planes, depth, right, &near, kept);
}

/// A closed, lumpy, sculpture-like mesh about 40 mm across, with a flat base
/// at the bottom and `rings * segments` vertices, most of them off its hull.
#[cfg(test)]
pub(super) fn lumpy_mesh(rings: u32, segments: u32) -> IndexedMesh {
    use sc_mesh_core::primitives::IndexedTriangle;
    let mut vertices = Vec::new();
    for i in 0..rings {
        let theta = std::f64::consts::PI * f64::from(i) / f64::from(rings - 1);
        for j in 0..segments {
            let phi = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
            let r = 20.0
                * (1.0
                    + 0.25 * (5.0 * theta).sin() * (3.0 * phi).cos()
                    + 0.05 * (17.0 * theta + 11.0 * phi).sin());
            let z = (r * theta.cos()).max(-12.0);
            vertices.push(Vertex::new([
                (r * theta.sin() * phi.cos()) as f32,
                (r * theta.sin() * phi.sin()) as f32 + 3.0,
                z as f32 + 12.0,
            ]));
        }
    }
    let mut faces = Vec::new();
    for i in 0..rings - 1 {
        for j in 0..segments {
            let a = i * segments + j;
            let b = i * segments + (j + 1) % segments;
            let (c, d) = (a + segments, b + segments);
            faces.push(IndexedTriangle::from_raw_indices([a, c, b]));
            faces.push(IndexedTriangle::from_raw_indices([b, c, d]));
        }
    }
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

#[cfg(test)]
mod tests {
    use glam::Quat;

    use super::*;

    /// The box of every vertex, the way the viewport walks them.
    fn full_box(vertices: &[Vertex], m: &Mat4) -> Aabb {
        let mut aabb = Aabb::empty();
        for v in vertices {
            let p = m.transform_point3(Vec3::from(v.coords()));
            aabb.extend(Vertex::new([p.x, p.y, p.z]));
        }
        aabb
    }

    /// A small deterministic generator, so a failure replays.
    struct Rng(u64);

    impl Rng {
        fn unit(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.unit()
        }

        /// A rotated, non-uniformly scaled and moved matrix, the shapes the
        /// gizmo and the modal tools produce.
        fn transform(&mut self, max_scale: f32, max_shift: f32) -> Mat4 {
            let axis = Vec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            )
            .normalize_or(Vec3::Z);
            let scale = Vec3::new(
                self.range(0.1, max_scale),
                self.range(0.1, max_scale),
                self.range(0.1, max_scale),
            );
            let shift = Vec3::new(
                self.range(-max_shift, max_shift),
                self.range(-max_shift, max_shift),
                self.range(-max_shift, max_shift),
            );
            Mat4::from_scale_rotation_translation(
                scale,
                Quat::from_axis_angle(axis, self.range(0.0, 7.0)),
                shift,
            )
        }
    }

    #[test]
    fn hull_points_keep_the_flat_base_and_little_else() {
        let mesh = lumpy_mesh(160, 200);
        let hull = HullPoints::of(&mesh.vertices).expect("a hull");
        // The base vertices lie on a hull face, so they all stay.
        let base = mesh.vertices.iter().filter(|v| v[2] == 0.0).count();
        assert!(
            hull.len() < base + mesh.vertices.len() / 20,
            "{} of {} vertices kept, {base} on the base",
            hull.len(),
            mesh.vertices.len()
        );
    }

    #[test]
    fn hull_box_equals_the_full_walk_for_any_transform_it_accepts() {
        let mesh = lumpy_mesh(160, 200);
        let hull = HullPoints::of(&mesh.vertices).expect("a hull");
        let mut rng = Rng(0x2920_5eed);
        let mut accepted = 0;
        for round in 0..400 {
            // Mostly plate-sized moves, with a tail of extreme ones.
            let m = if round % 10 == 9 {
                rng.transform(50.0, 1.0e6)
            } else {
                rng.transform(5.0, 400.0)
            };
            if let Some(aabb) = hull.world_box(&m) {
                accepted += 1;
                assert_eq!(aabb, full_box(&mesh.vertices, &m), "round {round}: {m:?}");
            }
        }
        assert!(
            accepted >= 360,
            "only {accepted} of 400 transforms accepted"
        );
    }

    #[test]
    fn hull_box_refuses_a_matrix_it_cannot_vouch_for() {
        let mesh = lumpy_mesh(160, 200);
        let hull = HullPoints::of(&mesh.vertices).expect("a hull");
        let tiny_far = Mat4::from_scale_rotation_translation(
            Vec3::splat(1e-4),
            Quat::IDENTITY,
            Vec3::new(1e5, 0.0, 0.0),
        );
        assert_eq!(hull.world_box(&tiny_far), None);
        let flattened = Mat4::from_scale(Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(hull.world_box(&flattened), None);
        let mut broken = Mat4::IDENTITY;
        broken.x_axis.x = f32::NAN;
        assert_eq!(hull.world_box(&broken), None);
    }

    #[test]
    fn hull_contains_every_point_of_a_cloud_with_coplanar_and_repeated_points() {
        let mut rng = Rng(7);
        let mut points = Vec::new();
        for _ in 0..20_000 {
            points.push(DVec3::new(
                f64::from(rng.range(-10.0, 10.0)),
                f64::from(rng.range(-10.0, 10.0)),
                f64::from(rng.range(-10.0, 10.0)),
            ));
        }
        // A grid on the top face, coplanar with each other and with the
        // hull, and the corners of the box, each twice.
        for i in 0..40 {
            for j in 0..40 {
                points.push(DVec3::new(
                    f64::from(i) / 2.0 - 10.0,
                    f64::from(j) / 2.0 - 10.0,
                    11.0,
                ));
            }
        }
        for corner in 0..16 {
            let c = corner % 8;
            points.push(DVec3::new(
                if c & 1 == 0 { -11.0 } else { 11.0 },
                if c & 2 == 0 { -11.0 } else { 11.0 },
                if c & 4 == 0 { -11.0 } else { 11.0 },
            ));
        }
        let planes = convex_hull_planes(&points, 11.0).expect("a hull");
        for p in &points {
            for (n, c) in &planes {
                assert!(n.dot(*p) - c <= 1e-9, "{p} outside a facet");
            }
        }
        // The hull is the box of the corners: every plane is a box face.
        for (n, c) in &planes {
            assert!((n.abs().max_element() - 1.0).abs() < 1e-12, "{n}");
            assert!((c - 11.0).abs() < 1e-9, "{c}");
        }
    }

    #[test]
    fn flat_or_all_hull_meshes_have_no_hull_points() {
        let flat: Vec<Vertex> = (0..5000)
            .map(|i| Vertex::new([(i % 70) as f32, (i / 70) as f32, 0.0]))
            .collect();
        assert!(HullPoints::of(&flat).is_none(), "flat");
        let line: Vec<Vertex> = (0..5000)
            .map(|i| Vertex::new([i as f32, 0.0, 0.0]))
            .collect();
        assert!(HullPoints::of(&line).is_none(), "line");
        // Every vertex of a sphere is on its hull: a walk is as fast.
        let sphere: Vec<Vertex> = lumpy_mesh(80, 80)
            .vertices
            .iter()
            .map(|v| {
                let p = Vec3::from(v.coords()) - Vec3::new(0.0, 3.0, 12.0);
                Vertex::new(p.normalize().to_array())
            })
            .collect();
        assert!(HullPoints::of(&sphere).is_none(), "sphere");
        let mut broken = lumpy_mesh(80, 80).vertices;
        broken[17] = Vertex::new([f32::NAN, 0.0, 0.0]);
        assert!(HullPoints::of(&broken).is_none(), "NaN");
    }

    fn wait_for_hull(cache: &mut HullPointsCache, mesh: &Arc<IndexedMesh>) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(30) {
            if cache.get(mesh).is_some() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn cache_builds_hull_points_for_large_meshes_only() {
        let mut cache = HullPointsCache::default();
        let small = Arc::new(lumpy_mesh(20, 20));
        assert!(cache.get(&small).is_none());
        assert!(cache.entries.is_empty(), "a small mesh is never queued");
        let large = Arc::new(lumpy_mesh(100, 100));
        assert!(wait_for_hull(&mut cache, &large));
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn cache_never_serves_the_hull_of_a_mesh_changed_in_place() {
        let mut cache = HullPointsCache::default();
        let mut mesh = Arc::new(lumpy_mesh(100, 100));
        assert!(wait_for_hull(&mut cache, &mesh));
        let before = Arc::as_ptr(&mesh);
        // The only strong reference: `make_mut` would edit in place, but the
        // `Weak` in the cache makes it move the mesh instead.
        Arc::make_mut(&mut mesh).vertices[0] = Vertex::new([500.0, 0.0, 0.0]);
        assert_ne!(Arc::as_ptr(&mesh), before);
        assert!(wait_for_hull(&mut cache, &mesh));
        let m = Mat4::IDENTITY;
        assert_eq!(
            cache.get(&mesh).and_then(|h| h.world_box(&m)),
            Some(full_box(&mesh.vertices, &m))
        );
        cache.prune();
        assert_eq!(cache.entries.len(), 1, "the old mesh is gone");
    }
}

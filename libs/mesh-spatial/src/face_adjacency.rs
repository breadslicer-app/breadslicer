/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Which faces of a mesh share an edge, and the walks a surface brush makes
//! over them (#183).
//!
//! A brush's footprint follows the surface from the face under the cursor
//! rather than a sphere around it, so it never jumps through a thin wall to the
//! other side. [`FaceAdjacency`] is the map it walks: compressed, built once per
//! mesh and kept beside the picking clusters. It takes any mesh: every face on
//! an edge is a neighbour of the others on it, however many there are.

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;

/// Above this many faces on one edge, each is linked to the next only: a fan
/// of thousands on one edge stays linear.
const FULL_LINK_FACES: usize = 8;

type P = [f32; 3];

/// The neighbours of each face, compressed:
/// `faces[start[f]..start[f + 1]]` share an edge with face `f`.
#[derive(Debug, Default)]
pub struct FaceAdjacency {
    start: Vec<u32>,
    faces: Vec<u32>,
}

impl FaceAdjacency {
    /// The adjacency of `mesh`.
    pub fn new(mesh: &IndexedMesh) -> Self {
        let count = mesh.faces().len();
        let mut edges: Vec<(u64, u32)> = Vec::with_capacity(count * 3);
        for (face, f) in mesh.faces().iter().enumerate() {
            let v = f.vertices.map(|v| v.get());
            for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
                if a != b {
                    let key = (u64::from(a.min(b)) << 32) | u64::from(a.max(b));
                    edges.push((key, face as u32));
                }
            }
        }
        edges.par_sort_unstable();
        let mut start = vec![0u32; count + 1];
        let links = |group: &[(u64, u32)], link: &mut dyn FnMut(u32, u32)| {
            if group.len() <= FULL_LINK_FACES {
                for (i, a) in group.iter().enumerate() {
                    for b in &group[i + 1..] {
                        link(a.1, b.1);
                    }
                }
            } else {
                for pair in group.windows(2) {
                    link(pair[0].1, pair[1].1);
                }
            }
        };
        for group in edges.chunk_by(|a, b| a.0 == b.0) {
            links(group, &mut |a, b| {
                start[a as usize + 1] += 1;
                start[b as usize + 1] += 1;
            });
        }
        for f in 0..count {
            start[f + 1] += start[f];
        }
        let mut next = start.clone();
        let mut faces = vec![0u32; *start.last().unwrap_or(&0) as usize];
        for group in edges.chunk_by(|a, b| a.0 == b.0) {
            links(group, &mut |a, b| {
                faces[next[a as usize] as usize] = b;
                next[a as usize] += 1;
                faces[next[b as usize] as usize] = a;
                next[b as usize] += 1;
            });
        }
        Self { start, faces }
    }

    /// The faces that share an edge with `face`.
    pub fn of(&self, face: u32) -> &[u32] {
        let f = face as usize;
        match (self.start.get(f), self.start.get(f + 1)) {
            (Some(&a), Some(&b)) => &self.faces[a as usize..b as usize],
            _ => &[],
        }
    }

    /// How many faces it knows.
    pub fn len(&self) -> usize {
        self.start.len().saturating_sub(1)
    }

    /// Whether it knows no face.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Marks of a walk over the faces, reused between walks: starting one clears
/// nothing, it only moves a counter.
#[derive(Debug, Default)]
pub struct WalkMarks {
    seen: Vec<u32>,
    walk: u32,
}

impl WalkMarks {
    fn begin(&mut self, faces: usize) {
        if self.seen.len() != faces || self.walk == u32::MAX {
            self.seen.clear();
            self.seen.resize(faces, 0);
            self.walk = 0;
        }
        self.walk += 1;
    }

    /// Marks `face`; whether it was unmarked in this walk.
    fn first_visit(&mut self, face: u32) -> bool {
        let slot = &mut self.seen[face as usize];
        let first = *slot != self.walk;
        *slot = self.walk;
        first
    }
}

/// Which way the surface must look for a brush to touch it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Facing {
    /// Both sides.
    Any,
    /// Towards this point of the mesh's frame: a perspective camera's eye.
    Towards(P),
    /// Against this direction of the mesh's frame: an orthographic camera's
    /// view.
    Against(P),
}

impl Facing {
    /// Whether a face at `at` with the (unnormalised) normal `n` looks the
    /// right way. `flip` turns the winding round, for a mirrored object.
    fn sees(&self, at: P, n: P, flip: bool) -> bool {
        let d = match *self {
            Self::Any => return true,
            Self::Towards(eye) => dot(sub(eye, at), n),
            Self::Against(view) => -dot(view, n),
        };
        (d > 0.0) != flip
    }
}

/// A brush dab, in the mesh's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    /// The point the brush touches.
    pub center: P,
    /// Its reach, in real millimetres.
    pub radius_mm: f32,
    /// What squared lengths in the mesh's frame are in real mm²: the object's
    /// `LᵀL`, for `L` the linear part of its world transform.
    pub metric: [[f32; 3]; 3],
    /// Which way a face must look for the brush to touch it.
    pub facing: Facing,
    /// The object's world transform mirrors it.
    pub mirrored: bool,
}

impl Dab {
    /// The metric of an object at `linear`, its world transform's linear part.
    pub fn metric_of(linear: &[[f64; 3]; 3]) -> [[f32; 3]; 3] {
        std::array::from_fn(|i| {
            std::array::from_fn(|j| (0..3).map(|k| linear[k][i] * linear[k][j]).sum::<f64>() as f32)
        })
    }

    /// The squared real distance from the brush's centre to `p`.
    fn distance_sq(&self, p: P) -> f32 {
        let d = sub(p, self.center);
        let m = &self.metric;
        d[0] * (m[0][0] * d[0] + m[0][1] * d[1] + m[0][2] * d[2])
            + d[1] * (m[1][0] * d[0] + m[1][1] * d[1] + m[1][2] * d[2])
            + d[2] * (m[2][0] * d[0] + m[2][1] * d[1] + m[2][2] * d[2])
    }
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: P, b: P) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn corners(mesh: &IndexedMesh, face: u32) -> Option<[P; 3]> {
    let vertices = mesh.vertices();
    let f = mesh.faces().get(face as usize)?;
    Some([
        vertices.get(usize::from(f.vertices[0]))?.coords(),
        vertices.get(usize::from(f.vertices[1]))?.coords(),
        vertices.get(usize::from(f.vertices[2]))?.coords(),
    ])
}

/// The faces a brush touches, into `out`: a walk over the surface from `seed`
/// (always among them) that goes on through every face with a corner in
/// reach and takes those whose centroid is. A face that does not look the
/// way `dab` asks stops the walk.
pub fn dab_faces(
    mesh: &IndexedMesh,
    adjacency: &FaceAdjacency,
    seed: u32,
    dab: &Dab,
    marks: &mut WalkMarks,
    out: &mut Vec<u32>,
) {
    out.clear();
    if seed as usize >= mesh.faces().len() {
        return;
    }
    marks.begin(mesh.faces().len());
    let radius_sq = dab.radius_mm * dab.radius_mm;
    let mut stack: Vec<u32> = vec![seed];
    marks.first_visit(seed);
    while let Some(face) = stack.pop() {
        let Some(t) = corners(mesh, face) else {
            continue;
        };
        let at = [
            (t[0][0] + t[1][0] + t[2][0]) / 3.0,
            (t[0][1] + t[1][1] + t[2][1]) / 3.0,
            (t[0][2] + t[1][2] + t[2][2]) / 3.0,
        ];
        let is_seed = face == seed;
        if !is_seed {
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            if !dab.facing.sees(at, n, dab.mirrored) {
                continue;
            }
        }
        if is_seed || dab.distance_sq(at) <= radius_sq {
            out.push(face);
        }
        if is_seed || t.iter().any(|&p| dab.distance_sq(p) <= radius_sq) {
            for &next in adjacency.of(face) {
                if marks.first_visit(next) {
                    stack.push(next);
                }
            }
        }
    }
}

/// The connected patch around `seed` whose normals lie within `max_angle_deg`
/// of the normal of `seed`, into `out`.
pub fn fill_faces(
    mesh: &IndexedMesh,
    adjacency: &FaceAdjacency,
    seed: u32,
    max_angle_deg: f32,
    marks: &mut WalkMarks,
    out: &mut Vec<u32>,
) {
    out.clear();
    let normal = |face: u32| -> Option<P> {
        let t = corners(mesh, face)?;
        let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let len = dot(n, n).sqrt();
        (len > 0.0).then(|| [n[0] / len, n[1] / len, n[2] / len])
    };
    let Some(reference) = normal(seed) else {
        return;
    };
    marks.begin(mesh.faces().len());
    let cos = max_angle_deg.to_radians().cos();
    let mut stack = vec![seed];
    marks.first_visit(seed);
    while let Some(face) = stack.pop() {
        if normal(face).is_none_or(|n| dot(n, reference) < cos) {
            continue;
        }
        out.push(face);
        for &next in adjacency.of(face) {
            if marks.first_visit(next) {
                stack.push(next);
            }
        }
    }
}

/// The faces one ring out from the painted ones, when `grow`; the painted
/// faces of the ring just inside the edge of the paint, when not. `painted`
/// tells which faces are.
pub fn ring_faces(
    adjacency: &FaceAdjacency,
    painted: impl Fn(u32) -> bool + Sync,
    grow: bool,
) -> Vec<u32> {
    (0..adjacency.len() as u32)
        .into_par_iter()
        .with_min_len(1 << 12)
        .filter(|&face| {
            painted(face) != grow && adjacency.of(face).iter().any(|&n| painted(n) == grow)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

    use super::*;

    const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    /// A flat sheet of `n` × `n` quads, 1 mm each, at height `z`, looking up.
    fn sheet(n: u32, z: f32) -> IndexedMesh {
        let mut vertices = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                vertices.push(Vertex::new([x as f32, y as f32, z]));
            }
        }
        let at = |x: u32, y: u32| y * (n + 1) + x;
        let mut faces = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
                faces.push(IndexedTriangle::from_raw_indices([a, b, c]));
                faces.push(IndexedTriangle::from_raw_indices([a, c, d]));
            }
        }
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn dab(center: P, radius_mm: f32) -> Dab {
        Dab {
            center,
            radius_mm,
            metric: IDENTITY,
            facing: Facing::Any,
            mirrored: false,
        }
    }

    #[test]
    fn neighbours_are_the_faces_across_each_edge() {
        let mesh = sheet(1, 0.0);
        let adjacency = FaceAdjacency::new(&mesh);
        assert_eq!(adjacency.len(), 2);
        assert_eq!(adjacency.of(0), [1]);
        assert_eq!(adjacency.of(1), [0]);
        assert!(adjacency.of(9).is_empty());
    }

    #[test]
    fn three_faces_on_one_edge_are_all_neighbours() {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, -1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![
                IndexedTriangle::from_raw_indices([0, 1, 2]),
                IndexedTriangle::from_raw_indices([1, 0, 3]),
                IndexedTriangle::from_raw_indices([0, 1, 4]),
            ],
        };
        let adjacency = FaceAdjacency::new(&mesh);
        for face in 0..3 {
            assert_eq!(adjacency.of(face).len(), 2);
        }
    }

    #[test]
    fn a_fan_of_thousands_on_one_edge_stays_linear() {
        let mut vertices = vec![Vertex::new([0.0, 0.0, 0.0]), Vertex::new([1.0, 0.0, 0.0])];
        let mut faces = Vec::new();
        for i in 0..5000u32 {
            vertices.push(Vertex::new([0.5, i as f32, 1.0]));
            faces.push(IndexedTriangle::from_raw_indices([0, 1, i + 2]));
        }
        let adjacency = FaceAdjacency::new(&IndexedMesh {
            meta: None,
            vertices,
            faces,
        });
        assert!((0..5000).all(|f| adjacency.of(f).len() <= 2));
        assert!(adjacency.of(2500).len() == 2);
    }

    #[test]
    fn a_dab_takes_the_faces_in_reach_and_the_face_under_it() {
        let mesh = sheet(20, 0.0);
        let adjacency = FaceAdjacency::new(&mesh);
        let mut marks = WalkMarks::default();
        let mut out = Vec::new();
        let center = [10.5, 10.5, 0.0];
        let seed = (10 * 20 + 10) * 2;
        dab_faces(
            &mesh,
            &adjacency,
            seed,
            &dab(center, 3.0),
            &mut marks,
            &mut out,
        );
        assert!(out.contains(&seed));
        assert!(out.len() > 20, "{}", out.len());
        for &face in &out {
            let t = corners(&mesh, face).unwrap();
            let c = [
                (t[0][0] + t[1][0] + t[2][0]) / 3.0,
                (t[0][1] + t[1][1] + t[2][1]) / 3.0,
                0.0,
            ];
            let d = sub(c, center);
            assert!(dot(d, d).sqrt() <= 3.01);
        }
        // A tiny dab still paints what it touches.
        dab_faces(
            &mesh,
            &adjacency,
            seed,
            &dab(center, 0.01),
            &mut marks,
            &mut out,
        );
        assert_eq!(out, [seed]);
    }

    #[test]
    fn a_dab_does_not_jump_through_a_thin_wall() {
        // Two sheets 0.1 mm apart, joined nowhere: only one is walked.
        let top = sheet(8, 0.1);
        let bottom = sheet(8, 0.0);
        let mut mesh = top.clone();
        let offset = mesh.vertices.len() as u32;
        mesh.vertices.extend(bottom.vertices.iter().copied());
        mesh.faces.extend(
            bottom
                .faces
                .iter()
                .map(|f| IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + offset))),
        );
        let adjacency = FaceAdjacency::new(&mesh);
        let mut marks = WalkMarks::default();
        let mut out = Vec::new();
        dab_faces(
            &mesh,
            &adjacency,
            (4 * 8 + 4) * 2,
            &dab([4.5, 4.5, 0.1], 2.0),
            &mut marks,
            &mut out,
        );
        assert!(!out.is_empty());
        assert!(out.iter().all(|&f| (f as usize) < top.faces.len()));
    }

    #[test]
    fn a_dab_stops_at_faces_that_look_away() {
        // A sheet with its far half folded down and away: seen from above, the
        // brush keeps to the near half.
        let mut mesh = sheet(8, 0.0);
        for v in &mut mesh.vertices {
            let p = v.coords();
            if p[0] > 4.0 {
                *v = Vertex::new([4.0 - (p[0] - 4.0), p[1], -(p[0] - 4.0) * 0.1]);
            }
        }
        let adjacency = FaceAdjacency::new(&mesh);
        let mut marks = WalkMarks::default();
        let mut out = Vec::new();
        let seen = Dab {
            facing: Facing::Against([0.0, 0.0, -1.0]),
            ..dab([3.5, 4.5, 0.0], 3.0)
        };
        dab_faces(
            &mesh,
            &adjacency,
            (4 * 8 + 3) * 2,
            &seen,
            &mut marks,
            &mut out,
        );
        assert!(!out.is_empty());
        for &face in &out {
            let t = corners(&mesh, face).unwrap();
            assert!(cross(sub(t[1], t[0]), sub(t[2], t[0]))[2] > 0.0);
        }
    }

    #[test]
    fn the_radius_is_in_real_millimetres_on_a_scaled_object() {
        let mesh = sheet(20, 0.0);
        let adjacency = FaceAdjacency::new(&mesh);
        let mut marks = WalkMarks::default();
        let seed = (10 * 20 + 10) * 2;
        let mut plain = Vec::new();
        dab_faces(
            &mesh,
            &adjacency,
            seed,
            &dab([10.5, 10.5, 0.0], 3.0),
            &mut marks,
            &mut plain,
        );
        // Twice the size: a 6 mm brush covers the same faces.
        let linear = [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]];
        let scaled = Dab {
            metric: Dab::metric_of(&linear),
            ..dab([10.5, 10.5, 0.0], 6.0)
        };
        let mut twice = Vec::new();
        dab_faces(&mesh, &adjacency, seed, &scaled, &mut marks, &mut twice);
        plain.sort_unstable();
        twice.sort_unstable();
        assert_eq!(plain, twice);
    }

    #[test]
    fn a_fill_stops_where_the_surface_turns() {
        // A sheet folded up at x = 4.
        let mut mesh = sheet(8, 0.0);
        for v in &mut mesh.vertices {
            let p = v.coords();
            if p[0] > 4.0 {
                *v = Vertex::new([4.0, p[1], p[0] - 4.0]);
            }
        }
        let adjacency = FaceAdjacency::new(&mesh);
        let mut marks = WalkMarks::default();
        let mut out = Vec::new();
        fill_faces(&mesh, &adjacency, 0, 30.0, &mut marks, &mut out);
        assert_eq!(out.len(), 4 * 8 * 2);
        for &face in &out {
            let t = corners(&mesh, face).unwrap();
            assert!(t.iter().all(|p| p[2] == 0.0));
        }
        fill_faces(&mesh, &adjacency, 0, 100.0, &mut marks, &mut out);
        assert_eq!(out.len(), mesh.faces().len());
    }

    #[test]
    fn grow_and_shrink_move_the_edge_of_the_paint_by_one_ring() {
        let mesh = sheet(6, 0.0);
        let adjacency = FaceAdjacency::new(&mesh);
        let mut values = vec![false; mesh.faces().len()];
        values[30] = true;
        let added = ring_faces(&adjacency, |f| values[f as usize], true);
        assert_eq!(added.len(), adjacency.of(30).len());
        for f in added {
            values[f as usize] = true;
        }
        let edge = ring_faces(&adjacency, |f| values[f as usize], false);
        assert!(!edge.is_empty());
        assert!(edge.iter().all(|&f| values[f as usize]));
    }
}

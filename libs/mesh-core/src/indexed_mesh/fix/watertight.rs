/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The watertight pass: makes a mesh that fails the slicer's edge check
//! closed and edge-manifold, so that it can be sliced.
//!
//! 1 round runs these steps in order:
//!
//! 1. Weld open boundaries (1st round only): a vertex on a boundary edge is
//!    merged into an earlier boundary vertex within the weld tolerance. Only
//!    boundary vertices move, so closed parts that touch on purpose (supports
//!    glued to a model) keep their own vertices.
//! 2. Pair the faces of every edge. An edge with more than 2 faces is resolved
//!    radially: its faces are sorted by angle around it, and a face that walks
//!    the edge 1 way is paired with the next face that walks it the other way,
//!    the 2 sides of a solid wedge.
//! 3. Split the ends of those edges into 1 vertex per fan of paired faces, so
//!    2 parts that touch at an edge or a vertex become 2 closed shells. A face
//!    still on an edge with more than 2 faces after the split is dropped; step
//!    6 closes the gap.
//! 4. Orient each connected piece: the winding spreads face to face across
//!    the pairs, and the smaller side is flipped. Where no consistent winding
//!    exists (a seam like a Moebius strip's), the faces across it are dropped.
//! 5. Drop the faces marked in steps 3 and 4.
//! 6. Fill holes: each boundary loop that touches itself is split into simple
//!    loops; a loop of up to [`WatertightOptions::min_area_fill_max_edges`]
//!    edges gets its minimum-area triangulation, never reusing an existing
//!    edge, and a larger one (or one with no such triangulation) a fan from its
//!    centroid. A hole whose fill would exceed
//!    [`WatertightOptions::max_fill_area_fraction`] of the mesh area is left
//!    open: it is more likely an open part on purpose than a defect.
//! 7. Turn closed inside-out shells outward, unless they sit inside another
//!    closed shell (a cavity, like a hollow's). An outward shell is never
//!    turned inward.
//!
//! Rounds repeat while the mesh still fails the edge check, up to
//! [`WatertightOptions::max_rounds`]: the 2nd round settles what a fill of the
//! 1st left. A round is kept only when it leaves fewer bad edges than before
//! (never worse); the 1st round that does not ends the run.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;

use crate::primitives::{IndexedTriangle, Vertex, VertexIndex};

use super::super::half_edges;
use super::records::MeshFix;

/// The tunables of the watertight pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WatertightOptions {
    /// Resolve edges shared by more than 2 faces (steps 2 to 5).
    pub(crate) resolve_non_manifold: bool,
    /// Weld open boundaries and fill holes (steps 1 and 6).
    pub(crate) close_holes: bool,
    /// The largest distance, in mm, between 2 boundary vertices that are
    /// welded.
    pub(crate) weld_tolerance_mm: f64,
    /// The largest fill, as a fraction of the area of the whole mesh, that a
    /// hole gets without asking.
    pub(crate) max_fill_area_fraction: f64,
    /// The largest hole, in boundary edges, that gets a minimum-area
    /// triangulation; larger ones get a fan from their centroid.
    pub(crate) min_area_fill_max_edges: usize,
    /// How many rounds run at most.
    pub(crate) max_rounds: usize,
}

/// What the watertight pass did to a mesh, or would do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WatertightOutcome {
    /// The repaired mesh and its fixes, or `None` when no round changed it.
    pub(crate) repaired: Option<WatertightMesh>,
    /// Holes left open because their fill would exceed the size cap, in the
    /// mesh the run ends with.
    pub(crate) holes_left_open: usize,
}

/// A mesh the watertight pass produced.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WatertightMesh {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) faces: Vec<IndexedTriangle>,
    pub(crate) fixes: Vec<MeshFix>,
}

/// Plans the watertight pass on a mesh whose face indices are in range and
/// whose faces have 3 distinct vertices. Changes nothing: the caller applies
/// [`WatertightOutcome::repaired`].
pub(crate) fn plan_watertight(
    vertices: &[Vertex],
    faces: &[IndexedTriangle],
    options: &WatertightOptions,
) -> WatertightOutcome {
    let mut best_errors = count_bad_edges(faces);
    let mut holes_left_open = 0;
    if best_errors == 0 || !(options.resolve_non_manifold || options.close_holes) {
        return WatertightOutcome {
            repaired: None,
            holes_left_open,
        };
    }
    let mut current: Option<(Vec<Vertex>, Vec<IndexedTriangle>)> = None;
    let mut total = Stats::default();
    for round in 0..options.max_rounds {
        let (mut round_vertices, mut round_faces) = match &current {
            Some((v, f)) => (v.clone(), f.clone()),
            None => (vertices.to_vec(), faces.to_vec()),
        };
        let stats = run_round(&mut round_vertices, &mut round_faces, options, round == 0);
        if !stats.changed() || round_faces.is_empty() {
            holes_left_open = stats.holes_left_open;
            break;
        }
        let errors = count_bad_edges(&round_faces);
        if errors >= best_errors {
            if current.is_none() {
                holes_left_open = stats.holes_left_open;
            }
            break;
        }
        best_errors = errors;
        holes_left_open = stats.holes_left_open;
        total.add(&stats);
        current = Some((round_vertices, round_faces));
        if errors == 0 {
            break;
        }
    }
    WatertightOutcome {
        repaired: current.map(|(vertices, faces)| WatertightMesh {
            vertices,
            faces,
            fixes: total.into_fixes(),
        }),
        holes_left_open,
    }
}

/// Counts the edges that break the slicer's edge check: boundary edges, edges
/// with more than 2 faces, and edges whose 2 faces wind the same way. An empty
/// mesh counts 1.
fn count_bad_edges(faces: &[IndexedTriangle]) -> usize {
    if faces.is_empty() {
        return 1;
    }
    let records = half_edges::sorted_half_edge_records(faces);
    records
        .chunk_by(|a, b| a.0 == b.0)
        .filter(|run| {
            run.len() != 2 || half_edges::is_reversed(run[0].1) == half_edges::is_reversed(run[1].1)
        })
        .count()
}

/// The counts of 1 round, or of all kept rounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stats {
    welded_vertices: usize,
    collapsed_faces: usize,
    non_manifold_edges: usize,
    split_vertices: usize,
    removed_faces: usize,
    reoriented_faces: usize,
    holes: usize,
    fill_faces: usize,
    largest_hole: usize,
    shells_outward: usize,
    shell_faces_outward: usize,
    holes_left_open: usize,
}

impl Stats {
    fn changed(&self) -> bool {
        self.welded_vertices > 0
            || self.collapsed_faces > 0
            || self.split_vertices > 0
            || self.removed_faces > 0
            || self.reoriented_faces > 0
            || self.fill_faces > 0
            || self.shells_outward > 0
    }

    fn add(&mut self, other: &Self) {
        self.welded_vertices += other.welded_vertices;
        self.collapsed_faces += other.collapsed_faces;
        self.non_manifold_edges += other.non_manifold_edges;
        self.split_vertices += other.split_vertices;
        self.removed_faces += other.removed_faces;
        self.reoriented_faces += other.reoriented_faces;
        self.holes += other.holes;
        self.fill_faces += other.fill_faces;
        self.largest_hole = self.largest_hole.max(other.largest_hole);
        self.shells_outward += other.shells_outward;
        self.shell_faces_outward += other.shell_faces_outward;
    }

    fn into_fixes(self) -> Vec<MeshFix> {
        let mut fixes = Vec::new();
        if self.welded_vertices > 0 || self.collapsed_faces > 0 {
            fixes.push(MeshFix::WeldedBoundaryVertices {
                vertices: self.welded_vertices,
                collapsed_faces: self.collapsed_faces,
            });
        }
        if self.split_vertices > 0 || self.non_manifold_edges > 0 {
            fixes.push(MeshFix::SplitNonManifoldEdges {
                edges: self.non_manifold_edges,
                split_vertices: self.split_vertices,
            });
        }
        if self.removed_faces > 0 {
            fixes.push(MeshFix::RemovedUnpairedFaces {
                faces: self.removed_faces,
            });
        }
        if self.reoriented_faces > 0 {
            fixes.push(MeshFix::ReorientedFaces {
                faces: self.reoriented_faces,
            });
        }
        if self.holes > 0 {
            fixes.push(MeshFix::FilledHoles {
                holes: self.holes,
                added_faces: self.fill_faces,
                largest_hole_edges: self.largest_hole,
            });
        }
        if self.shells_outward > 0 {
            fixes.push(MeshFix::TurnedShellsOutward {
                shells: self.shells_outward,
                faces: self.shell_faces_outward,
            });
        }
        fixes
    }
}

fn run_round(
    vertices: &mut Vec<Vertex>,
    faces: &mut Vec<IndexedTriangle>,
    options: &WatertightOptions,
    first: bool,
) -> Stats {
    let mut stats = Stats::default();
    if options.close_holes && first && options.weld_tolerance_mm > 0.0 {
        weld_boundary_vertices(vertices, faces, options.weld_tolerance_mm, &mut stats);
    }
    let dropped = pair_split_and_orient(vertices, faces, options.resolve_non_manifold, &mut stats);
    if let Some(dropped) = dropped {
        let mut idx = 0;
        faces.retain(|_| {
            let keep = !dropped[idx];
            idx += 1;
            keep
        });
    }
    if options.close_holes {
        fill_holes(vertices, faces, options, &mut stats);
    }
    turn_inside_out_shells_outward(vertices, faces, &mut stats);
    stats
}

#[inline]
fn raw(face: &IndexedTriangle) -> [u32; 3] {
    face.vertices.map(VertexIndex::get)
}

#[inline]
fn position(vertices: &[Vertex], v: u32) -> [f64; 3] {
    vertices[v as usize].coords().map(f64::from)
}

#[inline]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

#[inline]
fn triangle_area(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    0.5 * norm(cross(sub(b, a), sub(c, a)))
}

/// Step 1: merges each boundary vertex into the 1st earlier boundary vertex
/// within `tolerance`, and drops the faces the merge collapses.
fn weld_boundary_vertices(
    vertices: &[Vertex],
    faces: &mut Vec<IndexedTriangle>,
    tolerance: f64,
    stats: &mut Stats,
) {
    let records = half_edges::sorted_half_edge_records(faces);
    let mut on_boundary = vec![false; vertices.len()];
    let mut any = false;
    for run in records.chunk_by(|a, b| a.0 == b.0) {
        if run.len() == 1 {
            let (a, b) = half_edges::key_vertices(run[0].0);
            on_boundary[usize::from(a)] = true;
            on_boundary[usize::from(b)] = true;
            any = true;
        }
    }
    drop(records);
    if !any {
        return;
    }
    let inverse = 1.0 / tolerance;
    let tolerance_squared = tolerance * tolerance;
    let cell = |p: [f64; 3]| p.map(|c| (c * inverse).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut remap: HashMap<u32, u32> = HashMap::new();
    for (v, _) in on_boundary.iter().enumerate().filter(|(_, b)| **b) {
        let p = position(vertices, v as u32);
        let c = cell(p);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else {
                        continue;
                    };
                    for &w in list {
                        let d = sub(position(vertices, w), p);
                        if dot(d, d) <= tolerance_squared {
                            found = Some(w);
                            break 'search;
                        }
                    }
                }
            }
        }
        match found {
            Some(w) => {
                remap.insert(v as u32, w);
            }
            None => grid.entry(c).or_default().push(v as u32),
        }
    }
    if remap.is_empty() {
        return;
    }
    stats.welded_vertices += remap.len();
    faces.retain_mut(|face| {
        let r = raw(face).map(|v| remap.get(&v).copied().unwrap_or(v));
        if r[0] == r[1] || r[1] == r[2] || r[0] == r[2] {
            stats.collapsed_faces += 1;
            false
        } else {
            *face = IndexedTriangle::from_raw_indices(r);
            true
        }
    });
}

/// Marks a pair whose 2 faces walk their shared edge the same way.
const SAME_WAY: u32 = 1 << 31;

/// Steps 2 to 4. Returns which faces to drop, or `None` when none.
fn pair_split_and_orient(
    vertices: &mut Vec<Vertex>,
    faces: &mut [IndexedTriangle],
    resolve_non_manifold: bool,
    stats: &mut Stats,
) -> Option<Vec<bool>> {
    let face_count = faces.len();
    let records = half_edges::sorted_half_edge_records(faces);

    // The ends of the edges with more than 2 faces are the vertices to split.
    let mut split_ends: HashSet<u32> = HashSet::new();
    if resolve_non_manifold {
        for run in records.chunk_by(|a, b| a.0 == b.0) {
            if run.len() > 2 {
                stats.non_manifold_edges += 1;
                let (a, b) = half_edges::key_vertices(run[0].0);
                split_ends.insert(a.get());
                split_ends.insert(b.get());
            }
        }
    }

    let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(records.len() / 2);
    let mut any_same_way = false;
    let mut corners = CornerSets::default();
    let mut add_pair = |f: u32, g: u32, same_way: bool, key: u64, corners: &mut CornerSets| {
        pairs.push((f, g | if same_way { SAME_WAY } else { 0 }));
        any_same_way |= same_way;
        if split_ends.is_empty() {
            return;
        }
        let (lo, hi) = half_edges::key_vertices(key);
        for end in [lo.get(), hi.get()] {
            if split_ends.contains(&end) {
                let cf = corner_of(&faces[f as usize], end);
                let cg = corner_of(&faces[g as usize], end);
                corners.union(f * 3 + cf, g * 3 + cg);
            }
        }
    };
    for run in records.chunk_by(|a, b| a.0 == b.0) {
        match run.len() {
            1 => {}
            2 => {
                let f = half_edges::face_of(run[0].1).get() as u32;
                let g = half_edges::face_of(run[1].1).get() as u32;
                let same_way =
                    half_edges::is_reversed(run[0].1) == half_edges::is_reversed(run[1].1);
                add_pair(f, g, same_way, run[0].0, &mut corners);
            }
            _ if resolve_non_manifold => {
                for (f, g, same_way) in radial_pairs(vertices, faces, run) {
                    add_pair(f, g, same_way, run[0].0, &mut corners);
                }
            }
            _ => {}
        }
    }
    drop(records);

    let mut drop_faces: Option<Vec<bool>> = None;
    if !split_ends.is_empty() {
        stats.split_vertices += split_vertex_fans(vertices, faces, &split_ends, &mut corners);
        // The fans can still meet elsewhere and keep an edge with more than 2
        // faces; those faces go, and the fill closes the gap.
        let records = half_edges::sorted_half_edge_records(faces);
        let mut marks = vec![false; face_count];
        let mut any = false;
        for run in records.chunk_by(|a, b| a.0 == b.0) {
            if run.len() > 2 {
                for &(_, value) in run {
                    marks[half_edges::face_of(value).get()] = true;
                }
                any = true;
            }
        }
        if any {
            drop_faces = Some(marks);
        }
    }

    if any_same_way {
        let seam = orient_pieces(faces, &pairs, stats);
        if let Some(seam) = seam {
            let marks = drop_faces.get_or_insert_with(|| vec![false; face_count]);
            for (mark, s) in marks.iter_mut().zip(seam) {
                *mark |= s;
            }
        }
    }
    if let Some(marks) = &drop_faces {
        stats.removed_faces += marks.iter().filter(|&&m| m).count();
    }
    drop_faces
}

/// Returns the position, 0 to 2, of vertex `v` in `face`.
#[inline]
fn corner_of(face: &IndexedTriangle, v: u32) -> u32 {
    let r = raw(face);
    if r[0] == v {
        0
    } else if r[1] == v {
        1
    } else {
        2
    }
}

/// Pairs the faces of 1 edge with more than 2 faces, sorted by angle around
/// it: a solid wedge lies between a face that walks the edge from its higher
/// vertex to its lower one and the next face that walks it back. Faces left
/// over pair in sorted order; an odd one stays unpaired.
fn radial_pairs(
    vertices: &[Vertex],
    faces: &[IndexedTriangle],
    run: &[(u64, u32)],
) -> Vec<(u32, u32, bool)> {
    let (lo, hi) = half_edges::key_vertices(run[0].0);
    let base = position(vertices, lo.get());
    let axis = sub(position(vertices, hi.get()), base);
    let axis_length = norm(axis).max(f64::MIN_POSITIVE);
    let axis = axis.map(|c| c / axis_length);
    let helper = if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = cross(axis, helper);
    let u = u.map(|c| c / norm(u));
    let w = cross(axis, u);
    // (angle, face, walks lo to hi)
    let mut ring: Vec<(f64, u32, bool)> = run
        .iter()
        .map(|&(_, value)| {
            let face = half_edges::face_of(value).get() as u32;
            let apex = raw(&faces[face as usize])
                .into_iter()
                .find(|&v| v != lo.get() && v != hi.get())
                .unwrap_or(lo.get());
            let q = sub(position(vertices, apex), base);
            let along = dot(q, axis);
            let q = sub(q, axis.map(|c| c * along));
            (
                dot(q, w).atan2(dot(q, u)),
                face,
                !half_edges::is_reversed(value),
            )
        })
        .collect();
    ring.sort_by(|a, b| a.0.total_cmp(&b.0));
    let k = ring.len();
    let mut used = vec![false; k];
    let mut pairs = Vec::with_capacity(k / 2);
    for a in 0..k {
        let b = (a + 1) % k;
        if !used[a] && !used[b] && !ring[a].2 && ring[b].2 {
            used[a] = true;
            used[b] = true;
            pairs.push((ring[a].1, ring[b].1, false));
        }
    }
    let left: Vec<usize> = (0..k).filter(|&a| !used[a]).collect();
    for pair in left.chunks_exact(2) {
        let (a, b) = (pair[0], pair[1]);
        pairs.push((ring[a].1, ring[b].1, ring[a].2 == ring[b].2));
    }
    pairs
}

/// A union-find over face corners (`face * 3 + corner`), only over the
/// corners that are named.
#[derive(Default)]
struct CornerSets {
    index: HashMap<u32, u32>,
    parent: Vec<u32>,
}

impl CornerSets {
    fn slot(&mut self, corner: u32) -> u32 {
        let next = self.parent.len() as u32;
        let slot = *self.index.entry(corner).or_insert(next);
        if slot == next {
            self.parent.push(next);
        }
        slot
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let up = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = up;
            x = up;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let a = self.slot(a);
        let b = self.slot(b);
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[high as usize] = low;
        }
    }

    fn root(&mut self, corner: u32) -> u32 {
        let slot = self.slot(corner);
        self.find(slot)
    }
}

/// Step 3: gives each fan of a split vertex its own vertex. The 1st fan, in
/// face order, keeps the original index; each other fan gets a new vertex at
/// the same position. Returns how many vertices were added.
fn split_vertex_fans(
    vertices: &mut Vec<Vertex>,
    faces: &mut [IndexedTriangle],
    split_ends: &HashSet<u32>,
    corners: &mut CornerSets,
) -> usize {
    let mut fan_vertex: HashMap<u32, u32> = HashMap::new();
    let mut taken: HashSet<u32> = HashSet::new();
    let before = vertices.len();
    for (f, face) in faces.iter_mut().enumerate() {
        let mut r = raw(face);
        let mut changed = false;
        for (c, v) in r.iter_mut().enumerate() {
            if !split_ends.contains(v) {
                continue;
            }
            let root = corners.root(f as u32 * 3 + c as u32);
            let id = *fan_vertex.entry(root).or_insert_with(|| {
                if taken.insert(*v) {
                    *v
                } else {
                    vertices.push(vertices[*v as usize]);
                    (vertices.len() - 1) as u32
                }
            });
            if id != *v {
                *v = id;
                changed = true;
            }
        }
        if changed {
            *face = IndexedTriangle::from_raw_indices(r);
        }
    }
    vertices.len() - before
}

/// Step 4: orients each piece connected through `pairs`, flipping the smaller
/// side. Returns the faces across a seam with no consistent winding, or
/// `None` when there is none.
fn orient_pieces(
    faces: &mut [IndexedTriangle],
    pairs: &[(u32, u32)],
    stats: &mut Stats,
) -> Option<Vec<bool>> {
    let face_count = faces.len();
    let mut start = vec![0u32; face_count + 1];
    for &(f, g) in pairs {
        start[f as usize + 1] += 1;
        start[(g & !SAME_WAY) as usize + 1] += 1;
    }
    for i in 0..face_count {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut neighbours = vec![0u32; start[face_count] as usize];
    for &(f, g) in pairs {
        let flag = g & SAME_WAY;
        let g = g & !SAME_WAY;
        neighbours[fill[f as usize] as usize] = g | flag;
        fill[f as usize] += 1;
        neighbours[fill[g as usize] as usize] = f | flag;
        fill[g as usize] += 1;
    }
    drop(fill);

    const UNSET: u8 = u8::MAX;
    let mut parity = vec![UNSET; face_count];
    let mut seam: Option<Vec<bool>> = None;
    let mut stack = Vec::new();
    let mut members = Vec::new();
    for s in 0..face_count {
        if parity[s] != UNSET {
            continue;
        }
        parity[s] = 0;
        stack.push(s as u32);
        members.clear();
        while let Some(f) = stack.pop() {
            members.push(f);
            let pf = parity[f as usize];
            for &n in &neighbours[start[f as usize] as usize..start[f as usize + 1] as usize] {
                let g = n & !SAME_WAY;
                let want = if n & SAME_WAY != 0 { pf ^ 1 } else { pf };
                if parity[g as usize] == UNSET {
                    parity[g as usize] = want;
                    stack.push(g);
                } else if parity[g as usize] != want {
                    let marks = seam.get_or_insert_with(|| vec![false; face_count]);
                    marks[f as usize] = true;
                    marks[g as usize] = true;
                }
            }
        }
        let ones = members.iter().filter(|&&f| parity[f as usize] == 1).count();
        let flip = if ones * 2 <= members.len() { 1 } else { 0 };
        for &f in &members {
            if parity[f as usize] == flip {
                faces[f as usize].vertices.reverse();
                stats.reoriented_faces += 1;
            }
        }
    }
    seam
}

/// Step 6: fills the holes under the size cap.
fn fill_holes(
    vertices: &mut Vec<Vertex>,
    faces: &mut Vec<IndexedTriangle>,
    options: &WatertightOptions,
    stats: &mut Stats,
) {
    let records = half_edges::sorted_half_edge_records(faces);
    // Each boundary edge, in the direction its face walks it.
    let mut boundary: Vec<(u32, u32)> = records
        .chunk_by(|a, b| a.0 == b.0)
        .filter(|run| run.len() == 1)
        .map(|run| {
            let (lo, hi) = half_edges::key_vertices(run[0].0);
            if half_edges::is_reversed(run[0].1) {
                (hi.get(), lo.get())
            } else {
                (lo.get(), hi.get())
            }
        })
        .collect();
    if boundary.is_empty() {
        return;
    }
    let mesh_area: f64 = faces
        .par_iter()
        .map(|face| {
            let [a, b, c] = raw(face).map(|v| position(vertices, v));
            triangle_area(a, b, c)
        })
        .sum();
    let max_fill_area = mesh_area * options.max_fill_area_fraction;

    boundary.sort_unstable();
    let mut used = vec![false; boundary.len()];
    let mut new_edges: HashSet<u64> = HashSet::new();
    let edge_exists = |a: u32, b: u32, new_edges: &HashSet<u64>| {
        let key = half_edges::key(VertexIndex::from(a), VertexIndex::from(b));
        new_edges.contains(&key) || records.binary_search_by_key(&key, |r| r.0).is_ok()
    };
    let mut added: Vec<IndexedTriangle> = Vec::new();
    for start in 0..boundary.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (first, mut current) = boundary[start];
        let mut walk = vec![first];
        let mut closed = true;
        while current != first {
            let from = boundary.partition_point(|e| e.0 < current);
            let next = (from..boundary.len())
                .take_while(|&k| boundary[k].0 == current)
                .find(|&k| !used[k]);
            let Some(k) = next else {
                closed = false;
                break;
            };
            used[k] = true;
            walk.push(current);
            current = boundary[k].1;
        }
        if !closed {
            continue;
        }
        for mut hole in split_into_simple_loops(&walk) {
            if hole.len() < 3 {
                continue;
            }
            // The fill walks the boundary against its faces.
            hole.reverse();
            let points: Vec<[f64; 3]> = hole.iter().map(|&v| position(vertices, v)).collect();
            let centroid = centroid(&points);
            let fan_area: f64 = (0..points.len())
                .map(|i| triangle_area(points[i], points[(i + 1) % points.len()], centroid))
                .sum();
            if fan_area > max_fill_area {
                stats.holes_left_open += 1;
                continue;
            }
            let triangles = if hole.len() <= options.min_area_fill_max_edges {
                min_area_fill(&hole, &points, vertices, |a, b| {
                    edge_exists(a, b, &new_edges)
                })
            } else {
                None
            };
            let before = added.len();
            match triangles {
                Some(triangles) => {
                    for t in triangles {
                        for q in 0..3 {
                            new_edges.insert(half_edges::key(
                                VertexIndex::from(t[q]),
                                VertexIndex::from(t[(q + 1) % 3]),
                            ));
                        }
                        added.push(IndexedTriangle::from_raw_indices(t));
                    }
                }
                None => {
                    vertices.push(Vertex::new(centroid.map(|c| c as f32)));
                    let apex = (vertices.len() - 1) as u32;
                    for i in 0..hole.len() {
                        added.push(IndexedTriangle::from_raw_indices([
                            hole[i],
                            hole[(i + 1) % hole.len()],
                            apex,
                        ]));
                    }
                }
            }
            stats.holes += 1;
            stats.fill_faces += added.len() - before;
            stats.largest_hole = stats.largest_hole.max(hole.len());
        }
    }
    drop(records);
    faces.extend(added);
}

fn centroid(points: &[[f64; 3]]) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for p in points {
        for k in 0..3 {
            sum[k] += p[k];
        }
    }
    sum.map(|c| c / points.len() as f64)
}

/// Splits a closed boundary walk that visits a vertex more than once into
/// loops that visit each vertex once.
fn split_into_simple_loops(walk: &[u32]) -> Vec<Vec<u32>> {
    let mut stack: Vec<u32> = Vec::with_capacity(walk.len());
    let mut at: HashMap<u32, usize> = HashMap::new();
    let mut loops = Vec::new();
    for &v in walk {
        if let Some(&i) = at.get(&v) {
            let inner: Vec<u32> = stack.drain(i + 1..).collect();
            for x in &inner {
                at.remove(x);
            }
            let mut sub = Vec::with_capacity(inner.len() + 1);
            sub.push(v);
            sub.extend(inner);
            loops.push(sub);
        } else {
            at.insert(v, stack.len());
            stack.push(v);
        }
    }
    loops.push(stack);
    loops
}

/// The minimum-area triangulation of polygon `hole`, by dynamic programming.
/// A diagonal must not be an edge already (`edge_exists`), and no triangle may
/// hold 2 vertices at the same position (2 copies of a split vertex): such a
/// triangle has no area and turns degenerate when the file is saved as STL.
/// Returns `None` when no triangulation is allowed.
fn min_area_fill(
    hole: &[u32],
    points: &[[f64; 3]],
    vertices: &[Vertex],
    edge_exists: impl Fn(u32, u32) -> bool,
) -> Option<Vec<[u32; 3]>> {
    let n = hole.len();
    if n == 3 {
        let distinct = |a: u32, b: u32| vertices[a as usize] != vertices[b as usize];
        return (distinct(hole[0], hole[1])
            && distinct(hole[1], hole[2])
            && distinct(hole[0], hole[2]))
        .then(|| vec![[hole[0], hole[1], hole[2]]]);
    }
    let same_place = |i: usize, j: usize| vertices[hole[i] as usize] == vertices[hole[j] as usize];
    let mut cost = vec![f64::INFINITY; n * n];
    let mut split = vec![u32::MAX; n * n];
    for i in 0..n - 1 {
        cost[i * n + i + 1] = 0.0;
    }
    for len in 2..n {
        for i in 0..n - len {
            let j = i + len;
            let side = i == 0 && j == n - 1;
            if same_place(i, j) || (!side && edge_exists(hole[i], hole[j])) {
                continue;
            }
            let mut best = f64::INFINITY;
            let mut best_m = u32::MAX;
            for m in i + 1..j {
                let c = cost[i * n + m] + cost[m * n + j];
                if !c.is_finite() || same_place(i, m) || same_place(m, j) {
                    continue;
                }
                let c = c + triangle_area(points[i], points[m], points[j]);
                if c < best {
                    best = c;
                    best_m = m as u32;
                }
            }
            cost[i * n + j] = best;
            split[i * n + j] = best_m;
        }
    }
    if !cost[n - 1].is_finite() {
        return None;
    }
    let mut triangles = Vec::with_capacity(n - 2);
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j - i < 2 {
            continue;
        }
        let m = split[i * n + j] as usize;
        triangles.push([hole[i], hole[m], hole[j]]);
        stack.push((i, m));
        stack.push((m, j));
    }
    Some(triangles)
}

/// Step 7: turns each closed shell with negative volume outward, unless it
/// lies inside a closed shell with positive volume (then it is a cavity).
fn turn_inside_out_shells_outward(
    vertices: &[Vertex],
    faces: &mut [IndexedTriangle],
    stats: &mut Stats,
) {
    let face_count = faces.len();
    let records = half_edges::sorted_half_edge_records(faces);
    let mut parent: Vec<u32> = (0..face_count as u32).collect();
    let mut open = vec![false; face_count];
    for run in records.chunk_by(|a, b| a.0 == b.0) {
        let consistent = run.len() == 2
            && half_edges::is_reversed(run[0].1) != half_edges::is_reversed(run[1].1);
        if !consistent {
            for &(_, value) in run {
                open[half_edges::face_of(value).get()] = true;
            }
        }
        let first = half_edges::face_of(run[0].1).get() as u32;
        for &(_, value) in &run[1..] {
            union_faces(&mut parent, first, half_edges::face_of(value).get() as u32);
        }
    }
    drop(records);

    // Group the faces by shell.
    let mut shell_of = vec![u32::MAX; face_count];
    let mut shell_count = 0u32;
    let mut root_shell: HashMap<u32, u32> = HashMap::new();
    for f in 0..face_count as u32 {
        let root = find_face(&mut parent, f);
        let shell = *root_shell.entry(root).or_insert_with(|| {
            shell_count += 1;
            shell_count - 1
        });
        shell_of[f as usize] = shell;
    }
    drop(root_shell);
    drop(parent);
    let shell_count = shell_count as usize;
    let mut start = vec![0u32; shell_count + 1];
    for &s in &shell_of {
        start[s as usize + 1] += 1;
    }
    for i in 0..shell_count {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut members = vec![0u32; face_count];
    for (f, &s) in shell_of.iter().enumerate() {
        members[fill[s as usize] as usize] = f as u32;
        fill[s as usize] += 1;
    }
    drop(fill);
    let shell_faces = |s: usize| &members[start[s] as usize..start[s + 1] as usize];

    struct Shell {
        closed: bool,
        volume: f64,
        lo: [f64; 3],
        hi: [f64; 3],
    }
    let faces_ro: &[IndexedTriangle] = faces;
    let shells: Vec<Shell> = (0..shell_count)
        .into_par_iter()
        .map(|s| {
            let mut volume = 0.0;
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            let mut closed = true;
            for &f in shell_faces(s) {
                closed &= !open[f as usize];
                let [a, b, c] = raw(&faces_ro[f as usize]).map(|v| position(vertices, v));
                volume += dot(a, cross(b, c)) / 6.0;
                for p in [a, b, c] {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
            }
            Shell {
                closed,
                volume,
                lo,
                hi,
            }
        })
        .collect();
    // A closed shell with no volume (an open sheet the fill doubled) is
    // neither inside out nor outward.
    let has_volume = |s: &Shell| {
        let d = norm(sub(s.hi, s.lo));
        s.volume.abs() > 1e-9 * d * d * d
    };
    let negative: Vec<usize> = (0..shell_count)
        .filter(|&s| shells[s].closed && shells[s].volume < 0.0 && has_volume(&shells[s]))
        .collect();
    if negative.is_empty() {
        return;
    }
    let contains = |outer: &Shell, inner: &Shell| {
        (0..3).all(|k| outer.lo[k] <= inner.lo[k] && outer.hi[k] >= inner.hi[k])
    };
    let to_flip: Vec<usize> = negative
        .par_iter()
        .copied()
        .filter(|&s| {
            let own = shell_faces(s);
            let samples: Vec<[f64; 3]> = own
                .iter()
                .step_by((own.len() / 16).max(1))
                .take(16)
                .map(|&f| position(vertices, raw(&faces_ro[f as usize])[0]))
                .collect();
            let is_cavity = (0..shell_count).any(|d| {
                d != s
                    && shells[d].closed
                    && shells[d].volume > 0.0
                    && has_volume(&shells[d])
                    && contains(&shells[d], &shells[s])
                    && samples
                        .iter()
                        .all(|&p| winding_number(p, shell_faces(d), faces_ro, vertices) != 0)
            });
            !is_cavity
        })
        .collect();
    for s in to_flip {
        stats.shells_outward += 1;
        for &f in shell_faces(s) {
            faces[f as usize].vertices.reverse();
            stats.shell_faces_outward += 1;
        }
    }
}

fn find_face(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let up = parent[parent[x as usize] as usize];
        parent[x as usize] = up;
        x = up;
    }
    x
}

fn union_faces(parent: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find_face(parent, a), find_face(parent, b));
    if ra != rb {
        let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[high as usize] = low;
    }
}

/// The winding number of a closed shell around point `p`: the signed count
/// of its faces that a ray from `p` along +X crosses. The ray is nudged by an
/// infinitesimal offset (+e in Y, +e^2 in Z) when it meets an edge or a vertex
/// exactly, so each crossing counts once.
fn winding_number(
    p: [f64; 3],
    shell: &[u32],
    faces: &[IndexedTriangle],
    vertices: &[Vertex],
) -> i32 {
    // The sign of the 2D edge function of (a, b) at p, in the (y, z) plane,
    // with the nudge breaking ties.
    let side = |a: [f64; 3], b: [f64; 3]| -> i8 {
        let e = (b[1] - a[1]) * (p[2] - a[2]) - (b[2] - a[2]) * (p[1] - a[1]);
        if e > 0.0 {
            1
        } else if e < 0.0 {
            -1
        } else if b[2] != a[2] {
            if b[2] < a[2] { 1 } else { -1 }
        } else if b[1] > a[1] {
            1
        } else {
            -1
        }
    };
    let mut winding = 0;
    for &f in shell {
        let [a, b, c] = raw(&faces[f as usize]).map(|v| position(vertices, v));
        if (a[1] < p[1] && b[1] < p[1] && c[1] < p[1])
            || (a[1] > p[1] && b[1] > p[1] && c[1] > p[1])
            || (a[2] < p[2] && b[2] < p[2] && c[2] < p[2])
            || (a[2] > p[2] && b[2] > p[2] && c[2] > p[2])
            || (a[0] < p[0] && b[0] < p[0] && c[0] < p[0])
        {
            continue;
        }
        let (s0, s1, s2) = (side(b, c), side(c, a), side(a, b));
        if s0 != s1 || s1 != s2 {
            continue;
        }
        // Where the ray meets the plane of the face.
        let normal = cross(sub(b, a), sub(c, a));
        if normal[0] == 0.0 {
            continue;
        }
        let x = a[0] - (normal[1] * (p[1] - a[1]) + normal[2] * (p[2] - a[2])) / normal[0];
        if x > p[0] {
            winding += i32::from(s0);
        }
    }
    winding
}

#[cfg(test)]
mod watertight_helper_tests {
    use super::*;

    #[test]
    fn a_walk_that_visits_a_vertex_twice_splits_into_simple_loops() {
        // 2 triangles that touch at vertex 0.
        let loops = split_into_simple_loops(&[0, 1, 2, 0, 3, 4]);
        assert_eq!(loops, vec![vec![0, 1, 2], vec![0, 3, 4]]);
        assert_eq!(split_into_simple_loops(&[5, 6, 7]), vec![vec![5, 6, 7]]);
    }

    fn square() -> (Vec<u32>, Vec<[f64; 3]>, Vec<Vertex>) {
        let points = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let vertices = points
            .iter()
            .map(|p: &[f64; 3]| Vertex::new(p.map(|c| c as f32)))
            .collect();
        (vec![0, 1, 2, 3], points, vertices)
    }

    #[test]
    fn the_fill_never_reuses_an_existing_edge() {
        let (hole, points, vertices) = square();
        let blocked = |a: u32, b: u32| (a.min(b), a.max(b)) == (0, 2);
        let triangles = min_area_fill(&hole, &points, &vertices, blocked).expect("a fill");
        assert_eq!(triangles.len(), 2);
        for t in &triangles {
            assert!(t.contains(&1) && t.contains(&3), "{t:?} uses diagonal 0-2");
        }
        let both = |a: u32, b: u32| matches!((a.min(b), a.max(b)), (0, 2) | (1, 3));
        assert_eq!(min_area_fill(&hole, &points, &vertices, both), None);
    }

    #[test]
    fn the_fill_keeps_2_copies_of_a_vertex_out_of_1_triangle() {
        // Vertices 0 and 2 are copies at the same place.
        let (hole, mut points, mut vertices) = square();
        points[2] = points[0];
        vertices[2] = vertices[0];
        let triangles = min_area_fill(&hole, &points, &vertices, |_, _| false).expect("a fill");
        assert_eq!(triangles.len(), 2);
        for t in triangles {
            assert!(!(t.contains(&0) && t.contains(&2)), "{t:?}");
        }
    }

    #[test]
    fn the_winding_number_counts_a_box_once() {
        let vertices: Vec<Vertex> = (0..8u32)
            .map(|c| Vertex::new([(c & 1) as f32, ((c >> 1) & 1) as f32, ((c >> 2) & 1) as f32]))
            .collect();
        let faces: Vec<IndexedTriangle> = [
            [0, 2, 3],
            [0, 3, 1],
            [4, 5, 7],
            [4, 7, 6],
            [0, 1, 5],
            [0, 5, 4],
            [2, 6, 7],
            [2, 7, 3],
            [0, 4, 6],
            [0, 6, 2],
            [1, 3, 7],
            [1, 7, 5],
        ]
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
        let all: Vec<u32> = (0..12).collect();
        let w = |p| winding_number(p, &all, &faces, &vertices);
        assert_eq!(w([0.5, 0.5, 0.5]), 1);
        // The ray meets the diagonal of the +X side: still 1 crossing.
        assert_eq!(w([0.25, 0.25, 0.25]), 1);
        // Outside, with rays through 2 sides, along an edge, and past it.
        assert_eq!(w([-1.0, 0.5, 0.5]), 0);
        assert_eq!(w([-1.0, 0.0, 0.5]), 0);
        assert_eq!(w([-1.0, 1.0, 1.0]), 0);
        assert_eq!(w([2.0, 0.5, 0.5]), 0);
        assert_eq!(w([0.5, 2.0, 0.5]), 0);
    }
}

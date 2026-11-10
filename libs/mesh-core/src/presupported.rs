/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pre-supported files (#258): which faces of an imported mesh are the model,
//! and which are the supports and the raft its maker added, read from the
//! geometry alone (never the file name).
//!
//! A pre-supported file keeps the model, the raft and every piece of every
//! support (root cone, shaft, brace, joint sphere, tip cone) as separate
//! closed shells. [`find_supports`] sorts the shells ([`FaceShells`]):
//!
//! 1. a *cavity* is a shell facing inward inside a bigger one, a *fragment*
//!    one under 4 faces or with almost no volume;
//! 2. the *raft* touches the plate, is at most [`RAFT_MAX_HEIGHT_MM`] tall,
//!    and is wide;
//! 3. the *pieces* of the other shells (faces joined through an edge) are
//!    fitted as surfaces of revolution: cylinder, cone, disc or sphere;
//! 4. a *support candidate* is a shell made of such primitives no wider
//!    than a support, or a thin tube;
//! 5. a candidate is a support when it reaches the base, by itself or
//!    through other candidates it is joined to (an end of one inside the
//!    other); the thin end of a contact cone touches what it holds and joins
//!    nothing. A loose group of candidates with two cones between two parts
//!    of the model is a model-to-model support;
//! 6. the model is the biggest shell left and those with a fair share of
//!    it, what lies within its box, and what sticks out of it (an axe, a
//!    crown); small primitive or thin pieces welded to it at a vertex are
//!    supports;
//! 7. whatever is left goes with the model, so removing the imported
//!    supports never takes a part of the figure.
//!
//! On 37 real files the supports and the raft came out right on every
//! triangle of the files that ship a twin without supports; 0.39 % of the
//! corpus was left with the model unsorted.
//!
//! A file whose supports are fused into the model in one shell cannot be
//! separated; [`fused_supports`] tells one by the many thin pillars a cut
//! just above its base goes through.

use std::cmp::Ordering;
use std::collections::HashMap;

use rayon::prelude::*;

use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{IndexedTriangle, Vertex};
use crate::shells::FaceShells;

/// The name the mesh of the imported supports carries: what tells it from
/// a model wherever models are told apart from supports.
pub const IMPORTED_SUPPORTS_NAME: &str = "Imported supports";

/// Whether a mesh named `name` holds imported supports.
pub fn is_imported_supports_name(name: &str) -> bool {
    name == IMPORTED_SUPPORTS_NAME
}

/// Whether `mesh` holds imported supports ([`IMPORTED_SUPPORTS_NAME`]).
pub fn is_imported_supports(mesh: &IndexedMesh) -> bool {
    mesh.meta()
        .as_ref()
        .and_then(|m| m.name.as_deref())
        .is_some_and(is_imported_supports_name)
}

/// Fewer shells than this: not a pre-supported file, and not read.
pub const MIN_SHELLS: u32 = 12;
/// Fewer support shells than this: what looked like supports is detail of
/// the model.
const MIN_SUPPORT_SHELLS: usize = 3;
/// A shell this close to the lowest point touches the plate, mm.
const PLATE_EPS_MM: f64 = 0.05;
/// The tallest a raft is, mm.
pub const RAFT_MAX_HEIGHT_MM: f64 = 3.5;
/// The narrowest a raft is, mm, unless the model above it is narrower.
const RAFT_MIN_WIDTH_MM: f64 = 10.0;
/// A raft is at least this share of the width of the biggest shell above it.
const RAFT_WIDTH_SHARE: f64 = 0.3;
/// Shells with this share of the biggest volume left are model too.
const MODEL_SHARE: f64 = 0.2;
/// Of a shell's faces, the share on primitives for it to be a support.
const PRIMITIVE_SHARE: f64 = 0.9;
/// Ends of two primitives this far apart still join, mm.
const LINK_SLACK_MM: f64 = 0.15;
/// Volume over area of a tube of 1.8 mm across: thinner is a thin support.
const THIN_VOLUME_PER_AREA_MM: f64 = 0.45;
/// The second spread of a tube's surface, at most, mm: a sword blade is thin
/// across one direction only.
const TUBE_SPREAD_MM: f64 = 1.5;
/// A thin shell is at most this share of the biggest volume left.
const THIN_SHARE: f64 = 0.2;
/// A shell of primitives is at most this share of the biggest volume left.
const PRIMITIVES_SHARE_MAX: f64 = 0.5;
/// A cone end this much narrower than the other is a contact tip.
const TIP_RATIO: f64 = 0.6;
/// Wider primitives are bases or parts of the model, not supports, mm.
const MAX_SUPPORT_RADIUS_MM: f64 = 5.0;
/// Shells with more faces are not fitted: no support is that dense.
const MAX_FITTED_SHELL_FACES: usize = 50_000;
/// Pieces with more faces are not fitted.
const MAX_FITTED_PIECE_FACES: usize = 20_000;
/// A support reaches the base when it starts this close above the raft, mm.
const BASE_REACH_MM: f64 = 0.3;
/// The box of the model, grown by this much, holds its loose details, mm.
const MODEL_BOX_SLACK_MM: f64 = 0.1;

/// What a shell or a piece is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Unsorted,
    Cavity,
    Fragment,
    Raft,
    Support,
    Model,
}

/// The model and the supports of a pre-supported file, apart: every face of
/// the file in one of the two, its vertices where they were.
#[derive(Clone, Debug)]
pub struct Presupported {
    /// The model, with whatever was left unsorted.
    pub model: IndexedMesh,
    /// The supports and the raft, named [`IMPORTED_SUPPORTS_NAME`].
    pub supports: IndexedMesh,
    /// How many shells the model holds.
    pub model_shells: usize,
    /// How many shells the supports hold, the raft's apart.
    pub support_shells: usize,
    /// Whether a raft came with them.
    pub raft: bool,
}

/// The model and the supports of `mesh`, when it is a pre-supported file
/// whose pieces are separate shells; `None` when no supports are found.
pub fn find_supports(mesh: &IndexedMesh) -> Option<Presupported> {
    let shells = FaceShells::of_mesh(mesh);
    if shells.count() < MIN_SHELLS || !mesh.vertices().iter().all(finite) {
        return None;
    }
    let sorted = sort(mesh, &shells);
    let support_shells = (0..shells.count() as usize)
        .filter(|&s| sorted.shell[s] == Class::Support)
        .count();
    if support_shells < MIN_SUPPORT_SHELLS {
        return None;
    }
    let raft = sorted.shell.contains(&Class::Raft);
    let is_support = |f: usize| {
        matches!(
            sorted.piece[sorted.piece_of_face[f] as usize],
            Class::Support | Class::Raft
        )
    };
    let model_shells = (0..shells.count() as usize)
        .filter(|&s| !matches!(sorted.shell[s], Class::Support | Class::Raft))
        .count();
    let (model_faces, support_faces): (Vec<usize>, Vec<usize>) =
        (0..mesh.faces().len()).partition(|&f| !is_support(f));
    if model_faces.is_empty() {
        return None;
    }
    let supports_meta = MeshMetadata {
        name: Some(IMPORTED_SUPPORTS_NAME.to_owned()),
        header_comment: None,
        declared_facet_normals: None,
    };
    Some(Presupported {
        model: sub_mesh(mesh, &model_faces, mesh.meta().clone()),
        supports: sub_mesh(mesh, &support_faces, Some(supports_meta)),
        model_shells,
        support_shells,
        raft,
    })
}

fn finite(v: &Vertex) -> bool {
    v.coords().iter().all(|c| c.is_finite())
}

/// The faces `faces` of `mesh`, with only the vertices they use, in order.
fn sub_mesh(mesh: &IndexedMesh, faces: &[usize], meta: Option<MeshMetadata>) -> IndexedMesh {
    let mut remap = vec![u32::MAX; mesh.vertices().len()];
    let mut vertices = Vec::new();
    let mut out = Vec::with_capacity(faces.len());
    for &f in faces {
        let corners = mesh.faces()[f].vertices.map(|v| {
            let v = v.get() as usize;
            if remap[v] == u32::MAX {
                remap[v] = vertices.len() as u32;
                vertices.push(mesh.vertices()[v]);
            }
            remap[v]
        });
        out.push(IndexedTriangle::from_raw_indices(corners));
    }
    IndexedMesh::new(meta, vertices, out)
}

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

fn point(v: &Vertex) -> V3 {
    v.coords().map(f64::from)
}

/// The eigenvalues of the symmetric matrix `m`, ascending, and their unit
/// eigenvectors as columns (`vectors[row][column]`), by Jacobi rotations.
fn symmetric_eigen(mut m: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let off = m[0][1].abs() + m[0][2].abs() + m[1][2].abs();
        if off <= 1e-30 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if m[p][q].abs() <= 1e-300 {
                continue;
            }
            let theta = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            let r = 3 - p - q;
            let (mpp, mqq, mpq) = (m[p][p], m[q][q], m[p][q]);
            m[p][p] = mpp - t * mpq;
            m[q][q] = mqq + t * mpq;
            m[p][q] = 0.0;
            m[q][p] = 0.0;
            let (mrp, mrq) = (m[r][p], m[r][q]);
            m[r][p] = c * mrp - s * mrq;
            m[p][r] = m[r][p];
            m[r][q] = s * mrp + c * mrq;
            m[q][r] = m[r][q];
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| m[a][a].partial_cmp(&m[b][b]).unwrap_or(Ordering::Equal));
    let values = order.map(|k| m[k][k]);
    let mut vectors = [[0.0; 3]; 3];
    for (column, &k) in order.iter().enumerate() {
        for row in 0..3 {
            vectors[row][column] = v[row][k];
        }
    }
    (values, vectors)
}

fn column(m: &[[f64; 3]; 3], k: usize) -> V3 {
    [m[0][k], m[1][k], m[2][k]]
}

/// The median of `values`, which it sorts: the mean of the two middle ones
/// when there is an even number of them.
fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// A surface of revolution fitted to a piece's vertices.
#[derive(Clone, Copy, Debug)]
struct Fit {
    /// The axis, pointing up.
    axis: V3,
    centre: V3,
    /// The low and high ends along the axis, from the centre.
    t0: f64,
    t1: f64,
    /// The radius near each end.
    r0: f64,
    r1: f64,
    rmax: f64,
    /// The share of the points on rings, and how much a ring's radius varies,
    /// over the biggest radius.
    ring_fraction: f64,
    residual: f64,
    rings: usize,
    /// How evenly far from the centre the points are, and how far.
    sphericity: f64,
    radius: f64,
}

/// Points this far apart along the axis are on one ring, mm.
const RING_STEP_MM: f64 = 0.02;
/// A ring has this many vertices or more: a box's 4 corners at one height are
/// equally far from its axis too, and are not a ring.
const RING_MIN_POINTS: usize = 6;

/// The surface of revolution about the longest or the shortest axis of the
/// points that fits them best.
fn revolution_fit(points: &[V3]) -> Option<Fit> {
    let n = points.len();
    if n == 0 {
        return None;
    }
    let centre = scale(
        points.iter().fold([0.0; 3], |s, p| add(s, *p)),
        1.0 / n as f64,
    );
    let mut cov = [[0.0; 3]; 3];
    for p in points {
        let q = sub(*p, centre);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += q[i] * q[j];
            }
        }
    }
    for row in &mut cov {
        for c in row {
            *c /= n as f64;
        }
    }
    let (_, vectors) = symmetric_eigen(cov);
    let mut best: Option<(f64, Fit)> = None;
    for k in [2usize, 0] {
        let axis = column(&vectors, k);
        let mut t = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for p in points {
            let q = sub(*p, centre);
            let along = dot(q, axis);
            t.push(along);
            r.push(norm(sub(q, scale(axis, along))));
        }
        let rmax = r.iter().copied().fold(0.0f64, f64::max);
        if rmax <= 0.0 {
            continue;
        }
        let kept: Vec<usize> = (0..n).filter(|&i| r[i] > 0.1 * rmax).collect();
        if kept.is_empty() {
            continue;
        }
        let mut by_ring: Vec<(i64, usize)> = kept
            .iter()
            .map(|&i| ((t[i] / RING_STEP_MM).round() as i64, i))
            .collect();
        by_ring.sort_unstable();
        let mut on_rings = 0usize;
        let mut rings = 0usize;
        let mut deviations = Vec::new();
        let mut start = 0;
        while start < by_ring.len() {
            let mut end = start + 1;
            while end < by_ring.len() && by_ring[end].0 == by_ring[start].0 {
                end += 1;
            }
            if end - start >= RING_MIN_POINTS {
                rings += 1;
                on_rings += end - start;
                let mut radii: Vec<f64> = by_ring[start..end].iter().map(|&(_, i)| r[i]).collect();
                let ring_median = median(&mut radii);
                deviations.extend(
                    by_ring[start..end]
                        .iter()
                        .map(|&(_, i)| (r[i] - ring_median).abs()),
                );
            }
            start = end;
        }
        let ring_fraction = on_rings as f64 / kept.len() as f64;
        let residual = if deviations.is_empty() {
            1.0
        } else {
            median(&mut deviations) / rmax
        };
        let score = residual + (1.0 - ring_fraction);
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            let tmin = t.iter().copied().fold(f64::INFINITY, f64::min);
            let tmax = t.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let span = (tmax - tmin).max(1e-6);
            let near = |low: bool| {
                (0..n)
                    .filter(|&i| {
                        if low {
                            t[i] < tmin + 0.15 * span
                        } else {
                            t[i] > tmax - 0.15 * span
                        }
                    })
                    .map(|i| r[i])
                    .fold(0.0f64, f64::max)
            };
            best = Some((
                score,
                Fit {
                    axis,
                    centre,
                    t0: tmin,
                    t1: tmax,
                    r0: near(true),
                    r1: near(false),
                    rmax,
                    ring_fraction,
                    residual,
                    rings,
                    sphericity: 0.0,
                    radius: 0.0,
                },
            ));
        }
    }
    let (_, mut fit) = best?;
    let distances: Vec<f64> = points.iter().map(|p| norm(sub(*p, centre))).collect();
    let mean = distances.iter().sum::<f64>() / n as f64;
    let variance = distances.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    fit.sphericity = variance.sqrt() / mean.max(1e-9);
    fit.radius = mean;
    if fit.axis[2] < 0.0 {
        fit.axis = scale(fit.axis, -1.0);
        (fit.t0, fit.t1) = (-fit.t1, -fit.t0);
        (fit.r0, fit.r1) = (fit.r1, fit.r0);
    }
    Some(fit)
}

/// What primitive a fit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Primitive {
    Sphere,
    Disc,
    Cylinder,
    Cone,
}

fn primitive_kind(fit: &Fit) -> Option<Primitive> {
    // A sphere: points equally far from the centre, on 3 or more rings, as
    // long as it is wide and its widest ring is as wide as the sphere. A
    // cylinder with only its two end rings has its points equally far from
    // the centre too.
    let length = fit.t1 - fit.t0;
    if fit.sphericity < 0.03
        && fit.rings >= 3
        && (length - 2.0 * fit.radius).abs() <= 0.15 * fit.radius
        && fit.rmax >= 0.85 * fit.radius
    {
        return Some(Primitive::Sphere);
    }
    if fit.ring_fraction < 0.8 || fit.residual > 0.03 {
        return None;
    }
    let widest = fit.r0.max(fit.r1);
    if length < 0.6 * widest {
        Some(Primitive::Disc)
    } else if (fit.r0 - fit.r1).abs() <= 0.12 * widest {
        Some(Primitive::Cylinder)
    } else {
        Some(Primitive::Cone)
    }
}

/// A fitted piece: its axis segment and the radius at each end (a sphere is
/// a segment of length 0).
#[derive(Clone, Copy, Debug)]
struct Segment {
    ends: [V3; 2],
    radii: [f64; 2],
    /// Whether each end is the narrow end of a contact cone, which touches
    /// what it holds and joins nothing.
    tip: [bool; 2],
}

impl Segment {
    fn of(kind: Primitive, fit: &Fit) -> Self {
        if kind == Primitive::Sphere {
            return Self {
                ends: [fit.centre; 2],
                radii: [fit.radius; 2],
                tip: [false; 2],
            };
        }
        let cone = kind == Primitive::Cone;
        Self {
            ends: [
                add(fit.centre, scale(fit.axis, fit.t0)),
                add(fit.centre, scale(fit.axis, fit.t1)),
            ],
            radii: [fit.r0, fit.r1],
            tip: [
                cone && fit.r0 <= TIP_RATIO * fit.r1,
                cone && fit.r1 <= TIP_RATIO * fit.r0,
            ],
        }
    }

    fn widest(&self) -> f64 {
        self.radii[0].max(self.radii[1])
    }

    /// The distance from `p` to the axis segment.
    fn distance(&self, p: V3) -> f64 {
        let [a, b] = self.ends;
        let ab = sub(b, a);
        let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-12)).clamp(0.0, 1.0);
        norm(sub(p, add(a, scale(ab, t))))
    }

    fn bounds(&self) -> (V3, V3) {
        let reach = self.widest() + LINK_SLACK_MM;
        let lo = [0, 1, 2].map(|k| self.ends[0][k].min(self.ends[1][k]) - reach);
        let hi = [0, 1, 2].map(|k| self.ends[0][k].max(self.ends[1][k]) + reach);
        (lo, hi)
    }
}

/// Pairs (indices into `segments`) where an end of one, other than a contact
/// tip, is inside the other.
fn link_pairs(segments: &[Segment]) -> Vec<(usize, usize)> {
    let bounds: Vec<(V3, V3)> = segments.iter().map(Segment::bounds).collect();
    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by(|&a, &b| {
        bounds[a].0[0]
            .partial_cmp(&bounds[b].0[0])
            .unwrap_or(Ordering::Equal)
    });
    let mut pairs = Vec::new();
    for (i, &a) in order.iter().enumerate() {
        for &b in &order[i + 1..] {
            if bounds[b].0[0] > bounds[a].1[0] {
                break;
            }
            let apart =
                (0..3).any(|k| bounds[b].0[k] > bounds[a].1[k] || bounds[a].0[k] > bounds[b].1[k]);
            if apart {
                continue;
            }
            let reaches = |x: usize, y: usize| {
                (0..2).any(|e| {
                    !segments[x].tip[e]
                        && segments[y].distance(segments[x].ends[e])
                            <= segments[y].widest() + LINK_SLACK_MM
                })
            };
            if reaches(a, b) || reaches(b, a) {
                pairs.push((a, b));
            }
        }
    }
    pairs
}

/// Union-find over `n` items.
struct Groups(Vec<u32>);

impl Groups {
    fn new(n: usize) -> Self {
        Self((0..n as u32).collect())
    }

    fn find(&mut self, mut i: u32) -> u32 {
        while self.0[i as usize] != i {
            let up = self.0[self.0[i as usize] as usize];
            self.0[i as usize] = up;
            i = up;
        }
        i
    }

    fn join(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb) as usize] = ra.min(rb);
        }
    }
}

/// The piece of every face: faces joined through a shared edge. Two closed
/// shells that touch at one vertex are one shell but two pieces.
fn edge_pieces(faces: &[IndexedTriangle]) -> (Vec<u32>, usize) {
    let mut edges: Vec<(u64, u32)> = faces
        .par_iter()
        .enumerate()
        .flat_map_iter(|(f, face)| {
            let [a, b, c] = face.vertices.map(|v| v.get());
            [(a, b), (b, c), (c, a)].into_iter().map(move |(x, y)| {
                let (lo, hi) = (x.min(y), x.max(y));
                ((u64::from(lo) << 32) | u64::from(hi), f as u32)
            })
        })
        .collect();
    edges.par_sort_unstable();
    let mut groups = Groups::new(faces.len());
    for pair in edges.windows(2) {
        if pair[0].0 == pair[1].0 {
            groups.join(pair[0].1, pair[1].1);
        }
    }
    let mut label = vec![u32::MAX; faces.len()];
    let mut root_label = vec![u32::MAX; faces.len()];
    let mut count = 0u32;
    for (f, label) in label.iter_mut().enumerate() {
        let root = groups.find(f as u32) as usize;
        if root_label[root] == u32::MAX {
            root_label[root] = count;
            count += 1;
        }
        *label = root_label[root];
    }
    (label, count as usize)
}

/// What [`sort`] says of a mesh's shells and pieces.
struct Sorted {
    shell: Vec<Class>,
    piece: Vec<Class>,
    piece_of_face: Vec<u32>,
}

/// Measures of one shell.
#[derive(Clone, Copy, Debug)]
struct ShellMeasure {
    faces: usize,
    volume: f64,
    area: f64,
    min: V3,
    max: V3,
    /// The second spread (standard deviation, largest first) of its surface.
    spread: f64,
}

fn measure_shells(mesh: &IndexedMesh, shells: &FaceShells) -> Vec<ShellMeasure> {
    let count = shells.count() as usize;
    // Sums per shell: area, area-weighted centroid and second moments, about
    // the first vertex to keep them small.
    let origin = mesh.vertices().first().map_or([0.0; 3], point);
    let mut area = vec![0.0f64; count];
    let mut first = vec![[0.0f64; 3]; count];
    let mut second = vec![[0.0f64; 6]; count];
    let mut faces = vec![0usize; count];
    let mut min = vec![[f64::INFINITY; 3]; count];
    let mut max = vec![[f64::NEG_INFINITY; 3]; count];
    for (f, face) in mesh.faces().iter().enumerate() {
        let s = shells.labels()[f] as usize;
        let [a, b, c] = face
            .vertices
            .map(|v| sub(point(&mesh.vertices()[v.get() as usize]), origin));
        let w = norm(cross(sub(b, a), sub(c, a))) / 2.0;
        let centroid = scale(add(add(a, b), c), 1.0 / 3.0);
        faces[s] += 1;
        area[s] += w;
        for k in 0..3 {
            first[s][k] += w * centroid[k];
            min[s][k] = min[s][k].min(a[k]).min(b[k]).min(c[k]);
            max[s][k] = max[s][k].max(a[k]).max(b[k]).max(c[k]);
        }
        let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
        for (j, (x, y)) in pairs.into_iter().enumerate() {
            second[s][j] += w * centroid[x] * centroid[y];
        }
    }
    (0..count)
        .map(|s| {
            let a = area[s].max(1e-30);
            let m = scale(first[s], 1.0 / a);
            let e = |j: usize, x: usize, y: usize| second[s][j] / a - m[x] * m[y];
            let cov = [
                [e(0, 0, 0), e(1, 0, 1), e(2, 0, 2)],
                [e(1, 0, 1), e(3, 1, 1), e(4, 1, 2)],
                [e(2, 0, 2), e(4, 1, 2), e(5, 2, 2)],
            ];
            let (values, _) = symmetric_eigen(cov);
            ShellMeasure {
                faces: faces[s],
                volume: shells.volume(s as u32),
                area: area[s],
                min: add(min[s], origin),
                max: add(max[s], origin),
                spread: values[1].max(0.0).sqrt(),
            }
        })
        .collect()
}

/// The faces of each group of `label`, in face order.
fn grouped(label: &[u32], groups: usize) -> (Vec<u32>, Vec<usize>) {
    let mut starts = vec![0usize; groups + 1];
    for &l in label {
        starts[l as usize + 1] += 1;
    }
    for g in 0..groups {
        starts[g + 1] += starts[g];
    }
    let mut fill = starts.clone();
    let mut order = vec![0u32; label.len()];
    for (f, &l) in label.iter().enumerate() {
        order[fill[l as usize]] = f as u32;
        fill[l as usize] += 1;
    }
    (order, starts)
}

/// The vertices the faces `faces` of `mesh` use, each once.
fn vertices_of(mesh: &IndexedMesh, faces: &[u32]) -> Vec<V3> {
    let mut ids: Vec<u32> = faces
        .iter()
        .flat_map(|&f| mesh.faces()[f as usize].vertices.map(|v| v.get()))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.iter()
        .map(|&v| point(&mesh.vertices()[v as usize]))
        .collect()
}

fn piece_area_volume(mesh: &IndexedMesh, faces: &[u32]) -> (f64, f64) {
    let origin = faces.first().map_or([0.0; 3], |&f| {
        point(&mesh.vertices()[mesh.faces()[f as usize].vertices[0].get() as usize])
    });
    let (mut area, mut volume6) = (0.0, 0.0);
    for &f in faces {
        let [a, b, c] = mesh.faces()[f as usize]
            .vertices
            .map(|v| sub(point(&mesh.vertices()[v.get() as usize]), origin));
        area += norm(cross(sub(b, a), sub(c, a))) / 2.0;
        volume6 += dot(a, cross(b, c));
    }
    (area, (volume6 / 6.0).abs())
}

/// Sorts the shells of `mesh` and their pieces.
fn sort(mesh: &IndexedMesh, shells: &FaceShells) -> Sorted {
    let nb = shells.count() as usize;
    let m = measure_shells(mesh, shells);
    let avol: Vec<f64> = m.iter().map(|s| s.volume.abs()).collect();
    let zmin_all = m.iter().map(|s| s.min[2]).fold(f64::INFINITY, f64::min);
    let zlo: Vec<f64> = m.iter().map(|s| s.min[2] - zmin_all).collect();
    let height: Vec<f64> = m.iter().map(|s| s.max[2] - s.min[2]).collect();
    let foot: Vec<f64> = m
        .iter()
        .map(|s| (s.max[0] - s.min[0]).hypot(s.max[1] - s.min[1]))
        .collect();
    let mut cls = vec![Class::Unsorted; nb];

    // Cavities: inward-facing shells inside a bigger outward one.
    for i in 0..nb {
        if m[i].volume >= 0.0 {
            continue;
        }
        let inside = (0..nb).any(|j| {
            m[j].volume > avol[i]
                && (0..3)
                    .all(|k| m[j].min[k] <= m[i].min[k] + 1e-3 && m[j].max[k] >= m[i].max[k] - 1e-3)
        });
        if inside {
            cls[i] = Class::Cavity;
        }
    }
    for i in 0..nb {
        if cls[i] == Class::Unsorted && (m[i].faces < 4 || avol[i] < 1e-4) {
            cls[i] = Class::Fragment;
        }
    }

    // The raft: on the plate, low, wide next to the biggest shell above it.
    let above = (0..nb).filter(|&i| cls[i] == Class::Unsorted && zlo[i] > PLATE_EPS_MM);
    let reference = above
        .max_by(|&a, &b| avol[a].partial_cmp(&avol[b]).unwrap_or(Ordering::Equal))
        .unwrap_or_else(|| {
            (0..nb)
                .max_by(|&a, &b| avol[a].partial_cmp(&avol[b]).unwrap_or(Ordering::Equal))
                .unwrap_or(0)
        });
    let raft_width = RAFT_MIN_WIDTH_MM.max(RAFT_WIDTH_SHARE * foot[reference]);
    let mut raft_top = zmin_all;
    let mut any_raft = false;
    for i in 0..nb {
        if cls[i] == Class::Unsorted
            && zlo[i] <= PLATE_EPS_MM
            && height[i] <= RAFT_MAX_HEIGHT_MM
            && foot[i] >= raft_width
        {
            cls[i] = Class::Raft;
            raft_top = if any_raft {
                raft_top.max(m[i].max[2])
            } else {
                m[i].max[2]
            };
            any_raft = true;
        }
    }

    // The pieces of the other shells, fitted as primitives.
    let (piece_of_face, pieces) = edge_pieces(mesh.faces());
    let (order, starts) = grouped(&piece_of_face, pieces);
    let mut piece_shell = vec![0usize; pieces];
    for p in 0..pieces {
        if starts[p] < starts[p + 1] {
            piece_shell[p] = shells.labels()[order[starts[p]] as usize] as usize;
        }
    }
    let piece_faces = |p: usize| starts[p + 1] - starts[p];
    let fitted_shell: Vec<bool> = (0..nb)
        .map(|i| cls[i] == Class::Unsorted && m[i].faces <= MAX_FITTED_SHELL_FACES)
        .collect();
    let fits: Vec<Option<(Primitive, Fit)>> = (0..pieces)
        .into_par_iter()
        .map(|p| {
            if !fitted_shell[piece_shell[p]] || piece_faces(p) > MAX_FITTED_PIECE_FACES {
                return None;
            }
            let points = vertices_of(mesh, &order[starts[p]..starts[p + 1]]);
            let fit = revolution_fit(&points)?;
            primitive_kind(&fit).map(|kind| (kind, fit))
        })
        .collect();
    let mut primitive_faces = vec![0usize; nb];
    let mut primitive_rmax = vec![0.0f64; nb];
    for p in 0..pieces {
        if let Some((kind, fit)) = &fits[p] {
            let s = piece_shell[p];
            primitive_faces[s] += piece_faces(p);
            let r = if *kind == Primitive::Sphere {
                fit.radius
            } else {
                fit.rmax
            };
            primitive_rmax[s] = primitive_rmax[s].max(r);
        }
    }

    // Support candidates: small next to the biggest shell left, and either
    // made of primitives no wider than a support, or thin tubes (a support
    // whose tip bends off its shaft is one piece no single axis fits).
    let big_left = (0..nb)
        .filter(|&i| cls[i] == Class::Unsorted)
        .map(|i| avol[i])
        .fold(0.0f64, f64::max);
    let prim: Vec<bool> = (0..nb)
        .map(|i| {
            cls[i] == Class::Unsorted
                && fitted_shell[i]
                && primitive_faces[i] as f64 / m[i].faces.max(1) as f64 >= PRIMITIVE_SHARE
                && primitive_rmax[i] <= MAX_SUPPORT_RADIUS_MM
                && avol[i] <= PRIMITIVES_SHARE_MAX * big_left
        })
        .collect();
    let thin: Vec<bool> = (0..nb)
        .map(|i| {
            cls[i] == Class::Unsorted
                && avol[i] / m[i].area.max(1e-12) <= THIN_VOLUME_PER_AREA_MM
                && m[i].spread <= TUBE_SPREAD_MM
                && avol[i] <= THIN_SHARE * big_left
        })
        .collect();
    let cand: Vec<bool> = (0..nb).map(|i| prim[i] || thin[i]).collect();

    // A support reaches the raft or the plate, itself or through other
    // supports: a lone sphere or horn that only touches the model is model.
    let mut groups = Groups::new(nb);
    let cand_pieces: Vec<usize> = (0..pieces)
        .filter(|&p| fits[p].is_some() && cand[piece_shell[p]])
        .collect();
    let segments: Vec<Segment> = cand_pieces
        .iter()
        .map(|&p| {
            let (kind, fit) = fits[p].expect("a fitted piece");
            Segment::of(kind, &fit)
        })
        .collect();
    for (a, b) in link_pairs(&segments) {
        groups.join(
            piece_shell[cand_pieces[a]] as u32,
            piece_shell[cand_pieces[b]] as u32,
        );
    }
    // A thin shell no axis fits joins the support whose shaft (not a tip)
    // holds its lowest point.
    let shafts: Vec<usize> = (0..segments.len())
        .filter(|&j| !segments[j].tip[0] && !segments[j].tip[1])
        .collect();
    let (shell_order, shell_starts) = grouped(shells.labels(), nb);
    if !shafts.is_empty() {
        for i in (0..nb).filter(|&i| cand[i] && !prim[i]) {
            let points = vertices_of(mesh, &shell_order[shell_starts[i]..shell_starts[i + 1]]);
            let Some(low) = points
                .iter()
                .copied()
                .min_by(|a, b| a[2].partial_cmp(&b[2]).unwrap_or(Ordering::Equal))
            else {
                continue;
            };
            for &j in &shafts {
                let shaft = &segments[j];
                let shell = piece_shell[cand_pieces[j]];
                if shell != i && shaft.distance(low) <= shaft.widest() + LINK_SLACK_MM {
                    groups.join(i as u32, shell as u32);
                }
            }
        }
    }
    let comp: Vec<u32> = (0..nb as u32).map(|i| groups.find(i)).collect();
    let mut base_comp = vec![false; nb];
    for i in 0..nb {
        if cand[i] && m[i].min[2] <= raft_top + BASE_REACH_MM {
            base_comp[comp[i] as usize] = true;
        }
    }
    for i in 0..nb {
        if cand[i] && base_comp[comp[i] as usize] {
            cls[i] = Class::Support;
        }
    }
    let stray: Vec<bool> = (0..nb)
        .map(|i| cand[i] && cls[i] == Class::Unsorted)
        .collect();

    // Model: the biggest of what is left, and any with a fair share of it.
    let top = (0..nb)
        .filter(|&i| cls[i] == Class::Unsorted)
        .max_by(|&a, &b| avol[a].partial_cmp(&avol[b]).unwrap_or(Ordering::Equal));
    if let Some(top) = top {
        let biggest = avol[top].max(1e-12);
        for i in 0..nb {
            if cls[i] == Class::Unsorted && avol[i] / biggest >= MODEL_SHARE {
                cls[i] = Class::Model;
            }
        }
    }
    // Loose supports between two parts of the model: a group of two or more
    // candidates with two cones (tip, connector, tip) that never reaches the
    // base.
    let mut stray_count = vec![0usize; nb];
    for i in (0..nb).filter(|&i| stray[i]) {
        stray_count[comp[i] as usize] += 1;
    }
    let mut cones = vec![0usize; nb];
    for p in 0..pieces {
        if matches!(fits[p], Some((Primitive::Cone, _))) {
            cones[comp[piece_shell[p]] as usize] += 1;
        }
    }
    for i in 0..nb {
        let c = comp[i] as usize;
        if stray[i] && stray_count[c] >= 2 && cones[c] >= 2 {
            cls[i] = Class::Support;
        }
    }
    // What is left and lies within the model's box is part of the model (a
    // separate eye, horn or bead); a shell that is no support candidate (a
    // blob, not a tube or a primitive) above the base and touching that box
    // sticks out of the model: a weapon, a crown, a pet.
    let models: Vec<usize> = (0..nb).filter(|&i| cls[i] == Class::Model).collect();
    if !models.is_empty() {
        let lo = [0, 1, 2].map(|k| {
            models
                .iter()
                .map(|&i| m[i].min[k])
                .fold(f64::INFINITY, f64::min)
                - MODEL_BOX_SLACK_MM
        });
        let hi = [0, 1, 2].map(|k| {
            models
                .iter()
                .map(|&i| m[i].max[k])
                .fold(f64::NEG_INFINITY, f64::max)
                + MODEL_BOX_SLACK_MM
        });
        for i in 0..nb {
            if cls[i] != Class::Unsorted || m[i].min[2] <= raft_top + BASE_REACH_MM {
                continue;
            }
            let within = (0..3).all(|k| m[i].min[k] >= lo[k] && m[i].max[k] <= hi[k]);
            let touches = (0..3).all(|k| m[i].max[k] >= lo[k] && m[i].min[k] <= hi[k]);
            if within || (touches && !cand[i]) {
                cls[i] = Class::Model;
            }
        }
    }

    // Supports welded to the model at one vertex are pieces of the model's
    // shell: small pieces of it that are primitives or thin are supports.
    let mut piece: Vec<Class> = (0..pieces).map(|p| cls[piece_shell[p]]).collect();
    let mut shell_pieces: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for p in 0..pieces {
        if cls[piece_shell[p]] == Class::Model {
            shell_pieces[piece_shell[p]].push(p);
        }
    }
    let welded: Vec<usize> = shell_pieces
        .par_iter()
        .filter(|ps| ps.len() >= 2)
        .flat_map_iter(|ps| {
            let main = *ps
                .iter()
                .max_by_key(|&&p| piece_faces(p))
                .expect("two pieces");
            let limit = (MAX_FITTED_PIECE_FACES as f64).min(0.05 * piece_faces(main) as f64);
            ps.iter()
                .copied()
                .filter(move |&p| p != main && piece_faces(p) as f64 <= limit)
                .filter(|&p| {
                    let faces = &order[starts[p]..starts[p + 1]];
                    let kind = revolution_fit(&vertices_of(mesh, faces))
                        .as_ref()
                        .and_then(primitive_kind);
                    let (area, volume) = piece_area_volume(mesh, faces);
                    kind.is_some() || volume / area.max(1e-12) <= THIN_VOLUME_PER_AREA_MM
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for p in welded {
        piece[p] = Class::Support;
    }
    Sorted {
        shell: cls,
        piece,
        piece_of_face,
    }
}

/// Cuts this far above the lowest point look for the pillars of fused
/// supports, from the first to the last in steps, mm: above a raft, below the
/// model.
const FUSED_CUT_FIRST_MM: f64 = 1.5;
const FUSED_CUT_LAST_MM: f64 = 6.0;
const FUSED_CUT_STEP_MM: f64 = 0.5;
/// A loop of a cut no wider than this is a pillar, mm.
const PILLAR_WIDTH_MM: f64 = 3.0;
/// A cut through this many pillars or more may go through supports.
const FUSED_MIN_PILLARS: usize = 20;
/// At least this share of those pillars is round, like a shaft...
const FUSED_ROUND_SHARE: f64 = 0.7;
/// ...and they hold at least this share of the cut's area: nothing else
/// stands there, only supports. A textured base or a figure's toes fail it.
const FUSED_AREA_SHARE: f64 = 0.9;

/// Whether the supports of `mesh` look fused into its model: a cut a few
/// millimetres above its lowest point goes through many round pillars and
/// nothing else, in one shell [`find_supports`] cannot take apart.
pub fn fused_supports(mesh: &IndexedMesh) -> bool {
    let Some(zmin) = mesh
        .vertices()
        .iter()
        .map(|v| f64::from(v.coords()[2]))
        .reduce(f64::min)
    else {
        return false;
    };
    let steps = ((FUSED_CUT_LAST_MM - FUSED_CUT_FIRST_MM) / FUSED_CUT_STEP_MM).round() as usize;
    (0..=steps).any(|k| {
        let loops = cut_loops(
            mesh,
            zmin + FUSED_CUT_FIRST_MM + k as f64 * FUSED_CUT_STEP_MM,
        );
        let pillars: Vec<&CutLoop> = loops
            .iter()
            .filter(|l| l.width() <= PILLAR_WIDTH_MM)
            .collect();
        let round = pillars.iter().filter(|l| l.round()).count();
        let total: f64 = loops.iter().map(|l| l.area).sum();
        let held: f64 = pillars.iter().map(|l| l.area).sum();
        pillars.len() >= FUSED_MIN_PILLARS
            && round as f64 >= FUSED_ROUND_SHARE * pillars.len() as f64
            && held >= FUSED_AREA_SHARE * total
    })
}

/// A solid loop of a horizontal cut: its box and its area.
#[derive(Clone, Copy, Debug)]
struct CutLoop {
    lo: [f64; 2],
    hi: [f64; 2],
    area: f64,
}

impl CutLoop {
    fn width(&self) -> f64 {
        (self.hi[0] - self.lo[0]).max(self.hi[1] - self.lo[1])
    }

    /// Whether the loop looks like the cut of a round shaft: about as wide
    /// as it is deep, and filling its box like a circle does.
    fn round(&self) -> bool {
        let (w, d) = (self.hi[0] - self.lo[0], self.hi[1] - self.lo[1]);
        if w <= 0.0 || d <= 0.0 {
            return false;
        }
        let fill = self.area / (w * d);
        (w / d).max(d / w) <= 1.4 && (0.65..=0.92).contains(&fill)
    }
}

/// The solid loops of the cut of `mesh` at height `z`. Each face the cut
/// crosses gives a segment, turned so the solid is on its left; a loop's
/// area, summed over its segments, is positive round a solid and negative
/// round a hole.
fn cut_loops(mesh: &IndexedMesh, z: f64) -> Vec<CutLoop> {
    let at = |v: u32| point(&mesh.vertices()[v as usize]);
    let above = |v: u32| at(v)[2] >= z;
    let mut edge_index: HashMap<u64, u32> = HashMap::new();
    let mut points: Vec<[f64; 2]> = Vec::new();
    let mut links: Vec<(u32, u32)> = Vec::new();
    let mut edge_point = |a: u32, b: u32, points: &mut Vec<[f64; 2]>| -> u32 {
        let key = (u64::from(a.min(b)) << 32) | u64::from(a.max(b));
        *edge_index.entry(key).or_insert_with(|| {
            let (pa, pb) = (at(a), at(b));
            let t = ((z - pa[2]) / (pb[2] - pa[2])).clamp(0.0, 1.0);
            points.push([pa[0] + t * (pb[0] - pa[0]), pa[1] + t * (pb[1] - pa[1])]);
            (points.len() - 1) as u32
        })
    };
    for face in mesh.faces() {
        let [a, b, c] = face.vertices.map(|v| v.get());
        let crossing: Vec<(u32, u32)> = [(a, b), (b, c), (c, a)]
            .into_iter()
            .filter(|&(x, y)| above(x) != above(y))
            .collect();
        if let [(a0, b0), (a1, b1)] = crossing[..] {
            let p = edge_point(a0, b0, &mut points);
            let q = edge_point(a1, b1, &mut points);
            // The face's normal, crossed with up, runs along the loop with
            // the solid on its left.
            let normal = cross(sub(at(b), at(a)), sub(at(c), at(a)));
            let along = cross([0.0, 0.0, 1.0], normal);
            let (pp, qq) = (points[p as usize], points[q as usize]);
            if (qq[0] - pp[0]) * along[0] + (qq[1] - pp[1]) * along[1] < 0.0 {
                links.push((q, p));
            } else {
                links.push((p, q));
            }
        }
    }
    let mut groups = Groups::new(points.len());
    for &(p, q) in &links {
        groups.join(p, q);
    }
    let mut loops: HashMap<u32, ([f64; 2], [f64; 2], f64)> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        let root = groups.find(i as u32);
        let e = loops.entry(root).or_insert((*p, *p, 0.0));
        e.0 = [e.0[0].min(p[0]), e.0[1].min(p[1])];
        e.1 = [e.1[0].max(p[0]), e.1[1].max(p[1])];
    }
    for &(p, q) in &links {
        let (pp, qq) = (points[p as usize], points[q as usize]);
        if let Some(e) = loops.get_mut(&groups.find(p)) {
            e.2 += pp[0] * qq[1] - qq[0] * pp[1];
        }
    }
    loops
        .into_values()
        .filter(|(_, _, twice_area)| *twice_area > 0.0)
        .map(|(lo, hi, twice_area)| CutLoop {
            lo,
            hi,
            area: 0.5 * twice_area,
        })
        .collect()
}

#[cfg(test)]
#[path = "tests_presupported.rs"]
mod tests;

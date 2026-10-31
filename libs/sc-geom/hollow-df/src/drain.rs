//! Drain holes: an exact cut of a closed mesh by a convex prism.
//!
//! DragonFruit punches its drains with manifold3d (`punch_cylinders_manifold`,
//! a C++ library its build clones and compiles with CMake) and refuses its own
//! voxel-tunnel fallback. SoulCrafted builds offline and in pure Rust, so the
//! cut is done here, for the one shape a drain needs: a closed edge-manifold
//! mesh `A` minus a convex polyhedron `C` (a cylinder, as a prism of
//! DragonFruit's segment count).
//!
//! The result is edge-manifold by construction, because every vertex the cut
//! creates has a symbolic name computed once:
//!
//! - `V(a)`: a vertex of `A`;
//! - `E(a, b, k)`: edge `ab` of `A` crossing plane `k` of `C`, interpolated
//!   along the canonical edge, so both triangles of the edge get the same point;
//! - `F(t, j, k)`: triangle `t` crossing the line of planes `j` and `k`;
//! - `C(i)`: a corner of `C` inside `A`.
//!
//! Each triangle of `A` touching `C` is clipped against `C`'s planes
//! (Sutherland–Hodgman over named points); the part outside `C` is kept and
//! re-triangulated. The edges of the clipped part that lie on a plane of `C`
//! are the cut curves; on each face of `C` they bound the part of that face
//! inside `A`, which becomes the wall of the hole. A vertex sits on a plane
//! only by name; numerically a point is inside a plane when its signed
//! distance is negative, which settles every tie the same way everywhere.
//! When rounding still breaks the result, the cutter is turned and grown by a
//! hair and the cut retried; when that fails too the error is returned.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use crate::bvh::Bvh;
use crate::hollowing::{compact_unreferenced_vertices, point_inside_mesh_parity, vec3_normalize};
use crate::mesh::{IndexedMesh, Vec3};
use crate::topology::{EdgeManifoldDefects, edge_manifold_defects};

type P3 = [f64; 3];
type P2 = [f64; 2];

#[inline]
fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
fn lerp(a: P3, b: P3, t: f64) -> P3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
#[inline]
fn to64(v: Vec3) -> P3 {
    [v.x as f64, v.y as f64, v.z as f64]
}
#[inline]
fn to32(p: P3) -> Vec3 {
    Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32)
}

/// A plane `n·p = d` with `n` pointing out of the cutter.
#[derive(Debug, Clone, Copy)]
struct Plane {
    n: P3,
    d: f64,
}

impl Plane {
    #[inline]
    fn signed(&self, p: P3) -> f64 {
        dot(self.n, p) - self.d
    }
}

/// A convex polyhedron: face `k` lies on plane `k`, its corners listed
/// counter-clockwise seen from outside.
#[derive(Debug, Clone)]
pub struct ConvexCutter {
    planes: Vec<Plane>,
    corners: Vec<P3>,
    faces: Vec<Vec<u32>>,
    min: P3,
    max: P3,
    /// How it was built, when it is a prism: a retry turns it about its own
    /// axis instead of tilting it.
    prism: Option<Prism>,
}

/// A prism's construction, in `f64`.
#[derive(Debug, Clone, Copy)]
struct Prism {
    origin: P3,
    /// The basis: `u` and `v` across, `w` along the axis.
    u: P3,
    v: P3,
    w: P3,
    radius: [f64; 2],
    length: f64,
    segments: usize,
    phase: f64,
}

impl Prism {
    fn cutter(self) -> ConvexCutter {
        let Prism {
            origin: o,
            u,
            v,
            w,
            radius,
            length,
            segments: segs,
            phase,
        } = self;
        let top = [
            o[0] + w[0] * length,
            o[1] + w[1] * length,
            o[2] + w[2] * length,
        ];
        let mut corners = Vec::with_capacity(segs * 2);
        for i in 0..segs {
            let theta = phase + std::f64::consts::TAU * i as f64 / segs as f64;
            let (s, c) = theta.sin_cos();
            let r = [
                u[0] * c * radius[0] + v[0] * s * radius[1],
                u[1] * c * radius[0] + v[1] * s * radius[1],
                u[2] * c * radius[0] + v[2] * s * radius[1],
            ];
            corners.push([o[0] + r[0], o[1] + r[1], o[2] + r[2]]);
            corners.push([top[0] + r[0], top[1] + r[1], top[2] + r[2]]);
        }
        let b = |i: usize| (2 * (i % segs)) as u32;
        let t = |i: usize| (2 * (i % segs) + 1) as u32;
        let mut faces: Vec<Vec<u32>> = (0..segs)
            .map(|i| vec![b(i), b(i + 1), t(i + 1), t(i)])
            .collect();
        faces.push((0..segs).rev().map(b).collect());
        faces.push((0..segs).map(t).collect());
        let mut cutter = ConvexCutter::from_faces(corners, faces);
        cutter.prism = Some(self);
        cutter
    }

    /// The prism of retry `attempt`: turned about its axis by a fraction of
    /// a side (½, ¼, ¾, ⅛…, so each turn lands between the earlier ones), and
    /// both ends moved out by a micrometre per attempt. The hole is the same
    /// to any print resolution; every corner, side and end is somewhere new.
    fn retry(self, attempt: usize) -> Self {
        let mut fraction = 0.0;
        let (mut bits, mut weight) = (attempt, 0.5);
        while bits > 0 {
            if bits & 1 == 1 {
                fraction += weight;
            }
            bits >>= 1;
            weight *= 0.5;
        }
        let side = std::f64::consts::TAU / self.segments as f64;
        let out = RETRY_END_SHIFT_MM * attempt as f64;
        Prism {
            origin: [
                self.origin[0] - self.w[0] * out,
                self.origin[1] - self.w[1] * out,
                self.origin[2] - self.w[2] * out,
            ],
            length: self.length + 2.0 * out,
            phase: self.phase + side * fraction,
            ..self
        }
    }
}

/// How far each retry moves a prism's ends (see [`Prism::retry`]).
const RETRY_END_SHIFT_MM: f64 = 1e-3;

/// Closer than this, a mesh vertex and a cutter plane, a mesh edge and a
/// cutter edge, or a cutter corner and a mesh triangle count as touching: a
/// cut there would name one point twice (a zero-length edge once rounded to
/// `f32`, or a fold, #330), so the cutter is moved instead. A tenth of a
/// micrometre: above `f32` rounding up to a metre, far below any print
/// resolution.
const GENERAL_POSITION_MM: f64 = 1e-4;

impl ConvexCutter {
    /// A prism over an ellipse with `segments` sides, from `origin` along
    /// `axis` for `length`: DragonFruit's `build_cylinder_mesh` shape and
    /// basis, turned by `phase` radians.
    pub fn prism(
        origin: Vec3,
        axis: Vec3,
        radius_x: f32,
        radius_y: f32,
        length: f32,
        segments: usize,
        phase: f64,
    ) -> Self {
        let axis = vec3_normalize(axis).unwrap_or(Vec3::new(0.0, 0.0, -1.0));
        let helper = if axis.z.abs() < 0.95 {
            Vec3::new(0.0, 0.0, 1.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        };
        let u = to64(vec3_normalize(helper.cross(axis)).unwrap_or(Vec3::new(1.0, 0.0, 0.0)));
        let v = to64(vec3_normalize(axis.cross(to32(u))).unwrap_or(Vec3::new(0.0, 1.0, 0.0)));
        Prism {
            origin: to64(origin),
            u,
            v,
            w: to64(axis),
            radius: [radius_x as f64, radius_y as f64],
            length: length as f64,
            segments: segments.max(8),
            phase,
        }
        .cutter()
    }

    /// Whether `p` is inside, `margin` from every face.
    fn contains(&self, p: P3, margin: f64) -> bool {
        self.planes.iter().all(|pl| pl.signed(p) < -margin)
    }

    /// The cutter edges, as corner pairs.
    fn edges(&self) -> Vec<(u32, u32)> {
        let mut edges = Vec::new();
        for face in &self.faces {
            for c in 0..face.len() {
                let (a, b) = (face[c], face[(c + 1) % face.len()]);
                if a < b {
                    edges.push((a, b));
                }
            }
        }
        edges
    }

    fn from_faces(corners: Vec<P3>, faces: Vec<Vec<u32>>) -> Self {
        let planes = faces
            .iter()
            .map(|face| {
                // Newell's normal, robust for a face that is planar only up
                // to rounding; the offset is the mean over its corners.
                let mut n = [0.0; 3];
                for (i, &a) in face.iter().enumerate() {
                    let p = corners[a as usize];
                    let q = corners[face[(i + 1) % face.len()] as usize];
                    n[0] += (p[1] - q[1]) * (p[2] + q[2]);
                    n[1] += (p[2] - q[2]) * (p[0] + q[0]);
                    n[2] += (p[0] - q[0]) * (p[1] + q[1]);
                }
                let len = dot(n, n).sqrt().max(1e-300);
                let n = [n[0] / len, n[1] / len, n[2] / len];
                let d = face
                    .iter()
                    .map(|&a| dot(n, corners[a as usize]))
                    .sum::<f64>()
                    / face.len() as f64;
                Plane { n, d }
            })
            .collect();
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for c in &corners {
            for k in 0..3 {
                min[k] = min[k].min(c[k]);
                max[k] = max[k].max(c[k]);
            }
        }
        Self {
            planes,
            corners,
            faces,
            min,
            max,
            prism: None,
        }
    }

    /// The cutter as a closed mesh (for previews and tests).
    pub fn to_mesh(&self) -> IndexedMesh {
        let positions = self.corners.iter().map(|&c| to32(c)).collect();
        let mut triangles = Vec::new();
        for face in &self.faces {
            for i in 1..face.len() - 1 {
                triangles.push([face[0], face[i], face[i + 1]]);
            }
        }
        IndexedMesh::new(positions, triangles)
    }
}

/// Why a cut failed.
#[derive(Debug, Clone, PartialEq)]
pub enum CutError {
    /// The mesh to cut is not closed and edge-manifold.
    SourceNotManifold(EdgeManifoldDefects),
    /// Rounding made the cut inconsistent on every attempt.
    Inconsistent(&'static str),
    /// The cut came out with edge defects on every attempt.
    ResultNotManifold(EdgeManifoldDefects),
}

impl std::fmt::Display for CutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CutError::SourceNotManifold(d) => write!(f, "the mesh to cut is not closed ({d:?})"),
            CutError::Inconsistent(why) => write!(f, "the cut is numerically inconsistent: {why}"),
            CutError::ResultNotManifold(d) => write!(f, "the cut mesh is not closed ({d:?})"),
        }
    }
}

impl std::error::Error for CutError {}

/// Subtracts `cutter` from the closed edge-manifold `mesh`. Retries with the
/// cutter turned by a fraction of a segment and grown by a hair when rounding
/// breaks an attempt.
pub fn subtract_convex(mesh: &IndexedMesh, cutter: &ConvexCutter) -> Result<IndexedMesh, CutError> {
    let defects = edge_manifold_defects(mesh);
    if !defects.is_clean() {
        return Err(CutError::SourceNotManifold(defects));
    }
    subtract_convex_closed(mesh, cutter)
}

/// [`subtract_convex`] for a mesh the caller has already proven closed and
/// edge-manifold (the result of an earlier check or cut), which skips that
/// check. The result is still checked.
pub fn subtract_convex_closed(
    mesh: &IndexedMesh,
    cutter: &ConvexCutter,
) -> Result<IndexedMesh, CutError> {
    subtract_convex_attempts(mesh, |attempt| match cutter.prism {
        _ if attempt == 0 => cutter.clone(),
        Some(prism) => prism.retry(attempt).cutter(),
        None => jitter(cutter, attempt),
    })
}

/// Turns the cutter about its centroid by a small irrational angle and grows
/// it by `1e-5` per attempt: enough to move every tie off a plane, far below
/// any print resolution. Prisms are turned about their own axis instead
/// ([`Prism::retry`]).
fn jitter(cutter: &ConvexCutter, attempt: usize) -> ConvexCutter {
    let n = cutter.corners.len() as f64;
    let mut c = [0.0; 3];
    for p in &cutter.corners {
        for k in 0..3 {
            c[k] += p[k] / n;
        }
    }
    let angle = 1e-4 * attempt as f64 * std::f64::consts::SQRT_2;
    let axis = {
        let a = [0.577_350_3, 0.577_350_3 * 1.1, 0.577_350_3 * 0.9];
        let len = dot(a, a).sqrt();
        [a[0] / len, a[1] / len, a[2] / len]
    };
    let grow = 1.0 + 1e-5 * attempt as f64;
    let (s, co) = angle.sin_cos();
    let corners = cutter
        .corners
        .iter()
        .map(|&p| {
            let r = sub(p, c);
            // Rodrigues' rotation.
            let kxr = cross(axis, r);
            let kdr = dot(axis, r);
            let rot = [
                r[0] * co + kxr[0] * s + axis[0] * kdr * (1.0 - co),
                r[1] * co + kxr[1] * s + axis[1] * kdr * (1.0 - co),
                r[2] * co + kxr[2] * s + axis[2] * kdr * (1.0 - co),
            ];
            [
                c[0] + rot[0] * grow,
                c[1] + rot[1] * grow,
                c[2] + rot[2] * grow,
            ]
        })
        .collect();
    ConvexCutter::from_faces(corners, cutter.faces.clone())
}

const ATTEMPTS: usize = 8;

/// Cuts with the cutter of each attempt in turn until one is clean: in
/// general position against the mesh (no tie a rounding could break, #330),
/// closed, and without a sliver or a fold (a new triangle with a
/// zero-length edge, or inside the cutter, over the hole). A cutter not in
/// general position is not even tried, but on the last attempt. When none is
/// clean, the first closed result is returned, as before such checks.
fn subtract_convex_attempts(
    mesh: &IndexedMesh,
    cutter_for: impl Fn(usize) -> ConvexCutter,
) -> Result<IndexedMesh, CutError> {
    // Built only when a corner's side cannot be read from the cut itself.
    let parity = OnceCell::new();
    let mut last = CutError::Inconsistent("no attempt");
    let mut closed: Option<IndexedMesh> = None;
    for attempt in 0..ATTEMPTS {
        let cutter = cutter_for(attempt);
        let cut = Cut::new(mesh, &parity, &cutter);
        let (touching, near) = cut.candidates();
        let general = cut.in_general_position(&touching, &near);
        if !general && attempt + 1 < ATTEMPTS {
            continue;
        }
        match cut.run(&touching) {
            Ok((out, fresh)) => {
                let defects = edge_manifold_defects(&out);
                if !defects.is_clean() {
                    last = CutError::ResultNotManifold(defects);
                    continue;
                }
                if general && clean_triangles(&out, &fresh, &cutter) {
                    return Ok(out);
                }
                closed.get_or_insert(out);
            }
            Err(e) => last = e,
        }
    }
    closed.ok_or(last)
}

/// No triangle of `fresh` (those a cut made, in `out`) has a zero-length
/// edge once in `f32`, which the slicer refuses, nor lies inside `cutter`,
/// which only a fold over the hole does.
fn clean_triangles(out: &IndexedMesh, fresh: &[usize], cutter: &ConvexCutter) -> bool {
    fresh.iter().all(|&t| {
        let [a, b, c] = out.triangles[t].map(|v| out.positions[v as usize]);
        if a == b || b == c || c == a {
            return false;
        }
        let [a, b, c] = [a, b, c].map(to64);
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        !cutter.contains(centroid, GENERAL_POSITION_MM)
    })
}

/// Which cutter failed (its index), and why.
pub type CutFailure = (usize, CutError);

/// Closed edge-manifold shells that may overlap one another, such as infill
/// beams, which cross each other and run into the cavity wall: the solid is
/// their union. One cut across overlapping shells sees a cutter edge enter
/// the solid twice in a row, and its crossings do not alternate (#321);
/// since `(A ∪ B) − C = (A − C) ∪ (B − C)`, the cutters are subtracted from
/// each shell on its own instead. Shells are the edge-connected components
/// of the mesh; only those whose box meets a cutter's are cut.
#[derive(Debug)]
pub struct OverlappingShells<'a> {
    mesh: &'a IndexedMesh,
    /// The shell of each face.
    labels: Vec<u32>,
    /// Each shell's faces, in mesh order.
    faces: Vec<Vec<u32>>,
    bounds: Vec<(P3, P3)>,
}

impl<'a> OverlappingShells<'a> {
    /// Splits `mesh`, whose components are each closed and edge-manifold,
    /// into its shells.
    pub fn new(mesh: &'a IndexedMesh) -> Self {
        let (labels, count) = crate::topology::edge_components(mesh);
        let mut faces = vec![Vec::new(); count];
        let mut bounds = vec![([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]); count];
        for (f, (&label, tri)) in labels.iter().zip(&mesh.triangles).enumerate() {
            faces[label as usize].push(f as u32);
            let (min, max) = &mut bounds[label as usize];
            for &v in tri {
                let p = to64(mesh.positions[v as usize]);
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
        Self {
            mesh,
            labels,
            faces,
            bounds,
        }
    }

    /// How many shells there are.
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    /// Whether there is no shell.
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// The union less every cutter: each shell is cut by the cutters whose
    /// box meets its own, in order, and the shells no cutter reaches stay as
    /// they are, in place. On failure, the index of the first cutter that
    /// failed on some shell, and why.
    pub fn subtract_convex(&self, cutters: &[&ConvexCutter]) -> Result<IndexedMesh, CutFailure> {
        use rayon::prelude::*;
        let results: Vec<(usize, Result<IndexedMesh, CutFailure>)> = (0..self.len())
            .into_par_iter()
            .filter_map(|s| {
                let mut shell: Option<IndexedMesh> = None;
                for (i, cutter) in cutters.iter().enumerate() {
                    if !boxes_meet(self.bounds[s], (cutter.min, cutter.max)) {
                        continue;
                    }
                    let current = shell.get_or_insert_with(|| self.shell_mesh(s));
                    if current.triangles.is_empty() {
                        break; // a cutter took it all
                    }
                    match subtract_convex_closed(current, cutter) {
                        Ok(out) => *current = out,
                        Err(e) => return Some((s, Err((i, e)))),
                    }
                }
                shell.map(|m| (s, Ok(m)))
            })
            .collect();
        if results.is_empty() {
            return Ok(self.mesh.clone());
        }
        let mut failed: Option<CutFailure> = None;
        let mut cut = vec![false; self.len()];
        let mut shells = Vec::with_capacity(results.len());
        for (s, result) in results {
            match result {
                Ok(mesh) => {
                    cut[s] = true;
                    shells.push(mesh);
                }
                Err((i, e)) => {
                    if failed.as_ref().is_none_or(|(j, _)| i < *j) {
                        failed = Some((i, e));
                    }
                }
            }
        }
        if let Some(failure) = failed {
            return Err(failure);
        }
        let triangles = self
            .mesh
            .triangles
            .iter()
            .zip(&self.labels)
            .filter(|&(_, &label)| !cut[label as usize])
            .map(|(&tri, _)| tri)
            .collect();
        let mut out = IndexedMesh::new(self.mesh.positions.clone(), triangles);
        compact_unreferenced_vertices(&mut out);
        for shell in shells {
            let offset = out.positions.len() as u32;
            out.positions.extend_from_slice(&shell.positions);
            out.triangles
                .extend(shell.triangles.iter().map(|t| t.map(|v| v + offset)));
        }
        Ok(out)
    }

    /// Shell `s` as a mesh of its own.
    fn shell_mesh(&self, s: usize) -> IndexedMesh {
        let mut local: HashMap<u32, u32> = HashMap::new();
        let mut positions = Vec::new();
        let triangles = self.faces[s]
            .iter()
            .map(|&f| {
                self.mesh.triangles[f as usize].map(|v| {
                    *local.entry(v).or_insert_with(|| {
                        positions.push(self.mesh.positions[v as usize]);
                        (positions.len() - 1) as u32
                    })
                })
            })
            .collect();
        IndexedMesh::new(positions, triangles)
    }
}

fn boxes_meet(a: (P3, P3), b: (P3, P3)) -> bool {
    (0..3).all(|k| a.0[k] <= b.1[k] && b.0[k] <= a.1[k])
}

/// The box of `points`, grown by `grow`.
fn grown_box(points: &[P3], grow: f64) -> (P3, P3) {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            min[k] = min[k].min(p[k] - grow);
            max[k] = max[k].max(p[k] + grow);
        }
    }
    (min, max)
}

/// The squared distance between segments `pq` and `cd` (Ericson, *Real-Time
/// Collision Detection*, 5.1.9).
fn segments_distance2(p: P3, q: P3, c: P3, d: P3) -> f64 {
    let (d1, d2, r) = (sub(q, p), sub(d, c), sub(p, c));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (s, t) = if a <= f64::MIN_POSITIVE && e <= f64::MIN_POSITIVE {
        (0.0, 0.0)
    } else if a <= f64::MIN_POSITIVE {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let cc = dot(d1, r);
        if e <= f64::MIN_POSITIVE {
            ((-cc / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s = if denom > 0.0 {
                ((b * f - cc * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-cc / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - cc) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    let gap = sub(lerp(p, q, s), lerp(c, d, t));
    dot(gap, gap)
}

/// The squared distance from `p` to triangle `abc` (Ericson, 5.1.5).
fn point_triangle_distance2(p: P3, a: P3, b: P3, c: P3) -> f64 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let closest = if d1 <= 0.0 && d2 <= 0.0 {
        a
    } else {
        let bp = sub(p, b);
        let (d3, d4) = (dot(ab, bp), dot(ac, bp));
        let cp = sub(p, c);
        let (d5, d6) = (dot(ab, cp), dot(ac, cp));
        let vc = d1 * d4 - d3 * d2;
        let vb = d5 * d2 - d1 * d6;
        let va = d3 * d6 - d5 * d4;
        if d3 >= 0.0 && d4 <= d3 {
            b
        } else if d6 >= 0.0 && d5 <= d6 {
            c
        } else if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            lerp(a, b, d1 / (d1 - d3))
        } else if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            lerp(a, c, d2 / (d2 - d6))
        } else if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            lerp(b, c, (d4 - d3) / ((d4 - d3) + (d5 - d6)))
        } else {
            let denom = va + vb + vc;
            if denom.abs() <= f64::MIN_POSITIVE {
                a
            } else {
                let (v, w) = (vb / denom, vc / denom);
                [
                    a[0] + ab[0] * v + ac[0] * w,
                    a[1] + ab[1] * v + ac[1] * w,
                    a[2] + ab[2] * v + ac[2] * w,
                ]
            }
        }
    };
    let gap = sub(p, closest);
    dot(gap, gap)
}

/// A named point of the cut (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    V(u32),
    E(u32, u32, u16),
    F(u32, u16, u16),
    C(u32),
}

impl Key {
    fn on_plane(self, k: u16) -> bool {
        match self {
            Key::E(_, _, p) => p == k,
            Key::F(_, i, j) => i == k || j == k,
            _ => false,
        }
    }

    fn planes(self) -> [Option<u16>; 2] {
        match self {
            Key::E(_, _, p) => [Some(p), None],
            Key::F(_, i, j) => [Some(i), Some(j)],
            _ => [None, None],
        }
    }

    /// The edges of triangle `tri` (canonical) this point lies on.
    fn edges(self, tri: [u32; 3]) -> [Option<(u32, u32)>; 2] {
        match self {
            Key::V(a) => {
                let i = tri.iter().position(|&v| v == a).unwrap_or(0);
                let prev = tri[(i + 2) % 3];
                let next = tri[(i + 1) % 3];
                [Some(canon(a, next)), Some(canon(prev, a))]
            }
            Key::E(a, b, _) => [Some((a, b)), None],
            _ => [None, None],
        }
    }
}

#[inline]
fn canon(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    In,
    On,
    Out,
}

struct Cut<'a> {
    mesh: &'a IndexedMesh,
    parity: &'a OnceCell<Bvh>,
    cutter: &'a ConvexCutter,
    pos: Vec<P3>,
    /// Output index and position of every named point other than `V`.
    named: HashMap<Key, u32>,
    extra: Vec<P3>,
    /// Cut-curve segments per cutter face, oriented with `A`'s inside on the
    /// left seen from outside the cutter.
    segments: Vec<Vec<(Key, Key)>>,
    corner_inside: HashMap<u32, bool>,
}

impl<'a> Cut<'a> {
    fn new(mesh: &'a IndexedMesh, parity: &'a OnceCell<Bvh>, cutter: &'a ConvexCutter) -> Self {
        Self {
            mesh,
            parity,
            cutter,
            pos: mesh.positions.iter().map(|&p| to64(p)).collect(),
            named: HashMap::new(),
            extra: Vec::new(),
            segments: vec![Vec::new(); cutter.planes.len()],
            corner_inside: HashMap::new(),
        }
    }

    fn tri_plane(&self, t: u32) -> Plane {
        let [a, b, c] = self.mesh.triangles[t as usize].map(|v| self.pos[v as usize]);
        let n = cross(sub(b, a), sub(c, a));
        Plane { n, d: dot(n, a) }
    }

    /// The position of a named point, computed from its name only.
    fn compute(&self, key: Key) -> Result<P3, CutError> {
        Ok(match key {
            Key::V(a) => self.pos[a as usize],
            Key::C(i) => self.cutter.corners[i as usize],
            Key::E(a, b, k) => {
                let plane = self.cutter.planes[k as usize];
                let (pa, pb) = (self.pos[a as usize], self.pos[b as usize]);
                let (sa, sb) = (plane.signed(pa), plane.signed(pb));
                let denom = sa - sb;
                let t = if denom.abs() > 0.0 {
                    (sa / denom).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                lerp(pa, pb, t)
            }
            Key::F(t, j, k) => {
                let p0 = self.tri_plane(t);
                let p1 = self.cutter.planes[j as usize];
                let p2 = self.cutter.planes[k as usize];
                let n12 = cross(p1.n, p2.n);
                let det = dot(p0.n, n12);
                let scale = dot(p0.n, p0.n).sqrt();
                if det.abs() <= 1e-12 * scale {
                    return Err(CutError::Inconsistent("triangle parallel to a cutter edge"));
                }
                let a = cross(p2.n, p0.n);
                let b = cross(p0.n, p1.n);
                [
                    (p0.d * n12[0] + p1.d * a[0] + p2.d * b[0]) / det,
                    (p0.d * n12[1] + p1.d * a[1] + p2.d * b[1]) / det,
                    (p0.d * n12[2] + p1.d * a[2] + p2.d * b[2]) / det,
                ]
            }
        })
    }

    /// The output index of a named point, created on first use.
    fn index(&mut self, key: Key) -> Result<u32, CutError> {
        if let Key::V(a) = key {
            return Ok(a);
        }
        if let Some(&i) = self.named.get(&key) {
            return Ok(i);
        }
        let p = self.compute(key)?;
        let i = (self.pos.len() + self.extra.len()) as u32;
        self.extra.push(p);
        self.named.insert(key, i);
        Ok(i)
    }

    fn position(&self, index: u32) -> P3 {
        let i = index as usize;
        if i < self.pos.len() {
            self.pos[i]
        } else {
            self.extra[i - self.pos.len()]
        }
    }

    fn side(&mut self, key: Key, k: u16) -> Result<Side, CutError> {
        if key.on_plane(k) {
            return Ok(Side::On);
        }
        let i = self.index(key)?;
        Ok(
            if self.cutter.planes[k as usize].signed(self.position(i)) < 0.0 {
                Side::In
            } else {
                Side::Out
            },
        )
    }

    fn crossing(p: Key, q: Key, k: u16, t: u32, tri: [u32; 3]) -> Result<Key, CutError> {
        for e in p.edges(tri).into_iter().flatten() {
            if q.edges(tri).contains(&Some(e)) {
                return Ok(Key::E(e.0, e.1, k));
            }
        }
        for j in p.planes().into_iter().flatten() {
            if q.planes().contains(&Some(j)) && j != k {
                return Ok(Key::F(t, j.min(k), j.max(k)));
            }
        }
        Err(CutError::Inconsistent(
            "clipped edge without a common support",
        ))
    }

    /// The triangles that may touch the cutter (in mesh order): every other
    /// one is outside its box or wholly outside one of its planes, and is
    /// kept as it is. Second, those that do not but come within
    /// [`GENERAL_POSITION_MM`] of it, where a tie can still be.
    fn candidates(&self) -> (Vec<u32>, Vec<u32>) {
        let eps = GENERAL_POSITION_MM;
        let (cmin, cmax) = (self.cutter.min, self.cutter.max);
        let planes = &self.cutter.planes;
        let (mut touching, mut near) = (Vec::new(), Vec::new());
        for (t, tri) in self.mesh.triangles.iter().enumerate() {
            let pts = tri.map(|v| self.pos[v as usize]);
            let outside_box = |grow: f64| {
                (0..3).any(|k| {
                    pts.iter().all(|p| p[k] < cmin[k] - grow)
                        || pts.iter().all(|p| p[k] > cmax[k] + grow)
                })
            };
            if outside_box(eps) {
                continue;
            }
            let outside_plane = |grow: f64| {
                planes
                    .iter()
                    .any(|pl| pts.iter().all(|&p| pl.signed(p) >= grow))
            };
            if !outside_box(0.0) && !outside_plane(0.0) {
                touching.push(t as u32);
            } else if !outside_plane(eps) {
                near.push(t as u32);
            }
        }
        (touching, near)
    }

    /// Whether nothing of the mesh near the cutter lies on it by a tie
    /// ([`GENERAL_POSITION_MM`]): no vertex on a face, no edge through a
    /// cutter edge, no cutter corner on a triangle. Ties are what break a
    /// cut: one point gets two names (a sliver whose edge rounds to zero
    /// length) and the triangulation around it can fold over the hole
    /// (#330). Checks the triangles [`Cut::candidates`] found.
    fn in_general_position(&self, touching: &[u32], near: &[u32]) -> bool {
        let eps = GENERAL_POSITION_MM;
        let planes = &self.cutter.planes;
        let corners = &self.cutter.corners;
        let triangles: Vec<[u32; 3]> = touching
            .iter()
            .chain(near)
            .map(|&t| self.mesh.triangles[t as usize])
            .collect();
        // A vertex on a face: on its plane, and within every other.
        let mut seen: HashSet<u32> = HashSet::new();
        for &v in triangles.iter().flatten() {
            if !seen.insert(v) {
                continue;
            }
            let p = self.pos[v as usize];
            let signed: Vec<f64> = planes.iter().map(|pl| pl.signed(p)).collect();
            if signed.iter().all(|&s| s < eps) && signed.iter().any(|&s| s > -eps) {
                return false;
            }
        }
        // An edge through a cutter edge.
        let cutter_edges: Vec<(P3, P3)> = self
            .cutter
            .edges()
            .into_iter()
            .map(|(a, b)| (corners[a as usize], corners[b as usize]))
            .collect();
        let mut edges: HashSet<(u32, u32)> = HashSet::new();
        for tri in &triangles {
            for c in 0..3 {
                let (a, b) = canon(tri[c], tri[(c + 1) % 3]);
                if !edges.insert((a, b)) {
                    continue;
                }
                let (p, q) = (self.pos[a as usize], self.pos[b as usize]);
                if cutter_edges.iter().any(|&(c, d)| {
                    boxes_meet(grown_box(&[p, q], eps), grown_box(&[c, d], 0.0))
                        && segments_distance2(p, q, c, d) < eps * eps
                }) {
                    return false;
                }
            }
        }
        // A cutter corner on a triangle.
        for tri in &triangles {
            let [a, b, d] = tri.map(|v| self.pos[v as usize]);
            let around = grown_box(&[a, b, d], eps);
            for &c in corners {
                if boxes_meet(around, (c, c)) && point_triangle_distance2(c, a, b, d) < eps * eps {
                    return false;
                }
            }
        }
        true
    }

    /// The cut, and which of its triangles it made (indices into it).
    fn run(mut self, candidates: &[u32]) -> Result<(IndexedMesh, Vec<usize>), CutError> {
        let mut out: Vec<[u32; 3]> = Vec::with_capacity(self.mesh.triangles.len());
        let mut fresh: Vec<usize> = Vec::new();
        let mut next = candidates.iter().copied().peekable();
        for (t, &tri) in self.mesh.triangles.iter().enumerate() {
            if next.next_if_eq(&(t as u32)).is_none() {
                out.push(tri);
                continue;
            }
            let start = out.len();
            self.clip_triangle(t as u32, tri, &mut out)?;
            if out[start..] != [tri] {
                fresh.extend(start..out.len());
            }
        }
        self.resolve_corners()?;
        for k in 0..self.cutter.faces.len() {
            let start = out.len();
            self.cap_face(k, &mut out)?;
            fresh.extend(start..out.len());
        }
        let mut positions: Vec<Vec3> = self.mesh.positions.clone();
        positions.extend(self.extra.iter().map(|&p| to32(p)));
        let mut mesh = IndexedMesh::new(positions, out);
        compact_unreferenced_vertices(&mut mesh);
        Ok((mesh, fresh))
    }

    /// Keeps the part of triangle `t` outside the cutter and records its cut
    /// curves.
    fn clip_triangle(
        &mut self,
        t: u32,
        tri: [u32; 3],
        out: &mut Vec<[u32; 3]>,
    ) -> Result<(), CutError> {
        let mut poly: Vec<Key> = tri.iter().map(|&v| Key::V(v)).collect();
        for k in 0..self.cutter.planes.len() as u16 {
            if poly.is_empty() {
                break;
            }
            let sides: Vec<Side> = poly
                .iter()
                .map(|&p| self.side(p, k))
                .collect::<Result<_, _>>()?;
            let mut next = Vec::with_capacity(poly.len() + 2);
            for i in 0..poly.len() {
                let j = (i + 1) % poly.len();
                let (sp, sq) = (sides[i], sides[j]);
                if (sp == Side::In && sq == Side::Out) || (sp == Side::Out && sq == Side::In) {
                    let key = Self::crossing(poly[i], poly[j], k, t, tri)?;
                    next.push(key);
                }
                if sq != Side::Out {
                    next.push(poly[j]);
                }
            }
            next.dedup();
            if next.len() > 1 && next.first() == next.last() {
                next.pop();
            }
            poly = next;
        }
        if poly.len() < 3 {
            out.push(tri);
            return Ok(());
        }
        if poly.len() == 3 && poly.iter().all(|k| matches!(k, Key::V(_))) {
            return Ok(()); // entirely inside the cutter
        }

        // The cut curves: clipped edges on a cutter plane, reversed so the
        // inside of `A` is on their left seen from outside the cutter.
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            for k in p.planes().into_iter().flatten() {
                if q.planes().contains(&Some(k)) {
                    self.segments[k as usize].push((q, p));
                }
            }
        }

        // The outside part: the triangle's boundary less the clipped part's,
        // plus the clipped part's cut edges reversed.
        let inner: HashSet<(Key, Key)> = (0..poly.len())
            .map(|i| (poly[i], poly[(i + 1) % poly.len()]))
            .collect();
        let mut half_edges: Vec<(u32, u32)> = Vec::new();
        for c in 0..3 {
            let (a, b) = (tri[c], tri[(c + 1) % 3]);
            let edge = canon(a, b);
            let (pa, pb) = (self.pos[a as usize], self.pos[b as usize]);
            let dir = sub(pb, pa);
            let mut on_edge: Vec<(f64, Key)> = vec![(0.0, Key::V(a)), (1.0, Key::V(b))];
            for &key in &poly {
                if let Key::E(x, y, _) = key
                    && (x, y) == edge
                {
                    let i = self.index(key)?;
                    let p = self.position(i);
                    on_edge.push((dot(sub(p, pa), dir) / dot(dir, dir).max(1e-300), key));
                }
            }
            on_edge.sort_by(|x, y| x.0.total_cmp(&y.0));
            for w in on_edge.windows(2) {
                if !inner.contains(&(w[0].1, w[1].1)) {
                    half_edges.push((self.index(w[0].1)?, self.index(w[1].1)?));
                }
            }
        }
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let on_plane = p.planes().into_iter().flatten().any(|k| q.on_plane(k));
            if on_plane {
                half_edges.push((self.index(q)?, self.index(p)?));
            }
        }
        let normal = self.tri_plane(t).n;
        self.fill(&half_edges, normal, false, out)
    }

    /// Whether each corner of the cutter is inside `A`, read from the cut:
    /// along a cutter edge the side flips at every crossing, and the crossing
    /// triangle's normal says which way. Corners no crossing reaches share
    /// their neighbours' side; only a corner whose whole edge graph is
    /// uncrossed asks a ray (the one use of a BVH here).
    fn resolve_corners(&mut self) -> Result<(), CutError> {
        // Cutter edges as (corner, corner, face, face), lower corner first.
        let mut edges: Vec<(u32, u32, u16, u16)> = Vec::new();
        for (k, face) in self.cutter.faces.iter().enumerate() {
            for c in 0..face.len() {
                let (a, b) = (face[c], face[(c + 1) % face.len()]);
                if a < b {
                    let Some(j) = self.neighbour_face(k, a, b) else {
                        return Err(CutError::Inconsistent(
                            "cutter edge without a neighbour face",
                        ));
                    };
                    edges.push((a, b, k as u16, j));
                }
            }
        }
        let mut crossings: HashMap<(u16, u16), Vec<Key>> = HashMap::new();
        for segments in &self.segments {
            for &(p, q) in segments {
                for key in [p, q] {
                    if let Key::F(_, i, j) = key {
                        let entry = crossings.entry((i, j)).or_default();
                        if !entry.contains(&key) {
                            entry.push(key);
                        }
                    }
                }
            }
        }
        let mut side: Vec<Option<bool>> = vec![None; self.cutter.corners.len()];
        let set = |side: &mut [Option<bool>], corner: u32, inside: bool| -> Result<(), CutError> {
            match side[corner as usize] {
                Some(s) if s != inside => {
                    Err(CutError::Inconsistent("cutter corner on both sides"))
                }
                _ => {
                    side[corner as usize] = Some(inside);
                    Ok(())
                }
            }
        };
        let mut quiet: Vec<(u32, u32)> = Vec::new();
        for &(a, b, k, j) in &edges {
            let Some(keys) = crossings.get(&(k.min(j), k.max(j))).cloned() else {
                quiet.push((a, b));
                continue;
            };
            let (pa, pb) = (
                self.cutter.corners[a as usize],
                self.cutter.corners[b as usize],
            );
            let dir = sub(pb, pa);
            let mut along: Vec<(f64, f64)> = Vec::with_capacity(keys.len());
            for key in keys {
                let Key::F(t, _, _) = key else { continue };
                let i = self.index(key)?;
                // Positive second: the edge leaves `A` here.
                along.push((
                    dot(sub(self.position(i), pa), dir),
                    dot(self.tri_plane(t).n, dir),
                ));
            }
            along.sort_by(|x, y| x.0.total_cmp(&y.0));
            if along.windows(2).any(|w| (w[0].1 > 0.0) == (w[1].1 > 0.0)) {
                return Err(CutError::Inconsistent(
                    "crossings along a cutter edge do not alternate",
                ));
            }
            set(&mut side, a, along[0].1 > 0.0)?;
            set(&mut side, b, along[along.len() - 1].1 <= 0.0)?;
        }
        let spread = |side: &mut [Option<bool>]| -> Result<(), CutError> {
            loop {
                let mut changed = false;
                for &(a, b) in &quiet {
                    match (side[a as usize], side[b as usize]) {
                        (Some(x), None) => {
                            side[b as usize] = Some(x);
                            changed = true;
                        }
                        (None, Some(x)) => {
                            side[a as usize] = Some(x);
                            changed = true;
                        }
                        (Some(x), Some(y)) if x != y => {
                            return Err(CutError::Inconsistent(
                                "uncrossed cutter edge between sides",
                            ));
                        }
                        _ => {}
                    }
                }
                if !changed {
                    return Ok(());
                }
            }
        };
        spread(&mut side)?;
        for corner in 0..side.len() {
            if side[corner].is_none() {
                let bvh = self.parity.get_or_init(|| Bvh::build(self.mesh));
                let p = to32(self.cutter.corners[corner]);
                side[corner] = Some(point_inside_mesh_parity(self.mesh, bvh, p, 1e-3));
                spread(&mut side)?;
            }
        }
        for (corner, inside) in side.into_iter().enumerate() {
            self.corner_inside
                .insert(corner as u32, inside.unwrap_or(false));
        }
        Ok(())
    }

    /// Fills the part of cutter face `k` inside `A`, facing into the cutter.
    fn cap_face(&mut self, k: usize, out: &mut Vec<[u32; 3]>) -> Result<(), CutError> {
        let face = self.cutter.faces[k].clone();
        let segments = std::mem::take(&mut self.segments[k]);
        let mut half_edges: Vec<(u32, u32)> = Vec::with_capacity(segments.len() * 2);
        for &(p, q) in &segments {
            half_edges.push((self.index(p)?, self.index(q)?));
        }
        // The cut points on each edge of the face, from the segments' ends.
        let mut on_edge: HashMap<(u16, u16), Vec<Key>> = HashMap::new();
        for &(p, q) in &segments {
            for key in [p, q] {
                if let Key::F(_, i, j) = key {
                    let entry = on_edge.entry((i, j)).or_default();
                    if !entry.contains(&key) {
                        entry.push(key);
                    }
                }
            }
        }
        for c in 0..face.len() {
            let (a, b) = (face[c], face[(c + 1) % face.len()]);
            let Some(j) = self.neighbour_face(k, a, b) else {
                return Err(CutError::Inconsistent(
                    "cutter edge without a neighbour face",
                ));
            };
            let pair = ((k as u16).min(j), (k as u16).max(j));
            let (pa, pb) = (
                self.cutter.corners[a as usize],
                self.cutter.corners[b as usize],
            );
            let dir = sub(pb, pa);
            let mut points: Vec<(f64, Key)> = Vec::new();
            for &key in on_edge.get(&pair).map(Vec::as_slice).unwrap_or(&[]) {
                let i = self.index(key)?;
                let p = self.position(i);
                points.push((dot(sub(p, pa), dir) / dot(dir, dir).max(1e-300), key));
            }
            points.sort_by(|x, y| x.0.total_cmp(&y.0));
            let inside_a = self.corner_inside[&a];
            let inside_b = self.corner_inside[&b];
            if (points.len() % 2 == 1) != (inside_a != inside_b) {
                return Err(CutError::Inconsistent(
                    "cut points on a cutter edge do not alternate",
                ));
            }
            let mut chain = vec![Key::C(a)];
            chain.extend(points.iter().map(|&(_, key)| key));
            chain.push(Key::C(b));
            let mut inside = inside_a;
            for w in chain.windows(2) {
                if inside {
                    half_edges.push((self.index(w[0])?, self.index(w[1])?));
                }
                inside = !inside;
            }
        }
        if half_edges.is_empty() {
            return Ok(());
        }
        let normal = self.cutter.planes[k].n;
        self.fill(&half_edges, normal, true, out)
    }

    fn neighbour_face(&self, k: usize, a: u32, b: u32) -> Option<u16> {
        self.cutter.faces.iter().enumerate().find_map(|(j, f)| {
            if j == k {
                return None;
            }
            let n = f.len();
            (0..n)
                .any(|i| f[i] == b && f[(i + 1) % n] == a)
                .then_some(j as u16)
        })
    }

    /// Triangulates the region the half-edges bound (on their left seen
    /// along `normal`), emitting triangles facing `normal`, or facing away
    /// when `reverse`.
    fn fill(
        &self,
        half_edges: &[(u32, u32)],
        normal: P3,
        reverse: bool,
        out: &mut Vec<[u32; 3]>,
    ) -> Result<(), CutError> {
        let project = projector(normal);
        let at = |i: u32| project(self.position(i));
        let loops = trace_loops(half_edges, &at)?;
        for (outer, holes) in group_loops(loops, &at)? {
            for tri in triangulate(outer, holes, &at) {
                out.push(if reverse {
                    [tri[0], tri[2], tri[1]]
                } else {
                    tri
                });
            }
        }
        Ok(())
    }
}

/// 2D coordinates on the plane of `normal`, counter-clockwise seen from the
/// side it points to.
fn projector(normal: P3) -> impl Fn(P3) -> P2 {
    let abs = normal.map(f64::abs);
    let axis = if abs[0] >= abs[1] && abs[0] >= abs[2] {
        0
    } else if abs[1] >= abs[2] {
        1
    } else {
        2
    };
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let flip = normal[axis] < 0.0;
    move |p: P3| if flip { [p[v], p[u]] } else { [p[u], p[v]] }
}

#[inline]
fn orient(a: P2, b: P2, c: P2) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Closed loops of the half-edges, each with its region on the left. Every
/// half-edge's successor is the first edge leaving its end clockwise from
/// the way back, which is a permutation when the boundary is consistent; a
/// loop closes when the walk returns to its first half-edge.
fn trace_loops(
    half_edges: &[(u32, u32)],
    at: &impl Fn(u32) -> P2,
) -> Result<Vec<Vec<u32>>, CutError> {
    let mut outgoing: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, &(a, _)) in half_edges.iter().enumerate() {
        outgoing.entry(a).or_default().push(i);
    }
    let successor = |e: usize| -> Option<usize> {
        let (a, b) = half_edges[e];
        let (pa, pb) = (at(a), at(b));
        let back = (pa[1] - pb[1]).atan2(pa[0] - pb[0]);
        let mut best: Option<(f64, usize)> = None;
        for &o in outgoing.get(&b)? {
            let pc = at(half_edges[o].1);
            let mut cw = back - (pc[1] - pb[1]).atan2(pc[0] - pb[0]);
            while cw <= 0.0 {
                cw += std::f64::consts::TAU;
            }
            while cw > std::f64::consts::TAU {
                cw -= std::f64::consts::TAU;
            }
            if best.is_none_or(|(c, _)| cw < c) {
                best = Some((cw, o));
            }
        }
        best.map(|(_, o)| o)
    };
    let mut used = vec![false; half_edges.len()];
    let mut loops = Vec::new();
    for start in 0..half_edges.len() {
        if used[start] {
            continue;
        }
        let mut ring = Vec::new();
        let mut e = start;
        loop {
            if used[e] {
                return Err(CutError::Inconsistent("cut boundary branches"));
            }
            used[e] = true;
            ring.push(half_edges[e].0);
            match successor(e) {
                Some(next) if next == start => break,
                Some(next) => e = next,
                None => return Err(CutError::Inconsistent("open cut boundary")),
            }
        }
        // A there-and-back pair bounds nothing.
        if ring.len() >= 3 {
            loops.push(ring);
        }
    }
    Ok(loops)
}

fn signed_area(ring: &[u32], at: &impl Fn(u32) -> P2) -> f64 {
    let mut s = 0.0;
    for i in 0..ring.len() {
        let (p, q) = (at(ring[i]), at(ring[(i + 1) % ring.len()]));
        s += p[0] * q[1] - q[0] * p[1];
    }
    s * 0.5
}

fn point_in_ring(p: P2, ring: &[u32], at: &impl Fn(u32) -> P2) -> bool {
    let mut inside = false;
    let n = ring.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (at(ring[i]), at(ring[j]));
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Counter-clockwise loops are outer boundaries; each clockwise loop is a
/// hole of the smallest outer loop around it.
/// An outer loop and its holes.
type Region = (Vec<u32>, Vec<Vec<u32>>);

fn group_loops(loops: Vec<Vec<u32>>, at: &impl Fn(u32) -> P2) -> Result<Vec<Region>, CutError> {
    let mut outers: Vec<(f64, Vec<u32>, Vec<Vec<u32>>)> = Vec::new();
    let mut holes = Vec::new();
    for ring in loops {
        let area = signed_area(&ring, at);
        if area >= 0.0 {
            outers.push((area, ring, Vec::new()));
        } else {
            holes.push(ring);
        }
    }
    for hole in holes {
        // A vertex of the hole not shared with the candidate outer.
        let mut best: Option<usize> = None;
        for (i, (area, outer, _)) in outers.iter().enumerate() {
            let probe = hole
                .iter()
                .find(|v| !outer.contains(v))
                .copied()
                .unwrap_or(hole[0]);
            if point_in_ring(at(probe), outer, at) && best.is_none_or(|b| *area < outers[b].0) {
                best = Some(i);
            }
        }
        match best {
            Some(i) => outers[i].2.push(hole),
            None => return Err(CutError::Inconsistent("cut hole outside its region")),
        }
    }
    Ok(outers
        .into_iter()
        .map(|(_, outer, holes)| (outer, holes))
        .collect())
}

fn segments_cross(a: P2, b: P2, c: P2, d: P2) -> bool {
    let o1 = orient(a, b, c);
    let o2 = orient(a, b, d);
    let o3 = orient(c, d, a);
    let o4 = orient(c, d, b);
    (o1 > 0.0) != (o2 > 0.0)
        && (o3 > 0.0) != (o4 > 0.0)
        && o1 != 0.0
        && o2 != 0.0
        && o3 != 0.0
        && o4 != 0.0
}

/// Splices `hole` into `ring` through a bridge from its rightmost vertex to
/// the nearest ring vertex the bridge can reach without crossing an edge.
fn bridge(ring: &mut Vec<u32>, hole: &[u32], others: &[Vec<u32>], at: &impl Fn(u32) -> P2) {
    let (hi, _) = hole
        .iter()
        .enumerate()
        .max_by(|x, y| at(*x.1)[0].total_cmp(&at(*y.1)[0]))
        .unwrap_or((0, &hole[0]));
    let h = at(hole[hi]);
    let mut candidates: Vec<(f64, usize)> = ring
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let p = at(v);
            ((p[0] - h[0]).powi(2) + (p[1] - h[1]).powi(2), i)
        })
        .collect();
    candidates.sort_by(|x, y| x.0.total_cmp(&y.0));
    let edges_of = |r: &[u32]| -> Vec<(P2, P2)> {
        (0..r.len())
            .map(|i| (at(r[i]), at(r[(i + 1) % r.len()])))
            .collect()
    };
    let mut all_edges = edges_of(ring);
    all_edges.extend(edges_of(hole));
    for o in others {
        all_edges.extend(edges_of(o));
    }
    let chosen = candidates
        .iter()
        .find(|&&(_, i)| {
            let p = at(ring[i]);
            !all_edges.iter().any(|&(a, b)| segments_cross(h, p, a, b))
        })
        .or(candidates.first())
        .map(|&(_, i)| i)
        .unwrap_or(0);
    let mut spliced = Vec::with_capacity(ring.len() + hole.len() + 2);
    spliced.extend_from_slice(&ring[..=chosen]);
    for k in 0..=hole.len() {
        spliced.push(hole[(hi + k) % hole.len()]);
    }
    spliced.extend_from_slice(&ring[chosen..]);
    *ring = spliced;
}

/// Ear-clips an outer loop with holes. Every input vertex is kept (a
/// collinear one too: dropping it would leave a crack in the neighbour), so
/// an ear whose closing diagonal touches another vertex is refused.
fn triangulate(
    outer: Vec<u32>,
    mut holes: Vec<Vec<u32>>,
    at: &impl Fn(u32) -> P2,
) -> Vec<[u32; 3]> {
    let mut ring = outer;
    holes.sort_by(|a, b| {
        let ma = a
            .iter()
            .map(|&v| at(v)[0])
            .fold(f64::NEG_INFINITY, f64::max);
        let mb = b
            .iter()
            .map(|&v| at(v)[0])
            .fold(f64::NEG_INFINITY, f64::max);
        mb.total_cmp(&ma)
    });
    for i in 0..holes.len() {
        let (hole, rest) = (holes[i].clone(), &holes[i + 1..]);
        bridge(&mut ring, &hole, rest, at);
    }

    let mut tris = Vec::with_capacity(ring.len());
    let scale = ring
        .iter()
        .map(|&v| {
            let p = at(v);
            p[0].abs().max(p[1].abs())
        })
        .fold(1.0f64, f64::max);
    let eps = scale * scale * 1e-14;
    while ring.len() > 3 {
        let n = ring.len();
        // Repeats and spikes (`x, y, x`) bound nothing: drop them without a
        // triangle. Dropping a spike's tip leaves a repeat for the next turn.
        if let Some(i) = (0..n).find(|&i| ring[i] == ring[(i + 1) % n]) {
            ring.remove(i);
            continue;
        }
        if let Some(i) = (0..n).find(|&i| ring[(i + n - 1) % n] == ring[(i + 1) % n]) {
            ring.remove(i);
            continue;
        }
        let mut best: Option<(f64, usize)> = None;
        let mut clipped = false;
        for i in 0..n {
            let (ip, ic, iq) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let (p, c, q) = (at(ip), at(ic), at(iq));
            let area = orient(p, c, q);
            if best.is_none_or(|(a, _)| area > a) {
                best = Some((area, i));
            }
            if area <= eps {
                continue;
            }
            let blocked = ring.iter().any(|&v| {
                if v == ip || v == ic || v == iq {
                    return false;
                }
                let x = at(v);
                orient(p, c, x) >= -eps && orient(c, q, x) >= -eps && orient(q, p, x) >= -eps
            });
            if blocked {
                continue;
            }
            tris.push([ip, ic, iq]);
            ring.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // No clean ear (rounding, or a degenerate run): take the most
            // convex corner anyway; topology stays closed.
            let (_, i) = best.unwrap_or((0.0, 0));
            tris.push([ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]]);
            ring.remove(i);
        }
    }
    if ring.len() == 3 && ring[0] != ring[1] && ring[1] != ring[2] && ring[0] != ring[2] {
        tris.push([ring[0], ring[1], ring[2]]);
    }
    tris
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod tests;

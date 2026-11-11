/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where a plane crosses a closed mesh: the loops of the cross-section, and
//! the pieces they make.

use std::collections::HashMap;
use std::sync::Arc;

use manifold_rust::linalg::Vec2;
use manifold_rust::polygon::triangulate_idx;
use manifold_rust::types::{PolyVert, PolygonsIdx};
use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::shells::FaceShells;

use crate::{CutError, Plane, Rect, dot, is_closed};

/// A closed mesh ready to be cut: checked once, and its shells found once,
/// for every plane tried on it.
#[derive(Clone, Debug)]
pub struct CutModel {
    pub(crate) mesh: Arc<IndexedMesh>,
    /// The shell of each face.
    pub(crate) shells: Arc<[u32]>,
}

impl CutModel {
    /// `mesh`, when it is closed ([`is_closed`]).
    pub fn new(mesh: Arc<IndexedMesh>) -> Result<Self, CutError> {
        if !is_closed(&mesh) {
            return Err(CutError::NotClosed);
        }
        let shells = FaceShells::of_mesh(&mesh).labels().into();
        Ok(Self { mesh, shells })
    }

    pub fn mesh(&self) -> &Arc<IndexedMesh> {
        &self.mesh
    }

    /// Where `plane` crosses the mesh.
    pub fn section(&self, plane: Plane) -> Result<Section, CutError> {
        Section::new(self.clone(), plane)
    }
}

/// One closed loop of the cross-section, its points in the direction the
/// cap of the piece above runs.
#[derive(Debug)]
pub(crate) struct Loop {
    pub(crate) points: Vec<u32>,
    pub(crate) shell: u32,
    /// Its signed area in the plane's frame.
    pub(crate) area: f64,
}

/// One piece of the cross-section: an outline and the holes inside it.
#[derive(Clone, Debug)]
pub struct SectionPiece {
    pub(crate) outer: usize,
    pub(crate) holes: Vec<usize>,
    area: f64,
}

impl SectionPiece {
    /// Its area in mm², holes taken out.
    pub fn area_mm2(&self) -> f64 {
        self.area
    }
}

/// Where a piece of the cross-section lies against a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Inside,
    /// The rectangle's edge crosses it: cutting only part of it would leave
    /// a slit, so it cannot be cut.
    Crossed,
    Outside,
}

/// Where a plane crosses a closed mesh.
#[derive(Debug)]
pub struct Section {
    pub(crate) model: CutModel,
    pub(crate) plane: Plane,
    /// Whether each vertex counts as above the plane (on it counts as above).
    pub(crate) above: Vec<bool>,
    /// The points where the plane crosses the mesh's edges.
    pub(crate) points: Vec<[f64; 3]>,
    /// The same points in the plane's frame.
    pub(crate) flat: Vec<[f64; 2]>,
    /// The point on each crossed edge, by its two vertices, lower first.
    pub(crate) edge_point: HashMap<(u32, u32), u32>,
    pub(crate) loops: Vec<Loop>,
    /// The loop each crossed face belongs to.
    pub(crate) face_loop: HashMap<u32, u32>,
    pieces: Vec<SectionPiece>,
}

impl Section {
    fn new(model: CutModel, plane: Plane) -> Result<Self, CutError> {
        let mesh = &model.mesh;
        let above: Vec<bool> = mesh
            .vertices()
            .par_iter()
            .map(|v| plane.distance(v.as_array().map(f64::from)) >= 0.0)
            .collect();
        let crossed: Vec<u32> = mesh
            .faces()
            .par_iter()
            .enumerate()
            .filter(|(_, f)| {
                let s = f.vertices.map(|v| above[v.get() as usize]);
                !(s[0] == s[1] && s[1] == s[2])
            })
            .map(|(k, _)| k as u32)
            .collect();
        if crossed.is_empty() {
            return Err(CutError::Misses);
        }

        let mut points: Vec<[f64; 3]> = Vec::new();
        let mut edge_point: HashMap<(u32, u32), u32> = HashMap::new();
        let mut point = |a: u32, b: u32| -> u32 {
            let key = if a < b { (a, b) } else { (b, a) };
            *edge_point.entry(key).or_insert_with(|| {
                let pa = mesh.vertices()[key.0 as usize].as_array().map(f64::from);
                let pb = mesh.vertices()[key.1 as usize].as_array().map(f64::from);
                let (da, db) = (plane.distance(pa), plane.distance(pb));
                let t = (da / (da - db)).clamp(0.0, 1.0);
                points.push([0, 1, 2].map(|k| pa[k] + (pb[k] - pa[k]) * t));
                (points.len() - 1) as u32
            })
        };
        // Each crossed face gives one segment of the cap of the piece
        // above, which runs opposite that piece's edge on the plane.
        let mut next: HashMap<u32, (u32, u32)> = HashMap::with_capacity(crossed.len());
        for &face in &crossed {
            let t = mesh.faces()[face as usize].vertices.map(|v| v.get());
            let s = t.map(|v| above[v as usize]);
            let i = (0..3)
                .find(|&k| s[k] != s[(k + 1) % 3] && s[k] != s[(k + 2) % 3])
                .expect("a crossed face has a lone vertex");
            let (a, b, c) = (t[i], t[(i + 1) % 3], t[(i + 2) % 3]);
            let (p, q) = (point(a, b), point(a, c));
            let (from, to) = if s[i] { (q, p) } else { (p, q) };
            if next.insert(from, (to, face)).is_some() {
                return Err(CutError::NotClosed);
            }
        }

        let (u, v) = plane.frame();
        let flat: Vec<[f64; 2]> = points.iter().map(|&p| [dot(p, u), dot(p, v)]).collect();
        let mut loops = Vec::new();
        let mut face_loop = HashMap::with_capacity(crossed.len());
        let mut starts: Vec<u32> = next.keys().copied().collect();
        starts.sort_unstable();
        for start in starts {
            if !next.contains_key(&start) {
                continue;
            }
            let index = loops.len() as u32;
            let mut ring = vec![start];
            let mut at = start;
            let shell;
            loop {
                let Some((to, face)) = next.remove(&at) else {
                    return Err(CutError::NotClosed);
                };
                face_loop.insert(face, index);
                if to == start {
                    shell = model.shells[face as usize];
                    break;
                }
                ring.push(to);
                at = to;
            }
            let area = signed_area(&ring, &flat);
            loops.push(Loop {
                points: ring,
                shell,
                area,
            });
        }

        let pieces = pieces_of(&loops, &flat);
        Ok(Self {
            model,
            plane,
            above,
            points,
            flat,
            edge_point,
            loops,
            face_loop,
            pieces,
        })
    }

    pub fn plane(&self) -> Plane {
        self.plane
    }

    pub fn model(&self) -> &CutModel {
        &self.model
    }

    /// The pieces of the cross-section, largest first.
    pub fn pieces(&self) -> &[SectionPiece] {
        &self.pieces
    }

    /// The loops of piece `piece`, its outline first, as points on the mesh.
    pub fn outline(&self, piece: usize) -> Vec<Vec<[f32; 3]>> {
        let piece = &self.pieces[piece];
        std::iter::once(piece.outer)
            .chain(piece.holes.iter().copied())
            .map(|l| {
                self.loops[l]
                    .points
                    .iter()
                    .map(|&p| self.points[p as usize].map(|c| c as f32))
                    .collect()
            })
            .collect()
    }

    /// Triangles that fill piece `piece`, holes left open: points on the
    /// mesh and three indices into them per triangle.
    pub fn fill(&self, piece: usize) -> (Vec<[f32; 3]>, Vec<u32>) {
        let piece = &self.pieces[piece];
        let loops: Vec<usize> = std::iter::once(piece.outer)
            .chain(piece.holes.iter().copied())
            .collect();
        let mirror = self.loops[piece.outer].area < 0.0;
        let mut positions = Vec::new();
        let polys: PolygonsIdx = loops
            .iter()
            .map(|&l| {
                self.loops[l]
                    .points
                    .iter()
                    .map(|&p| {
                        let [x, y] = self.flat[p as usize];
                        positions.push(self.points[p as usize].map(|c| c as f32));
                        PolyVert {
                            pos: Vec2::new(x, if mirror { -y } else { y }),
                            idx: (positions.len() - 1) as i32,
                        }
                    })
                    .collect()
            })
            .collect();
        let indices = triangulate_idx(&polys, -1.0, true)
            .into_iter()
            .flat_map(|t| [t.x as u32, t.y as u32, t.z as u32])
            .collect();
        (positions, indices)
    }

    /// The piece under `point`, a point on the plane: the smallest whose
    /// outline holds it and none of whose holes do.
    pub fn piece_at(&self, point: [f64; 3]) -> Option<usize> {
        let (u, v) = self.plane.frame();
        let at = [dot(point, u), dot(point, v)];
        let mut found: Option<usize> = None;
        for (k, piece) in self.pieces.iter().enumerate() {
            let holds =
                self.inside(at, piece.outer) && !piece.holes.iter().any(|&h| self.inside(at, h));
            if holds && found.is_none_or(|f| self.pieces[f].area > piece.area) {
                found = Some(k);
            }
        }
        found
    }

    /// Where each piece lies against `rect`, in the order of [`Self::pieces`].
    ///
    /// The rectangle lies in a frame of its own on the plane: `to_rect` takes
    /// a point of the mesh to it, and `from_rect` a point of it back to the
    /// mesh. Both are affine, so a model turned or scaled in the scene can
    /// be cut by a rectangle drawn in the scene's frame.
    pub fn fits(
        &self,
        rect: &Rect,
        to_rect: impl Fn([f64; 3]) -> [f64; 2],
        from_rect: impl Fn([f64; 2]) -> [f64; 3],
    ) -> Vec<Fit> {
        let at: Vec<[f64; 2]> = self.points.iter().map(|&p| to_rect(p)).collect();
        let (u, v) = self.plane.frame();
        let corners: Vec<[f64; 2]> = rect
            .corners()
            .iter()
            .map(|&c| {
                let p = from_rect(c);
                [dot(p, u), dot(p, v)]
            })
            .collect();
        self.pieces
            .iter()
            .map(|piece| {
                let ring = &self.loops[piece.outer].points;
                let inside = ring
                    .iter()
                    .filter(|&&p| rect.contains(at[p as usize]))
                    .count();
                if inside == ring.len() {
                    Fit::Inside
                } else if inside > 0
                    || corners.iter().any(|&c| self.inside(c, piece.outer))
                    || (0..ring.len()).any(|k| {
                        let a = at[ring[k] as usize];
                        let b = at[ring[(k + 1) % ring.len()] as usize];
                        crosses(rect, a, b)
                    })
                {
                    Fit::Crossed
                } else {
                    Fit::Outside
                }
            })
            .collect()
    }

    /// Whether `at`, in the plane's frame, lies inside loop `l`.
    fn inside(&self, at: [f64; 2], l: usize) -> bool {
        inside(at, &self.loops[l].points, &self.flat)
    }
}

/// The pieces the loops make: each loop that runs the way the largest one
/// does is an outline, and each other loop a hole of the smallest outline
/// around it. A hole with no outline around it is a piece of its own.
fn pieces_of(loops: &[Loop], flat: &[[f64; 2]]) -> Vec<SectionPiece> {
    let outer_sign = loops
        .iter()
        .map(|l| l.area)
        .max_by(|a, b| a.abs().total_cmp(&b.abs()))
        .map_or(1.0, f64::signum);
    let mut pieces: Vec<SectionPiece> = Vec::new();
    let mut piece_of_outer: HashMap<usize, usize> = HashMap::new();
    for (k, l) in loops.iter().enumerate() {
        if l.area * outer_sign >= 0.0 {
            piece_of_outer.insert(k, pieces.len());
            pieces.push(SectionPiece {
                outer: k,
                holes: Vec::new(),
                area: l.area.abs(),
            });
        }
    }
    for (k, l) in loops.iter().enumerate() {
        if l.area * outer_sign >= 0.0 {
            continue;
        }
        let at = flat[l.points[0] as usize];
        let around = loops
            .iter()
            .enumerate()
            .filter(|(_, outer)| {
                outer.area * outer_sign >= 0.0
                    && outer.area.abs() > l.area.abs()
                    && inside(at, &outer.points, flat)
            })
            .min_by(|a, b| a.1.area.abs().total_cmp(&b.1.area.abs()))
            .map(|(o, _)| o);
        match around {
            Some(o) => {
                let piece = &mut pieces[piece_of_outer[&o]];
                piece.holes.push(k);
                piece.area -= l.area.abs();
            }
            None => pieces.push(SectionPiece {
                outer: k,
                holes: Vec::new(),
                area: l.area.abs(),
            }),
        }
    }
    pieces.sort_by(|a, b| b.area.total_cmp(&a.area));
    pieces
}

fn signed_area(ring: &[u32], flat: &[[f64; 2]]) -> f64 {
    let mut sum = 0.0;
    for k in 0..ring.len() {
        let p = flat[ring[k] as usize];
        let q = flat[ring[(k + 1) % ring.len()] as usize];
        sum += p[0] * q[1] - q[0] * p[1];
    }
    sum / 2.0
}

fn inside(at: [f64; 2], ring: &[u32], flat: &[[f64; 2]]) -> bool {
    let mut holds = false;
    for k in 0..ring.len() {
        let a = flat[ring[k] as usize];
        let b = flat[ring[(k + 1) % ring.len()] as usize];
        if (a[1] > at[1]) != (b[1] > at[1])
            && at[0] < a[0] + (b[0] - a[0]) * (at[1] - a[1]) / (b[1] - a[1])
        {
            holds = !holds;
        }
    }
    holds
}

/// Whether the segment from `a` to `b` passes through `rect` (clipped
/// against it, Liang-Barsky).
fn crosses(rect: &Rect, a: [f64; 2], b: [f64; 2]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for axis in 0..2 {
        let min = rect.center[axis] - rect.half[axis];
        let max = rect.center[axis] + rect.half[axis];
        if d[axis].abs() < 1e-15 {
            if a[axis] < min || a[axis] > max {
                return false;
            }
            continue;
        }
        let (mut t0, mut t1) = ((min - a[axis]) / d[axis], (max - a[axis]) / d[axis]);
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        lo = lo.max(t0);
        hi = hi.min(t1);
        if lo > hi {
            return false;
        }
    }
    true
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Connected shells of a mesh, and the bodies they make.
//!
//! A shell is the faces joined through shared vertices. A mesh can hold
//! several shells that overlap without sharing a vertex: a support tree's
//! tubes and joints, the model, raft and supports of a pre-supported STL. A
//! shell with negative signed volume faces inward: it is a void, such as the
//! sealed cavity of a hollow without a drain.
//!
//! [`FaceShells`] labels every face with its shell; the slicer groups each
//! layer's rings by it. [`Bodies`] groups the shells into the objects that
//! "Split into bodies" makes:
//! - a cavity (a void inside a solid shell) stays with the shell around it,
//!   and so does whatever sits inside the cavity, so a hollow, its infill or a
//!   ball sealed in a sphere never come apart;
//! - fragments (shells under 4 faces, or with almost no volume) go together
//!   into one body of their own, so nothing is ever dropped;
//! - every other shell is a body.

use std::f64::consts::PI;

use rayon::iter::{
    IndexedParallelIterator, IntoParallelIterator, IntoParallelRefIterator, ParallelIterator,
};

use crate::indexed_mesh::{DeclaredFacetNormals, IndexedMesh};
use crate::primitives::{FaceIndex, IndexedTriangle, Vertex};

/// The shell of every face: faces sharing a vertex are in the same shell.
#[derive(Clone, Debug)]
pub struct FaceShells {
    /// Per face, its shell, numbered `0..count` in order of first face.
    labels: Vec<u32>,
    count: u32,
    /// Per shell, 6 times its signed volume (divergence theorem), about the
    /// mesh's first vertex.
    volume6: Vec<f64>,
}

impl FaceShells {
    /// Labels the faces of a mesh by shell. A face index past `vertices`
    /// still joins its shell; its corner counts at the first vertex for the
    /// volume.
    pub fn build(vertices: &[Vertex], faces: &[IndexedTriangle]) -> Self {
        let vertex_count = faces
            .iter()
            .flat_map(|f| f.vertices)
            .map(|v| v.get() as usize + 1)
            .max()
            .unwrap_or(0);
        let mut parent: Vec<u32> = (0..vertex_count as u32).collect();
        for f in faces {
            let [a, b, c] = f.vertices.map(|v| v.get());
            for (x, y) in [(a, b), (b, c)] {
                let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
                if rx != ry {
                    parent[rx.max(ry) as usize] = rx.min(ry);
                }
            }
        }
        let mut shell_of_root: Vec<u32> = vec![u32::MAX; vertex_count];
        let mut count = 0u32;
        let labels = faces
            .iter()
            .map(|f| {
                let r = root(&mut parent, f.vertices[0].get()) as usize;
                if shell_of_root[r] == u32::MAX {
                    shell_of_root[r] = count;
                    count += 1;
                }
                shell_of_root[r]
            })
            .collect::<Vec<u32>>();
        // In f64 about the mesh's first vertex, so far-off models keep
        // precision.
        let origin = vertices
            .first()
            .map_or([0.0; 3], |v| v.coords().map(f64::from));
        let at = |i: u32| relative(vertices, origin, i);
        let mut volume6 = vec![0.0f64; count as usize];
        for (f, &shell) in faces.iter().zip(&labels) {
            let [a, b, c] = f.vertices.map(|v| at(v.get()));
            volume6[shell as usize] += a[0] * (b[1] * c[2] - b[2] * c[1])
                - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
        }
        Self {
            labels,
            count,
            volume6,
        }
    }

    /// Labels the faces of `mesh` by shell.
    pub fn of_mesh(mesh: &IndexedMesh) -> Self {
        Self::build(mesh.vertices(), mesh.faces())
    }

    /// The number of shells.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The shell of `face`.
    pub fn of(&self, face: FaceIndex) -> u32 {
        self.labels[face.get()]
    }

    /// Per face, its shell, numbered `0..count` in order of first face.
    pub fn labels(&self) -> &[u32] {
        &self.labels
    }

    /// The signed volume of `shell`, in mm³: negative when it faces inward.
    pub fn volume(&self, shell: u32) -> f64 {
        self.volume6[shell as usize] / 6.0
    }

    /// Whether `shell` faces inward (a sealed void).
    pub fn is_void(&self, shell: u32) -> bool {
        self.volume6[shell as usize] < 0.0
    }
}

fn root(parent: &mut [u32], mut v: u32) -> u32 {
    while parent[v as usize] != v {
        let up = parent[parent[v as usize] as usize];
        parent[v as usize] = up;
        v = up;
    }
    v
}

/// Vertex `i` in f64, about `origin`; the origin itself when `i` is past
/// `vertices`.
fn relative(vertices: &[Vertex], origin: [f64; 3], i: u32) -> [f64; 3] {
    vertices.get(i as usize).map_or([0.0; 3], |v| {
        let c = v.coords();
        [
            f64::from(c[0]) - origin[0],
            f64::from(c[1]) - origin[1],
            f64::from(c[2]) - origin[2],
        ]
    })
}

/// A shell under this many faces is a fragment: it cannot close a volume.
pub const FRAGMENT_MAX_FACES: usize = 3;

/// A shell whose volume is under this, in mm³, is a fragment (a flat patch,
/// a sliver): a 46 µm cube.
pub const FRAGMENT_MAX_VOLUME_MM3: f64 = 1e-4;

/// How far, in mm, a shell may poke out of another's box and still count as
/// inside it.
const BOX_SLACK_MM: f64 = 1e-3;

/// What a body of [`Bodies`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// A shell, with its cavities and what sits inside them.
    Solid,
    /// Every fragment of the mesh: shells under 4 faces or with almost no
    /// volume.
    Fragments,
}

/// One body of [`Bodies`].
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub kind: BodyKind,
    /// How many faces it holds.
    pub faces: usize,
    /// How many shells it holds, its cavities among them.
    pub shells: usize,
    /// How many of its shells are cavities.
    pub cavities: usize,
    /// Its signed volume, in mm³: its shells' together, so a cavity takes
    /// its own off.
    pub volume: f64,
}

/// The bodies a mesh splits into: its shells, with each cavity, and what it
/// holds, kept in the shell around it, and the fragments together.
///
/// Bodies are numbered by volume, the biggest first, then by first face; the
/// fragments, if any, come last.
#[derive(Clone, Debug)]
pub struct Bodies {
    shells: FaceShells,
    /// Per shell, its body.
    body_of_shell: Vec<u32>,
    bodies: Vec<Body>,
}

impl Bodies {
    /// Groups the shells of a mesh into bodies.
    pub fn find(vertices: &[Vertex], faces: &[IndexedTriangle]) -> Self {
        let shells = FaceShells::build(vertices, faces);
        let n = shells.count() as usize;
        let mut face_count = vec![0usize; n];
        for &s in shells.labels() {
            face_count[s as usize] += 1;
        }
        let boxes = shell_boxes(vertices, faces, &shells);
        let volume: Vec<f64> = (0..n as u32).map(|s| shells.volume(s)).collect();
        let shell_faces = ShellFaces::build(&shells);
        let winding = |shell: usize, point: [f64; 3]| {
            winding_number(vertices, faces, shell_faces.of(shell), point)
        };
        let first_vertex = |shell: usize| {
            let face = faces[shell_faces.of(shell)[0] as usize];
            point(vertices, face.vertices[0].get())
        };

        // Joined shells, by union-find on the shell; the root of a set is its
        // container, the shell the others sit in.
        let mut container: Vec<u32> = (0..n as u32).collect();
        let mut cavity = vec![false; n];
        let small = |s: usize| {
            face_count[s] <= FRAGMENT_MAX_FACES || volume[s].abs() < FRAGMENT_MAX_VOLUME_MM3
        };
        // Solid shells that can hold a cavity or sit in one; a fragment does
        // neither.
        let solids: Vec<usize> = (0..n).filter(|&s| volume[s] > 0.0 && !small(s)).collect();

        // A void goes to the smallest solid shell around it: the one whose box
        // holds its box, with more volume, and whose winding number at the
        // void says the void is inside. With none such by winding (an open
        // shell around it), the smallest box that holds it will do.
        let mut voids_in: Vec<(usize, usize)> = Vec::new();
        for v in (0..n).filter(|&s| volume[s] < 0.0) {
            let mut around: Vec<usize> = solids
                .iter()
                .copied()
                .filter(|&s| volume[s] > -volume[v] && boxes[s].holds(&boxes[v]))
                .collect();
            if around.is_empty() {
                // Turned inside out with nothing around it: a body, not a
                // cavity.
                continue;
            }
            around.sort_by(|&a, &b| volume[a].total_cmp(&volume[b]).then(a.cmp(&b)));
            let p = first_vertex(v);
            let outer = around
                .iter()
                .copied()
                .find(|&s| winding(s, p) > 0.5)
                .unwrap_or(around[0]);
            cavity[v] = true;
            container[v] = outer as u32;
            voids_in.push((v, outer));
        }

        // A solid shell inside a cavity (infill, a ball sealed in a sphere)
        // goes with the cavity's shell.
        for &s in &solids {
            let p = first_vertex(s);
            let mut holders: Vec<&(usize, usize)> = voids_in
                .iter()
                .filter(|(v, outer)| {
                    *outer != s && -volume[*v] > volume[s] && boxes[*v].holds(&boxes[s])
                })
                .collect();
            holders.sort_by(|a, b| volume[b.0].total_cmp(&volume[a.0]).then(a.0.cmp(&b.0)));
            if let Some(&&(_, outer)) = holders.iter().find(|(v, _)| winding(*v, p) < -0.5) {
                container[s] = outer as u32;
            }
        }
        let top = |mut s: usize| {
            // Each link goes to a different shell and none loops: a void
            // only points at a solid, and a solid at the outer shell of a
            // bigger void.
            for _ in 0..n {
                let up = container[s] as usize;
                if up == s {
                    break;
                }
                s = up;
            }
            s
        };

        let fragment = |s: usize| !cavity[s] && small(s);
        // Bodies by their top shell, then numbered by volume.
        let mut body_volume = vec![0.0f64; n];
        let mut body_faces = vec![0usize; n];
        let mut body_shells = vec![0usize; n];
        let mut body_cavities = vec![0usize; n];
        let mut tops = vec![0u32; n];
        let mut fragments = Body {
            kind: BodyKind::Fragments,
            faces: 0,
            shells: 0,
            cavities: 0,
            volume: 0.0,
        };
        for s in 0..n {
            if fragment(s) {
                fragments.faces += face_count[s];
                fragments.shells += 1;
                fragments.volume += volume[s];
                tops[s] = u32::MAX;
                continue;
            }
            let t = top(s);
            tops[s] = t as u32;
            body_volume[t] += volume[s];
            body_faces[t] += face_count[s];
            body_shells[t] += 1;
            body_cavities[t] += usize::from(cavity[s]);
        }
        let mut order: Vec<usize> = (0..n).filter(|&s| tops[s] == s as u32).collect();
        // Shells are numbered in order of first face, so the shell number
        // breaks ties by first face.
        order.sort_by(|&a, &b| body_volume[b].total_cmp(&body_volume[a]).then(a.cmp(&b)));
        let mut number = vec![u32::MAX; n];
        let mut bodies: Vec<Body> = order
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                number[t] = i as u32;
                Body {
                    kind: BodyKind::Solid,
                    faces: body_faces[t],
                    shells: body_shells[t],
                    cavities: body_cavities[t],
                    volume: body_volume[t],
                }
            })
            .collect();
        let fragments_body = bodies.len() as u32;
        if fragments.shells > 0 {
            bodies.push(fragments);
        }
        let body_of_shell = tops
            .iter()
            .map(|&t| {
                if t == u32::MAX {
                    fragments_body
                } else {
                    number[t as usize]
                }
            })
            .collect();
        Self {
            shells,
            body_of_shell,
            bodies,
        }
    }

    /// Groups the shells of `mesh` into bodies.
    pub fn of_mesh(mesh: &IndexedMesh) -> Self {
        Self::find(mesh.vertices(), mesh.faces())
    }

    /// The number of bodies.
    pub fn count(&self) -> usize {
        self.bodies.len()
    }

    /// The bodies, in their order.
    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    /// The shells the bodies come from.
    pub fn shells(&self) -> &FaceShells {
        &self.shells
    }

    /// The body of `face`.
    pub fn of(&self, face: FaceIndex) -> u32 {
        self.body_of_shell[self.shells.of(face) as usize]
    }

    /// The mesh of each body, in body order, cut from `mesh`, the mesh these
    /// bodies were found in. Each keeps its faces and vertices exactly as
    /// they were, in the same order and in the same frame, so nothing moves;
    /// vertices no face uses are left out. Each keeps the mesh's name and
    /// header, and the facet normals the source file declared for its faces.
    pub fn meshes(&self, mesh: &IndexedMesh) -> Vec<IndexedMesh> {
        let count = self.bodies.len();
        let labels = self.shells.labels();
        let body_of_face = |f: usize| self.body_of_shell[labels[f] as usize];
        let mut body_of_vertex = vec![u32::MAX; mesh.vertices.len()];
        for (f, face) in mesh.faces.iter().enumerate() {
            let body = body_of_face(f);
            for v in face.vertices {
                if let Some(slot) = body_of_vertex.get_mut(v.get() as usize) {
                    *slot = body;
                }
            }
        }
        let mut vertex_count = vec![0u32; count];
        for &b in &body_of_vertex {
            if b != u32::MAX {
                vertex_count[b as usize] += 1;
            }
        }
        let mut vertices: Vec<Vec<Vertex>> = vertex_count
            .iter()
            .map(|&c| Vec::with_capacity(c as usize))
            .collect();
        // A vertex's index within its body.
        let mut index = vec![u32::MAX; mesh.vertices.len()];
        for (v, &b) in body_of_vertex.iter().enumerate() {
            if b != u32::MAX {
                let list = &mut vertices[b as usize];
                index[v] = list.len() as u32;
                list.push(mesh.vertices[v]);
            }
        }
        let mut faces: Vec<Vec<IndexedTriangle>> = self
            .bodies
            .iter()
            .map(|b| Vec::with_capacity(b.faces))
            .collect();
        let declared = mesh.declared_facet_normals();
        let mut normals: Option<Vec<Vec<[u32; 3]>>> = declared.map(|_| {
            self.bodies
                .iter()
                .map(|b| Vec::with_capacity(b.faces))
                .collect()
        });
        for (f, face) in mesh.faces.iter().enumerate() {
            let body = body_of_face(f) as usize;
            // An index past the vertices stays past them: the body is as
            // broken as the mesh was there, and no less.
            let remapped = face.vertices.map(|v| {
                index
                    .get(v.get() as usize)
                    .copied()
                    .unwrap_or(u32::MAX)
                    .into()
            });
            faces[body].push(IndexedTriangle { vertices: remapped });
            if let (Some(lists), Some(declared)) = (normals.as_mut(), declared) {
                lists[body].push(declared[f]);
            }
        }
        let meta = mesh.meta().as_ref().map(|m| crate::MeshMetadata {
            name: m.name.clone(),
            header_comment: m.header_comment.clone(),
            declared_facet_normals: None,
        });
        let normals: Vec<Option<Vec<[u32; 3]>>> = match normals {
            Some(lists) => lists.into_iter().map(Some).collect(),
            None => vec![None; count],
        };
        // Binding the normals hashes each body's geometry: in parallel.
        vertices
            .into_par_iter()
            .zip(faces)
            .zip(normals)
            .map(|((vertices, faces), normals)| {
                let mut body = IndexedMesh::new(meta.clone(), vertices, faces);
                if let Some(normals) = normals
                    && let Ok(declared) = DeclaredFacetNormals::capture(&body, normals)
                    && let Some(meta) = body.meta.as_mut()
                {
                    meta.declared_facet_normals = Some(declared);
                }
                body
            })
            .collect()
    }
}

/// The faces of each shell, in ascending order, back to back.
struct ShellFaces {
    start: Vec<usize>,
    faces: Vec<u32>,
}

impl ShellFaces {
    fn build(shells: &FaceShells) -> Self {
        let n = shells.count() as usize;
        let mut start = vec![0usize; n + 1];
        for &s in shells.labels() {
            start[s as usize + 1] += 1;
        }
        for s in 0..n {
            start[s + 1] += start[s];
        }
        let mut next = start.clone();
        let mut faces = vec![0u32; shells.labels().len()];
        for (f, &s) in shells.labels().iter().enumerate() {
            faces[next[s as usize]] = f as u32;
            next[s as usize] += 1;
        }
        Self { start, faces }
    }

    fn of(&self, shell: usize) -> &[u32] {
        &self.faces[self.start[shell]..self.start[shell + 1]]
    }
}

/// A box in f64.
#[derive(Clone, Copy, Debug)]
struct Box3 {
    min: [f64; 3],
    max: [f64; 3],
}

impl Box3 {
    const EMPTY: Self = Self {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };

    fn add(&mut self, p: [f64; 3]) {
        for (k, c) in p.into_iter().enumerate() {
            self.min[k] = self.min[k].min(c);
            self.max[k] = self.max[k].max(c);
        }
    }

    /// Whether `other` lies inside this box, give or take [`BOX_SLACK_MM`].
    fn holds(&self, other: &Self) -> bool {
        (0..3).all(|k| {
            other.min[k] >= self.min[k] - BOX_SLACK_MM && other.max[k] <= self.max[k] + BOX_SLACK_MM
        })
    }
}

fn point(vertices: &[Vertex], i: u32) -> [f64; 3] {
    vertices
        .get(i as usize)
        .map_or([0.0; 3], |v| v.coords().map(f64::from))
}

fn shell_boxes(vertices: &[Vertex], faces: &[IndexedTriangle], shells: &FaceShells) -> Vec<Box3> {
    let mut boxes = vec![Box3::EMPTY; shells.count() as usize];
    for (face, &s) in faces.iter().zip(shells.labels()) {
        for v in face.vertices {
            if let Some(vertex) = vertices.get(v.get() as usize) {
                boxes[s as usize].add(vertex.coords().map(f64::from));
            }
        }
    }
    boxes
}

/// The generalized winding number of the faces `faces` around `p`: about 1
/// inside a closed shell that faces outward, -1 inside one that faces
/// inward, 0 outside. Each face adds its solid angle seen from `p` (Van
/// Oosterom and Strackee), over 4π.
fn winding_number(
    vertices: &[Vertex],
    all_faces: &[IndexedTriangle],
    faces: &[u32],
    p: [f64; 3],
) -> f64 {
    let angle = |&f: &u32| {
        let [a, b, c] = all_faces[f as usize].vertices.map(|v| {
            let q = point(vertices, v.get());
            [q[0] - p[0], q[1] - p[1], q[2] - p[2]]
        });
        let (la, lb, lc) = (len(a), len(b), len(c));
        let det = dot(a, cross(b, c));
        let den = la * lb * lc + dot(a, b) * lc + dot(a, c) * lb + dot(b, c) * la;
        2.0 * det.atan2(den)
    };
    // Only how near the total is to a whole number matters, so the order of
    // the parallel sum does not.
    let total: f64 = if faces.len() < PARALLEL_WINDING_FACES {
        faces.iter().map(angle).sum()
    } else {
        faces.par_iter().map(angle).sum()
    };
    total / (4.0 * PI)
}

/// From this many faces a winding number is summed in parallel.
const PARALLEL_WINDING_FACES: usize = 16_384;

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

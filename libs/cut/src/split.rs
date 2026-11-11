/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cutting at the chosen pieces of a cross-section, and closing the cut.

use std::collections::{HashMap, HashSet};

use manifold_rust::linalg::Vec2;
use manifold_rust::polygon::triangulate_idx;
use manifold_rust::types::{PolyVert, PolygonsIdx};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::section::Section;
use crate::{CutError, dot, edge, fallback, paired};

/// What a cut leaves.
#[derive(Debug)]
pub struct Cut {
    /// The pieces of the model, each closed, the one furthest along the
    /// plane's normal first. Only one when the cut does not split the model.
    pub pieces: Vec<IndexedMesh>,
    /// With a single piece: the pieces of the cross-section left out, which
    /// still join the two sides, by their index in [`Section::pieces`].
    pub joined_by: Vec<usize>,
    /// Whether the cut was done again with Manifold, ours not coming out
    /// closed.
    pub by_fallback: bool,
}

/// Which side of the plane a triangle of the result lies on.
const ABOVE: u8 = 1;
const BELOW: u8 = 2;
/// A face at a piece of the cross-section that was not cut: on both.
const BOTH: u8 = ABOVE | BELOW;

/// The triangles a cut makes, before they are grouped into pieces.
struct Split {
    tris: Vec<[u32; 3]>,
    sides: Vec<u8>,
    shells: Vec<u32>,
}

impl Section {
    /// Cuts the model at the pieces of [`Self::pieces`] `chosen` marks.
    ///
    /// Through the whole plane, the cut leaves two pieces: what lies above
    /// it and what lies below (Split into bodies parts them further). With
    /// only some pieces chosen, the pieces are the parts the cut leaves
    /// apart; a shell the cut does not reach (a separate head, an arm that
    /// crosses the plane where it is left whole) goes with the part it
    /// overlaps most.
    pub fn cut(&self, chosen: &[bool]) -> Result<Cut, CutError> {
        let pieces = self.pieces();
        if !chosen.iter().take(pieces.len()).any(|&c| c) {
            return Err(CutError::NothingChosen);
        }
        let mut loop_chosen = vec![false; self.loops.len()];
        for (piece, _) in pieces.iter().zip(chosen).filter(|(_, c)| **c) {
            loop_chosen[piece.outer] = true;
            for &h in &piece.holes {
                loop_chosen[h] = true;
            }
        }
        let whole = loop_chosen.iter().all(|&c| c);

        let mesh = &self.model.mesh;
        let nv = mesh.vertices().len() as u32;
        let np = self.points.len() as u32;
        let split = self.split(&loop_chosen);
        if !cut_edges_paired(&split.tris, nv) {
            if !whole {
                return Err(CutError::NotClosedAfter);
            }
            return fallback::cut(mesh, self.plane);
        }

        let position = |id: u32| -> [f32; 3] {
            if id < nv {
                *mesh.vertices()[id as usize].as_array()
            } else {
                self.points[((id - nv) % np) as usize].map(|c| c as f32)
            }
        };
        let groups: Vec<Vec<usize>> = if whole {
            [ABOVE, BELOW]
                .into_iter()
                .map(|side| {
                    (0..split.tris.len())
                        .filter(|&t| split.sides[t] == side)
                        .collect::<Vec<_>>()
                })
                .filter(|g| !g.is_empty())
                .collect()
        } else {
            let cut_shells: HashSet<u32> = self
                .loops
                .iter()
                .zip(&loop_chosen)
                .filter(|(_, c)| **c)
                .map(|(l, _)| l.shell)
                .collect();
            group(&split, &cut_shells, (nv + 2 * np) as usize, &position)
        };

        let mut built: Vec<(f64, Vec<[u32; 3]>)> = groups
            .into_iter()
            .map(|members| {
                let faces: Vec<[u32; 3]> = members.iter().map(|&t| split.tris[t]).collect();
                let reach = faces
                    .iter()
                    .flat_map(|f| f.iter())
                    .map(|&v| self.plane.distance(position(v).map(f64::from)))
                    .sum::<f64>()
                    / (3 * faces.len().max(1)) as f64;
                (reach, faces)
            })
            .collect();
        built.sort_by(|a, b| b.0.total_cmp(&a.0));
        let joined_by = if built.len() == 1 {
            (0..pieces.len())
                .filter(|&k| !chosen.get(k).copied().unwrap_or(false))
                .collect()
        } else {
            Vec::new()
        };
        let mut index = vec![u32::MAX; (nv + 2 * np) as usize];
        let pieces = built
            .into_iter()
            .map(|(_, faces)| compact(&faces, &position, &mut index))
            .collect();
        Ok(Cut {
            pieces,
            joined_by,
            by_fallback: false,
        })
    }

    /// The faces split at the chosen loops, and the caps that close them.
    /// A cut point is two vertices of the result, one on each side:
    /// `nv + p` above and `nv + np + p` below.
    fn split(&self, loop_chosen: &[bool]) -> Split {
        let mesh = &self.model.mesh;
        let nv = mesh.vertices().len() as u32;
        let np = self.points.len() as u32;
        let up = |p: u32| nv + p;
        let down = |p: u32| nv + np + p;
        let point = |a: u32, b: u32| self.edge_point[&if a < b { (a, b) } else { (b, a) }];

        let capacity = mesh.faces().len() + 4 * self.points.len();
        let mut out = Split {
            tris: Vec::with_capacity(capacity),
            sides: Vec::with_capacity(capacity),
            shells: Vec::with_capacity(capacity),
        };
        let push = |out: &mut Split, tri: [u32; 3], side: u8, shell: u32| {
            out.tris.push(tri);
            out.sides.push(side);
            out.shells.push(shell);
        };
        for (face, f) in mesh.faces().iter().enumerate() {
            let t = f.vertices.map(|v| v.get());
            let s = t.map(|v| self.above[v as usize]);
            let shell = self.model.shells[face];
            if s[0] == s[1] && s[1] == s[2] {
                push(&mut out, t, if s[0] { ABOVE } else { BELOW }, shell);
                continue;
            }
            if !loop_chosen[self.face_loop[&(face as u32)] as usize] {
                push(&mut out, t, BOTH, shell);
                continue;
            }
            let i = (0..3)
                .find(|&k| s[k] != s[(k + 1) % 3] && s[k] != s[(k + 2) % 3])
                .expect("a crossed face has a lone vertex");
            let (a, b, c) = (t[i], t[(i + 1) % 3], t[(i + 2) % 3]);
            let (p, q) = (point(a, b), point(a, c));
            if s[i] {
                push(&mut out, [a, up(p), up(q)], ABOVE, shell);
                push(&mut out, [down(p), b, c], BELOW, shell);
                push(&mut out, [down(p), c, down(q)], BELOW, shell);
            } else {
                push(&mut out, [a, down(p), down(q)], BELOW, shell);
                push(&mut out, [up(p), b, c], ABOVE, shell);
                push(&mut out, [up(p), c, up(q)], ABOVE, shell);
            }
        }

        // The caps, each shell on its own. Its loops run the way the cap
        // above does; the triangulator wants the outlines counterclockwise.
        let mut by_shell: HashMap<u32, Vec<usize>> = HashMap::new();
        for (l, ring) in self.loops.iter().enumerate() {
            if loop_chosen[l] {
                by_shell.entry(ring.shell).or_default().push(l);
            }
        }
        let mut shell_keys: Vec<u32> = by_shell.keys().copied().collect();
        shell_keys.sort_unstable();
        for shell in shell_keys {
            let loops = &by_shell[&shell];
            let largest = loops
                .iter()
                .map(|&l| self.loops[l].area)
                .max_by(|a, b| a.abs().total_cmp(&b.abs()))
                .unwrap_or(1.0);
            let mirror = largest < 0.0;
            let polys: PolygonsIdx = loops
                .iter()
                .map(|&l| {
                    self.loops[l]
                        .points
                        .iter()
                        .map(|&p| {
                            let [x, y] = self.flat[p as usize];
                            PolyVert {
                                pos: Vec2::new(x, if mirror { -y } else { y }),
                                idx: p as i32,
                            }
                        })
                        .collect()
                })
                .collect();
            for t in triangulate_idx(&polys, -1.0, true) {
                let [a, b, c] = [t.x as u32, t.y as u32, t.z as u32];
                // Counterclockwise in the frame faces along the normal; the
                // cap above faces against it.
                let cap = if mirror { [a, b, c] } else { [a, c, b] };
                push(&mut out, cap.map(up), ABOVE, shell);
                push(&mut out, [cap[0], cap[2], cap[1]].map(down), BELOW, shell);
            }
        }
        out
    }
}

/// Whether the edges at the cut, the ones with a cut point (vertex `nv` or
/// over), each have exactly one partner running the other way. The rest of
/// the mesh was closed and keeps its edges.
fn cut_edges_paired(tris: &[[u32; 3]], nv: u32) -> bool {
    let edges: Vec<u64> = tris
        .iter()
        .flat_map(|t| (0..3).map(move |k| (t[k], t[(k + 1) % 3])))
        .filter(|&(a, b)| a >= nv || b >= nv)
        .map(|(a, b)| edge(a, b))
        .collect();
    paired(edges)
}

/// Groups the triangles of a cut that does not take the whole plane into
/// the pieces it leaves. Triangles that touch make one part. The parts of
/// the shells the cut reaches are the pieces, those of different shells on
/// one side going together when their boxes meet (shells of one model that
/// overlap are one solid there). Each part of a shell the cut does not reach
/// goes with the piece it overlaps most, or stays a piece of its own when it
/// meets none.
fn group(
    split: &Split,
    cut_shells: &HashSet<u32>,
    vertices: usize,
    position: &impl Fn(u32) -> [f32; 3],
) -> Vec<Vec<usize>> {
    let mut parent: Vec<u32> = (0..vertices as u32).collect();
    for t in &split.tris {
        let a = find(&mut parent, t[0]);
        for &v in &t[1..] {
            let b = find(&mut parent, v);
            if a != b {
                parent[b as usize] = a;
            }
        }
    }
    let mut part_of: HashMap<u32, usize> = HashMap::new();
    let mut parts: Vec<Part> = Vec::new();
    for (k, t) in split.tris.iter().enumerate() {
        let root = find(&mut parent, t[0]);
        let part = *part_of.entry(root).or_insert_with(|| {
            parts.push(Part {
                sides: 0,
                shell: split.shells[k],
                lo: [f32::MAX; 3],
                hi: [f32::MIN; 3],
                tris: Vec::new(),
            });
            parts.len() - 1
        });
        let part = &mut parts[part];
        part.sides |= split.sides[k];
        part.tris.push(k);
        for &v in t {
            for (axis, c) in position(v).into_iter().enumerate() {
                part.lo[axis] = part.lo[axis].min(c);
                part.hi[axis] = part.hi[axis].max(c);
            }
        }
    }

    let (reached, others): (Vec<usize>, Vec<usize>) =
        (0..parts.len()).partition(|&p| cut_shells.contains(&parts[p].shell));
    let mut joined: Vec<usize> = (0..parts.len()).collect();
    for (i, &a) in reached.iter().enumerate() {
        for &b in &reached[i + 1..] {
            let (pa, pb) = (&parts[a], &parts[b]);
            if pa.shell != pb.shell && pa.sides & pb.sides != 0 && pa.meets(pb) {
                let (ra, rb) = (root_of(&mut joined, a), root_of(&mut joined, b));
                if ra != rb {
                    joined[rb] = ra;
                }
            }
        }
    }
    for &other in &others {
        let o = &parts[other];
        let best = reached
            .iter()
            .copied()
            .filter(|&r| parts[r].sides & o.sides != 0 && o.meets(&parts[r]))
            .max_by(|&a, &b| {
                o.overlap(&parts[a])
                    .total_cmp(&o.overlap(&parts[b]))
                    .then_with(|| o.gap(&parts[b]).total_cmp(&o.gap(&parts[a])))
            });
        if let Some(best) = best {
            let (ra, rb) = (root_of(&mut joined, best), root_of(&mut joined, other));
            if ra != rb {
                joined[rb] = ra;
            }
        }
    }

    let mut group_of: HashMap<usize, usize> = HashMap::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    for (p, part) in parts.iter().enumerate() {
        let root = root_of(&mut joined, p);
        let g = *group_of.entry(root).or_insert_with(|| {
            members.push(Vec::new());
            members.len() - 1
        });
        members[g].extend(part.tris.iter().copied());
    }
    members
}

/// Triangles of the result that touch: which sides they lie on, their shell
/// and their box.
struct Part {
    sides: u8,
    shell: u32,
    lo: [f32; 3],
    hi: [f32; 3],
    tris: Vec<usize>,
}

impl Part {
    fn meets(&self, other: &Self) -> bool {
        (0..3).all(|k| self.lo[k] <= other.hi[k] && other.lo[k] <= self.hi[k])
    }

    /// The volume the two boxes share.
    fn overlap(&self, other: &Self) -> f64 {
        (0..3)
            .map(|k| {
                f64::from((self.hi[k].min(other.hi[k]) - self.lo[k].max(other.lo[k])).max(0.0))
            })
            .product()
    }

    /// How far apart the two boxes' centres are, squared.
    fn gap(&self, other: &Self) -> f64 {
        (0..3)
            .map(|k| {
                let d = f64::from((self.lo[k] + self.hi[k]) - (other.lo[k] + other.hi[k])) / 2.0;
                d * d
            })
            .sum()
    }
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

fn root_of(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// `faces` as a mesh of its own, each vertex once, in the order they first
/// come. `index` is scratch, all `u32::MAX`, and is left so.
fn compact(
    faces: &[[u32; 3]],
    position: &impl Fn(u32) -> [f32; 3],
    index: &mut [u32],
) -> IndexedMesh {
    let mut used = Vec::new();
    let mut vertices = Vec::new();
    let faces = faces
        .iter()
        .map(|f| {
            IndexedTriangle::from_raw_indices(f.map(|v| {
                let slot = &mut index[v as usize];
                if *slot == u32::MAX {
                    *slot = vertices.len() as u32;
                    vertices.push(Vertex::new(position(v)));
                    used.push(v);
                }
                *slot
            }))
        })
        .collect();
    for v in used {
        index[v as usize] = u32::MAX;
    }
    IndexedMesh::new(None, vertices, faces)
}

/// The volume `mesh` bounds, in mm³.
pub fn volume(mesh: &IndexedMesh) -> f64 {
    let v = mesh.vertices();
    mesh.faces()
        .iter()
        .map(|f| {
            let [a, b, c] = f
                .vertices
                .map(|i| v[i.get() as usize].as_array().map(f64::from));
            dot(a, crate::cross(b, c)) / 6.0
        })
        .sum()
}

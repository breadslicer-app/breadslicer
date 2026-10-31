/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The synthetic repair corpus: 18 meshes, each a clean solid with 1
//! controlled defect, to measure how many meshes Repair makes sliceable.
//!
//! Most are built on a sphere of radius 10 centred at (0, 0, 10), subdivided 5
//! times from an icosahedron (20 480 faces). Each builder is deterministic: the
//! random picks come from a fixed-seed generator, so every run measures the
//! same meshes. Coordinates are rounded to `f32` and exactly equal ones are
//! welded, as the STL reader does on load, so 2 parts that touch share their
//! vertices (that is what makes 2 cubes sharing an edge or a face
//! non-manifold; the slicer separates them on its own copy, so they pass its
//! gate).
//!
//! [`RepairExpectation`] records what each mesh is as built: 10 fail the
//! slicer's gate ([`IndexedMesh::is_sliceable`]), 6 pass it and are right,
//! and 2 pass it but print wrong (a mesh inside out, a cavity wound outward),
//! which the sliceable metric cannot see.

use std::collections::HashMap;

use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};

/// What a corpus mesh is before any repair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairExpectation {
    /// Passes the slicer's gate and prints as intended.
    Sliceable,
    /// Passes the slicer's gate but prints wrong: the defect is in the
    /// winding of a whole closed shell, which the gate does not judge.
    SliceableButWrong,
    /// Fails the slicer's gate: slicing a plate with it fails.
    Broken,
}

/// 1 mesh of the synthetic repair corpus.
#[derive(Clone, Copy, Debug)]
pub struct RepairFixture {
    /// Stable short name, `sNN-what`, also set as the mesh name.
    pub name: &'static str,
    /// The defect, in a few words.
    pub defect: &'static str,
    /// What the mesh is before any repair.
    pub expectation: RepairExpectation,
    build: fn() -> Soup,
}

impl RepairFixture {
    /// Builds the mesh, named after the fixture.
    pub fn mesh(&self) -> IndexedMesh {
        crate::named((self.build)().into_mesh(), self.name)
    }
}

/// Returns the 18 fixtures of the synthetic repair corpus, in name order.
pub fn repair_corpus() -> [RepairFixture; 18] {
    use RepairExpectation::{Broken, Sliceable, SliceableButWrong};
    let fixture = |name, defect, expectation, build| RepairFixture {
        name,
        defect,
        expectation,
        build,
    };
    [
        fixture("s01-clean-sphere", "none", Sliceable, clean_sphere),
        fixture(
            "s02-small-hole",
            "0.2 % of faces missing on top",
            Broken,
            small_hole,
        ),
        fixture(
            "s03-big-hole-20pct",
            "20 % of the surface missing",
            Broken,
            big_hole,
        ),
        fixture(
            "s04-many-holes-2pct",
            "2 % of faces missing at random",
            Broken,
            many_holes,
        ),
        fixture(
            "s05-flipped-patch-30pct",
            "a patch of 30 % of faces inverted",
            Broken,
            flipped_patch_30,
        ),
        fixture(
            "s06-flipped-patch-60pct",
            "a patch of 60 % of faces inverted",
            Broken,
            flipped_patch_60,
        ),
        fixture(
            "s07-inside-out",
            "every face inverted",
            SliceableButWrong,
            inside_out,
        ),
        fixture(
            "s08-flipped-scattered-5pct",
            "5 % of faces inverted at random",
            Broken,
            flipped_scattered,
        ),
        fixture(
            "s09-cubes-sharing-edge",
            "2 cubes sharing an edge",
            Sliceable,
            cubes_sharing_edge,
        ),
        fixture(
            "s10-cubes-sharing-face",
            "2 cubes sharing a face",
            Sliceable,
            cubes_sharing_face,
        ),
        fixture(
            "s11-duplicate-faces",
            "10 % of faces duplicated",
            Broken,
            duplicate_faces,
        ),
        fixture(
            "s12-fin-on-edge",
            "a single face glued to an edge of a cube",
            Broken,
            fin_on_edge,
        ),
        fixture("s13-crack-1um", "the halves 1 um apart", Broken, crack_1um),
        fixture(
            "s14-crack-50um",
            "the halves 50 um apart",
            Broken,
            crack_50um,
        ),
        fixture(
            "s15-overlapping-spheres",
            "2 closed spheres overlapping",
            Sliceable,
            overlapping_spheres,
        ),
        fixture(
            "s16-loose-specks",
            "30 loose 0.05 mm cubes",
            Sliceable,
            loose_specks,
        ),
        fixture(
            "s17-hollow-cavity-outward",
            "a cavity wound outward",
            SliceableButWrong,
            hollow_cavity_outward,
        ),
        fixture(
            "s18-hollow-cavity-ok",
            "a cavity wound inward",
            Sliceable,
            hollow_cavity_ok,
        ),
    ]
}

/// Triangles over `f64` positions, before welding.
#[derive(Clone, Default)]
struct Soup {
    positions: Vec<[f64; 3]>,
    faces: Vec<[u32; 3]>,
}

impl Soup {
    /// Appends `other`, offsetting its indices; nothing is shared until the
    /// weld.
    fn append(mut self, other: Soup) -> Soup {
        let offset = self.positions.len() as u32;
        self.positions.extend(other.positions);
        self.faces
            .extend(other.faces.into_iter().map(|f| f.map(|i| i + offset)));
        self
    }

    /// Returns the soup with the faces at `indices` reversed.
    fn flip(mut self, indices: &[usize]) -> Soup {
        for &i in indices {
            self.faces[i].reverse();
        }
        self
    }

    /// Returns the soup without the faces at `indices`.
    fn without(mut self, indices: &[usize]) -> Soup {
        let mut drop = vec![false; self.faces.len()];
        for &i in indices {
            drop[i] = true;
        }
        let mut k = 0;
        self.faces.retain(|_| {
            k += 1;
            !drop[k - 1]
        });
        self
    }

    /// Rounds positions to `f32`, welds exactly equal ones (the first
    /// occurrence keeps its index order), drops unused positions and keeps
    /// the face order.
    fn into_mesh(self) -> IndexedMesh {
        let mut index_of: HashMap<[u32; 3], u32> = HashMap::new();
        let mut vertices = Vec::new();
        let mut remap = vec![None; self.positions.len()];
        let faces = self
            .faces
            .iter()
            .map(|face| {
                IndexedTriangle::from_raw_indices(face.map(|i| {
                    *remap[i as usize].get_or_insert_with(|| {
                        // `+ 0.0` turns -0.0 into 0.0, so both weld.
                        let p = self.positions[i as usize].map(|c| c as f32 + 0.0);
                        *index_of.entry(p.map(f32::to_bits)).or_insert_with(|| {
                            vertices.push(Vertex::new(p));
                            (vertices.len() - 1) as u32
                        })
                    })
                }))
            })
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }
}

/// Returns an icosahedron of radius `r` centred at `c`, subdivided `level`
/// times with every new vertex pushed onto the sphere.
fn icosphere(level: u32, r: f64, c: [f64; 3]) -> Soup {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut unit: Vec<[f64; 3]> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .into_iter()
    .map(normalized)
    .collect();
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..level {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |a: u32, b: u32| {
            *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let (pa, pb) = (unit[a as usize], unit[b as usize]);
                unit.push(normalized([pa[0] + pb[0], pa[1] + pb[1], pa[2] + pb[2]]));
                (unit.len() - 1) as u32
            })
        };
        faces = faces
            .iter()
            .flat_map(|&[a, b, cc]| {
                let (ab, bc, ca) = (mid(a, b), mid(b, cc), mid(cc, a));
                [[a, ab, ca], [b, bc, ab], [cc, ca, bc], [ab, bc, ca]]
            })
            .collect();
    }
    Soup {
        positions: unit
            .into_iter()
            .map(|p| [p[0] * r + c[0], p[1] * r + c[1], p[2] * r + c[2]])
            .collect(),
        faces,
    }
}

fn normalized(p: [f64; 3]) -> [f64; 3] {
    let n = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
    p.map(|c| c / n)
}

/// Returns an axis-aligned box from `lo` to `hi`, wound outward. Vertex `i`
/// takes `hi` on the axes whose bit is set in `i` (bit 0 = x, 1 = y, 2 = z).
fn cuboid(lo: [f64; 3], hi: [f64; 3]) -> Soup {
    let positions = (0..8)
        .map(|i| {
            std::array::from_fn(|axis| {
                if i >> axis & 1 == 1 {
                    hi[axis]
                } else {
                    lo[axis]
                }
            })
        })
        .collect();
    Soup {
        positions,
        faces: vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ],
    }
}

/// The base sphere: radius 10 at (0, 0, 10), 20 480 faces.
fn sphere() -> Soup {
    icosphere(5, 10.0, [0.0, 0.0, 10.0])
}

/// Returns the indices of the `frac` share of faces whose centroids reach
/// furthest along `dir`: a connected cap. Ties keep the lower index first.
fn cap(soup: &Soup, dir: [f64; 3], frac: f64) -> Vec<usize> {
    let reach: Vec<f64> = soup
        .faces
        .iter()
        .map(|f| {
            (0..3)
                .map(|axis| {
                    f.iter()
                        .map(|&i| soup.positions[i as usize][axis])
                        .sum::<f64>()
                        / 3.0
                        * dir[axis]
                })
                .sum()
        })
        .collect();
    let mut order: Vec<usize> = (0..soup.faces.len()).collect();
    order.sort_by(|&a, &b| reach[b].total_cmp(&reach[a]).then(a.cmp(&b)));
    order.truncate((soup.faces.len() as f64 * frac) as usize);
    order
}

/// A fixed-seed SplitMix64 generator, so the random picks never change.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A uniform value in `[lo, hi)`.
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * (hi - lo)
    }

    /// `k` distinct indices below `n`, in ascending order.
    fn choose(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut pool: Vec<usize> = (0..n).collect();
        for i in 0..k {
            let j = i + (self.next_u64() % (n - i) as u64) as usize;
            pool.swap(i, j);
        }
        pool.truncate(k);
        pool.sort_unstable();
        pool
    }
}

fn clean_sphere() -> Soup {
    sphere()
}

fn small_hole() -> Soup {
    let s = sphere();
    let hole = cap(&s, [0.0, 0.0, 1.0], 0.002);
    s.without(&hole)
}

fn big_hole() -> Soup {
    let s = sphere();
    let hole = cap(&s, [0.0, 0.0, 1.0], 0.2);
    s.without(&hole)
}

fn many_holes() -> Soup {
    let s = sphere();
    let missing = Rng::new(2).choose(s.faces.len(), s.faces.len() / 50);
    s.without(&missing)
}

fn flipped_patch_30() -> Soup {
    let s = sphere();
    let patch = cap(&s, [1.0, 0.0, 0.0], 0.3);
    s.flip(&patch)
}

fn flipped_patch_60() -> Soup {
    let s = sphere();
    let patch = cap(&s, [1.0, 0.0, 0.0], 0.6);
    s.flip(&patch)
}

fn inside_out() -> Soup {
    let s = sphere();
    let all: Vec<usize> = (0..s.faces.len()).collect();
    s.flip(&all)
}

fn flipped_scattered() -> Soup {
    let s = sphere();
    let flipped = Rng::new(8).choose(s.faces.len(), s.faces.len() / 20);
    s.flip(&flipped)
}

fn cubes_sharing_edge() -> Soup {
    cuboid([0.0; 3], [10.0; 3]).append(cuboid([10.0, 10.0, 0.0], [20.0, 20.0, 10.0]))
}

fn cubes_sharing_face() -> Soup {
    cuboid([0.0; 3], [10.0; 3]).append(cuboid([10.0, 0.0, 0.0], [20.0, 10.0, 10.0]))
}

fn duplicate_faces() -> Soup {
    let mut s = sphere();
    let copies = s.faces[..s.faces.len() / 10].to_vec();
    s.faces.extend(copies);
    s
}

/// A cube with 1 extra face on its top edge from (0, 0, 10) to (0, 10, 10),
/// sticking out like a fin.
fn fin_on_edge() -> Soup {
    let mut s = cuboid([0.0; 3], [10.0; 3]);
    s.positions.push([-5.0, 5.0, 15.0]);
    s.faces.push([4, 6, 8]);
    s
}

/// The sphere cut at its equator: the lower half moved `gap` mm along X.
fn crack(gap: f64) -> Soup {
    let s = sphere();
    let n = s.positions.len() as u32;
    let mut positions = s.positions.clone();
    positions.extend(s.positions.iter().map(|p| [p[0] + gap, p[1], p[2]]));
    let faces = s
        .faces
        .iter()
        .map(|f| {
            let centroid_z = f.iter().map(|&i| s.positions[i as usize][2]).sum::<f64>() / 3.0;
            if centroid_z > 10.0 {
                *f
            } else {
                f.map(|i| i + n)
            }
        })
        .collect();
    Soup { positions, faces }
}

fn crack_1um() -> Soup {
    crack(0.001)
}

fn crack_50um() -> Soup {
    crack(0.05)
}

fn overlapping_spheres() -> Soup {
    sphere().append(icosphere(4, 6.0, [8.0, 0.0, 10.0]))
}

fn loose_specks() -> Soup {
    let mut rng = Rng::new(16);
    (0..30).fold(sphere(), |s, _| {
        let lo = [
            rng.uniform(-15.0, 15.0),
            rng.uniform(-15.0, 15.0),
            rng.uniform(-15.0, 15.0) + 10.0,
        ];
        s.append(cuboid(lo, lo.map(|c| c + 0.05)))
    })
}

/// The sphere with a concentric inner sphere of radius 7, wound outward
/// like a second solid instead of inward like a cavity.
fn hollow_cavity_outward() -> Soup {
    sphere().append(icosphere(4, 7.0, [0.0, 0.0, 10.0]))
}

fn hollow_cavity_ok() -> Soup {
    let inner = icosphere(4, 7.0, [0.0, 0.0, 10.0]);
    let all: Vec<usize> = (0..inner.faces.len()).collect();
    sphere().append(inner.flip(&all))
}

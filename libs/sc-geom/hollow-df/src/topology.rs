//! Edge topology over an [`IndexedMesh`]: adjacency, boundary edges,
//! non-manifold edges and winding disagreements.
//!
//! Ported from DragonFruit `rust/dragonfruit-mesh-core/src/halfedge.rs` (the
//! parts the hollowing engine reads), plus [`is_edge_manifold`], the check
//! SoulCrafted's slicer applies before it slices.

use ahash::{AHashMap, RandomState};
use smallvec::SmallVec;

use crate::mesh::IndexedMesh;

/// Fixed seeds, so every walk over `edges` runs in the same order in every
/// process (DragonFruit's choice, kept for reproducible results).
fn edge_hasher() -> RandomState {
    RandomState::with_seeds(
        0x9e37_79b9_7f4a_7c15,
        0xbf58_476d_1ce4_e5b9,
        0x94d0_49bb_1331_11eb,
        0x2545_f491_4f6c_dd1d,
    )
}

/// Canonical undirected edge key (sorted endpoints).
pub type EdgeKey = (u32, u32);

#[inline]
pub fn edge_key(a: u32, b: u32) -> EdgeKey {
    if a < b { (a, b) } else { (b, a) }
}

#[derive(Debug, Default, Clone)]
pub struct EdgeInfo {
    /// Faces incident to this edge.
    pub faces: SmallVec<[u32; 2]>,
    /// Directed samples `(from, to, face)`.
    pub directed: SmallVec<[(u32, u32, u32); 2]>,
}

#[derive(Debug, Default)]
pub struct Topology {
    pub edges: AHashMap<EdgeKey, EdgeInfo>,
}

impl Topology {
    pub fn build(mesh: &IndexedMesh) -> Self {
        let mut edges: AHashMap<EdgeKey, EdgeInfo> =
            AHashMap::with_capacity_and_hasher(mesh.triangles.len() * 2, edge_hasher());
        for (fi, tri) in mesh.triangles.iter().enumerate() {
            let fi = fi as u32;
            let [a, b, c] = *tri;
            for &(u, v) in &[(a, b), (b, c), (c, a)] {
                let entry = edges.entry(edge_key(u, v)).or_default();
                entry.faces.push(fi);
                entry.directed.push((u, v, fi));
            }
        }
        Self { edges }
    }

    /// Sorted, so two runs over the same mesh give the same list.
    pub fn boundary_edges(&self) -> Vec<EdgeKey> {
        let mut edges: Vec<EdgeKey> = self
            .edges
            .iter()
            .filter(|(_, info)| info.faces.len() == 1)
            .map(|(k, _)| *k)
            .collect();
        edges.sort_unstable();
        edges
    }

    /// Edges whose faces cannot be paired by direction: some traverse it each
    /// way, but not equally many (a fan with a face it cannot pair).
    pub fn non_manifold_edges(&self) -> Vec<EdgeKey> {
        let mut edges: Vec<EdgeKey> = self
            .edges
            .iter()
            .filter(|(_, info)| {
                let mut forward = 0usize;
                let mut backward = 0usize;
                for &(u, v, _) in &info.directed {
                    if u < v {
                        forward += 1;
                    } else {
                        backward += 1;
                    }
                }
                forward > 0 && backward > 0 && forward != backward
            })
            .map(|(k, _)| *k)
            .collect();
        edges.sort_unstable();
        edges
    }

    /// Edges whose two faces traverse them the same way.
    pub fn inconsistent_edges(&self) -> usize {
        self.edges
            .values()
            .filter(|info| {
                if info.directed.len() != 2 {
                    return false;
                }
                let (a0, b0, _) = info.directed[0];
                let (a1, b1, _) = info.directed[1];
                (a0, b0) == (a1, b1)
            })
            .count()
    }
}

/// Counts of the edge defects SoulCrafted's slicer rejects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EdgeManifoldDefects {
    /// Edges with a single face.
    pub boundary: usize,
    /// Edges with more than two faces.
    pub overloaded: usize,
    /// Edges whose two faces traverse them the same way.
    pub same_direction: usize,
    /// Faces repeating a vertex.
    pub degenerate_faces: usize,
}

impl EdgeManifoldDefects {
    pub fn is_clean(&self) -> bool {
        *self == Self::default()
    }
}

/// Every edge is shared by exactly two faces traversing it in opposite
/// directions, and no face repeats a vertex: the precondition of
/// SoulCrafted's slicer (`ValidatedFaces::validated_edge_manifold`).
pub fn edge_manifold_defects(mesh: &IndexedMesh) -> EdgeManifoldDefects {
    use rayon::prelude::*;
    let mut defects = EdgeManifoldDefects {
        degenerate_faces: mesh
            .triangles
            .par_iter()
            .filter(|[a, b, c]| a == b || b == c || a == c)
            .count(),
        ..Default::default()
    };
    // Every half-edge as `min << 33 | max << 1 | backward`, sorted: an edge's
    // half-edges end up adjacent. A sort beats a hash map several times over
    // on millions of edges, and runs in parallel.
    debug_assert!(mesh.positions.len() < 1 << 31);
    let mut half: Vec<u64> = mesh
        .triangles
        .par_iter()
        .filter(|[a, b, c]| a != b && b != c && a != c)
        .flat_map_iter(|&[a, b, c]| {
            [(a, b), (b, c), (c, a)].map(|(u, v)| {
                let (lo, hi) = if u < v { (u, v) } else { (v, u) };
                ((lo as u64) << 33) | ((hi as u64) << 1) | u64::from(u > v)
            })
        })
        .collect();
    half.par_sort_unstable();
    let mut i = 0;
    while i < half.len() {
        let edge = half[i] >> 1;
        let mut j = i;
        let mut backward = 0;
        while j < half.len() && half[j] >> 1 == edge {
            backward += (half[j] & 1) as usize;
            j += 1;
        }
        let forward = (j - i) - backward;
        match (forward, backward) {
            (1, 1) => {}
            (f, b) if f + b == 1 => defects.boundary += 1,
            (2, 0) | (0, 2) => defects.same_direction += 1,
            _ => defects.overloaded += 1,
        }
        i = j;
    }
    defects
}

/// Whether [`edge_manifold_defects`] finds nothing.
pub fn is_edge_manifold(mesh: &IndexedMesh) -> bool {
    edge_manifold_defects(mesh).is_clean()
}

/// Edge-connected components (shells) of `mesh`, as one label per face,
/// numbered `0..count` in order of each shell's first face.
pub fn face_shells(mesh: &IndexedMesh) -> (Vec<u32>, usize) {
    fn find(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    let mut parent: Vec<u32> = (0..mesh.positions.len() as u32).collect();
    for tri in &mesh.triangles {
        let r0 = find(&mut parent, tri[0]);
        for &v in &tri[1..] {
            let r = find(&mut parent, v);
            if r != r0 {
                parent[r as usize] = r0;
            }
        }
    }
    let mut label_of_root: AHashMap<u32, u32> = AHashMap::new();
    let mut labels = Vec::with_capacity(mesh.triangles.len());
    for tri in &mesh.triangles {
        let root = find(&mut parent, tri[0]);
        let next = label_of_root.len() as u32;
        labels.push(*label_of_root.entry(root).or_insert(next));
    }
    let count = label_of_root.len();
    (labels, count)
}

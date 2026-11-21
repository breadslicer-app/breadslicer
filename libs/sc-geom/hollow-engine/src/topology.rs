//! How the triangles of an [`IndexedMesh`] hang together: the edge defects
//! the slicer refuses, and the pieces a mesh falls into.

use rayon::prelude::*;

use crate::mesh::IndexedMesh;

/// Counts of the edge defects the slicer rejects.
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

/// What stands between `mesh` and the slicer: every edge has to be shared by
/// exactly two faces that run along it opposite ways, and no face may
/// repeat a corner.
pub fn edge_manifold_defects(mesh: &IndexedMesh) -> EdgeManifoldDefects {
    let mut defects = EdgeManifoldDefects {
        degenerate_faces: mesh
            .triangles
            .par_iter()
            .filter(|[a, b, c]| a == b || b == c || a == c)
            .count(),
        ..Default::default()
    };
    // Every half-edge as `min << 33 | max << 1 | backward`, sorted: an edge's
    // half-edges end up next to each other. A sort beats a hash map several
    // times over on millions of edges, and runs in parallel.
    debug_assert!(mesh.positions.len() < 1 << 31);
    let mut half: Vec<u64> = mesh
        .triangles
        .par_iter()
        .filter(|[a, b, c]| a != b && b != c && a != c)
        .flat_map_iter(|&[a, b, c]| {
            [(a, b), (b, c), (c, a)].map(|(u, v)| {
                let (lo, hi) = if u < v { (u, v) } else { (v, u) };
                (u64::from(lo) << 33) | (u64::from(hi) << 1) | u64::from(u > v)
            })
        })
        .collect();
    half.par_sort_unstable();
    for run in half.chunk_by(|a, b| a >> 1 == b >> 1) {
        let backward = run.iter().filter(|h| *h & 1 == 1).count();
        match (run.len() - backward, backward) {
            (1, 1) => {}
            (f, b) if f + b == 1 => defects.boundary += 1,
            (2, 0) | (0, 2) => defects.same_direction += 1,
            _ => defects.overloaded += 1,
        }
    }
    defects
}

/// Whether [`edge_manifold_defects`] finds nothing.
pub fn is_edge_manifold(mesh: &IndexedMesh) -> bool {
    edge_manifold_defects(mesh).is_clean()
}

/// Disjoint sets over `0..n`, each pointing at the smallest member it joined.
struct Groups(Vec<u32>);

impl Groups {
    fn new(n: usize) -> Self {
        Self((0..n as u32).collect())
    }

    fn root(&mut self, mut i: u32) -> u32 {
        while self.0[i as usize] != i {
            let up = self.0[self.0[i as usize] as usize];
            self.0[i as usize] = up;
            i = up;
        }
        i
    }

    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.0[a.max(b) as usize] = a.min(b);
        }
    }

    /// One label per member in `0..count`, numbered by first appearance in
    /// `members`, and `count`.
    fn labels(&mut self, members: impl Iterator<Item = u32>) -> (Vec<u32>, usize) {
        let mut label = vec![u32::MAX; self.0.len()];
        let mut count = 0u32;
        let labels = members
            .map(|m| {
                let root = self.root(m) as usize;
                if label[root] == u32::MAX {
                    label[root] = count;
                    count += 1;
                }
                label[root]
            })
            .collect();
        (labels, count as usize)
    }
}

/// The pieces of `mesh` held together by shared edges, as a label per face
/// numbered `0..count` in the order of each piece's first face. Unlike
/// [`face_shells`], faces that only share a corner stay apart: two closed
/// tubes welded at one point are two pieces.
pub fn edge_components(mesh: &IndexedMesh) -> (Vec<u32>, usize) {
    let mut edges: Vec<(u64, u32)> = mesh
        .triangles
        .iter()
        .enumerate()
        .flat_map(|(f, &[a, b, c])| {
            [(a, b), (b, c), (c, a)].map(|(u, v)| {
                let (lo, hi) = if u < v { (u, v) } else { (v, u) };
                ((u64::from(lo) << 32) | u64::from(hi), f as u32)
            })
        })
        .collect();
    edges.par_sort_unstable();
    let mut groups = Groups::new(mesh.triangles.len());
    for run in edges.chunk_by(|a, b| a.0 == b.0) {
        for pair in run.windows(2) {
            groups.join(pair[0].1, pair[1].1);
        }
    }
    groups.labels(0..mesh.triangles.len() as u32)
}

/// The shells of `mesh`, held together by shared corners, as a label per
/// face numbered `0..count` in the order of each shell's first face.
pub fn face_shells(mesh: &IndexedMesh) -> (Vec<u32>, usize) {
    let mut groups = Groups::new(mesh.positions.len());
    for &[a, b, c] in &mesh.triangles {
        groups.join(a, b);
        groups.join(a, c);
    }
    groups.labels(mesh.triangles.iter().map(|t| t[0]))
}

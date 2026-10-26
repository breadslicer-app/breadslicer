/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Connected shells of a leaf mesh.
//!
//! Outer/hole grouping ([`crate::grouping`]) assumes the rings of a layer
//! nest cleanly or are disjoint, which holds for 1 closed manifold shell. A
//! leaf can hold several shells that overlap without sharing a vertex (a
//! support tree's tubes and joints, a pre-supported STL): their rings then
//! cross, and a ring inside another shell's ring would be taken for a hole.
//! Rings are therefore grouped shell by shell; overlapping shells' polygons
//! stay separate and the non-zero raster unites them.

use sc_mesh_core::indexed_mesh::ValidatedEdgeManifoldMesh;
use sc_mesh_core::primitives::FaceIndex;

/// The shell of every face: faces sharing a vertex are in the same shell.
pub(crate) struct FaceShells {
    /// Per face, its shell, numbered `0..count` in order of first face.
    labels: Vec<u32>,
    count: u32,
}

impl FaceShells {
    pub(crate) fn build(mesh: &ValidatedEdgeManifoldMesh<'_>) -> Self {
        let faces = mesh.faces();
        let vertex_count = faces
            .iter()
            .flat_map(|f| f.vertices)
            .map(|v| v.get() as usize + 1)
            .max()
            .unwrap_or(0);
        let mut parent: Vec<u32> = (0..vertex_count as u32).collect();
        fn root(parent: &mut [u32], mut v: u32) -> u32 {
            while parent[v as usize] != v {
                let up = parent[parent[v as usize] as usize];
                parent[v as usize] = up;
                v = up;
            }
            v
        }
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
            .collect();
        Self { labels, count }
    }

    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    pub(crate) fn of(&self, face: FaceIndex) -> u32 {
        self.labels[face.get()]
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Connected shells of a leaf mesh.
//!
//! A leaf can hold several shells that overlap without sharing a vertex (a
//! support tree's tubes and joints, a pre-supported STL). Each ring is an
//! outer or a hole by its own facing ([`crate::grouping`]), and rings are
//! grouped shell by shell, so a hole only ever attaches to an outer of its
//! own shell; overlapping shells' polygons stay separate and the non-zero
//! raster unites them.
//!
//! A shell with negative signed volume faces inward: it is a void, such as
//! the sealed cavity of a hollow without a drain. Its rings are holes of
//! whatever solid encloses them, not polygons of their own.

use sc_mesh_core::indexed_mesh::ValidatedEdgeManifoldMesh;
use sc_mesh_core::primitives::FaceIndex;

/// The shell of every face: faces sharing a vertex are in the same shell.
pub(crate) struct FaceShells {
    /// Per face, its shell, numbered `0..count` in order of first face.
    labels: Vec<u32>,
    count: u32,
    /// Per shell, whether it faces inward (negative signed volume).
    void: Vec<bool>,
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
            .collect::<Vec<u32>>();
        // Signed volume per shell (divergence theorem, times 6), in f64
        // about the mesh's first vertex so far-off models keep precision.
        let vertices = mesh.vertices();
        let origin = vertices
            .first()
            .map_or([0.0; 3], |v| v.coords().map(f64::from));
        let at = |i: u32| {
            let c = vertices[i as usize].coords();
            [
                f64::from(c[0]) - origin[0],
                f64::from(c[1]) - origin[1],
                f64::from(c[2]) - origin[2],
            ]
        };
        let mut volume = vec![0.0f64; count as usize];
        for (f, &shell) in faces.iter().zip(&labels) {
            let [a, b, c] = f.vertices.map(|v| at(v.get()));
            volume[shell as usize] += a[0] * (b[1] * c[2] - b[2] * c[1])
                - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
        }
        let void = volume.iter().map(|&v| v < 0.0).collect();
        Self {
            labels,
            count,
            void,
        }
    }

    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    pub(crate) fn of(&self, face: FaceIndex) -> u32 {
        self.labels[face.get()]
    }

    /// Whether `shell` faces inward (a sealed void).
    pub(crate) fn is_void(&self, shell: u32) -> bool {
        self.void[shell as usize]
    }
}

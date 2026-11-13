/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-mesh acceleration structures.
//!
//! [`MeshSpatial`] is a private type. Callers cannot hold one in
//! isolation from the [`IndexedMesh`] it belongs to: the only path to
//! these caches is through
//! [`SceneSpatial::object`](crate::SceneSpatial::object), which pairs
//! a mesh and its `MeshSpatial` from a single internal table. That
//! pairing is what makes mismatched (mesh, cache) queries
//! structurally impossible.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use sc_mesh_core::{Aabb, IndexedMesh, Vertex};

use crate::clusters::TriangleClusters;
use crate::face_adjacency::FaceAdjacency;

#[derive(Default)]
pub(crate) struct MeshSpatial {
    local_aabb: OnceLock<Aabb>,
    vertex_blocks: OnceLock<Vec<Aabb>>,
    clusters: OnceLock<TriangleClusters>,
    /// Built when a brush first needs it, not by [`Self::prime`]: most
    /// meshes are never painted.
    adjacency: OnceLock<FaceAdjacency>,
    /// Cache for the AABB of the mesh's vertices under a fixed
    /// linear transform (translation excluded), keyed by bit-equal
    /// match on the linear part, the latest first. The clamp helper in
    /// the GUI hits it every tick of a translate-mode drag, where the
    /// linear part stays constant. It keeps up to [`LINEAR_AABB_SLOTS`]
    /// transforms: a mesh several objects share at different sizes or
    /// turns used to evict one for the other at every query, each a
    /// walk of every vertex (#476, #561: 106 copies each turned its own
    /// way by Arrange all, asked on every scene change).
    linear_aabb: Mutex<HashMap<[u64; 9], Aabb>>,
}

/// How many linear transforms [`MeshSpatial::linear_aabb`] remembers per
/// mesh before it starts over: far more than the copies of one mesh a
/// scene holds, each turned or sized its own way.
pub(crate) const LINEAR_AABB_SLOTS: usize = 4096;

/// A linear part as a key, bit for bit.
fn linear_key(linear: &[[f64; 3]; 3]) -> [u64; 9] {
    let mut key = [0u64; 9];
    for (i, row) in linear.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            key[i * 3 + j] = v.to_bits();
        }
    }
    key
}

impl MeshSpatial {
    /// Object-space AABB of `mesh`. Cached on first call; `None` for
    /// empty meshes.
    ///
    /// The caller is responsible for passing the same `mesh` it
    /// would pass on every other call to this `MeshSpatial`. The
    /// public surface of the crate ([`SceneSpatial::object`]) takes
    /// care of that; this type is module-private precisely to keep
    /// the pairing inside the spatial layer.
    pub(crate) fn local_aabb(&self, mesh: &IndexedMesh) -> Option<Aabb> {
        if let Some(cached) = self.local_aabb.get() {
            return Some(*cached);
        }
        let computed = mesh.local_aabb()?;
        let _ = self.local_aabb.set(computed);
        Some(computed)
    }

    /// Object-space cluster AABBs over `mesh.faces`. Cached on first
    /// call; `None` for empty face sets or out-of-range face vertex
    /// indices (corrupt mesh).
    pub(crate) fn clusters(&self, mesh: &IndexedMesh) -> Option<&TriangleClusters> {
        if mesh.faces.is_empty() {
            return None;
        }
        if let Some(cached) = self.clusters.get() {
            return Some(cached);
        }
        let built = TriangleClusters::for_mesh(mesh)?;
        let _ = self.clusters.set(built);
        self.clusters.get()
    }

    /// Which faces of `mesh` share an edge. Cached on first call; a caller
    /// that reaches it while a worker builds it waits for that worker.
    pub(crate) fn face_adjacency(&self, mesh: &IndexedMesh) -> &FaceAdjacency {
        self.adjacency.get_or_init(|| FaceAdjacency::new(mesh))
    }

    /// Whether the face adjacency is built: asking for it now does not wait.
    pub(crate) fn has_face_adjacency(&self) -> bool {
        self.adjacency.get().is_some()
    }

    /// AABB of `mesh.vertices` after applying `linear` (no
    /// translation). Tight: walks vertices directly rather than
    /// passing the local AABB through Arvo, so it does not inherit
    /// Arvo's slack on rotated non-box meshes. Cached for up to
    /// [`LINEAR_AABB_SLOTS`] linear parts, then started over. `None` for
    /// empty meshes.
    pub(crate) fn linear_aabb(&self, mesh: &IndexedMesh, linear: &[[f64; 3]; 3]) -> Option<Aabb> {
        let key = linear_key(linear);
        if let Some(aabb) = self
            .linear_aabb
            .lock()
            .expect("linear_aabb poisoned")
            .get(&key)
        {
            return Some(*aabb);
        }
        let mut iter = mesh.vertices.iter();
        let first = apply_linear(linear, *iter.next()?);
        let mut aabb = Aabb::from_min_max(first, first);
        for v in iter {
            aabb.extend(apply_linear(linear, *v));
        }
        let mut guard = self.linear_aabb.lock().expect("linear_aabb poisoned");
        if guard.len() >= LINEAR_AABB_SLOTS {
            guard.clear();
        }
        guard.insert(key, aabb);
        Some(aabb)
    }

    // Vertex-order blocks preserve exact extrema while cheaply pruning interior
    // regions. Owned by the mesh cache, so edits cannot reuse stale bounds.
    fn vertex_blocks(&self, mesh: &IndexedMesh) -> &[Aabb] {
        self.vertex_blocks.get_or_init(|| {
            mesh.vertices
                .chunks(256)
                .map(|vertices| {
                    let mut bounds = Aabb::from_min_max(vertices[0], vertices[0]);
                    for vertex in &vertices[1..] {
                        bounds.extend(*vertex);
                    }
                    bounds
                })
                .collect()
        })
    }

    pub(crate) fn minimum_projection(&self, mesh: &IndexedMesh, row: [f64; 3]) -> Option<f64> {
        let project = |v: Vertex| {
            let p = v.coords();
            row[0].mul_add(
                f64::from(p[0]),
                row[1].mul_add(f64::from(p[1]), row[2] * f64::from(p[2])),
            )
        };
        let mut best = project(*mesh.vertices.first()?);
        for (vertices, bounds) in mesh.vertices.chunks(256).zip(self.vertex_blocks(mesh)) {
            let lo = bounds.min().coords();
            let hi = bounds.max().coords();
            let corner = Vertex::new(std::array::from_fn(|i| {
                if row[i] >= 0.0 { lo[i] } else { hi[i] }
            }));
            let lower = project(corner);
            let margin = 1e-9 * (1.0 + best.abs() + lower.abs());
            if lower > best + margin {
                continue;
            }
            for vertex in vertices {
                best = best.min(project(*vertex));
            }
        }
        Some(best)
    }

    /// Eagerly populate every cache in this slot. Used by
    /// [`SceneSpatial::primed_graph`](crate::SceneSpatial::primed_graph)
    /// to do the work off the UI thread before the mesh ever reaches
    /// a query site.
    ///
    /// `linear_aabb` is intentionally not primed: its key depends on
    /// the object's current transform, which is not known until a
    /// query site presents it.
    pub(crate) fn prime(&self, mesh: &IndexedMesh) {
        let _ = self.local_aabb(mesh);
        let _ = self.clusters(mesh);
        let _ = self.vertex_blocks(mesh);
    }
}

fn apply_linear(m: &[[f64; 3]; 3], v: Vertex) -> Vertex {
    let v = v.coords();
    let v = [v[0] as f64, v[1] as f64, v[2] as f64];
    let x = m[0][0].mul_add(v[0], m[0][1].mul_add(v[1], m[0][2] * v[2]));
    let y = m[1][0].mul_add(v[0], m[1][1].mul_add(v[1], m[1][2] * v[2]));
    let z = m[2][0].mul_add(v[0], m[2][1].mul_add(v[1], m[2][2] * v[2]));
    Vertex::new([x as f32, y as f32, z as f32])
}

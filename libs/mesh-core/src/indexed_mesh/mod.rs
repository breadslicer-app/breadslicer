/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

pub mod clusters;
pub(crate) mod fix;
mod validate;
mod validated;

pub use clusters::TriangleClusters;
pub use validated::{ValidatedFaces, ValidatedFacesMut};

use std::sync::OnceLock;

use crate::aabb::Aabb;
use crate::errors::{MeshFaceError, MeshFix};
use crate::primitives::{IndexedTriangle, Vertex};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshMetadata {
    pub name: Option<String>,
    pub header_comment: Option<String>,
}

/// Which face-orientation repairs `fix_faces_orientation` should apply.
///
/// Both flags can be set independently; they correspond to two passes
/// the routine can take over the mesh:
///
/// - `flipped`: detect inconsistently-wound faces (via manifold errors)
///   and reverse their vertex order.
/// - `normals`: recompute the stored normal vector from the triangle's
///   vertices for faces whose normal was wrong (non-unit-length, or
///   misaligned with the geometric normal).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixFacesOptions {
    pub flipped: bool,
    pub normals: bool,
}

pub struct IndexedMesh {
    pub meta: Option<MeshMetadata>,
    /// Mutating vertex positions in place invalidates
    /// [`Self::local_aabb_cache`] and [`Self::clusters_cache`].
    /// Callers that change positions must call
    /// [`Self::invalidate_local_aabb`] and
    /// [`Self::invalidate_clusters`].
    pub vertices: Vec<Vertex>,
    /// Mutating the order of faces in place invalidates
    /// [`Self::clusters_cache`] (cluster ranges index `faces` by
    /// position). Witnessed mutations via [`ValidatedFacesMut`] only
    /// flip winding / recompute normals, never change order, so they
    /// leave the cache valid.
    pub faces: Vec<IndexedTriangle>,
    pub local_aabb_cache: OnceLock<Aabb>,
    pub clusters_cache: OnceLock<TriangleClusters>,
}

impl Clone for IndexedMesh {
    fn clone(&self) -> Self {
        let local_aabb_cache = OnceLock::new();
        if let Some(cached) = self.local_aabb_cache.get() {
            let _ = local_aabb_cache.set(*cached);
        }
        let clusters_cache = OnceLock::new();
        if let Some(cached) = self.clusters_cache.get() {
            let _ = clusters_cache.set(cached.clone());
        }
        Self {
            meta: self.meta.clone(),
            vertices: self.vertices.clone(),
            faces: self.faces.clone(),
            local_aabb_cache,
            clusters_cache,
        }
    }
}

impl PartialEq for IndexedMesh {
    fn eq(&self, other: &Self) -> bool {
        self.meta == other.meta && self.vertices == other.vertices && self.faces == other.faces
    }
}

impl std::fmt::Debug for IndexedMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexedMesh")
            .field("meta", &self.meta)
            .field("vertices", &self.vertices)
            .field("faces", &self.faces)
            .finish()
    }
}

impl IndexedMesh {
    pub fn new(
        meta: Option<MeshMetadata>,
        vertices: Vec<Vertex>,
        faces: Vec<IndexedTriangle>,
    ) -> Self {
        Self {
            meta,
            vertices,
            faces,
            local_aabb_cache: OnceLock::new(),
            clusters_cache: OnceLock::new(),
        }
    }

    pub fn meta(&self) -> &Option<MeshMetadata> {
        &self.meta
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    pub fn faces(&self) -> &[IndexedTriangle] {
        &self.faces
    }

    pub fn set_meta(&mut self, meta: Option<MeshMetadata>) {
        self.meta = meta;
    }

    /// Object-space AABB of the vertex set. Cached on first call;
    /// subsequent calls are `O(1)` until [`Self::invalidate_local_aabb`]
    /// clears the cache. Returns `None` for an empty mesh.
    pub fn local_aabb(&self) -> Option<Aabb> {
        if self.vertices.is_empty() {
            return None;
        }
        Some(*self.local_aabb_cache.get_or_init(|| {
            let mut iter = self.vertices.iter();
            let first = *iter.next().expect("non-empty checked above");
            let mut aabb = Aabb::from_min_max(first, first);
            for vertex in iter {
                aabb.extend(*vertex);
            }
            aabb
        }))
    }

    /// Clears the cached local AABB. Must be called by any code that
    /// mutates vertex positions in place. The validation witnesses
    /// only mutate faces, never positions, so they leave the cache
    /// valid.
    pub fn invalidate_local_aabb(&mut self) {
        self.local_aabb_cache.take();
    }

    #[cfg(test)]
    pub(crate) fn local_aabb_is_cached(&self) -> bool {
        self.local_aabb_cache.get().is_some()
    }

    /// Object-space cluster AABBs over the face array, used by spatial
    /// query callers (the picker today) to skip linear walks. Cached
    /// on first call; subsequent calls are `O(1)` until
    /// [`Self::invalidate_clusters`] clears the cache. Returns `None`
    /// for an empty mesh or one with out-of-range face indices —
    /// callers treat that as "no spatial index available" and either
    /// fall back to a linear walk or skip the object.
    pub fn triangle_clusters(&self) -> Option<&TriangleClusters> {
        if self.faces.is_empty() {
            return None;
        }
        // `OnceLock::get_or_init` cannot return `Option<&T>`; build
        // outside, then cache only on success. A failed build (corrupt
        // mesh) leaves the cell empty so a later mutation that fixes
        // the mesh can succeed without an explicit invalidation.
        if let Some(cached) = self.clusters_cache.get() {
            return Some(cached);
        }
        let built = TriangleClusters::build(&self.vertices, &self.faces)?;
        let _ = self.clusters_cache.set(built);
        self.clusters_cache.get()
    }

    /// Clears the cached cluster AABBs. Must be called by any code
    /// that mutates vertex positions or face order in place. Witnessed
    /// face mutations (winding flips, normal recomputes) preserve
    /// order and leave the cache valid.
    pub fn invalidate_clusters(&mut self) {
        self.clusters_cache.take();
    }

    #[cfg(test)]
    pub(crate) fn clusters_are_cached(&self) -> bool {
        self.clusters_cache.get().is_some()
    }

    /// Validate face indices and, on success, return a witness whose
    /// methods can index into `vertices` / `faces` without per-call
    /// bounds checks. Lifetime-tied to `self`: any later mutation of
    /// the mesh invalidates the witness.
    pub fn validated_faces(&self) -> Result<ValidatedFaces<'_>, Vec<MeshFaceError>> {
        self.validate_faces()?;
        Ok(ValidatedFaces::new_unchecked(self))
    }

    /// Mutable counterpart of [`validated_faces`]. The returned
    /// witness exposes only mutations that preserve the in-range
    /// invariant (winding flips, normal recomputes, etc.); operations
    /// that could invalidate it (e.g. removing whole faces) live on
    /// `IndexedMesh` itself.
    pub fn validated_faces_mut(&mut self) -> Result<ValidatedFacesMut<'_>, Vec<MeshFaceError>> {
        self.validate_faces()?;
        Ok(ValidatedFacesMut::new_unchecked(self))
    }

    /// Repair the `ZeroLengthEdge` errors in `errors` (other variants
    /// are ignored), then return a witness for the post-removal mesh.
    ///
    /// If every input error is a `ZeroLengthEdge`, the resulting
    /// mesh's face indices are guaranteed to remain in range —
    /// removing whole faces does not change `vertices.len()` and does
    /// not alter any survivor's `face.vertices[k]` — so the witness
    /// is minted directly without re-running `validate_faces`.
    /// Otherwise, the post-removal mesh is re-validated and the
    /// validation result is returned alongside the applied fixes.
    pub fn fix_zero_length_edges_into_validated(
        &mut self,
        errors: &[MeshFaceError],
    ) -> (
        Vec<MeshFix>,
        Result<ValidatedFacesMut<'_>, Vec<MeshFaceError>>,
    ) {
        let only_zero_length = errors
            .iter()
            .all(|e| matches!(e, MeshFaceError::ZeroLengthEdge { .. }));
        let fixes = fix::fix_zero_length_edges_impl(self, errors);
        let witness = if only_zero_length {
            Ok(ValidatedFacesMut::new_unchecked(self))
        } else {
            self.validated_faces_mut()
        };
        (fixes, witness)
    }
}

#[cfg(test)]
mod local_aabb_tests {
    use super::*;
    use crate::primitives::Vertex;

    fn mesh_with_vertices(vertices: Vec<Vertex>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices,
            faces: vec![],
            local_aabb_cache: OnceLock::new(),
            clusters_cache: OnceLock::new(),
        }
    }

    #[test]
    fn local_aabb_is_none_for_empty_mesh() {
        let mesh = mesh_with_vertices(vec![]);
        assert!(mesh.local_aabb().is_none());
        assert!(!mesh.local_aabb_is_cached());
    }

    #[test]
    fn local_aabb_returns_min_max_for_known_vertices() {
        let mesh = mesh_with_vertices(vec![
            Vertex::new([-1.0, -2.0, -3.0]),
            Vertex::new([3.0, 0.0, 0.0]),
            Vertex::new([-1.0, 4.0, 5.0]),
        ]);
        let aabb = mesh.local_aabb().unwrap();
        assert_eq!(aabb.min().coords(), [-1.0, -2.0, -3.0]);
        assert_eq!(aabb.max().coords(), [3.0, 4.0, 5.0]);
    }

    #[test]
    fn local_aabb_caches_first_result() {
        let mut mesh = mesh_with_vertices(vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 1.0, 1.0]),
        ]);
        let first = mesh.local_aabb().unwrap();
        assert!(mesh.local_aabb_is_cached());
        // Mutate vertex positions in place without invalidating; the
        // next call must still return the originally cached AABB.
        mesh.vertices[1] = Vertex::new([100.0, 100.0, 100.0]);
        let second = mesh.local_aabb().unwrap();
        assert_eq!(first.min().coords(), second.min().coords());
        assert_eq!(first.max().coords(), second.max().coords());
    }

    #[test]
    fn local_aabb_cache_accessor_reflects_priming() {
        let mesh = mesh_with_vertices(vec![Vertex::new([0.0, 0.0, 0.0])]);
        assert!(!mesh.local_aabb_is_cached());
        let _ = mesh.local_aabb();
        assert!(mesh.local_aabb_is_cached());
    }

    #[test]
    fn invalidate_local_aabb_clears_cache_and_recomputes() {
        let mut mesh = mesh_with_vertices(vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 1.0, 1.0]),
        ]);
        let _ = mesh.local_aabb();
        assert!(mesh.local_aabb_is_cached());
        mesh.vertices[1] = Vertex::new([5.0, 5.0, 5.0]);
        mesh.invalidate_local_aabb();
        assert!(!mesh.local_aabb_is_cached());
        let aabb = mesh.local_aabb().unwrap();
        assert_eq!(aabb.max().coords(), [5.0, 5.0, 5.0]);
    }

    #[test]
    fn clone_propagates_cached_aabb() {
        let mesh = mesh_with_vertices(vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([2.0, 2.0, 2.0]),
        ]);
        let original_aabb = mesh.local_aabb().unwrap();
        let cloned = mesh.clone();
        assert!(cloned.local_aabb_is_cached());
        assert_eq!(
            cloned.local_aabb().unwrap().max().coords(),
            original_aabb.max().coords()
        );
    }

    #[test]
    fn clone_of_unprimed_mesh_starts_unprimed() {
        let mesh = mesh_with_vertices(vec![Vertex::new([0.0, 0.0, 0.0])]);
        let cloned = mesh.clone();
        assert!(!cloned.local_aabb_is_cached());
    }

    #[test]
    fn partial_eq_ignores_cache_state() {
        let primed = mesh_with_vertices(vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 1.0, 1.0]),
        ]);
        let unprimed = mesh_with_vertices(vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 1.0, 1.0]),
        ]);
        let _ = primed.local_aabb();
        assert!(primed.local_aabb_is_cached());
        assert!(!unprimed.local_aabb_is_cached());
        assert_eq!(primed, unprimed);
    }
}

#[cfg(test)]
mod triangle_clusters_tests {
    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};

    fn unit_face_mesh(num_faces: usize) -> IndexedMesh {
        // num_faces independent CCW triangles laid along +X. Each face
        // consumes three fresh vertices, so out-of-range indices are
        // impossible.
        let mut vertices = Vec::with_capacity(num_faces * 3);
        let mut faces = Vec::with_capacity(num_faces);
        for i in 0..num_faces {
            let base = vertices.len() as u32;
            let x = i as f32;
            vertices.push(Vertex::new([x, 0.0, 0.0]));
            vertices.push(Vertex::new([x + 1.0, 0.0, 0.0]));
            vertices.push(Vertex::new([x, 1.0, 0.0]));
            faces.push(IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [base, base + 1, base + 2],
            });
        }
        IndexedMesh::new(None, vertices, faces)
    }

    #[test]
    fn triangle_clusters_is_none_for_empty_face_set() {
        let mesh = IndexedMesh::new(None, vec![Vertex::new([0.0, 0.0, 0.0])], vec![]);
        assert!(mesh.triangle_clusters().is_none());
        assert!(!mesh.clusters_are_cached());
    }

    #[test]
    fn triangle_clusters_caches_on_first_call() {
        let mesh = unit_face_mesh(3);
        assert!(!mesh.clusters_are_cached());
        let first = mesh.triangle_clusters().unwrap() as *const TriangleClusters;
        assert!(mesh.clusters_are_cached());
        let second = mesh.triangle_clusters().unwrap() as *const TriangleClusters;
        assert_eq!(first, second, "second call must return cached reference");
    }

    #[test]
    fn invalidate_clusters_clears_cache_and_recomputes() {
        let mut mesh = unit_face_mesh(2);
        let _ = mesh.triangle_clusters();
        assert!(mesh.clusters_are_cached());
        mesh.invalidate_clusters();
        assert!(!mesh.clusters_are_cached());
        let _ = mesh.triangle_clusters().unwrap();
        assert!(mesh.clusters_are_cached());
    }

    #[test]
    fn clone_propagates_cluster_cache() {
        let mesh = unit_face_mesh(2);
        let _ = mesh.triangle_clusters();
        let cloned = mesh.clone();
        assert!(cloned.clusters_are_cached());
    }

    #[test]
    fn clone_of_unprimed_mesh_starts_unprimed() {
        let mesh = unit_face_mesh(2);
        let cloned = mesh.clone();
        assert!(!cloned.clusters_are_cached());
    }

    #[test]
    fn triangle_clusters_returns_none_on_out_of_range_indices() {
        let mesh = IndexedMesh::new(
            None,
            vec![Vertex::new([0.0, 0.0, 0.0])],
            vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        );
        assert!(mesh.triangle_clusters().is_none());
        assert!(
            !mesh.clusters_are_cached(),
            "failed builds must not poison the cache"
        );
    }
}

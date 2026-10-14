/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Lifetime-bound witnesses for mesh invariants, layered from weakest to
//! strongest:
//!
//! 1. [`ValidatedFaces`] / [`ValidatedFacesMut`]: every `face.vertices[k]` is
//!    `< mesh.vertices.len()`. Minted by `IndexedMesh::validated_faces` (or the
//!    `_mut` form), which runs `validate_faces` once and on success hands back
//!    a witness whose lifetime ties safe access to the still-valid borrow.
//!    Routines that index into `vertices` / `faces` without bounds checks live
//!    on the witness, so the in-range invariant is statically enforced rather
//!    than relying on the caller's discipline.
//! 2. [`ValidatedEdgeManifoldMesh`]: in addition, every edge is incident to
//!    exactly 2 faces and consistently oriented across that edge. Minted by
//!    [`ValidatedFaces::validated_edge_manifold`], which consumes the in-range
//!    witness, runs the edge-manifold check, and on success upgrades the borrow
//!    to the stronger witness. Vertex-manifoldness is deliberately NOT part of
//!    the claim: pinch vertices pass.
//!
//! Operations that preserve the in-range invariant, such as
//! `fix_faces_orientation` (it reverses the vertex order of faces in place) and
//! the read-only validators, are methods on the witnesses. Zero-length-edge
//! face removal lives on `IndexedMesh` itself:
//! `IndexedMesh::fix_zero_length_edges_into_validated` removes the faces and
//! returns the witness for the result.

use std::fmt;

use crate::geometry::compute_reference_normal_f64;
use crate::primitives::{FaceIndex, IndexedTriangle, Vertex, VertexIndex};

use super::fix::{MeshFix, MeshFixError};
use super::half_edges;
use super::mesh::IndexedMesh;

/// A face with zero area: its vertices are collinear or coincide, so it has no
/// normal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroAreaFace {
    pub face_idx: FaceIndex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshEdgeManifoldError {
    EmptyMesh,
    /// An edge that belongs to only 1 face (the mesh has a boundary).
    BoundaryEdge {
        edge: (VertexIndex, VertexIndex),
        face_idx: FaceIndex,
    },
    /// An edge shared by more than 2 faces.
    NonManifoldEdge {
        edge: (VertexIndex, VertexIndex),
        face_indices: Vec<FaceIndex>,
    },
    /// 2 faces share an edge but traverse it in the same direction, so their
    /// windings disagree.
    InconsistentWindingOrder {
        edge: (VertexIndex, VertexIndex),
        face_indices: [FaceIndex; 2],
    },
}

impl fmt::Display for ZeroAreaFace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "face #{} is degenerate: zero area", self.face_idx)
    }
}

impl fmt::Display for MeshEdgeManifoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMesh => f.write_str("mesh has no faces"),
            Self::BoundaryEdge { edge, face_idx } => write!(
                f,
                "edge (#{}, #{}) is shared by only face #{face_idx} (a manifold edge must be shared by exactly 2 faces)",
                edge.0, edge.1,
            ),
            Self::NonManifoldEdge { edge, face_indices } => write!(
                f,
                "edge (#{}, #{}) is shared by {} faces (a manifold edge must be shared by exactly 2)",
                edge.0,
                edge.1,
                face_indices.len(),
            ),
            Self::InconsistentWindingOrder { edge, face_indices } => write!(
                f,
                "edge (#{}, #{}) is traversed in the same direction by faces #{} and #{} (windings disagree)",
                edge.0, edge.1, face_indices[0], face_indices[1],
            ),
        }
    }
}

/// Read-only witness that the underlying `IndexedMesh`'s face indices are all
/// in range.
#[derive(Debug)]
pub struct ValidatedFaces<'a> {
    mesh: &'a IndexedMesh,
}

/// Mutable witness that the underlying `IndexedMesh`'s face indices are all in
/// range. Methods on the witness only perform mutations that preserve that
/// invariant.
#[derive(Debug)]
pub struct ValidatedFacesMut<'a> {
    mesh: &'a mut IndexedMesh,
}

impl<'a> ValidatedFaces<'a> {
    /// Builds the witness for a mesh whose face indices have just been
    /// confirmed in range. `pub(super)` so the only path is via
    /// `IndexedMesh::validated_faces`.
    pub(super) fn new_unchecked(mesh: &'a IndexedMesh) -> Self {
        Self { mesh }
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.mesh.vertices
    }

    pub fn faces(&self) -> &[IndexedTriangle] {
        &self.mesh.faces
    }

    pub fn validate_face_areas(&self) -> Result<(), Vec<ZeroAreaFace>> {
        validate_face_areas_impl(self.mesh)
    }

    pub fn validate_edge_manifold(&self) -> Result<(), Vec<MeshEdgeManifoldError>> {
        validate_edge_manifold_impl(self.mesh)
    }

    /// Runs the edge-manifold validation and, on success, returns a stronger
    /// witness whose existence proves the wrapped mesh is edge-manifold.
    /// Consumes `self` so the in-range witness is upgraded rather than
    /// duplicated; the resulting `ValidatedEdgeManifoldMesh<'a>` ties to the
    /// same `&'a IndexedMesh` borrow.
    pub fn validated_edge_manifold(
        self,
    ) -> Result<ValidatedEdgeManifoldMesh<'a>, Vec<MeshEdgeManifoldError>> {
        validate_edge_manifold_impl(self.mesh)?;
        Ok(ValidatedEdgeManifoldMesh::new_unchecked(self.mesh))
    }

    /// Computes the volume in cubic millimetres the mesh encloses, signed by
    /// its winding: positive when faces wind outward, negative when they wind
    /// inward.
    ///
    /// Only a closed surface encloses anything. An open one still returns a
    /// number, whatever its boundary leaves unbalanced, so callers wanting a
    /// physical volume should establish closure first through
    /// [`Self::validated_edge_manifold`].
    ///
    /// Vertices are `f32` but the sum is taken in `f64`, since each term scales
    /// with the mesh's distance from the origin while the total does not.
    pub fn signed_volume_mm3(&self) -> f64 {
        self.mesh
            .faces
            .iter()
            .map(|face| {
                // In range by this witness's invariant.
                let [i0, i1, i2] = face.vertices.map(usize::from);
                let v0 = self.mesh.vertices[i0].to_f64();
                let v1 = self.mesh.vertices[i1].to_f64();
                let v2 = self.mesh.vertices[i2].to_f64();
                v0.dot_product(&v1.cross_product(&v2))
            })
            .sum::<f64>()
            / 6.0
    }
}

/// Read-only witness that the underlying `IndexedMesh` is edge-manifold: every
/// edge is incident to exactly 2 faces (no boundary edges, no edges shared by 3
/// or more faces) and consistently oriented across that edge (no 2 coincident
/// half-edges traversing the same direction). The in-range invariant of
/// [`ValidatedFaces`] is also carried by this witness, since the only path to
/// construct one is through [`ValidatedFaces::validated_edge_manifold`].
///
/// What this witness does NOT guarantee:
/// - Vertex-manifoldness: a vertex may be a pinch where several surface lobes
///   touch at a single point, its incident faces then forming several disjoint
///   fans. Such a mesh still bounds a well-defined solid; consumers that need
///   the per-vertex fan structure get it from `MeshTopology`.
/// - Every triangle has non-zero area (covered by
///   [`ValidatedFaces::validate_face_areas`]).
/// - Any particular global winding convention (CCW outward, etc.):
///   edge-manifold inputs may still be globally inside-out.
#[derive(Debug)]
pub struct ValidatedEdgeManifoldMesh<'a> {
    mesh: &'a IndexedMesh,
}

impl<'a> ValidatedEdgeManifoldMesh<'a> {
    /// Builds an edge-manifold witness for a mesh whose edge-manifoldness has
    /// just been confirmed. `pub(super)` so the only public path is via
    /// [`ValidatedFaces::validated_edge_manifold`].
    pub(super) fn new_unchecked(mesh: &'a IndexedMesh) -> Self {
        Self { mesh }
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.mesh.vertices
    }

    pub fn faces(&self) -> &[IndexedTriangle] {
        &self.mesh.faces
    }

    pub fn as_indexed_mesh(&self) -> &IndexedMesh {
        self.mesh
    }
}

impl<'a> ValidatedFacesMut<'a> {
    pub(super) fn new_unchecked(mesh: &'a mut IndexedMesh) -> Self {
        Self { mesh }
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.mesh.vertices
    }

    pub fn faces(&self) -> &[IndexedTriangle] {
        &self.mesh.faces
    }

    pub fn validate_face_areas(&self) -> Result<(), Vec<ZeroAreaFace>> {
        validate_face_areas_impl(self.mesh)
    }

    pub fn validate_edge_manifold(&self) -> Result<(), Vec<MeshEdgeManifoldError>> {
        validate_edge_manifold_impl(self.mesh)
    }

    /// Reverses the winding of the faces that `manifold_errors` shows as
    /// flipped. On `Err` the mesh is unchanged. The vertex indices of a face
    /// only change places, so `self` stays a valid witness. `manifold_errors`
    /// must come from the edge-manifold validation of this mesh.
    pub fn fix_faces_orientation(
        &mut self,
        manifold_errors: &[MeshEdgeManifoldError],
    ) -> Result<Vec<MeshFix>, MeshFixError> {
        super::fix::fix_faces_orientation_impl(self.mesh, manifold_errors)
    }
}

/// Carries out `validate_face_areas` for both `ValidatedFaces` and
/// `ValidatedFacesMut`. The in-range invariant is what makes the unguarded
/// `mesh.vertices[i]` indexing inside this routine safe; both witness types
/// only call this on a mesh whose `validate_faces` returned `Ok`.
fn validate_face_areas_impl(mesh: &IndexedMesh) -> Result<(), Vec<ZeroAreaFace>> {
    let mut errors: Vec<ZeroAreaFace> = vec![];

    for (idx, face) in mesh.faces.iter().enumerate() {
        let corners = face.vertices.map(|i| mesh.vertices[usize::from(i)]);
        // A face has zero area exactly when it has no normal; a NaN coordinate
        // also leaves it without one.
        if compute_reference_normal_f64(&corners).is_err() {
            errors.push(ZeroAreaFace {
                face_idx: FaceIndex::from(idx),
            });
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Carries out `validate_edge_manifold`. Strictly speaking this routine never
/// indexes into `mesh.vertices` (it only uses `face.vertices` values as
/// half-edge keys), so the in-range invariant is not a soundness requirement
/// here: the witness gates this method only for API symmetry with
/// `validate_face_areas`.
///
/// The directed half-edges are sorted so each undirected edge forms 1
/// contiguous run: run length 1 is a boundary edge, more than 2 an over-shared
/// edge, and a length-2 run whose 2 faces traverse the edge in the same
/// direction an inconsistent winding. Errors come out sorted by canonical edge
/// (deterministic across runs), with the face lists inside each error
/// ascending.
fn validate_edge_manifold_impl(mesh: &IndexedMesh) -> Result<(), Vec<MeshEdgeManifoldError>> {
    if mesh.faces.is_empty() {
        return Err(vec![MeshEdgeManifoldError::EmptyMesh]);
    }

    let records = half_edges::sorted_half_edge_records(&mesh.faces);

    let mut errors: Vec<MeshEdgeManifoldError> = vec![];
    for run in records.chunk_by(|lhs, rhs| lhs.0 == rhs.0) {
        let edge = half_edges::key_vertices(run[0].0);
        match run.len() {
            1 => errors.push(MeshEdgeManifoldError::BoundaryEdge {
                edge,
                face_idx: half_edges::face_of(run[0].1),
            }),
            2 => {
                if half_edges::is_reversed(run[0].1) == half_edges::is_reversed(run[1].1) {
                    errors.push(MeshEdgeManifoldError::InconsistentWindingOrder {
                        edge,
                        face_indices: [
                            half_edges::face_of(run[0].1),
                            half_edges::face_of(run[1].1),
                        ],
                    });
                }
            }
            _ => errors.push(MeshEdgeManifoldError::NonManifoldEdge {
                edge,
                face_indices: run.iter().map(|&(_, v)| half_edges::face_of(v)).collect(),
            }),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod zero_area_face_display_tests {
    use super::*;

    #[test]
    fn zero_area_face_renders_face_id() {
        let msg = format!(
            "{}",
            ZeroAreaFace {
                face_idx: 17_usize.into(),
            },
        );
        assert_eq!(msg, "face #17 is degenerate: zero area");
    }
}

#[cfg(test)]
mod mesh_edge_manifold_error_display_tests {
    use super::*;

    #[test]
    fn empty_mesh_renders_short_phrase() {
        let msg = format!("{}", MeshEdgeManifoldError::EmptyMesh);
        assert_eq!(msg, "mesh has no faces");
    }

    #[test]
    fn boundary_edge_renders_edge_and_face() {
        let msg = format!(
            "{}",
            MeshEdgeManifoldError::BoundaryEdge {
                edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
                face_idx: 17_usize.into(),
            },
        );
        assert_eq!(
            msg,
            "edge (#0, #1) is shared by only face #17 (a manifold edge must be shared by exactly 2 faces)",
        );
    }

    #[test]
    fn non_manifold_edge_renders_edge_and_count() {
        let msg = format!(
            "{}",
            MeshEdgeManifoldError::NonManifoldEdge {
                edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
                face_indices: vec![0_usize.into(), 1_usize.into(), 2_usize.into(),],
            },
        );
        assert_eq!(
            msg,
            "edge (#0, #1) is shared by 3 faces (a manifold edge must be shared by exactly 2)",
        );
    }

    #[test]
    fn inconsistent_winding_renders_edge_and_both_face_ids() {
        let msg = format!(
            "{}",
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
                face_indices: [17_usize.into(), 42_usize.into()],
            },
        );
        assert_eq!(
            msg,
            "edge (#0, #1) is traversed in the same direction by faces #17 and #42 (windings disagree)",
        );
    }
}

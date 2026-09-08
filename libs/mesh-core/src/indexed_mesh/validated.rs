/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Lifetime-bound witnesses for mesh invariants, layered from weakest
//! to strongest:
//!
//! 1. [`ValidatedFaces`] / [`ValidatedFacesMut`]: every
//!    `face.vertices[k]` is `< mesh.vertices.len()`. Minted by
//!    `IndexedMesh::validated_faces` (or the `_mut` form), which runs
//!    `validate_faces` once and on success hands back a witness whose
//!    lifetime ties safe access to the still-valid borrow. Routines
//!    that index into `vertices` / `faces` without bounds checks live
//!    on the witness, so the in-range invariant is statically enforced
//!    rather than relying on the caller's discipline.
//! 2. [`ValidatedEdgeManifoldMesh`]: in addition, every edge is
//!    incident to exactly two faces and consistently oriented across
//!    that edge. Minted by [`ValidatedFaces::validated_edge_manifold`],
//!    which consumes the in-range witness, runs the edge-manifold
//!    check, and on success upgrades the borrow to the stronger
//!    witness. Vertex-manifoldness is deliberately NOT part of the
//!    claim: pinch vertices pass.
//!
//! Operations that preserve the in-range invariant
//! (`fix_faces_orientation`, either reverses a face's vertex order in
//! place or mutates its normal, and the same for the read-only
//! validators) are methods on the witnesses. `fix_zero_length_edges`
//! lives on `IndexedMesh` itself and minted-via-shortcut: see
//! `IndexedMesh::fix_zero_length_edges_into_validated`.

use std::fmt;

use float_cmp::approx_eq;

use crate::geometry::{
    DEGENERACY_SIN_SQ_THRESHOLD, NORMAL_ANGLE_TOLERANCE_COS, UNIT_NORM_TOLERANCE, gt_strict,
};
use crate::primitives::{FaceIndex, IndexedTriangle, Vector3D, Vertex, VertexIndex};

use super::fix::{MeshFix, MeshFixError};
use super::half_edges;
use super::{FixFacesOptions, IndexedMesh};

#[derive(Clone, Debug, PartialEq)]
pub enum MeshNormalVectorError {
    ZeroAreaTriangle { face_idx: FaceIndex },
    NonUnitNorm { face_idx: FaceIndex, norm: f32 },
    ExpectationMismatch { face_idx: FaceIndex },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshEdgeManifoldError {
    EmptyMesh,
    /// An edge that belongs to only one face (the mesh has a boundary).
    BoundaryEdge {
        edge: (VertexIndex, VertexIndex),
        face_idx: FaceIndex,
    },
    /// An edge shared by more than two faces.
    NonManifoldEdge {
        edge: (VertexIndex, VertexIndex),
        face_indices: Vec<FaceIndex>,
    },
    /// Two faces share an edge but traverse it in the same direction,
    /// meaning their normals are inconsistently oriented.
    InconsistentWindingOrder {
        edge: (VertexIndex, VertexIndex),
        face_indices: [FaceIndex; 2],
    },
}

impl fmt::Display for MeshNormalVectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroAreaTriangle { face_idx } => {
                write!(f, "face #{face_idx} is degenerate: zero area",)
            }
            Self::NonUnitNorm { face_idx, norm } => write!(
                f,
                "face #{face_idx} has a non-unit normal vector (norm = {norm})",
            ),
            Self::ExpectationMismatch { face_idx } => write!(
                f,
                "face #{face_idx} declares a normal vector that disagrees with the triangle plane",
            ),
        }
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

/// Read-only witness that the underlying `IndexedMesh`'s face indices
/// are all in range.
#[derive(Debug)]
pub struct ValidatedFaces<'a> {
    mesh: &'a IndexedMesh,
}

/// Mutable witness that the underlying `IndexedMesh`'s face indices
/// are all in range. Methods on the witness only perform mutations
/// that preserve that invariant.
#[derive(Debug)]
pub struct ValidatedFacesMut<'a> {
    mesh: &'a mut IndexedMesh,
}

impl<'a> ValidatedFaces<'a> {
    /// Construct from a mesh whose face indices have just been
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

    pub fn validate_normal_vectors(&self) -> Result<(), Vec<MeshNormalVectorError>> {
        validate_normal_vectors_impl(self.mesh)
    }

    pub fn validate_edge_manifold(&self) -> Result<(), Vec<MeshEdgeManifoldError>> {
        validate_edge_manifold_impl(self.mesh)
    }

    /// Run the edge-manifold validation and, on success, return a
    /// stronger witness whose existence proves the wrapped mesh is
    /// edge-manifold. Consumes `self` so the in-range witness is
    /// upgraded rather than duplicated; the resulting
    /// `ValidatedEdgeManifoldMesh<'a>` ties to the same
    /// `&'a IndexedMesh` borrow.
    pub fn validated_edge_manifold(
        self,
    ) -> Result<ValidatedEdgeManifoldMesh<'a>, Vec<MeshEdgeManifoldError>> {
        validate_edge_manifold_impl(self.mesh)?;
        Ok(ValidatedEdgeManifoldMesh::new_unchecked(self.mesh))
    }

    /// Compute the volume in cubic millimetres the mesh encloses,
    /// signed by its winding: positive when faces wind outward,
    /// negative when they wind inward.
    ///
    /// Only a closed surface encloses anything. An open one still
    /// returns a number, whatever its boundary leaves unbalanced, so
    /// callers wanting a physical volume should establish closure
    /// first through [`Self::validated_edge_manifold`].
    ///
    /// Vertices are `f32` but the sum is taken in `f64`, since each
    /// term scales with the mesh's distance from the origin while the
    /// total does not.
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

/// Read-only witness that the underlying `IndexedMesh` is
/// edge-manifold: every edge is incident to exactly two faces (no
/// boundary edges, no edges shared by three or more faces) and
/// consistently oriented across that edge (no two coincident
/// half-edges traversing the same direction). The in-range invariant
/// of [`ValidatedFaces`] is also carried by this witness, since the
/// only path to construct one is through
/// [`ValidatedFaces::validated_edge_manifold`].
///
/// What this witness does NOT guarantee:
/// - Vertex-manifoldness: a vertex may be a pinch where several
///   surface lobes touch at a single point, its incident faces then
///   forming several disjoint fans. Such a mesh still bounds a
///   well-defined solid; consumers that need the per-vertex fan
///   structure get it from `MeshTopology`.
/// - Per-face normal vectors are correct (covered by
///   [`ValidatedFaces::validate_normal_vectors`]).
/// - Every triangle has non-zero area / no zero-length edges (covered
///   by `validate_faces`).
/// - Any particular global winding convention (CCW outward, etc.):
///   edge-manifold inputs may still be globally inside-out.
#[derive(Debug)]
pub struct ValidatedEdgeManifoldMesh<'a> {
    mesh: &'a IndexedMesh,
}

impl<'a> ValidatedEdgeManifoldMesh<'a> {
    /// Mint an edge-manifold witness from a mesh whose
    /// edge-manifoldness has just been confirmed. `pub(super)` so the
    /// only public path is via
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

    pub fn validate_normal_vectors(&self) -> Result<(), Vec<MeshNormalVectorError>> {
        validate_normal_vectors_impl(self.mesh)
    }

    pub fn validate_edge_manifold(&self) -> Result<(), Vec<MeshEdgeManifoldError>> {
        validate_edge_manifold_impl(self.mesh)
    }

    /// Repair winding-order and/or normal-vector errors. The flip
    /// pass reverses a face's three vertex indices in place; the
    /// normal-recompute pass replaces `face.normal` only. Neither
    /// touches `vertices.len()` nor introduces new
    /// `face.vertices[k]` values, so the in-range invariant the
    /// witness encodes is preserved across the call: `self`
    /// remains a valid witness afterwards.
    pub fn fix_faces_orientation(
        &mut self,
        manifold_errors: &[MeshEdgeManifoldError],
        normal_vector_errors: &[MeshNormalVectorError],
        options: FixFacesOptions,
    ) -> Result<Vec<MeshFix>, MeshFixError> {
        super::fix::fix_faces_orientation_impl(
            self.mesh,
            manifold_errors,
            normal_vector_errors,
            options,
        )
    }
}

/// Body of `validate_normal_vectors`, factored out so both
/// `ValidatedFaces` and `ValidatedFacesMut` can call it. The
/// in-range invariant is what makes the unguarded `self.vertices[i]`
/// indexing inside this routine safe; both witness types only call
/// this on a mesh whose `validate_faces` returned `Ok`.
fn validate_normal_vectors_impl(mesh: &IndexedMesh) -> Result<(), Vec<MeshNormalVectorError>> {
    let mut errors: Vec<MeshNormalVectorError> = vec![];

    for (idx, face) in mesh.faces.iter().enumerate() {
        let face_idx = FaceIndex::from(idx);
        let [i0, i1, i2] = face.vertices.map(usize::from);

        let v0 = mesh.vertices[i0].to_f64();
        let v1 = mesh.vertices[i1].to_f64();
        let v2 = mesh.vertices[i2].to_f64();

        let e1 = v1.diff(&v0);
        let e2 = v2.diff(&v0);

        let cross = e1.cross_product(&e2);
        let cross_mag_sq = cross.squared_norm();
        let edges_mag_sq_product = e1.squared_norm() * e2.squared_norm();

        // degenerate (zero-area) triangles; also handles NaN coordinates.
        if !gt_strict(
            cross_mag_sq,
            DEGENERACY_SIN_SQ_THRESHOLD * edges_mag_sq_product,
        ) {
            errors.push(MeshNormalVectorError::ZeroAreaTriangle { face_idx });
            continue;
        }

        let n = face.normal.to_f64();
        let n_mag_sq = n.squared_norm();

        if !approx_eq!(f64, n_mag_sq, 1.0, epsilon = UNIT_NORM_TOLERANCE) {
            errors.push(MeshNormalVectorError::NonUnitNorm {
                face_idx,
                norm: n_mag_sq.sqrt() as f32,
            });
            continue;
        }

        let cross_mag = cross_mag_sq.sqrt();
        let cross = Vector3D::new([
            cross[0] / cross_mag,
            cross[1] / cross_mag,
            cross[2] / cross_mag,
        ]);

        let dot = cross.dot_product(&n);
        if dot < NORMAL_ANGLE_TOLERANCE_COS {
            errors.push(MeshNormalVectorError::ExpectationMismatch { face_idx });
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Body of `validate_edge_manifold`. Strictly speaking this routine never
/// indexes into `mesh.vertices` (it only uses `face.vertices` values
/// as half-edge keys), so the in-range invariant is not a soundness
/// requirement here: the witness gates this method only for API
/// symmetry with `validate_normal_vectors`.
///
/// The directed half-edges are sorted so each undirected edge forms
/// one contiguous run: run length 1 is a boundary edge, more than 2 an
/// over-shared edge, and a length-2 run whose two faces traverse the
/// edge in the same direction an inconsistent winding. Errors come out
/// sorted by canonical edge (deterministic across runs), with the face
/// lists inside each error ascending.
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
mod indexed_mesh_validated_faces_tests {
    use super::*;
    use crate::indexed_mesh::validate::MeshFaceError;
    use crate::primitives::{Normal, TriVertexSlot};

    fn face(v0: u32, v1: u32, v2: u32) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [v0, v1, v2])
    }

    fn make_mesh(num_vertices: usize, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0]); num_vertices],
            faces,
        }
    }

    #[test]
    fn validated_faces_succeeds_on_clean_mesh() {
        let mesh = make_mesh(3, vec![face(0, 1, 2)]);
        let validated = mesh.validated_faces().expect("clean mesh");
        assert_eq!(validated.vertices().len(), 3);
        assert_eq!(validated.faces().len(), 1);
    }

    #[test]
    fn validated_faces_returns_face_errors_on_out_of_range_indices() {
        let mesh = make_mesh(3, vec![face(0, 1, 99)]);
        let errors = mesh.validated_faces().expect_err("out-of-range index");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, MeshFaceError::OutOfRangeVertexIndex { .. }))
        );
    }

    #[test]
    fn validated_faces_returns_face_errors_on_zero_length_edge() {
        let mesh = make_mesh(3, vec![face(0, 1, 1)]);
        let errors = mesh.validated_faces().expect_err("zero-length edge");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, MeshFaceError::ZeroLengthEdge { .. }))
        );
    }

    #[test]
    fn validated_faces_mut_succeeds_on_clean_mesh() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
        let _ = mesh.validated_faces_mut().expect("clean mesh");
    }

    /// Pin the shortcut: when every input error is a
    /// `ZeroLengthEdge`, removing the affected faces preserves the
    /// in-range invariant for survivors and `validated_faces_mut`
    /// is *not* re-run. We cannot easily observe "didn't re-run",
    /// but we can observe the post-condition: the returned witness
    /// must be `Ok` for a mesh whose only flaw was zero-length
    /// edges.
    #[test]
    fn fix_zero_length_edges_into_validated_returns_ok_witness_when_only_zle() {
        let mut mesh = make_mesh(4, vec![face(0, 1, 1), face(0, 1, 2)]);
        let face_errors = mesh.validate_faces().expect_err("zle");
        let (fixes, witness) = mesh.fix_zero_length_edges_into_validated(&face_errors);
        assert_eq!(fixes.len(), face_errors.len());
        let witness = witness.expect("post-removal mesh validates");
        assert_eq!(witness.faces().len(), 1);
        assert_eq!(witness.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    /// Pin the slow path: when the input errors include a non-ZLE
    /// variant, the routine re-runs `validate_faces` to confirm the
    /// post-removal mesh actually validates. Here we hand-craft an
    /// error vector that includes an `OutOfRangeVertexIndex` for an
    /// otherwise-clean mesh: the post-removal validation still
    /// returns `Ok`, the witness is `Ok`, but the path that
    /// produced it is the re-validate one.
    #[test]
    fn fix_zero_length_edges_into_validated_revalidates_when_mixed_errors() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
        let synthetic_errors = vec![MeshFaceError::OutOfRangeVertexIndex {
            face_idx: 0_usize.into(),
            vertex_pos: TriVertexSlot::V0,
            vertex_idx: VertexIndex::from(99_u32),
        }];
        let (fixes, witness) = mesh.fix_zero_length_edges_into_validated(&synthetic_errors);
        assert!(fixes.is_empty());
        let witness = witness.expect("post-removal mesh validates");
        assert_eq!(witness.faces().len(), 1);
    }

    /// Pin: a real out-of-range face combined with the slow path
    /// (any non-ZLE error in the input) makes the witness `Err`.
    #[test]
    fn fix_zero_length_edges_into_validated_returns_err_when_post_removal_invalid() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 99)]);
        let face_errors = mesh.validate_faces().expect_err("oor");
        let (fixes, witness) = mesh.fix_zero_length_edges_into_validated(&face_errors);
        assert!(fixes.is_empty());
        let _ = witness.expect_err("post-removal mesh still has out-of-range index");
    }

    /// Hand-rolled closed-tetrahedron mesh with consistent outward
    /// winding, mirroring the helper in
    /// `indexed_mesh_validate_edge_manifold_tests`.
    fn closed_tetrahedron() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 1.0, 1.0]),
                Vertex::new([1.0, -1.0, -1.0]),
                Vertex::new([-1.0, 1.0, -1.0]),
                Vertex::new([-1.0, -1.0, 1.0]),
            ],
            faces: vec![face(0, 1, 2), face(0, 3, 1), face(0, 2, 3), face(1, 3, 2)],
        }
    }

    #[test]
    fn validated_edge_manifold_succeeds_on_closed_mesh() {
        let mesh = closed_tetrahedron();
        let witness = mesh
            .validated_faces()
            .expect("in-range indices")
            .validated_edge_manifold()
            .expect("manifold tetrahedron");
        assert_eq!(witness.vertices().len(), 4);
        assert_eq!(witness.faces().len(), 4);
    }

    #[test]
    fn validated_edge_manifold_round_trips_buffers() {
        let mesh = closed_tetrahedron();
        let witness = mesh
            .validated_faces()
            .unwrap()
            .validated_edge_manifold()
            .unwrap();
        assert_eq!(witness.vertices(), mesh.vertices.as_slice());
        assert_eq!(witness.faces(), mesh.faces.as_slice());
        assert!(std::ptr::eq(witness.as_indexed_mesh(), &mesh));
    }

    #[test]
    fn validated_edge_manifold_fails_on_boundary_edge() {
        // Two triangles glued along one edge: a non-closed strip whose
        // remaining edges are boundary edges, which the manifold check
        // rejects.
        let mesh = make_mesh(4, vec![face(0, 1, 2), face(1, 0, 3)]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validated_edge_manifold()
            .expect_err("open strip");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, MeshEdgeManifoldError::BoundaryEdge { .. }))
        );
    }
}

#[cfg(test)]
mod mesh_normal_vector_error_display_tests {
    use super::*;

    #[test]
    fn zero_area_triangle_renders_face_id() {
        let msg = format!(
            "{}",
            MeshNormalVectorError::ZeroAreaTriangle {
                face_idx: 17_usize.into(),
            },
        );
        assert_eq!(msg, "face #17 is degenerate: zero area");
    }

    #[test]
    fn non_unit_norm_renders_face_id_and_norm() {
        let msg = format!(
            "{}",
            MeshNormalVectorError::NonUnitNorm {
                face_idx: 17_usize.into(),
                norm: 1.5,
            },
        );
        assert_eq!(msg, "face #17 has a non-unit normal vector (norm = 1.5)",);
    }

    #[test]
    fn expectation_mismatch_renders_face_id() {
        let msg = format!(
            "{}",
            MeshNormalVectorError::ExpectationMismatch {
                face_idx: 42_usize.into(),
            },
        );
        assert_eq!(
            msg,
            "face #42 declares a normal vector that disagrees with the triangle plane",
        );
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

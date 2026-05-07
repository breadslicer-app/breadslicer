/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Lifetime-bound witnesses that encode the invariant
//! "every `face.vertices[k]` value is < `mesh.vertices.len()`".
//!
//! `IndexedMesh::validated_faces` and `validated_faces_mut` consume
//! the mesh borrow, run `validate_faces` once, and on success return a
//! handle whose lifetime ties safe access to the still-valid borrow.
//! Routines that index into `vertices` / `faces` without bounds checks
//! live on the witness, so the in-range invariant is statically
//! enforced rather than relying on the caller's discipline.
//!
//! Operations that preserve the invariant (`fix_faces_orientation`,
//! either reverses a face's vertex order in place or mutates its
//! normal, and the same for the read-only validators) are methods on
//! the witnesses. `fix_zero_length_edges` lives on `IndexedMesh`
//! itself and minted-via-shortcut: see
//! `IndexedMesh::fix_zero_length_edges_into_validated`.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use float_cmp::approx_eq;

use crate::errors::{FaceIndex, MeshFix, MeshManifoldError, MeshNormalVectorError, VertexIndex};
use crate::geometry::{
    DEGENERACY_SIN_SQ_THRESHOLD, NORMAL_ANGLE_TOLERANCE_COS, UNIT_NORM_TOLERANCE, gt_strict,
};
use crate::primitives::{IndexedTriangle, Vector3D, Vertex};

use super::{FixFacesOptions, IndexedMesh};

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

    pub fn validate_manifold(&self) -> Result<(), Vec<MeshManifoldError>> {
        validate_manifold_impl(self.mesh)
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

    pub fn validate_manifold(&self) -> Result<(), Vec<MeshManifoldError>> {
        validate_manifold_impl(self.mesh)
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
        manifold_errors: &[MeshManifoldError],
        normal_vector_errors: &[MeshNormalVectorError],
        options: FixFacesOptions,
    ) -> Result<Vec<MeshFix>> {
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

/// Body of `validate_manifold`. Strictly speaking this routine never
/// indexes into `mesh.vertices` (it only uses `face.vertices` values
/// as half-edge keys), so the in-range invariant is not a soundness
/// requirement here: the witness gates this method only for API
/// symmetry with `validate_normal_vectors`.
fn validate_manifold_impl(mesh: &IndexedMesh) -> Result<(), Vec<MeshManifoldError>> {
    let mut errors: Vec<MeshManifoldError> = vec![];

    let mut half_edges: HashMap<(VertexIndex, VertexIndex), Vec<FaceIndex>> = HashMap::new();

    if mesh.faces.is_empty() {
        errors.push(MeshManifoldError::EmptyMesh);
        return Err(errors);
    }

    for (idx, face) in mesh.faces.iter().enumerate() {
        let face_idx = FaceIndex::from(idx);
        let [v0, v1, v2] = face.vertices;
        for &(a, b) in &[(v0, v1), (v1, v2), (v2, v0)] {
            half_edges.entry((a, b)).or_default().push(face_idx);
        }
    }

    let mut checked: HashSet<(VertexIndex, VertexIndex)> = HashSet::new();

    for &(a, b) in half_edges.keys() {
        let canonical = if a <= b { (a, b) } else { (b, a) };
        if !checked.insert(canonical) {
            continue;
        }

        let forward = half_edges.get(&(a, b)).cloned().unwrap_or_default();
        let reverse = half_edges.get(&(b, a)).cloned().unwrap_or_default();

        let total = forward.len() + reverse.len();

        if total < 2 {
            let face_idx = forward.first().or(reverse.first()).copied().unwrap();
            errors.push(MeshManifoldError::BoundaryEdge {
                edge: canonical,
                face_idx,
            });
        } else if total > 2 {
            let mut face_indices = forward.clone();
            face_indices.extend(&reverse);
            errors.push(MeshManifoldError::NonManifoldEdge {
                edge: canonical,
                face_indices,
            });
        } else if forward.len() == 2 {
            errors.push(MeshManifoldError::InconsistentWindingOrder {
                edge: canonical,
                face_indices: [forward[0], forward[1]],
            });
        } else if reverse.len() == 2 {
            errors.push(MeshManifoldError::InconsistentWindingOrder {
                edge: canonical,
                face_indices: [reverse[0], reverse[1]],
            });
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
    use crate::errors::{MeshFaceError, TriVertexSlot, VertexIndex};
    use crate::primitives::Normal;

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
}

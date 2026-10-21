/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-mesh diagnosis policy: which validators run, in which order, and what
//! happens when the face gate fails. The validators themselves live in
//! [`super::validate`] and [`super::validated`]; this module owns the read-side
//! decisions around them so every frontend diagnoses meshes the same way.

use super::mesh::IndexedMesh;
use super::validate::{MeshFaceError, MeshVertexError};
use super::validated::{MeshEdgeManifoldError, ZeroAreaFace};

/// The face gate's result. The face-area and edge-manifold validators run on
/// the witness face validation mints, so when the gate fails their findings do
/// not exist for this mesh: absent by construction, not empty.
#[derive(Clone, Debug, PartialEq)]
pub enum FaceDiagnosis {
    /// Face validation failed; the witness-gated validators could not run.
    Invalid(Vec<MeshFaceError>),
    /// Face validation passed; both witness-gated validators ran (they are
    /// independent of each other).
    Valid {
        /// Faces with zero area.
        zero_area_faces: Vec<ZeroAreaFace>,
        /// Edges breaking the 2-faces-per-edge or consistent-winding rules.
        edge_manifold_errors: Vec<MeshEdgeManifoldError>,
    },
}

/// Everything a diagnosis found in a mesh. Produced by
/// [`IndexedMesh::diagnose`]; diagnosing never mutates the mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshDiagnosis {
    /// Non-finite vertex coordinates.
    pub vertex_errors: Vec<MeshVertexError>,
    /// The face gate's result, carrying the witness-gated validators' findings
    /// when the gate passed.
    pub faces: FaceDiagnosis,
}

impl MeshDiagnosis {
    /// Returns whether the diagnosis found anything at all.
    pub fn has_problems(&self) -> bool {
        if !self.vertex_errors.is_empty() {
            return true;
        }
        match &self.faces {
            FaceDiagnosis::Invalid(_) => true,
            FaceDiagnosis::Valid {
                zero_area_faces,
                edge_manifold_errors,
            } => !zero_area_faces.is_empty() || !edge_manifold_errors.is_empty(),
        }
    }
}

impl IndexedMesh {
    /// Runs every validator over this mesh in the order the frontends present
    /// them: vertices first (they never gate the rest), then the face gate;
    /// when the gate passes, the face-area and edge-manifold validators both
    /// run.
    pub fn diagnose(&self) -> MeshDiagnosis {
        let vertex_errors = self.validate_vertices().err().unwrap_or_default();
        let faces = match self.validated_faces() {
            Err(face_errors) => FaceDiagnosis::Invalid(face_errors),
            Ok(validated) => FaceDiagnosis::Valid {
                zero_area_faces: validated.validate_face_areas().err().unwrap_or_default(),
                edge_manifold_errors: validated.validate_edge_manifold().err().unwrap_or_default(),
            },
        };
        MeshDiagnosis {
            vertex_errors,
            faces,
        }
    }
}

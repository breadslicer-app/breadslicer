/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-mesh repair policy: which passes run, in which order, and when
//! repair is refused. The repair primitives (zero-length-edge face
//! removal, face orientation fixes) live in [`super::fix`]; this
//! module owns the decisions around them so every frontend repairs
//! meshes the same way.

use std::fmt;

use super::fix::{MeshFix, MeshFixError};
use super::validate::{MeshFaceError, MeshVertexError};
use super::{FixFacesOptions, IndexedMesh};

/// Which repair passes [`IndexedMesh::repair`] runs. Each flag
/// enables an independent pass; passes that find nothing to repair
/// are no-ops. `Default` is all-off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepairPasses {
    /// Remove faces that contain a zero-length edge.
    pub zero_length_edges: bool,
    /// Reverse the vertex order of inconsistently wound faces.
    pub flipped_faces: bool,
    /// Recompute stored normal vectors that disagree with the
    /// geometry.
    pub face_normals: bool,
}

impl RepairPasses {
    /// The default repair pass set: remove zero-length-edge faces and
    /// correct flipped faces; leave normal recomputation off. The
    /// single source for every frontend's default (CLI `fix` flag
    /// fallbacks, `slice --repair`, the GUI repair action).
    pub const fn default_repair() -> Self {
        Self {
            zero_length_edges: true,
            flipped_faces: true,
            face_normals: false,
        }
    }
}

/// Why [`IndexedMesh::repair`] refused to run, or to finish, its
/// passes.
#[derive(Clone, Debug, PartialEq)]
pub enum RepairRefusal {
    /// The mesh has non-finite vertex coordinates; no pass can run.
    /// The mesh is left untouched.
    InvalidVertices(Vec<MeshVertexError>),
    /// Face errors the enabled passes cannot address: all of them
    /// when the zero-length-edge pass is disabled, otherwise the
    /// residue that is not a zero-length edge. The mesh is left
    /// untouched.
    UnfixableFaceErrors(Vec<MeshFaceError>),
    /// The orientation pass failed while planning its mutations (see
    /// [`MeshFixError`]). Fixes applied by earlier passes remain in
    /// place; the orientation pass itself left the mesh untouched
    /// (the primitive is transactional).
    OrientationFixFailed(MeshFixError),
}

impl fmt::Display for RepairRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVertices(errors) => {
                write!(f, "mesh has {} invalid vertex coordinates", errors.len())
            }
            Self::UnfixableFaceErrors(errors) => {
                write!(
                    f,
                    "{} face errors the enabled repair passes cannot address",
                    errors.len()
                )
            }
            Self::OrientationFixFailed(err) => {
                write!(f, "face orientation fix failed: {err}")
            }
        }
    }
}

impl std::error::Error for RepairRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::OrientationFixFailed(err) => Some(err),
            Self::InvalidVertices(_) | Self::UnfixableFaceErrors(_) => None,
        }
    }
}

/// Everything one [`IndexedMesh::repair`] run did, and did not do, to
/// a mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshRepairReport {
    /// Fixes actually applied, in pass order. Can be non-empty even
    /// when `refusal` is set: a pass-level refusal keeps the fixes
    /// earlier passes applied.
    pub fixes: Vec<MeshFix>,
    /// Why the run stopped early, if it did. `None` means every
    /// enabled pass ran to completion; it does NOT mean the mesh is
    /// problem-free (problems whose pass is disabled, and problems no
    /// pass addresses, such as holes, remain untouched).
    pub refusal: Option<RepairRefusal>,
    /// Whether the mesh is edge-manifold AFTER the passes ran,
    /// obtained by attempting the edge-manifold witness mint: the
    /// same check the scene health sweep uses. Purely combinatorial;
    /// it says nothing about vertex validity or normal correctness.
    pub edge_manifold_after: bool,
}

/// What a repair run would do to a mesh, decided on a shared borrow
/// (see [`assess_repair`]).
#[derive(Clone, Debug, PartialEq)]
pub enum RepairAssessment {
    /// Every enabled pass would find nothing to do.
    Clean,
    /// The run would be refused; the mesh would not change.
    Refused(RepairRefusal),
    /// At least one enabled pass would mutate the mesh.
    Work,
}

/// Decide, without mutating anything, what [`IndexedMesh::repair`]
/// would do to `mesh` under `passes`. Callers that manage shared
/// storage (the scene driver) use this to fork only meshes that will
/// actually change. The gates mirror `run_repair_passes` exactly,
/// orientation planning included, so the assessment and the report
/// can never disagree; the assessment-equivalence pins keep the pair
/// in lockstep.
pub fn assess_repair(mesh: &IndexedMesh, passes: RepairPasses) -> RepairAssessment {
    if let Err(errors) = mesh.validate_vertices() {
        return RepairAssessment::Refused(RepairRefusal::InvalidVertices(errors));
    }
    let witness = match mesh.validated_faces() {
        Ok(witness) => witness,
        Err(face_errors) => {
            if !passes.zero_length_edges {
                return RepairAssessment::Refused(RepairRefusal::UnfixableFaceErrors(face_errors));
            }
            let residue: Vec<MeshFaceError> = face_errors
                .iter()
                .filter(|e| !matches!(e, MeshFaceError::ZeroLengthEdge { .. }))
                .cloned()
                .collect();
            if !residue.is_empty() {
                return RepairAssessment::Refused(RepairRefusal::UnfixableFaceErrors(residue));
            }
            // Every face error is a zero-length edge and the pass is
            // enabled: faces will be removed.
            return RepairAssessment::Work;
        }
    };
    let normal_vector_errors = witness.validate_normal_vectors().err().unwrap_or_default();
    let manifold_errors = witness.validate_edge_manifold().err().unwrap_or_default();
    if normal_vector_errors.is_empty() && manifold_errors.is_empty() {
        return RepairAssessment::Clean;
    }
    match super::fix::plan_faces_orientation_impl(
        mesh,
        &manifold_errors,
        &normal_vector_errors,
        FixFacesOptions {
            flipped: passes.flipped_faces,
            normals: passes.face_normals,
        },
    ) {
        Err(err) => RepairAssessment::Refused(RepairRefusal::OrientationFixFailed(err)),
        Ok(plan) if plan.is_empty() => RepairAssessment::Clean,
        Ok(_) => RepairAssessment::Work,
    }
}

impl IndexedMesh {
    /// Run the enabled repair passes over this mesh and report what
    /// happened.
    ///
    /// Pass order and refusal semantics:
    /// 1. Vertex gate: non-finite coordinates refuse the whole run
    ///    with the mesh untouched.
    /// 2. Face gate, decided before any mutation: when face
    ///    validation fails and the enabled passes cannot address
    ///    every error, the run is refused with the mesh untouched.
    /// 3. Zero-length-edge pass: removes the offending faces.
    /// 4. Orientation stage: winding and normal validations always
    ///    run; the flip / normal fixes fire per `passes`. A failure
    ///    here refuses the rest of the run but keeps the fixes
    ///    already applied; the mesh never holds a half-applied pass.
    ///
    /// Every path is total: no panic, and at every observable point
    /// the mesh is at least as valid as it was before the call.
    pub fn repair(&mut self, passes: RepairPasses) -> MeshRepairReport {
        let (fixes, refusal) = self.run_repair_passes(passes);
        let edge_manifold_after = self
            .validated_faces()
            .is_ok_and(|faces| faces.validated_edge_manifold().is_ok());
        MeshRepairReport {
            fixes,
            refusal,
            edge_manifold_after,
        }
    }

    fn run_repair_passes(&mut self, passes: RepairPasses) -> (Vec<MeshFix>, Option<RepairRefusal>) {
        if let Err(errors) = self.validate_vertices() {
            return (Vec::new(), Some(RepairRefusal::InvalidVertices(errors)));
        }

        let mut fixes: Vec<MeshFix> = Vec::new();

        let mut validated = match self.validated_faces_mut() {
            Ok(witness) => witness,
            Err(face_errors) => {
                // Refusals are decided BEFORE any mutation: with the
                // zero-length-edge pass disabled every face error is
                // unaddressable; with it enabled, any non-ZLE residue
                // refuses the run while the mesh is still untouched.
                if !passes.zero_length_edges {
                    return (fixes, Some(RepairRefusal::UnfixableFaceErrors(face_errors)));
                }
                let residue: Vec<MeshFaceError> = face_errors
                    .iter()
                    .filter(|e| !matches!(e, MeshFaceError::ZeroLengthEdge { .. }))
                    .cloned()
                    .collect();
                if !residue.is_empty() {
                    return (fixes, Some(RepairRefusal::UnfixableFaceErrors(residue)));
                }
                let (edge_fixes, witness) = self.fix_zero_length_edges_into_validated(&face_errors);
                fixes.extend(edge_fixes);
                match witness {
                    Ok(w) => w,
                    // Unreachable when every input error was a
                    // zero-length edge (whole-face removal preserves
                    // the in-range invariant); mapped instead of
                    // panicking so the function stays total.
                    Err(residual) => {
                        return (fixes, Some(RepairRefusal::UnfixableFaceErrors(residual)));
                    }
                }
            }
        };

        let normal_vector_errors = validated
            .validate_normal_vectors()
            .err()
            .unwrap_or_default();
        let manifold_errors = validated.validate_edge_manifold().err().unwrap_or_default();

        if !normal_vector_errors.is_empty() || !manifold_errors.is_empty() {
            match validated.fix_faces_orientation(
                &manifold_errors,
                &normal_vector_errors,
                FixFacesOptions {
                    flipped: passes.flipped_faces,
                    normals: passes.face_normals,
                },
            ) {
                Ok(orientation_fixes) => fixes.extend(orientation_fixes),
                // Pass-level refusal: the orientation primitive is
                // transactional, so the mesh holds exactly the
                // completed passes' state and their fixes stay in
                // the report.
                Err(err) => {
                    return (fixes, Some(RepairRefusal::OrientationFixFailed(err)));
                }
            }
        }

        (fixes, None)
    }
}

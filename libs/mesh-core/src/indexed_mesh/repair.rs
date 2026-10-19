/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-mesh repair policy: which passes run, in which order, and when repair is
//! refused. The repair primitives (zero-length-edge face removal, face
//! orientation fixes) live in [`super::fix`]; this module owns the decisions
//! around them so every frontend repairs meshes the same way.

use std::fmt;

use super::fix::MeshFixError;
use super::fix::records::MeshFix;
use super::mesh::IndexedMesh;
use super::validate::{MeshFaceError, MeshVertexError};
use super::validated::ZeroAreaFace;

/// Which repair passes [`IndexedMesh::repair`] runs. Each flag enables an
/// independent pass; passes that find nothing to repair are no-ops. `Default`
/// is all-off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepairPasses {
    /// Removes faces that contain a zero-length edge.
    pub zero_length_edges: bool,
    /// Reverses the vertex order of inconsistently wound faces.
    pub flipped_faces: bool,
}

impl RepairPasses {
    /// Returns the default pass set: zero-length-edge face removal and flipped
    /// face correction.
    pub const fn default_repair() -> Self {
        Self {
            zero_length_edges: true,
            flipped_faces: true,
        }
    }
}

/// Why [`IndexedMesh::repair`] refused to run, or to finish, its passes.
#[derive(Clone, Debug, PartialEq)]
pub enum RepairRefusal {
    /// The mesh has non-finite vertex coordinates; no pass can run. The mesh is
    /// left untouched.
    InvalidVertices(Vec<MeshVertexError>),
    /// Face errors the enabled passes cannot address: all of them when the
    /// zero-length-edge pass is disabled, otherwise the residue that is not a
    /// zero-length edge. The mesh is left untouched.
    UnfixableFaceErrors(Vec<MeshFaceError>),
    /// The orientation pass failed while planning its mutations (see
    /// [`MeshFixError`]). Fixes applied by earlier passes remain in place; the
    /// orientation pass itself left the mesh untouched (the primitive is
    /// transactional).
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

/// Everything a [`IndexedMesh::repair`] run did, and did not do, to a mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshRepairReport {
    /// Fixes applied, in pass order. Can be non-empty also when the run ends
    /// refused: a pass-level refusal keeps the fixes earlier passes applied.
    pub fixes: Vec<MeshFix>,
    /// How the run ended.
    pub end: RepairEnd,
}

/// How a [`IndexedMesh::repair`] run ended.
#[derive(Clone, Debug, PartialEq)]
pub enum RepairEnd {
    /// The run stopped early.
    Refused(RepairRefusal),
    /// Every enabled pass ran to completion. This does not mean that the mesh
    /// is problem-free: problems whose pass is disabled, and problems that no
    /// pass addresses, such as holes, remain untouched.
    Completed(MeshStateAfterRepair),
}

/// The state of a mesh after a completed [`IndexedMesh::repair`] run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshStateAfterRepair {
    /// Whether the mesh is edge-manifold. The check is combinatorial: it does
    /// not look at the vertex positions.
    pub edge_manifold: bool,
    /// How many faces have zero area (see [`ZeroAreaFace`]).
    pub zero_area_face_count: usize,
}

/// What a repair run would do to a mesh, decided on a shared borrow (see
/// [`assess_repair`]).
#[derive(Clone, Debug, PartialEq)]
pub enum RepairAssessment {
    /// Every enabled pass would find nothing to do. Holds the state that the
    /// run reports.
    Clean(MeshStateAfterRepair),
    /// The run would be refused; the mesh would not change.
    Refused(RepairRefusal),
    /// At least 1 enabled pass would mutate the mesh.
    Work,
}

/// Decides, without changing anything, what [`IndexedMesh::repair`] would do to
/// `mesh` under `passes`. The assessment always agrees with the report of a
/// repair run with the same passes.
pub fn assess_repair(mesh: &IndexedMesh, passes: RepairPasses) -> RepairAssessment {
    // Mirrors the gates of `run_repair_passes`, orientation planning included:
    // a change to either must go to both.
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
            // Every face error is a zero-length edge and the pass is enabled:
            // faces will be removed.
            return RepairAssessment::Work;
        }
    };
    let edge_manifold = witness.validate_edge_manifold();
    if passes.flipped_faces
        && let Err(manifold_errors) = &edge_manifold
    {
        return match super::fix::plan_faces_orientation_impl(mesh, manifold_errors) {
            Err(err) => RepairAssessment::Refused(RepairRefusal::OrientationFixFailed(err)),
            Ok(faces_to_flip) if faces_to_flip.is_empty() => {
                RepairAssessment::Clean(state_after_repair(false, witness.validate_face_areas()))
            }
            Ok(_) => RepairAssessment::Work,
        };
    }
    RepairAssessment::Clean(state_after_repair(
        edge_manifold.is_ok(),
        witness.validate_face_areas(),
    ))
}

fn state_after_repair(
    edge_manifold: bool,
    face_areas: Result<(), Vec<ZeroAreaFace>>,
) -> MeshStateAfterRepair {
    MeshStateAfterRepair {
        edge_manifold,
        zero_area_face_count: face_areas.err().map_or(0, |faces| faces.len()),
    }
}

impl IndexedMesh {
    /// Runs the enabled repair passes over this mesh and reports what happened.
    ///
    /// Pass order and refusal semantics:
    /// 1. Vertex gate: non-finite coordinates refuse the whole run with the
    ///    mesh untouched.
    /// 2. Face gate, decided before any mutation: when face validation fails
    ///    and the enabled passes cannot address every error, the run is refused
    ///    with the mesh untouched.
    /// 3. Zero-length-edge pass: removes the offending faces.
    /// 4. Flipped-face pass: reverses the winding of inconsistently wound
    ///    faces. A failure here refuses the rest of the run but keeps the fixes
    ///    already applied; the mesh never holds a half-applied pass.
    ///
    /// When a pass changes the mesh, the declared facet normals in its metadata
    /// are dropped.
    ///
    /// Every path is total: no panic, and at every observable point the mesh is
    /// at least as valid as it was before the call.
    pub fn repair(&mut self, passes: RepairPasses) -> MeshRepairReport {
        let (fixes, end) = self.run_repair_passes(passes);
        if !fixes.is_empty()
            && let Some(meta) = &mut self.meta
        {
            meta.declared_facet_normals = None;
        }
        MeshRepairReport { fixes, end }
    }

    fn run_repair_passes(&mut self, passes: RepairPasses) -> (Vec<MeshFix>, RepairEnd) {
        if let Err(errors) = self.validate_vertices() {
            return (
                Vec::new(),
                RepairEnd::Refused(RepairRefusal::InvalidVertices(errors)),
            );
        }

        let mut fixes: Vec<MeshFix> = Vec::new();

        let mut validated = match self.validated_faces_mut() {
            Ok(witness) => witness,
            Err(face_errors) => {
                // Refusals are decided before any mutation: with the
                // zero-length-edge pass disabled every face error is
                // unaddressable; with it enabled, any residue that is not a
                // zero-length edge refuses the run while the mesh is still
                // untouched.
                if !passes.zero_length_edges {
                    return (
                        fixes,
                        RepairEnd::Refused(RepairRefusal::UnfixableFaceErrors(face_errors)),
                    );
                }
                let residue: Vec<MeshFaceError> = face_errors
                    .iter()
                    .filter(|e| !matches!(e, MeshFaceError::ZeroLengthEdge { .. }))
                    .cloned()
                    .collect();
                if !residue.is_empty() {
                    return (
                        fixes,
                        RepairEnd::Refused(RepairRefusal::UnfixableFaceErrors(residue)),
                    );
                }
                let (edge_fixes, witness) = self.fix_zero_length_edges_into_validated(&face_errors);
                fixes.extend(edge_fixes);
                match witness {
                    Ok(w) => w,
                    // Unreachable when every input error was a zero-length edge
                    // (whole-face removal preserves the in-range invariant);
                    // mapped instead of panicking so the function stays total.
                    Err(residual) => {
                        return (
                            fixes,
                            RepairEnd::Refused(RepairRefusal::UnfixableFaceErrors(residual)),
                        );
                    }
                }
            }
        };

        if passes.flipped_faces
            && let Err(manifold_errors) = validated.validate_edge_manifold()
        {
            match validated.fix_faces_orientation(&manifold_errors) {
                Ok(orientation_fixes) => fixes.extend(orientation_fixes),
                // Pass-level refusal: the orientation primitive is
                // transactional, so the mesh holds exactly the completed
                // passes' state and their fixes stay in the report.
                Err(err) => {
                    return (
                        fixes,
                        RepairEnd::Refused(RepairRefusal::OrientationFixFailed(err)),
                    );
                }
            }
        }

        let state = state_after_repair(
            validated.validate_edge_manifold().is_ok(),
            validated.validate_face_areas(),
        );
        (fixes, RepairEnd::Completed(state))
    }
}

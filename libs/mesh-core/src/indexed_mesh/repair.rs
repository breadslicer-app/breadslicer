/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL, 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-mesh repair policy: which passes run, in which order, and when repair is
//! refused. The repair primitives (invalid-face removal, the watertight pass,
//! the orientation pass, collinear-face removal) live in
//! [`super::fix`]; this module owns the decisions around them so every frontend
//! repairs meshes the same way.

use std::fmt;

use super::fix::collinear_faces::CollinearFaceRemovals;
use super::fix::records::MeshFix;
use super::fix::watertight::{
    WatertightOptions, count_bad_edge_records, plan_orientation_with_records, plan_watertight,
};
use super::half_edges;
use super::mesh::IndexedMesh;
use super::validate::{MeshFaceError, MeshVertexError};
use super::validated::ZeroAreaFace;

/// Which repair passes [`IndexedMesh::repair`] runs. Each flag enables an
/// independent pass; passes that find nothing to repair are no-ops. `Default`
/// is all-off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepairPasses {
    /// Removes invalid faces: faces that contain a zero-length edge (a vertex
    /// named twice) and faces that name a vertex that does not exist. With it
    /// off, a mesh with either is refused.
    pub zero_length_edges: bool,
    /// Orients each piece as a whole, flipping the smaller side of faces that
    /// wind against their neighbours, and turns closed inside-out shells
    /// outward unless they are cavities. It never leaves more bad edges than
    /// it found.
    pub flipped_faces: bool,
    /// Removes collinear faces: zero-area faces whose 3 distinct vertices lie
    /// on 1 line. It runs only on a mesh that is edge-manifold after the
    /// orientation pass.
    pub collinear_faces: bool,
    /// Resolves edges shared by more than 2 faces: pairs the faces around each
    /// edge and splits its ends into 1 vertex per fan, so parts that touch at
    /// an edge or a vertex become separate closed shells. Part of the
    /// watertight pass, which also orients each piece as a whole.
    pub non_manifold_edges: bool,
    /// Welds open boundaries within [`WELD_TOLERANCE_MM`] and fills holes up
    /// to [`MAX_AUTO_FILL_AREA_FRACTION`] of the mesh area. Part of the
    /// watertight pass, which also orients each piece as a whole.
    pub holes: bool,
}

impl RepairPasses {
    /// Returns the default pass set: every pass.
    pub const fn default_repair() -> Self {
        Self {
            zero_length_edges: true,
            flipped_faces: true,
            collinear_faces: true,
            non_manifold_edges: true,
            holes: true,
        }
    }

    const fn watertight(self) -> bool {
        self.non_manifold_edges || self.holes
    }

    fn watertight_options(self) -> WatertightOptions {
        WatertightOptions {
            resolve_non_manifold: self.non_manifold_edges,
            close_holes: self.holes,
            weld_tolerance_mm: WELD_TOLERANCE_MM,
            max_fill_area_fraction: MAX_AUTO_FILL_AREA_FRACTION,
            min_area_fill_max_edges: MIN_AREA_FILL_MAX_EDGES,
            max_rounds: MAX_WATERTIGHT_ROUNDS,
            split_vertex_offset_mm: SPLIT_VERTEX_OFFSET_MM,
        }
    }
}

/// The largest distance, in mm, between 2 open-boundary vertices that the
/// hole pass welds into 1. Half the smallest pixel among the bundled printer
/// profiles (10.5 um), so a weld moves geometry by less than half a pixel on
/// any of them. Cracks narrower than this close; wider ones are filled on each
/// side, keeping the gap as modelled.
pub const WELD_TOLERANCE_MM: f64 = 0.005;

/// The largest hole the hole pass fills on its own, as the area of its fill
/// over the area of the whole mesh. A larger hole is more likely an open part
/// on purpose (a plane, a relief without a back, an open bowl) than a defect,
/// so it is left open and reported in
/// [`MeshStateAfterRepair::holes_left_open`]. The largest fill the repair
/// corpus needs is 0.28 (a sphere cut at its equator, each half capped); the
/// largest in the real files measured, 0.21. A flat plate needs 1, an open
/// hemisphere 0.5.
pub const MAX_AUTO_FILL_AREA_FRACTION: f64 = 0.3;

/// The largest hole, in boundary edges, that gets a minimum-area
/// triangulation; a larger one gets a fan from its centroid.
pub const MIN_AREA_FILL_MAX_EDGES: usize = 200;

/// How many rounds the watertight pass runs at most: a later round settles
/// what a fill of an earlier one left.
pub const MAX_WATERTIGHT_ROUNDS: usize = 3;

/// How far, in mm, the watertight pass moves a vertex that shares its exact
/// position with another (the copies it makes to separate parts that touch at
/// an edge) into its own part, so that saving as STL, which merges vertices by
/// position, keeps the parts apart. A tenth of the smallest pixel among the
/// bundled printer profiles (10.5 um): no printer sees it.
pub const SPLIT_VERTEX_OFFSET_MM: f64 = 0.001;

/// Why [`IndexedMesh::repair`] refused to run, or to finish, its passes.
#[derive(Clone, Debug, PartialEq)]
pub enum RepairRefusal {
    /// The mesh has non-finite vertex coordinates; no pass can run. The mesh is
    /// left untouched.
    InvalidVertices(Vec<MeshVertexError>),
    /// Face errors with the invalid-face pass
    /// ([`RepairPasses::zero_length_edges`]) disabled. The mesh is left
    /// untouched.
    UnfixableFaceErrors(Vec<MeshFaceError>),
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
        }
    }
}

impl std::error::Error for RepairRefusal {}

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
    /// The mesh has no faces: there is nothing to repair, and no pass ran.
    NoFaces,
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
    /// How many holes the hole pass left open because their fill would exceed
    /// [`MAX_AUTO_FILL_AREA_FRACTION`] of the mesh area.
    pub holes_left_open: usize,
}

/// What a repair run would do to a mesh, decided on a shared borrow (see
/// [`assess_repair`]).
#[derive(Clone, Debug, PartialEq)]
pub enum RepairAssessment {
    /// The mesh has no faces; the run would end with [`RepairEnd::NoFaces`].
    NoFaces,
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
/// repair run with the same passes. On a mesh that fails the edge check it
/// plans the whole watertight pass, as long as the run would; to repair, call
/// [`repaired_copy`], which plans it once.
pub fn assess_repair(mesh: &IndexedMesh, passes: RepairPasses) -> RepairAssessment {
    assess(mesh, passes, true)
}

/// Runs [`IndexedMesh::repair`] on a copy of `mesh` and returns its report,
/// with the copy when the run changed it (when the report has fixes). A mesh
/// that [`assess_repair`] finds clean, refused or without faces is not copied.
/// Same report as [`assess_repair`] followed by a run, at the cost of 1
/// planning of the watertight pass instead of 2.
pub fn repaired_copy(
    mesh: &IndexedMesh,
    passes: RepairPasses,
) -> (MeshRepairReport, Option<IndexedMesh>) {
    match assess(mesh, passes, false) {
        RepairAssessment::Clean(state) => (
            MeshRepairReport {
                fixes: Vec::new(),
                end: RepairEnd::Completed(state),
            },
            None,
        ),
        RepairAssessment::Refused(refusal) => (
            MeshRepairReport {
                fixes: Vec::new(),
                end: RepairEnd::Refused(refusal),
            },
            None,
        ),
        RepairAssessment::NoFaces => (
            MeshRepairReport {
                fixes: Vec::new(),
                end: RepairEnd::NoFaces,
            },
            None,
        ),
        RepairAssessment::Work => {
            let mut copy = mesh.clone();
            let report = copy.repair(passes);
            let changed = !report.fixes.is_empty();
            (report, changed.then_some(copy))
        }
    }
}

/// [`assess_repair`]; with `plan_pass` off, a mesh the watertight pass
/// would look at assesses as [`RepairAssessment::Work`] without planning it.
fn assess(mesh: &IndexedMesh, passes: RepairPasses, plan_pass: bool) -> RepairAssessment {
    // Mirrors the gates of `run_repair_passes`, orientation planning included:
    // a change to either must go to both.
    if mesh.faces.is_empty() {
        return RepairAssessment::NoFaces;
    }
    if let Err(errors) = mesh.validate_vertices() {
        return RepairAssessment::Refused(RepairRefusal::InvalidVertices(errors));
    }
    let witness = match mesh.validated_faces() {
        Ok(witness) => witness,
        Err(face_errors) => {
            if !passes.zero_length_edges {
                return RepairAssessment::Refused(RepairRefusal::UnfixableFaceErrors(face_errors));
            }
            // The invalid-face pass is enabled: faces will be removed.
            return RepairAssessment::Work;
        }
    };
    let face_areas = witness.validate_face_areas();
    // 1 sort of the edges serves the edge check and the orientation pass. The
    // mesh has faces, so no bad edge means it is edge-manifold.
    let records = half_edges::sorted_half_edge_records(&mesh.faces);
    let edge_manifold = count_bad_edge_records(&records) == 0;
    let mut holes_left_open = 0;
    if passes.watertight() && !edge_manifold {
        if !plan_pass {
            return RepairAssessment::Work;
        }
        let outcome = plan_watertight(&mesh.vertices, &mesh.faces, &passes.watertight_options());
        if outcome.repaired.is_some() {
            return RepairAssessment::Work;
        }
        holes_left_open = outcome.holes_left_open;
    }
    if passes.flipped_faces
        && plan_orientation_with_records(&mesh.vertices, &mesh.faces, records).is_some()
    {
        return RepairAssessment::Work;
    }
    // Here the orientation pass changes nothing, so the run gives the
    // collinear-face pass this same mesh.
    // #[gamma::skip(logical.and_remove_right, reason = "finds no face")]
    if passes.collinear_faces
        && face_areas.is_err()
        && edge_manifold
        && let Ok(manifold) = witness.validated_edge_manifold()
        && CollinearFaceRemovals::new(&manifold).next().is_some()
    {
        return RepairAssessment::Work;
    }
    RepairAssessment::Clean(state_after_repair(
        edge_manifold,
        face_areas,
        holes_left_open,
    ))
}

fn state_after_repair(
    edge_manifold: bool,
    face_areas: Result<(), Vec<ZeroAreaFace>>,
    holes_left_open: usize,
) -> MeshStateAfterRepair {
    MeshStateAfterRepair {
        edge_manifold,
        zero_area_face_count: face_areas.err().map_or(0, |faces| faces.len()),
        holes_left_open,
    }
}

impl IndexedMesh {
    /// Runs the enabled repair passes over this mesh and reports what happened.
    ///
    /// Pass order and refusal semantics:
    /// 0. A mesh without faces ends with [`RepairEnd::NoFaces`], untouched.
    /// 1. Vertex gate: non-finite coordinates refuse the whole run with the
    ///    mesh untouched.
    /// 2. Face gate, decided before any mutation: when face validation fails
    ///    and the invalid-face pass is disabled, the run is refused with the
    ///    mesh untouched.
    /// 3. Invalid-face pass: removes the faces with a zero-length edge or an
    ///    out-of-range index.
    /// 4. Watertight pass (the non-manifold-edge and hole passes), on a mesh
    ///    that fails the edge check: welds open boundaries, resolves edges with
    ///    more than 2 faces, orients each piece, fills holes and turns closed
    ///    inside-out shells outward, in up to [`MAX_WATERTIGHT_ROUNDS`]
    ///    rounds. It is all or nothing, and kept only when it leaves fewer bad
    ///    edges than it found.
    /// 5. Orientation pass: orients each piece as a whole and turns closed
    ///    inside-out shells outward, unless they are cavities. It runs on
    ///    every mesh, since a mesh inside out passes the edge check, and is
    ///    kept only when it leaves fewer bad edges than it found, or flips
    ///    only whole closed shells.
    /// 6. Collinear-face pass: removes collinear faces when the mesh is
    ///    edge-manifold at this point. On a mesh that is not, it does not run,
    ///    and that is no refusal.
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
        if self.faces.is_empty() {
            return (Vec::new(), RepairEnd::NoFaces);
        }
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
                // invalid-face pass disabled every face error is
                // unaddressable.
                if !passes.zero_length_edges {
                    return (
                        fixes,
                        RepairEnd::Refused(RepairRefusal::UnfixableFaceErrors(face_errors)),
                    );
                }
                let (face_fixes, witness) = self.fix_invalid_faces_into_validated(&face_errors);
                fixes.extend(face_fixes);
                match witness {
                    Ok(w) => w,
                    // Unreachable: the errors name every invalid face, and
                    // removing whole faces keeps the others as they were;
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

        let mut holes_left_open = 0;
        if passes.watertight() && validated.validate_edge_manifold().is_err() {
            let (watertight_fixes, left_open) =
                validated.fix_watertight(&passes.watertight_options());
            fixes.extend(watertight_fixes);
            holes_left_open = left_open;
        }

        if passes.flipped_faces {
            fixes.extend(validated.fix_orientation());
        }

        // The face-area test only skips the mint for a mesh without zero-area
        // faces: the pass finds its faces itself.
        // #[gamma::skip(logical.and_remove_right, reason = "finds no face")]
        if passes.collinear_faces
            && validated.validate_face_areas().is_err()
            && let Ok(manifold) = validated.validated_edge_manifold_mut()
        {
            fixes.extend(manifold.fix_collinear_faces());
        }

        let state = state_after_repair(
            validated.validate_edge_manifold().is_ok(),
            validated.validate_face_areas(),
            holes_left_open,
        );
        (fixes, RepairEnd::Completed(state))
    }
}

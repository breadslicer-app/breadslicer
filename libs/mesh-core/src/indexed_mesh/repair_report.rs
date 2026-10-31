/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The report of a repair run on 1 mesh, for people: what the run changed,
//! told as actions with counts ([`repair_actions`]); the mesh before and after
//! (faces, volume, whether it slices); the time of each stage; and, when the
//! mesh still does not slice, what is left and on which body
//! ([`Leftovers`]). [`repaired_copy_with_details`] repairs exactly as
//! [`repaired_copy`](super::repair::repaired_copy) does and only measures
//! around it.

use std::time::{Duration, Instant};

use super::fix::records::MeshFix;
use super::fix::watertight::leftover_edges;
use super::mesh::IndexedMesh;
use super::repair::{
    MAX_AUTO_FILL_AREA_FRACTION, MeshRepairReport, RepairAssessment, RepairEnd, RepairPasses,
    assess_with_verdict,
};
use crate::shells::{Bodies, BodyKind};

/// A stage of a repair run that can change the mesh, in run order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairStage {
    /// Removes faces that name a vertex twice or a vertex that does not
    /// exist.
    InvalidFaces,
    /// Welds cracks, separates parts that touch at an edge, removes duplicate
    /// faces and double walls, orients each piece, fills holes and turns
    /// inside-out shells outward.
    Watertight,
    /// Orients each piece of a mesh that passes the edge check.
    Orientation,
    /// Removes zero-area slivers.
    CollinearFaces,
}

/// How long 1 stage of a repair run took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageTime {
    pub stage: RepairStage,
    pub elapsed: Duration,
    /// Whether the stage changed the mesh: a stage that found nothing to do
    /// still takes the time of its check.
    pub changed: bool,
}

impl StageTime {
    /// The time of `stage` from `start` to now.
    pub(crate) fn since(stage: RepairStage, start: Instant, changed: bool) -> Self {
        Self {
            stage,
            elapsed: start.elapsed(),
            changed,
        }
    }
}

/// The figures of a mesh before or after a repair run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshFigures {
    pub faces: usize,
    /// Whether the mesh passes the slicer's gate
    /// ([`IndexedMesh::is_sliceable`]).
    pub sliceable: bool,
    /// The volume the mesh encloses, in mm³; only for a mesh that slices,
    /// since an open one encloses nothing.
    pub volume_mm3: Option<f64>,
}

/// Everything around a repair run on 1 mesh that its [`MeshRepairReport`]
/// does not hold.
#[derive(Clone, Debug, PartialEq)]
pub struct RepairDetails {
    pub before: MeshFigures,
    pub after: MeshFigures,
    /// The stages the run went through, in run order. Empty when the mesh
    /// needed no run (clean, refused or without faces).
    pub stages: Vec<StageTime>,
    /// The whole time spent on the mesh: the checks before and after
    /// included.
    pub elapsed: Duration,
    /// What keeps the mesh from slicing after the run, and where. `None` for
    /// a mesh that slices, and for a refused one (the refusal says why).
    pub leftovers: Option<Leftovers>,
}

impl RepairDetails {
    /// The time spent outside the stages that changed the mesh: checking it
    /// before, between and after them.
    pub fn checking_time(&self) -> Duration {
        let changing: Duration = self
            .stages
            .iter()
            .filter(|stage| stage.changed)
            .map(|stage| stage.elapsed)
            .sum();
        self.elapsed.saturating_sub(changing)
    }
}

/// What keeps a mesh from slicing after a repair run, per body.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftovers {
    /// How many bodies the mesh has, numbered as
    /// [`Bodies`] numbers them (as "Split into bodies" does).
    pub bodies: usize,
    /// The bodies with something left, in body order.
    pub per_body: Vec<BodyLeftovers>,
}

/// What is left on 1 body of a mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyLeftovers {
    /// The body, 0-based, in [`Bodies`] order.
    pub body: usize,
    /// The body is the group of a mesh's fragments (tiny loose shells).
    pub fragments: bool,
    /// The holes left open, the largest first.
    pub holes: Vec<HoleLeft>,
    /// Open edges on no closed loop: a boundary that runs into a tangled
    /// edge.
    pub open_edges: usize,
    /// Edges shared by more than 2 faces, or by 2 faces that wind the same
    /// way.
    pub tangled_edges: usize,
}

/// A hole left open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoleLeft {
    /// Its boundary length, in edges.
    pub edges: usize,
    /// The area a fill would cover, in mm² (a fan from its centroid, as the
    /// size cap measures it).
    pub area_mm2: f64,
    /// That area over the area of the whole mesh.
    pub surface_fraction: f64,
}

impl HoleLeft {
    /// Whether the hole is over the size the repair fills on its own
    /// ([`MAX_AUTO_FILL_AREA_FRACTION`]): more likely open on purpose than a
    /// defect.
    pub fn too_large_to_fill(&self) -> bool {
        self.surface_fraction > MAX_AUTO_FILL_AREA_FRACTION
    }
}

/// 1 thing a repair run did to a mesh, with its counts, for people. A run
/// lists only what changed the mesh: a stage that found nothing gives no
/// action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RepairAction {
    /// Holes closed with new faces; the area of the largest fill, in mm².
    FilledHoles { holes: usize, largest_area_mm2: f64 },
    /// Open-boundary vertices welded to close cracks; the faces the welds
    /// flattened went with them.
    WeldedCracks {
        vertices: usize,
        collapsed_faces: usize,
    },
    /// Parts that touched at edges were separated.
    SeparatedParts { edges: usize },
    /// Faces stored twice; 1 copy of each stays.
    RemovedDuplicateFaces { faces: usize },
    /// Faces of double walls: 2 parts glued face to face.
    RemovedDoubleWalls { faces: usize },
    /// Faces left without a partner where faces were tangled.
    RemovedLeftoverFaces { faces: usize },
    /// Closed shells that were inside out.
    TurnedShellsOutward { shells: usize },
    /// Faces flipped so each piece faces 1 way.
    FlippedFaces { faces: usize },
    /// Faces that named a vertex twice.
    RemovedDegenerateFaces { faces: usize },
    /// Faces that named a vertex that does not exist.
    RemovedBrokenFaces { faces: usize },
    /// Zero-area slivers on a line.
    RemovedSlivers { faces: usize },
}

/// Folds the fix records of a run into actions, the most telling first: what
/// closes the mesh, then what cleans it. Each kind of action appears once,
/// and only when its count is not 0.
pub fn repair_actions(fixes: &[MeshFix]) -> Vec<RepairAction> {
    let mut holes = 0;
    let mut largest_area_mm2: f64 = 0.0;
    let mut welded = 0;
    let mut collapsed = 0;
    let mut separated = 0;
    let mut duplicate = 0;
    let mut double_walls = 0;
    let mut leftover = 0;
    let mut turned = 0;
    let mut flipped = 0;
    let mut degenerate = 0;
    let mut broken = 0;
    let mut slivers = 0;
    for fix in fixes {
        match fix {
            MeshFix::FilledHoles {
                holes: n,
                largest_hole_area_mm2,
                ..
            } => {
                holes += n;
                largest_area_mm2 = largest_area_mm2.max(*largest_hole_area_mm2);
            }
            MeshFix::WeldedBoundaryVertices {
                vertices,
                collapsed_faces,
            } => {
                welded += vertices;
                collapsed += collapsed_faces;
            }
            MeshFix::SplitNonManifoldEdges { edges, .. } => separated += edges,
            MeshFix::RemovedDuplicateFaces { faces } => duplicate += faces,
            MeshFix::RemovedOppositeFaces { faces } => double_walls += faces,
            MeshFix::RemovedUnpairedFaces { faces } => leftover += faces,
            MeshFix::TurnedShellsOutward { shells, .. } => turned += shells,
            MeshFix::ReorientedFaces { faces } => flipped += faces,
            MeshFix::RemovedZeroLengthEdgeFace { .. } => degenerate += 1,
            MeshFix::RemovedOutOfRangeFace { .. } => broken += 1,
            MeshFix::RemovedCollinearFaceByMerge { .. }
            | MeshFix::RemovedCollinearFaceBySplit { .. } => slivers += 1,
        }
    }
    let mut actions = Vec::new();
    if holes > 0 {
        actions.push(RepairAction::FilledHoles {
            holes,
            largest_area_mm2,
        });
    }
    if welded > 0 || collapsed > 0 {
        actions.push(RepairAction::WeldedCracks {
            vertices: welded,
            collapsed_faces: collapsed,
        });
    }
    let counted = [
        (separated, RepairAction::SeparatedParts { edges: separated }),
        (
            duplicate,
            RepairAction::RemovedDuplicateFaces { faces: duplicate },
        ),
        (
            double_walls,
            RepairAction::RemovedDoubleWalls {
                faces: double_walls,
            },
        ),
        (
            leftover,
            RepairAction::RemovedLeftoverFaces { faces: leftover },
        ),
        (turned, RepairAction::TurnedShellsOutward { shells: turned }),
        (flipped, RepairAction::FlippedFaces { faces: flipped }),
        (
            degenerate,
            RepairAction::RemovedDegenerateFaces { faces: degenerate },
        ),
        (broken, RepairAction::RemovedBrokenFaces { faces: broken }),
        (slivers, RepairAction::RemovedSlivers { faces: slivers }),
    ];
    actions.extend(
        counted
            .into_iter()
            .filter(|(count, _)| *count > 0)
            .map(|(_, action)| action),
    );
    actions
}

/// Runs [`IndexedMesh::repair`] on a copy of `mesh`, as
/// [`repaired_copy`](super::repair::repaired_copy) does, and measures around
/// it: returns the report, the copy when the run changed it, and the details.
/// The measuring changes nothing the run does.
pub fn repaired_copy_with_details(
    mesh: &IndexedMesh,
    passes: RepairPasses,
) -> (MeshRepairReport, Option<IndexedMesh>, RepairDetails) {
    let start = Instant::now();
    let (assessment, sliceable_before) = assess_with_verdict(mesh, passes, false);
    let before = figures(mesh, sliceable_before);
    let mut stages = Vec::new();
    let (report, repaired) = match assessment {
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
            let report = copy.repair_timed(passes, &mut stages);
            let changed = !report.fixes.is_empty();
            (report, changed.then_some(copy))
        }
    };
    let after = match (&repaired, &report.end) {
        (Some(copy), RepairEnd::Completed(state)) => figures(
            copy,
            // The run left valid faces; its last check is the edge check.
            state.edge_manifold && copy.validate_vertices().is_ok(),
        ),
        (Some(copy), _) => figures(copy, copy.is_sliceable()),
        // Unchanged: a run that changed nothing ends on the same mesh.
        (None, _) => before,
    };
    let leftovers = match &report.end {
        RepairEnd::Completed(_) if !after.sliceable => {
            Some(find_leftovers(repaired.as_ref().unwrap_or(mesh)))
        }
        _ => None,
    };
    let details = RepairDetails {
        before,
        after,
        stages,
        elapsed: start.elapsed(),
        leftovers,
    };
    (report, repaired, details)
}

/// The figures of `mesh`, whose verdict the caller knows.
fn figures(mesh: &IndexedMesh, sliceable: bool) -> MeshFigures {
    MeshFigures {
        faces: mesh.faces.len(),
        sliceable,
        volume_mm3: sliceable
            .then(|| {
                mesh.validated_faces()
                    .ok()
                    .map(|faces| faces.signed_volume_mm3())
            })
            .flatten(),
    }
}

/// Finds what keeps `mesh`, whose faces are valid, from passing the edge
/// check, and on which body.
fn find_leftovers(mesh: &IndexedMesh) -> Leftovers {
    let edges = leftover_edges(&mesh.vertices, &mesh.faces);
    let bodies = Bodies::find(&mesh.vertices, &mesh.faces);
    let mut per_body: Vec<BodyLeftovers> = bodies
        .bodies()
        .iter()
        .enumerate()
        .map(|(body, b)| BodyLeftovers {
            body,
            fragments: b.kind == BodyKind::Fragments,
            holes: Vec::new(),
            open_edges: 0,
            tangled_edges: 0,
        })
        .collect();
    let surface = edges.mesh_area;
    for hole in &edges.loops {
        per_body[bodies.of(hole.face) as usize]
            .holes
            .push(HoleLeft {
                edges: hole.edges,
                area_mm2: hole.area,
                surface_fraction: if surface > 0.0 {
                    hole.area / surface
                } else {
                    1.0
                },
            });
    }
    for &face in &edges.open {
        per_body[bodies.of(face) as usize].open_edges += 1;
    }
    for &face in &edges.tangled {
        per_body[bodies.of(face) as usize].tangled_edges += 1;
    }
    for body in &mut per_body {
        body.holes.sort_by(|a, b| b.area_mm2.total_cmp(&a.area_mm2));
    }
    per_body.retain(|b| !b.holes.is_empty() || b.open_edges > 0 || b.tangled_edges > 0);
    Leftovers {
        bodies: bodies.count(),
        per_body,
    }
}

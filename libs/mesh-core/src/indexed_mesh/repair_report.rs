/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The report of a repair run on 1 mesh, for people: what the run changed,
//! told as actions with counts ([`repair_actions`]); the mesh before and after
//! (faces, volume, whether it slices); the time of each stage; and, when the
//! mesh still does not slice, what is left and on which body
//! ([`Leftovers`]), with its edges, so they can be shown on the model.
//! [`repaired_copy_with_details`] repairs exactly as
//! [`repaired_copy`](super::repair::repaired_copy) does and only measures
//! around it; [`repaired_copy_watched`] also tells a [`RepairWatch`] how far
//! it is, and stops when it asks to.

use std::time::{Duration, Instant};

use rayon::prelude::*;

use super::fix::records::MeshFix;
use super::fix::watertight::leftover_edges;
use super::mesh::IndexedMesh;
use super::repair::{
    MAX_AUTO_FILL_AREA_FRACTION, MeshRepairReport, RepairAssessment, RepairEnd, RepairPasses,
    assess_with_verdict,
};
use super::repair_progress::{Progress, RepairCancelled, RepairStep, RepairWatch, Unwatched};
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
    /// The share of the surface of the mesh after the run that its hole
    /// fills cover; `None` when the run filled no hole.
    pub filled_fraction: Option<f64>,
}

/// The share of a mesh's surface that hole fills may cover before the
/// report warns that the model may not be meant to be solid: an open plate
/// or a relief without a back closes into a thin solid, which slices but is
/// not what was modelled. The fills of the real broken meshes of the
/// repair corpus cover at most 1 % of their surface, but for 3 models with
/// large openings (10 to 37 %).
pub const LOOKS_OPEN_FILL_FRACTION: f64 = 0.05;

impl RepairDetails {
    /// Whether the run's fills cover so much of the surface
    /// ([`LOOKS_OPEN_FILL_FRACTION`]) that the model may be open on purpose.
    pub fn filled_a_lot(&self) -> bool {
        self.filled_fraction
            .is_some_and(|fraction| fraction > LOOKS_OPEN_FILL_FRACTION)
    }

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
    /// Every edge left, to show on the model: the edges of the holes, then
    /// the open edges, then the tangled ones.
    pub edges: Vec<LeftoverEdge>,
}

/// 1 edge that keeps a mesh from slicing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeftoverEdge {
    /// Its 2 vertices, in the mesh the run ended with.
    pub vertices: [u32; 2],
    pub kind: LeftoverEdgeKind,
}

/// What is wrong at a [`LeftoverEdge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftoverEdgeKind {
    /// On a hole over the fill cap ([`HoleLeft::too_large_to_fill`]): may be
    /// open on purpose.
    LargeHole,
    /// On a hole Repair could not close.
    Hole,
    /// An open edge on no closed hole.
    Open,
    /// An edge with more than 2 faces, or with 2 that wind the same way.
    Tangled,
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
    match repaired_copy_watched(mesh, passes, &Unwatched) {
        Ok(repaired) => repaired,
        // An unwatched run is never cancelled.
        Err(RepairCancelled) => unreachable!("an unwatched repair was cancelled"),
    }
}

/// [`repaired_copy_with_details`], telling `watch` each step and how far
/// the run is, and stopping before a step when it asks to. A run that stops
/// returns [`RepairCancelled`] and drops its copy: `mesh` is never touched.
/// Watching changes nothing the run does.
pub fn repaired_copy_watched(
    mesh: &IndexedMesh,
    passes: RepairPasses,
    watch: &dyn RepairWatch,
) -> Result<(MeshRepairReport, Option<IndexedMesh>, RepairDetails), RepairCancelled> {
    let start = Instant::now();
    let progress = Progress::of(watch);
    progress.step(RepairStep::Checking, 0.0)?;
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
            let report = copy.repair_timed(passes, &mut stages, progress.part(RUN_FROM, RUN_TO))?;
            let changed = !report.fixes.is_empty();
            (report, changed.then_some(copy))
        }
    };
    progress.step(RepairStep::CheckingResult, RUN_TO)?;
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
    let filled_area: f64 = report
        .fixes
        .iter()
        .map(|fix| match fix {
            MeshFix::FilledHoles {
                filled_area_mm2, ..
            } => *filled_area_mm2,
            _ => 0.0,
        })
        .sum();
    let filled_fraction = match &repaired {
        Some(copy) if filled_area > 0.0 => Some(filled_area / surface_area(copy).max(filled_area)),
        _ => None,
    };
    let details = RepairDetails {
        before,
        after,
        stages,
        elapsed: start.elapsed(),
        leftovers,
        filled_fraction,
    };
    Ok((report, repaired, details))
}

/// The share of a watched run where the passes start and end: the checks
/// before and after take the rest.
const RUN_FROM: f32 = 0.03;
const RUN_TO: f32 = 0.95;

/// The area of the surface of `mesh`, in mm².
fn surface_area(mesh: &IndexedMesh) -> f64 {
    let at = |v: crate::primitives::VertexIndex| {
        mesh.vertices
            .get(v.get() as usize)
            .map_or([0.0; 3], |p| p.coords().map(f64::from))
    };
    mesh.faces
        .par_iter()
        .map(|face| {
            let [a, b, c] = face.vertices.map(at);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
        })
        .sum()
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
    let mut marks = Vec::new();
    for hole in &edges.loops {
        let left = HoleLeft {
            edges: hole.edges,
            area_mm2: hole.area,
            surface_fraction: if surface > 0.0 {
                hole.area / surface
            } else {
                1.0
            },
        };
        let kind = if left.too_large_to_fill() {
            LeftoverEdgeKind::LargeHole
        } else {
            LeftoverEdgeKind::Hole
        };
        let ring = &hole.vertices;
        marks.extend((0..ring.len()).map(|i| LeftoverEdge {
            vertices: [ring[i], ring[(i + 1) % ring.len()]],
            kind,
        }));
        per_body[bodies.of(hole.face) as usize].holes.push(left);
    }
    for &(face, vertices) in &edges.open {
        per_body[bodies.of(face) as usize].open_edges += 1;
        marks.push(LeftoverEdge {
            vertices,
            kind: LeftoverEdgeKind::Open,
        });
    }
    for &(face, vertices) in &edges.tangled {
        per_body[bodies.of(face) as usize].tangled_edges += 1;
        marks.push(LeftoverEdge {
            vertices,
            kind: LeftoverEdgeKind::Tangled,
        });
    }
    for body in &mut per_body {
        body.holes.sort_by(|a, b| b.area_mm2.total_cmp(&a.area_mm2));
    }
    per_body.retain(|b| !b.holes.is_empty() || b.open_edges > 0 || b.tangled_edges > 0);
    Leftovers {
        bodies: bodies.count(),
        per_body,
        edges: marks,
    }
}

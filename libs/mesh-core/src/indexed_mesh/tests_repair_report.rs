/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the repair report: the actions folded from the fix records, and
//! what is left, and where, on a mesh that still does not slice.

use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::mesh::IndexedMesh;
use crate::indexed_mesh::repair::{RepairEnd, RepairPasses};
use crate::indexed_mesh::repair_report::{
    RepairAction, RepairStage, repair_actions, repaired_copy_with_details,
};
use crate::primitives::{FaceIndex, IndexedTriangle, Vertex};

fn mesh(vertices: &[[f32; 3]], faces: &[[u32; 3]]) -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vertices.iter().map(|&v| Vertex::new(v)).collect(),
        faces: faces
            .iter()
            .map(|&f| IndexedTriangle::from_raw_indices(f))
            .collect(),
    }
}

/// A flat 2 by 1 plate: 1 hole, its whole outline, far over the size cap.
fn plate() -> IndexedMesh {
    mesh(
        &[
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
}

/// A closed tetrahedron with `plate` beside it, in 1 mesh: 2 bodies, the
/// plate a fragment.
fn tetrahedron_and_plate() -> IndexedMesh {
    mesh(
        &[
            [1.0, 1.0, 1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [10.0, 0.0, 0.0],
            [30.0, 0.0, 0.0],
            [30.0, 10.0, 0.0],
            [10.0, 10.0, 0.0],
        ],
        &[
            [0, 1, 2],
            [0, 3, 1],
            [0, 2, 3],
            [1, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
        ],
    )
}

#[test]
fn the_actions_fold_the_records_by_kind_in_report_order() {
    let fixes = [
        MeshFix::ReorientedFaces { faces: 3 },
        MeshFix::RemovedZeroLengthEdgeFace {
            face_idx: FaceIndex::from(4),
        },
        MeshFix::RemovedZeroLengthEdgeFace {
            face_idx: FaceIndex::from(9),
        },
        MeshFix::FilledHoles {
            holes: 2,
            added_faces: 6,
            largest_hole_edges: 5,
            largest_hole_area_mm2: 1.5,
        },
        MeshFix::WeldedBoundaryVertices {
            vertices: 12,
            collapsed_faces: 1,
        },
    ];
    assert_eq!(
        repair_actions(&fixes),
        [
            RepairAction::FilledHoles {
                holes: 2,
                largest_area_mm2: 1.5,
            },
            RepairAction::WeldedCracks {
                vertices: 12,
                collapsed_faces: 1,
            },
            RepairAction::FlippedFaces { faces: 3 },
            RepairAction::RemovedDegenerateFaces { faces: 2 },
        ]
    );
}

#[test]
fn no_records_give_no_action() {
    assert!(repair_actions(&[]).is_empty());
}

#[test]
fn a_plate_leaves_1_hole_too_large_to_fill_on_its_only_body() {
    let plate = plate();
    let (report, repaired, details) =
        repaired_copy_with_details(&plate, RepairPasses::default_repair());
    assert!(report.fixes.is_empty());
    assert!(repaired.is_none());
    assert!(matches!(report.end, RepairEnd::Completed(_)));
    assert!(!details.before.sliceable);
    assert!(!details.after.sliceable);
    assert_eq!(details.after.volume_mm3, None);
    // The watertight stage ran and changed nothing.
    assert!(
        details
            .stages
            .iter()
            .any(|s| s.stage == RepairStage::Watertight && !s.changed)
    );
    let leftovers = details.leftovers.expect("the plate does not slice");
    assert_eq!(leftovers.bodies, 1);
    assert_eq!(leftovers.per_body.len(), 1);
    let body = &leftovers.per_body[0];
    assert_eq!(body.body, 0);
    assert_eq!((body.open_edges, body.tangled_edges), (0, 0));
    assert_eq!(body.holes.len(), 1);
    let hole = body.holes[0];
    assert_eq!(hole.edges, 4);
    assert!((hole.area_mm2 - 2.0).abs() < 1e-9, "area {}", hole.area_mm2);
    assert!((hole.surface_fraction - 1.0).abs() < 1e-9);
    assert!(hole.too_large_to_fill());
}

#[test]
fn what_is_left_names_the_body_it_is_on() {
    let mesh = tetrahedron_and_plate();
    let (_, _, details) = repaired_copy_with_details(&mesh, RepairPasses::default_repair());
    let leftovers = details.leftovers.expect("the plate does not slice");
    assert_eq!(leftovers.bodies, 2);
    assert_eq!(leftovers.per_body.len(), 1, "the tetrahedron is sound");
    let body = &leftovers.per_body[0];
    // The tetrahedron is body 0; the plate, a fragment, comes last.
    assert_eq!(body.body, 1);
    assert!(body.fragments);
    assert_eq!(body.holes.len(), 1);
}

#[test]
fn an_edge_with_3_faces_left_counts_as_tangled() {
    // 3 triangles on 1 edge, with the watertight pass off: nothing resolves
    // it.
    let fin = mesh(
        &[
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
        ],
        &[[0, 1, 2], [1, 0, 3], [0, 1, 4]],
    );
    let passes = RepairPasses {
        non_manifold_edges: false,
        holes: false,
        ..RepairPasses::default_repair()
    };
    let (_, _, details) = repaired_copy_with_details(&fin, passes);
    let leftovers = details.leftovers.expect("the fin does not slice");
    let tangled: usize = leftovers.per_body.iter().map(|b| b.tangled_edges).sum();
    assert_eq!(tangled, 1);
}

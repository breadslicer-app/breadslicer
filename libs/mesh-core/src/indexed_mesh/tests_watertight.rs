/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the watertight pass, through the repair policy: welding cracks,
//! separating parts that touch at an edge, filling holes, turning inside-out
//! shells outward while keeping cavities, the size cap, and the agreement
//! between the assessment and the run.

use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::mesh::IndexedMesh;
use crate::indexed_mesh::repair::{
    MAX_AUTO_FILL_AREA_FRACTION, MeshRepairReport, MeshStateAfterRepair, RepairAssessment,
    RepairEnd, RepairPasses, WELD_TOLERANCE_MM, assess_repair,
};
use crate::primitives::{IndexedTriangle, Vertex};

/// The 12 outward faces of a box, over its 8 corners numbered by bits (x = 1,
/// y = 2, z = 4).
const BOX_FACES: [[u32; 3]; 12] = [
    [0, 2, 3],
    [0, 3, 1],
    [4, 5, 7],
    [4, 7, 6],
    [0, 1, 5],
    [0, 5, 4],
    [2, 6, 7],
    [2, 7, 3],
    [0, 4, 6],
    [0, 6, 2],
    [1, 3, 7],
    [1, 7, 5],
];

/// The 2 faces of the box's top (z = hi).
const TOP: [usize; 2] = [2, 3];
/// The 2 faces of the box's +X side.
const RIGHT: [usize; 2] = [10, 11];
/// The 2 faces of the box's -X side.
const LEFT: [usize; 2] = [8, 9];

/// A triangle soup builder: boxes appended one after another, each with its
/// own vertices.
#[derive(Default)]
struct Soup {
    vertices: Vec<[f32; 3]>,
    faces: Vec<[u32; 3]>,
}

impl Soup {
    /// Appends a box from `lo` to `hi`, without the faces in `skip`, wound
    /// inward when `inward`.
    fn add_box(mut self, lo: [f32; 3], hi: [f32; 3], skip: &[usize], inward: bool) -> Self {
        let base = self.vertices.len() as u32;
        for corner in 0..8u32 {
            self.vertices.push([
                if corner & 1 == 0 { lo[0] } else { hi[0] },
                if corner & 2 == 0 { lo[1] } else { hi[1] },
                if corner & 4 == 0 { lo[2] } else { hi[2] },
            ]);
        }
        for (i, face) in BOX_FACES.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            let [a, b, c] = face.map(|v| v + base);
            self.faces.push(if inward { [a, c, b] } else { [a, b, c] });
        }
        self
    }

    /// Builds the mesh, merging vertices at exactly the same position, as the
    /// STL reader does: boxes that touch then share their vertices.
    fn mesh(self) -> IndexedMesh {
        let mut index: Vec<u32> = Vec::with_capacity(self.vertices.len());
        let mut vertices: Vec<[f32; 3]> = Vec::new();
        for v in &self.vertices {
            let found = vertices.iter().position(|w| w == v);
            index.push(match found {
                Some(i) => i as u32,
                None => {
                    vertices.push(*v);
                    (vertices.len() - 1) as u32
                }
            });
        }
        IndexedMesh {
            meta: None,
            vertices: vertices.into_iter().map(Vertex::new).collect(),
            faces: self
                .faces
                .into_iter()
                .map(|f| IndexedTriangle::from_raw_indices(f.map(|v| index[v as usize])))
                .collect(),
        }
    }
}

fn volume(mesh: &IndexedMesh) -> f64 {
    mesh.validated_faces()
        .expect("valid faces")
        .signed_volume_mm3()
}

/// Repairs as the Repair button does, after checking that the assessment
/// predicts the run.
fn repair(mesh: &mut IndexedMesh) -> MeshRepairReport {
    let passes = RepairPasses::default_repair();
    let assessment = assess_repair(mesh, passes);
    let before = mesh.clone();
    let report = mesh.repair(passes);
    match assessment {
        RepairAssessment::Work => assert!(!report.fixes.is_empty(), "{report:?}"),
        RepairAssessment::Clean(state) => {
            assert_eq!(report.fixes, Vec::new());
            assert_eq!(report.end, RepairEnd::Completed(state));
            assert_eq!(*mesh, before);
        }
        RepairAssessment::Refused(_) => panic!("refused: {report:?}"),
    }
    report
}

fn has_fix(report: &MeshRepairReport, pred: impl Fn(&MeshFix) -> bool) -> bool {
    report.fixes.iter().any(pred)
}

#[test]
fn a_box_without_its_top_is_closed() {
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &TOP, false)
        .mesh();
    assert!(!mesh.is_sliceable());
    let report = repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!((volume(&mesh) - 1.0).abs() < 1e-6);
    assert!(has_fix(&report, |f| matches!(
        f,
        MeshFix::FilledHoles {
            holes: 1,
            added_faces: 2,
            largest_hole_edges: 4
        }
    )));
    // A square needs no new vertex.
    assert_eq!(mesh.vertices.len(), 8);
}

#[test]
fn an_inside_out_box_with_a_hole_comes_out_outward() {
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &TOP, true)
        .mesh();
    let report = repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!((volume(&mesh) - 1.0).abs() < 1e-6, "{}", volume(&mesh));
    assert!(has_fix(&report, |f| matches!(
        f,
        MeshFix::TurnedShellsOutward { shells: 1, .. }
    )));
}

#[test]
fn a_cavity_stays_a_cavity() {
    // A hollow box: the outer shell has a hole, the inner one is wound inward.
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [3.0; 3], &TOP, false)
        .add_box([1.0; 3], [2.0; 3], &[], true)
        .mesh();
    let report = repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!((volume(&mesh) - 26.0).abs() < 1e-6, "{}", volume(&mesh));
    assert!(!has_fix(&report, |f| matches!(
        f,
        MeshFix::TurnedShellsOutward { .. }
    )));
}

#[test]
fn boxes_that_share_an_edge_become_2_closed_shells() {
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .add_box([1.0, 1.0, 0.0], [2.0, 2.0, 1.0], &[], false)
        .mesh();
    assert_eq!(mesh.vertices.len(), 14);
    assert!(!mesh.is_sliceable());
    let report = repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!((volume(&mesh) - 2.0).abs() < 1e-6);
    // The 2 ends of the shared edge got a copy each; no face was lost.
    assert_eq!(mesh.vertices.len(), 16);
    assert_eq!(mesh.faces.len(), 24);
    assert_eq!(
        report.fixes,
        vec![MeshFix::SplitNonManifoldEdges {
            edges: 1,
            split_vertices: 2
        }]
    );
}

#[test]
fn boxes_that_share_a_face_slice_as_their_union() {
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .add_box([1.0, 0.0, 0.0], [2.0, 1.0, 1.0], &[], false)
        .mesh();
    assert!(!mesh.is_sliceable());
    repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!((volume(&mesh) - 2.0).abs() < 1e-6);
}

#[test]
fn a_crack_under_the_weld_tolerance_is_welded_shut() {
    let gap = (WELD_TOLERANCE_MM / 5.0) as f32;
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &RIGHT, false)
        .add_box([1.0 + gap, 0.0, 0.0], [2.0, 1.0, 1.0], &LEFT, false)
        .mesh();
    let report = repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!(has_fix(&report, |f| matches!(
        f,
        MeshFix::WeldedBoundaryVertices { vertices: 4, .. }
    )));
    assert!(!has_fix(&report, |f| matches!(
        f,
        MeshFix::FilledHoles { .. }
    )));
    assert!((volume(&mesh) - 2.0).abs() < 1e-4);
}

#[test]
fn a_crack_over_the_weld_tolerance_is_capped_on_each_side() {
    let gap = (WELD_TOLERANCE_MM * 20.0) as f32;
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &RIGHT, false)
        .add_box([1.0 + gap, 0.0, 0.0], [2.0, 1.0, 1.0], &LEFT, false)
        .mesh();
    let report = repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!(!has_fix(&report, |f| matches!(
        f,
        MeshFix::WeldedBoundaryVertices { .. }
    )));
    assert!(has_fix(&report, |f| matches!(
        f,
        MeshFix::FilledHoles { holes: 2, .. }
    )));
    // The gap stays as modelled.
    assert!((volume(&mesh) - (2.0 - f64::from(gap))).abs() < 1e-4);
}

#[test]
fn holes_that_touch_at_a_vertex_are_filled_apart() {
    // The bottom's 1st face and the front's 1st face share only vertex 0.
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[0, 4], false)
        .mesh();
    let report = repair(&mut mesh);
    assert!(mesh.is_sliceable());
    assert!((volume(&mesh) - 1.0).abs() < 1e-6);
    assert!(has_fix(&report, |f| matches!(
        f,
        MeshFix::FilledHoles {
            holes: 2,
            added_faces: 2,
            ..
        }
    )));
}

#[test]
fn a_lone_plate_is_left_open_over_the_size_cap() {
    // Filling a flat plate would double it: its fill is all of its area.
    let mut mesh = Soup::default()
        .add_box(
            [0.0; 3],
            [1.0, 1.0, 0.0],
            &[0, 1, 4, 5, 6, 7, 8, 9, 10, 11],
            false,
        )
        .mesh();
    assert_eq!(mesh.faces.len(), 2);
    const { assert!(MAX_AUTO_FILL_AREA_FRACTION < 1.0) };
    let report = repair(&mut mesh);
    assert_eq!(report.fixes, Vec::new());
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: false,
            zero_area_face_count: 0,
            holes_left_open: 1,
        })
    );
}

#[test]
fn the_passes_can_be_turned_off() {
    let mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &TOP, false)
        .mesh();
    let without = RepairPasses {
        non_manifold_edges: false,
        holes: false,
        ..RepairPasses::default_repair()
    };
    assert!(matches!(
        assess_repair(&mesh, without),
        RepairAssessment::Clean(MeshStateAfterRepair {
            edge_manifold: false,
            ..
        })
    ));
    // Separating parts alone does not fill holes.
    let only_edges = RepairPasses {
        holes: false,
        ..RepairPasses::default_repair()
    };
    assert!(matches!(
        assess_repair(&mesh, only_edges),
        RepairAssessment::Clean(_)
    ));
}

#[test]
fn a_sliceable_mesh_is_left_alone() {
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .mesh();
    let before = mesh.clone();
    let report = repair(&mut mesh);
    assert_eq!(report.fixes, Vec::new());
    assert_eq!(mesh, before);
}

#[test]
fn the_repair_is_deterministic() {
    let build = || {
        Soup::default()
            .add_box([0.0; 3], [1.0; 3], &TOP, true)
            .add_box([1.0, 1.0, 0.0], [2.0, 2.0, 1.0], &[0], false)
            .add_box([3.0, 0.0, 0.0], [4.0, 1.0, 1.0], &[], false)
            .add_box([4.0, 0.0, 0.0], [5.0, 1.0, 1.0], &[], false)
            .mesh()
    };
    let mut a = build();
    let mut b = build();
    let ra = repair(&mut a);
    let rb = repair(&mut b);
    assert_eq!(a, b);
    assert_eq!(ra, rb);
    assert!(a.is_sliceable());
}

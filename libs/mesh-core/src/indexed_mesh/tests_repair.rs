/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the per-mesh repair policy: pass gating, refusal boundaries
//! (refusals leave the mesh byte-identical), the fix log, and the state of the
//! mesh after a completed run.

use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
    SPLIT_TETRAHEDRON, SQUARE_FAN_PYRAMID, blocked_split_tetrahedron, fan_pyramid,
    flat_tetrahedron, split_tetrahedron,
};
use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::mesh::declared_facet_normals::DeclaredFacetNormals;
use crate::indexed_mesh::mesh::{IndexedMesh, MeshMetadata};
use crate::indexed_mesh::repair::{
    MeshRepairReport, MeshStateAfterRepair, RepairAssessment, RepairEnd, RepairPasses,
    RepairRefusal, assess_repair,
};
use crate::indexed_mesh::validate::{MeshFaceError, MeshVertexError};
use crate::primitives::{
    CoordAxis, FaceIndex, IndexedTriangle, TriVertexSlot, Vertex, VertexIndex,
};

/// Returns the default pass set without the watertight pass, to pin the
/// passes that run after it.
fn passes_before_watertight() -> RepairPasses {
    RepairPasses {
        non_manifold_edges: false,
        holes: false,
        ..RepairPasses::default_repair()
    }
}

/// Returns a closed, consistently wound tetrahedron.
fn tetrahedron() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([1.0, 1.0, 1.0]),
        Vertex::new([1.0, -1.0, -1.0]),
        Vertex::new([-1.0, 1.0, -1.0]),
        Vertex::new([-1.0, -1.0, 1.0]),
    ];
    let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// Returns an open strip of 2 consistently wound triangles sharing 1 edge: the
/// 4 outer edges are boundary edges (a hole).
fn open_strip() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([0.0, 0.0, 0.0]),
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([0.0, 1.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
    ];
    let faces = [[0, 1, 2], [2, 1, 3]]
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// Returns the open strip with vertex 3 at (2, -1, 0), so that face 1 is
/// collinear: vertex 1 is the midpoint of vertices 2 and 3.
fn open_strip_with_a_collinear_face() -> IndexedMesh {
    let mut mesh = open_strip();
    mesh.vertices[3] = Vertex::new([2.0, -1.0, 0.0]);
    mesh
}

/// Appends a face whose 2nd and 3rd slots repeat a vertex index, so face
/// validation reports exactly 1 `ZeroLengthEdge` for it.
fn append_zero_length_edge_face(mesh: &mut IndexedMesh) -> FaceIndex {
    let idx = mesh.faces.len();
    mesh.faces
        .push(IndexedTriangle::from_raw_indices([0, 1, 1]));
    FaceIndex::from(idx)
}

/// Returns the split tetrahedron with face 5 reversed: a winding error and a
/// collinear face.
fn flipped_split_tetrahedron() -> IndexedMesh {
    let mut mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
    mesh.faces[5].vertices.reverse();
    mesh
}

fn triangle(vertices: [u32; 3]) -> IndexedTriangle {
    IndexedTriangle::from_raw_indices(vertices)
}

/// The merge of the collinear face of the split tetrahedron.
fn split_tetrahedron_merge() -> MeshFix {
    MeshFix::RemovedCollinearFaceByMerge {
        collinear: triangle([0, 1, 2]),
        merged_neighbours: [[0, 2, 3], [2, 1, 3]].map(triangle),
        merged: triangle([0, 1, 3]),
    }
}

/// The faces of the split tetrahedron after the merge.
fn merged_split_tetrahedron_faces() -> [IndexedTriangle; 4] {
    [[0, 1, 3], [0, 4, 1], [0, 3, 4], [1, 4, 3]].map(triangle)
}

#[test]
fn default_repair_enables_the_3_passes() {
    let passes = RepairPasses::default_repair();
    assert!(passes.zero_length_edges);
    assert!(passes.flipped_faces);
    assert!(passes.collinear_faces);
    assert_eq!(
        RepairPasses::default(),
        RepairPasses {
            zero_length_edges: false,
            flipped_faces: false,
            collinear_faces: false,
            non_manifold_edges: false,
            holes: false,
        }
    );
}

#[test]
fn all_passes_off_is_a_no_op_on_a_clean_mesh() {
    let mut mesh = tetrahedron();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses::default());
    assert!(report.fixes.is_empty());
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count: 0,
            holes_left_open: 0,
        })
    );
    assert_eq!(mesh, original);
}

#[test]
fn all_passes_off_leaves_a_flipped_face_untouched() {
    let mut mesh = tetrahedron();
    mesh.faces[0].vertices.reverse();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses::default());
    assert!(report.fixes.is_empty());
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: false,
            zero_area_face_count: 0,
            holes_left_open: 0,
        })
    );
    assert_eq!(mesh, original);
}

#[test]
fn default_repair_fixes_a_flipped_face() {
    let mut mesh = tetrahedron();
    mesh.faces[0].vertices.reverse();
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(report.fixes, vec![MeshFix::ReorientedFaces { faces: 1 }]);
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count: 0,
            holes_left_open: 0,
        })
    );
    assert_eq!(mesh.faces[0].vertices, tetrahedron().faces[0].vertices);
}

#[test]
fn zero_length_edge_faces_are_removed_with_a_pinned_log() {
    let mut mesh = tetrahedron();
    let degenerate = append_zero_length_edge_face(&mut mesh);
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(
        report.fixes,
        vec![MeshFix::RemovedZeroLengthEdgeFace {
            face_idx: degenerate,
        }]
    );
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count: 0,
            holes_left_open: 0,
        })
    );
    assert_eq!(mesh.faces.len(), 4);
}

/// Returns `mesh` with 1 declared facet normal per face in its metadata.
fn with_declared_facet_normals(mut mesh: IndexedMesh) -> IndexedMesh {
    let normals = vec![[0, 0, 0x3f80_0000]; mesh.faces.len()];
    let declared = DeclaredFacetNormals::capture(&mesh, normals).expect("1 normal per face");
    mesh.set_meta(Some(MeshMetadata {
        declared_facet_normals: Some(declared),
        ..Default::default()
    }));
    mesh
}

#[test]
fn a_repair_that_applies_a_fix_drops_the_declared_facet_normals() {
    let mut mesh = tetrahedron();
    append_zero_length_edge_face(&mut mesh);
    let mut mesh = with_declared_facet_normals(mesh);
    let report = mesh.repair(RepairPasses::default_repair());
    assert!(!report.fixes.is_empty());
    assert_eq!(mesh.meta, Some(MeshMetadata::default()));
}

#[test]
fn a_repair_without_a_fix_keeps_the_declared_facet_normals() {
    let mut mesh = with_declared_facet_normals(tetrahedron());
    let report = mesh.repair(RepairPasses::default_repair());
    assert!(report.fixes.is_empty());
    assert_eq!(
        mesh.declared_facet_normals(),
        Some(&[[0, 0, 0x3f80_0000]; 4][..])
    );
}

#[test]
fn nan_vertices_refuse_with_the_mesh_untouched() {
    let mut mesh = tetrahedron();
    mesh.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses::default_repair());
    assert!(report.fixes.is_empty());
    match &report.end {
        RepairEnd::Refused(RepairRefusal::InvalidVertices(errors)) => {
            assert!(!errors.is_empty());
        }
        other => panic!("expected InvalidVertices, got {other:?}"),
    }
    // `PartialEq` cannot pin an untouched NaN mesh (NaN != NaN), so the
    // vertices are compared bit for bit.
    assert_eq!(mesh.faces, original.faces);
    assert_eq!(mesh.vertices.len(), original.vertices.len());
    for (kept, expected) in mesh.vertices.iter().zip(original.vertices.iter()) {
        assert_eq!(
            kept.coords().map(f32::to_bits),
            expected.coords().map(f32::to_bits)
        );
    }
}

#[test]
fn zle_pass_disabled_refuses_carrying_every_face_error() {
    let mut mesh = tetrahedron();
    append_zero_length_edge_face(&mut mesh);
    let expected_errors = mesh.validate_faces().unwrap_err();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses {
        zero_length_edges: false,
        flipped_faces: true,
        collinear_faces: false,
        non_manifold_edges: false,
        holes: false,
    });
    assert!(report.fixes.is_empty());
    assert_eq!(
        report.end,
        RepairEnd::Refused(RepairRefusal::UnfixableFaceErrors(expected_errors))
    );
    assert_eq!(mesh, original);
}

#[test]
fn zle_pass_disabled_assesses_as_a_refusal_carrying_every_face_error() {
    let mut mesh = tetrahedron();
    append_zero_length_edge_face(&mut mesh);
    let expected_errors = mesh.validate_faces().unwrap_err();
    let passes = RepairPasses {
        zero_length_edges: false,
        flipped_faces: true,
        collinear_faces: false,
        non_manifold_edges: false,
        holes: false,
    };
    assert_eq!(
        assess_repair(&mesh, passes),
        RepairAssessment::Refused(RepairRefusal::UnfixableFaceErrors(expected_errors))
    );
}

/// Pins that a face naming a vertex that does not exist is dropped with the
/// zero-length-edge faces, instead of refusing the whole mesh, and that the
/// rest of the run goes on.
#[test]
fn faces_with_an_out_of_range_index_are_dropped_not_refused() {
    let mut mesh = tetrahedron();
    let degenerate = append_zero_length_edge_face(&mut mesh);
    let out_of_range = FaceIndex::from(mesh.faces.len());
    mesh.faces
        .push(IndexedTriangle::from_raw_indices([0, 99, 100]));
    mesh.faces[0].vertices.reverse();

    let report = mesh.repair(RepairPasses::default_repair());

    assert_eq!(
        report.fixes[..2],
        [
            MeshFix::RemovedZeroLengthEdgeFace {
                face_idx: degenerate,
            },
            MeshFix::RemovedOutOfRangeFace {
                face_idx: out_of_range,
            },
        ]
    );
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count: 0,
            holes_left_open: 0,
        })
    );
    assert_eq!(
        mesh.faces,
        tetrahedron().faces,
        "the flipped face is fixed too"
    );
}

/// Pins #298: a mesh without faces gets its own result, with nothing run and
/// nothing changed, whatever its vertices and passes.
#[test]
fn a_mesh_without_faces_ends_as_no_faces() {
    let empty = IndexedMesh {
        meta: None,
        vertices: vec![Vertex::new([f32::NAN, 0.0, 0.0])],
        faces: Vec::new(),
    };
    for passes in [RepairPasses::default_repair(), RepairPasses::default()] {
        assert_eq!(assess_repair(&empty, passes), RepairAssessment::NoFaces);
        let mut mesh = empty.clone();
        let report = mesh.repair(passes);
        assert!(report.fixes.is_empty());
        assert_eq!(report.end, RepairEnd::NoFaces);
        assert_eq!(mesh.faces, empty.faces);
    }
}

/// Pins that half the faces flipped, where the old per-face rule refused, is
/// oriented as a whole and comes out outward, after the zero-length-edge
/// removal.
#[test]
fn half_the_faces_flipped_come_out_outward() {
    let mut mesh = tetrahedron();
    mesh.faces[0].vertices.reverse();
    mesh.faces[1].vertices.reverse();
    let degenerate = append_zero_length_edge_face(&mut mesh);

    let report = mesh.repair(passes_before_watertight());

    assert_eq!(
        report.fixes[0],
        MeshFix::RemovedZeroLengthEdgeFace {
            face_idx: degenerate,
        }
    );
    assert!(matches!(report.end, RepairEnd::Completed(state) if state.edge_manifold));
    assert_eq!(mesh.faces, tetrahedron().faces);
}

/// Pins the flip pass gate in both functions: with the pass off, a lone
/// triangle and a flipped face assess as clean and are left untouched.
#[test]
fn a_disabled_flip_pass_assesses_winding_problems_as_clean() {
    let passes = RepairPasses {
        zero_length_edges: true,
        ..RepairPasses::default()
    };
    let flipped = {
        let mut mesh = tetrahedron();
        mesh.faces[0].vertices.reverse();
        mesh
    };
    let lone_triangle = {
        let mut mesh = open_strip();
        mesh.faces.truncate(1);
        mesh
    };
    let state = MeshStateAfterRepair {
        edge_manifold: false,
        zero_area_face_count: 0,
        holes_left_open: 0,
    };
    for (name, mesh) in [("flipped", flipped), ("lone-triangle", lone_triangle)] {
        assert_eq!(
            assess_repair(&mesh, passes),
            RepairAssessment::Clean(state),
            "{name}"
        );
        let mut repaired = mesh.clone();
        let report = repaired.repair(passes);
        assert!(report.fixes.is_empty(), "{name}");
        assert_eq!(report.end, RepairEnd::Completed(state), "{name}");
        assert_eq!(repaired, mesh, "{name}");
    }
}

#[test]
fn a_hole_is_reported_by_the_verdict_not_a_refusal() {
    // The strip is a flat plate: filling it would double it, over the size
    // cap, so the hole stays open and is counted.
    let mut mesh = open_strip();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses::default_repair());
    assert!(report.fixes.is_empty());
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: false,
            zero_area_face_count: 0,
            holes_left_open: 1,
        })
    );
    assert_eq!(mesh, original);
}

#[test]
fn default_repair_completes_a_clean_tetrahedron_with_no_zero_area_face() {
    let mut mesh = tetrahedron();
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(
        report,
        MeshRepairReport {
            fixes: Vec::new(),
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: true,
                zero_area_face_count: 0,
                holes_left_open: 0,
            }),
        }
    );
}

/// Pins that no pass removes the collinear face of a closed, flat tetrahedron,
/// and that the report counts it.
#[test]
fn default_repair_counts_the_zero_area_face_of_a_flat_tetrahedron() {
    let mut mesh = flat_tetrahedron();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(
        report,
        MeshRepairReport {
            fixes: Vec::new(),
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: true,
                zero_area_face_count: 1,
                holes_left_open: 0,
            }),
        }
    );
    assert_eq!(mesh, original);
}

/// Pins that the report counts the zero-area faces of a mesh that is not
/// edge-manifold too.
#[test]
fn default_repair_counts_the_collinear_face_of_an_open_mesh() {
    let mut mesh = open_strip_with_a_collinear_face();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(
        report,
        MeshRepairReport {
            fixes: Vec::new(),
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: false,
                zero_area_face_count: 1,
                holes_left_open: 1,
            }),
        }
    );
    assert_eq!(mesh, original);
}

#[test]
fn the_collinear_face_pass_merges_the_neighbours_of_a_collinear_face() {
    let mut mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(
        report,
        MeshRepairReport {
            fixes: vec![split_tetrahedron_merge()],
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: true,
                zero_area_face_count: 0,
                holes_left_open: 0,
            }),
        }
    );
    assert_eq!(mesh.faces, merged_split_tetrahedron_faces());
}

#[test]
fn the_collinear_face_pass_splits_the_face_across_the_long_edge() {
    let mut mesh = fan_pyramid(SQUARE_FAN_PYRAMID);
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(
        report,
        MeshRepairReport {
            fixes: vec![MeshFix::RemovedCollinearFaceBySplit {
                collinear: triangle([0, 1, 2]),
                opposite: triangle([1, 0, 3]),
                halves: [[2, 0, 3], [1, 2, 3]].map(triangle),
            }],
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: true,
                zero_area_face_count: 0,
                holes_left_open: 0,
            }),
        }
    );
    let mut expected = fan_pyramid(SQUARE_FAN_PYRAMID).faces;
    expected[0] = triangle([2, 0, 3]);
    expected[3] = triangle([1, 2, 3]);
    assert_eq!(mesh.faces, expected);
}

/// Pins the pass order: the flip makes the mesh edge-manifold, so the
/// collinear-face pass runs after it, in the same run.
#[test]
fn the_orientation_pass_runs_before_the_collinear_face_pass() {
    let mut mesh = flipped_split_tetrahedron();
    let report = mesh.repair(passes_before_watertight());
    assert_eq!(
        report.fixes,
        vec![
            MeshFix::ReorientedFaces { faces: 1 },
            split_tetrahedron_merge(),
        ]
    );
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count: 0,
            holes_left_open: 0,
        })
    );
    assert_eq!(mesh.faces, merged_split_tetrahedron_faces());
}

#[test]
fn a_collinear_face_fix_drops_the_declared_facet_normals() {
    let mut mesh = with_declared_facet_normals(split_tetrahedron(SPLIT_TETRAHEDRON));
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(report.fixes, [split_tetrahedron_merge()]);
    assert_eq!(mesh.meta, Some(MeshMetadata::default()));
}

#[test]
fn a_disabled_collinear_face_pass_leaves_the_collinear_face() {
    let mut mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses {
        collinear_faces: false,
        ..RepairPasses::default_repair()
    });
    assert_eq!(
        report,
        MeshRepairReport {
            fixes: Vec::new(),
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: true,
                zero_area_face_count: 1,
                holes_left_open: 0,
            }),
        }
    );
    assert_eq!(mesh, original);
}

#[test]
fn a_winding_error_that_stays_keeps_the_collinear_pass_from_running() {
    let mut mesh = flipped_split_tetrahedron();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses {
        flipped_faces: false,
        ..passes_before_watertight()
    });
    assert_eq!(
        report,
        MeshRepairReport {
            fixes: Vec::new(),
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: false,
                zero_area_face_count: 1,
                holes_left_open: 0,
            }),
        }
    );
    assert_eq!(mesh, original);
}

/// Pins that the shared-borrow assessment and the mutating run never disagree:
/// for every fixture class and 3 pass sets, `assess_repair` predicts exactly
/// what `repair` reports on a clone of the same mesh.
#[test]
fn assessment_matches_the_repair_report_for_every_fixture_class() {
    let flipped = {
        let mut mesh = tetrahedron();
        mesh.faces[0].vertices.reverse();
        mesh
    };
    let with_zle = {
        let mut mesh = tetrahedron();
        append_zero_length_edge_face(&mut mesh);
        mesh
    };
    let mixed = {
        let mut mesh = tetrahedron();
        append_zero_length_edge_face(&mut mesh);
        mesh.faces
            .push(IndexedTriangle::from_raw_indices([0, 1, 99]));
        mesh
    };
    let nan = {
        let mut mesh = tetrahedron();
        mesh.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
        mesh
    };
    let inside_out = {
        let mut mesh = tetrahedron();
        for face in &mut mesh.faces {
            face.vertices.reverse();
        }
        mesh
    };
    let empty = IndexedMesh {
        meta: None,
        vertices: Vec::new(),
        faces: Vec::new(),
    };
    let lone_triangle = {
        let mut mesh = open_strip();
        mesh.faces.truncate(1);
        mesh
    };

    let cases: Vec<(&str, IndexedMesh)> = vec![
        ("clean", tetrahedron()),
        ("flipped", flipped),
        ("zle", with_zle),
        ("mixed", mixed),
        ("nan", nan),
        ("inside-out", inside_out),
        ("empty", empty),
        ("lone-triangle", lone_triangle),
        ("open-strip", open_strip()),
        ("flat-tetrahedron", flat_tetrahedron()),
        ("open-collinear", open_strip_with_a_collinear_face()),
        ("collinear-merge", split_tetrahedron(SPLIT_TETRAHEDRON)),
        ("collinear-split", fan_pyramid(SQUARE_FAN_PYRAMID)),
        ("flipped-collinear", flipped_split_tetrahedron()),
        ("blocked-collinear", blocked_split_tetrahedron()),
    ];
    let pass_sets = [
        RepairPasses::default_repair(),
        passes_before_watertight(),
        RepairPasses {
            flipped_faces: false,
            ..RepairPasses::default_repair()
        },
        RepairPasses {
            collinear_faces: false,
            ..RepairPasses::default_repair()
        },
    ];
    for passes in pass_sets {
        for (name, mesh) in &cases {
            let assessment = assess_repair(mesh, passes);
            let mut clone = mesh.clone();
            let report = clone.repair(passes);
            match assessment {
                RepairAssessment::Clean(state) => {
                    assert!(
                        report.fixes.is_empty(),
                        "{name} under {passes:?}: Clean predicts no fixes"
                    );
                    assert_eq!(
                        report.end,
                        RepairEnd::Completed(state),
                        "{name} under {passes:?}: Clean predicts the state after the run"
                    );
                }
                RepairAssessment::Refused(refusal) => {
                    assert!(
                        report.fixes.is_empty(),
                        "{name} under {passes:?}: Refused predicts no fixes"
                    );
                    assert_eq!(
                        report.end,
                        RepairEnd::Refused(refusal),
                        "{name} under {passes:?}: the refusal must match"
                    );
                }
                RepairAssessment::Work => {
                    assert!(
                        !report.fixes.is_empty(),
                        "{name} under {passes:?}: Work predicts applied fixes"
                    );
                }
                RepairAssessment::NoFaces => {
                    assert!(report.fixes.is_empty(), "{name} under {passes:?}");
                    assert_eq!(report.end, RepairEnd::NoFaces, "{name} under {passes:?}");
                }
            }
        }
    }
    // The matrix covers all 4 assessment kinds; pin the per-case kinds so a
    // fixture drifting into another class is caught.
    let passes = RepairPasses::default_repair();
    for (mesh, edge_manifold, zero_area_face_count, holes_left_open) in [
        (tetrahedron(), true, 0, 0),
        (open_strip(), false, 0, 1),
        (flat_tetrahedron(), true, 1, 0),
        (blocked_split_tetrahedron(), true, 1, 0),
        (open_strip_with_a_collinear_face(), false, 1, 1),
        (
            {
                let mut m = open_strip();
                m.faces.truncate(1);
                m
            },
            false,
            0,
            1,
        ),
    ] {
        assert_eq!(
            assess_repair(&mesh, passes),
            RepairAssessment::Clean(MeshStateAfterRepair {
                edge_manifold,
                zero_area_face_count,
                holes_left_open,
            })
        );
    }
    assert!(matches!(
        assess_repair(
            &{
                let mut m = tetrahedron();
                m.faces[0].vertices.reverse();
                m
            },
            passes
        ),
        RepairAssessment::Work
    ));
    for mesh in [
        split_tetrahedron(SPLIT_TETRAHEDRON),
        fan_pyramid(SQUARE_FAN_PYRAMID),
        flipped_split_tetrahedron(),
    ] {
        assert_eq!(assess_repair(&mesh, passes), RepairAssessment::Work);
    }
    let without_flips = RepairPasses {
        flipped_faces: false,
        ..passes_before_watertight()
    };
    assert_eq!(
        assess_repair(&flipped_split_tetrahedron(), without_flips),
        RepairAssessment::Clean(MeshStateAfterRepair {
            edge_manifold: false,
            zero_area_face_count: 1,
            holes_left_open: 0,
        })
    );
}

#[test]
fn repair_refusals_display_their_cause() {
    let nan = MeshVertexError::NaNCoordinate {
        vertex_idx: 0,
        coord: CoordAxis::X,
    };
    let zle = MeshFaceError::ZeroLengthEdge {
        face_idx: FaceIndex::from(4_usize),
        vertex_pos_a: TriVertexSlot::V1,
        vertex_pos_b: TriVertexSlot::V2,
        vertex_idx: VertexIndex::from(1_u32),
    };
    let cases = [
        (
            RepairRefusal::InvalidVertices(vec![nan.clone(), nan]),
            "mesh has 2 invalid vertex coordinates",
        ),
        (
            RepairRefusal::UnfixableFaceErrors(vec![zle.clone(), zle]),
            "2 face errors the enabled repair passes cannot address",
        ),
    ];
    for (refusal, text) in cases {
        assert_eq!(refusal.to_string(), text);
    }
}

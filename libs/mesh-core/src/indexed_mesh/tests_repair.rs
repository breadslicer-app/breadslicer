/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the per-mesh repair policy: pass gating, refusal boundaries
//! (mesh-level refusals leave the mesh byte-identical, pass-level refusals keep
//! earlier passes' fixes), the fix log, and the state of the mesh after a
//! completed run.

use std::error::Error;

use crate::indexed_mesh::declared_facet_normals::DeclaredFacetNormals;
use crate::indexed_mesh::fix::MeshFixError;
use crate::indexed_mesh::fix::collinear_faces::test_meshes::flat_tetrahedron;
use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::mesh::{IndexedMesh, MeshMetadata};
use crate::indexed_mesh::repair::{
    MeshRepairReport, MeshStateAfterRepair, RepairAssessment, RepairEnd, RepairPasses,
    RepairRefusal, assess_repair,
};
use crate::indexed_mesh::validate::{MeshFaceError, MeshVertexError};
use crate::primitives::{
    CoordAxis, FaceIndex, IndexedTriangle, TriVertexSlot, Vertex, VertexIndex,
};

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
/// 4 outer edges are boundary edges (a hole), and with 2 faces the flip pass's
/// inside-out threshold (`faces / 2 = 1`) is above the 0 flips detected, so the
/// strip exercises the hole verdict without the lone-face case, where a
/// threshold of 0 makes the flip pass refuse.
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

#[test]
fn default_repair_enables_zle_and_flips_only() {
    let passes = RepairPasses::default_repair();
    assert!(passes.zero_length_edges);
    assert!(passes.flipped_faces);
    assert_eq!(
        RepairPasses::default(),
        RepairPasses {
            zero_length_edges: false,
            flipped_faces: false,
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
        })
    );
    assert_eq!(mesh, original);
}

#[test]
fn default_repair_fixes_a_flipped_face() {
    let mut mesh = tetrahedron();
    mesh.faces[0].vertices.reverse();
    let report = mesh.repair(RepairPasses::default_repair());
    assert_eq!(
        report.fixes,
        vec![MeshFix::FlippedFaceOrientation {
            face_idx: FaceIndex::from(0_usize),
        }]
    );
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count: 0,
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
    };
    assert_eq!(
        assess_repair(&mesh, passes),
        RepairAssessment::Refused(RepairRefusal::UnfixableFaceErrors(expected_errors))
    );
}

/// Pins the mesh-level refusal boundary: face errors the enabled passes cannot
/// address refuse the run before any mutation, and the refusal carries exactly
/// the unaddressable residue.
#[test]
fn mixed_face_errors_refuse_with_the_mesh_byte_identical() {
    let mut mesh = tetrahedron();
    append_zero_length_edge_face(&mut mesh);
    mesh.faces
        .push(IndexedTriangle::from_raw_indices([0, 1, 99]));
    let residue: Vec<MeshFaceError> = mesh
        .validate_faces()
        .unwrap_err()
        .into_iter()
        .filter(|e| !matches!(e, MeshFaceError::ZeroLengthEdge { .. }))
        .collect();
    assert!(!residue.is_empty(), "fixture must carry a non-ZLE error");
    let original = mesh.clone();

    let report = mesh.repair(RepairPasses::default_repair());

    assert!(report.fixes.is_empty());
    assert_eq!(
        report.end,
        RepairEnd::Refused(RepairRefusal::UnfixableFaceErrors(residue))
    );
    assert_eq!(mesh, original, "mesh-level refusal must not mutate");
}

/// Pins the pass-level refusal boundary: an orientation failure keeps the
/// zero-length-edge fixes already applied; the mesh holds exactly the completed
/// passes' state.
#[test]
fn orientation_refusal_keeps_the_zero_length_edge_fixes() {
    let mut mesh = tetrahedron();
    // 2 of 4 faces flipped puts every face at 2 winding complaints, so the flip
    // pass refuses with TooManyFlippedFaces.
    mesh.faces[0].vertices.reverse();
    mesh.faces[1].vertices.reverse();
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
        RepairEnd::Refused(RepairRefusal::OrientationFixFailed(
            MeshFixError::TooManyFlippedFaces
        ))
    );
    assert_eq!(mesh.faces.len(), 4, "the ZLE removal must stand");
}

/// Pins the flip pass gate in both functions: with the pass off, a lone
/// triangle, which trips the flip threshold when the pass is on, assesses as
/// clean and is left untouched, as is a flipped face.
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
    let mut mesh = open_strip();
    let original = mesh.clone();
    let report = mesh.repair(RepairPasses::default_repair());
    assert!(report.fixes.is_empty());
    assert_eq!(
        report.end,
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: false,
            zero_area_face_count: 0,
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
            }),
        }
    );
    assert_eq!(mesh, original);
}

/// Pins that the shared-borrow assessment and the mutating run never disagree:
/// for every fixture class, `assess_repair` predicts exactly what `repair`
/// reports on a clone of the same mesh.
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
    // A lone open triangle trips the flip pass's threshold of 0 (half its face
    // count, rounded down); the pin here is only that assessment and run agree
    // about it.
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
        ("lone-triangle", lone_triangle),
        ("open-strip", open_strip()),
        ("flat-tetrahedron", flat_tetrahedron()),
        ("open-collinear", open_strip_with_a_collinear_face()),
    ];
    for (name, mesh) in cases {
        let assessment = assess_repair(&mesh, RepairPasses::default_repair());
        let mut clone = mesh.clone();
        let report = clone.repair(RepairPasses::default_repair());
        match assessment {
            RepairAssessment::Clean(state) => {
                assert!(report.fixes.is_empty(), "{name}: Clean predicts no fixes");
                assert_eq!(
                    report.end,
                    RepairEnd::Completed(state),
                    "{name}: Clean predicts the state after the run"
                );
            }
            RepairAssessment::Refused(refusal) => {
                assert!(report.fixes.is_empty(), "{name}: Refused predicts no fixes");
                assert_eq!(
                    report.end,
                    RepairEnd::Refused(refusal),
                    "{name}: the refusal must match"
                );
            }
            RepairAssessment::Work => {
                assert!(
                    !report.fixes.is_empty(),
                    "{name}: Work predicts applied fixes"
                );
            }
        }
    }
    // The matrix covers all 3 assessment kinds; pin the per-case kinds so a
    // fixture drifting into another class is caught.
    let passes = RepairPasses::default_repair();
    for (mesh, edge_manifold, zero_area_face_count) in [
        (tetrahedron(), true, 0),
        (open_strip(), false, 0),
        (flat_tetrahedron(), true, 1),
        (open_strip_with_a_collinear_face(), false, 1),
    ] {
        assert_eq!(
            assess_repair(&mesh, passes),
            RepairAssessment::Clean(MeshStateAfterRepair {
                edge_manifold,
                zero_area_face_count,
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
    assert!(matches!(
        assess_repair(
            &{
                let mut m = open_strip();
                m.faces.truncate(1);
                m
            },
            passes
        ),
        RepairAssessment::Refused(RepairRefusal::OrientationFixFailed(
            MeshFixError::TooManyFlippedFaces
        ))
    ));
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
        (
            RepairRefusal::OrientationFixFailed(MeshFixError::TooManyFlippedFaces),
            "face orientation fix failed: too many flipped faces",
        ),
    ];
    for (refusal, text) in cases {
        assert_eq!(refusal.to_string(), text);
    }
}

#[test]
fn only_the_orientation_refusal_has_a_source() {
    let failed = RepairRefusal::OrientationFixFailed(MeshFixError::TooManyFlippedFaces);
    assert_eq!(
        failed
            .source()
            .and_then(|source| source.downcast_ref::<MeshFixError>()),
        Some(&MeshFixError::TooManyFlippedFaces)
    );
    assert!(RepairRefusal::InvalidVertices(vec![]).source().is_none());
    assert!(
        RepairRefusal::UnfixableFaceErrors(vec![])
            .source()
            .is_none()
    );
}

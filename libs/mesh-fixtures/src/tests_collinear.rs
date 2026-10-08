/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::indexed_mesh::ZeroAreaFace;
use sc_mesh_core::primitives::FaceIndex;

use crate::unit_cube_with_collinear_faces;

#[test]
fn collinear_cube_is_closed_and_edge_manifold() {
    let mesh = unit_cube_with_collinear_faces();
    assert_eq!(mesh.vertices.len(), 10);
    assert_eq!(mesh.faces.len(), 16);
    // On an edge-manifold mesh every edge bounds 2 faces, so 16 faces have 24
    // edges and the Euler characteristic is 10 - 24 + 16 = 2.
    mesh.validated_faces()
        .expect("indices in range, no repeated index")
        .validated_edge_manifold()
        .expect("every edge bounds 2 consistently wound faces");
}

#[test]
fn collinear_cube_has_exactly_the_2_collinear_faces_with_zero_area() {
    let mesh = unit_cube_with_collinear_faces();
    let faces = mesh.validated_faces().expect("indices in range");
    assert_eq!(
        faces.validate_face_areas(),
        Err(vec![
            ZeroAreaFace {
                face_idx: FaceIndex::from(14)
            },
            ZeroAreaFace {
                face_idx: FaceIndex::from(15)
            },
        ])
    );
}

#[test]
fn collinear_cube_encloses_the_volume_of_the_unit_cube() {
    let mesh = unit_cube_with_collinear_faces();
    let faces = mesh.validated_faces().expect("indices in range");
    assert_eq!(faces.signed_volume_mm3(), 1.0);
}

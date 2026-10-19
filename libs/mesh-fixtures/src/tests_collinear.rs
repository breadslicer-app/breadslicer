/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::ZeroAreaFace;
use sc_mesh_core::primitives::FaceIndex;

use crate::{
    cuboid_with_y_tunnel_and_collinear_faces,
    cuboid_with_y_tunnel_and_collinear_faces_at_fan_centres,
    cuboid_with_y_tunnel_and_flat_tetrahedron, cuboid_with_z_tunnel_and_collinear_faces,
    cuboid_with_z_tunnel_and_collinear_faces_at_fan_centres,
    cuboid_with_z_tunnel_and_flat_tetrahedron, unit_cube_and_flat_tetrahedron,
    unit_cube_with_collinear_faces, unit_cube_with_collinear_faces_at_fan_centres,
};

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

#[test]
fn collinear_cube_keeps_its_literal_vertices_and_faces() {
    let mesh = unit_cube_with_collinear_faces();
    let vertices: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.coords()).collect();
    assert_eq!(
        vertices,
        vec![
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
            [0.0, -0.5, 0.5],
            [0.5, -0.5, 0.0],
        ]
    );
    let faces: Vec<[u32; 3]> = mesh
        .faces
        .iter()
        .map(|f| f.vertices.map(u32::from))
        .collect();
    assert_eq!(
        faces,
        vec![
            [0, 3, 2],
            [0, 2, 1],
            [4, 8, 6],
            [4, 6, 7],
            [3, 7, 6],
            [3, 6, 2],
            [0, 1, 9],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [0, 4, 7],
            [0, 7, 3],
            [8, 5, 6],
            [0, 9, 5],
            [4, 5, 8],
            [1, 5, 9],
        ]
    );
}

/// A variant with zero-area faces and what it must hold.
struct ZeroAreaVariant {
    name: &'static str,
    mesh: IndexedMesh,
    vertex_count: usize,
    face_count: usize,
    zero_area_faces: &'static [usize],
    volume_mm3: f64,
    volume_tolerance_mm3: f64,
}

/// Returns the variants with zero-area faces, other than
/// [`unit_cube_with_collinear_faces`]. The prisms hold the f32 values of 0.4,
/// 0.6 and 1.4, so their volumes miss 6.72 by less than 1e-7.
fn zero_area_variants() -> [ZeroAreaVariant; 8] {
    let cube = |name, mesh, vertex_count, zero_area_faces| ZeroAreaVariant {
        name,
        mesh,
        vertex_count,
        face_count: 16,
        zero_area_faces,
        volume_mm3: 1.0,
        volume_tolerance_mm3: 0.0,
    };
    let prism = |name, mesh, vertex_count, zero_area_faces| ZeroAreaVariant {
        name,
        mesh,
        vertex_count,
        face_count: 36,
        zero_area_faces,
        volume_mm3: 6.72,
        volume_tolerance_mm3: 1e-6,
    };
    [
        prism(
            "z_tunnel_collinear",
            cuboid_with_z_tunnel_and_collinear_faces(),
            18,
            &[34, 35],
        ),
        prism(
            "y_tunnel_collinear",
            cuboid_with_y_tunnel_and_collinear_faces(),
            18,
            &[34, 35],
        ),
        cube(
            "cube_fan_centres",
            unit_cube_with_collinear_faces_at_fan_centres(),
            10,
            &[14, 15],
        ),
        prism(
            "z_tunnel_fan_centres",
            cuboid_with_z_tunnel_and_collinear_faces_at_fan_centres(),
            18,
            &[34, 35],
        ),
        prism(
            "y_tunnel_fan_centres",
            cuboid_with_y_tunnel_and_collinear_faces_at_fan_centres(),
            18,
            &[34, 35],
        ),
        cube(
            "cube_flat_tetrahedron",
            unit_cube_and_flat_tetrahedron(),
            12,
            &[12],
        ),
        prism(
            "z_tunnel_flat_tetrahedron",
            cuboid_with_z_tunnel_and_flat_tetrahedron(),
            20,
            &[32],
        ),
        prism(
            "y_tunnel_flat_tetrahedron",
            cuboid_with_y_tunnel_and_flat_tetrahedron(),
            20,
            &[32],
        ),
    ]
}

#[test]
fn each_zero_area_variant_has_its_vertex_and_face_counts() {
    for variant in zero_area_variants() {
        let name = variant.name;
        assert_eq!(variant.mesh.vertices.len(), variant.vertex_count, "{name}");
        assert_eq!(variant.mesh.faces.len(), variant.face_count, "{name}");
    }
}

#[test]
fn each_zero_area_variant_is_closed_and_edge_manifold() {
    for variant in zero_area_variants() {
        let name = variant.name;
        let faces = variant.mesh.validated_faces().expect(name);
        assert!(faces.validated_edge_manifold().is_ok(), "{name}");
    }
}

#[test]
fn each_zero_area_variant_has_exactly_its_zero_area_faces() {
    for variant in zero_area_variants() {
        let name = variant.name;
        let faces = variant.mesh.validated_faces().expect(name);
        let expected: Vec<ZeroAreaFace> = variant
            .zero_area_faces
            .iter()
            .map(|&idx| ZeroAreaFace {
                face_idx: FaceIndex::from(idx),
            })
            .collect();
        assert_eq!(faces.validate_face_areas(), Err(expected), "{name}");
    }
}

#[test]
fn each_zero_area_variant_encloses_the_volume_of_its_donor() {
    for variant in zero_area_variants() {
        let name = variant.name;
        let faces = variant.mesh.validated_faces().expect(name);
        let error = (faces.signed_volume_mm3() - variant.volume_mm3).abs();
        assert!(error <= variant.volume_tolerance_mm3, "{name}: {error}");
    }
}

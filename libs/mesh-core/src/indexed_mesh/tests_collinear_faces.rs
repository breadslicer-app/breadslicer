/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the removal of collinear faces through
//! `ValidatedEdgeManifoldMeshMut::fix_collinear_faces`: the records, the faces
//! after the pass, the order of the scans, and the faces that stay. After each
//! run, the mesh is edge-manifold and has exactly the listed zero-area faces.

use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
    SPLIT_TETRAHEDRON, SQUARE_FAN_PYRAMID, blocked_split_tetrahedron, collinear_pair, fan_pyramid,
    flat_tetrahedron, split_tetrahedron,
};
use crate::indexed_mesh::validated::ZeroAreaFace;
use crate::indexed_mesh::{IndexedMesh, MeshFix};
use crate::primitives::{FaceIndex, IndexedTriangle, Vertex, VertexIndex};

fn triangle(vertices: [u32; 3]) -> IndexedTriangle {
    IndexedTriangle::from_raw_indices(vertices)
}

fn triangles(raw: &[[u32; 3]]) -> Vec<IndexedTriangle> {
    raw.iter().copied().map(triangle).collect()
}

fn merge(collinear: [u32; 3], merged_neighbours: [[u32; 3]; 2], merged: [u32; 3]) -> MeshFix {
    MeshFix::RemovedCollinearFaceByMerge {
        collinear: triangle(collinear),
        merged_neighbours: merged_neighbours.map(triangle),
        merged: triangle(merged),
    }
}

fn split(collinear: [u32; 3], opposite: [u32; 3], halves: [[u32; 3]; 2]) -> MeshFix {
    MeshFix::RemovedCollinearFaceBySplit {
        collinear: triangle(collinear),
        opposite: triangle(opposite),
        halves: halves.map(triangle),
    }
}

/// Runs the pass on `mesh`, which must be edge-manifold, and checks that the
/// mesh is still edge-manifold, with the zero-area faces `zero_area_after`.
fn fix_collinear_faces(mesh: &mut IndexedMesh, zero_area_after: &[usize]) -> Vec<MeshFix> {
    let fixes = mesh
        .validated_faces_mut()
        .expect("in-range indices")
        .validated_edge_manifold_mut()
        .expect("edge-manifold")
        .fix_collinear_faces();
    let faces = mesh.validated_faces().expect("in-range indices");
    let expected: Vec<ZeroAreaFace> = zero_area_after
        .iter()
        .map(|&face| ZeroAreaFace {
            face_idx: FaceIndex::from(face),
        })
        .collect();
    assert_eq!(
        faces.validate_face_areas().err().unwrap_or_default(),
        expected
    );
    assert!(faces.validated_edge_manifold().is_ok(), "edge-manifold");
    fixes
}

/// Returns 1 mesh with the vertices and the faces of `parts`, in their order.
fn disjoint_union(parts: &[IndexedMesh]) -> IndexedMesh {
    let mut union = IndexedMesh {
        meta: None,
        vertices: Vec::new(),
        faces: Vec::new(),
    };
    for part in parts {
        let offset = union.vertices.len() as u32;
        union.vertices.extend_from_slice(&part.vertices);
        union
            .faces
            .extend(part.faces.iter().map(|face| IndexedTriangle {
                vertices: face.vertices.map(|v| v.offset(offset)),
            }));
    }
    union
}

#[test]
fn a_merge_drops_2_faces_and_keeps_the_order_of_the_others() {
    let mut mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
    let vertices = mesh.vertices.clone();
    let fixes = fix_collinear_faces(&mut mesh, &[]);
    assert_eq!(fixes, [merge([0, 1, 2], [[0, 2, 3], [2, 1, 3]], [0, 1, 3])]);
    assert_eq!(
        mesh.faces,
        triangles(&[[0, 1, 3], [0, 4, 1], [0, 3, 4], [1, 4, 3]])
    );
    assert_eq!(mesh.vertices, vertices);
}

/// Returns a pyramid on the square P (0), Q (1), Y (3), X (2) with its apex S
/// (4), where R1 (5) and R2 (6) split the edge P-Q in 3. The collinear face 8,
/// (P, R2, R1), has its long edge P-R2 on the collinear face 9, (P, Q, R2).
fn chained_pyramid() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: [
            [0.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [0.0, -3.0, 0.0],
            [3.0, -3.0, 0.0],
            [1.5, -1.5, 1.5],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
        ]
        .map(Vertex::new)
        .to_vec(),
        faces: triangles(&[
            [0, 5, 2],
            [5, 3, 2],
            [5, 6, 3],
            [6, 1, 3],
            [1, 0, 4],
            [3, 1, 4],
            [2, 3, 4],
            [0, 2, 4],
            [0, 6, 5],
            [0, 1, 6],
        ]),
    }
}

#[test]
fn a_chain_in_the_unfavourable_order_needs_a_2nd_scan() {
    let mut mesh = chained_pyramid();
    let fixes = fix_collinear_faces(&mut mesh, &[]);
    // Face 8 could only split into a collinear face while face 9 was there, so
    // its removal comes 2nd although its index is lower.
    assert_eq!(
        fixes,
        [
            split([0, 1, 6], [1, 0, 4], [[6, 0, 4], [1, 6, 4]]),
            split([0, 6, 5], [6, 0, 4], [[5, 0, 4], [6, 5, 4]]),
        ]
    );
    let mut expected = chained_pyramid().faces;
    expected[4] = triangle([1, 6, 4]);
    expected[8] = triangle([5, 0, 4]);
    expected[9] = triangle([6, 5, 4]);
    assert_eq!(mesh.faces, expected);
}

/// Returns a tetrahedron with vertex 3 at the position of vertex 0: its faces 1
/// and 2 have zero area and 2 vertices at 1 position.
fn tetrahedron_with_coincident_vertices() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: [
            [1.0, 1.0, 1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, 1.0],
        ]
        .map(Vertex::new)
        .to_vec(),
        faces: triangles(&[[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]),
    }
}

#[test]
fn zero_area_faces_without_a_removal_stay() {
    let mut mesh = disjoint_union(&[
        flat_tetrahedron(),
        collinear_pair(),
        blocked_split_tetrahedron(),
        tetrahedron_with_coincident_vertices(),
    ]);
    let original = mesh.clone();
    let fixes = fix_collinear_faces(&mut mesh, &[0, 4, 5, 6, 13, 14]);
    assert_eq!(fixes, []);
    assert_eq!(mesh, original);
}

/// Returns 2 copies of the square fan pyramid that share only R (2) and S (3):
/// the 2nd copy has its own P (6), Q (7), X (8) and Y (9), at the positions of
/// those of the 1st.
fn fan_pyramids_sharing_r_and_s() -> IndexedMesh {
    let mut mesh = fan_pyramid(SQUARE_FAN_PYRAMID);
    let in_the_copy = [6, 7, 2, 3, 8, 9].map(VertexIndex::from);
    let copied_vertices = [0, 1, 4, 5].map(|v| mesh.vertices[v]);
    mesh.vertices.extend(copied_vertices);
    let copied_faces: Vec<IndexedTriangle> = mesh
        .faces
        .iter()
        .map(|face| IndexedTriangle {
            vertices: face.vertices.map(|v| in_the_copy[usize::from(v)]),
        })
        .collect();
    mesh.faces.extend(copied_faces);
    mesh
}

#[test]
fn a_split_that_would_repeat_the_edge_of_an_earlier_split_is_refused() {
    let mut mesh = fan_pyramids_sharing_r_and_s();
    // After the 1st split, R-S is an edge: the split of the 2nd copy would give
    // it 4 faces, so the collinear face 8 stays.
    let fixes = fix_collinear_faces(&mut mesh, &[8]);
    assert_eq!(fixes, [split([0, 1, 2], [1, 0, 3], [[2, 0, 3], [1, 2, 3]])]);
    let mut expected = fan_pyramids_sharing_r_and_s().faces;
    expected[0] = triangle([2, 0, 3]);
    expected[3] = triangle([1, 2, 3]);
    assert_eq!(mesh.faces, expected);
}

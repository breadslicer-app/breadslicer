/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::error::Error;

use crate::indexed_mesh::mesh::declared_facet_normals::{
    DeclaredFacetNormals, FacetCountMismatch, RestoreRefusal,
};
use crate::indexed_mesh::mesh::geometry_fingerprint::GeometryFingerprint;
use crate::indexed_mesh::mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{IndexedTriangle, Vertex};

// Bits the geometry never gives for these faces, which point along +Z: a normal
// along +X with a -0.0 component, and a zero vector.
const DECLARED: [[u32; 3]; 2] = [[0x3f80_0000, 0x8000_0000, 0], [0, 0, 0]];

fn square() -> IndexedMesh {
    let vertices = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
    ];
    IndexedMesh::new(
        None,
        vertices.into_iter().map(Vertex::new).collect(),
        [[0, 1, 2], [2, 1, 3]]
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect(),
    )
}

/// Returns `square()` with `DECLARED` captured into its metadata.
fn declared_square() -> IndexedMesh {
    let mut mesh = square();
    let declared =
        DeclaredFacetNormals::capture(&mesh, DECLARED.to_vec()).expect("1 normal per face");
    mesh.set_meta(Some(MeshMetadata {
        declared_facet_normals: Some(declared),
        ..Default::default()
    }));
    mesh
}

fn moved_square() -> IndexedMesh {
    let mut mesh = square();
    mesh.vertices[3] = Vertex::new([2.0, 1.0, 0.0]);
    mesh
}

#[test]
fn capture_fails_when_the_count_differs_from_the_face_count() {
    assert_eq!(
        DeclaredFacetNormals::capture(&square(), vec![DECLARED[0]]),
        Err(FacetCountMismatch {
            facets: 1,
            faces: 2
        }),
    );
}

#[test]
fn capture_binds_the_fingerprint_of_the_mesh() {
    let mesh = square();
    let declared =
        DeclaredFacetNormals::capture(&mesh, DECLARED.to_vec()).expect("1 normal per face");
    assert_eq!(declared.fingerprint(), mesh.geometry_fingerprint());
}

#[test]
fn declared_facet_normals_returns_the_captured_bits() {
    assert_eq!(
        declared_square().declared_facet_normals(),
        Some(&DECLARED[..])
    );
}

#[test]
fn declared_facet_normals_is_none_after_a_vertex_edit() {
    let mut mesh = declared_square();
    mesh.vertices[3] = Vertex::new([2.0, 1.0, 0.0]);
    assert_eq!(mesh.declared_facet_normals(), None);
}

#[test]
fn declared_facet_normals_is_none_without_metadata() {
    assert_eq!(square().declared_facet_normals(), None);
}

#[test]
fn declared_facet_normals_is_none_without_a_copy() {
    let mut mesh = square();
    mesh.set_meta(Some(MeshMetadata {
        name: Some("square".to_string()),
        ..Default::default()
    }));
    assert_eq!(mesh.declared_facet_normals(), None);
}

#[test]
fn restore_accepts_the_geometry_the_normals_were_stored_with() {
    let mut mesh = square();
    let stored = GeometryFingerprint::from_bytes(mesh.geometry_fingerprint().into());
    let restored = DeclaredFacetNormals::restore(&mesh, DECLARED.to_vec(), stored)
        .expect("same geometry, 1 normal per face");
    mesh.set_meta(Some(MeshMetadata {
        declared_facet_normals: Some(restored),
        ..Default::default()
    }));
    assert_eq!(mesh.declared_facet_normals(), Some(&DECLARED[..]));
}

#[test]
fn restore_refuses_a_changed_geometry() {
    assert_eq!(
        DeclaredFacetNormals::restore(
            &moved_square(),
            DECLARED.to_vec(),
            square().geometry_fingerprint()
        ),
        Err(RestoreRefusal::GeometryChanged),
    );
}

#[test]
fn restore_refuses_a_count_mismatch_when_the_geometry_matches() {
    let mesh = square();
    assert_eq!(
        DeclaredFacetNormals::restore(&mesh, vec![DECLARED[0]], mesh.geometry_fingerprint()),
        Err(RestoreRefusal::FacetCountMismatch(FacetCountMismatch {
            facets: 1,
            faces: 2
        })),
    );
}

#[test]
fn restore_checks_the_geometry_before_the_count() {
    assert_eq!(
        DeclaredFacetNormals::restore(
            &moved_square(),
            vec![DECLARED[0]],
            square().geometry_fingerprint()
        ),
        Err(RestoreRefusal::GeometryChanged),
    );
}

#[test]
fn debug_prints_the_facet_count_and_not_the_normals() {
    let mesh = square();
    let declared =
        DeclaredFacetNormals::capture(&mesh, DECLARED.to_vec()).expect("1 normal per face");
    assert_eq!(
        format!("{declared:?}"),
        format!(
            "DeclaredFacetNormals {{ facets: 2, fingerprint: {:?} }}",
            mesh.geometry_fingerprint()
        ),
    );
}

#[test]
fn geometry_fingerprint_is_the_same_with_and_without_a_copy() {
    assert_eq!(
        declared_square().geometry_fingerprint(),
        square().geometry_fingerprint()
    );
}

#[test]
fn facet_count_mismatch_displays_both_counts() {
    assert_eq!(
        FacetCountMismatch {
            facets: 1,
            faces: 2
        }
        .to_string(),
        "facet normal count 1 differs from face count 2",
    );
}

#[test]
fn restore_refusals_display_their_cause() {
    assert_eq!(
        RestoreRefusal::GeometryChanged.to_string(),
        "the mesh geometry differs from the geometry the facet normals were stored with",
    );
    assert_eq!(
        RestoreRefusal::FacetCountMismatch(FacetCountMismatch {
            facets: 1,
            faces: 2
        })
        .to_string(),
        "facet normal count 1 differs from face count 2",
    );
}

#[test]
fn only_the_count_refusal_has_a_source() {
    let mismatch = FacetCountMismatch {
        facets: 1,
        faces: 2,
    };
    let source = RestoreRefusal::FacetCountMismatch(mismatch)
        .source()
        .map(ToString::to_string);
    assert_eq!(
        source.as_deref(),
        Some("facet normal count 1 differs from face count 2")
    );
    assert!(RestoreRefusal::GeometryChanged.source().is_none());
}

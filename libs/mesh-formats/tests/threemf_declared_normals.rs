/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Our 3MF writers store the facet normals that a mesh's source file declared
//! in a package part bound to the mesh geometry, and the 3MF reader restores
//! them while the read mesh has that geometry.

use std::io::Cursor;

use sc_3mf::model::{ObjectPayload, parse_model, write_model};
use sc_3mf::names::MUST_PRESERVE_REL_TYPE;
use sc_3mf::package::{Package, PartName};
use sc_mesh_core::indexed_mesh::DeclaredFacetNormals;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};
use sc_mesh_formats::messages::LoadThreeMfWarning;
use sc_mesh_formats::obj::read_obj;
use sc_mesh_formats::threemf::{read_threemf, write_threemf};
use sc_scene::Scene;

const PART: &str = "/Metadata/declared-facet-normals/1.bin";

// Bit patterns that no geometry gives, so a written normal cannot pass for a
// computed one.
const CRAFTED: [[u32; 3]; 4] = [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]];

// SHA-384 of the canonical geometry encoding of `tetrahedron()`, computed with
// Python's hashlib outside the code under test.
const FINGERPRINT: [u8; 48] = [
    0xa5, 0xe7, 0xf7, 0xde, 0x85, 0x4f, 0xfe, 0x6c, 0x9d, 0x3b, 0xeb, 0x22, 0xa1, 0x79, 0x67, 0x89,
    0x41, 0x5a, 0x38, 0x50, 0xa6, 0x21, 0x19, 0x2e, 0xf0, 0x44, 0xc3, 0x93, 0x87, 0x68, 0xfb, 0x2d,
    0x5e, 0xa3, 0xae, 0xb0, 0x71, 0x7a, 0xa7, 0xa2, 0xad, 0x93, 0xc2, 0x2c, 0x68, 0xff, 0x83, 0x3f,
];

/// Returns a closed tetrahedron whose metadata holds `CRAFTED` as its declared
/// facet normals.
fn tetrahedron() -> IndexedMesh {
    let vertices = [
        [1.0, 1.0, 1.0],
        [1.0, -1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
    ];
    let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];
    with_declared(mesh(&vertices, &faces), CRAFTED.to_vec())
}

fn mesh(vertices: &[[f32; 3]], faces: &[[u32; 3]]) -> IndexedMesh {
    IndexedMesh::new(
        None,
        vertices.iter().copied().map(Vertex::new).collect(),
        faces
            .iter()
            .map(|&indices| {
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), indices)
            })
            .collect(),
    )
}

fn with_declared(mut mesh: IndexedMesh, normals: Vec<[u32; 3]>) -> IndexedMesh {
    let declared = DeclaredFacetNormals::capture(&mesh, normals).expect("1 normal per face");
    mesh.set_meta(Some(MeshMetadata {
        declared_facet_normals: Some(declared),
        ..Default::default()
    }));
    mesh
}

fn write(scene: &Scene) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_threemf(scene, &mut bytes).expect("the 3MF writes");
    bytes
}

/// Returns the mesh of the only leaf of the 3MF package `bytes`, and the load
/// warnings.
fn read(bytes: Vec<u8>) -> (IndexedMesh, Vec<LoadThreeMfWarning>) {
    let (scene, warnings) = read_threemf(&mut Cursor::new(bytes)).expect("the 3MF reads");
    let leaf = scene.first_leaf_id().expect("the scene has a leaf");
    let mesh = scene.mesh_at_leaf(leaf).expect("the leaf holds a mesh");
    (mesh.clone(), warnings)
}

fn reopen(bytes: Vec<u8>) -> Package {
    Package::read_from(Cursor::new(bytes)).expect("the package reads")
}

#[test]
fn the_package_holds_a_part_with_the_declared_normals_and_the_fingerprint() {
    let package = reopen(write(&Scene::from(tetrahedron())));
    let part = PartName::new(PART).expect("a valid part name");
    assert_eq!(
        package.part_content_type(&part),
        Some("application/vnd.soulcrafted.declared-facet-normals"),
    );
    let bytes = package.part_data(&part).expect("the part is present");
    assert_eq!(bytes.len(), 112);
    assert_eq!(bytes[0..4], *b"SCFN", "magic");
    assert_eq!(bytes[4..8], [1, 0, 0, 0], "version");
    assert_eq!(bytes[8..12], [1, 0, 0, 0], "object id");
    assert_eq!(bytes[12..16], [4, 0, 0, 0], "face count");
    assert_eq!(bytes[16..64], FINGERPRINT, "fingerprint");
    assert_eq!(
        bytes[64..],
        [
            1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 6, 0, 0, 0, 7, 0, 0, 0, 8,
            0, 0, 0, 9, 0, 0, 0, 10, 0, 0, 0, 11, 0, 0, 0, 12, 0, 0, 0,
        ],
        "normals",
    );
}

#[test]
fn the_part_is_linked_from_the_root_and_marked_must_preserve() {
    let package = reopen(write(&Scene::from(tetrahedron())));
    let links: Vec<(&str, &str)> = package
        .root_relationships()
        .iter()
        .filter(|relationship| relationship.target == PART)
        .map(|relationship| (relationship.id.as_str(), relationship.rel_type.as_str()))
        .collect();
    assert_eq!(
        links,
        [
            (
                "declared-facet-normals-1",
                "http://schemas.soulcrafted.it/slicer/2026/10/declared-facet-normals",
            ),
            ("declared-facet-normals-1-preserve", MUST_PRESERVE_REL_TYPE),
        ],
    );
}

#[test]
fn the_reader_restores_the_declared_normals() {
    let (mesh, warnings) = read(write(&Scene::from(tetrahedron())));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(mesh.declared_facet_normals(), Some(&CRAFTED[..]));
}

#[test]
fn a_negative_zero_a_subnormal_and_a_huge_coordinate_keep_the_declared_normals() {
    let vertices = [
        [-0.0, f32::from_bits(1), f32::MAX],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let source = with_declared(mesh(&vertices, &[[0, 1, 2]]), vec![CRAFTED[0]]);
    let (mesh, warnings) = read(write(&Scene::from(source)));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(mesh.declared_facet_normals(), Some(&CRAFTED[..1]));
}

#[test]
fn a_mesh_without_declared_normals_gets_no_part() {
    let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let (scene, _) = read_obj(&mut Cursor::new(obj.as_bytes())).expect("the OBJ reads");
    let package = reopen(write(&scene));
    assert!(package.part_names().all(|name| {
        !name
            .as_str()
            .starts_with("/Metadata/declared-facet-normals/")
    }),);
    assert!(
        package
            .root_relationships()
            .iter()
            .all(|relationship| !relationship.id.starts_with("declared-facet-normals")),
    );
}

/// Returns the package `bytes` with `data` as the data of `part`, which is
/// added when the package does not hold it.
fn with_part(bytes: Vec<u8>, part: &str, data: Vec<u8>) -> Vec<u8> {
    let source = reopen(bytes);
    let target = PartName::new(part).expect("a valid part name");
    let mut package = Package::new();
    for name in source.part_names() {
        let content_type = source.part_content_type(name).expect("a content type");
        let kept = source.part_data(name).expect("the part data").to_vec();
        let data = if *name == target { data.clone() } else { kept };
        package
            .add_part(name.clone(), content_type, data)
            .expect("a unique name");
        for relationship in source.part_relationships(name) {
            package.add_part_relationship(name, relationship.clone());
        }
    }
    if source.part_data(&target).is_none() {
        package
            .add_part(
                target,
                "application/vnd.soulcrafted.declared-facet-normals",
                data,
            )
            .expect("a unique name");
    }
    for relationship in source.root_relationships() {
        package.add_root_relationship(relationship.clone());
    }
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("the package writes");
    out.into_inner()
}

fn has_declared_normals(mesh: &IndexedMesh) -> bool {
    mesh.meta()
        .as_ref()
        .is_some_and(|meta| meta.declared_facet_normals.is_some())
}

#[test]
fn a_mesh_moved_after_the_write_drops_the_declared_normals() {
    let bytes = write(&Scene::from(tetrahedron()));
    let model = reopen(bytes.clone())
        .part_data(&PartName::new("/3D/3dmodel.model").expect("a valid part name"))
        .expect("the model part")
        .to_vec();
    let mut document = parse_model(&model).expect("the model parses");
    let ObjectPayload::Mesh(mesh) = &mut document.objects[0].payload else {
        panic!("the first object is the tetrahedron");
    };
    mesh.vertices[0].x += 1.0;
    let mut edited = Vec::new();
    write_model(&document, &mut edited).expect("the model writes");

    let (mesh, warnings) = read(with_part(bytes, "/3D/3dmodel.model", edited));
    assert_eq!(
        warnings,
        [LoadThreeMfWarning::DeclaredFacetNormalsMismatch { object_id: 1 }],
    );
    assert!(!has_declared_normals(&mesh));
}

#[test]
fn a_truncated_part_is_reported_and_the_load_goes_on() {
    let bytes = write(&Scene::from(tetrahedron()));
    let mut part = reopen(bytes.clone())
        .part_data(&PartName::new(PART).expect("a valid part name"))
        .expect("the part")
        .to_vec();
    part.pop();

    let (mesh, warnings) = read(with_part(bytes, PART, part));
    assert_eq!(
        warnings,
        [LoadThreeMfWarning::DeclaredFacetNormalsMalformed {
            part: PART.to_string(),
        }],
    );
    assert!(!has_declared_normals(&mesh));
}

#[test]
fn a_part_name_without_an_object_id_is_reported() {
    let bytes = write(&Scene::from(tetrahedron()));
    let part = reopen(bytes.clone())
        .part_data(&PartName::new(PART).expect("a valid part name"))
        .expect("the part")
        .to_vec();
    let stray = "/Metadata/declared-facet-normals/x.bin";

    let (mesh, warnings) = read(with_part(bytes, stray, part));
    assert_eq!(
        warnings,
        [LoadThreeMfWarning::DeclaredFacetNormalsMalformed {
            part: stray.to_string(),
        }],
    );
    assert_eq!(mesh.declared_facet_normals(), Some(&CRAFTED[..]));
}

#[test]
fn a_part_with_the_right_fingerprint_and_another_facet_count_is_reported() {
    let part = [
        b"SCFN".as_slice(),
        &[1, 0, 0, 0],
        &[1, 0, 0, 0],
        &[3, 0, 0, 0],
        &FINGERPRINT,
        &[0; 36],
    ]
    .concat();

    let (mesh, warnings) = read(with_part(write(&Scene::from(tetrahedron())), PART, part));
    assert_eq!(
        warnings,
        [LoadThreeMfWarning::DeclaredFacetNormalsMalformed {
            part: PART.to_string(),
        }],
    );
    assert!(!has_declared_normals(&mesh));
}

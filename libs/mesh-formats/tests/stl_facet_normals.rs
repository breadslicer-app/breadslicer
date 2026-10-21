/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The facet normals an STL file declares are written back as read while the
//! mesh is unchanged and placed by a translation, also after a round trip
//! through a 3MF package; otherwise the STL writers compute each facet normal
//! from the written geometry.

use std::f64::consts::FRAC_PI_2;
use std::io::Cursor;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_mesh_formats::ports::MeshWriter;
use sc_mesh_formats::stl::{StlAscii, StlBin, read_stla, read_stlb, write_stla, write_stlb};
use sc_mesh_formats::threemf::{read_threemf, write_threemf};
use sc_scene::Scene;
use sc_scene_fixtures::leaves_as_roots;

const BINARY_FIXTURE: &[u8] = include_bytes!("fixtures/tetrahedron_declared_normals.stl");
const ASCII_FIXTURE: &str = include_str!("fixtures/tetrahedron_declared_normals.stla");

// The bits of the normals both fixtures declare: a zero vector, a vector of
// length 2, a vector against the winding, and a vector with -0.0 components.
const DECLARED: [[u32; 3]; 4] = [
    [0, 0, 0],
    [0, 0, 0x4000_0000],
    [0, 0xbf80_0000, 0],
    [0x8000_0000, 0x8000_0000, 0xbf80_0000],
];

const VERTICES: [[f32; 3]; 4] = [
    [1.0, 1.0, 1.0],
    [1.0, -1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
];

const FACETS: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

// 1 / sqrt(3).
const S: f32 = 0.577_350_26;

// The outward normals of the fixture's facets after a rotation by pi/2 about Z,
// which maps (x, y, z) to (-y, x, z).
const ROTATED: [[f32; 3]; 4] = [[-S, S, -S], [S, S, S], [-S, -S, S], [S, -S, -S]];

fn read_binary_fixture() -> IndexedMesh {
    let (scene, _) = read_stlb(&mut Cursor::new(BINARY_FIXTURE)).expect("the fixture reads");
    only_mesh(&scene)
}

fn read_ascii_fixture() -> IndexedMesh {
    let scene = read_stla(&mut Cursor::new(ASCII_FIXTURE.as_bytes())).expect("the fixture reads");
    only_mesh(&scene)
}

fn only_mesh(scene: &Scene) -> IndexedMesh {
    let leaf = scene.first_leaf_id().expect("the scene has a leaf");
    scene
        .mesh_at_leaf(leaf)
        .expect("the leaf holds a mesh")
        .clone()
}

/// Returns the fixture's tetrahedron with no declared normals, as the 3MF and
/// OBJ readers build it.
fn tetrahedron_without_declared_normals() -> IndexedMesh {
    let vertices: Vec<Vertex> = VERTICES.into_iter().map(Vertex::new).collect();
    let faces = FACETS
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
    IndexedMesh::new(None, vertices, faces)
}

fn translation() -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation([10.0, -5.0, 2.0]))
}

fn rotation() -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_rotation_z(FRAC_PI_2))
}

fn write_binary(scene: &Scene) -> Vec<u8> {
    let mut bytes = Vec::new();
    StlBin::write_with_progress(scene, &mut bytes, |_| {}).expect("the scene writes");
    bytes
}

fn write_ascii(scene: &Scene) -> String {
    let mut bytes = Vec::new();
    StlAscii::write_with_progress(scene, &mut bytes, |_| {}).expect("the scene writes");
    String::from_utf8(bytes).expect("ASCII STL is UTF-8")
}

/// Returns the normal bits of each 50-byte record after the 84-byte preamble of
/// a binary STL.
fn binary_normals(bytes: &[u8]) -> Vec<[u32; 3]> {
    bytes[84..]
        .as_chunks::<50>()
        .0
        .iter()
        .map(|record| {
            [0, 4, 8].map(|at| u32::from_le_bytes(record[at..at + 4].try_into().unwrap()))
        })
        .collect()
}

/// Returns the bits of the values on each `facet normal` line of an ASCII STL.
fn ascii_normals(text: &str) -> Vec<[u32; 3]> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("facet normal "))
        .map(|values| {
            let values: Vec<f32> = values
                .split_whitespace()
                .map(|value| value.parse().expect("a number"))
                .collect();
            [values[0], values[1], values[2]].map(f32::to_bits)
        })
        .collect()
}

fn assert_close(actual: &[[u32; 3]], expected: &[[f32; 3]]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?}");
    for (bits, wanted) in actual.iter().zip(expected) {
        for (bits, wanted) in bits.iter().zip(wanted) {
            let value = f32::from_bits(*bits);
            assert!((value - wanted).abs() < 1e-6, "{value} != {wanted}");
        }
    }
}

#[test]
fn binary_write_back_keeps_the_declared_normals() {
    let mut bytes = Vec::new();
    write_stlb(&read_binary_fixture(), &mut bytes).expect("the mesh writes");
    assert_eq!(binary_normals(&bytes), DECLARED);
}

#[test]
fn ascii_write_back_keeps_the_declared_normals() {
    let mut bytes = Vec::new();
    write_stla(&read_ascii_fixture(), &mut bytes).expect("the mesh writes");
    assert_eq!(
        ascii_normals(&String::from_utf8(bytes).expect("UTF-8")),
        DECLARED
    );
}

#[test]
fn a_translated_scene_keeps_the_declared_normals() {
    let scene = leaves_as_roots([(read_binary_fixture(), translation())]);
    assert_eq!(binary_normals(&write_binary(&scene)), DECLARED);
    assert_eq!(ascii_normals(&write_ascii(&scene)), DECLARED);
}

#[test]
fn a_rotated_scene_gets_the_normals_of_the_rotated_geometry() {
    let scene = leaves_as_roots([(read_binary_fixture(), rotation())]);
    assert_close(&binary_normals(&write_binary(&scene)), &ROTATED);
    assert_close(&ascii_normals(&write_ascii(&scene)), &ROTATED);
}

#[test]
fn a_2_object_scene_resolves_the_normals_of_each_object_on_its_own() {
    let scene = leaves_as_roots([
        (read_binary_fixture(), translation()),
        (read_binary_fixture(), rotation()),
    ]);
    let normals = binary_normals(&write_binary(&scene));
    assert_eq!(normals[..4], DECLARED);
    assert_close(&normals[4..], &ROTATED);
}

#[test]
fn the_declared_normals_survive_a_3mf_round_trip() {
    let (scene, _) = read_stlb(&mut Cursor::new(BINARY_FIXTURE)).expect("the fixture reads");
    let mut package = Vec::new();
    write_threemf(&scene, &mut package).expect("the 3MF writes");
    let (scene, warnings) = read_threemf(&mut Cursor::new(package)).expect("the 3MF reads");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(binary_normals(&write_binary(&scene)), DECLARED);
}

#[test]
fn a_rotated_mesh_without_declared_normals_gets_its_rotated_normals() {
    let scene = leaves_as_roots([(tetrahedron_without_declared_normals(), rotation())]);
    assert_close(&binary_normals(&write_binary(&scene)), &ROTATED);
}

#[test]
fn a_collinear_face_without_declared_normals_is_written_with_the_zero_vector() {
    let collinear = IndexedMesh::new(
        None,
        vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([2.0, 0.0, 0.0]),
        ],
        vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    );
    let scene = Scene::from(collinear);
    assert_eq!(binary_normals(&write_binary(&scene)), [[0, 0, 0]]);
    assert!(
        write_ascii(&scene).contains("facet normal 0e0 0e0 0e0"),
        "{}",
        write_ascii(&scene)
    );
}

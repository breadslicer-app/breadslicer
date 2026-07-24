/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Integration tests against unmodified 3MF Consortium sample
//! packages (see tests/fixtures/official/README.md). These pin the
//! reader to files produced outside this codebase, so the test suite
//! cannot drift into checking a home-grown idea of the format.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use sc_3mf::model::{ModelDocument, ObjectPayload, Unit, parse_model, write_model};
use sc_3mf::names::MODEL_PART_CONTENT_TYPE;
use sc_3mf::package::Package;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("official")
        .join(name)
}

/// Open an official package, check the start part's content type, and
/// parse its model document.
fn load_official(name: &str) -> ModelDocument {
    let bytes = fs::read(fixture_path(name)).expect("fixture readable");
    let package = Package::read_from(Cursor::new(bytes)).expect("official package reads");
    let start = package.start_part().expect("start part resolves");
    assert_eq!(
        package.part_content_type(&start),
        Some(MODEL_PART_CONTENT_TYPE),
        "start part of {name} must carry the 3D Model content type",
    );
    let data = package.part_data(&start).expect("start part data");
    parse_model(data).expect("official model parses")
}

fn mesh_counts(document: &ModelDocument, object_index: usize) -> (usize, usize) {
    let ObjectPayload::Mesh(mesh) = &document.objects[object_index].payload else {
        panic!("object {object_index} does not carry a mesh");
    };
    (mesh.vertices.len(), mesh.triangles.len())
}

#[test]
fn box_sample_parses_with_known_geometry() {
    let document = load_official("box.3mf");
    assert_eq!(document.unit, Unit::Millimeter);
    assert_eq!(document.objects.len(), 1);
    assert_eq!(document.objects[0].id.get(), 1);
    assert_eq!(mesh_counts(&document, 0), (8, 12));
    assert_eq!(document.build.len(), 1);
    assert_eq!(document.build[0].object_id.get(), 1);
    assert!(document.build[0].transform.is_none());
    assert!(
        document
            .metadata
            .iter()
            .any(|m| m.name == "Copyright" && m.value.contains("3MF Consortium")),
    );
}

#[test]
fn cylinder_sample_parses_with_known_geometry() {
    let document = load_official("cylinder.3mf");
    assert_eq!(document.objects.len(), 1);
    assert_eq!(mesh_counts(&document, 0), (46, 88));
    assert_eq!(document.build.len(), 1);
}

#[test]
fn multiple_cylinders_sample_carries_six_placements_of_one_object() {
    let document = load_official("multiple_cylinders.3mf");
    assert_eq!(document.objects.len(), 1);
    assert_eq!(document.objects[0].id.get(), 2);
    assert_eq!(mesh_counts(&document, 0), (46, 88));
    assert_eq!(document.build.len(), 6);
    assert!(document.build.iter().all(|item| item.object_id.get() == 2));
    let with_transform = document
        .build
        .iter()
        .filter(|item| item.transform.is_some())
        .count();
    assert_eq!(with_transform, 5);
    let second = document.build[1].transform.expect("second item transform");
    assert_eq!(second.0[9], 21.0, "x translation of the second placement");
}

#[test]
fn torus_sample_parses_with_known_geometry() {
    let document = load_official("torus.3mf");
    assert_eq!(document.objects.len(), 1);
    assert_eq!(mesh_counts(&document, 0), (1100, 2200));
}

#[test]
fn every_official_sample_round_trips_through_the_writer() {
    for name in [
        "box.3mf",
        "cylinder.3mf",
        "multiple_cylinders.3mf",
        "torus.3mf",
    ] {
        let document = load_official(name);
        let mut bytes = Vec::new();
        write_model(&document, &mut bytes).expect("document writes");
        let reparsed = parse_model(&bytes).expect("rewritten document parses");
        assert_eq!(reparsed, document, "round trip mismatch for {name}");
    }
}

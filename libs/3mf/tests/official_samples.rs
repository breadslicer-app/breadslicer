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

use sc_3mf::model::{
    MeshResolution, ModelDocument, ObjectPayload, SliceStackContent, Unit, parse_model, write_model,
};
use sc_3mf::names::MODEL_PART_CONTENT_TYPE;
use sc_3mf::package::{Package, PartName};

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
fn box_sliced_sample_parses_root_part_with_sliceref_stack() {
    let document = load_official("box_sliced.3mf");
    assert_eq!(document.unit, Unit::Millimeter);
    assert_eq!(document.slice_stacks.len(), 1);
    let stack = &document.slice_stacks[0];
    assert_eq!(stack.id.get(), 1);
    assert_eq!(stack.z_bottom, 0.0);
    let SliceStackContent::Refs(refs) = &stack.content else {
        panic!("the root part's stack holds slicerefs");
    };
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slice_stack.get(), 1);
    assert!(
        refs[0].slice_path.starts_with("/2D/"),
        "{}",
        refs[0].slice_path
    );

    assert_eq!(document.objects.len(), 1);
    let object = &document.objects[0];
    assert_eq!(object.id.get(), 2);
    assert_eq!(object.slice_stack.map(|s| s.get()), Some(1));
    assert_eq!(object.mesh_resolution, MeshResolution::Fullres);
    assert_eq!(mesh_counts(&document, 0), (8, 12));

    // The item transform writes its planar cells as "0.0000" and
    // "1.0000"; parsing must accept them numerically.
    let transform = document.build[0].transform.expect("item transform");
    assert_eq!(transform.0[8], 1.0);
    assert_eq!(transform.0[9], 33.0327);
}

#[test]
fn box_sliced_sample_slice_part_parses_with_inline_slices() {
    let bytes = fs::read(fixture_path("box_sliced.3mf")).expect("fixture readable");
    let package = Package::read_from(Cursor::new(bytes)).expect("official package reads");
    let start = package.start_part().expect("start part resolves");
    let root =
        parse_model(package.part_data(&start).expect("start part data")).expect("root part parses");
    let SliceStackContent::Refs(refs) = &root.slice_stacks[0].content else {
        panic!("root stack holds slicerefs");
    };

    let slice_part_name =
        PartName::new(&refs[0].slice_path).expect("sliceref path is a valid part name");
    // The spec requires the slice part to be reachable through a
    // relationship of the referencing model part.
    assert!(
        package
            .part_relationships(&start)
            .iter()
            .any(|rel| rel.target == refs[0].slice_path),
        "slice part must be related to the root model part",
    );
    let slice_part = parse_model(
        package
            .part_data(&slice_part_name)
            .expect("slice part present"),
    )
    .expect("slice part parses");

    assert_eq!(slice_part.slice_stacks.len(), 1);
    let SliceStackContent::Slices(slices) = &slice_part.slice_stacks[0].content else {
        panic!("the slice part holds inline slices");
    };
    assert_eq!(slices.len(), 379);
    assert_eq!(slices[0].z_top, 0.08);
    assert_eq!(slices[0].vertices.len(), 4);
    assert_eq!(slices[0].polygons.len(), 1);
    assert!(slices[0].polygons[0].is_closed());
    let last = slices.last().expect("at least one slice");
    assert_eq!(last.z_top, 30.32);
    assert!(last.vertices.is_empty(), "final slice is a void layer");

    // The heavyweight part round-trips through the writer too.
    let mut bytes = Vec::new();
    write_model(&slice_part, &mut bytes).expect("slice part writes");
    let reparsed = parse_model(&bytes).expect("rewritten slice part parses");
    assert_eq!(reparsed, slice_part);
}

#[test]
fn every_official_sample_round_trips_through_the_writer() {
    for name in [
        "box.3mf",
        "cylinder.3mf",
        "multiple_cylinders.3mf",
        "torus.3mf",
        "box_sliced.3mf",
    ] {
        let document = load_official(name);
        let mut bytes = Vec::new();
        write_model(&document, &mut bytes).expect("document writes");
        let reparsed = parse_model(&bytes).expect("rewritten document parses");
        assert_eq!(reparsed, document, "round trip mismatch for {name}");
    }
}

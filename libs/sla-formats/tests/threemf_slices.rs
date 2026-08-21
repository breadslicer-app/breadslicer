/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end coverage of the 3MF slice-package writer: a scene
//! with the unit cube plus the Z-tunnel and Y-tunnel prisms is
//! sliced for real, written to `.slices.3mf`, and the package is
//! then inspected part by part with the sc-3mf parser. A final test
//! feeds the same file to the plain mesh loader to pin the
//! full-resolution fallback story (geometry survives, slice data is
//! reported as dropped).

use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use sc_3mf::model::{MeshResolution, ModelDocument, ObjectPayload, SliceStackContent, parse_model};
use sc_3mf::names::START_PART_REL_TYPE;
use sc_3mf::package::{Package, PartName};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::messages::LoadThreeMfWarning;
use sc_mesh_formats::threemf::read_threemf;
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, SceneMetadata};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::slice_scene;
use sc_sla_formats::{WriterOptions, write_scene_slices};
use tempfile::tempdir;

fn named(name: &str) -> Option<SceneMetadata> {
    Some(SceneMetadata {
        name: Some(name.to_string()),
        ..Default::default()
    })
}

fn add_root_leaf(scene: &mut Scene, name: &str, mesh: sc_mesh_core::IndexedMesh, x: f64, z: f64) {
    let transform = Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, z]));
    let leaf = scene
        .add_leaf(named(name), transform, LeafPayload::Mesh(Arc::new(mesh)))
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
}

/// Cube at x = -5 (z in [0, 1]), Z-tunnel prism at the origin and
/// Y-tunnel prism at x = +5 (both z in [0, 2] already).
fn prism_trio_scene() -> Scene {
    let mut scene = Scene::new();
    add_root_leaf(&mut scene, "cube", unit_cube_mesh(), -5.0, 0.5);
    add_root_leaf(&mut scene, "z-tunnel", cuboid_with_z_tunnel(), 0.0, 0.0);
    add_root_leaf(&mut scene, "y-tunnel", cuboid_with_y_tunnel(), 5.0, 0.0);
    scene
}

/// 4 uniform 0.5 mm slabs: tops at 0.5, 1.0, 1.5, 2.0; centred
/// sampling planes at 0.25, 0.75, 1.25, 1.75.
fn four_layer_profile() -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(0.5, 0.5, 4).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

/// Slices the trio scene and writes the package to `out`.
fn write_trio_package(out: &Path) {
    let scene = prism_trio_scene();
    let profile = four_layer_profile();
    let slices = slice_scene(&scene, &profile).expect("trio scene slices cleanly");
    let options = WriterOptions::from_profile(&profile, "0.0.0-test");
    write_scene_slices(&scene, &slices, out, &options).expect("package writes cleanly");
}

fn read_package(path: &Path) -> Package {
    let bytes = std::fs::read(path).expect("written package is readable");
    Package::read_from(Cursor::new(bytes)).expect("written package parses")
}

fn model_part(package: &Package, name: &str) -> ModelDocument {
    let part = PartName::new(name).expect("valid part name");
    let data = package.part_data(&part).expect("part is present");
    parse_model(data).expect("part parses as a model document")
}

#[test]
fn root_part_carries_one_sliced_fullres_object_per_leaf() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("trio.slices.3mf");
    write_trio_package(&out);

    let package = read_package(&out);
    let start = package.start_part().expect("start part resolves");
    assert_eq!(start.as_str(), "/3D/3dmodel.model");

    let root = model_part(&package, "/3D/3dmodel.model");
    assert_eq!(root.slice_stacks.len(), 3);
    assert_eq!(root.objects.len(), 3);
    assert_eq!(root.build.len(), 3);

    for (index, (stack, object)) in root
        .slice_stacks
        .iter()
        .zip(root.objects.iter())
        .enumerate()
    {
        let ordinal = index + 1;
        assert_eq!(stack.z_bottom, 0.0);
        match &stack.content {
            SliceStackContent::Refs(refs) => {
                assert_eq!(refs.len(), 1);
                assert_eq!(refs[0].slice_path, format!("/2D/slices-{ordinal}.model"));
                assert_eq!(refs[0].slice_stack.get(), 1);
            }
            SliceStackContent::Slices(_) => {
                panic!("root stacks must reference /2D/ parts, found inline slices")
            }
        }
        assert_eq!(object.slice_stack, Some(stack.id));
        assert_eq!(object.mesh_resolution, MeshResolution::Fullres);
    }

    let names: Vec<Option<&str>> = root.objects.iter().map(|o| o.name.as_deref()).collect();
    assert_eq!(
        names,
        vec![Some("cube"), Some("z-tunnel"), Some("y-tunnel")],
    );

    for item in &root.build {
        assert!(
            item.transform.is_none(),
            "meshes are baked to world space, so items carry no transform",
        );
    }
}

#[test]
fn embedded_meshes_are_baked_to_world_space() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("trio.slices.3mf");
    write_trio_package(&out);

    let package = read_package(&out);
    let root = model_part(&package, "/3D/3dmodel.model");

    let cube = &root.objects[0];
    let ObjectPayload::Mesh(mesh) = &cube.payload else {
        panic!("leaf objects embed meshes");
    };
    assert_eq!(mesh.vertices.len(), 8);
    for vertex in &mesh.vertices {
        assert!(
            (-5.5..=-4.5).contains(&vertex.x),
            "cube x must sit around -5, got {}",
            vertex.x,
        );
        assert!(
            (0.0..=1.0).contains(&vertex.z),
            "cube z must sit on the plate, got {}",
            vertex.z,
        );
    }

    let ObjectPayload::Mesh(tunnel) = &root.objects[1].payload else {
        panic!("leaf objects embed meshes");
    };
    assert_eq!(tunnel.vertices.len(), 16);
    assert_eq!(tunnel.triangles.len(), 32);
}

#[test]
fn each_leaf_gets_its_own_2d_part_with_inline_slices_and_empty_build() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("trio.slices.3mf");
    write_trio_package(&out);

    let package = read_package(&out);
    let root_part = PartName::new("/3D/3dmodel.model").unwrap();
    let rels = package.part_relationships(&root_part);
    assert_eq!(rels.len(), 3);
    for (index, rel) in rels.iter().enumerate() {
        assert_eq!(rel.rel_type, START_PART_REL_TYPE);
        assert_eq!(rel.target, format!("/2D/slices-{}.model", index + 1));
    }

    for ordinal in 1..=3 {
        let part = model_part(&package, &format!("/2D/slices-{ordinal}.model"));
        assert!(part.objects.is_empty(), "slice parts hold no objects");
        assert!(part.build.is_empty(), "slice parts hold an empty build");
        assert_eq!(part.slice_stacks.len(), 1);
        let stack = &part.slice_stacks[0];
        assert_eq!(stack.id.get(), 1);
        assert_eq!(stack.z_bottom, 0.0);
        let SliceStackContent::Slices(slices) = &stack.content else {
            panic!("slice parts hold inline slices, not refs");
        };
        assert_eq!(slices.len(), 4);
        let tops: Vec<f64> = slices.iter().map(|s| s.z_top).collect();
        assert_eq!(tops, vec![0.5, 1.0, 1.5, 2.0]);
        for slice in slices {
            for polygon in &slice.polygons {
                assert!(polygon.is_closed(), "model-object polygons must close");
            }
        }
    }
}

#[test]
fn tunnel_sections_keep_their_shape_per_layer() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("trio.slices.3mf");
    write_trio_package(&out);

    let package = read_package(&out);

    // Cube (leaf 1): top at z = 1.0 covers the first 2 slabs only.
    let cube = model_part(&package, "/2D/slices-1.model");
    let SliceStackContent::Slices(slices) = &cube.slice_stacks[0].content else {
        panic!("inline slices expected");
    };
    let polygon_counts: Vec<usize> = slices.iter().map(|s| s.polygons.len()).collect();
    assert_eq!(polygon_counts, vec![1, 1, 0, 0]);

    // Z-tunnel (leaf 2): every layer is an annulus, outer + hole.
    let z_tunnel = model_part(&package, "/2D/slices-2.model");
    let SliceStackContent::Slices(slices) = &z_tunnel.slice_stacks[0].content else {
        panic!("inline slices expected");
    };
    let polygon_counts: Vec<usize> = slices.iter().map(|s| s.polygons.len()).collect();
    assert_eq!(polygon_counts, vec![2, 2, 2, 2]);

    // Y-tunnel (leaf 3): the tunnel spans z in [0.6, 1.4], so the
    // sampling planes 0.75 and 1.25 cut it into two disjoint solids
    // while 0.25 and 1.75 see the full square.
    let y_tunnel = model_part(&package, "/2D/slices-3.model");
    let SliceStackContent::Slices(slices) = &y_tunnel.slice_stacks[0].content else {
        panic!("inline slices expected");
    };
    let polygon_counts: Vec<usize> = slices.iter().map(|s| s.polygons.len()).collect();
    assert_eq!(polygon_counts, vec![1, 2, 2, 1]);
}

#[test]
fn root_part_recommends_but_does_not_require_the_slice_extension() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("trio.slices.3mf");
    write_trio_package(&out);

    let package = read_package(&out);
    let root_part = PartName::new("/3D/3dmodel.model").unwrap();
    let xml = package.part_data(&root_part).expect("root part present");
    let xml = std::str::from_utf8(xml).expect("model XML is UTF-8");
    assert!(
        xml.contains("recommendedextensions=\"s\""),
        "fullres slice content is recommended, not required",
    );
    assert!(
        !xml.contains("requiredextensions"),
        "fullres packages must stay loadable by core-only consumers",
    );
}

#[test]
fn package_loads_as_plain_meshes_with_a_slice_data_warning() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("trio.slices.3mf");
    write_trio_package(&out);

    let mut file = File::open(&out).expect("written package opens");
    let (scene, warnings) = read_threemf(&mut file).expect("package loads as a mesh scene");

    let meshes = scene
        .merge_meshes()
        .expect("scene merges")
        .expect("geometry");
    assert_eq!(meshes.vertices.len(), 8 + 16 + 16);
    assert_eq!(meshes.faces.len(), 12 + 32 + 32);

    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, LoadThreeMfWarning::SliceDataIgnored { stack_count: 3 })),
        "the loader must report the 3 dropped slice stacks, got {warnings:?}",
    );
}

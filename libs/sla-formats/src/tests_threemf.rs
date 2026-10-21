/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the 3MF slice-package writer: input validation,
//! void-layer emission, ztop placement, and ring closure/winding.
//! Structural assertions over full multi-model packages live in the
//! `threemf_slices` integration test.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use sc_3mf::model::{ModelDocument, Slice, SlicePolygon, SliceStackContent, parse_model};
use sc_3mf::package::{Package, PartName};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_z_tunnel, unit_cube_mesh};
use sc_print_primitives::{AaLevel, BuildVolume, Mirror, PixelResolution};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};
use sc_sl1cer::{LeafSlices, slice_scene};
use tempfile::tempdir;

use crate::error::WriterError;
use crate::options::WriterOptions;
use crate::threemf::write_scene_slices;

fn profile(layer_height: f64, first_layer_height: f64, total_layers: u32) -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(layer_height, first_layer_height, total_layers).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

fn options(profile: &SliceProfile) -> WriterOptions {
    WriterOptions::from_profile(profile, "0.0.0-test")
}

/// A single-leaf scene holding `mesh` under `transform`, rooted
/// directly (no wrapping group).
fn single_leaf_scene(mesh: IndexedMesh, transform: Transform3D) -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
    scene
}

/// The origin-centred unit cube lifted onto the plate (z in [0, 1]).
fn cube_scene() -> Scene {
    single_leaf_scene(
        unit_cube_mesh(),
        Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 0.5])),
    )
}

fn sliced(scene: &Scene, profile: &SliceProfile) -> Vec<LeafSlices> {
    slice_scene(scene, profile).expect("fixture scenes slice cleanly")
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

/// The inline slices of the single stack in a `/2D/` part.
fn inline_slices(document: &ModelDocument) -> &[Slice] {
    assert_eq!(document.slice_stacks.len(), 1);
    match &document.slice_stacks[0].content {
        SliceStackContent::Slices(slices) => slices,
        SliceStackContent::Refs(_) => panic!("a /2D/ part must hold inline slices, found refs"),
    }
}

/// Ring points in chain order, closing vertex dropped.
fn ring_points(slice: &Slice, polygon: &SlicePolygon) -> Vec<(f64, f64)> {
    let mut indices = vec![polygon.start.get()];
    indices.extend(polygon.segments.iter().map(|s| s.end.get()));
    assert_eq!(
        indices.last(),
        indices.first(),
        "polygon must close back onto its start vertex",
    );
    indices.pop();
    indices
        .into_iter()
        .map(|i| {
            let v = &slice.vertices[i as usize];
            (v.x, v.y)
        })
        .collect()
}

/// Shoelace signed area: positive for counter-clockwise rings.
fn signed_area(points: &[(f64, f64)]) -> f64 {
    let mut doubled = 0.0;
    for (i, &(x1, y1)) in points.iter().enumerate() {
        let (x2, y2) = points[(i + 1) % points.len()];
        doubled += x1 * y2 - x2 * y1;
    }
    doubled / 2.0
}

mod input_validation_tests {
    use super::*;

    #[test]
    fn empty_slices_list_is_refused() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.slices.3mf");
        let err = write_scene_slices(&Scene::new(), &[], &out, &options(&profile(0.25, 0.25, 4)))
            .unwrap_err();
        assert!(matches!(err, WriterError::NoModelsToWrite));
        assert!(!out.exists());
    }

    #[test]
    fn stack_not_matching_the_schedule_grid_is_refused() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.slices.3mf");
        let scene = cube_scene();
        let slices = sliced(&scene, &profile(0.25, 0.25, 4));
        let mismatched = options(&profile(0.25, 0.25, 6));
        let err = write_scene_slices(&scene, &slices, &out, &mismatched).unwrap_err();
        match err {
            WriterError::LayerGridMismatch {
                schedule_layers,
                stack_layers,
                ..
            } => {
                assert_eq!(schedule_layers, 6);
                assert_eq!(stack_layers, 4);
            }
            other => panic!("expected LayerGridMismatch, got {other:?}"),
        }
        assert!(!out.exists());
    }

    #[test]
    fn existing_directory_as_output_is_refused() {
        let dir = tempdir().unwrap();
        let scene = cube_scene();
        let prof = profile(0.25, 0.25, 4);
        let slices = sliced(&scene, &prof);
        let err = write_scene_slices(&scene, &slices, dir.path(), &options(&prof)).unwrap_err();
        assert!(matches!(err, WriterError::OutputPathInvalid { .. }));
    }
}

mod emission_tests {
    use super::*;

    #[test]
    fn short_model_gets_trailing_void_slices() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.slices.3mf");
        // Cube top at z = 1.0; the schedule spans up to 2.0, so the
        // upper 4 slabs hold no geometry.
        let scene = cube_scene();
        let prof = profile(0.25, 0.25, 8);
        write_scene_slices(&scene, &sliced(&scene, &prof), &out, &options(&prof)).unwrap();

        let package = read_package(&out);
        let part = model_part(&package, "/2D/slices-1.model");
        let slices = inline_slices(&part);
        assert_eq!(slices.len(), 8);
        for slice in &slices[..4] {
            assert!(!slice.polygons.is_empty(), "cube layers carry contours");
        }
        for slice in &slices[4..] {
            assert!(slice.polygons.is_empty(), "layers above the cube are void");
            assert!(slice.vertices.is_empty(), "void slices carry no vertices");
        }
        assert!((slices[7].z_top - 2.0).abs() < 1e-9);
    }

    #[test]
    fn ztop_ladder_follows_the_slab_tops_not_the_centred_planes() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.slices.3mf");
        let scene = cube_scene();
        // Distinct first-layer height: slab tops at 0.5, 0.75, 1.0.
        let prof = profile(0.25, 0.5, 3);
        write_scene_slices(&scene, &sliced(&scene, &prof), &out, &options(&prof)).unwrap();

        let package = read_package(&out);
        let part = model_part(&package, "/2D/slices-1.model");
        let slices = inline_slices(&part);
        let tops: Vec<f64> = slices.iter().map(|s| s.z_top).collect();
        let expected = [0.5, 0.75, 1.0];
        assert_eq!(tops.len(), expected.len());
        for (got, want) in tops.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
        }
        // The centred sampling plane of slab 0 is 0.25; it must NOT
        // be what the file records.
        assert!(tops[0] > 0.25 + 1e-9);
    }

    #[test]
    fn rings_close_explicitly_and_keep_their_winding() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.slices.3mf");
        // The z-tunnel prism already sits on the plate (z in [0, 2])
        // and every layer crosses the tunnel: outer ring + hole.
        let scene = single_leaf_scene(cuboid_with_z_tunnel(), Transform3D::Identity);
        let prof = profile(0.5, 0.5, 4);
        write_scene_slices(&scene, &sliced(&scene, &prof), &out, &options(&prof)).unwrap();

        let package = read_package(&out);
        let part = model_part(&package, "/2D/slices-1.model");
        for slice in inline_slices(&part) {
            assert_eq!(slice.polygons.len(), 2, "outer boundary plus tunnel hole");
            for polygon in &slice.polygons {
                assert!(polygon.is_closed());
            }
            let areas: Vec<f64> = slice
                .polygons
                .iter()
                .map(|p| signed_area(&ring_points(slice, p)))
                .collect();
            let outer = areas.iter().cloned().fold(f64::MIN, f64::max);
            let hole = areas.iter().cloned().fold(f64::MAX, f64::min);
            assert!(outer > 0.0, "outer ring must stay counter-clockwise");
            assert!(hole < 0.0, "hole ring must stay clockwise");
            // Mesh vertices are f32, so section coordinates carry
            // f32 quantization; compare with a matching tolerance.
            assert!(
                (outer - 4.0).abs() < 1e-6,
                "outer is the 2x2 box section, got {outer}"
            );
            assert!(
                (hole + 0.64).abs() < 1e-6,
                "hole is the 0.8x0.8 tunnel, got {hole}"
            );
        }
    }
}

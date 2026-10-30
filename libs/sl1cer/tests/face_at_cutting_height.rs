/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! A horizontal face exactly at a cutting height is on the plane at any
//! height (#226): the layer cut there is empty, as at the bottom of the
//! plate. Vertex heights are f32, so at 100.275 mm the top face lands
//! 1.5e-6 mm above the plane, past the old fixed 1e-6 mm band, and printed
//! one more layer than the same box near the plate.

use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::slice_leaf;

const LAYER_MM: f64 = 0.05;

/// A closed 1 mm square box from `bottom` to `top`, wound outward.
fn box_between(bottom: f32, top: f32) -> IndexedMesh {
    let vertices = vec![
        Vertex::new([0.0, 0.0, bottom]),
        Vertex::new([1.0, 0.0, bottom]),
        Vertex::new([1.0, 1.0, bottom]),
        Vertex::new([0.0, 1.0, bottom]),
        Vertex::new([0.0, 0.0, top]),
        Vertex::new([1.0, 0.0, top]),
        Vertex::new([1.0, 1.0, top]),
        Vertex::new([0.0, 1.0, top]),
    ];
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            f([0, 3, 2]),
            f([0, 2, 1]),
            f([4, 5, 6]),
            f([4, 6, 7]),
            f([3, 7, 6]),
            f([3, 6, 2]),
            f([0, 1, 5]),
            f([0, 5, 4]),
            f([1, 2, 6]),
            f([1, 6, 5]),
            f([0, 4, 7]),
            f([0, 7, 3]),
        ],
    }
}

/// The number of polygons on each of the two layers around `top`, a box's
/// top face at the centre of a 50 µm layer: the layer below it and the
/// layer cut at it.
fn polygons_around_top(top: f64) -> [usize; 2] {
    // Layer k is cut at (k + 0.5) * LAYER_MM.
    let at_top = (top / LAYER_MM - 0.5).round() as usize;
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(box_between((top - 0.2) as f32, top as f32))),
        )
        .expect("add_leaf accepts an inline mesh");
    scene.add_root(leaf).expect("add_root accepts the leaf");
    let profile = SliceProfile::new(
        BuildVolume::new(10.0, 10.0, 200.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(LAYER_MM, LAYER_MM, at_top as u32 + 2).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    );
    let stack = slice_leaf(&scene, leaf, &profile).expect("the box slices cleanly");
    let layers = stack.layers();
    assert!((layers[at_top].z_mm() - top).abs() < 1e-9);
    [
        layers[at_top - 1].polygons().len(),
        layers[at_top].polygons().len(),
    ]
}

#[test]
fn a_top_face_at_a_cutting_height_is_on_the_plane_near_the_plate() {
    assert_eq!(polygons_around_top(0.275), [1, 0]);
}

#[test]
fn a_top_face_at_a_cutting_height_is_on_the_plane_high_up() {
    // The f32 top really lands outside the old band.
    assert!(f64::from(100.275_f32) - 100.275 > 1e-6);
    assert_eq!(polygons_around_top(100.275), [1, 0]);
    assert_eq!(polygons_around_top(150.025), [1, 0]);
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The geometric edge cases of the slicing research (#21), each against
//! the reference raster of [`crate::overlapping_parts`]: oriented segments
//! and the non-zero rule, which knows nothing of rings. Meshes are made
//! here; the research checked the same cases on real files against UVtools.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::slice_leaf;

use crate::overlapping_parts::{
    LAYER_MM, PLATE_MM, PLATE_PX, Welded, assert_matches_reference, slice,
};

/// The pixels whose centres lie within `[lo, hi)` along one axis.
fn pixels_within(lo: f64, hi: f64) -> usize {
    let pixel_mm = PLATE_MM / f64::from(PLATE_PX);
    (0..PLATE_PX)
        .map(|i| (f64::from(i) + 0.5) * pixel_mm - 0.5 * PLATE_MM)
        .filter(|c| *c >= lo && *c < hi)
        .count()
}

/// A sealed void with a solid island inside it: three nested shells.
/// Each layer through all three lights the ring and the island, not the
/// void between them.
#[test]
fn nested_shells_light_the_ring_and_the_island() {
    let mut mesh = Welded::default();
    mesh.cuboid([-8.0, -8.0, 0.0], [8.0, 8.0, 1.0]);
    let mut void = Welded::default();
    void.cuboid([-5.0, -5.0, 0.2], [5.0, 5.0, 0.8]);
    let void = void.turned_inside_out();
    let mut island = Welded::default();
    island.cuboid([-2.0, -2.0, 0.3], [2.0, 2.0, 0.7]);
    for part in [void, island] {
        for face in part.faces {
            let face = face.map(|i| mesh.vertex(part.vertices[i as usize].map(f64::from)));
            mesh.faces.push(face);
        }
    }
    let mesh = mesh.mesh();
    let lit = assert_matches_reference(&mesh, 20);
    let stack = slice(&mesh, 20);
    let side = |half: f64| pixels_within(-half, half);
    for (layer, lit) in stack.layers().iter().zip(lit) {
        let z = layer.z_mm();
        let mut want = side(8.0).pow(2);
        if z > 0.2 && z < 0.8 {
            want -= side(5.0).pow(2);
        }
        if z > 0.3 && z < 0.7 {
            want += side(2.0).pow(2);
        }
        assert_eq!(lit, want, "at z {z}");
    }
}

/// An octahedron whose apex and equator lie exactly on cutting planes
/// (the middle of layers: 0.025 mm and on, a layer apart), and a step
/// whose ledge and top do: vertices and faces on a plane slice as the
/// reference does (a vertex on the plane counts as just below it). Sizes
/// keep every edge off the pixel centres, where a tie would decide.
#[test]
fn vertices_and_faces_on_the_cutting_plane() {
    let mut octahedron = Welded::default();
    let (top, bottom) = (
        octahedron.vertex([0.0, 0.0, 0.975]),
        octahedron.vertex([0.0, 0.0, 0.025]),
    );
    let equator: Vec<u32> = [[3.97, 0.0], [0.0, 3.97], [-3.97, 0.0], [0.0, -3.97]]
        .iter()
        .map(|&[x, y]| octahedron.vertex([x, y, 0.475]))
        .collect();
    for k in 0..4 {
        let (a, b) = (equator[k], equator[(k + 1) % 4]);
        octahedron.faces.push([a, b, top]);
        octahedron.faces.push([b, a, bottom]);
    }
    assert_matches_reference(&octahedron.mesh(), 20);

    // A step: a square to 0.475 mm, then half of it up to 0.975 mm, both
    // flats on cutting planes.
    let mut step = Welded::default();
    step.prism(
        &[
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 5.0],
            [0.0, 5.0],
            [0.0, 0.0],
            [-5.0, 0.0],
        ],
        &[
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 5.0],
            [0.0, 5.0],
            [0.0, 0.0],
            [-5.0, 0.0],
        ],
        &[[0, 1, 4], [1, 2, 4], [2, 3, 4], [0, 4, 5]],
        [0.0, 0.475],
    );
    step.cuboid([0.0, -5.0, 0.475], [5.0, 5.0, 0.975]);
    let step = step.mesh();
    let lit = assert_matches_reference(&step, 20);
    // Every plane through the step cuts it; the one on its top face cuts
    // nothing above it: that layer is empty, and the print schedule leaves
    // it out (#226).
    let stack = slice(&step, 20);
    for (layer, lit) in stack.layers().iter().zip(lit) {
        let on_top = (layer.z_mm() - 0.975).abs() < 1e-6;
        assert_eq!(lit == 0, on_top, "z {}: {lit} pixels", layer.z_mm());
    }
}

/// A box that runs off the plate: what lies beyond the edge is not lit,
/// what lies on it is, as the reference clips.
#[test]
fn a_model_off_the_plate_edge_lights_what_lies_on_it() {
    let mut mesh = Welded::default();
    mesh.cuboid([8.0, -3.0, 0.0], [16.0, 3.0, 0.5]);
    let lit = assert_matches_reference(&mesh.mesh(), 10);
    let want = pixels_within(8.0, 12.0) * pixels_within(-3.0, 3.0);
    assert!(lit.iter().all(|&n| n == want), "{lit:?} against {want}");
}

fn slice_with(mesh: &IndexedMesh, schedule: LayerSchedule) -> sc_sl1cer::contour::ContourStack {
    let profile = SliceProfile::new(
        BuildVolume::new(PLATE_MM, PLATE_MM, 10.0).unwrap(),
        PixelResolution::new(PLATE_PX, PLATE_PX).unwrap(),
        schedule,
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    );
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh.clone())),
        )
        .unwrap();
    scene.add_root(leaf).unwrap();
    slice_leaf(&scene, leaf, &profile).expect("the mesh slices")
}

/// A thicker first layer: the first plane sits at its height, the next
/// ones a layer apart, and a model shorter than it still prints one layer.
#[test]
fn the_first_layer_takes_its_own_height() {
    let mut mesh = Welded::default();
    mesh.cuboid([-3.0, -3.0, 0.0], [3.0, 3.0, 0.5]);
    let mesh = mesh.mesh();
    let schedule = LayerSchedule::uniform(LAYER_MM, 0.2, 7).unwrap();
    // Each plane is the middle of its layer.
    let heights: Vec<f64> = schedule.iter_z().collect();
    assert!((heights[0] - 0.1).abs() < 1e-9, "{heights:?}");
    assert!(
        (heights[1] - (0.2 + LAYER_MM / 2.0)).abs() < 1e-9,
        "{heights:?}"
    );
    for pair in heights[1..].windows(2) {
        assert!((pair[1] - pair[0] - LAYER_MM).abs() < 1e-9, "{heights:?}");
    }
    let stack = slice_with(&mesh, schedule);
    assert_eq!(stack.layers().len(), 7);
    assert!(
        stack
            .layers()
            .iter()
            .all(|layer| !layer.polygons().is_empty())
    );

    let mut thin = Welded::default();
    thin.cuboid([-3.0, -3.0, 0.0], [3.0, 3.0, 0.1]);
    let stack = slice_with(
        &thin.mesh(),
        LayerSchedule::uniform(LAYER_MM, 0.2, 1).unwrap(),
    );
    assert_eq!(stack.layers().len(), 1);
}

/// The same mesh slices to the same contours on one thread and on eight.
#[test]
fn slicing_is_the_same_on_one_thread_and_on_many() {
    let mut mesh = Welded::default();
    mesh.cuboid([-8.0, -8.0, 0.0], [8.0, 8.0, 2.0]);
    let mut void = Welded::default();
    void.cuboid([-5.0, -5.0, 0.4], [5.0, 5.0, 1.6]);
    let void = void.turned_inside_out();
    for face in void.faces {
        let face = face.map(|i| mesh.vertex(void.vertices[i as usize].map(f64::from)));
        mesh.faces.push(face);
    }
    let mesh = mesh.mesh();
    let on = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| slice(&mesh, 40))
    };
    let one = on(1);
    assert_eq!(one, on(8));
    assert_eq!(one, on(3));
}

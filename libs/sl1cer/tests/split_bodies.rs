/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Split into bodies (#257) leaves the print as it was: a mesh sliced as
//! one leaf and its bodies sliced as one leaf each light the same pixels on
//! every layer, with and without anti-aliasing.

use std::sync::Arc;

use sc_mesh_core::shells::Bodies;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::contour::{ContourStack, merge_stacks};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, PositiveFiniteMm,
    SliceProfile,
};
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;
use sc_sl1cer::slice_leaf;

const PLATE_MM: f64 = 24.0;
/// 0.024 mm pixels: no corner here lands on a pixel centre.
const PLATE_PX: u32 = 1000;
const LAYER_MM: f64 = 0.05;
const LAYERS: u32 = 260;

/// The 12 faces of a box over its corners, `x` in bit 0, `y` in bit 1 and
/// `z` in bit 2 of the corner index, facing outward.
const BOX_FACES: [[u32; 3]; 12] = [
    [0, 2, 1],
    [1, 2, 3],
    [4, 5, 6],
    [5, 7, 6],
    [0, 1, 4],
    [1, 5, 4],
    [2, 6, 3],
    [3, 6, 7],
    [0, 4, 2],
    [2, 4, 6],
    [1, 3, 5],
    [3, 7, 5],
];

#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    faces: Vec<IndexedTriangle>,
}

impl Builder {
    /// A box from `min` to `max`, facing inward (a void) when `inward`.
    fn cube(mut self, min: [f32; 3], max: [f32; 3], inward: bool) -> Self {
        let base = self.vertices.len() as u32;
        for k in 0..8 {
            let pick = |axis: usize| {
                if k & (1 << axis) != 0 {
                    max[axis]
                } else {
                    min[axis]
                }
            };
            self.vertices.push(Vertex::new([pick(0), pick(1), pick(2)]));
        }
        for [a, b, c] in BOX_FACES {
            let face = if inward { [a, c, b] } else { [a, b, c] };
            self.faces
                .push(IndexedTriangle::from_raw_indices(face.map(|v| v + base)));
        }
        self
    }

    fn build(self) -> IndexedMesh {
        IndexedMesh::new(None, self.vertices, self.faces)
    }
}

fn profile(aa: AaLevel) -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(PLATE_MM, PLATE_MM, 20.0).unwrap(),
        PixelResolution::new(PLATE_PX, PLATE_PX).unwrap(),
        LayerSchedule::uniform(LAYER_MM, LAYER_MM, LAYERS).unwrap(),
        Mirror::default(),
        aa,
        PackingMode::None,
    )
}

fn pixel_map() -> PixelMap {
    let side = PositiveFiniteMm::new(PLATE_MM).unwrap();
    PixelMap::new(
        side,
        side,
        PixelResolution::new(PLATE_PX, PLATE_PX).unwrap(),
        Mirror::default(),
    )
}

/// The leaves of `meshes` sliced, one leaf each, merged as a plate is.
fn slice(meshes: Vec<IndexedMesh>, profile: &SliceProfile) -> ContourStack {
    let mut scene = Scene::new();
    let mut stacks = Vec::new();
    let mut leaves = Vec::new();
    for mesh in meshes {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        scene.add_root(leaf).unwrap();
        leaves.push(leaf);
    }
    for leaf in leaves {
        stacks.push(slice_leaf(&scene, leaf, profile).expect("the leaf slices"));
    }
    merge_stacks(stacks).expect("same schedule")
}

/// A hollow box with a ball sealed in its cavity, a box running into it
/// without sharing a vertex, and a box apart.
fn plate() -> IndexedMesh {
    Builder::default()
        .cube([-9.3, -8.7, 0.0], [3.1, 2.9, 11.3], false)
        .cube([-7.1, -6.3, 2.2], [0.9, 0.7, 9.1], true)
        .cube([-4.4, -3.9, 4.3], [-1.6, -1.1, 6.7], false)
        .cube([1.7, -4.1, 0.0], [8.3, 6.6, 12.4], false)
        .cube([4.6, -10.2, 0.0], [10.1, -6.3, 7.7], false)
        .build()
}

#[test]
fn the_bodies_slice_as_the_mesh_did() {
    let mesh = plate();
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 3);
    let pixel_map = pixel_map();
    for aa in [AaLevel::Off, AaLevel::X4] {
        let profile = profile(aa);
        let whole = slice(vec![mesh.clone()], &profile);
        let split = slice(bodies.meshes(&mesh), &profile);
        assert_eq!(whole.layers().len(), split.layers().len());
        let mut lit = 0;
        for (a, b) in whole.layers().iter().zip(split.layers()) {
            let a = rasterize_layer(a, &pixel_map, aa, 0.0);
            let b = rasterize_layer(b, &pixel_map, aa, 0.0);
            assert_eq!(a.runs(), b.runs());
            lit += usize::from(a.runs().len() > 1);
        }
        assert!(lit > 200, "the plate is on most layers");
    }
}

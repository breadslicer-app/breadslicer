/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end tests of [`slice_leaf`] against curved analytical
//! meshes: a UV sphere and a Z-axis cylinder. The polygon area at
//! each layer is compared to the analytic cross-section area within
//! a tessellation-dependent tolerance.
//!
//! Cap-Z layers (Z exactly on a cap's plane) are not exercised here
//! because the per-triangle intersection treats coplanar triangles
//! as empty. Lifting that policy belongs to a follow-on plan.

use std::f64::consts::PI;
use std::sync::Arc;

use sc_mesh_core::scene::node::{LeafPayload, NodeId};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Scene, Vertex};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::slice_leaf;

/// Longitude segments per ring on the sphere and cylinder. The
/// circle-vs-inscribed-polygon area error scales as
/// (2 pi / N)^2 / 6 = pi^2 * 2 / (3 N^2) for a regular N-gon, so
/// N = 64 keeps the longitude contribution to the area error below
/// ~0.16% on both meshes.
const N_LON: u32 = 64;

/// Latitude bands on the UV sphere. Picked odd so no vertex ring
/// lands at the equator Z plane: an even count would put a vertex
/// ring exactly at z = 1 mm (with the equator at z = 1), and the
/// plane-vertex-on-plane case currently feeds back as `Empty` from
/// the triangle intersector.
const N_LAT: u32 = 33;

/// Relative area tolerance for the per-layer cross-section check.
/// The longitude contribution is ~0.16% (see [`N_LON`]); the
/// latitude contribution near the sphere's equator is bounded by
/// the band width (pi / N_LAT)^2 / 8 ~= 0.11%. Real per-band slice
/// area depends on where in the band the slicing Z falls; 2% leaves
/// comfortable headroom for f32 vertex storage and accumulated FP
/// noise.
const AREA_REL_TOLERANCE: f64 = 0.02;

/// Generate a UV sphere of `radius` centred at `(cx, cy, cz)` with
/// `n_lat` latitude bands (poles included) and `n_lon` longitude
/// segments. Outward-facing winding, manifold by construction.
fn uv_sphere(cx: f64, cy: f64, cz: f64, radius: f64, n_lat: u32, n_lon: u32) -> IndexedMesh {
    assert!(n_lat >= 2 && n_lon >= 3);
    let mut vertices = Vec::with_capacity(((n_lat - 1) * n_lon + 2) as usize);
    // 0: north pole
    vertices.push(Vertex::new([cx as f32, cy as f32, (cz + radius) as f32]));
    // 1..=(n_lat - 1) * n_lon: intermediate latitude rings (k = 1..n_lat - 1)
    for k in 1..n_lat {
        let theta = PI * (k as f64) / (n_lat as f64);
        let sin_t = theta.sin();
        let cos_t = theta.cos();
        let z = cz + radius * cos_t;
        let r_at_z = radius * sin_t;
        for j in 0..n_lon {
            let phi = 2.0 * PI * (j as f64) / (n_lon as f64);
            let x = cx + r_at_z * phi.cos();
            let y = cy + r_at_z * phi.sin();
            vertices.push(Vertex::new([x as f32, y as f32, z as f32]));
        }
    }
    // Last vertex: south pole
    let south_pole = vertices.len() as u32;
    vertices.push(Vertex::new([cx as f32, cy as f32, (cz - radius) as f32]));

    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f =
        |abc: [u32; 3]| -> IndexedTriangle { IndexedTriangle::from_raw_indices(placeholder, abc) };
    let mut faces = Vec::new();

    // Top cap: north pole (0) to first intermediate ring.
    for j in 0..n_lon {
        let next_j = (j + 1) % n_lon;
        let a = 1 + j;
        let b = 1 + next_j;
        faces.push(f([0, a, b]));
    }
    // Middle bands between consecutive intermediate rings.
    for k in 1..(n_lat - 1) {
        let ring_a_start = 1 + (k - 1) * n_lon;
        let ring_b_start = 1 + k * n_lon;
        for j in 0..n_lon {
            let next_j = (j + 1) % n_lon;
            let a = ring_a_start + j;
            let a_next = ring_a_start + next_j;
            let b = ring_b_start + j;
            let b_next = ring_b_start + next_j;
            faces.push(f([a, b, b_next]));
            faces.push(f([a, b_next, a_next]));
        }
    }
    // Bottom cap: last intermediate ring to south pole.
    let last_ring_start = 1 + (n_lat - 2) * n_lon;
    for j in 0..n_lon {
        let next_j = (j + 1) % n_lon;
        let a = last_ring_start + j;
        let b = last_ring_start + next_j;
        faces.push(f([south_pole, b, a]));
    }

    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// Generate a closed Z-axis cylinder of `radius` and `height` with
/// `n_lon` longitude segments. Base at `z = 0`, top at `z = height`.
/// Outward-facing winding, manifold by construction.
fn cylinder_z(radius: f64, height: f64, n_lon: u32) -> IndexedMesh {
    assert!(n_lon >= 3);
    let mut vertices = Vec::with_capacity((2 * n_lon + 2) as usize);
    // Bottom rim: indices 0..n_lon
    for j in 0..n_lon {
        let phi = 2.0 * PI * (j as f64) / (n_lon as f64);
        vertices.push(Vertex::new([
            (radius * phi.cos()) as f32,
            (radius * phi.sin()) as f32,
            0.0,
        ]));
    }
    // Top rim: indices n_lon..2*n_lon
    for j in 0..n_lon {
        let phi = 2.0 * PI * (j as f64) / (n_lon as f64);
        vertices.push(Vertex::new([
            (radius * phi.cos()) as f32,
            (radius * phi.sin()) as f32,
            height as f32,
        ]));
    }
    let bot_centre = 2 * n_lon;
    vertices.push(Vertex::new([0.0, 0.0, 0.0]));
    let top_centre = 2 * n_lon + 1;
    vertices.push(Vertex::new([0.0, 0.0, height as f32]));

    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f =
        |abc: [u32; 3]| -> IndexedTriangle { IndexedTriangle::from_raw_indices(placeholder, abc) };
    let mut faces = Vec::new();

    // Side wall.
    for j in 0..n_lon {
        let next_j = (j + 1) % n_lon;
        let bl = j;
        let br = next_j;
        let tl = n_lon + j;
        let tr = n_lon + next_j;
        faces.push(f([bl, br, tr]));
        faces.push(f([bl, tr, tl]));
    }
    // Bottom cap (outward normal -Z).
    for j in 0..n_lon {
        let next_j = (j + 1) % n_lon;
        let bl = j;
        let br = next_j;
        faces.push(f([bot_centre, br, bl]));
    }
    // Top cap (outward normal +Z).
    for j in 0..n_lon {
        let next_j = (j + 1) % n_lon;
        let tl = n_lon + j;
        let tr = n_lon + next_j;
        faces.push(f([top_centre, tl, tr]));
    }

    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

fn scene_with_mesh_root(mesh: IndexedMesh) -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let leaf_id = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("add_leaf accepts an inline mesh");
    scene
        .add_root(leaf_id)
        .expect("add_root accepts a freshly added leaf");
    (scene, leaf_id)
}

fn make_profile(layer_schedule: LayerSchedule) -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(10.0, 10.0, 10.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        layer_schedule,
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

#[test]
fn sphere_equator_layer_area_approximates_pi() {
    // Unit sphere centred at (0, 0, 1), so z spans [0, 2] with the
    // equator at z = 1. Single-slab schedule h0 = 2.0 puts the only
    // centred Z at h0 / 2 = 1.0.
    let mesh = uv_sphere(0.0, 0.0, 1.0, 1.0, N_LAT, N_LON);
    let (scene, leaf_id) = scene_with_mesh_root(mesh);
    let profile = make_profile(LayerSchedule::uniform(0.5, 2.0, 1).unwrap());
    let stack = slice_leaf(&scene, leaf_id, &profile).expect("sphere slices cleanly");

    assert_eq!(stack.layers().len(), 1);
    let layer = &stack.layers()[0];
    assert!(
        (layer.z_mm() - 1.0).abs() < 1e-12,
        "equator layer z = {}, expected 1.0",
        layer.z_mm(),
    );
    assert_eq!(layer.polygons().len(), 1);
    let polygon = &layer.polygons()[0];
    assert!(polygon.holes().is_empty());
    let area = polygon.outer().signed_area();
    let expected = PI;
    let rel_err = (area - expected).abs() / expected;
    assert!(
        rel_err <= AREA_REL_TOLERANCE,
        "equator polygon area = {area}, expected ~{expected}, rel_err = {rel_err}",
    );
}

#[test]
fn cylinder_every_interior_layer_area_approximates_pi_r_squared() {
    // r = 1 mm, h = 2 mm cylinder along Z, base at z = 0, top at
    // z = 2. Schedule h0 = h = 0.5, n = 4 yields centred Z values
    // [0.25, 0.75, 1.25, 1.75], all strictly inside (0, 2).
    let r = 1.0;
    let h = 2.0;
    let mesh = cylinder_z(r, h, N_LON);
    let (scene, leaf_id) = scene_with_mesh_root(mesh);
    let profile = make_profile(LayerSchedule::uniform(0.5, 0.5, 4).unwrap());
    let stack = slice_leaf(&scene, leaf_id, &profile).expect("cylinder slices cleanly");

    let expected = PI * r * r;
    assert_eq!(stack.layers().len(), 4);
    for (i, layer) in stack.layers().iter().enumerate() {
        assert_eq!(
            layer.polygons().len(),
            1,
            "cylinder layer {i} (z = {z}) must hold one polygon",
            z = layer.z_mm(),
        );
        let polygon = &layer.polygons()[0];
        assert!(
            polygon.holes().is_empty(),
            "cylinder layer {i} must have no holes",
        );
        let area = polygon.outer().signed_area();
        assert!(
            area > 0.0,
            "cylinder layer {i} outer ring must be CCW (area > 0), got {area}",
        );
        let rel_err = (area - expected).abs() / expected;
        assert!(
            rel_err <= AREA_REL_TOLERANCE,
            "cylinder layer {i} area = {area}, expected ~{expected}, rel_err = {rel_err}",
        );
    }
}

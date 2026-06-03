/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end test of [`slice_leaf`] against a torus in two
//! orientations:
//!
//! - Horizontal-axis torus (symmetry axis along Y): a horizontal
//!   slice cuts through both tubes and produces two disjoint
//!   polygons, neither carrying holes.
//! - Vertical-axis torus (symmetry axis along Z): a horizontal slice
//!   through the equatorial plane produces one polygon with one
//!   hole. The outer ring is CCW and the hole is CW, matching the
//!   invariant `Polygon::new` checks for.
//!
//! Both meshes shift the ring or tube parametric angle by half a
//! step so no vertex lands exactly on the slicing plane. That keeps
//! the per-triangle intersection out of the conservative
//! through-vertex AOB case (`TriangleSlice::Empty`).

use std::f64::consts::PI;
use std::sync::Arc;

use sc_mesh_core::scene::node::{LeafPayload, NodeId};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Scene, Vertex};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::slice_leaf;

/// Ring tessellation (segments around the symmetry axis).
const N_RING: u32 = 32;
/// Tube tessellation (segments around the tube cross-section).
const N_TUBE: u32 = 32;

/// Major / tube radii used by both fixtures.
const MAJOR_R: f64 = 3.0;
const MINOR_R: f64 = 1.0;

/// Relative tolerance on cross-section area. Inscribed-polygon area
/// error for an N-gon approximating a circle is (2 pi / N)^2 / 6;
/// with N = 32 that's ~2.6% on the tube cross-section. The vertical
/// torus' outer-and-hole geometry approximates a circle of much
/// higher count (the N_RING-side polygon), so its error is well
/// under 1%, but we share a single tolerance for both tests.
const AREA_REL_TOLERANCE: f64 = 0.05;

/// Torus with symmetry axis along Y, ring centred on the circle
/// `x^2 + z^2 = major_r^2` in the y = 0 plane, shifted so its centre
/// sits at `(0, 0, z_centre)`. Outward winding, manifold by
/// construction. The half-step offset on the ring parameter `phi`
/// keeps every vertex strictly off `z = z_centre`.
fn horizontal_axis_torus(
    major_r: f64,
    minor_r: f64,
    z_centre: f64,
    n_ring: u32,
    n_tube: u32,
) -> IndexedMesh {
    let offset_phi = PI / (n_ring as f64);
    let offset_theta = PI / (n_tube as f64);
    let mut vertices = Vec::with_capacity((n_ring * n_tube) as usize);
    for j in 0..n_ring {
        let phi = 2.0 * PI * (j as f64) / (n_ring as f64) + offset_phi;
        let (cos_phi, sin_phi) = (phi.cos(), phi.sin());
        for k in 0..n_tube {
            let theta = 2.0 * PI * (k as f64) / (n_tube as f64) + offset_theta;
            let (cos_t, sin_t) = (theta.cos(), theta.sin());
            let r_at_theta = major_r + minor_r * cos_t;
            let x = r_at_theta * cos_phi;
            let y = minor_r * sin_t;
            let z = r_at_theta * sin_phi + z_centre;
            vertices.push(Vertex::new([x as f32, y as f32, z as f32]));
        }
    }
    torus_faces(n_ring, n_tube, vertices)
}

/// Torus with symmetry axis along Z, ring centred on the circle
/// `x^2 + y^2 = major_r^2` in the z = z_centre plane. Outward
/// winding, manifold by construction. The half-step offset on the
/// tube parameter `theta` keeps every vertex strictly off
/// `z = z_centre`.
fn vertical_axis_torus(
    major_r: f64,
    minor_r: f64,
    z_centre: f64,
    n_ring: u32,
    n_tube: u32,
) -> IndexedMesh {
    let offset_theta = PI / (n_tube as f64);
    let mut vertices = Vec::with_capacity((n_ring * n_tube) as usize);
    for j in 0..n_ring {
        let phi = 2.0 * PI * (j as f64) / (n_ring as f64);
        let (cos_phi, sin_phi) = (phi.cos(), phi.sin());
        for k in 0..n_tube {
            let theta = 2.0 * PI * (k as f64) / (n_tube as f64) + offset_theta;
            let (cos_t, sin_t) = (theta.cos(), theta.sin());
            let r_at_theta = major_r + minor_r * cos_t;
            let x = r_at_theta * cos_phi;
            let y = r_at_theta * sin_phi;
            let z = minor_r * sin_t + z_centre;
            vertices.push(Vertex::new([x as f32, y as f32, z as f32]));
        }
    }
    torus_faces(n_ring, n_tube, vertices)
}

fn torus_faces(n_ring: u32, n_tube: u32, vertices: Vec<Vertex>) -> IndexedMesh {
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f =
        |abc: [u32; 3]| -> IndexedTriangle { IndexedTriangle::from_raw_indices(placeholder, abc) };
    let idx = |j: u32, k: u32| -> u32 { (j % n_ring) * n_tube + (k % n_tube) };
    let mut faces = Vec::with_capacity((2 * n_ring * n_tube) as usize);
    for j in 0..n_ring {
        for k in 0..n_tube {
            let v00 = idx(j, k);
            let v10 = idx(j + 1, k);
            let v11 = idx(j + 1, k + 1);
            let v01 = idx(j, k + 1);
            faces.push(f([v00, v10, v11]));
            faces.push(f([v00, v11, v01]));
        }
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
        BuildVolume::new(20.0, 20.0, 20.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        layer_schedule,
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

#[test]
fn horizontal_axis_torus_through_tube_centres_yields_two_disjoint_polygons() {
    // Torus centred at (0, 0, 1). The slicing plane at z = 1 hits
    // the two tube cross-sections (centred at (+R, 0, 1) and
    // (-R, 0, 1)), so the layer must contain two polygons with no
    // holes.
    let z_centre = 1.0;
    let mesh = horizontal_axis_torus(MAJOR_R, MINOR_R, z_centre, N_RING, N_TUBE);
    let (scene, leaf_id) = scene_with_mesh_root(mesh);
    let profile = make_profile(LayerSchedule::uniform(0.5, 2.0, 1).unwrap());
    let stack = slice_leaf(&scene, leaf_id, &profile).expect("torus slices cleanly");

    assert_eq!(stack.layers().len(), 1);
    let layer = &stack.layers()[0];
    assert!(
        (layer.z_mm() - z_centre).abs() < 1e-12,
        "expected single layer at z = {z_centre}, got {}",
        layer.z_mm(),
    );

    assert_eq!(
        layer.polygons().len(),
        2,
        "horizontal-axis torus must split into two disjoint polygons",
    );
    for polygon in layer.polygons() {
        assert!(polygon.holes().is_empty());
        assert!(polygon.outer().signed_area() > 0.0);
    }

    let total_area: f64 = layer
        .polygons()
        .iter()
        .map(|p| p.outer().signed_area())
        .sum();
    let expected = 2.0 * PI * MINOR_R * MINOR_R;
    let rel_err = (total_area - expected).abs() / expected;
    assert!(
        rel_err <= AREA_REL_TOLERANCE,
        "total area = {total_area}, expected ~{expected}, rel_err = {rel_err}",
    );

    // Each polygon's representative point (vertex centroid) should
    // sit near one of (+R, 0) or (-R, 0). Sort by signed x of the
    // centroid so the assertion is order-independent.
    let mut centroid_xs: Vec<f64> = layer
        .polygons()
        .iter()
        .map(|p| {
            let vs = p.outer().vertices();
            let sx: f64 = vs.iter().map(|v| v.x).sum();
            sx / (vs.len() as f64)
        })
        .collect();
    centroid_xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(
        (centroid_xs[0] - (-MAJOR_R)).abs() < 0.2,
        "left polygon centroid x = {}, expected ~{}",
        centroid_xs[0],
        -MAJOR_R,
    );
    assert!(
        (centroid_xs[1] - MAJOR_R).abs() < 0.2,
        "right polygon centroid x = {}, expected ~{}",
        centroid_xs[1],
        MAJOR_R,
    );
}

#[test]
fn vertical_axis_torus_at_equator_yields_one_polygon_with_one_hole() {
    // Torus centred at (0, 0, 1). The slicing plane at z = 1 cuts
    // through the equator of every tube, producing an annular
    // cross-section: outer ring at radius ~ R + r, inner ring at
    // ~ R - r. After group_into_polygons the inner ring lands as a
    // CW hole inside a CCW outer.
    let z_centre = 1.0;
    let mesh = vertical_axis_torus(MAJOR_R, MINOR_R, z_centre, N_RING, N_TUBE);
    let (scene, leaf_id) = scene_with_mesh_root(mesh);
    let profile = make_profile(LayerSchedule::uniform(0.5, 2.0, 1).unwrap());
    let stack = slice_leaf(&scene, leaf_id, &profile).expect("torus slices cleanly");

    assert_eq!(stack.layers().len(), 1);
    let layer = &stack.layers()[0];

    assert_eq!(
        layer.polygons().len(),
        1,
        "vertical-axis torus at equator must collapse to one polygon",
    );
    let polygon = &layer.polygons()[0];
    assert_eq!(
        polygon.holes().len(),
        1,
        "annular cross-section must carry exactly one hole",
    );
    assert!(
        polygon.outer().signed_area() > 0.0,
        "outer ring must be CCW",
    );
    assert!(
        polygon.holes()[0].signed_area() < 0.0,
        "hole ring must be CW",
    );

    // Net polygon area: outer minus inner ~ pi * ((R + r)^2 - (R - r)^2)
    //                                       = 4 * pi * R * r.
    let net_area = polygon.outer().signed_area() + polygon.holes()[0].signed_area();
    let expected = 4.0 * PI * MAJOR_R * MINOR_R;
    let rel_err = (net_area - expected).abs() / expected;
    assert!(
        rel_err <= AREA_REL_TOLERANCE,
        "annulus net area = {net_area}, expected ~{expected}, rel_err = {rel_err}",
    );
}

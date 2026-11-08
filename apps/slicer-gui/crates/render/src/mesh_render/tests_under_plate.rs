/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The view from under the plate (#481), through the real shaders: the tests
//! draw a workspace frame from under the plate and from above it, read the
//! middle pixel back and tell whether the plate, a raft or what stands above
//! them shows. They soft-skip without an adapter, as the rest of the GPU tests
//! do.

use std::sync::Arc;

use sc_geom::df_supports::DF_SUPPORT_MESH_NAME;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::MeshMetadata;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::node::LeafPayload;

use super::*;
use crate::build_volume::NO_PRINTER_BUILD_VOLUME;
use crate::test_support::gpu_devices::shared_test_device;
use crate::under_plate::under_plate_fade;

const BV: BuildVolume = NO_PRINTER_BUILD_VOLUME;
const SIZE: u32 = 64;
/// The rafts' top the tests hand the renderer: the default raft's plate and
/// wall.
const RAFT_TOP_MM: f32 = 0.85;

fn renderer() -> Option<Renderer> {
    let (device, queue) = shared_test_device()?;
    Some(
        Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
            .expect("Renderer::new under the test device"),
    )
}

/// A square `side` mm wide, flat at height `z`, seen from both sides.
fn square(side: f32, z: f32) -> IndexedMesh {
    let h = side / 2.0;
    let vertices = [[-h, -h, z], [h, -h, z], [h, h, z], [-h, h, z]]
        .map(Vertex::new)
        .to_vec();
    let faces = [[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]]
        .map(IndexedTriangle::from_raw_indices)
        .to_vec();
    IndexedMesh::new(None, vertices, faces)
}

fn supports(mut mesh: IndexedMesh) -> IndexedMesh {
    mesh.set_meta(Some(MeshMetadata {
        name: Some(DF_SUPPORT_MESH_NAME.to_owned()),
        header_comment: None,
        declared_facet_normals: None,
    }));
    mesh
}

fn scene_of(meshes: Vec<IndexedMesh>) -> Scene {
    let mut graph = Scene::new();
    let root = graph.add_group(None, Transform3D::Identity);
    graph.add_root(root).expect("group root");
    for mesh in meshes {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::identity()),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        graph.add_child(root, leaf).expect("attach leaf");
    }
    graph
}

/// A camera 100 mm from a point 10 mm over the middle of the plate, `pitch_deg`
/// above (or, negative, below) it.
fn camera(pitch_deg: f32) -> OrbitCamera {
    let target = Vec3::new(0.0, 0.0, 10.0);
    let mut camera = OrbitCamera::new(target, 100.0);
    camera.target = target;
    camera.distance = 100.0;
    camera.pitch = pitch_deg.to_radians();
    camera.yaw = 0.3;
    camera
}

fn under() -> OrbitCamera {
    let camera = camera(-60.0);
    assert_eq!(under_plate_fade(&camera), 1.0);
    camera
}

fn above() -> OrbitCamera {
    let camera = camera(60.0);
    assert_eq!(under_plate_fade(&camera), 0.0);
    camera
}

/// The workspace frame's middle pixel, the rafts reaching `raft_top_mm`.
fn middle(
    renderer: &mut Renderer,
    graph: &Scene,
    camera: &OrbitCamera,
    raft_top_mm: f32,
) -> [u8; 3] {
    renderer.upload_scene(graph).expect("upload the scene");
    renderer.set_raft_top(raft_top_mm);
    let frame = renderer
        .workspace_pixels_for_test(
            SceneRender {
                graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            },
            BV,
            camera,
            SIZE,
            SIZE,
        )
        .expect("the frame is not refused")
        .expect("a sized frame");
    let i = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

/// Whether a pixel is the empty background: nothing drawn there.
fn is_background(pixel: [u8; 3]) -> bool {
    let clear = [
        frame::CLEAR_COLOR.r,
        frame::CLEAR_COLOR.g,
        frame::CLEAR_COLOR.b,
    ]
    .map(|c| (c * 255.0).round() as i32);
    pixel
        .iter()
        .zip(clear)
        .all(|(p, c)| (i32::from(*p) - c).abs() <= 2)
}

#[test]
fn from_under_the_plate_the_raft_and_the_plate_fade_out() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph = scene_of(vec![supports(square(60.0, 0.4))]);
    let hidden = middle(&mut renderer, &graph, &under(), RAFT_TOP_MM);
    assert!(is_background(hidden), "{hidden:?}");
    // With no raft to hide, a support mesh shows from under the plate.
    let shown = middle(&mut renderer, &graph, &under(), 0.0);
    assert!(!is_background(shown), "{shown:?}");
}

#[test]
fn from_above_the_plate_the_raft_shows() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph = scene_of(vec![supports(square(60.0, 0.4))]);
    let empty = scene_of(Vec::new());
    let raft = middle(&mut renderer, &graph, &above(), RAFT_TOP_MM);
    let plate = middle(&mut renderer, &empty, &above(), RAFT_TOP_MM);
    assert!(!is_background(raft), "{raft:?}");
    assert_ne!(raft, plate, "the raft, not the plate under it");
}

#[test]
fn from_under_the_plate_what_stands_above_the_raft_shows() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    for mesh in [supports(square(60.0, 5.0)), square(60.0, 5.0)] {
        let graph = scene_of(vec![mesh]);
        let pixel = middle(&mut renderer, &graph, &under(), RAFT_TOP_MM);
        assert!(!is_background(pixel), "{pixel:?}");
    }
    // A model is never taken for a raft, low as it stands.
    let graph = scene_of(vec![square(60.0, 0.4)]);
    let model = middle(&mut renderer, &graph, &under(), RAFT_TOP_MM);
    assert!(!is_background(model), "{model:?}");
}

#[test]
fn the_plate_fades_in_step_with_the_camera() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let empty = scene_of(Vec::new());
    // Just under the plate: the eye 2 mm below it, the fade barely begun.
    let just_under = camera((-0.12f32).asin().to_degrees());
    let fade = under_plate_fade(&just_under);
    assert!(fade > 0.0 && fade < 0.5, "{fade}");
    let faint = middle(&mut renderer, &empty, &just_under, 0.0);
    assert!(!is_background(faint), "the plate still shows: {faint:?}");
    let gone = middle(&mut renderer, &empty, &under(), 0.0);
    assert!(is_background(gone), "{gone:?}");
    let floor = middle(&mut renderer, &empty, &above(), 0.0);
    assert!(!is_background(floor), "{floor:?}");
}

/// A closed box from `lo` to `hi` (world mm), its faces turned outwards.
fn closed_box(lo: [f32; 3], hi: [f32; 3]) -> IndexedMesh {
    let cube = sc_mesh_fixtures::unit_cube_mesh();
    let (clo, chi) = cube
        .vertices()
        .iter()
        .fold(([f32::MAX; 3], [f32::MIN; 3]), |(a, b), v| {
            let p = v.as_array();
            (
                std::array::from_fn(|k| a[k].min(p[k])),
                std::array::from_fn(|k| b[k].max(p[k])),
            )
        });
    let vertices = cube
        .vertices()
        .iter()
        .map(|v| {
            let p = v.as_array();
            Vertex::new(std::array::from_fn(|k| {
                lo[k] + (p[k] - clo[k]) / (chi[k] - clo[k]) * (hi[k] - lo[k])
            }))
        })
        .collect();
    IndexedMesh::new(None, vertices, cube.faces().to_vec())
}

/// A shaft standing in the raft, opened from under the plate where the raft
/// faded, reads solid: its inside is drawn as the face of the cut, in the
/// shade its own underside takes drawn whole, not the background through it.
#[test]
fn from_under_the_plate_a_shaft_the_raft_opened_reads_solid() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph = scene_of(vec![supports(closed_box(
        [-10.0, -10.0, 0.0],
        [10.0, 10.0, 20.0],
    ))]);
    let cut = middle(&mut renderer, &graph, &under(), RAFT_TOP_MM);
    assert!(!is_background(cut), "{cut:?}");
    let whole = middle(&mut renderer, &graph, &under(), 0.0);
    assert!(
        cut.iter()
            .zip(whole)
            .all(|(c, w)| (i32::from(*c) - i32::from(w)).abs() <= 16),
        "the cut {cut:?}, the underside {whole:?}"
    );
    // From above, the shaft is as ever.
    let above_box = middle(&mut renderer, &graph, &above(), RAFT_TOP_MM);
    assert!(!is_background(above_box), "{above_box:?}");
}

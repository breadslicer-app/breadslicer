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

/// Zoomed in, an orthographic view looking up steeply had its eye just under
/// the plate, the fade barely begun, and drew most of a raft's pixels: the
/// scattered marks Damian saw. Its rays decide now, and the raft is gone.
#[test]
fn an_orthographic_view_from_under_the_plate_hides_the_raft_zoomed_in() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph = scene_of(vec![supports(square(60.0, 0.4))]);
    let mut close = camera(-40.0);
    close.distance = 16.0;
    close.orthographic = true;
    let eye = close.eye().z;
    assert!(eye < 0.0 && eye > -0.05 * close.distance, "{eye}");
    let pixel = middle(&mut renderer, &graph, &close, RAFT_TOP_MM);
    assert!(is_background(pixel), "{pixel:?}");
}

/// A raft's wall tops out at the rafts' top to the last bit: its top face,
/// drawn by the cut pass from under the plate, left marks along each raft's
/// rim. The cut stands a hair over the rafts' top, and the raft is gone.
#[test]
fn from_under_the_plate_a_raft_topping_out_at_the_cut_leaves_no_rim() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph = scene_of(vec![supports(closed_box(
        [-30.0, -30.0, 0.0],
        [30.0, 30.0, RAFT_TOP_MM],
    ))]);
    let pixel = middle(&mut renderer, &graph, &under(), RAFT_TOP_MM);
    assert!(is_background(pixel), "{pixel:?}");
}

/// A round raft `radius` mm wide reaching `top` mm, closed, in `sides` walls.
fn round_raft(radius: f32, top: f32, sides: u32) -> IndexedMesh {
    let mut vertices = vec![Vertex::new([0.0, 0.0, 0.0]), Vertex::new([0.0, 0.0, top])];
    for k in 0..sides {
        let a = k as f32 / sides as f32 * std::f32::consts::TAU;
        let (x, y) = (radius * a.cos(), radius * a.sin());
        vertices.push(Vertex::new([x, y, 0.0]));
        vertices.push(Vertex::new([x, y, top]));
    }
    let mut faces = Vec::new();
    for k in 0..sides {
        let (b0, t0) = (2 + 2 * k, 3 + 2 * k);
        let (b1, t1) = (2 + 2 * ((k + 1) % sides), 3 + 2 * ((k + 1) % sides));
        faces.push([0, b1, b0]);
        faces.push([1, t0, t1]);
        faces.push([b0, b1, t1]);
        faces.push([b0, t1, t0]);
    }
    let faces = faces
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
    IndexedMesh::new(None, vertices, faces)
}

/// How many pixels of the workspace frame `graph` changes from the empty
/// one, seen by `camera` with the rafts reaching `raft_top_mm`.
fn pixels_drawn(
    renderer: &mut Renderer,
    graph: &Scene,
    camera: &OrbitCamera,
    raft_top_mm: f32,
    side: u32,
) -> usize {
    pixels_drawn_cut(renderer, graph, camera, raft_top_mm, None, side)
}

/// Like [`pixels_drawn`], with the slice view cut at `clip_z`.
fn pixels_drawn_cut(
    renderer: &mut Renderer,
    graph: &Scene,
    camera: &OrbitCamera,
    raft_top_mm: f32,
    clip_z: Option<f32>,
    side: u32,
) -> usize {
    let mut frame_of = |graph: &Scene| {
        renderer.upload_scene(graph).expect("upload the scene");
        renderer.set_raft_top(raft_top_mm);
        renderer
            .workspace_pixels_for_test(
                SceneRender {
                    graph,
                    selection: &[],
                    gizmo: None,
                    clip_z,
                },
                BV,
                camera,
                side,
                side,
            )
            .expect("the frame is not refused")
            .expect("a sized frame")
    };
    let empty = frame_of(&scene_of(Vec::new()));
    let drawn = frame_of(graph);
    empty
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(drawn.rgba.as_chunks::<4>().0)
        .filter(|(e, d)| e.iter().zip(d.iter()).any(|(a, b)| a.abs_diff(*b) > 2))
        .count()
}

/// A raft's wall seen almost edge on, at the left and right of a round raft
/// from under the plate, covers some of a pixel's samples but not its
/// centre: the height there, read at the centre, ran far past the wall's top
/// and the cut kept the pixel, a speck on the raft's rim.
#[test]
fn from_under_the_plate_a_round_raft_leaves_no_specks_on_its_rim() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph = scene_of(vec![supports(round_raft(30.0, RAFT_TOP_MM, 96))]);
    let mut found = Vec::new();
    for (pitch, orthographic) in [(-30.0, true), (-60.0, true), (-30.0, false), (-60.0, false)] {
        let mut camera = camera(pitch);
        camera.orthographic = orthographic;
        assert_eq!(under_plate_fade(&camera), 1.0);
        let specks = pixels_drawn(&mut renderer, &graph, &camera, RAFT_TOP_MM, 512);
        // With nothing to hide, the raft shows.
        let whole = pixels_drawn(&mut renderer, &graph, &camera, 0.0, 512);
        assert!(whole > 10_000, "{whole}");
        found.push((pitch, orthographic, specks));
    }
    assert!(found.iter().all(|f| f.2 == 0), "{found:?}");
}

/// A closed round post `radius` mm wide from `bottom` to `top` mm, in
/// `sides` walls.
fn round_post(radius: f32, bottom: f32, top: f32, sides: u32) -> IndexedMesh {
    let mut post = round_raft(radius, top - bottom, sides);
    let lifted = post
        .vertices()
        .iter()
        .map(|v| {
            let [x, y, z] = v.coords();
            Vertex::new([x, y, z + bottom])
        })
        .collect();
    let faces = post.faces().to_vec();
    post = IndexedMesh::new(None, lifted, faces);
    post
}

/// The slice view (Slice tab) hides what stands above the layer. A wall of
/// a part above the cut, seen almost edge on, covers some of a pixel's
/// samples but not its centre; the height read at the centre must not let
/// that pixel through as a speck of the hidden part.
#[test]
fn the_slice_view_leaves_no_specks_of_what_stands_above_the_layer() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    const CUT_MM: f32 = 5.0;
    let graph = scene_of(vec![round_post(20.0, CUT_MM + 2.0, CUT_MM + 22.0, 96)]);
    let mut found = Vec::new();
    for (pitch, orthographic) in [
        (30.0, true),
        (60.0, true),
        (85.0, true),
        (30.0, false),
        (60.0, false),
        (85.0, false),
    ] {
        let mut camera = camera(pitch);
        camera.orthographic = orthographic;
        let specks = pixels_drawn_cut(&mut renderer, &graph, &camera, 0.0, Some(CUT_MM), 512);
        // Without the cut, the post shows.
        let whole = pixels_drawn_cut(&mut renderer, &graph, &camera, 0.0, None, 512);
        assert!(whole > 10_000, "{whole}");
        found.push((pitch, orthographic, specks));
    }
    assert!(found.iter().all(|f| f.2 == 0), "{found:?}");
}

/// The workspace frame of `graph` from `camera`, `side` pixels square.
fn frame_of(
    renderer: &mut Renderer,
    graph: &Scene,
    camera: &OrbitCamera,
    side: u32,
) -> FramePixels {
    renderer.upload_scene(graph).expect("upload the scene");
    renderer
        .workspace_pixels_for_test(
            SceneRender {
                graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            },
            BV,
            camera,
            side,
            side,
        )
        .expect("the frame is not refused")
        .expect("a sized frame")
}

/// How many pixels the support lines `lines` (and the balls at their tips)
/// change, seen by `camera` with the supports shown up to `shown_up_to_mm`:
/// the frame with them against the same frame without them.
fn line_pixels(
    renderer: &mut Renderer,
    graph: &Scene,
    camera: &OrbitCamera,
    shown_up_to_mm: f32,
    lines: &Arc<SupportLines>,
) -> usize {
    renderer.set_support_lines(shown_up_to_mm, None);
    let bare = frame_of(renderer, graph, camera, 128);
    renderer.set_support_lines(shown_up_to_mm, Some(lines));
    let drawn = frame_of(renderer, graph, camera, 128);
    bare.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(drawn.rgba.as_chunks::<4>().0)
        .filter(|(a, b)| (0..3).any(|k| a[k].abs_diff(b[k]) > 40))
        .count()
}

/// #493: from under the plate with the supports drawn as lines, a support
/// mesh fades out whole, raft and all, and its line and the ball at its tip
/// show in its place; from above the plate, the support shows as ever.
#[test]
fn from_under_the_plate_the_supports_turn_into_lines() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let graph = scene_of(vec![supports(closed_box(
        [-3.0, -3.0, 0.0],
        [3.0, 3.0, 12.0],
    ))]);
    let shown = pixels_drawn(&mut renderer, &graph, &under(), 0.0, 128);
    assert!(shown > 100, "{shown}");
    renderer.set_support_lines(-1.0, None);
    let hidden = pixels_drawn(&mut renderer, &graph, &under(), 0.0, 128);
    assert_eq!(hidden, 0, "the support fades out whole");

    // Faded out above its raft only: the raft stays.
    renderer.set_support_lines(RAFT_TOP_MM, None);
    let raft = pixels_drawn(&mut renderer, &graph, &under(), 0.0, 128);
    assert!(raft > 0 && raft < shown, "{raft} of {shown}");

    let lines = Arc::new(SupportLines::new(
        &[vec![[0.0, 0.0, 12.0], [0.0, 0.0, 1.0]]],
        &[([0.0, 0.0, 12.0], 1.5)],
    ));
    let drawn = line_pixels(&mut renderer, &graph, &under(), -1.0, &lines);
    assert!(drawn > 20, "{drawn}");

    // From above nothing fades and no line is drawn.
    let from_above = line_pixels(&mut renderer, &graph, &above(), -1.0, &lines);
    assert_eq!(from_above, 0);
    let whole = pixels_drawn(&mut renderer, &graph, &above(), 0.0, 128);
    assert!(whole > 100, "{whole}");
}

/// A line behind a model is hidden by it: the lines are tested against the
/// scene's depth.
#[test]
fn a_support_line_behind_the_model_does_not_show() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    // A wide model slab at 5 mm; a line 2 mm above it, which a camera under
    // the plate sees through the slab only.
    let graph = scene_of(vec![square(60.0, 5.0)]);
    let lines = Arc::new(SupportLines::new(
        &[vec![[-10.0, 0.0, 7.0], [10.0, 0.0, 7.0]]],
        &[],
    ));
    let behind = line_pixels(&mut renderer, &graph, &under(), -1.0, &lines);
    assert_eq!(behind, 0);
    // The same line under the slab shows.
    let lines = Arc::new(SupportLines::new(
        &[vec![[-10.0, 0.0, 3.0], [10.0, 0.0, 3.0]]],
        &[],
    ));
    let below = line_pixels(&mut renderer, &graph, &under(), -1.0, &lines);
    assert!(below > 20, "{below}");
}

/// While the plate fades, what of a support fades with it goes evenly
/// translucent, selected or not: every pixel it covers shows it, none
/// dropped in a pattern for the selection's outline to run round (#493).
#[test]
fn a_support_fading_with_the_plate_goes_evenly_translucent() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    // A support 40 mm up, seen steeply from 2.5 mm under the plate: half
    // faded, with the supports drawn as lines (fading whole).
    let target = Vec3::new(0.0, 0.0, 40.0);
    let mut camera = OrbitCamera::new(target, 50.0);
    camera.target = target;
    camera.distance = 50.0;
    camera.pitch = (-0.85f32).asin();
    camera.yaw = 0.3;
    let fade = under_plate_fade(&camera);
    assert!(fade > 0.3 && fade < 0.7, "{fade}");
    let mut graph = Scene::new();
    let root = graph.add_group(None, Transform3D::Identity);
    graph.add_root(root).expect("group root");
    let leaf = graph
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::identity()),
            LeafPayload::Mesh(Arc::new(supports(square(60.0, 40.0)))),
        )
        .expect("add leaf");
    graph.add_child(root, leaf).expect("attach leaf");
    let empty = scene_of(Vec::new());
    renderer.set_support_lines(-1.0, None);
    for selection in [&[][..], &[leaf][..]] {
        let mut frame = |graph: &Scene| {
            renderer.upload_scene(graph).expect("upload the scene");
            renderer
                .workspace_pixels_for_test(
                    SceneRender {
                        graph,
                        selection,
                        gizmo: None,
                        clip_z: None,
                    },
                    BV,
                    &camera,
                    SIZE,
                    SIZE,
                )
                .expect("the frame is not refused")
                .expect("a sized frame")
        };
        let bare = frame(&empty);
        let drawn = frame(&graph);
        // The middle of the frame, all support.
        let mut kept = Vec::new();
        for y in SIZE / 2 - 8..SIZE / 2 + 8 {
            for x in SIZE / 2 - 8..SIZE / 2 + 8 {
                let i = ((y * SIZE + x) * 4) as usize;
                let shows = (0..3).any(|k| bare.rgba[i + k].abs_diff(drawn.rgba[i + k]) > 2);
                if !shows {
                    kept.push((x, y));
                }
            }
        }
        assert!(
            kept.is_empty(),
            "selected {}: {} pixels show no support: {kept:?}",
            !selection.is_empty(),
            kept.len()
        );
    }
}

/// A support fading with the plate shows its nearest surface alone, as the
/// whole mesh did: what of it lies behind does not blend in, in whatever
/// order its triangles come.
#[test]
fn a_fading_support_shows_its_nearest_surface_alone() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let target = Vec3::new(0.0, 0.0, 40.0);
    let mut camera = OrbitCamera::new(target, 50.0);
    camera.target = target;
    camera.distance = 50.0;
    camera.pitch = (-0.85f32).asin();
    camera.yaw = 0.3;
    let fade = under_plate_fade(&camera);
    assert!(fade > 0.3 && fade < 0.7, "{fade}");
    renderer.set_support_lines(-1.0, None);
    // From below, the lower square is the nearer. One mesh, the far square's
    // triangles first, as a support mesh may list them.
    let (far, near) = (square(60.0, 55.0), square(60.0, 40.0));
    let base = far.vertices().len() as u32;
    let mut vertices = far.vertices().to_vec();
    vertices.extend_from_slice(near.vertices());
    let mut faces = far.faces().to_vec();
    faces.extend(
        near.faces()
            .iter()
            .map(|f| IndexedTriangle::from_raw_indices(f.vertices.map(|v| u32::from(v) + base))),
    );
    let both = supports(IndexedMesh::new(None, vertices, faces));
    let near_only = supports(near);
    let one = middle_of(&mut renderer, &scene_of(vec![near_only]), &camera);
    let two = middle_of(&mut renderer, &scene_of(vec![both]), &camera);
    let gap = (0..3).map(|k| one[k].abs_diff(two[k])).max().unwrap();
    assert!(
        gap <= 3,
        "the far square shows through: {one:?} against {two:?}"
    );
}

/// The frame's middle pixel from `camera`, no raft hidden.
fn middle_of(renderer: &mut Renderer, graph: &Scene, camera: &OrbitCamera) -> [u8; 3] {
    let frame = frame_of(renderer, graph, camera, SIZE);
    let i = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

#[test]
fn a_fading_set_marks_the_stencil_by_how_opaque_it_still_is() {
    assert_eq!(fading_stencil(0.0), 202);
    assert_eq!(fading_stencil(0.5), 102);
    assert_eq!(fading_stencil(1.0), 2);
    assert_eq!(fading_stencil(2.0), 2);
}

/// The plate as it looks, for a person to judge (#507): with
/// `BREADSLICER_PLATE_SHOT` naming a PNG, a 1400 x 860 frame of an M7 Pro's
/// plate with its label, rulers and a 30 mm cube on it.
#[test]
fn the_plate_picture_is_written_when_asked() {
    let Some(path) = std::env::var_os("BREADSLICER_PLATE_SHOT") else {
        return;
    };
    let Some(mut renderer) = renderer() else {
        return;
    };
    let bv = BuildVolume::new(223.642, 126.976, 230.0);
    let corner = bv.envelope_min_mm();
    renderer
        .update_front_label(1, &mut || {
            crate::front_label::rasterise_label(
                "FRONT  ·  223.6 × 127 mm",
                Some("Anycubic Photon Mono M7 Pro"),
            )
        })
        .expect("the label uploads");
    renderer
        .update_plate_marks(1, &mut || {
            crate::front_label::rasterise_plate_marks(
                [corner.x, corner.y],
                bv.width_mm(),
                bv.depth_mm(),
            )
        })
        .expect("the marks upload");
    let cube = {
        let h = 15.0;
        let c = [
            [-h, -h, 0.0],
            [h, -h, 0.0],
            [h, h, 0.0],
            [-h, h, 0.0],
            [-h, -h, 2.0 * h],
            [h, -h, 2.0 * h],
            [h, h, 2.0 * h],
            [-h, h, 2.0 * h],
        ];
        let faces = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        IndexedMesh::new(
            None,
            c.map(Vertex::new).to_vec(),
            faces.map(IndexedTriangle::from_raw_indices).to_vec(),
        )
    };
    let graph = scene_of(vec![cube]);
    renderer.upload_scene(&graph).expect("upload the scene");
    let (centre, radius) = bv.framing(None);
    let mut camera = OrbitCamera::new(Vec3::new(centre.x, centre.y, 25.0), radius);
    camera.distance *= 0.72;
    // `BREADSLICER_PLATE_SHOT_PITCH` (degrees, negative from below) views it
    // from another height.
    if let Some(pitch) = std::env::var("BREADSLICER_PLATE_SHOT_PITCH")
        .ok()
        .and_then(|p| p.parse::<f32>().ok())
    {
        camera.pitch = pitch.to_radians();
    }
    let (w, h) = (1400, 860);
    let frame = renderer
        .workspace_pixels_for_test(
            SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            },
            bv,
            &camera,
            w,
            h,
        )
        .expect("the frame is not refused")
        .expect("a sized frame");
    image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
        .expect("RGBA")
        .save(path)
        .expect("the picture is written");
}

/// The Hollow tab's lasso leaves the supports out whole (#512): a support
/// mesh, selected or not, draws nothing, from above the plate as from under
/// it, while a model next to it still draws.
#[test]
fn hidden_supports_draw_nothing_and_models_still_draw() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let target = Vec3::new(0.0, 0.0, 40.0);
    let frame = |renderer: &mut Renderer, graph: &Scene, selection: &[NodeId], pitch: f32| {
        let mut camera = OrbitCamera::new(target, 50.0);
        camera.target = target;
        camera.distance = 50.0;
        camera.pitch = pitch.asin();
        camera.yaw = 0.3;
        renderer.upload_scene(graph).expect("upload the scene");
        renderer
            .workspace_pixels_for_test(
                SceneRender {
                    graph,
                    selection,
                    gizmo: None,
                    clip_z: None,
                },
                BV,
                &camera,
                SIZE,
                SIZE,
            )
            .expect("the frame is not refused")
            .expect("a sized frame")
    };
    let differs = |a: &[u8], b: &[u8]| {
        a.chunks(4)
            .zip(b.chunks(4))
            .filter(|(p, q)| (0..3).any(|k| p[k].abs_diff(q[k]) > 2))
            .count()
    };
    let support = scene_of(vec![supports(square(60.0, 40.0))]);
    let model = scene_of(vec![square(60.0, 40.0)]);
    let empty = scene_of(Vec::new());
    let leaf = support.first_leaf_id().expect("the support leaf");
    for pitch in [0.85f32, -0.85] {
        renderer.set_supports_hidden(false);
        let bare = frame(&mut renderer, &empty, &[], pitch).rgba;
        let shown = frame(&mut renderer, &support, &[], pitch).rgba;
        assert!(differs(&bare, &shown) > 100, "the support shows");
        renderer.set_supports_hidden(true);
        for selection in [&[][..], &[leaf][..]] {
            let hidden = frame(&mut renderer, &support, selection, pitch).rgba;
            assert_eq!(differs(&bare, &hidden), 0, "pitch {pitch}");
        }
        let drawn = frame(&mut renderer, &model, &[], pitch).rgba;
        assert!(differs(&bare, &drawn) > 100, "a model still draws");
    }
}

/// The scene as it looks, for a person to judge (#523): with
/// `BREADSLICER_SCENE_SHOT` naming a folder and `BREADSLICER_SCENE_MODEL` /
/// `BREADSLICER_SCENE_SUPPORTS` binary STLs of a model and its supports,
/// three copies side by side on an M7 Pro's plate, written as `none.png`
/// (nothing selected) and `selected.png` (the middle model selected, its
/// supports in focus with it), plus `close.png`, the selected one up close.
#[test]
fn the_scene_picture_is_written_when_asked() {
    let (Some(dir), Some(model), Some(supports)) = (
        std::env::var_os("BREADSLICER_SCENE_SHOT"),
        std::env::var_os("BREADSLICER_SCENE_MODEL"),
        std::env::var_os("BREADSLICER_SCENE_SUPPORTS"),
    ) else {
        return;
    };
    let Some(mut renderer) = renderer() else {
        return;
    };
    let read = |path: &std::ffi::OsStr, name: Option<&str>| {
        let data = std::fs::read(path).expect("the STL reads");
        let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
        let mut vertices = Vec::with_capacity(n * 3);
        let mut faces = Vec::with_capacity(n);
        for t in 0..n {
            let at = 84 + t * 50 + 12;
            for k in 0..3 {
                let f = |i: usize| {
                    f32::from_le_bytes(
                        data[at + k * 12 + i * 4..at + k * 12 + i * 4 + 4]
                            .try_into()
                            .unwrap(),
                    )
                };
                vertices.push(Vertex::new([f(0), f(1), f(2)]));
            }
            let b = (t * 3) as u32;
            faces.push(IndexedTriangle::from_raw_indices([b, b + 1, b + 2]));
        }
        let meta = name.map(|name| MeshMetadata {
            name: Some(name.to_owned()),
            ..Default::default()
        });
        Arc::new(IndexedMesh::new(meta, vertices, faces))
    };
    let model = read(&model, None);
    let supports = read(&supports, Some(DF_SUPPORT_MESH_NAME));
    // Centred on the plate, its lowest point on it.
    let (lo, hi) = model.vertices().iter().chain(supports.vertices()).fold(
        ([f32::MAX; 3], [f32::MIN; 3]),
        |(lo, hi), v| {
            let p = v.as_array();
            (
                std::array::from_fn(|k| lo[k].min(p[k])),
                std::array::from_fn(|k| hi[k].max(p[k])),
            )
        },
    );
    let mut graph = Scene::new();
    let root = graph.add_group(None, Transform3D::Identity);
    graph.add_root(root).expect("group root");
    let mut leaves = Vec::new();
    for (k, dx) in [-62.0f64, 0.0, 62.0].into_iter().enumerate() {
        let at = [
            dx - f64::from(lo[0] + hi[0]) / 2.0,
            f64::from(k as f32 * 3.0) - f64::from(lo[1] + hi[1]) / 2.0,
            -f64::from(lo[2]),
        ];
        let mut pair = Vec::new();
        for mesh in [&model, &supports] {
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation(at)),
                    LeafPayload::Mesh(mesh.clone()),
                )
                .expect("add leaf");
            graph.add_child(root, leaf).expect("attach leaf");
            pair.push(leaf);
        }
        leaves.push(pair);
    }
    renderer.upload_scene(&graph).expect("upload the scene");
    let bv = BuildVolume::new(223.642, 126.976, 230.0);
    let dir = std::path::PathBuf::from(dir);
    let shot = |renderer: &mut Renderer, name: &str, selection: &[NodeId], camera: &OrbitCamera| {
        let mut sorted = selection.to_vec();
        sorted.sort_unstable();
        let (w, h) = (1400, 820);
        let frame = renderer
            .workspace_pixels_for_test(
                SceneRender {
                    graph: &graph,
                    selection: &sorted,
                    gizmo: None,
                    clip_z: None,
                },
                bv,
                camera,
                w,
                h,
            )
            .expect("the frame is not refused")
            .expect("a sized frame");
        image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
            .expect("RGBA")
            .save(dir.join(name))
            .expect("the picture is written");
    };
    let mut wide = OrbitCamera::new(Vec3::new(0.0, 0.0, 20.0), 100.0);
    wide.target = Vec3::new(0.0, 0.0, 20.0);
    wide.distance = 230.0;
    wide.pitch = 0.42;
    wide.yaw = -1.45;
    renderer.set_focus(&[]);
    shot(&mut renderer, "none.png", &[], &wide);
    renderer.set_focus(&leaves[1]);
    shot(&mut renderer, "selected.png", &leaves[1][..1], &wide);
    let mut close = wide.clone();
    close.target = Vec3::new(0.0, 0.0, 22.0);
    close.distance = 110.0;
    shot(&mut renderer, "close.png", &leaves[1][..1], &close);
}

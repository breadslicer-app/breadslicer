/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The overhang map against the support engine, through the real shader: the
//! tests draw a face with the map on, read the pixel back and compare it with
//! what the engine's classification says of the same face, posed. They
//! soft-skip without an adapter, as the rest of the GPU tests do.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_geom::df_supports::{DF_SUPPORT_MESH_NAME, overhang_and_stability_from_soup};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::MeshMetadata;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::node::LeafPayload;

use super::*;
use crate::build_volume::NO_PRINTER_BUILD_VOLUME;
use crate::test_support::gpu_devices::shared_test_device;

const BV: BuildVolume = NO_PRINTER_BUILD_VOLUME;
const SIZE: u32 = 64;
/// Where the test faces stand: well inside the volume, above the plate.
const CENTRE: [f64; 3] = [0.0, 0.0, 60.0];
/// Under the 150 mm² from which the engine also takes a steep flat as a topple
/// lever: here only the angle decides.
const QUAD_SIDE: f32 = 10.0;

fn renderer() -> Option<Renderer> {
    let (device, queue) = shared_test_device()?;
    Some(
        Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
            .expect("Renderer::new under the test device"),
    )
}

/// A square facing straight down, centred on the origin of its frame.
fn down_facing_quad() -> IndexedMesh {
    let h = QUAD_SIDE / 2.0;
    let vertices = [[-h, -h, 0.0], [-h, h, 0.0], [h, h, 0.0], [h, -h, 0.0]]
        .map(Vertex::new)
        .to_vec();
    let faces = vec![
        IndexedTriangle::from_raw_indices([0, 1, 2]),
        IndexedTriangle::from_raw_indices([0, 2, 3]),
    ];
    IndexedMesh::new(None, vertices, faces)
}

fn named_supports(mut mesh: IndexedMesh) -> IndexedMesh {
    mesh.set_meta(Some(MeshMetadata {
        name: Some(DF_SUPPORT_MESH_NAME.to_owned()),
        header_comment: None,
        declared_facet_normals: None,
    }));
    mesh
}

fn scene_of(meshes: Vec<(IndexedMesh, AffineTransform3D)>) -> (Scene, Vec<NodeId>) {
    let mut graph = Scene::new();
    let root = graph.add_group(None, Transform3D::Identity);
    graph.add_root(root).expect("group root");
    let leaves = meshes
        .into_iter()
        .map(|(mesh, pose)| {
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(pose),
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .expect("add leaf");
            graph.add_child(root, leaf).expect("attach leaf");
            leaf
        })
        .collect();
    (graph, leaves)
}

/// A turn that leaves the down-facing quad `tilt_deg` from horizontal, about a
/// horizontal axis `heading_deg` round from X, standing at [`CENTRE`].
fn tilted(tilt_deg: f64, heading_deg: f64, squash_z: f64) -> AffineTransform3D {
    let heading = heading_deg.to_radians();
    let turn = AffineTransform3D::from_axis_angle(
        [heading.cos(), heading.sin(), 0.0],
        tilt_deg.to_radians(),
    )
    .expect("a horizontal axis");
    let squash = AffineTransform3D::from_non_uniform_scale([1.0, 1.0, squash_z]);
    let place = AffineTransform3D::from_translation(CENTRE);
    place.compose(&squash.compose(&turn))
}

/// The mesh in world space, as the support scans take it.
fn posed_soup(mesh: &IndexedMesh, pose: &AffineTransform3D) -> Vec<f32> {
    let mut soup = Vec::with_capacity(mesh.faces().len() * 9);
    for face in mesh.faces() {
        for index in face.vertices {
            let vertex = mesh.vertices()[u32::from(index) as usize];
            soup.extend(pose.apply_to_vertex(vertex).coords());
        }
    }
    soup
}

fn engine_calls_it_an_overhang(soup: &[f32], angle_deg: f32) -> bool {
    let (regions, _) = overhang_and_stability_from_soup(soup, angle_deg, 0.25, false);
    !regions.is_empty()
}

/// A camera looking square at the first face of `soup`, from its front.
fn camera_facing(soup: &[f32]) -> OrbitCamera {
    let p = |i: usize| Vec3::new(soup[i], soup[i + 1], soup[i + 2]);
    let (a, b, c) = (p(0), p(3), p(6));
    let normal = (b - a).cross(c - a).normalize();
    let target = Vec3::new(CENTRE[0] as f32, CENTRE[1] as f32, CENTRE[2] as f32);
    let mut camera = OrbitCamera::new(target, 30.0);
    camera.target = target;
    camera.distance = 25.0;
    camera.pitch = normal.z.asin();
    camera.yaw = normal.y.atan2(normal.x);
    camera
}

fn draw(
    renderer: &mut Renderer,
    graph: &Scene,
    selection: &[NodeId],
    camera: &OrbitCamera,
    map: Option<OverhangMap>,
) -> FramePixels {
    let scene_render = SceneRender {
        graph,
        selection,
        gizmo: None,
        clip_z: None,
    };
    let texture = renderer
        .render_frame(
            Some(scene_render),
            BV,
            camera,
            SIZE,
            SIZE,
            FrameLook {
                contents: FrameContents::ModelsOnly,
                overhang_map: map,
                raft_top_mm: 0.0,
                supports_shown_up_to_mm: f32::MAX,
                supports_hidden: false,
            },
        )
        .expect("the frame is not refused")
        .expect("a sized frame");
    readback::read_target_rgba(&renderer.core.device, &renderer.core.queue, &texture)
        .expect("read the frame back")
}

fn centre_pixel(frame: &FramePixels) -> [u8; 3] {
    let i = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

/// Whether a pixel is the map's red. Lighting scales the channels alike, so
/// their ratio tells: the red is 2.7 or more, the rose under 1.5 and the
/// model's own grey under 1.
fn is_overhang_red([r, g, _]: [u8; 3]) -> bool {
    f32::from(r) > 1.8 * f32::from(g)
}

/// The acceptance test of #91's angle: the shader and the engine classify the
/// same faces alike, in the world frame. The turn only ever reaches the GPU as
/// the model matrix: the scene goes up once, as when turning a model by hand.
#[test]
fn the_map_and_the_engine_classify_every_pose_alike() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    let (mut graph, leaves) = scene_of(vec![(mesh.clone(), tilted(0.0, 0.0, 1.0))]);
    renderer.upload_scene(&graph).expect("upload the quad");

    let mut overhangs = 0;
    let mut safe = 0;
    // 45° is the engine's default; 52° and 55° what the zone presets' 38° and
    // 35° from vertical read as; 20° and 75° the bounds.
    for angle in [20.0_f32, 30.0, 45.0, 52.0, 55.0, 75.0] {
        let map = OverhangMap::new(angle);
        let a = f64::from(angle);
        let tilts = [
            a - 12.0,
            a - 1.0,
            a - 0.5,
            a + 0.5,
            a + 1.0,
            a + 12.0,
            3.0,
            90.0,
            130.0,
        ];
        for tilt in tilts {
            for heading in [0.0, 37.0, 140.0, 275.0] {
                // A squash after the turn bends the normal the way only the
                // posed mesh knows: the engine's face normal, not the model
                // matrix applied to a stored normal.
                for squash in [1.0, 0.6] {
                    let pose = tilted(tilt, heading, squash);
                    let soup = posed_soup(&mesh, &pose);
                    let expected = engine_calls_it_an_overhang(&soup, angle);
                    graph
                        .set_local_transform(leaves[0], Transform3D::Affine(pose))
                        .expect("turn the quad");
                    let frame = draw(&mut renderer, &graph, &[], &camera_facing(&soup), Some(map));
                    let pixel = centre_pixel(&frame);
                    assert_eq!(
                        is_overhang_red(pixel),
                        expected,
                        "limit {angle}°, tilt {tilt}°, heading {heading}°, \
                         squash {squash}: pixel {pixel:?}",
                    );
                    if expected {
                        overhangs += 1;
                    } else {
                        safe += 1;
                    }
                }
            }
        }
    }
    assert!(overhangs > 100 && safe > 100, "{overhangs} / {safe}");
}

#[test]
fn without_the_map_a_face_is_drawn_as_ever() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    let pose = tilted(5.0, 20.0, 1.0);
    let soup = posed_soup(&mesh, &pose);
    let (graph, _) = scene_of(vec![(mesh, pose)]);
    renderer.upload_scene(&graph).expect("upload the quad");
    let camera = camera_facing(&soup);

    let plain = draw(&mut renderer, &graph, &[], &camera, None);
    assert!(!is_overhang_red(centre_pixel(&plain)));
    let mapped = draw(
        &mut renderer,
        &graph,
        &[],
        &camera,
        Some(OverhangMap::new(45.0)),
    );
    assert!(is_overhang_red(centre_pixel(&mapped)));
    let plain_again = draw(&mut renderer, &graph, &[], &camera, None);
    assert_eq!(plain.rgba, plain_again.rgba);
}

#[test]
fn the_red_deepens_towards_a_flat_ceiling_and_the_rose_warns_before_the_limit() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    let (mut graph, leaves) = scene_of(vec![(mesh.clone(), tilted(0.0, 0.0, 1.0))]);
    renderer.upload_scene(&graph).expect("upload the quad");
    let map = OverhangMap::new(45.0);
    let mut pixel_at = |tilt: f64, renderer: &mut Renderer| {
        let pose = tilted(tilt, 0.0, 1.0);
        let soup = posed_soup(&mesh, &pose);
        graph
            .set_local_transform(leaves[0], Transform3D::Affine(pose))
            .expect("turn the quad");
        centre_pixel(&draw(
            renderer,
            &graph,
            &[],
            &camera_facing(&soup),
            Some(map),
        ))
    };
    let ratio = |[r, g, _]: [u8; 3]| f32::from(r) / f32::from(g).max(1.0);

    let ceiling = ratio(pixel_at(2.0, &mut renderer));
    let edge = ratio(pixel_at(44.0, &mut renderer));
    let near = ratio(pixel_at(46.0, &mut renderer));
    let away = ratio(pixel_at(60.0, &mut renderer));
    assert!(ceiling > edge, "{ceiling} / {edge}");
    assert!(
        edge > 2.0 * near / 1.5,
        "the limit is a clear step: {edge} / {near}"
    );
    assert!(near > 1.15, "the rose shows: {near}");
    // The model's own grey is neutral (#523): red and green alike.
    assert!(away < 1.03, "past the band the model is plain: {away}");
}

#[test]
fn supports_and_rafts_are_left_out_of_the_map() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = named_supports(down_facing_quad());
    let pose = tilted(5.0, 0.0, 1.0);
    let soup = posed_soup(&mesh, &pose);
    let (graph, _) = scene_of(vec![(mesh, pose)]);
    renderer.upload_scene(&graph).expect("upload the supports");

    let frame = draw(
        &mut renderer,
        &graph,
        &[],
        &camera_facing(&soup),
        Some(OverhangMap::new(45.0)),
    );
    assert!(!is_overhang_red(centre_pixel(&frame)));
}

#[test]
fn a_face_resting_on_the_plate_is_left_plain_and_one_off_it_is_not() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    let map = Some(OverhangMap::new(45.0));
    let mut camera = OrbitCamera::new(Vec3::ZERO, 30.0);
    camera.target = Vec3::ZERO;
    camera.distance = 25.0;
    camera.pitch = -1.5;
    camera.yaw = 0.0;
    let on_plate = AffineTransform3D::from_translation([0.0, 0.0, 0.0]);
    let (mut graph, leaves) = scene_of(vec![(mesh, on_plate)]);
    renderer.upload_scene(&graph).expect("upload the quad");

    let resting = draw(&mut renderer, &graph, &[], &camera, map);
    assert!(!is_overhang_red(centre_pixel(&resting)));

    let lifted = AffineTransform3D::from_translation([0.0, 0.0, 0.05]);
    graph
        .set_local_transform(leaves[0], Transform3D::Affine(lifted))
        .expect("lift the quad");
    let floating = draw(&mut renderer, &graph, &[], &camera, map);
    assert!(is_overhang_red(centre_pixel(&floating)));
}

#[test]
fn the_map_reads_on_a_selected_model_and_keeps_its_outline() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    let pose = tilted(20.0, 0.0, 1.0);
    let soup = posed_soup(&mesh, &pose);
    let (graph, leaves) = scene_of(vec![(mesh, pose)]);
    renderer.upload_scene(&graph).expect("upload the quad");
    let camera = camera_facing(&soup);
    let map = Some(OverhangMap::new(45.0));

    let selected = draw(&mut renderer, &graph, &leaves, &camera, map);
    assert!(is_overhang_red(centre_pixel(&selected)));
    let unselected = draw(&mut renderer, &graph, &[], &camera, map);
    assert_ne!(
        selected.rgba, unselected.rgba,
        "the selection's outline stays on a mapped model",
    );
}

#[test]
fn a_face_past_the_build_volume_stays_amber_under_the_map() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    // Past the +X wall, a 96 mm half width.
    let pose = AffineTransform3D::from_translation([110.0, 0.0, 60.0]);
    let (graph, _) = scene_of(vec![(mesh, pose)]);
    renderer.upload_scene(&graph).expect("upload the quad");
    let target = Vec3::new(110.0, 0.0, 60.0);
    let mut camera = OrbitCamera::new(target, 30.0);
    camera.target = target;
    camera.distance = 25.0;
    camera.pitch = -1.5;

    let plain = draw(&mut renderer, &graph, &[], &camera, None);
    let mapped = draw(
        &mut renderer,
        &graph,
        &[],
        &camera,
        Some(OverhangMap::new(45.0)),
    );
    let [r, g, b] = centre_pixel(&plain);
    assert!(r > g && g > b, "the warning's amber: {r}, {g}, {b}");
    assert_eq!(centre_pixel(&mapped), [r, g, b]);
}

fn red_pixels(frame: &FramePixels) -> usize {
    frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|[r, g, b, _]| is_overhang_red([*r, *g, *b]))
        .count()
}

/// What a tip reaches the map leaves plain, and only that (#484): through the
/// real shader, its two data textures and the hashed lookup.
#[test]
fn a_tip_clears_the_map_within_its_reach() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    let pose = tilted(0.0, 0.0, 1.0);
    let soup = posed_soup(&mesh, &pose);
    let (graph, _) = scene_of(vec![(mesh, pose)]);
    renderer.upload_scene(&graph).expect("upload the quad");
    let camera = camera_facing(&soup);
    let map = Some(OverhangMap::new(45.0));
    let centre = CENTRE.map(|v| v as f32);
    let tip = |pos: [f32; 3], reach_mm: f32| SupportTip { pos, reach_mm };
    let mut drawn = |tips: &[SupportTip]| {
        renderer.set_support_tips(tips);
        draw(&mut renderer, &graph, &[], &camera, map)
    };

    let bare = drawn(&[]);
    let all_red = red_pixels(&bare);
    assert!(is_overhang_red(centre_pixel(&bare)));
    assert!(all_red > 0);

    // A tip in the middle, reaching 2 mm of the 10 mm square: the middle is
    // plain, about an eighth of the red goes, the rest stays.
    let held = drawn(&[tip(centre, 2.0)]);
    assert!(!is_overhang_red(centre_pixel(&held)));
    let left = red_pixels(&held) as f32 / all_red as f32;
    let expected = 1.0 - std::f32::consts::PI * 4.0 / 100.0;
    assert!(
        (left - expected).abs() < 0.03,
        "{left} of the red left, not {expected}"
    );

    // One far off holds nothing here; one reaching past the corners, all.
    let far = drawn(&[tip([centre[0] + 40.0, centre[1], centre[2]], 2.8)]);
    assert_eq!(red_pixels(&far), all_red);
    assert_eq!(red_pixels(&drawn(&[tip(centre, 7.5)])), 0);

    // Many tips in the scene, a short and a long reach among them: the
    // ones around the square clear it, the others change nothing.
    let mut tips: Vec<SupportTip> = (0..2000)
        .map(|k| {
            tip(
                [
                    -200.0 + (k % 50) as f32 * 4.0,
                    80.0 + (k / 50) as f32 * 4.0,
                    30.0,
                ],
                2.8,
            )
        })
        .collect();
    tips.push(tip(centre, 1.8));
    let crowded = drawn(&tips);
    assert!(!is_overhang_red(centre_pixel(&crowded)));
    let left = red_pixels(&crowded) as f32 / all_red as f32;
    let expected = 1.0 - std::f32::consts::PI * 1.8 * 1.8 / 100.0;
    assert!(
        (left - expected).abs() < 0.03,
        "{left} of the red left, not {expected}"
    );

    // Without the map, the tips paint nothing.
    renderer.set_support_tips(&[tip(centre, 7.5)]);
    let plain = draw(&mut renderer, &graph, &[], &camera, None);
    assert_eq!(red_pixels(&plain), 0);
    assert_eq!(
        centre_pixel(&plain),
        centre_pixel(&draw(&mut renderer, &graph, &[], &camera, None))
    );
}

#[test]
fn a_picture_of_the_print_never_carries_the_map() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let mesh = down_facing_quad();
    let pose = tilted(5.0, 0.0, 1.0);
    let soup = posed_soup(&mesh, &pose);
    let (graph, _) = scene_of(vec![(mesh, pose)]);
    renderer.upload_scene(&graph).expect("upload the quad");
    let camera = camera_facing(&soup);

    renderer.set_overhang_map(Some(OverhangMap::new(45.0)));
    let picture = renderer
        .preview_pixels(&graph, BV, &camera, SIZE, SIZE)
        .expect("the preview is not refused")
        .expect("a sized preview");
    assert!(!is_overhang_red(centre_pixel(&picture)));
}

/// The model the manual tests below draw: `BREADSLICER_OVERHANG_STL`, a binary
/// or ASCII STL, standing on the plate's centre.
fn model_from_env() -> Option<(IndexedMesh, AffineTransform3D)> {
    let path = std::env::var_os("BREADSLICER_OVERHANG_STL")?;
    let mut file = std::io::BufReader::new(std::fs::File::open(path).expect("open the model"));
    let (scene, _) = sc_mesh_formats::stl::read_stl(&mut file).expect("read the model");
    let mut found = None;
    scene
        .for_each_expanded_leaf::<sc_scene::SceneError, _>(
            sc_scene::CsgPolicy::Descend,
            |_, _, mesh| {
                found.get_or_insert_with(|| mesh.clone());
                Ok(())
            },
        )
        .expect("walk the model");
    let mesh = found.expect("a mesh in the file");
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for vertex in mesh.vertices() {
        let c = vertex.coords();
        for axis in 0..3 {
            lo[axis] = lo[axis].min(c[axis]);
            hi[axis] = hi[axis].max(c[axis]);
        }
    }
    let centred = [
        -f64::from(lo[0] + hi[0]) / 2.0,
        -f64::from(lo[1] + hi[1]) / 2.0,
        -f64::from(lo[2] + hi[2]) / 2.0,
    ];
    let height = f64::from(hi[2] - lo[2]);
    let place = AffineTransform3D::from_translation([0.0, 0.0, height / 2.0 + 5.0])
        .compose(&AffineTransform3D::from_translation(centred));
    Some((mesh, place))
}

fn framing_camera(renderer: &Renderer) -> OrbitCamera {
    let (lo, hi) = renderer.scene_bounds().expect("a scene");
    let centre = (lo + hi) * 0.5;
    let radius = (hi - lo).length() * 0.5;
    let mut camera = OrbitCamera::new(centre, radius);
    camera.target = centre;
    camera.distance = radius * 2.6;
    camera
}

fn turned(place: &AffineTransform3D, centre: Vec3, degrees: f64) -> Transform3D {
    let pivot = [
        f64::from(centre.x),
        f64::from(centre.y),
        f64::from(centre.z),
    ];
    let turn = AffineTransform3D::from_axis_angle([0.6, 0.8, 0.0], degrees.to_radians())
        .expect("a unit axis");
    Transform3D::Affine(
        AffineTransform3D::from_translation(pivot)
            .compose(&turn)
            .compose(&AffineTransform3D::from_translation(pivot.map(|v| -v)))
            .compose(place),
    )
}

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    let at = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[at].as_secs_f64() * 1000.0
}

/// The cost of a frame while a model turns, at 1920×1080, end to end (the
/// matrix, the draw, the GPU finishing). Manual: set
/// `BREADSLICER_OVERHANG_STL`, `BREADSLICER_OVERHANG_MAP=1` for the map on and
/// `BREADSLICER_OVERHANG_CLOSE=1` for the model filling the frame, where the
/// shader has the most pixels to paint, and run with `--ignored --nocapture` in
/// the release profile; a comparison wants the runs alternated, one process
/// each.
#[test]
#[ignore = "manual timing on a large model"]
fn frame_cost_while_turning() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let Some((mesh, place)) = model_from_env() else {
        eprintln!("skipping: BREADSLICER_OVERHANG_STL is not set");
        return;
    };
    println!("triangles: {}", mesh.faces().len());
    let tip_mesh = mesh.clone();
    let (mut graph, leaves) = scene_of(vec![(mesh, place)]);
    renderer.upload_scene(&graph).expect("upload the model");
    let mut camera = framing_camera(&renderer);
    if std::env::var_os("BREADSLICER_OVERHANG_CLOSE").is_some() {
        camera.distance /= 2.6;
    }
    let pivot = camera.target;
    let selection = [leaves[0]];
    let map = std::env::var_os("BREADSLICER_OVERHANG_MAP").is_some();
    renderer.set_overhang_map(map.then(|| OverhangMap::new(45.0)));
    // `BREADSLICER_OVERHANG_TIPS=1`: a tip every 1.8 mm or so over the
    // down-facing faces of the model as it first stands, reaching 1.8 mm
    // (#484), so most red fragments look tips up.
    let tips = if std::env::var_os("BREADSLICER_OVERHANG_TIPS").is_some() {
        tips_under(&tip_mesh, &place, 1.8)
    } else {
        Vec::new()
    };
    renderer.set_support_tips(&tips);

    let mut samples = Vec::new();
    for frame in 0..(60 + 960) {
        let start = Instant::now();
        graph
            .set_local_transform(leaves[0], turned(&place, pivot, (frame % 240) as f64 * 1.5))
            .expect("turn the model");
        let scene_render = SceneRender {
            graph: &graph,
            selection: &selection,
            gizmo: None,
            clip_z: None,
        };
        renderer
            .render(Some(scene_render), BV, &camera, 1920, 1080)
            .expect("the frame is not refused");
        renderer.wait_for_gpu();
        if frame >= 60 {
            samples.push(start.elapsed());
        }
    }
    samples.sort_unstable();
    let mean = samples.iter().sum::<Duration>().as_secs_f64() * 1000.0 / samples.len() as f64;
    println!(
        "map {}, {} tips: mean {mean:.3} ms, p50 {:.3} ms, p95 {:.3} ms, p99 {:.3} ms ({} frames)",
        if map { "on" } else { "off" },
        tips.len(),
        percentile(&samples, 0.5),
        percentile(&samples, 0.95),
        percentile(&samples, 0.99),
        samples.len(),
    );
}

/// A tip about every `spacing` mm over the faces of `mesh` that face down,
/// posed by `place`: the centre of the first such face in each cell of
/// that side.
fn tips_under(mesh: &IndexedMesh, place: &AffineTransform3D, spacing: f32) -> Vec<SupportTip> {
    let at = |index: sc_mesh_core::primitives::VertexIndex| {
        Vec3::from(
            place
                .apply_to_vertex(mesh.vertices()[u32::from(index) as usize])
                .coords(),
        )
    };
    let mut seen = std::collections::HashSet::new();
    let mut tips = Vec::new();
    for face in mesh.faces() {
        let [a, b, c] = face.vertices.map(at);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.z > -0.3 {
            continue;
        }
        let centre = (a + b + c) / 3.0;
        let cell = (centre / spacing).floor().as_ivec3();
        if seen.insert(cell) {
            tips.push(SupportTip {
                pos: centre.into(),
                reach_mm: spacing,
            });
        }
    }
    tips
}

/// Pictures of a real model under the map, for looking at the colours.
/// Manual: set `BREADSLICER_OVERHANG_STL` and `BREADSLICER_OVERHANG_PNG_DIR`
/// and run with `--ignored`.
#[test]
#[ignore = "manual pictures of a large model"]
fn pictures_of_the_map() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let Some((mesh, place)) = model_from_env() else {
        eprintln!("skipping: BREADSLICER_OVERHANG_STL is not set");
        return;
    };
    let dir = std::path::PathBuf::from(
        std::env::var_os("BREADSLICER_OVERHANG_PNG_DIR").expect("a folder for the pictures"),
    );
    let (mut graph, leaves) = scene_of(vec![(mesh, place)]);
    renderer.upload_scene(&graph).expect("upload the model");
    let mut camera = framing_camera(&renderer);
    let pivot = camera.target;
    let mut save = |name: &str, selection: &[NodeId], map, camera: &OrbitCamera, graph: &Scene| {
        renderer.set_overhang_map(map);
        let scene_render = SceneRender {
            graph,
            selection,
            gizmo: None,
            clip_z: None,
        };
        let texture = renderer
            .render(Some(scene_render), BV, camera, 1100, 900)
            .expect("the frame is not refused")
            .expect("a sized frame");
        let frame =
            readback::read_target_rgba(&renderer.core.device, &renderer.core.queue, &texture)
                .expect("read the frame back");
        image::save_buffer(
            dir.join(format!("{name}.png")),
            &frame.rgba,
            frame.width,
            frame.height,
            image::ExtendedColorType::Rgba8,
        )
        .expect("save the picture");
    };
    let map = Some(OverhangMap::new(45.0));
    let select = [leaves[0]];
    camera.yaw = -1.2;
    camera.pitch = -0.25;
    save("low-plain", &[], None, &camera, &graph);
    save("low-mapped", &[], map, &camera, &graph);
    save("low-selected", &select, map, &camera, &graph);
    save(
        "low-zone-52",
        &select,
        Some(OverhangMap::new(52.0)),
        &camera,
        &graph,
    );
    graph
        .set_local_transform(leaves[0], turned(&place, pivot, 40.0))
        .expect("turn the model");
    save("turned-selected", &select, map, &camera, &graph);
    camera.pitch = 0.35;
    save("turned-above", &[], map, &camera, &graph);
}

/// A sheet of fine scales 3 mm a side, like a sculpted texture: each scale
/// rises 0.02 mm over 0.1 mm along Y and then folds back under itself, its
/// underside tilting 63° down. Facets of 0.025 to 0.1 mm, the sheet as a
/// whole facing up.
fn scaled_sheet() -> IndexedMesh {
    const COLUMNS: usize = 120;
    const PERIODS: usize = 50;
    let mut profile = vec![[0.0f32, 0.0]];
    for k in 0..PERIODS {
        let start = k as f32 * 0.06;
        profile.push([start + 0.1, 0.02]);
        profile.push([start + 0.06, 0.0]);
    }
    let mut vertices = Vec::new();
    for [y, z] in &profile {
        for i in 0..=COLUMNS {
            vertices.push(Vertex::new([i as f32 * 0.025 - 1.5, y - 1.5, *z]));
        }
    }
    let at = |i: usize, j: usize| (j * (COLUMNS + 1) + i) as u32;
    let mut faces = Vec::new();
    for j in 0..profile.len() - 1 {
        for i in 0..COLUMNS {
            faces.push(IndexedTriangle::from_raw_indices([
                at(i, j),
                at(i + 1, j),
                at(i + 1, j + 1),
            ]));
            faces.push(IndexedTriangle::from_raw_indices([
                at(i, j),
                at(i + 1, j + 1),
                at(i, j + 1),
            ]));
        }
    }
    IndexedMesh::new(None, vertices, faces)
}

/// Draws `graph` from `camera` until the surface the map reads is ready
/// (#498): the first frame, painted by facet, and the last.
fn by_facet_then_by_surface(
    renderer: &mut Renderer,
    graph: &Scene,
    camera: &OrbitCamera,
) -> (FramePixels, FramePixels) {
    const SIDE: u32 = 256;
    let map = Some(OverhangMap::new(45.0));
    let draw = |renderer: &mut Renderer| {
        let scene_render = SceneRender {
            graph,
            selection: &[],
            gizmo: None,
            clip_z: None,
        };
        let look = FrameLook {
            contents: FrameContents::ModelsOnly,
            overhang_map: map,
            raft_top_mm: 0.0,
            supports_shown_up_to_mm: f32::MAX,
            supports_hidden: false,
        };
        let texture = renderer
            .render_frame(Some(scene_render), BV, camera, SIDE, SIDE, look)
            .expect("the frame is not refused")
            .expect("a sized frame");
        readback::read_target_rgba(&renderer.core.device, &renderer.core.queue, &texture)
            .expect("read the frame back")
    };
    let first = draw(renderer);
    assert!(
        renderer.overhang_map_pending(),
        "the surface is read on a worker"
    );
    let start = Instant::now();
    loop {
        let frame = draw(renderer);
        if !renderer.overhang_map_pending() {
            return (first, frame);
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the surface never came"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The pixels of the frame the sheet covers: not the clear colour.
fn sheet_pixels(frame: &FramePixels) -> usize {
    let clear = frame::CLEAR_COLOR;
    let clear = [clear.r, clear.g, clear.b].map(|c| (c * 255.0).round() as i32);
    frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|[r, g, b, _]| {
            [*r, *g, *b]
                .iter()
                .zip(clear)
                .any(|(&c, k)| (i32::from(c) - k).abs() > 3)
        })
        .count()
}

fn camera_at(yaw_deg: f32, pitch_deg: f32) -> OrbitCamera {
    let target = Vec3::new(CENTRE[0] as f32, CENTRE[1] as f32, CENTRE[2] as f32);
    let mut camera = OrbitCamera::new(target, 30.0);
    camera.target = target;
    camera.distance = 4.0;
    camera.yaw = yaw_deg.to_radians();
    camera.pitch = pitch_deg.to_radians();
    camera
}

/// The fine scales of a surface that faces up tilt down past the limit: by
/// facet the map spatters them red, reading the surface around them it
/// leaves the sheet plain (#498).
#[test]
fn fine_scales_on_a_surface_facing_up_leave_no_red_specks() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let (graph, _) = scene_of(vec![(
        scaled_sheet(),
        AffineTransform3D::from_translation(CENTRE),
    )]);
    renderer.upload_scene(&graph).expect("upload the sheet");
    // From the side the scales open to, a little above the sheet: their
    // folds and their backs both face the camera.
    let (by_facet, by_surface) =
        by_facet_then_by_surface(&mut renderer, &graph, &camera_at(90.0, 18.0));
    let sheet = sheet_pixels(&by_surface);
    assert!(sheet > 8_000, "the sheet fills the frame ({sheet} pixels)");
    let specks = red_pixels(&by_facet);
    assert!(
        specks > sheet / 20,
        "by facet the folds show red: {specks} of {sheet}"
    );
    assert_eq!(red_pixels(&by_surface), 0);
}

/// Turned over, the same sheet is an overhang whose fine scales tilt up: by
/// facet they leave plain specks in its red, reading the surface it is red
/// throughout (#498).
#[test]
fn fine_scales_inside_an_overhang_leave_no_plain_specks() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let over =
        AffineTransform3D::from_axis_angle([1.0, 0.0, 0.0], std::f64::consts::PI).expect("an axis");
    let pose = AffineTransform3D::from_translation(CENTRE).compose(&over);
    let (graph, _) = scene_of(vec![(scaled_sheet(), pose)]);
    renderer.upload_scene(&graph).expect("upload the sheet");
    let (by_facet, by_surface) =
        by_facet_then_by_surface(&mut renderer, &graph, &camera_at(-90.0, -18.0));
    let sheet = sheet_pixels(&by_surface);
    assert!(sheet > 8_000, "the sheet fills the frame ({sheet} pixels)");
    let plain = |frame: &FramePixels| sheet_pixels(frame) - red_pixels(frame);
    assert!(
        plain(&by_facet) > sheet / 20,
        "by facet: {} plain",
        plain(&by_facet)
    );
    // The rim of the sheet blends into the background on a few pixels.
    assert!(
        plain(&by_surface) < sheet / 100,
        "{} plain of {sheet}",
        plain(&by_surface)
    );
}

/// Manual (#498): how long the surface of `BREADSLICER_OVERHANG_STL`, scaled
/// by `BREADSLICER_OVERHANG_SCALE`, takes to read, and the map by facet and
/// by surface from a few sides, counted and saved in
/// `BREADSLICER_OVERHANG_PNG_DIR`. Run with `--ignored --nocapture`.
#[test]
#[ignore = "manual measure of a large model"]
fn surface_of_a_real_model() {
    let Some(mut renderer) = renderer() else {
        eprintln!("skipping: no wgpu adapter available in this environment");
        return;
    };
    let Some((mesh, _)) = model_from_env() else {
        eprintln!("skipping: BREADSLICER_OVERHANG_STL is not set");
        return;
    };
    let scale: f64 = std::env::var("BREADSLICER_OVERHANG_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let dir = std::env::var_os("BREADSLICER_OVERHANG_PNG_DIR").map(std::path::PathBuf::from);
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for vertex in mesh.vertices() {
        let c = vertex.coords();
        for axis in 0..3 {
            lo[axis] = lo[axis].min(c[axis]);
            hi[axis] = hi[axis].max(c[axis]);
        }
    }
    let centred = [0, 1, 2].map(|a| -f64::from(lo[a] + hi[a]) / 2.0);
    let height = f64::from(hi[2] - lo[2]) * scale;
    let place = AffineTransform3D::from_translation([0.0, 0.0, height / 2.0 + 5.0])
        .compose(&AffineTransform3D::from_non_uniform_scale([scale; 3]))
        .compose(&AffineTransform3D::from_translation(centred));
    let matrix = Mat4::from_scale(Vec3::splat(scale as f32));
    let reach = surface_normals::reach_for_test(&matrix).expect("a reach");
    let start = Instant::now();
    let surface = surface_normals::read_surface(&mesh, reach);
    let read = start.elapsed();
    let reading = surface
        .as_ref()
        .map_or(0, |s| s.iter().filter(|v| v[3] > 0.0).count());
    eprintln!(
        "{} vertices, {} facets, height {height:.1} mm, reach {reach:.4} in the model's frame: \
         read in {:.0} ms, {reading} vertices read the surface",
        mesh.vertices().len(),
        mesh.faces().len(),
        read.as_secs_f64() * 1000.0
    );

    let (graph, _) = scene_of(vec![(mesh, place)]);
    renderer.upload_scene(&graph).expect("upload the model");
    let base = framing_camera(&renderer);
    let map = Some(OverhangMap::new(45.0));
    let views = [
        ("low", -1.2, -0.25, 1.0),
        ("above", 0.5, 0.45, 1.0),
        ("side", 2.5, 0.1, 1.0),
        ("base-close", 0.9, 0.6, 0.45),
        ("staff-close", -1.6, 0.2, 0.4),
    ];
    for (name, yaw, pitch, zoom) in views {
        // A renderer of its own, for its first frame to paint by facet.
        let Some(fresh) = self::renderer() else {
            return;
        };
        renderer = fresh;
        renderer.upload_scene(&graph).expect("upload the model");
        let mut camera = base.clone();
        camera.distance *= zoom;
        camera.yaw = yaw;
        camera.pitch = pitch;
        let mut frames = Vec::new();
        let start = Instant::now();
        loop {
            let scene_render = SceneRender {
                graph: &graph,
                selection: &[],
                gizmo: None,
                clip_z: None,
            };
            let look = FrameLook {
                contents: FrameContents::ModelsOnly,
                overhang_map: map,
                raft_top_mm: 0.0,
                supports_shown_up_to_mm: f32::MAX,
                supports_hidden: false,
            };
            let texture = renderer
                .render_frame(Some(scene_render), BV, &camera, 1100, 900, look)
                .expect("the frame is not refused")
                .expect("a sized frame");
            let frame =
                readback::read_target_rgba(&renderer.core.device, &renderer.core.queue, &texture)
                    .expect("read the frame back");
            let pending = renderer.overhang_map_pending();
            if frames.is_empty() || !pending {
                frames.push(frame);
            }
            if !pending {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let waited = start.elapsed();
        let first = frames.first().expect("a frame");
        let last = frames.last().expect("a frame");
        eprintln!(
            "{name}: red by facet {}, by surface {} ({} frames, shown after {:.0} ms)",
            red_pixels(first),
            red_pixels(last),
            frames.len(),
            waited.as_secs_f64() * 1000.0
        );
        if let Some(dir) = &dir {
            for (frame, kind) in [(first, "facet"), (last, "surface")] {
                image::save_buffer(
                    dir.join(format!("{name}-{kind}.png")),
                    &frame.rgba,
                    frame.width,
                    frame.height,
                    image::ExtendedColorType::Rgba8,
                )
                .expect("save the picture");
            }
        }
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the brush and the paint cost on a heavy model (#183, #184): the
//! controller as the window drives it, a stroke at the pace of the mouse, the
//! Orient search with and without paint, a mesh swap, a save. Ignored: it
//! needs the posed Wolverine STL (1.9 M triangles).
//!
//! ```text
//! WOLVERINE_STL=<path> cargo test -p sc-slicer-gui --release --lib paint_timing -- --ignored --nocapture
//! ```
//!
//! With `PAINT_SHOTS` naming a folder, `the_paint_colours_on_a_heavy_model`
//! writes what each candidate colour looks like there.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use glam::Vec3;
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_orient::{OrientAnalysis, OrientContext, Preset, SearchMode, SearchOutcome};
use sc_scene::{NodeId, PAINTED, PaintChanges, PaintKind, SurfaceTags, UNPAINTED, carry_tags};
use sc_scene_edit::SceneEdit;

use crate::camera::OrbitCamera;
use crate::mesh_render::{
    BrushRing, DeviceLoss, DeviceToken, FramePixels, PaintLayer, Renderer, SceneRender,
};
use crate::print_selection::PrintDefaults;
use crate::scene_document::SceneDocument;
use crate::selection::{LeafId, Selection};
use crate::surface_brush::SurfaceBrush;
use crate::test_support::gpu_devices::shared_test_device;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::tests_model_import::finish_loads;
use crate::viewport::{ScreenPoint, ViewportController};
use crate::{PointerModifiers, WorkspaceSection};

const VIEW: (f32, f32) = (1280.0, 800.0);

fn wolverine_path() -> Option<PathBuf> {
    std::env::var_os("WOLVERINE_STL").map(PathBuf::from)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// The mean, the 95th percentile and the worst of `samples`, ms.
fn spread(mut samples: Vec<f64>) -> String {
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    let p95 = samples[samples.len() * 95 / 100];
    let worst = samples[samples.len() - 1];
    format!("media {mean:.2} ms, p95 {p95:.2} ms, peor {worst:.2} ms")
}

fn controller() -> Rc<ViewportController> {
    let ctrl = Rc::new(ViewportController::new_for_test());
    ctrl.register_document_listener();
    if let Some((device, queue)) = shared_test_device() {
        let token = DeviceToken::next();
        ctrl.loader().set_gpu(device.clone(), token, queue.clone());
        let renderer =
            Renderer::new(device, token, queue, DeviceLoss::default()).expect("a renderer");
        ctrl.set_renderer_for_test(Box::new(renderer));
    }
    ctrl
}

fn balanced(mode: SearchMode) -> OrientContext {
    OrientContext {
        mode,
        ..OrientContext::new(Preset::Balanced)
    }
}

fn search(analysis: &OrientAnalysis, mode: SearchMode) -> (f64, SearchOutcome) {
    let started = Instant::now();
    let outcome = analysis
        .search(&balanced(mode), &AtomicBool::new(false), &mut |_, _| {})
        .expect("not cancelled");
    (ms(started.elapsed()), outcome)
}

fn say(label: &str, time: f64, outcome: &SearchOutcome) {
    let best = outcome.recommended();
    println!(
        "{label}: {time:.0} ms; opciones {}, sigue como está {}, ≈{:.0} puntas, {:.1} N, {} sobre detalle{}",
        outcome.options.len(),
        outcome.keep_current,
        best.cost.estimated_tips(),
        best.cost.peel.peak_force_n,
        best.cost.detail.map_or(0, |d| d.whole_tips()),
        best.cost.detail.map_or(String::new(), |d| format!(
            ", mira arriba {:.0} %",
            d.facing_up * 100.0
        )),
    );
}

/// The faces within `radius` of the model's highest point: a head, on a
/// figure standing up.
fn head_faces(mesh: &IndexedMesh, radius: f32) -> Vec<u32> {
    let top = mesh
        .vertices()
        .iter()
        .map(|v| v.coords())
        .max_by(|a, b| a[2].total_cmp(&b[2]))
        .expect("vertices");
    mesh.faces()
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            let p = mesh.vertices()[usize::from(f.vertices[0])].coords();
            let d: [f32; 3] = std::array::from_fn(|k| p[k] - top[k]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < radius
        })
        .map(|(i, _)| i as u32)
        .collect()
}

/// The scene's one model, selected.
fn the_model(document: &SceneDocument) -> NodeId {
    let model = document
        .with_scene(|s| s.leaf_id_at_index(0))
        .flatten()
        .expect("the model opened");
    document.set_selection(Selection::One(LeafId(model)));
    model
}

fn paint(document: &SceneDocument, leaf: NodeId, faces: &[u32]) {
    let edit = SceneEdit::PaintSurface {
        node_id: leaf,
        kind: PaintKind::Detail,
        changes: PaintChanges::from_faces(faces.iter().map(|&f| (f, UNPAINTED, PAINTED)).collect()),
    };
    document.apply_edit(edit.clone()).expect("the paint");
    document.push_edit(edit);
}

#[test]
#[ignore = "needs the Wolverine STL named in WOLVERINE_STL"]
fn paint_timing_on_a_heavy_model() {
    let Some(wolverine) = wolverine_path() else {
        eprintln!("WOLVERINE_STL names no file: skipped");
        return;
    };
    on_fresh_slint_thread(move || {
        let ctrl = controller();
        ctrl.open_models(vec![wolverine]);
        finish_loads(&ctrl);
        for _ in 0..3 {
            ctrl.handle_before_rendering();
        }
        let document = ctrl.document();
        let model = the_model(&document);
        ctrl.set_viewport_for_test(VIEW.0, VIEW.1);
        ctrl.set_section(WorkspaceSection::Transform);
        let mesh: Arc<IndexedMesh> = document
            .with_scene(|s| s.graph().leaf_mesh_arc(model).cloned())
            .flatten()
            .expect("a mesh");
        // The camera on the model, filling the view.
        let (lo, hi) = document
            .with_scene(|s| {
                let query = s.leaf(model)?;
                let aabb = query.transformed_aabb(query.world_transform())?;
                Some((
                    Vec3::from(aabb.min().coords()),
                    Vec3::from(aabb.max().coords()),
                ))
            })
            .flatten()
            .expect("a box");
        ctrl.set_camera_for_test(OrbitCamera::new((lo + hi) * 0.5, (hi - lo).length() * 0.28));
        println!(
            "modelo: {} triángulos, {} vértices",
            mesh.faces().len(),
            mesh.vertices().len()
        );

        // The adjacency, as the brush's worker builds it.
        let job = document
            .with_scene(|s| s.adjacency_job(model))
            .flatten()
            .expect("a job");
        let started = Instant::now();
        job.run();
        println!("adyacencia de caras: {:.0} ms", ms(started.elapsed()));

        let brush = SurfaceBrush::new(document.clone(), PaintKind::Detail);
        brush.set_armed(true, Some(model));
        ctrl.set_surface_brush(brush.clone());
        ctrl.show_paint(true);

        // A stroke at the pace of a mouse, for each brush.
        let keys = PointerModifiers::default;
        let path = |i: usize| {
            let t = i as f32 / 300.0 * std::f32::consts::TAU;
            ScreenPoint::new(
                VIEW.0 / 2.0 + 120.0 * t.cos(),
                VIEW.1 / 2.0 + 180.0 * (2.0 * t).sin(),
            )
        };
        for radius in [3.0f32, 8.0] {
            brush.set_radius_mm(radius);
            let mut hover = Vec::new();
            for i in 0..120 {
                let started = Instant::now();
                ctrl.on_pointer_move(path(i), keys());
                hover.push(ms(started.elapsed()));
            }
            println!("{radius} mm, mover el aro: {}", spread(hover));
            let before = document
                .surface_tags(model, PaintKind::Detail)
                .map_or(0, |t| t.painted_faces());
            assert_eq!(before, 0);
            ctrl.on_pointer_down(path(0), keys());
            let mut moves = Vec::new();
            for i in 1..300 {
                let started = Instant::now();
                ctrl.on_pointer_move(path(i), keys());
                moves.push(ms(started.elapsed()));
            }
            let started = Instant::now();
            ctrl.on_pointer_up(path(299), keys());
            let release = ms(started.elapsed());
            let painted = document
                .surface_tags(model, PaintKind::Detail)
                .map_or(0, |t| t.painted_faces());
            println!(
                "{radius} mm, pincelada: {}; soltar {release:.2} ms; {painted} caras pintadas",
                spread(moves)
            );
            let started = Instant::now();
            document.undo().expect("undo");
            println!("{radius} mm, Deshacer: {:.2} ms", ms(started.elapsed()));
        }

        // The panel's steps on a real patch.
        brush.set_radius_mm(3.0);
        paint(&document, model, &head_faces(&mesh, 25.0));
        for (name, step) in [
            ("Crecer", 0),
            ("Achicar", 1),
            ("Invertir", 2),
            ("Invertir de vuelta", 2),
        ] {
            let started = Instant::now();
            match step {
                0 => brush.step_edge(model, true),
                1 => brush.step_edge(model, false),
                _ => brush.invert(model),
            };
            println!("{name}: {:.1} ms", ms(started.elapsed()));
        }

        // Orient, on the same analysis.
        let world = document
            .with_scene(|s| s.leaf(model).map(|q| q.world_transform()))
            .flatten()
            .expect("the leaf");
        let world = match world {
            sc_mesh_core::transform::Transform3D::Identity => {
                sc_mesh_core::transform::AffineTransform3D::identity()
            }
            sc_mesh_core::transform::Transform3D::Affine(a) => a,
        };
        let started = Instant::now();
        let plain = OrientAnalysis::new(&mesh, &world, None, &AtomicBool::new(false))
            .expect("not cancelled");
        println!("análisis de Orientar: {:.0} ms", ms(started.elapsed()));
        for _ in 0..2 {
            let (t, o) = search(&plain, SearchMode::Quick);
            say("Rápido sin pintura", t, &o);
            let (t, o) = search(&plain, SearchMode::Full);
            say("Completo sin pintura", t, &o);
        }
        let tags = document
            .surface_tags(model, PaintKind::Detail)
            .expect("painted");
        for patch in [10.0f32, 25.0, 60.0] {
            let mut values = vec![UNPAINTED; mesh.faces().len()];
            for f in head_faces(&mesh, patch) {
                values[f as usize] = PAINTED;
            }
            let started = Instant::now();
            let painted = plain.with_detail(Some(&values));
            let prepare = ms(started.elapsed());
            let faces = values.iter().filter(|v| **v != UNPAINTED).count();
            println!("pintura de {faces} caras: preparar {prepare:.1} ms");
            let (t, o) = search(&painted, SearchMode::Quick);
            say("Rápido con pintura", t, &o);
            let (t, o) = search(&painted, SearchMode::Full);
            say("Completo con pintura", t, &o);
        }

        // A mesh swap: every vertex nudged, so nothing matches exactly.
        let mut moved = (*mesh).clone();
        for v in &mut moved.vertices {
            let p = v.coords();
            *v = Vertex::new([p[0] + 1e-3, p[1], p[2]]);
        }
        let moved = Arc::new(moved);
        let started = Instant::now();
        let carried = carry_tags(&tags, &moved);
        println!(
            "llevar {} caras pintadas a otra malla: {:.0} ms, {:.1} de {:.1} mm²",
            tags.painted_faces(),
            ms(started.elapsed()),
            carried.area_after,
            carried.area_before
        );
        // What Hollow's landing waits for: a patch of a real size.
        for patch in [10.0f32, 25.0] {
            let mut values = vec![UNPAINTED; mesh.faces().len()];
            for f in head_faces(&mesh, patch) {
                values[f as usize] = PAINTED;
            }
            let local = SurfaceTags::new(mesh.clone(), values).unwrap();
            let started = Instant::now();
            let carried = carry_tags(&local, &moved);
            println!(
                "llevar un parche de {} caras: {:.0} ms, se perdió {:.2} %",
                local.painted_faces(),
                ms(started.elapsed()),
                carried.share_lost() * 100.0
            );
        }
        let all = SurfaceTags::new(mesh.clone(), vec![PAINTED; mesh.faces().len()]).unwrap();
        let started = Instant::now();
        let carried = carry_tags(&all, &moved);
        println!(
            "llevar toda la malla pintada: {:.0} ms, se perdió {:.2} %",
            ms(started.elapsed()),
            carried.share_lost() * 100.0
        );

        // The chunk.
        let started = Instant::now();
        let fingerprint = mesh.geometry_fingerprint();
        println!(
            "huella de la malla: {:.0} ms ({:02x})",
            ms(started.elapsed()),
            fingerprint.as_ref()[0]
        );
        let saved = || {
            let payload = document.compose_project_payload(&PrintDefaults::default());
            let bundle =
                crate::scene_save::prepare_save(&document, payload, PathBuf::from("paint.bread"))
                    .expect("a bundle");
            crate::scene_save::encode_bundle(&bundle, |_, _, _| {}).expect("written")
        };
        let started = Instant::now();
        let bytes = saved();
        let write = ms(started.elapsed());
        let started = Instant::now();
        let read = sc_project_formats::read_bread_project(bytes.as_slice()).expect("read");
        let read_ms = ms(started.elapsed());
        assert_eq!(read.scene.surface_tags_iter().count(), 1);
        assert!(brush.clear(model));
        let bare = saved();
        println!(
            "archivo .bread con pintura: {} bytes (sin pintura {}), guardar {write:.0} ms, abrir {read_ms:.0} ms",
            bytes.len(),
            bare.len()
        );
    });
}

/// What a frame with the head painted looks like in `tint`.
fn shot(
    renderer: &mut Renderer,
    scene: &sc_scene::Scene,
    leaf: NodeId,
    camera: &OrbitCamera,
    tags: &SurfaceTags,
    tint: [f32; 4],
    ring: Option<BrushRing>,
) -> FramePixels {
    renderer.set_surface_paint(
        &[PaintLayer {
            leaf,
            tags: tags.clone(),
        }],
        tint,
    );
    renderer.set_brush_ring(ring);
    renderer
        .workspace_pixels_for_test(
            SceneRender {
                graph: scene,
                selection: &[leaf],
                gizmo: None,
                clip_z: None,
            },
            crate::build_volume::NO_PRINTER_BUILD_VOLUME,
            camera,
            1280,
            800,
        )
        .expect("drawn")
        .expect("a frame")
}

#[test]
#[ignore = "needs the Wolverine STL named in WOLVERINE_STL and a folder in PAINT_SHOTS"]
fn the_paint_colours_on_a_heavy_model() {
    let (Some(wolverine), Some(folder)) = (
        wolverine_path(),
        std::env::var_os("PAINT_SHOTS").map(PathBuf::from),
    ) else {
        eprintln!("WOLVERINE_STL or PAINT_SHOTS missing: skipped");
        return;
    };
    let colours: [(&str, [u8; 3], f32); 3] = [
        ("A-cian", [0x22, 0xc3, 0xd6], 0.85),
        ("B-violeta", [0x8f, 0x6b, 0xff], 0.85),
        ("C-azul", [0x2f, 0x7d, 0xf6], 0.85),
    ];
    on_fresh_slint_thread(move || {
        let ctrl = controller();
        ctrl.open_models(vec![wolverine.clone()]);
        finish_loads(&ctrl);
        let document = ctrl.document();
        let model = the_model(&document);
        let scene = document.with_scene(|s| s.graph().clone()).expect("a scene");
        let mesh = scene.leaf_mesh_arc(model).expect("a mesh").clone();
        let mut values = vec![UNPAINTED; mesh.faces().len()];
        for f in head_faces(&mesh, 22.0) {
            values[f as usize] = PAINTED;
        }
        let tags = SurfaceTags::new(mesh.clone(), values).expect("a mask");
        let (device, queue) = shared_test_device().expect("a GPU");
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue, DeviceLoss::default())
            .expect("a renderer");
        renderer.upload_scene(&scene).expect("uploaded");
        let (lo, hi) = renderer.scene_bounds().expect("bounds");
        let centre = (lo + hi) * 0.5;
        let radius = (hi - lo).length() * 0.5;
        let top = mesh
            .vertices()
            .iter()
            .copied()
            .max_by(|a, b| a.coords()[2].total_cmp(&b.coords()[2]))
            .expect("vertices");
        // The ring and the camera are in the world, the mesh is not.
        let top = document
            .with_scene(|s| s.leaf(model).map(|q| q.world_transform()))
            .flatten()
            .expect("the leaf")
            .apply_to_vertex(top)
            .coords();
        let mut close = OrbitCamera::new(Vec3::from(top) - Vec3::Z * 5.0, radius * 0.16);
        close.yaw += 0.5;
        let whole = OrbitCamera::new(centre, radius * 0.75);
        std::fs::create_dir_all(&folder).expect("the folder");
        // A frame of a stroke: new paint, written once, and the scene drawn.
        let render_one = |renderer: &mut Renderer, tags: Option<&SurfaceTags>| {
            let started = Instant::now();
            if let Some(tags) = tags {
                renderer.set_surface_paint(
                    &[PaintLayer {
                        leaf: model,
                        tags: tags.clone(),
                    }],
                    [0.13, 0.76, 0.84, 0.85],
                );
            }
            let upload = ms(started.elapsed());
            renderer
                .render(
                    Some(SceneRender {
                        graph: &scene,
                        selection: &[model],
                        gizmo: None,
                        clip_z: None,
                    }),
                    crate::build_volume::NO_PRINTER_BUILD_VOLUME,
                    &whole,
                    1280,
                    800,
                )
                .expect("drawn");
            renderer.wait_for_gpu();
            (upload, ms(started.elapsed()))
        };
        let mut bare = Vec::new();
        for _ in 0..60 {
            bare.push(render_one(&mut renderer, None).1);
        }
        println!("cuadro sin pintura: {}", spread(bare));
        let mut uploads = Vec::new();
        let mut frames = Vec::new();
        for _ in 0..60 {
            // A new revision each time, as every frame of a stroke has.
            let fresh = SurfaceTags::new(mesh.clone(), tags.values().to_vec()).expect("a mask");
            let (upload, frame) = render_one(&mut renderer, Some(&fresh));
            uploads.push(upload);
            frames.push(frame);
        }
        println!(
            "pintura de {} caras: armar y subir {}; cuadro entero {}",
            tags.painted_faces(),
            spread(uploads),
            spread(frames)
        );
        for (name, rgb, alpha) in colours {
            let tint = [
                f32::from(rgb[0]) / 255.0,
                f32::from(rgb[1]) / 255.0,
                f32::from(rgb[2]) / 255.0,
                alpha,
            ];
            for (view, camera) in [("cerca", &close), ("entero", &whole)] {
                let ring = BrushRing {
                    leaf: model,
                    center: top,
                    radius_mm: 6.0,
                    color: [1.0, 1.0, 1.0, 0.95],
                };
                let frame = shot(
                    &mut renderer,
                    &scene,
                    model,
                    camera,
                    &tags,
                    tint,
                    Some(ring),
                );
                let path = folder.join(format!("{name}-{view}.png"));
                image::save_buffer(
                    &path,
                    &frame.rgba,
                    frame.width,
                    frame.height,
                    image::ColorType::Rgba8,
                )
                .expect("written");
                println!("{}", path.display());
            }
        }
    });
}

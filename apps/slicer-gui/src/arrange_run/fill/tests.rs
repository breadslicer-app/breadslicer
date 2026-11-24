/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use bread_arrange::Plate;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{LeafPayload, NodeId, PAINTED, PaintChanges, PaintKind, Scene, UNPAINTED};

use super::{Landed, Pending, Progress, fill_now, gather_fill, land, make};
use crate::arrange_run::ArrangeOptions;
use crate::scene_document::{SceneDocument, SceneEdit};
use crate::selection::{LeafId, Selection};

/// A scene of 4 mm cubes standing on the plate at `at` (their middles in
/// X/Y), each with a mesh of its own, and their leaves.
fn cubes(at: &[[f64; 2]]) -> (Rc<SceneDocument>, Vec<NodeId>) {
    let mut graph = Scene::new();
    let mut leaves = Vec::new();
    for &[x, y] in at {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                    [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
                    [x, y, 2.0],
                )),
                LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        leaves.push(leaf);
    }
    let document = SceneDocument::new();
    document.install_graph(graph, PathBuf::from("cubes.stl"));
    (document, leaves)
}

fn plate() -> Plate {
    Plate {
        min: [0.0, 0.0],
        max: [50.0, 40.0],
        floor: 0.0,
        keep_out: bread_arrange::KeepOut::default(),
    }
}

fn select(document: &SceneDocument, leaves: &[NodeId]) {
    document.set_selection(Selection::of(leaves.iter().map(|&l| LeafId(l))));
}

/// Every model leaf of the scene, in row order, with its mesh and world
/// transform.
fn models(document: &SceneDocument) -> Vec<(NodeId, Arc<IndexedMesh>, AffineTransform3D)> {
    document
        .with_scene(|spatial| {
            (0..)
                .map_while(|i| spatial.leaf_id_at_index(i))
                .filter_map(|id| {
                    let mesh = spatial.graph().leaf_mesh_arc(id)?.clone();
                    let world = match spatial.leaf(id)?.world_transform() {
                        Transform3D::Identity => AffineTransform3D::identity(),
                        Transform3D::Affine(a) => a,
                    };
                    Some((id, mesh, world))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The middles of two 4 mm cubes are at least a cube and the gap apart
/// along X or Y, whatever their turn about Z.
fn apart(a: &AffineTransform3D, b: &AffineTransform3D) -> bool {
    let (p, q) = (a.translation(), b.translation());
    // A cube turned by any angle stays within its circumscribed circle;
    // turned by 45° steps its extent is at most 4·√2.
    (p[0] - q[0]).abs().max((p[1] - q[1]).abs()) >= 4.0 + 1.0 - 1e-6
        || (p[0] - q[0]).hypot(p[1] - q[1]) >= 4.0 * std::f64::consts::SQRT_2 + 1.0 - 1e-6
}

/// Whether the fill moved the model of body `body` to let more copies in.
fn moved(filled: &bread_arrange::Filled, body: usize) -> bool {
    filled.moved.iter().any(|m| m.source == body)
}

/// #356: a model in the plate's middle splits it; it moves to let the
/// copies in rows, in the same step, and Undo puts it back.
#[test]
fn a_model_in_the_middle_moves_to_let_more_copies_in() {
    let (document, leaves) = cubes(&[[25.0, 11.0]]);
    select(&document, &leaves);
    let before = models(&document);
    // 4 mm cubes on a 50 x 22 mm plate: 12 mm high inside the margin, two
    // rows of 5 mm; the cube in the middle leaves 3.5 mm above and below.
    let plate = Plate {
        min: [0.0, 0.0],
        max: [50.0, 22.0],
        floor: 0.0,
        keep_out: bread_arrange::KeepOut::default(),
    };
    let filled = fill_now(&document, &plate, old_margin()).expect("filled").0;
    assert!(moved(&filled, 0), "it moves");
    let after = models(&document);
    assert_ne!(after[0].2, before[0].2);
    assert_eq!(after.len(), 1 + filled.copies.len());
    // Staying, only its own row takes copies: 6.
    assert!(filled.copies.len() >= 12, "{}", filled.copies.len());
    for (i, a) in after.iter().enumerate() {
        for b in &after[i + 1..] {
            assert!(apart(&a.2, &b.2), "{:?} {:?}", a.2, b.2);
        }
    }
    assert!(document.undo().unwrap());
    let back = models(&document);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].2, before[0].2, "back where it was");
    assert!(!document.can_undo(), "one step");
}

/// #356: one model selected; its copies fill the plate around it and the
/// others, which stay where they are, share its mesh, and go in one step.
#[test]
fn copies_of_one_model_fill_the_room_left_in_one_step() {
    let (document, leaves) = cubes(&[[10.0, 10.0], [25.0, 20.0], [40.0, 30.0]]);
    select(&document, &leaves[..1]);
    let before = models(&document);
    let filled = fill_now(&document, &plate(), old_margin())
        .expect("filled")
        .0;
    let after = models(&document);
    let added = filled.copies.len();
    assert!(added > 10, "{added} copies");
    assert_eq!(after.len(), before.len() + added);
    // What was there stays, the selected model too unless moving it lets
    // more copies in.
    for (row, (b, a)) in before.iter().zip(&after).enumerate() {
        assert_eq!(b.0, a.0);
        assert!(Arc::ptr_eq(&b.1, &a.1));
        assert_eq!(b.2 == a.2, !moved(&filled, row), "{row}");
    }
    // Each copy shows the selected model's mesh, never a duplicate, on the
    // plate, inside the margin, standing as it did.
    for (_, mesh, world) in &after[before.len()..] {
        assert!(Arc::ptr_eq(mesh, &before[0].1));
        let t = world.translation();
        assert!((t[2] - 2.0).abs() < 1e-9, "still on the plate");
        assert!(
            t[0] > 5.0 && t[0] < 45.0 && t[1] > 5.0 && t[1] < 35.0,
            "{t:?}"
        );
    }
    for (i, a) in after.iter().enumerate() {
        for b in &after[i + 1..] {
            assert!(apart(&a.2, &b.2), "{:?} {:?}", a.2, b.2);
        }
    }
    // One Undo takes every copy away; Redo puts them back.
    assert!(document.undo().unwrap());
    assert_eq!(models(&document).len(), before.len());
    assert!(!document.can_undo(), "one step");
    assert!(document.redo().unwrap());
    assert_eq!(models(&document).len(), after.len());
}

/// #356: two models selected: copies of both, as many of one as of the
/// other, give or take one; the unselected one stays.
#[test]
fn copies_of_two_models_mix_evenly() {
    let (document, leaves) = cubes(&[[10.0, 10.0], [25.0, 20.0], [40.0, 30.0]]);
    // Different meshes, so the copies tell which model they copy.
    select(&document, &[leaves[0], leaves[2]]);
    let before = models(&document);
    let filled = fill_now(&document, &plate(), old_margin())
        .expect("filled")
        .0;
    let count = |source: usize| filled.copies.iter().filter(|c| c.source == source).count();
    let (a, b) = (count(0), count(2));
    assert!(a > 3 && b > 3, "{a} and {b}");
    assert!(a.abs_diff(b) <= 1, "{a} and {b}");
    assert_eq!(count(1), 0, "the unselected one is not copied");
    let after = models(&document);
    assert_eq!(before[1].2, after[1].2, "the unselected one stays");
    for (row, (b, a)) in before.iter().zip(&after).enumerate() {
        assert_eq!(b.2 == a.2, !moved(&filled, row), "{row}");
    }
    let of = |mesh: &Arc<IndexedMesh>| {
        after[before.len()..]
            .iter()
            .filter(|(_, m, _)| Arc::ptr_eq(m, mesh))
            .count()
    };
    assert_eq!(of(&before[0].1), a);
    assert_eq!(of(&before[2].1), b);
    assert_eq!(of(&before[1].1), 0);
}

/// #356: Max copies caps the count; a full plate adds none; nothing
/// selected fills nothing.
#[test]
fn a_fill_takes_at_most_the_copies_asked_for() {
    let (document, leaves) = cubes(&[[10.0, 10.0]]);
    assert!(gather_fill(&document, None).is_none(), "nothing selected");
    select(&document, &leaves);
    let options = ArrangeOptions {
        max_copies: 3,
        ..ArrangeOptions::default()
    };
    let filled = fill_now(&document, &plate(), options).expect("filled").0;
    assert_eq!(filled.copies.len(), 3);
    assert_eq!(models(&document).len(), 4);

    let small = Plate {
        min: [3.0, 3.0],
        max: [17.0, 17.0],
        floor: 0.0,
        keep_out: bread_arrange::KeepOut::default(),
    };
    let none = fill_now(&document, &small, old_margin()).expect("ran").0;
    assert!(none.copies.is_empty());
    assert_eq!(models(&document).len(), 4, "nothing added");
}

/// #183: each copy keeps the paint of its model, on its own mask, and the
/// one Undo of the fill takes the copies and their paint.
#[test]
fn a_filled_copy_keeps_the_paint_of_its_model() {
    let (document, leaves) = cubes(&[[10.0, 10.0]]);
    let edit = SceneEdit::PaintSurface {
        node_id: leaves[0],
        kind: PaintKind::Detail,
        changes: PaintChanges::from_faces(vec![(0, UNPAINTED, PAINTED), (1, UNPAINTED, PAINTED)]),
    };
    document.apply_edit(edit.clone()).unwrap();
    document.push_edit(edit);
    select(&document, &leaves);
    let options = ArrangeOptions {
        max_copies: 2,
        ..ArrangeOptions::default()
    };
    fill_now(&document, &plate(), options).expect("filled");
    let all = models(&document);
    assert_eq!(all.len(), 3);
    for (id, _, _) in &all {
        let tags = document
            .surface_tags(*id, PaintKind::Detail)
            .expect("painted");
        assert_eq!(tags.painted_faces(), 2);
    }
    document.undo().unwrap();
    assert_eq!(models(&document).len(), 1);
    assert!(
        document
            .surface_tags(leaves[0], PaintKind::Detail)
            .is_some()
    );
}

/// The card's options with the edge margin these plates were sized for.
fn old_margin() -> ArrangeOptions {
    ArrangeOptions {
        margin_mm: 5.0,
        ..ArrangeOptions::default()
    }
}

/// #356: a cancelled fill makes nothing, and a result worked out for a
/// scene that changed since adds nothing.
#[test]
fn a_cancelled_or_stale_fill_adds_nothing() {
    let (document, leaves) = cubes(&[[10.0, 10.0]]);
    select(&document, &leaves);
    let (bodies, _, sources) = gather_fill(&document, None).expect("a source");
    let settings = old_margin().settings();
    let cancelled = make(
        &bodies,
        &sources,
        &plate(),
        &settings,
        None,
        &Progress::default(),
        &AtomicBool::new(true),
    );
    assert!(cancelled.is_none());

    let pending = Pending {
        revision: document.scene_revision(),
        sources: sources.clone(),
        started: std::time::Instant::now(),
    };
    let made = make(
        &bodies,
        &sources,
        &plate(),
        &settings,
        None,
        &Progress::default(),
        &AtomicBool::new(false),
    )
    .expect("made");
    assert!(!made.filled.copies.is_empty());
    // The model moves meanwhile.
    let moved = SceneEdit::SetLocalTransform {
        node_id: leaves[0],
        before: document
            .with_scene(|s| s.leaf(leaves[0]).map(|q| q.local_transform()))
            .flatten()
            .unwrap(),
        after: Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
            [30.0, 20.0, 2.0],
        )),
    };
    document.apply_edit(moved.clone()).unwrap();
    document.push_edit(moved);
    assert_eq!(land(&document, &pending, made), Landed::Stale);
    assert_eq!(models(&document).len(), 1, "nothing added");
}

/// Fill bed on a real project, its first model alone (#555):
/// `BREADSLICER_FILL_BREAD` names it. Prints copies and time with and
/// without squeezing; `BREADSLICER_FILL_DUMP` writes the placements.
#[test]
#[ignore = "a measurement on a real project"]
fn fill_bed_on_a_real_model() {
    let Some(path) = std::env::var_os("BREADSLICER_FILL_BREAD") else {
        return;
    };
    let path = PathBuf::from(path);
    let loaded = sc_project_formats::load_scene(&path, None, |_| {}).expect("it opens");
    let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
        panic!("a .bread is a project")
    };
    let document = SceneDocument::new();
    document.install_graph(project.scene, path);
    let (mesh, world) = document
        .with_scene(|spatial| {
            let leaf = spatial.leaf_id_at_index(0)?;
            let q = spatial.leaf(leaf)?;
            Some((
                spatial.graph().leaf_mesh_arc(leaf)?.clone(),
                super::affine(&q.world_transform()),
            ))
        })
        .flatten()
        .expect("a model");
    let (w, h) = (223.642, 126.976);
    let plate = Plate {
        min: [-w / 2.0, -h / 2.0],
        max: [w / 2.0, h / 2.0],
        floor: 0.0,
        keep_out: bread_arrange::KeepOut::default(),
    };
    // In the plate's middle, as it stands.
    let t = world.translation();
    let world = AffineTransform3D::from_linear_and_translation(*world.linear(), [0.0, 0.0, t[2]]);
    let mut bodies = vec![bread_arrange::Body {
        parts: vec![bread_arrange::Part { mesh, world }],
        fixed: false,
    }];
    // Measured with the supports it will get, as the window does (#556):
    // the Miniatures preset on an M7 Pro, lifted as Generate lifts it.
    // `BREADSLICER_FILL_ONE_RAFT=1`: under one raft for all (#556).
    let one_raft = std::env::var_os("BREADSLICER_FILL_ONE_RAFT").is_some();
    let options = sc_geom::supports::SupportOptions {
        zones: Some(sc_geom::supports::zone_presets::minis_30()),
        one_raft,
        ..Default::default()
    };
    let request = sc_geom::supports::SupportRequest {
        layer_height_mm: 0.05,
        pixel_pitch_mm: Some([0.016_8, 0.016_8]),
        options: options.clone(),
        ..Default::default()
    };
    let ctx = crate::support_forecast::ForecastCtx {
        lift_to_mm: Some(crate::raft_clash::support_lift_mm(&options, 0.05)),
        request,
        raft_top_mm: if one_raft {
            options.raft_top_mm().unwrap_or(0.0)
        } else {
            0.0
        },
    };
    let t = std::time::Instant::now();
    crate::support_forecast::with_forecasts(&mut bodies, &[true], &ctx, &AtomicBool::new(false))
        .expect("a forecast");
    println!(
        "forecast: {:.2} s, {} parts",
        t.elapsed().as_secs_f64(),
        bodies[0].parts.len()
    );
    for squeeze in [false, true] {
        let s = bread_arrange::Settings {
            squeeze,
            ..ArrangeOptions {
                one_raft,
                raft_top_mm: ctx.raft_top_mm as f32,
                ..ArrangeOptions::default()
            }
            .settings()
        };
        let started = std::time::Instant::now();
        let filled = bread_arrange::fill(
            &bodies,
            &[0],
            &plate,
            &s,
            None,
            &std::sync::atomic::AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        if let Some(dir) = std::env::var_os("BREADSLICER_FILL_DUMP") {
            // The body's triangles in the world, as a binary STL, and each
            // copy's move: `angle x y` per line.
            let dir = PathBuf::from(dir);
            let mut stl = vec![0u8; 80];
            let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
            for part in &bodies[0].parts {
                let (l, t) = (part.world.linear(), part.world.translation());
                let v = part.mesh.vertices();
                for f in part.mesh.faces() {
                    tris.push(std::array::from_fn(|k| {
                        let c = v[f.vertices[k].get() as usize].coords();
                        std::array::from_fn(|r| {
                            (l[r][0] * f64::from(c[0])
                                + l[r][1] * f64::from(c[1])
                                + l[r][2] * f64::from(c[2])
                                + t[r]) as f32
                        })
                    }));
                }
            }
            stl.extend((tris.len() as u32).to_le_bytes());
            for t in &tris {
                stl.extend([0u8; 12]);
                for p in t {
                    for c in p {
                        stl.extend(c.to_le_bytes());
                    }
                }
                stl.extend([0u8; 2]);
            }
            std::fs::write(dir.join("body.stl"), stl).unwrap();
            let mut out = String::from(
                "0 0 0
",
            );
            for c in &filled.copies {
                let p = c.placement;
                out += &format!(
                    "{} {} {}
",
                    p.angle, p.translation[0], p.translation[1]
                );
            }
            std::fs::write(dir.join(format!("copies-{squeeze}.txt")), out).unwrap();
        }
        println!(
            "squeeze {squeeze}: 1 + {} copies ({} moved) in {:.2} s",
            filled.copies.len(),
            filled.moved.len(),
            started.elapsed().as_secs_f64()
        );
    }
}

/// A report (#628): fills an M7 Pro plate with copies of an STL measured
/// with their forecast supports, as the window does, then generates their
/// supports and tells which reach past the plate's edge.
///
/// `BREADSLICER_EDGE_STL=<model.stl>`, `BREADSLICER_EDGE_ONE_RAFT=1`.
#[test]
#[ignore = "a report on a local model, not a check"]
fn fill_then_generate_keeps_the_supports_on_the_plate() {
    let Some(path) = std::env::var_os("BREADSLICER_EDGE_STL") else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let (loaded, _) =
        sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).expect("an STL");
    let leaf = loaded.roots()[0];
    let mesh = loaded.leaf_mesh_arc(leaf).unwrap().clone();
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for v in mesh.vertices() {
        for k in 0..3 {
            lo[k] = lo[k].min(v.coords()[k]);
            hi[k] = hi[k].max(v.coords()[k]);
        }
    }
    let world = AffineTransform3D::from_translation([
        -f64::from(lo[0] + hi[0]) / 2.0,
        -f64::from(lo[1] + hi[1]) / 2.0,
        -f64::from(lo[2]),
    ]);
    let (w, h) = (223.642, 126.976);
    let plate = Plate {
        min: [-w / 2.0, -h / 2.0],
        max: [w / 2.0, h / 2.0],
        floor: 0.0,
        keep_out: bread_arrange::KeepOut::default(),
    };
    let one_raft = std::env::var_os("BREADSLICER_EDGE_ONE_RAFT").is_some();
    let options = sc_geom::supports::SupportOptions {
        zones: Some(sc_geom::supports::zone_presets::minis_30()),
        one_raft,
        ..Default::default()
    };
    let request = sc_geom::supports::SupportRequest {
        layer_height_mm: 0.05,
        pixel_pitch_mm: Some([0.016_8, 0.024_8]),
        options: options.clone(),
        ..Default::default()
    };
    let ctx = crate::support_forecast::ForecastCtx {
        lift_to_mm: Some(crate::raft_clash::support_lift_mm(&options, 0.05)),
        request: request.clone(),
        raft_top_mm: if one_raft {
            options.raft_top_mm().unwrap_or(0.0)
        } else {
            0.0
        },
    };
    let mut bodies = vec![bread_arrange::Body {
        parts: vec![bread_arrange::Part {
            mesh: mesh.clone(),
            world,
        }],
        fixed: false,
    }];
    crate::support_forecast::with_forecasts(&mut bodies, &[true], &ctx, &AtomicBool::new(false))
        .expect("a forecast");
    let s = ArrangeOptions {
        one_raft,
        raft_top_mm: ctx.raft_top_mm as f32,
        ..ArrangeOptions::default()
    }
    .settings();
    let filled = bread_arrange::fill(
        &bodies,
        &[0],
        &plate,
        &s,
        None,
        &std::sync::atomic::AtomicUsize::new(0),
        &AtomicBool::new(false),
    )
    .unwrap();
    // The original where it stood, then each copy.
    let mut at = vec![bodies[0].parts[0].world];
    at.extend(
        filled
            .copies
            .iter()
            .map(|c| c.placement.motion().compose(&bodies[0].parts[0].world)),
    );
    let past = |a: [f32; 3], b: [f32; 3]| {
        [
            plate.min[0] - f64::from(a[0]),
            f64::from(b[0]) - plate.max[0],
            plate.min[1] - f64::from(a[1]),
            f64::from(b[1]) - plate.max[1],
        ]
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max)
    };
    let bounds = |mesh: &IndexedMesh, t: &AffineTransform3D| {
        let (mut a, mut b) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in mesh.vertices() {
            let p = t.apply_to_vertex(*v).coords();
            for k in 0..3 {
                a[k] = a[k].min(p[k]);
                b[k] = b[k].max(p[k]);
            }
        }
        (a, b)
    };
    let mut forecast_past = 0.0f64;
    if let Some(sup) = bodies[0].parts.get(1) {
        for t in &at {
            let motion = t.compose(&bodies[0].parts[0].world.inverse().unwrap());
            let (a, b) = bounds(&sup.mesh, &motion.compose(&sup.world));
            forecast_past = forecast_past.max(past(a, b));
        }
    }
    let mut scene = Scene::new();
    let rid = scene.add_mesh_resource_arc(mesh.clone());
    for t in &at {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(*t),
                LeafPayload::MeshResource(rid),
            )
            .unwrap();
        scene.add_root(leaf).unwrap();
    }
    let outcome = sc_geom::supports::start_supports(&scene, request)
        .unwrap()
        .join()
        .unwrap();
    let mut worst = f64::NEG_INFINITY;
    let mut out = 0;
    let mut drawn: Vec<Option<Arc<IndexedMesh>>> = Vec::new();
    for set in &outcome.sets {
        let (mesh, t) = match (&set.same_as, &set.mesh) {
            (Some(same), _) => (
                drawn[same.set].clone().unwrap(),
                match same.transform() {
                    Transform3D::Affine(a) => a,
                    Transform3D::Identity => AffineTransform3D::identity(),
                },
            ),
            (None, Some(m)) => (Arc::new(m.clone()), AffineTransform3D::identity()),
            (None, None) => {
                drawn.push(None);
                continue;
            }
        };
        drawn.push(set.same_as.is_none().then(|| mesh.clone()));
        let (a, b) = bounds(&mesh, &t);
        let p = past(a, b);
        worst = worst.max(p);
        if p > 0.01 {
            out += 1;
        }
    }
    let mut raft_past = f64::NEG_INFINITY;
    if one_raft {
        let sets: Vec<_> = outcome
            .sets
            .iter()
            .map(|set| (&set.session, AffineTransform3D::identity()))
            .collect();
        if let Some(raft) = sc_geom::supports::plate_raft(&sets) {
            let (a, b) = bounds(&raft, &AffineTransform3D::identity());
            raft_past = past(a, b);
        }
    }
    println!("one raft for all reaches {raft_past:.2} mm past the edge");
    println!(
        "{} copies; forecast reaches {forecast_past:.2} mm past the edge at most; generated: {out} sets past the edge, {worst:.2} mm at most",
        at.len()
    );
}

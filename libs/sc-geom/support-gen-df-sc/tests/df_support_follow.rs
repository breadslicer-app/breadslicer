//! Supports that move with their model in X/Y (#119) and turn with it about
//! the vertical axis (#153): the session taken along draws the same supports
//! where the model now is, saves and reopens on that scene, edits by hand
//! there. Each model has a set of its own (#197), which moves alone.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, NodeId, Scene};
use serde_json::Value;
use support_gen_df_sc::{
    DF_SUPPORT_DOCUMENT_VERSION, DfSupportRequest, DfSupportSession, model_index,
    scene_model_leaves, start_df_supports,
};

/// A closed 10 mm cube, outward-wound.
fn cube() -> IndexedMesh {
    let s = 10.0;
    let v = |x: f32, y: f32, z: f32| Vertex::new([x * s, y * s, z * s]);
    let vertices = vec![
        v(0.0, 0.0, 0.0),
        v(1.0, 0.0, 0.0),
        v(1.0, 1.0, 0.0),
        v(0.0, 1.0, 0.0),
        v(0.0, 0.0, 1.0),
        v(1.0, 0.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(0.0, 1.0, 1.0),
    ];
    let f = |a, b, c| IndexedTriangle::from_raw_indices([a, b, c]);
    let faces = vec![
        f(0, 2, 1),
        f(0, 3, 2),
        f(4, 5, 6),
        f(4, 6, 7),
        f(0, 1, 5),
        f(0, 5, 4),
        f(1, 2, 6),
        f(1, 6, 5),
        f(2, 3, 7),
        f(2, 7, 6),
        f(3, 0, 4),
        f(3, 4, 7),
    ];
    IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces)
}

fn scene_with(meshes: Vec<(IndexedMesh, [f64; 3])>) -> (Scene, Vec<NodeId>) {
    scene_placed(
        meshes
            .into_iter()
            .map(|(mesh, offset)| (mesh, AffineTransform3D::from_translation(offset)))
            .collect(),
    )
}

fn scene_placed(meshes: Vec<(IndexedMesh, AffineTransform3D)>) -> (Scene, Vec<NodeId>) {
    let mut g = Scene::new();
    let mut leaves = Vec::new();
    for (mesh, placed) in meshes {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(placed),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        leaves.push(leaf);
    }
    (g, leaves)
}

fn run_on(scene: &Scene) -> DfSupportSession {
    start_df_supports(scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap()
        .sets
        .remove(0)
        .session
}

/// Keys whose points are positions, and keys whose points are directions.
const MOVED: [&str; 4] = ["pos", "rootPos", "controlPoint1", "controlPoint2"];
const TURNED: [&str; 5] = [
    "normal",
    "surfaceNormal",
    "coneAxis",
    "startTangent",
    "endTangent",
];

/// A motion of the plate plane: a turn by `angle` radians about the Z axis,
/// then a move by `d`.
#[derive(Clone, Copy)]
struct Motion {
    angle: f64,
    d: [f64; 2],
}

impl Motion {
    fn shift(d: [f64; 2]) -> Self {
        Motion { angle: 0.0, d }
    }

    fn direction(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let (s, c) = self.angle.sin_cos();
        [c * x - s * y, s * x + c * y, z]
    }

    fn point(&self, p: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = self.direction(p);
        [x + self.d[0], y + self.d[1], z]
    }

    /// The turn applied after the rotation `q` (`x, y, z, w`).
    fn rotation(&self, [x, y, z, w]: [f64; 4]) -> [f64; 4] {
        let (s, c) = (self.angle / 2.0).sin_cos();
        [c * x - s * y, c * y + s * x, c * z + s * w, c * w - s * z]
    }
}

fn near<const N: usize>(a: [f64; N], b: [f64; N]) -> bool {
    (0..N).all(|k| (a[k] - b[k]).abs() < 1e-9)
}

/// How far the point of `from` farthest from all of `to` is from them.
fn farthest(from: &[[f64; 3]], to: &[[f64; 3]]) -> f64 {
    from.iter()
        .map(|a| {
            to.iter()
                .map(|b| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>())
                .fold(f64::MAX, f64::min)
                .sqrt()
        })
        .fold(0.0, f64::max)
}

/// Walks `before` and `after` together: every `{x, y, z}` point under a
/// position key went where `m` takes it, every one under a direction key
/// only turned, a root's rotation turned, and any other point did one or
/// stayed. Z never changes. Returns how many points moved.
fn check_points(key: &str, before: &Value, after: &Value, m: Motion) -> usize {
    let mut moved = 0;
    match (before, after) {
        (Value::Object(b), Value::Object(a)) => {
            let xyz = |o: &serde_json::Map<String, Value>| {
                Some([
                    o.get("x")?.as_f64()?,
                    o.get("y")?.as_f64()?,
                    o.get("z")?.as_f64()?,
                ])
            };
            let mut point = false;
            if let (Some(p0), Some(p1)) = (xyz(b), xyz(a)) {
                point = true;
                if key == "rot" {
                    let w = |o: &serde_json::Map<String, Value>| o["w"].as_f64().unwrap();
                    let (q0, q1) = ([p0[0], p0[1], p0[2], w(b)], [p1[0], p1[1], p1[2], w(a)]);
                    assert!(
                        near(q1, m.rotation(q0)),
                        "rot did not turn: {before} -> {after}"
                    );
                } else {
                    let went = near(p1, m.point(p0));
                    let same = p1 == p0;
                    assert_eq!(p1[2], p0[2], "{key}: a motion in the plane keeps Z");
                    if MOVED.contains(&key) {
                        assert!(went, "{key} did not move: {before} -> {after}");
                    } else if TURNED.contains(&key) {
                        assert!(
                            near(p1, m.direction(p0)),
                            "{key} is a direction and did not just turn: {before} -> {after}"
                        );
                    } else {
                        assert!(went || same, "{key}: {before} -> {after}");
                    }
                    moved += usize::from(went && !same);
                }
            }
            assert_eq!(b.len(), a.len(), "{key}: the same fields");
            for (k, v) in b {
                if !(point && ["x", "y", "z", "w"].contains(&k.as_str())) {
                    moved += check_points(k, v, &a[k], m);
                }
            }
        }
        (Value::Array(b), Value::Array(a)) => {
            assert_eq!(b.len(), a.len(), "{key}: the same items");
            for (b, a) in b.iter().zip(a) {
                moved += check_points(key, b, a, m);
            }
        }
        (b, a) => assert_eq!(b, a, "{key}"),
    }
    moved
}

#[test]
fn supports_move_with_their_model_in_the_plane() {
    let (scene, _) = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let session = run_on(&scene);
    assert!(session.supports_only_on(0));
    let mesh = session.mesh().unwrap().expect("a support mesh");
    let d = [12.5, -7.25];
    let moved = session.moved_in_plane(0, d);

    // Every position moves, no direction does, and nothing is placed again.
    let before = serde_json::to_value(session.state()).unwrap();
    let after = serde_json::to_value(moved.state()).unwrap();
    assert!(check_points("state", &before, &after, Motion::shift(d)) > 0);

    // The same supports, drawn where the model now is.
    let drawn = moved.mesh().unwrap().expect("a support mesh");
    assert_eq!(drawn.faces(), mesh.faces());
    assert_eq!(drawn.vertices().len(), mesh.vertices().len());
    for (a, b) in drawn.vertices().iter().zip(mesh.vertices()) {
        let (a, b) = (a.as_array(), b.as_array());
        assert!(
            (a[0] - b[0] - d[0] as f32).abs() < 1e-3
                && (a[1] - b[1] - d[1] as f32).abs() < 1e-3
                && a[2] == b[2],
            "{a:?} is not {b:?} moved"
        );
    }

    // It fits the moved scene, keeps as data and reopens there.
    let (there, _) = scene_with(vec![(cube(), [12.5, -7.25, 8.0])]);
    assert!(moved.fits_models_of(&there));
    assert!(!moved.fits_models_of(&scene));
    let document = moved.to_document().expect("a run from options is data");
    let mut reopened =
        DfSupportSession::from_document(&there, DF_SUPPORT_DOCUMENT_VERSION, &document)
            .expect("the moved models");
    assert_eq!(reopened.state(), moved.state());
    let again = reopened.mesh().unwrap().expect("a support mesh");
    assert_eq!(again.faces(), drawn.faces());

    // Hand edits go on where the model now is.
    let mut edited = moved.clone();
    edited
        .add_trunk_at([1.5 + d[0], 1.5 + d[1], 8.0])
        .expect("a way to hold it");
    reopened
        .remove_near([1.5 + d[0], 1.5 + d[1], 7.9])
        .expect("a support there");
}

/// #153: a model turned about the vertical axis still stands on the plate
/// the way it did, so its supports turn with it instead of being made again.
#[test]
fn supports_turn_with_their_model_about_the_vertical_axis() {
    let (scene, _) = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let session = run_on(&scene);
    let mesh = session.mesh().unwrap().expect("a support mesh");
    // 30 degrees about the cube's centre line, then 12 mm along X.
    let angle = 30f64.to_radians();
    let (s, c) = angle.sin_cos();
    let d = [5.0 - (c * 5.0 - s * 5.0) + 12.0, 5.0 - (s * 5.0 + c * 5.0)];
    let m = Motion { angle, d };
    let placed = AffineTransform3D::from_linear_and_translation(
        [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
        m.point([0.0, 0.0, 8.0]),
    );
    let (there, _) = scene_placed(vec![(cube(), placed)]);
    let (_, now) = scene_model_leaves(&there, None).unwrap().remove(0);
    let turned = session.turned_in_plane(0, now, angle, d);

    // Every position turns and moves, every direction turns, nothing is
    // placed again.
    let before = serde_json::to_value(session.state()).unwrap();
    let after = serde_json::to_value(turned.state()).unwrap();
    assert!(check_points("state", &before, &after, m) > 0);

    // Drawn where the model now is: the same supports, turned. Each round
    // piece is tessellated from a fixed world angle, so its ring vertices
    // slide along their circle (0.13 mm on this cube's), but no further.
    let drawn = turned.mesh().unwrap().expect("a support mesh");
    assert_eq!(drawn.faces().len(), mesh.faces().len());
    assert_eq!(drawn.vertices().len(), mesh.vertices().len());
    let points = |mesh: &IndexedMesh| -> Vec<[f64; 3]> {
        mesh.vertices()
            .iter()
            .map(|v| v.as_array().map(f64::from))
            .collect()
    };
    let drawn = points(&drawn);
    let expected: Vec<[f64; 3]> = points(&mesh).into_iter().map(|p| m.point(p)).collect();
    let off = farthest(&drawn, &expected).max(farthest(&expected, &drawn));
    assert!(off < 0.25, "a vertex is {off} mm off the turned supports");

    // It fits the turned scene, keeps as data and reopens there.
    assert!(turned.fits_models_of(&there));
    assert!(!turned.fits_models_of(&scene));
    let document = turned.to_document().expect("a run from options is data");
    let mut reopened =
        DfSupportSession::from_document(&there, DF_SUPPORT_DOCUMENT_VERSION, &document)
            .expect("the turned model");
    assert_eq!(reopened.state(), turned.state());

    // Hand edits go on where the model now is.
    let mut edited = turned.clone();
    edited
        .add_trunk_at(m.point([1.5, 1.5, 8.0]))
        .expect("a way to hold it");
    reopened
        .remove_near(m.point([1.5, 1.5, 7.9]))
        .expect("a support there");
}

/// A run made again would not keep hand edits (#153): a session tells
/// whether it has any, through a move and, for hand additions, a reopen.
/// The graph as data (#432): every piece with a line, and the contacts it
/// has where the model is, the tip going into the model past the surface.
#[test]
fn every_piece_tells_its_line_and_where_it_touches_the_model() {
    let (scene, _) = scene_with(vec![(cube(), [0.0, 0.0, 5.0])]);
    let session = run_on(&scene);
    let pieces = session.pieces();
    assert!(!pieces.is_empty());
    let contacts: Vec<_> = pieces.iter().flat_map(|p| &p.contacts).collect();
    assert_eq!(contacts.len(), session.tips().len());
    for contact in &contacts {
        // The cube's underside.
        assert!((contact.surface[2] - 5.0).abs() < 0.5, "{contact:?}");
        assert!(contact.tip[2] >= contact.surface[2] - 1e-9, "{contact:?}");
        assert!(contact.socket[2] < contact.surface[2], "{contact:?}");
        assert!(contact.tip_radius_mm > 0.0 && contact.socket_radius_mm > 0.0);
        let length: f64 = contact.axis.iter().map(|c| c * c).sum();
        assert!((length - 1.0).abs() < 1e-9, "{contact:?}");
    }
    for piece in &pieces {
        assert!(!piece.line.is_empty(), "{piece:?}");
    }
}

#[test]
fn a_session_knows_it_was_edited_by_hand() {
    let (scene, _) = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let session = run_on(&scene);
    assert!(!session.hand_edited());
    assert!(!session.moved_in_plane(0, [5.0, 0.0]).hand_edited());

    let mut added = session.clone();
    added
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    assert!(added.hand_edited());
    assert!(added.moved_in_plane(0, [5.0, 0.0]).hand_edited());
    let reopen = |s: &DfSupportSession| {
        let document = s.to_document().expect("a run from options is data");
        DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION, &document)
            .expect("the same models")
    };
    assert!(reopen(&added).hand_edited());

    let mut removed = reopen(&session);
    assert!(!removed.hand_edited());
    removed
        .remove_near([1.5, 1.5, 7.9])
        .expect("a support there");
    assert!(removed.hand_edited());
}

#[test]
fn each_model_gets_a_set_of_its_own_that_moves_alone() {
    let (scene, leaves) = scene_with(vec![(cube(), [0.0, 0.0, 8.0]), (cube(), [40.0, 0.0, 8.0])]);
    let outcome = start_df_supports(&scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(outcome.sets.len(), 2);
    let mut roots = 0;
    for (set, (leaf, (lo, hi))) in outcome
        .sets
        .iter()
        .zip(leaves.iter().zip([(-20.0, 20.0), (20.0, 60.0)]))
    {
        assert_eq!(set.model, *leaf);
        assert_eq!(set.session.model_count(), 1);
        assert!(set.session.supports_only_on(0));
        roots += set.session.state().collections.roots.len();
        // Its supports and raft stand under its own model only.
        let mesh = set.mesh.as_ref().expect("supports for a lifted cube");
        assert!(
            mesh.vertices()
                .iter()
                .all(|v| (lo..hi).contains(&v.as_array()[0])),
            "set of {leaf:?} reaches past its model"
        );
    }
    assert_eq!(roots, outcome.support.collections.roots.len());
    // The second set moves; the first, a session of its own, does not care.
    let moved = outcome.sets[1].session.moved_in_plane(0, [0.0, 25.0]);
    let (there, _) = scene_with(vec![(cube(), [0.0, 0.0, 8.0]), (cube(), [40.0, 25.0, 8.0])]);
    assert!(moved.fits_models_of(&there));
    assert!(outcome.sets[0].session.fits_models_of(&there));
    assert!(!outcome.sets[1].session.fits_models_of(&there));
}

#[test]
fn a_run_supports_only_the_models_it_is_asked_for() {
    let (scene, leaves) = scene_with(vec![(cube(), [0.0, 0.0, 8.0]), (cube(), [40.0, 0.0, 8.0])]);
    let request = DfSupportRequest {
        models: Some(vec![leaves[1]]),
        ..Default::default()
    };
    let outcome = start_df_supports(&scene, request).unwrap().join().unwrap();
    assert_eq!(outcome.models, 1);
    assert_eq!(outcome.sets.len(), 1);
    assert_eq!(outcome.sets[0].model, leaves[1]);
    let mesh = outcome.sets[0].mesh.as_ref().expect("supports");
    assert!(mesh.vertices().iter().all(|v| v.as_array()[0] > 20.0));
}

#[test]
fn a_model_is_known_by_its_place_among_the_models() {
    let (scene, leaves) = scene_with(vec![(cube(), [0.0, 0.0, 8.0]), (cube(), [40.0, 0.0, 8.0])]);
    let mesh = run_on(&scene).mesh().unwrap().expect("a support mesh");
    // The support leaf first: it is not a model, and does not count.
    let (with_supports, leaves_there) = scene_with(vec![
        (mesh, [0.0, 0.0, 0.0]),
        (cube(), [0.0, 0.0, 8.0]),
        (cube(), [40.0, 0.0, 8.0]),
    ]);
    assert_eq!(model_index(&scene, leaves[0]), Some(0));
    assert_eq!(model_index(&scene, leaves[1]), Some(1));
    assert_eq!(model_index(&with_supports, leaves_there[0]), None);
    assert_eq!(model_index(&with_supports, leaves_there[1]), Some(0));
    assert_eq!(model_index(&with_supports, leaves_there[2]), Some(1));
}

/// Moving a real model's supports, timed, with every kind of support the run
/// placed checked point by point: `SC_STRESS_STL=... cargo test --release
/// -- --ignored --nocapture moving_a_real_model_s_supports`.
#[test]
#[ignore = "needs SC_STRESS_STL"]
fn moving_a_real_model_s_supports() {
    let Ok(path) = std::env::var("SC_STRESS_STL") else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let (loaded, _) = sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
    let leaf = loaded
        .roots()
        .iter()
        .copied()
        .find(|id| loaded.leaf_mesh_arc(*id).is_some())
        .expect("an STL loads as one leaf root");
    let mesh = (**loaded.leaf_mesh_arc(leaf).unwrap()).clone();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in mesh.vertices() {
        for k in 0..3 {
            lo[k] = lo[k].min(v.as_array()[k]);
            hi[k] = hi[k].max(v.as_array()[k]);
        }
    }
    let shift = [
        60.0 - f64::from(lo[0] + hi[0]) / 2.0,
        40.0 - f64::from(lo[1] + hi[1]) / 2.0,
        5.0 - f64::from(lo[2]),
    ];
    let (scene, _) = scene_with(vec![(mesh, shift)]);
    let t = std::time::Instant::now();
    let session = run_on(&scene);
    println!("generate {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let c = &session.state().collections;
    println!(
        "roots {} trunks {} branches {} leaves {} twigs {} sticks {} braces {} stumps {} kickstands {} knots {}",
        c.roots.len(),
        c.trunks.len(),
        c.branches.len(),
        c.leaves.len(),
        c.twigs.len(),
        c.sticks.len(),
        c.braces.len(),
        c.stumps.len(),
        c.kickstands.len(),
        c.knots.len()
    );
    let t = std::time::Instant::now();
    let mesh = session.mesh().unwrap().expect("a support mesh");
    println!(
        "draw {:.0} ms ({} triangles)",
        t.elapsed().as_secs_f64() * 1000.0,
        mesh.faces().len()
    );
    let d = [-18.5, 11.25];
    let t = std::time::Instant::now();
    let moved = session.moved_in_plane(0, d);
    let shifted = t.elapsed();
    let drawn = moved.mesh().unwrap().expect("a support mesh");
    println!(
        "move {:.1} ms, redraw {:.0} ms",
        shifted.as_secs_f64() * 1000.0,
        (t.elapsed() - shifted).as_secs_f64() * 1000.0
    );
    let before = serde_json::to_value(session.state()).unwrap();
    let after = serde_json::to_value(moved.state()).unwrap();
    println!(
        "{} points moved",
        check_points("state", &before, &after, Motion::shift(d))
    );
    assert_eq!(drawn.faces().len(), mesh.faces().len());
    let off = drawn
        .vertices()
        .iter()
        .zip(mesh.vertices())
        .map(|(a, b)| {
            let (a, b) = (a.as_array(), b.as_array());
            (a[0] - b[0] - d[0] as f32)
                .abs()
                .max((a[1] - b[1] - d[1] as f32).abs())
                .max((a[2] - b[2]).abs())
        })
        .fold(0.0f32, f32::max);
    println!("largest vertex off its moved place: {off} mm");
    assert!(off < 1e-3);
}

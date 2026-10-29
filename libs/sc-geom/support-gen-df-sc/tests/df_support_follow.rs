//! Supports that move with their model in X/Y (#119): the session moved in
//! the plane draws the same supports moved, saves and reopens on the moved
//! scene, edits by hand there. Each model has a set of its own (#197), which
//! moves alone.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, NodeId, Scene};
use serde_json::Value;
use support_gen_df_sc::{
    DF_SUPPORT_DOCUMENT_VERSION, DfSupportRequest, DfSupportSession, model_index, start_df_supports,
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
    let mut g = Scene::new();
    let mut leaves = Vec::new();
    for (mesh, offset) in meshes {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(offset)),
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
const KEPT: [&str; 6] = [
    "normal",
    "surfaceNormal",
    "coneAxis",
    "startTangent",
    "endTangent",
    "rot",
];

/// Walks `before` and `after` together: every `{x, y, z}` point under a
/// position key moved by `d`, every one under a direction key did not, and
/// any other point did one or the other. Returns how many points moved.
fn check_points(key: &str, before: &Value, after: &Value, d: [f64; 2]) -> usize {
    let mut moved = 0;
    match (before, after) {
        (Value::Object(b), Value::Object(a)) => {
            let mut point = false;
            if let (Some(x0), Some(y0), Some(z0), Some(x1), Some(y1), Some(z1)) = (
                b.get("x").and_then(Value::as_f64),
                b.get("y").and_then(Value::as_f64),
                b.get("z").and_then(Value::as_f64),
                a.get("x").and_then(Value::as_f64),
                a.get("y").and_then(Value::as_f64),
                a.get("z").and_then(Value::as_f64),
            ) {
                let shifted = (x1 - x0 - d[0]).abs() < 1e-9 && (y1 - y0 - d[1]).abs() < 1e-9;
                let same = x1 == x0 && y1 == y0;
                assert_eq!(z1, z0, "{key}: a move in the plane keeps Z");
                if MOVED.contains(&key) {
                    assert!(shifted, "{key} did not move: {before} -> {after}");
                } else if KEPT.contains(&key) {
                    assert!(same, "{key} is a direction and moved: {before} -> {after}");
                } else {
                    assert!(shifted || same, "{key}: {before} -> {after}");
                }
                moved += usize::from(shifted && !same);
                point = true;
            }
            assert_eq!(b.len(), a.len(), "{key}: the same fields");
            for (k, v) in b {
                if !(point && ["x", "y", "z"].contains(&k.as_str())) {
                    moved += check_points(k, v, &a[k], d);
                }
            }
        }
        (Value::Array(b), Value::Array(a)) => {
            assert_eq!(b.len(), a.len(), "{key}: the same items");
            for (b, a) in b.iter().zip(a) {
                moved += check_points(key, b, a, d);
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
    assert!(check_points("state", &before, &after, d) > 0);

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

/// A run made again would not keep hand edits (#153): a session tells
/// whether it has any, through a move and, for hand additions, a reopen.
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
    println!("{} points moved", check_points("state", &before, &after, d));
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

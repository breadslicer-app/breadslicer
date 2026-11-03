//! The DragonFruit support run on a SoulCrafted scene: a lifted cube gets
//! supports and a raft as one named, welded mesh; a scene that already holds
//! that mesh is not supported again; cancelling stops the job.

use std::sync::Arc;

use sc_geom_transport_inprocess::JobError;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, Scene};
use support_gen_df_sc::{
    DF_SUPPORT_MESH_NAME, DfSupportRequest, scene_models, start_df_prescan, start_df_supports,
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

fn scene_with(meshes: Vec<(IndexedMesh, [f64; 3])>) -> Scene {
    let mut g = Scene::new();
    for (mesh, offset) in meshes {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(offset)),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
    }
    g
}

#[test]
fn a_lifted_cube_gets_supports_and_a_raft() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let handle = start_df_supports(&scene, DfSupportRequest::default()).unwrap();
    let outcome = handle.join().unwrap();
    assert_eq!(outcome.models, 1);
    assert!(outcome.islands > 0, "the cube's underside is an island");
    assert!(
        !outcome.support.collections.roots.is_empty(),
        "supports reach the plate"
    );
    let mesh = outcome.merged_mesh().expect("a support mesh");
    assert_eq!(
        mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
        Some(DF_SUPPORT_MESH_NAME)
    );
    assert_eq!(outcome.triangles, mesh.faces().len());
    // The raft sits on the plate; the supports reach up to the cube.
    let zs: Vec<f32> = mesh.vertices().iter().map(|v| v.as_array()[2]).collect();
    let (lo, hi) = zs
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &z| {
            (a.min(z), b.max(z))
        });
    // On the plate, not a rounding error below it (the viewport reads any
    // vertex under Z = 0 as breaching the build volume).
    assert_eq!(lo, 0.0, "lowest point on the plate");
    assert!(zs.iter().all(|z| *z >= 0.0 && z.is_sign_positive()));
    assert!(hi > 7.0, "supports reach the cube ({hi})");

    // Edge-manifold, as the slicer requires: every edge (by vertex index)
    // is shared by exactly two triangles, although pieces overlap.
    let mut edges: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for f in mesh.faces() {
        let v = f.vertices.map(u32::from);
        for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let bad = edges.values().filter(|&&c| c != 2).count();
    assert_eq!(bad, 0, "{bad} edges not shared by exactly two triangles");

    // Pasted back, the support mesh is not a model of the next run.
    let again = scene_with(vec![(cube(), [0.0, 0.0, 8.0]), (mesh, [0.0, 0.0, 0.0])]);
    assert_eq!(scene_models(&again).unwrap().len(), 1);
}

#[test]
fn cancelling_stops_the_run() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let handle = start_df_supports(&scene, DfSupportRequest::default()).unwrap();
    // The run takes seconds; its first stage report sees the flag and the
    // next boundary stops it.
    handle.cancel();
    assert!(matches!(handle.join(), Err(JobError::Cancelled)));
}

/// The block-8a meshes (tables, knots, spheres, ripples...) placed in one
/// scene: the support mesh stays edge-manifold however its pieces overlap.
#[test]
fn support_meshes_of_real_scenes_are_edge_manifold() {
    let path = format!(
        "{}/../support-gen-df/tests/fixtures/df_native_islands.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let meshes = fixture["meshes"].as_object().unwrap();
    let mut checked = 0;
    for (name, soup) in meshes {
        let soup: Vec<f64> = serde_json::from_value(soup.clone()).unwrap();
        let vertices: Vec<Vertex> = soup
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| Vertex::new([p[0] as f32, p[1] as f32, p[2] as f32]))
            .collect();
        let faces: Vec<IndexedTriangle> = (0..vertices.len() as u32 / 3)
            .map(|t| IndexedTriangle::from_raw_indices([3 * t, 3 * t + 1, 3 * t + 2]))
            .collect();
        let scene = scene_with(vec![(
            IndexedMesh::new(None, vertices, faces),
            [0.0, 0.0, 0.0],
        )]);
        let outcome = start_df_supports(&scene, DfSupportRequest::default())
            .unwrap()
            .join()
            .unwrap();
        let Some(mesh) = outcome.merged_mesh() else {
            continue;
        };
        let mut edges: std::collections::HashMap<(u32, u32), u32> = Default::default();
        for f in mesh.faces() {
            let v = f.vertices.map(u32::from);
            for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let bad = edges.values().filter(|&&c| c != 2).count();
        assert_eq!(
            bad, 0,
            "{name}: {bad} edges not shared by exactly two triangles"
        );
        // The slicer reads an inward-facing shell as a void (a sealed
        // cavity): every support shell must face outward.
        let inward = shell_volumes(&mesh).iter().filter(|&&v| v <= 0.0).count();
        assert_eq!(inward, 0, "{name}: {inward} support shells face inward");
        checked += 1;
    }
    assert!(checked >= 8, "{checked} scenes supported");
}

#[test]
fn a_prescan_is_reused_and_gives_the_same_supports() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0]), (cube(), [25.0, 0.0, 6.0])]);
    let plain = start_df_supports(&scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap();
    let prescan = start_df_prescan(&scene, DfSupportRequest::default())
        .join()
        .unwrap();
    assert_eq!(prescan.models, 2);
    let request = DfSupportRequest {
        prescanned: Some(prescan.scans),
        ..Default::default()
    };
    let reused = start_df_supports(&scene, request).unwrap().join().unwrap();
    let prescanned = reused
        .timings
        .entries
        .iter()
        .find(|e| e.label == "scan:prescanned")
        .map(|e| e.count);
    assert_eq!(
        prescanned,
        Some(2),
        "both models were served from the prescan"
    );
    let (a, b) = (plain.merged_mesh().unwrap(), reused.merged_mesh().unwrap());
    assert_eq!(a.vertices(), b.vertices());
    assert_eq!(a.faces(), b.faces());
}

#[test]
fn a_prescan_can_be_cancelled() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let handle = start_df_prescan(&scene, DfSupportRequest::default());
    handle.cancel();
    match handle.join() {
        Err(JobError::Cancelled) | Ok(_) => {}
        Err(other) => panic!("unexpected {other:?}"),
    }
}

/// Signed volume (times 6) of every vertex-connected shell.
fn shell_volumes(mesh: &IndexedMesh) -> Vec<f64> {
    let n = mesh.vertices().len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut v: usize) -> usize {
        while parent[v] != v {
            parent[v] = parent[parent[v]];
            v = parent[v];
        }
        v
    }
    for f in mesh.faces() {
        let [a, b, c] = f.vertices.map(|v| u32::from(v) as usize);
        for (x, y) in [(a, b), (b, c)] {
            let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
            parent[rx] = ry;
        }
    }
    let mut volumes: std::collections::HashMap<usize, f64> = Default::default();
    for f in mesh.faces() {
        let [a, b, c] = f.vertices.map(|v| {
            mesh.vertices()[u32::from(v) as usize]
                .coords()
                .map(f64::from)
        });
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        *volumes
            .entry(root(&mut parent, u32::from(f.vertices[0]) as usize))
            .or_default() += det;
    }
    volumes.into_values().collect()
}

fn bad_edges(mesh: &IndexedMesh) -> usize {
    let mut edges: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for f in mesh.faces() {
        let v = f.vertices.map(u32::from);
        for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    edges.values().filter(|&&c| c != 2).count()
}

#[test]
fn supports_are_added_and_removed_by_hand() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let outcome = start_df_supports(&scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap();
    let mut session = outcome.sets.into_iter().next().unwrap().session;
    let count = |s: &support_gen_df_sc::DfSupportSession| {
        let c = &s.state().collections;
        c.roots.len() + c.trunks.len() + c.branches.len() + c.leaves.len() + c.knots.len()
    };
    let before = count(&session);

    // The preview of a click on the underside (z = 8) shows what it adds,
    // and leaves the session as it was.
    let preview = session.preview_add_at([1.5, 1.5, 8.0]).expect("a preview");
    assert!(preview.valid, "the underside can be held");
    assert!(!preview.mesh.faces().is_empty());
    assert_eq!(bad_edges(&preview.mesh), 0);
    assert_eq!(count(&session), before);

    // The click itself adds it (its own trunk, or a member on a neighbour).
    session
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    assert!(count(&session) > before);
    let mesh = session.mesh().unwrap().expect("a support mesh");
    assert_eq!(bad_edges(&mesh), 0, "still edge-manifold");

    // A click at its tip previews, then takes away, what was added.
    let removal = session
        .preview_remove_at([1.5, 1.5, 7.9])
        .expect("a support there");
    assert!(!removal.valid && !removal.mesh.faces().is_empty());
    session
        .remove_near([1.5, 1.5, 7.9])
        .expect("a support there");
    assert_eq!(count(&session), before, "nothing left behind");

    // Far from everything, nothing happens.
    assert!(session.remove_near([500.0, 500.0, 500.0]).is_err());
    assert!(session.add_trunk_at([500.0, 500.0, 500.0]).is_err());
    assert!(session.preview_add_at([500.0, 500.0, 500.0]).is_err());
}

/// Measurement, not a gate: on the real fixture scenes, how many clicks on
/// overhangs a hand edit can hold and how long a preview takes (the hover
/// budget). `cargo test --release -- --ignored --nocapture manual_edit_on`.
#[test]
#[ignore = "measurement"]
fn manual_edit_on_real_scenes() {
    let path = format!(
        "{}/../support-gen-df/tests/fixtures/df_native_islands.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (name, soup) in fixture["meshes"].as_object().unwrap() {
        let soup: Vec<f64> = serde_json::from_value(soup.clone()).unwrap();
        let vertices: Vec<Vertex> = soup
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| Vertex::new([p[0] as f32, p[1] as f32, p[2] as f32]))
            .collect();
        let faces: Vec<IndexedTriangle> = (0..vertices.len() as u32 / 3)
            .map(|t| IndexedTriangle::from_raw_indices([3 * t, 3 * t + 1, 3 * t + 2]))
            .collect();
        // Downward faces above the plate: where a click would want support.
        let points: Vec<[f64; 3]> = soup
            .as_chunks::<9>()
            .0
            .iter()
            .filter_map(|t| {
                let a = [t[0], t[1], t[2]];
                let b = [t[3], t[4], t[5]];
                let c = [t[6], t[7], t[8]];
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let nz = u[0] * v[1] - u[1] * v[0];
                let len = ((u[1] * v[2] - u[2] * v[1]).powi(2)
                    + (u[2] * v[0] - u[0] * v[2]).powi(2)
                    + nz * nz)
                    .sqrt();
                let centre = [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ];
                (len > 1e-9 && nz / len < -0.7 && centre[2] > 2.0).then_some(centre)
            })
            .collect();
        if points.is_empty() {
            continue;
        }
        let step = (points.len() / 25).max(1);
        let scene = scene_with(vec![(
            IndexedMesh::new(None, vertices, faces),
            [0.0, 0.0, 0.0],
        )]);
        let session = start_df_supports(&scene, DfSupportRequest::default())
            .unwrap()
            .join()
            .unwrap()
            .sets
            .remove(0)
            .session;
        let (mut tried, mut held, mut slowest) = (0, 0, 0.0f64);
        let started = std::time::Instant::now();
        for p in points.iter().step_by(step).take(25) {
            tried += 1;
            let t = std::time::Instant::now();
            if session.preview_add_at(*p).is_ok_and(|v| v.valid) {
                held += 1;
            }
            slowest = slowest.max(t.elapsed().as_secs_f64());
        }
        println!(
            "{name}: {held}/{tried} overhang clicks held; preview mean {:.0} ms, slowest {:.0} ms",
            started.elapsed().as_secs_f64() * 1000.0 / tried as f64,
            slowest * 1000.0
        );
    }
}

/// Stress on a real model, not a gate: hover, click and remove on the
/// downward faces of `SC_STRESS_STL` (lifted 5 mm), catching panics and
/// timing each step. `SC_STRESS_STL=... cargo test --release -- --ignored
/// --nocapture hand_edits_on_a_real_model`.
#[test]
#[ignore = "needs SC_STRESS_STL"]
fn hand_edits_on_a_real_model() {
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
    let points: Vec<[f64; 3]> = mesh
        .faces()
        .iter()
        .filter_map(|f| {
            let [a, b, c] = f.vertices.map(|v| {
                mesh.vertices()[u32::from(v) as usize]
                    .as_array()
                    .map(f64::from)
            });
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (len > 1e-12 && n[2] / len < -0.5)
                .then(|| std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0 + shift[k]))
        })
        .collect();
    println!("{} faces, {} downward", mesh.faces().len(), points.len());
    let scene = scene_with(vec![(mesh, shift)]);
    let t = std::time::Instant::now();
    let mut session = start_df_supports(&scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap()
        .sets
        .remove(0)
        .session;
    println!("generate {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let step = (points.len() / 30).max(1);
    for p in points.iter().step_by(step).take(30) {
        let t = std::time::Instant::now();
        let preview =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.preview_add_at(*p)));
        let preview_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = std::time::Instant::now();
        let mut copy = session.clone();
        let added = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            copy.add_trunk_at(*p).and_then(|()| copy.mesh())
        }));
        let add_ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{p:?}: preview {preview_ms:.0} ms {:?}; click {add_ms:.0} ms {:?}",
            preview.as_ref().map(|r| r.as_ref().map(|v| v.valid)),
            added.as_ref().map(|r| r.as_ref().map(|m| m.is_some())),
        );
        if let Ok(Ok(Some(_))) = added {
            session = copy;
        }
    }
}

/// Measurement, not a gate (#3): what a hand click costs on a big real
/// scene. `SC_STRESS_STL` is the model (scaled to `SC_STRESS_HEIGHT_MM` tall
/// when set, lifted 5 mm); for clicks on its downward faces it times the
/// hover preview (a worker computes it) and, apart, drawing the whole set
/// again once the click takes the previewed edit (`mesh`, the slow part of
/// a click, made off the UI thread), plus a click with no preview ready. `cargo test
/// --release -p support-gen-df-sc --test df_supports_scene -- --ignored
/// --nocapture click_cost_on_a_real_model`.
#[test]
#[ignore = "needs SC_STRESS_STL"]
fn click_cost_on_a_real_model() {
    let Ok(path) = std::env::var("SC_STRESS_STL") else {
        return;
    };
    let height: Option<f32> = std::env::var("SC_STRESS_HEIGHT_MM")
        .ok()
        .and_then(|h| h.parse().ok());
    let bytes = std::fs::read(&path).unwrap();
    let (loaded, _) = sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
    let leaf = loaded
        .roots()
        .iter()
        .copied()
        .find(|id| loaded.leaf_mesh_arc(*id).is_some())
        .expect("an STL loads as one leaf root");
    let mut mesh = (**loaded.leaf_mesh_arc(leaf).unwrap()).clone();
    let bounds = |mesh: &IndexedMesh| {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in mesh.vertices() {
            for k in 0..3 {
                lo[k] = lo[k].min(v.as_array()[k]);
                hi[k] = hi[k].max(v.as_array()[k]);
            }
        }
        (lo, hi)
    };
    if let Some(h) = height {
        let (lo, hi) = bounds(&mesh);
        let s = h / (hi[2] - lo[2]);
        let vertices = mesh
            .vertices()
            .iter()
            .map(|v| Vertex::new(v.as_array().map(|c| c * s)))
            .collect();
        mesh = IndexedMesh::new(None, vertices, mesh.faces().to_vec());
    }
    let (lo, hi) = bounds(&mesh);
    let shift = [
        60.0 - f64::from(lo[0] + hi[0]) / 2.0,
        40.0 - f64::from(lo[1] + hi[1]) / 2.0,
        5.0 - f64::from(lo[2]),
    ];
    let points: Vec<[f64; 3]> = mesh
        .faces()
        .iter()
        .filter_map(|f| {
            let [a, b, c] = f.vertices.map(|v| {
                mesh.vertices()[u32::from(v) as usize]
                    .as_array()
                    .map(f64::from)
            });
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (len > 1e-12 && n[2] / len < -0.5)
                .then(|| std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0 + shift[k]))
        })
        .collect();
    let faces = mesh.faces().len();
    let scene = scene_with(vec![(mesh, shift)]);
    let t = std::time::Instant::now();
    let mut session = start_df_supports(&scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap()
        .sets
        .remove(0)
        .session;
    let generate_ms = t.elapsed().as_secs_f64() * 1000.0;
    let set_faces = session.mesh().unwrap().map_or(0, |m| m.faces().len());
    println!(
        "{faces} faces, {} downward; generate {generate_ms:.0} ms; {} roots, set mesh {set_faces} triangles",
        points.len(),
        session.state().collections.roots.len()
    );
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let (mut previews, mut draws, mut cold) = (Vec::new(), Vec::new(), Vec::new());
    let (mut attached, mut placed) = (0, 0);
    let step = (points.len() / 20).max(1);
    for p in points.iter().step_by(step).take(20) {
        let t = std::time::Instant::now();
        let Ok((preview, edited)) = session.preview_add(*p) else {
            continue;
        };
        previews.push(ms(t));
        let Some(edited) = edited else {
            continue;
        };
        placed += 1;
        attached += usize::from(preview.attached);
        // The click on the previewed point: the set drawn again.
        let t = std::time::Instant::now();
        let drawn = edited.mesh().unwrap();
        draws.push(ms(t));
        assert!(drawn.is_some());
        // A click before the preview came back: placed and drawn.
        let t = std::time::Instant::now();
        let mut copy = session.clone();
        copy.add_trunk_at(*p).unwrap();
        copy.mesh().unwrap();
        cold.push(ms(t));
        session = edited;
    }
    let stats = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
        format!(
            "mean {mean:.0} ms, median {:.0} ms, max {:.0} ms (n {})",
            v.get(v.len() / 2).copied().unwrap_or(0.0),
            v.last().copied().unwrap_or(0.0),
            v.len()
        )
    };
    println!("placed {placed}, hanging {attached}");
    println!("hover preview (worker): {}", stats(&previews));
    println!(
        "click, preview taken (the set drawn again): {}",
        stats(&draws)
    );
    println!("click, no preview (placed and drawn): {}", stats(&cold));
}

/// Measurement, not a gate (#3): what a hover preview costs where a fast
/// mouse goes, every kind of surface (faces that look down, walls, faces
/// that look up), with and without Shift. `SC_STRESS_STL` is the model
/// (scaled to `SC_STRESS_HEIGHT_MM` tall when set). `cargo test --release
/// -p support-gen-df-sc --test df_supports_scene -- --ignored --nocapture
/// hover_cost_over_every_surface`.
#[test]
#[ignore = "needs SC_STRESS_STL"]
fn hover_cost_over_every_surface() {
    let Ok(path) = std::env::var("SC_STRESS_STL") else {
        return;
    };
    let height: Option<f32> = std::env::var("SC_STRESS_HEIGHT_MM")
        .ok()
        .and_then(|h| h.parse().ok());
    let bytes = std::fs::read(&path).unwrap();
    let (loaded, _) = sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
    let leaf = loaded.roots()[0];
    let mut mesh = (**loaded.leaf_mesh_arc(leaf).unwrap()).clone();
    let bounds = |mesh: &IndexedMesh| {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in mesh.vertices() {
            for k in 0..3 {
                lo[k] = lo[k].min(v.as_array()[k]);
                hi[k] = hi[k].max(v.as_array()[k]);
            }
        }
        (lo, hi)
    };
    if let Some(h) = height {
        let (lo, hi) = bounds(&mesh);
        let s = h / (hi[2] - lo[2]);
        let vertices = mesh
            .vertices()
            .iter()
            .map(|v| Vertex::new(v.as_array().map(|c| c * s)))
            .collect();
        mesh = IndexedMesh::new(None, vertices, mesh.faces().to_vec());
    }
    let (lo, hi) = bounds(&mesh);
    let shift = [
        60.0 - f64::from(lo[0] + hi[0]) / 2.0,
        40.0 - f64::from(lo[1] + hi[1]) / 2.0,
        5.0 - f64::from(lo[2]),
    ];
    // Face centres, every so many faces, with how their normal leans.
    let step = (mesh.faces().len() / 120).max(1);
    let points: Vec<([f64; 3], f64)> = mesh
        .faces()
        .iter()
        .step_by(step)
        .filter_map(|f| {
            let [a, b, c] = f.vertices.map(|v| {
                mesh.vertices()[u32::from(v) as usize]
                    .as_array()
                    .map(f64::from)
            });
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (len > 1e-12).then(|| {
                (
                    std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0 + shift[k]),
                    n[2] / len,
                )
            })
        })
        .collect();
    let scene = scene_with(vec![(mesh, shift)]);
    let session = start_df_supports(&scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap()
        .sets
        .remove(0)
        .session;
    for attach in [false, true] {
        let how = support_gen_df_sc::AddOptions { attach, size: None };
        let mut by_kind: [Vec<f64>; 3] = Default::default();
        let mut outcomes = [0usize; 3];
        for (p, nz) in &points {
            let t = std::time::Instant::now();
            let preview = session.preview_add_with(*p, &how);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let kind = if *nz < -0.3 {
                0
            } else if *nz < 0.3 {
                1
            } else {
                2
            };
            by_kind[kind].push(ms);
            outcomes[match preview {
                Ok((v, _)) if v.valid => 0,
                Ok(_) => 1,
                Err(_) => 2,
            }] += 1;
        }
        let stats = |v: &[f64]| {
            let mut v = v.to_vec();
            v.sort_by(f64::total_cmp);
            let at = |q: f64| {
                v.get(((v.len() as f64 - 1.0) * q) as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            format!(
                "median {:.0} ms, p90 {:.0} ms, max {:.0} ms (n {})",
                at(0.5),
                at(0.9),
                at(1.0),
                v.len()
            )
        };
        println!(
            "attach {attach}: valid {} red {} none {}",
            outcomes[0], outcomes[1], outcomes[2]
        );
        for (name, v) in ["down", "wall", "up"].iter().zip(&by_kind) {
            println!("  {name}: {}", stats(v));
        }
        let all: Vec<f64> = by_kind.concat();
        println!("  all: {}", stats(&all));
    }
}

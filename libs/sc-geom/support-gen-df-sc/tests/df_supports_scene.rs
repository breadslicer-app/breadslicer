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
    let mesh = outcome.mesh.expect("a support mesh");
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
            .chunks_exact(3)
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
        let Some(mesh) = outcome.mesh else { continue };
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
    let (a, b) = (plain.mesh.unwrap(), reused.mesh.unwrap());
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
    let mut session = outcome.session;
    let roots = session.state().collections.roots.len();

    // A click on the underside of the cube (z = 8), near a corner.
    session
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a route to the plate");
    assert_eq!(session.state().collections.roots.len(), roots + 1);
    let mesh = session.mesh().unwrap().expect("a support mesh");
    assert_eq!(bad_edges(&mesh), 0, "still edge-manifold");

    // A click on the new support takes it away, with its root.
    let removed = session
        .remove_near([1.5, 1.5, 7.5])
        .expect("a support there");
    assert!(removed >= 2, "the trunk and its root ({removed})");
    assert_eq!(session.state().collections.roots.len(), roots);

    // Far from everything, nothing happens.
    assert!(session.remove_near([500.0, 500.0, 500.0]).is_err());
    assert!(session.add_trunk_at([500.0, 500.0, 500.0]).is_err());
}

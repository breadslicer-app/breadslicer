//! The DragonFruit support run on a SoulCrafted scene: a lifted cube gets
//! supports and a raft as one named, welded mesh; a scene that already holds
//! that mesh is not supported again; cancelling stops the job.

use std::sync::Arc;

use sc_geom_transport_inprocess::JobError;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, Scene};
use support_gen_df_sc::{DF_SUPPORT_MESH_NAME, DfSupportRequest, scene_models, start_df_supports};

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
        checked += 1;
    }
    assert!(checked >= 8, "{checked} scenes supported");
}

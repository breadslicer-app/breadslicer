//! A solid raft of another shape (#138, #99): pads under each support, holes
//! between them, peel tabs off its ends. Each draws a closed raft, one the
//! slicer takes, under every root.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, Scene};
use support_gen_df_sc::{
    DfSupportOptions, DfSupportRequest, DfSupportSession, RaftMode, start_df_supports,
};

/// A closed 40 mm block, 4 mm thick, outward-wound.
fn cube() -> IndexedMesh {
    let s = 40.0;
    let v = |x: f32, y: f32, z: f32| Vertex::new([x * s, y * s, z * 4.0]);
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

fn run_on(scene: &Scene) -> DfSupportSession {
    start_df_supports(scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap()
        .sets
        .remove(0)
        .session
}

#[test]
fn every_solid_raft_shape_draws_a_closed_raft() {
    // A block held 8 mm over the plate: its supports stand on a raft.
    let session = run_on(&scene_with(vec![(cube(), [0.0, 0.0, 8.0])]));
    let solid = DfSupportOptions {
        raft: RaftMode::Solid,
        ..DfSupportOptions::default()
    };
    let plain = session.with_raft_of(&solid).expect("a solid raft");
    let plain_mesh = plain.mesh().unwrap().expect("supports");
    assert!(plain_mesh.is_sliceable(), "the plain solid raft");
    let shapes = [
        ("pads", true, 0.0, false),
        ("holes", false, 1.0, false),
        ("tabs", false, 0.0, true),
        ("all", true, 1.0, true),
    ];
    for (name, pads, holes, tabs) in shapes {
        let options = DfSupportOptions {
            raft_pads: pads,
            raft_holes_mm: holes,
            raft_tabs: tabs,
            ..solid.clone()
        };
        let next = plain.with_raft_of(&options).expect("another raft");
        let mesh = next.mesh().unwrap().expect("supports");
        assert!(mesh.is_sliceable(), "{name}: a raft the slicer takes");
        assert_ne!(
            mesh.faces().len(),
            plain_mesh.faces().len(),
            "{name}: another raft"
        );
        // The supports themselves stay where they stood.
        assert_eq!(next.tips(), plain.tips(), "{name}");
    }
}

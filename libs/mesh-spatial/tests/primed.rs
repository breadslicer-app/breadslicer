/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Caches built ahead of time on a worker (#395): a slot that takes a mesh
//! through [`SceneSpatial::apply_edit_primed`] gets exactly the caches a
//! query would have built, and only for the very mesh they were built for.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::{PrimedMesh, SceneSpatial, TriangleClusters};
use sc_scene::node::LeafAttachment;
use sc_scene::{LeafPayload, MeshResourceId, NodeId, Scene};
use sc_scene_edit::SceneEdit;

/// A wavy grid of `n` x `n` quads, `2 n²` triangles, with a `seed`-shifted
/// height so two grids of the same `n` differ.
fn grid(n: u32, seed: f32) -> IndexedMesh {
    let mut vertices = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (i as f32, j as f32);
            let z = (x * 0.37 + seed).sin() * (y * 0.23 - seed).cos() * 3.0;
            vertices.push(Vertex::new([x, y, z]));
        }
    }
    let at = |i: u32, j: u32| j * (n + 1) + i;
    let mut faces = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            faces.push(IndexedTriangle::from_raw_indices([a, b, c]));
            faces.push(IndexedTriangle::from_raw_indices([a, c, d]));
        }
    }
    IndexedMesh::new(None, vertices, faces)
}

/// A scene with one model, its mesh a resource, primed as a load leaves it.
fn scene_with_a_model() -> SceneSpatial {
    let mut graph = Scene::new();
    let rid = graph.add_mesh_resource(grid(8, 0.0));
    let leaf = graph
        .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
        .expect("leaf");
    graph.add_root(leaf).expect("root");
    SceneSpatial::primed_graph(graph)
}

/// The edit an import makes: `mesh` as a fresh resource and a root leaf
/// showing it, moved by `x`.
fn import_edit(spatial: &mut SceneSpatial, mesh: &Arc<IndexedMesh>, x: f64) -> (SceneEdit, NodeId) {
    let rid: MeshResourceId = spatial.allocate_unbound_mesh_resource_id();
    let node_id = spatial.allocate_unbound_node_id();
    let position = spatial.graph().roots().len();
    let edit = SceneEdit::Compound(vec![
        SceneEdit::AddMeshResource {
            rid,
            mesh: Arc::clone(mesh),
        },
        SceneEdit::AddLeaf {
            node_id,
            attachment: LeafAttachment::Root { position },
            payload: LeafPayload::MeshResource(rid),
            local_transform: Transform3D::Affine(AffineTransform3D::from_translation([
                x, 0.0, 0.0,
            ])),
            meta: None,
        },
    ]);
    (edit, node_id)
}

fn clusters_of(spatial: &SceneSpatial, id: NodeId) -> &TriangleClusters {
    spatial
        .leaf(id)
        .expect("a live leaf")
        .clusters()
        .expect("the mesh has faces")
}

#[test]
fn primed_caches_are_exactly_the_ones_a_query_builds() {
    let mesh = Arc::new(grid(120, 1.5));
    let primed = PrimedMesh::new(Arc::clone(&mesh));
    assert_eq!(
        primed.clusters(),
        TriangleClusters::for_mesh(&mesh).as_ref(),
        "the primed clusters equal the ones built on demand"
    );

    // The same import, once primed and once not: the scenes answer alike.
    let mut ahead = scene_with_a_model();
    let (edit, id) = import_edit(&mut ahead, &mesh, 30.0);
    ahead
        .apply_edit_primed(edit, std::slice::from_ref(&primed))
        .expect("the import applies");
    let mut lazy = scene_with_a_model();
    let (edit, lazy_id) = import_edit(&mut lazy, &mesh, 30.0);
    lazy.apply_edit(edit).expect("the import applies");

    // The slot took the caches built ahead, not a fresh build.
    assert!(std::ptr::eq(
        clusters_of(&ahead, id),
        primed.clusters().unwrap()
    ));
    assert_eq!(clusters_of(&ahead, id), clusters_of(&lazy, lazy_id));
    let (a, l) = (ahead.leaf(id).unwrap(), lazy.leaf(lazy_id).unwrap());
    assert_eq!(a.local_aabb(), l.local_aabb());
    let turned = Transform3D::Affine(AffineTransform3D::from_rotation_z(0.7));
    assert_eq!(a.minimum_world_z(turned), l.minimum_world_z(turned));
    assert_eq!(a.transformed_aabb(turned), l.transformed_aabb(turned));
}

#[test]
fn caches_built_for_another_mesh_are_never_taken() {
    // Same content, another `Arc`: the slot builds its own.
    let mesh = Arc::new(grid(16, 0.4));
    let twin = PrimedMesh::new(Arc::new(grid(16, 0.4)));
    let mut spatial = scene_with_a_model();
    let (edit, id) = import_edit(&mut spatial, &mesh, 30.0);
    spatial
        .apply_edit_primed(edit, std::slice::from_ref(&twin))
        .expect("the import applies");
    assert!(!std::ptr::eq(
        clusters_of(&spatial, id),
        twin.clusters().unwrap()
    ));
    assert_eq!(
        Some(clusters_of(&spatial, id)),
        TriangleClusters::for_mesh(&mesh).as_ref()
    );
}

#[test]
fn a_mesh_replaced_after_its_import_drops_the_primed_caches() {
    let mesh = Arc::new(grid(16, 0.4));
    let primed = PrimedMesh::new(Arc::clone(&mesh));
    let mut spatial = scene_with_a_model();
    let (edit, id) = import_edit(&mut spatial, &mesh, 30.0);
    spatial
        .apply_edit_primed(edit, std::slice::from_ref(&primed))
        .expect("the import applies");
    let rid = match spatial.graph().node(id).map(|n| &n.kind) {
        Some(sc_scene::NodeKind::Leaf(LeafPayload::MeshResource(rid))) => *rid,
        _ => panic!("the import adds a resource leaf"),
    };

    // A repair, a hollow or an edit swaps the mesh: the old caches go,
    // even when the swap is applied with them at hand.
    let repaired = Arc::new(grid(16, 2.0));
    let swap = SceneEdit::ReplaceMeshResource {
        rid,
        before: Arc::clone(&mesh),
        after: Arc::clone(&repaired),
    };
    spatial
        .apply_edit_primed(swap.clone(), std::slice::from_ref(&primed))
        .expect("the swap applies");
    assert!(!std::ptr::eq(
        clusters_of(&spatial, id),
        primed.clusters().unwrap()
    ));
    assert_eq!(
        Some(clusters_of(&spatial, id)),
        TriangleClusters::for_mesh(&repaired).as_ref()
    );
    assert_ne!(
        Some(clusters_of(&spatial, id)),
        TriangleClusters::for_mesh(&mesh).as_ref()
    );

    // Undo puts the first mesh back; the caches it gets are of that mesh.
    spatial
        .apply_edit_primed(swap.inverse(), std::slice::from_ref(&primed))
        .expect("the undo applies");
    assert!(std::ptr::eq(
        clusters_of(&spatial, id),
        primed.clusters().unwrap()
    ));
    spatial.apply_edit(swap).expect("the redo applies");
    assert_eq!(
        Some(clusters_of(&spatial, id)),
        TriangleClusters::for_mesh(&repaired).as_ref()
    );
}

#[test]
fn a_swap_to_a_primed_mesh_takes_its_caches() {
    let mut graph = Scene::new();
    let before = Arc::new(grid(16, 0.4));
    let leaf = graph
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::clone(&before)),
        )
        .expect("leaf");
    graph.add_root(leaf).expect("root");
    let mut spatial = SceneSpatial::primed_graph(graph);
    let after = Arc::new(grid(16, 2.0));
    let primed = PrimedMesh::new(Arc::clone(&after));
    spatial
        .apply_edit_primed(
            SceneEdit::ReplaceLeafMesh {
                node_id: leaf,
                before,
                after: Arc::clone(&after),
            },
            std::slice::from_ref(&primed),
        )
        .expect("the swap applies");
    assert!(std::ptr::eq(
        clusters_of(&spatial, leaf),
        primed.clusters().unwrap()
    ));
}

#[test]
fn a_refused_edit_changes_nothing_even_with_primed_caches() {
    let mesh = Arc::new(grid(16, 0.4));
    let primed = PrimedMesh::new(Arc::clone(&mesh));
    let mut spatial = scene_with_a_model();
    let model = spatial.leaf_id_at_index(0).expect("the model");
    let kept: *const TriangleClusters = clusters_of(&spatial, model);
    let rid = spatial.allocate_unbound_mesh_resource_id();
    // Replacing an unbound slot is refused.
    let refused = SceneEdit::ReplaceMeshResource {
        rid,
        before: Arc::new(grid(4, 0.0)),
        after: Arc::clone(&mesh),
    };
    assert!(
        spatial
            .apply_edit_primed(refused, std::slice::from_ref(&primed))
            .is_err()
    );
    assert!(std::ptr::eq(clusters_of(&spatial, model), kept));
    assert_eq!(spatial.graph().mesh_resource_arc(rid), None);
}

/// An inline-mesh root leaf showing `mesh`, installed by an edit at a fresh
/// id, as Fill bed adds the pieces and supports of each copy.
fn add_inline_leaf(spatial: &mut SceneSpatial, mesh: &Arc<IndexedMesh>) -> (SceneEdit, NodeId) {
    let node_id = spatial.allocate_unbound_node_id();
    let position = spatial.graph().roots().len();
    let edit = SceneEdit::AddLeaf {
        node_id,
        attachment: LeafAttachment::Root { position },
        payload: LeafPayload::Mesh(Arc::clone(mesh)),
        local_transform: Transform3D::Identity,
        meta: None,
    };
    spatial.apply_edit(edit.clone()).expect("the leaf is added");
    (edit, node_id)
}

#[test]
fn an_inline_leaf_added_by_an_edit_can_be_picked() {
    let mut spatial = scene_with_a_model();
    let mesh = Arc::new(grid(16, 0.4));
    let (edit, id) = add_inline_leaf(&mut spatial, &mesh);
    assert_eq!(
        Some(clusters_of(&spatial, id)),
        TriangleClusters::for_mesh(&mesh).as_ref()
    );

    // A copy showing the same mesh shares what the first one built.
    let (_, copy) = add_inline_leaf(&mut spatial, &mesh);
    assert!(std::ptr::eq(
        clusters_of(&spatial, copy),
        clusters_of(&spatial, id)
    ));

    // Removed and put back by undo, it still answers for its mesh.
    let built: *const TriangleClusters = clusters_of(&spatial, id);
    let remove = edit.inverse();
    spatial
        .apply_edit(remove.clone())
        .expect("the removal applies");
    assert!(spatial.leaf(id).is_none());
    spatial
        .apply_edit(remove.inverse())
        .expect("the undo applies");
    assert!(std::ptr::eq(clusters_of(&spatial, id), built));
}

#[test]
fn an_inline_leaf_added_with_primed_caches_takes_them() {
    let mut spatial = scene_with_a_model();
    let mesh = Arc::new(grid(16, 0.4));
    let primed = PrimedMesh::new(Arc::clone(&mesh));
    let node_id = spatial.allocate_unbound_node_id();
    let position = spatial.graph().roots().len();
    spatial
        .apply_edit_primed(
            SceneEdit::AddLeaf {
                node_id,
                attachment: LeafAttachment::Root { position },
                payload: LeafPayload::Mesh(Arc::clone(&mesh)),
                local_transform: Transform3D::Identity,
                meta: None,
            },
            std::slice::from_ref(&primed),
        )
        .expect("the leaf is added");
    assert!(std::ptr::eq(
        clusters_of(&spatial, node_id),
        primed.clusters().unwrap()
    ));
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-wide removal of exact duplicate faces (see
//! [`IndexedMesh::duplicate_faces`]) over every stored mesh (the
//! [`Scene::mesh_targets`] walk). Each target is checked through a shared
//! borrow first; storage shared through `Arc`s is forked only for meshes that
//! have duplicates, so the others keep their pointer identity.
//!
//! [`IndexedMesh::duplicate_faces`]: sc_mesh_core::indexed_mesh::IndexedMesh::duplicate_faces

use crate::Scene;
use crate::mesh_targets::MeshTarget;

/// The duplicate faces removed from 1 stored mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedDuplicateFaces {
    /// The storage place of the mesh.
    pub target: MeshTarget,
    /// How many faces were removed.
    pub faces: usize,
}

impl Scene {
    /// Removes the exact duplicate faces of every stored mesh and lists the
    /// meshes that had any, in [`Scene::mesh_targets`] order. Meshes without
    /// duplicates are left untouched.
    pub fn drop_duplicate_faces(&mut self) -> Vec<DroppedDuplicateFaces> {
        let mut dropped = Vec::new();
        for target in self.mesh_targets() {
            let has_duplicates = self
                .target_mesh(target)
                .is_some_and(|mesh| !mesh.duplicate_faces().is_empty());
            if !has_duplicates {
                continue;
            }
            let Some(mesh) = self.target_mesh_mut(target) else {
                continue;
            };
            let faces = mesh.drop_duplicate_faces();
            dropped.push(DroppedDuplicateFaces { target, faces });
        }
        dropped
    }
}

#[cfg(test)]
mod scene_duplicate_faces_tests {
    use std::sync::Arc;

    use super::*;
    use crate::node::{LeafPayload, NodeKind};
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;

    const TETRA_FACES: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

    fn tetrahedron(copies: usize) -> IndexedMesh {
        let vertices = [
            [1.0, 1.0, 1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
        ]
        .into_iter()
        .map(Vertex::new)
        .collect();
        let faces = TETRA_FACES
            .iter()
            .cycle()
            .take(4 * copies)
            .copied()
            .map(IndexedTriangle::from_raw_indices)
            .collect();
        IndexedMesh::new(None, vertices, faces)
    }

    fn inline_arc(scene: &Scene, id: crate::NodeId) -> Arc<IndexedMesh> {
        match &scene.node(id).expect("live").kind {
            NodeKind::Leaf(LeafPayload::Mesh(arc)) => Arc::clone(arc),
            _ => panic!("inline leaf"),
        }
    }

    #[test]
    fn only_meshes_with_duplicates_change() {
        let mut scene = Scene::new();
        let clean = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tetrahedron(1))),
            )
            .expect("leaf");
        let doubled = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tetrahedron(2))),
            )
            .expect("leaf");
        let rid = scene.add_mesh_resource(tetrahedron(3));
        scene
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .expect("leaf");
        let clean_before = inline_arc(&scene, clean);

        let dropped = scene.drop_duplicate_faces();

        assert_eq!(
            dropped,
            vec![
                DroppedDuplicateFaces {
                    target: MeshTarget::Resource(rid),
                    faces: 8,
                },
                DroppedDuplicateFaces {
                    target: MeshTarget::InlineLeaf(doubled),
                    faces: 4,
                },
            ]
        );
        assert!(Arc::ptr_eq(&inline_arc(&scene, clean), &clean_before));
        assert_eq!(inline_arc(&scene, doubled).faces().len(), 4);
        assert_eq!(scene.mesh_resource(rid).expect("live").faces().len(), 4);
    }

    #[test]
    fn a_clean_scene_reports_nothing() {
        let mut scene = Scene::new();
        scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tetrahedron(1))),
            )
            .expect("leaf");
        assert!(scene.drop_duplicate_faces().is_empty());
    }
}

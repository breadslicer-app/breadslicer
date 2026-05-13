/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Bidirectional conversion between the flat [`Scene`] and
//! [`SceneGraph`]. These shims exist for as long as the two types
//! coexist in the workspace.

use crate::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
use crate::scene_graph::{SceneGraph, SceneGraphError};
use crate::scene_graph_node::{LeafPayload, MeshResourceId, NodeId, NodeKind};

impl From<Scene> for SceneGraph {
    /// Wraps the flat scene under a single identity-`Group` root.
    /// Resources keep their order: `scene.resources[i]` becomes
    /// `MeshResourceId(i + 1)`. With `try_flatten` going the other way,
    /// `Scene -> SceneGraph -> Scene` is the identity for any valid
    /// input scene.
    ///
    /// Panics on a `SceneObjectContent::Resource(idx)` whose index is
    /// out of range. `Scene` carries no construction-time bounds check,
    /// but every internal producer (format readers, the GUI document,
    /// `try_flatten` itself) preserves the invariant.
    fn from(scene: Scene) -> Self {
        let mut g = SceneGraph::new();
        g.set_meta(scene.meta);

        let mut mesh_ids: Vec<MeshResourceId> = Vec::with_capacity(scene.resources.len());
        for mesh in scene.resources {
            mesh_ids.push(g.add_mesh_resource(mesh));
        }

        let root = g.add_group(None, Transform3D::Identity);
        for obj in scene.objects {
            let payload = match obj.content {
                SceneObjectContent::Mesh(m) => LeafPayload::Mesh(m),
                SceneObjectContent::Resource(idx) => LeafPayload::MeshResource(
                    *mesh_ids
                        .get(idx)
                        .expect("Scene::from: out-of-range resource index"),
                ),
            };
            let leaf = g
                .add_leaf(obj.meta, obj.transform, payload)
                .expect("payload validated above");
            g.add_child(root, leaf)
                .expect("freshly added leaf has no parent");
        }
        g.add_root(root).expect("freshly added group is free");
        g
    }
}

impl SceneGraph {
    /// Builds a flat [`Scene`] equivalent to this graph, composing
    /// transforms along each ancestor chain and expanding `Instance`
    /// nodes inline. Errors with `UnevaluatedCsg` on any `Csg` node:
    /// without a kernel, CSG bodies cannot be reduced to a flat mesh
    /// list.
    ///
    /// Mesh resources are emitted in `MeshResourceId` order, so a graph
    /// produced by `SceneGraph::from(scene)` round-trips back to
    /// `scene` exactly.
    pub fn try_flatten(&self) -> Result<Scene, SceneGraphError> {
        let mut resources = Vec::with_capacity(self.mesh_resources.len());
        let mut rid_to_idx: Vec<Option<usize>> = vec![None; self.mesh_resources.len()];
        for (i, slot) in self.mesh_resources.iter().enumerate() {
            if let Some(mesh) = slot {
                rid_to_idx[i] = Some(resources.len());
                resources.push(mesh.clone());
            }
        }

        let mut objects: Vec<SceneObject> = Vec::new();
        for &root in self.roots() {
            self.flatten_into(root, Transform3D::Identity, &rid_to_idx, &mut objects)?;
        }

        Ok(Scene {
            meta: self.meta().clone(),
            resources,
            objects,
        })
    }

    fn flatten_into(
        &self,
        node_id: NodeId,
        parent_world: Transform3D,
        rid_to_idx: &[Option<usize>],
        objects: &mut Vec<SceneObject>,
    ) -> Result<(), SceneGraphError> {
        let node = self
            .node(node_id)
            .ok_or(SceneGraphError::DanglingNodeId { id: node_id })?;
        let world_from_self = parent_world.compose(&node.local_transform);
        match &node.kind {
            NodeKind::Group { children } => {
                for &c in children {
                    self.flatten_into(c, world_from_self, rid_to_idx, objects)?;
                }
            }
            NodeKind::Csg { .. } => {
                return Err(SceneGraphError::UnevaluatedCsg { node_id });
            }
            NodeKind::Leaf(payload) => {
                let content = match payload {
                    LeafPayload::Mesh(m) => SceneObjectContent::Mesh(m.clone()),
                    LeafPayload::MeshResource(rid) => {
                        let idx = rid_to_idx
                            .get(rid.index())
                            .copied()
                            .flatten()
                            .ok_or(SceneGraphError::DanglingMeshResource { id: *rid })?;
                        SceneObjectContent::Resource(idx)
                    }
                };
                objects.push(SceneObject {
                    meta: node.meta.clone(),
                    transform: world_from_self,
                    content,
                });
            }
            NodeKind::Instance { resource } => {
                let root = self
                    .subtree_resource_root(*resource)
                    .ok_or(SceneGraphError::DanglingSubtreeResource { id: *resource })?;
                self.flatten_into(root, world_from_self, rid_to_idx, objects)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod scene_graph_shims_tests {
    use super::*;
    use crate::indexed_mesh::IndexedMesh;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::scene::SceneMetadata;
    use crate::scene_graph_node::{CsgOp, LeafPayload};
    use crate::transform::AffineTransform3D;
    use float_cmp::assert_approx_eq;

    fn triangle_mesh(vertices: Vec<[f32; 3]>, face: [u32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vertices.into_iter().map(Vertex::new).collect(),
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                face,
            )],
        }
    }

    fn inline_object(transform: Transform3D, mesh: IndexedMesh) -> SceneObject {
        SceneObject {
            meta: None,
            transform,
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn resource_object(transform: Transform3D, idx: usize) -> SceneObject {
        SceneObject {
            meta: None,
            transform,
            content: SceneObjectContent::Resource(idx),
        }
    }

    #[test]
    fn round_trip_of_empty_scene_is_identity() {
        let scene = Scene::default();
        let round_tripped = SceneGraph::from(scene.clone()).try_flatten().unwrap();
        assert_eq!(round_tripped, scene);
    }

    #[test]
    fn round_trip_of_single_inline_identity_object_is_identity() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(
                Transform3D::Identity,
                triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                ),
            )],
        };
        let round_tripped = SceneGraph::from(scene.clone()).try_flatten().unwrap();
        assert_eq!(round_tripped, scene);
    }

    #[test]
    fn round_trip_of_single_inline_affine_object_is_identity() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(
                Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0])),
                triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                ),
            )],
        };
        let round_tripped = SceneGraph::from(scene.clone()).try_flatten().unwrap();
        assert_eq!(round_tripped, scene);
    }

    #[test]
    fn round_trip_preserves_resource_order_and_references() {
        let r0 = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let r1 = triangle_mesh(
            vec![[2.0, 2.0, 2.0], [3.0, 2.0, 2.0], [2.0, 3.0, 2.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![r0, r1],
            objects: vec![
                resource_object(Transform3D::Identity, 1),
                resource_object(
                    Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0])),
                    0,
                ),
            ],
        };
        let round_tripped = SceneGraph::from(scene.clone()).try_flatten().unwrap();
        assert_eq!(round_tripped, scene);
    }

    #[test]
    fn round_trip_preserves_scene_metadata() {
        let scene = Scene {
            meta: Some(SceneMetadata {
                name: Some("test".to_string()),
                comment: Some("round trip".to_string()),
            }),
            resources: vec![],
            objects: vec![],
        };
        let round_tripped = SceneGraph::from(scene.clone()).try_flatten().unwrap();
        assert_eq!(round_tripped, scene);
    }

    #[test]
    fn round_trip_preserves_object_metadata() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: Some(SceneMetadata {
                    name: Some("object".to_string()),
                    comment: None,
                }),
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                )),
            }],
        };
        let round_tripped = SceneGraph::from(scene.clone()).try_flatten().unwrap();
        assert_eq!(round_tripped, scene);
    }

    #[test]
    fn try_flatten_composes_group_ancestor_transform_into_each_leaf() {
        // A graph built outside the From shim: Group with a translation
        // T1, containing a leaf with a translation T2. The flattened
        // scene must have one object whose transform is T1 . T2 applied
        // to the leaf's mesh.
        let mut g = SceneGraph::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
                LeafPayload::Mesh(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                )),
            )
            .unwrap();
        let parent = g.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
        );
        g.add_child(parent, leaf).unwrap();
        g.add_root(parent).unwrap();

        let flat = g.try_flatten().unwrap();
        assert_eq!(flat.objects.len(), 1);
        let obj_transform = flat.objects[0].transform;
        match obj_transform {
            Transform3D::Affine(a) => {
                assert_approx_eq!(f64, a.translation()[0], 11.0, epsilon = 1e-9);
            }
            Transform3D::Identity => panic!("expected Affine, got Identity"),
        }
    }

    #[test]
    fn try_flatten_errors_on_csg_node() {
        let mut g = SceneGraph::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                )),
            )
            .unwrap();
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf])
            .unwrap();
        g.add_root(csg).unwrap();
        assert_eq!(
            g.try_flatten(),
            Err(SceneGraphError::UnevaluatedCsg { node_id: csg })
        );
    }

    #[test]
    fn try_flatten_expands_subtree_instances() {
        let mut g = SceneGraph::new();
        let res_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                )),
            )
            .unwrap();
        let res_root = g.add_group(None, Transform3D::Identity);
        g.add_child(res_root, res_leaf).unwrap();
        let rid = g.register_subtree_resource(res_root).unwrap();

        let r1 = g.add_group(None, Transform3D::Identity);
        let i1 = g.add_instance(None, Transform3D::Identity, rid).unwrap();
        g.add_child(r1, i1).unwrap();
        g.add_root(r1).unwrap();

        let r2 = g.add_group(None, Transform3D::Identity);
        let i2 = g
            .add_instance(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([100.0, 0.0, 0.0])),
                rid,
            )
            .unwrap();
        g.add_child(r2, i2).unwrap();
        g.add_root(r2).unwrap();

        let flat = g.try_flatten().unwrap();
        assert_eq!(flat.objects.len(), 2);
        assert!(matches!(flat.objects[0].transform, Transform3D::Identity));
        match flat.objects[1].transform {
            Transform3D::Affine(a) => {
                assert_approx_eq!(f64, a.translation()[0], 100.0, epsilon = 1e-9);
            }
            Transform3D::Identity => panic!("expected Affine, got Identity"),
        }
    }

    #[test]
    fn from_scene_uses_identity_root_so_leaf_transform_passes_through() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(
                Transform3D::Affine(AffineTransform3D::from_translation([7.0, 0.0, 0.0])),
                triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                ),
            )],
        };
        let g = SceneGraph::from(scene);
        assert_eq!(g.roots().len(), 1);
        let root_id = g.roots()[0];
        let root_node = g.node(root_id).unwrap();
        assert!(matches!(root_node.local_transform, Transform3D::Identity));
        assert!(matches!(root_node.kind, NodeKind::Group { ref children } if children.len() == 1));
    }

    #[test]
    fn round_trip_of_multi_object_scene_with_mixed_content_is_identity() {
        let resource = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let inline = triangle_mesh(
            vec![[5.0, 5.0, 5.0], [6.0, 5.0, 5.0], [5.0, 6.0, 5.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: Some(SceneMetadata {
                name: Some("mixed".to_string()),
                comment: None,
            }),
            resources: vec![resource],
            objects: vec![
                resource_object(
                    Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
                    0,
                ),
                inline_object(Transform3D::Identity, inline),
            ],
        };
        let round_tripped = SceneGraph::from(scene.clone()).try_flatten().unwrap();
        assert_eq!(round_tripped, scene);
    }
}

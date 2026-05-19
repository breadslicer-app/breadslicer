/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! World-space mesh baking for [`Scene`].

use crate::indexed_mesh::IndexedMesh;
use crate::primitives::{IndexedTriangle, Vertex};
use crate::scene_graph::{Scene, SceneError};
use crate::scene_graph_node::{LeafPayload, Node, NodeId, NodeKind};
use crate::transform::AffineTransform3D;
use crate::transform::Transform3D;

impl Scene {
    /// Merges all leaf meshes reachable from scene roots into a single
    /// [`IndexedMesh`], baking the composed ancestor-chain transform
    /// into each leaf's vertices and normals. `Group` and `Instance`
    /// nodes are traversed. `Csg` nodes are an error: there is no
    /// evaluator yet, so any caller that wants to flatten a graph
    /// containing CSG must first resolve those nodes through a kernel.
    pub fn merge_meshes(&self) -> Result<Option<IndexedMesh>, SceneError> {
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut faces: Vec<IndexedTriangle> = Vec::new();
        for &root in self.roots() {
            self.bake_subtree(root, Transform3D::Identity, &mut vertices, &mut faces)?;
        }
        if faces.is_empty() {
            Ok(None)
        } else {
            Ok(Some(IndexedMesh {
                meta: None,
                vertices,
                faces,
            }))
        }
    }

    fn bake_subtree(
        &self,
        node_id: NodeId,
        parent_world: Transform3D,
        vertices: &mut Vec<Vertex>,
        faces: &mut Vec<IndexedTriangle>,
    ) -> Result<(), SceneError> {
        let node = self
            .node(node_id)
            .ok_or(SceneError::DanglingNodeId { id: node_id })?;
        let world_from_self = parent_world.compose(&node.local_transform);
        match &node.kind {
            NodeKind::Group { children } => {
                for &c in children {
                    self.bake_subtree(c, world_from_self, vertices, faces)?;
                }
            }
            NodeKind::Csg { .. } => {
                return Err(SceneError::UnevaluatedCsg { node_id });
            }
            NodeKind::Leaf(payload) => {
                let mesh = self.resolve_leaf_mesh(payload)?;
                bake_into(mesh, &world_from_self, vertices, faces)?;
            }
            NodeKind::Instance { resource } => {
                let root = self
                    .subtree_resource_root(*resource)
                    .ok_or(SceneError::DanglingSubtreeResource { id: *resource })?;
                self.bake_subtree(root, world_from_self, vertices, faces)?;
            }
        }
        Ok(())
    }

    fn resolve_leaf_mesh<'a>(
        &'a self,
        payload: &'a LeafPayload,
    ) -> Result<&'a IndexedMesh, SceneError> {
        match payload {
            LeafPayload::Mesh(m) => Ok(m),
            LeafPayload::MeshResource(rid) => self
                .mesh_resource(*rid)
                .ok_or(SceneError::DanglingMeshResource { id: *rid }),
        }
    }

    /// Resolve the mesh a leaf addresses by stable [`NodeId`]: for
    /// `LeafPayload::Mesh` returns the inline mesh; for
    /// `LeafPayload::MeshResource` returns the resource it points at.
    /// Errors with [`SceneError::DanglingNodeId`] if `id` does
    /// not name a live node, [`SceneError::NotALeaf`] if the
    /// addressed node is not a `Leaf`, and
    /// [`SceneError::DanglingMeshResource`] if the leaf
    /// references a missing resource.
    pub fn mesh_at_leaf(&self, id: NodeId) -> Result<&IndexedMesh, SceneError> {
        let node = self.node(id).ok_or(SceneError::DanglingNodeId { id })?;
        let payload = match &node.kind {
            NodeKind::Leaf(p) => p,
            _ => return Err(SceneError::NotALeaf { node_id: id }),
        };
        self.resolve_leaf_mesh(payload)
    }
}

/// Bakes a single leaf into its ancestor-chain frame. For a leaf under
/// a scene root, the result is in world space. For a leaf inside a
/// subtree resource, the result is in the subtree's own external frame
/// (the Instance call site is not consulted), which is the natural
/// per-leaf invariant of stable [`NodeId`] addressing.
///
/// Errors if `node_id` does not resolve to a `Leaf`.
pub fn bake_leaf_to_world(graph: &Scene, node_id: NodeId) -> Result<IndexedMesh, SceneError> {
    let leaf_node = graph
        .node(node_id)
        .ok_or(SceneError::DanglingNodeId { id: node_id })?;
    let payload = match &leaf_node.kind {
        NodeKind::Leaf(p) => p,
        _ => return Err(SceneError::NotALeaf { node_id }),
    };
    let mesh = match payload {
        LeafPayload::Mesh(m) => m,
        LeafPayload::MeshResource(rid) => graph
            .mesh_resource(*rid)
            .ok_or(SceneError::DanglingMeshResource { id: *rid })?,
    };
    let world = world_from_ancestor_chain(graph, leaf_node, node_id)?;
    let mut vertices = Vec::with_capacity(mesh.vertices.len());
    let mut faces = Vec::with_capacity(mesh.faces.len());
    bake_into(mesh, &world, &mut vertices, &mut faces)?;
    Ok(IndexedMesh {
        meta: None,
        vertices,
        faces,
    })
}

fn world_from_ancestor_chain(
    graph: &Scene,
    leaf_node: &Node,
    leaf_id: NodeId,
) -> Result<Transform3D, SceneError> {
    let mut chain: Vec<Transform3D> = vec![leaf_node.local_transform];
    let mut current = leaf_id;
    while let Some(p) = graph.parent_of(current) {
        let pnode = graph.node(p).ok_or(SceneError::DanglingNodeId { id: p })?;
        chain.push(pnode.local_transform);
        current = p;
    }
    let mut world = Transform3D::Identity;
    for t in chain.iter().rev() {
        world = world.compose(t);
    }
    Ok(world)
}

fn bake_into(
    mesh: &IndexedMesh,
    world: &Transform3D,
    vertices: &mut Vec<Vertex>,
    faces: &mut Vec<IndexedTriangle>,
) -> Result<(), SceneError> {
    let offset = u32::try_from(vertices.len()).expect("vertex index fits in u32");
    match world {
        Transform3D::Identity => {
            vertices.extend(mesh.vertices.iter().copied());
            for face in &mesh.faces {
                faces.push(IndexedTriangle {
                    normal: face.normal,
                    vertices: [
                        face.vertices[0].offset(offset),
                        face.vertices[1].offset(offset),
                        face.vertices[2].offset(offset),
                    ],
                });
            }
        }
        Transform3D::Affine(a) => {
            bake_into_affine(mesh, a, offset, vertices, faces)?;
        }
    }
    Ok(())
}

fn bake_into_affine(
    mesh: &IndexedMesh,
    a: &AffineTransform3D,
    offset: u32,
    vertices: &mut Vec<Vertex>,
    faces: &mut Vec<IndexedTriangle>,
) -> Result<(), SceneError> {
    vertices.extend(mesh.vertices.iter().map(|v| a.apply_to_vertex(*v)));
    let reverse_winding = a.determinant() < 0.0;
    for face in &mesh.faces {
        let normal = a.apply_to_normal(face.normal)?;
        let v = if reverse_winding {
            [face.vertices[0], face.vertices[2], face.vertices[1]]
        } else {
            face.vertices
        };
        faces.push(IndexedTriangle {
            normal,
            vertices: [
                v[0].offset(offset),
                v[1].offset(offset),
                v[2].offset(offset),
            ],
        });
    }
    Ok(())
}

#[cfg(test)]
mod scene_graph_bake_tests {
    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::scene_graph_node::{CsgOp, LeafPayload};
    use float_cmp::assert_approx_eq;
    use std::f64::consts::FRAC_PI_2;

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

    fn cube_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        }
    }

    #[test]
    fn merge_meshes_on_empty_graph_returns_none() {
        let g = Scene::new();
        assert_eq!(g.merge_meshes().unwrap(), None);
    }

    #[test]
    fn merge_meshes_on_single_leaf_identity_passes_through() {
        let mut g = Scene::new();
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh.clone()))
            .unwrap();
        g.add_root(leaf).unwrap();
        let merged = g.merge_meshes().unwrap().unwrap();
        assert_eq!(merged.vertices, mesh.vertices);
        assert_eq!(merged.faces, mesh.faces);
    }

    #[test]
    fn merge_meshes_composes_group_and_leaf_translations() {
        // Group T1 + leaf T2 should give T1 + T2 baked into vertices.
        let mut g = Scene::new();
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
        let group = g.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
        );
        g.add_child(group, leaf).unwrap();
        g.add_root(group).unwrap();
        let merged = g.merge_meshes().unwrap().unwrap();
        assert_approx_eq!(f32, merged.vertices[0][0], 11.0, epsilon = 1e-6);
        assert_approx_eq!(f32, merged.vertices[1][0], 12.0, epsilon = 1e-6);
        assert_approx_eq!(f32, merged.vertices[2][0], 11.0, epsilon = 1e-6);
    }

    #[test]
    fn merge_meshes_errors_on_csg_node() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(cube_mesh()))
            .unwrap();
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf])
            .unwrap();
        g.add_root(csg).unwrap();
        assert_eq!(
            g.merge_meshes(),
            Err(SceneError::UnevaluatedCsg { node_id: csg })
        );
    }

    #[test]
    fn merge_meshes_duplicates_geometry_for_two_instances() {
        // Subtree contains one triangle. Two instances at different
        // translations should each contribute a baked copy.
        let mut g = Scene::new();
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

        let merged = g.merge_meshes().unwrap().unwrap();
        assert_eq!(merged.vertices.len(), 6);
        assert_eq!(merged.faces.len(), 2);
        // Second instance's first vertex is at (100, 0, 0).
        assert_approx_eq!(f32, merged.vertices[3][0], 100.0, epsilon = 1e-6);
    }

    #[test]
    fn bake_leaf_to_world_under_identity_root_matches_mesh() {
        let mut g = Scene::new();
        let mesh = triangle_mesh(
            vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            [0, 1, 2],
        );
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh.clone()))
            .unwrap();
        g.add_root(leaf).unwrap();
        let baked = bake_leaf_to_world(&g, leaf).unwrap();
        assert_eq!(baked.vertices, mesh.vertices);
        assert_eq!(baked.faces, mesh.faces);
    }

    #[test]
    fn bake_leaf_to_world_composes_ancestor_chain_translations() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
                LeafPayload::Mesh(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
                    [0, 1, 2],
                )),
            )
            .unwrap();
        let parent = g.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([0.0, 10.0, 0.0])),
        );
        let grandparent = g.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 100.0])),
        );
        g.add_child(parent, leaf).unwrap();
        g.add_child(grandparent, parent).unwrap();
        g.add_root(grandparent).unwrap();
        let baked = bake_leaf_to_world(&g, leaf).unwrap();
        // World = grandparent ∘ parent ∘ leaf = (0,0,100) + (0,10,0) + (1,0,0) = (1,10,100)
        assert_approx_eq!(f32, baked.vertices[0][0], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.vertices[0][1], 10.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.vertices[0][2], 100.0, epsilon = 1e-6);
    }

    #[test]
    fn bake_leaf_to_world_reverses_winding_under_mirror() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0])),
                LeafPayload::Mesh(triangle_mesh(
                    vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    [0, 1, 2],
                )),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let baked = bake_leaf_to_world(&g, leaf).unwrap();
        assert_eq!(baked.faces[0].vertices.map(u32::from), [0, 2, 1]);
    }

    #[test]
    fn bake_leaf_to_world_rotates_normal() {
        let mut g = Scene::new();
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        };
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_rotation_z(FRAC_PI_2)),
                LeafPayload::Mesh(mesh),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let baked = bake_leaf_to_world(&g, leaf).unwrap();
        // +X normal rotates to +Y under pi/2 about Z.
        assert_approx_eq!(f32, baked.faces[0].normal[0], 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.faces[0].normal[1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.faces[0].normal[2], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn bake_leaf_to_world_on_non_leaf_errors() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_root(group).unwrap();
        assert_eq!(
            bake_leaf_to_world(&g, group),
            Err(SceneError::NotALeaf { node_id: group })
        );
    }
}

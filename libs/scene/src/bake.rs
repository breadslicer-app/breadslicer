/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! World-space mesh baking for [`Scene`].

use crate::Scene;
use crate::error::SceneError;
use crate::expand::CsgPolicy;
use crate::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::transform::TransformError;

/// A visit of [`Scene::for_each_expanded_leaf`]: the leaf node and the composed
/// world transform of that visit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeafOccurrence {
    pub leaf: NodeId,
    pub world: Transform3D,
}

/// A merged mesh and the occurrences baked into it, in the order of their
/// faces: each occurrence owns as many consecutive faces as its leaf's mesh
/// holds.
#[derive(Clone, Debug, PartialEq)]
pub struct MergedMeshes {
    pub mesh: IndexedMesh,
    pub occurrences: Vec<LeafOccurrence>,
}

impl Scene {
    /// Merges all leaf meshes reachable from the scene roots into a single
    /// [`IndexedMesh`], baking the composed ancestor-chain transform into each
    /// leaf's vertices, or returns `None` when no leaf has a face. Runs on
    /// [`Scene::for_each_expanded_leaf`], so `Group` and `Reference` nodes are
    /// traversed (a referenced subtree contributes 1 baked copy per
    /// occurrence). `Csg` nodes are an error ([`CsgPolicy::Refuse`]): without
    /// an evaluator, a caller that flattens a graph with CSG must first resolve
    /// those nodes through a kernel.
    pub fn merge_meshes(&self) -> Result<Option<IndexedMesh>, SceneError> {
        Ok(self
            .merge_meshes_with_occurrences()?
            .map(|merged| merged.mesh))
    }

    /// Merges as [`Scene::merge_meshes`] does, and also returns the occurrences
    /// it baked.
    pub fn merge_meshes_with_occurrences(&self) -> Result<Option<MergedMeshes>, SceneError> {
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut faces: Vec<IndexedTriangle> = Vec::new();
        let mut occurrences: Vec<LeafOccurrence> = Vec::new();
        self.for_each_expanded_leaf(CsgPolicy::Refuse, |leaf, world, mesh| {
            occurrences.push(LeafOccurrence {
                leaf,
                world: *world,
            });
            bake_into(mesh, world, &mut vertices, &mut faces)
        })?;
        if faces.is_empty() {
            Ok(None)
        } else {
            Ok(Some(MergedMeshes {
                mesh: IndexedMesh {
                    meta: None,
                    vertices,
                    faces,
                },
                occurrences,
            }))
        }
    }

    fn resolve_leaf_mesh<'a>(
        &'a self,
        payload: &'a LeafPayload,
    ) -> Result<&'a IndexedMesh, SceneError> {
        match payload {
            LeafPayload::Mesh(m) => Ok(m.as_ref()),
            LeafPayload::MeshResource(rid) => self
                .mesh_resource(*rid)
                .ok_or(SceneError::DanglingMeshResource { id: *rid }),
        }
    }

    /// Resolves the mesh a leaf addresses by stable [`NodeId`]: for
    /// `LeafPayload::Mesh` returns the inline mesh; for
    /// `LeafPayload::MeshResource` returns the resource it points at. Errors
    /// with [`SceneError::DanglingNodeId`] if `id` does not name a live node,
    /// [`SceneError::NotALeaf`] if the addressed node is not a `Leaf`, and
    /// [`SceneError::DanglingMeshResource`] if the leaf references a missing
    /// resource.
    pub fn mesh_at_leaf(&self, id: NodeId) -> Result<&IndexedMesh, SceneError> {
        let node = self.node(id).ok_or(SceneError::DanglingNodeId { id })?;
        let payload = match &node.kind {
            NodeKind::Leaf(p) => p,
            _ => return Err(SceneError::NotALeaf { node_id: id }),
        };
        self.resolve_leaf_mesh(payload)
    }
}

/// Bakes a single leaf into its ancestor-chain frame. For a leaf under a scene
/// root, the result is in world space. For a leaf inside a subtree resource,
/// the result is in the frame of the subtree root, without the transform of any
/// reference to it: a [`NodeId`] names 1 leaf, not 1 of its occurrences.
///
/// Errors if `node_id` does not resolve to a `Leaf`, if the mesh resource of
/// the leaf is missing, or if its frame has no inverse.
pub fn bake_leaf_to_world(graph: &Scene, node_id: NodeId) -> Result<IndexedMesh, SceneError> {
    let leaf_node = graph
        .node(node_id)
        .ok_or(SceneError::DanglingNodeId { id: node_id })?;
    let payload = match &leaf_node.kind {
        NodeKind::Leaf(p) => p,
        _ => return Err(SceneError::NotALeaf { node_id }),
    };
    let mesh = match payload {
        LeafPayload::Mesh(m) => m.as_ref(),
        LeafPayload::MeshResource(rid) => graph
            .mesh_resource(*rid)
            .ok_or(SceneError::DanglingMeshResource { id: *rid })?,
    };
    let world = graph.world_transform_of(node_id)?;
    bake_mesh_to_world(mesh, &world)
}

/// Bakes `mesh` under the frame `world`: vertices are transformed, and triangle
/// winding is reversed when `world` mirrors (negative determinant). The result
/// carries no metadata; callers that want the source mesh's name re-attach it
/// themselves.
///
/// This is the per-occurrence companion of [`Scene::for_each_expanded_leaf`]:
/// the walk hands each occurrence's composed world transform to its callback,
/// and this bakes the occurrence's mesh under that transform.
/// [`bake_leaf_to_world`] covers the ancestor-chain case instead.
///
/// Errors when `world` has no inverse (singular linear part).
pub fn bake_mesh_to_world(
    mesh: &IndexedMesh,
    world: &Transform3D,
) -> Result<IndexedMesh, SceneError> {
    let mut vertices = Vec::with_capacity(mesh.vertices.len());
    let mut faces = Vec::with_capacity(mesh.faces.len());
    bake_into(mesh, world, &mut vertices, &mut faces)?;
    Ok(IndexedMesh {
        meta: None,
        vertices,
        faces,
    })
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
    if !a.is_invertible() {
        return Err(TransformError::SingularLinear.into());
    }
    vertices.extend(mesh.vertices.iter().map(|v| a.apply_to_vertex(*v)));
    let reverse_winding = a.determinant() < 0.0;
    for face in &mesh.faces {
        let v = if reverse_winding {
            [face.vertices[0], face.vertices[2], face.vertices[1]]
        } else {
            face.vertices
        };
        faces.push(IndexedTriangle {
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
mod scene_bake_tests {
    use super::*;
    use crate::node::{CsgOp, LeafPayload};
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use std::sync::Arc;

    fn triangle_mesh(vertices: Vec<[f32; 3]>, face: [u32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vertices.into_iter().map(Vertex::new).collect(),
            faces: vec![IndexedTriangle::from_raw_indices(face)],
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
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh.clone())),
            )
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
                LeafPayload::Mesh(Arc::new(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                ))),
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
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(cube_mesh())),
            )
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
    fn merge_meshes_duplicates_geometry_for_two_references_and_lists_the_occurrences() {
        // Subtree contains 1 triangle. 2 references at different translations
        // should each contribute a baked copy.
        let mut g = Scene::new();
        let res_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0, 1, 2],
                ))),
            )
            .unwrap();
        let res_root = g.add_group(None, Transform3D::Identity);
        g.add_child(res_root, res_leaf).unwrap();
        let rid = g.register_subtree_resource(res_root).unwrap();

        let r1 = g.add_group(None, Transform3D::Identity);
        let i1 = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        g.add_child(r1, i1).unwrap();
        g.add_root(r1).unwrap();

        let r2 = g.add_group(None, Transform3D::Identity);
        let i2 = g
            .add_reference(
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
        // Second occurrence's first vertex is at (100, 0, 0).
        assert_approx_eq!(f32, merged.vertices[3][0], 100.0, epsilon = 1e-6);

        let with_occurrences = g.merge_meshes_with_occurrences().unwrap().unwrap();
        assert_eq!(with_occurrences.mesh, merged);
        assert_eq!(
            with_occurrences.occurrences,
            vec![
                LeafOccurrence {
                    leaf: res_leaf,
                    world: Transform3D::Identity,
                },
                LeafOccurrence {
                    leaf: res_leaf,
                    world: Transform3D::Affine(AffineTransform3D::from_translation([
                        100.0, 0.0, 0.0
                    ])),
                },
            ]
        );
    }

    #[test]
    fn bake_leaf_to_world_under_identity_root_matches_mesh() {
        let mut g = Scene::new();
        let mesh = triangle_mesh(
            vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            [0, 1, 2],
        );
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh.clone())),
            )
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
                LeafPayload::Mesh(Arc::new(triangle_mesh(
                    vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
                    [0, 1, 2],
                ))),
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
        // World = grandparent ∘ parent ∘ leaf = (0,0,100) + (0,10,0) + (1,0,0)
        // = (1,10,100)
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
                LeafPayload::Mesh(Arc::new(triangle_mesh(
                    vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    [0, 1, 2],
                ))),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let baked = bake_leaf_to_world(&g, leaf).unwrap();
        assert_eq!(baked.faces[0].vertices.map(u32::from), [0, 2, 1]);
    }

    #[test]
    fn bake_mesh_to_world_identity_passes_geometry_through() {
        let mesh = triangle_mesh(
            vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            [0, 1, 2],
        );
        let baked = bake_mesh_to_world(&mesh, &Transform3D::Identity).unwrap();
        assert_eq!(baked.vertices, mesh.vertices);
        assert_eq!(baked.faces, mesh.faces);
    }

    #[test]
    fn bake_mesh_to_world_drops_source_metadata() {
        use sc_mesh_core::indexed_mesh::MeshMetadata;

        let mut mesh = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        mesh.meta = Some(MeshMetadata {
            name: Some("Named".to_string()),
            ..Default::default()
        });
        let baked = bake_mesh_to_world(&mesh, &Transform3D::Identity).unwrap();
        assert!(baked.meta.is_none());
    }

    #[test]
    fn bake_mesh_to_world_applies_translation() {
        let mesh = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let world = Transform3D::Affine(AffineTransform3D::from_translation([10.0, -5.0, 2.0]));
        let baked = bake_mesh_to_world(&mesh, &world).unwrap();
        assert_approx_eq!(f32, baked.vertices[0][0], 10.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.vertices[0][1], -5.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.vertices[0][2], 2.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.vertices[1][0], 11.0, epsilon = 1e-6);
    }

    #[test]
    fn bake_mesh_to_world_reverses_winding_under_mirror() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let world =
            Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]));
        let baked = bake_mesh_to_world(&mesh, &world).unwrap();
        assert_eq!(baked.faces[0].vertices.map(u32::from), [0, 2, 1]);
    }

    #[test]
    fn bake_mesh_to_world_errors_on_singular_transform() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        assert!(bake_mesh_to_world(&mesh, &Transform3D::Affine(singular)).is_err());
    }

    #[test]
    fn bake_mesh_to_world_takes_a_collinear_face_under_a_rotation_with_scale() {
        let mesh = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        // The placement of a 3MF build item: a rotation about Z with a uniform
        // scale of 0.5853570, and a translation.
        let item = AffineTransform3D::from_linear_and_translation(
            [
                [-0.04546128719263793, -0.5835889231040134, 0.0],
                [0.5835889231040134, -0.04546128719263793, 0.0],
                [0.0, 0.0, 0.5853569507598877],
            ],
            [-54.116507902278265, 19.34432236804706, 9.951433181762695],
        );
        bake_mesh_to_world(&mesh, &Transform3D::Affine(item)).expect("the mesh bakes");
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

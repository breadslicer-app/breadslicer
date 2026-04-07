/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::IndexedMesh;
use crate::primitives::IndexedTriangle;
use anyhow::{Result, anyhow};

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Transform3D {
    #[default]
    Identity,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneMetadata {
    pub name: Option<String>,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneObjectContent {
    Mesh(IndexedMesh),
    Resource(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneObject {
    pub meta: Option<SceneMetadata>,
    pub transform: Transform3D,
    pub content: SceneObjectContent,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub meta: Option<SceneMetadata>,
    pub resources: Vec<IndexedMesh>,
    pub objects: Vec<SceneObject>,
}

impl Scene {
    /// Merges all meshes in the scene into a single [`IndexedMesh`].
    /// Returns `Ok(None)` if the scene contains no geometry,
    /// or an error if any resource index is out of bounds.
    pub fn merge_meshes(&self) -> Result<Option<IndexedMesh>> {
        let mut vertices = Vec::new();
        let mut faces = Vec::new();

        for obj in &self.objects {
            let mesh = match &obj.content {
                SceneObjectContent::Mesh(m) => m,
                SceneObjectContent::Resource(idx) => self
                    .resources
                    .get(*idx)
                    .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
            };
            let offset = vertices.len() as u32;
            vertices.extend_from_slice(&mesh.vertices);
            for face in &mesh.faces {
                faces.push(IndexedTriangle {
                    normal: face.normal,
                    vertices: [
                        face.vertices[0] + offset,
                        face.vertices[1] + offset,
                        face.vertices[2] + offset,
                    ],
                });
            }
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
}

#[cfg(test)]
mod merge_meshes_tests {
    use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};

    fn triangle_mesh(vertices: Vec<[f32; 3]>, face_vertices: [u32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vertices.into_iter().map(Vertex::new).collect(),
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: face_vertices,
            }],
        }
    }

    fn inline_object(mesh: IndexedMesh) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn resource_object(idx: usize) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Resource(idx),
        }
    }

    #[test]
    fn empty_scene_returns_none() {
        let scene = Scene::default();
        assert_eq!(scene.merge_meshes().unwrap(), None);
    }

    #[test]
    fn single_inline_mesh_passes_through() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(mesh.clone())],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        assert_eq!(result.vertices, mesh.vertices);
        assert_eq!(result.faces, mesh.faces);
    }

    #[test]
    fn two_inline_meshes_concatenate_with_offset_indices() {
        let mesh_a = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let mesh_b = triangle_mesh(
            vec![[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(mesh_a.clone()), inline_object(mesh_b.clone())],
        };
        let result = scene.merge_meshes().unwrap().unwrap();

        assert_eq!(result.vertices.len(), 6);
        assert_eq!(result.vertices[..3], mesh_a.vertices[..]);
        assert_eq!(result.vertices[3..], mesh_b.vertices[..]);

        assert_eq!(result.faces.len(), 2);
        assert_eq!(result.faces[0].vertices, [0, 1, 2]);
        assert_eq!(result.faces[1].vertices, [3, 4, 5]);
    }

    #[test]
    fn valid_resource_reference_is_resolved() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![mesh.clone()],
            objects: vec![resource_object(0)],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        assert_eq!(result.vertices, mesh.vertices);
        assert_eq!(result.faces, mesh.faces);
    }

    #[test]
    fn invalid_resource_index_returns_error() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![resource_object(0)],
        };
        assert!(scene.merge_meshes().is_err());
    }

    #[test]
    fn result_has_no_meta() {
        let mut mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        mesh.meta = Some(MeshMetadata {
            name: Some("SomeMesh".to_string()),
            ..Default::default()
        });
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(mesh)],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        assert_eq!(result.meta, None);
    }
}

impl From<IndexedMesh> for Scene {
    fn from(mesh: IndexedMesh) -> Self {
        Self {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(mesh),
            }],
        }
    }
}

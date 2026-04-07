/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::IndexedMesh;

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

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::IndexedMesh;
use crate::primitives::{IndexedTriangle, Normal, Vertex};
use crate::transform::AffineTransform3D;
use anyhow::{Result, anyhow};

/// Per-object transform. The `Identity` variant lets callers skip the
/// matrix application entirely for the (very common) case where an object
/// has not been moved since import.
///
/// Currently semantics: world-space transform of the object's mesh, i.e.
/// `world_vertex = transform.apply_to_vertex(local_vertex)`. When a
/// scene-graph hierarchy is added later, this becomes "transform from the
/// object's local frame to its parent's frame"; for a flat scene the two
/// coincide.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Transform3D {
    #[default]
    Identity,
    Affine(AffineTransform3D),
}

impl Transform3D {
    /// Applies the transform to a point. Identity short-circuits.
    pub fn apply_to_vertex(&self, v: Vertex) -> Vertex {
        match self {
            Self::Identity => v,
            Self::Affine(a) => a.apply_to_vertex(v),
        }
    }

    /// Applies the transform to a unit surface normal. Identity
    /// short-circuits; for `Affine`, see
    /// `AffineTransform3D::apply_to_normal` for the precise semantics
    /// (inverse-transpose with sign-flip on negative determinant, so that
    /// the result matches a winding-reversed triangle).
    pub fn apply_to_normal(&self, n: Normal) -> Result<Normal> {
        match self {
            Self::Identity => Ok(n),
            Self::Affine(a) => a.apply_to_normal(n),
        }
    }

    /// Composes two transforms: `self.compose(other)` applies `other`
    /// first and then `self`. Identity short-circuits on either side
    /// without touching float arithmetic.
    pub fn compose(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Identity, _) => *other,
            (_, Self::Identity) => *self,
            (Self::Affine(a), Self::Affine(b)) => Self::Affine(a.compose(b)),
        }
    }

    /// Inverse of the transform. Identity is its own inverse;
    /// `Affine` propagates the singular-matrix error from
    /// `AffineTransform3D::inverse`.
    pub fn inverse(&self) -> Result<Self> {
        match self {
            Self::Identity => Ok(Self::Identity),
            Self::Affine(a) => Ok(Self::Affine(a.inverse()?)),
        }
    }

    /// Determinant of the linear part. Identity returns `1.0` exactly
    /// without going through the cofactor expansion.
    pub fn determinant(&self) -> f64 {
        match self {
            Self::Identity => 1.0,
            Self::Affine(a) => a.determinant(),
        }
    }

    /// Whether the transform is the explicit identity variant. Useful
    /// for callers that want to take a fast path without doing the full
    /// match themselves.
    pub fn is_identity(&self) -> bool {
        matches!(self, Self::Identity)
    }
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
    /// Axis-aligned bounding box over every vertex referenced by any object
    /// in the scene, resolving `Resource` references against `self.resources`.
    /// Returns `None` when the scene has no geometry, and an error if any
    /// resource index is out of bounds.
    pub fn aabb(&self) -> Result<Option<(Vertex, Vertex)>> {
        let mut min = Vertex::new([f32::INFINITY; 3]);
        let mut max = Vertex::new([f32::NEG_INFINITY; 3]);
        let mut has_any = false;

        for obj in &self.objects {
            let mesh = match &obj.content {
                SceneObjectContent::Mesh(m) => m,
                SceneObjectContent::Resource(idx) => self
                    .resources
                    .get(*idx)
                    .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
            };
            for vertex in &mesh.vertices {
                let v = vertex.0;
                min.0[0] = min.0[0].min(v[0]);
                min.0[1] = min.0[1].min(v[1]);
                min.0[2] = min.0[2].min(v[2]);
                max.0[0] = max.0[0].max(v[0]);
                max.0[1] = max.0[1].max(v[1]);
                max.0[2] = max.0[2].max(v[2]);
                has_any = true;
            }
        }

        Ok(has_any.then_some((min, max)))
    }

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

#[cfg(test)]
mod transform_3d_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn translation_affine() -> AffineTransform3D {
        AffineTransform3D::from_translation([1.0, 2.0, 3.0])
    }

    #[test]
    fn identity_is_default() {
        assert_eq!(Transform3D::default(), Transform3D::Identity);
    }

    #[test]
    fn is_identity_distinguishes_variants() {
        assert!(Transform3D::Identity.is_identity());
        assert!(!Transform3D::Affine(translation_affine()).is_identity());
    }

    #[test]
    fn identity_apply_to_vertex_is_pass_through() {
        let v = Vertex::new([1.5, -2.0, 7.25]);
        assert_eq!(Transform3D::Identity.apply_to_vertex(v), v);
    }

    #[test]
    fn affine_apply_to_vertex_delegates_to_inner() {
        let t = Transform3D::Affine(translation_affine());
        let v = Vertex::new([10.0, 20.0, 30.0]);
        let expected = translation_affine().apply_to_vertex(v);
        assert_eq!(t.apply_to_vertex(v), expected);
    }

    #[test]
    fn identity_apply_to_normal_is_pass_through() {
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_eq!(Transform3D::Identity.apply_to_normal(n).unwrap(), n);
    }

    #[test]
    fn affine_apply_to_normal_delegates_to_inner() {
        let t = Transform3D::Affine(AffineTransform3D::from_rotation_z(
            std::f64::consts::FRAC_PI_2,
        ));
        let n = Normal::new([1.0, 0.0, 0.0]);
        let result = t.apply_to_normal(n).unwrap();
        // π/2 rotation about Z sends +X to +Y.
        assert_approx_eq!(f32, result.0[0], 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result.0[1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result.0[2], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn compose_identity_with_anything_returns_other() {
        let a = Transform3D::Affine(translation_affine());
        assert_eq!(Transform3D::Identity.compose(&a), a);
        assert_eq!(a.compose(&Transform3D::Identity), a);
    }

    #[test]
    fn compose_two_identities_is_identity() {
        let composed = Transform3D::Identity.compose(&Transform3D::Identity);
        assert_eq!(composed, Transform3D::Identity);
    }

    #[test]
    fn compose_two_affines_returns_affine() {
        let a = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0]));
        let b = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 2.0, 0.0]));
        match a.compose(&b) {
            Transform3D::Affine(inner) => {
                assert_eq!(inner.translation, [1.0, 2.0, 0.0]);
            }
            Transform3D::Identity => panic!("expected Affine, got Identity"),
        }
    }

    #[test]
    fn inverse_of_identity_is_identity() {
        assert_eq!(
            Transform3D::Identity.inverse().unwrap(),
            Transform3D::Identity
        );
    }

    #[test]
    fn inverse_of_affine_round_trips() {
        let a = Transform3D::Affine(translation_affine());
        let inv = a.inverse().unwrap();
        let composed = a.compose(&inv);
        // The composition equals the affine identity, but the variant is
        // `Affine`, not `Identity` — `compose` doesn't fold an `Affine ∘
        // Affine` back to `Identity`. Check the action on a point.
        let v = Vertex::new([5.0, 7.0, -1.0]);
        let round_trip = composed.apply_to_vertex(v);
        assert_approx_eq!(f32, round_trip.0[0], v.0[0], epsilon = 1e-5);
        assert_approx_eq!(f32, round_trip.0[1], v.0[1], epsilon = 1e-5);
        assert_approx_eq!(f32, round_trip.0[2], v.0[2], epsilon = 1e-5);
    }

    #[test]
    fn inverse_of_singular_affine_propagates_error() {
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        assert!(Transform3D::Affine(singular).inverse().is_err());
    }

    #[test]
    fn determinant_of_identity_is_one() {
        assert_approx_eq!(f64, Transform3D::Identity.determinant(), 1.0);
    }

    #[test]
    fn determinant_of_affine_delegates_to_inner() {
        let t = Transform3D::Affine(AffineTransform3D::from_uniform_scale(2.0));
        assert_approx_eq!(f64, t.determinant(), 8.0);
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
    fn empty_scene_has_no_aabb() {
        assert_eq!(Scene::default().aabb().unwrap(), None);
    }

    #[test]
    fn aabb_covers_inline_and_resource_meshes() {
        let resource = triangle_mesh(
            vec![[1.0, -2.0, 3.0], [4.0, 5.0, -6.0], [0.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        let inline = triangle_mesh(
            vec![[-7.0, 8.0, 0.5], [0.0, 0.0, 9.0], [2.0, 2.0, 2.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![resource],
            objects: vec![resource_object(0), inline_object(inline)],
        };
        let (min, max) = scene.aabb().unwrap().unwrap();
        assert_eq!(min.0, [-7.0, -2.0, -6.0]);
        assert_eq!(max.0, [4.0, 8.0, 9.0]);
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

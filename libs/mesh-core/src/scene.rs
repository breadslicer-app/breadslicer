/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::aabb::Aabb;
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

impl SceneObject {
    /// Resolves the object's referenced mesh against `resources`.
    /// Returns an error if the object is a `Resource(idx)` whose index
    /// is out of bounds.
    pub fn resolve_mesh<'a>(&'a self, resources: &'a [IndexedMesh]) -> Result<&'a IndexedMesh> {
        match &self.content {
            SceneObjectContent::Mesh(m) => Ok(m),
            SceneObjectContent::Resource(idx) => resources
                .get(*idx)
                .ok_or_else(|| anyhow!("Resource index {idx} out of bounds")),
        }
    }

    /// Returns a mesh with the object's transform baked into its
    /// vertices and face normals.
    ///
    /// For `Transform3D::Identity`, this clones the referenced mesh
    /// without touching its data. For `Affine`, every vertex is passed
    /// through `apply_to_vertex`, every face normal through
    /// `apply_to_normal`, and — when the transform's determinant is
    /// negative (a mirror) — each face's vertex order is reversed so
    /// the resulting mesh is consistently oriented in world space.
    ///
    /// The output mesh has no metadata; downstream consumers
    /// (`merge_meshes`, format writers) attach their own.
    pub fn baked_mesh(&self, resources: &[IndexedMesh]) -> Result<IndexedMesh> {
        let mesh = self.resolve_mesh(resources)?;
        match &self.transform {
            Transform3D::Identity => Ok(IndexedMesh {
                meta: None,
                vertices: mesh.vertices.clone(),
                faces: mesh.faces.clone(),
            }),
            Transform3D::Affine(a) => {
                let vertices: Vec<Vertex> = mesh
                    .vertices
                    .iter()
                    .map(|v| a.apply_to_vertex(*v))
                    .collect();
                let reverse_winding = a.determinant() < 0.0;
                let mut faces: Vec<IndexedTriangle> = Vec::with_capacity(mesh.faces.len());
                for face in &mesh.faces {
                    let normal = a.apply_to_normal(face.normal)?;
                    let v = if reverse_winding {
                        [face.vertices[0], face.vertices[2], face.vertices[1]]
                    } else {
                        face.vertices
                    };
                    faces.push(IndexedTriangle {
                        normal,
                        vertices: v,
                    });
                }
                Ok(IndexedMesh {
                    meta: None,
                    vertices,
                    faces,
                })
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub meta: Option<SceneMetadata>,
    pub resources: Vec<IndexedMesh>,
    pub objects: Vec<SceneObject>,
}

impl Scene {
    pub fn new(
        meta: Option<SceneMetadata>,
        resources: Vec<IndexedMesh>,
        objects: Vec<SceneObject>,
    ) -> Self {
        Self {
            meta,
            resources,
            objects,
        }
    }

    pub fn meta(&self) -> &Option<SceneMetadata> {
        &self.meta
    }

    pub fn resources(&self) -> &[IndexedMesh] {
        &self.resources
    }

    pub fn objects(&self) -> &[SceneObject] {
        &self.objects
    }
}

/// Computes the local (object-space) AABB of a mesh's vertices. Returns
/// `None` if the mesh has no vertices.
fn local_aabb(mesh: &IndexedMesh) -> Option<Aabb> {
    let mut iter = mesh.vertices.iter();
    let first = *iter.next()?;
    let mut aabb = Aabb::from_min_max(first, first);
    for vertex in iter {
        aabb.extend(*vertex);
    }
    Some(aabb)
}

/// World-space AABB of an object-local AABB after applying an
/// `AffineTransform3D`, computed in `f64` via Arvo's method:
///
/// > For an AABB with centre `c` and half-extents `h`, the transformed
/// > AABB has centre `M·c + t` and half-extents
/// > `h'ᵢ = Σⱼ |Mᵢⱼ| · hⱼ`.
///
/// Source: Jim Arvo, "Transforming Axis-Aligned Bounding Boxes",
/// *Graphics Gems I* (1990), pp. 548-550. See also Ericson,
/// *Real-Time Collision Detection*, §4.2.6. The output is conservative —
/// it contains every transformed vertex and is exactly tight for
/// axis-aligned scaling and translation.
fn arvo_world_aabb(local: Aabb, a: &AffineTransform3D) -> Aabb {
    let lmin = local.min().coords();
    let lmax = local.max().coords();
    // Promote to f64 so the matrix-vector products match the precision of
    // the rest of the transform module.
    let cx = (lmin[0] as f64 + lmax[0] as f64) * 0.5;
    let cy = (lmin[1] as f64 + lmax[1] as f64) * 0.5;
    let cz = (lmin[2] as f64 + lmax[2] as f64) * 0.5;
    let hx = (lmax[0] as f64 - lmin[0] as f64) * 0.5;
    let hy = (lmax[1] as f64 - lmin[1] as f64) * 0.5;
    let hz = (lmax[2] as f64 - lmin[2] as f64) * 0.5;

    let m = &a.linear;
    let t = &a.translation;

    let center = [
        m[0][0].mul_add(cx, m[0][1].mul_add(cy, m[0][2].mul_add(cz, t[0]))),
        m[1][0].mul_add(cx, m[1][1].mul_add(cy, m[1][2].mul_add(cz, t[1]))),
        m[2][0].mul_add(cx, m[2][1].mul_add(cy, m[2][2].mul_add(cz, t[2]))),
    ];
    let half = [
        m[0][0]
            .abs()
            .mul_add(hx, m[0][1].abs().mul_add(hy, m[0][2].abs() * hz)),
        m[1][0]
            .abs()
            .mul_add(hx, m[1][1].abs().mul_add(hy, m[1][2].abs() * hz)),
        m[2][0]
            .abs()
            .mul_add(hx, m[2][1].abs().mul_add(hy, m[2][2].abs() * hz)),
    ];

    let min = Vertex::new([
        (center[0] - half[0]) as f32,
        (center[1] - half[1]) as f32,
        (center[2] - half[2]) as f32,
    ]);
    let max = Vertex::new([
        (center[0] + half[0]) as f32,
        (center[1] + half[1]) as f32,
        (center[2] + half[2]) as f32,
    ]);
    Aabb::from_min_max(min, max)
}

impl Scene {
    /// Axis-aligned bounding box over every object in the scene,
    /// resolving `Resource` references against `self.resources` and
    /// applying each object's `transform`. Returns `None` when the scene
    /// has no geometry, and an error if any resource index is out of
    /// bounds.
    ///
    /// Identity objects fold their vertices directly into the running
    /// min/max. Affine objects use Arvo's method on the local AABB,
    /// which is conservative and (for non-rotated transforms) tight.
    /// TODO: cache the local AABB on `IndexedMesh` if recomputation
    /// becomes a measured cost — it depends only on the mesh, not on
    /// the transform.
    pub fn aabb(&self) -> Result<Option<Aabb>> {
        let mut aabb = Aabb::empty();
        let mut has_any = false;

        for obj in &self.objects {
            let mesh = match &obj.content {
                SceneObjectContent::Mesh(m) => m,
                SceneObjectContent::Resource(idx) => self
                    .resources
                    .get(*idx)
                    .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
            };

            match &obj.transform {
                Transform3D::Identity => {
                    for vertex in &mesh.vertices {
                        aabb.extend(*vertex);
                        has_any = true;
                    }
                }
                Transform3D::Affine(a) => {
                    if let Some(local) = local_aabb(mesh) {
                        aabb.union(&arvo_world_aabb(local, a));
                        has_any = true;
                    }
                }
            }
        }

        Ok(has_any.then_some(aabb))
    }

    /// Merges all meshes in the scene into a single [`IndexedMesh`],
    /// baking each object's transform into its vertices and face normals.
    /// Returns `Ok(None)` if the scene contains no geometry, or an error
    /// if any resource index is out of bounds or any object's transform
    /// is singular (which makes `apply_to_normal` fail).
    pub fn merge_meshes(&self) -> Result<Option<IndexedMesh>> {
        let mut vertices = Vec::new();
        let mut faces = Vec::new();

        for obj in &self.objects {
            let baked = obj.baked_mesh(&self.resources)?;
            let offset = vertices.len() as u32;
            vertices.extend(baked.vertices);
            for face in baked.faces {
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
        assert_approx_eq!(f32, result[0], 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result[1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result[2], 0.0, epsilon = 1e-6);
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
        assert_approx_eq!(f32, round_trip[0], v[0], epsilon = 1e-5);
        assert_approx_eq!(f32, round_trip[1], v[1], epsilon = 1e-5);
        assert_approx_eq!(f32, round_trip[2], v[2], epsilon = 1e-5);
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
mod scene_object_baked_mesh_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use std::f64::consts::FRAC_PI_2;

    fn unit_triangle_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        }
    }

    fn inline(mesh: IndexedMesh, t: Transform3D) -> SceneObject {
        SceneObject {
            meta: None,
            transform: t,
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    #[test]
    fn resolve_mesh_returns_inline_content() {
        let mesh = unit_triangle_mesh();
        let obj = inline(mesh.clone(), Transform3D::Identity);
        assert_eq!(obj.resolve_mesh(&[]).unwrap(), &mesh);
    }

    #[test]
    fn resolve_mesh_resolves_resource_index() {
        let mesh = unit_triangle_mesh();
        let obj = SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Resource(0),
        };
        assert_eq!(
            obj.resolve_mesh(std::slice::from_ref(&mesh)).unwrap(),
            &mesh
        );
    }

    #[test]
    fn resolve_mesh_errors_on_out_of_bounds_resource() {
        let obj = SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Resource(7),
        };
        assert!(obj.resolve_mesh(&[]).is_err());
    }

    #[test]
    fn baked_mesh_identity_clones_vertices_and_faces() {
        let mesh = unit_triangle_mesh();
        let obj = inline(mesh.clone(), Transform3D::Identity);
        let baked = obj.baked_mesh(&[]).unwrap();
        assert_eq!(baked.vertices, mesh.vertices);
        assert_eq!(baked.faces, mesh.faces);
        assert_eq!(baked.meta, None);
    }

    #[test]
    fn baked_mesh_affine_applies_translation_to_vertices() {
        let obj = inline(
            unit_triangle_mesh(),
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, -5.0, 2.0])),
        );
        let baked = obj.baked_mesh(&[]).unwrap();
        assert_eq!(baked.vertices[0].coords(), [10.0, -5.0, 2.0]);
        assert_eq!(baked.vertices[1].coords(), [11.0, -5.0, 2.0]);
        assert_eq!(baked.vertices[2].coords(), [10.0, -4.0, 2.0]);
    }

    #[test]
    fn baked_mesh_affine_rotates_face_normals() {
        // π/2 rotation about Z: a +Z normal stays at +Z (rotation axis
        // preserves its own direction), but a +X normal rotates to +Y.
        // Use a triangle with a +X normal to make the rotation visible.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([1.0, 0.0, 0.0]),
                vertices: [0, 1, 2],
            }],
        };
        let obj = inline(
            mesh,
            Transform3D::Affine(AffineTransform3D::from_rotation_z(FRAC_PI_2)),
        );
        let baked = obj.baked_mesh(&[]).unwrap();
        assert_approx_eq!(f32, baked.faces[0].normal[0], 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.faces[0].normal[1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f32, baked.faces[0].normal[2], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn baked_mesh_affine_preserves_winding_for_positive_determinant() {
        let obj = inline(
            unit_triangle_mesh(),
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        );
        let baked = obj.baked_mesh(&[]).unwrap();
        assert_eq!(baked.faces[0].vertices, [0, 1, 2]);
    }

    #[test]
    fn baked_mesh_affine_reverses_winding_for_negative_determinant() {
        // Mirror across YZ (det = -1): winding reversed.
        let obj = inline(
            unit_triangle_mesh(),
            Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0])),
        );
        let baked = obj.baked_mesh(&[]).unwrap();
        assert_eq!(baked.faces[0].vertices, [0, 2, 1]);
    }

    #[test]
    fn baked_mesh_resolves_resource_reference() {
        let mesh = unit_triangle_mesh();
        let obj = SceneObject {
            meta: None,
            transform: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0])),
            content: SceneObjectContent::Resource(0),
        };
        let baked = obj.baked_mesh(std::slice::from_ref(&mesh)).unwrap();
        assert_eq!(baked.vertices[0].coords(), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn baked_mesh_drops_source_metadata() {
        let mut mesh = unit_triangle_mesh();
        mesh.meta = Some(crate::indexed_mesh::MeshMetadata {
            name: Some("Source".to_string()),
            ..Default::default()
        });
        let obj = inline(mesh, Transform3D::Identity);
        let baked = obj.baked_mesh(&[]).unwrap();
        assert_eq!(baked.meta, None);
    }

    #[test]
    fn baked_mesh_propagates_singular_normal_error() {
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let obj = inline(unit_triangle_mesh(), Transform3D::Affine(singular));
        assert!(obj.baked_mesh(&[]).is_err());
    }
}

#[cfg(test)]
mod merge_meshes_tests {
    use crate::aabb::Aabb;
    use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use crate::transform::AffineTransform3D;
    use float_cmp::assert_approx_eq;
    use std::f64::consts::FRAC_PI_4;

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
        let aabb = scene.aabb().unwrap().unwrap();
        assert_eq!(aabb.min().coords(), [-7.0, -2.0, -6.0]);
        assert_eq!(aabb.max().coords(), [4.0, 8.0, 9.0]);
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

    /// Unit cube spanning [0, 1]³, made of two triangles on each of its
    /// six faces. Provides 8 distinct vertex positions and 12 faces,
    /// enough for the corner-method-vs-Arvo conservativeness checks.
    fn unit_cube_mesh() -> IndexedMesh {
        let v: Vec<Vertex> = vec![
            Vertex::new([0.0, 0.0, 0.0]), // 0
            Vertex::new([1.0, 0.0, 0.0]), // 1
            Vertex::new([1.0, 1.0, 0.0]), // 2
            Vertex::new([0.0, 1.0, 0.0]), // 3
            Vertex::new([0.0, 0.0, 1.0]), // 4
            Vertex::new([1.0, 0.0, 1.0]), // 5
            Vertex::new([1.0, 1.0, 1.0]), // 6
            Vertex::new([0.0, 1.0, 1.0]), // 7
        ];
        // Faces are arbitrary; only their vertex references matter for
        // AABB. Normals are placeholders.
        let n = Normal::new([0.0, 0.0, 1.0]);
        let face = |a, b, c| IndexedTriangle {
            normal: n,
            vertices: [a, b, c],
        };
        let faces = vec![
            face(0, 1, 2),
            face(0, 2, 3), // -Z
            face(4, 6, 5),
            face(4, 7, 6), // +Z
            face(0, 4, 5),
            face(0, 5, 1), // -Y
            face(3, 6, 7),
            face(3, 2, 6), // +Y
            face(0, 3, 7),
            face(0, 7, 4), // -X
            face(1, 5, 6),
            face(1, 6, 2), // +X
        ];
        IndexedMesh {
            meta: None,
            vertices: v,
            faces,
        }
    }

    fn affine_object(mesh: IndexedMesh, t: AffineTransform3D) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Affine(t),
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn assert_aabb_approx(actual: Aabb, expected: ([f32; 3], [f32; 3]), eps: f32) {
        for (a, e) in actual.min().iter().zip(expected.0.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
        for (a, e) in actual.max().iter().zip(expected.1.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
    }

    /// Reference per-corner AABB: transform the 8 corners of the local
    /// AABB and fold them into a fresh AABB. This is the math the Arvo
    /// formula compresses into closed form; for a unit cube whose
    /// vertices are exactly the corners, it gives the tight world-space
    /// AABB of the transformed vertex set.
    fn corner_aabb(mesh: &IndexedMesh, t: &AffineTransform3D) -> Aabb {
        let local = super::local_aabb(mesh).unwrap();
        let lmin = local.min().coords();
        let lmax = local.max().coords();
        let corners = [
            Vertex::new([lmin[0], lmin[1], lmin[2]]),
            Vertex::new([lmax[0], lmin[1], lmin[2]]),
            Vertex::new([lmin[0], lmax[1], lmin[2]]),
            Vertex::new([lmax[0], lmax[1], lmin[2]]),
            Vertex::new([lmin[0], lmin[1], lmax[2]]),
            Vertex::new([lmax[0], lmin[1], lmax[2]]),
            Vertex::new([lmin[0], lmax[1], lmax[2]]),
            Vertex::new([lmax[0], lmax[1], lmax[2]]),
        ];
        let mut aabb = Aabb::empty();
        for c in corners.iter() {
            aabb.extend(t.apply_to_vertex(*c));
        }
        aabb
    }

    #[test]
    fn aabb_with_identity_transform_matches_unwrapped() {
        // A scene with explicit `Transform3D::Identity` produces the
        // same AABB as one whose objects had no transform field at all
        // — the `Identity` arm uses the same per-vertex fold.
        let mesh = unit_cube_mesh();
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(mesh)],
        };
        let aabb = scene.aabb().unwrap().unwrap();
        assert_eq!(aabb.min().coords(), [0.0, 0.0, 0.0]);
        assert_eq!(aabb.max().coords(), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn aabb_applies_translation() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                unit_cube_mesh(),
                AffineTransform3D::from_translation([10.0, -5.0, 2.0]),
            )],
        };
        let aabb = scene.aabb().unwrap().unwrap();
        assert_aabb_approx(aabb, ([10.0, -5.0, 2.0], [11.0, -4.0, 3.0]), 1e-5);
    }

    #[test]
    fn aabb_applies_non_uniform_scale() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                unit_cube_mesh(),
                AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0]),
            )],
        };
        let aabb = scene.aabb().unwrap().unwrap();
        assert_aabb_approx(aabb, ([0.0, 0.0, 0.0], [2.0, 3.0, 4.0]), 1e-5);
    }

    #[test]
    fn aabb_handles_rotation_centered_cube() {
        // Rotate a cube centred at the origin by 45° about Z. The world
        // AABB has half-extents (√2/2, √2/2, 0.5) (Z extent unchanged).
        // Build a unit cube centred at origin by translating a [0,1]³
        // cube by (-0.5, -0.5, -0.5).
        let centred = AffineTransform3D::from_translation([-0.5, -0.5, -0.5]);
        let rotate = AffineTransform3D::from_rotation_z(FRAC_PI_4);
        let combined = rotate.compose(&centred);
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(unit_cube_mesh(), combined)],
        };
        let aabb = scene.aabb().unwrap().unwrap();
        let half = std::f32::consts::FRAC_1_SQRT_2; // √2 / 2 ≈ 0.7071
        assert_aabb_approx(aabb, ([-half, -half, -0.5], [half, half, 0.5]), 1e-5);
    }

    #[test]
    fn aabb_arvo_matches_corner_aabb_for_a_cube() {
        // For a unit cube whose vertices are exactly the corners of its
        // local AABB, the Arvo method must give the same world AABB as
        // transforming all 8 corners and folding them — they are
        // computing the same set.
        let mesh = unit_cube_mesh();
        let t = AffineTransform3D::from_translation([2.0, 0.0, 0.0])
            .compose(&AffineTransform3D::from_non_uniform_scale([1.5, 0.75, 2.0]))
            .compose(&AffineTransform3D::from_rotation_z(FRAC_PI_4));

        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(mesh.clone(), t)],
        };
        let arvo = scene.aabb().unwrap().unwrap();
        let corners = corner_aabb(&mesh, &t);
        assert_aabb_approx(arvo, (corners.min().coords(), corners.max().coords()), 1e-4);
    }

    #[test]
    fn aabb_arvo_contains_per_vertex_for_non_box_mesh() {
        // For a tetrahedron — whose convex hull is strictly smaller than
        // its local AABB — Arvo's method must STILL produce an AABB
        // that contains every transformed vertex (the conservativeness
        // guarantee). It may be larger than the tightest possible AABB
        // of the transformed vertices, which is fine.
        let tetra = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![
                IndexedTriangle {
                    normal: Normal::new([0.0, 0.0, 1.0]),
                    vertices: [0, 2, 1],
                },
                IndexedTriangle {
                    normal: Normal::new([0.0, -1.0, 0.0]),
                    vertices: [0, 1, 3],
                },
                IndexedTriangle {
                    normal: Normal::new([-1.0, 0.0, 0.0]),
                    vertices: [0, 3, 2],
                },
                IndexedTriangle {
                    normal: Normal::new([1.0, 1.0, 1.0]),
                    vertices: [1, 2, 3],
                },
            ],
        };
        let t = AffineTransform3D::from_translation([5.0, -3.0, 1.0])
            .compose(&AffineTransform3D::from_rotation_z(FRAC_PI_4));

        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(tetra.clone(), t)],
        };
        let arvo = scene.aabb().unwrap().unwrap();

        // Per-vertex transformed AABB.
        let mut pmin = [f32::INFINITY; 3];
        let mut pmax = [f32::NEG_INFINITY; 3];
        for v in tetra.vertices.iter() {
            let w = t.apply_to_vertex(*v).coords();
            for i in 0..3 {
                pmin[i] = pmin[i].min(w[i]);
                pmax[i] = pmax[i].max(w[i]);
            }
        }
        let amin = arvo.min().coords();
        let amax = arvo.max().coords();
        for i in 0..3 {
            assert!(
                amin[i] <= pmin[i] + 1e-5,
                "Arvo min[{i}] = {} is not <= per-vertex min = {}",
                amin[i],
                pmin[i]
            );
            assert!(
                amax[i] >= pmax[i] - 1e-5,
                "Arvo max[{i}] = {} is not >= per-vertex max = {}",
                amax[i],
                pmax[i]
            );
        }
    }

    #[test]
    fn aabb_combines_identity_and_affine_objects() {
        // Two objects: one at identity, one translated. The scene AABB
        // is the union.
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                inline_object(unit_cube_mesh()),
                affine_object(
                    unit_cube_mesh(),
                    AffineTransform3D::from_translation([10.0, 0.0, 0.0]),
                ),
            ],
        };
        let aabb = scene.aabb().unwrap().unwrap();
        assert_aabb_approx(aabb, ([0.0, 0.0, 0.0], [11.0, 1.0, 1.0]), 1e-5);
    }

    #[test]
    fn aabb_with_empty_mesh_under_affine_is_skipped() {
        // An object with no vertices contributes nothing to the AABB,
        // even when it has a non-identity transform.
        let empty = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                empty,
                AffineTransform3D::from_translation([100.0, 0.0, 0.0]),
            )],
        };
        assert_eq!(scene.aabb().unwrap(), None);
    }

    #[test]
    fn merge_meshes_bakes_translation_into_vertices() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                mesh,
                AffineTransform3D::from_translation([10.0, -5.0, 2.0]),
            )],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        assert_eq!(result.vertices[0].coords(), [11.0, -5.0, 2.0]);
        assert_eq!(result.vertices[1].coords(), [10.0, -4.0, 2.0]);
        assert_eq!(result.vertices[2].coords(), [10.0, -5.0, 3.0]);
    }

    #[test]
    fn merge_meshes_bakes_rotation_into_face_normals() {
        // π/2 rotation about Z sends a +X normal to +Y.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([1.0, 0.0, 0.0]),
                vertices: [0, 1, 2],
            }],
        };
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                mesh,
                AffineTransform3D::from_rotation_z(std::f64::consts::FRAC_PI_2),
            )],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        assert_approx_eq!(f32, result.faces[0].normal[0], 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result.faces[0].normal[1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result.faces[0].normal[2], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn merge_meshes_reverses_winding_for_negative_determinant() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                mesh,
                AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]),
            )],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        assert_eq!(result.faces[0].vertices, [0, 2, 1]);
    }

    #[test]
    fn merge_meshes_propagates_singular_transform_error() {
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(mesh, singular)],
        };
        assert!(scene.merge_meshes().is_err());
    }

    #[test]
    fn merge_meshes_offsets_indices_after_baking() {
        // Two translated objects: vertex indices in the second object's
        // faces should be offset by the count from the first.
        let mesh_a = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let mesh_b = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                affine_object(mesh_a, AffineTransform3D::from_translation([0.0, 0.0, 0.0])),
                affine_object(
                    mesh_b,
                    AffineTransform3D::from_translation([10.0, 0.0, 0.0]),
                ),
            ],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        assert_eq!(result.vertices.len(), 6);
        assert_eq!(result.faces.len(), 2);
        assert_eq!(result.faces[0].vertices, [0, 1, 2]);
        assert_eq!(result.faces[1].vertices, [3, 4, 5]);
        // Second object's vertices were translated by (10, 0, 0).
        assert_approx_eq!(f32, result.vertices[3][0], 10.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result.vertices[4][0], 11.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result.vertices[5][0], 10.0, epsilon = 1e-6);
    }

    #[test]
    fn merge_meshes_mixes_identity_and_affine_objects() {
        // Identity object passes through untouched; affine object is
        // baked. Both contribute to the merged result.
        let mesh_id = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let mesh_aff = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                inline_object(mesh_id),
                affine_object(
                    mesh_aff,
                    AffineTransform3D::from_translation([100.0, 0.0, 0.0]),
                ),
            ],
        };
        let result = scene.merge_meshes().unwrap().unwrap();
        // Identity object's vertices are unchanged.
        assert_eq!(result.vertices[0].coords(), [0.0, 0.0, 0.0]);
        assert_eq!(result.vertices[1].coords(), [1.0, 0.0, 0.0]);
        // Affine object's vertices are translated.
        assert_approx_eq!(f32, result.vertices[3][0], 100.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result.vertices[4][0], 101.0, epsilon = 1e-6);
    }
}

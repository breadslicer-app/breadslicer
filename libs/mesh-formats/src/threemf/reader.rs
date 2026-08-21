/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversion from parsed 3MF documents to [`Scene`] graphs.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io::{Read, Seek};

use sc_3mf::model::{
    Matrix3D, MeshData, MeshResolution, ModelDocument, ObjectPayload, ObjectResource, ObjectType,
    parse_model,
};
use sc_3mf::names::MODEL_PART_CONTENT_TYPE;
use sc_3mf::package::Package;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, MeshResourceId, NodeId, SceneMetadata};

use crate::error::LoadThreeMfError;
use crate::messages::LoadThreeMfWarning;

/// The shared expansion budget, re-exported from `sc-mesh-core`
/// (which owns the expansion vocabulary). The parser's
/// definition-before-use rule makes reference cycles impossible, but
/// a component DAG can still multiply, so the expanded leaf count is
/// computed up front and an oversized document is refused before any
/// scene node is built, in the same spirit as
/// [`crate::MAX_MESH_ELEMENT_COUNT`].
pub use sc_scene::MAX_EXPANDED_LEAF_COUNT;

/// Hard ceiling on component nesting depth. The conversion recurses
/// one frame per `<components>` level; the parser's
/// definition-before-use rule bounds depth by the object count, which
/// a crafted file can push high enough to overflow the stack. Real
/// assemblies nest a handful of levels.
pub const MAX_COMPONENT_DEPTH: usize = 64;

/// Reads a 3MF package into a [`Scene`].
///
/// The scene mirrors the loader convention for multi-object formats:
/// one identity-Group root whose children are the build items, in
/// document order. Every mesh object referenced by the build becomes
/// one shared mesh resource; each referencing item (or component)
/// contributes a [`LeafPayload::MeshResource`] leaf. Objects made of
/// components become Group nodes. All lengths are converted from the
/// document unit to millimetres.
pub fn read_threemf<R>(read: &mut R) -> Result<(Scene, Vec<LoadThreeMfWarning>), LoadThreeMfError>
where
    R: Read + Seek,
{
    let package = Package::read_from(read)?;
    read_threemf_from_package(&package)
}

/// Converts an already-parsed OPC package into a [`Scene`], with the
/// same semantics as [`read_threemf`]. Public so package-level
/// callers (project-file readers) can parse the container once and
/// consume other parts from the same [`Package`].
pub fn read_threemf_from_package(
    package: &Package,
) -> Result<(Scene, Vec<LoadThreeMfWarning>), LoadThreeMfError> {
    let start_part = package.start_part()?;
    let content_type = package
        .part_content_type(&start_part)
        .expect("start_part only resolves to parts present in the package");
    if content_type != MODEL_PART_CONTENT_TYPE {
        return Err(LoadThreeMfError::StartPartNotAModel {
            content_type: content_type.to_string(),
        });
    }
    let bytes = package
        .part_data(&start_part)
        .expect("start_part only resolves to parts present in the package");
    let document = parse_model(bytes)?;
    convert_document(&document)
}

struct Converter<'doc> {
    document: &'doc ModelDocument,
    scale: f64,
    scene: Scene,
    /// Object id -> registered mesh resource, so N references to one
    /// object share one resource.
    resources: HashMap<u32, MeshResourceId>,
    /// Object ids reached from the build, for the unbuilt-object
    /// warning and the once-per-object type warning.
    reached: HashSet<u32>,
    warnings: Vec<LoadThreeMfWarning>,
}

/// Counts the leaves the build section would expand to, without
/// building anything. One forward pass suffices: components only
/// reference earlier objects, so each object's count is ready when a
/// later object needs it. Saturating arithmetic keeps a doubling
/// chain from overflowing; a saturated total still compares correctly
/// against the cap.
fn expanded_leaf_count(document: &ModelDocument) -> usize {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for object in &document.objects {
        let count = match &object.payload {
            ObjectPayload::Mesh(_) => 1,
            ObjectPayload::Components(components) => components
                .iter()
                .map(|component| {
                    counts
                        .get(&component.object_id.get())
                        .expect("parse_model enforces definition before use")
                })
                .fold(0_usize, |total, child| total.saturating_add(*child)),
        };
        counts.insert(object.id.get(), count);
    }
    document
        .build
        .iter()
        .map(|item| {
            counts
                .get(&item.object_id.get())
                .expect("parse_model verified every build item reference")
        })
        .fold(0_usize, |total, count| total.saturating_add(*count))
}

fn convert_document(
    document: &ModelDocument,
) -> Result<(Scene, Vec<LoadThreeMfWarning>), LoadThreeMfError> {
    if expanded_leaf_count(document) > MAX_EXPANDED_LEAF_COUNT {
        return Err(LoadThreeMfError::LeafBudgetExceeded {
            limit: MAX_EXPANDED_LEAF_COUNT,
        });
    }

    let mut converter = Converter {
        document,
        scale: document.unit.scale_to_mm(),
        scene: Scene::new(),
        resources: HashMap::new(),
        reached: HashSet::new(),
        warnings: Vec::new(),
    };

    if !document.base_materials.is_empty() {
        converter
            .warnings
            .push(LoadThreeMfWarning::MaterialsDropped {
                group_count: document.base_materials.len(),
            });
    }
    if !document.slice_stacks.is_empty() {
        converter
            .warnings
            .push(LoadThreeMfWarning::SliceDataIgnored {
                stack_count: document.slice_stacks.len(),
            });
    }
    if document.build.is_empty() {
        converter.warnings.push(LoadThreeMfWarning::EmptyBuild);
    }

    let root = converter.scene.add_group(None, Transform3D::Identity);
    converter
        .scene
        .add_root(root)
        .expect("freshly allocated group is not yet a root");

    for item in &document.build {
        let object = document
            .object(item.object_id)
            .expect("parse_model verified every build item reference");
        let transform = convert_transform(item.transform, converter.scale);
        let node = converter.convert_object(object, transform, 0)?;
        converter
            .scene
            .add_child(root, node)
            .expect("freshly converted node has no parent and is not a root");
    }

    for object in &document.objects {
        if !converter.reached.contains(&object.id.get()) {
            converter
                .warnings
                .push(LoadThreeMfWarning::UnbuiltObjectSkipped {
                    id: object.id.get(),
                    name: object.name.clone(),
                });
        }
    }

    let name = document
        .metadata
        .iter()
        .find(|m| m.name == "Title")
        .map(|m| m.value.clone());
    let comment = document
        .metadata
        .iter()
        .find(|m| m.name == "Description")
        .map(|m| m.value.clone());
    if name.is_some() || comment.is_some() {
        converter
            .scene
            .set_meta(Some(SceneMetadata { name, comment }));
    }

    Ok((converter.scene, converter.warnings))
}

impl Converter<'_> {
    /// Converts one object reference into a scene node carrying
    /// `local_transform`: a mesh object becomes a resource-backed
    /// leaf, a components object becomes a Group with one converted
    /// child per component.
    fn convert_object(
        &mut self,
        object: &ObjectResource,
        local_transform: Transform3D,
        depth: usize,
    ) -> Result<NodeId, LoadThreeMfError> {
        if depth > MAX_COMPONENT_DEPTH {
            return Err(LoadThreeMfError::ComponentDepthExceeded {
                limit: MAX_COMPONENT_DEPTH,
            });
        }
        if object.mesh_resolution == MeshResolution::Lowres {
            return Err(LoadThreeMfError::LowresMeshUnsupported {
                object_id: object.id.get(),
            });
        }
        if self.reached.insert(object.id.get()) && object.object_type != ObjectType::Model {
            self.warnings
                .push(LoadThreeMfWarning::NonModelObjectTypeFlattened {
                    id: object.id.get(),
                    object_type: object.object_type,
                });
        }
        match &object.payload {
            ObjectPayload::Mesh(data) => {
                let resource = self.mesh_resource_for(object, data)?;
                let leaf = self
                    .scene
                    .add_leaf(None, local_transform, LeafPayload::MeshResource(resource))
                    .expect("the referenced mesh resource was just registered");
                Ok(leaf)
            }
            ObjectPayload::Components(components) => {
                let group = self.scene.add_group(None, local_transform);
                for component in components {
                    let child_object = self
                        .document
                        .object(component.object_id)
                        .expect("parse_model verified every component reference");
                    let child_transform = convert_transform(component.transform, self.scale);
                    let child = self.convert_object(child_object, child_transform, depth + 1)?;
                    self.scene
                        .add_child(group, child)
                        .expect("freshly converted node has no parent and is not a root");
                }
                Ok(group)
            }
        }
    }

    /// Returns the mesh resource for `object`, converting and
    /// registering its mesh on first use.
    fn mesh_resource_for(
        &mut self,
        object: &ObjectResource,
        data: &MeshData,
    ) -> Result<MeshResourceId, LoadThreeMfError> {
        if let Some(&resource) = self.resources.get(&object.id.get()) {
            return Ok(resource);
        }
        let mesh = convert_mesh(object, data, self.scale)?;
        let resource = self.scene.add_mesh_resource(mesh);
        self.resources.insert(object.id.get(), resource);
        Ok(resource)
    }
}

/// Converts mesh data to an [`IndexedMesh`], scaling document units
/// to millimetres and downcasting to the mesh's f32 coordinates.
fn convert_mesh(
    object: &ObjectResource,
    data: &MeshData,
    scale: f64,
) -> Result<IndexedMesh, LoadThreeMfError> {
    let mut vertices = Vec::with_capacity(data.vertices.len());
    for vertex in &data.vertices {
        let x = (vertex.x * scale) as f32;
        let y = (vertex.y * scale) as f32;
        let z = (vertex.z * scale) as f32;
        // The document values are finite f64, but a magnitude beyond
        // f32 range overflows to infinity in the downcast.
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            return Err(LoadThreeMfError::NonFiniteVertex {
                object_id: object.id.get(),
            });
        }
        vertices.push(Vertex::new([x, y, z]));
    }
    let mut faces = Vec::with_capacity(data.triangles.len());
    for triangle in &data.triangles {
        let indices = [triangle.v1.get(), triangle.v2.get(), triangle.v3.get()];
        // parse_model range-checked every index against the vertex
        // count, so direct indexing cannot go out of bounds.
        let corners = [
            vertices[indices[0] as usize],
            vertices[indices[1] as usize],
            vertices[indices[2] as usize],
        ];
        // 3MF stores no normals; recompute from the CCW winding. A
        // degenerate triangle keeps a zero-normal sentinel so the
        // geometry is not silently dropped; downstream validation
        // flags it as ZeroAreaTriangle.
        let normal = compute_normal_from_triangle_vertices(&corners)
            .unwrap_or_else(|_| Normal::new([0.0, 0.0, 0.0]));
        faces.push(IndexedTriangle::from_raw_indices(normal, indices));
    }
    let meta = object.name.as_ref().map(|name| MeshMetadata {
        name: Some(name.clone()),
        header_comment: None,
    });
    Ok(IndexedMesh::new(meta, vertices, faces))
}

/// Converts a 3MF transform to a [`Transform3D`].
///
/// 3MF uses the row-vector convention: a point is a row on the left
/// of the 4x3 matrix, whose rows are m00..m02, m10..m12, m20..m22 and
/// the translation m30..m32. Our transforms multiply column vectors,
/// so the 3x3 part transposes: linear[i][j] = m[j*3 + i]. The pinning
/// test lives in the sibling unit tests.
///
/// Translations are document lengths and get the millimetre scale;
/// the linear part is unitless. Scaling vertices and translations by
/// the same uniform factor commutes with every linear part, so the
/// composed world geometry comes out in millimetres exactly.
fn convert_transform(matrix: Option<Matrix3D>, scale: f64) -> Transform3D {
    let Some(matrix) = matrix else {
        return Transform3D::Identity;
    };
    if matrix == Matrix3D::IDENTITY {
        return Transform3D::Identity;
    }
    let m = matrix.0;
    let linear = [[m[0], m[3], m[6]], [m[1], m[4], m[7]], [m[2], m[5], m[8]]];
    let translation = [m[9] * scale, m[10] * scale, m[11] * scale];
    Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        linear,
        translation,
    ))
}

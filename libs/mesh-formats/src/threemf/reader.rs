/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversion from parsed 3MF documents to [`Scene`] graphs.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io::{Read, Seek};

use sc_3mf::error::{ModelParseError, PackageReadError};
use sc_3mf::model::resource_ids::ResourceId;
use sc_3mf::model::{
    Matrix3D, MeshData, MeshResolution, ModelDocument, ObjectPayload, ObjectResource, ObjectType,
    parse_model,
};
use sc_3mf::names::MODEL_PART_CONTENT_TYPE;
use sc_3mf::package::{Package, PartName};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, MeshResourceId, NodeId, SceneMetadata};

use crate::error::LoadThreeMfError;
use crate::messages::LoadThreeMfWarning;
use crate::threemf::declared_normals::{
    StoredFacetNormals, read_declared_facet_normals_parts, restore,
};

/// The shared expansion budget, re-exported from `sc-scene` (which owns the
/// expansion vocabulary). The parser's definition-before-use rule makes
/// reference cycles impossible, but a component DAG can still multiply, so the
/// expanded leaf count is computed up front and an oversized document is
/// refused before any scene node is built, in the same spirit as
/// [`crate::MAX_MESH_ELEMENT_COUNT`].
pub use sc_scene::MAX_EXPANDED_LEAF_COUNT;

/// Hard ceiling on component nesting depth. The conversion recurses 1 frame per
/// `<components>` level; the parser's definition-before-use rule bounds depth
/// by the object count, which a crafted file can push high enough to overflow
/// the stack. Real assemblies nest a handful of levels.
pub const MAX_COMPONENT_DEPTH: usize = 64;

/// Reads a 3MF package into a [`Scene`].
///
/// The scene mirrors the loader convention for multi-object formats: a single
/// identity-Group root whose children are the build items, in document order.
/// Every mesh object referenced by the build becomes a single shared mesh
/// resource; each referencing item (or component) contributes a
/// [`LeafPayload::MeshResource`] leaf. Objects made of components become Group
/// nodes. All lengths are converted from the document unit to millimetres.
///
/// Objects that the start part references in other model parts, through the
/// production extension's `p:path` (as Bambu Studio and Orca write every
/// object), are read from those parts, each in its own unit. Parts without a
/// content type are skipped with a warning.
pub fn read_threemf<R>(read: &mut R) -> Result<(Scene, Vec<LoadThreeMfWarning>), LoadThreeMfError>
where
    R: Read + Seek,
{
    let package = Package::read_from(read)?;
    read_threemf_from_package(&package)
}

/// Converts an already-parsed OPC package into a [`Scene`], with the same
/// semantics as [`read_threemf`].
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
    let referenced = read_referenced_models(package, &document)?;
    let mut warnings = Vec::new();
    if !package.untyped_parts().is_empty() {
        warnings.push(LoadThreeMfWarning::UntypedPartsSkipped {
            parts: package
                .untyped_parts()
                .iter()
                .map(|part| part.as_str().to_string())
                .collect(),
        });
    }
    let (declared, declared_warnings) = read_declared_facet_normals_parts(package);
    warnings.extend(declared_warnings);
    let (scene, conversion_warnings) = convert_document(&document, &referenced, declared)?;
    warnings.extend(conversion_warnings);
    Ok((scene, warnings))
}

/// A model part that the start part references through the production
/// extension's `p:path`, parsed.
struct ReferencedModel {
    /// The path as the start part first writes it, ASCII-lowercased, which is
    /// how OPC compares part names.
    key: String,
    /// The path as the start part first writes it, for messages.
    path: String,
    document: ModelDocument,
}

/// Returns the `p:path` of every reference in `document`: its build items and
/// its components.
fn production_paths(document: &ModelDocument) -> impl Iterator<Item = &str> {
    let items = document
        .build
        .iter()
        .filter_map(|item| item.path.as_deref());
    let components = document
        .objects
        .iter()
        .filter_map(|object| match &object.payload {
            ObjectPayload::Components(components) => Some(components),
            ObjectPayload::Mesh(_) => None,
        })
        .flatten()
        .filter_map(|component| component.path.as_deref());
    items.chain(components)
}

/// Parses every model part that `root` references through `p:path`, once
/// each, and checks that every reference finds its object: once these parts
/// pass, every reference resolves during conversion.
fn read_referenced_models(
    package: &Package,
    root: &ModelDocument,
) -> Result<Vec<ReferencedModel>, LoadThreeMfError> {
    let mut models: Vec<ReferencedModel> = Vec::new();
    for path in production_paths(root) {
        let key = path.to_ascii_lowercase();
        if models.iter().any(|model| model.key == key) {
            continue;
        }
        let not_found = || LoadThreeMfError::ReferencedPartNotFound {
            path: path.to_string(),
        };
        let name = PartName::new(path).map_err(|_| not_found())?;
        let Some(content_type) = package.part_content_type(&name) else {
            if let Some(untyped) = package
                .untyped_parts()
                .iter()
                .find(|part| part.as_str().eq_ignore_ascii_case(path))
            {
                return Err(PackageReadError::PartWithoutContentType {
                    part: untyped.as_str().to_string(),
                }
                .into());
            }
            return Err(not_found());
        };
        if content_type != MODEL_PART_CONTENT_TYPE {
            return Err(LoadThreeMfError::ReferencedPartNotAModel {
                path: path.to_string(),
                content_type: content_type.to_string(),
            });
        }
        let bytes = package
            .part_data(&name)
            .expect("a part with a content type has data");
        let document = parse_model(bytes).map_err(|source| LoadThreeMfError::ReferencedModel {
            path: path.to_string(),
            source,
        })?;
        if production_paths(&document).next().is_some() {
            return Err(LoadThreeMfError::NestedProductionPath {
                path: path.to_string(),
            });
        }
        models.push(ReferencedModel {
            key,
            path: path.to_string(),
            document,
        });
    }
    check_references(root, &models)?;
    Ok(models)
}

/// Checks that every `p:path` reference of `root` names an object of its part,
/// and that no build item names an object of type "other", as `parse_model`
/// checks for the references inside 1 document.
fn check_references(
    root: &ModelDocument,
    models: &[ReferencedModel],
) -> Result<(), LoadThreeMfError> {
    let find = |path: &str, id: ResourceId| {
        let model = models
            .iter()
            .find(|model| model.key == path.to_ascii_lowercase())
            .expect("every path was read");
        let object = model.document.objects.iter().find(|object| object.id == id);
        (model, object)
    };
    let unknown = |model: &ReferencedModel, id: ResourceId| LoadThreeMfError::ReferencedModel {
        path: model.path.clone(),
        source: ModelParseError::UnknownObjectReference { id: id.get() },
    };
    for item in &root.build {
        let Some(path) = &item.path else { continue };
        let (model, object) = find(path, item.object_id);
        let object = object.ok_or_else(|| unknown(model, item.object_id))?;
        if object.object_type == ObjectType::Other {
            return Err(LoadThreeMfError::ReferencedModel {
                path: model.path.clone(),
                source: ModelParseError::BuildItemReferencesOtherType {
                    id: item.object_id.get(),
                },
            });
        }
    }
    for object in &root.objects {
        let ObjectPayload::Components(components) = &object.payload else {
            continue;
        };
        for component in components {
            let Some(path) = &component.path else {
                continue;
            };
            let (model, object) = find(path, component.object_id);
            object.ok_or_else(|| unknown(model, component.object_id))?;
        }
    }
    Ok(())
}

/// Index of a model part among the parts being converted: [`ROOT_PART`] for
/// the start part, then the parts it references, in [`ReferencedModel`] order.
type PartIndex = usize;

const ROOT_PART: PartIndex = 0;

/// An object of 1 of the model parts being converted, by part and id.
type ObjectKey = (PartIndex, ResourceId);

struct Converter<'doc> {
    /// Every object of every part, keyed by part and id, so resolving a build
    /// item or a component does not rescan the object lists.
    objects: HashMap<ObjectKey, &'doc ObjectResource>,
    /// Each part's unit as a millimetre factor, by [`PartIndex`].
    scales: Vec<f64>,
    /// Lowercased `p:path` -> part, for the parts the start part references.
    parts_by_path: HashMap<String, PartIndex>,
    scene: Scene,
    /// Object -> registered mesh resource, so N references to the same object
    /// share 1 resource.
    resources: HashMap<ObjectKey, MeshResourceId>,
    /// Objects reached from the build, for the unbuilt-object warning and the
    /// once-per-object type warning.
    reached: HashSet<ObjectKey>,
    /// Declared facet normals read from the package, by object id of the start
    /// part, until the mesh of their object is converted. Breadslicer writes
    /// them only for single-part documents.
    declared: HashMap<u32, StoredFacetNormals>,
    warnings: Vec<LoadThreeMfWarning>,
}

/// Counts the leaves the build section would expand to, without building
/// anything. The count saturates instead of overflowing; a saturated total
/// still compares correctly against the cap.
fn expanded_leaf_count(document: &ModelDocument, referenced: &[ReferencedModel]) -> usize {
    // Referenced parts reference nothing outside themselves, so they count
    // first, and the start part then reads their counts.
    let referenced_counts: Vec<HashMap<u32, usize>> = referenced
        .iter()
        .map(|model| object_leaf_counts(&model.document, &[], &[]))
        .collect();
    let counts = object_leaf_counts(document, referenced, &referenced_counts);
    document
        .build
        .iter()
        .map(|item| match &item.path {
            None => counts.get(&item.object_id.get()),
            Some(path) => referenced_count(referenced, &referenced_counts, path, item.object_id),
        })
        .map(|count| count.expect("every build item reference was verified"))
        .fold(0_usize, |total, count| total.saturating_add(*count))
}

/// Returns how many leaves each object of `document` expands to, by object id.
/// `referenced` and `referenced_counts` resolve its `p:path` components.
fn object_leaf_counts(
    document: &ModelDocument,
    referenced: &[ReferencedModel],
    referenced_counts: &[HashMap<u32, usize>],
) -> HashMap<u32, usize> {
    // A single forward pass suffices: components only reference earlier
    // objects, so each object's count is ready when a later object needs it.
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for object in &document.objects {
        let count = match &object.payload {
            ObjectPayload::Mesh(_) => 1,
            ObjectPayload::Components(components) => components
                .iter()
                .map(|component| match &component.path {
                    None => *counts
                        .get(&component.object_id.get())
                        .expect("parse_model enforces definition before use"),
                    Some(path) => {
                        *referenced_count(referenced, referenced_counts, path, component.object_id)
                            .expect("every component reference was verified")
                    }
                })
                .fold(0_usize, |total, child| total.saturating_add(child)),
        };
        counts.insert(object.id.get(), count);
    }
    counts
}

/// Returns the leaf count of object `id` of the referenced part `path`.
fn referenced_count<'a>(
    referenced: &[ReferencedModel],
    referenced_counts: &'a [HashMap<u32, usize>],
    path: &str,
    id: ResourceId,
) -> Option<&'a usize> {
    let key = path.to_ascii_lowercase();
    let index = referenced.iter().position(|model| model.key == key)?;
    referenced_counts[index].get(&id.get())
}

fn convert_document(
    document: &ModelDocument,
    referenced: &[ReferencedModel],
    declared: HashMap<u32, StoredFacetNormals>,
) -> Result<(Scene, Vec<LoadThreeMfWarning>), LoadThreeMfError> {
    if expanded_leaf_count(document, referenced) > MAX_EXPANDED_LEAF_COUNT {
        return Err(LoadThreeMfError::LeafBudgetExceeded {
            limit: MAX_EXPANDED_LEAF_COUNT,
        });
    }

    let documents: Vec<&ModelDocument> = std::iter::once(document)
        .chain(referenced.iter().map(|model| &model.document))
        .collect();
    let mut converter = Converter {
        objects: documents
            .iter()
            .enumerate()
            .flat_map(|(part, document)| document.objects.iter().map(move |o| ((part, o.id), o)))
            .collect(),
        scales: documents.iter().map(|d| d.unit.scale_to_mm()).collect(),
        parts_by_path: referenced
            .iter()
            .enumerate()
            .map(|(index, model)| (model.key.clone(), ROOT_PART + 1 + index))
            .collect(),
        scene: Scene::new(),
        resources: HashMap::new(),
        reached: HashSet::new(),
        declared,
        warnings: Vec::new(),
    };

    let group_count: usize = documents.iter().map(|d| d.base_materials.len()).sum();
    if group_count > 0 {
        converter
            .warnings
            .push(LoadThreeMfWarning::MaterialsDropped { group_count });
    }
    let stack_count: usize = documents.iter().map(|d| d.slice_stacks.len()).sum();
    if stack_count > 0 {
        converter
            .warnings
            .push(LoadThreeMfWarning::SliceDataIgnored { stack_count });
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
        let part = converter.part_of(ROOT_PART, item.path.as_deref());
        let object = converter
            .objects
            .get(&(part, item.object_id))
            .copied()
            .expect("every build item reference was verified");
        let transform = convert_transform(item.transform, converter.scales[ROOT_PART]);
        let node = converter.convert_object(part, object, transform, 0)?;
        converter
            .scene
            .add_child(root, node)
            .expect("freshly converted node has no parent and is not a root");
    }

    // Objects of referenced parts that nothing reaches are not reported: those
    // parts hold the objects of the start part's assemblies.
    for object in &document.objects {
        if !converter.reached.contains(&(ROOT_PART, object.id)) {
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
    /// Returns the part that a reference from part `from` with `p:path` `path`
    /// points into.
    fn part_of(&self, from: PartIndex, path: Option<&str>) -> PartIndex {
        match path {
            None => from,
            Some(path) => *self
                .parts_by_path
                .get(&path.to_ascii_lowercase())
                .expect("every path was read"),
        }
    }

    /// Converts an object reference into a scene node carrying
    /// `local_transform`: a mesh object becomes a resource-backed leaf, a
    /// components object becomes a Group with 1 converted child per component.
    /// `part` is the model part that defines `object`.
    fn convert_object(
        &mut self,
        part: PartIndex,
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
        if self.reached.insert((part, object.id)) && object.object_type != ObjectType::Model {
            self.warnings
                .push(LoadThreeMfWarning::NonModelObjectTypeFlattened {
                    id: object.id.get(),
                    object_type: object.object_type,
                });
        }
        match &object.payload {
            ObjectPayload::Mesh(data) => {
                let resource = self.mesh_resource_for(part, object, data)?;
                let leaf = self
                    .scene
                    .add_leaf(None, local_transform, LeafPayload::MeshResource(resource))
                    .expect("the referenced mesh resource was just registered");
                Ok(leaf)
            }
            ObjectPayload::Components(components) => {
                let group = self.scene.add_group(None, local_transform);
                for component in components {
                    let child_part = self.part_of(part, component.path.as_deref());
                    let child_object = self
                        .objects
                        .get(&(child_part, component.object_id))
                        .copied()
                        .expect("every component reference was verified");
                    // The transform is written in the units of the part that
                    // holds the component.
                    let child_transform = convert_transform(component.transform, self.scales[part]);
                    let child =
                        self.convert_object(child_part, child_object, child_transform, depth + 1)?;
                    self.scene
                        .add_child(group, child)
                        .expect("freshly converted node has no parent and is not a root");
                }
                Ok(group)
            }
        }
    }

    /// Returns the mesh resource for `object`, converting and registering its
    /// mesh on first use.
    fn mesh_resource_for(
        &mut self,
        part: PartIndex,
        object: &ObjectResource,
        data: &MeshData,
    ) -> Result<MeshResourceId, LoadThreeMfError> {
        if let Some(&resource) = self.resources.get(&(part, object.id)) {
            return Ok(resource);
        }
        let mut mesh = convert_mesh(object, data, self.scales[part])?;
        if part == ROOT_PART
            && let Some(stored) = self.declared.remove(&object.id.get())
            && let Err(warning) = restore(&mut mesh, object.id.get(), stored)
        {
            self.warnings.push(warning);
        }
        let resource = self.scene.add_mesh_resource(mesh);
        self.resources.insert((part, object.id), resource);
        Ok(resource)
    }
}

/// Converts mesh data to an [`IndexedMesh`], scaling document units to
/// millimetres and downcasting to the mesh's f32 coordinates.
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
        // The document values are finite f64, but a magnitude beyond f32 range
        // overflows to infinity in the downcast.
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
        faces.push(IndexedTriangle::from_raw_indices(indices));
    }
    let meta = object.name.as_ref().map(|name| MeshMetadata {
        name: Some(name.clone()),
        ..Default::default()
    });
    Ok(IndexedMesh::new(meta, vertices, faces))
}

/// Converts a 3MF transform to a [`Transform3D`].
///
/// 3MF uses the row-vector convention: a point is a row on the left of the 4x3
/// matrix, whose rows are m00..m02, m10..m12, m20..m22 and the translation
/// m30..m32. Our transforms multiply column vectors, so the 3x3 part
/// transposes: linear[i][j] = m[j*3 + i].
///
/// Translations are document lengths and get the millimetre scale; the linear
/// part is unitless. Scaling vertices and translations by the same uniform
/// factor commutes with every linear part, so the composed world geometry comes
/// out in millimetres exactly.
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

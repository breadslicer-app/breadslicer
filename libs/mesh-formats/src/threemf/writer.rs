/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversion from [`Scene`] graphs to 3MF packages.
//!
//! Unlike the STL/OBJ writers, nothing is flattened or baked: meshes stay
//! shared (1 object per unique mesh), groups become component objects,
//! referenced subtrees serialise once and are referenced per occurrence, and
//! node transforms become 3MF item/component transforms. Coordinates are
//! written as millimetres, the workspace unit.

use std::collections::HashMap;
use std::io::Cursor;
use std::io::Write;
use std::sync::Arc;

use sc_3mf::model::serialization::ModelSerialization;
use sc_3mf::model::{
    BuildItem, Component, Matrix3D, MeshData, MeshResolution, Metadata, ModelDocument,
    ObjectPayload, ObjectResource, ObjectType, ResourceId, ResourceIndex, TriangleRef, Vertex3,
};
use sc_3mf::names::{DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_scene::node::{Node, NodeId, NodeKind, SubtreeResourceId};
use sc_scene::{Scene, SceneError};

use crate::clean_name::CleanName;
use crate::error::WriteThreeMfError;
use crate::ports::WriteSceneProgress;
use crate::threemf::declared_normals::add_declared_facet_normals_parts;

/// Returns an Object or Title name as emitted into the model XML: cleaned
/// ([`CleanName`]: control characters and noncharacters removed, whitespace
/// runs collapsed, trimmed), charset untouched. Beyond name hygiene this is
/// what keeps the document well-formed: XML 1.0 forbids most control characters
/// outright, and quick-xml escapes markup, not illegal characters. A name that
/// cleans to nothing is emitted as no name at all.
fn cleaned_name(name: Option<&str>) -> Option<String> {
    name.and_then(CleanName::clean).map(CleanName::into_string)
}

/// Writes `graph` as a complete 3MF package.
///
/// Scene roots map to build items through the collapse rule that inverts the
/// loader convention: an anonymous identity-transform Group root contributes
/// each of its children as a build item, while any other root becomes a build
/// item itself. The zip layer needs a seekable sink, so the package is
/// serialized into memory first and then copied to `writer`. Meshes that hold
/// declared facet normals matching their geometry also get a
/// declared-facet-normals part.
pub fn write_threemf<W>(graph: &Scene, writer: &mut W) -> Result<(), WriteThreeMfError>
where
    W: Write,
{
    write_threemf_with_progress(graph, writer, |_| {})
}

/// Writes as [`write_threemf`] does, and reports `Serialize` progress in XML
/// bytes against an estimated total (real output can run over or under, so a
/// consumer deriving a fraction should clamp it) and `Compress` progress in raw
/// part bytes as the zip layer consumes them.
pub fn write_threemf_with_progress<W, F>(
    graph: &Scene,
    writer: &mut W,
    mut progress: F,
) -> Result<(), WriteThreeMfError>
where
    W: Write,
    F: FnMut(WriteSceneProgress),
{
    let model = scene_to_model_document(graph)?;

    let serialization = ModelSerialization::new([&model.document]);
    let total = serialization.estimated_total_bytes();
    // Opens the phase before the first byte: `write` reports only bytes
    // written.
    progress(WriteSceneProgress::Serialize { done: 0, total });
    let [model_xml] = serialization.write(|report| {
        progress(WriteSceneProgress::Serialize {
            done: report.serialized_bytes,
            total: report.estimated_total_bytes,
        })
    })?;

    let mut package = Package::new();
    let part = PartName::new(DEFAULT_MODEL_PART_NAME).expect("constant part name is valid");
    package.add_part(part, MODEL_PART_CONTENT_TYPE, model_xml)?;
    package.add_root_relationship(Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: DEFAULT_MODEL_PART_NAME.to_string(),
    });
    add_declared_facet_normals_parts(&mut package, &model.mesh_objects)?;

    let mut buffer = Cursor::new(Vec::new());
    package.write_to_with_progress(&mut buffer, |done, total| {
        progress(WriteSceneProgress::Compress { done, total })
    })?;
    writer.write_all(buffer.get_ref())?;
    Ok(())
}

struct DocumentBuilder<'graph> {
    graph: &'graph Scene,
    document: ModelDocument,
    /// Mesh identity (Arc pointer) -> the object id it was emitted as, so every
    /// leaf sharing a mesh references the same object.
    mesh_objects: HashMap<*const IndexedMesh, ResourceId>,
    /// Subtree resource -> the object id its root was emitted as, so every
    /// reference to a subtree names the same object.
    subtree_objects: HashMap<SubtreeResourceId, ResourceId>,
    /// Each mesh object emitted so far, with the mesh it was converted from.
    emitted_mesh_objects: Vec<(ResourceId, &'graph IndexedMesh)>,
    next_id: u32,
}

/// A scene converted to a 3MF model document, with the mesh that each mesh
/// object of the document was converted from, in document order.
#[derive(Debug)]
pub struct SceneModel<'graph> {
    pub document: ModelDocument,
    pub mesh_objects: Vec<(ResourceId, &'graph IndexedMesh)>,
}

/// Converts `graph` into a 3MF model document without touching any container
/// concerns, and returns with it the mesh of each mesh object, which
/// [`add_declared_facet_normals_parts`] reads. Scene roots map to build items
/// through the same collapse rule as [`write_threemf`]. Public so writers that
/// assemble their own packages (project files, slice packages) can reuse the
/// scene conversion.
pub fn scene_to_model_document(graph: &Scene) -> Result<SceneModel<'_>, WriteThreeMfError> {
    let mut builder = DocumentBuilder {
        graph,
        document: ModelDocument::default(),
        mesh_objects: HashMap::new(),
        subtree_objects: HashMap::new(),
        emitted_mesh_objects: Vec::new(),
        next_id: 1,
    };

    for &root in graph.roots() {
        let node = builder.node(root)?;
        if is_anonymous_identity_group(node) {
            let children = match &node.kind {
                NodeKind::Group { children } => children.clone(),
                _ => unreachable!("is_anonymous_identity_group checked the kind"),
            };
            for child in children {
                builder.add_build_item(child)?;
            }
        } else {
            builder.add_build_item(root)?;
        }
    }

    if let Some(meta) = graph.meta() {
        if let Some(name) = meta.name.as_deref().and_then(CleanName::clean) {
            builder.document.metadata.push(Metadata {
                name: "Title".to_string(),
                value: name.into_string(),
                preserve: false,
                value_type: None,
            });
        }
        if let Some(comment) = &meta.comment {
            builder.document.metadata.push(Metadata {
                name: "Description".to_string(),
                value: comment.clone(),
                preserve: false,
                value_type: None,
            });
        }
    }

    Ok(SceneModel {
        document: builder.document,
        mesh_objects: builder.emitted_mesh_objects,
    })
}

/// Returns whether `node` is the anonymous identity-transform Group that the
/// loader wraps every build item under; such a root unwraps back into its
/// children on write. A named or transformed group carries meaning of its own
/// and stays a component object instead.
fn is_anonymous_identity_group(node: &Node) -> bool {
    node.meta.is_none()
        && node.local_transform.is_identity()
        && matches!(node.kind, NodeKind::Group { .. })
}

impl<'graph> DocumentBuilder<'graph> {
    fn node(&self, id: NodeId) -> Result<&'graph Node, WriteThreeMfError> {
        self.graph
            .node(id)
            .ok_or(WriteThreeMfError::Graph(SceneError::DanglingNodeId { id }))
    }

    fn alloc_id(&mut self) -> ResourceId {
        let id = ResourceId::new(self.next_id).expect("fewer than 2^31 objects in a scene");
        self.next_id += 1;
        id
    }

    fn add_build_item(&mut self, node_id: NodeId) -> Result<(), WriteThreeMfError> {
        let (object_id, transform) = self.object_for_node(node_id)?;
        self.document.build.push(BuildItem {
            object_id,
            transform,
            partnumber: None,
        });
        Ok(())
    }

    /// Emits (or reuses) the object resource representing the geometry at
    /// `node_id` and returns its id together with the node's transform in
    /// attribute form. Children are emitted before the components object that
    /// references them, which satisfies the spec's definition-before-use rule.
    fn object_for_node(
        &mut self,
        node_id: NodeId,
    ) -> Result<(ResourceId, Option<Matrix3D>), WriteThreeMfError> {
        let node = self.node(node_id)?;
        let transform = transform_to_matrix(&node.local_transform);
        match &node.kind {
            NodeKind::Leaf(_) => {
                let arc = self
                    .graph
                    .leaf_mesh_arc(node_id)
                    .expect("a live leaf resolves to its mesh by Scene invariants");
                let key = Arc::as_ptr(arc);
                if let Some(&existing) = self.mesh_objects.get(&key) {
                    return Ok((existing, transform));
                }
                let mesh = arc.as_ref();
                let id = self.alloc_id();
                self.document.objects.push(ObjectResource {
                    id,
                    object_type: ObjectType::Model,
                    name: cleaned_name(mesh.meta().as_ref().and_then(|m| m.name.as_deref())),
                    partnumber: None,
                    property_ref: None,
                    slice_stack: None,
                    mesh_resolution: MeshResolution::default(),
                    payload: ObjectPayload::Mesh(mesh_to_model_data(mesh)),
                });
                self.mesh_objects.insert(key, id);
                self.emitted_mesh_objects.push((id, mesh));
                Ok((id, transform))
            }
            NodeKind::Group { children } => {
                let children = children.clone();
                let mut components = Vec::with_capacity(children.len());
                for child in children {
                    let (child_id, child_transform) = self.object_for_node(child)?;
                    components.push(Component {
                        object_id: child_id,
                        transform: child_transform,
                    });
                }
                let id = self.alloc_id();
                self.document.objects.push(ObjectResource {
                    id,
                    object_type: ObjectType::Model,
                    name: cleaned_name(node.meta.as_ref().and_then(|m| m.name.as_deref())),
                    partnumber: None,
                    property_ref: None,
                    slice_stack: None,
                    mesh_resolution: MeshResolution::default(),
                    payload: ObjectPayload::Components(components),
                });
                Ok((id, transform))
            }
            NodeKind::Csg { .. } => Err(WriteThreeMfError::CsgUnrepresentable { node_id }),
            NodeKind::Reference { resource } => {
                // The referenced subtree serialises once (its root becomes an
                // object, recursively) and every reference site names that
                // object. A 3MF reference carries a single matrix, so the
                // call-site transform composes with the subtree root's own
                // transform, call site first, matching the expanding walk.
                let resource = *resource;
                let root_id =
                    self.graph
                        .subtree_resource_root(resource)
                        .ok_or(WriteThreeMfError::Graph(
                            SceneError::DanglingSubtreeResource { id: resource },
                        ))?;
                let root_local = self.node(root_id)?.local_transform;
                let object_id = match self.subtree_objects.get(&resource) {
                    Some(&existing) => existing,
                    None => {
                        let (id, _) = self.object_for_node(root_id)?;
                        self.subtree_objects.insert(resource, id);
                        id
                    }
                };
                let combined = node.local_transform.compose(&root_local);
                Ok((object_id, transform_to_matrix(&combined)))
            }
        }
    }
}

/// Converts an [`IndexedMesh`] into the 3MF mesh payload: vertices widened to
/// f64 (millimetres both sides) and faces mapped to vertex-index triples. Face
/// normals are not part of the 3MF data model and are dropped; readers rederive
/// them from the CCW winding. Public so slice-format writers can embed meshes
/// without re-implementing the mapping.
pub fn mesh_to_model_data(mesh: &IndexedMesh) -> MeshData {
    MeshData {
        vertices: mesh
            .vertices
            .iter()
            .map(|v| Vertex3 {
                x: f64::from(v[0]),
                y: f64::from(v[1]),
                z: f64::from(v[2]),
            })
            .collect(),
        triangles: mesh
            .faces
            .iter()
            .map(|f| TriangleRef {
                v1: vertex_index(f.vertices[0].get()),
                v2: vertex_index(f.vertices[1].get()),
                v3: vertex_index(f.vertices[2].get()),
            })
            .collect(),
    }
}

fn vertex_index(raw: u32) -> ResourceIndex {
    ResourceIndex::new(raw).expect("mesh vertex counts stay below 2^31")
}

/// Returns the 3MF matrix of `transform`, the inverse of the reader's matrix
/// conversion: our column-vector linear part transposes back into 3MF's
/// row-vector layout, row j holding linear[0][j], linear[1][j], linear[2][j],
/// with the translation as the final row. Identity transforms omit the
/// attribute entirely.
fn transform_to_matrix(transform: &Transform3D) -> Option<Matrix3D> {
    match transform {
        Transform3D::Identity => None,
        Transform3D::Affine(affine) => {
            let l = affine.linear();
            let t = affine.translation();
            Some(Matrix3D([
                l[0][0], l[1][0], l[2][0], //
                l[0][1], l[1][1], l[2][1], //
                l[0][2], l[1][2], l[2][2], //
                t[0], t[1], t[2],
            ]))
        }
    }
}

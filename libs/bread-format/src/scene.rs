/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The scene chunk: the node graph, without the meshes.
//!
//! Meshes travel in their own chunks ([`crate::mesh`]); the scene names them by
//! their order. A mesh shared by several leaves or resources is stored once
//! and comes back shared.
//!
//! Nodes are listed children first, so every record names only nodes and
//! subtree resources listed before it, and the decoder rebuilds the scene
//! through the scene's own checked API. Node and resource ids are renumbered
//! in that order; the graph, names, transforms and child order come back as
//! they were. Nodes no root or reference reaches are not part of the scene
//! and are not stored. Leaves whose mesh the application rebuilds (supports)
//! can be stored with an empty mesh ([`encode_scene_emptying`]).
//!
//! ```text
//! metadata    u8 0 | 1, then name and comment (u8 0 | 1, u32 length, UTF-8)
//! meshes      u32 number of mesh chunks the scene names
//! resources   u32 count, then u32 mesh per mesh resource
//! records     u32 count, then per record a u8 kind:
//!               0 group      node, u32 count, u32 child nodes
//!               1 leaf       node, u32 mesh
//!               2 leaf       node, u32 mesh resource
//!               3 CSG        node, u8 union | difference | intersection,
//!                            u32 count, u32 child nodes
//!               4 reference  node, u32 subtree resource
//!               5 subtree    u32 node, registered as the next subtree resource
//!             where node is its metadata, then its transform:
//!             u8 0 (identity) | 1 and 12 f64 (rows of the linear part, then
//!             the translation)
//! roots       u32 count, u32 nodes
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use sc_mesh_core::indexed_mesh::{IndexedMesh, MeshMetadata};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{
    CsgOp, LeafPayload, MeshResourceId, NodeId, NodeKind, Scene, SceneError, SceneMetadata,
    SubtreeResourceId,
};

use crate::bytes::{In, Out};
use crate::container::ChunkTag;

/// The tag of the scene chunk.
pub const SCENE_TAG: ChunkTag = ChunkTag(*b"SCNE");

/// The scene payload layout this build writes and reads.
pub const SCENE_VERSION: u16 = 1;

const GROUP: u8 = 0;
const LEAF_MESH: u8 = 1;
const LEAF_RESOURCE: u8 = 2;
const CSG: u8 = 3;
const REFERENCE: u8 = 4;
const SUBTREE: u8 = 5;

/// Why a scene could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeSceneError {
    /// A node or resource the graph names is missing, or a subtree resource
    /// reaches itself.
    Inconsistent,
}

impl fmt::Display for EncodeSceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the scene graph is inconsistent")
    }
}

impl std::error::Error for EncodeSceneError {}

/// Why a scene payload could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeSceneError {
    /// The payload is cut short, padded, or names something not listed
    /// before it.
    Malformed,
    /// The payload names a different number of meshes than the file holds.
    MeshCount { named: usize, found: usize },
    /// The scene refused a record.
    Scene(SceneError),
}

impl fmt::Display for DecodeSceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("the scene data is malformed"),
            Self::MeshCount { named, found } => {
                write!(
                    f,
                    "the scene names {named} meshes but the file holds {found}"
                )
            }
            Self::Scene(err) => write!(f, "the scene data is inconsistent: {err}"),
        }
    }
}

impl std::error::Error for DecodeSceneError {}

impl From<SceneError> for DecodeSceneError {
    fn from(err: SceneError) -> Self {
        Self::Scene(err)
    }
}

/// An encoded scene and the meshes it names, in order: each goes in a mesh
/// chunk.
#[derive(Clone, Debug)]
pub struct EncodedScene {
    pub payload: Vec<u8>,
    pub meshes: Vec<Arc<IndexedMesh>>,
    /// The index each stored node is listed at, for other chunks to name it.
    pub nodes: HashMap<NodeId, usize>,
}

/// A decoded scene and its nodes in the order they were listed.
#[derive(Clone, Debug)]
pub struct DecodedScene {
    pub scene: Scene,
    pub nodes: Vec<NodeId>,
}

/// Encodes `scene` as a scene chunk payload plus the meshes it names.
pub fn encode_scene(scene: &Scene) -> Result<EncodedScene, EncodeSceneError> {
    encode_scene_emptying(scene, &HashSet::new())
}

/// Encodes `scene` as [`encode_scene`] does, storing the leaves in `emptied`
/// with an empty mesh that keeps only the mesh's name and comment: meshes the
/// application rebuilds from other chunks, such as supports.
pub fn encode_scene_emptying(
    scene: &Scene,
    emptied: &HashSet<NodeId>,
) -> Result<EncodedScene, EncodeSceneError> {
    let mut encoder = Encoder {
        scene,
        emptied,
        records: Out::default(),
        record_count: 0,
        nodes: HashMap::new(),
        subtrees: HashMap::new(),
        open_subtrees: HashSet::new(),
        meshes: Vec::new(),
        mesh_index: HashMap::new(),
        resources: HashMap::new(),
    };
    let mut out = Out::default();
    write_meta(&mut out, scene.meta().as_ref());

    let mut resources = Vec::new();
    for (id, _) in scene.mesh_resources_iter() {
        let arc = scene
            .mesh_resource_arc(id)
            .ok_or(EncodeSceneError::Inconsistent)?;
        let mesh = encoder.mesh(arc);
        encoder.resources.insert(id, resources.len());
        resources.push(mesh);
    }
    let roots = scene
        .roots()
        .iter()
        .map(|&root| encoder.node(root))
        .collect::<Result<Vec<_>, _>>()?;

    out.len(encoder.meshes.len());
    out.len(resources.len());
    for mesh in resources {
        out.len(mesh);
    }
    out.len(encoder.record_count);
    out.0.extend_from_slice(&encoder.records.0);
    out.len(roots.len());
    for root in roots {
        out.len(root);
    }
    Ok(EncodedScene {
        payload: out.0,
        meshes: encoder.meshes,
        nodes: encoder.nodes,
    })
}

struct Encoder<'s> {
    scene: &'s Scene,
    emptied: &'s HashSet<NodeId>,
    records: Out,
    record_count: usize,
    nodes: HashMap<NodeId, usize>,
    subtrees: HashMap<SubtreeResourceId, usize>,
    open_subtrees: HashSet<SubtreeResourceId>,
    meshes: Vec<Arc<IndexedMesh>>,
    mesh_index: HashMap<*const IndexedMesh, usize>,
    resources: HashMap<MeshResourceId, usize>,
}

impl Encoder<'_> {
    fn mesh(&mut self, arc: &Arc<IndexedMesh>) -> usize {
        *self.mesh_index.entry(Arc::as_ptr(arc)).or_insert_with(|| {
            self.meshes.push(Arc::clone(arc));
            self.meshes.len() - 1
        })
    }

    /// Lists `id` after everything it names; returns its node index.
    fn node(&mut self, id: NodeId) -> Result<usize, EncodeSceneError> {
        if let Some(&index) = self.nodes.get(&id) {
            return Ok(index);
        }
        let node = self.scene.node(id).ok_or(EncodeSceneError::Inconsistent)?;
        let (kind, tail) = match &node.kind {
            NodeKind::Leaf(payload) if self.emptied.contains(&id) => {
                let mesh = match payload {
                    LeafPayload::Mesh(arc) => Some(arc),
                    LeafPayload::MeshResource(resource) => self.scene.mesh_resource_arc(*resource),
                }
                .ok_or(EncodeSceneError::Inconsistent)?;
                let meta = mesh.meta().as_ref().map(|meta| MeshMetadata {
                    name: meta.name.clone(),
                    header_comment: meta.header_comment.clone(),
                    declared_facet_normals: None,
                });
                let mut tail = Out::default();
                tail.len(self.mesh(&Arc::new(IndexedMesh::new(meta, vec![], vec![]))));
                (LEAF_MESH, tail)
            }
            NodeKind::Group { children } => (GROUP, self.children(children)?),
            NodeKind::Csg { op, children } => {
                let mut tail = Out::default();
                tail.u8(match op {
                    CsgOp::Union => 0,
                    CsgOp::Difference => 1,
                    CsgOp::Intersection => 2,
                });
                tail.0.extend_from_slice(&self.children(children)?.0);
                (CSG, tail)
            }
            NodeKind::Leaf(LeafPayload::Mesh(arc)) => {
                let mut tail = Out::default();
                tail.len(self.mesh(arc));
                (LEAF_MESH, tail)
            }
            NodeKind::Leaf(LeafPayload::MeshResource(resource)) => {
                let mut tail = Out::default();
                tail.len(
                    *self
                        .resources
                        .get(resource)
                        .ok_or(EncodeSceneError::Inconsistent)?,
                );
                (LEAF_RESOURCE, tail)
            }
            NodeKind::Reference { resource } => {
                let mut tail = Out::default();
                tail.len(self.subtree(*resource)?);
                (REFERENCE, tail)
            }
        };
        self.records.u8(kind);
        write_meta(&mut self.records, node.meta.as_ref());
        write_transform(&mut self.records, &node.local_transform);
        self.records.0.extend_from_slice(&tail.0);
        self.record_count += 1;
        let index = self.nodes.len();
        self.nodes.insert(id, index);
        Ok(index)
    }

    fn children(&mut self, children: &[NodeId]) -> Result<Out, EncodeSceneError> {
        let mut tail = Out::default();
        tail.len(children.len());
        for &child in children {
            let index = self.node(child)?;
            tail.len(index);
        }
        Ok(tail)
    }

    /// Lists the subtree resource `id` and its nodes; returns its index.
    fn subtree(&mut self, id: SubtreeResourceId) -> Result<usize, EncodeSceneError> {
        if let Some(&index) = self.subtrees.get(&id) {
            return Ok(index);
        }
        if !self.open_subtrees.insert(id) {
            return Err(EncodeSceneError::Inconsistent);
        }
        let root = self
            .scene
            .subtree_resource_root(id)
            .ok_or(EncodeSceneError::Inconsistent)?;
        let root = self.node(root)?;
        self.records.u8(SUBTREE);
        self.records.len(root);
        self.record_count += 1;
        self.open_subtrees.remove(&id);
        let index = self.subtrees.len();
        self.subtrees.insert(id, index);
        Ok(index)
    }
}

/// Decodes a scene chunk payload over the meshes of the file's mesh chunks,
/// in file order.
pub fn decode_scene(
    payload: &[u8],
    meshes: Vec<Arc<IndexedMesh>>,
) -> Result<DecodedScene, DecodeSceneError> {
    let mut input = In(payload);
    let mut scene = Scene::new();
    scene.set_meta(read_meta(&mut input).ok_or(DecodeSceneError::Malformed)?);
    let named = input.len().ok_or(DecodeSceneError::Malformed)?;
    if named != meshes.len() {
        return Err(DecodeSceneError::MeshCount {
            named,
            found: meshes.len(),
        });
    }
    let mesh = |index: usize| {
        meshes
            .get(index)
            .cloned()
            .ok_or(DecodeSceneError::Malformed)
    };

    let mut resources = Vec::new();
    for _ in 0..input.count(4).ok_or(DecodeSceneError::Malformed)? {
        let index = input.len().ok_or(DecodeSceneError::Malformed)?;
        resources.push(scene.add_mesh_resource_arc(mesh(index)?));
    }

    let mut nodes: Vec<NodeId> = Vec::new();
    let mut subtrees: Vec<SubtreeResourceId> = Vec::new();
    for _ in 0..input.count(5).ok_or(DecodeSceneError::Malformed)? {
        let malformed = || DecodeSceneError::Malformed;
        let kind = input.u8().ok_or_else(malformed)?;
        if kind == SUBTREE {
            let root = node_at(&nodes, input.len())?;
            subtrees.push(scene.register_subtree_resource(root)?);
            continue;
        }
        let meta = read_meta(&mut input).ok_or_else(malformed)?;
        let transform = read_transform(&mut input).ok_or_else(malformed)?;
        let id = match kind {
            GROUP => {
                let children = read_children(&mut input, &nodes)?;
                let group = scene.add_group(meta, transform);
                for child in children {
                    scene.add_child(group, child)?;
                }
                group
            }
            CSG => {
                let op = match input.u8().ok_or_else(malformed)? {
                    0 => CsgOp::Union,
                    1 => CsgOp::Difference,
                    2 => CsgOp::Intersection,
                    _ => return Err(malformed()),
                };
                let children = read_children(&mut input, &nodes)?;
                scene.add_csg(meta, transform, op, children)?
            }
            LEAF_MESH => {
                let index = input.len().ok_or_else(malformed)?;
                scene.add_leaf(meta, transform, LeafPayload::Mesh(mesh(index)?))?
            }
            LEAF_RESOURCE => {
                let index = input.len().ok_or_else(malformed)?;
                let resource = *resources.get(index).ok_or_else(malformed)?;
                scene.add_leaf(meta, transform, LeafPayload::MeshResource(resource))?
            }
            REFERENCE => {
                let index = input.len().ok_or_else(malformed)?;
                let resource = *subtrees.get(index).ok_or_else(malformed)?;
                scene.add_reference(meta, transform, resource)?
            }
            _ => return Err(malformed()),
        };
        nodes.push(id);
    }

    for _ in 0..input.count(4).ok_or(DecodeSceneError::Malformed)? {
        scene.add_root(node_at(&nodes, input.len())?)?;
    }
    if !input.is_empty() {
        return Err(DecodeSceneError::Malformed);
    }
    Ok(DecodedScene { scene, nodes })
}

fn node_at(nodes: &[NodeId], index: Option<usize>) -> Result<NodeId, DecodeSceneError> {
    index
        .and_then(|index| nodes.get(index).copied())
        .ok_or(DecodeSceneError::Malformed)
}

fn read_children(input: &mut In<'_>, nodes: &[NodeId]) -> Result<Vec<NodeId>, DecodeSceneError> {
    let count = input.count(4).ok_or(DecodeSceneError::Malformed)?;
    (0..count).map(|_| node_at(nodes, input.len())).collect()
}

fn write_meta(out: &mut Out, meta: Option<&SceneMetadata>) {
    match meta {
        None => out.u8(0),
        Some(meta) => {
            out.u8(1);
            out.text(meta.name.as_deref());
            out.text(meta.comment.as_deref());
        }
    }
}

fn read_meta(input: &mut In<'_>) -> Option<Option<SceneMetadata>> {
    match input.u8()? {
        0 => Some(None),
        1 => Some(Some(SceneMetadata {
            name: input.text()?,
            comment: input.text()?,
        })),
        _ => None,
    }
}

fn write_transform(out: &mut Out, transform: &Transform3D) {
    match transform {
        Transform3D::Identity => out.u8(0),
        Transform3D::Affine(affine) => {
            out.u8(1);
            for value in affine.linear().iter().flatten().chain(affine.translation()) {
                out.f64(*value);
            }
        }
    }
}

fn read_transform(input: &mut In<'_>) -> Option<Transform3D> {
    match input.u8()? {
        0 => Some(Transform3D::Identity),
        1 => {
            let mut values = [0.0; 12];
            for value in &mut values {
                *value = input.f64()?;
            }
            let linear = [
                [values[0], values[1], values[2]],
                [values[3], values[4], values[5]],
                [values[6], values[7], values[8]],
            ];
            Some(Transform3D::Affine(
                AffineTransform3D::from_linear_and_translation(
                    linear,
                    [values[9], values[10], values[11]],
                ),
            ))
        }
        _ => None,
    }
}

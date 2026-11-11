/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Breadslicer's scene in a `.bread` file, through the `bread-format` crate
//! (#201, #202).
//!
//! The crate names everything by index; Breadslicer's [`Scene`] by id. This
//! module turns one into the other: nodes are listed children first, a mesh
//! several leaves or resources show is stored once and comes back shared, and
//! nodes no root or reference reaches are left out, as are mesh resources no
//! stored leaf shows (#220).
//!
//! Support leaves are stored with an empty mesh that keeps only its name and
//! comment: their supports are rebuilt from their documents after opening.
//! Each supports entry names the model the supports stand on, and each
//! hollows entry the mesh the model was hollowed from, which the file stores
//! as one more mesh (#211).
//!
//! Surface paint (#183) rides in the scene. Each mask is a document of
//! Breadslicer's, version 1:
//!
//! ```text
//! u32 face count of the mesh
//! 48 bytes, the mesh's geometry fingerprint
//! u32 run count, per run: u32 length, u8 value
//! ```
//!
//! The runs give the value of the faces in face order and cover them all: 0
//! is bare, 255 painted, the values between a soft brush's. A mask painted on
//! another mesh than the one its leaf shows is left out on opening, since the
//! faces no longer mean the same.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use bread_format as bf;
use rayon::prelude::*;
use sc_mesh_core::indexed_mesh::{GeometryFingerprint, IndexedMesh, MeshMetadata};
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{
    LeafPayload, MeshResourceId, NodeId, NodeKind, PaintKind, Scene, SceneMetadata,
    SubtreeResourceId, SurfaceTags,
};

use crate::error::{ReadBreadProjectError, WriteBreadProjectError};

/// The supports of one model, kept as data: their generator's own document
/// (tips, trunks, joints, rafts and the settings they were built with).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportDocument {
    /// The model leaf the supports stand on. Always set when saving; `None`
    /// only for supports a file saved before #211 kept, which name no model.
    pub model: Option<NodeId>,
    /// The leaf that shows the supports. The file stores it with an empty
    /// mesh, rebuilt from `document` after opening.
    pub leaf: NodeId,
    /// Which generator wrote `document`, such as `"df"`.
    pub generator: String,
    /// The generator's own version of `document`.
    pub version: u32,
    pub document: Vec<u8>,
}

/// How one model was hollowed (#161): the mesh it was hollowed from, and its
/// hollower's own document (the settings and the drains).
#[derive(Clone, Debug)]
pub struct HollowDocument {
    /// The leaf that shows the hollow.
    pub leaf: NodeId,
    /// The mesh the model was hollowed from.
    pub original: Arc<IndexedMesh>,
    /// Which hollower wrote `document`, such as `"df"`.
    pub hollower: String,
    /// The hollower's own version of `document`.
    pub version: u32,
    pub document: Vec<u8>,
}

/// The surface paint document this build writes and reads.
pub const SURFACE_DOCUMENT_VERSION: u32 = 1;

/// Paint a file holds that did not make it into the scene.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceNote {
    /// The paint document does not decode.
    Malformed,
    /// Paint of a kind this build does not know.
    UnknownKind { code: u8 },
    /// Paint written in a newer layout.
    NewerEntry { version: u32 },
    /// Paint made on a mesh the file does not hold under that leaf.
    MeshChanged,
    /// Paint that does not cover its mesh's faces.
    BadRuns,
}

impl fmt::Display for SurfaceNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("some surface paint is malformed"),
            Self::UnknownKind { code } => write!(f, "the file holds paint of unknown kind {code}"),
            Self::NewerEntry { version } => write!(
                f,
                "some paint was written by a newer Breadslicer (version {version})"
            ),
            Self::MeshChanged => {
                f.write_str("some paint was made on a mesh that is no longer the model's")
            }
            Self::BadRuns => f.write_str("some paint does not cover its model's faces"),
        }
    }
}

/// `scene` and what rides with it, as the crate writes it. `settings` is the
/// project settings document.
pub(crate) fn to_file(
    scene: &Scene,
    settings: Option<Vec<u8>>,
    supports: &[SupportDocument],
    hollows: &[HollowDocument],
) -> Result<bf::BreadFile, WriteBreadProjectError> {
    let emptied: HashSet<NodeId> = supports.iter().map(|s| s.leaf).collect();
    let mut encoder = Encoder {
        scene,
        emptied: &emptied,
        out: bf::Scene {
            meta: scene.meta().as_ref().map(meta_out),
            ..bf::Scene::default()
        },
        nodes: HashMap::new(),
        subtrees: HashMap::new(),
        open_subtrees: HashSet::new(),
        mesh_index: HashMap::new(),
        resources: HashMap::new(),
    };
    for &root in scene.roots() {
        let index = encoder.node(root)?;
        encoder.out.roots.push(index);
    }

    let node = |id: NodeId, missing: WriteBreadProjectError| {
        encoder.nodes.get(&id).copied().ok_or(missing)
    };
    let supports = supports
        .iter()
        .map(|s| {
            Ok(bf::SupportEntry {
                model: s
                    .model
                    .and_then(|model| encoder.nodes.get(&model).copied())
                    .ok_or(WriteBreadProjectError::SupportModelMissing)?,
                support: node(s.leaf, WriteBreadProjectError::SupportLeafMissing)?,
                generator: s.generator.clone(),
                version: s.version,
                document: s.document.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let hollow_models = hollows
        .iter()
        .map(|h| node(h.leaf, WriteBreadProjectError::HollowLeafMissing))
        .collect::<Result<Vec<_>, _>>()?;
    let surface = encoder.surface();
    let hollows = hollows
        .iter()
        .zip(hollow_models)
        .map(|(h, model)| bf::HollowEntry {
            model,
            original: encoder.mesh(&h.original),
            generator: h.hollower.clone(),
            version: h.version,
            document: h.document.clone(),
        })
        .collect();
    Ok(bf::BreadFile {
        scene: encoder.out,
        settings,
        supports,
        hollows,
        thumbnail: None,
        support_previews: Vec::new(),
        surface,
    })
}

struct Encoder<'s> {
    scene: &'s Scene,
    emptied: &'s HashSet<NodeId>,
    out: bf::Scene,
    nodes: HashMap<NodeId, usize>,
    subtrees: HashMap<SubtreeResourceId, usize>,
    open_subtrees: HashSet<SubtreeResourceId>,
    /// The index each stored mesh got, by address: every mesh named here is
    /// held by the scene or a document for the whole encoding.
    mesh_index: HashMap<*const IndexedMesh, usize>,
    resources: HashMap<MeshResourceId, usize>,
}

impl Encoder<'_> {
    /// The index of `arc` among the stored meshes, storing it the first time.
    fn mesh(&mut self, arc: &Arc<IndexedMesh>) -> usize {
        *self.mesh_index.entry(Arc::as_ptr(arc)).or_insert_with(|| {
            self.out.meshes.push(mesh_out(arc));
            self.out.meshes.len() - 1
        })
    }

    /// Lists the mesh resource `id` the first time a stored leaf shows it.
    fn resource(&mut self, id: MeshResourceId) -> Result<usize, WriteBreadProjectError> {
        if let Some(&index) = self.resources.get(&id) {
            return Ok(index);
        }
        let arc = self
            .scene
            .mesh_resource_arc(id)
            .ok_or(WriteBreadProjectError::SceneInconsistent)?;
        let mesh = self.mesh(arc);
        let index = self.out.mesh_resources.len();
        self.out.mesh_resources.push(mesh);
        self.resources.insert(id, index);
        Ok(index)
    }

    /// Lists `id` after everything it names; returns its node index.
    fn node(&mut self, id: NodeId) -> Result<usize, WriteBreadProjectError> {
        if let Some(&index) = self.nodes.get(&id) {
            return Ok(index);
        }
        let node = self
            .scene
            .node(id)
            .ok_or(WriteBreadProjectError::SceneInconsistent)?;
        let kind = match &node.kind {
            NodeKind::Leaf(payload) if self.emptied.contains(&id) => {
                let mesh = match payload {
                    LeafPayload::Mesh(arc) => Some(arc),
                    LeafPayload::MeshResource(resource) => self.scene.mesh_resource_arc(*resource),
                }
                .ok_or(WriteBreadProjectError::SceneInconsistent)?;
                self.out.meshes.push(bf::Mesh {
                    meta: mesh.meta().as_ref().map(mesh_meta_out),
                    vertices: Vec::new(),
                    triangles: Vec::new(),
                });
                bf::NodeKind::Leaf {
                    mesh: self.out.meshes.len() - 1,
                }
            }
            NodeKind::Group { children } => bf::NodeKind::Group {
                children: children
                    .iter()
                    .map(|&child| self.node(child))
                    .collect::<Result<_, _>>()?,
            },
            // The format has no CSG nodes (bread-format#4), and nothing in
            // Breadslicer makes one yet.
            NodeKind::Csg { .. } => return Err(WriteBreadProjectError::Csg),
            NodeKind::Leaf(LeafPayload::Mesh(arc)) => bf::NodeKind::Leaf {
                mesh: self.mesh(arc),
            },
            NodeKind::Leaf(LeafPayload::MeshResource(resource)) => bf::NodeKind::SharedLeaf {
                resource: self.resource(*resource)?,
            },
            NodeKind::Reference { resource } => bf::NodeKind::Reference {
                subtree: self.subtree(*resource)?,
            },
        };
        self.out.nodes.push(bf::Node {
            meta: node.meta.as_ref().map(meta_out),
            transform: transform_out(&node.local_transform),
            kind,
        });
        let index = self.out.nodes.len() - 1;
        self.nodes.insert(id, index);
        Ok(index)
    }

    /// Lists the subtree resource `id` and its nodes; returns its index.
    fn subtree(&mut self, id: SubtreeResourceId) -> Result<usize, WriteBreadProjectError> {
        if let Some(&index) = self.subtrees.get(&id) {
            return Ok(index);
        }
        if !self.open_subtrees.insert(id) {
            return Err(WriteBreadProjectError::SceneInconsistent);
        }
        let root = self
            .scene
            .subtree_resource_root(id)
            .ok_or(WriteBreadProjectError::SceneInconsistent)?;
        let root = self.node(root)?;
        self.out.subtrees.push(root);
        self.open_subtrees.remove(&id);
        let index = self.out.subtrees.len() - 1;
        self.subtrees.insert(id, index);
        Ok(index)
    }

    /// The paint of the stored leaves that keep their mesh, in leaf and kind
    /// order.
    fn surface(&self) -> Vec<bf::SurfaceEntry> {
        let mut known = HashMap::new();
        let mut entries: Vec<(usize, PaintKind, &SurfaceTags)> = self
            .scene
            .surface_tags_iter()
            .filter(|(leaf, _, _)| !self.emptied.contains(leaf))
            .filter_map(|(leaf, kind, tags)| Some((*self.nodes.get(&leaf)?, kind, tags)))
            .collect();
        entries.sort_by_key(|(index, kind, _)| (*index, *kind));
        entries
            .into_iter()
            .map(|(leaf, kind, tags)| {
                let mut document = Vec::new();
                let runs = tags.runs();
                push_u32(&mut document, tags.values().len() as u32);
                document.extend_from_slice(fingerprint_of(tags.mesh(), &mut known).as_ref());
                push_u32(&mut document, runs.len() as u32);
                for (len, value) in runs {
                    push_u32(&mut document, len);
                    document.push(value);
                }
                bf::SurfaceEntry {
                    leaf,
                    kind: kind.code(),
                    version: SURFACE_DOCUMENT_VERSION,
                    document,
                }
            })
            .collect()
    }
}

/// What a `.bread` file opened into.
pub(crate) struct Opened {
    pub(crate) scene: Scene,
    pub(crate) settings: Option<Vec<u8>>,
    pub(crate) supports: Vec<SupportDocument>,
    pub(crate) hollows: Vec<HollowDocument>,
    pub(crate) surface_notes: Vec<SurfaceNote>,
}

/// Builds Breadslicer's scene from `file`, which the crate already checked.
/// `drafts` are the supports and hollows of a file saved before #211, which
/// the crate skips ([`draft_documents`]).
pub(crate) fn from_file(
    file: bf::BreadFile,
    drafts: Drafts,
) -> Result<Opened, ReadBreadProjectError> {
    let bf::BreadFile {
        scene: stored,
        settings,
        supports,
        hollows,
        surface,
        ..
    } = file;
    let meshes: Vec<Arc<IndexedMesh>> = stored
        .meshes
        .into_par_iter()
        .map(|mesh| Arc::new(mesh_in(mesh)))
        .collect();
    let mut scene = Scene::new();
    scene.set_meta(stored.meta.map(meta_in));
    let resources: Vec<MeshResourceId> = stored
        .mesh_resources
        .iter()
        .map(|&mesh| scene.add_mesh_resource_arc(meshes[mesh].clone()))
        .collect();

    let mut nodes: Vec<NodeId> = Vec::with_capacity(stored.nodes.len());
    let mut subtrees: Vec<SubtreeResourceId> = Vec::with_capacity(stored.subtrees.len());
    for (index, node) in stored.nodes.into_iter().enumerate() {
        let meta = node.meta.map(meta_in);
        let transform = transform_in(node.transform);
        let id = match node.kind {
            bf::NodeKind::Group { children } => {
                let group = scene.add_group(meta, transform);
                for child in children {
                    scene.add_child(group, nodes[child])?;
                }
                group
            }
            bf::NodeKind::Leaf { mesh } => {
                scene.add_leaf(meta, transform, LeafPayload::Mesh(meshes[mesh].clone()))?
            }
            bf::NodeKind::SharedLeaf { resource } => scene.add_leaf(
                meta,
                transform,
                LeafPayload::MeshResource(resources[resource]),
            )?,
            bf::NodeKind::Reference { subtree } => {
                scene.add_reference(meta, transform, subtrees[subtree])?
            }
        };
        nodes.push(id);
        // A subtree's root has no other place, so it is registered as soon
        // as it is listed, before any reference to it.
        while let Some(&root) = stored.subtrees.get(subtrees.len()) {
            if root > index {
                break;
            }
            subtrees.push(scene.register_subtree_resource(nodes[root])?);
        }
    }
    for root in stored.roots {
        scene.add_root(nodes[root])?;
    }

    let surface_notes = paint(&mut scene, &nodes, surface);
    let mut supports: Vec<SupportDocument> = supports
        .into_iter()
        .map(|entry| SupportDocument {
            model: Some(nodes[entry.model]),
            leaf: nodes[entry.support],
            generator: entry.generator,
            version: entry.version,
            document: entry.document,
        })
        .collect();
    let mut hollows: Vec<HollowDocument> = hollows
        .into_iter()
        .map(|entry| HollowDocument {
            leaf: nodes[entry.model],
            original: meshes[entry.original].clone(),
            hollower: entry.generator,
            version: entry.version,
            document: entry.document,
        })
        .collect();
    let leaf = |index: usize| {
        nodes
            .get(index)
            .copied()
            .filter(|&id| scene.leaf_mesh_arc(id).is_some())
    };
    supports.extend(drafts.supports.into_iter().filter_map(|draft| {
        Some(SupportDocument {
            model: None,
            leaf: leaf(draft.node)?,
            generator: draft.name,
            version: draft.version,
            document: draft.document,
        })
    }));
    hollows.extend(drafts.hollows.into_iter().filter_map(|draft| {
        let leaf = leaf(draft.node)?;
        let (document, original) = split_draft_original(draft.document)?;
        Some(HollowDocument {
            leaf,
            original: Arc::new(original),
            hollower: draft.name,
            version: draft.version,
            document,
        })
    }));
    Ok(Opened {
        scene,
        settings,
        supports,
        hollows,
        surface_notes,
    })
}

/// One entry of a supports or hollows chunk of version 1, which files saved
/// before #211 wrote: the node it belongs to and its document.
pub(crate) struct Draft {
    node: usize,
    name: String,
    version: u32,
    document: Vec<u8>,
}

/// The version 1 supports and hollows chunks of a file, which the crate
/// skips as drafts of the format.
#[derive(Default)]
pub(crate) struct Drafts {
    supports: Vec<Draft>,
    hollows: Vec<Draft>,
}

/// Reads the supports and hollows chunks of version 1 in `bytes`, a whole
/// `.bread` file, so that files saved before #211 open with their supports
/// editable and their hollows known. A chunk that does not read is left
/// out, as the crate leaves it.
pub(crate) fn draft_documents(bytes: &[u8]) -> Drafts {
    let mut drafts = Drafts::default();
    let Ok(mut reader) = bf::BreadReader::new(bytes) else {
        return drafts;
    };
    while let Ok(Some(chunk)) = reader.next_chunk() {
        let into = match (chunk.tag, chunk.version) {
            (bf::SUPPORTS_TAG, 1) => &mut drafts.supports,
            (bf::HOLLOWS_TAG, 1) => &mut drafts.hollows,
            _ => continue,
        };
        into.extend(read_drafts(&chunk.payload).unwrap_or_default());
    }
    drafts
}

/// A version 1 supports or hollows payload: u32 count, then per entry a u32
/// node index, a u32-long UTF-8 name, a u32 version and a u32-long document.
fn read_drafts(payload: &[u8]) -> Option<Vec<Draft>> {
    let mut input = payload;
    let count = take_u32(&mut input)? as usize;
    if count > input.len() / 16 {
        return None;
    }
    let mut drafts = Vec::with_capacity(count);
    for _ in 0..count {
        let node = take_u32(&mut input)? as usize;
        let len = take_u32(&mut input)? as usize;
        let name = String::from_utf8(take(&mut input, len)?.to_vec()).ok()?;
        let version = take_u32(&mut input)?;
        let len = take_u32(&mut input)? as usize;
        let document = take(&mut input, len)?.to_vec();
        drafts.push(Draft {
            node,
            name,
            version,
            document,
        });
    }
    input.is_empty().then_some(drafts)
}

/// A hollow document saved before #211 ends with the original mesh, as a
/// `u32` length and a mesh chunk payload of version 1. Splits it off at the
/// first place where a length naming exactly the rest of the document is
/// followed by a mesh that decodes.
fn split_draft_original(mut document: Vec<u8>) -> Option<(Vec<u8>, IndexedMesh)> {
    let (at, mesh) = (0..document.len().checked_sub(4)?).find_map(|at| {
        let len = u32::from_le_bytes(document[at..at + 4].try_into().ok()?) as usize;
        if len != document.len() - at - 4 {
            return None;
        }
        Some((at, bf::decode_mesh(&document[at + 4..]).ok()?))
    })?;
    document.truncate(at);
    Some((document, mesh_in(mesh)))
}

/// Puts the paint of `entries` on the leaves of `scene`. Never fails: what
/// cannot be read is left out and noted, and the rest of the file stands.
fn paint(scene: &mut Scene, nodes: &[NodeId], entries: Vec<bf::SurfaceEntry>) -> Vec<SurfaceNote> {
    let mut notes = Vec::new();
    let mut known = HashMap::new();
    for entry in entries {
        let Some(kind) = PaintKind::from_code(entry.kind) else {
            notes.push(SurfaceNote::UnknownKind { code: entry.kind });
            continue;
        };
        if entry.version > SURFACE_DOCUMENT_VERSION {
            notes.push(SurfaceNote::NewerEntry {
                version: entry.version,
            });
            continue;
        }
        let Some((faces, fingerprint, runs)) = read_paint(&entry.document) else {
            notes.push(SurfaceNote::Malformed);
            continue;
        };
        let leaf = nodes[entry.leaf];
        let Some(mesh) = scene.leaf_mesh_arc(leaf).cloned() else {
            notes.push(SurfaceNote::MeshChanged);
            continue;
        };
        if faces != mesh.faces().len() || fingerprint_of(&mesh, &mut known) != fingerprint {
            notes.push(SurfaceNote::MeshChanged);
            continue;
        }
        let Some(tags) = SurfaceTags::from_runs(mesh, runs) else {
            notes.push(SurfaceNote::BadRuns);
            continue;
        };
        if scene.set_surface_tags(leaf, kind, Some(tags)).is_err() {
            notes.push(SurfaceNote::MeshChanged);
        }
    }
    notes
}

/// What a version 1 paint document holds: the face count, the fingerprint
/// and the runs.
type Paint = (usize, GeometryFingerprint, Vec<(u32, u8)>);

/// Reads a version 1 paint document.
fn read_paint(document: &[u8]) -> Option<Paint> {
    let mut input = document;
    let faces = take_u32(&mut input)? as usize;
    let digest: [u8; 48] = take(&mut input, 48)?.try_into().ok()?;
    let count = take_u32(&mut input)? as usize;
    if count > input.len() / 5 {
        return None;
    }
    let mut runs = Vec::with_capacity(count);
    for _ in 0..count {
        let len = take_u32(&mut input)?;
        let value = *take(&mut input, 1)?.first()?;
        runs.push((len, value));
    }
    input
        .is_empty()
        .then(|| (faces, GeometryFingerprint::from_bytes(digest), runs))
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if input.len() < n {
        return None;
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Some(head)
}

fn take_u32(input: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(input, 4)?.try_into().ok()?))
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// The fingerprint of `mesh`, worked out once for all the leaves that show it.
fn fingerprint_of(
    mesh: &Arc<IndexedMesh>,
    known: &mut HashMap<*const IndexedMesh, GeometryFingerprint>,
) -> GeometryFingerprint {
    *known
        .entry(Arc::as_ptr(mesh))
        .or_insert_with(|| mesh.geometry_fingerprint())
}

fn meta_out(meta: &SceneMetadata) -> bf::Meta {
    bf::Meta {
        name: meta.name.clone(),
        comment: meta.comment.clone(),
    }
}

fn meta_in(meta: bf::Meta) -> SceneMetadata {
    SceneMetadata {
        name: meta.name,
        comment: meta.comment,
    }
}

fn mesh_meta_out(meta: &MeshMetadata) -> bf::Meta {
    bf::Meta {
        name: meta.name.clone(),
        comment: meta.header_comment.clone(),
    }
}

/// `mesh` as a mesh chunk keeps it. Facet normals a source STL declared are
/// not kept: they are file data, not geometry.
fn mesh_out(mesh: &IndexedMesh) -> bf::Mesh {
    bf::Mesh {
        meta: mesh.meta().as_ref().map(mesh_meta_out),
        vertices: mesh.vertices().iter().map(|v| *v.as_array()).collect(),
        triangles: mesh
            .faces()
            .iter()
            .map(|face| face.vertices.map(|index| index.get()))
            .collect(),
    }
}

fn mesh_in(mesh: bf::Mesh) -> IndexedMesh {
    IndexedMesh::new(
        mesh.meta.map(|meta| MeshMetadata {
            name: meta.name,
            header_comment: meta.comment,
            declared_facet_normals: None,
        }),
        mesh.vertices.into_iter().map(Vertex::new).collect(),
        mesh.triangles
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect(),
    )
}

fn transform_out(transform: &Transform3D) -> Option<bf::Transform> {
    match transform {
        Transform3D::Identity => None,
        Transform3D::Affine(affine) => Some(bf::Transform {
            linear: *affine.linear(),
            translation: *affine.translation(),
        }),
    }
}

fn transform_in(transform: Option<bf::Transform>) -> Transform3D {
    match transform {
        None => Transform3D::Identity,
        Some(t) => Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            t.linear,
            t.translation,
        )),
    }
}

/// The thumbnail of a `.bread` file, the PNG's bytes as stored, or `None`
/// when it has none. Writers put it first, so this reads one chunk and never
/// the meshes. Decoding the picture is the caller's.
pub fn read_bread_thumbnail<R: std::io::Read>(input: R) -> Option<Vec<u8>> {
    let mut reader = bf::BreadReader::new(input).ok()?;
    let chunk = reader.next_chunk().ok()??;
    (chunk.tag == bf::THUMBNAIL_TAG).then_some(chunk.payload)
}

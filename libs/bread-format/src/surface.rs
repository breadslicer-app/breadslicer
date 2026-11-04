/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The surface paint chunk (#183): what the person painted on the faces of
//! the models.
//!
//! Each entry is one mask of one model leaf. It names the mesh it was painted
//! on by its fingerprint: a mesh that is not that one keeps no paint, since the
//! faces no longer mean the same. Nothing else in the file depends on this
//! chunk, so a build that does not know it opens the file without the paint,
//! and one that cannot read an entry opens the file without that entry and says
//! so. Saving from a build that does not know the chunk loses the paint, as it
//! loses the hollows.
//!
//! ```text
//! entries   u32 count, then per entry:
//!           u32 the leaf's node index in the scene chunk
//!           u8  the paint kind: 0 detail, 1 support enforce, 2 support block
//!           u32 document version of the entry
//!           u32 length, entry document, version 1:
//!               u32 face count of the mesh
//!               48 bytes, the mesh's geometry fingerprint
//!               u32 run count, per run: u32 length, u8 value
//! ```
//!
//! Each run gives the value of its faces, in face order, and the runs cover
//! every face: 0 is bare, 255 painted, the values between a soft brush's.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::GeometryFingerprint;
use sc_scene::{NodeId, PaintKind, Scene, SurfaceTags};

use crate::bytes::{In, Out};
use crate::container::ChunkTag;

/// The tag of the surface paint chunk.
pub const SURFACE_TAG: ChunkTag = ChunkTag(*b"SURF");

/// The surface paint payload layout this build writes and reads.
pub const SURFACE_VERSION: u16 = 1;

/// The entry document this build writes and reads.
pub const SURFACE_DOCUMENT_VERSION: u32 = 1;

/// Paint a file holds that did not make it into the scene.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceNote {
    /// The chunk was written in a newer layout: all its paint is left out.
    NewerChunk { version: u16 },
    /// The chunk does not decode: all its paint is left out.
    Malformed,
    /// An entry of a kind this build does not know.
    UnknownKind { code: u8 },
    /// An entry written in a newer layout.
    NewerEntry { version: u32 },
    /// An entry names a node that is not a model.
    NotALeaf,
    /// An entry was painted on a mesh the file does not hold under that leaf.
    MeshChanged,
    /// An entry does not cover its mesh's faces.
    BadRuns,
}

impl fmt::Display for SurfaceNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewerChunk { version } => write!(
                f,
                "the surface paint was written by a newer Breadslicer (version {version})"
            ),
            Self::Malformed => f.write_str("the surface paint is malformed"),
            Self::UnknownKind { code } => write!(f, "the file holds paint of unknown kind {code}"),
            Self::NewerEntry { version } => write!(
                f,
                "some paint was written by a newer Breadslicer (version {version})"
            ),
            Self::NotALeaf => f.write_str("some paint belongs to a part that is not a model"),
            Self::MeshChanged => {
                f.write_str("some paint was made on a mesh that is no longer the model's")
            }
            Self::BadRuns => f.write_str("some paint does not cover its model's faces"),
        }
    }
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

/// Encodes the paint of `scene`, naming each leaf by its index from
/// `node_index` and leaving out the leaves it does not store; `None` when no
/// paint is left to store.
pub(crate) fn encode_surface(
    scene: &Scene,
    node_index: impl Fn(NodeId) -> Option<usize>,
) -> Option<Vec<u8>> {
    let mut known = HashMap::new();
    let mut entries: Vec<(usize, PaintKind, &SurfaceTags)> = scene
        .surface_tags_iter()
        .filter_map(|(leaf, kind, tags)| Some((node_index(leaf)?, kind, tags)))
        .collect();
    entries.sort_by_key(|(index, kind, _)| (*index, *kind));
    if entries.is_empty() {
        return None;
    }
    let mut out = Out::default();
    out.len(entries.len());
    for (index, kind, tags) in entries {
        out.len(index);
        out.u8(kind.code());
        out.u32(SURFACE_DOCUMENT_VERSION);
        let mut document = Out::default();
        document.len(tags.values().len());
        document
            .0
            .extend_from_slice(fingerprint_of(tags.mesh(), &mut known).as_ref());
        let runs = tags.runs();
        document.len(runs.len());
        for (len, value) in runs {
            document.u32(len);
            document.u8(value);
        }
        out.bytes(&document.0);
    }
    Some(out.0)
}

/// One entry as stored.
struct Entry<'a> {
    index: usize,
    code: u8,
    version: u32,
    document: &'a [u8],
}

fn read_entries(payload: &[u8]) -> Option<Vec<Entry<'_>>> {
    let mut input = In(payload);
    let count = input.count(13)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(Entry {
            index: input.len()?,
            code: input.u8()?,
            version: input.u32()?,
            document: input.bytes()?,
        });
    }
    input.is_empty().then_some(entries)
}

/// What a version 1 entry document holds.
struct Document {
    faces: usize,
    fingerprint: GeometryFingerprint,
    runs: Vec<(u32, u8)>,
}

fn read_document(document: &[u8]) -> Option<Document> {
    let mut input = In(document);
    let faces = input.len()?;
    let digest: [u8; 48] = input.take(48)?.try_into().ok()?;
    let count = input.count(5)?;
    let mut runs = Vec::with_capacity(count);
    for _ in 0..count {
        runs.push((input.u32()?, input.u8()?));
    }
    input.is_empty().then(|| Document {
        faces,
        fingerprint: GeometryFingerprint::from_bytes(digest),
        runs,
    })
}

/// Puts the paint of a surface payload on the leaves of `scene`, the decoded
/// `nodes` naming them. Never fails: what cannot be read is left out and
/// noted, and the rest of the file stands.
pub(crate) fn decode_surface(
    payload: &[u8],
    nodes: &[NodeId],
    scene: &mut Scene,
) -> Vec<SurfaceNote> {
    let Some(entries) = read_entries(payload) else {
        return vec![SurfaceNote::Malformed];
    };
    let mut notes = Vec::new();
    let mut known = HashMap::new();
    for entry in entries {
        let Some(kind) = PaintKind::from_code(entry.code) else {
            notes.push(SurfaceNote::UnknownKind { code: entry.code });
            continue;
        };
        if entry.version > SURFACE_DOCUMENT_VERSION {
            notes.push(SurfaceNote::NewerEntry {
                version: entry.version,
            });
            continue;
        }
        let Some(document) = read_document(entry.document) else {
            notes.push(SurfaceNote::Malformed);
            continue;
        };
        let Some((leaf, mesh)) = nodes
            .get(entry.index)
            .and_then(|&id| Some((id, scene.leaf_mesh_arc(id)?.clone())))
        else {
            notes.push(SurfaceNote::NotALeaf);
            continue;
        };
        if document.faces != mesh.faces().len()
            || fingerprint_of(&mesh, &mut known) != document.fingerprint
        {
            notes.push(SurfaceNote::MeshChanged);
            continue;
        }
        let Some(tags) = SurfaceTags::from_runs(mesh, document.runs) else {
            notes.push(SurfaceNote::BadRuns);
            continue;
        };
        if scene.set_surface_tags(leaf, kind, Some(tags)).is_err() {
            notes.push(SurfaceNote::NotALeaf);
        }
    }
    notes
}

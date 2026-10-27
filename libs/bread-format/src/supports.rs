/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The supports chunk: supports as data, not as a mesh.
//!
//! Each entry is one support leaf of the scene and its generator's own
//! document (tips, trunks, joints, rafts and the settings they were built
//! with), which the format carries without reading. The leaf is stored with
//! an empty mesh, and the application rebuilds the mesh from the document
//! after opening, so the supports stay editable.
//!
//! ```text
//! entries   u32 count, then per entry:
//!           u32 the leaf's node index in the scene chunk
//!           u32 length, UTF-8 generator name
//!           u32 generator document version
//!           u32 length, document bytes
//! ```

use std::fmt;

use sc_scene::NodeId;

use crate::bytes::{In, Out};
use crate::container::ChunkTag;

/// The tag of the supports chunk.
pub const SUPPORTS_TAG: ChunkTag = ChunkTag(*b"SUPP");

/// The supports payload layout this build writes and reads.
pub const SUPPORTS_VERSION: u16 = 1;

/// The supports of one leaf, as their generator describes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportDocument {
    /// The scene leaf that shows these supports.
    pub leaf: NodeId,
    /// Which generator wrote `document`, such as `"df"`.
    pub generator: String,
    /// The generator's own version of `document`.
    pub version: u32,
    pub document: Vec<u8>,
}

/// Why a supports payload could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeSupportsError {
    /// The payload is cut short, padded or not text where text belongs.
    Malformed,
    /// An entry names a node the scene does not list, or one that is not a
    /// leaf.
    NotALeaf,
}

impl fmt::Display for DecodeSupportsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the support data is malformed",
            Self::NotALeaf => "the support data names a scene part that is not a model",
        })
    }
}

impl std::error::Error for DecodeSupportsError {}

/// Encodes `supports`, naming each leaf by its index from `node_index`, or
/// `None` when a leaf is not one of the stored nodes.
pub(crate) fn encode_supports(
    supports: &[SupportDocument],
    node_index: impl Fn(NodeId) -> Option<usize>,
) -> Option<Vec<u8>> {
    let mut out = Out::default();
    out.len(supports.len());
    for support in supports {
        out.len(node_index(support.leaf)?);
        out.bytes(support.generator.as_bytes());
        out.u32(support.version);
        out.bytes(&support.document);
    }
    Some(out.0)
}

/// Decodes a supports payload over the nodes of the decoded scene, checking
/// that each named node is a leaf with `is_leaf`.
pub(crate) fn decode_supports(
    payload: &[u8],
    nodes: &[NodeId],
    is_leaf: impl Fn(NodeId) -> bool,
) -> Result<Vec<SupportDocument>, DecodeSupportsError> {
    let mut input = In(payload);
    let malformed = DecodeSupportsError::Malformed;
    let count = input.count(16).ok_or(malformed.clone())?;
    let mut supports = Vec::with_capacity(count);
    for _ in 0..count {
        let index = input.len().ok_or(malformed.clone())?;
        let generator = input
            .bytes()
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
            .ok_or(malformed.clone())?;
        let version = input.u32().ok_or(malformed.clone())?;
        let document = input.bytes().ok_or(malformed.clone())?.to_vec();
        let leaf = *nodes.get(index).ok_or(DecodeSupportsError::NotALeaf)?;
        if !is_leaf(leaf) {
            return Err(DecodeSupportsError::NotALeaf);
        }
        supports.push(SupportDocument {
            leaf,
            generator,
            version,
            document,
        });
    }
    if !input.is_empty() {
        return Err(malformed);
    }
    Ok(supports)
}

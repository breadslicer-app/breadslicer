/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The hollows chunk (#161): how the models hollowed in the application
//! were made.
//!
//! Each entry is one model leaf of the scene and its hollower's own document
//! (the mesh it was hollowed from, the settings and the drains), which the
//! format carries without reading. Unlike a support leaf, the leaf keeps its
//! mesh, the hollow: a build that does not know this chunk opens the model
//! hollowed, and one that does can hollow the original again, with new
//! drains.
//!
//! ```text
//! entries   u32 count, then per entry:
//!           u32 the leaf's node index in the scene chunk
//!           u32 length, UTF-8 hollower name
//!           u32 hollower document version
//!           u32 length, document bytes
//! ```

use std::fmt;

use sc_scene::NodeId;

use crate::bytes::{In, Out};
use crate::container::ChunkTag;

/// The tag of the hollows chunk.
pub const HOLLOWS_TAG: ChunkTag = ChunkTag(*b"HOLW");

/// The hollows payload layout this build writes and reads.
pub const HOLLOWS_VERSION: u16 = 1;

/// How one leaf's model was hollowed, as its hollower describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HollowDocument {
    /// The scene leaf that shows the hollow.
    pub leaf: NodeId,
    /// Which hollower wrote `document`, such as `"df"`.
    pub hollower: String,
    /// The hollower's own version of `document`.
    pub version: u32,
    pub document: Vec<u8>,
}

/// Why a hollows payload could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeHollowsError {
    /// The payload is cut short, padded or not text where text belongs.
    Malformed,
    /// An entry names a node the scene does not list, or one that is not a
    /// leaf.
    NotALeaf,
}

impl fmt::Display for DecodeHollowsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the hollow data is malformed",
            Self::NotALeaf => "the hollow data names a scene part that is not a model",
        })
    }
}

impl std::error::Error for DecodeHollowsError {}

/// Encodes `hollows`, naming each leaf by its index from `node_index`, or
/// `None` when a leaf is not one of the stored nodes.
pub(crate) fn encode_hollows(
    hollows: &[HollowDocument],
    node_index: impl Fn(NodeId) -> Option<usize>,
) -> Option<Vec<u8>> {
    let mut out = Out::default();
    out.len(hollows.len());
    for hollow in hollows {
        out.len(node_index(hollow.leaf)?);
        out.bytes(hollow.hollower.as_bytes());
        out.u32(hollow.version);
        out.bytes(&hollow.document);
    }
    Some(out.0)
}

/// Decodes a hollows payload over the nodes of the decoded scene, checking
/// that each named node is a leaf with `is_leaf`.
pub(crate) fn decode_hollows(
    payload: &[u8],
    nodes: &[NodeId],
    is_leaf: impl Fn(NodeId) -> bool,
) -> Result<Vec<HollowDocument>, DecodeHollowsError> {
    let mut input = In(payload);
    let malformed = DecodeHollowsError::Malformed;
    let count = input.count(16).ok_or(malformed.clone())?;
    let mut hollows = Vec::with_capacity(count);
    for _ in 0..count {
        let index = input.len().ok_or(malformed.clone())?;
        let hollower = input
            .bytes()
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
            .ok_or(malformed.clone())?;
        let version = input.u32().ok_or(malformed.clone())?;
        let document = input.bytes().ok_or(malformed.clone())?.to_vec();
        let leaf = *nodes.get(index).ok_or(DecodeHollowsError::NotALeaf)?;
        if !is_leaf(leaf) {
            return Err(DecodeHollowsError::NotALeaf);
        }
        hollows.push(HollowDocument {
            leaf,
            hollower,
            version,
            document,
        });
    }
    if !input.is_empty() {
        return Err(malformed);
    }
    Ok(hollows)
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Slice-extension resource types: slice stacks attached to objects
//! via `s:slicestackid`. Like the rest of the document model, plain
//! data in document units; cross-part reference resolution (sliceref
//! targets) and stack semantics live with the consumers.

use super::types::{ResourceId, ResourceIndex};

/// The `s:meshresolution` marker on an object: whether its mesh is
/// the authoritative geometry or a decimated stand-in for viewing,
/// with the slice data carrying the real shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeshResolution {
    #[default]
    Fullres,
    Lowres,
}

impl MeshResolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fullres => "fullres",
            Self::Lowres => "lowres",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fullres" => Some(Self::Fullres),
            "lowres" => Some(Self::Lowres),
            _ => None,
        }
    }
}

/// An `<s:slicestack>` resource: a stack bottom plus either inline
/// slices or references to stacks stored in other model parts.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceStack {
    pub id: ResourceId,
    /// Absolute Z of the stack bottom, document units. Defaults to 0.
    pub z_bottom: f64,
    pub content: SliceStackContent,
}

/// The spec forbids mixing inline slices and slicerefs in one stack;
/// the enum makes the mixed state unrepresentable.
#[derive(Clone, Debug, PartialEq)]
pub enum SliceStackContent {
    Slices(Vec<Slice>),
    Refs(Vec<SliceRef>),
}

/// One `<s:slice>`: the slab from the previous boundary up to
/// `z_top`. A slice with no vertices and no polygons is a void layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    /// Absolute Z of the TOP of this slab, document units.
    pub z_top: f64,
    pub vertices: Vec<SliceVertex>,
    pub polygons: Vec<SlicePolygon>,
}

/// A 2D vertex of a slice, document units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliceVertex {
    pub x: f64,
    pub y: f64,
}

/// An `<s:polygon>`: a start vertex followed by a segment chain. The
/// polygon is closed iff the last segment ends at `start`.
#[derive(Clone, Debug, PartialEq)]
pub struct SlicePolygon {
    pub start: ResourceIndex,
    pub segments: Vec<SliceSegment>,
}

impl SlicePolygon {
    /// Whether the segment chain returns to the start vertex.
    pub fn is_closed(&self) -> bool {
        self.segments.last().map(|s| s.end) == Some(self.start)
    }
}

/// One `<s:segment>` edge to the next vertex. The optional segment
/// properties (p1/p2/pid) are accepted and dropped by the parser and
/// never written: the spec's XSD and prose disagree on them and this
/// crate does not interpret property semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceSegment {
    pub end: ResourceIndex,
}

/// An `<s:sliceref>`: pulls in a stack defined in another model part.
/// `slice_stack` identifies the stack WITHIN the target part; it is
/// not a reference into this document's resources.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceRef {
    pub slice_stack: ResourceId,
    pub slice_path: String,
}

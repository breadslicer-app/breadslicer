/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Slices that are too large for 1 package part.
//!
//! A large plate slices to gigabytes of XML: more than a reader takes in 1 part
//! and more than memory should hold beside the scene. So such a stack is
//! written as a run of model parts of a bounded size, each a stack of
//! consecutive layers, and [`SLICE_PART_NAME`] holds the slice extension's
//! references to them, in layer order. A stack that fits 1 part stays inline in
//! [`SLICE_PART_NAME`], as it always was.
//!
//! [`SplitSlices`] makes the parts 1 at a time as the package is written, and
//! [`SliceParts`] takes them 1 at a time as it is read and keeps only the
//! contours.

use std::collections::BTreeMap;
use std::ops::Range;

use sc_3mf::error::{ModelWriteError, PackageReadError, PackageWriteError};
use sc_3mf::model::resource_ids::ResourceId;
use sc_3mf::model::{
    ModelDocument, SliceRef, SliceStack, SliceStackContent, parse_model, write_model,
};
use sc_3mf::package::{PartHandOff, PartName};
use sc_sl1cer::contour::{ContourLayer, ContourStack};

use crate::error::WriteThreeMfProjectError;
use crate::slices::PlateSlices;
use crate::threemf::names::{
    SLICE_PART_NAME, SLICE_PART_NUMBERED_PREFIX, SLICE_PART_NUMBERED_SUFFIX,
};
use crate::threemf::slice_mapping::{
    contour_stack, contours_from_slice_stack, layers_from_slice_stack, slice_stack_from_layers,
};
use crate::threemf::writer::checked_part_name;

/// About how much XML goes in 1 part: what a save holds of the slices at a time
/// (a few times this, with the compressed copy). Far under the reader's ceiling
/// for a part, and large enough that a part keeps every core busy.
pub(super) const SLICE_PART_TARGET_BYTES: u64 = 128 * 1024 * 1024;

/// The sole stack of a slice part.
const SLICE_STACK_ID: ResourceId = ResourceId::MIN;

/// The most slice XML a reader takes from 1 file, summed across its parts. A
/// real plate stays far under it; it stops a crafted file from filling memory
/// with contours through parts that are each within the ceiling.
const MAX_SLICE_XML_BYTES: u64 = 64 * (1 << 30);

/// The XML a contour point takes in a slice part, as a guess: its vertex
/// element with 2 shortest-round-trip numbers, and its segment element.
const ESTIMATED_POINT_BYTES: u64 = 80;

/// The XML around the points of a layer, as a guess.
const ESTIMATED_LAYER_BYTES: u64 = 64;

/// A guess at the XML size of `layer`, for choosing where parts end and for
/// progress. Mapping drops repeated points, so it tends to run high.
fn estimated_layer_bytes(layer: &ContourLayer) -> u64 {
    let points: usize = layer
        .polygons()
        .iter()
        .map(|polygon| {
            let holes: usize = polygon.holes().iter().map(|ring| ring.len()).sum();
            polygon.outer().len() + holes
        })
        .sum();
    ESTIMATED_LAYER_BYTES + points as u64 * ESTIMATED_POINT_BYTES
}

/// 1 part of a split stack: a run of consecutive layers.
#[derive(Debug)]
pub(super) struct SlicePart {
    name: PartName,
    layers: Range<usize>,
    estimated_bytes: u64,
}

impl SlicePart {
    pub(super) fn name(&self) -> &PartName {
        &self.name
    }

    pub(super) fn estimated_bytes(&self) -> u64 {
        self.estimated_bytes
    }
}

/// A plate's slices laid out across parts.
#[derive(Debug)]
pub(super) struct SplitSlices<'p> {
    plate: &'p PlateSlices,
    parts: Vec<SlicePart>,
}

impl<'p> SplitSlices<'p> {
    /// Lays out `plate` in parts of about `target_bytes`, or gives `None` when
    /// it fits 1 part. Every layer goes whole in 1 part, so a layer larger than
    /// the target gets a part to itself. A plate whose slab tops do not match
    /// its layers also gives `None`: the inline path reports that.
    pub(super) fn of(
        plate: &'p PlateSlices,
        target_bytes: u64,
    ) -> Result<Option<Self>, WriteThreeMfProjectError> {
        let layers = plate.contours.layers();
        if layers.len() != plate.slab_tops.len() {
            return Ok(None);
        }
        let mut parts = Vec::new();
        let (mut start, mut bytes) = (0, 0);
        for (index, layer) in layers.iter().enumerate() {
            bytes += estimated_layer_bytes(layer);
            if bytes >= target_bytes || index + 1 == layers.len() {
                let ordinal = parts.len() + 1;
                parts.push(SlicePart {
                    name: checked_part_name(format!(
                        "{SLICE_PART_NUMBERED_PREFIX}{ordinal}{SLICE_PART_NUMBERED_SUFFIX}"
                    ))?,
                    layers: start..index + 1,
                    estimated_bytes: bytes,
                });
                (start, bytes) = (index + 1, 0);
            }
        }
        Ok((parts.len() > 1).then_some(Self { plate, parts }))
    }

    pub(super) fn parts(&self) -> &[SlicePart] {
        &self.parts
    }

    /// The XML of [`SLICE_PART_NAME`]: a stack of references to the parts, in
    /// layer order.
    pub(super) fn index_xml(&self) -> Result<Vec<u8>, ModelWriteError> {
        let refs = self
            .parts
            .iter()
            .map(|part| SliceRef {
                slice_stack: SLICE_STACK_ID,
                slice_path: part.name.as_str().to_string(),
            })
            .collect();
        let mut xml = Vec::new();
        write_model(
            &slice_document(SliceStack {
                id: SLICE_STACK_ID,
                z_bottom: 0.0,
                content: SliceStackContent::Refs(refs),
            }),
            &mut xml,
        )?;
        Ok(xml)
    }

    /// The XML of the part `name`. Only this part's layers are mapped, and all
    /// of it goes when the part is written.
    pub(super) fn part_xml(&self, name: &PartName) -> Result<Vec<u8>, PackageWriteError> {
        let failed =
            |source: Box<dyn std::error::Error + Send + Sync>| PackageWriteError::PartSource {
                part: name.as_str().to_string(),
                source,
            };
        let part = self
            .parts
            .iter()
            .find(|part| part.name == *name)
            .ok_or_else(|| failed("it is not 1 of the slice parts".into()))?;
        let range = part.layers.clone();
        let tops = &self.plate.slab_tops;
        let z_bottom = range.start.checked_sub(1).map_or(0.0, |below| tops[below]);
        let stack = slice_stack_from_layers(
            &self.plate.contours.layers()[range.clone()],
            &tops[range],
            z_bottom,
            SLICE_STACK_ID,
        );
        let mut xml = Vec::new();
        xml.try_reserve(part.estimated_bytes as usize)
            .map_err(|error| failed(error.into()))?;
        write_model(&slice_document(stack), &mut xml).map_err(|error| failed(error.into()))?;
        Ok(xml)
    }
}

/// A model document holding only `stack`.
fn slice_document(stack: SliceStack) -> ModelDocument {
    ModelDocument {
        slice_stacks: vec![stack],
        ..ModelDocument::default()
    }
}

/// Whether `part` is [`SLICE_PART_NAME`] or a numbered slice part, ignoring
/// ASCII case, as OPC names do.
fn is_slice_part(part: &str) -> bool {
    let part = part.to_ascii_lowercase();
    if part == SLICE_PART_NAME.as_str().to_ascii_lowercase() {
        return true;
    }
    part.strip_prefix(&SLICE_PART_NUMBERED_PREFIX.to_ascii_lowercase())
        .and_then(|rest| rest.strip_suffix(SLICE_PART_NUMBERED_SUFFIX))
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

/// What [`SLICE_PART_NAME`] holds.
#[derive(Debug)]
enum SliceRoot {
    /// The whole stack.
    Inline(ContourStack),
    /// The names of the parts that hold it, in layer order.
    Refs(Vec<String>),
}

/// Takes the slice parts of a package as it is read, keeping the contours and
/// never more than 1 part of XML.
///
/// A part that is missing, too large or unreadable means the file has no
/// usable slices: the scene opens without them, which is what happens when
/// slices fail their fingerprint check, and slices can always be cut again.
#[derive(Debug, Default)]
pub(super) struct SliceParts {
    root: Option<SliceRoot>,
    /// The layers of each numbered part, by lowercase name.
    numbered: BTreeMap<String, Vec<ContourLayer>>,
    xml_bytes: u64,
}

impl SliceParts {
    fn take_root(&mut self, document: &ModelDocument) {
        let Some(stack) = document.slice_stacks.first() else {
            return;
        };
        self.root = match &stack.content {
            SliceStackContent::Slices(_) => {
                contours_from_slice_stack(stack).ok().map(SliceRoot::Inline)
            }
            SliceStackContent::Refs(refs) => Some(SliceRoot::Refs(
                refs.iter().map(|r| r.slice_path.clone()).collect(),
            )),
        };
    }

    fn take_numbered(&mut self, part: &str, document: &ModelDocument) {
        let layers = document
            .slice_stacks
            .first()
            .and_then(|stack| layers_from_slice_stack(stack).ok());
        if let Some(layers) = layers {
            self.numbered.insert(part.to_ascii_lowercase(), layers);
        }
    }

    /// The contour stack the parts make up, or `None` when the file has no
    /// usable slices.
    pub(super) fn into_stack(mut self) -> Option<ContourStack> {
        match self.root? {
            SliceRoot::Inline(stack) => Some(stack),
            SliceRoot::Refs(names) => {
                let mut layers = Vec::new();
                for name in names {
                    layers.append(&mut self.numbered.remove(&name.to_ascii_lowercase())?);
                }
                contour_stack(layers).ok()
            }
        }
    }
}

impl PartHandOff for SliceParts {
    fn wants(&self, part: &str) -> bool {
        is_slice_part(part)
    }

    fn take(&mut self, part: &str, data: Result<Vec<u8>, PackageReadError>) {
        let Ok(bytes) = data else {
            return;
        };
        self.xml_bytes += bytes.len() as u64;
        if self.xml_bytes > MAX_SLICE_XML_BYTES {
            self.root = None;
            self.numbered.clear();
            return;
        }
        let Ok(document) = parse_model(&bytes) else {
            return;
        };
        drop(bytes);
        if part.eq_ignore_ascii_case(SLICE_PART_NAME.as_str()) {
            self.take_root(&document);
        } else {
            self.take_numbered(part, &document);
        }
    }
}

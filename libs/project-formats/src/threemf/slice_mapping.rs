/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Forward mapping from the slicer's merged contour stack to the
//! `sc-3mf` slice-extension vocabulary.
//!
//! This module only emits: a merged `ContourStack` becomes a
//! `SliceStack`. It does not parse a `SliceStack` back into a
//! `ContourStack`; that reverse reader, when needed, lives elsewhere.
//! Within one layer every ring shares a single vertex list; identical
//! points are stored once and consecutive duplicates are dropped so no
//! zero-length segment is emitted. Each ring, outer or hole alike,
//! becomes one closed `SlicePolygon`: the outer/hole grouping is a
//! modelling concern with no meaning in the flat slice output.

use std::collections::HashMap;

use sc_3mf::model::{
    ResourceId, ResourceIndex, Slice, SlicePolygon, SliceSegment, SliceStack, SliceStackContent,
    SliceVertex,
};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack};
use sc_sl1cer::geom2d::Point2D;

/// Why building a [`SliceStack`] from a contour stack failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceMappingError {
    /// One slab top must be supplied per layer; the counts differ.
    LayerCountMismatch { layers: usize, tops: usize },
}

impl std::fmt::Display for SliceMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LayerCountMismatch { layers, tops } => write!(
                f,
                "contour stack has {layers} layers but {tops} slab tops were supplied"
            ),
        }
    }
}

impl std::error::Error for SliceMappingError {}

/// Map a merged, world-coordinate [`ContourStack`] to a [`SliceStack`]
/// with `stack_id` and a bottom at Z = 0. `slab_tops` gives the top Z
/// of each layer, index-aligned with the stack's layers; the centred
/// sample Z the slicer used is not part of the output.
pub(crate) fn slice_stack_from_contours(
    stack: &ContourStack,
    slab_tops: &[f64],
    stack_id: ResourceId,
) -> Result<SliceStack, SliceMappingError> {
    let layers = stack.layers();
    if layers.len() != slab_tops.len() {
        return Err(SliceMappingError::LayerCountMismatch {
            layers: layers.len(),
            tops: slab_tops.len(),
        });
    }

    let slices = layers
        .iter()
        .zip(slab_tops)
        .map(|(layer, &z_top)| slice_from_layer(layer, z_top))
        .collect();

    Ok(SliceStack {
        id: stack_id,
        z_bottom: 0.0,
        content: SliceStackContent::Slices(slices),
    })
}

/// Build one [`Slice`] from a contour layer. A layer with no emittable
/// polygon yields a void slice (no vertices, no polygons).
fn slice_from_layer(layer: &ContourLayer, z_top: f64) -> Slice {
    let mut vertices = LayerVertices::default();
    let mut polygons = Vec::new();
    for polygon in layer.polygons() {
        if let Some(emitted) = vertices.ring_polygon(polygon.outer()) {
            polygons.push(emitted);
        }
        for hole in polygon.holes() {
            if let Some(emitted) = vertices.ring_polygon(hole) {
                polygons.push(emitted);
            }
        }
    }
    Slice {
        z_top,
        vertices: vertices.into_vec(),
        polygons,
    }
}

/// The bit pattern of a point, used to intern exactly-equal vertices.
fn point_bits(p: Point2D) -> (u64, u64) {
    (p.x.to_bits(), p.y.to_bits())
}

/// The shared vertex list of a single layer plus its interning index.
#[derive(Default)]
struct LayerVertices {
    vertices: Vec<SliceVertex>,
    index_of: HashMap<(u64, u64), u32>,
}

impl LayerVertices {
    fn into_vec(self) -> Vec<SliceVertex> {
        self.vertices
    }

    /// Return this point's index in the shared list, inserting it if it
    /// is new.
    fn intern(&mut self, p: Point2D) -> ResourceIndex {
        let key = point_bits(p);
        let index = *self.index_of.entry(key).or_insert_with(|| {
            let index = self.vertices.len() as u32;
            self.vertices.push(SliceVertex { x: p.x, y: p.y });
            index
        });
        resource_index(index)
    }

    /// Build one closed [`SlicePolygon`] from a ring. Consecutive
    /// duplicate points (including a final point equal to the first)
    /// are dropped before interning; a ring that collapses to fewer
    /// than 3 distinct vertices cannot bound an area and yields no
    /// polygon.
    fn ring_polygon(&mut self, ring: &ClosedRing) -> Option<SlicePolygon> {
        let points = distinct_consecutive(ring.vertices());
        if points.len() < 3 {
            return None;
        }
        let indices: Vec<ResourceIndex> = points.iter().map(|&p| self.intern(p)).collect();
        let start = indices[0];
        let mut segments: Vec<SliceSegment> = indices[1..]
            .iter()
            .map(|&end| SliceSegment { end })
            .collect();
        // Close the chain back to the start vertex.
        segments.push(SliceSegment { end: start });
        Some(SlicePolygon { start, segments })
    }
}

/// The ring's points with consecutive duplicates removed, treating the
/// list as cyclic so a trailing point equal to the first is dropped.
fn distinct_consecutive(points: &[Point2D]) -> Vec<Point2D> {
    let mut out: Vec<Point2D> = Vec::with_capacity(points.len());
    for &p in points {
        if out.last().map(|&last| point_bits(last)) != Some(point_bits(p)) {
            out.push(p);
        }
    }
    if out.len() > 1 && out.first().map(|&p| point_bits(p)) == out.last().map(|&p| point_bits(p)) {
        out.pop();
    }
    out
}

/// A vertex index as a [`ResourceIndex`]. A slice layer holds far fewer
/// than the 2^31 vertices the 3MF index type allows, so the conversion
/// cannot fail in practice.
fn resource_index(value: u32) -> ResourceIndex {
    ResourceIndex::new(value).expect("a slice layer holds far fewer than 2^31 vertices")
}

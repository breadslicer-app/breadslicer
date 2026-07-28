/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Mapping between the slicer's merged contour stack and the `sc-3mf`
//! slice-extension vocabulary, both directions.
//!
//! Forward, a merged `ContourStack` becomes a `SliceStack`: within one
//! layer every ring shares a single vertex list; identical points are
//! stored once and consecutive duplicates are dropped so no zero-length
//! segment is emitted. Each ring, outer or hole alike, becomes one
//! closed `SlicePolygon`: the outer/hole grouping is a modelling concern
//! with no meaning in the flat slice output.
//!
//! Reverse, a `SliceStack` becomes a `ContourStack` for the viewer: each
//! `SlicePolygon` returns as a lone `Polygon` with its winding preserved.
//! The grouping is not rebuilt, because the nonzero-winding rasteriser
//! fills the union of all rings identically either way.

use std::collections::HashMap;

use sc_3mf::model::{
    ResourceId, ResourceIndex, Slice, SlicePolygon, SliceSegment, SliceStack, SliceStackContent,
    SliceVertex,
};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;

/// Why mapping between a contour stack and a [`SliceStack`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceMappingError {
    /// One slab top must be supplied per layer; the counts differ.
    LayerCountMismatch { layers: usize, tops: usize },
    /// A slice polygon referenced a vertex index outside the slice's
    /// vertex list.
    VertexIndexOutOfRange { index: u32 },
    /// The reconstructed layers are not in strictly increasing Z, so they
    /// cannot form a contour stack.
    NonMonotoneLayers,
    /// The stack stores slicerefs to other model parts, which the reverse
    /// mapping does not follow; our own projects always inline slices.
    SliceRefsUnsupported,
}

impl std::fmt::Display for SliceMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LayerCountMismatch { layers, tops } => write!(
                f,
                "contour stack has {layers} layers but {tops} slab tops were supplied"
            ),
            Self::VertexIndexOutOfRange { index } => write!(
                f,
                "a slice polygon referenced vertex index {index}, which is out of range"
            ),
            Self::NonMonotoneLayers => {
                write!(
                    f,
                    "the reconstructed slice layers are not in strictly increasing Z"
                )
            }
            Self::SliceRefsUnsupported => write!(
                f,
                "the slice stack uses slicerefs to other parts, which are not supported"
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

/// Map a [`SliceStack`] of inline slices back to a [`ContourStack`] for
/// the viewer. Each `SlicePolygon` (one ring) returns as a lone
/// [`Polygon`] with its vertex order, hence its winding, preserved, so
/// the nonzero-winding rasteriser fills the union of rings the same way
/// the original grouped polygons did. Each layer takes the slice's
/// absolute `z_top` as its Z: the slicer's centred sample Z is not
/// persisted, and the viewer reads layer tops from the schedule anyway.
pub(crate) fn contours_from_slice_stack(
    stack: &SliceStack,
) -> Result<ContourStack, SliceMappingError> {
    let SliceStackContent::Slices(slices) = &stack.content else {
        return Err(SliceMappingError::SliceRefsUnsupported);
    };
    let layers = slices
        .iter()
        .map(layer_from_slice)
        .collect::<Result<Vec<_>, _>>()?;
    ContourStack::new(layers).map_err(|_| SliceMappingError::NonMonotoneLayers)
}

/// Rebuild one [`ContourLayer`] from a slice. Rings that collapse to
/// fewer than 3 distinct vertices bound no area and are dropped, matching
/// the forward emit; a void slice yields a layer with no polygons.
fn layer_from_slice(slice: &Slice) -> Result<ContourLayer, SliceMappingError> {
    let mut polygons = Vec::new();
    for poly in &slice.polygons {
        if let Some(ring) = ring_from_slice_polygon(&slice.vertices, poly)? {
            polygons.push(Polygon::new_unchecked(ring, vec![]));
        }
    }
    Ok(ContourLayer::new(slice.z_top, polygons))
}

/// Reconstruct a ring's points by walking the segment chain from the
/// start vertex. A trailing point equal to the first closes the loop and
/// is dropped, mirroring the forward mapper's cyclic dedup. Returns
/// `None` when fewer than 3 distinct vertices remain.
fn ring_from_slice_polygon(
    vertices: &[SliceVertex],
    poly: &SlicePolygon,
) -> Result<Option<ClosedRing>, SliceMappingError> {
    let mut points = Vec::with_capacity(poly.segments.len() + 1);
    points.push(point_at(vertices, poly.start)?);
    for segment in &poly.segments {
        points.push(point_at(vertices, segment.end)?);
    }
    if points.len() > 1 && point_bits(points[0]) == point_bits(points[points.len() - 1]) {
        points.pop();
    }
    if points.len() < 3 {
        return Ok(None);
    }
    Ok(Some(ClosedRing::new(points)))
}

/// Resolve a vertex index into its point, erroring if it is out of range.
fn point_at(vertices: &[SliceVertex], index: ResourceIndex) -> Result<Point2D, SliceMappingError> {
    let i = index.get() as usize;
    let vertex = vertices
        .get(i)
        .ok_or(SliceMappingError::VertexIndexOutOfRange { index: index.get() })?;
    Ok(Point2D::new(vertex.x, vertex.y))
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

#[cfg(test)]
mod reverse_mapping_tests {
    use super::*;
    use sc_3mf::model::SliceRef;
    use sc_sl1cer::contour::Winding;

    fn ring(points: &[(f64, f64)]) -> ClosedRing {
        ClosedRing::new(points.iter().map(|&(x, y)| Point2D::new(x, y)).collect())
    }

    fn coords(polygon: &Polygon) -> Vec<(f64, f64)> {
        polygon
            .outer()
            .vertices()
            .iter()
            .map(|v| (v.x, v.y))
            .collect()
    }

    #[test]
    fn round_trips_a_holeless_stack() {
        let sq0 = Polygon::new(
            ring(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]),
            vec![],
        )
        .expect("a CCW square");
        let sq1 = Polygon::new(
            ring(&[(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)]),
            vec![],
        )
        .expect("a CCW square");
        let stack = ContourStack::new(vec![
            ContourLayer::new(1.0, vec![sq0]),
            ContourLayer::new(2.0, vec![sq1]),
        ])
        .expect("z is strictly increasing");

        let mapped = slice_stack_from_contours(&stack, &[1.0, 2.0], ResourceId::MIN)
            .expect("the fixture maps forward");
        let back = contours_from_slice_stack(&mapped).expect("the stack maps back");

        assert_eq!(back.layers().len(), 2);
        // The layer Z is the persisted slab top (here equal to the sample
        // Z, so the round trip is an identity).
        assert_eq!(back.layers()[0].z_mm(), 1.0);
        assert_eq!(back.layers()[1].z_mm(), 2.0);
        assert_eq!(back.layers()[0].polygons().len(), 1);
        assert_eq!(
            coords(&back.layers()[0].polygons()[0]),
            vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]
        );
        assert_eq!(
            coords(&back.layers()[1].polygons()[0]),
            vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)]
        );
    }

    #[test]
    fn flattens_a_polygon_with_a_hole_into_two_lone_rings() {
        let outer = ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let hole = ring(&[(3.0, 3.0), (3.0, 6.0), (6.0, 6.0), (6.0, 3.0)]);
        let poly = Polygon::new(outer, vec![hole]).expect("outer CCW, hole CW");
        let stack = ContourStack::new(vec![ContourLayer::new(1.0, vec![poly])]).expect("one layer");

        let mapped = slice_stack_from_contours(&stack, &[1.0], ResourceId::MIN)
            .expect("the fixture maps forward");
        let back = contours_from_slice_stack(&mapped).expect("the stack maps back");

        let layer = &back.layers()[0];
        assert_eq!(
            layer.polygons().len(),
            2,
            "the outer and the hole return as two lone polygons"
        );
        // The forward mapper emits the outer before the holes, and the
        // reverse preserves winding: outer stays CCW, hole stays CW.
        assert_eq!(layer.polygons()[0].outer().winding(), Some(Winding::Ccw));
        assert_eq!(layer.polygons()[1].outer().winding(), Some(Winding::Cw));
        assert!(layer.polygons()[0].holes().is_empty());
        assert!(layer.polygons()[1].holes().is_empty());
    }

    #[test]
    fn drops_a_void_layer_to_no_polygons() {
        let solid = Polygon::new(ring(&[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)]), vec![])
            .expect("a CCW triangle");
        let stack = ContourStack::new(vec![
            ContourLayer::new(1.0, vec![solid]),
            ContourLayer::new(2.0, vec![]),
        ])
        .expect("z is strictly increasing");

        let mapped = slice_stack_from_contours(&stack, &[1.0, 2.0], ResourceId::MIN)
            .expect("the fixture maps forward");
        let back = contours_from_slice_stack(&mapped).expect("the stack maps back");

        assert_eq!(back.layers()[0].polygons().len(), 1);
        assert!(back.layers()[1].polygons().is_empty());
    }

    #[test]
    fn errors_on_an_out_of_range_vertex_index() {
        let stack = SliceStack {
            id: ResourceId::MIN,
            z_bottom: 0.0,
            content: SliceStackContent::Slices(vec![Slice {
                z_top: 1.0,
                vertices: vec![
                    SliceVertex { x: 0.0, y: 0.0 },
                    SliceVertex { x: 1.0, y: 0.0 },
                    SliceVertex { x: 0.0, y: 1.0 },
                ],
                polygons: vec![SlicePolygon {
                    start: ResourceIndex::new(0).unwrap(),
                    segments: vec![
                        SliceSegment {
                            end: ResourceIndex::new(1).unwrap(),
                        },
                        SliceSegment {
                            end: ResourceIndex::new(9).unwrap(),
                        },
                        SliceSegment {
                            end: ResourceIndex::new(0).unwrap(),
                        },
                    ],
                }],
            }]),
        };
        assert!(matches!(
            contours_from_slice_stack(&stack),
            Err(SliceMappingError::VertexIndexOutOfRange { index: 9 })
        ));
    }

    #[test]
    fn rejects_slicerefs() {
        let stack = SliceStack {
            id: ResourceId::MIN,
            z_bottom: 0.0,
            content: SliceStackContent::Refs(vec![SliceRef {
                slice_stack: ResourceId::MIN,
                slice_path: "/3D/other.model".to_string(),
            }]),
        };
        assert!(matches!(
            contours_from_slice_stack(&stack),
            Err(SliceMappingError::SliceRefsUnsupported)
        ));
    }
}

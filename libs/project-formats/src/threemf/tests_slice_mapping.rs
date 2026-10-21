/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_3mf::model::resource_ids::ResourceId;
use sc_3mf::model::{ModelDocument, SliceStackContent, parse_model, write_model};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;

use crate::threemf::slice_mapping::{SliceMappingError, slice_stack_from_contours};

fn ring(points: &[(f64, f64)]) -> ClosedRing {
    ClosedRing::new(points.iter().map(|&(x, y)| Point2D::new(x, y)).collect())
}

fn stack_id() -> ResourceId {
    ResourceId::new(1).expect("1 is a valid nonzero resource id")
}

/// A CCW unit-ish square with a CW square hole punched in the middle.
fn square_with_hole() -> Polygon {
    let outer = ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    let hole = ring(&[(2.0, 2.0), (2.0, 8.0), (8.0, 8.0), (8.0, 2.0)]);
    Polygon::new(outer, vec![hole]).expect("outer is CCW and hole is CW")
}

fn slices(stack: &sc_3mf::model::SliceStack) -> &[sc_3mf::model::Slice] {
    match &stack.content {
        SliceStackContent::Slices(slices) => slices,
        SliceStackContent::Refs(_) => panic!("mapping never produces slicerefs"),
    }
}

#[test]
fn maps_layers_to_slices_and_round_trips_through_3mf() {
    let solid = ContourLayer::new(0.025, vec![square_with_hole()]);
    let void = ContourLayer::new(0.075, vec![]);
    let stack = ContourStack::new(vec![solid, void]).expect("z is strictly increasing");

    let mapped = slice_stack_from_contours(&stack, &[0.05, 0.10], stack_id())
        .expect("layer and slab-top counts match");

    assert_eq!(mapped.z_bottom, 0.0);
    let mapped_slices = slices(&mapped);
    assert_eq!(mapped_slices.len(), 2);

    // The solid layer carries the outer and hole rings as 2 closed polygons
    // over 1 shared, deduplicated vertex list.
    let solid_slice = &mapped_slices[0];
    assert_eq!(solid_slice.z_top, 0.05);
    assert_eq!(solid_slice.vertices.len(), 8);
    assert_eq!(solid_slice.polygons.len(), 2);
    assert!(solid_slice.polygons.iter().all(|p| p.is_closed()));

    // The empty layer becomes a void slice: only a top, no geometry.
    let void_slice = &mapped_slices[1];
    assert_eq!(void_slice.z_top, 0.10);
    assert!(void_slice.vertices.is_empty());
    assert!(void_slice.polygons.is_empty());

    // The emitted stack survives 3MF's own writer and reader unchanged.
    let document = ModelDocument {
        slice_stacks: vec![mapped.clone()],
        ..ModelDocument::default()
    };
    let mut xml = Vec::new();
    write_model(&document, &mut xml).expect("a slicestack-only model writes");
    let parsed = parse_model(&xml).expect("the written model parses back");
    assert_eq!(parsed.slice_stacks, vec![mapped]);
}

#[test]
fn shared_vertices_are_interned_once_per_layer() {
    // 2 CCW triangles meeting at the origin: 5 distinct vertices.
    let a = Polygon::new(ring(&[(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)]), vec![])
        .expect("triangle a is CCW");
    let b = Polygon::new(ring(&[(0.0, 0.0), (-10.0, 0.0), (0.0, -10.0)]), vec![])
        .expect("triangle b is CCW");
    let layer = ContourLayer::new(0.05, vec![a, b]);
    let stack = ContourStack::new(vec![layer]).expect("single layer");

    let mapped = slice_stack_from_contours(&stack, &[0.1], stack_id()).expect("counts match");
    let slice = &slices(&mapped)[0];
    assert_eq!(slice.vertices.len(), 5);
    assert_eq!(slice.polygons.len(), 2);
}

#[test]
fn consecutive_duplicate_points_emit_no_zero_length_segment() {
    // The first vertex is repeated; the duplicate must be dropped so the
    // closing segment is not zero length.
    let outer = ring(&[(0.0, 0.0), (0.0, 0.0), (10.0, 0.0), (0.0, 10.0)]);
    let polygon = Polygon::new(outer, vec![]).expect("triangle is CCW");
    let layer = ContourLayer::new(0.05, vec![polygon]);
    let stack = ContourStack::new(vec![layer]).expect("single layer");

    let mapped = slice_stack_from_contours(&stack, &[0.1], stack_id()).expect("counts match");
    let slice = &slices(&mapped)[0];
    assert_eq!(slice.vertices.len(), 3);
    let polygon = &slice.polygons[0];
    assert_eq!(polygon.segments.len(), 3);
    assert!(polygon.is_closed());
    // No segment returns to the vertex it started from.
    let mut current = polygon.start;
    for segment in &polygon.segments {
        assert_ne!(segment.end, current);
        current = segment.end;
    }
}

#[test]
fn a_ring_that_collapses_below_three_vertices_yields_no_polygon() {
    // A ring that degenerates to a single point cannot bound an area, so it
    // produces no polygon and, having interned no vertices, leaves a void
    // slice.
    let collapsed = ring(&[(5.0, 5.0), (5.0, 5.0), (5.0, 5.0)]);
    let polygon = Polygon::new_unchecked(collapsed, vec![]);
    let layer = ContourLayer::new(0.05, vec![polygon]);
    let stack = ContourStack::new(vec![layer]).expect("single layer");

    let mapped = slice_stack_from_contours(&stack, &[0.1], stack_id()).expect("counts match");
    let slice = &slices(&mapped)[0];
    assert!(slice.vertices.is_empty());
    assert!(slice.polygons.is_empty());
}

#[test]
fn mismatched_slab_top_count_is_an_error() {
    let layer = ContourLayer::new(0.05, vec![square_with_hole()]);
    let stack = ContourStack::new(vec![layer]).expect("single layer");

    let error = slice_stack_from_contours(&stack, &[0.1, 0.2], stack_id())
        .expect_err("two slab tops for one layer must fail");
    assert_eq!(
        error,
        SliceMappingError::LayerCountMismatch { layers: 1, tops: 2 }
    );
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;

use crate::container::{BreadReader, BreadWriter};
use crate::slices::{DecodeSlicesError, SLICES_TAG, SLICES_VERSION, decode_slices, encode_slices};

fn ring(points: &[(f64, f64)]) -> ClosedRing {
    ClosedRing::new(points.iter().map(|&(x, y)| Point2D::new(x, y)).collect())
}

fn square(x: f64, y: f64, side: f64) -> ClosedRing {
    ring(&[(x, y), (x + side, y), (x + side, y + side), (x, y + side)])
}

fn hole(x: f64, y: f64, side: f64) -> ClosedRing {
    let mut ring = square(x, y, side);
    ring.reverse();
    ring
}

/// A stack with awkward values: a polygon with two holes, an empty layer,
/// coordinates that are not short decimals, negative zero and huge values.
fn stack() -> (ContourStack, Vec<f64>) {
    let framed = Polygon::new(
        square(-10.0, -10.0, 20.0),
        vec![hole(-8.0, -8.0, 3.0), hole(2.0, 2.0, 4.0)],
    )
    .unwrap();
    // Not a valid winding on purpose: the codec keeps what it is given.
    let odd = Polygon::new_unchecked(
        ring(&[
            (0.1 + 0.2, 1.0 / 3.0),
            (std::f64::consts::PI, -0.0),
            (1e-300, 123_456_789.123_456_78),
        ]),
        vec![],
    );
    let layers = vec![
        ContourLayer::new(0.025, vec![framed.clone(), odd]),
        ContourLayer::new(0.075, vec![]),
        ContourLayer::new(0.125, vec![framed]),
    ];
    (ContourStack::new(layers).unwrap(), vec![0.05, 0.1, 0.15])
}

/// Each vertex's bits, per ring, per polygon, per layer.
type StackBits = Vec<Vec<Vec<Vec<(u64, u64)>>>>;

fn bits(stack: &ContourStack) -> StackBits {
    stack
        .layers()
        .iter()
        .map(|layer| {
            layer
                .polygons()
                .iter()
                .map(|polygon| {
                    std::iter::once(polygon.outer())
                        .chain(polygon.holes())
                        .map(|ring| {
                            ring.vertices()
                                .iter()
                                .map(|p| (p.x.to_bits(), p.y.to_bits()))
                                .collect()
                        })
                        .collect()
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_stack_comes_back_bit_for_bit_with_its_holes_grouped() {
    let (stack, tops) = stack();
    let (back, back_tops) = decode_slices(&encode_slices(&stack, &tops)).unwrap();
    assert_eq!(bits(&back), bits(&stack));
    let z = |s: &ContourStack| {
        s.layers()
            .iter()
            .map(|l| l.z_mm().to_bits())
            .collect::<Vec<_>>()
    };
    assert_eq!(z(&back), z(&stack));
    assert_eq!(back_tops, tops);
    assert_eq!(back.layers()[0].polygons()[0].holes().len(), 2);
}

#[test]
fn an_empty_stack_round_trips() {
    let stack = ContourStack::new(vec![]).unwrap();
    let (back, tops) = decode_slices(&encode_slices(&stack, &[])).unwrap();
    assert!(back.layers().is_empty());
    assert!(tops.is_empty());
}

#[test]
fn the_stack_survives_the_container() {
    let (stack, tops) = stack();
    let mut writer = BreadWriter::new(Vec::new()).unwrap();
    writer
        .chunk(SLICES_TAG, SLICES_VERSION, &encode_slices(&stack, &tops))
        .unwrap();
    let bytes = writer.finish().unwrap();
    let mut reader = BreadReader::new(bytes.as_slice()).unwrap();
    let chunk = reader.next_chunk().unwrap().unwrap();
    assert_eq!((chunk.tag, chunk.version), (SLICES_TAG, SLICES_VERSION));
    let (back, _) = decode_slices(&chunk.payload).unwrap();
    assert_eq!(bits(&back), bits(&stack));
}

#[test]
fn a_cut_or_padded_payload_is_malformed() {
    let (stack, tops) = stack();
    let payload = encode_slices(&stack, &tops);
    for cut in [0, 3, 20, payload.len() / 2, payload.len() - 1] {
        assert_eq!(
            decode_slices(&payload[..cut]).unwrap_err(),
            DecodeSlicesError::Malformed,
            "cut at {cut}"
        );
    }
    let mut padded = payload;
    padded.push(0);
    assert_eq!(
        decode_slices(&padded).unwrap_err(),
        DecodeSlicesError::Malformed
    );
}

#[test]
fn a_polygon_without_rings_is_malformed() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&0.05f64.to_le_bytes());
    payload.extend_from_slice(&0.05f64.to_le_bytes());
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&0u64.to_le_bytes());
    assert_eq!(
        decode_slices(&payload).unwrap_err(),
        DecodeSlicesError::Malformed
    );
}

#[test]
fn layers_out_of_order_are_refused() {
    let (stack, _) = stack();
    let mut layers = stack.layers().to_vec();
    layers.reverse();
    let reversed = ContourStack::new_unchecked(layers);
    let payload = encode_slices(&reversed, &[0.15, 0.1, 0.05]);
    assert!(matches!(
        decode_slices(&payload),
        Err(DecodeSlicesError::Stack(_))
    ));
}

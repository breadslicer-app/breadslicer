/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The sliced plate: a merged contour stack and each layer's slab top.
//!
//! Exact: every coordinate comes back bit for bit, so a reopened slice
//! rasterises to the same pixels as the one saved. Unlike the 3MF slice
//! part, outer rings and their holes stay grouped into polygons.
//!
//! The payload is a structure section and a coordinate section:
//!
//! ```text
//! layers u32
//! per layer: z f64, slab top f64, polygons u32
//!   per polygon: rings u32 (outer first, then holes)
//!     per ring: vertices u32
//! points u64
//! xs, then ys: each value's bits XORed with the previous value's, then
//!   split into 8 byte planes (all low bytes, ..., all high bytes)
//! ```
//!
//! Neighbouring vertices are close, so the XOR leaves mostly zero high bytes
//! and the planes group them: zstd shrinks the coordinates far better than
//! it would the raw doubles, with no loss.

use std::fmt;

use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, ContourStackError, Polygon};
use sc_sl1cer::geom2d::Point2D;

use crate::container::ChunkTag;

/// The tag of the sliced-plate chunk.
pub const SLICES_TAG: ChunkTag = ChunkTag(*b"SLCE");

/// The sliced-plate payload layout this build writes and reads.
pub const SLICES_VERSION: u16 = 1;

/// Why a sliced-plate payload could not be decoded.
#[derive(Clone, Debug, PartialEq)]
pub enum DecodeSlicesError {
    /// The payload ends early or has bytes left over.
    Malformed,
    /// The layers do not form a contour stack (Z out of order or not finite).
    Stack(ContourStackError),
}

impl fmt::Display for DecodeSlicesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("the saved slice is malformed"),
            Self::Stack(err) => write!(f, "the saved slice's layers are invalid: {err:?}"),
        }
    }
}

impl std::error::Error for DecodeSlicesError {}

/// Encodes `stack` and its per-layer `slab_tops` (index-aligned with its
/// layers) as a sliced-plate payload.
///
/// # Panics
///
/// If `slab_tops` does not have one entry per layer.
pub fn encode_slices(stack: &ContourStack, slab_tops: &[f64]) -> Vec<u8> {
    let layers = stack.layers();
    assert_eq!(layers.len(), slab_tops.len(), "one slab top per layer");
    let mut out = Vec::new();
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    put_u32(&mut out, layers.len());
    for (layer, &top) in layers.iter().zip(slab_tops) {
        out.extend_from_slice(&layer.z_mm().to_le_bytes());
        out.extend_from_slice(&top.to_le_bytes());
        put_u32(&mut out, layer.polygons().len());
        for polygon in layer.polygons() {
            put_u32(&mut out, 1 + polygon.holes().len());
            for ring in std::iter::once(polygon.outer()).chain(polygon.holes()) {
                put_u32(&mut out, ring.len());
                for point in ring.vertices() {
                    xs.push(point.x);
                    ys.push(point.y);
                }
            }
        }
    }
    out.extend_from_slice(&(xs.len() as u64).to_le_bytes());
    put_planes(&mut out, &xs);
    put_planes(&mut out, &ys);
    out
}

/// Decodes a sliced-plate payload into its contour stack and slab tops.
pub fn decode_slices(payload: &[u8]) -> Result<(ContourStack, Vec<f64>), DecodeSlicesError> {
    let mut cursor = Cursor {
        bytes: payload,
        at: 0,
    };
    // The structure, with ring sizes, before the points it indexes.
    struct LayerShape {
        z: f64,
        polygons: Vec<Vec<usize>>,
    }
    let layer_count = cursor.u32()?;
    let mut shapes = Vec::with_capacity(layer_count.min(1 << 16));
    let mut tops = Vec::with_capacity(layer_count.min(1 << 16));
    let mut ring_points = 0usize;
    for _ in 0..layer_count {
        let z = cursor.f64()?;
        tops.push(cursor.f64()?);
        let polygon_count = cursor.u32()?;
        let mut polygons = Vec::with_capacity(polygon_count.min(1 << 16));
        for _ in 0..polygon_count {
            let ring_count = cursor.u32()?;
            if ring_count == 0 {
                return Err(DecodeSlicesError::Malformed);
            }
            let mut rings = Vec::with_capacity(ring_count.min(1 << 16));
            for _ in 0..ring_count {
                let len = cursor.u32()?;
                ring_points = ring_points
                    .checked_add(len)
                    .ok_or(DecodeSlicesError::Malformed)?;
                rings.push(len);
            }
            polygons.push(rings);
        }
        shapes.push(LayerShape { z, polygons });
    }
    let points = usize::try_from(cursor.u64()?).map_err(|_| DecodeSlicesError::Malformed)?;
    if points != ring_points {
        return Err(DecodeSlicesError::Malformed);
    }
    let xs = cursor.planes(points)?;
    let ys = cursor.planes(points)?;
    if cursor.at != payload.len() {
        return Err(DecodeSlicesError::Malformed);
    }

    let mut next = 0;
    let mut ring = |len: usize| {
        let vertices = (next..next + len)
            .map(|i| Point2D::new(xs[i], ys[i]))
            .collect();
        next += len;
        ClosedRing::new(vertices)
    };
    let layers = shapes
        .into_iter()
        .map(|shape| {
            let polygons = shape
                .polygons
                .into_iter()
                .map(|rings| {
                    let outer = ring(rings[0]);
                    let holes = rings[1..].iter().map(|&len| ring(len)).collect();
                    // Saved from a stack the slicer built: its windings are
                    // already the slicer's.
                    Polygon::new_unchecked(outer, holes)
                })
                .collect();
            ContourLayer::new(shape.z, polygons)
        })
        .collect();
    let stack = ContourStack::new(layers).map_err(DecodeSlicesError::Stack)?;
    Ok((stack, tops))
}

fn put_u32(out: &mut Vec<u8>, value: usize) {
    let value = u32::try_from(value).expect("a slice count fits in u32");
    out.extend_from_slice(&value.to_le_bytes());
}

/// Writes `values` XOR-chained and split into 8 byte planes.
fn put_planes(out: &mut Vec<u8>, values: &[f64]) {
    let mut previous = 0u64;
    let chained: Vec<[u8; 8]> = values
        .iter()
        .map(|value| {
            let bits = value.to_bits();
            let xored = bits ^ previous;
            previous = bits;
            xored.to_le_bytes()
        })
        .collect();
    for plane in 0..8 {
        out.extend(chained.iter().map(|bytes| bytes[plane]));
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], DecodeSlicesError> {
        let end = self
            .at
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(DecodeSlicesError::Malformed)?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<usize, DecodeSlicesError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")) as usize)
    }

    fn u64(&mut self) -> Result<u64, DecodeSlicesError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    fn f64(&mut self) -> Result<f64, DecodeSlicesError> {
        self.u64().map(f64::from_bits)
    }

    /// Reads `count` values written by [`put_planes`].
    fn planes(&mut self, count: usize) -> Result<Vec<f64>, DecodeSlicesError> {
        let len = count.checked_mul(8).ok_or(DecodeSlicesError::Malformed)?;
        let bytes = self.take(len)?;
        let mut previous = 0u64;
        Ok((0..count)
            .map(|i| {
                let mut word = [0u8; 8];
                for (plane, byte) in word.iter_mut().enumerate() {
                    *byte = bytes[plane * count + i];
                }
                let bits = u64::from_le_bytes(word) ^ previous;
                previous = bits;
                f64::from_bits(bits)
            })
            .collect())
    }
}

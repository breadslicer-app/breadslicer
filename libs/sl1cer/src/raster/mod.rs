/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Raster tier of the slicer IR.
//!
//! Turns a vector [`ContourLayer`](crate::contour::ContourLayer)
//! into a per-layer grayscale raster of run-length-encoded pixel
//! runs in physical pixel space. Input layers are printer-frame
//! millimetres, XY centred on the plate origin; the projection to
//! the corner-indexed pixel grid is owned by this tier. The
//! submodules:
//!
//! - [`pixel_map`] holds the millimetre-to-pixel projection.
//! - [`rle`] holds the run-length-encoded raster types and the
//!   streaming accumulator that builds them.
//! - [`scanline`] turns a vector contour layer into a binary
//!   [`rle::LayerRaster`] via the classic active-edge scanline fill.

pub mod pixel_map;
pub mod rle;
pub mod scanline;

pub use scanline::{rasterize_layer, rasterize_layer_binary};

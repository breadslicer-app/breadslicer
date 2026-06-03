/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Raster tier of the slicer IR.
//!
//! Turns a vector [`ContourLayer`](crate::contour::ContourLayer)
//! into a per-layer grayscale raster of run-length-encoded pixel
//! runs in physical pixel space. The submodules:
//!
//! - [`pixel_map`] holds the millimetre-to-pixel projection.

pub mod pixel_map;

pub use pixel_map::PixelMap;

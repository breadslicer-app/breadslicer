/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The merged slice result a project persists.

use std::sync::Arc;

use sc_sl1cer::contour::ContourStack;

/// The whole build plate's merged per-layer contours together with the
/// top Z of each layer, ready to persist as a 3MF slice stack.
///
/// `slab_tops[i]` is the top Z of `contours.layers()[i]` (the bottom is
/// Z = 0); the two are index-aligned. The pairing is checked when the
/// slices are mapped, not here, so this stays plain transport data.
#[derive(Clone, Debug, PartialEq)]
pub struct PlateSlices {
    /// One merged, world-coordinate contour stack for the whole plate, shared
    /// with whoever holds it: a save does not copy the slices.
    pub contours: Arc<ContourStack>,
    /// Per-layer top Z, index-aligned with `contours.layers()`.
    pub slab_tops: Vec<f64>,
}

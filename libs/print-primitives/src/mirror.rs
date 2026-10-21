/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-axis mirror flags for raster output orientation.
//!
//! [`Mirror`] carries two independent booleans. The struct holds no
//! invariants and never fails to construct; downstream code uses it
//! to decide whether to flip a rasterised layer horizontally or
//! vertically before writing it.

/// Per-axis mirror flags for raster output orientation. No invariants
/// to validate; both flags are free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Mirror {
    pub x: bool,
    pub y: bool,
}

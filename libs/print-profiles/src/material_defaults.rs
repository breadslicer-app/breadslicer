/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Defaults block of a material profile.
//!
//! [`MaterialDefaults`] mirrors the `[defaults]` section of a
//! material-profile TOML file: resin-constant fallbacks used when
//! no per-printer override exists for a field. Every field is
//! optional; a missing field means "no resin-level default for
//! this dimension". The resolver reads these values as-is and
//! never derives them.

use sc_print_primitives::ScaleCompensation;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct MaterialDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_compensation_pct: Option<ScaleCompensation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_layer_count: Option<u32>,
}

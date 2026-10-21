/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Intent block of a material profile.
//!
//! [`MaterialIntent`] mirrors the `[intent]` section of a
//! material-profile TOML file. Its fields capture what the resin
//! is designed for: the target energy density and target layer
//! height. The resolver consumes these when no calibrated value is
//! available for a printer, deriving process values from the
//! intent plus the printer's hardware constants (light intensity,
//! etc.). Every field is optional: a missing field means "no
//! intent expressed for this dimension".

use sc_print_primitives::{PositiveFiniteMjPerCm2, PositiveFiniteMm};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct MaterialIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_energy_density_mj_per_cm2: Option<PositiveFiniteMjPerCm2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_layer_height_mm: Option<PositiveFiniteMm>,
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Physical block of a material profile.
//!
//! [`MaterialPhysical`] mirrors the `[physical]` section of a
//! material-profile TOML file: measured constants of the resin
//! itself, as its vendor publishes them. Nothing here changes how a
//! print runs; these values turn a sliced volume into the figures a
//! printer file reports. Every field is optional, and the whole
//! section may be absent.

use sc_print_primitives::DensityGPerCm3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct MaterialPhysical {
    /// Density of the resin in the vat, which vendors measure under
    /// ASTM D1475.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liquid_density_g_per_cm3: Option<DensityGPerCm3>,
    /// Density of the cured part, which vendors measure under ASTM
    /// D792. Higher than the liquid figure, because the resin
    /// shrinks as it cures while its mass stays put.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cured_density_g_per_cm3: Option<DensityGPerCm3>,
}

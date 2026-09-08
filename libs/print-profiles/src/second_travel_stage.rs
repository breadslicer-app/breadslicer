/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The second stage of the platform's travel over a layer.
//!
//! A printer that separates the peel from the rest of the move rises
//! a short way at one speed and the remainder at another, and comes
//! back down over the same total distance. Vendors publish the 3
//! numbers together, and a stage with a distance but no speed means
//! nothing, so they are one value here: a calibration block either
//! carries a second stage or moves in one.

use sc_print_primitives::{PositiveFiniteMm, PositiveFiniteMmPerMin};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct SecondTravelStage {
    /// How far the platform rises in this stage, on top of what the
    /// first stage covers.
    pub lift_distance_mm: PositiveFiniteMm,
    pub lift_speed_mm_min: PositiveFiniteMmPerMin,
    /// How fast the platform returns over this stage's distance. The
    /// return runs in reverse, so this part comes first on the way
    /// down.
    pub retract_speed_mm_min: PositiveFiniteMmPerMin,
}

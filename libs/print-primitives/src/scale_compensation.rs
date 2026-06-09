/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-axis scale-compensation percentages for material profiles.
//!
//! [`ScaleCompensationPercent`] is the validated scalar primitive: a
//! finite percentage in the inclusive range `[-10.0, +10.0]` used to
//! describe per-axis scale correction. The bounds are a sanity check
//! against typos; a legitimately needed correction outside the range
//! suggests the calibration data is in the wrong shape.
//! [`ScaleCompensation`] composes three per-axis values into the
//! `{ x, y, z }` block the material profile carries.
//! [`ScaleCompensationPercentOutOfBound`] reports rejected inputs and
//! carries the bound values it expected so the message can be
//! self-describing.

use std::fmt;

const SCALE_COMPENSATION_MIN_PCT: f64 = -10.0;
const SCALE_COMPENSATION_MAX_PCT: f64 = 10.0;

/// A finite percentage in the inclusive range
/// `[-10.0, +10.0]`. Constructed through
/// [`ScaleCompensationPercent::new`]; once you hold one, the bound
/// invariant holds by type.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct ScaleCompensationPercent(f64);

/// Returned by [`ScaleCompensationPercent::new`] (and its
/// `TryFrom<f64>` impl) when the input was outside the inclusive
/// `[-10.0, +10.0]` range or non-finite. Carries the bound values
/// so the Display message can describe what was expected without
/// the caller needing to know the constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleCompensationPercentOutOfBound {
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

impl fmt::Display for ScaleCompensationPercentOutOfBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "scale compensation must be finite and in [{}%, {}%], got {}%",
            self.min, self.max, self.value,
        )
    }
}

impl std::error::Error for ScaleCompensationPercentOutOfBound {}

impl ScaleCompensationPercent {
    /// Construct from a raw `f64`. Fails with
    /// [`ScaleCompensationPercentOutOfBound`] when the value is
    /// non-finite or outside the inclusive `[-10.0, +10.0]` range.
    pub fn new(value: f64) -> Result<Self, ScaleCompensationPercentOutOfBound> {
        if value.is_finite()
            && (SCALE_COMPENSATION_MIN_PCT..=SCALE_COMPENSATION_MAX_PCT).contains(&value)
        {
            Ok(Self(value))
        } else {
            Err(ScaleCompensationPercentOutOfBound {
                value,
                min: SCALE_COMPENSATION_MIN_PCT,
                max: SCALE_COMPENSATION_MAX_PCT,
            })
        }
    }

    /// The underlying percentage as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for ScaleCompensationPercent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for ScaleCompensationPercent {
    type Error = ScaleCompensationPercentOutOfBound;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ScaleCompensationPercent> for f64 {
    fn from(value: ScaleCompensationPercent) -> Self {
        value.0
    }
}

/// Per-axis scale-compensation block of a material profile. Each
/// axis carries an independent [`ScaleCompensationPercent`] value;
/// the axes do not constrain each other.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ScaleCompensation {
    pub x: ScaleCompensationPercent,
    pub y: ScaleCompensationPercent,
    pub z: ScaleCompensationPercent,
}

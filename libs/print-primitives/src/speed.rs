/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated speeds and speed ranges in millimetres per minute.
//!
//! [`PositiveFiniteMmPerMin`] is the scalar primitive: a strictly
//! positive, finite speed in mm/min. [`SpeedRangeMmPerMin`] is the
//! `{ min, max }` envelope used by printer constraints; construction
//! enforces `min < max` strictly so the range is non-empty.
//! [`SpeedMmPerMinOutOfRange`] and [`SpeedRangeMmPerMinBoundsInverted`]
//! report the respective rejected inputs.

use std::fmt;

/// A strictly positive, finite speed in millimetres per minute.
/// Constructed through [`PositiveFiniteMmPerMin::new`]; once you hold
/// one, the invariant holds by type. Use [`Self::value`] to unpack the
/// raw `f64` for APIs that take a plain speed.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct PositiveFiniteMmPerMin(f64);

/// Returned by [`PositiveFiniteMmPerMin::new`] (and its `TryFrom<f64>`
/// impl) when the input was zero, negative, or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedMmPerMinOutOfRange {
    pub value: f64,
}

impl fmt::Display for SpeedMmPerMinOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "speed must be strictly positive and finite, got {} mm/min",
            self.value,
        )
    }
}

impl std::error::Error for SpeedMmPerMinOutOfRange {}

impl PositiveFiniteMmPerMin {
    /// Construct from a raw `f64`. Fails with
    /// [`SpeedMmPerMinOutOfRange`] when the value is zero, negative,
    /// or non-finite (`NaN` / +/- infinity).
    pub fn new(value: f64) -> Result<Self, SpeedMmPerMinOutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(SpeedMmPerMinOutOfRange { value })
        }
    }

    /// The underlying mm/min value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for PositiveFiniteMmPerMin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for PositiveFiniteMmPerMin {
    type Error = SpeedMmPerMinOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PositiveFiniteMmPerMin> for f64 {
    fn from(value: PositiveFiniteMmPerMin) -> Self {
        value.0
    }
}

/// A `{ min, max }` envelope of speeds in millimetres per minute.
/// Construction enforces `min < max` strictly; both bounds are
/// [`PositiveFiniteMmPerMin`], so non-positive or non-finite values
/// fail before this type is reached.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(
    feature = "serde",
    serde(try_from = "RawSpeedRangeMmPerMin", into = "RawSpeedRangeMmPerMin")
)]
pub struct SpeedRangeMmPerMin {
    min: PositiveFiniteMmPerMin,
    max: PositiveFiniteMmPerMin,
}

/// Returned by [`SpeedRangeMmPerMin::new`] (and TOML deserialisation)
/// when `min` is not strictly less than `max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedRangeMmPerMinBoundsInverted {
    pub min: f64,
    pub max: f64,
}

impl fmt::Display for SpeedRangeMmPerMinBoundsInverted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "speed range min ({} mm/min) is not strictly less than max ({} mm/min)",
            self.min, self.max,
        )
    }
}

impl std::error::Error for SpeedRangeMmPerMinBoundsInverted {}

impl SpeedRangeMmPerMin {
    /// Construct from already-validated bounds. Fails with
    /// [`SpeedRangeMmPerMinBoundsInverted`] when `min` is not strictly
    /// less than `max`.
    pub fn new(
        min: PositiveFiniteMmPerMin,
        max: PositiveFiniteMmPerMin,
    ) -> Result<Self, SpeedRangeMmPerMinBoundsInverted> {
        if min.value() < max.value() {
            Ok(Self { min, max })
        } else {
            Err(SpeedRangeMmPerMinBoundsInverted {
                min: min.value(),
                max: max.value(),
            })
        }
    }

    #[inline]
    pub fn min(self) -> PositiveFiniteMmPerMin {
        self.min
    }

    #[inline]
    pub fn max(self) -> PositiveFiniteMmPerMin {
        self.max
    }
}

#[cfg(feature = "serde")]
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct RawSpeedRangeMmPerMin {
    min: PositiveFiniteMmPerMin,
    max: PositiveFiniteMmPerMin,
}

#[cfg(feature = "serde")]
impl TryFrom<RawSpeedRangeMmPerMin> for SpeedRangeMmPerMin {
    type Error = SpeedRangeMmPerMinBoundsInverted;
    fn try_from(raw: RawSpeedRangeMmPerMin) -> Result<Self, Self::Error> {
        Self::new(raw.min, raw.max)
    }
}

#[cfg(feature = "serde")]
impl From<SpeedRangeMmPerMin> for RawSpeedRangeMmPerMin {
    fn from(value: SpeedRangeMmPerMin) -> Self {
        Self {
            min: value.min,
            max: value.max,
        }
    }
}

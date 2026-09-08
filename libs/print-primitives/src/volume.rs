/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated volume scalars in cubic millimetres.
//!
//! [`VolumeMm3`] is the scalar primitive, carrying the material a
//! print encloses: output containers report it in their headers, and
//! a resin density turns it into a weight. [`VolumeOutOfRange`]
//! reports rejected inputs.

use std::fmt;

/// A volume in cubic millimetres. Constructed through
/// [`VolumeMm3::new`]; once you hold one, it is non-negative and
/// finite by type. Use [`Self::value`] to unpack the raw `f64` for
/// APIs that take a plain volume.
///
/// Zero is admissible: an empty plate encloses nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct VolumeMm3(f64);

/// Returned by [`VolumeMm3::new`] (and its `TryFrom<f64>` impl) when
/// the input was negative or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeOutOfRange {
    pub value: f64,
}

impl fmt::Display for VolumeOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "volume must be non-negative and finite, got {} mm^3",
            self.value,
        )
    }
}

impl std::error::Error for VolumeOutOfRange {}

impl VolumeMm3 {
    /// An empty volume.
    pub const ZERO: Self = Self(0.0);

    /// Construct from a raw `f64`. Fails with [`VolumeOutOfRange`]
    /// when the value is negative or non-finite (`NaN` / +/-
    /// infinity).
    pub fn new(value: f64) -> Result<Self, VolumeOutOfRange> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(VolumeOutOfRange { value })
        }
    }

    /// Read the underlying mm^3 value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for VolumeMm3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for VolumeMm3 {
    type Error = VolumeOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VolumeMm3> for f64 {
    fn from(value: VolumeMm3) -> Self {
        value.0
    }
}

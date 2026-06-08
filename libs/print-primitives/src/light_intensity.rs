/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated light-intensity scalars in milliwatts per square centimetre.
//!
//! [`PositiveFiniteMwPerCm2`] is the scalar primitive: a strictly
//! positive, finite light intensity in mW/cm^2. Used by printer
//! profiles to record the measured delivered intensity at the build
//! plane, which the resolver consumes when deriving exposure from a
//! material's target energy density. [`MwPerCm2OutOfRange`] reports
//! rejected inputs.

use std::fmt;

/// A strictly positive, finite light intensity in milliwatts per
/// square centimetre. Constructed through
/// [`PositiveFiniteMwPerCm2::new`]; once you hold one, the invariant
/// holds by type. Use [`Self::value`] to unpack the raw `f64` for
/// APIs that take a plain intensity.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct PositiveFiniteMwPerCm2(f64);

/// Returned by [`PositiveFiniteMwPerCm2::new`] (and its `TryFrom<f64>`
/// impl) when the input was zero, negative, or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MwPerCm2OutOfRange {
    pub value: f64,
}

impl fmt::Display for MwPerCm2OutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "light intensity must be strictly positive and finite, got {} mW/cm^2",
            self.value,
        )
    }
}

impl std::error::Error for MwPerCm2OutOfRange {}

impl PositiveFiniteMwPerCm2 {
    /// Construct from a raw `f64`. Fails with [`MwPerCm2OutOfRange`]
    /// when the value is zero, negative, or non-finite (`NaN` /
    /// +/- infinity).
    pub fn new(value: f64) -> Result<Self, MwPerCm2OutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(MwPerCm2OutOfRange { value })
        }
    }

    /// The underlying mW/cm^2 value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for PositiveFiniteMwPerCm2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for PositiveFiniteMwPerCm2 {
    type Error = MwPerCm2OutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PositiveFiniteMwPerCm2> for f64 {
    fn from(value: PositiveFiniteMwPerCm2) -> Self {
        value.0
    }
}

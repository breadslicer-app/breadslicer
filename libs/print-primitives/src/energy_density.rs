/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated energy-density scalars in millijoules per square centimetre.
//!
//! [`PositiveFiniteMjPerCm2`] is the scalar primitive: a strictly
//! positive, finite energy density in mJ/cm^2. Used by material
//! profiles to encode the target energy density the resin should
//! receive per layer, which the resolver consumes when deriving
//! per-layer exposure time from a printer's measured intensity.
//! [`MjPerCm2OutOfRange`] reports rejected inputs.

use std::fmt;

/// A strictly positive, finite energy density in millijoules per
/// square centimetre. Constructed through
/// [`PositiveFiniteMjPerCm2::new`]; once you hold one, the invariant
/// holds by type. Use [`Self::value`] to unpack the raw `f64` for
/// APIs that take a plain energy density.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct PositiveFiniteMjPerCm2(f64);

/// Returned by [`PositiveFiniteMjPerCm2::new`] (and its `TryFrom<f64>`
/// impl) when the input was zero, negative, or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MjPerCm2OutOfRange {
    pub value: f64,
}

impl fmt::Display for MjPerCm2OutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "energy density must be strictly positive and finite, got {} mJ/cm^2",
            self.value,
        )
    }
}

impl std::error::Error for MjPerCm2OutOfRange {}

impl PositiveFiniteMjPerCm2 {
    /// Construct from a raw `f64`. Fails with [`MjPerCm2OutOfRange`]
    /// when the value is zero, negative, or non-finite (`NaN` /
    /// +/- infinity).
    pub fn new(value: f64) -> Result<Self, MjPerCm2OutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(MjPerCm2OutOfRange { value })
        }
    }

    /// The underlying mJ/cm^2 value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for PositiveFiniteMjPerCm2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for PositiveFiniteMjPerCm2 {
    type Error = MjPerCm2OutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PositiveFiniteMjPerCm2> for f64 {
    fn from(value: PositiveFiniteMjPerCm2) -> Self {
        value.0
    }
}

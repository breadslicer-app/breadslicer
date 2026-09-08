/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated mass scalars in grams, and the resin density that
//! produces them.
//!
//! [`MassG`] is how much resin a print consumes, which output
//! containers report in their headers. [`DensityGPerCm3`] is the
//! property of the liquid that turns a volume into that mass;
//! [`DensityGPerCm3::mass_of`] is where the two units meet.
//! [`MassOutOfRange`] and [`DensityOutOfRange`] report rejected
//! inputs.

use std::fmt;

use crate::volume::VolumeMm3;

/// Cubic millimetres in a cubic centimetre.
const MM3_PER_CM3: f64 = 1_000.0;

/// A mass in grams. Constructed through [`MassG::new`]; once you hold
/// one, it is non-negative and finite by type. Use [`Self::value`] to
/// unpack the raw `f64`.
///
/// Zero is admissible: an empty plate consumes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct MassG(f64);

/// Returned by [`MassG::new`] (and its `TryFrom<f64>` impl) when the
/// input was negative or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassOutOfRange {
    pub value: f64,
}

impl fmt::Display for MassOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "mass must be non-negative and finite, got {} g",
            self.value,
        )
    }
}

impl std::error::Error for MassOutOfRange {}

impl MassG {
    /// An empty mass.
    pub const ZERO: Self = Self(0.0);

    /// Construct from a raw `f64`. Fails with [`MassOutOfRange`] when
    /// the value is negative or non-finite (`NaN` / +/- infinity).
    pub fn new(value: f64) -> Result<Self, MassOutOfRange> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(MassOutOfRange { value })
        }
    }

    /// Read the underlying grams as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for MassG {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for MassG {
    type Error = MassOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MassG> for f64 {
    fn from(value: MassG) -> Self {
        value.0
    }
}

/// A mass density in grams per cubic centimetre, the unit resin
/// vendors publish. Constructed through [`DensityGPerCm3::new`]; once
/// you hold one, it is strictly positive and finite by type.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct DensityGPerCm3(f64);

/// Returned by [`DensityGPerCm3::new`] (and its `TryFrom<f64>` impl)
/// when the input was zero, negative, or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensityOutOfRange {
    pub value: f64,
}

impl fmt::Display for DensityOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "density must be strictly positive and finite, got {} g/cm^3",
            self.value,
        )
    }
}

impl std::error::Error for DensityOutOfRange {}

impl DensityGPerCm3 {
    /// Construct from a raw `f64`. Fails with [`DensityOutOfRange`]
    /// when the value is zero, negative, or non-finite (`NaN` / +/-
    /// infinity). Nothing pourable weighs nothing, so zero is
    /// rejected rather than treated as an unstated density.
    pub fn new(value: f64) -> Result<Self, DensityOutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(DensityOutOfRange { value })
        }
    }

    /// Read the underlying g/cm^3 value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }

    /// Weigh a volume at this density.
    ///
    /// # Errors
    ///
    /// [`MassOutOfRange`] when the product overflows, which takes a
    /// volume no build plate holds.
    pub fn mass_of(self, volume: VolumeMm3) -> Result<MassG, MassOutOfRange> {
        MassG::new(volume.value() / MM3_PER_CM3 * self.0)
    }
}

impl fmt::Display for DensityGPerCm3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for DensityGPerCm3 {
    type Error = DensityOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<DensityGPerCm3> for f64 {
    fn from(value: DensityGPerCm3) -> Self {
        value.0
    }
}

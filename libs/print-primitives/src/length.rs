/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated positive-finite lengths in their canonical units.
//!
//! [`PositiveFiniteMm`] and [`PositiveFiniteMicrons`] are newtypes
//! carrying the "strictly positive and finite" invariant for a length
//! in millimetres and microns respectively. Both share the
//! [`LengthOutOfRange`] failure type (the invariant is unit-agnostic;
//! the wrapping newtype carries the unit) and the [`ParseLengthError`]
//! used by their `FromStr` impls. [`LengthRange`] is the `{ min, max }`
//! envelope in millimetres; construction enforces `min < max`
//! strictly through [`LengthRangeBoundsInverted`].

use std::fmt;
use std::str::FromStr;

/// A strictly positive, finite length in millimetres. Constructed
/// through [`PositiveFiniteMm::new`] or its [`FromStr`] impl; once
/// you hold one, the invariant holds by type. Use [`Self::value`]
/// to unpack the raw `f64` for APIs that take a plain length.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct PositiveFiniteMm(f64);

/// A strictly positive, finite length in microns (micrometres).
/// Constructed through [`PositiveFiniteMicrons::new`] or its
/// [`FromStr`] impl; once you hold one, the invariant holds by type.
/// Use [`Self::value`] to unpack the raw `f64`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct PositiveFiniteMicrons(f64);

/// Returned by [`PositiveFiniteMm::new`] and
/// [`PositiveFiniteMicrons::new`] (and their `FromStr` /
/// `TryFrom<f64>` impls) when the input was zero, negative, or
/// non-finite. The invariant captured is unit-agnostic; the wrapping
/// newtype carries the unit context.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LengthOutOfRange {
    pub value: f64,
}

impl fmt::Display for LengthOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "length must be strictly positive and finite, got {}",
            self.value,
        )
    }
}

impl std::error::Error for LengthOutOfRange {}

/// Returned by the `FromStr` impls of [`PositiveFiniteMm`] and
/// [`PositiveFiniteMicrons`]: either the input was not a valid
/// floating point number, or the parsed number was out of range.
#[derive(Debug)]
pub enum ParseLengthError {
    NotANumber(std::num::ParseFloatError),
    OutOfRange(LengthOutOfRange),
}

impl fmt::Display for ParseLengthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber(err) => write!(f, "invalid length value: {err}"),
            Self::OutOfRange(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ParseLengthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotANumber(err) => Some(err),
            Self::OutOfRange(err) => Some(err),
        }
    }
}

impl PositiveFiniteMm {
    /// Construct from a raw `f64`. Fails with
    /// [`LengthOutOfRange`] when the value is zero, negative,
    /// or non-finite (`NaN` / +/- infinity).
    pub fn new(value: f64) -> Result<Self, LengthOutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(LengthOutOfRange { value })
        }
    }

    /// The underlying millimetre value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for PositiveFiniteMm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for PositiveFiniteMm {
    type Error = LengthOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PositiveFiniteMm> for f64 {
    fn from(value: PositiveFiniteMm) -> Self {
        value.0
    }
}

impl FromStr for PositiveFiniteMm {
    type Err = ParseLengthError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let value: f64 = s.parse().map_err(ParseLengthError::NotANumber)?;
        Self::new(value).map_err(ParseLengthError::OutOfRange)
    }
}

impl PositiveFiniteMicrons {
    /// Construct from a raw `f64`. Fails with
    /// [`LengthOutOfRange`] when the value is zero, negative,
    /// or non-finite (`NaN` / +/- infinity).
    pub fn new(value: f64) -> Result<Self, LengthOutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(LengthOutOfRange { value })
        }
    }

    /// The underlying micron value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for PositiveFiniteMicrons {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for PositiveFiniteMicrons {
    type Error = LengthOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PositiveFiniteMicrons> for f64 {
    fn from(value: PositiveFiniteMicrons) -> Self {
        value.0
    }
}

impl FromStr for PositiveFiniteMicrons {
    type Err = ParseLengthError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let value: f64 = s.parse().map_err(ParseLengthError::NotANumber)?;
        Self::new(value).map_err(ParseLengthError::OutOfRange)
    }
}

/// A `{ min, max }` envelope of lengths in millimetres. Construction
/// enforces `min < max` strictly; both bounds are
/// [`PositiveFiniteMm`], so non-positive or non-finite values fail
/// before this type is reached.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(
    feature = "serde",
    serde(try_from = "RawLengthRange", into = "RawLengthRange")
)]
pub struct LengthRange {
    min: PositiveFiniteMm,
    max: PositiveFiniteMm,
}

/// Returned by [`LengthRange::new`] (and TOML deserialisation) when
/// `min` is not strictly less than `max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LengthRangeBoundsInverted {
    pub min: f64,
    pub max: f64,
}

impl fmt::Display for LengthRangeBoundsInverted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "length range min ({} mm) is not strictly less than max ({} mm)",
            self.min, self.max,
        )
    }
}

impl std::error::Error for LengthRangeBoundsInverted {}

impl LengthRange {
    /// Construct from already-validated bounds. Fails with
    /// [`LengthRangeBoundsInverted`] when `min` is not strictly less
    /// than `max`.
    pub fn new(
        min: PositiveFiniteMm,
        max: PositiveFiniteMm,
    ) -> Result<Self, LengthRangeBoundsInverted> {
        if min.value() < max.value() {
            Ok(Self { min, max })
        } else {
            Err(LengthRangeBoundsInverted {
                min: min.value(),
                max: max.value(),
            })
        }
    }

    #[inline]
    pub fn min(self) -> PositiveFiniteMm {
        self.min
    }

    #[inline]
    pub fn max(self) -> PositiveFiniteMm {
        self.max
    }
}

#[cfg(feature = "serde")]
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct RawLengthRange {
    min: PositiveFiniteMm,
    max: PositiveFiniteMm,
}

#[cfg(feature = "serde")]
impl TryFrom<RawLengthRange> for LengthRange {
    type Error = LengthRangeBoundsInverted;
    fn try_from(raw: RawLengthRange) -> Result<Self, Self::Error> {
        Self::new(raw.min, raw.max)
    }
}

#[cfg(feature = "serde")]
impl From<LengthRange> for RawLengthRange {
    fn from(value: LengthRange) -> Self {
        Self {
            min: value.min,
            max: value.max,
        }
    }
}

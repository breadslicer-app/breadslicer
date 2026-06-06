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
//! used by their `FromStr` impls.

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

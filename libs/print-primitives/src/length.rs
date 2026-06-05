/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated millimetre lengths.
//!
//! [`PositiveFiniteMm`] is a newtype carrying the "strictly positive
//! and finite" invariant for a millimetre length. The companion
//! [`NegativeLengthError`] and [`ParseLengthError`] cover the two
//! failure modes (out-of-range construction and string parsing).

use std::fmt;
use std::str::FromStr;

/// A strictly positive, finite length in millimetres. Constructed
/// through [`PositiveFiniteMm::new`] or its [`FromStr`] impl; once
/// you hold one, the invariant holds by type. Use [`Self::value`]
/// to unpack the raw `f64` for APIs that take a plain length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositiveFiniteMm(f64);

/// Returned by [`PositiveFiniteMm::new`] when the input was zero,
/// negative, or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NegativeLengthError {
    pub value: f64,
}

impl fmt::Display for NegativeLengthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "millimetre length must be strictly positive and finite, got {}",
            self.value,
        )
    }
}

impl std::error::Error for NegativeLengthError {}

/// Returned by [`PositiveFiniteMm`]'s [`FromStr`] impl: either the
/// input was not a valid floating point number, or the parsed
/// number was out of range.
#[derive(Debug)]
pub enum ParseLengthError {
    NotANumber(std::num::ParseFloatError),
    OutOfRange(NegativeLengthError),
}

impl fmt::Display for ParseLengthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber(err) => write!(f, "invalid millimetre value: {err}"),
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
    /// [`NegativeLengthError`] when the value is zero, negative,
    /// or non-finite (`NaN` / +/- infinity).
    pub fn new(value: f64) -> Result<Self, NegativeLengthError> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(NegativeLengthError { value })
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

impl FromStr for PositiveFiniteMm {
    type Err = ParseLengthError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let value: f64 = s.parse().map_err(ParseLengthError::NotANumber)?;
        Self::new(value).map_err(ParseLengthError::OutOfRange)
    }
}

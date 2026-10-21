/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated time-duration scalars in seconds.
//!
//! [`PositiveFiniteSec`] is the scalar primitive: a strictly
//! positive, finite duration in seconds. Used by material profiles
//! to encode per-printer exposure times (normal and bottom-layer)
//! and by any other field that needs a positive-finite seconds
//! value. [`SecOutOfRange`] reports rejected inputs.

use std::fmt;

/// A strictly positive, finite duration in seconds. Constructed
/// through [`PositiveFiniteSec::new`]; once you hold one, the
/// invariant holds by type. Use [`Self::value`] to unpack the raw
/// `f64` for APIs that take a plain duration.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct PositiveFiniteSec(f64);

/// Returned by [`PositiveFiniteSec::new`] (and its `TryFrom<f64>`
/// impl) when the input was zero, negative, or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecOutOfRange {
    pub value: f64,
}

impl fmt::Display for SecOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "duration must be strictly positive and finite, got {} s",
            self.value,
        )
    }
}

impl std::error::Error for SecOutOfRange {}

impl PositiveFiniteSec {
    /// Construct from a raw `f64`. Fails with [`SecOutOfRange`]
    /// when the value is zero, negative, or non-finite (`NaN` /
    /// +/- infinity).
    pub fn new(value: f64) -> Result<Self, SecOutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(SecOutOfRange { value })
        }
    }

    /// Return the underlying seconds value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }

    /// Interpolate from this duration towards `target` by `fraction`,
    /// clamped to `[0.0, 1.0]`.
    #[inline]
    #[must_use]
    pub fn interpolate_towards(self, target: Self, fraction: f64) -> Self {
        let fraction = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        let interpolated = (target.0 - self.0).mul_add(fraction, self.0);
        let (low, high) = if self.0 <= target.0 {
            (self.0, target.0)
        } else {
            (target.0, self.0)
        };
        Self(interpolated.clamp(low, high))
    }
}

impl fmt::Display for PositiveFiniteSec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for PositiveFiniteSec {
    type Error = SecOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PositiveFiniteSec> for f64 {
    fn from(value: PositiveFiniteSec) -> Self {
        value.0
    }
}

/// A non-negative, finite duration in seconds. Constructed through
/// [`NonNegativeFiniteSec::new`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct NonNegativeFiniteSec(f64);

/// Returned by [`NonNegativeFiniteSec::new`] (and its `TryFrom<f64>`
/// impl) when the input was negative or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NegativeDuration {
    pub value: f64,
}

impl fmt::Display for NegativeDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "duration must be non-negative and finite, got {} s",
            self.value,
        )
    }
}

impl std::error::Error for NegativeDuration {}

impl NonNegativeFiniteSec {
    /// A zero-length pause.
    pub const ZERO: Self = Self(0.0);

    /// Construct from a raw `f64`. Fails with
    /// [`NegativeDuration`] when the value is negative or
    /// non-finite (`NaN` / +/- infinity).
    pub fn new(value: f64) -> Result<Self, NegativeDuration> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(NegativeDuration { value })
        }
    }

    /// The underlying seconds value as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for NonNegativeFiniteSec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<PositiveFiniteSec> for NonNegativeFiniteSec {
    fn from(value: PositiveFiniteSec) -> Self {
        Self(value.value())
    }
}

impl TryFrom<f64> for NonNegativeFiniteSec {
    type Error = NegativeDuration;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NonNegativeFiniteSec> for f64 {
    fn from(value: NonNegativeFiniteSec) -> Self {
        value.0
    }
}

/// A factor scaling a duration, as in the multiplier of a linear
/// correction. Constructed through [`TimeMultiplier::new`]; once you
/// hold one, it is strictly positive and finite by type.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct TimeMultiplier(f64);

/// Returned by [`TimeMultiplier::new`] (and its `TryFrom<f64>` impl)
/// when the input was zero, negative, or non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeMultiplierOutOfRange {
    pub value: f64,
}

impl fmt::Display for TimeMultiplierOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a time multiplier must be strictly positive and finite, got {}",
            self.value,
        )
    }
}

impl std::error::Error for TimeMultiplierOutOfRange {}

impl TimeMultiplier {
    /// The multiplier that changes nothing.
    pub const IDENTITY: Self = Self(1.0);

    /// Construct from a raw `f64`. Fails with
    /// [`TimeMultiplierOutOfRange`] when the value is zero, negative,
    /// or non-finite (`NaN` / +/- infinity). Zero is rejected because
    /// it would report every print as instantaneous.
    pub fn new(value: f64) -> Result<Self, TimeMultiplierOutOfRange> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(TimeMultiplierOutOfRange { value })
        }
    }

    /// Read the underlying factor as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for TimeMultiplier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for TimeMultiplier {
    type Error = TimeMultiplierOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TimeMultiplier> for f64 {
    fn from(value: TimeMultiplier) -> Self {
        value.0
    }
}

/// A shift along the time axis in seconds, as in the additive term of
/// a linear correction. Constructed through [`TimeOffsetSec::new`];
/// once you hold one, it is finite by type. Negative is meaningful:
/// it shortens what it corrects.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct TimeOffsetSec(f64);

/// Returned by [`TimeOffsetSec::new`] (and its `TryFrom<f64>` impl)
/// when the input was non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeOffsetOutOfRange {
    pub value: f64,
}

impl fmt::Display for TimeOffsetOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a time offset must be finite, got {} s", self.value)
    }
}

impl std::error::Error for TimeOffsetOutOfRange {}

impl TimeOffsetSec {
    /// The offset that changes nothing.
    pub const ZERO: Self = Self(0.0);

    /// Construct from a raw `f64`. Fails with
    /// [`TimeOffsetOutOfRange`] when the value is non-finite (`NaN` /
    /// +/- infinity).
    pub fn new(value: f64) -> Result<Self, TimeOffsetOutOfRange> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(TimeOffsetOutOfRange { value })
        }
    }

    /// Read the underlying seconds as a plain `f64`.
    #[inline]
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for TimeOffsetSec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<f64> for TimeOffsetSec {
    type Error = TimeOffsetOutOfRange;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TimeOffsetSec> for f64 {
    fn from(value: TimeOffsetSec) -> Self {
        value.0
    }
}

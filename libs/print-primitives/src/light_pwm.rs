/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! How hard the UV source is driven while a layer cures.
//!
//! [`LightPwm`] is the validated scalar: a duty cycle as a percentage
//! in the inclusive range `[0.0, 100.0]`. It is stored as a percentage
//! because that is what an operator sets and what a profile records;
//! printer files store a byte instead, and converting is the writer's
//! business.
//!
//! Because it is a duty cycle, average optical power is linear in it.
//! Anything deriving an exposure time from a target energy density has
//! to divide by the duty as well as by the light intensity, or it
//! silently assumes full power. [`LightPwmOutOfRange`] reports
//! rejected inputs.

use std::fmt;

const LIGHT_PWM_MIN_PCT: f64 = 0.0;
const LIGHT_PWM_MAX_PCT: f64 = 100.0;

/// A UV-source duty cycle, as a finite percentage in the inclusive
/// range `[0.0, 100.0]`. Constructed through [`LightPwm::new`]; once
/// you hold one, the bound invariant holds by type.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct LightPwm(f64);

/// Returned by [`LightPwm::new`] (and its `TryFrom<f64>` impl) when
/// the input was outside the inclusive `[0.0, 100.0]` range or
/// non-finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightPwmOutOfRange {
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

impl fmt::Display for LightPwmOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "light PWM must be a finite percentage within [{}, {}], got {}",
            self.min, self.max, self.value,
        )
    }
}

impl std::error::Error for LightPwmOutOfRange {}

impl LightPwm {
    /// The source driven at full power.
    pub const FULL: Self = Self(LIGHT_PWM_MAX_PCT);

    /// Construct from a raw percentage. Fails with
    /// [`LightPwmOutOfRange`] when the value is outside
    /// `[0.0, 100.0]` or non-finite.
    pub fn new(percent: f64) -> Result<Self, LightPwmOutOfRange> {
        if percent.is_finite() && (LIGHT_PWM_MIN_PCT..=LIGHT_PWM_MAX_PCT).contains(&percent) {
            Ok(Self(percent))
        } else {
            Err(LightPwmOutOfRange {
                value: percent,
                min: LIGHT_PWM_MIN_PCT,
                max: LIGHT_PWM_MAX_PCT,
            })
        }
    }

    /// Return the duty cycle as a percentage.
    #[inline]
    pub fn percent(self) -> f64 {
        self.0
    }

    /// Return the duty cycle as a fraction in `[0.0, 1.0]`.
    ///
    /// This is the factor average optical power scales by, so it is
    /// what an energy-density calculation divides through.
    #[inline]
    pub fn duty(self) -> f64 {
        self.0 / LIGHT_PWM_MAX_PCT
    }
}

impl Default for LightPwm {
    fn default() -> Self {
        Self::FULL
    }
}

impl fmt::Display for LightPwm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", self.0)
    }
}

impl TryFrom<f64> for LightPwm {
    type Error = LightPwmOutOfRange;
    fn try_from(percent: f64) -> Result<Self, Self::Error> {
        Self::new(percent)
    }
}

impl From<LightPwm> for f64 {
    fn from(value: LightPwm) -> Self {
        value.0
    }
}

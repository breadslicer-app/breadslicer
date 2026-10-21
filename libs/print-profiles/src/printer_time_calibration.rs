/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Time-calibration block of a printer profile.
//!
//! [`PrinterTimeCalibration`] mirrors the `[time_calibration]`
//! section of a printer-profile TOML file: the coefficients of the
//! linear correction `A * T + B` that turns the duration a print's
//! own schedule adds up to into the duration the machine actually
//! takes. A machine spends time no schedule accounts for, on
//! acceleration ramps and fixed per-layer overhead, and how much
//! differs per model, so the pair is fitted from real prints rather
//! than derived.
//!
//! Both fields are optional. Absent means the correction leaves the
//! schedule alone, which is what an uncalibrated printer reports.

use sc_print_primitives::{TimeMultiplier, TimeOffsetSec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct PrinterTimeCalibration {
    /// `A`, scaling the schedule. Above 1 for the usual case of a
    /// machine slower than the sum of its own instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_time_multiplier: Option<TimeMultiplier>,
    /// `B`, added after the scaling. Carries whatever a print costs
    /// once rather than per layer, such as homing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_time_offset_sec: Option<TimeOffsetSec>,
}

impl PrinterTimeCalibration {
    /// Apply the correction to a duration in seconds, never
    /// returning less than zero: a correction can subtract more than
    /// a short print takes, and no print runs backwards.
    pub fn corrected_sec(&self, schedule_sec: f64) -> f64 {
        let multiplier = self
            .print_time_multiplier
            .unwrap_or(TimeMultiplier::IDENTITY);
        let offset = self.print_time_offset_sec.unwrap_or(TimeOffsetSec::ZERO);
        multiplier
            .value()
            .mul_add(schedule_sec, offset.value())
            .max(0.0)
    }
}

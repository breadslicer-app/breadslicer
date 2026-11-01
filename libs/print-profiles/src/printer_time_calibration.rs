/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Time-calibration block of a printer profile.
//!
//! [`PrinterTimeCalibration`] mirrors the `[time_calibration]`
//! section of a printer-profile TOML file: the coefficients of the
//! linear correction `A * T + C * N + B` that turns the duration `T`
//! a print's own schedule of `N` layers adds up to into the duration
//! the machine actually takes. A machine spends time no schedule
//! accounts for, on acceleration ramps and fixed per-layer overhead,
//! and how much differs per model, so the coefficients are fitted
//! from real prints rather than derived.
//!
//! Every field is optional. Absent means the correction leaves the
//! schedule alone, which is what an uncalibrated printer reports.

use sc_print_primitives::{NonNegativeFiniteSec, TimeMultiplier, TimeOffsetSec};
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
    /// `C`, added once per layer and never scaled: what each layer
    /// costs on top of its record, such as ramping the platform and
    /// loading the next image. Unlike `A`, it leaves the exposure
    /// alone, which the machine spends exactly as told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_layer_overhead_sec: Option<NonNegativeFiniteSec>,
}

impl PrinterTimeCalibration {
    /// Apply the correction to the duration in seconds a schedule of
    /// `layer_count` layers adds up to, never returning less than
    /// zero: a correction can subtract more than a short print takes,
    /// and no print runs backwards.
    pub fn corrected_sec(&self, schedule_sec: f64, layer_count: usize) -> f64 {
        let multiplier = self
            .print_time_multiplier
            .unwrap_or(TimeMultiplier::IDENTITY);
        let offset = self.print_time_offset_sec.unwrap_or(TimeOffsetSec::ZERO);
        let per_layer = self
            .per_layer_overhead_sec
            .unwrap_or(NonNegativeFiniteSec::ZERO);
        let overhead = per_layer
            .value()
            .mul_add(layer_count as f64, offset.value());
        multiplier.value().mul_add(schedule_sec, overhead).max(0.0)
    }
}

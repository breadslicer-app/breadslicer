/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Top-level printer profile.
//!
//! [`PrinterProfile`] composes the four section blocks of a
//! printer-profile TOML file: identity, hardware, capabilities, and
//! constraints. Deserialisation rejects the reserved `[instance]`
//! section with [`PrinterProfileReservedInstanceSection`]: that
//! section is intended for a future per-physical-device overlay and
//! has no v0 semantics, so silently dropping it would let users
//! think their overlay took effect when it did not.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::printer_capabilities::PrinterCapabilities;
use crate::printer_constraints::PrinterConstraints;
use crate::printer_defaults::PrinterDefaults;
use crate::printer_hardware::PrinterHardware;
use crate::printer_identity::PrinterIdentity;
use crate::printer_time_calibration::PrinterTimeCalibration;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "RawPrinterProfile")]
pub struct PrinterProfile {
    pub identity: PrinterIdentity,
    pub hardware: PrinterHardware,
    pub capabilities: PrinterCapabilities,
    pub constraints: PrinterConstraints,
    /// What this machine expects where no resin states its own.
    #[serde(default)]
    pub defaults: PrinterDefaults,
    /// How far this machine's real timing departs from the schedule
    /// it is handed. Absent in profiles written before the section
    /// existed, and in those nobody has calibrated.
    #[serde(default)]
    pub time_calibration: PrinterTimeCalibration,
}

#[derive(Debug, Deserialize)]
struct RawPrinterProfile {
    identity: PrinterIdentity,
    hardware: PrinterHardware,
    capabilities: PrinterCapabilities,
    constraints: PrinterConstraints,
    #[serde(default)]
    defaults: PrinterDefaults,
    #[serde(default)]
    time_calibration: PrinterTimeCalibration,
    #[serde(default)]
    instance: Option<serde::de::IgnoredAny>,
}

/// Returned when a printer-profile TOML carries a top-level
/// `[instance]` section. The section is reserved for a future
/// per-physical-device overlay and is not interpreted by v0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrinterProfileReservedInstanceSection;

impl fmt::Display for PrinterProfileReservedInstanceSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the [instance] section is reserved for a future per-device overlay and is not supported in v0",
        )
    }
}

impl std::error::Error for PrinterProfileReservedInstanceSection {}

impl TryFrom<RawPrinterProfile> for PrinterProfile {
    type Error = PrinterProfileReservedInstanceSection;
    fn try_from(raw: RawPrinterProfile) -> Result<Self, Self::Error> {
        if raw.instance.is_some() {
            return Err(PrinterProfileReservedInstanceSection);
        }
        Ok(Self {
            identity: raw.identity,
            hardware: raw.hardware,
            capabilities: raw.capabilities,
            constraints: raw.constraints,
            defaults: raw.defaults,
            time_calibration: raw.time_calibration,
        })
    }
}

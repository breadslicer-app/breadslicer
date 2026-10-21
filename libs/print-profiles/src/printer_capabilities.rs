/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Capability block of a printer profile.
//!
//! [`PrinterCapabilities`] mirrors the `[capabilities]` section of a
//! printer-profile TOML file: the non-empty list of output formats
//! the printer can consume, whether the printer's panel can display
//! grayscale (PWM-modulated per-pixel intensity), whether the printer
//! supports two-stage lift, the maximum number of transition layers
//! it accepts (when supported), whether the printer has a camera,
//! and the network protocol it exposes for remote control (when
//! networked).

use sc_print_primitives::{NetworkProtocol, OutputFormatBindings};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrinterCapabilities {
    pub output_formats: OutputFormatBindings,
    pub supports_grayscale: bool,
    pub two_stage_lift: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_layers: Option<u32>,
    pub has_camera: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_protocol: Option<NetworkProtocol>,
}

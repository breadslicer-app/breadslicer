/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Top-level material profile.
//!
//! [`MaterialProfile`] composes the four sections of a
//! material-profile TOML file: identity, intent, resin-level
//! defaults, and per-printer calibrated values. Deserialisation
//! rejects three reserved shapes through
//! [`MaterialProfileReservedSection`]:
//!
//! - top-level `[per_environment.*]` and `[per_session_state.*]`
//!   sections, intended for future conditional override layers;
//! - the per-printer `[per_printer."<id>".constraints]` section,
//!   intended for future material tightening of printer constraints.
//!
//! Each is reserved so adding it today produces a clear
//! "not supported in v0" message rather than a silent acceptance
//! that would let users believe the override is taking effect.

use std::collections::BTreeMap;
use std::fmt;

use sc_print_primitives::PrinterId;
use serde::{Deserialize, Serialize};

use crate::material_defaults::MaterialDefaults;
use crate::material_identity::MaterialIdentity;
use crate::material_intent::MaterialIntent;
use crate::material_per_printer::MaterialPerPrinter;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "RawMaterialProfile")]
pub struct MaterialProfile {
    /// True for a profile that supplements the same-id base profile
    /// with additional per-printer calibration blocks instead of
    /// replacing it. Declared first so it serialises above the
    /// section tables; omitted from the TOML when false.
    #[serde(skip_serializing_if = "is_false")]
    pub extension: bool,
    pub identity: MaterialIdentity,
    pub intent: MaterialIntent,
    pub defaults: MaterialDefaults,
    pub per_printer: BTreeMap<PrinterId, MaterialPerPrinter>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Deserialize)]
struct RawMaterialProfile {
    #[serde(default)]
    extension: bool,
    identity: MaterialIdentity,
    intent: MaterialIntent,
    defaults: MaterialDefaults,
    #[serde(default)]
    per_printer: BTreeMap<PrinterId, RawPerPrinterEntry>,
    #[serde(default)]
    per_environment: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    per_session_state: Option<serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct RawPerPrinterEntry {
    #[serde(flatten)]
    inner: MaterialPerPrinter,
    #[serde(default)]
    constraints: Option<serde::de::IgnoredAny>,
}

/// Reserved section detected in a material-profile TOML. The
/// matching variants describe which top-level or per-printer
/// section was present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterialProfileReservedSection {
    PerEnvironment,
    PerSessionState,
    PerPrinterConstraints { printer_id: PrinterId },
}

impl fmt::Display for MaterialProfileReservedSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PerEnvironment => write!(
                f,
                "the [per_environment.*] sections are reserved for future conditional overrides and are not supported in v0",
            ),
            Self::PerSessionState => write!(
                f,
                "the [per_session_state.*] sections are reserved for future conditional overrides and are not supported in v0",
            ),
            Self::PerPrinterConstraints { printer_id } => write!(
                f,
                "[per_printer.\"{}\".constraints] is reserved for material tightening of printer constraints and is not supported in v0",
                printer_id.as_str(),
            ),
        }
    }
}

impl std::error::Error for MaterialProfileReservedSection {}

impl TryFrom<RawMaterialProfile> for MaterialProfile {
    type Error = MaterialProfileReservedSection;
    fn try_from(raw: RawMaterialProfile) -> Result<Self, Self::Error> {
        if raw.per_environment.is_some() {
            return Err(MaterialProfileReservedSection::PerEnvironment);
        }
        if raw.per_session_state.is_some() {
            return Err(MaterialProfileReservedSection::PerSessionState);
        }
        let mut per_printer = BTreeMap::new();
        for (printer_id, entry) in raw.per_printer {
            if entry.constraints.is_some() {
                return Err(MaterialProfileReservedSection::PerPrinterConstraints { printer_id });
            }
            per_printer.insert(printer_id, entry.inner);
        }
        Ok(Self {
            extension: raw.extension,
            identity: raw.identity,
            intent: raw.intent,
            defaults: raw.defaults,
            per_printer,
        })
    }
}

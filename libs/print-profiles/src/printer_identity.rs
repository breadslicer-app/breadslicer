/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Identity block of a printer profile.
//!
//! [`PrinterIdentity`] mirrors the `[printer_identity]` section of a
//! printer-profile TOML file: validated slug, vendor naming, profile
//! version (semver), and an optional asset path used by the loader to
//! resolve a thumbnail image relative to the profile bundle.

use sc_print_primitives::{NonEmptyString, PrinterId};
use semver::Version;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrinterIdentity {
    pub id: PrinterId,
    pub manufacturer: NonEmptyString,
    pub family: NonEmptyString,
    pub name: NonEmptyString,
    pub profile_version: Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_asset_path: Option<String>,
}

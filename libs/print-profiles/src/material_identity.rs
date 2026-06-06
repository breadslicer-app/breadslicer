/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Identity block of a material profile.
//!
//! [`MaterialIdentity`] mirrors the `[material_identity]` section of
//! a material-profile TOML file: validated slug, vendor naming, an
//! open family string (resin families are not enumerable upstream),
//! and a semver profile version used by the loader to gate import
//! compatibility.

use sc_print_primitives::{MaterialId, NonEmptyString};
use semver::Version;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct MaterialIdentity {
    pub id: MaterialId,
    pub brand: NonEmptyString,
    pub family: NonEmptyString,
    pub name: NonEmptyString,
    pub profile_version: Version,
}

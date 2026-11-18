/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where a profile of the user's own came from (#20): the profile it was
//! made from, the file it was imported from, and the day. Carried in the
//! identity section as an `origin` table; absent in the profiles that ship
//! and in those written before it existed.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProfileOrigin {
    /// The name of the profile it was made from, as it read then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_from: Option<String>,
    /// That profile's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_from_id: Option<String>,
    /// The name of the file it was imported from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// The day it was made, `2026-11-18`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_on: Option<String>,
}

impl ProfileOrigin {
    /// Made from the profile `name` (`id`) on `day`.
    pub fn made_from(name: &str, id: &str, day: &str) -> Self {
        Self {
            made_from: Some(name.to_owned()),
            made_from_id: Some(id.to_owned()),
            imported_from: None,
            made_on: Some(day.to_owned()),
        }
    }

    /// Imported from the file `file` on `day`.
    pub fn imported_from(file: &str, day: &str) -> Self {
        Self {
            made_from: None,
            made_from_id: None,
            imported_from: Some(file.to_owned()),
            made_on: Some(day.to_owned()),
        }
    }
}

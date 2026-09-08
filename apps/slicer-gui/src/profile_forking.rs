/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Assembling a forked resin profile.
//!
//! A fork is a new full custom profile seeded from a source profile:
//! the identity keeps the source's family / profile version under a
//! user-entered name and brand plus a fresh id derived from the name,
//! and the user picks which parts to carry over (intent, defaults,
//! any of the source's own calibration blocks). Selected parts are
//! copied verbatim; unselected ones start empty. The result is an
//! ordinary material profile, never an extension. The dialog wiring
//! lives in the preferences window.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use slint::SharedString;

use sc_print_primitives::{MaterialId, NonEmptyString};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialProfile,
};

use crate::i18n::ctr;

/// Which parts of the source profile the fork carries over.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ForkParts {
    pub intent: bool,
    pub defaults: bool,
    /// The source calibration blocks to copy, by target.
    pub blocks: Vec<CalibrationTarget>,
}

/// Why a fork could not be assembled. `Display` keeps the English
/// text for logs; the dialog shows [`Self::user_message`] instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ForkError {
    EmptyName,
    EmptyBrand,
    NoAvailableId,
}

impl ForkError {
    /// The message shown in the fork dialog's inline error line, in
    /// the user's language.
    pub(crate) fn user_message(&self) -> SharedString {
        match self {
            Self::EmptyName => ctr!("ForkDialog", "Enter a name for the forked profile"),
            Self::EmptyBrand => ctr!("ForkDialog", "Enter a brand for the forked profile"),
            Self::NoAvailableId => {
                ctr!(
                    "ForkDialog",
                    "Could not derive a free profile id from that name"
                )
            }
        }
    }
}

impl fmt::Display for ForkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "Enter a name for the forked profile"),
            Self::EmptyBrand => write!(f, "Enter a brand for the forked profile"),
            Self::NoAvailableId => write!(f, "Could not derive a free profile id from that name"),
        }
    }
}

impl std::error::Error for ForkError {}

/// Assemble the fork: a fresh unique id derived from `name`, the
/// entered name and brand, the source's other identity fields, and
/// verbatim copies of exactly the selected parts. `existing` is the
/// set of material ids already in the catalog, used to keep the
/// derived id unique.
pub(crate) fn build_fork(
    source: &MaterialProfile,
    parts: &ForkParts,
    name: &str,
    brand: &str,
    existing: &BTreeSet<MaterialId>,
) -> Result<MaterialProfile, ForkError> {
    let name = NonEmptyString::new(name.trim()).map_err(|_| ForkError::EmptyName)?;
    let brand = NonEmptyString::new(brand.trim()).map_err(|_| ForkError::EmptyBrand)?;
    let id =
        unique_material_id(&slugify(name.as_str()), existing).ok_or(ForkError::NoAvailableId)?;

    Ok(MaterialProfile {
        extension: false,
        identity: MaterialIdentity {
            id,
            brand,
            family: source.identity.family.clone(),
            name,
            profile_version: source.identity.profile_version.clone(),
        },
        intent: if parts.intent {
            source.intent
        } else {
            MaterialIntent {
                target_energy_density_mj_per_cm2: None,
                target_layer_height_mm: None,
            }
        },
        defaults: if parts.defaults {
            source.defaults
        } else {
            MaterialDefaults::default()
        },
        // The fork is the same liquid under another name, so the
        // measured constants come along whatever else is copied.
        physical: source.physical,
        per_printer: parts
            .blocks
            .iter()
            .filter_map(|target| {
                source
                    .per_printer
                    .get(target)
                    .map(|block| (target.clone(), *block))
            })
            .collect::<BTreeMap<_, _>>(),
    })
}

/// Reduce a display name to a slug matching `^[a-z0-9][a-z0-9-]*$`:
/// lowercase ASCII alphanumerics kept, every other run collapsed to a
/// single hyphen, no leading or trailing hyphen. May be empty when the
/// name has no slug-able characters.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_hyphen = false;
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            if pending_hyphen && !out.is_empty() {
                out.push('-');
            }
            pending_hyphen = false;
            out.push(lower);
        } else {
            pending_hyphen = true;
        }
    }
    out
}

/// The first free id from `slug`, `slug-2`, `slug-3`, ... that is a
/// valid [`MaterialId`] and not already present. `None` when `slug` is
/// empty or every candidate is taken.
fn unique_material_id(slug: &str, existing: &BTreeSet<MaterialId>) -> Option<MaterialId> {
    if slug.is_empty() {
        return None;
    }
    let base = MaterialId::new(slug).ok()?;
    if !existing.contains(&base) {
        return Some(base);
    }
    (2..10_000).find_map(|n| {
        MaterialId::new(&format!("{slug}-{n}"))
            .ok()
            .filter(|id| !existing.contains(id))
    })
}

#[cfg(test)]
mod tests;

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Authoring a new resin calibration profile from the Preferences resin
//! list.
//!
//! When a resin has no calibration for the selected printer, the user
//! can supply the values themselves. That produces an ordinary material
//! profile, seeded from the source resin's identity / intent / defaults
//! plus a single `[per_printer.<id>]` block carrying the entered values,
//! written to the user library like any other profile. Nothing about
//! the merge or the resolver changes.
//!
//! This module holds the window-independent core: validating the typed
//! form values through the `sc_print_primitives` constructors, deriving
//! a unique id from the entered name, and assembling the new
//! [`MaterialProfile`]. The dialog wiring lives in the preferences
//! window.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sc_print_primitives::{
    MaterialId, NonEmptyString, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
    PrinterId,
};
use sc_print_profiles::{MaterialIdentity, MaterialPerPrinter, MaterialProfile};

/// The raw form values entered in the create-calibration dialog. All
/// fields are the strings straight out of the text inputs.
#[derive(Clone, Debug, Default)]
pub(crate) struct CalibrationInput {
    pub name: String,
    pub normal_exposure_sec: String,
    pub bottom_exposure_sec: String,
    pub bottom_layer_count: String,
    pub lift_distance_mm: String,
    pub lift_speed_mm_min: String,
    pub retract_speed_mm_min: String,
}

/// A form value that passed validation: a unique id, a non-empty name,
/// and the calibrated per-printer block.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ValidatedCalibration {
    pub id: MaterialId,
    pub name: NonEmptyString,
    pub per_printer: MaterialPerPrinter,
}

/// Why a create-calibration form could not be accepted. Rendered
/// straight into the dialog's inline error message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AuthoringError {
    EmptyName,
    NotANumber { field: &'static str },
    OutOfRange { field: &'static str },
    NoAvailableId,
}

impl fmt::Display for AuthoringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "Enter a name for the new calibration profile"),
            Self::NotANumber { field } => write!(f, "{field} must be a number"),
            Self::OutOfRange { field } => {
                write!(f, "{field} must be a positive, finite value")
            }
            Self::NoAvailableId => write!(f, "Could not derive a free profile id from that name"),
        }
    }
}

impl std::error::Error for AuthoringError {}

/// Validate a form and, on success, produce the id / name / per-printer
/// block. `existing` is the set of material ids already in the catalog,
/// used to keep the derived id unique.
pub(crate) fn validate_input(
    input: &CalibrationInput,
    existing: &BTreeSet<MaterialId>,
) -> Result<ValidatedCalibration, AuthoringError> {
    let name = input.name.trim();
    let name = NonEmptyString::new(name).map_err(|_| AuthoringError::EmptyName)?;
    let id = unique_material_id(&slugify(name.as_str()), existing)
        .ok_or(AuthoringError::NoAvailableId)?;

    let per_printer = MaterialPerPrinter {
        normal_exposure_sec: Some(parse_sec("Normal exposure", &input.normal_exposure_sec)?),
        bottom_exposure_sec: Some(parse_sec("Bottom exposure", &input.bottom_exposure_sec)?),
        bottom_layer_count: Some(parse_count(
            "Bottom layer count",
            &input.bottom_layer_count,
        )?),
        lift_distance_mm: Some(parse_mm("Lift distance", &input.lift_distance_mm)?),
        lift_speed_mm_min: Some(parse_speed("Lift speed", &input.lift_speed_mm_min)?),
        retract_speed_mm_min: Some(parse_speed("Retract speed", &input.retract_speed_mm_min)?),
    };

    Ok(ValidatedCalibration {
        id,
        name,
        per_printer,
    })
}

/// Assemble the new material profile: identity seeded from the source
/// resin (brand / family / profile version) with the new id and name,
/// the source's intent and defaults, and a single per-printer block for
/// `printer`.
pub(crate) fn build_material_profile(
    source: &MaterialProfile,
    printer: &PrinterId,
    validated: ValidatedCalibration,
) -> MaterialProfile {
    MaterialProfile {
        identity: MaterialIdentity {
            id: validated.id,
            brand: source.identity.brand.clone(),
            family: source.identity.family.clone(),
            name: validated.name,
            profile_version: source.identity.profile_version.clone(),
        },
        intent: source.intent,
        defaults: source.defaults,
        per_printer: BTreeMap::from([(printer.clone(), validated.per_printer)]),
    }
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

fn parse_f64(field: &'static str, text: &str) -> Result<f64, AuthoringError> {
    text.trim()
        .parse::<f64>()
        .map_err(|_| AuthoringError::NotANumber { field })
}

fn parse_sec(field: &'static str, text: &str) -> Result<PositiveFiniteSec, AuthoringError> {
    PositiveFiniteSec::new(parse_f64(field, text)?)
        .map_err(|_| AuthoringError::OutOfRange { field })
}

fn parse_mm(field: &'static str, text: &str) -> Result<PositiveFiniteMm, AuthoringError> {
    PositiveFiniteMm::new(parse_f64(field, text)?).map_err(|_| AuthoringError::OutOfRange { field })
}

fn parse_speed(field: &'static str, text: &str) -> Result<PositiveFiniteMmPerMin, AuthoringError> {
    PositiveFiniteMmPerMin::new(parse_f64(field, text)?)
        .map_err(|_| AuthoringError::OutOfRange { field })
}

fn parse_count(field: &'static str, text: &str) -> Result<u32, AuthoringError> {
    text.trim()
        .parse::<u32>()
        .map_err(|_| AuthoringError::NotANumber { field })
}

#[cfg(test)]
mod tests;

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resolved material-side values, ready to drive a print.
//!
//! [`MaterialResolved`] is the output of the resolver's
//! material-side composition: pick a calibrated value, fall back to
//! a material default, derive from intent, or apply a scene
//! override. Every field is required; the resolver errors with
//! [`crate::resolve_error::ResolveError::MissingRequiredField`]
//! when any resolution path fails to supply one.
//!
//! Each field uses the per-unit primitive that carries the
//! positive-finite invariant on the type, matching the section
//! types in [`crate::material_per_printer::MaterialPerPrinter`] and
//! [`crate::material_defaults::MaterialDefaults`].

use sc_print_primitives::{
    PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, ScaleCompensation,
};

use crate::material_identity::MaterialIdentity;

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialResolved {
    pub identity: MaterialIdentity,
    pub layer_height_mm: PositiveFiniteMm,
    pub normal_exposure_sec: PositiveFiniteSec,
    pub bottom_exposure_sec: PositiveFiniteSec,
    pub bottom_layer_count: u32,
    pub lift_distance_mm: PositiveFiniteMm,
    pub lift_speed_mm_min: PositiveFiniteMmPerMin,
    pub retract_speed_mm_min: PositiveFiniteMmPerMin,
    pub scale_compensation_pct: ScaleCompensation,
}

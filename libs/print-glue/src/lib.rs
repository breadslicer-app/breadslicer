/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Application-layer glue between the profile-resolution bounded
//! context and the slicing engine.
//!
//! The resolver in `sc-print-profiles` produces a
//! [`sc_print_profiles::ResolvedPrintConfig`]: what to print, in
//! terms of printer hardware plus resolved material values. The
//! slicing engine in `sc-sl1cer` consumes a
//! [`sc_sl1cer::profile::SliceProfile`]: the geometry and schedule
//! it needs to rasterise. These are two different shapes owned by
//! two different crates, neither of which depends on the other.
//!
//! This crate owns the projection that maps the former onto the
//! latter, combined with the per-print slicing parameters (the
//! anti-aliasing level, the packing mode, the first-layer height,
//! and the layer count derived from the mesh) that are not part of
//! either profile. Both application frontends (the CLI and the
//! eventual GUI) depend on this crate so the mapping lives in one
//! place.

pub mod fingerprint;
pub mod projection;

pub use fingerprint::{
    PersistedSliceValidity, SliceInputFingerprint, SliceInputFingerprintError,
    slice_input_fingerprint, validate_persisted_slices,
};
pub use projection::{
    ProjectionError, project_hardware_to_slice_profile, project_to_slice_profile,
    total_layers_for_height,
};

#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_projection;

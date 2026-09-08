/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

#![deny(missing_docs)]

//! Application services between the profile bounded context and the
//! slicing engine. The 2 crates do not know each other; this one
//! maps between them and hosts the use cases both frontends share.
//!
//! - [`slice_plate`] runs a whole slicing pass: schedule resolution,
//!   the engine run with progress and cancellation, and the merge of
//!   the per-leaf stacks into one plate stack.
//! - [`prepare`] derives the engine's input for a measured scene
//!   from a chosen printer + material.
//! - [`projection`] maps printer hardware and per-print parameters
//!   onto the engine's profile type.
//! - [`print_program`] maps a resolved configuration onto the
//!   printer-file writers' input port.
//! - [`fingerprint`] computes the slice-input token project files
//!   use to revalidate persisted slices.
//! - [`fallback_report`] lists the resolved values no profile
//!   supplied, for a frontend to show before it writes a file.

pub mod fallback_report;
pub mod fingerprint;
pub mod prepare;
pub mod print_program;
pub mod projection;
pub mod slice_plate;

pub use fallback_report::{FallbackKind, FallbackUsed, fallback_report_lines, fallbacks_used};
pub use fingerprint::{
    PersistedSliceValidity, SliceInputFingerprint, SliceInputFingerprintError,
    slice_input_fingerprint, validate_persisted_slices,
};
pub use prepare::{
    UnsliceableScene, UnsliceableSelection, slice_profile_for_scene, slice_profile_for_selection,
};
pub use print_program::{
    SlabTopOutOfRange, UnprojectablePrint, creation_time_now, project_to_print_program,
};
pub use projection::{
    UnschedulableHeight, project_hardware_to_slice_profile, project_to_slice_profile,
    total_layers_for_height,
};
pub use slice_plate::{
    SlicePlateError, SliceRun, SlicedPlate, SlicingParams, schedule_for_selection, slice_plate,
};

#[cfg(test)]
mod tests_fallback_report;
#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_prepare;
#[cfg(test)]
mod tests_print_program;
#[cfg(test)]
mod tests_projection;
#[cfg(test)]
mod tests_slice_plate;

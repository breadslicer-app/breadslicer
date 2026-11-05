/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

#![deny(missing_docs)]

//! Application services: use cases that join crates which do not know each
//! other, such as the print profiles and the slicing engine, or the scene and
//! the build volume.
//!
//! - [`slice_plate`] runs a whole slicing pass: schedule resolution, the engine
//!   run with progress and cancellation, and the merge of the per-leaf stacks
//!   into 1 plate stack.
//! - [`export`] prepares the printer-file export of a slice: the full print
//!   configuration, its checks against the contours' layers, and the machine
//!   program.
//! - [`prepare`] derives the engine's input for a measured scene from a chosen
//!   printer + material.
//! - [`projection`] maps printer hardware and per-print parameters onto the
//!   engine's profile type.
//! - [`print_program`] maps a resolved configuration onto the printer-file
//!   writers' input port.
//! - [`fingerprint`] computes the slice-input token project files use to
//!   revalidate persisted slices.
//! - [`fallback_report`] lists the resolved values no profile supplied, for a
//!   frontend to show before it writes a file.
//! - [`placement`] drops a scene onto the build plate of a build volume.
//! - [`scale_compensation`] scales each model and its supports by the
//!   material's scale compensation before slicing.

pub mod export;
pub mod fallback_report;
pub mod fingerprint;
pub mod placement;
pub mod prepare;
pub mod print_program;
pub mod projection;
pub mod scale_compensation;
pub mod slice_plate;

pub use export::{ExportJob, UnexportableSlice, prepare_export};
pub use fallback_report::{FallbackKind, FallbackUsed, fallback_report_lines, fallbacks_used};
pub use fingerprint::{
    PersistedSliceValidity, SliceInputFingerprint, SliceInputFingerprintError,
    slice_input_fingerprint, validate_persisted_slices,
};
pub use placement::place_on_plate;
pub use prepare::{UnsliceableScene, UnsliceableSelection};
pub use print_program::{SlabTopOutOfRange, UnprojectablePrint};
pub use projection::{
    UnschedulableHeight, project_hardware_to_slice_profile, project_to_slice_profile,
    total_layers_for_height,
};
pub use scale_compensation::{SupportOwners, scale_factors};
pub use slice_plate::{
    MismatchedLayerCount, PlateContours, PlateFrame, SlicePlateError, SliceRun, SlicedPlate,
    SlicingParams, schedule_for_selection, slice_plate,
};

#[cfg(test)]
mod tests_export;
#[cfg(test)]
mod tests_fallback_report;
#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_placement;
#[cfg(test)]
mod tests_prepare;
#[cfg(test)]
mod tests_print_program;
#[cfg(test)]
mod tests_projection;
#[cfg(test)]
mod tests_scale_compensation;
#[cfg(test)]
mod tests_slice_plate;

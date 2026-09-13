/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! CTB at format version 5, unencrypted.
//!
//! The revision that added the resin parameters block to the v4
//! skeleton. Its sections are linked by absolute file offset rather
//! than laid out in a fixed order, so a writer measures the chain
//! before it writes the header that points into it.

pub mod disclaimer;
pub mod header;
pub mod layout;
pub mod print_parameters;
pub mod print_parameters_v4;
pub mod slicer_info;

/// The container carries a single light-off delay and 3 separate
/// waits with nothing to select between them, and a layer program
/// holds the waits. Every delay slot is written zero so that only one
/// of the 2 schemes describes the pause.
pub(crate) const LIGHT_OFF_SEC: f32 = 0.0;

#[cfg(test)]
mod tests_disclaimer;
#[cfg(test)]
mod tests_header;
#[cfg(test)]
mod tests_layout;
#[cfg(test)]
mod tests_print_parameters;
#[cfg(test)]
mod tests_print_parameters_v4;
#[cfg(test)]
mod tests_slicer_info;

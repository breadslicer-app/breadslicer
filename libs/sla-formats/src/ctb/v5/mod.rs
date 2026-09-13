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
pub mod file;
pub mod header;
pub mod layer_record;
pub mod layout;
pub mod preview_record;
pub mod print_parameters;
pub mod print_parameters_v4;
pub mod resin_parameters;
pub mod slicer_info;

#[cfg(test)]
mod tests_disclaimer;
#[cfg(test)]
mod tests_file;
#[cfg(test)]
mod tests_header;
#[cfg(test)]
mod tests_layer_record;
#[cfg(test)]
mod tests_layout;
#[cfg(test)]
mod tests_preview_record;
#[cfg(test)]
mod tests_print_parameters;
#[cfg(test)]
mod tests_print_parameters_v4;
#[cfg(test)]
mod tests_resin_parameters;
#[cfg(test)]
mod tests_slicer_info;

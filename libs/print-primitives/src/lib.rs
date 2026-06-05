/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Shared primitive types for the resin-printing pipeline.
//!
//! This crate hosts the typed values that describe a printer's
//! physical characteristics (build volume, pixel resolution,
//! display mirror flags) along with the measurement primitives
//! that travel with them (validated positive-finite millimetre
//! lengths and their parse errors).
//!
//! It is consumed by both the slicing engine and the print-profile
//! crate so neither has to depend on the other for these shared
//! foundations.

pub mod length;
pub mod mirror;

pub use length::{LengthOutOfRange, ParseLengthError, PositiveFiniteMm};
pub use mirror::Mirror;

#[cfg(test)]
mod tests_length;

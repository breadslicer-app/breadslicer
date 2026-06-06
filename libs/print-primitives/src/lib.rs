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

pub mod build_volume;
pub mod identity;
pub mod length;
pub mod mirror;
pub mod non_empty_string;
pub mod pixel_resolution;
pub mod pixel_size;

pub use build_volume::{AxisOutOfRange, BuildVolume};
pub use identity::{InvalidProfileId, MaterialId, PrinterId};
pub use length::{LengthOutOfRange, ParseLengthError, PositiveFiniteMicrons, PositiveFiniteMm};
pub use mirror::Mirror;
pub use non_empty_string::{EmptyStringError, NonEmptyString};
pub use pixel_resolution::{PixelResolution, ZeroPixelExtentError};
pub use pixel_size::{PixelAxis, PixelAxisOutOfRange, PixelSizeMicrons};

#[cfg(test)]
mod tests_build_volume;
#[cfg(test)]
mod tests_identity;
#[cfg(test)]
mod tests_length;
#[cfg(test)]
mod tests_mirror;
#[cfg(test)]
mod tests_non_empty_string;
#[cfg(test)]
mod tests_pixel_resolution;
#[cfg(test)]
mod tests_pixel_size;

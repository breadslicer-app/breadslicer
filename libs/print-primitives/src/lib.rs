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

pub mod aa_level;
pub mod build_volume;
pub mod energy_density;
pub mod format;
pub mod identity;
pub mod length;
pub mod light_intensity;
pub mod mirror;
pub mod network;
pub mod non_empty_string;
pub mod pixel_resolution;
pub mod pixel_size;
pub mod speed;

pub use aa_level::{AaLevel, UnknownAaLevel};
pub use build_volume::{AxisOutOfRange, BuildVolume};
pub use energy_density::{MjPerCm2OutOfRange, PositiveFiniteMjPerCm2};
pub use format::{FormatKind, OutputFormatBinding, OutputFormatBindings};
pub use identity::{InvalidProfileId, MaterialId, PrinterId};
pub use length::{
    LengthOutOfRange, LengthRange, LengthRangeBoundsInverted, ParseLengthError,
    PositiveFiniteMicrons, PositiveFiniteMm,
};
pub use light_intensity::{MwPerCm2OutOfRange, PositiveFiniteMwPerCm2};
pub use mirror::Mirror;
pub use network::NetworkProtocol;
pub use non_empty_string::{EmptyStringError, NonEmptyString};
pub use pixel_resolution::{PixelResolution, ZeroPixelExtentError};
pub use pixel_size::{PixelAxis, PixelAxisOutOfRange, PixelSizeMicrons};
pub use speed::{
    PositiveFiniteMmPerMin, SpeedMmPerMinOutOfRange, SpeedRangeMmPerMin,
    SpeedRangeMmPerMinBoundsInverted,
};

#[cfg(test)]
mod tests_aa_level;
#[cfg(test)]
mod tests_build_volume;
#[cfg(test)]
mod tests_energy_density;
#[cfg(test)]
mod tests_format;
#[cfg(test)]
mod tests_identity;
#[cfg(test)]
mod tests_length;
#[cfg(test)]
mod tests_light_intensity;
#[cfg(test)]
mod tests_mirror;
#[cfg(test)]
mod tests_network;
#[cfg(test)]
mod tests_non_empty_string;
#[cfg(test)]
mod tests_pixel_resolution;
#[cfg(test)]
mod tests_pixel_size;
#[cfg(test)]
mod tests_speed;

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated pixel size in microns.
//!
//! [`PixelSizeMicrons`] carries the X and Y micron extents of a single
//! display pixel, each validated as strictly positive and finite via
//! the [`PositiveFiniteMicrons`] primitive. [`PixelAxisOutOfRange`]
//! reports which 2D axis came in with an invalid length and chains
//! the underlying [`LengthOutOfRange`] from the per-axis validation.

use std::fmt;

use crate::length::{LengthOutOfRange, PositiveFiniteMicrons};

/// The two axes of a display pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelAxis {
    X,
    Y,
}

impl fmt::Display for PixelAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::X => "x",
            Self::Y => "y",
        })
    }
}

/// Pixel size in microns, validated finite and strictly positive per
/// axis on construction. The typed [`PositiveFiniteMicrons`] fields
/// guarantee the invariant continues to hold under direct field
/// access.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct PixelSizeMicrons {
    pub x: PositiveFiniteMicrons,
    pub y: PositiveFiniteMicrons,
}

/// Returned by [`PixelSizeMicrons::new`] when one of the axes failed
/// the strictly-positive-finite invariant. `source` carries the
/// underlying [`LengthOutOfRange`] from the rejected axis's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelAxisOutOfRange {
    pub axis: PixelAxis,
    pub source: LengthOutOfRange,
}

impl fmt::Display for PixelAxisOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pixel size {} axis must be a finite positive length in microns, got {}",
            self.axis, self.source.value,
        )
    }
}

impl std::error::Error for PixelAxisOutOfRange {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl PixelSizeMicrons {
    /// Construct from per-axis micron values. Fails with
    /// [`PixelAxisOutOfRange`] when an axis is zero, negative, or
    /// non-finite.
    pub fn new(x_um: f64, y_um: f64) -> Result<Self, PixelAxisOutOfRange> {
        let x = PositiveFiniteMicrons::new(x_um).map_err(|source| PixelAxisOutOfRange {
            axis: PixelAxis::X,
            source,
        })?;
        let y = PositiveFiniteMicrons::new(y_um).map_err(|source| PixelAxisOutOfRange {
            axis: PixelAxis::Y,
            source,
        })?;
        Ok(Self { x, y })
    }
}

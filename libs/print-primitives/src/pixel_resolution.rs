/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated raster resolution in pixels per axis.
//!
//! [`PixelResolution`] guarantees a strictly positive pixel count on
//! each of its two axes. [`ZeroPixelExtentError`] reports which axis
//! came in as zero when construction fails.

use std::fmt;

use sc_mesh_core::primitives::CoordAxis;

/// A raster resolution, validated non-zero per axis on construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(
    feature = "serde",
    serde(try_from = "RawPixelResolution", into = "RawPixelResolution")
)]
pub struct PixelResolution {
    x: u32,
    y: u32,
}

#[cfg(feature = "serde")]
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct RawPixelResolution {
    x: u32,
    y: u32,
}

#[cfg(feature = "serde")]
impl TryFrom<RawPixelResolution> for PixelResolution {
    type Error = ZeroPixelExtentError;
    fn try_from(raw: RawPixelResolution) -> Result<Self, Self::Error> {
        Self::new(raw.x, raw.y)
    }
}

#[cfg(feature = "serde")]
impl From<PixelResolution> for RawPixelResolution {
    fn from(res: PixelResolution) -> Self {
        Self { x: res.x, y: res.y }
    }
}

/// Returned by [`PixelResolution::new`] when one of the axes was zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroPixelExtentError {
    pub axis: CoordAxis,
}

impl fmt::Display for ZeroPixelExtentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pixel resolution {} axis must be non-zero", self.axis)
    }
}

impl std::error::Error for ZeroPixelExtentError {}

impl PixelResolution {
    /// Construct from per-axis pixel counts. Fails with
    /// [`ZeroPixelExtentError`] when either axis is zero.
    pub fn new(x: u32, y: u32) -> Result<Self, ZeroPixelExtentError> {
        if x == 0 {
            return Err(ZeroPixelExtentError { axis: CoordAxis::X });
        }
        if y == 0 {
            return Err(ZeroPixelExtentError { axis: CoordAxis::Y });
        }
        Ok(Self { x, y })
    }

    #[inline]
    pub fn x(&self) -> u32 {
        self.x
    }

    #[inline]
    pub fn y(&self) -> u32 {
        self.y
    }
}

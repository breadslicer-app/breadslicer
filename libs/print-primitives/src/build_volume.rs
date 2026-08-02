/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated build volume in millimetres.
//!
//! [`BuildVolume`] guarantees a strictly positive, finite length on
//! each of its three axes. [`AxisOutOfRange`] reports which axis came
//! in with an invalid length and chains the underlying
//! [`LengthOutOfRange`] from the per-axis validation.

use std::fmt;

use sc_mesh_core::primitives::CoordAxis;

use crate::length::{LengthOutOfRange, PositiveFiniteMm};

/// Build volume in millimetres, validated finite and strictly positive
/// per axis on construction. Each axis is stored as its
/// [`PositiveFiniteMm`] witness, so `Eq` is sound (NaN and both
/// zeroes are unrepresentable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(
    feature = "serde",
    serde(try_from = "RawBuildVolume", into = "RawBuildVolume")
)]
pub struct BuildVolume {
    x: PositiveFiniteMm,
    y: PositiveFiniteMm,
    z: PositiveFiniteMm,
}

#[cfg(feature = "serde")]
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct RawBuildVolume {
    x_mm: f64,
    y_mm: f64,
    z_mm: f64,
}

#[cfg(feature = "serde")]
impl TryFrom<RawBuildVolume> for BuildVolume {
    type Error = AxisOutOfRange;
    fn try_from(raw: RawBuildVolume) -> Result<Self, Self::Error> {
        Self::new(raw.x_mm, raw.y_mm, raw.z_mm)
    }
}

#[cfg(feature = "serde")]
impl From<BuildVolume> for RawBuildVolume {
    fn from(bv: BuildVolume) -> Self {
        Self {
            x_mm: bv.x.value(),
            y_mm: bv.y.value(),
            z_mm: bv.z.value(),
        }
    }
}

/// Returned by [`BuildVolume::new`] when one of the axes failed the
/// strictly-positive-finite invariant. `source` carries the underlying
/// [`LengthOutOfRange`] from the rejected axis's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisOutOfRange {
    pub axis: CoordAxis,
    pub source: LengthOutOfRange,
}

impl fmt::Display for AxisOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "build volume {} axis must be a finite positive length in millimetres, got {}",
            self.axis, self.source.value,
        )
    }
}

impl std::error::Error for AxisOutOfRange {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl BuildVolume {
    /// Construct from per-axis lengths in millimetres. Fails with
    /// [`AxisOutOfRange`] when any axis is zero, negative, or
    /// non-finite.
    pub fn new(x_mm: f64, y_mm: f64, z_mm: f64) -> Result<Self, AxisOutOfRange> {
        let x = PositiveFiniteMm::new(x_mm).map_err(|source| AxisOutOfRange {
            axis: CoordAxis::X,
            source,
        })?;
        let y = PositiveFiniteMm::new(y_mm).map_err(|source| AxisOutOfRange {
            axis: CoordAxis::Y,
            source,
        })?;
        let z = PositiveFiniteMm::new(z_mm).map_err(|source| AxisOutOfRange {
            axis: CoordAxis::Z,
            source,
        })?;
        Ok(Self { x, y, z })
    }

    #[inline]
    pub fn x_mm(&self) -> f64 {
        self.x.value()
    }

    #[inline]
    pub fn y_mm(&self) -> f64 {
        self.y.value()
    }

    #[inline]
    pub fn z_mm(&self) -> f64 {
        self.z.value()
    }

    /// Validated witness for the X axis length.
    #[inline]
    pub fn x(&self) -> PositiveFiniteMm {
        self.x
    }

    /// Validated witness for the Y axis length.
    #[inline]
    pub fn y(&self) -> PositiveFiniteMm {
        self.y
    }

    /// Validated witness for the Z axis length.
    #[inline]
    pub fn z(&self) -> PositiveFiniteMm {
        self.z
    }

    /// Minimum corner of the build envelope in printer coordinates:
    /// `[-x_mm/2, -y_mm/2, 0]`. XY is centred on the origin; the
    /// floor of the build plate sits at `z = 0`. Matches the GUI's
    /// [`BuildVolume::envelope_min_mm`] in `apps/slicer-gui`.
    #[inline]
    pub fn envelope_min_mm(&self) -> [f64; 3] {
        [-0.5 * self.x.value(), -0.5 * self.y.value(), 0.0]
    }

    /// Maximum corner of the build envelope in printer coordinates:
    /// `[+x_mm/2, +y_mm/2, z_mm]`. Mirror of [`Self::envelope_min_mm`].
    #[inline]
    pub fn envelope_max_mm(&self) -> [f64; 3] {
        [0.5 * self.x.value(), 0.5 * self.y.value(), self.z.value()]
    }
}

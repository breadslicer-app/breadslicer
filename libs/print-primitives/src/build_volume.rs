/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Validated build volume in millimetres.
//!
//! [`BuildVolume`] guarantees a strictly positive, finite length on each of its
//! 3 axes. [`AxisOutOfRange`] reports which axis came in with an invalid length
//! and chains the underlying [`LengthOutOfRange`] from the per-axis validation.

use std::fmt;

use sc_mesh_core::primitives::CoordAxis;
use sc_mesh_core::{Aabb, Vertex};

use crate::length::{LengthOutOfRange, PositiveFiniteMm};
use crate::wall::{Side, Wall, Walls};

/// Build volume in millimetres, validated finite and strictly positive per axis
/// on construction. Each axis is stored as its [`PositiveFiniteMm`] witness, so
/// `Eq` is sound (NaN and both zeroes are unrepresentable).
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

/// Returned by [`BuildVolume::new`] when an axis failed the
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
    /// Builds a build volume from its lengths in millimetres. Fails with
    /// [`AxisOutOfRange`] for the first axis, in X, Y, Z order, whose length is
    /// 0, negative, or non-finite.
    pub const fn new(x_mm: f64, y_mm: f64, z_mm: f64) -> Result<Self, AxisOutOfRange> {
        // `map_err` and `?` cannot run in a `const fn`.
        let x = match PositiveFiniteMm::new(x_mm) {
            Ok(x) => x,
            Err(source) => {
                return Err(AxisOutOfRange {
                    axis: CoordAxis::X,
                    source,
                });
            }
        };
        let y = match PositiveFiniteMm::new(y_mm) {
            Ok(y) => y,
            Err(source) => {
                return Err(AxisOutOfRange {
                    axis: CoordAxis::Y,
                    source,
                });
            }
        };
        let z = match PositiveFiniteMm::new(z_mm) {
            Ok(z) => z,
            Err(source) => {
                return Err(AxisOutOfRange {
                    axis: CoordAxis::Z,
                    source,
                });
            }
        };
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

    /// Returns the X length as its validated witness.
    #[inline]
    pub fn x(&self) -> PositiveFiniteMm {
        self.x
    }

    /// Returns the Y length as its validated witness.
    #[inline]
    pub fn y(&self) -> PositiveFiniteMm {
        self.y
    }

    /// Returns the Z length as its validated witness.
    #[inline]
    pub fn z(&self) -> PositiveFiniteMm {
        self.z
    }

    /// Returns the minimum corner of the build envelope in printer coordinates:
    /// `[-x_mm/2, -y_mm/2, 0]`. XY is centred on the origin, and the floor of
    /// the build plate is at `z = 0`.
    #[inline]
    pub fn envelope_min_mm(&self) -> [f64; 3] {
        [-0.5 * self.x.value(), -0.5 * self.y.value(), 0.0]
    }

    /// Returns the maximum corner of the build envelope in printer coordinates:
    /// `[x_mm/2, y_mm/2, z_mm]`.
    #[inline]
    pub fn envelope_max_mm(&self) -> [f64; 3] {
        [0.5 * self.x.value(), 0.5 * self.y.value(), self.z.value()]
    }

    /// Returns the build envelope as a box in f32, the precision of mesh
    /// vertices. Its corners come from the lengths rounded to f32, so they can
    /// differ from the f64 corners of [`Self::envelope_min_mm`] and
    /// [`Self::envelope_max_mm`].
    pub fn envelope_aabb(&self) -> Aabb {
        let x = self.x.value() as f32;
        let y = self.y.value() as f32;
        let z = self.z.value() as f32;
        Aabb::from_min_max(
            Vertex::new([-0.5 * x, -0.5 * y, 0.0]),
            Vertex::new([0.5 * x, 0.5 * y, z]),
        )
    }

    /// Returns the walls of the envelope that `aabb` reaches strictly past,
    /// compared in f32 with the corners of [`Self::envelope_aabb`].
    pub fn walls_crossed_by(&self, aabb: &Aabb) -> Walls {
        let envelope = self.envelope_aabb();
        let (env_min, env_max) = (envelope.min().coords(), envelope.max().coords());
        let (min, max) = (aabb.min().coords(), aabb.max().coords());
        CoordAxis::ALL
            .into_iter()
            .flat_map(|axis| {
                let i = axis.as_index();
                [
                    (min[i] < env_min[i]).then_some(Wall {
                        axis,
                        side: Side::Min,
                    }),
                    (max[i] > env_max[i]).then_some(Wall {
                        axis,
                        side: Side::Max,
                    }),
                ]
            })
            .flatten()
            .collect()
    }

    /// Returns whether `aabb` lies inside the envelope, walls included. An
    /// empty box fits.
    pub fn contains(&self, aabb: &Aabb) -> bool {
        aabb.is_empty() || self.walls_crossed_by(aabb).is_empty()
    }
}

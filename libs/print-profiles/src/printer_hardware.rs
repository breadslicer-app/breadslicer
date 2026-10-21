/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Hardware block of a printer profile.
//!
//! [`PrinterHardware`] composes the four physical descriptors of a
//! resin printer: build volume, pixel resolution, per-axis pixel size
//! in microns, and per-axis raster mirror flags. Construction validates
//! that the declared XY build volume agrees with the pixel grid implied
//! by `pixel_resolution * pixel_size_microns` within a 0.5% relative
//! tolerance; failure is reported through [`EnvelopeAxisMismatch`].

use std::fmt;

use sc_print_primitives::{BuildVolume, Mirror, PixelAxis, PixelResolution, PixelSizeMicrons};
use serde::{Deserialize, Serialize};

const ENVELOPE_TOLERANCE: f64 = 0.005;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "RawPrinterHardware", into = "RawPrinterHardware")]
pub struct PrinterHardware {
    build_volume: BuildVolume,
    pixel_resolution: PixelResolution,
    pixel_size_microns: PixelSizeMicrons,
    mirror: Mirror,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawPrinterHardware {
    build_volume: BuildVolume,
    pixel_resolution: PixelResolution,
    pixel_size_microns: PixelSizeMicrons,
    mirror: Mirror,
}

/// Returned by [`PrinterHardware::new`] when the declared XY build
/// volume disagrees with the pixel grid implied by `pixel_resolution`
/// and `pixel_size_microns` by more than the 0.5% relative tolerance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeAxisMismatch {
    pub axis: PixelAxis,
    pub build_volume_mm: f64,
    pub derived_mm: f64,
    pub relative_diff: f64,
}

impl fmt::Display for EnvelopeAxisMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "build volume {} axis ({:.4} mm) disagrees with pixel-grid-derived value ({:.4} mm); relative difference {:.4}% exceeds tolerance {:.4}%",
            self.axis,
            self.build_volume_mm,
            self.derived_mm,
            self.relative_diff * 100.0,
            ENVELOPE_TOLERANCE * 100.0,
        )
    }
}

impl std::error::Error for EnvelopeAxisMismatch {}

impl PrinterHardware {
    /// Construct from validated component primitives. Fails with
    /// [`EnvelopeAxisMismatch`] when the declared XY build volume
    /// disagrees with `pixel_resolution * pixel_size_microns` by more
    /// than the 0.5% relative tolerance on either axis.
    pub fn new(
        build_volume: BuildVolume,
        pixel_resolution: PixelResolution,
        pixel_size_microns: PixelSizeMicrons,
        mirror: Mirror,
    ) -> Result<Self, EnvelopeAxisMismatch> {
        check_axis(
            PixelAxis::X,
            build_volume.x_mm(),
            pixel_resolution.x(),
            pixel_size_microns.x.value(),
        )?;
        check_axis(
            PixelAxis::Y,
            build_volume.y_mm(),
            pixel_resolution.y(),
            pixel_size_microns.y.value(),
        )?;
        Ok(Self {
            build_volume,
            pixel_resolution,
            pixel_size_microns,
            mirror,
        })
    }

    #[inline]
    pub fn build_volume(&self) -> BuildVolume {
        self.build_volume
    }

    #[inline]
    pub fn pixel_resolution(&self) -> PixelResolution {
        self.pixel_resolution
    }

    #[inline]
    pub fn pixel_size_microns(&self) -> PixelSizeMicrons {
        self.pixel_size_microns
    }

    #[inline]
    pub fn mirror(&self) -> Mirror {
        self.mirror
    }
}

fn check_axis(
    axis: PixelAxis,
    build_mm: f64,
    pixel_count: u32,
    pixel_size_um: f64,
) -> Result<(), EnvelopeAxisMismatch> {
    let derived_mm = (pixel_count as f64) * pixel_size_um / 1000.0;
    let relative_diff = (build_mm - derived_mm).abs() / build_mm;
    if relative_diff > ENVELOPE_TOLERANCE {
        Err(EnvelopeAxisMismatch {
            axis,
            build_volume_mm: build_mm,
            derived_mm,
            relative_diff,
        })
    } else {
        Ok(())
    }
}

impl TryFrom<RawPrinterHardware> for PrinterHardware {
    type Error = EnvelopeAxisMismatch;
    fn try_from(raw: RawPrinterHardware) -> Result<Self, Self::Error> {
        Self::new(
            raw.build_volume,
            raw.pixel_resolution,
            raw.pixel_size_microns,
            raw.mirror,
        )
    }
}

impl From<PrinterHardware> for RawPrinterHardware {
    fn from(hw: PrinterHardware) -> Self {
        Self {
            build_volume: hw.build_volume,
            pixel_resolution: hw.pixel_resolution,
            pixel_size_microns: hw.pixel_size_microns,
            mirror: hw.mirror,
        }
    }
}

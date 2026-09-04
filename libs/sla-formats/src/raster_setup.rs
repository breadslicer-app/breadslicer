/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! How a layer's contours become pixels.
//!
//! Every writer that produces an image needs the same five values, and
//! nothing about the machine's print schedule. Holding them together
//! keeps the debug writers and the printer-file writers on one
//! projection rather than two that can drift apart.

use sc_print_primitives::{AaLevel, BuildVolume, Mirror, PixelResolution};
use sc_sl1cer::profile::SliceProfile;

/// The plate geometry and sampling settings a rasteriser needs.
#[derive(Clone, Debug, PartialEq)]
pub struct RasterSetup {
    build_volume: BuildVolume,
    pixel_resolution: PixelResolution,
    mirror: Mirror,
    aa: AaLevel,
    min_alpha_pct: f32,
}

impl RasterSetup {
    /// Take the raster half of an already-validated [`SliceProfile`].
    ///
    /// `min_alpha_pct` starts at `0.0`, disabling the anti-aliasing
    /// exposure floor; [`with_min_alpha_pct`](Self::with_min_alpha_pct)
    /// raises it.
    pub fn from_profile(profile: &SliceProfile) -> Self {
        Self {
            build_volume: *profile.build_volume(),
            pixel_resolution: *profile.pixel_resolution(),
            mirror: profile.mirror(),
            aa: profile.aa_level(),
            min_alpha_pct: 0.0,
        }
    }

    /// Set the anti-aliasing exposure floor in percent.
    #[inline]
    #[must_use]
    pub fn with_min_alpha_pct(mut self, min_alpha_pct: f32) -> Self {
        self.min_alpha_pct = min_alpha_pct;
        self
    }

    /// Report the plate envelope the projection maps onto.
    #[inline]
    pub fn build_volume(&self) -> &BuildVolume {
        &self.build_volume
    }

    /// Report the panel's pixel count.
    #[inline]
    pub fn pixel_resolution(&self) -> &PixelResolution {
        &self.pixel_resolution
    }

    /// Report which axes the projection flips.
    #[inline]
    pub fn mirror(&self) -> Mirror {
        self.mirror
    }

    /// Report the per-pixel anti-aliasing level.
    #[inline]
    pub fn aa(&self) -> AaLevel {
        self.aa
    }

    /// Report the anti-aliasing exposure floor in percent.
    #[inline]
    pub fn min_alpha_pct(&self) -> f32 {
        self.min_alpha_pct
    }
}

#[cfg(test)]
mod raster_setup_tests {
    use sc_sl1cer::profile::{LayerSchedule, PackingMode};

    use super::*;

    fn make_profile() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
            PixelResolution::new(1920, 1080).unwrap(),
            LayerSchedule::uniform(0.05, 0.05, 10).unwrap(),
            Mirror::default(),
            AaLevel::X4,
            PackingMode::None,
        )
    }

    #[test]
    fn from_profile_copies_the_raster_components() {
        let profile = make_profile();
        let raster = RasterSetup::from_profile(&profile);
        assert_eq!(raster.build_volume(), profile.build_volume());
        assert_eq!(raster.pixel_resolution(), profile.pixel_resolution());
        assert_eq!(raster.mirror(), profile.mirror());
        assert_eq!(raster.aa(), profile.aa_level());
    }

    #[test]
    fn from_profile_defaults_min_alpha_pct_to_zero() {
        assert_eq!(
            RasterSetup::from_profile(&make_profile()).min_alpha_pct(),
            0.0
        );
    }

    #[test]
    fn with_min_alpha_pct_overrides_the_default() {
        let raster = RasterSetup::from_profile(&make_profile()).with_min_alpha_pct(2.5);
        assert_eq!(raster.min_alpha_pct(), 2.5);
    }

    #[test]
    fn with_min_alpha_pct_preserves_the_other_components() {
        let profile = make_profile();
        let raster = RasterSetup::from_profile(&profile).with_min_alpha_pct(1.0);
        assert_eq!(raster.build_volume(), profile.build_volume());
        assert_eq!(raster.pixel_resolution(), profile.pixel_resolution());
        assert_eq!(raster.mirror(), profile.mirror());
        assert_eq!(raster.aa(), profile.aa_level());
    }
}

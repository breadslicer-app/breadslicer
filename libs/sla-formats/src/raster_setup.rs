/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! How a layer's contours become pixels.
//!
//! Every writer that produces an image needs the same few values, and
//! nothing about the machine's print schedule. Holding them together
//! keeps the debug writers and the printer-file writers on one
//! projection rather than two that can drift apart.
//!
//! The build volume and pixel resolution describe the plate as it sits
//! in the machine. A printer file stores the plate's image turned when
//! the panel's rows run the other way ([`PanelRotation`]); its headers
//! then name the panel's size, which [`RasterSetup::panel_resolution`]
//! and [`RasterSetup::panel_size_mm`] report.

use sc_print_primitives::{AaLevel, BuildVolume, Mirror, PanelRotation, PixelResolution};
use sc_sl1cer::profile::SliceProfile;

/// The plate geometry and sampling settings a rasteriser needs.
#[derive(Clone, Debug, PartialEq)]
pub struct RasterSetup {
    build_volume: BuildVolume,
    pixel_resolution: PixelResolution,
    mirror: Mirror,
    panel_rotation: PanelRotation,
    aa: AaLevel,
    min_alpha_pct: f32,
}

impl RasterSetup {
    /// Take the raster half of an already-validated [`SliceProfile`].
    ///
    /// `min_alpha_pct` starts at `0.0`, disabling the anti-aliasing
    /// exposure floor; [`with_min_alpha_pct`](Self::with_min_alpha_pct)
    /// raises it. The panel starts unturned;
    /// [`with_panel_rotation`](Self::with_panel_rotation) turns it.
    pub fn from_profile(profile: &SliceProfile) -> Self {
        Self {
            build_volume: *profile.build_volume(),
            pixel_resolution: *profile.pixel_resolution(),
            mirror: profile.mirror(),
            panel_rotation: PanelRotation::None,
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

    /// Sample the pixels at `aa` instead.
    #[inline]
    #[must_use]
    pub fn with_aa(mut self, aa: AaLevel) -> Self {
        self.aa = aa;
        self
    }

    /// Set the turn between the plate's image and the image a printer
    /// file stores.
    #[inline]
    #[must_use]
    pub fn with_panel_rotation(mut self, panel_rotation: PanelRotation) -> Self {
        self.panel_rotation = panel_rotation;
        self
    }

    /// Report the plate envelope the projection maps onto.
    #[inline]
    pub fn build_volume(&self) -> &BuildVolume {
        &self.build_volume
    }

    /// Report the plate's pixel count.
    #[inline]
    pub fn pixel_resolution(&self) -> &PixelResolution {
        &self.pixel_resolution
    }

    /// Report the turn between the plate's image and the stored image.
    #[inline]
    pub fn panel_rotation(&self) -> PanelRotation {
        self.panel_rotation
    }

    /// Report the stored image's pixel count: the plate's, with the axes
    /// swapped when the panel is turned a quarter.
    pub fn panel_resolution(&self) -> PixelResolution {
        let (x, y) = self
            .panel_rotation
            .to_panel((self.pixel_resolution.x(), self.pixel_resolution.y()));
        PixelResolution::new(x, y).expect("swapping the axes keeps both non-zero")
    }

    /// Report the stored image's extent in millimetres, width then
    /// height: the plate's, swapped like
    /// [`panel_resolution`](Self::panel_resolution).
    pub fn panel_size_mm(&self) -> (f64, f64) {
        self.panel_rotation
            .to_panel((self.build_volume.x_mm(), self.build_volume.y_mm()))
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
    fn an_unturned_panel_is_the_plate() {
        let raster = RasterSetup::from_profile(&make_profile());
        assert_eq!(raster.panel_rotation(), PanelRotation::None);
        assert_eq!(raster.panel_resolution(), *raster.pixel_resolution());
        assert_eq!(raster.panel_size_mm(), (60.0, 60.0));
    }

    #[test]
    fn a_quarter_turned_panel_swaps_the_plate_axes() {
        let profile = SliceProfile::new(
            BuildVolume::new(128.0, 81.0, 160.0).unwrap(),
            PixelResolution::new(2560, 1620).unwrap(),
            LayerSchedule::uniform(0.05, 0.05, 10).unwrap(),
            Mirror::default(),
            AaLevel::X4,
            PackingMode::None,
        );
        let raster = RasterSetup::from_profile(&profile).with_panel_rotation(PanelRotation::Ccw90);
        assert_eq!(raster.panel_rotation(), PanelRotation::Ccw90);
        assert_eq!(
            raster.panel_resolution(),
            PixelResolution::new(1620, 2560).unwrap()
        );
        assert_eq!(raster.panel_size_mm(), (81.0, 128.0));
        // The projection still maps onto the plate.
        assert_eq!(raster.pixel_resolution(), profile.pixel_resolution());
        assert_eq!(raster.build_volume(), profile.build_volume());
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

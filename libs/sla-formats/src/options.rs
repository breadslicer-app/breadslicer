/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Parameter bundle handed to every writer.
//!
//! [`WriterOptions`] carries every field any writer in this crate
//! might need. Per-writer relevance varies (SVG writers ignore
//! `aa` and `min_alpha_pct`; PNG writers consume them); each writer
//! reads the subset it actually needs and ignores the rest.

use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PixelResolution, SliceProfile,
};

/// Bundle of slicer-run parameters used to write a contour stack to
/// disk. Built from a [`SliceProfile`] plus the slicer's version
/// string; `min_alpha_pct` defaults to `0.0` and can be set via
/// [`with_min_alpha_pct`](Self::with_min_alpha_pct).
#[derive(Clone, Debug, PartialEq)]
pub struct WriterOptions {
    build_volume: BuildVolume,
    pixel_resolution: PixelResolution,
    mirror: Mirror,
    aa: AaLevel,
    min_alpha_pct: f32,
    layer_schedule: LayerSchedule,
    slicer_version: &'static str,
}

impl WriterOptions {
    /// Assemble [`WriterOptions`] from an already-validated
    /// [`SliceProfile`] plus the slicer's version string.
    ///
    /// `min_alpha_pct` defaults to `0.0` (raster anti-aliasing
    /// exposure floor disabled); callers that want a non-zero floor
    /// chain [`with_min_alpha_pct`](Self::with_min_alpha_pct) after
    /// construction.
    pub fn from_profile(profile: &SliceProfile, slicer_version: &'static str) -> Self {
        Self {
            build_volume: *profile.build_volume(),
            pixel_resolution: *profile.pixel_resolution(),
            mirror: profile.mirror(),
            aa: profile.aa_level(),
            min_alpha_pct: 0.0,
            layer_schedule: *profile.layer_schedule(),
            slicer_version,
        }
    }

    /// Set the raster anti-aliasing exposure floor in percent. Pass
    /// `0.0` to keep the default (disabled).
    #[inline]
    #[must_use]
    pub fn with_min_alpha_pct(mut self, min_alpha_pct: f32) -> Self {
        self.min_alpha_pct = min_alpha_pct;
        self
    }

    #[inline]
    pub fn build_volume(&self) -> &BuildVolume {
        &self.build_volume
    }

    #[inline]
    pub fn pixel_resolution(&self) -> &PixelResolution {
        &self.pixel_resolution
    }

    #[inline]
    pub fn mirror(&self) -> Mirror {
        self.mirror
    }

    #[inline]
    pub fn aa(&self) -> AaLevel {
        self.aa
    }

    #[inline]
    pub fn min_alpha_pct(&self) -> f32 {
        self.min_alpha_pct
    }

    #[inline]
    pub fn layer_schedule(&self) -> &LayerSchedule {
        &self.layer_schedule
    }

    #[inline]
    pub fn slicer_version(&self) -> &'static str {
        self.slicer_version
    }
}

#[cfg(test)]
mod writer_options_tests {
    use sc_sl1cer::profile::PackingMode;

    use super::*;

    fn make_profile() -> SliceProfile {
        let bv = BuildVolume::new(60.0, 60.0, 100.0).unwrap();
        let res = PixelResolution::new(1920, 1080).unwrap();
        let sched = LayerSchedule::uniform(0.05, 0.05, 10).unwrap();
        SliceProfile::new(
            bv,
            res,
            sched,
            Mirror::default(),
            AaLevel::X4,
            PackingMode::None,
        )
    }

    #[test]
    fn from_profile_copies_components() {
        let profile = make_profile();
        let opts = WriterOptions::from_profile(&profile, "0.0.12");
        assert_eq!(opts.build_volume(), profile.build_volume());
        assert_eq!(opts.pixel_resolution(), profile.pixel_resolution());
        assert_eq!(opts.layer_schedule(), profile.layer_schedule());
        assert_eq!(opts.mirror(), profile.mirror());
        assert_eq!(opts.aa(), profile.aa_level());
        assert_eq!(opts.slicer_version(), "0.0.12");
    }

    #[test]
    fn from_profile_defaults_min_alpha_pct_to_zero() {
        let opts = WriterOptions::from_profile(&make_profile(), "0.0.12");
        assert_eq!(opts.min_alpha_pct(), 0.0);
    }

    #[test]
    fn with_min_alpha_pct_overrides_default() {
        let opts = WriterOptions::from_profile(&make_profile(), "0.0.12").with_min_alpha_pct(2.5);
        assert_eq!(opts.min_alpha_pct(), 2.5);
    }

    #[test]
    fn with_min_alpha_pct_preserves_other_fields() {
        let profile = make_profile();
        let opts = WriterOptions::from_profile(&profile, "0.0.12").with_min_alpha_pct(1.0);
        assert_eq!(opts.build_volume(), profile.build_volume());
        assert_eq!(opts.aa(), profile.aa_level());
    }
}

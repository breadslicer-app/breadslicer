/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The stress view in the 3D view (#245): the models painted by how hard
//! the tips holding each height are pulled, and each support by the load on
//! its own tip.
//!
//! One scale for both, the share of what a tip is estimated to hold: green
//! for little, yellow at half, red at all of it and past. A height the plate
//! holds is left plain: nothing hangs there.
//!
//! The models' colour comes from bands of height in the mesh uniforms
//! ([`STRESS_BANDS`] of them, between the lowest and highest point painted),
//! so no mesh is uploaded again. The supports are drawn over themselves,
//! each in one of [`LOAD_LEVELS`] colours.

use std::sync::Arc;

use super::{Overlay, OverlayTint, Renderer};

/// How many bands of height the models' paint has: four to a `vec4` of the
/// uniforms.
pub const STRESS_BANDS: usize = 512;

/// How many colours the supports' paint has (`OverlayTint::Load0` onwards).
pub const LOAD_LEVELS: u32 = 8;

/// The models' paint: from `bottom_mm` to `top_mm`, the share of what the
/// most loaded tip holds on each band of height (0 where the plate holds
/// it, past 1 beyond what it is estimated to hold).
#[derive(Clone, Debug, PartialEq)]
pub struct StressBands {
    pub bottom_mm: f32,
    pub top_mm: f32,
    pub bands: Vec<f32>,
}

impl StressBands {
    /// The bands as the uniforms carry them: [`STRESS_BANDS`] values, the
    /// given ones spread over them, the hardest of those falling in each.
    pub(super) fn packed(&self) -> [[f32; 4]; STRESS_BANDS / 4] {
        let mut out = [[0.0f32; 4]; STRESS_BANDS / 4];
        let n = self.bands.len();
        if n == 0 {
            return out;
        }
        for (k, value) in self.bands.iter().enumerate() {
            let at = (k * STRESS_BANDS / n).min(STRESS_BANDS - 1);
            let to = (((k + 1) * STRESS_BANDS).div_ceil(n)).clamp(at + 1, STRESS_BANDS);
            for band in at..to {
                let slot = &mut out[band / 4][band % 4];
                *slot = slot.max(*value);
            }
        }
        out
    }

    /// Where the bands lie, as the uniforms carry it: their bottom, their
    /// top, and 1 to paint.
    pub(super) fn range(&self) -> [f32; 4] {
        [
            self.bottom_mm,
            self.top_mm.max(self.bottom_mm + 1e-3),
            1.0,
            0.0,
        ]
    }
}

/// The supports' colour for a tip that uses `share` of what it holds.
pub fn load_tint(share: f64) -> OverlayTint {
    let level = (share.clamp(0.0, 1.0) * f64::from(LOAD_LEVELS - 1)).round() as u32;
    OverlayTint::load(level)
}

/// The stress view of the frames to come.
#[derive(Clone, Debug, Default)]
pub struct StressPaint {
    pub bands: Option<Arc<StressBands>>,
    /// Each support drawn over itself in its colour.
    pub supports: Option<Overlay>,
}

impl Renderer {
    /// Paints the models and the supports of the workspace frames to come by
    /// load (#245); `None` turns it off.
    pub fn set_stress(&mut self, stress: Option<&StressPaint>) {
        self.stress_bands = stress.and_then(|s| s.bands.clone());
        let supports = stress.and_then(|s| s.supports.as_ref());
        self.stress_marks.install(
            &self.core.device,
            &self.core.queue,
            &self.pipelines.mesh.models_bind_group_layout,
            supports,
            "stress marks",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bands_spread_over_the_uniforms_keep_their_hardest() {
        let bands = StressBands {
            bottom_mm: 0.0,
            top_mm: 10.0,
            bands: vec![0.0, 0.5, 1.2, 0.1],
        };
        let packed = bands.packed();
        let flat: Vec<f32> = packed.iter().flatten().copied().collect();
        assert_eq!(flat.len(), STRESS_BANDS);
        assert_eq!(flat[0], 0.0);
        assert_eq!(flat[STRESS_BANDS / 4 + 1], 0.5);
        assert_eq!(flat[STRESS_BANDS / 2 + 1], 1.2);
        assert_eq!(flat[STRESS_BANDS - 1], 0.1);
    }

    #[test]
    fn more_bands_than_the_uniforms_hold_keep_every_peak() {
        let mut values = vec![0.0f32; 3000];
        values[1777] = 0.9;
        let bands = StressBands {
            bottom_mm: 0.0,
            top_mm: 150.0,
            bands: values,
        };
        let flat: Vec<f32> = bands.packed().iter().flatten().copied().collect();
        assert_eq!(flat.iter().filter(|&&v| v == 0.9).count(), 1);
        assert_eq!(flat[1777 * STRESS_BANDS / 3000], 0.9);
    }

    #[test]
    fn a_share_picks_its_level() {
        assert_eq!(load_tint(0.0), OverlayTint::Load0);
        assert_eq!(load_tint(0.5), OverlayTint::load(4));
        assert_eq!(load_tint(3.0), OverlayTint::Load7);
    }
}

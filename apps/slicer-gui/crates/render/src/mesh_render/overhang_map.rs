/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The live overhang map (#91): the fragment shader paints each model's faces
//! by the support engine's own overhang test, so a pose shows where supports
//! will go while it is being set. It reads the world face normal of the frame
//! being drawn: turning a model only changes the model matrix every frame
//! writes anyway, with no vertex walked and no buffer uploaded.
//!
//! Faces the engine calls overhangs are red; faces a few degrees from it are
//! rose. A face resting on the plate is left plain. Only the per-face test is
//! mirrored: the engine also supports large steep flats that topple a part,
//! which depends on the size of the patch and not on a single normal.

use sc_geom::df_supports::overhang_normal_z_limit;

/// How close to the plate, mm, a face counts as resting on it: what "on the
/// plate" means for lifting a model before its supports (#389).
const PLATE_CONTACT_MM: f32 = 0.01;

/// The overhang map's setting: the support engine's self-support angle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverhangMap {
    self_support_angle_deg: f32,
}

impl OverhangMap {
    /// A map for `self_support_angle_deg`, degrees from horizontal (0° = flat
    /// ceiling): the number the support scans classify by.
    pub fn new(self_support_angle_deg: f32) -> Self {
        Self {
            self_support_angle_deg,
        }
    }

    /// The angle the map paints by.
    pub fn self_support_angle_deg(self) -> f32 {
        self.self_support_angle_deg
    }

    /// The `overhang` lane of the frame uniforms, for a plate at world height
    /// `plate_z`.
    pub(super) fn uniform(self, plate_z: f32) -> [f32; 4] {
        [
            overhang_normal_z_limit(self.self_support_angle_deg),
            self.self_support_angle_deg.to_radians(),
            plate_z + PLATE_CONTACT_MM,
            0.0,
        ]
    }
}

#[cfg(test)]
mod overhang_map_tests {
    use super::*;

    #[test]
    fn the_uniform_carries_the_engines_limit_and_the_plate_contact() {
        let [limit, radians, contact, _] = OverhangMap::new(52.0).uniform(0.0);
        assert_eq!(limit, overhang_normal_z_limit(52.0));
        assert_eq!(radians, 52f32.to_radians());
        assert_eq!(contact, PLATE_CONTACT_MM);
    }
}

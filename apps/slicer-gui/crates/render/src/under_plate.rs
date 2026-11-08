/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The view from under the plate (#481): as the camera goes below the plate,
//! the plate and the rafts fade out, the way a gizmo handle fades as it turns
//! toward the camera, so the model's underside and the tips on it show. Back
//! above the plate they fade back in. Only the picture changes: the scene, the
//! file and the printed layers do not.

use crate::camera::OrbitCamera;

/// How far below the plate, as a share of the camera's distance to what it
/// orbits, the fade ends: about six degrees past the plate's plane.
pub const UNDER_PLATE_FADE_SPAN: f32 = 0.1;

/// How far the plate and the rafts have faded for `camera`: 0 with the camera
/// above the plate, 1 once it is [`UNDER_PLATE_FADE_SPAN`] under it, eased in
/// between.
pub fn under_plate_fade(camera: &OrbitCamera) -> f32 {
    let distance = camera.distance.max(f32::EPSILON);
    let below = -camera.eye().z / distance;
    let x = (below / UNDER_PLATE_FADE_SPAN).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Whether the plate and the rafts are hidden enough, for `camera`, that a
/// click goes through them: from past half the fade.
pub fn sees_through_the_plate(camera: &OrbitCamera) -> bool {
    under_plate_fade(camera) >= 0.5
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    fn camera_at(pitch_deg: f32, target_z: f32) -> OrbitCamera {
        let target = Vec3::new(0.0, 0.0, target_z);
        let mut camera = OrbitCamera::new(target, 100.0);
        camera.target = target;
        camera.distance = 100.0;
        camera.pitch = pitch_deg.to_radians();
        camera
    }

    #[test]
    fn above_the_plate_nothing_fades() {
        for pitch in [80.0, 30.0, 0.0, -5.0] {
            // Looking a little up at a target 20 mm high, the eye is still
            // above the plate.
            assert_eq!(under_plate_fade(&camera_at(pitch, 20.0)), 0.0, "{pitch}");
        }
    }

    #[test]
    fn under_the_plate_everything_fades_past_the_span() {
        assert_eq!(under_plate_fade(&camera_at(-45.0, 20.0)), 1.0);
        assert_eq!(under_plate_fade(&camera_at(-89.0, 0.0)), 1.0);
        assert!(sees_through_the_plate(&camera_at(-45.0, 20.0)));
    }

    #[test]
    fn the_fade_grows_smoothly_as_the_eye_sinks() {
        // The eye crosses the plate at sin(pitch) = -0.2 for a target 20 mm
        // high 100 mm away; the fade ends 0.1 further.
        let fades: Vec<f32> = (0..=20)
            .map(|i| {
                let sin = -0.18 - 0.01 * i as f32;
                under_plate_fade(&camera_at(sin.asin().to_degrees(), 20.0))
            })
            .collect();
        assert!(fades.windows(2).all(|w| w[0] <= w[1]), "{fades:?}");
        assert_eq!(fades[0], 0.0);
        assert_eq!(*fades.last().unwrap(), 1.0);
        assert!(fades.iter().any(|f| *f > 0.1 && *f < 0.9), "{fades:?}");
    }
}

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
///
/// A perspective camera sees the plate from under it once its eye is below
/// it. An orthographic one looks along parallel rays and has no eye to speak
/// of: its distance only sets the zoom, and zoomed in, the eye stayed above the
/// plate, or just under it, half faded, while the rays climbed steeply, which
/// left a raft's pixels scattered on screen. It sees the plate from under it
/// once its rays climb.
pub fn under_plate_fade(camera: &OrbitCamera) -> f32 {
    let below = if camera.orthographic {
        -camera.pitch.sin()
    } else {
        -camera.eye().z / camera.distance.max(f32::EPSILON)
    };
    let x = (below / UNDER_PLATE_FADE_SPAN).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// How far over the rafts' top the view from under the plate cuts a support
/// mesh, mm. A raft's wall tops out at that height to the last bit: cut right
/// there, its top face was kept in some pixels and dropped in others, a dotted
/// rim along every raft.
pub const RAFT_CUT_MARGIN_MM: f32 = 0.05;

/// The height below which the view from under the plate hides a support mesh
/// whose rafts reach `raft_top_mm`; 0 without a raft.
pub fn hidden_below_mm(raft_top_mm: f32) -> f32 {
    if raft_top_mm > 0.0 {
        raft_top_mm + RAFT_CUT_MARGIN_MM
    } else {
        0.0
    }
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

    /// Zoomed in, an orthographic camera's eye stands close to what it
    /// orbits: looking up steeply it can still be above the plate. Its rays
    /// decide, not its eye.
    #[test]
    fn an_orthographic_camera_fades_by_how_steeply_its_rays_climb() {
        let mut camera = camera_at(-40.0, 10.0);
        camera.distance = 12.0;
        assert!(camera.eye().z > 0.0, "the eye above the plate");
        assert_eq!(under_plate_fade(&camera), 0.0, "in perspective, from above");
        camera.orthographic = true;
        assert_eq!(under_plate_fade(&camera), 1.0);
        for (pitch, fade) in [(30.0, 0.0), (0.0, 0.0), (-89.0, 1.0)] {
            camera.pitch = f32::to_radians(pitch);
            assert_eq!(under_plate_fade(&camera), fade, "{pitch}");
        }
        // Past the span below the horizon the fade is whole.
        camera.pitch = (-UNDER_PLATE_FADE_SPAN).asin() - 0.01;
        assert_eq!(under_plate_fade(&camera), 1.0);
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

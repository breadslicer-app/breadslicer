/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Keyboard-driven gizmo integrator: data types (`KbdFocus`,
//! `ArrowDrive`) plus the per-tick pure function `keyboard_step` that
//! turns a held-arrow bitset into a transform delta. The
//! `ViewportController` owns the `Cell`s that the Slint callbacks
//! read and write; this module hosts only the controller-agnostic
//! pieces.

use glam::Vec3;
use sc_mesh_core::scene::Transform3D;
use sc_mesh_core::transform::AffineTransform3D;

use super::gizmo_drag::{axis_rotation, compose_about_pivot};
use crate::gizmo::axis::Axis;

/// Keyboard gizmo focus. `T`/`R`/`S` keypresses cycle this through
/// the three operation types; Esc returns to `None`. While focus is
/// non-`None`, the arrow / PageUp / PageDown keys are hijacked by
/// the per-frame integrator and no longer drive the camera.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub(super) enum KbdFocus {
    #[default]
    None,
    Translate,
    Rotate,
    Scale,
}

/// Bitset of held arrow-driving keys, with the per-axis sign:
/// `Right`/`Left` = ±X, `Up`/`Down` = ±Y, `PageUp`/`PageDown` = ±Z.
/// Driven by `key-pressed` / `key-released` from the FocusScope and
/// sampled every animation tick to integrate continuous motion.
#[derive(Copy, Clone, Default, Debug)]
pub(super) struct ArrowDrive {
    pub x_pos: bool,
    pub x_neg: bool,
    pub y_pos: bool,
    pub y_neg: bool,
    pub z_pos: bool,
    pub z_neg: bool,
}

impl ArrowDrive {
    /// Per-axis sign as `(±1, ±1, ±1)`, with `0` for any axis whose
    /// keys are both unpressed (or both pressed at once, in which
    /// case they cancel).
    pub(super) fn signs(self) -> (f32, f32, f32) {
        let x = i32::from(self.x_pos) - i32::from(self.x_neg);
        let y = i32::from(self.y_pos) - i32::from(self.y_neg);
        let z = i32::from(self.z_pos) - i32::from(self.z_neg);
        (x as f32, y as f32, z as f32)
    }

    pub(super) fn any_held(self) -> bool {
        self.x_pos || self.x_neg || self.y_pos || self.y_neg || self.z_pos || self.z_neg
    }
}

/// Codes carried on `gizmo-arrow-pressed` / `gizmo-arrow-released`.
/// `0/1` = ±X, `2/3` = ±Y, `4/5` = ±Z. The Slint layer uses bare
/// `int`s because Slint's typed enums can't be passed through a
/// callback that's bound at runtime; the Rust layer normalises into
/// the `ArrowDrive` bitset above.
pub(super) const ARROW_X_POS: i32 = 0;
pub(super) const ARROW_X_NEG: i32 = 1;
pub(super) const ARROW_Y_POS: i32 = 2;
pub(super) const ARROW_Y_NEG: i32 = 3;
pub(super) const ARROW_Z_POS: i32 = 4;
pub(super) const ARROW_Z_NEG: i32 = 5;

/// Translation rate for keyboard-focus drive: 30 logical pixels of
/// screen-projected motion per second along the chosen world axis.
/// Converted to world units per frame via the camera's
/// `world_per_pixel` ratio.
const KBD_TRANSLATE_PIXELS_PER_SEC: f32 = 30.0;

/// Rotation rate for keyboard-focus drive: 45° per second about the
/// chosen world axis.
const KBD_ROTATE_RAD_PER_SEC: f32 = std::f32::consts::FRAC_PI_4;

/// Scale-rate base for keyboard-focus drive: factor `1.5` per second
/// of held key. Combined with per-axis sign, the effective per-axis
/// factor each tick is `1.5 ^ (sign * dt)`.
const KBD_SCALE_BASE: f32 = 1.5;

/// Maximum integrator step. Caps the per-frame delta so a stalled
/// frame (debugger pause, garbage collection, anything that delays
/// the next render past ~100 ms) doesn't translate the object across
/// the viewport in a single jump when the loop resumes. Applied at
/// the controller's call site, before `keyboard_step` is invoked.
pub(super) const KBD_MAX_DT: f32 = 0.1;

/// Pure helper: produces one tick of the keyboard-focus integrator.
/// Returns `None` when there's nothing to integrate (no axes held,
/// or the focus is `None`); otherwise returns the new running
/// transform composed onto the previous frame's `running` transform.
///
/// `dt` is the per-tick delta in seconds; callers clamp it to
/// `KBD_MAX_DT` and gate against non-positive values before calling.
///
/// Translate steps compose as `T(delta) ∘ running`. Rotate steps
/// compose as `T(p) ∘ R_z ∘ R_y ∘ R_x ∘ T(-p) ∘ running` — a fixed
/// canonical order across held axes; for the small per-tick angles
/// this matters only by a second-order amount. Scale steps compose
/// as `T(p) ∘ diag(s) ∘ T(-p) ∘ running`, with off-axes held at
/// `1.0`.
pub(super) fn keyboard_step(
    focus: KbdFocus,
    arrows: ArrowDrive,
    dt: f32,
    pivot: Vec3,
    running: &Transform3D,
    world_per_pixel: f32,
) -> Option<Transform3D> {
    let (sx, sy, sz) = arrows.signs();
    if sx == 0.0 && sy == 0.0 && sz == 0.0 {
        return None;
    }
    match focus {
        KbdFocus::None => None,
        KbdFocus::Translate => {
            let speed_world = world_per_pixel * KBD_TRANSLATE_PIXELS_PER_SEC * dt;
            let delta = Vec3::new(sx, sy, sz) * speed_world;
            let translation = AffineTransform3D::from_translation([
                delta.x as f64,
                delta.y as f64,
                delta.z as f64,
            ]);
            Some(Transform3D::Affine(translation).compose(running))
        }
        KbdFocus::Rotate => {
            let mut t = *running;
            for (sign, axis) in [(sx, Axis::X), (sy, Axis::Y), (sz, Axis::Z)] {
                if sign == 0.0 {
                    continue;
                }
                let angle = sign * KBD_ROTATE_RAD_PER_SEC * dt;
                if let Some(r) = axis_rotation(axis, angle) {
                    t = compose_about_pivot(&r, pivot, &t);
                }
            }
            Some(t)
        }
        KbdFocus::Scale => {
            let pow = |sign: f32| {
                if sign == 0.0 {
                    1.0
                } else {
                    KBD_SCALE_BASE.powf(sign * dt)
                }
            };
            let scale_mat = AffineTransform3D::from_non_uniform_scale([
                pow(sx) as f64,
                pow(sy) as f64,
                pow(sz) as f64,
            ]);
            Some(compose_about_pivot(&scale_mat, pivot, running))
        }
    }
}

#[cfg(test)]
mod keyboard_step_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn arrow_drive_signs_compose_per_axis_kbd() {
        // +X and −X cancel; +Y alone keeps a sign; mixed bits across
        // axes drive each axis independently.
        let d = ArrowDrive {
            x_pos: true,
            x_neg: true,
            ..ArrowDrive::default()
        };
        let (sx, sy, sz) = d.signs();
        assert_eq!((sx, sy, sz), (0.0, 0.0, 0.0));

        let d = ArrowDrive {
            y_pos: true,
            ..ArrowDrive::default()
        };
        let (sx, sy, sz) = d.signs();
        assert_eq!((sx, sy, sz), (0.0, 1.0, 0.0));

        let d = ArrowDrive {
            x_neg: true,
            z_pos: true,
            ..ArrowDrive::default()
        };
        let (sx, sy, sz) = d.signs();
        assert_eq!((sx, sy, sz), (-1.0, 0.0, 1.0));
    }

    #[test]
    fn keyboard_step_translate_produces_pure_translation() {
        // 1 second of held +X at 30 px/sec, world_per_pixel = 0.1
        // → world delta = 30 * 0.1 = 3.0 along +X. Off-axis stays 0.
        let arrows = ArrowDrive {
            x_pos: true,
            ..ArrowDrive::default()
        };
        let result = keyboard_step(
            KbdFocus::Translate,
            arrows,
            1.0,
            Vec3::ZERO,
            &Transform3D::Identity,
            0.1,
        )
        .unwrap();
        match result {
            Transform3D::Affine(a) => {
                assert_approx_eq!(f64, a.translation()[0], 3.0, epsilon = 1e-6);
                assert_approx_eq!(f64, a.translation()[1], 0.0, epsilon = 1e-6);
                assert_approx_eq!(f64, a.translation()[2], 0.0, epsilon = 1e-6);
                // Linear part stays identity.
                assert_approx_eq!(f64, a.linear()[0][0], 1.0, epsilon = 1e-6);
            }
            other => panic!("expected Affine, got {other:?}"),
        }
    }

    #[test]
    fn keyboard_step_no_keys_returns_none() {
        let result = keyboard_step(
            KbdFocus::Translate,
            ArrowDrive::default(),
            1.0,
            Vec3::ZERO,
            &Transform3D::Identity,
            1.0,
        );
        assert!(result.is_none());
    }

    #[test]
    fn keyboard_step_focus_none_returns_none() {
        let arrows = ArrowDrive {
            x_pos: true,
            ..ArrowDrive::default()
        };
        let result = keyboard_step(
            KbdFocus::None,
            arrows,
            1.0,
            Vec3::ZERO,
            &Transform3D::Identity,
            1.0,
        );
        assert!(result.is_none());
    }

    #[test]
    fn keyboard_step_rotate_about_x_takes_y_into_z() {
        // 1 s at 45°/s about +X = π/4 rotation. R_x(π/4) maps
        // (0, 1, 0) → (0, cos π/4, sin π/4).
        let arrows = ArrowDrive {
            x_pos: true,
            ..ArrowDrive::default()
        };
        let result = keyboard_step(
            KbdFocus::Rotate,
            arrows,
            1.0,
            Vec3::ZERO,
            &Transform3D::Identity,
            0.0, // world_per_pixel unused for rotate
        )
        .unwrap();
        match result {
            Transform3D::Affine(a) => {
                let cos = (std::f64::consts::PI * 0.25).cos();
                let sin = (std::f64::consts::PI * 0.25).sin();
                // Column 1 of the linear part is the image of `+Y`.
                assert_approx_eq!(f64, a.linear()[0][1], 0.0, epsilon = 1e-6);
                assert_approx_eq!(f64, a.linear()[1][1], cos, epsilon = 1e-6);
                assert_approx_eq!(f64, a.linear()[2][1], sin, epsilon = 1e-6);
            }
            other => panic!("expected Affine, got {other:?}"),
        }
    }

    #[test]
    fn keyboard_step_scale_factor_15_per_second_along_x() {
        // 1 s of held +X scale → factor 1.5 on X, 1.0 on Y/Z.
        let arrows = ArrowDrive {
            x_pos: true,
            ..ArrowDrive::default()
        };
        let result = keyboard_step(
            KbdFocus::Scale,
            arrows,
            1.0,
            Vec3::ZERO,
            &Transform3D::Identity,
            0.0,
        )
        .unwrap();
        match result {
            Transform3D::Affine(a) => {
                assert_approx_eq!(f64, a.linear()[0][0], 1.5, epsilon = 1e-5);
                assert_approx_eq!(f64, a.linear()[1][1], 1.0, epsilon = 1e-6);
                assert_approx_eq!(f64, a.linear()[2][2], 1.0, epsilon = 1e-6);
            }
            other => panic!("expected Affine, got {other:?}"),
        }
    }

    #[test]
    fn keyboard_step_scale_negative_sign_inverts_factor() {
        // Held −X for 1 s → factor 1/1.5 along X.
        let arrows = ArrowDrive {
            x_neg: true,
            ..ArrowDrive::default()
        };
        let result = keyboard_step(
            KbdFocus::Scale,
            arrows,
            1.0,
            Vec3::ZERO,
            &Transform3D::Identity,
            0.0,
        )
        .unwrap();
        match result {
            Transform3D::Affine(a) => {
                assert_approx_eq!(f64, a.linear()[0][0], 1.0 / 1.5, epsilon = 1e-5);
            }
            other => panic!("expected Affine, got {other:?}"),
        }
    }
}

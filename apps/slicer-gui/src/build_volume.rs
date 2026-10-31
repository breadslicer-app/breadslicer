/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The build volume as the viewport draws and edits against it, and the camera
//! framing that keeps both the volume and the scene in view.

use glam::Vec3;

/// The build volume as the viewport uses it. It holds a validated
/// [`sc_print_primitives::BuildVolume`] and reads its lengths and corners in
/// f32, the precision of mesh vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildVolume {
    volume: sc_print_primitives::BuildVolume,
}

impl From<sc_print_primitives::BuildVolume> for BuildVolume {
    fn from(volume: sc_print_primitives::BuildVolume) -> Self {
        Self { volume }
    }
}

impl BuildVolume {
    /// Builds a volume from its lengths in millimetres. Panics when a length is
    /// 0, negative, or non-finite.
    #[cfg(test)]
    pub(crate) fn new(x_mm: f64, y_mm: f64, z_mm: f64) -> Self {
        Self::from(
            sc_print_primitives::BuildVolume::new(x_mm, y_mm, z_mm)
                .expect("test build volumes have positive finite lengths"),
        )
    }

    /// Returns the wrapped library value, which owns the wall test.
    pub fn volume(&self) -> sc_print_primitives::BuildVolume {
        self.volume
    }

    /// Returns the X length in millimetres, rounded to f32.
    pub fn width_mm(&self) -> f32 {
        self.volume.x_mm() as f32
    }

    /// Returns the Y length in millimetres, rounded to f32.
    pub fn depth_mm(&self) -> f32 {
        self.volume.y_mm() as f32
    }

    /// Returns the Z length in millimetres, rounded to f32.
    pub fn height_mm(&self) -> f32 {
        self.volume.z_mm() as f32
    }

    /// Returns the minimum corner of
    /// [`sc_print_primitives::BuildVolume::envelope_aabb`].
    ///
    /// For "is geometry inside the envelope" checks, prefer the wall test of
    /// [`Self::volume`] over comparing against the raw corner.
    pub fn envelope_min_mm(&self) -> Vec3 {
        Vec3::from(self.volume.envelope_aabb().min().coords())
    }

    /// Returns the maximum corner of
    /// [`sc_print_primitives::BuildVolume::envelope_aabb`].
    ///
    /// For "is geometry inside the envelope" checks, prefer the wall test of
    /// [`Self::volume`] over comparing against the raw corner.
    pub fn envelope_max_mm(&self) -> Vec3 {
        Vec3::from(self.volume.envelope_aabb().max().coords())
    }

    /// Returns the orbit-camera framing for the union of an optional scene AABB
    /// and the build envelope: the centre and the bounding-sphere radius
    /// (clamped to `1e-3` mm to keep `OrbitCamera::new` from producing a
    /// distance of 0 for degenerate inputs).
    ///
    /// `scene_aabb` is `Option<(min, max)>` to match
    /// `Renderer::scene_bounds()`; `None` frames on the envelope alone.
    /// Component-wise `min` / `max` against the envelope corners yields the
    /// union, so a degenerate scene AABB (sentinel `(+inf, -inf)`) collapses
    /// safely to envelope-only.
    pub fn framing(&self, scene_aabb: Option<(Vec3, Vec3)>) -> (Vec3, f32) {
        let env_min = self.envelope_min_mm();
        let env_max = self.envelope_max_mm();
        let (min, max) = match scene_aabb {
            Some((sm, sx)) => (sm.min(env_min), sx.max(env_max)),
            None => (env_min, env_max),
        };
        let centre = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(1e-3);
        (centre, radius)
    }
}

/// The build volume that the viewport uses when it knows no printer.
pub const NO_PRINTER_BUILD_VOLUME: BuildVolume =
    match sc_print_primitives::BuildVolume::new(192.0, 120.0, 200.0) {
        Ok(volume) => BuildVolume { volume },
        Err(_) => panic!("the placeholder lengths are positive and finite"),
    };

#[cfg(test)]
mod build_volume_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn vol() -> BuildVolume {
        // 100 x 60 x 80 mm: small, asymmetric, easy to reason about.
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    #[test]
    fn envelope_corners_are_centred_on_xy_with_z_floor() {
        let v = vol();
        let lo = v.envelope_min_mm();
        let hi = v.envelope_max_mm();
        assert_approx_eq!(f32, lo.x, -50.0);
        assert_approx_eq!(f32, lo.y, -30.0);
        assert_approx_eq!(f32, lo.z, 0.0);
        assert_approx_eq!(f32, hi.x, 50.0);
        assert_approx_eq!(f32, hi.y, 30.0);
        assert_approx_eq!(f32, hi.z, 80.0);
    }

    #[test]
    fn lengths_and_corners_are_read_in_f32() {
        // 143.43 and 89.6 are not exact in f32: the lengths are their f32
        // roundings, and the corners are the halves of those roundings.
        let v = BuildVolume::from(
            sc_print_primitives::BuildVolume::new(143.43, 89.6, 175.0).expect("valid build volume"),
        );
        assert_eq!(v.width_mm(), 143.43);
        assert_eq!(v.depth_mm(), 89.6);
        assert_eq!(v.height_mm(), 175.0);
        assert_eq!(v.envelope_min_mm(), Vec3::new(-71.715, -44.8, 0.0));
        assert_eq!(v.envelope_max_mm(), Vec3::new(71.715, 44.8, 175.0));
    }

    #[test]
    fn volume_returns_the_wrapped_library_value() {
        let library =
            sc_print_primitives::BuildVolume::new(143.43, 89.6, 175.0).expect("valid build volume");
        assert_eq!(BuildVolume::from(library).volume(), library);
    }

    #[test]
    fn no_printer_envelope_is_finite_and_positive() {
        let v = NO_PRINTER_BUILD_VOLUME;
        let lo = v.envelope_min_mm();
        let hi = v.envelope_max_mm();
        assert!(lo.x.is_finite() && lo.y.is_finite() && lo.z.is_finite());
        assert!(hi.x > lo.x && hi.y > lo.y && hi.z > lo.z);
    }

    #[test]
    fn placeholder_is_192_by_120_by_200_mm() {
        let v = NO_PRINTER_BUILD_VOLUME;
        assert_eq!(v.width_mm(), 192.0);
        assert_eq!(v.depth_mm(), 120.0);
        assert_eq!(v.height_mm(), 200.0);
        assert_eq!(v.envelope_min_mm(), Vec3::new(-96.0, -60.0, 0.0));
        assert_eq!(v.envelope_max_mm(), Vec3::new(96.0, 60.0, 200.0));
    }

    #[test]
    fn framing_with_no_scene_returns_plate_geometry() {
        // Envelope 100 x 60 x 80: corners at (-50, -30, 0) and (50, 30, 80).
        // Centre at (0, 0, 40), bounding-sphere radius is half the diagonal =
        // sqrt(100^2 + 60^2 + 80^2) / 2.
        let v = vol();
        let (centre, radius) = v.framing(None);
        let expected_radius =
            (100.0_f32.powi(2) + 60.0_f32.powi(2) + 80.0_f32.powi(2)).sqrt() * 0.5;
        assert_approx_eq!(f32, centre.x, 0.0);
        assert_approx_eq!(f32, centre.y, 0.0);
        assert_approx_eq!(f32, centre.z, 40.0);
        assert_approx_eq!(f32, radius, expected_radius, epsilon = 1e-3);
    }

    #[test]
    fn framing_with_scene_inside_envelope_is_dominated_by_plate() {
        // A small scene AABB strictly inside the envelope must not shift the
        // framing: the union is the envelope itself.
        let v = vol();
        let scene = (Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 2.0));
        let (centre_with, radius_with) = v.framing(Some(scene));
        let (centre_alone, radius_alone) = v.framing(None);
        assert_approx_eq!(f32, centre_with.x, centre_alone.x);
        assert_approx_eq!(f32, centre_with.y, centre_alone.y);
        assert_approx_eq!(f32, centre_with.z, centre_alone.z);
        assert_approx_eq!(f32, radius_with, radius_alone);
    }

    #[test]
    fn framing_with_scene_outside_envelope_extends_bounds() {
        // Scene AABB poking out past +X / +Y. The union takes the envelope on
        // the negative side and the scene on the positive side.
        let v = vol();
        let scene = (Vec3::new(100.0, 100.0, 50.0), Vec3::new(110.0, 110.0, 60.0));
        let (centre, radius) = v.framing(Some(scene));
        // Union: (-50, -30, 0) to (110, 110, 80). Centre = (30, 40, 40).
        // Diagonal = (160, 140, 80); half-length is the radius.
        let expected_radius =
            (160.0_f32.powi(2) + 140.0_f32.powi(2) + 80.0_f32.powi(2)).sqrt() * 0.5;
        assert_approx_eq!(f32, centre.x, 30.0);
        assert_approx_eq!(f32, centre.y, 40.0);
        assert_approx_eq!(f32, centre.z, 40.0);
        assert_approx_eq!(f32, radius, expected_radius, epsilon = 1e-3);
    }
}

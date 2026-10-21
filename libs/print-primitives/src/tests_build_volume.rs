/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use float_cmp::assert_approx_eq;
use sc_mesh_core::primitives::CoordAxis;
use sc_mesh_core::{Aabb, Vector3D, Vertex};

use super::build_volume::{AxisOutOfRange, BuildVolume};
use super::wall::{Side, Wall, Walls};

fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
    Aabb::from_min_max(Vertex::new(min), Vertex::new(max))
}

/// Returns a volume of 100 x 60 x 80 mm, whose envelope runs from [-50,-30,0]
/// to [50,30,80].
fn volume() -> BuildVolume {
    BuildVolume::new(100.0, 60.0, 80.0).unwrap()
}

fn walls(crossed: &[(CoordAxis, Side)]) -> Walls {
    crossed
        .iter()
        .map(|&(axis, side)| Wall { axis, side })
        .collect()
}

#[test]
fn build_volume_new_accepts_positive_finite() {
    let bv = BuildVolume::new(60.0, 60.0, 100.0).unwrap();
    assert_eq!(bv.x_mm(), 60.0);
    assert_eq!(bv.y_mm(), 60.0);
    assert_eq!(bv.z_mm(), 100.0);
}

#[test]
fn build_volume_is_eq() {
    // Sound since the axes are stored as PositiveFiniteMm witnesses (NaN and
    // both zeroes are unrepresentable); guards against the derive being
    // dropped.
    fn assert_eq_bound<T: Eq>() {}
    assert_eq_bound::<BuildVolume>();
}

#[test]
fn build_volume_witness_accessors_match_the_raw_accessors() {
    let bv = BuildVolume::new(60.0, 40.0, 100.0).unwrap();
    assert_eq!(bv.x().value(), bv.x_mm());
    assert_eq!(bv.y().value(), bv.y_mm());
    assert_eq!(bv.z().value(), bv.z_mm());
}

#[test]
fn build_volume_new_rejects_zero_x_axis() {
    let err = BuildVolume::new(0.0, 60.0, 100.0).unwrap_err();
    assert_eq!(err.axis, CoordAxis::X);
    assert_eq!(err.source.value, 0.0);
}

#[test]
fn build_volume_new_rejects_negative_y_axis() {
    let err = BuildVolume::new(60.0, -1.0, 100.0).unwrap_err();
    assert_eq!(err.axis, CoordAxis::Y);
    assert_eq!(err.source.value, -1.0);
}

#[test]
fn build_volume_new_rejects_nan_z_axis() {
    let err = BuildVolume::new(60.0, 60.0, f64::NAN).unwrap_err();
    assert_eq!(err.axis, CoordAxis::Z);
    assert!(err.source.value.is_nan());
}

#[test]
fn build_volume_new_rejects_infinite_x_axis() {
    let err = BuildVolume::new(f64::INFINITY, 60.0, 100.0).unwrap_err();
    assert_eq!(err.axis, CoordAxis::X);
    assert!(err.source.value.is_infinite());
}

#[test]
fn build_volume_envelope_centres_xy_on_origin_and_floors_z_at_zero() {
    let bv = BuildVolume::new(192.0, 120.0, 200.0).unwrap();
    assert_eq!(bv.envelope_min_mm(), [-96.0, -60.0, 0.0]);
    assert_eq!(bv.envelope_max_mm(), [96.0, 60.0, 200.0]);
}

#[test]
fn build_volume_envelope_extent_matches_per_axis_dimensions() {
    let bv = BuildVolume::new(60.0, 40.0, 100.0).unwrap();
    let min = bv.envelope_min_mm();
    let max = bv.envelope_max_mm();
    assert!((max[0] - min[0] - bv.x_mm()).abs() < 1e-12);
    assert!((max[1] - min[1] - bv.y_mm()).abs() < 1e-12);
    assert!((max[2] - min[2] - bv.z_mm()).abs() < 1e-12);
}

#[test]
fn build_volume_new_runs_in_a_const() {
    const ACCEPTED: BuildVolume = match BuildVolume::new(192.0, 120.0, 200.0) {
        Ok(volume) => volume,
        Err(_) => panic!("192 x 120 x 200 mm is a valid build volume"),
    };
    const REFUSED: Result<BuildVolume, AxisOutOfRange> = BuildVolume::new(192.0, 0.0, 200.0);
    assert_eq!(ACCEPTED.x_mm(), 192.0);
    assert_eq!(ACCEPTED.y_mm(), 120.0);
    assert_eq!(ACCEPTED.z_mm(), 200.0);
    assert_eq!(REFUSED.unwrap_err().axis, CoordAxis::Y);
}

#[test]
fn envelope_aabb_spans_the_envelope_corners() {
    let aabb = BuildVolume::new(192.0, 120.0, 200.0)
        .unwrap()
        .envelope_aabb();
    assert_eq!(aabb.min().coords(), [-96.0, -60.0, 0.0]);
    assert_eq!(aabb.max().coords(), [96.0, 60.0, 200.0]);
}

#[test]
fn envelope_aabb_halves_the_lengths_rounded_to_f32() {
    // 143.43 and 89.6 are not exact in f32. The corners are the halves of the
    // f32 lengths, 71.71499633789062 and 44.79999923706055: the f32 values
    // nearest to 71.715 and 44.8.
    let aabb = BuildVolume::new(143.43, 89.6, 175.0)
        .unwrap()
        .envelope_aabb();
    assert_eq!(aabb.min().coords(), [-71.715, -44.8, 0.0]);
    assert_eq!(aabb.max().coords(), [71.715, 44.8, 175.0]);
}

#[test]
fn envelope_aabb_floor_is_positive_zero() {
    // An object goes onto the floor with the translation `floor - min`, so a
    // -0.0 floor would give an object that already rests on it a -0.0
    // translation.
    let aabb = BuildVolume::new(192.0, 120.0, 200.0)
        .unwrap()
        .envelope_aabb();
    assert_eq!(aabb.min().coords()[2].to_bits(), 0.0_f32.to_bits());
}

#[test]
fn floor_centre_is_positive_zero_on_every_axis() {
    // A scene goes onto the plate by the translation from its box to this
    // point, so a -0.0 here can give a root a -0.0 translation, whose bits
    // differ from those of +0.0.
    for (x, y, z) in [(192.0, 120.0, 200.0), (143.43, 89.6, 175.0)] {
        let centre = BuildVolume::new(x, y, z).unwrap().floor_centre_mm();
        assert_eq!(
            centre.map(f64::to_bits),
            [0.0_f64.to_bits(); 3],
            "{x} x {y} x {z}"
        );
    }
}

#[test]
fn contains_returns_true_for_aabb_strictly_inside() {
    let inside = aabb([-10.0, -10.0, 1.0], [10.0, 10.0, 50.0]);
    assert!(volume().contains(&inside));
}

#[test]
fn contains_returns_true_when_aabb_touches_walls() {
    // Faces flush against every wall; still contained (contains is inclusive on
    // the boundary).
    let flush = aabb([-50.0, -30.0, 0.0], [50.0, 30.0, 80.0]);
    assert!(volume().contains(&flush));
}

#[test]
fn contains_returns_false_when_extending_past_plus_x() {
    let past_plus_x = aabb([0.0, 0.0, 1.0], [60.0, 10.0, 10.0]);
    assert!(!volume().contains(&past_plus_x));
}

#[test]
fn contains_returns_false_when_extending_past_minus_x() {
    let past_minus_x = aabb([-60.0, 0.0, 1.0], [0.0, 10.0, 10.0]);
    assert!(!volume().contains(&past_minus_x));
}

#[test]
fn contains_returns_false_when_extending_below_plate() {
    let below_plate = aabb([-10.0, -10.0, -1.0], [10.0, 10.0, 10.0]);
    assert!(!volume().contains(&below_plate));
}

#[test]
fn contains_returns_false_when_extending_above_ceiling() {
    let above_ceiling = aabb([-10.0, -10.0, 1.0], [10.0, 10.0, 100.0]);
    assert!(!volume().contains(&above_ceiling));
}

#[test]
fn contains_returns_true_for_empty_aabb() {
    // Vacuously contained: no extent to breach a wall.
    assert!(volume().contains(&Aabb::empty()));
}

#[test]
fn walls_crossed_by_names_each_wall_crossed_alone() {
    let cases = [
        (
            aabb([-60.0, 0.0, 1.0], [0.0, 10.0, 10.0]),
            CoordAxis::X,
            Side::Min,
        ),
        (
            aabb([0.0, 0.0, 1.0], [60.0, 10.0, 10.0]),
            CoordAxis::X,
            Side::Max,
        ),
        (
            aabb([-10.0, -40.0, 1.0], [10.0, 10.0, 10.0]),
            CoordAxis::Y,
            Side::Min,
        ),
        (
            aabb([-10.0, -10.0, 1.0], [10.0, 40.0, 10.0]),
            CoordAxis::Y,
            Side::Max,
        ),
        (
            aabb([-10.0, -10.0, -1.0], [10.0, 10.0, 10.0]),
            CoordAxis::Z,
            Side::Min,
        ),
        (
            aabb([-10.0, -10.0, 1.0], [10.0, 10.0, 100.0]),
            CoordAxis::Z,
            Side::Max,
        ),
    ];
    for (aabb, axis, side) in cases {
        assert_eq!(
            volume().walls_crossed_by(&aabb),
            walls(&[(axis, side)]),
            "{axis} {side:?}"
        );
    }
}

#[test]
fn walls_crossed_by_names_every_wall_a_box_crosses() {
    let past_2_walls = aabb([-60.0, 0.0, 1.0], [10.0, 40.0, 10.0]);
    assert_eq!(
        volume().walls_crossed_by(&past_2_walls),
        walls(&[(CoordAxis::X, Side::Min), (CoordAxis::Y, Side::Max)])
    );
}

#[test]
fn a_box_flush_with_every_wall_crosses_none() {
    let flush = aabb([-50.0, -30.0, 0.0], [50.0, 30.0, 80.0]);
    assert!(volume().walls_crossed_by(&flush).is_empty());
}

#[test]
fn the_empty_box_crosses_no_wall() {
    assert!(volume().walls_crossed_by(&Aabb::empty()).is_empty());
}

#[test]
fn a_box_empty_on_1_axis_fits_even_past_a_wall_on_another() {
    // Empty on Z, as `Aabb::empty` is, and past the `Min` wall of X.
    let empty_on_z = aabb([-60.0, 0.0, f32::INFINITY], [0.0, 10.0, f32::NEG_INFINITY]);
    assert!(volume().contains(&empty_on_z));
}

#[test]
fn walls_are_tested_against_the_f32_corners() {
    // The X corner is half the f32 length of 218.88, 109.44_f32
    // (109.44000244140625), above the f64 corner: a box flush with it fits, and
    // 1 that reaches the next f32 crosses the `Max` wall of X.
    let plate = BuildVolume::new(218.88, 100.0, 100.0).unwrap();
    let flush = aabb([0.0, 0.0, 0.0], [109.44, 1.0, 1.0]);
    let past = aabb([0.0, 0.0, 0.0], [109.440_01, 1.0, 1.0]);
    assert!(plate.contains(&flush));
    assert_eq!(
        plate.walls_crossed_by(&past),
        walls(&[(CoordAxis::X, Side::Max)])
    );
}

#[test]
fn zero_delta_is_zero() {
    let pre = aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]);
    let out = volume().clamp_translation_delta(&pre, Vector3D::new([0.0; 3]));
    assert_eq!(out, Vector3D::new([0.0; 3]));
}

#[test]
fn fully_interior_small_delta_passes_through() {
    let pre = aabb([-1.0, -1.0, 1.0], [1.0, 1.0, 3.0]);
    let delta = Vector3D::new([2.0, 1.0, 0.5]);
    let [x, y, z] = volume().clamp_translation_delta(&pre, delta).coords();
    assert_approx_eq!(f32, x, 2.0);
    assert_approx_eq!(f32, y, 1.0);
    assert_approx_eq!(f32, z, 0.5);
}

#[test]
fn delta_breaching_plus_x_wall_clamps_x_only() {
    // pre.max.x = 49, only 1 mm of room before +X wall.
    let pre = aabb([47.0, -1.0, 0.0], [49.0, 1.0, 2.0]);
    let delta = Vector3D::new([5.0, 1.0, 1.0]);
    let [x, y, z] = volume().clamp_translation_delta(&pre, delta).coords();
    assert_approx_eq!(f32, x, 1.0);
    assert_approx_eq!(f32, y, 1.0);
    assert_approx_eq!(f32, z, 1.0);
}

#[test]
fn touching_minus_x_wall_blocks_negative_x() {
    // pre.min.x = -50: flush against -X wall.
    let pre = aabb([-50.0, -1.0, 0.0], [-48.0, 1.0, 2.0]);
    let delta = Vector3D::new([-3.0, 0.5, 0.5]);
    let [x, y, z] = volume().clamp_translation_delta(&pre, delta).coords();
    assert_approx_eq!(f32, x, 0.0);
    assert_approx_eq!(f32, y, 0.5);
    assert_approx_eq!(f32, z, 0.5);
}

#[test]
fn already_outside_plus_x_blocks_further_plus_x() {
    // pre.max.x = 60 > env_max.x = 50.
    let pre = aabb([55.0, -1.0, 0.0], [60.0, 1.0, 2.0]);
    let delta = Vector3D::new([2.0, 0.0, 0.0]);
    let [x, _, _] = volume().clamp_translation_delta(&pre, delta).coords();
    assert_approx_eq!(f32, x, 0.0);
}

#[test]
fn already_outside_plus_x_allows_negative_x_back_inside() {
    // pre.max.x = 60 > 50. Allow movement back toward inside up to the opposite
    // wall: lower bound is env_min.x - pre.min.x = -50 - 55 = -105. A small
    // inward delta is preserved.
    let pre = aabb([55.0, -1.0, 0.0], [60.0, 1.0, 2.0]);
    let delta = Vector3D::new([-3.0, 0.0, 0.0]);
    let [x, _, _] = volume().clamp_translation_delta(&pre, delta).coords();
    assert_approx_eq!(f32, x, -3.0);
}

#[test]
fn z_floor_is_one_sided() {
    // Resting on the plate: pre.min.z = 0. -Z motion is blocked, +Z motion is
    // allowed up to env_max.z - pre.max.z = 80 - 5 = 75.
    let pre = aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 5.0]);
    let [_, _, down_z] = volume()
        .clamp_translation_delta(&pre, Vector3D::new([0.0, 0.0, -2.0]))
        .coords();
    assert_approx_eq!(f32, down_z, 0.0);
    let [_, _, up_z] = volume()
        .clamp_translation_delta(&pre, Vector3D::new([0.0, 0.0, 100.0]))
        .coords();
    assert_approx_eq!(f32, up_z, 75.0);
}

#[test]
fn aabb_larger_than_envelope_clamps_to_zero_on_that_axis() {
    // pre.x = [-200, 200] is wider than envelope on X. Y/Z fit.
    let pre = aabb([-200.0, -1.0, 0.0], [200.0, 1.0, 2.0]);
    let [x, y, z] = volume()
        .clamp_translation_delta(&pre, Vector3D::new([10.0, 5.0, 5.0]))
        .coords();
    assert_approx_eq!(f32, x, 0.0);
    assert_approx_eq!(f32, y, 5.0);
    assert_approx_eq!(f32, z, 5.0);
}

#[test]
fn diagonal_delta_slides_along_a_wall() {
    // Touching the +X wall, with room on Y and Z. A diagonal +X+Y push clamps X
    // to 0 and lets Y through unchanged: the box slides along the wall.
    let pre = aabb([48.0, -1.0, 0.0], [50.0, 1.0, 2.0]);
    let [x, y, z] = volume()
        .clamp_translation_delta(&pre, Vector3D::new([5.0, 4.0, 0.0]))
        .coords();
    assert_approx_eq!(f32, x, 0.0);
    assert_approx_eq!(f32, y, 4.0);
    assert_approx_eq!(f32, z, 0.0);
}

#[test]
fn a_box_flush_with_both_walls_of_an_axis_cannot_move_along_it() {
    // The box spans X from -50 to 50, flush with both walls: the range of X is
    // [0, 0].
    let pre = aabb([-50.0, -1.0, 0.0], [50.0, 1.0, 2.0]);
    let [x, _, _] = volume()
        .clamp_translation_delta(&pre, Vector3D::new([7.0, 0.0, 0.0]))
        .coords();
    assert_approx_eq!(f32, x, 0.0);
}

#[test]
fn the_clamp_stops_at_the_f32_corner() {
    // The X corner is 109.44_f32 (109.44000244140625). From a box whose maximum
    // X is 100, the room is 9.44000244140625, exact in f32; on the f64 corner
    // it would be 9.43999958038330078125, the f32 of 9.44.
    let plate = BuildVolume::new(218.88, 100.0, 100.0).unwrap();
    let pre = aabb([0.0, 0.0, 0.0], [100.0, 1.0, 1.0]);
    let [x, _, _] = plate
        .clamp_translation_delta(&pre, Vector3D::new([20.0, 0.0, 0.0]))
        .coords();
    assert_eq!(x, 9.440_002);
}

#[test]
fn axis_out_of_range_display_includes_axis_and_value() {
    let err = BuildVolume::new(0.0, 60.0, 100.0).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("X axis"), "msg: {msg}");
    assert!(msg.contains("got 0"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn build_volume_deserialises_from_toml() {
    let toml = "x_mm = 220.0\ny_mm = 220.0\nz_mm = 250.0\n";
    let bv: BuildVolume = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(bv.x_mm(), 220.0);
    assert_eq!(bv.y_mm(), 220.0);
    assert_eq!(bv.z_mm(), 250.0);
}

#[cfg(feature = "serde")]
#[test]
fn build_volume_deserialise_rejects_zero_axis() {
    let toml = "x_mm = 0.0\ny_mm = 220.0\nz_mm = 250.0\n";
    assert!(toml_edit::de::from_str::<BuildVolume>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn build_volume_deserialise_rejects_negative_axis() {
    let toml = "x_mm = 220.0\ny_mm = -1.0\nz_mm = 250.0\n";
    assert!(toml_edit::de::from_str::<BuildVolume>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn build_volume_roundtrip_through_toml_preserves_value() {
    let original = BuildVolume::new(192.0, 120.0, 200.0).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: BuildVolume = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

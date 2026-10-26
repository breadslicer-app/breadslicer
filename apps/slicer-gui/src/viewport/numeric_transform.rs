/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! The Transform inspector's numbers (BS-14, BS-15): the selected object's real
//! size, where it sits, its rotation and scale, and the edits typed into
//! them.
//!
//! Everything is in world space and in the slicer's terms: the position is
//! the centre of the object's tight footprint in X/Y and its lowest point in
//! Z (its lift off the plate); the size is its tight world box; rotation is
//! XYZ Euler angles (the linear part read as `Rz · Ry · Rx · scale`); scale is
//! the uniform factor against the file's own size. Edits rebuild the world
//! transform and keep the object where it was: a rotation or a rescale turns
//! about the footprint centre and keeps the lift.

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::ObjectQuery;

/// What the inspector shows for the selected object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Readout {
    pub size: [f64; 3],
    /// Footprint centre X/Y, lowest point Z.
    pub position: [f64; 3],
    pub rotation_deg: [f64; 3],
    pub scale_pct: f64,
}

/// An edit typed into the inspector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NumericEdit {
    /// Footprint centre X or Y, mm.
    Position { axis: usize, mm: f64 },
    /// Lowest point above the plate, mm.
    Lift(f64),
    /// One Euler angle, degrees.
    Rotation { axis: usize, deg: f64 },
    /// Uniform scale against the file's size, percent.
    ScalePct(f64),
    /// Uniform scale that brings one world dimension to `mm`.
    Size { axis: usize, mm: f64 },
    /// Lift 0.
    DropToPlate,
    /// Footprint centre on the plate's centre.
    Center([f64; 2]),
}

impl NumericEdit {
    /// The edit a UI field reports: its code and its value.
    pub(crate) fn from_ui(code: i32, value: f64, plate_centre: [f64; 2]) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        Some(match code {
            0 | 1 => NumericEdit::Position {
                axis: code as usize,
                mm: value,
            },
            2 => NumericEdit::Lift(value),
            3..=5 => NumericEdit::Rotation {
                axis: (code - 3) as usize,
                deg: value,
            },
            6 => NumericEdit::ScalePct(value),
            7..=9 => NumericEdit::Size {
                axis: (code - 7) as usize,
                mm: value,
            },
            10 => NumericEdit::DropToPlate,
            11 => NumericEdit::Center(plate_centre),
            _ => return None,
        })
    }
}

fn affine(t: Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => a,
    }
}

fn column_norms(m: &[[f64; 3]; 3]) -> [f64; 3] {
    [0, 1, 2].map(|j| (m[0][j] * m[0][j] + m[1][j] * m[1][j] + m[2][j] * m[2][j]).sqrt())
}

/// XYZ Euler angles (degrees) of the rotation in `m` once its column
/// scales are divided out.
fn euler_deg(m: &[[f64; 3]; 3]) -> [f64; 3] {
    let s = column_norms(m);
    if s.iter().any(|v| *v <= 1e-12) {
        return [0.0; 3];
    }
    let r = |i: usize, j: usize| m[i][j] / s[j];
    let ry = (-r(2, 0)).clamp(-1.0, 1.0).asin();
    let (rx, rz) = if r(2, 0).abs() < 1.0 - 1e-9 {
        (r(2, 1).atan2(r(2, 2)), r(1, 0).atan2(r(0, 0)))
    } else {
        // Gimbal lock: fold Z into X.
        ((-r(1, 2)).atan2(r(1, 1)), 0.0)
    };
    let tidy = |a: f64| {
        let d = a.to_degrees();
        if d.abs() < 1e-9 { 0.0 } else { d }
    };
    [tidy(rx), tidy(ry), tidy(rz)]
}

/// `Rz · Ry · Rx` for XYZ Euler angles in degrees.
fn rotation_from_euler(deg: [f64; 3]) -> [[f64; 3]; 3] {
    let [x, y, z] = deg.map(f64::to_radians);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// The tight world box of the object under the world transform `world`.
fn world_box(query: &ObjectQuery<'_>, world: Transform3D) -> Option<([f64; 3], [f64; 3])> {
    let aabb = query.transformed_aabb(world)?;
    let (min, max) = (aabb.min().coords(), aabb.max().coords());
    Some((min.map(f64::from), max.map(f64::from)))
}

fn anchor(min: [f64; 3], max: [f64; 3]) -> [f64; 3] {
    [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, min[2]]
}

/// The full readout of the object under its current world transform.
pub(crate) fn readout(query: &ObjectQuery<'_>) -> Option<Readout> {
    let world = query.world_transform();
    let (min, max) = world_box(query, world)?;
    Some(Readout {
        size: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
        position: anchor(min, max),
        ..cheap_readout(world)
    })
}

/// Rotation and scale only, from the matrix: what a gesture tick can afford.
/// Size and position are left at zero for the caller to carry over.
pub(crate) fn cheap_readout(world: Transform3D) -> Readout {
    let m = *affine(world).linear();
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    Readout {
        size: [0.0; 3],
        position: [0.0; 3],
        rotation_deg: euler_deg(&m),
        scale_pct: det.abs().cbrt() * 100.0,
    }
}

/// The new local transform for `edit`, or `None` when the edit does not
/// apply (no geometry, a degenerate value, a non-invertible parent).
pub(crate) fn apply(query: &ObjectQuery<'_>, edit: NumericEdit) -> Option<Transform3D> {
    let world = query.world_transform();
    let (min, max) = world_box(query, world)?;
    let before = anchor(min, max);
    let current = affine(world);
    let m = *current.linear();
    let t = *current.translation();

    // Linear part of the new world transform, if the edit changes it.
    let new_linear = match edit {
        NumericEdit::Rotation { axis, deg } => {
            let mut angles = euler_deg(&m);
            angles[axis] = deg;
            let s = column_norms(&m);
            let scale = [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]];
            Some(mat_mul(&rotation_from_euler(angles), &scale))
        }
        NumericEdit::ScalePct(pct) => {
            let now = cheap_readout(world).scale_pct;
            if pct <= 0.0 || now <= 0.0 {
                return None;
            }
            let f = pct / now;
            Some(m.map(|row| row.map(|v| v * f)))
        }
        NumericEdit::Size { axis, mm } => {
            let now = max[axis] - min[axis];
            if mm <= 0.0 || now <= 1e-9 {
                return None;
            }
            let f = mm / now;
            Some(m.map(|row| row.map(|v| v * f)))
        }
        _ => None,
    };
    let reshaped = match new_linear {
        Some(linear) => AffineTransform3D::from_linear_and_translation(linear, t),
        None => current,
    };

    // Where the anchor must end up.
    let target = match edit {
        NumericEdit::Position { axis, mm } => {
            let mut p = before;
            p[axis] = mm;
            p
        }
        NumericEdit::Lift(mm) => [before[0], before[1], mm.max(0.0)],
        NumericEdit::DropToPlate => [before[0], before[1], 0.0],
        NumericEdit::Center([x, y]) => [x, y, before[2]],
        _ => before,
    };
    let reshaped_world = Transform3D::Affine(reshaped);
    let (rmin, rmax) = world_box(query, reshaped_world)?;
    let now = anchor(rmin, rmax);
    let shift = [target[0] - now[0], target[1] - now[1], target[2] - now[2]];
    let moved = AffineTransform3D::from_translation(shift).compose(&reshaped);
    let inverse_parent = query.parent_world().inverse().ok()?;
    Some(inverse_parent.compose(&Transform3D::Affine(moved)))
}

/// What a gesture has done so far, for the viewport chip (BS-69): the
/// world translation, the rotation angle (signed about `axis` when the
/// gesture is locked to one) and the scale ratio between `before` and `now`.
pub(crate) fn gesture_delta_text(
    kind: GestureKind,
    before: Transform3D,
    now: Transform3D,
    axis: Option<usize>,
) -> String {
    let (a, b) = (affine(before), affine(now));
    match kind {
        GestureKind::Move => {
            let (t0, t1) = (a.translation(), b.translation());
            let d = [t1[0] - t0[0], t1[1] - t0[1], t1[2] - t0[2]];
            match axis {
                Some(k) => format!("{} {:+.2} mm", ["X", "Y", "Z"][k], d[k]),
                None => format!(
                    "X {:+.2}  Y {:+.2}  Z {:+.2} mm  ({:.2} mm)",
                    d[0],
                    d[1],
                    d[2],
                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                ),
            }
        }
        GestureKind::Rotate => {
            let deg = rotation_between_deg(a.linear(), b.linear(), axis);
            match axis {
                Some(k) => format!("{} {:+.1}°", ["X", "Y", "Z"][k], deg),
                None => format!("{deg:.1}°"),
            }
        }
        GestureKind::Scale => {
            let (s0, s1) = (
                cheap_readout(before).scale_pct,
                cheap_readout(now).scale_pct,
            );
            if s0 <= 0.0 {
                return String::new();
            }
            format!("{:.1} %", s1 / s0 * 100.0)
        }
    }
}

/// Which kind of gesture a delta describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GestureKind {
    Move,
    Rotate,
    Scale,
}

/// The angle in degrees of the rotation taking `from` to `to` (column scales
/// divided out); signed by `axis` when given, else unsigned.
fn rotation_between_deg(from: &[[f64; 3]; 3], to: &[[f64; 3]; 3], axis: Option<usize>) -> f64 {
    let unit = |m: &[[f64; 3]; 3]| {
        let s = column_norms(m);
        let mut r = *m;
        for row in r.iter_mut() {
            for (j, v) in row.iter_mut().enumerate() {
                *v /= s[j].max(1e-12);
            }
        }
        r
    };
    let (r0, r1) = (unit(from), unit(to));
    // delta = r1 * r0^T
    let mut d = [[0.0; 3]; 3];
    for (i, row) in d.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| r1[i][k] * r0[j][k]).sum();
        }
    }
    let cos = ((d[0][0] + d[1][1] + d[2][2] - 1.0) * 0.5).clamp(-1.0, 1.0);
    let angle = cos.acos().to_degrees();
    match axis {
        Some(k) => {
            // The rotation vector's component along the axis carries the sign.
            let v = [d[2][1] - d[1][2], d[0][2] - d[2][0], d[1][0] - d[0][1]];
            if v[k] < 0.0 { -angle } else { angle }
        }
        None => angle,
    }
}

#[cfg(test)]
mod numeric_transform_tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn euler_round_trips_through_the_matrix() {
        for angles in [[10.0, 20.0, 30.0], [-45.0, 0.0, 90.0], [0.0, -30.0, 170.0]] {
            let m = rotation_from_euler(angles);
            assert!(close(euler_deg(&m), angles), "{angles:?}");
        }
    }

    #[test]
    fn euler_ignores_the_scale() {
        let m = rotation_from_euler([15.0, -25.0, 40.0]);
        let scaled = mat_mul(&m, &[[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]]);
        assert!(close(euler_deg(&scaled), [15.0, -25.0, 40.0]));
        let r = cheap_readout(Transform3D::Affine(
            AffineTransform3D::from_linear_and_translation(scaled, [0.0; 3]),
        ));
        assert!((r.scale_pct - 200.0).abs() < 1e-9);
    }

    #[test]
    fn deltas_read_as_the_gesture_did() {
        let id = Transform3D::Identity;
        let moved = Transform3D::Affine(AffineTransform3D::from_translation([3.0, 0.0, 0.0]));
        assert_eq!(
            gesture_delta_text(GestureKind::Move, id, moved, Some(0)),
            "X +3.00 mm"
        );
        let turned = Transform3D::Affine(AffineTransform3D::from_rotation_z(-30f64.to_radians()));
        assert_eq!(
            gesture_delta_text(GestureKind::Rotate, id, turned, Some(2)),
            "Z -30.0°"
        );
        let grown = Transform3D::Affine(AffineTransform3D::from_uniform_scale(1.5));
        assert_eq!(
            gesture_delta_text(GestureKind::Scale, id, grown, None),
            "150.0 %"
        );
    }

    #[test]
    fn ui_codes_map_to_edits() {
        let c = [10.0, 20.0];
        assert_eq!(
            NumericEdit::from_ui(2, 5.0, c),
            Some(NumericEdit::Lift(5.0))
        );
        assert_eq!(
            NumericEdit::from_ui(4, 30.0, c),
            Some(NumericEdit::Rotation { axis: 1, deg: 30.0 })
        );
        assert_eq!(
            NumericEdit::from_ui(11, 0.0, c),
            Some(NumericEdit::Center(c))
        );
        assert_eq!(NumericEdit::from_ui(99, 0.0, c), None);
        assert_eq!(NumericEdit::from_ui(0, f64::NAN, c), None);
    }
}

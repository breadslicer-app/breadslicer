/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Lay on face (#15): a click on a face of a model turns the model so that
//! face lies on the plate, in one Undo step.
//!
//! The face is the flat patch the click lands on: every triangle of the mesh
//! in the clicked triangle's plane (normals within [`FLAT_DEG`], plane
//! within a hair), averaged by area, so a flat face made of many slivers,
//! or a scan's slightly noisy one, lies flat as a whole. The model turns
//! about the centre of its footprint (as Orient and the Rotate card do) and
//! is kept on its plate; a model with no supports ends on the plate, one on
//! supports keeps its lift for the new supports a tilt makes (#153).

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

/// How far, in degrees, a triangle's normal may turn from the clicked one's
/// and still belong to the same flat face.
pub(crate) const FLAT_DEG: f64 = 2.0;

/// Below this angle, in degrees, the face already lies on the plate.
const ALREADY_FLAT_DEG: f64 = 0.01;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn corners(mesh: &IndexedMesh, face: usize) -> Option<[[f64; 3]; 3]> {
    let tri = mesh.faces().get(face)?;
    let mut out = [[0.0; 3]; 3];
    for (slot, v) in out.iter_mut().zip(tri.vertices) {
        *slot = mesh.vertices().get(usize::from(v))?.coords().map(f64::from);
    }
    Some(out)
}

/// The area-weighted normal (not unit) of a triangle: its cross product.
fn area_normal(c: &[[f64; 3]; 3]) -> [f64; 3] {
    cross(sub(c[1], c[0]), sub(c[2], c[0]))
}

/// The outward unit normal, in the mesh's own axes, of the flat face that
/// triangle `face` belongs to: every triangle within [`FLAT_DEG`] of its
/// normal and in its plane, averaged by area. `None` for a degenerate
/// triangle or one out of range.
pub(crate) fn flat_normal(mesh: &IndexedMesh, face: usize) -> Option<[f64; 3]> {
    let seed = corners(mesh, face)?;
    let n0 = area_normal(&seed);
    let len0 = length(n0);
    if len0 <= 1e-12 {
        return None;
    }
    let n0 = n0.map(|v| v / len0);
    let d0 = dot(n0, seed[0]);
    // In the plane: within a thousandth of the model's size, at least a
    // micron.
    let (lo, hi) = mesh.vertices().iter().fold(
        ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
        |(lo, hi), v| {
            let p = v.coords().map(f64::from);
            (
                [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
                [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
            )
        },
    );
    let tolerance = (length(sub(hi, lo)) * 1e-3).max(1e-3);
    let cos_flat = FLAT_DEG.to_radians().cos();
    let mut sum = [0.0; 3];
    for i in 0..mesh.faces().len() {
        let Some(c) = corners(mesh, i) else {
            continue;
        };
        let n = area_normal(&c);
        let len = length(n);
        if len <= 1e-12 || dot(n, n0) < cos_flat * len {
            continue;
        }
        if c.iter().any(|p| (dot(n0, *p) - d0).abs() > tolerance) {
            continue;
        }
        sum = [sum[0] + n[0], sum[1] + n[1], sum[2] + n[2]];
    }
    let len = length(sum);
    (len > 1e-12).then(|| sum.map(|v| v / len))
}

/// `normal` (a direction in the mesh's own axes) in world axes under
/// `world`: through the inverse transpose of its linear part, so a scaled
/// or mirrored model's face keeps its direction. Unit, or `None`.
pub(crate) fn world_normal(world: &AffineTransform3D, normal: [f64; 3]) -> Option<[f64; 3]> {
    let inverse = world.inverse().ok()?;
    let m = inverse.linear();
    // (M⁻¹)ᵀ · n
    let n = [0, 1, 2].map(|j| (0..3).map(|i| m[i][j] * normal[i]).sum::<f64>());
    let len = length(n);
    (len > 1e-12).then(|| n.map(|v| v / len))
}

/// The rotation (row-major, world axes) that turns `normal` (unit) to point
/// straight down, by the shortest turn; `None` when it already does.
pub(crate) fn rotation_to_floor(normal: [f64; 3]) -> Option<[[f64; 3]; 3]> {
    let down = [0.0, 0.0, -1.0];
    let cos = dot(normal, down).clamp(-1.0, 1.0);
    if cos.acos().to_degrees() < ALREADY_FLAT_DEG {
        return None;
    }
    let axis = cross(normal, down);
    let sin = length(axis);
    let axis = if sin <= 1e-9 {
        // Facing straight up: half a turn about X.
        [1.0, 0.0, 0.0]
    } else {
        axis.map(|v| v / sin)
    };
    let angle = cos.acos();
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = axis;
    Some([
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ])
}

/// The world pose that lays `mesh`, now at `world`, on the face whose world
/// normal is `normal`: turned about the centre of its footprint, kept inside
/// the plate `plate` (min, max in X/Y) when it was inside, and with `drop`
/// its lowest point on the plate; otherwise the lift it had. `None` when the
/// face already lies down and nothing is to drop.
pub(crate) fn laid_pose(
    mesh: &IndexedMesh,
    world: &AffineTransform3D,
    normal: [f64; 3],
    plate: Option<([f64; 2], [f64; 2])>,
    drop: bool,
) -> Option<AffineTransform3D> {
    let kept = match rotation_to_floor(normal) {
        Some(rotation) => {
            let turned = sc_orient::turned_about_footprint(mesh, world, &rotation);
            match plate {
                Some((lo, hi)) => sc_orient::kept_on_plate(mesh, world, &turned, lo, hi),
                None => turned,
            }
        }
        None if drop => *world,
        None => return None,
    };
    if !drop {
        return Some(kept);
    }
    let lowest = f64::from(sc_orient::drawn_lowest(mesh, &kept));
    if !lowest.is_finite() {
        return Some(kept);
    }
    let down = AffineTransform3D::from_translation([0.0, 0.0, -lowest]).compose(&kept);
    Some(sc_orient::drawn_no_lower_than(mesh, &down, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_mesh_fixtures::unit_cube_mesh;

    fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|i| dot(m[i], v))
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn the_rotation_turns_the_normal_straight_down() {
        for n in [
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.6, 0.0, 0.8],
            [0.48, 0.6, -0.64],
        ] {
            let r = rotation_to_floor(n).expect("turns");
            assert!(close(apply(&r, n), [0.0, 0.0, -1.0]), "{n:?}");
            // A rotation: its rows are unit and the determinant is 1.
            let det = dot(r[0], cross(r[1], r[2]));
            assert!((det - 1.0).abs() < 1e-9);
        }
        assert!(rotation_to_floor([0.0, 0.0, -1.0]).is_none());
    }

    #[test]
    fn a_cube_face_is_found_flat_as_a_whole() {
        let cube = unit_cube_mesh();
        // Faces 8 and 9 are the +X side, two triangles.
        for face in [8, 9] {
            let n = flat_normal(&cube, face).expect("a face");
            assert!(close(n, [1.0, 0.0, 0.0]), "{n:?}");
        }
        // The top, faces 2 and 3.
        assert!(close(flat_normal(&cube, 2).unwrap(), [0.0, 0.0, 1.0]));
    }

    #[test]
    fn a_scaled_model_keeps_its_face_direction() {
        // Stretched along X: a slanted normal leans less towards X.
        let world = AffineTransform3D::from_linear_and_translation(
            [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0; 3],
        );
        let s = 0.5f64.sqrt();
        let n = world_normal(&world, [s, 0.0, s]).unwrap();
        assert!(n[0] < n[2] && n[0] > 0.0, "{n:?}");
    }

    #[test]
    fn a_cube_laid_on_its_side_ends_on_the_plate() {
        let cube = unit_cube_mesh();
        // A cube floating 3 mm up, its centre at (10, 10, 3.5).
        let world = AffineTransform3D::from_translation([10.0, 10.0, 3.5]);
        let laid = laid_pose(&cube, &world, [1.0, 0.0, 0.0], None, true).unwrap();
        assert!(f64::from(sc_orient::drawn_lowest(&cube, &laid)).abs() < 1e-5);
        // The +X face now faces down.
        let n = world_normal(&laid, [1.0, 0.0, 0.0]).unwrap();
        assert!(close(n.map(|v| (v * 1e6).round() / 1e6), [0.0, 0.0, -1.0]));
        // Same footprint centre.
        let t = laid.translation();
        assert!((t[0] - 10.0).abs() < 1e-6 && (t[1] - 10.0).abs() < 1e-6);
        // Without the drop it keeps the lift.
        let kept = laid_pose(&cube, &world, [1.0, 0.0, 0.0], None, false).unwrap();
        assert!((f64::from(sc_orient::drawn_lowest(&cube, &kept)) - 3.0).abs() < 1e-4);
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure drag-math FSM for the transform gizmo. The data types
//! (`GizmoState`, `DragOp`, `HandleHit`) and the per-frame compose
//! function (`apply_drag_move`) are controller-agnostic: they take the
//! current cursor / camera / pivot inputs and return the new transform
//! plus the (possibly mutated) op state. The `ViewportController` owns
//! the `Cell`s and drives the Slint callbacks; everything in this module
//! is a free function.

use glam::Vec3;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;

use crate::gizmo::axis::Axis;
use crate::gizmo::dimensions::HANDLE_PIXEL_LENGTH;
use crate::gizmo::drag_math::ScreenTurn;
use crate::gizmo::modifiers::DragModifiers;
use crate::gizmo::{self};
use crate::picking;

/// Gizmo input FSM. `Idle` is the resting state (no drag in
/// progress). `Drag` captures the operation (translate / rotate /
/// scale-axis / scale-uniform), the object's transform at gesture
/// start, the world-space pivot, and the dragged leaf's stable
/// [`NodeId`]. The running transform is composed each frame *from the
/// captured original*, not from the running transform: composing-
/// from-original keeps long drags numerically clean (no float drift
/// across hundreds of move events).
#[derive(Copy, Clone, Debug)]
pub(super) enum GizmoState {
    Idle,
    Drag {
        op: DragOp,
        original: Transform3D,
        pivot: Vec3,
        node_id: NodeId,
    },
}

/// Per-operation drag state carried inside `GizmoState::Drag`.
///
/// - `Translate` composes a translation; `anchor` is the world-space
///   closest point on the axis at press, recomputed each move.
/// - `Rotate` composes a rotation about the axis through the pivot. The
///   turn is measured on screen (`turn`, around the projected pivot, with
///   `sign` set at press from whether the ring faces the camera), so a
///   circle drawn on screen turns evenly whatever the tilt of the ring;
///   it unwraps, so a full-revolution drag yields a real 360°+ rotation.
///   `start_angle` is where the ring was grabbed, in the basis of
///   `axis_basis`, for the marks of the turn; `applied` is the turn
///   actually applied (snapped with Ctrl).
/// - `ScaleAxis` composes a per-axis scale about the pivot from the
///   world-space closest points on the axis at press and now.
/// - `ScaleUniform` (the centre ball) composes a uniform scale from the
///   cursor travel since `press`: right or up grows, left or down
///   shrinks.
#[derive(Copy, Clone, Debug)]
pub(super) enum DragOp {
    Translate {
        axis: Axis,
        anchor: Vec3,
    },
    Rotate {
        axis: Axis,
        start_angle: f32,
        turn: ScreenTurn,
        sign: f32,
        applied: f32,
    },
    ScaleAxis {
        axis: Axis,
        start_anchor: Vec3,
        /// Latest closest-point-on-axis under the cursor. Captured at
        /// press (= `start_anchor`) and refreshed each move so the
        /// renderer can slide the per-axis cube along the axis to
        /// follow the cursor as visual feedback for the live scale.
        current_anchor: Vec3,
    },
    ScaleUniform {
        press: (f32, f32),
    },
}

/// Categorised winner of the press-time (or hover-time) gizmo
/// hit-test. Used by the controller to map across-category arbitration
/// into the matching `DragOp` variant on press, and into the renderer's
/// hover-tint mask on hover.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum HandleHit {
    Translate(Axis),
    Rotate(Axis),
    ScaleAxis(Axis),
    ScaleUniform,
}

/// Uniform scale factor of the centre ball for a cursor travel of
/// `(dx, dy)` pixels: doubles per shaft length to the right or up, halves
/// per shaft length to the left or down.
pub(super) fn uniform_drag_factor(dx: f32, dy: f32) -> f32 {
    ((dx - dy) / HANDLE_PIXEL_LENGTH).exp2().clamp(1e-3, 1e3)
}

/// Per-op delta computation for a move event. Returns the new
/// transform composed onto `original`, paired with the (possibly
/// updated) `DragOp` state: Rotate's unwrap counter mutates per
/// frame; the others are pure functions of the press capture and the
/// current cursor. `ray` and `cursor` are the gesture cursor (slowed
/// down while Shift is held); `modifiers.snap` rounds the result to
/// steps.
///
/// Returns `None` if the cursor produces no usable delta this frame
/// (e.g. rotate ray slipped into the lock-out region between move
/// events): the controller leaves the running transform unchanged
/// in that case rather than snapping the object.
pub(super) fn apply_drag_move(
    op: DragOp,
    ray: &picking::Ray,
    pivot: Vec3,
    parent_world: &Transform3D,
    original: &Transform3D,
    cursor: (f32, f32),
    modifiers: DragModifiers,
) -> Option<(Transform3D, DragOp)> {
    match op {
        DragOp::Translate { axis, anchor } => {
            let new_anchor = gizmo::drag_math::closest_point_on_axis(ray, pivot, axis)?;
            let along = modifiers.distance((new_anchor - anchor).dot(axis.world_dir()));
            let delta = axis.world_dir() * along;
            let translation = AffineTransform3D::from_translation([
                delta.x as f64,
                delta.y as f64,
                delta.z as f64,
            ]);
            Some((Transform3D::Affine(translation).compose(original), op))
        }
        DragOp::Rotate {
            axis,
            start_angle,
            mut turn,
            sign,
            ..
        } => {
            let applied = modifiers.angle(sign * turn.update(cursor));
            let new_op = DragOp::Rotate {
                axis,
                start_angle,
                turn,
                sign,
                applied,
            };
            // R(axis, applied) about the world-space pivot, expressed in
            // the leaf's parent space by `compose_about_pivot`.
            let rotation = axis_rotation(axis, applied)
                .map(|r| compose_about_pivot(&r, pivot, parent_world, original));
            rotation.map(|new_t| (new_t, new_op))
        }
        DragOp::ScaleAxis {
            axis, start_anchor, ..
        } => {
            // `None` here means the camera snapped into the axis lock-out
            // since press; freeze the drag for this frame.
            let current_anchor = gizmo::drag_math::closest_point_on_axis(ray, pivot, axis)?;
            let factor = modifiers.factor(gizmo::drag_math::scale_factor_along_axis(
                start_anchor,
                current_anchor,
                pivot,
                axis,
            ));
            let new_op = DragOp::ScaleAxis {
                axis,
                start_anchor,
                current_anchor,
            };
            Some((
                compose_about_pivot(
                    &scale_along_axis(axis, factor),
                    pivot,
                    parent_world,
                    original,
                ),
                new_op,
            ))
        }
        DragOp::ScaleUniform { press } => {
            let factor =
                modifiers.factor(uniform_drag_factor(cursor.0 - press.0, cursor.1 - press.1));
            let scale = AffineTransform3D::from_uniform_scale(factor as f64);
            Some((
                compose_about_pivot(&scale, pivot, parent_world, original),
                op,
            ))
        }
    }
}

/// Affine rotation about a world-aligned axis by `angle` radians,
/// right-handed. Returns `None` if the axis happens to be zero:
/// can't happen for `Axis::{X,Y,Z}` but the propagation keeps the
/// caller's `?` chain typed.
pub(super) fn axis_rotation(axis: Axis, angle: f32) -> Option<AffineTransform3D> {
    let angle_f64 = angle as f64;
    Some(match axis {
        Axis::X => AffineTransform3D::from_rotation_x(angle_f64),
        Axis::Y => AffineTransform3D::from_rotation_y(angle_f64),
        Axis::Z => AffineTransform3D::from_rotation_z(angle_f64),
    })
}

/// Per-axis scale, with the off-axis components held at 1.0 (no
/// non-uniformity bleed). Used by `ScaleAxis`.
pub(super) fn scale_along_axis(axis: Axis, factor: f32) -> AffineTransform3D {
    let f = factor as f64;
    let s = match axis {
        Axis::X => [f, 1.0, 1.0],
        Axis::Y => [1.0, f, 1.0],
        Axis::Z => [1.0, 1.0, f],
    };
    AffineTransform3D::from_non_uniform_scale(s)
}

/// Composes a rotation/scale operation `m` about a world-space
/// `pivot`, then applies it to `original` (the leaf's
/// parent-relative transform).
///
/// `parent_world` is the leaf's ancestor chain composed up to but
/// not including the leaf itself. When it is `Identity`, the
/// world-space "rotate/scale about `pivot`" reduces to the textbook
/// `T(pivot) ∘ m ∘ T(-pivot) ∘ original`. When the parent is
/// non-trivial (e.g. the placement translation `place_on_plate`
/// writes onto the scene root), the world pivot must be expressed
/// in the leaf's parent space first: otherwise the composition
/// applies the rotation/scale about a point offset by the parent's
/// own translation, displacing the object instead of holding it
/// fixed.
///
/// For a pure-translation parent (the only kind today's scene
/// shapes produce), the conversion is `pivot_local = pivot - δ_p`,
/// where `δ_p` is the parent's translation. The general form goes
/// through `parent_world.inverse()` applied as a point transform.
pub(super) fn compose_about_pivot(
    m: &AffineTransform3D,
    pivot: Vec3,
    parent_world: &Transform3D,
    original: &Transform3D,
) -> Transform3D {
    let pivot_local = world_pivot_to_local(pivot, parent_world);
    let to_pivot = AffineTransform3D::from_translation([
        pivot_local.x as f64,
        pivot_local.y as f64,
        pivot_local.z as f64,
    ]);
    let from_pivot = AffineTransform3D::from_translation([
        -pivot_local.x as f64,
        -pivot_local.y as f64,
        -pivot_local.z as f64,
    ]);
    let about_pivot = to_pivot.compose(&m.compose(&from_pivot));
    Transform3D::Affine(about_pivot).compose(original)
}

/// Express the world-space `pivot` in the leaf's parent space, i.e.
/// apply `parent_world.inverse()` as a point transform. Falls back
/// to the world `pivot` unchanged if the parent transform is
/// singular (cannot happen for valid scene graphs today: every
/// production parent is a pure translation, which is invertible).
fn world_pivot_to_local(pivot: Vec3, parent_world: &Transform3D) -> Vec3 {
    let Transform3D::Affine(p) = parent_world else {
        return pivot;
    };
    let Ok(inv) = p.inverse() else {
        return pivot;
    };
    let mapped = inv.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([
        pivot.x, pivot.y, pivot.z,
    ]));
    Vec3::from(mapped.coords())
}

#[cfg(test)]
mod apply_drag_move_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    const NO_KEYS: DragModifiers = DragModifiers {
        snap: false,
        fine: false,
    };

    /// A rotate drag of a ring facing the camera, pressed at `press` around
    /// the projected pivot `pivot_screen`.
    fn rotate_op(axis: Axis, pivot_screen: (f32, f32), press: (f32, f32)) -> DragOp {
        DragOp::Rotate {
            axis,
            start_angle: 0.0,
            turn: ScreenTurn::new(pivot_screen, press),
            sign: -1.0,
            applied: 0.0,
        }
    }

    fn ray_from(origin: Vec3, target: Vec3) -> picking::Ray {
        picking::Ray {
            origin,
            direction: (target - origin).normalize(),
        }
    }

    fn pivot_origin() -> Vec3 {
        Vec3::ZERO
    }

    /// Read the affine linear part as a row-major 3×3 of f64.
    fn read_linear(t: &Transform3D) -> [[f64; 3]; 3] {
        match t {
            Transform3D::Identity => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            Transform3D::Affine(a) => *a.linear(),
        }
    }

    fn read_translation_t(t: &Transform3D) -> [f64; 3] {
        match t {
            Transform3D::Identity => [0.0; 3],
            Transform3D::Affine(a) => *a.translation(),
        }
    }

    #[test]
    fn translate_op_composes_a_pure_translation() {
        // Press anchor on +X at (1, 0, 0); cursor moves the closest
        // point to (1.5, 0, 0). Object should end up translated by
        // +0.5 along X.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::Translate {
            axis: Axis::X,
            anchor: Vec3::new(1.0, 0.0, 0.0),
        };
        // Construct a ray whose closest point on +X axis is (1.5, 0, 0).
        let ray = ray_from(Vec3::new(1.5, -3.0, 0.0), Vec3::new(1.5, 0.0, 0.0));
        let (new_t, _) = apply_drag_move(
            op,
            &ray,
            pivot,
            &Transform3D::Identity,
            &original,
            (0.0, 0.0),
            NO_KEYS,
        )
        .unwrap();
        let translation = read_translation_t(&new_t);
        assert_approx_eq!(f64, translation[0], 0.5, epsilon = 1e-4);
        assert_approx_eq!(f64, translation[1], 0.0, epsilon = 1e-4);
        assert_approx_eq!(f64, translation[2], 0.0, epsilon = 1e-4);
        // Linear part stays identity.
        let m = read_linear(&new_t);
        assert_approx_eq!(f64, m[0][0], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m[1][1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m[2][2], 1.0, epsilon = 1e-6);
    }

    #[test]
    fn scale_axis_op_composes_a_diagonal_scale_about_pivot() {
        // Press anchor at (1, 0, 0); current anchor at (2, 0, 0)
        // → factor 2 along X. Pivot at origin → translation stays 0.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::ScaleAxis {
            axis: Axis::X,
            start_anchor: Vec3::new(1.0, 0.0, 0.0),
            current_anchor: Vec3::new(1.0, 0.0, 0.0),
        };
        let ray = ray_from(Vec3::new(2.0, -3.0, 0.0), Vec3::new(2.0, 0.0, 0.0));
        let (new_t, _) = apply_drag_move(
            op,
            &ray,
            pivot,
            &Transform3D::Identity,
            &original,
            (0.0, 0.0),
            NO_KEYS,
        )
        .unwrap();
        let m = read_linear(&new_t);
        assert_approx_eq!(f64, m[0][0], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m[2][2], 1.0, epsilon = 1e-6);
        let translation = read_translation_t(&new_t);
        assert_approx_eq!(f64, translation[0], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn centre_ball_doubles_per_shaft_length_right_or_up() {
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::ScaleUniform {
            press: (100.0, 100.0),
        };
        let ray = ray_from(Vec3::new(0.0, -3.0, 0.0), Vec3::ZERO);
        let run = |cursor: (f32, f32)| {
            let (t, _) = apply_drag_move(
                op,
                &ray,
                pivot,
                &Transform3D::Identity,
                &original,
                cursor,
                NO_KEYS,
            )
            .unwrap();
            read_linear(&t)
        };
        let right = run((100.0 + HANDLE_PIXEL_LENGTH, 100.0));
        let up = run((100.0, 100.0 - HANDLE_PIXEL_LENGTH));
        let left = run((100.0 - HANDLE_PIXEL_LENGTH, 100.0));
        for (m, expected) in [(right, 2.0), (up, 2.0), (left, 0.5)] {
            for (i, row) in m.iter().enumerate() {
                assert_approx_eq!(f64, row[i], expected, epsilon = 1e-4);
            }
        }
    }

    #[test]
    fn ctrl_snaps_every_drag_to_its_step() {
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let snap = DragModifiers {
            snap: true,
            fine: false,
        };
        // Move: 0.5 + 2.37 along X snaps to 2 mm from the press.
        let op = DragOp::Translate {
            axis: Axis::X,
            anchor: Vec3::new(0.5, 0.0, 0.0),
        };
        let ray = ray_from(Vec3::new(2.87, -3.0, 0.0), Vec3::new(2.87, 0.0, 0.0));
        let (t, _) = apply_drag_move(
            op,
            &ray,
            pivot,
            &Transform3D::Identity,
            &original,
            (0.0, 0.0),
            snap,
        )
        .unwrap();
        assert_approx_eq!(f64, read_translation_t(&t)[0], 2.0, epsilon = 1e-5);

        // Rotate: a 20° turn snaps to 15°, and `applied` says so.
        // On screen the cursor turns 20° counter-clockwise around the
        // pivot of a ring facing the camera.
        let op = rotate_op(Axis::Z, (100.0, 100.0), (200.0, 100.0));
        let a = 20f32.to_radians();
        let (t, new_op) = apply_drag_move(
            op,
            &ray,
            pivot,
            &Transform3D::Identity,
            &original,
            (100.0 + 100.0 * a.cos(), 100.0 - 100.0 * a.sin()),
            snap,
        )
        .unwrap();
        let m = read_linear(&t);
        assert_approx_eq!(f64, m[1][0], 15f64.to_radians().sin(), epsilon = 1e-5);
        match new_op {
            DragOp::Rotate { applied, .. } => {
                assert_approx_eq!(f32, applied, 15f32.to_radians(), epsilon = 1e-6)
            }
            other => panic!("expected Rotate, got {other:?}"),
        }

        // Scale along an axis: x1.73 snaps to x1.7.
        let op = DragOp::ScaleAxis {
            axis: Axis::X,
            start_anchor: Vec3::new(1.0, 0.0, 0.0),
            current_anchor: Vec3::new(1.0, 0.0, 0.0),
        };
        let ray = ray_from(Vec3::new(1.73, -3.0, 0.0), Vec3::new(1.73, 0.0, 0.0));
        let (t, _) = apply_drag_move(
            op,
            &ray,
            pivot,
            &Transform3D::Identity,
            &original,
            (0.0, 0.0),
            snap,
        )
        .unwrap();
        assert_approx_eq!(f64, read_linear(&t)[0][0], 1.7, epsilon = 1e-5);
    }

    #[test]
    fn rotate_op_composes_a_rotation_about_pivot() {
        // The X ring faces the camera; the cursor turns a quarter turn
        // counter-clockwise on screen around the pivot, which is +π/2 about
        // +X and takes +Y to +Z.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = rotate_op(Axis::X, (100.0, 100.0), (200.0, 100.0));
        let ray = ray_from(Vec3::new(5.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        let (new_t, new_op) = apply_drag_move(
            op,
            &ray,
            pivot,
            &Transform3D::Identity,
            &original,
            (100.0, 0.0),
            NO_KEYS,
        )
        .unwrap();
        let m = read_linear(&new_t);
        // R(X, π/2) maps (0, 1, 0) → (0, 0, 1):
        //   m[0][1] = 0, m[1][1] = 0, m[2][1] = 1.
        assert_approx_eq!(f64, m[0][1], 0.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 0.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[2][1], 1.0, epsilon = 1e-4);
        match new_op {
            DragOp::Rotate { applied, .. } => {
                assert_approx_eq!(f32, applied, std::f32::consts::FRAC_PI_2, epsilon = 1e-4)
            }
            other => panic!("expected Rotate, got {other:?}"),
        }
    }

    #[test]
    fn a_ring_facing_away_turns_the_other_way() {
        let op = DragOp::Rotate {
            axis: Axis::X,
            start_angle: 0.0,
            turn: ScreenTurn::new((100.0, 100.0), (200.0, 100.0)),
            sign: 1.0,
            applied: 0.0,
        };
        let ray = ray_from(Vec3::new(5.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        let (_, new_op) = apply_drag_move(
            op,
            &ray,
            pivot_origin(),
            &Transform3D::Identity,
            &Transform3D::Identity,
            (100.0, 0.0),
            NO_KEYS,
        )
        .unwrap();
        match new_op {
            DragOp::Rotate { applied, .. } => {
                assert_approx_eq!(f32, applied, -std::f32::consts::FRAC_PI_2, epsilon = 1e-4)
            }
            other => panic!("expected Rotate, got {other:?}"),
        }
    }

    #[test]
    fn uniform_scale_under_translated_parent_keeps_world_centre_fixed() {
        // Regression for the displacement-on-scale bug: the gizmo pivot lives
        // in world space; the leaf's transform is the PARENT-relative local.
        // With a root translation `parent_world = T(40, 0, 0)` and an identity
        // leaf carrying a mesh centred on the LOCAL origin, the world centre
        // of the object is at `(40, 0, 0)`, which is also the gizmo pivot. A
        // uniform scale by factor 2 about that world pivot must leave the
        // world image of the mesh centre at `(40, 0, 0)`.
        let parent_world =
            Transform3D::Affine(AffineTransform3D::from_translation([40.0, 0.0, 0.0]));
        let original = Transform3D::Identity;
        let world_pivot = Vec3::new(40.0, 0.0, 0.0);
        let op = DragOp::ScaleUniform {
            press: (100.0, 100.0),
        };
        let ray = ray_from(Vec3::new(40.0, -3.0, 0.0), Vec3::new(40.0, 0.0, 0.0));
        let (new_local, _) = apply_drag_move(
            op,
            &ray,
            world_pivot,
            &parent_world,
            &original,
            (100.0 + HANDLE_PIXEL_LENGTH, 100.0),
            NO_KEYS,
        )
        .unwrap();

        let world_centre = match parent_world.compose(&new_local) {
            Transform3D::Identity => Vec3::ZERO,
            Transform3D::Affine(a) => {
                let v = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([0.0, 0.0, 0.0]));
                Vec3::from(v.coords())
            }
        };
        assert_approx_eq!(f32, world_centre.x, 40.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.y, 0.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.z, 0.0, epsilon = 1e-4);
        let m = read_linear(&new_local);
        assert_approx_eq!(f64, m[0][0], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[2][2], 2.0, epsilon = 1e-4);
    }

    #[test]
    fn rotate_under_translated_parent_keeps_world_centre_fixed() {
        // Sibling of the scale regression: the rotate composition
        // also goes through `compose_about_pivot`, so the same
        // parent-translation displacement would manifest as the
        // object orbiting around `(world_pivot + δ_root)` instead
        // of spinning in place. Verify that the rotated leaf's
        // world image of the mesh-local origin still sits at the
        // world pivot.
        let parent_world =
            Transform3D::Affine(AffineTransform3D::from_translation([40.0, 0.0, 0.0]));
        let original = Transform3D::Identity;
        let world_pivot = Vec3::new(40.0, 0.0, 0.0);
        // A quarter turn counter-clockwise on screen around the pivot.
        let op = rotate_op(Axis::Z, (100.0, 100.0), (200.0, 100.0));
        let ray = ray_from(Vec3::new(40.0, 1.0, 5.0), Vec3::new(40.0, 1.0, 0.0));
        let (new_local, _) = apply_drag_move(
            op,
            &ray,
            world_pivot,
            &parent_world,
            &original,
            (100.0, 0.0),
            NO_KEYS,
        )
        .unwrap();
        let world_centre = match parent_world.compose(&new_local) {
            Transform3D::Identity => Vec3::ZERO,
            Transform3D::Affine(a) => {
                let v = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([0.0, 0.0, 0.0]));
                Vec3::from(v.coords())
            }
        };
        assert_approx_eq!(f32, world_centre.x, 40.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.y, 0.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.z, 0.0, epsilon = 1e-4);
    }
}

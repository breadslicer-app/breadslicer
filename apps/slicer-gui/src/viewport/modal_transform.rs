/* SPDX-License-Identifier: AGPL-3.0-only */
//! Cursor-driven modal transform math, independent of the UI and document.
//! Ctrl snaps the gesture to steps and Shift slows it down (#275), as in the
//! gizmo drags.
use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;

use crate::gizmo::modifiers::{DragModifiers, PrecisionCursor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModalOp {
    Move,
    Rotate,
    Scale,
}
impl ModalOp {
    pub(super) fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Move),
            1 => Some(Self::Rotate),
            2 => Some(Self::Scale),
            _ => None,
        }
    }
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Move => "G",
            Self::Rotate => "R",
            Self::Scale => "S",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ModalGesture {
    pub node_id: NodeId,
    pub original: Transform3D,
    pub parent: Transform3D,
    pub pivot: Vec3,
    pub start: [f32; 2],
    pub cursor: [f32; 2],
    pub screen_pivot: [f32; 2],
    pub right: Vec3,
    pub up: Vec3,
    pub view_axis: Vec3,
    pub world_per_pixel: f32,
    pub op: ModalOp,
    pub axis: Option<usize>,
    pub last_angle: f32,
    pub angle: f32,
    /// The cursor the gesture reads: slowed down while Shift is held.
    pub precision: PrecisionCursor,
    /// The modifier keys of the last pointer event.
    pub modifiers: DragModifiers,
    /// The scale factor applied so far (1 until an S gesture moves).
    pub factor: f32,
}
impl ModalGesture {
    /// The turn an R gesture has applied so far, in radians, right-handed
    /// about [`Self::turn_axis`] (snapped as the rotation is).
    pub(super) fn applied_turn(&self) -> f32 {
        let axis = self.turn_axis();
        let sign = if axis.dot(self.view_axis) >= 0.0 {
            -1.0
        } else {
            1.0
        };
        self.modifiers.angle(self.angle) * sign
    }

    /// The axis an R gesture turns about: the locked axis, or the view
    /// direction (toward the eye) without one.
    pub(super) fn turn_axis(&self) -> Vec3 {
        self.axis.map(axis_vector).unwrap_or(self.view_axis)
    }

    /// The plane of the ring an R gesture shows, as two unit vectors (the
    /// turn grows from the first toward the second): the locked axis's ring,
    /// or the screen ring in the camera plane.
    pub(super) fn ring_basis(&self) -> (Vec3, Vec3) {
        match self.axis {
            Some(k) => crate::gizmo::axis::axis_basis(crate::gizmo::axis::ALL_AXES[k]),
            None => (self.right, self.up),
        }
    }

    /// Where on its ring an R gesture started, in the basis of
    /// [`Self::ring_basis`]: the direction from the pivot to the press on
    /// screen, laid onto the ring's plane.
    pub(super) fn ring_start(&self) -> f32 {
        let (u, v) = self.ring_basis();
        let on_screen = self.right * (self.start[0] - self.screen_pivot[0])
            - self.up * (self.start[1] - self.screen_pivot[1]);
        let n = self.turn_axis();
        let d = on_screen - n * on_screen.dot(n);
        if d.length() < 1e-6 {
            return 0.0;
        }
        d.dot(v).atan2(d.dot(u))
    }

    pub(super) fn world_translation(&self, local: Transform3D, delta: Vec3) -> Option<Transform3D> {
        let shift = Transform3D::Affine(AffineTransform3D::from_translation(
            delta.to_array().map(f64::from),
        ));
        Some(
            self.parent
                .inverse()
                .ok()?
                .compose(&shift.compose(&self.parent.compose(&local))),
        )
    }
    fn about_pivot(&self, delta: AffineTransform3D) -> Option<Transform3D> {
        let to = Transform3D::Affine(AffineTransform3D::from_translation(
            self.pivot.to_array().map(f64::from),
        ));
        let from = Transform3D::Affine(AffineTransform3D::from_translation(
            (-self.pivot).to_array().map(f64::from),
        ));
        let world = to.compose(
            &Transform3D::Affine(delta)
                .compose(&from.compose(&self.parent.compose(&self.original))),
        );
        Some(self.parent.inverse().ok()?.compose(&world))
    }
    /// The transform for the real cursor at `cursor` with `modifiers` held.
    pub(super) fn propose(
        &mut self,
        cursor: [f32; 2],
        modifiers: DragModifiers,
    ) -> Option<Transform3D> {
        if !cursor.into_iter().all(f32::is_finite) {
            return None;
        }
        self.cursor = cursor;
        self.modifiers = modifiers;
        let cursor = self.precision.map(cursor, modifiers.fine);
        let dx = cursor[0] - self.start[0];
        let dy = cursor[1] - self.start[1];
        match self.op {
            ModalOp::Move => {
                let delta = if let Some(axis) = self.axis {
                    let unit = axis_vector(axis);
                    let sx = unit.dot(self.right);
                    let sy = -unit.dot(self.up);
                    let norm = sx * sx + sy * sy;
                    let distance = if norm > 0.02 {
                        (dx * sx + dy * sy) / norm
                    } else {
                        -dy
                    };
                    unit * modifiers.distance(distance * self.world_per_pixel)
                } else {
                    ((self.right * dx - self.up * dy) * self.world_per_pixel)
                        .to_array()
                        .map(|c| modifiers.distance(c))
                        .into()
                };
                self.world_translation(self.original, delta)
            }
            ModalOp::Scale => {
                let distance = |p: [f32; 2]| {
                    ((p[0] - self.screen_pivot[0]).powi(2) + (p[1] - self.screen_pivot[1]).powi(2))
                        .sqrt()
                };
                let radius = distance(self.start);
                let factor = if radius >= 8.0 {
                    distance(cursor) / radius
                } else {
                    ((dx - dy) * 0.01).exp()
                }
                .clamp(0.001, 1000.0);
                let factor = modifiers.factor(factor);
                self.factor = factor;
                let scale = if let Some(axis) = self.axis {
                    let mut factors = [1.0; 3];
                    factors[axis] = f64::from(factor);
                    AffineTransform3D::from_non_uniform_scale(factors)
                } else {
                    AffineTransform3D::from_uniform_scale(f64::from(factor))
                };
                self.about_pivot(scale)
            }
            ModalOp::Rotate => {
                let sx = self.start[0] - self.screen_pivot[0];
                let sy = self.start[1] - self.screen_pivot[1];
                if sx * sx + sy * sy >= 64.0 {
                    let raw =
                        (cursor[1] - self.screen_pivot[1]).atan2(cursor[0] - self.screen_pivot[0]);
                    let change = (raw - self.last_angle + std::f32::consts::PI)
                        .rem_euclid(std::f32::consts::TAU)
                        - std::f32::consts::PI;
                    self.angle += change;
                    self.last_angle = raw;
                } else {
                    self.angle = dx * 0.01;
                }
                let rotation = AffineTransform3D::from_axis_angle(
                    self.turn_axis().to_array().map(f64::from),
                    f64::from(self.applied_turn()),
                )
                .ok()?;
                self.about_pivot(rotation)
            }
        }
    }
}
fn axis_vector(axis: usize) -> Vec3 {
    match axis {
        0 => Vec3::X,
        1 => Vec3::Y,
        _ => Vec3::Z,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_mesh_core::Vertex;
    use sc_scene::{LeafPayload, Scene};
    use std::sync::Arc;
    fn gesture(op: ModalOp) -> ModalGesture {
        let mut scene = Scene::new();
        let id = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(sc_mesh_core::IndexedMesh {
                    meta: None,
                    vertices: vec![],
                    faces: vec![],
                })),
            )
            .unwrap();
        ModalGesture {
            node_id: id,
            original: Transform3D::Identity,
            parent: Transform3D::Identity,
            pivot: Vec3::ZERO,
            start: [100.0, 0.0],
            cursor: [100.0, 0.0],
            screen_pivot: [0.0, 0.0],
            right: Vec3::X,
            up: Vec3::Y,
            view_axis: -Vec3::Z,
            world_per_pixel: 1.0,
            op,
            axis: None,
            last_angle: 0.0,
            angle: 0.0,
            precision: PrecisionCursor::new([100.0, 0.0], false),
            modifiers: DragModifiers::default(),
            factor: 1.0,
        }
    }

    const NO_KEYS: DragModifiers = DragModifiers {
        snap: false,
        fine: false,
    };
    const CTRL: DragModifiers = DragModifiers {
        snap: true,
        fine: false,
    };
    const SHIFT: DragModifiers = DragModifiers {
        snap: false,
        fine: true,
    };
    fn point(t: Transform3D, p: [f32; 3]) -> [f32; 3] {
        t.apply_to_vertex(Vertex::new(p)).coords()
    }
    #[test]
    fn move_restricts_to_each_world_axis_including_end_on_view() {
        for axis in 0..3 {
            let mut g = gesture(ModalOp::Move);
            g.axis = Some(axis);
            let p = point(g.propose([130.0, 20.0], NO_KEYS).unwrap(), [0.0; 3]);
            for (i, value) in p.iter().enumerate() {
                if i != axis {
                    assert_eq!(*value, 0.0);
                } else {
                    assert!(value.abs() > 0.0);
                }
            }
        }
    }
    #[test]
    fn uniform_and_axis_scale() {
        let mut g = gesture(ModalOp::Scale);
        assert_eq!(
            point(g.propose([200.0, 0.0], NO_KEYS).unwrap(), [1.0; 3]),
            [2.0; 3]
        );
        for axis in 0..3 {
            g.axis = Some(axis);
            let p = point(g.propose([200.0, 0.0], NO_KEYS).unwrap(), [1.0; 3]);
            for (i, v) in p.iter().enumerate() {
                assert_eq!(*v, if i == axis { 2.0 } else { 1.0 });
            }
        }
    }
    #[test]
    fn rotation_fixes_locked_axis() {
        for axis in 0..3 {
            let mut g = gesture(ModalOp::Rotate);
            g.axis = Some(axis);
            let p = axis_vector(axis).to_array();
            let actual = point(g.propose([0.0, 100.0], NO_KEYS).unwrap(), p);
            for i in 0..3 {
                assert!((p[i] - actual[i]).abs() < 1e-5);
            }
        }
    }
    #[test]
    fn previews_compose_from_original_not_previous_frame() {
        let mut g = gesture(ModalOp::Move);
        g.axis = Some(0);
        g.propose([130.0, 0.0], NO_KEYS);
        let p = point(g.propose([140.0, 0.0], NO_KEYS).unwrap(), [0.0; 3]);
        assert_eq!(p, [40.0, 0.0, 0.0]);
    }
    #[test]
    fn axis_switch_discards_unwanted_components_without_resetting_origin() {
        let mut g = gesture(ModalOp::Move);
        g.propose([130.0, 20.0], NO_KEYS);
        g.axis = Some(1);
        assert_eq!(
            point(g.propose([130.0, 20.0], NO_KEYS).unwrap(), [0.0; 3]),
            [0.0, -20.0, 0.0]
        );
    }
    #[test]
    fn world_constraint_respects_rotated_parent() {
        let mut g = gesture(ModalOp::Move);
        g.parent = Transform3D::Affine(AffineTransform3D::from_rotation_z(0.7));
        g.axis = Some(0);
        let local = g.propose([130.0, 20.0], NO_KEYS).unwrap();
        let p = point(g.parent.compose(&local), [0.0; 3]);
        assert!((p[0] - 30.0).abs() < 1e-5 && p[1].abs() < 1e-5);
    }
    #[test]
    fn rotation_unwraps_across_pi() {
        let mut g = gesture(ModalOp::Rotate);
        g.propose([-100.0, 1.0], NO_KEYS);
        let first = g.angle;
        g.propose([-100.0, -1.0], NO_KEYS);
        assert!((g.angle - first).abs() < 0.03);
    }
    #[test]
    fn scale_keeps_pivot_fixed() {
        let mut g = gesture(ModalOp::Scale);
        g.pivot = Vec3::new(10.0, 20.0, 30.0);
        let p = point(
            g.propose([200.0, 0.0], NO_KEYS).unwrap(),
            g.pivot.to_array(),
        );
        assert_eq!(p, g.pivot.to_array());
    }

    #[test]
    fn ctrl_snaps_a_move_a_turn_and_a_scale() {
        let mut g = gesture(ModalOp::Move);
        g.axis = Some(0);
        let p = point(g.propose([133.7, 0.0], CTRL).unwrap(), [0.0; 3]);
        assert_eq!(p, [34.0, 0.0, 0.0]);
        // A free move snaps each world axis.
        g.axis = None;
        let p = point(g.propose([112.4, -7.6], CTRL).unwrap(), [0.0; 3]);
        assert_eq!(p, [12.0, 8.0, 0.0]);

        let mut g = gesture(ModalOp::Scale);
        let p = point(g.propose([173.0, 0.0], CTRL).unwrap(), [1.0; 3]);
        for v in p {
            assert!((v - 1.7).abs() < 1e-5, "{v}");
        }

        let mut g = gesture(ModalOp::Rotate);
        g.axis = Some(2);
        // From +X around the screen pivot to 20° off it: snaps to 15°.
        let a = 20f32.to_radians();
        let turned = g.propose([100.0 * a.cos(), 100.0 * a.sin()], CTRL).unwrap();
        let x = point(turned, [1.0, 0.0, 0.0]);
        let angle = x[1].atan2(x[0]).abs().to_degrees();
        assert!((angle - 15.0).abs() < 1e-3, "{angle}");
    }

    #[test]
    fn shift_moves_a_tenth_as_far_and_never_jumps() {
        let mut g = gesture(ModalOp::Move);
        g.axis = Some(0);
        assert_eq!(
            point(g.propose([150.0, 0.0], NO_KEYS).unwrap(), [0.0; 3]),
            [50.0, 0.0, 0.0]
        );
        // Shift goes down at x = 150: nothing moves, then a tenth of the travel.
        assert_eq!(
            point(g.propose([150.0, 0.0], SHIFT).unwrap(), [0.0; 3]),
            [50.0, 0.0, 0.0]
        );
        let p = point(g.propose([250.0, 0.0], SHIFT).unwrap(), [0.0; 3]);
        assert!((p[0] - 60.0).abs() < 1e-4, "{p:?}");
    }
}

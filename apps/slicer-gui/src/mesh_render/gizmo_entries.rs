/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::camera::OrbitCamera;
use crate::gizmo::axis::Axis;

/// Per-frame uniform for the gizmo pass: the camera transform plus
/// the world-space pivot and shaft-length, both packed into a vec4
/// so the shader gets one contiguous load.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(super) struct GizmoUniformsGpu {
    pub(super) view_proj: [[f32; 4]; 4],
    pub(super) pivot_and_scale: [f32; 4],
}

/// Per-axis draw entry uploaded to the dynamic-offset uniform buffer.
/// Mirrors `DrawEntry` in `gizmo.wgsl` one-for-one.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(super) struct GizmoDrawEntry {
    axis_rotation: [[f32; 4]; 4],
    tint: [f32; 4],
}

pub(super) const GIZMO_DRAW_ENTRY_SIZE: u64 = std::mem::size_of::<GizmoDrawEntry>() as u64;

/// Total number of per-frame draw entries reserved in
/// `gizmo_entries_buffer`. See `GIZMO_SLOTS` for the per-category
/// layout that fills it.
pub(super) const GIZMO_ENTRY_SLOTS: u64 = 9;

/// Range of `gizmo_entries_buffer` slots reserved for one handle
/// category. `start` is the first slot index, `count` is how many
/// consecutive slots belong to the category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct GizmoSlotRange {
    start: u64,
    count: u64,
}

impl GizmoSlotRange {
    pub(super) const fn new(start: u64, count: u64) -> Self {
        Self { start, count }
    }
    pub(super) fn start(self) -> u64 {
        self.start
    }
    #[cfg(test)]
    pub(super) fn count(self) -> u64 {
        self.count
    }
    pub(super) fn slot(self, axis_ix: u64) -> u64 {
        debug_assert!(axis_ix < self.count);
        self.start + axis_ix
    }
}

/// Static layout of `gizmo_entries_buffer`. Slots are contiguous and
/// non-overlapping; the layout is a compile-time constant so the
/// renderer never queries it at runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct GizmoSlots {
    pub(super) translate: GizmoSlotRange,
    pub(super) rotate: GizmoSlotRange,
    pub(super) scale_axis: GizmoSlotRange,
}

pub(super) const GIZMO_SLOTS: GizmoSlots = GizmoSlots {
    translate: GizmoSlotRange::new(0, 3),
    rotate: GizmoSlotRange::new(3, 3),
    scale_axis: GizmoSlotRange::new(6, 3),
};

/// Per-frame data the renderer needs to draw the gizmo. `pivot` is
/// the world-space gizmo origin; `scale` is the world-unit length of
/// the translate shaft (so the gizmo subtends a constant pixel size);
/// `visibility` tells the renderer which handle types to draw and
/// which axis (if any) is the currently-active drag target — handles
/// matching the active axis render at full brightness, the rest at
/// dim brightness. `locked_translate` / `locked_rotate` flag axes the
/// user can't grab right now (matching `gizmo::axis_pickable` and
/// `gizmo::arc_pickable` respectively); locked axes render greyed.
#[derive(Copy, Clone, Debug)]
pub struct GizmoFrame {
    pub pivot: Vec3,
    pub scale: f32,
    pub visibility: GizmoVisibility,
    /// Indexed [X, Y, Z]. Greys the translate cone *and* the per-axis
    /// scale midpoint cube for that axis (both share the closest-
    /// point-on-axis math and thus the same lock-out condition).
    pub locked_translate: [bool; 3],
    /// Indexed [X, Y, Z]. Greys the rotate arc for that axis.
    pub locked_rotate: [bool; 3],
    /// Handle the cursor is currently hovering over (no button held),
    /// or `None` when the cursor is off any handle. Renders that
    /// handle at hover brightness — between inactive and active.
    /// Ignored while `visibility` is `Dragging*` (the dragged handle
    /// is already at full brightness).
    pub hover: Option<GizmoHover>,
    /// Live drag visual. Drives the per-frame override that slides
    /// the active scale cube along its axis (so the cube tracks the
    /// cursor) or rolls the active rotate arc to the cursor angle
    /// (so the cones at the arc midpoint follow the cursor). `None`
    /// when no drag is in progress; carries the dragged axis plus
    /// the live cursor-driven value when a drag is active.
    pub drag_visual: Option<GizmoDragVisual>,
}

/// Live cursor-driven drag state, used by the renderer to override
/// the per-axis transform of the active handle so the gizmo provides
/// real-time visual feedback during a drag.
#[derive(Copy, Clone, Debug)]
pub enum GizmoDragVisual {
    /// Per-axis scale: signed distance from the pivot to the cursor's
    /// closest-point-on-axis, in world units. The renderer slides the
    /// `axis` cube along the axis to that distance.
    ScaleAxis { axis: Axis, axial_distance: f32 },
    /// Rotate: signed angle of the cursor in the axis-perpendicular
    /// plane, measured from the handle-local +Y reference (matches
    /// `gizmo::drag_math::rotate_drag_angle`). The renderer rolls
    /// the `axis` arc + cone-spindle to that angle so the cones
    /// follow the cursor.
    Rotate { axis: Axis, angle: f32 },
}

/// Which handle the pointer is currently over, in `Idle` visibility.
/// Used by the renderer to apply the hover-tint level on the matching
/// axis without disturbing the active/inactive layout.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GizmoHover {
    Translate(Axis),
    Rotate(Axis),
    ScaleAxis(Axis),
}

/// Which gizmo handle category is currently visible (and which axis,
/// if any, is the active drag target). The rule:
/// - `Idle`: render all three handle categories (translate cones,
///   rotate arcs, midpoint scale cubes). No axis is highlighted as
///   "active".
/// - `Dragging*`: render only the *category* the drag belongs to.
///   The named axis is the active drag target and renders bright;
///   other axes in the same category render dim.
//
// `DraggingRotate` / `DraggingScaleAxis` have no controller-side
// producer until the FSM extension lands; they're consumed by tests
// today.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GizmoVisibility {
    Idle,
    DraggingTranslate { axis: Axis },
    DraggingRotate { axis: Axis },
    DraggingScaleAxis { axis: Axis },
}

/// Translate / scale per-axis colours (full saturation). The renderer
/// modulates these with a brightness factor that depends on the axis's
/// active/locked state for the current handle category.
const fn axis_base_color(axis: Axis) -> [f32; 3] {
    match axis {
        Axis::X => [1.0, 0.25, 0.25],
        Axis::Y => [0.25, 1.0, 0.25],
        Axis::Z => [0.35, 0.45, 1.0],
    }
}

/// Rotation per-axis colours: complementaries of the translate
/// palette so the rotate arcs and their cone spindles read as a
/// distinct family from the translate shafts and the scale cubes.
/// X-rotate / cyan, Y-rotate / magenta, Z-rotate / yellow-orange.
const fn rotate_base_color(axis: Axis) -> [f32; 3] {
    match axis {
        Axis::X => [0.25, 0.85, 1.0],
        Axis::Y => [1.0, 0.35, 0.85],
        Axis::Z => [1.0, 0.85, 0.25],
    }
}

/// Brightness multipliers for the four handle states. Active is the
/// drag-target axis at full brightness; hover is the cursor-target axis
/// at midway between inactive and active; inactive axes still render
/// visibly so the user can see the gizmo at reduced contrast; locked
/// axes render in a fixed grey.
const TINT_ACTIVE: f32 = 1.0;
const TINT_HOVER: f32 = (TINT_ACTIVE + TINT_INACTIVE) * 0.5;
const TINT_INACTIVE: f32 = 0.65;
const TINT_LOCKED: f32 = 0.35;

fn tint_with(base: [f32; 3], active: bool, hover: bool, locked: bool) -> [f32; 4] {
    if locked {
        return [TINT_LOCKED, TINT_LOCKED, TINT_LOCKED, 1.0];
    }
    let brightness = if active {
        TINT_ACTIVE
    } else if hover {
        TINT_HOVER
    } else {
        TINT_INACTIVE
    };
    let [r, g, b] = base;
    [r * brightness, g * brightness, b * brightness, 1.0]
}

fn tint_for(axis: Axis, active: bool, hover: bool, locked: bool) -> [f32; 4] {
    tint_with(axis_base_color(axis), active, hover, locked)
}

fn rotate_tint_for(axis: Axis, active: bool, hover: bool, locked: bool) -> [f32; 4] {
    tint_with(rotate_base_color(axis), active, hover, locked)
}

/// Rotation that maps handle-local +X onto each world axis. Indexed
/// in the order `[X, Y, Z]`.
fn axis_world_rotations() -> [Mat4; 3] {
    use std::f32::consts::FRAC_PI_2;
    [
        Mat4::IDENTITY,
        Mat4::from_rotation_z(FRAC_PI_2),
        Mat4::from_rotation_y(-FRAC_PI_2),
    ]
}

/// Three translate-pass entries (X/Y/Z), one shaft+cone draw each.
pub(super) fn build_translate_entries(frame: &GizmoFrame) -> [GizmoDrawEntry; 3] {
    let rotations = axis_world_rotations();
    let axes = [Axis::X, Axis::Y, Axis::Z];
    let active_axis = match frame.visibility {
        GizmoVisibility::DraggingTranslate { axis } => Some(axis),
        _ => None,
    };
    let hover_axis = match frame.hover {
        Some(GizmoHover::Translate(axis)) => Some(axis),
        _ => None,
    };
    let mut out = [GizmoDrawEntry {
        axis_rotation: [[0.0; 4]; 4],
        tint: [0.0; 4],
    }; 3];
    for (i, axis) in axes.into_iter().enumerate() {
        let active = active_axis == Some(axis);
        let hover = hover_axis == Some(axis);
        out[i] = GizmoDrawEntry {
            axis_rotation: rotations[i].to_cols_array_2d(),
            tint: tint_for(axis, active, hover, frame.locked_translate[i]),
        };
    }
    out
}

/// Three scale-axis-pass entries (X/Y/Z midpoint cubes), sharing
/// the per-axis world rotation with translate. The cube sub-range
/// of the axis IBO carries the geometry; only the tint differs from
/// the translate entry. The scale-axis lock-out is the same as
/// translate's (closest-point-on-axis math).
///
/// When a `ScaleAxis` drag is live, the dragged axis's matrix bakes
/// in an additional handle-local +X translation that slides the cube
/// from its default mid-shaft position to the cursor's projection on
/// the axis, so the cube tracks the cursor as visual feedback.
pub(super) fn build_scale_axis_entries(frame: &GizmoFrame) -> [GizmoDrawEntry; 3] {
    let rotations = axis_world_rotations();
    let axes = [Axis::X, Axis::Y, Axis::Z];
    let active_axis = match frame.visibility {
        GizmoVisibility::DraggingScaleAxis { axis } => Some(axis),
        _ => None,
    };
    let hover_axis = match frame.hover {
        Some(GizmoHover::ScaleAxis(axis)) => Some(axis),
        _ => None,
    };
    let drag = match frame.drag_visual {
        Some(GizmoDragVisual::ScaleAxis {
            axis,
            axial_distance,
        }) => Some((axis, axial_distance)),
        _ => None,
    };
    let mut out = [GizmoDrawEntry {
        axis_rotation: [[0.0; 4]; 4],
        tint: [0.0; 4],
    }; 3];
    for (i, axis) in axes.into_iter().enumerate() {
        let active = active_axis == Some(axis);
        let hover = hover_axis == Some(axis);
        // World-units offset along the handle-local +X direction.
        // The cube's default vertex sits at handle-local
        // `(MIDPOINT_FRACTION, 0, 0)` and is then scaled by
        // `pivot_and_scale.w` (= world shaft length); to put the
        // cube centre at world signed distance `d` from the pivot,
        // shift by `(d - MIDPOINT_FRACTION * scale)` in the post-
        // scale, pre-rotation handle-local frame. The per-frame
        // shaft length is in `frame.scale` so we can compute the
        // shift CPU-side and bake it into the matrix.
        let translation_x = match drag {
            Some((dragged, distance)) if dragged == axis => {
                distance - frame.scale * MIDPOINT_FRACTION_LOCAL
            }
            _ => 0.0,
        };
        let translated = if translation_x.abs() > 0.0 {
            rotations[i] * Mat4::from_translation(Vec3::new(translation_x, 0.0, 0.0))
        } else {
            rotations[i]
        };
        out[i] = GizmoDrawEntry {
            axis_rotation: translated.to_cols_array_2d(),
            tint: tint_for(axis, active, hover, frame.locked_translate[i]),
        };
    }
    out
}

/// Mirror of `gizmo::dimensions::MIDPOINT_FRACTION` — kept here as a
/// private const so `build_scale_axis_entries` doesn't need a fresh
/// import path through the `gizmo` module.
const MIDPOINT_FRACTION_LOCAL: f32 = crate::gizmo::dimensions::MIDPOINT_FRACTION;

/// Per-axis offset between the rotate-drag angle frame and the X-roll
/// frame (CCW about the rotation axis):
/// - **X** — drag frame's 0° = world +Y; arc midpoint's 0° (after
///   `axis_world_rotations[X]` = identity) = world +Y → offset 0.
/// - **Y** — drag frame's 0° = world +Z; arc midpoint's 0° (after
///   `Rz(+π/2)`) = world -X → offset +π/2.
/// - **Z** — drag frame's 0° = world +X; arc midpoint's 0° (after
///   `Ry(-π/2)`) = world +Y → offset −π/2.
fn arc_drag_to_roll_offset(axis: Axis) -> f32 {
    use std::f32::consts::FRAC_PI_2;
    match axis {
        Axis::X => 0.0,
        Axis::Y => FRAC_PI_2,
        Axis::Z => -FRAC_PI_2,
    }
}

/// Three rotate-pass entries (X/Y/Z arcs). Each axis bakes a
/// per-frame X-roll into its `axis_rotation` so the 90° arc faces
/// the camera in handle-local space *before* the world-axis rotation
/// maps it onto +X / +Y / +Z.
pub(super) fn build_rotate_entries(
    frame: &GizmoFrame,
    camera: &OrbitCamera,
) -> [GizmoDrawEntry; 3] {
    let rotations = axis_world_rotations();
    let axes = [Axis::X, Axis::Y, Axis::Z];
    let active_axis = match frame.visibility {
        GizmoVisibility::DraggingRotate { axis } => Some(axis),
        _ => None,
    };
    let hover_axis = match frame.hover {
        Some(GizmoHover::Rotate(axis)) => Some(axis),
        _ => None,
    };
    let drag = match frame.drag_visual {
        Some(GizmoDragVisual::Rotate { axis, angle }) => Some((axis, angle)),
        _ => None,
    };
    let mut out = [GizmoDrawEntry {
        axis_rotation: [[0.0; 4]; 4],
        tint: [0.0; 4],
    }; 3];
    for (i, axis) in axes.into_iter().enumerate() {
        // Default roll faces the camera; for the dragged axis,
        // roll to the live cursor angle instead so the cone-spindle
        // (anchored at the arc midpoint, handle-local +Y) tracks
        // the cursor and the arc sweeps with the gesture.
        //
        // `rotate_drag_angle` measures the cursor angle in a
        // *world-frame* basis whose 0° reference depends on the
        // axis (`axis_basis`: X→+Y, Y→+Z, Z→+X). The handle-local
        // arc midpoint, by contrast, is at handle-local +Y, which
        // `axis_world_rotations` maps to world +Y / -X / +Y for
        // X / Y / Z. Subtracting the handle midpoint's drag-frame
        // angle from `last_raw` aligns the two reference frames so
        // the cones land under the cursor on every axis.
        let roll = match drag {
            Some((dragged, angle)) if dragged == axis => angle + arc_drag_to_roll_offset(axis),
            _ => arc_camera_roll(camera, frame.pivot, axis),
        };
        // Compose: world_axis_rotation × X-roll. The arc is laid out
        // around handle-local +X, centred on +Y; rolling it around
        // +X swings the centre onto the camera-facing direction
        // before the world-axis rotation lifts it onto X/Y/Z.
        let composed = rotations[i] * Mat4::from_rotation_x(roll);
        let active = active_axis == Some(axis);
        let hover = hover_axis == Some(axis);
        out[i] = GizmoDrawEntry {
            axis_rotation: composed.to_cols_array_2d(),
            tint: rotate_tint_for(axis, active, hover, frame.locked_rotate[i]),
        };
    }
    out
}

/// Per-axis X-roll in handle-local space that swings the rotate arc
/// onto the camera-facing 90° quadrant.
///
/// In handle-local space the arc is centred on +Y, spanning ±π/4. We
/// want the centre of the arc to align with the camera direction
/// projected into the handle-local YZ plane. The roll is the angle
/// from +Y to that projection, measured CCW about +X (right-hand
/// rule). When the camera is on the axis itself (rotate lock-out),
/// the projection is degenerate; the controller's lock-out should
/// already have removed the axis from the visible set, but the
/// geometry guards against div-by-zero with a fallback to `0.0`.
fn arc_camera_roll(camera: &OrbitCamera, pivot: Vec3, axis: Axis) -> f32 {
    // Camera direction in world space, then expressed in handle-
    // local coords by inverting the world-axis rotation. The
    // rotation matrix is orthogonal, so the inverse is the
    // transpose — but glam's `inverse` on rigid rotations is cheap
    // enough to avoid premature optimisation here.
    let world_to_local = world_to_handle_local(axis);
    let to_camera_world = camera.eye() - pivot;
    let to_camera_local = world_to_local.transform_vector3(to_camera_world);
    // YZ-plane projection (drop +X axis component).
    let y = to_camera_local.y;
    let z = to_camera_local.z;
    if y.hypot(z) < 1e-6 {
        return 0.0;
    }
    z.atan2(y)
}

/// Inverse of the per-axis world rotation: maps world-space vectors
/// into the axis's handle-local frame. The world rotation maps
/// handle-local +X onto the world axis; the inverse undoes that.
fn world_to_handle_local(axis: Axis) -> Mat4 {
    use std::f32::consts::FRAC_PI_2;
    match axis {
        Axis::X => Mat4::IDENTITY,
        // World rotation is Rz(+π/2); inverse is Rz(-π/2).
        Axis::Y => Mat4::from_rotation_z(-FRAC_PI_2),
        // World rotation is Ry(-π/2); inverse is Ry(+π/2).
        Axis::Z => Mat4::from_rotation_y(FRAC_PI_2),
    }
}

#[cfg(test)]
mod gizmo_entry_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use glam::Vec4;

    fn frame_with(visibility: GizmoVisibility, locked_translate: [bool; 3]) -> GizmoFrame {
        GizmoFrame {
            pivot: Vec3::ZERO,
            scale: 1.0,
            visibility,
            locked_translate,
            locked_rotate: [false; 3],
            hover: None,
            drag_visual: None,
        }
    }

    fn idle_frame() -> GizmoFrame {
        frame_with(GizmoVisibility::Idle, [false; 3])
    }

    #[test]
    fn translate_axis_rotations_map_handle_local_x_to_world_axes() {
        let entries = build_translate_entries(&idle_frame());
        let local_x = Vec4::new(1.0, 0.0, 0.0, 1.0);
        let rx = Mat4::from_cols_array_2d(&entries[0].axis_rotation) * local_x;
        let ry = Mat4::from_cols_array_2d(&entries[1].axis_rotation) * local_x;
        let rz = Mat4::from_cols_array_2d(&entries[2].axis_rotation) * local_x;
        assert_approx_eq!(f32, rx.x, 1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, ry.y, 1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, rz.z, 1.0, epsilon = 1e-5);
    }

    #[test]
    fn idle_visibility_yields_inactive_brightness_for_all_translate_axes() {
        let entries = build_translate_entries(&idle_frame());
        // Inactive brightness multiplier is 0.65; X base is (1.0, 0.25, 0.25).
        assert_approx_eq!(f32, entries[0].tint[0], 0.65, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[0].tint[1], 0.25 * 0.65, epsilon = 1e-5);
    }

    #[test]
    fn dragging_translate_renders_active_axis_at_full_brightness() {
        let entries = build_translate_entries(&frame_with(
            GizmoVisibility::DraggingTranslate { axis: Axis::X },
            [false; 3],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 1.0, epsilon = 1e-5);
        // Y stays inactive — only the dragging axis brightens.
        assert_approx_eq!(f32, entries[1].tint[1], 1.0 * 0.65, epsilon = 1e-5);
    }

    #[test]
    fn locked_axis_renders_grey_regardless_of_active_state() {
        let entries = build_translate_entries(&frame_with(
            GizmoVisibility::DraggingTranslate { axis: Axis::X },
            [true, false, false],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 0.35, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[0].tint[1], 0.35, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[0].tint[2], 0.35, epsilon = 1e-5);
    }

    #[test]
    fn dragging_rotate_does_not_brighten_translate_axes() {
        // Visibility carries the active *for the rotate category*;
        // the translate entries should see no active axis even
        // though `axis: X` is named.
        let entries = build_translate_entries(&frame_with(
            GizmoVisibility::DraggingRotate { axis: Axis::X },
            [false; 3],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 0.65, epsilon = 1e-5);
    }

    #[test]
    fn dragging_scale_axis_brightens_only_the_matching_midpoint_cube() {
        let entries = build_scale_axis_entries(&frame_with(
            GizmoVisibility::DraggingScaleAxis { axis: Axis::Y },
            [false; 3],
        ));
        // Y axis is active.
        assert_approx_eq!(f32, entries[1].tint[1], 1.0, epsilon = 1e-5);
        // X / Z stay inactive.
        assert_approx_eq!(f32, entries[0].tint[0], 0.65, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[2].tint[2], 0.65, epsilon = 1e-5);
    }

    #[test]
    fn scale_axis_inherits_translate_lockout() {
        // The midpoint cubes share the closest-point-on-axis math
        // with the translate cones, so they must grey together.
        let entries = build_scale_axis_entries(&frame_with(
            GizmoVisibility::DraggingScaleAxis { axis: Axis::X },
            [true, false, false],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 0.35, epsilon = 1e-5);
    }

    #[test]
    fn rotate_arc_roll_aligns_with_camera_in_handle_local_yz_plane() {
        // Camera at +Z above the pivot. For the X-axis arc (axis = +X
        // in handle-local space), the camera-facing direction projects
        // onto the YZ plane as +Z. The roll should bring +Y onto +Z,
        // i.e. +π/2 about +X. Loose tolerance because `set_axis_view`
        // uses a clamped pitch slightly less than π/2 to avoid the
        // gimbal-lock degenerate, so the eye sits ~1e-3 off the +Z
        // axis.
        let mut camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        camera.set_axis_view(crate::camera::AxisView::Top);
        let pivot = Vec3::ZERO;
        let roll = arc_camera_roll(&camera, pivot, Axis::X);
        assert_approx_eq!(f32, roll, std::f32::consts::FRAC_PI_2, epsilon = 1e-2);
    }

    #[test]
    fn rotate_locked_axis_renders_grey() {
        let mut frame = idle_frame();
        frame.visibility = GizmoVisibility::DraggingRotate { axis: Axis::X };
        frame.locked_rotate = [true, false, false];
        let camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        let entries = build_rotate_entries(&frame, &camera);
        assert_approx_eq!(f32, entries[0].tint[0], 0.35, epsilon = 1e-5);
    }

    #[test]
    fn rotate_palette_is_distinct_from_translate_palette() {
        let camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        let translate = build_translate_entries(&idle_frame());
        let rotate = build_rotate_entries(&idle_frame(), &camera);
        // For each axis, at least one channel must differ — the
        // rotate arcs and cones must not visually share a colour
        // with the translate shafts/cones for the same axis.
        for i in 0..3 {
            let t = translate[i].tint;
            let r = rotate[i].tint;
            let differs = (t[0] - r[0]).abs() > 1e-3
                || (t[1] - r[1]).abs() > 1e-3
                || (t[2] - r[2]).abs() > 1e-3;
            assert!(differs, "axis {i} shares its translate tint with rotate");
        }
    }

    #[test]
    fn hover_brightness_sits_between_inactive_and_active() {
        // X translate base = (1.0, 0.25, 0.25). At hover brightness
        // the red channel = TINT_HOVER (0.825).
        let mut frame = idle_frame();
        frame.hover = Some(GizmoHover::Translate(Axis::X));
        let entries = build_translate_entries(&frame);
        assert_approx_eq!(f32, entries[0].tint[0], TINT_HOVER, epsilon = 1e-5);
        // Y stays inactive — hover is per-axis.
        assert_approx_eq!(f32, entries[1].tint[1], TINT_INACTIVE, epsilon = 1e-5);
        // Hover sits strictly between inactive and active.
        const _: () = {
            assert!(TINT_INACTIVE < TINT_HOVER);
            assert!(TINT_HOVER < TINT_ACTIVE);
        };
    }

    #[test]
    fn hover_does_not_brighten_handle_in_a_different_category() {
        // Hover marks a translate axis; rotate / scale-axis entries
        // for the same axis must stay inactive.
        let mut frame = idle_frame();
        frame.hover = Some(GizmoHover::Translate(Axis::X));
        let camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        let rotate = build_rotate_entries(&frame, &camera);
        let scale = build_scale_axis_entries(&frame);
        // Rotate X (cyan: 0.25, 0.85, 1.0). Inactive blue channel = 0.65.
        assert_approx_eq!(f32, rotate[0].tint[2], 1.0 * TINT_INACTIVE, epsilon = 1e-5);
        // Scale X red channel inactive = 0.65.
        assert_approx_eq!(f32, scale[0].tint[0], 1.0 * TINT_INACTIVE, epsilon = 1e-5);
    }

    #[test]
    fn locked_axis_ignores_hover() {
        let mut frame = idle_frame();
        frame.locked_translate = [true, false, false];
        frame.hover = Some(GizmoHover::Translate(Axis::X));
        let entries = build_translate_entries(&frame);
        // Locked grey overrides hover.
        assert_approx_eq!(f32, entries[0].tint[0], TINT_LOCKED, epsilon = 1e-5);
    }

    #[test]
    fn scale_axis_drag_slides_cube_along_axis() {
        // Frame.scale = 1.0; default cube centre sits at handle-local
        // x = MIDPOINT_FRACTION (0.5), so the entry's matrix should
        // bake `axial_distance - 0.5` into a +X translation when the
        // dragged axis matches.
        let mut frame = frame_with(
            GizmoVisibility::DraggingScaleAxis { axis: Axis::X },
            [false; 3],
        );
        frame.drag_visual = Some(GizmoDragVisual::ScaleAxis {
            axis: Axis::X,
            axial_distance: 1.5,
        });
        let entries = build_scale_axis_entries(&frame);
        let m = Mat4::from_cols_array_2d(&entries[0].axis_rotation);
        // Translation column for handle-local +X axis: m * (0, 0, 0, 1)
        // = m's translation = the column-major last column.
        let t = m.col(3);
        // 1.5 - 0.5 = 1.0.
        assert_approx_eq!(f32, t.x, 1.0, epsilon = 1e-5);
        // Y / Z untouched: translation column (xyz) stays zero.
        let m_y = Mat4::from_cols_array_2d(&entries[1].axis_rotation);
        let ty = m_y.col(3);
        assert_approx_eq!(f32, ty.x, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, ty.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, ty.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn rotate_drag_places_cone_spindle_under_cursor_on_every_axis() {
        // For every axis, the cone-spindle midpoint (handle-local
        // (0, R, 0)) must land in the rotation plane at the cursor's
        // drag-frame angle. `rotate_drag_angle` reads angle in the
        // world-frame `axis_basis` (X→+Y/+Z, Y→+Z/+X, Z→+X/+Y); the
        // expected world position is `pivot + R*(cos(θ)*u + sin(θ)*v)`.
        let camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        let r = 0.85; // ROTATE_ARC_RADIUS_FRACTION
        let cursor_angle = std::f32::consts::FRAC_PI_3;
        let basis = |axis: Axis| -> (Vec3, Vec3) {
            match axis {
                Axis::X => (Vec3::Y, Vec3::Z),
                Axis::Y => (Vec3::Z, Vec3::X),
                Axis::Z => (Vec3::X, Vec3::Y),
            }
        };
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            let mut frame = frame_with(GizmoVisibility::DraggingRotate { axis }, [false; 3]);
            frame.drag_visual = Some(GizmoDragVisual::Rotate {
                axis,
                angle: cursor_angle,
            });
            let entries = build_rotate_entries(&frame, &camera);
            let i = match axis {
                Axis::X => 0,
                Axis::Y => 1,
                Axis::Z => 2,
            };
            let m = Mat4::from_cols_array_2d(&entries[i].axis_rotation);
            let p = m * glam::Vec4::new(0.0, r, 0.0, 1.0);
            let (u, v) = basis(axis);
            let expected = u * (r * cursor_angle.cos()) + v * (r * cursor_angle.sin());
            assert_approx_eq!(f32, p.x, expected.x, epsilon = 1e-5);
            assert_approx_eq!(f32, p.y, expected.y, epsilon = 1e-5);
            assert_approx_eq!(f32, p.z, expected.z, epsilon = 1e-5);
        }
    }

    #[test]
    fn active_axis_ignores_hover() {
        // Dragging an axis pins it at TINT_ACTIVE; the hover field
        // for the same axis should not pull it back down to hover.
        let mut frame = frame_with(
            GizmoVisibility::DraggingTranslate { axis: Axis::X },
            [false; 3],
        );
        frame.hover = Some(GizmoHover::Translate(Axis::X));
        let entries = build_translate_entries(&frame);
        assert_approx_eq!(f32, entries[0].tint[0], TINT_ACTIVE, epsilon = 1e-5);
    }
}

#[cfg(test)]
mod gizmo_slots_tests {
    use super::*;

    #[test]
    fn slots_are_contiguous_and_non_overlapping() {
        assert_eq!(
            GIZMO_SLOTS.translate.start() + GIZMO_SLOTS.translate.count(),
            GIZMO_SLOTS.rotate.start()
        );
        assert_eq!(
            GIZMO_SLOTS.rotate.start() + GIZMO_SLOTS.rotate.count(),
            GIZMO_SLOTS.scale_axis.start()
        );
        assert_eq!(
            GIZMO_SLOTS.scale_axis.start() + GIZMO_SLOTS.scale_axis.count(),
            GIZMO_ENTRY_SLOTS
        );
    }

    #[test]
    fn slot_returns_offset_within_range() {
        assert_eq!(GIZMO_SLOTS.translate.slot(0), 0);
        assert_eq!(GIZMO_SLOTS.translate.slot(2), 2);
        assert_eq!(GIZMO_SLOTS.rotate.slot(0), 3);
        assert_eq!(GIZMO_SLOTS.rotate.slot(2), 5);
        assert_eq!(GIZMO_SLOTS.scale_axis.slot(0), 6);
        assert_eq!(GIZMO_SLOTS.scale_axis.slot(2), 8);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic]
    fn slot_panics_when_axis_out_of_range() {
        let _ = GIZMO_SLOTS.translate.slot(3);
    }
}

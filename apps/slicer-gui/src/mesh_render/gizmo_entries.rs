/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the gizmo pass draws in a frame, in order: the faint back halves of
//! the rotate rings, then the dark outline of every visible handle, then the
//! handles themselves (the translucent plane squares last), then the marks of
//! a turn in progress. Pure data, so the style rules (#208, #274) are tested
//! without a device.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::gizmo::axis::{ALL_AXES, Axis};
use crate::gizmo::dimensions::px;
use crate::gizmo::look::{Family, GizmoLook};
use crate::gizmo::style::{
    ACTIVE_MIX, CENTRE_COLOR, HOVER_MIX, OUTLINE_ALPHA, OUTLINE_COLOR, PLANE_ALPHA_MAX,
    PLANE_LIT_GAIN, REFERENCE_ALPHA, RING_BACK_ALPHA, TICK_ALPHA, TICK_MIX, VIEW_RING_COLOR,
    WEDGE_ALPHA, axis_color, axis_fade, lighten, plane_fade, ring_fade,
};

/// Per-frame uniform for the gizmo pass: the camera transform, the
/// world-space pivot and shaft length packed into a vec4, and the eye.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(super) struct GizmoUniformsGpu {
    pub(super) view_proj: [[f32; 4]; 4],
    pub(super) pivot_and_scale: [f32; 4],
    /// `w = 1`: the eye position; `w = 0` (orthographic): the direction
    /// toward the eye.
    pub(super) eye: [f32; 4],
}

/// Per-draw entry uploaded to the dynamic-offset uniform buffer.
/// Mirrors `DrawEntry` in `gizmo.wgsl` one-for-one.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
pub(super) struct GizmoDrawEntry {
    pub(super) axis_rotation: [[f32; 4]; 4],
    pub(super) tint: [f32; 4],
    /// `x`: outline growth in handle-local units; `y`: which half to keep
    /// (0 all, 1 front, 2 back).
    pub(super) params: [f32; 4],
}

pub(super) const GIZMO_DRAW_ENTRY_SIZE: u64 = size_of::<GizmoDrawEntry>() as u64;

/// Draw entries reserved in `gizmo_entries_buffer`: at most 9 per axis
/// (arrow, ring, cube and plane outlines and bodies, ring back), 2 for the
/// centre ball and 2 for the marks of a turn, with room to spare.
pub(super) const GIZMO_ENTRY_SLOTS: u64 = 40;

/// Which static or per-frame mesh a draw uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GizmoMesh {
    Arrow,
    Ring,
    Cube,
    Ball,
    /// The square of a plane handle (#207).
    Plane,
    /// The thin shaft under a lone scale cube (the focused style).
    Stalk,
    /// The swept slice of a turn, built per frame.
    Wedge,
    /// The ticks and spokes of a turn, built per frame.
    Marks,
}

/// Which pipeline a draw uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GizmoLayer {
    /// Back faces culled, no depth: the outlines and the ring backs.
    Halo,
    /// Back faces culled, depth-tested so handles order among themselves.
    Core,
    /// Both faces, no depth: the flat marks of a turn.
    Flat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GizmoDraw {
    pub(super) mesh: GizmoMesh,
    pub(super) layer: GizmoLayer,
    pub(super) entry: GizmoDrawEntry,
}

/// Per-frame data the renderer needs to draw the gizmo. `pivot` is
/// the world-space gizmo origin; `scale` is the world-unit length of
/// the translate shaft (so the gizmo subtends a constant pixel size);
/// `view_dir` is the unit direction the camera looks along, which fades
/// the handles that point at it; `visibility` tells the renderer which
/// handles to draw and which one is being dragged.
#[derive(Copy, Clone, Debug)]
pub struct GizmoFrame {
    pub pivot: Vec3,
    pub scale: f32,
    pub view_dir: Vec3,
    pub visibility: GizmoVisibility,
    /// Handle the cursor is currently hovering over (no button held),
    /// or `None` when the cursor is off any handle. Ignored while
    /// `visibility` is `Dragging*`.
    pub hover: Option<GizmoHover>,
    /// Live drag visual: slides the dragged scale cube to the cursor,
    /// or carries the turn so far for the rotate marks. `None` when no
    /// drag is in progress.
    pub drag_visual: Option<GizmoDragVisual>,
    /// The style, size and tool the user picked (#274, #276). `scale`
    /// already holds the size.
    pub look: GizmoLook,
}

/// Live cursor-driven drag state, used by the renderer to give real-time
/// visual feedback during a drag.
#[derive(Copy, Clone, Debug)]
pub enum GizmoDragVisual {
    /// Per-axis scale: signed distance from the pivot to the cursor's
    /// closest-point-on-axis, in world units. The renderer slides the
    /// `axis` cube along the axis to that distance.
    ScaleAxis { axis: Axis, axial_distance: f32 },
    /// Rotate: the cursor angle at press (`start`, in the basis of
    /// `gizmo::axis::axis_basis`, as `rotate_drag_angle` measures it) and
    /// the turn applied so far (`sweep`, snapped when Ctrl is held).
    Rotate { axis: Axis, start: f32, sweep: f32 },
    /// A modal R without an axis turns about the view direction: the screen
    /// ring lies in the camera plane spanned by the unit vectors `right` and
    /// `up`; `start` and `sweep` are measured from `right` toward `up`.
    ViewRotate {
        right: Vec3,
        up: Vec3,
        start: f32,
        sweep: f32,
    },
}

/// Which handle the pointer is currently over, in `Idle` visibility.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GizmoHover {
    Translate(Axis),
    /// The plane square perpendicular to the axis (#207).
    Plane(Axis),
    Rotate(Axis),
    ScaleAxis(Axis),
    ScaleUniform,
}

/// Which gizmo handles are visible, and which one is dragged:
/// - `Idle`: every handle.
/// - `Dragging*`: only the dragged handle; while moving, the other
///   arrows stay as faint references.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GizmoVisibility {
    Idle,
    DraggingTranslate {
        axis: Axis,
    },
    /// A move in the plane perpendicular to `normal`: a plane handle
    /// (#207), or the body of a model pressed and dragged (#209). The
    /// square lights and the arrows stay as faint references.
    DraggingPlane {
        normal: Axis,
    },
    DraggingRotate {
        axis: Axis,
    },
    DraggingScaleAxis {
        axis: Axis,
    },
    DraggingScaleUniform,
    /// A modal G without an axis: every arrow lit, moving in the view plane.
    MovingFree,
    /// A modal R without an axis: only the screen ring, turning about the
    /// view direction.
    DraggingViewRotate,
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

/// The state of one handle in this frame.
#[derive(Clone, Copy)]
enum Look {
    Rest,
    Hover,
    Active,
}

impl Look {
    fn of(hovered: bool, active: bool) -> Self {
        if active {
            Look::Active
        } else if hovered {
            Look::Hover
        } else {
            Look::Rest
        }
    }

    fn color(self, base: [f32; 3]) -> [f32; 3] {
        match self {
            Look::Rest => base,
            Look::Hover => lighten(base, HOVER_MIX),
            Look::Active => lighten(base, ACTIVE_MIX),
        }
    }
}

/// One visible handle: its mesh and placement, colour, opacity, how much of
/// that opacity its body keeps (a plane square is translucent, its outline is
/// not), and whether it is a ring split into a front and a faint back half.
struct Handle {
    mesh: GizmoMesh,
    transform: Mat4,
    color: [f32; 3],
    alpha: f32,
    fill: f32,
    split_ring: bool,
}

impl Handle {
    fn solid(mesh: GizmoMesh, transform: Mat4, color: [f32; 3], alpha: f32) -> Self {
        Self {
            mesh,
            transform,
            color,
            alpha,
            fill: 1.0,
            split_ring: false,
        }
    }
}

const HALF_ALL: f32 = 0.0;
const HALF_FRONT: f32 = 1.0;
const HALF_BACK: f32 = 2.0;
/// Below this opacity a draw is skipped.
const MIN_ALPHA: f32 = 0.01;

fn entry(transform: Mat4, color: [f32; 3], alpha: f32, grow: f32, half: f32) -> GizmoDrawEntry {
    GizmoDrawEntry {
        axis_rotation: transform.to_cols_array_2d(),
        tint: [color[0], color[1], color[2], alpha],
        params: [grow, half, 0.0, 0.0],
    }
}

/// Builds the draws of one frame, in drawing order.
pub(super) fn build_gizmo_draws(frame: &GizmoFrame) -> Vec<GizmoDraw> {
    let handles = visible_handles(frame);
    let outline = px(frame.look.metrics().outline_px);
    let mut draws = Vec::with_capacity(GIZMO_ENTRY_SLOTS as usize);
    let half_of = |h: &Handle| if h.split_ring { HALF_FRONT } else { HALF_ALL };
    for h in handles.iter().filter(|h| h.split_ring) {
        let alpha = RING_BACK_ALPHA * h.alpha;
        if alpha >= MIN_ALPHA {
            draws.push(GizmoDraw {
                mesh: h.mesh,
                layer: GizmoLayer::Halo,
                entry: entry(h.transform, h.color, alpha, 0.0, HALF_BACK),
            });
        }
    }
    for h in &handles {
        draws.push(GizmoDraw {
            mesh: h.mesh,
            layer: GizmoLayer::Halo,
            entry: entry(
                h.transform,
                OUTLINE_COLOR,
                OUTLINE_ALPHA * h.alpha,
                outline,
                half_of(h),
            ),
        });
    }
    for h in &handles {
        draws.push(GizmoDraw {
            mesh: h.mesh,
            layer: GizmoLayer::Core,
            entry: entry(h.transform, h.color, h.alpha * h.fill, 0.0, half_of(h)),
        });
    }
    let marks_color = match (frame.visibility, frame.drag_visual) {
        (GizmoVisibility::DraggingRotate { axis }, Some(GizmoDragVisual::Rotate { .. })) => {
            Some(axis_color(axis))
        }
        (GizmoVisibility::DraggingViewRotate, Some(GizmoDragVisual::ViewRotate { .. })) => {
            Some(VIEW_RING_COLOR)
        }
        _ => None,
    };
    if let Some(base) = marks_color {
        draws.push(GizmoDraw {
            mesh: GizmoMesh::Wedge,
            layer: GizmoLayer::Flat,
            entry: entry(Mat4::IDENTITY, base, WEDGE_ALPHA, 0.0, HALF_ALL),
        });
        draws.push(GizmoDraw {
            mesh: GizmoMesh::Marks,
            layer: GizmoLayer::Flat,
            entry: entry(
                Mat4::IDENTITY,
                lighten(base, TICK_MIX),
                TICK_ALPHA,
                0.0,
                HALF_ALL,
            ),
        });
    }
    draws
}

fn visible_handles(frame: &GizmoFrame) -> Vec<Handle> {
    let rotations = axis_world_rotations();
    let look = frame.look;
    let m = look.metrics();
    let hover = match frame.visibility {
        GizmoVisibility::Idle => frame.hover,
        _ => None,
    };
    let idle = frame.visibility == GizmoVisibility::Idle;
    let mut out = Vec::with_capacity(16);
    for axis in ALL_AXES {
        let i = axis.index();
        let base = axis_color(axis);
        let fade = axis_fade(axis, frame.view_dir);
        let arrow = match frame.visibility {
            GizmoVisibility::Idle if look.shows(Family::Arrows) => Some((
                Look::of(hover == Some(GizmoHover::Translate(axis)), false),
                fade,
            )),
            GizmoVisibility::DraggingTranslate { axis: dragged } if dragged == axis => {
                Some((Look::Active, 1.0))
            }
            GizmoVisibility::DraggingTranslate { .. } | GizmoVisibility::DraggingPlane { .. } => {
                Some((Look::Rest, REFERENCE_ALPHA * fade))
            }
            GizmoVisibility::MovingFree => Some((Look::Active, fade)),
            _ => None,
        };
        if let Some((look, alpha)) = arrow {
            out.push(Handle::solid(
                GizmoMesh::Arrow,
                rotations[i],
                look.color(base),
                alpha,
            ));
        }
        let ring = match frame.visibility {
            GizmoVisibility::Idle if look.shows(Family::Rings) => Some((
                Look::of(hover == Some(GizmoHover::Rotate(axis)), false),
                true,
            )),
            GizmoVisibility::DraggingRotate { axis: dragged } if dragged == axis => {
                Some((Look::Active, false))
            }
            _ => None,
        };
        if let Some((look, split)) = ring {
            out.push(Handle {
                split_ring: split,
                ..Handle::solid(
                    GizmoMesh::Ring,
                    rotations[i],
                    look.color(base),
                    if split {
                        ring_fade(axis, frame.view_dir)
                    } else {
                        1.0
                    },
                )
            });
        }
        let cube = match frame.visibility {
            GizmoVisibility::Idle if look.shows(Family::Cubes) => Some((
                Look::of(hover == Some(GizmoHover::ScaleAxis(axis)), false),
                fade,
            )),
            GizmoVisibility::DraggingScaleAxis { axis: dragged } if dragged == axis => {
                Some((Look::Active, 1.0))
            }
            _ => None,
        };
        if let Some((look, alpha)) = cube {
            // While a scale drag is live, the cube slides along its axis to
            // the cursor: shift it, in world units, from its rest position.
            let rest = frame.scale * m.cube_at;
            let slide = match frame.drag_visual {
                Some(GizmoDragVisual::ScaleAxis {
                    axis: dragged,
                    axial_distance,
                }) if dragged == axis => axial_distance - rest,
                _ => 0.0,
            };
            let color = look.color(base);
            if m.stalk_radius_px > 0.0 {
                // The stalk stretches with the cube.
                let stretch = if rest > 0.0 {
                    ((rest + slide) / rest).max(0.0)
                } else {
                    1.0
                };
                out.push(Handle::solid(
                    GizmoMesh::Stalk,
                    rotations[i] * Mat4::from_scale(Vec3::new(stretch, 1.0, 1.0)),
                    color,
                    alpha,
                ));
            }
            out.push(Handle::solid(
                GizmoMesh::Cube,
                rotations[i] * Mat4::from_translation(Vec3::new(slide, 0.0, 0.0)),
                color,
                alpha,
            ));
        }
    }
    if let (
        GizmoVisibility::DraggingViewRotate,
        Some(GizmoDragVisual::ViewRotate { right, up, .. }),
    ) = (frame.visibility, frame.drag_visual)
    {
        // The ring mesh lies across handle-local +X: turn +X onto the
        // direction toward the eye, so the ring faces the viewer.
        let toward_eye = right.cross(up);
        out.push(Handle::solid(
            GizmoMesh::Ring,
            Mat4::from_cols(
                toward_eye.extend(0.0),
                right.extend(0.0),
                up.extend(0.0),
                glam::Vec4::W,
            ),
            Look::Active.color(VIEW_RING_COLOR),
            1.0,
        ));
    }
    let ball = match frame.visibility {
        GizmoVisibility::Idle if look.shows(Family::Ball) => {
            Some(Look::of(hover == Some(GizmoHover::ScaleUniform), false))
        }
        GizmoVisibility::DraggingScaleUniform => Some(Look::Active),
        _ => None,
    };
    if let Some(look) = ball {
        out.push(Handle::solid(
            GizmoMesh::Ball,
            Mat4::IDENTITY,
            look.color(CENTRE_COLOR),
            1.0,
        ));
    }
    // The plane squares come last: translucent, they show what lies behind.
    for normal in ALL_AXES {
        let lit = match frame.visibility {
            _ if idle && look.shows(Family::Planes) => {
                Some(Look::of(hover == Some(GizmoHover::Plane(normal)), false))
            }
            GizmoVisibility::DraggingPlane { normal: dragged } if dragged == normal => {
                Some(Look::Active)
            }
            _ => None,
        };
        let Some(state) = lit else {
            continue;
        };
        let alpha = match state {
            Look::Active => 1.0,
            _ => plane_fade(normal, frame.view_dir),
        };
        let fill = match state {
            Look::Rest => m.plane_alpha,
            _ => (m.plane_alpha + PLANE_LIT_GAIN).min(PLANE_ALPHA_MAX),
        };
        out.push(Handle {
            fill,
            ..Handle::solid(
                GizmoMesh::Plane,
                plane_transform(normal, frame.view_dir),
                state.color(axis_color(normal)),
                alpha,
            )
        });
    }
    out.retain(|h| h.alpha >= MIN_ALPHA);
    out
}

/// Lays the plane mesh (built across handle-local +X, its square toward +Y
/// and +Z) onto the plane perpendicular to `normal`, in the quadrant that
/// faces the camera looking along `view_dir` (`plane_quadrant`, which the hit
/// test shares).
fn plane_transform(normal: Axis, view_dir: Vec3) -> Mat4 {
    let (u, v) = normal.plane_axes();
    let (su, sv) = crate::gizmo::hit_test::plane_quadrant(normal, view_dir);
    Mat4::from_cols(
        normal.world_dir().extend(0.0),
        (u.world_dir() * su).extend(0.0),
        (v.world_dir() * sv).extend(0.0),
        glam::Vec4::W,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gizmo::dimensions::{MIDPOINT_FRACTION, OUTLINE_PIXELS};
    use crate::gizmo::look::OUTLINE_METRICS;
    use crate::gizmo::style::{AXIS_COLORS, RING_EDGE_ALPHA};
    use float_cmp::assert_approx_eq;
    use glam::Vec4;

    /// A view looking down at an angle, so no axis faces the camera.
    fn oblique() -> Vec3 {
        Vec3::new(-0.5, 0.6, -0.62).normalize()
    }

    fn frame(visibility: GizmoVisibility) -> GizmoFrame {
        GizmoFrame {
            pivot: Vec3::ZERO,
            scale: 1.0,
            view_dir: oblique(),
            visibility,
            hover: None,
            drag_visual: None,
            look: GizmoLook::default(),
        }
    }

    fn cores(draws: &[GizmoDraw], mesh: GizmoMesh) -> Vec<GizmoDrawEntry> {
        draws
            .iter()
            .filter(|d| d.mesh == mesh && d.layer == GizmoLayer::Core)
            .map(|d| d.entry)
            .collect()
    }

    fn rgb(e: &GizmoDrawEntry) -> [f32; 3] {
        [e.tint[0], e.tint[1], e.tint[2]]
    }

    fn assert_rgb(actual: [f32; 3], expected: [f32; 3]) {
        for i in 0..3 {
            assert_approx_eq!(f32, actual[i], expected[i], epsilon = 1e-6);
        }
    }

    #[test]
    fn idle_shows_every_handle_in_its_axis_colour() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        for mesh in [GizmoMesh::Arrow, GizmoMesh::Ring, GizmoMesh::Cube] {
            let c = cores(&draws, mesh);
            assert_eq!(c.len(), 3, "{mesh:?}");
            for (i, e) in c.iter().enumerate() {
                assert_rgb(rgb(e), AXIS_COLORS[i]);
            }
        }
        assert_eq!(cores(&draws, GizmoMesh::Ball).len(), 1);
    }

    #[test]
    fn rings_use_their_axis_colour_like_the_arrows() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        let arrows = cores(&draws, GizmoMesh::Arrow);
        let rings = cores(&draws, GizmoMesh::Ring);
        for i in 0..3 {
            assert_rgb(rgb(&rings[i]), rgb(&arrows[i]));
        }
    }

    #[test]
    fn every_handle_has_a_dark_outline_drawn_before_any_handle() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        let first_core = draws
            .iter()
            .position(|d| d.layer == GizmoLayer::Core)
            .unwrap();
        let outlines: Vec<_> = draws
            .iter()
            .filter(|d| d.layer == GizmoLayer::Halo && d.entry.params[1] != HALF_BACK)
            .collect();
        assert_eq!(
            outlines.len(),
            cores(&draws, GizmoMesh::Arrow).len() * 3 + 1 + cores(&draws, GizmoMesh::Plane).len()
        );
        for (i, d) in draws.iter().enumerate() {
            if d.layer == GizmoLayer::Halo {
                assert!(i < first_core);
            }
        }
        for o in outlines {
            assert_rgb(rgb(&o.entry), OUTLINE_COLOR);
            assert_approx_eq!(f32, o.entry.tint[3], OUTLINE_ALPHA);
            assert_approx_eq!(f32, o.entry.params[0], px(OUTLINE_PIXELS));
        }
    }

    #[test]
    fn idle_rings_split_into_a_grabbable_front_and_a_faint_back() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        let backs: Vec<_> = draws
            .iter()
            .filter(|d| d.entry.params[1] == HALF_BACK)
            .collect();
        assert_eq!(backs.len(), 3);
        for b in &backs {
            assert_eq!(b.mesh, GizmoMesh::Ring);
            assert_approx_eq!(f32, b.entry.tint[3], RING_BACK_ALPHA);
        }
        for e in cores(&draws, GizmoMesh::Ring) {
            assert_approx_eq!(f32, e.params[1], HALF_FRONT);
        }
    }

    #[test]
    fn hover_lightens_toward_white_only_the_hovered_handle() {
        let mut f = frame(GizmoVisibility::Idle);
        f.hover = Some(GizmoHover::Translate(Axis::X));
        let draws = build_gizmo_draws(&f);
        let arrows = cores(&draws, GizmoMesh::Arrow);
        assert_rgb(rgb(&arrows[0]), lighten(AXIS_COLORS[0], HOVER_MIX));
        assert_rgb(rgb(&arrows[1]), AXIS_COLORS[1]);
        // The X ring and cube are another handle: not lightened.
        assert_rgb(rgb(&cores(&draws, GizmoMesh::Ring)[0]), AXIS_COLORS[0]);
        assert_rgb(rgb(&cores(&draws, GizmoMesh::Cube)[0]), AXIS_COLORS[0]);
    }

    #[test]
    fn moving_shows_the_active_arrow_and_the_others_faint() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::DraggingTranslate { axis: Axis::Y }));
        let arrows = cores(&draws, GizmoMesh::Arrow);
        assert_eq!(arrows.len(), 3);
        assert_rgb(rgb(&arrows[1]), lighten(AXIS_COLORS[1], ACTIVE_MIX));
        assert_approx_eq!(f32, arrows[1].tint[3], 1.0);
        assert!(arrows[0].tint[3] <= REFERENCE_ALPHA + 1e-6);
        assert!(arrows[2].tint[3] <= REFERENCE_ALPHA + 1e-6);
        assert!(cores(&draws, GizmoMesh::Ring).is_empty());
        assert!(cores(&draws, GizmoMesh::Cube).is_empty());
        assert!(cores(&draws, GizmoMesh::Ball).is_empty());
    }

    #[test]
    fn rotating_shows_only_the_whole_active_ring_and_its_marks() {
        let mut f = frame(GizmoVisibility::DraggingRotate { axis: Axis::Z });
        f.drag_visual = Some(GizmoDragVisual::Rotate {
            axis: Axis::Z,
            start: 0.0,
            sweep: 0.6,
        });
        let draws = build_gizmo_draws(&f);
        let rings = cores(&draws, GizmoMesh::Ring);
        assert_eq!(rings.len(), 1);
        assert_approx_eq!(f32, rings[0].params[1], HALF_ALL);
        assert!(draws.iter().all(|d| d.entry.params[1] != HALF_BACK));
        assert!(cores(&draws, GizmoMesh::Arrow).is_empty());
        let wedge: Vec<_> = draws
            .iter()
            .filter(|d| d.mesh == GizmoMesh::Wedge)
            .collect();
        assert_eq!(wedge.len(), 1);
        assert_approx_eq!(f32, wedge[0].entry.tint[3], WEDGE_ALPHA);
        assert_rgb(rgb(&wedge[0].entry), AXIS_COLORS[2]);
        // The marks come last, over the ring.
        assert_eq!(draws.last().unwrap().mesh, GizmoMesh::Marks);
    }

    #[test]
    fn scaling_shows_only_the_dragged_cube_slid_to_the_cursor() {
        let mut f = frame(GizmoVisibility::DraggingScaleAxis { axis: Axis::X });
        f.drag_visual = Some(GizmoDragVisual::ScaleAxis {
            axis: Axis::X,
            axial_distance: 1.5,
        });
        let draws = build_gizmo_draws(&f);
        let cubes = cores(&draws, GizmoMesh::Cube);
        assert_eq!(cubes.len(), 1);
        let m = Mat4::from_cols_array_2d(&cubes[0].axis_rotation);
        let centre = m * Vec4::new(MIDPOINT_FRACTION, 0.0, 0.0, 1.0);
        assert_approx_eq!(f32, centre.x, 1.5, epsilon = 1e-5);
        assert!(cores(&draws, GizmoMesh::Arrow).is_empty());
    }

    #[test]
    fn uniform_scaling_shows_only_the_centre_ball() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::DraggingScaleUniform));
        assert_eq!(cores(&draws, GizmoMesh::Ball).len(), 1);
        assert_eq!(
            draws.iter().filter(|d| d.layer == GizmoLayer::Core).count(),
            1
        );
    }

    #[test]
    fn an_axis_pointing_at_the_camera_fades_out() {
        let mut f = frame(GizmoVisibility::Idle);
        f.view_dir = -Vec3::Z;
        let draws = build_gizmo_draws(&f);
        // The Z arrow and cube are gone; X and Y stay at full opacity.
        let arrows = cores(&draws, GizmoMesh::Arrow);
        assert_eq!(arrows.len(), 2);
        for e in arrows {
            assert_approx_eq!(f32, e.tint[3], 1.0);
        }
        assert_eq!(cores(&draws, GizmoMesh::Cube).len(), 2);
        // Seen along Z, the X and Y rings are edge-on: faded, still drawn.
        let rings = cores(&draws, GizmoMesh::Ring);
        assert_eq!(rings.len(), 3);
        assert_approx_eq!(f32, rings[0].tint[3], RING_EDGE_ALPHA);
        assert_approx_eq!(f32, rings[2].tint[3], 1.0);
    }

    #[test]
    fn axis_rotations_map_handle_local_x_to_world_axes() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        let arrows = cores(&draws, GizmoMesh::Arrow);
        let local_x = Vec4::new(1.0, 0.0, 0.0, 0.0);
        for (i, e) in arrows.iter().enumerate() {
            let w = Mat4::from_cols_array_2d(&e.axis_rotation) * local_x;
            assert!((w.truncate() - ALL_AXES[i].world_dir()).length() < 1e-6);
        }
    }

    #[test]
    fn a_frame_never_needs_more_entries_than_reserved() {
        let mut f = frame(GizmoVisibility::Idle);
        assert!(build_gizmo_draws(&f).len() as u64 <= GIZMO_ENTRY_SLOTS);
        f.visibility = GizmoVisibility::DraggingRotate { axis: Axis::X };
        f.drag_visual = Some(GizmoDragVisual::Rotate {
            axis: Axis::X,
            start: 0.0,
            sweep: 1.0,
        });
        assert!(build_gizmo_draws(&f).len() as u64 <= GIZMO_ENTRY_SLOTS);
    }

    #[test]
    fn a_modal_r_without_axis_shows_only_the_yellow_screen_ring_facing_the_viewer() {
        let right = Vec3::new(0.6, 0.8, 0.0);
        let up = Vec3::new(-0.8 * 0.5, 0.6 * 0.5, 0.75_f32.sqrt());
        let mut f = frame(GizmoVisibility::DraggingViewRotate);
        f.drag_visual = Some(GizmoDragVisual::ViewRotate {
            right,
            up,
            start: 0.2,
            sweep: 0.7,
        });
        let draws = build_gizmo_draws(&f);
        let rings = cores(&draws, GizmoMesh::Ring);
        assert_eq!(rings.len(), 1);
        assert_rgb(rgb(&rings[0]), lighten(VIEW_RING_COLOR, ACTIVE_MIX));
        assert_approx_eq!(f32, rings[0].params[1], HALF_ALL);
        // The ring's plane is the camera plane: its local axis maps onto the
        // direction toward the eye, its local Y and Z onto right and up.
        let m = Mat4::from_cols_array_2d(&rings[0].axis_rotation);
        let normal = (m * Vec4::new(1.0, 0.0, 0.0, 0.0)).truncate();
        assert!((normal - right.cross(up)).length() < 1e-5);
        assert!(((m * Vec4::new(0.0, 1.0, 0.0, 0.0)).truncate() - right).length() < 1e-5);
        assert!((m.determinant() - 1.0).abs() < 1e-4);
        for mesh in [GizmoMesh::Arrow, GizmoMesh::Cube, GizmoMesh::Ball] {
            assert!(cores(&draws, mesh).is_empty(), "{mesh:?}");
        }
        let wedge: Vec<_> = draws
            .iter()
            .filter(|d| d.mesh == GizmoMesh::Wedge)
            .collect();
        assert_eq!(wedge.len(), 1);
        assert_rgb(rgb(&wedge[0].entry), VIEW_RING_COLOR);
        assert_eq!(draws.last().unwrap().mesh, GizmoMesh::Marks);
    }

    #[test]
    fn the_screen_ring_yellow_stays_apart_from_the_axes_and_the_selection_amber() {
        let amber = [
            0xd7 as f32 / 255.0,
            0xa7 as f32 / 255.0,
            0x5a as f32 / 255.0,
        ];
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        for axis in AXIS_COLORS {
            assert!(distance(VIEW_RING_COLOR, axis) > 0.3);
        }
        assert!(distance(VIEW_RING_COLOR, amber) > 0.15);
        // No other mode draws the screen ring.
        let idle = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        for d in idle {
            assert_ne!(rgb(&d.entry), VIEW_RING_COLOR);
        }
    }

    #[test]
    fn a_free_modal_move_lights_every_arrow() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::MovingFree));
        let arrows = cores(&draws, GizmoMesh::Arrow);
        assert_eq!(arrows.len(), 3);
        for (i, e) in arrows.iter().enumerate() {
            assert_rgb(rgb(e), lighten(AXIS_COLORS[i], ACTIVE_MIX));
        }
        assert!(cores(&draws, GizmoMesh::Ring).is_empty());
    }

    #[test]
    fn idle_shows_three_translucent_plane_squares_in_the_colour_of_their_normal() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        let planes = cores(&draws, GizmoMesh::Plane);
        assert_eq!(planes.len(), 3);
        for (i, e) in planes.iter().enumerate() {
            assert_rgb(rgb(e), AXIS_COLORS[i]);
            let fade = plane_fade(ALL_AXES[i], oblique());
            assert_approx_eq!(f32, e.tint[3], fade * OUTLINE_METRICS.plane_alpha);
            // The square lies in the plane of the two other axes, in the
            // quadrant facing the camera.
            let m = Mat4::from_cols_array_2d(&e.axis_rotation);
            let n = (m * Vec4::new(1.0, 0.0, 0.0, 0.0)).truncate();
            assert!((n - ALL_AXES[i].world_dir()).length() < 1e-6);
            let (u, v) = ALL_AXES[i].plane_axes();
            let (su, sv) = crate::gizmo::hit_test::plane_quadrant(ALL_AXES[i], oblique());
            let corner = (m * Vec4::new(0.0, 1.0, 1.0, 0.0)).truncate();
            assert!((corner - u.world_dir() * su - v.world_dir() * sv).length() < 1e-6);
            assert!(corner.dot(oblique()) <= 0.0, "toward the camera");
        }
        // The squares come after every other handle.
        let last_other = draws
            .iter()
            .rposition(|d| d.layer == GizmoLayer::Core && d.mesh != GizmoMesh::Plane)
            .unwrap();
        let first_plane = draws
            .iter()
            .position(|d| d.layer == GizmoLayer::Core && d.mesh == GizmoMesh::Plane)
            .unwrap();
        assert!(first_plane > last_other);
        // Their outline is as dark as any other.
        let plane_outlines: Vec<_> = draws
            .iter()
            .filter(|d| d.layer == GizmoLayer::Halo && d.mesh == GizmoMesh::Plane)
            .collect();
        assert_eq!(plane_outlines.len(), 3);
    }

    #[test]
    fn a_square_seen_edge_on_disappears() {
        let mut f = frame(GizmoVisibility::Idle);
        f.view_dir = -Vec3::Z;
        let draws = build_gizmo_draws(&f);
        let planes = cores(&draws, GizmoMesh::Plane);
        assert_eq!(planes.len(), 1);
        assert_rgb(rgb(&planes[0]), AXIS_COLORS[2]);
    }

    #[test]
    fn a_hovered_square_lightens_and_grows_more_opaque() {
        let mut f = frame(GizmoVisibility::Idle);
        f.hover = Some(GizmoHover::Plane(Axis::Z));
        let draws = build_gizmo_draws(&f);
        let planes = cores(&draws, GizmoMesh::Plane);
        assert_rgb(rgb(&planes[2]), lighten(AXIS_COLORS[2], HOVER_MIX));
        assert!(planes[2].tint[3] > planes[0].tint[3]);
        assert_rgb(rgb(&planes[0]), AXIS_COLORS[0]);
    }

    #[test]
    fn a_plane_drag_shows_its_square_and_the_arrows_faint() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::DraggingPlane { normal: Axis::Z }));
        let planes = cores(&draws, GizmoMesh::Plane);
        assert_eq!(planes.len(), 1);
        assert_rgb(rgb(&planes[0]), lighten(AXIS_COLORS[2], ACTIVE_MIX));
        let arrows = cores(&draws, GizmoMesh::Arrow);
        assert_eq!(arrows.len(), 3);
        for a in arrows {
            assert!(a.tint[3] <= REFERENCE_ALPHA + 1e-6);
        }
        assert!(cores(&draws, GizmoMesh::Ring).is_empty());
        assert!(cores(&draws, GizmoMesh::Ball).is_empty());
    }

    fn focused(tool: crate::gizmo::look::GizmoTool) -> GizmoFrame {
        let mut f = frame(GizmoVisibility::Idle);
        f.look = GizmoLook {
            style: crate::gizmo::look::GizmoStyle::Focused,
            size: 1.0,
            tool,
        };
        f
    }

    #[test]
    fn the_focused_style_shows_one_tool_at_a_time() {
        use crate::gizmo::look::GizmoTool;
        let count = |tool, mesh| cores(&build_gizmo_draws(&focused(tool)), mesh).len();
        assert_eq!(count(GizmoTool::Move, GizmoMesh::Arrow), 3);
        assert_eq!(count(GizmoTool::Move, GizmoMesh::Plane), 3);
        assert_eq!(count(GizmoTool::Move, GizmoMesh::Ring), 0);
        assert_eq!(count(GizmoTool::Move, GizmoMesh::Ball), 0);
        assert_eq!(count(GizmoTool::Rotate, GizmoMesh::Ring), 3);
        assert_eq!(count(GizmoTool::Rotate, GizmoMesh::Arrow), 0);
        assert_eq!(count(GizmoTool::Scale, GizmoMesh::Cube), 3);
        assert_eq!(count(GizmoTool::Scale, GizmoMesh::Stalk), 3);
        assert_eq!(count(GizmoTool::Scale, GizmoMesh::Ball), 1);
        assert_eq!(count(GizmoTool::Scale, GizmoMesh::Arrow), 0);
        // Its outline is thicker.
        let draws = build_gizmo_draws(&focused(GizmoTool::Move));
        let halo = draws.iter().find(|d| d.layer == GizmoLayer::Halo).unwrap();
        assert_approx_eq!(
            f32,
            halo.entry.params[0],
            px(crate::gizmo::look::FOCUSED_METRICS.outline_px)
        );
    }

    #[test]
    fn a_drag_in_the_focused_style_shows_its_handle_whatever_the_tool() {
        use crate::gizmo::look::GizmoTool;
        // A modal R with an axis while the rail says Move.
        let mut f = focused(GizmoTool::Move);
        f.visibility = GizmoVisibility::DraggingRotate { axis: Axis::X };
        assert_eq!(cores(&build_gizmo_draws(&f), GizmoMesh::Ring).len(), 1);
    }

    #[test]
    fn the_outline_style_draws_no_stalks() {
        let draws = build_gizmo_draws(&frame(GizmoVisibility::Idle));
        assert!(cores(&draws, GizmoMesh::Stalk).is_empty());
    }
}

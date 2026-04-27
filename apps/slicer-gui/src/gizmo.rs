/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure-CPU helpers for the translate gizmo: pivot, world-aligned
//! axes, screen-constant scale, ray-vs-axis-handle hit test, and the
//! drag-time closest-point math. Renderer-agnostic so it can be
//! unit-tested without a `wgpu::Device`.

use glam::Vec3;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::{Scene, SceneObjectContent, Transform3D};

use crate::camera::OrbitCamera;
use crate::picking::{Ray, ray_hits_aabb_t};

/// World-aligned gizmo axis. The X/Y/Z directions are global +X, +Y,
/// +Z; a local-axis option is left as a future toggle.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const fn world_dir(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

pub const ALL_AXES: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

/// Screen-pixel length of the gizmo shaft. Drives `gizmo_scale`.
/// Sized to leave room for the per-axis midpoint cube *and* the cone
/// tip without either occluding the other.
pub const HANDLE_PIXEL_LENGTH: f32 = 120.0;

/// Screen-pixel radius of the cylinder hit volume. The visual shaft
/// is thinner than this — the hit cylinder is fattened so a click
/// near (but not on) the shaft still registers.
pub const HANDLE_PIXEL_RADIUS: f32 = 6.0;

/// Cosine of the lock-out angle: when
/// `|axis_world · ray_dir| > LOCKOUT_COS`, the angle between the
/// camera ray and the axis is below 8° and the closest-point math
/// becomes unstable — `1 − (axis · ray_dir)²` drops below `sin²(8°)`
/// and a one-pixel cursor jitter would translate the object far more
/// than the user expects. Such an axis is dropped from the hit test
/// and rendered greyed so the user sees why it is unresponsive.
///
/// `cos(8°) ≈ 0.99027`.
const LOCKOUT_COS: f32 = 0.990_268_07;

/// Cosine of the rotate-arc lock-out's complementary threshold:
/// when `|axis_world · ray_dir| < ROTATE_LOCKOUT_COS`, the camera ray
/// is within 8° of the arc's plane. The ray-vs-arc-plane intersection
/// then becomes ill-defined and the angular position around the axis
/// jumps under one-pixel cursor jitter. Threshold magnitude matches
/// `LOCKOUT_COS` (8° margin); the geometric configuration is the
/// complementary one (axis perpendicular to ray instead of parallel).
///
/// `cos(82°) ≈ 0.13917`.
const ROTATE_LOCKOUT_COS: f32 = 0.139_173_1;

/// Length, in handle-local space, of each cone in the rotate
/// "two-cone spindle" hit target. The two cones share their bases at
/// the arc's camera-facing midpoint and point along ±axis with this
/// length. Mirrors the translate cone's `1.18 - 1.0 = 0.18`.
pub const ROTATE_CONE_LENGTH: f32 = 0.18;

/// Base radius, in handle-local space, of each rotate cone — same as
/// the translate cone's base radius for visual consistency.
pub const ROTATE_CONE_RADIUS: f32 = 0.07;

/// Lower bound on a single-axis or uniform scale factor. Factors
/// below this are clamped up; factors above `1.0 / EPS_SCALE` are
/// clamped down. Mirroring (negative scale) is disallowed through
/// the gizmo, so the bound also rejects sign flips. The user sees
/// the gizmo "stick" at the limit.
#[allow(dead_code)]
pub const EPS_SCALE: f32 = 1e-3;

/// Side length, in screen pixels, of the per-axis scale cube. The
/// cube sits at the midpoint of the translate shaft (`scale * 0.5`)
/// and is small enough to leave the cone tip clearly distinct.
pub const AXIS_SCALE_CUBE_PIXELS: f32 = 10.0;

/// Side length, in screen pixels, of the centre cube driving the
/// uniform-scale gesture. Sized larger than the per-axis cube so the
/// pivot's handle reads as a single distinct target rather than a
/// fourth axis.
pub const CENTRE_CUBE_PIXELS: f32 = 14.0;

/// Fraction of the translate shaft length at which the scale-axis
/// midpoint cube sits (`pivot + axis * scale * MIDPOINT_FRACTION`).
/// Pinned at the *midpoint* — not the tip — so per-axis scale doesn't
/// collide with the translate cone.
pub const MIDPOINT_FRACTION: f32 = 0.5;

/// Tube (minor) radius of the rotate arc, in screen pixels. The
/// matching world radius is derived per-frame from `gizmo_scale`.
pub const ROTATE_ARC_TUBE_PIXELS: f32 = 3.0;

/// Major (circle) radius of the rotate arc, expressed as a fraction
/// of the translate shaft length. 0.85 keeps the arc inside the
/// translate cone tip so the two handle types don't fight for the
/// same screen pixels.
pub const ROTATE_ARC_RADIUS_FRACTION: f32 = 0.85;

/// World-space pivot for the gizmo: the local-AABB centre of the
/// object, transformed by the object's `Transform3D`. Returns `None`
/// when the object has no vertices, an out-of-bounds resource
/// reference, or `idx` is out of range — all of which render the
/// gizmo unattachable.
pub fn pivot_for_object(scene: &Scene, idx: usize) -> Option<Vec3> {
    let object = scene.objects.get(idx)?;
    let mesh = match &object.content {
        SceneObjectContent::Mesh(m) => m,
        SceneObjectContent::Resource(r) => scene.resources.get(*r)?,
    };
    let (lmin, lmax) = local_aabb_centre(mesh)?;
    let local_centre = (lmin + lmax) * 0.5;
    Some(match &object.transform {
        Transform3D::Identity => local_centre,
        Transform3D::Affine(a) => {
            let v = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([
                local_centre.x,
                local_centre.y,
                local_centre.z,
            ]));
            Vec3::from(v.0)
        }
    })
}

fn local_aabb_centre(mesh: &IndexedMesh) -> Option<(Vec3, Vec3)> {
    let mut iter = mesh.vertices.iter();
    let first = Vec3::from(iter.next()?.0);
    let mut min = first;
    let mut max = first;
    for v in iter {
        let v = Vec3::from(v.0);
        min = min.min(v);
        max = max.max(v);
    }
    Some((min, max))
}

/// World-units-per-pixel × the handle's screen-pixel length, so that
/// every handle subtends `HANDLE_PIXEL_LENGTH` pixels regardless of
/// camera distance. Uses the same `world_per_pixel` formula as the
/// camera's pan code path.
pub fn gizmo_scale(camera: &OrbitCamera, fov_y: f32, viewport_h: f32) -> f32 {
    let h = viewport_h.max(1.0);
    let world_per_pixel = 2.0 * camera.distance * (fov_y * 0.5).tan() / h;
    world_per_pixel * HANDLE_PIXEL_LENGTH
}

/// World-unit radius of the hit cylinder, derived the same way as
/// `gizmo_scale` so the hit volume keeps its ~6 px screen radius
/// regardless of camera distance.
pub fn hit_radius_world(camera: &OrbitCamera, fov_y: f32, viewport_h: f32) -> f32 {
    let h = viewport_h.max(1.0);
    let world_per_pixel = 2.0 * camera.distance * (fov_y * 0.5).tan() / h;
    world_per_pixel * HANDLE_PIXEL_RADIUS
}

/// Whether an axis is grabbable. Returns false when the camera is
/// looking too closely along the axis (see `LOCKOUT_COS`). The
/// renderer should also grey the axis tint when this returns false
/// so the user sees that the lock-out is intentional.
pub fn axis_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() < LOCKOUT_COS
}

/// Lower bound on the translate cone's hit cylinder, in handle-local
/// space. Below this `s` along the axis the cylinder is dormant —
/// the *shaft line* between the gizmo origin and the cone is purely
/// decorative (clicks there fall through to other handle categories
/// or the picker), the per-axis scale cube at the midpoint owns its
/// own discrete target.
const TRANSLATE_HIT_S_MIN: f32 = 1.0;
/// Upper bound on the translate cone's hit cylinder. Extends past
/// the visual tip (`CONE_TIP_X = 1.18`) by a small forgiving margin.
const TRANSLATE_HIT_S_MAX: f32 = 1.25;

/// Hit-test the three world-aligned axis handles against `ray`.
/// Returns the front-most pickable axis (smallest positive `t` on
/// the ray) whose cone cylinder the ray intersects, paired with that
/// `t`, or `None`. The active region is the cone tip *only* — the
/// shaft line below the cone is decorative.
///
/// The returned `t` is exposed so the controller can arbitrate
/// front-most across the four hit-test categories (translate cones,
/// rotate cones, scale midpoint cubes, scale centre cube).
pub fn hit_test(
    ray: &Ray,
    pivot: Vec3,
    length_world: f32,
    radius_world: f32,
) -> Option<(Axis, f32)> {
    let s_min = length_world * TRANSLATE_HIT_S_MIN;
    let s_max = length_world * TRANSLATE_HIT_S_MAX;
    let mut best: Option<(Axis, f32)> = None;
    for &axis in &ALL_AXES {
        if !axis_pickable(ray, axis) {
            continue;
        }
        if let Some(t) =
            ray_hits_finite_cylinder(ray, pivot, axis.world_dir(), s_min, s_max, radius_world)
        {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((axis, t)),
            }
        }
    }
    best
}

/// Ray-vs-finite-cylinder. The cylinder is centred at `centre`,
/// oriented along the unit vector `axis_dir`, with parameter `s`
/// along the axis ranging over `[s_min, s_max]` and radius `radius`
/// perpendicular to the axis. Returns the smallest positive `t` at
/// which the ray *enters* the cylinder from outside, or `None` if it
/// misses, originates inside, or hits behind the camera.
///
/// Used for both the translate cone (centre = pivot, s ∈ [1, 1.25]
/// times shaft length) and the rotate two-cone spindle (centre = arc
/// midpoint, s ∈ ±cone half-length around 0).
///
/// The intersection is the conjunction of two slabs: the s-range
/// (cap-bounded) and the radial-distance constraint (wall-bounded).
/// `t_in = max(t_s_lo, t_r_lo)`, `t_out = min(t_s_hi, t_r_hi)`; the
/// ray is inside iff `t_in ≤ t_out`. The hit `t` is `t_in` provided
/// it is in front of the ray's origin.
fn ray_hits_finite_cylinder(
    ray: &Ray,
    centre: Vec3,
    axis_dir: Vec3,
    s_min: f32,
    s_max: f32,
    radius: f32,
) -> Option<f32> {
    let q = ray.origin - centre;
    let d_dot_a = ray.direction.dot(axis_dir);
    let q_dot_a = q.dot(axis_dir);

    // Slab 1: cap-bounded s-range.
    let (t_s_lo, t_s_hi) = if d_dot_a.abs() < 1e-10 {
        // Ray parallel to the cylinder's axis. `s` is constant; the
        // ray is in the slab iff its constant `s` lies in range.
        if q_dot_a < s_min || q_dot_a > s_max {
            return None;
        }
        (f32::NEG_INFINITY, f32::INFINITY)
    } else {
        let t1 = (s_min - q_dot_a) / d_dot_a;
        let t2 = (s_max - q_dot_a) / d_dot_a;
        if t1 < t2 { (t1, t2) } else { (t2, t1) }
    };

    // Slab 2: radial distance ≤ radius (wall constraint).
    let dp = ray.direction - axis_dir * d_dot_a;
    let qp = q - axis_dir * q_dot_a;
    let qa = dp.dot(dp);
    let qb = 2.0 * dp.dot(qp);
    let qc = qp.dot(qp) - radius * radius;
    let (t_r_lo, t_r_hi) = if qa.abs() < 1e-10 {
        // Ray parallel to the cylinder's axis (zero radial motion).
        // Inside iff its current radial distance is within `radius`.
        if qc > 0.0 {
            return None;
        }
        (f32::NEG_INFINITY, f32::INFINITY)
    } else {
        let disc = qb * qb - 4.0 * qa * qc;
        if disc < 0.0 {
            return None;
        }
        let sqrt_disc = disc.sqrt();
        let inv_2qa = 1.0 / (2.0 * qa);
        let t1 = (-qb - sqrt_disc) * inv_2qa;
        let t2 = (-qb + sqrt_disc) * inv_2qa;
        if t1 < t2 { (t1, t2) } else { (t2, t1) }
    };

    let t_in = t_s_lo.max(t_r_lo);
    let t_out = t_s_hi.min(t_r_hi);
    if t_in > t_out || t_out <= 0.0 {
        return None;
    }
    // Origin-inside or behind-the-camera entry: reject. The user must
    // be outside the hit volume to grab the handle.
    if t_in <= 0.0 {
        return None;
    }
    Some(t_in)
}

/// Closest point on the infinite world-space line `pivot + t * axis`
/// to the world-space ray. Used during a drag to convert the cursor
/// position into the pinned-to-axis world point.
///
/// Stable as long as `axis_pickable(ray, axis)` is true at gesture
/// start; if the camera rotates into the unstable region mid-drag,
/// the drag continues using the same (now-noisier) formula — we
/// don't switch policies inside a gesture, since a mid-drag jump in
/// world delta would be more confusing than a few noisy frames.
pub fn closest_point_on_axis(ray: &Ray, pivot: Vec3, axis: Axis) -> Vec3 {
    let a = axis.world_dir();
    let d = ray.direction;
    let oc = pivot - ray.origin;

    let d_dot_a = d.dot(a);
    let oc_dot_a = oc.dot(a);
    let oc_dot_d = oc.dot(d);

    let denom = 1.0 - d_dot_a * d_dot_a;
    // Caller is responsible for ensuring denom is non-tiny via the
    // `axis_pickable` lock-out at gesture start. Inside a
    // tolerated-but-marginal drag, we clamp denom to a small positive
    // floor rather than dividing by zero — keeps the formula
    // well-defined even at the exact lock-out edge.
    let denom = denom.max(1e-6);

    // Derivation: with `q = pivot − O`, the perpendicularity
    // conditions `(δ·D) = 0` and `(δ·a) = 0` solve to
    //   `t · (1 − (a·D)²) = q·D · (a·D) − q·a`.
    let t_axis = (oc_dot_d * d_dot_a - oc_dot_a) / denom;
    pivot + a * t_axis
}

/// Whether a rotate arc is grabbable. Returns `false` when the camera
/// ray sits within 8° of the arc's plane: `|axis · ray.direction|`
/// drops below `cos(82°)` and the perpendicular-plane intersection
/// — the basis of both hit-test and drag math — degenerates. The
/// renderer should grey the arc in that state so the user sees why
/// it is unresponsive.
pub fn arc_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() >= ROTATE_LOCKOUT_COS
}

/// In-plane orthonormal basis around `axis`, right-handed about the
/// axis (`axis × u = v`). Used by every rotate primitive so the
/// angular position returned by [`axis_plane_angle`] grows CCW about
/// `+axis` (right-hand rule). Caller code that wants the user's
/// "screen clockwise = +rotation" convention flips the sign at
/// composition time, where the camera direction is also known.
#[allow(dead_code)]
fn axis_basis(axis: Axis) -> (Vec3, Vec3) {
    match axis {
        Axis::X => (Vec3::Y, Vec3::Z),
        Axis::Y => (Vec3::Z, Vec3::X),
        Axis::Z => (Vec3::X, Vec3::Y),
    }
}

/// Intersects `ray` with the plane through `pivot` perpendicular to
/// `axis`. Returns the world-space hit, or `None` if the ray is
/// parallel to the plane or hits behind the camera. The rotate
/// lock-out (`arc_pickable`) is what keeps `denom` away from zero
/// in the steady state; the small floor here is a defensive guard
/// for callers that bypass the lock-out (e.g. tests).
#[allow(dead_code)]
fn ray_hits_axis_plane(ray: &Ray, pivot: Vec3, axis: Axis) -> Option<Vec3> {
    let a = axis.world_dir();
    let denom = ray.direction.dot(a);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (pivot - ray.origin).dot(a) / denom;
    if t <= 0.0 {
        return None;
    }
    Some(ray.origin + ray.direction * t)
}

/// Angular position (radians, in `[-π, +π]`) of `point` around
/// `axis` using the right-handed basis from `axis_basis`. A point on
/// `pivot + u` returns 0; on `pivot + v` returns `π/2`; on
/// `pivot - u` returns ±π.
#[allow(dead_code)]
pub fn axis_plane_angle(point: Vec3, pivot: Vec3, axis: Axis) -> f32 {
    let (u, v) = axis_basis(axis);
    let p = point - pivot;
    p.dot(v).atan2(p.dot(u))
}

/// Cursor-ray → angular position around `axis` for the rotate drag.
/// `Drag::Rotate` captures one of these at press time as
/// `start_angle` and recomputes it every move event; the controller
/// composes `delta_angle = current - start` (with an unwrap counter
/// to follow the cursor across the ±π discontinuity) into a rotation
/// `R(axis, delta_angle)` about the world-space pivot.
///
/// Returns `None` only for the degenerate cases that
/// [`arc_pickable`] is supposed to forbid; the controller is
/// expected to gate this call on that lock-out at gesture start.
#[allow(dead_code)]
pub fn rotate_drag_angle(ray: &Ray, pivot: Vec3, axis: Axis) -> Option<f32> {
    let hit = ray_hits_axis_plane(ray, pivot, axis)?;
    Some(axis_plane_angle(hit, pivot, axis))
}

/// World-space midpoint of the rotate arc for `axis`, i.e. the
/// position of the two-cone spindle's shared base. The arc is the
/// 90° quadrant of the major circle (radius `arc_radius_world` in
/// the axis-perpendicular plane through `pivot`) centred on the
/// projection of the eye-to-pivot direction onto that plane.
///
/// Returns `None` when the camera sits on the axis itself — the
/// in-plane direction degenerates and the controller's `arc_pickable`
/// gate is expected to have rejected the axis upstream.
fn arc_midpoint_world(ray: &Ray, pivot: Vec3, axis: Axis, arc_radius_world: f32) -> Option<Vec3> {
    let a = axis.world_dir();
    let to_camera = ray.origin - pivot;
    let in_plane = to_camera - a * to_camera.dot(a);
    let len = in_plane.length();
    if len < 1e-6 {
        return None;
    }
    Some(pivot + in_plane * (arc_radius_world / len))
}

/// Hit-test the rotate handle for one axis. The handle is two cones
/// sharing their bases at the arc's camera-facing midpoint, tips
/// pointing along ±axis. Approximated as a single finite cylinder
/// bounding both cones (centre at the midpoint, oriented along the
/// axis, extending ±`cone_length_world`, radius `cone_radius_world`).
///
/// Returns the entry parameter `t` if the ray crosses the bounding
/// cylinder, or `None` if it misses. The torus *tube* itself is no
/// longer a click target — it's a visual aid that connects the
/// two-cone spindle around the rotation axis.
pub fn ray_hits_rotate_cones(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    arc_radius_world: f32,
    cone_length_world: f32,
    cone_radius_world: f32,
) -> Option<f32> {
    let midpoint = arc_midpoint_world(ray, pivot, axis, arc_radius_world)?;
    ray_hits_finite_cylinder(
        ray,
        midpoint,
        axis.world_dir(),
        -cone_length_world,
        cone_length_world,
        cone_radius_world,
    )
}

/// Hit-test the three world-aligned rotate handles against `ray`.
/// Returns the front-most pickable axis (smallest positive `t`)
/// whose rotate-cone spindle the ray intersects, paired with that
/// `t`, or `None`. Mirrors `hit_test`'s structure for the translate
/// cones, using `arc_pickable` as the lock-out and
/// `ray_hits_rotate_cones` as the per-axis test.
///
/// `arc_radius_world` is the major-circle radius (typically
/// `gizmo_scale * ROTATE_ARC_RADIUS_FRACTION`); `cone_length_world`
/// is each cone's length along the axis; `cone_radius_world` is the
/// shared cone base radius — both follow the per-frame world-per-
/// pixel ratio so the cones keep a constant on-screen size.
pub fn rotate_hit_test(
    ray: &Ray,
    pivot: Vec3,
    arc_radius_world: f32,
    cone_length_world: f32,
    cone_radius_world: f32,
) -> Option<(Axis, f32)> {
    let mut best: Option<(Axis, f32)> = None;
    for &axis in &ALL_AXES {
        if !arc_pickable(ray, axis) {
            continue;
        }
        if let Some(t) = ray_hits_rotate_cones(
            ray,
            pivot,
            axis,
            arc_radius_world,
            cone_length_world,
            cone_radius_world,
        ) {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((axis, t)),
            }
        }
    }
    best
}

/// Ray-vs-AABB for one axis's midpoint scale cube. The cube is
/// axis-aligned in world space, centred at
/// `pivot + axis_dir * shaft_length_world * MIDPOINT_FRACTION`,
/// with side length `cube_world`. Returns the entry parameter `t`
/// when the ray hits, or `None`.
#[allow(dead_code)]
pub fn ray_hits_axis_cube(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    shaft_length_world: f32,
    cube_world: f32,
) -> Option<f32> {
    let centre = pivot + axis.world_dir() * (shaft_length_world * MIDPOINT_FRACTION);
    let half = Vec3::splat(cube_world * 0.5);
    ray_hits_aabb_t(ray, centre - half, centre + half)
}

/// Ray-vs-AABB for the centre (uniform-scale) cube. The cube is
/// axis-aligned in world space, centred at `pivot`, with side
/// length `cube_world`.
#[allow(dead_code)]
pub fn ray_hits_centre_cube(ray: &Ray, pivot: Vec3, cube_world: f32) -> Option<f32> {
    let half = Vec3::splat(cube_world * 0.5);
    ray_hits_aabb_t(ray, pivot - half, pivot + half)
}

/// Hit-test the four scale handles (three midpoint cubes + the
/// centre cube) against `ray`. Returns
/// - `Some(Some(axis))` for a per-axis scale cube hit;
/// - `Some(None)` for the centre (uniform-scale) cube;
/// - `None` if the ray misses every cube.
///
/// Front-most-`t` wins across all four candidates. The per-axis
/// cubes inherit the translate lock-out (the closest-point-on-axis
/// drag math has the same instability as translate near the
/// camera-parallel-to-axis configuration); the centre cube has no
/// per-axis lock-out since its drag uses screen-space distance.
#[allow(dead_code)]
pub fn scale_hit_test(
    ray: &Ray,
    pivot: Vec3,
    shaft_length_world: f32,
    axis_cube_world: f32,
    centre_cube_world: f32,
) -> Option<(Option<Axis>, f32)> {
    let mut best: Option<(Option<Axis>, f32)> = None;

    if let Some(t) = ray_hits_centre_cube(ray, pivot, centre_cube_world) {
        best = Some((None, t));
    }

    for &axis in &ALL_AXES {
        if !axis_pickable(ray, axis) {
            continue;
        }
        if let Some(t) = ray_hits_axis_cube(ray, pivot, axis, shaft_length_world, axis_cube_world) {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((Some(axis), t)),
            }
        }
    }

    best
}

/// Per-axis scale factor for a drag whose press anchor was
/// `start_anchor` and whose current anchor is `current_anchor`.
/// Both anchors are world-space points on the axis line through
/// `pivot` (typically produced by `closest_point_on_axis`). The raw
/// ratio `current_proj / start_proj` is clamped into
/// `[EPS_SCALE, 1/EPS_SCALE]`, which also rejects negative ratios
/// — mirroring through the gizmo is disallowed.
///
/// Returns `1.0` (a no-op factor) when the press anchor coincides
/// with the pivot along the axis: the ratio is undefined there
/// and the drag should not move the object until the cursor leaves
/// the pivot's neighbourhood.
#[allow(dead_code)]
pub fn scale_factor_along_axis(
    start_anchor: Vec3,
    current_anchor: Vec3,
    pivot: Vec3,
    axis: Axis,
) -> f32 {
    let a = axis.world_dir();
    let start_proj = (start_anchor - pivot).dot(a);
    let current_proj = (current_anchor - pivot).dot(a);
    if start_proj.abs() < 1e-6 {
        return 1.0;
    }
    (current_proj / start_proj).clamp(EPS_SCALE, 1.0 / EPS_SCALE)
}

/// Uniform scale factor for a drag of the centre cube (or the
/// per-axis cube with the proportional-scale modifier held). Driven
/// by the screen-space distance between the cursor and the
/// projected pivot: the factor is the ratio of the current distance
/// to the press distance, clamped into `[EPS_SCALE, 1/EPS_SCALE]`.
///
/// `pivot_screen_*` must be the projection of the world-space
/// pivot into the same logical-pixel coordinate space as the
/// cursor positions. The controller computes that projection from
/// its camera + viewport once per move event.
///
/// Returns `1.0` when the press lands within `1e-6` pixels of the
/// projected pivot — the ratio is undefined there.
#[allow(dead_code)]
pub fn uniform_scale_factor(
    pivot_screen_x: f32,
    pivot_screen_y: f32,
    x_press: f32,
    y_press: f32,
    x_now: f32,
    y_now: f32,
) -> f32 {
    let dx0 = x_press - pivot_screen_x;
    let dy0 = y_press - pivot_screen_y;
    let start_dist_sq = dx0 * dx0 + dy0 * dy0;
    if start_dist_sq < 1e-12 {
        return 1.0;
    }
    let dx1 = x_now - pivot_screen_x;
    let dy1 = y_now - pivot_screen_y;
    let current_dist_sq = dx1 * dx1 + dy1 * dy1;
    let raw = (current_dist_sq / start_dist_sq).sqrt();
    raw.clamp(EPS_SCALE, 1.0 / EPS_SCALE)
}

#[cfg(test)]
mod axis_tests {
    use super::*;

    #[test]
    fn world_dir_returns_basis_vectors() {
        assert_eq!(Axis::X.world_dir(), Vec3::X);
        assert_eq!(Axis::Y.world_dir(), Vec3::Y);
        assert_eq!(Axis::Z.world_dir(), Vec3::Z);
    }
}

#[cfg(test)]
mod pivot_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;

    fn triangle_mesh(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new(v0), Vertex::new(v1), Vertex::new(v2)],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        }
    }

    #[test]
    fn pivot_under_identity_is_local_aabb_centre() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(triangle_mesh(
                    [-1.0, -2.0, -3.0],
                    [3.0, 0.0, 0.0],
                    [-1.0, 4.0, 5.0],
                )),
            }],
        };
        let p = pivot_for_object(&scene, 0).unwrap();
        // local AABB = ([-1, -2, -3], [3, 4, 5]) → centre (1, 1, 1)
        assert_approx_eq!(f32, p.x, 1.0);
        assert_approx_eq!(f32, p.y, 1.0);
        assert_approx_eq!(f32, p.z, 1.0);
    }

    #[test]
    fn pivot_under_translation_shifts_with_object() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Affine(AffineTransform3D::from_translation([
                    10.0, 20.0, 30.0,
                ])),
                content: SceneObjectContent::Mesh(triangle_mesh(
                    [-1.0, -2.0, -3.0],
                    [3.0, 0.0, 0.0],
                    [-1.0, 4.0, 5.0],
                )),
            }],
        };
        let p = pivot_for_object(&scene, 0).unwrap();
        // Local centre (1, 1, 1) translated by (10, 20, 30).
        assert_approx_eq!(f32, p.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 31.0, epsilon = 1e-5);
    }

    #[test]
    fn pivot_for_empty_mesh_is_none() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(IndexedMesh {
                    meta: None,
                    vertices: vec![],
                    faces: vec![],
                }),
            }],
        };
        assert!(pivot_for_object(&scene, 0).is_none());
    }

    #[test]
    fn pivot_for_out_of_range_index_is_none() {
        let scene = Scene::default();
        assert!(pivot_for_object(&scene, 99).is_none());
    }

    #[test]
    fn pivot_for_dangling_resource_is_none() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Resource(7),
            }],
        };
        assert!(pivot_for_object(&scene, 0).is_none());
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    fn cam_at_distance(d: f32) -> OrbitCamera {
        let mut c = OrbitCamera::new(Vec3::ZERO, 1.0);
        c.distance = d;
        c
    }

    #[test]
    fn gizmo_scale_grows_with_camera_distance() {
        let near = gizmo_scale(&cam_at_distance(2.0), 1.0, 480.0);
        let far = gizmo_scale(&cam_at_distance(20.0), 1.0, 480.0);
        // 10× farther → 10× larger world units per pixel → 10× scale.
        assert!(far > near * 9.0 && far < near * 11.0);
    }

    #[test]
    fn gizmo_scale_shrinks_with_viewport_height() {
        let small_h = gizmo_scale(&cam_at_distance(5.0), 1.0, 240.0);
        let big_h = gizmo_scale(&cam_at_distance(5.0), 1.0, 1080.0);
        assert!(big_h < small_h);
    }
}

#[cfg(test)]
mod axis_pickable_tests {
    use super::*;

    fn ray_along(d: Vec3) -> Ray {
        Ray {
            origin: Vec3::ZERO,
            direction: d.normalize(),
        }
    }

    #[test]
    fn perpendicular_ray_is_pickable() {
        // Ray along +Z, axis along +X: perfectly perpendicular.
        assert!(axis_pickable(&ray_along(Vec3::Z), Axis::X));
    }

    #[test]
    fn ray_exactly_along_axis_is_locked_out() {
        assert!(!axis_pickable(&ray_along(Vec3::X), Axis::X));
    }

    #[test]
    fn ray_within_threshold_is_locked_out() {
        // 5° off-axis: still inside the 8° lock-out.
        let theta = 5.0_f32.to_radians();
        let d = Vec3::new(theta.cos(), theta.sin(), 0.0);
        assert!(!axis_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn ray_just_outside_threshold_is_pickable() {
        // 9° off-axis: outside the 8° lock-out, should be grabbable.
        let theta = 9.0_f32.to_radians();
        let d = Vec3::new(theta.cos(), theta.sin(), 0.0);
        assert!(axis_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn lockout_is_symmetric_in_axis_direction() {
        // Ray pointing along -X is just as locked-out as +X — the
        // dot-product magnitude check is sign-agnostic.
        assert!(!axis_pickable(&ray_along(-Vec3::X), Axis::X));
    }
}

#[cfg(test)]
mod ray_hits_finite_cylinder_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn perpendicular_ray_hits_cylinder_at_radius_distance() {
        // Cylinder centred at origin along +X, s ∈ [-1, 1], radius 0.05.
        // Ray crosses through (+0.5, 0, 0) along -Z; entry at z=+0.05.
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let t = ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).unwrap();
        assert_approx_eq!(f32, t, 5.0 - 0.05, epsilon = 1e-4);
    }

    #[test]
    fn ray_outside_radius_misses() {
        let ray = Ray {
            origin: Vec3::new(0.5, 1.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_outside_s_range_misses() {
        // Ray hits the X axis at x = 5; cylinder s ∈ [-1, 1] excludes
        // it. Even with the wider radius the s-test rejects.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_pointing_away_from_cylinder_misses() {
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: Vec3::Z, // away from origin
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).is_none());
    }

    #[test]
    fn translate_cone_region_rejects_shaft_clicks() {
        // Translate hit uses s ∈ [length, length·1.25] — clicks on
        // the shaft (s in [0, length]) must miss. Pin the contract.
        // length = 1.0; ray crosses (0.5, 0, *) at s = 0.5.
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, 1.0, 1.25, 0.05).is_none());
    }

    #[test]
    fn translate_cone_region_accepts_cone_clicks() {
        // Same cylinder, ray crosses at s = 1.1 (inside cone region).
        let ray = Ray {
            origin: Vec3::new(1.1, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, 1.0, 1.25, 0.05).is_some());
    }
}

#[cfg(test)]
mod hit_test_tests {
    use super::*;

    #[test]
    fn empty_space_returns_none() {
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: -Vec3::Z,
        };
        assert!(hit_test(&ray, Vec3::ZERO, 1.0, 0.05).is_none());
    }

    #[test]
    fn x_axis_ray_picks_x_handle() {
        // Ray crosses (1.1, 0, 0) — inside the cone region
        // (s ∈ [1.0, 1.25]).
        let ray = Ray {
            origin: Vec3::new(1.1, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            hit_test(&ray, Vec3::ZERO, 1.0, 0.05).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn shaft_line_clicks_miss_translate_handle() {
        // Click on the visible shaft line (s ∈ [0, 1.0)) must NOT
        // start a translate drag — the shaft is decorative; only the
        // cone tip is the active translate handle. The midpoint
        // (s = 0.5) belongs to the per-axis scale cube anyway.
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(hit_test(&ray, Vec3::ZERO, 1.0, 0.05).is_none());
    }

    #[test]
    fn frontmost_handle_wins_when_screen_overlap() {
        // Both X and Y cones near the cone region. Construct a ray
        // that hits the X cone (s ≈ 1.1 along +X). The Y cone is
        // around (0, 1.1, *), which the ray (offset 1.1 in x) is
        // far from — Y misses, X wins.
        let ray = Ray {
            origin: Vec3::new(1.1, 0.05, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            hit_test(&ray, Vec3::ZERO, 1.0, 0.05).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn locked_out_axis_is_not_picked() {
        // Ray pointing nearly along +X — X is locked out by the
        // 8° threshold in `axis_pickable`.
        // A barely-perpendicular component still places the ray
        // close enough to the X handle that the cylinder math would
        // otherwise hit. The lock-out should override.
        let theta = 5.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), 0.0, theta.sin()).normalize();
        let ray = Ray {
            origin: Vec3::new(-1.0, 0.0, -theta.tan()),
            direction: dir,
        };
        // We don't assert non-None for a lock-out-disabled axis;
        // we assert it's not X. Y/Z might or might not be hit
        // depending on the geometry — the only contract is X is
        // forbidden.
        let picked = hit_test(&ray, Vec3::ZERO, 1.0, 0.05).map(|(a, _)| a);
        assert_ne!(picked, Some(Axis::X));
    }
}

#[cfg(test)]
mod closest_point_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ray_intersecting_axis_returns_intersection() {
        // Ray crosses X-axis at x = 2.
        let ray = Ray {
            origin: Vec3::new(2.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let p = closest_point_on_axis(&ray, Vec3::ZERO, Axis::X);
        assert_approx_eq!(f32, p.x, 2.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn ray_perpendicular_returns_foot_of_perpendicular() {
        // Ray along +Z passing through (3, 4, 0). X-axis is the line
        // y=z=0; foot of perpendicular from (3, 4, *) onto X-axis is
        // (3, 0, 0).
        let ray = Ray {
            origin: Vec3::new(3.0, 4.0, -5.0),
            direction: Vec3::Z,
        };
        let p = closest_point_on_axis(&ray, Vec3::ZERO, Axis::X);
        assert_approx_eq!(f32, p.x, 3.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn cursor_delta_along_screen_yields_axis_delta() {
        // The whole point of the formula: dragging the cursor along
        // the X handle's screen projection should produce a clean
        // world-space X translation, not contaminated by Y/Z.
        let pivot = Vec3::ZERO;
        let r0 = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let r1 = Ray {
            origin: Vec3::new(2.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let p0 = closest_point_on_axis(&r0, pivot, Axis::X);
        let p1 = closest_point_on_axis(&r1, pivot, Axis::X);
        let delta = p1 - p0;
        assert_approx_eq!(f32, delta.x, 2.5, epsilon = 1e-5);
        assert_approx_eq!(f32, delta.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, delta.z, 0.0, epsilon = 1e-5);
    }
}

#[cfg(test)]
mod arc_pickable_tests {
    use super::*;

    fn ray_along(d: Vec3) -> Ray {
        Ray {
            origin: Vec3::ZERO,
            direction: d.normalize(),
        }
    }

    #[test]
    fn ray_perpendicular_to_arc_plane_is_pickable() {
        // X-axis arc lives in the YZ plane. A ray along +X is
        // perfectly perpendicular to that plane, i.e. axis-parallel,
        // so the arc faces the camera squarely.
        assert!(arc_pickable(&ray_along(Vec3::X), Axis::X));
    }

    #[test]
    fn ray_in_arc_plane_is_locked_out() {
        // X-axis arc lives in the YZ plane. A ray along +Y lies
        // exactly in that plane — the arc is edge-on.
        assert!(!arc_pickable(&ray_along(Vec3::Y), Axis::X));
        assert!(!arc_pickable(&ray_along(Vec3::Z), Axis::X));
    }

    #[test]
    fn ray_within_5_degrees_of_arc_plane_is_locked_out() {
        // 5° off the YZ plane (the X arc's plane) → 85° from +X.
        // |axis · ray| = sin(5°) ≈ 0.087 < cos(82°) ≈ 0.139.
        let theta = 5.0_f32.to_radians();
        let d = Vec3::new(theta.sin(), theta.cos(), 0.0);
        assert!(!arc_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn ray_just_outside_threshold_is_pickable() {
        // 9° off the YZ plane → 81° from +X.
        // |axis · ray| = sin(9°) ≈ 0.156 > cos(82°) ≈ 0.139.
        let theta = 9.0_f32.to_radians();
        let d = Vec3::new(theta.sin(), theta.cos(), 0.0);
        assert!(arc_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn lockout_is_symmetric_in_axis_direction() {
        // Ray along -X is just as pickable as +X — the
        // dot-product magnitude is sign-agnostic.
        assert!(arc_pickable(&ray_along(-Vec3::X), Axis::X));
    }
}

#[cfg(test)]
mod axis_plane_angle_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn point_on_u_axis_is_zero() {
        // For Axis::X, basis is u=Y, v=Z; point on +Y at angle 0.
        let p = Vec3::new(0.0, 1.0, 0.0);
        assert_approx_eq!(
            f32,
            axis_plane_angle(p, Vec3::ZERO, Axis::X),
            0.0,
            epsilon = 1e-6
        );
    }

    #[test]
    fn point_on_v_axis_is_quarter_turn() {
        // For Axis::X, basis is u=Y, v=Z; point on +Z at angle π/2.
        let p = Vec3::new(0.0, 0.0, 1.0);
        assert_approx_eq!(
            f32,
            axis_plane_angle(p, Vec3::ZERO, Axis::X),
            std::f32::consts::FRAC_PI_2,
            epsilon = 1e-6
        );
    }

    #[test]
    fn pivot_translation_does_not_change_angle() {
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let p = pivot + Vec3::new(0.0, 0.0, 1.0);
        assert_approx_eq!(
            f32,
            axis_plane_angle(p, pivot, Axis::X),
            std::f32::consts::FRAC_PI_2,
            epsilon = 1e-6
        );
    }

    #[test]
    fn rh_rule_holds_for_each_axis() {
        // For each axis, the basis (u, v) is right-handed about +axis.
        // Y-axis: u=Z, v=X; point on +Z is at angle 0.
        assert_approx_eq!(
            f32,
            axis_plane_angle(Vec3::Z, Vec3::ZERO, Axis::Y),
            0.0,
            epsilon = 1e-6
        );
        // Z-axis: u=X, v=Y; point on +X is at angle 0.
        assert_approx_eq!(
            f32,
            axis_plane_angle(Vec3::X, Vec3::ZERO, Axis::Z),
            0.0,
            epsilon = 1e-6
        );
    }
}

#[cfg(test)]
mod rotate_drag_angle_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ray_through_pivot_plane_returns_in_plane_angle() {
        // X-axis arc plane = YZ. Camera looks along -X from (5, 1, 0)
        // and the ray hits the YZ plane at (0, 1, 0) — on the +u (Y)
        // axis, angle 0.
        let ray = Ray {
            origin: Vec3::new(5.0, 1.0, 0.0),
            direction: -Vec3::X,
        };
        let theta = rotate_drag_angle(&ray, Vec3::ZERO, Axis::X).unwrap();
        assert_approx_eq!(f32, theta, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn ray_parallel_to_plane_returns_none() {
        // X-axis arc plane = YZ. A ray along +Y has zero +X component,
        // so it never crosses the YZ plane (apart from the degenerate
        // case where the ray's origin lies in the plane, which the
        // implementation also rejects via the `t > 0` check).
        let ray = Ray {
            origin: Vec3::new(1.0, 0.0, 0.5),
            direction: Vec3::Y,
        };
        assert!(rotate_drag_angle(&ray, Vec3::ZERO, Axis::X).is_none());
    }

    #[test]
    fn ray_pointing_away_returns_none() {
        // Plane is at x=0; ray sits at x=1 going +X (away from plane).
        let ray = Ray {
            origin: Vec3::new(1.0, 0.0, 0.0),
            direction: Vec3::X,
        };
        assert!(rotate_drag_angle(&ray, Vec3::ZERO, Axis::X).is_none());
    }

    #[test]
    fn moving_cursor_around_pivot_yields_increasing_angle() {
        // Two rays crossing the YZ plane on the camera-facing side
        // (camera at +X), one at angle 0 (point on +Y), one rotated
        // ~30° CCW about +X (point at (0, cos 30°, sin 30°)).
        let r0 = Ray {
            origin: Vec3::new(5.0, 1.0, 0.0),
            direction: -Vec3::X,
        };
        let theta = 30.0_f32.to_radians();
        let r1 = Ray {
            origin: Vec3::new(5.0, theta.cos(), theta.sin()),
            direction: -Vec3::X,
        };
        let a0 = rotate_drag_angle(&r0, Vec3::ZERO, Axis::X).unwrap();
        let a1 = rotate_drag_angle(&r1, Vec3::ZERO, Axis::X).unwrap();
        assert_approx_eq!(f32, a1 - a0, theta, epsilon = 1e-4);
    }
}

#[cfg(test)]
mod ray_hits_rotate_cones_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ray_through_camera_facing_midpoint_hits() {
        // X-axis arc midpoint faces the camera. Place camera at
        // (5, 0.1, 0): the in-plane camera direction (in YZ) is +Y,
        // so the arc midpoint is at world (0, 1, 0) for arc_radius=1.
        // The two rotate cones share their base there, tips at +X and
        // −X with cone_length = 0.18. A ray crossing (0, 1, 0) along
        // -X enters the bounding cylinder.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(ray_hits_rotate_cones(&ray, Vec3::ZERO, Axis::X, 1.0, 0.18, 0.07).is_some());
    }

    #[test]
    fn arc_midpoint_falls_back_when_camera_on_axis() {
        // Camera exactly on +X axis → in-plane camera projection is
        // zero → the helper returns `None`, and the hit test follows
        // suit.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.0, 0.0),
            direction: -Vec3::X,
        };
        assert!(arc_midpoint_world(&ray, Vec3::ZERO, Axis::X, 1.0).is_none());
        assert!(ray_hits_rotate_cones(&ray, Vec3::ZERO, Axis::X, 1.0, 0.18, 0.07).is_none());
    }

    #[test]
    fn ray_far_from_midpoint_misses() {
        // Camera at (5, 0.1, 0), arc midpoint at world (0, 1, 0). A
        // ray crossing (0, -1, 0) — the *back* side of the rotation
        // axis, 180° away — is far from the midpoint cone spindle.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, -1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(ray_hits_rotate_cones(&ray, Vec3::ZERO, Axis::X, 1.0, 0.18, 0.07).is_none());
    }

    #[test]
    fn arc_midpoint_lies_on_camera_facing_radial() {
        // Sanity for the helper: with camera at (0, 0, 5), the X
        // arc's in-plane camera direction is +Z, so the midpoint
        // sits at (0, 0, R).
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let m = arc_midpoint_world(&ray, Vec3::ZERO, Axis::X, 1.5).unwrap();
        assert_approx_eq!(f32, m.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, m.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, m.z, 1.5, epsilon = 1e-6);
    }
}

#[cfg(test)]
mod rotate_hit_test_tests {
    use super::*;

    #[test]
    fn empty_space_returns_none() {
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        assert!(rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.18, 0.07).is_none());
    }

    #[test]
    fn ray_through_x_arc_midpoint_picks_x() {
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert_eq!(
            rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.18, 0.07).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn locked_out_axis_is_skipped() {
        // X arc lives in the YZ plane; ray inside that plane is
        // edge-on — `arc_pickable` rejects X. Y/Z may or may not be
        // hit; the only contract is X is forbidden.
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: -Vec3::Y,
        };
        assert_ne!(
            rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.18, 0.07).map(|(a, _)| a),
            Some(Axis::X)
        );
    }
}

#[cfg(test)]
mod ray_hits_axis_cube_tests {
    use super::*;

    #[test]
    fn cube_sits_at_shaft_midpoint_not_tip() {
        // X-axis cube with shaft length 2.0 must be centred at x = 1.0
        // (midpoint), *not* x = 2.0 (tip). Ray pointing through (1, 0)
        // hits; ray pointing through (2, 0) misses (cube extents are
        // [0.95, 1.05] for cube_world = 0.1).
        let ray_at_midpoint = Ray {
            origin: Vec3::new(1.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let ray_at_tip = Ray {
            origin: Vec3::new(2.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray_at_midpoint, Vec3::ZERO, Axis::X, 2.0, 0.1).is_some());
        assert!(ray_hits_axis_cube(&ray_at_tip, Vec3::ZERO, Axis::X, 2.0, 0.1).is_none());
    }

    #[test]
    fn cube_centre_uses_axis_dir() {
        // Y-axis cube on a shaft of length 4 sits at (0, 2, 0).
        let ray = Ray {
            origin: Vec3::new(0.0, 2.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray, Vec3::ZERO, Axis::Y, 4.0, 0.1).is_some());
    }

    #[test]
    fn ray_outside_cube_misses() {
        let ray = Ray {
            origin: Vec3::new(1.0, 5.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray, Vec3::ZERO, Axis::X, 2.0, 0.1).is_none());
    }

    #[test]
    fn pivot_translation_carries_cube() {
        // Scene-space pivot at (10, 20, 30) → X cube centred at
        // (10 + 1, 20, 30) for shaft length 2.
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let ray = Ray {
            origin: Vec3::new(11.0, 20.0, 35.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray, pivot, Axis::X, 2.0, 0.1).is_some());
    }
}

#[cfg(test)]
mod ray_hits_centre_cube_tests {
    use super::*;

    #[test]
    fn ray_through_pivot_hits() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_centre_cube(&ray, Vec3::ZERO, 0.2).is_some());
    }

    #[test]
    fn ray_outside_cube_misses() {
        let ray = Ray {
            origin: Vec3::new(1.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_centre_cube(&ray, Vec3::ZERO, 0.2).is_none());
    }
}

#[cfg(test)]
mod scale_hit_test_tests {
    use super::*;

    #[test]
    fn ray_through_centre_returns_some_none() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, 0.2).map(|(a, _)| a),
            Some(None)
        );
    }

    #[test]
    fn ray_through_x_midpoint_returns_some_some_x() {
        let ray = Ray {
            origin: Vec3::new(1.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, 0.2).map(|(a, _)| a),
            Some(Some(Axis::X))
        );
    }

    #[test]
    fn empty_space_returns_none() {
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: -Vec3::Z,
        };
        assert!(scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, 0.2).is_none());
    }

    #[test]
    fn frontmost_t_wins_between_axis_cubes() {
        // A ray hitting both Y and Z midpoint cubes: starts at
        // (0, 5, -4), direction toward (0, 1, 0) then continuing on
        // through (0, 0, 1). Y cube hits first (smaller `t`).
        // Direction is 45° off both Y and Z so neither axis is
        // locked out.
        let dir = Vec3::new(0.0, -1.0, 1.0).normalize();
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, -4.0),
            direction: dir,
        };
        assert_eq!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, 0.2).map(|(a, _)| a),
            Some(Some(Axis::Y))
        );
    }

    #[test]
    fn locked_out_axis_cube_is_skipped() {
        // Ray nearly along +X — X axis is lock-out; the X cube must
        // not be returned even if the ray geometrically crosses it.
        let theta = 5.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), 0.0, theta.sin()).normalize();
        let ray = Ray {
            origin: Vec3::new(-5.0, 0.0, 0.0),
            direction: dir,
        };
        let hit = scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, 0.2).map(|(a, _)| a);
        assert_ne!(hit, Some(Some(Axis::X)));
    }
}

#[cfg(test)]
mod scale_factor_along_axis_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ratio_of_anchor_projections_drives_factor() {
        // Press anchor at x = 1, current anchor at x = 2 → factor 2.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn off_axis_anchor_components_are_ignored() {
        // Only the projection onto the axis matters; y/z drift is
        // irrelevant.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 100.0, -200.0),
            Vec3::new(0.5, 999.0, 500.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 0.5, epsilon = 1e-6);
    }

    #[test]
    fn pivot_translation_does_not_change_factor() {
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let factor = scale_factor_along_axis(
            pivot + Vec3::new(2.0, 0.0, 0.0),
            pivot + Vec3::new(3.0, 0.0, 0.0),
            pivot,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 1.5, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_lower_bound() {
        // Press at x = 1, current at x = +tiny → ratio under EPS,
        // clamped to EPS_SCALE.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1e-9, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, EPS_SCALE, epsilon = 1e-6);
    }

    #[test]
    fn negative_ratio_is_clamped_not_mirrored() {
        // Cursor crosses the pivot to the other side: raw ratio is
        // negative, which the clamp turns into the lower bound.
        // Mirroring through the gizmo is disallowed.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-2.0, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, EPS_SCALE, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_upper_bound() {
        // start_proj > 1e-6 (the press-at-pivot guard) but small
        // enough that the raw ratio exceeds `1/EPS_SCALE`.
        let factor = scale_factor_along_axis(
            Vec3::new(1e-4, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 1.0 / EPS_SCALE, epsilon = 1e-3);
    }

    #[test]
    fn press_at_pivot_returns_unit_factor() {
        // start_proj ≈ 0 → factor undefined; the no-op fallback is
        // 1.0 so the drag doesn't move the object until the cursor
        // leaves the pivot's neighbourhood.
        let factor =
            scale_factor_along_axis(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO, Axis::X);
        assert_approx_eq!(f32, factor, 1.0, epsilon = 1e-6);
    }
}

#[cfg(test)]
mod uniform_scale_factor_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ratio_of_distances_drives_factor() {
        // Press 50 px away, now 100 px away → factor 2.
        let factor = uniform_scale_factor(0.0, 0.0, 50.0, 0.0, 100.0, 0.0);
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn factor_uses_euclidean_distance() {
        // Press at (3, 4) → distance 5. Now at (6, 8) → distance 10.
        // Factor = 10 / 5 = 2.
        let factor = uniform_scale_factor(0.0, 0.0, 3.0, 4.0, 6.0, 8.0);
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn pivot_offset_does_not_change_factor() {
        let factor = uniform_scale_factor(100.0, 200.0, 150.0, 200.0, 200.0, 200.0);
        // Press 50 px away, now 100 px away → factor 2.
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_lower_bound() {
        // Cursor moves toward the pivot — ratio approaches zero,
        // clamps at EPS_SCALE.
        let factor = uniform_scale_factor(0.0, 0.0, 100.0, 0.0, 1e-9, 0.0);
        assert_approx_eq!(f32, factor, EPS_SCALE, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_upper_bound() {
        let factor = uniform_scale_factor(0.0, 0.0, 1e-3, 0.0, 1e6, 0.0);
        assert_approx_eq!(f32, factor, 1.0 / EPS_SCALE, epsilon = 1e-3);
    }

    #[test]
    fn press_at_pivot_returns_unit_factor() {
        // Press lands exactly on the projected pivot → no defined
        // factor; helper returns 1.0 so the drag is a no-op.
        let factor = uniform_scale_factor(0.0, 0.0, 0.0, 0.0, 50.0, 0.0);
        assert_approx_eq!(f32, factor, 1.0, epsilon = 1e-6);
    }
}

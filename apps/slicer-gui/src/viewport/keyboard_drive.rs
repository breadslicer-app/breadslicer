/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Keyboard-driven gizmo integrator. Data types (`KbdFocus`, `ArrowDrive`), the
//! per-tick pure function `keyboard_step`, and the `KeyboardDriveState` struct
//! that owns the controller's three keyboard cells (focus, arrow bitset, last
//! integrator tick) and drives them through the Slint callbacks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use glam::Vec3;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;

use super::gizmo_drag::{axis_rotation, compose_about_pivot};
use super::group_move::GroupMove;
use super::types::ViewportSize;
use crate::camera::OrbitCamera;
use crate::gizmo::axis::Axis;
use crate::mesh_render::default_fov_y;
use crate::scene_document::{SceneDocument, SceneEdit};

/// Keyboard gizmo focus. `T`/`R`/`S` keypresses cycle this through the three
/// operation types; Esc returns to `None`. While focus is non-`None`, the arrow
/// / PageUp / PageDown keys are hijacked by the per-frame integrator and no
/// longer drive the camera.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub(super) enum KbdFocus {
    #[default]
    None,
    Translate,
    Rotate,
    Scale,
}

/// Bitset of held arrow-driving keys, with the per-axis sign: `Right`/`Left` =
/// ±X, `Up`/`Down` = ±Y, `PageUp`/`PageDown` = ±Z. Driven by `key-pressed` /
/// `key-released` from the FocusScope and sampled every animation tick to
/// integrate continuous motion.
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
    /// Per-axis sign as `(±1, ±1, ±1)`, with `0` for any axis whose keys are
    /// both unpressed (or both pressed at once, in which case they cancel).
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

/// Codes carried on `gizmo-arrow-pressed` / `gizmo-arrow-released`. `0/1` = ±X,
/// `2/3` = ±Y, `4/5` = ±Z. The Slint layer uses bare `int`s because Slint's
/// typed enums can't be passed through a callback that's bound at runtime; the
/// Rust layer normalises into the `ArrowDrive` bitset above.
pub(super) const ARROW_X_POS: i32 = 0;
pub(super) const ARROW_X_NEG: i32 = 1;
pub(super) const ARROW_Y_POS: i32 = 2;
pub(super) const ARROW_Y_NEG: i32 = 3;
pub(super) const ARROW_Z_POS: i32 = 4;
pub(super) const ARROW_Z_NEG: i32 = 5;

/// Translation rate for keyboard-focus drive: 30 logical pixels of
/// screen-projected motion per second along the chosen world axis. Converted to
/// world units per frame at the depth of the pivot.
const KBD_TRANSLATE_PIXELS_PER_SEC: f32 = 30.0;

/// Rotation rate for keyboard-focus drive: 45° per second about the chosen
/// world axis.
const KBD_ROTATE_RAD_PER_SEC: f32 = std::f32::consts::FRAC_PI_4;

/// Scale-rate base for keyboard-focus drive: factor `1.5` per second of held
/// key. Combined with per-axis sign, the effective per-axis factor each tick is
/// `1.5 ^ (sign * dt)`.
const KBD_SCALE_BASE: f32 = 1.5;

/// Maximum integrator step. Caps the per-frame delta so a stalled frame
/// (debugger pause, garbage collection, anything that delays the next render
/// past ~100 ms) doesn't translate the object across the viewport in a single
/// jump when the loop resumes. Applied at the controller's call site, before
/// `keyboard_step` is invoked.
pub(super) const KBD_MAX_DT: f32 = 0.1;

/// Pure helper: produces one tick of the keyboard-focus integrator. Returns
/// `None` when there's nothing to integrate (no axes held, or the focus is
/// `None`); otherwise returns the new running transform composed onto the
/// previous frame's `running` transform.
///
/// `dt` is the per-tick delta in seconds; callers clamp it to `KBD_MAX_DT` and
/// gate against non-positive values before calling.
///
/// Translate steps compose as `T(delta) ∘ running`. Rotate steps compose as
/// `T(p) ∘ R_z ∘ R_y ∘ R_x ∘ T(-p) ∘ running`, a fixed canonical order across
/// held axes; for the small per-tick angles this matters only by a second-order
/// amount. Scale steps compose as `T(p) ∘ diag(s) ∘ T(-p) ∘ running`, with
/// off-axes held at `1.0`.
pub(super) fn keyboard_step(
    focus: KbdFocus,
    arrows: ArrowDrive,
    dt: f32,
    pivot: Vec3,
    parent_world: &Transform3D,
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
                    t = compose_about_pivot(&r, pivot, parent_world, &t);
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
            Some(compose_about_pivot(
                &scale_mat,
                pivot,
                parent_world,
                running,
            ))
        }
    }
}

/// Per-frame inputs the keyboard integrator pulls off the controller. Bundling
/// them avoids a five-arg method while keeping ownership where it lives (scene
/// on the document, camera in a `RefCell` on the controller).
pub(super) struct IntegrationCtx<'a> {
    pub(super) document: &'a Rc<SceneDocument>,
    pub(super) camera: &'a RefCell<Option<OrbitCamera>>,
    pub(super) viewport: Option<ViewportSize>,
    pub(super) selection_id: Option<NodeId>,
}

/// Tracks the in-progress "keyboard burst": the user holds one or more arrow /
/// page keys while a gizmo focus is active. The first held key arms the tracker
/// with the object's transform at that moment; the burst ends when all keys are
/// released, the focus changes, or `reset` is called.
///
/// `last_event_at` is currently unused but written on every event so a future
/// coalesce window (one undo entry per ~Xms regardless of release/re-press) can
/// gate the flush from inside `take` without touching the public API.
struct BurstTracker {
    armed: Cell<Option<(NodeId, Transform3D)>>,
    last_event_at: Cell<Option<Instant>>,
}

impl BurstTracker {
    fn new() -> Self {
        Self {
            armed: Cell::new(None),
            last_event_at: Cell::new(None),
        }
    }

    fn is_armed(&self) -> bool {
        self.armed.get().is_some()
    }

    fn arm(&self, node_id: NodeId, before: Transform3D) {
        self.armed.set(Some((node_id, before)));
    }

    /// Returns the captured pre-image and clears the tracker. Returns `None`
    /// when nothing is armed.
    fn take(&self) -> Option<(NodeId, Transform3D)> {
        self.armed.take()
    }

    fn discard(&self) {
        self.armed.set(None);
    }

    fn note_event(&self) {
        self.last_event_at.set(Some(Instant::now()));
    }
}

/// Owns the controller's three keyboard cells and the methods that mutate them.
/// Used by `ViewportController` through composition.
pub(super) struct KeyboardDriveState {
    focus: Cell<KbdFocus>,
    arrows: Cell<ArrowDrive>,
    last_tick: Cell<Option<Instant>>,
    burst: BurstTracker,
    /// The leaves a burst moves when several are selected (#13): they all
    /// follow the offset the arrows give the primary one.
    group: RefCell<Option<Rc<GroupMove>>>,
}

impl KeyboardDriveState {
    pub(super) fn new() -> Self {
        Self {
            focus: Cell::new(KbdFocus::None),
            arrows: Cell::new(ArrowDrive::default()),
            last_tick: Cell::new(None),
            burst: BurstTracker::new(),
            group: RefCell::new(None),
        }
    }

    pub(super) fn current_focus(&self) -> KbdFocus {
        self.focus.get()
    }

    /// Visible to sibling tests for asserting bitset state.
    #[cfg(test)]
    pub(super) fn current_arrows(&self) -> ArrowDrive {
        self.arrows.get()
    }

    /// Force-clear focus + bitset + tick. Used on scene close. Any in-progress
    /// burst is dropped without flushing; the object indices it referred to are
    /// about to become meaningless.
    pub(super) fn reset(&self) {
        self.focus.set(KbdFocus::None);
        self.arrows.set(ArrowDrive::default());
        self.last_tick.set(None);
        self.burst.discard();
        self.group.replace(None);
    }

    /// Sets `focus` if the move is legal. Returns `true` when the cell changed;
    /// the caller should mark the controller dirty and update the
    /// `gizmo-keyboard-focus-active` UI property accordingly. Refuses
    /// non-`None` focus when nothing is selected, and is a no-op when the focus
    /// is already `focus`.
    ///
    /// Side effect: an in-progress burst is flushed before the focus switch, so
    /// the user's key-presses-so-far land as one undo entry instead of bleeding
    /// across modes.
    pub(super) fn try_set_focus(
        &self,
        document: &Rc<SceneDocument>,
        focus: KbdFocus,
        has_selection: bool,
    ) -> bool {
        if focus != KbdFocus::None && !has_selection {
            return false;
        }
        if self.focus.get() == focus {
            return false;
        }
        self.flush_burst(document);
        self.focus.set(focus);
        self.arrows.set(ArrowDrive::default());
        self.last_tick.set(None);
        true
    }

    /// Held-arrow bitset update. `code` is one of `ARROW_*`. Returns `true` if
    /// the bitset changed (caller marks dirty), `false` for out-of-range codes.
    ///
    /// Burst lifecycle: a press that flips the bitset from empty → non-empty
    /// arms the burst tracker with the selected object's current transform; a
    /// release that flips it back to empty flushes the burst as one undo entry.
    /// Out-of-range codes short-circuit before the bitset write, so they don't
    /// influence the burst.
    pub(super) fn update_arrow(
        &self,
        document: &Rc<SceneDocument>,
        selection_id: Option<NodeId>,
        code: i32,
        pressed: bool,
    ) -> bool {
        let prev_any_held = self.arrows.get().any_held();
        let mut d = self.arrows.get();
        match code {
            ARROW_X_POS => d.x_pos = pressed,
            ARROW_X_NEG => d.x_neg = pressed,
            ARROW_Y_POS => d.y_pos = pressed,
            ARROW_Y_NEG => d.y_neg = pressed,
            ARROW_Z_POS => d.z_pos = pressed,
            ARROW_Z_NEG => d.z_neg = pressed,
            _ => return false,
        }
        self.arrows.set(d);
        let now_any_held = d.any_held();
        self.burst.note_event();

        if !prev_any_held && now_any_held && pressed {
            self.try_arm_burst(document, selection_id);
        } else if prev_any_held && !now_any_held {
            self.flush_burst(document);
        }

        if pressed && self.last_tick.get().is_none() {
            self.last_tick.set(Some(Instant::now()));
        }
        true
    }

    /// Captures the current transform of the selected object as the burst's
    /// pre-image. No-op if the burst is already armed (a second key press
    /// during an existing burst), the selection is empty, or the scene isn't
    /// loaded.
    fn try_arm_burst(&self, document: &Rc<SceneDocument>, selection_id: Option<NodeId>) {
        if self.burst.is_armed() {
            return;
        }
        let Some(node_id) = selection_id else {
            return;
        };
        let Some(before) = document
            .with_scene(|s| s.leaf(node_id).map(|q| q.local_transform()))
            .flatten()
        else {
            return;
        };
        self.burst.arm(node_id, before);
        let group = GroupMove::capture(document, &document.selection_node_ids());
        self.group.replace(group.map(Rc::new));
    }

    /// Pushes the in-progress burst as one undo entry, using the scene's
    /// current transform of the captured object as `after`. `push_edit` filters
    /// the no-op case (the user pressed and released without an integration
    /// tick), so we don't gate it here.
    fn flush_burst(&self, document: &Rc<SceneDocument>) {
        let group = self.group.take();
        let Some((node_id, before)) = self.burst.take() else {
            return;
        };
        if let Some(group) = group {
            group.commit(document);
            return;
        }
        let after = document
            .with_scene(|s| s.leaf(node_id).map(|q| q.local_transform()))
            .flatten();
        if let Some(after) = after {
            document.push_edit(SceneEdit::SetLocalTransform {
                node_id,
                before,
                after,
            });
        }
    }

    /// Per-frame integration. If the focus is active, axes are held, the
    /// selection has a pivot-attachable object in front of the eye, and the
    /// camera + viewport are present, applies one tick of `keyboard_step` to
    /// the selected object's transform and returns `true`. The caller marks
    /// dirty on `true`.
    pub(super) fn integrate(&self, ctx: IntegrationCtx) -> bool {
        let focus = self.focus.get();
        if focus == KbdFocus::None {
            return false;
        }
        let arrows = self.arrows.get();
        if !arrows.any_held() {
            self.last_tick.set(None);
            return false;
        }
        let Some(node_id) = ctx.selection_id else {
            return false;
        };
        let Some(vp) = ctx.viewport else {
            return false;
        };
        let h = vp.height();

        let now = Instant::now();
        let last = self.last_tick.get().unwrap_or(now);
        let dt = (now - last).as_secs_f32().min(KBD_MAX_DT);
        self.last_tick.set(Some(now));
        if dt <= 0.0 {
            return false;
        }

        // Pull the running transform + pivot + camera + the leaf's
        // ancestor-chain transform through the document's read borrow + camera
        // borrow; all drop at the closure's end before the scene write below.
        let group = self.group.borrow().clone();
        let prepared: Option<(Vec3, Transform3D, Transform3D, f32)> = ctx
            .document
            .with_scene(|spatial| {
                let pivot = if group.is_some() {
                    super::group_move::group_pivot(spatial, &ctx.document.selection_node_ids())?
                } else {
                    crate::gizmo::pivot_for_leaf(spatial, node_id)?
                };
                let query = spatial.leaf(node_id)?;
                let running = query.local_transform();
                let parent_world = query.parent_world();
                let camera_slot = ctx.camera.borrow();
                let camera = camera_slot.as_ref()?;
                Some((
                    pivot,
                    running,
                    parent_world,
                    camera.world_per_pixel_at(pivot, default_fov_y(), h)?,
                ))
            })
            .flatten();
        let Some((pivot, running, parent_world, world_per_pixel)) = prepared else {
            return false;
        };

        let Some(new_transform) = keyboard_step(
            focus,
            arrows,
            dt,
            pivot,
            &parent_world,
            &running,
            world_per_pixel,
        ) else {
            return false;
        };

        // Several objects: each gets the world transform the burst has given
        // the primary one since it began, about their shared centre; a turn
        // or a scale keeps the group on the plate.
        if let Some(group) = group
            && let Some((_, start)) = self.burst.armed.get()
        {
            let Some(delta) = super::group_move::world_delta(
                &parent_world.compose(&start),
                &parent_world.compose(&new_transform),
            ) else {
                return false;
            };
            let delta = if focus == KbdFocus::Translate {
                delta
            } else {
                group.kept_on_bed(ctx.document, delta)
            };
            return group.apply_world(ctx.document, delta).is_ok();
        }

        // Same policy as mouse drags: translation is free, also out of the
        // plate; rotate/scale lift their bounding box above the plate without
        // creating another undo step.
        let new_transform = if focus == KbdFocus::Translate {
            new_transform
        } else {
            ctx.document
                .with_scene(|spatial| {
                    super::bed_clearance::keep_above_bed(spatial, node_id, running, new_transform)
                })
                .unwrap_or(new_transform)
        };

        ctx.document
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: running,
                after: new_transform,
            })
            .is_ok()
    }
}

#[cfg(test)]
mod keyboard_step_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn arrow_drive_signs_compose_per_axis_kbd() {
        // +X and -X cancel; +Y alone keeps a sign; mixed bits across axes drive
        // each axis independently.
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
        // 1 second of held +X at 30 px/sec, world_per_pixel = 0.1 → world delta
        // = 30 * 0.1 = 3.0 along +X. Off-axis stays 0.
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
            &Transform3D::Identity,
            1.0,
        );
        assert!(result.is_none());
    }

    #[test]
    fn keyboard_step_rotate_about_x_takes_y_into_z() {
        // 1 s at 45°/s about +X = π/4 rotation. R_x(π/4) maps (0, 1, 0) →
        // (0, cos π/4, sin π/4).
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
        // Held -X for 1 s → factor 1/1.5 along X.
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

#[cfg(test)]
mod integrate_tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_scene::{LeafPayload, Scene};

    use super::*;
    use crate::camera::AxisView;
    use crate::picking;

    /// Builds a scene with one leaf whose local AABB is a unit cube, translated
    /// so that its pivot lands on `pivot`.
    fn graph_with_cube_at(pivot: [f64; 3]) -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let mesh = IndexedMesh::new(
            None,
            vec![
                Vertex::new([-0.5, -0.5, -0.5]),
                Vertex::new([0.5, 0.5, 0.5]),
            ],
            vec![],
        );
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(pivot)),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        g
    }

    fn document_with_cube_at(pivot: [f64; 3]) -> (Rc<SceneDocument>, NodeId) {
        let document = SceneDocument::new();
        document.install_graph(graph_with_cube_at(pivot), PathBuf::from("<test>"));
        let node_id = document
            .with_scene(|s| s.leaf_id_at_index(0))
            .flatten()
            .unwrap();
        (document, node_id)
    }

    fn pivot_of(document: &Rc<SceneDocument>, node_id: NodeId) -> Vec3 {
        document
            .with_scene(|s| crate::gizmo::pivot_for_leaf(s, node_id))
            .flatten()
            .unwrap()
    }

    /// Returns a state with Translate focus and +X held. The next `integrate`
    /// step is exactly KBD_MAX_DT (0.1 s) long, because the last tick is older
    /// than that.
    fn translate_x_held(document: &Rc<SceneDocument>, node_id: NodeId) -> KeyboardDriveState {
        let state = KeyboardDriveState::new();
        assert!(state.try_set_focus(document, KbdFocus::Translate, true));
        assert!(state.update_arrow(document, Some(node_id), ARROW_X_POS, true));
        state
            .last_tick
            .set(Some(Instant::now() - Duration::from_secs(1)));
        state
    }

    /// Returns a camera with the eye at (0, -3, 0), looking along +Y, so +X is
    /// parallel to the image plane.
    fn front_camera() -> OrbitCamera {
        let mut camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        camera.set_axis_view(AxisView::Front);
        camera
    }

    fn integrate_once(
        state: &KeyboardDriveState,
        document: &Rc<SceneDocument>,
        node_id: NodeId,
        camera: &OrbitCamera,
    ) -> bool {
        state.integrate(IntegrationCtx {
            document,
            camera: &RefCell::new(Some(camera.clone())),
            viewport: ViewportSize::new(400.0, 200.0),
            selection_id: Some(node_id),
        })
    }

    #[test]
    fn translate_moves_the_pivot_30_px_per_second_off_the_orbit_target_plane() {
        // Pivots at y = -1 and y = +1 are in front of and behind the plane
        // through the orbit target.
        let camera = front_camera();
        let screen_x = |p: Vec3| {
            picking::project_world_to_screen(&camera, default_fov_y(), 400.0, 200.0, p)
                .unwrap()
                .0
        };
        for pivot_y in [-1.0, 1.0] {
            let (document, node_id) = document_with_cube_at([0.0, pivot_y, 1.0]);
            let state = translate_x_held(&document, node_id);

            let before = screen_x(pivot_of(&document, node_id));
            assert!(integrate_once(&state, &document, node_id, &camera));
            let after = screen_x(pivot_of(&document, node_id));

            // 30 px/s for 0.1 s.
            assert_approx_eq!(f32, after - before, 3.0, epsilon = 1e-3);
        }
    }

    #[test]
    fn translate_skips_the_step_when_the_pivot_is_behind_the_eye() {
        // The eye is at y = -3.
        let (document, node_id) = document_with_cube_at([0.0, -4.0, 1.0]);
        let state = translate_x_held(&document, node_id);

        let before = pivot_of(&document, node_id);
        assert!(!integrate_once(&state, &document, node_id, &front_camera()));
        assert_eq!(pivot_of(&document, node_id), before);
    }
}

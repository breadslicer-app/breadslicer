/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-edit clamp that keeps a translate operation's transformed AABB
//! inside the [`BuildVolume`].
//!
//! The viewport calls [`clamp_translate_edit`] from the two sites
//! that build a "translate `before` to `proposed_after`" edit: the
//! gizmo-translate-handle drag (in [`super::gizmo_fsm`]) and the
//! keyboard `Translate`-focus integrator (in [`super::keyboard_drive`]).
//! Other operation kinds (rotate, scale) are not clamped: their
//! composed translation component is a side effect of the pivot
//! pre/post-translate, and constraining it would surprise the user.
//!
//! The helper assumes `proposed_after = Affine(delta).compose(before)`
//! for some translation `delta` (which is how both call sites build
//! it). It extracts that delta as the difference of the two
//! transforms' translation components, runs it through
//! [`BuildVolume::clamp_translation_delta`], and rebuilds the result
//! by composing the clamped delta onto `before`.

use glam::Vec3;
use sc_mesh_core::Aabb;
use sc_mesh_core::scene::Transform3D;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_spatial::SceneSpatial;

use crate::build_volume::BuildVolume;

/// Clamp a translate-mode edit so the object's transformed AABB
/// stays inside `build_volume`. Returns:
///
/// - `proposed_after` unchanged when the object is missing, has no
///   local AABB (empty mesh), or no clamping was needed.
/// - `before` when the clamp blocks the entire delta (motion was
///   zero on every axis after clamping). Preserves the
///   `Transform3D::Identity` variant when `before` is identity.
/// - `Affine(clamped_delta).compose(before)` otherwise.
pub(super) fn clamp_translate_edit(
    spatial: &SceneSpatial,
    object_idx: usize,
    before: Transform3D,
    proposed_after: Transform3D,
    build_volume: BuildVolume,
) -> Transform3D {
    let Some(query) = spatial.object(object_idx) else {
        return proposed_after;
    };
    let Some(local) = query.local_aabb() else {
        return proposed_after;
    };
    let aabb_pre = transformed_arvo_or_identity(local, before);
    let delta = translation_delta(before, proposed_after);
    let clamped = build_volume.clamp_translation_delta(aabb_pre, delta);
    if clamped == delta {
        return proposed_after;
    }
    if clamped == Vec3::ZERO {
        return before;
    }
    let translation =
        AffineTransform3D::from_translation([clamped.x as f64, clamped.y as f64, clamped.z as f64]);
    Transform3D::Affine(translation).compose(&before)
}

fn transformed_arvo_or_identity(local: Aabb, t: Transform3D) -> Aabb {
    match t {
        Transform3D::Identity => local,
        Transform3D::Affine(a) => local.transformed_arvo(&a),
    }
}

fn translation_delta(before: Transform3D, after: Transform3D) -> Vec3 {
    let t_before = translation_of(before);
    let t_after = translation_of(after);
    Vec3::new(
        (t_after[0] - t_before[0]) as f32,
        (t_after[1] - t_before[1]) as f32,
        (t_after[2] - t_before[2]) as f32,
    )
}

fn translation_of(t: Transform3D) -> [f64; 3] {
    match t {
        Transform3D::Identity => [0.0; 3],
        Transform3D::Affine(a) => *a.translation(),
    }
}

#[cfg(test)]
mod translation_clamp_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent};

    fn vol() -> BuildVolume {
        // 100 x 60 x 80 mm, matching the BuildVolume tests' fixture.
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    fn spatial_with_centred_unit_cube() -> SceneSpatial {
        // Two-vertex "mesh" stored just for its local AABB:
        // [-1, 1]^2 on X/Y, [0, 2] on Z. Faces are empty; the AABB
        // path only walks vertices.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([-1.0, -1.0, 0.0]), Vertex::new([1.0, 1.0, 2.0])],
            faces: vec![],
        };
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(mesh),
            }],
        };
        SceneSpatial::new(scene)
    }

    fn spatial_with_empty_mesh() -> SceneSpatial {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(mesh),
            }],
        };
        SceneSpatial::new(scene)
    }

    fn translate(d: [f64; 3]) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation(d))
    }

    fn affine_translation(t: Transform3D) -> [f64; 3] {
        match t {
            Transform3D::Identity => [0.0; 3],
            Transform3D::Affine(a) => *a.translation(),
        }
    }

    #[test]
    fn small_in_bounds_delta_passes_through_unchanged() {
        let spatial = spatial_with_centred_unit_cube();
        let proposed = translate([2.0, 1.0, 3.0]);
        let out = clamp_translate_edit(&spatial, 0, Transform3D::Identity, proposed, vol());
        assert_eq!(out, proposed);
    }

    #[test]
    fn delta_breaching_plus_x_clamps_x_only() {
        let spatial = spatial_with_centred_unit_cube();
        // Cube AABB is [-1, 1]; envelope is [-50, 50] on X. Room to
        // +X = 49. Asking for +60 should clamp to +49.
        let proposed = translate([60.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, 0, Transform3D::Identity, proposed, vol());
        let t = affine_translation(out);
        assert_approx_eq!(f64, t[0], 49.0);
        assert_approx_eq!(f64, t[1], 0.0);
        assert_approx_eq!(f64, t[2], 0.0);
    }

    #[test]
    fn diagonal_delta_against_plus_x_wall_slides_along_y() {
        // Pre-translate the cube to the +X wall, then ask for a
        // diagonal +X / +Y move. X is fully blocked; Y passes
        // through.
        let spatial = spatial_with_centred_unit_cube();
        let before = translate([49.0, 0.0, 0.0]); // cube max.x = 50, flush against +X wall.
        let proposed = translate([54.0, 4.0, 0.0]); // +5 on X, +4 on Y from before.
        let out = clamp_translate_edit(&spatial, 0, before, proposed, vol());
        let t = affine_translation(out);
        assert_approx_eq!(f64, t[0], 49.0);
        assert_approx_eq!(f64, t[1], 4.0);
        assert_approx_eq!(f64, t[2], 0.0);
    }

    #[test]
    fn fully_blocked_delta_returns_before_preserving_identity_variant() {
        // Cube already touching +X wall; asking for further +X is
        // blocked entirely. The result must equal `before` *as the
        // same variant*: passing in `Identity` and getting back an
        // `Affine` zero would change the document's variant
        // unnecessarily.
        let spatial = spatial_with_centred_unit_cube();
        let before = translate([49.0, 0.0, 0.0]);
        let proposed = translate([54.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, 0, before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn already_outside_plus_x_can_be_dragged_back_inside() {
        let spatial = spatial_with_centred_unit_cube();
        // Cube AABB.max.x = 70 > envelope max.x = 50.
        let before = translate([69.0, 0.0, 0.0]);
        // -10 on X is "back toward inside", allowed.
        let proposed = translate([59.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, 0, before, proposed, vol());
        let t = affine_translation(out);
        assert_approx_eq!(f64, t[0], 59.0);
    }

    #[test]
    fn already_outside_plus_x_blocks_further_plus_x() {
        let spatial = spatial_with_centred_unit_cube();
        let before = translate([69.0, 0.0, 0.0]);
        // +5 on X would push further out; clamped to 0, returns before.
        let proposed = translate([74.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, 0, before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn empty_mesh_is_pass_through() {
        let spatial = spatial_with_empty_mesh();
        // Wildly out-of-bounds delta: with no AABB to constrain,
        // the helper returns proposed_after unchanged.
        let proposed = translate([10000.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, 0, Transform3D::Identity, proposed, vol());
        assert_eq!(out, proposed);
    }

    #[test]
    fn missing_object_index_is_pass_through() {
        let spatial = spatial_with_centred_unit_cube();
        let proposed = translate([10000.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, 999, Transform3D::Identity, proposed, vol());
        assert_eq!(out, proposed);
    }

    #[test]
    fn z_floor_is_one_sided() {
        let spatial = spatial_with_centred_unit_cube();
        // Cube AABB.min.z = 0 with identity. -2 on Z would go
        // below the plate; clamped to 0.
        let down = translate([0.0, 0.0, -2.0]);
        let out_down = clamp_translate_edit(&spatial, 0, Transform3D::Identity, down, vol());
        assert_eq!(out_down, Transform3D::Identity);

        // +50 on Z is fine (cube max.z = 2 → room to ceiling = 78).
        let up = translate([0.0, 0.0, 50.0]);
        let out_up = clamp_translate_edit(&spatial, 0, Transform3D::Identity, up, vol());
        let t = affine_translation(out_up);
        assert_approx_eq!(f64, t[2], 50.0);
    }
}

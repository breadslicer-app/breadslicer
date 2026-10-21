/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-edit clamp that keeps a translate operation's transformed AABB inside
//! the [`BuildVolume`].
//!
//! The viewport calls [`clamp_translate_edit`] from the 2 sites that build a
//! "translate `before` to `proposed_after`" edit: the gizmo-translate-handle
//! drag (in [`super::gizmo_fsm`]) and the keyboard `Translate`-focus integrator
//! (in [`super::keyboard_drive`]). Other operation kinds (rotate, scale) are
//! not clamped: their composed translation component is a side effect of the
//! pivot pre/post-translate, and constraining it would surprise the user.
//!
//! The helper assumes `proposed_after = Affine(delta).compose(before)` for some
//! translation `delta`, which is how both call sites build it. It extracts that
//! delta as the difference of the translations of the 2 world transforms, runs
//! it through [`sc_print_primitives::BuildVolume::clamp_translation_delta`],
//! and rebuilds the result by composing the clamped delta onto `before`.
//!
//! `aabb_pre` comes from `ObjectQuery::transformed_aabb`, which walks the
//! transformed vertices instead of passing the local AABB through Arvo. The
//! renderer tests the box of the same vertices for the out-of-volume tint and
//! the wall stripes, so the 2 agree up to rounding: the renderer applies the
//! whole world matrix in f32, while `transformed_aabb` applies the linear part
//! in f64, rounds each vertex to f32, and adds the translation in f32.
//!
//! `before` is the leaf's parent-relative transform (as carried by the gizmo
//! and keyboard FSMs), and the wall test needs the AABB in world space. The
//! helper composes the leaf's `parent_world` onto `before` before walking the
//! mesh, so a non-identity ancestor transform (e.g. the placement translation
//! that `place_on_plate` writes onto the scene root) is honoured.

use sc_mesh_core::Vector3D;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::build_volume::BuildVolume;

/// Clamps a translate edit so that the object's transformed AABB stays inside
/// `build_volume`. Returns:
///
/// - `proposed_after` unchanged when the object is missing, has no local AABB
///   (empty mesh), or needs no clamp.
/// - `before` when the clamp blocks the whole delta (the clamped motion is 0 on
///   every axis). This keeps the `Transform3D::Identity` variant of an identity
///   `before`.
/// - `Affine(clamped_delta).compose(before)` otherwise.
///
/// `before` is the leaf's parent-relative transform, and the clamp works on the
/// world-space delta between `before` and `proposed_after`. The result is
/// correct when the leaf's parent chain is a pure translation, because only
/// then is that world-space delta also the delta in the parent's frame.
pub(super) fn clamp_translate_edit(
    spatial: &SceneSpatial,
    node_id: NodeId,
    before: Transform3D,
    proposed_after: Transform3D,
    build_volume: BuildVolume,
) -> Transform3D {
    let Some(query) = spatial.leaf(node_id) else {
        return proposed_after;
    };
    let parent_world = query.parent_world();
    let world_before = parent_world.compose(&before);
    let world_after = parent_world.compose(&proposed_after);
    let Some(aabb_pre) = query.transformed_aabb(world_before) else {
        return proposed_after;
    };
    let delta = translation_delta(world_before, world_after);
    let clamped = build_volume
        .volume()
        .clamp_translation_delta(&aabb_pre, delta);
    if clamped == delta {
        return proposed_after;
    }
    if clamped == Vector3D::new([0.0; 3]) {
        return before;
    }
    let translation = AffineTransform3D::from_translation(clamped.coords().map(f64::from));
    Transform3D::Affine(translation).compose(&before)
}

fn translation_delta(before: Transform3D, after: Transform3D) -> Vector3D<f32> {
    let t_before = translation_of(before);
    let t_after = translation_of(after);
    Vector3D::new([
        (t_after[0] - t_before[0]) as f32,
        (t_after[1] - t_before[1]) as f32,
        (t_after[2] - t_before[2]) as f32,
    ])
}

fn translation_of(t: Transform3D) -> [f64; 3] {
    match t {
        Transform3D::Identity => [0.0; 3],
        Transform3D::Affine(a) => *a.translation(),
    }
}

#[cfg(test)]
mod translation_clamp_tests {
    use std::sync::Arc;

    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_scene::{LeafPayload, Scene};

    fn vol() -> BuildVolume {
        // 100 x 60 x 80 mm, matching the BuildVolume tests' fixture.
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    /// Builds a scene with 1 leaf under an identity Group root, as the OBJ and
    /// 3MF readers build theirs. Tests that inspect `graph().roots()[0]` rely
    /// on that Group.
    fn one_leaf_spatial(mesh: IndexedMesh) -> SceneSpatial {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        SceneSpatial::from_graph(g)
    }

    fn spatial_with_centred_unit_cube() -> SceneSpatial {
        // 8-vertex axis-aligned cube centred on (0, 0, 1): [-1, 1]^2 on X/Y,
        // [0, 2] on Z. Its vertices reach every corner of its AABB, so its
        // tight box and Arvo's box agree under any rotation, which the rotation
        // tests below rely on.
        one_leaf_spatial(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([-1.0, -1.0, 0.0]),
                Vertex::new([1.0, -1.0, 0.0]),
                Vertex::new([-1.0, 1.0, 0.0]),
                Vertex::new([1.0, 1.0, 0.0]),
                Vertex::new([-1.0, -1.0, 2.0]),
                Vertex::new([1.0, -1.0, 2.0]),
                Vertex::new([-1.0, 1.0, 2.0]),
                Vertex::new([1.0, 1.0, 2.0]),
            ],
            faces: vec![],
        })
    }

    fn spatial_with_octahedron() -> SceneSpatial {
        // 6 unit-axis vertices: the AABB is [-1, 1]^3, but no vertex reaches a
        // corner, so this is the worst case for a clamp on Arvo's box: after a
        // 45 degree rotation about Z, Arvo's box reaches sqrt(2) on X and Y,
        // while the box of the rotated vertices reaches sqrt(2) / 2.
        one_leaf_spatial(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([-1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, -1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
                Vertex::new([0.0, 0.0, -1.0]),
            ],
            faces: vec![],
        })
    }

    /// Builds the cube of [`spatial_with_centred_unit_cube`] with a
    /// non-identity translation on its Group root, as the loader's
    /// `place_on_plate` leaves a scene whose root is a Group: the placement
    /// delta is on the root, and the leaf keeps its local transform. A clamp
    /// that took the leaf's local transform as its world transform would block
    /// too much or too little.
    fn spatial_with_unit_cube_under_root_translation(root_translation: [f64; 3]) -> SceneSpatial {
        use sc_scene::NodeId;

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([-1.0, -1.0, 0.0]),
                Vertex::new([1.0, -1.0, 0.0]),
                Vertex::new([-1.0, 1.0, 0.0]),
                Vertex::new([1.0, 1.0, 0.0]),
                Vertex::new([-1.0, -1.0, 2.0]),
                Vertex::new([1.0, -1.0, 2.0]),
                Vertex::new([-1.0, 1.0, 2.0]),
                Vertex::new([1.0, 1.0, 2.0]),
            ],
            faces: vec![],
        };
        let mut graph = Scene::new();
        let root: NodeId = graph.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(root_translation)),
        );
        graph.add_root(root).unwrap();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_child(root, leaf).unwrap();
        SceneSpatial::primed_graph(graph)
    }

    fn spatial_with_empty_mesh() -> SceneSpatial {
        one_leaf_spatial(IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        })
    }

    fn translate(d: [f64; 3]) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation(d))
    }

    fn first_leaf(spatial: &SceneSpatial) -> NodeId {
        spatial
            .leaf_id_at_index(0)
            .expect("fixture has a leaf at idx 0")
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
        let out = clamp_translate_edit(
            &spatial,
            first_leaf(&spatial),
            Transform3D::Identity,
            proposed,
            vol(),
        );
        assert_eq!(out, proposed);
    }

    #[test]
    fn delta_breaching_plus_x_clamps_x_only() {
        let spatial = spatial_with_centred_unit_cube();
        // Cube AABB is [-1, 1]; envelope is [-50, 50] on X. Room to +X = 49.
        // Asking for +60 should clamp to +49.
        let proposed = translate([60.0, 0.0, 0.0]);
        let out = clamp_translate_edit(
            &spatial,
            first_leaf(&spatial),
            Transform3D::Identity,
            proposed,
            vol(),
        );
        let t = affine_translation(out);
        assert_approx_eq!(f64, t[0], 49.0);
        assert_approx_eq!(f64, t[1], 0.0);
        assert_approx_eq!(f64, t[2], 0.0);
    }

    #[test]
    fn diagonal_delta_against_plus_x_wall_slides_along_y() {
        // Pre-translate the cube to the +X wall, then ask for a diagonal move
        // of +5 on X and +4 on Y. X is fully blocked; Y passes through.
        let spatial = spatial_with_centred_unit_cube();
        let before = translate([49.0, 0.0, 0.0]); // cube max.x = 50, flush against +X wall.
        let proposed = translate([54.0, 4.0, 0.0]); // +5 on X, +4 on Y from before.
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        let t = affine_translation(out);
        assert_approx_eq!(f64, t[0], 49.0);
        assert_approx_eq!(f64, t[1], 4.0);
        assert_approx_eq!(f64, t[2], 0.0);
    }

    #[test]
    fn fully_blocked_delta_returns_before() {
        // The cube already touches the +X wall, and the move asks for more +X:
        // the whole delta is blocked, and the result is `before`.
        let spatial = spatial_with_centred_unit_cube();
        let before = translate([49.0, 0.0, 0.0]);
        let proposed = translate([54.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn already_outside_plus_x_can_be_dragged_back_inside() {
        let spatial = spatial_with_centred_unit_cube();
        // Cube AABB.max.x = 70 > envelope max.x = 50.
        let before = translate([69.0, 0.0, 0.0]);
        // -10 on X is "back toward inside", allowed.
        let proposed = translate([59.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        let t = affine_translation(out);
        assert_approx_eq!(f64, t[0], 59.0);
    }

    #[test]
    fn already_outside_plus_x_blocks_further_plus_x() {
        let spatial = spatial_with_centred_unit_cube();
        let before = translate([69.0, 0.0, 0.0]);
        // +5 on X would push further out; clamped to 0, returns before.
        let proposed = translate([74.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn root_translation_offsets_world_aabb_so_leaf_can_drag_back_inside() {
        // A scene as `place_on_plate` leaves it: the placement delta is on the
        // root, and the leaf's local transform is the identity. With the cube's
        // local AABB at `[-1, 1] x [-1, 1] x [0, 2]` and the root translated by
        // (+69, 0, 0), the world AABB's maximum X is 70, past the envelope wall
        // at +50. An inward drag (-10 on X, composed onto the leaf's identity
        // local transform) must be allowed, as on a flat scene where the leaf's
        // own transform carries the (+69, 0, 0) offset.
        let spatial = spatial_with_unit_cube_under_root_translation([69.0, 0.0, 0.0]);
        let before = Transform3D::Identity;
        let proposed = translate([-10.0, 0.0, 0.0]); // inward
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        let t = affine_translation(out);
        assert_approx_eq!(f64, t[0], -10.0);
        assert_approx_eq!(f64, t[1], 0.0);
        assert_approx_eq!(f64, t[2], 0.0);
    }

    #[test]
    fn root_translation_offsets_world_aabb_so_further_outward_is_blocked() {
        // As in the previous test, the cube's world AABB reaches past +X = 50
        // because of the root translation. A further outward drag (+5 on X) is
        // clamped to 0, and the result is the leaf's `before`. A clamp on the
        // local AABB, which is inside the envelope, would let the drag through
        // and push the object further out.
        let spatial = spatial_with_unit_cube_under_root_translation([69.0, 0.0, 0.0]);
        let before = Transform3D::Identity;
        let proposed = translate([5.0, 0.0, 0.0]); // outward
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn root_translation_z_floor_blocks_downward_through_plate() {
        // The scene of the loader's placement: the root carries a translation,
        // 0 here, and the cube's bottom rests on Z = 0. A move of -1 on Z,
        // below the plate, is blocked.
        let spatial = spatial_with_unit_cube_under_root_translation([0.0, 0.0, 0.0]);
        let before = Transform3D::Identity;
        let proposed = translate([0.0, 0.0, -1.0]);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn empty_mesh_is_pass_through() {
        let spatial = spatial_with_empty_mesh();
        // Wildly out-of-bounds delta: with no AABB to constrain, the helper
        // returns proposed_after unchanged.
        let proposed = translate([10000.0, 0.0, 0.0]);
        let out = clamp_translate_edit(
            &spatial,
            first_leaf(&spatial),
            Transform3D::Identity,
            proposed,
            vol(),
        );
        assert_eq!(out, proposed);
    }

    #[test]
    fn non_leaf_node_is_pass_through() {
        // Group nodes (e.g. the scene root) carry no resolvable mesh, so
        // SceneSpatial::leaf returns None and the helper returns the proposed
        // transform unchanged.
        let spatial = spatial_with_centred_unit_cube();
        let root_id = spatial.graph().roots()[0];
        let proposed = translate([10000.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, root_id, Transform3D::Identity, proposed, vol());
        assert_eq!(out, proposed);
    }

    #[test]
    fn z_floor_is_one_sided() {
        let spatial = spatial_with_centred_unit_cube();
        // Cube AABB.min.z = 0 with identity. -2 on Z would go below the plate;
        // clamped to 0.
        let down = translate([0.0, 0.0, -2.0]);
        let out_down = clamp_translate_edit(
            &spatial,
            first_leaf(&spatial),
            Transform3D::Identity,
            down,
            vol(),
        );
        assert_eq!(out_down, Transform3D::Identity);

        // +50 on Z is fine: the cube's max.z is 2, so 78 mm of room remain
        // below the ceiling.
        let up = translate([0.0, 0.0, 50.0]);
        let out_up = clamp_translate_edit(
            &spatial,
            first_leaf(&spatial),
            Transform3D::Identity,
            up,
            vol(),
        );
        let t = affine_translation(out_up);
        assert_approx_eq!(f64, t[2], 50.0);
    }

    fn rotation_z(angle: f64) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_rotation_z(angle))
    }

    fn translation_then_rotation_z(d: [f64; 3], angle: f64) -> Transform3D {
        Transform3D::Affine(
            AffineTransform3D::from_translation(d)
                .compose(&AffineTransform3D::from_rotation_z(angle)),
        )
    }

    #[test]
    fn rotated_octahedron_can_travel_further_than_arvo_would_allow() {
        // After a 45 degree rotation about Z, the octahedron's vertices reach
        // sqrt(2)/2 (about 0.707) on X, while Arvo's box of its local AABB
        // reaches sqrt(2) (about 1.414). With the envelope at X in [-50, 50], a
        // move of +60 must stop at 50-sqrt(2)/2 (about 49.29), past where a
        // clamp on Arvo's box would stop: 50-sqrt(2) (about 48.59).
        let spatial = spatial_with_octahedron();
        let before = rotation_z(std::f64::consts::FRAC_PI_4);
        let proposed = translation_then_rotation_z([60.0, 0.0, 0.0], std::f64::consts::FRAC_PI_4);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        let t = affine_translation(out);
        let expected = 50.0 - f64::from(std::f32::consts::FRAC_1_SQRT_2);
        let arvo_bound = 50.0 - f64::from(std::f32::consts::SQRT_2);
        assert_approx_eq!(f64, t[0], expected, epsilon = 1e-4);
        // The result must be past where a clamp on Arvo's box would stop.
        assert!(
            t[0] > arvo_bound + 0.1,
            "rotated octahedron over-clamped: got {}, Arvo bound was {}",
            t[0],
            arvo_bound,
        );
    }

    #[test]
    fn rotated_cube_clamps_at_arvo_bound_because_it_fills_its_aabb() {
        // The cube fills its local AABB, so its tight box and Arvo's box
        // coincide. A 45 degree rotation about Z extends the cube on X to
        // sqrt(2) on each side of its centre. With the envelope X in [-50, 50],
        // a move of +60 on X must clamp to 50-sqrt(2), about 48.586, the room
        // that is left.
        let spatial = spatial_with_centred_unit_cube();
        let before = rotation_z(std::f64::consts::FRAC_PI_4);
        let proposed = translation_then_rotation_z([60.0, 0.0, 0.0], std::f64::consts::FRAC_PI_4);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        let t = affine_translation(out);
        let expected = 50.0 - f64::from(std::f32::consts::SQRT_2);
        assert_approx_eq!(f64, t[0], expected, epsilon = 1e-4);
    }

    #[test]
    fn small_in_bounds_delta_passes_through_under_rotation() {
        // A cube away from any wall, rotated 45 degrees about Z, gets a small
        // in-bounds delta. The result must be `proposed` unchanged, with no
        // false clamp.
        let spatial = spatial_with_centred_unit_cube();
        let before = rotation_z(std::f64::consts::FRAC_PI_4);
        let proposed = translation_then_rotation_z([2.0, 1.0, 3.0], std::f64::consts::FRAC_PI_4);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, proposed);
    }
}

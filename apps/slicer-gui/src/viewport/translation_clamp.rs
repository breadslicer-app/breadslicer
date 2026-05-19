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
//!
//! `aabb_pre` comes from `ObjectQuery::transformed_aabb`, which walks
//! the rotated vertices rather than passing the local AABB through
//! Arvo. The tight AABB matches the envelope-warning overlay's
//! per-vertex test, so the two never disagree: an axis is clamped iff
//! some vertex would breach the corresponding wall.
//!
//! `before` is the leaf's parent-relative transform (as carried by
//! the gizmo / keyboard FSMs); the AABB the envelope cares about is
//! in world space. The helper composes the leaf's `parent_world`
//! onto `before` before walking the mesh, so a non-identity ancestor
//! transform (e.g. the placement translation `place_on_plate` writes
//! onto the scene root) is honoured.

use glam::Vec3;
use sc_mesh_core::NodeId;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
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
///
/// `before` is the leaf's parent-relative transform. The world-space
/// AABB is computed by composing the leaf's `parent_world` onto
/// `before`. The translation delta passed to
/// [`BuildVolume::clamp_translation_delta`] is the world-space delta,
/// derived from the world translations of `before` and
/// `proposed_after`; the result is rewritten back as a world-space
/// translation composed with `before` (the leaf's parent ancestor
/// is currently a pure translation in every production scene, so the
/// world delta lands on the leaf transform unchanged).
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
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::{IndexedMesh, LeafPayload, SceneGraph};

    fn vol() -> BuildVolume {
        // 100 x 60 x 80 mm, matching the BuildVolume tests' fixture.
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    /// Single-leaf scene under a Group root, mirroring what the
    /// loader pipeline installs: every file-format reader funnels its
    /// objects under an identity-Group root. Tests that inspect
    /// `graph().roots()[0]` rely on that wrapping Group being present.
    fn one_leaf_spatial(mesh: IndexedMesh) -> SceneSpatial {
        let mut g = SceneGraph::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh))
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        SceneSpatial::from_graph(g)
    }

    fn spatial_with_centred_unit_cube() -> SceneSpatial {
        // Eight-vertex axis-aligned unit cube centred on (0, 0, 1):
        // [-1, 1]^2 on X/Y, [0, 2] on Z. Vertices reach every AABB
        // corner, so per-vertex and Arvo agree under any rotation,
        // which is what the rotation-coverage tests below rely on.
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
        // Six unit-axis vertices: AABB is [-1, 1]^3 but no vertex
        // reaches a corner. The local AABB is the same as a unit
        // cube's, so this is the worst-case fixture for the old
        // Arvo-based clamp: after a 45° Z rotation, Arvo extends X
        // and Y by sqrt(2) while the true rotated-mesh AABB stays
        // at sqrt(2) / 2 on those axes.
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

    /// Builds the same centred unit cube as
    /// [`spatial_with_centred_unit_cube`] but with a non-identity
    /// translation written onto the scene root (mimicking the
    /// loader's `place_on_plate`: the placement delta lives on the
    /// root Group, leaves keep their authored local transform).
    /// Without the world-space AABB fix, the clamp would treat the
    /// leaf's identity local transform as the world transform and
    /// either over-block or fail to block the drag.
    fn spatial_with_unit_cube_under_root_translation(root_translation: [f64; 3]) -> SceneSpatial {
        use sc_mesh_core::NodeId;

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
        let mut graph = SceneGraph::new();
        let root: NodeId = graph.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(root_translation)),
        );
        graph.add_root(root).unwrap();
        let leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh))
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
        // Cube AABB is [-1, 1]; envelope is [-50, 50] on X. Room to
        // +X = 49. Asking for +60 should clamp to +49.
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
        // Pre-translate the cube to the +X wall, then ask for a
        // diagonal +X / +Y move. X is fully blocked; Y passes
        // through.
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
    fn fully_blocked_delta_returns_before_preserving_identity_variant() {
        // Cube already touching +X wall; asking for further +X is
        // blocked entirely. The result must equal `before` *as the
        // same variant*: passing in `Identity` and getting back an
        // `Affine` zero would change the document's variant
        // unnecessarily.
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
        // Regression for the `place_on_plate`-shaped scene: the
        // placement delta lives on the root, the leaf carries
        // identity local. With the cube's local AABB at
        // `[-1, 1] x [-1, 1] x [0, 2]` and the root translated by
        // (+69, 0, 0), the WORLD AABB.max.x is 70, past the envelope
        // wall at +50. An inward drag (-10 on X composed onto the
        // leaf's identity local) must be allowed, exactly the same
        // way it would be on a flat scene where the leaf's own
        // transform carried the (+69, 0, 0) offset.
        //
        // Before the fix, `transformed_aabb(leaf_local)` would have
        // produced the LOCAL AABB (no offset), looking like both
        // walls were inside the envelope; the helper would have
        // either pass-throughed (wrong-but-permissive) or blocked
        // incorrectly when combined with an outward proposal.
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
        // Sibling of the previous regression: the cube's WORLD AABB
        // sticks past +X = 50 because of the root translation. A
        // further outward drag (+5 on X) must be clamped to zero,
        // returning the leaf's `before` unchanged. The bug being
        // pinned here is that without parent_world composition, the
        // helper would mistakenly let the drag through (LOCAL AABB
        // is inside the envelope), pushing the object further out.
        let spatial = spatial_with_unit_cube_under_root_translation([69.0, 0.0, 0.0]);
        let before = Transform3D::Identity;
        let proposed = translate([5.0, 0.0, 0.0]); // outward
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn root_translation_z_floor_blocks_downward_through_plate() {
        // The plate-placement case the GUI actually exercises: the
        // loader translates the root so the cube's bottom rests on
        // Z = 0. Asking to translate the leaf by -1 on Z (the
        // textbook "below the plate" case) must be blocked.
        let spatial = spatial_with_unit_cube_under_root_translation([0.0, 0.0, 0.0]);
        let before = Transform3D::Identity;
        let proposed = translate([0.0, 0.0, -1.0]);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, before);
    }

    #[test]
    fn empty_mesh_is_pass_through() {
        let spatial = spatial_with_empty_mesh();
        // Wildly out-of-bounds delta: with no AABB to constrain,
        // the helper returns proposed_after unchanged.
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
        // Group nodes (e.g. the scene root) carry no resolvable mesh,
        // so SceneSpatial::leaf returns None and the helper short-
        // circuits to the unchanged proposed transform.
        let spatial = spatial_with_centred_unit_cube();
        let root_id = spatial.graph().roots()[0];
        let proposed = translate([10000.0, 0.0, 0.0]);
        let out = clamp_translate_edit(&spatial, root_id, Transform3D::Identity, proposed, vol());
        assert_eq!(out, proposed);
    }

    #[test]
    fn z_floor_is_one_sided() {
        let spatial = spatial_with_centred_unit_cube();
        // Cube AABB.min.z = 0 with identity. -2 on Z would go
        // below the plate; clamped to 0.
        let down = translate([0.0, 0.0, -2.0]);
        let out_down = clamp_translate_edit(
            &spatial,
            first_leaf(&spatial),
            Transform3D::Identity,
            down,
            vol(),
        );
        assert_eq!(out_down, Transform3D::Identity);

        // +50 on Z is fine (cube max.z = 2 → room to ceiling = 78).
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
        // Regression for the over-clamping bug: a 45° Z rotation of
        // the unit-axis octahedron has true rotated-mesh
        // half-extents of √2 / 2 (≈ 0.707) on X. Arvo on the local
        // AABB (the unit cube) would report √2 (≈ 1.414). With the
        // envelope at X ∈ [-50, 50], the tight clamp lets the mesh
        // travel up to 50 - √2 / 2 ≈ 49.29 on +X; Arvo would have
        // stopped it at 50 - √2 ≈ 48.59. Ask for +60; the new path
        // must report ~49.29, not ~48.59.
        let spatial = spatial_with_octahedron();
        let before = rotation_z(std::f64::consts::FRAC_PI_4);
        let proposed = translation_then_rotation_z([60.0, 0.0, 0.0], std::f64::consts::FRAC_PI_4);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        let t = affine_translation(out);
        let expected = 50.0 - f64::from(std::f32::consts::FRAC_1_SQRT_2);
        let arvo_bound = 50.0 - f64::from(std::f32::consts::SQRT_2);
        assert_approx_eq!(f64, t[0], expected, epsilon = 1e-4);
        // Hard guard against regressing to the Arvo bound: the new
        // result must be strictly past where Arvo would have stopped.
        assert!(
            t[0] > arvo_bound + 0.1,
            "rotated octahedron over-clamped: got {}, Arvo bound was {}",
            t[0],
            arvo_bound,
        );
    }

    #[test]
    fn rotated_cube_clamps_at_arvo_bound_because_it_fills_its_aabb() {
        // The cube fills its local AABB, so the tight and Arvo
        // bounds coincide. A 45° Z rotation of the centred unit
        // cube extends X to ±√2 around the cube centre. With the
        // envelope X ∈ [-50, 50], asking for +60 on X must clamp to
        // (envelope_max - rotated_max) = 50 - √2 ≈ 48.586.
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
        // A cube already away from any wall, rotated 45° about Z,
        // gets a small in-bounds delta. The result must be the
        // unchanged `proposed`, not silently clamped: the tighten
        // must not introduce a false clamp for cubes either.
        let spatial = spatial_with_centred_unit_cube();
        let before = rotation_z(std::f64::consts::FRAC_PI_4);
        let proposed = translation_then_rotation_z([2.0, 1.0, 3.0], std::f64::consts::FRAC_PI_4);
        let out = clamp_translate_edit(&spatial, first_leaf(&spatial), before, proposed, vol());
        assert_eq!(out, proposed);
    }
}

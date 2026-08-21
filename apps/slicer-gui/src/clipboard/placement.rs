/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure placement algorithm for the paste pipeline.
//!
//! Picks a world-space translation for a fragment given the cursor's
//! projection onto the build plate, how many consecutive pastes the
//! clipboard has produced, and the AABBs of every existing root in
//! the target scene. The algorithm is deterministic and side-effect-
//! free: callers can unit-test the geometry without standing up a
//! viewport, a window, or a GPU device.
//!
//! Inputs:
//! - `fragment_aabb`: the axis-aligned bounds of the fragment in its
//!   own local frame (already includes the fragment root's rotation +
//!   scale, since that is baked into the leaf's local transform on
//!   copy).
//! - `target_world_xy`: the cursor's projection onto `Z = 0`, or the
//!   viewport-centre fallback when the cursor sits outside the
//!   viewport.
//! - `cascade_count`: number of consecutive pastes since the last
//!   copy. Spaces stacked copies apart so they don't sit on top of
//!   each other when the user spams `Ctrl+V`.
//! - `obstacles`: world-space AABBs of every existing root in the
//!   target scene. Empty AABBs are skipped; everything else is tested
//!   for 3-axis overlap against the prospective fragment world AABB.
//!
//! Output: a `[f64; 3]` translation that becomes the pasted root's
//! local transform's translation component on the destination scene.
//! The fragment's floor sits exactly at `Z = 0` regardless of the
//! fragment's own min.z, so the pasted object lands flat on the
//! build plate and never crosses it.

use sc_mesh_core::Aabb;
use sc_mesh_spatial::SceneSpatial;

/// XY offset added per consecutive paste, in millimetres. Small
/// enough to keep the cascade visible without scattering copies
/// across the build platform.
const CASCADE_STEP_MM: f64 = 5.0;

/// Lower bound on the ring-search radius, in millimetres. The actual
/// per-step radius scales with the fragment's XY extent so a single
/// hop clears the fragment's own footprint; this floor stops the ring
/// from collapsing to zero on a degenerate (zero-extent) fragment.
const MIN_RING_RADIUS_MM: f64 = 5.0;

/// Radii used by the ring search, in multiples of the per-step
/// `ring_step`. Three rings of eight angles each cover 24 candidates
/// before the algorithm gives up; that matches the plan's budget and
/// keeps the search bounded for tight scenes.
const RING_RADII_MULTIPLIERS: [f64; 3] = [1.0, 2.0, 3.0];

/// Number of angles per ring. Eight is a compromise between
/// resolution and budget; the angles are evenly spaced around the
/// circle starting from `+x`.
const RING_ANGLE_COUNT: usize = 8;

/// Pick a placement translation for a paste.
///
/// Geometry, in order:
/// 1. Cascade-shift the target by `cascade_count * CASCADE_STEP_MM`
///    in both X and Y, so consecutive pastes don't pile up.
/// 2. Center the fragment's XY-AABB under the shifted target.
/// 3. Drop the fragment's floor to `Z = 0` (the build plate).
/// 4. Test the resulting world-AABB for overlap against `obstacles`.
///    No overlap → return that translation.
/// 5. Otherwise walk the ring pattern (8 angles × 3 radii) and
///    return the first candidate that clears every obstacle.
/// 6. If every candidate overlaps, return the step-3 translation.
///    The plan accepts overlap when the budget is exhausted: it is
///    rare in normal use and the alternative (failing the paste) is
///    worse.
///
/// Empty obstacles (e.g. an empty-mesh root or the `Aabb::empty()`
/// sentinel from an unprimed cache) are ignored: they cannot
/// overlap anything by definition.
pub(crate) fn paste_translation_for_fragment(
    fragment_aabb: &Aabb,
    target_world_xy: [f64; 2],
    cascade_count: u32,
    obstacles: &[Aabb],
) -> [f64; 3] {
    let (fmin, fmax) = aabb_f64(fragment_aabb);
    let centre_x = 0.5 * (fmin[0] + fmax[0]);
    let centre_y = 0.5 * (fmin[1] + fmax[1]);
    let extent_x = fmax[0] - fmin[0];
    let extent_y = fmax[1] - fmin[1];

    let cascade = f64::from(cascade_count) * CASCADE_STEP_MM;
    let target_x = target_world_xy[0] + cascade;
    let target_y = target_world_xy[1] + cascade;

    let initial = [target_x - centre_x, target_y - centre_y, -fmin[2]];

    if !overlaps_any(&fmin, &fmax, initial, obstacles) {
        return initial;
    }

    let ring_step = extent_x.max(extent_y).max(MIN_RING_RADIUS_MM);
    for radius_mult in RING_RADII_MULTIPLIERS {
        let radius = ring_step * radius_mult;
        for k in 0..RING_ANGLE_COUNT {
            let angle = (k as f64) * std::f64::consts::TAU / (RING_ANGLE_COUNT as f64);
            let candidate = [
                initial[0] + radius * angle.cos(),
                initial[1] + radius * angle.sin(),
                initial[2],
            ];
            if !overlaps_any(&fmin, &fmax, candidate, obstacles) {
                return candidate;
            }
        }
    }

    initial
}

/// Gather the world-space AABBs of every reachable leaf in
/// `spatial`. The paste path hands the result to
/// [`paste_translation_for_fragment`] as the obstacle list, so the
/// pasted fragment lands somewhere that does not collide with
/// existing geometry.
///
/// Leaves whose mesh has no vertices are skipped: an empty leaf has
/// no AABB to overlap against. The walk uses the same pre-order
/// leaf enumerator the picker relies on, so the obstacle order
/// matches the picker's iteration order even though the placement
/// algorithm does not care about ordering.
pub(crate) fn obstacle_aabbs_for_scene(spatial: &SceneSpatial) -> Vec<Aabb> {
    let mut obstacles = Vec::new();
    let mut idx = 0usize;
    while let Some(node_id) = spatial.leaf_id_at_index(idx) {
        idx += 1;
        if let Some(query) = spatial.leaf(node_id)
            && let Some(aabb) = query.transformed_aabb(query.world_transform())
        {
            obstacles.push(aabb);
        }
    }
    obstacles
}

/// Promote an `Aabb`'s f32 corners to f64, matching the precision
/// used by the placement algorithm and the destination scene's
/// transform matrices.
fn aabb_f64(a: &Aabb) -> ([f64; 3], [f64; 3]) {
    let min = a.min().coords();
    let max = a.max().coords();
    (
        [min[0] as f64, min[1] as f64, min[2] as f64],
        [max[0] as f64, max[1] as f64, max[2] as f64],
    )
}

/// True iff the fragment's world AABB at `translation` overlaps any
/// non-empty obstacle. Uses strict separation on each axis: AABBs
/// that touch face-to-face count as non-overlapping, which is the
/// "snug fit" outcome the paste path wants. Two coincident AABBs
/// still register as overlap (no axis is strictly separated).
fn overlaps_any(
    fmin: &[f64; 3],
    fmax: &[f64; 3],
    translation: [f64; 3],
    obstacles: &[Aabb],
) -> bool {
    let cand_min = [
        fmin[0] + translation[0],
        fmin[1] + translation[1],
        fmin[2] + translation[2],
    ];
    let cand_max = [
        fmax[0] + translation[0],
        fmax[1] + translation[1],
        fmax[2] + translation[2],
    ];
    obstacles.iter().any(|obs| {
        if obs.is_empty() {
            return false;
        }
        let (omin, omax) = aabb_f64(obs);
        (0..3).all(|i| cand_min[i] < omax[i] && omin[i] < cand_max[i])
    })
}

#[cfg(test)]
mod placement_tests {
    use std::sync::Arc;

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_spatial::SceneSpatial;
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

    use super::*;

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb::from_min_max(Vertex::new(min), Vertex::new(max))
    }

    /// Unit cube centred at origin in XY, floor at Z = 0, height 2.
    fn unit_fragment() -> Aabb {
        aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0])
    }

    #[test]
    fn no_obstacles_returns_centred_initial_delta() {
        let t = paste_translation_for_fragment(&unit_fragment(), [5.0, 7.0], 0, &[]);
        // centre is (0, 0), so translation = target. floor is already at
        // 0 in fragment-space, so Z translation is 0.
        assert_eq!(t, [5.0, 7.0, 0.0]);
    }

    #[test]
    fn floor_lands_at_z_zero_regardless_of_fragment_min_z() {
        let floating = aabb([-1.0, -1.0, 7.5], [1.0, 1.0, 10.0]);
        let t = paste_translation_for_fragment(&floating, [0.0, 0.0], 0, &[]);
        assert_eq!(t[2], -7.5);

        let underground = aabb([-1.0, -1.0, -3.0], [1.0, 1.0, 4.0]);
        let t = paste_translation_for_fragment(&underground, [0.0, 0.0], 0, &[]);
        assert_eq!(t[2], 3.0);
    }

    #[test]
    fn cascade_offset_accumulates_per_paste() {
        let frag = unit_fragment();
        let zero = paste_translation_for_fragment(&frag, [0.0, 0.0], 0, &[]);
        let one = paste_translation_for_fragment(&frag, [0.0, 0.0], 1, &[]);
        let two = paste_translation_for_fragment(&frag, [0.0, 0.0], 2, &[]);
        assert_eq!(zero, [0.0, 0.0, 0.0]);
        assert_eq!(one, [CASCADE_STEP_MM, CASCADE_STEP_MM, 0.0]);
        assert_eq!(two, [2.0 * CASCADE_STEP_MM, 2.0 * CASCADE_STEP_MM, 0.0]);
    }

    #[test]
    fn obstacle_at_target_shifts_to_first_ring_along_plus_x() {
        // Initial delta sits the fragment at the target. An obstacle
        // that exactly overlaps the initial fragment position forces
        // the ring search; angle 0 (cos = 1, sin = 0) is the first
        // candidate and clears at radius == ring_step.
        let frag = unit_fragment();
        let obstacle_at_initial = aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0, &[obstacle_at_initial]);
        // extent_x = extent_y = 2, MIN_RING_RADIUS_MM = 5, so
        // ring_step = 5; candidate at angle 0 = (5, 0, 0).
        assert_eq!(t, [MIN_RING_RADIUS_MM, 0.0, 0.0]);
    }

    #[test]
    fn ring_step_scales_with_fragment_extent_when_larger_than_floor() {
        // Fragment with extent 20 in XY: ring_step = max(20, 20, 5) = 20.
        let frag = aabb([-10.0, -10.0, 0.0], [10.0, 10.0, 2.0]);
        let obstacle_at_initial = aabb([-10.0, -10.0, 0.0], [10.0, 10.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0, &[obstacle_at_initial]);
        assert_eq!(t, [20.0, 0.0, 0.0]);
    }

    #[test]
    fn all_candidates_blocked_returns_initial_overlapping_delta() {
        // An obstacle large enough to cover every ring candidate
        // forces the budget-exhausted path: the function returns
        // the initial (overlapping) delta rather than failing.
        let frag = unit_fragment();
        let huge = aabb([-1000.0, -1000.0, -1000.0], [1000.0, 1000.0, 1000.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0, &[huge]);
        assert_eq!(t, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn empty_obstacle_does_not_count_as_overlap() {
        // The Aabb::empty() sentinel must be skipped: it carries
        // infinities that would otherwise overlap the entire scene.
        let frag = unit_fragment();
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0, &[Aabb::empty()]);
        assert_eq!(t, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn touching_obstacle_does_not_count_as_overlap() {
        // Obstacle's max.x == fragment's min.x at the initial position:
        // they share a face but don't interpenetrate. The plan's
        // "snug fit" semantics treat this as no overlap, so the
        // initial delta is accepted without invoking the ring search.
        let frag = unit_fragment();
        // Initial fragment world AABB = [-1,-1,0]..[1,1,2]. An
        // obstacle whose max.x = -1 touches that face.
        let touching = aabb([-3.0, -1.0, 0.0], [-1.0, 1.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0, &[touching]);
        assert_eq!(t, [0.0, 0.0, 0.0]);
    }

    fn unit_cube_mesh() -> Arc<IndexedMesh> {
        // Eight-vertex cube spanning [-0.5, 0.5] on each axis. The
        // obstacle helper relies on local_aabb being Some, which
        // requires at least one vertex; a single triangle would do,
        // but a cube makes the assertions readable.
        Arc::new(IndexedMesh::new(
            None,
            vec![
                Vertex::new([-0.5, -0.5, -0.5]),
                Vertex::new([0.5, -0.5, -0.5]),
                Vertex::new([-0.5, 0.5, -0.5]),
                Vertex::new([0.5, 0.5, -0.5]),
                Vertex::new([-0.5, -0.5, 0.5]),
                Vertex::new([0.5, -0.5, 0.5]),
                Vertex::new([-0.5, 0.5, 0.5]),
                Vertex::new([0.5, 0.5, 0.5]),
            ],
            vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        ))
    }

    fn empty_mesh() -> Arc<IndexedMesh> {
        Arc::new(IndexedMesh::new(None, Vec::new(), Vec::new()))
    }

    fn translation_xform(x: f64, y: f64, z: f64) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation([x, y, z]))
    }

    #[test]
    fn obstacle_aabbs_for_empty_scene_is_empty() {
        let spatial = SceneSpatial::primed_graph(Scene::new());
        assert!(obstacle_aabbs_for_scene(&spatial).is_empty());
    }

    #[test]
    fn obstacle_aabbs_for_scene_yields_one_aabb_per_leaf() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        for dx in [0.0, 10.0, 20.0] {
            let leaf = g
                .add_leaf(
                    None,
                    translation_xform(dx, 0.0, 0.0),
                    LeafPayload::Mesh(unit_cube_mesh()),
                )
                .unwrap();
            g.add_child(root, leaf).unwrap();
        }
        let spatial = SceneSpatial::primed_graph(g);
        let obstacles = obstacle_aabbs_for_scene(&spatial);
        assert_eq!(obstacles.len(), 3);
        let xs: Vec<f32> = obstacles.iter().map(|a| a.min().coords()[0]).collect();
        assert_eq!(xs, vec![-0.5, 9.5, 19.5]);
    }

    #[test]
    fn obstacle_aabbs_skip_empty_meshes() {
        // A leaf with an empty mesh has no AABB to overlap against:
        // the helper must drop it from the obstacle list so the
        // placement search treats it as nonexistent rather than
        // surfacing the Aabb::empty() sentinel.
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let real = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_cube_mesh()),
            )
            .unwrap();
        g.add_child(root, real).unwrap();
        let empty = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(empty_mesh()))
            .unwrap();
        g.add_child(root, empty).unwrap();
        let spatial = SceneSpatial::primed_graph(g);
        let obstacles = obstacle_aabbs_for_scene(&spatial);
        assert_eq!(obstacles.len(), 1);
    }

    #[test]
    fn obstacle_blocking_first_ring_radius_shifts_to_second() {
        // One large obstacle blocks every angle at ring 1 (radius 5)
        // but not ring 2 (radius 10). The function returns the first
        // candidate at ring 2, angle 0 (= initial + 10 on +x).
        let frag = unit_fragment();
        // Obstacle spans the entire ring-1 circle around origin in XY
        // (radius = 5, so include up to ~6 to keep ring-1 candidates
        // overlapping after the fragment's ±1 extent is added) and
        // leaves ring-2 clear.
        let ring_blocker = aabb([-7.0, -7.0, 0.0], [7.0, 7.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0, &[ring_blocker]);
        // Ring 2 radius = 2 * 5 = 10. Candidate at angle 0 = (10, 0, 0).
        // Fragment at translation (10, 0, 0) occupies [9, -1, 0]..[11, 1, 2],
        // which is outside the blocker's max.x = 7.
        assert_eq!(t, [10.0, 0.0, 0.0]);
    }
}

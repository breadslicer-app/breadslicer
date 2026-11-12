/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure placement algorithm for the paste pipeline.
//!
//! Picks a world-space translation for a fragment given the cursor's projection
//! onto the build plate, how many consecutive pastes the clipboard has
//! produced, and the world-space AABBs of the leaves of the target scene. The
//! algorithm is deterministic and side-effect-free: callers can unit-test the
//! geometry without standing up a viewport, a window, or a GPU device.
//!
//! Inputs:
//! - `fragment_aabb`: the axis-aligned bounds of the fragment in its own local
//!   frame. They include the rotation and scale that the copy keeps in the
//!   fragment root's transform.
//! - `target_world_xy`: the cursor's projection onto the plate's floor, or the
//!   viewport-centre fallback when the cursor sits outside the viewport.
//! - `floor_z`: the height of the plate's floor.
//! - `cascade_count`: number of consecutive pastes since the last copy. It
//!   spaces stacked copies apart so that they do not sit on top of each other
//!   when the user presses `Ctrl+V` repeatedly.
//! - `obstacles`: world-space AABBs of the leaves of the target scene. Empty
//!   AABBs are skipped; everything else is tested for 3-axis overlap against
//!   the prospective fragment world AABB.
//!
//! Output: a `[f64; 3]` translation that becomes the pasted root's local
//! transform's translation component on the destination scene. The fragment's
//! floor sits exactly at `floor_z` regardless of the fragment's own min.z, so
//! the pasted object rests on the build plate and never crosses it.

use sc_mesh_core::Aabb;
use sc_mesh_spatial::SceneSpatial;

/// XY offset added per consecutive paste, in millimetres. Small enough to keep
/// the cascade visible without scattering copies across the build platform.
const CASCADE_STEP_MM: f64 = 5.0;

/// Lower bound on the ring-search radius, in millimetres. The per-step radius
/// is the larger XY extent of the fragment, so a hop along X or Y clears the
/// fragment's own footprint; this floor keeps the ring from shrinking to a
/// point for a fragment with no XY extent.
const MIN_RING_RADIUS_MM: f64 = 5.0;

/// Radii used by the ring search, in multiples of the per-step `ring_step`. 3
/// rings of 8 angles each cover 24 candidates before the algorithm gives up,
/// which keeps the search bounded for tight scenes.
const RING_RADII_MULTIPLIERS: [f64; 3] = [1.0, 2.0, 3.0];

/// Number of angles per ring. 8 is a compromise between resolution and budget;
/// the angles are evenly spaced around the circle starting from `+x`.
const RING_ANGLE_COUNT: usize = 8;

/// Picks a placement translation for a paste.
///
/// Geometry, in order:
/// 1. Cascade-shift the target by `cascade_count * CASCADE_STEP_MM` in both X
///    and Y, so consecutive pastes do not pile up.
/// 2. Center the fragment's XY-AABB under the shifted target.
/// 3. Drop the fragment's floor to `floor_z`, the floor of the build plate.
/// 4. Test the resulting world-AABB for overlap against `obstacles`. No
///    overlap: return that translation.
/// 5. Otherwise walk the ring pattern (8 angles x 3 radii) and return the first
///    candidate that clears every obstacle.
/// 6. If every candidate overlaps, return the step-3 translation. Overlap is
///    accepted when the budget is exhausted: it is rare in normal use, and
///    failing the paste would be worse.
///
/// Empty obstacles, such as `Aabb::empty()`, are ignored: they cannot overlap
/// anything.
pub(crate) fn paste_translation_for_fragment(
    fragment_aabb: &Aabb,
    target_world_xy: [f64; 2],
    floor_z: f64,
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

    let initial = [target_x - centre_x, target_y - centre_y, floor_z - fmin[2]];
    let ring_step = extent_x.max(extent_y).max(MIN_RING_RADIUS_MM);
    std::iter::once(initial)
        .chain(
            RING_RADII_MULTIPLIERS
                .into_iter()
                .flat_map(move |radius_mult| {
                    let radius = ring_step * radius_mult;
                    (0..RING_ANGLE_COUNT).map(move |k| {
                        let angle = (k as f64) * std::f64::consts::TAU / (RING_ANGLE_COUNT as f64);
                        [
                            initial[0] + radius * angle.cos(),
                            initial[1] + radius * angle.sin(),
                            initial[2],
                        ]
                    })
                }),
        )
        .find(|&candidate| !overlaps_any(&fmin, &fmax, candidate, obstacles))
        .unwrap_or(initial)
}

/// A plate's area on the floor: its least and greatest X and Y, in mm.
pub(crate) type Area = ([f64; 2], [f64; 2]);

/// How far apart, in mm, the spots a paste tries on a plate stand.
const SCAN_STEP_MM: f64 = 2.0;

/// Where a paste lands: the translation, and whether a plate has to be
/// added after the last for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Spot {
    pub(crate) at: [f64; 3],
    pub(crate) new_plate: bool,
}

/// Where a paste lands on the plates (#557): by the cursor, as
/// [`paste_translation_for_fragment`] looks, when that spot is on a plate
/// and clear; if not, the first spot clear on the first plate with room;
/// if none has room, on `next`, the plate added after the last.
/// With no plate to add, by the cursor as before. The boxes are what is
/// kept clear, as by the cursor.
pub(crate) fn paste_spot(
    fragment_aabb: &Aabb,
    target_world_xy: [f64; 2],
    floor_z: f64,
    cascade_count: u32,
    obstacles: &[Aabb],
    plates: &[Area],
    next: Option<Area>,
) -> Spot {
    let (fmin, fmax) = aabb_f64(fragment_aabb);
    let inside = |t: [f64; 3], area: &Area| {
        fmin[0] + t[0] >= area.0[0]
            && fmax[0] + t[0] <= area.1[0]
            && fmin[1] + t[1] >= area.0[1]
            && fmax[1] + t[1] <= area.1[1]
    };
    let by_cursor = paste_translation_for_fragment(
        fragment_aabb,
        target_world_xy,
        floor_z,
        cascade_count,
        obstacles,
    );
    if plates.iter().any(|area| inside(by_cursor, area))
        && !overlaps_any(&fmin, &fmax, by_cursor, obstacles)
    {
        return Spot {
            at: by_cursor,
            new_plate: false,
        };
    }
    let z = floor_z - fmin[2];
    for area in plates {
        if let Some(at) = clear_spot_on(&fmin, &fmax, z, area, obstacles) {
            return Spot {
                at,
                new_plate: false,
            };
        }
    }
    match next {
        // Where the next copies leave room for each other; its middle when
        // the copy is bigger than a plate.
        Some(area) => Spot {
            at: clear_spot_on(&fmin, &fmax, z, &area, &[]).unwrap_or([
                0.5 * (area.0[0] + area.1[0]) - 0.5 * (fmin[0] + fmax[0]),
                0.5 * (area.0[1] + area.1[1]) - 0.5 * (fmin[1] + fmax[1]),
                z,
            ]),
            new_plate: true,
        },
        None => Spot {
            at: by_cursor,
            new_plate: false,
        },
    }
}

/// The first spot on `area`, row by row from its front left corner, where
/// the box `fmin`..`fmax` lies wholly on it and clear of `obstacles`.
fn clear_spot_on(
    fmin: &[f64; 3],
    fmax: &[f64; 3],
    z: f64,
    area: &Area,
    obstacles: &[Aabb],
) -> Option<[f64; 3]> {
    let (x0, x1) = (area.0[0] - fmin[0], area.1[0] - fmax[0]);
    let (y0, y1) = (area.0[1] - fmin[1], area.1[1] - fmax[1]);
    if x1 < x0 || y1 < y0 {
        return None;
    }
    // Only what stands over the plate can be in the way.
    let near: Vec<Aabb> = obstacles
        .iter()
        .filter(|obstacle| {
            if obstacle.is_empty() {
                return false;
            }
            let (omin, omax) = aabb_f64(obstacle);
            omin[0] < area.1[0] && area.0[0] < omax[0] && omin[1] < area.1[1] && area.0[1] < omax[1]
        })
        .copied()
        .collect();
    let steps = |from: f64, to: f64| {
        let count = ((to - from) / SCAN_STEP_MM).floor() as usize;
        (0..=count)
            .map(move |k| from + k as f64 * SCAN_STEP_MM)
            .chain((to > from).then_some(to))
    };
    for y in steps(y0, y1) {
        for x in steps(x0, x1) {
            let at = [x, y, z];
            if !overlaps_any(fmin, fmax, at, &near) {
                return Some(at);
            }
        }
    }
    None
}

/// Gathers the world-space AABBs of every reachable leaf in `spatial`: the
/// obstacle list of [`paste_translation_for_fragment`].
///
/// Leaves whose mesh has no vertices are skipped: an empty leaf has no AABB to
/// overlap against.
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

/// Promotes an `Aabb`'s f32 corners to f64, matching the precision used by the
/// placement algorithm and the destination scene's transform matrices.
fn aabb_f64(a: &Aabb) -> ([f64; 3], [f64; 3]) {
    let min = a.min().coords();
    let max = a.max().coords();
    (
        [min[0] as f64, min[1] as f64, min[2] as f64],
        [max[0] as f64, max[1] as f64, max[2] as f64],
    )
}

/// Returns whether the fragment's world AABB at `translation` overlaps any
/// non-empty obstacle. Uses strict separation on each axis: AABBs that touch
/// face to face do not overlap, so a pasted object can sit snug against
/// another. 2 coincident AABBs still overlap (no axis is strictly separated).
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
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_spatial::SceneSpatial;
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

    use super::*;

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb::from_min_max(Vertex::new(min), Vertex::new(max))
    }

    /// Two plates of 100 x 50 side by side, as `plates::offset` lays them.
    const PLATE_0: Area = ([-50.0, -25.0], [50.0, 25.0]);
    const PLATE_1: Area = ([90.0, -25.0], [190.0, 25.0]);
    const PLATE_2: Area = ([230.0, -25.0], [330.0, 25.0]);

    fn box40() -> Aabb {
        aabb([-20.0, -20.0, 0.0], [20.0, 20.0, 10.0])
    }

    #[test]
    fn a_paste_by_the_cursor_stays_there_when_it_is_on_a_plate_and_clear() {
        let spot = paste_spot(
            &box40(),
            [-25.0, 0.0],
            0.0,
            0,
            &[],
            &[PLATE_0],
            Some(PLATE_1),
        );
        assert_eq!(spot.at, [-25.0, 0.0, 0.0]);
        assert!(!spot.new_plate);
    }

    #[test]
    fn a_paste_off_the_plates_goes_to_the_first_plate_with_room() {
        // The cursor beside the plates: the copy lands on plate 0 all the same.
        let spot = paste_spot(
            &box40(),
            [400.0, 0.0],
            0.0,
            0,
            &[],
            &[PLATE_0],
            Some(PLATE_1),
        );
        assert!(!spot.new_plate);
        let at = spot.at;
        assert!(at[0] - 20.0 >= -50.0 && at[0] + 20.0 <= 50.0, "{at:?}");
        assert!(at[1] - 20.0 >= -25.0 && at[1] + 20.0 <= 25.0, "{at:?}");
    }

    #[test]
    fn a_paste_with_no_room_on_any_plate_asks_for_a_new_one() {
        // Plate 0 holds two boxes of 40: a third one does not fit.
        let full = [
            aabb([-45.0, -20.0, 0.0], [-5.0, 20.0, 10.0]),
            aabb([5.0, -20.0, 0.0], [45.0, 20.0, 10.0]),
        ];
        let spot = paste_spot(
            &box40(),
            [0.0, 0.0],
            0.0,
            0,
            &full,
            &[PLATE_0],
            Some(PLATE_1),
        );
        assert!(spot.new_plate);
        let at = spot.at;
        assert!(
            at[0] - 20.0 >= 90.0 && at[0] + 20.0 <= 190.0,
            "on the new plate: {at:?}"
        );
        // From its front left corner, so the next copy has room beside it.
        assert_eq!((at[0], at[1]), (110.0, -5.0));
    }

    #[test]
    fn a_full_first_plate_sends_the_paste_to_the_next_with_room() {
        let full = [
            aabb([-45.0, -20.0, 0.0], [-5.0, 20.0, 10.0]),
            aabb([5.0, -20.0, 0.0], [45.0, 20.0, 10.0]),
            aabb([90.0, -25.0, 0.0], [130.0, 15.0, 10.0]),
        ];
        let spot = paste_spot(
            &box40(),
            [0.0, 0.0],
            0.0,
            0,
            &full,
            &[PLATE_0, PLATE_1],
            Some(PLATE_2),
        );
        assert!(!spot.new_plate);
        assert!(
            spot.at[0] - 20.0 >= 130.0 && spot.at[0] + 20.0 <= 190.0,
            "{:?}",
            spot.at
        );
    }

    #[test]
    fn with_no_plate_left_to_add_the_paste_stays_by_the_cursor() {
        let full = [aabb([-50.0, -25.0, 0.0], [50.0, 25.0, 10.0])];
        let spot = paste_spot(&box40(), [0.0, 0.0], 0.0, 0, &full, &[PLATE_0], None);
        assert!(!spot.new_plate);
    }

    /// Returns a 2 x 2 x 2 box centred on the origin in XY, its floor at Z = 0.
    fn unit_fragment() -> Aabb {
        aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0])
    }

    #[test]
    fn no_obstacles_returns_centred_initial_delta() {
        let t = paste_translation_for_fragment(&unit_fragment(), [5.0, 7.0], 0.0, 0, &[]);
        // centre is (0, 0), so translation = target. floor is already at 0 in
        // fragment-space, so Z translation is 0.
        assert_eq!(t, [5.0, 7.0, 0.0]);
    }

    #[test]
    fn floor_lands_at_z_zero_regardless_of_fragment_min_z() {
        let floating = aabb([-1.0, -1.0, 7.5], [1.0, 1.0, 10.0]);
        let t = paste_translation_for_fragment(&floating, [0.0, 0.0], 0.0, 0, &[]);
        assert_eq!(t[2], -7.5);

        let underground = aabb([-1.0, -1.0, -3.0], [1.0, 1.0, 4.0]);
        let t = paste_translation_for_fragment(&underground, [0.0, 0.0], 0.0, 0, &[]);
        assert_eq!(t[2], 3.0);
    }

    #[test]
    fn a_fragment_lands_on_a_floor_above_z_zero() {
        let t = paste_translation_for_fragment(&unit_fragment(), [0.0, 0.0], 5.0, 0, &[]);
        assert_eq!(t[2], 5.0);

        let floating = aabb([-1.0, -1.0, 7.5], [1.0, 1.0, 10.0]);
        let t = paste_translation_for_fragment(&floating, [0.0, 0.0], 5.0, 0, &[]);
        assert_eq!(t[2], -2.5);

        let underground = aabb([-1.0, -1.0, -3.0], [1.0, 1.0, 4.0]);
        let t = paste_translation_for_fragment(&underground, [0.0, 0.0], 5.0, 0, &[]);
        assert_eq!(t[2], 8.0);
    }

    #[test]
    fn a_fragment_on_its_floor_gets_a_positive_zero_z_translation() {
        // The slice fingerprint hashes the bits of this translation. If it were
        // -0.0, a later drag that moves nothing would turn it into +0.0 without
        // an edit, and a project saved after that drag would reopen without its
        // slices.
        let frag = unit_fragment();
        let initial = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[]);
        let obstacle_at_initial = aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]);
        let ring =
            paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[obstacle_at_initial]);
        assert!(
            !initial[2].is_sign_negative(),
            "initial z: {:?}",
            initial[2]
        );
        assert!(!ring[2].is_sign_negative(), "ring z: {:?}", ring[2]);
    }

    #[test]
    fn cascade_offset_accumulates_per_paste() {
        let frag = unit_fragment();
        let zero = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[]);
        let one = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 1, &[]);
        let two = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 2, &[]);
        assert_eq!(zero, [0.0, 0.0, 0.0]);
        assert_eq!(one, [CASCADE_STEP_MM, CASCADE_STEP_MM, 0.0]);
        assert_eq!(two, [2.0 * CASCADE_STEP_MM, 2.0 * CASCADE_STEP_MM, 0.0]);
    }

    #[test]
    fn obstacle_at_target_shifts_to_first_ring_along_plus_x() {
        // Initial delta sits the fragment at the target. An obstacle that
        // exactly overlaps the initial fragment position forces the ring
        // search; angle 0 (cos = 1, sin = 0) is the first candidate and clears
        // at radius == ring_step.
        let frag = unit_fragment();
        let obstacle_at_initial = aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[obstacle_at_initial]);
        // extent_x = extent_y = 2, MIN_RING_RADIUS_MM = 5, so ring_step = 5;
        // candidate at angle 0 = (5, 0, 0).
        assert_eq!(t, [MIN_RING_RADIUS_MM, 0.0, 0.0]);
    }

    #[test]
    fn ring_step_scales_with_fragment_extent_when_larger_than_floor() {
        // Fragment with extent 20 in XY: ring_step = max(20, 20, 5) = 20.
        let frag = aabb([-10.0, -10.0, 0.0], [10.0, 10.0, 2.0]);
        let obstacle_at_initial = aabb([-10.0, -10.0, 0.0], [10.0, 10.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[obstacle_at_initial]);
        assert_eq!(t, [20.0, 0.0, 0.0]);
    }

    #[test]
    fn all_candidates_blocked_returns_initial_overlapping_delta() {
        // An obstacle large enough to cover every ring candidate forces the
        // budget-exhausted path: the function returns the initial, overlapping
        // delta.
        let frag = unit_fragment();
        let huge = aabb([-1000.0, -1000.0, -1000.0], [1000.0, 1000.0, 1000.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[huge]);
        assert_eq!(t, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn empty_obstacle_does_not_count_as_overlap() {
        // An empty box, such as the Aabb::empty() sentinel, overlaps nothing.
        let frag = unit_fragment();
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[Aabb::empty()]);
        assert_eq!(t, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn touching_obstacle_does_not_count_as_overlap() {
        // The obstacle's max.x equals the fragment's min.x at the initial
        // position: the 2 boxes share a face. Boxes that only touch do not
        // overlap, so the initial delta is accepted without the ring search.
        let frag = unit_fragment();
        // Initial fragment world AABB = [-1,-1,0]..[1,1,2]. An obstacle whose
        // max.x = -1 touches that face.
        let touching = aabb([-3.0, -1.0, 0.0], [-1.0, 1.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[touching]);
        assert_eq!(t, [0.0, 0.0, 0.0]);
    }

    fn unit_cube_mesh() -> Arc<IndexedMesh> {
        // 8 vertices at the corners of a cube that spans [-0.5, 0.5] on each
        // axis, and 1 face. The obstacle helper needs `transformed_aabb` to
        // return Some, which needs at least 1 vertex; the AABB comes from all 8
        // vertices, so the assertions can use the corners of the cube.
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
            vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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
        // A leaf with an empty mesh has no AABB to overlap against, so the
        // helper drops it from the obstacle list.
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
        // 1 large obstacle blocks every angle at ring 1 (radius 5), and the
        // angle-0 candidate of ring 2 (radius 10) is clear. The function
        // returns that candidate: initial + 10 on +x.
        let frag = unit_fragment();
        // The obstacle covers every ring-1 candidate in XY: the radius is 5 mm
        // and the fragment reaches 1 mm further on each side, so each candidate
        // stays within 6 mm of the origin, inside the 7 mm half-width of the
        // blocker.
        let ring_blocker = aabb([-7.0, -7.0, 0.0], [7.0, 7.0, 2.0]);
        let t = paste_translation_for_fragment(&frag, [0.0, 0.0], 0.0, 0, &[ring_blocker]);
        // Ring 2 radius = 2 * 5 = 10. Candidate at angle 0 = (10, 0, 0).
        // Fragment at translation (10, 0, 0) occupies [9, -1, 0]..[11, 1, 2],
        // which is outside the blocker's max.x = 7.
        assert_eq!(t, [10.0, 0.0, 0.0]);
    }
}

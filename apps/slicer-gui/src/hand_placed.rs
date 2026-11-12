/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Which models were placed by hand (#554).
//!
//! Arrange all repacks every plate for the fewest plates, but it asks before
//! it moves a model the person laid out themselves. A model is placed by hand
//! when it stands somewhere the app did not put it: each model keeps the
//! poses it was given by Arrange, an import, a paste or its first showing,
//! and stands by hand when its pose is none of them. A move undone puts the
//! model back on a pose it was given, so it is not by hand any more.

use std::collections::HashMap;

use sc_geom::df_supports::is_any_support_mesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

/// A model's world transform: the linear part row by row, then the move.
pub(crate) type Pose = [f64; 12];

/// How many poses a model keeps: enough for a few Undo steps back.
const KEPT: usize = 8;

/// Two poses closer than this in every term are the same pose.
const SAME: f64 = 1e-5;

/// The poses each model was given.
#[derive(Debug, Default)]
pub(crate) struct Placements {
    by_leaf: HashMap<NodeId, Vec<Pose>>,
}

impl Placements {
    pub(crate) fn clear(&mut self) {
        self.by_leaf.clear();
    }

    /// `leaf` was given `pose`.
    pub(crate) fn note(&mut self, leaf: NodeId, pose: Pose) {
        let poses = self.by_leaf.entry(leaf).or_default();
        if poses.iter().any(|p| same(p, &pose)) {
            return;
        }
        if poses.len() == KEPT {
            poses.remove(0);
        }
        poses.push(pose);
    }

    /// `leaf` was given `pose`, unless it was given one before: a model the
    /// app shows for the first time.
    pub(crate) fn note_first(&mut self, leaf: NodeId, pose: Pose) {
        if !self.by_leaf.contains_key(&leaf) {
            self.by_leaf.insert(leaf, vec![pose]);
        }
    }

    /// `leaf` stands where the person put it, whatever pose it has now: a
    /// project saved with it placed by hand.
    pub(crate) fn mark_by_hand(&mut self, leaf: NodeId) {
        self.by_leaf.insert(leaf, Vec::new());
    }

    /// Whether `leaf`, standing at `pose`, was placed by hand. A model the
    /// app never saw was not.
    pub(crate) fn by_hand(&self, leaf: NodeId, pose: &Pose) -> bool {
        self.by_leaf
            .get(&leaf)
            .is_some_and(|poses| !poses.iter().any(|p| same(p, pose)))
    }
}

fn same(a: &Pose, b: &Pose) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < SAME)
}

/// Where `leaf` stands now.
pub(crate) fn pose_of(spatial: &SceneSpatial, leaf: NodeId) -> Option<Pose> {
    let world = match spatial.leaf(leaf)?.world_transform() {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => a,
    };
    let l = world.linear();
    let t = world.translation();
    Some([
        l[0][0], l[0][1], l[0][2], l[1][0], l[1][1], l[1][2], l[2][0], l[2][1], l[2][2], t[0],
        t[1], t[2],
    ])
}

/// The models of the scene, supports left out, in the list's order.
pub(crate) fn models(spatial: &SceneSpatial) -> Vec<NodeId> {
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter(|&leaf| {
            spatial
                .leaf(leaf)
                .is_some_and(|query| !is_any_support_mesh(query.mesh()))
        })
        .collect()
}

/// A pose as a project keeps it: millionths, so a file compares whole.
pub(crate) fn saved(pose: &Pose) -> [i64; 12] {
    std::array::from_fn(|k| (pose[k] * 1e6).round() as i64)
}

/// A pose a project kept.
pub(crate) fn loaded(saved: &[i64; 12]) -> Pose {
    std::array::from_fn(|k| saved[k] as f64 * 1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f64) -> Pose {
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, x, 0.0, 0.0]
    }

    #[test]
    fn a_model_off_every_pose_it_was_given_is_placed_by_hand() {
        let mut placements = Placements::default();
        let mut graph = sc_scene::Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                sc_scene::node::LeafPayload::Mesh(std::sync::Arc::new(
                    sc_mesh_fixtures::unit_cube_mesh(),
                )),
            )
            .expect("a leaf");
        assert!(!placements.by_hand(leaf, &at(1.0)), "never seen");
        placements.note_first(leaf, at(1.0));
        assert!(!placements.by_hand(leaf, &at(1.0)));
        assert!(placements.by_hand(leaf, &at(2.0)), "moved");
        placements.note(leaf, at(5.0));
        assert!(!placements.by_hand(leaf, &at(5.0)), "arranged");
        assert!(!placements.by_hand(leaf, &at(1.0)), "undone back");
        placements.note_first(leaf, at(9.0));
        assert!(placements.by_hand(leaf, &at(9.0)), "seen before");
        placements.mark_by_hand(leaf);
        assert!(placements.by_hand(leaf, &at(5.0)));
        assert_eq!(loaded(&saved(&at(12.345_678))), at(12.345_678));
    }
}

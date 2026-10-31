/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Several selected objects moved as one (#13).
//!
//! The gizmo's arrows and `G` move every selected leaf by the same offset in
//! the world, about the centre of their shared box, and the move lands on
//! the undo stack as one entry. While it is live, each model's supports ride
//! along in the plate plane, as they do for a single model (#119): a move in
//! Z leaves them standing. On commit they go back where the history has them
//! and the recorded edit takes them along or away
//! ([`crate::support_follow::commit`]). Turning and scaling several objects
//! about a shared or a per-object pivot is #218.

use glam::Vec3;
use sc_geom::df_supports::is_support_mesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::scene_document::{EditNotApplied, SceneDocument, SceneEdit};
use crate::support_persist::{in_force, model_of, sets_of_model};

/// A leaf the move sets: where it started, in its parent's frame.
#[derive(Clone, Copy, Debug)]
struct Member {
    node_id: NodeId,
    original: Transform3D,
    parent: Transform3D,
}

/// A support set that rides along with its model while the move is live.
#[derive(Clone, Copy, Debug)]
struct Rider {
    node_id: NodeId,
    start: Transform3D,
}

/// A live move of several leaves, from the press to the commit or abort.
#[derive(Debug)]
pub(super) struct GroupMove {
    members: Vec<Member>,
    riders: Vec<Rider>,
}

/// The centre of the box that holds every leaf of `ids` in the world: where
/// the gizmo stands for several objects. `None` when none of them has
/// geometry.
pub(crate) fn group_pivot(spatial: &SceneSpatial, ids: &[NodeId]) -> Option<Vec3> {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for &id in ids {
        let Some(query) = spatial.leaf(id) else {
            continue;
        };
        let Some(aabb) = query.transformed_aabb(query.world_transform()) else {
            continue;
        };
        lo = lo.min(Vec3::from(aabb.min().coords()));
        hi = hi.max(Vec3::from(aabb.max().coords()));
    }
    let pivot = (lo + hi) * 0.5;
    pivot.is_finite().then_some(pivot)
}

fn translation(d: [f64; 3]) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation(d))
}

impl GroupMove {
    /// Captures where the leaves of `ids` stand. `None` for fewer than 2
    /// leaves that can move: one leaf moves on its own path. A support set
    /// selected with its model rides along with it rather than moving twice.
    pub(super) fn capture(document: &SceneDocument, ids: &[NodeId]) -> Option<Self> {
        if ids.len() < 2 {
            return None;
        }
        // A live move of one model may have left its supports moved.
        crate::support_follow::settle(document);
        let sessions = document.support_sessions();
        let group = document.with_scene(|spatial| {
            let mut members: Vec<Member> = Vec::new();
            for &id in ids {
                let Some(query) = spatial.leaf(id) else {
                    continue;
                };
                if is_support_mesh(query.mesh())
                    && model_of(&sessions, id).is_some_and(|model| ids.contains(&model))
                {
                    continue;
                }
                members.push(Member {
                    node_id: id,
                    original: query.local_transform(),
                    parent: query.parent_world(),
                });
            }
            let mut riders: Vec<Rider> = Vec::new();
            for member in &members {
                let is_model = spatial
                    .leaf(member.node_id)
                    .is_some_and(|q| !is_support_mesh(q.mesh()));
                if !is_model {
                    continue;
                }
                for leaf in sets_of_model(spatial, &sessions, member.node_id) {
                    let Some(query) = spatial.leaf(leaf) else {
                        continue;
                    };
                    if members.iter().any(|m| m.node_id == leaf)
                        || riders.iter().any(|r| r.node_id == leaf)
                        || !query.parent_world().is_identity()
                        || in_force(spatial, &sessions, leaf).is_none()
                    {
                        continue;
                    }
                    riders.push(Rider {
                        node_id: leaf,
                        start: query.local_transform(),
                    });
                }
            }
            Self { members, riders }
        })?;
        (group.members.len() >= 2).then_some(group)
    }

    /// How far up the offset `d` must go so that no leaf it moves sinks
    /// below the plate or below where it started, whichever is lower: the
    /// floor `G` keeps, for the whole group at once so it moves as one.
    pub(super) fn floor_lift(&self, document: &SceneDocument, d: [f64; 3]) -> f64 {
        let shift = translation(d);
        document
            .with_scene(|spatial| {
                self.members
                    .iter()
                    .filter_map(|m| {
                        let query = spatial.leaf(m.node_id)?;
                        let start = m.parent.compose(&m.original);
                        let baseline = query.minimum_world_z(start)?;
                        let next = query.minimum_world_z(shift.compose(&start))?;
                        Some((baseline.min(0.0) - next).max(0.0))
                    })
                    .fold(0.0, f64::max)
            })
            .unwrap_or(0.0)
    }

    /// Sets every leaf where the offset `d` (world millimetres) takes it from
    /// where it started, and each model's supports along in the plate plane.
    /// Not recorded: the commit records the move.
    pub(super) fn apply(
        &self,
        document: &SceneDocument,
        d: [f64; 3],
    ) -> Result<(), EditNotApplied> {
        if !d.iter().all(|c| c.is_finite()) {
            return Ok(());
        }
        let shift = translation(d);
        // As one model's supports do: they follow a move in the plate plane
        // and stand where they were for any other.
        let ride = if d[2].abs() <= 1e-6 {
            translation([d[0], d[1], 0.0])
        } else {
            Transform3D::Identity
        };
        let edits = document
            .with_scene(|spatial| {
                let mut edits = Vec::new();
                for m in &self.members {
                    let Some(query) = spatial.leaf(m.node_id) else {
                        continue;
                    };
                    let Ok(up) = m.parent.inverse() else {
                        continue;
                    };
                    let after = up.compose(&shift.compose(&m.parent.compose(&m.original)));
                    edits.push(SceneEdit::SetLocalTransform {
                        node_id: m.node_id,
                        before: query.local_transform(),
                        after,
                    });
                }
                for r in &self.riders {
                    let Some(query) = spatial.leaf(r.node_id) else {
                        continue;
                    };
                    edits.push(SceneEdit::SetLocalTransform {
                        node_id: r.node_id,
                        before: query.local_transform(),
                        after: ride.compose(&r.start),
                    });
                }
                edits
            })
            .unwrap_or_default();
        let edits: Vec<SceneEdit> = edits.into_iter().filter(|e| !e.is_noop()).collect();
        if edits.is_empty() {
            return Ok(());
        }
        document.apply_edit_unfollowed(SceneEdit::Compound(edits))
    }

    /// Ends the move: the supports go back where the history has them and
    /// the leaves' move is recorded as one undo entry, which takes the
    /// supports along or away.
    pub(super) fn commit(&self, document: &SceneDocument) {
        self.put_riders_back(document);
        let moves: Vec<SceneEdit> = document
            .with_scene(|spatial| {
                self.members
                    .iter()
                    .filter_map(|m| {
                        Some(SceneEdit::SetLocalTransform {
                            node_id: m.node_id,
                            before: m.original,
                            after: spatial.leaf(m.node_id)?.local_transform(),
                        })
                    })
                    .filter(|e| !e.is_noop())
                    .collect()
            })
            .unwrap_or_default();
        let mut moves = moves;
        let edit = match moves.len() {
            0 => return,
            1 => moves.pop().expect("len == 1"),
            _ => SceneEdit::Compound(moves),
        };
        document.push_edit(edit);
    }

    /// Cancels the move: every leaf and support set goes back where it
    /// started, and nothing is recorded.
    pub(super) fn abort(&self, document: &SceneDocument) {
        let starts = self
            .members
            .iter()
            .map(|m| (m.node_id, m.original))
            .chain(self.riders.iter().map(|r| (r.node_id, r.start)));
        set_unrecorded(document, starts);
    }

    fn put_riders_back(&self, document: &SceneDocument) {
        set_unrecorded(document, self.riders.iter().map(|r| (r.node_id, r.start)));
    }
}

/// Sets each leaf to its transform, in one edit that is not recorded.
fn set_unrecorded(document: &SceneDocument, targets: impl Iterator<Item = (NodeId, Transform3D)>) {
    let edits: Vec<SceneEdit> = document
        .with_scene(|spatial| {
            targets
                .filter_map(|(node_id, after)| {
                    Some(SceneEdit::SetLocalTransform {
                        node_id,
                        before: spatial.leaf(node_id)?.local_transform(),
                        after,
                    })
                })
                .filter(|e| !e.is_noop())
                .collect()
        })
        .unwrap_or_default();
    if !edits.is_empty() {
        // A rejected revert leaves the leaves where the last tick put them;
        // the gesture still ends.
        let _ = document.apply_edit_unfollowed(SceneEdit::Compound(edits));
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Several selected objects moved, turned or scaled as one (#13, #218).
//!
//! The gizmo, `G`, `R`, `S` and the arrow keys work out what the gesture does
//! to the primary leaf about the centre of the group's shared box, and every
//! selected leaf gets the same world transform; the gesture lands on the
//! undo stack as one entry. While it is live, each model's supports ride
//! along when it keeps to the plate plane (a move in X/Y, a turn about the
//! vertical axis), as they do for a single model (#119, #153); a move in Z, a
//! tilt or a scale leaves them standing. On commit they go back where the
//! history has them and the recorded edit takes them along or away to be
//! made again ([`crate::support_follow::commit`]). Turning each object about
//! its own centre is the rest of #218.

use std::cell::RefCell;

use glam::Vec3;
use sc_geom::df_supports::is_support_mesh;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::{ObjectQuery, SceneSpatial};
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

/// A live move, turn or scale of several leaves, from the press to the
/// commit or abort.
#[derive(Debug)]
pub(super) struct GroupMove {
    members: Vec<Member>,
    riders: Vec<Rider>,
    /// Where the members and the riders stood when the gesture began or was
    /// last re-anchored: what the next world transform applies to.
    base: RefCell<(Vec<Transform3D>, Vec<Transform3D>)>,
}

/// The centre of the box that holds every leaf of `ids` in the world: where
/// the gizmo stands for several objects, and the pivot they turn and scale
/// about. `None` when none of them has geometry.
///
/// Each leaf counts by the corners of its own box carried into the world
/// ([`world_box_corners`]), not by its tight world box: the hover asks for
/// this on every pointer move, and copies of one model share a mesh whose
/// tight-box cache holds one turn, so copies turned apart would walk every
/// vertex on each move (#13).
pub(crate) fn group_pivot(spatial: &SceneSpatial, ids: &[NodeId]) -> Option<Vec3> {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for &id in ids {
        let Some(corners) = spatial.leaf(id).and_then(|q| world_box_corners(&q)) else {
            continue;
        };
        for c in corners {
            lo = lo.min(c);
            hi = hi.max(c);
        }
    }
    let pivot = (lo + hi) * 0.5;
    pivot.is_finite().then_some(pivot)
}

/// The 8 corners of a leaf's own box, carried into the world: a box around
/// the leaf that costs no walk over its vertices (the local box is cached).
/// Looser than the tight world box when the leaf is turned.
pub(crate) fn world_box_corners(query: &ObjectQuery<'_>) -> Option<[Vec3; 8]> {
    let aabb = query.local_aabb()?;
    let (lo, hi) = (aabb.min().coords(), aabb.max().coords());
    let world = query.world_transform();
    let corners = std::array::from_fn(|k| {
        let corner = Vertex::new([
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ]);
        Vec3::from(world.apply_to_vertex(corner).coords())
    });
    corners
        .iter()
        .all(|c: &Vec3| c.is_finite())
        .then_some(corners)
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
            let base = members.iter().map(|m| m.original).collect();
            let rider_base = riders.iter().map(|r| r.start).collect();
            Self {
                members,
                riders,
                base: RefCell::new((base, rider_base)),
            }
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

    /// `delta`, a turn or a scale about the group's pivot, followed by the
    /// lift in Z that keeps the group on the plate as a single object keeps
    /// it ([`super::bed_clearance::keep_above_bed`]): the lowest point of the
    /// whole group stays where it was, or on the plate when it rested there.
    /// One lift for every leaf, so the group stays rigid.
    pub(super) fn kept_on_bed(&self, document: &SceneDocument, delta: Transform3D) -> Transform3D {
        // A turn about the vertical axis leaves every lowest point where it
        // was; a lift would only make the supports give way.
        if in_plane(&delta) {
            return delta;
        }
        let lows = document.with_scene(|spatial| {
            let (base, _) = &*self.base.borrow();
            let mut before = f64::INFINITY;
            let mut after = f64::INFINITY;
            for (m, b) in self.members.iter().zip(base) {
                let Some(query) = spatial.leaf(m.node_id) else {
                    continue;
                };
                let world = m.parent.compose(b);
                if let (Some(lo), Some(hi)) = (
                    query.minimum_world_z(world),
                    query.minimum_world_z(delta.compose(&world)),
                ) {
                    before = before.min(lo);
                    after = after.min(hi);
                }
            }
            (before, after)
        });
        let Some((before, after)) = lows else {
            return delta;
        };
        if !before.is_finite() || !after.is_finite() {
            return delta;
        }
        let target = if before > 1e-4 { before } else { 0.0 };
        let correction = target - after;
        if correction.abs() <= 1e-6 {
            return delta;
        }
        // Tiny clearance absorbs f32 GPU rounding, as for one object.
        translation([0.0, 0.0, correction + 1e-5]).compose(&delta)
    }

    /// Sets every leaf where the offset `d` (world millimetres) takes it from
    /// where it started. See [`Self::apply_world`].
    pub(super) fn apply(
        &self,
        document: &SceneDocument,
        d: [f64; 3],
    ) -> Result<(), EditNotApplied> {
        if !d.iter().all(|c| c.is_finite()) {
            return Ok(());
        }
        self.apply_world(document, translation(d))
    }

    /// Sets every leaf where the world transform `delta` takes it from its
    /// base (where it stood when the gesture began or was last re-anchored),
    /// and each model's supports along when `delta` keeps to the plate plane:
    /// a move in X/Y, maybe after a turn about the vertical axis. For any
    /// other motion they stand where they were, as one model's do (#153).
    /// Not recorded: the commit records the gesture.
    pub(super) fn apply_world(
        &self,
        document: &SceneDocument,
        delta: Transform3D,
    ) -> Result<(), EditNotApplied> {
        let ride = if in_plane(&delta) {
            delta
        } else {
            Transform3D::Identity
        };
        let edits = document
            .with_scene(|spatial| {
                let (base, rider_base) = &*self.base.borrow();
                let mut edits = Vec::new();
                for (m, b) in self.members.iter().zip(base) {
                    let Some(query) = spatial.leaf(m.node_id) else {
                        continue;
                    };
                    let Ok(up) = m.parent.inverse() else {
                        continue;
                    };
                    let after = up.compose(&delta.compose(&m.parent.compose(b)));
                    edits.push(SceneEdit::SetLocalTransform {
                        node_id: m.node_id,
                        before: query.local_transform(),
                        after,
                    });
                }
                for (r, b) in self.riders.iter().zip(rider_base) {
                    let Some(query) = spatial.leaf(r.node_id) else {
                        continue;
                    };
                    edits.push(SceneEdit::SetLocalTransform {
                        node_id: r.node_id,
                        before: query.local_transform(),
                        after: ride.compose(b),
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

    /// Ends the gesture: the supports go back where the history has them and
    /// the leaves' change is recorded as one undo entry, which takes the
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

    /// Cancels the gesture: every leaf and support set goes back where it
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

/// The world transform that takes the world pose `from` to `to`:
/// `to ∘ from⁻¹`. What a one-object drag of the primary leaf did, for the
/// other leaves of the group to do the same about the same pivot.
pub(super) fn world_delta(from: &Transform3D, to: &Transform3D) -> Option<Transform3D> {
    let delta = to.compose(&from.inverse().ok()?);
    let finite = match &delta {
        Transform3D::Identity => true,
        Transform3D::Affine(a) => {
            a.linear().iter().flatten().all(|v| v.is_finite())
                && a.translation().iter().all(|v| v.is_finite())
        }
    };
    finite.then_some(delta)
}

/// Whether `delta` keeps to the plate plane: no move in Z, no tilt, no
/// scale; a turn about the vertical axis and a move in X/Y at most. The
/// motions a model's supports follow (#119, #153).
fn in_plane(delta: &Transform3D) -> bool {
    let Transform3D::Affine(a) = delta else {
        return true;
    };
    let l = a.linear();
    let (c, s) = (l[0][0], l[1][0]);
    (l[1][1] - c).abs() <= 1e-9
        && (l[0][1] + s).abs() <= 1e-9
        && (l[2][2] - 1.0).abs() <= 1e-9
        && [l[0][2], l[1][2], l[2][0], l[2][1]]
            .iter()
            .all(|v| v.abs() <= 1e-9)
        && (c * c + s * s - 1.0).abs() <= 1e-9
        && a.translation()[2].abs() <= 1e-6
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

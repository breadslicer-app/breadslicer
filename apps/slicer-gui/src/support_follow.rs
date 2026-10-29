/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Supports that move with their model in X/Y (#119), and that make way
//! for new ones when it moves in Z, turns or scales (#153).
//!
//! The document calls in here for every transform edit of a leaf, however it
//! was made (the gizmo, `G`, the arrow keys, the Transform inspector,
//! Center). Each model has a support set of its own (#197): while a move is
//! live, the model's support leaf rides along on its own transform. When the
//! move is committed in the plate plane, the set's session is moved with the
//! model ([`DfSupportSession::moved_in_plane`]) and redrawn, the redrawn mesh
//! takes the old one's place in the same undo entry as the move, and the
//! support leaf is back where it was. Nothing is placed again, so the
//! supports stay the ones the person made, editable by hand and saved as
//! data. The other models' sets are left alone.
//!
//! A move in Z, a rotation or a scale (Drop to plate and a lift clear of the
//! raft among them) leaves the supports where they stood while it is live.
//! When it is committed, they no longer fit the model: the set is removed in
//! the same undo entry, and the model waits for new supports, which the
//! support driver generates and adds to that entry
//! ([`SceneDocument::supports_wanted`]).

use std::sync::Arc;

use sc_geom::df_supports::{DfSupportSession, is_support_mesh};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;

use crate::scene_document::{SceneDocument, SceneEdit, SceneRevision, leaf_mesh_swap};
use crate::support_persist::{in_force, in_force_at, sets_of_model};

/// Where a model and its supports stood when it started to move: the scene
/// as last committed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Anchor {
    revision: SceneRevision,
    model: NodeId,
    model_start: Transform3D,
    supports: NodeId,
    supports_start: Transform3D,
}

/// The leaf a transform edit moves; `None` for any other edit.
pub(crate) fn moved_leaf(edit: &SceneEdit) -> Option<NodeId> {
    match edit {
        SceneEdit::SetLocalTransform { node_id, .. } => Some(*node_id),
        _ => None,
    }
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => *a,
    }
}

/// The move in the plate plane that takes the world transform `from` to `to`,
/// when that is all that changed: no rotation, no scale, nothing in Z. `None`
/// otherwise, and for no move at all.
fn plane_move(from: &Transform3D, to: &Transform3D) -> Option<[f64; 2]> {
    let (a, b) = (affine(from), affine(to));
    let same_linear = a
        .linear()
        .iter()
        .flatten()
        .zip(b.linear().iter().flatten())
        .all(|(x, y)| (x - y).abs() <= 1e-9);
    let [ax, ay, az] = *a.translation();
    let [bx, by, bz] = *b.translation();
    let d = [bx - ax, by - ay];
    (same_linear && (bz - az).abs() <= 1e-6 && d != [0.0, 0.0]).then_some(d)
}

/// The support set of the model `model` whose session is in force, with the
/// model standing at `model_world`: its leaf and transform, when the leaf is
/// a root (its mesh is in world space, as the session draws it).
fn model_set(
    doc: &SceneDocument,
    model: NodeId,
    model_world: Option<&Transform3D>,
) -> Option<(NodeId, Arc<DfSupportSession>, Transform3D)> {
    let sessions = doc.support_sessions();
    doc.with_scene(|spatial| {
        sets_of_model(spatial, &sessions, model)
            .into_iter()
            .find_map(|leaf| {
                let drawn = match model_world {
                    Some(world) => in_force_at(spatial, &sessions, leaf, world),
                    None => in_force(spatial, &sessions, leaf),
                }?;
                let query = spatial.leaf(leaf)?;
                query
                    .parent_world()
                    .is_identity()
                    .then(|| (leaf, drawn.session, query.local_transform()))
            })
    })
    .flatten()
}

/// Before a live edit of `leaf`'s transform: the first one since the last
/// commit remembers where the model and its supports stand.
pub(crate) fn anchor(doc: &SceneDocument, leaf: NodeId) {
    let revision = doc.scene_revision();
    let slot = doc.support_follow();
    if slot
        .get()
        .is_some_and(|a| a.revision == revision && a.model == leaf)
    {
        return;
    }
    // A live move of another model may have left its supports moved.
    settle(doc);
    if doc.support_sessions().borrow().is_empty() {
        return;
    }
    let model = doc
        .with_scene(|spatial| {
            let query = spatial.leaf(leaf)?;
            (!is_support_mesh(query.mesh())).then(|| query.local_transform())
        })
        .flatten();
    let (Some(model_start), Some((supports, _, supports_start))) =
        (model, model_set(doc, leaf, None))
    else {
        return;
    };
    slot.set(Some(Anchor {
        revision,
        model: leaf,
        model_start,
        supports,
        supports_start,
    }));
}

/// After a live edit of `leaf`'s transform: its support leaf rides along by
/// the model's move in the plate plane, or stands where it was.
pub(crate) fn follow_live(doc: &SceneDocument, leaf: NodeId) {
    let Some(anchor) = doc.support_follow().get() else {
        return;
    };
    if anchor.model != leaf || anchor.revision != doc.scene_revision() {
        return;
    }
    let moved = doc
        .with_scene(|spatial| {
            let query = spatial.leaf(leaf)?;
            let parent = query.parent_world();
            plane_move(
                &parent.compose(&anchor.model_start),
                &query.world_transform(),
            )
        })
        .flatten();
    let target = match moved {
        Some([dx, dy]) => Transform3D::Affine(AffineTransform3D::from_translation([dx, dy, 0.0]))
            .compose(&anchor.supports_start),
        None => anchor.supports_start,
    };
    set_supports_transform(doc, anchor.supports, target);
}

/// Ends a live move: the support leaf goes back where the history has it.
/// Once the scene has moved on (a commit, Undo, another scene) the anchor no
/// longer describes it and is only dropped.
pub(crate) fn settle(doc: &SceneDocument) {
    if let Some(anchor) = doc.support_follow().take()
        && anchor.revision == doc.scene_revision()
    {
        set_supports_transform(doc, anchor.supports, anchor.supports_start);
    }
}

fn set_supports_transform(doc: &SceneDocument, supports: NodeId, target: Transform3D) {
    let Some(current) = doc
        .with_scene(|spatial| spatial.leaf(supports).map(|q| q.local_transform()))
        .flatten()
    else {
        return;
    };
    if current != target {
        // Not recorded: it only shows the supports where the live move puts
        // them, and the commit puts the leaf back.
        let _ = doc.apply_edit_unfollowed(SceneEdit::SetLocalTransform {
            node_id: supports,
            before: current,
            after: target,
        });
    }
}

/// The support set a committed move redrew, once the edit is recorded: its
/// leaf, its model and the session that drew it.
pub(crate) struct MovedSet {
    pub(crate) leaf: NodeId,
    pub(crate) model: NodeId,
    pub(crate) session: Arc<DfSupportSession>,
}

/// An edit about to be recorded, as [`commit`] hands it back.
pub(crate) struct Committed {
    /// The edit, with the supports' part.
    pub(crate) edit: SceneEdit,
    /// The set a move in the plate plane redrew.
    pub(crate) moved: Option<MovedSet>,
    /// The models whose supports a move in Z, a rotation or a scale took
    /// away: they get new ones (#153).
    pub(crate) regenerate: Vec<NodeId>,
    /// Whether the supports taken away were edited by hand.
    pub(crate) hand_edits_lost: bool,
}

/// An edit about to be recorded. Supports a live move left moved go back
/// first, whatever the edit. A move of a model in the plate plane comes back
/// with its supports redrawn where the model now is, applied and in the same
/// entry, and the set it redrew. Any other move of a model, by itself or
/// among others in one edit, comes back with the model's supports removed,
/// applied and in the same entry, and the model to regenerate. Any other edit
/// comes back as it was.
pub(crate) fn commit(doc: &SceneDocument, edit: SceneEdit) -> Committed {
    settle(doc);
    if let Some((swap, moved)) = follow_commit(doc, &edit) {
        return Committed {
            edit: SceneEdit::Compound(vec![edit, swap]),
            moved: Some(moved),
            regenerate: Vec::new(),
            hand_edits_lost: false,
        };
    }
    match take_commit(doc, &edit) {
        Some((removal, regenerate, hand_edits_lost)) => Committed {
            edit: SceneEdit::Compound(vec![edit, removal]),
            moved: None,
            regenerate,
            hand_edits_lost,
        },
        None => Committed {
            edit,
            moved: None,
            regenerate: Vec::new(),
            hand_edits_lost: false,
        },
    }
}

/// The transform edits `edit` makes: itself, or each part of a compound
/// that only moves leaves (a lift of several models clear of the raft).
fn transform_edits(edit: &SceneEdit) -> Vec<(NodeId, &Transform3D, &Transform3D)> {
    let parts = match edit {
        SceneEdit::Compound(parts) => parts.as_slice(),
        other => std::slice::from_ref(other),
    };
    parts
        .iter()
        .map(|part| match part {
            SceneEdit::SetLocalTransform {
                node_id,
                before,
                after,
            } => Some((*node_id, before, after)),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// For a committed move of models other than in the plate plane: the edit
/// that removes the support sets in force where they came from, applied,
/// the models whose sets it removes, and whether any of them was edited by
/// hand. `None` when no such model has a set in force.
fn take_commit(doc: &SceneDocument, edit: &SceneEdit) -> Option<(SceneEdit, Vec<NodeId>, bool)> {
    if doc.support_sessions().borrow().is_empty() {
        return None;
    }
    let mut taken: Vec<(NodeId, NodeId)> = Vec::new();
    let mut hand_edits_lost = false;
    for (node_id, before, after) in transform_edits(edit) {
        if before == after {
            continue;
        }
        let from = doc
            .with_scene(|spatial| {
                let query = spatial.leaf(node_id)?;
                if is_support_mesh(query.mesh()) {
                    return None;
                }
                let parent = query.parent_world();
                let from = parent.compose(before);
                // A move in the plate plane takes them along instead.
                plane_move(&from, &parent.compose(after))
                    .is_none()
                    .then_some(from)
            })
            .flatten();
        // The edit is already live: the set was in force where the model
        // came from.
        if let Some(from) = from
            && let Some((supports, session, _)) = model_set(doc, node_id, Some(&from))
        {
            hand_edits_lost |= session.hand_edited();
            taken.push((node_id, supports));
        }
    }
    if taken.is_empty() {
        return None;
    }
    let mut graph = doc.with_scene(|spatial| spatial.graph().clone())?;
    let mut removals = Vec::new();
    // Each removal is worked out on the scene the ones before it leave.
    for (_, supports) in &taken {
        if let Some(remove) = SceneEdit::remove_leaf_with_gc(&graph, *supports) {
            remove.apply_to_scene(&mut graph).ok()?;
            removals.push(remove);
        }
    }
    let removal = match removals.len() {
        0 => return None,
        1 => removals.pop().expect("len == 1"),
        _ => SceneEdit::Compound(removals),
    };
    doc.apply_edit_unfollowed(removal.clone()).ok()?;
    crate::breadcrumbs::note(&format!(
        "supports: {} set(s) removed to regenerate",
        taken.len()
    ));
    Some((
        removal,
        taken.into_iter().map(|(model, _)| model).collect(),
        hand_edits_lost,
    ))
}

fn follow_commit(doc: &SceneDocument, edit: &SceneEdit) -> Option<(SceneEdit, MovedSet)> {
    let SceneEdit::SetLocalTransform {
        node_id,
        before,
        after,
    } = edit
    else {
        return None;
    };
    if doc.support_sessions().borrow().is_empty() {
        return None;
    }
    let (delta, from) = doc
        .with_scene(|spatial| {
            let query = spatial.leaf(*node_id)?;
            if is_support_mesh(query.mesh()) {
                return None;
            }
            let parent = query.parent_world();
            let from = parent.compose(before);
            Some((plane_move(&from, &parent.compose(after))?, from))
        })
        .flatten()?;
    // The edit is already live: the set was in force where the model came
    // from.
    let (supports, session, _) = model_set(doc, *node_id, Some(&from))?;
    let moved = session.moved_in_plane(0, delta);
    // The port is DragonFruit's code: a panic in it leaves the supports
    // where they were, as before, not a failed move.
    let mesh = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| moved.mesh()))
        .ok()?
        .ok()??;
    crate::breadcrumbs::note(&format!(
        "supports: moved with their model by {delta:?} ({} triangles)",
        mesh.faces().len()
    ));
    let swap = doc
        .with_scene(|spatial| leaf_mesh_swap(spatial.graph(), supports, Arc::new(mesh)))
        .flatten()?;
    doc.apply_edit_unfollowed(swap.clone()).ok()?;
    Some((
        swap,
        MovedSet {
            leaf: supports,
            model: *node_id,
            session: Arc::new(moved),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: [f64; 3]) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation(t))
    }

    #[test]
    fn only_a_move_in_the_plate_plane_counts() {
        assert_eq!(
            plane_move(&at([1.0, 2.0, 3.0]), &at([4.0, 0.0, 3.0])),
            Some([3.0, -2.0])
        );
        assert_eq!(
            plane_move(&Transform3D::Identity, &at([0.5, 0.0, 0.0])),
            Some([0.5, 0.0])
        );
        assert_eq!(plane_move(&at([1.0, 2.0, 3.0]), &at([1.0, 2.0, 3.0])), None);
        assert_eq!(plane_move(&at([1.0, 2.0, 3.0]), &at([1.0, 2.0, 4.0])), None);
        let turned = Transform3D::Affine(AffineTransform3D::from_rotation_z(0.3));
        assert_eq!(plane_move(&Transform3D::Identity, &turned), None);
        let scaled = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
            [5.0, 0.0, 0.0],
        ));
        assert_eq!(plane_move(&Transform3D::Identity, &scaled), None);
    }

    #[test]
    fn a_compound_of_moves_counts_as_each_move() {
        let mut scene = sc_scene::Scene::new();
        let mut leaf = || {
            scene
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    sc_scene::LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap()
        };
        let (a, b) = (leaf(), leaf());
        let lift = |node_id| SceneEdit::SetLocalTransform {
            node_id,
            before: at([0.0, 0.0, 0.0]),
            after: at([0.0, 0.0, 2.0]),
        };
        assert_eq!(transform_edits(&lift(a)).len(), 1);
        let both = SceneEdit::Compound(vec![lift(a), lift(b)]);
        let moved: Vec<NodeId> = transform_edits(&both).iter().map(|m| m.0).collect();
        assert_eq!(moved, vec![a, b]);
        let mixed = SceneEdit::Compound(vec![lift(a), SceneEdit::Compound(Vec::new())]);
        assert!(transform_edits(&mixed).is_empty(), "not only moves");
    }
}

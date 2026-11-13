/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Supports that move with their model in the plate plane, in X/Y (#119) and
//! turned about the vertical axis (#153), and that make way for new ones
//! when it moves in Z, tilts or scales (#153).
//!
//! The document calls in here for every transform edit of a leaf, however it
//! was made (the gizmo, `G`, `R`, the arrow keys, the Transform inspector,
//! Center). Each model has a support set of its own (#197): while a move in
//! the plate plane is live, the model's support leaf rides along on its own
//! transform. When it is committed, the set's session is taken along with the
//! model ([`DfSupportSession::moved_in_plane`],
//! [`DfSupportSession::turned_in_plane`]), and the support leaf keeps its
//! mesh and takes the move on its own transform, as one rigid piece with the
//! model, in the same undo entry as the move (#560). Nothing is drawn again:
//! an Arrange of a hundred models would otherwise draw millions of triangles
//! and leave the old ones to Undo. A set drawn anew later (a hand edit,
//! Generate) is drawn where its session stands, in world space, and its leaf
//! goes back to no transform
//! ([`crate::scene_document::leaf_mesh_swap`]). A turn about the vertical
//! axis leaves the model
//! standing on the plate as it did, so its supports still hold it. Nothing
//! is placed again, so the supports stay the ones the person made, editable
//! by hand and saved as data. The other models' sets are left alone. One
//! edit may move several models (Arrange, #180): each one's set is taken
//! along in that edit's entry.
//!
//! A move in Z, a tilt or a scale (Drop to plate and a lift clear of the
//! raft among them) leaves the supports where they stood while it is live.
//! When it is committed, they no longer fit the model: the set is removed in
//! the same undo entry, and the model waits for new supports, which the
//! support driver generates and adds to that entry
//! ([`SceneDocument::supports_wanted`]).
//!
//! The supports a pre-supported file came with (#258) follow their model the
//! same way (#259): a move in the plate plane takes their leaf along on its
//! transform, and any other move removes them in the same entry and leaves
//! the model waiting for supports made here
//! ([`crate::imported_supports`]).

use std::sync::Arc;

use sc_geom::df_supports::{DfSupportSession, is_any_support_mesh, scene_model_leaves};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;

use crate::imported_supports::imported_of;
use crate::scene_document::{SceneDocument, SceneEdit, SceneRevision};
use crate::support_persist::{in_force, in_force_at, sets_of_model};

/// Where a model and its supports stood when it started to move: the scene
/// as last committed. The supports are its set in force, its imported
/// supports (#259), or both.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Anchor {
    revision: SceneRevision,
    model: NodeId,
    model_start: Transform3D,
    supports: Option<(NodeId, Transform3D)>,
    imported: Option<(NodeId, Transform3D)>,
}

impl Anchor {
    /// Each support leaf that rides along, with where it started.
    fn riders(&self) -> impl Iterator<Item = (NodeId, Transform3D)> {
        self.supports.into_iter().chain(self.imported)
    }
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

/// A move of a model that its supports follow: in X/Y, or a turn about the
/// vertical axis and then a move in X/Y (mm, radians).
#[derive(Clone, Copy, Debug, PartialEq)]
enum PlaneMotion {
    Move([f64; 2]),
    Turn { angle: f64, d: [f64; 2] },
}

impl PlaneMotion {
    /// The motion as a world transform.
    fn affine(self) -> AffineTransform3D {
        match self {
            PlaneMotion::Move([dx, dy]) => AffineTransform3D::from_translation([dx, dy, 0.0]),
            PlaneMotion::Turn { angle, d: [dx, dy] } => {
                let (s, c) = angle.sin_cos();
                AffineTransform3D::from_linear_and_translation(
                    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
                    [dx, dy, 0.0],
                )
            }
        }
    }
}

/// The motion in the plate plane that takes the world transform `from` to
/// `to`, when that is all that changed: a move in X/Y, maybe after a turn
/// about the vertical axis; nothing in Z, no tilt, no scale. `None`
/// otherwise, and for no move at all.
fn plane_motion(from: &Transform3D, to: &Transform3D) -> Option<PlaneMotion> {
    if let Some(d) = plane_move(from, to) {
        return Some(PlaneMotion::Move(d));
    }
    let delta = affine(to).compose(&affine(from).inverse().ok()?);
    let l = delta.linear();
    let (c, s) = (l[0][0], l[1][0]);
    let about_z = (l[1][1] - c).abs() <= 1e-9
        && (l[0][1] + s).abs() <= 1e-9
        && (l[2][2] - 1.0).abs() <= 1e-9
        && [l[0][2], l[1][2], l[2][0], l[2][1]]
            .iter()
            .all(|v| v.abs() <= 1e-9)
        && (c * c + s * s - 1.0).abs() <= 1e-9;
    let [dx, dy, dz] = *delta.translation();
    let angle = s.atan2(c);
    (about_z && dz.abs() <= 1e-6 && angle != 0.0)
        .then_some(PlaneMotion::Turn { angle, d: [dx, dy] })
}

/// Whether a model going from the world pose `from` to `to` keeps its
/// supports, which move and turn with it (#153), instead of having them made
/// again: the rule [`commit`] applies.
#[cfg(test)]
pub(crate) fn supports_follow(from: &Transform3D, to: &Transform3D) -> bool {
    plane_motion(from, to).is_some()
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
    let Some((model_start, imported)) = doc
        .with_scene(|spatial| {
            let query = spatial.leaf(leaf)?;
            if is_any_support_mesh(query.mesh()) {
                return None;
            }
            let imported = imported_of(spatial, leaf, None)
                .first()
                .and_then(|&set| Some((set, spatial.leaf(set)?.local_transform())));
            Some((query.local_transform(), imported))
        })
        .flatten()
    else {
        return;
    };
    let supports = if doc.support_sessions().borrow().is_empty() {
        None
    } else {
        model_set(doc, leaf, None).map(|(supports, _, start)| (supports, start))
    };
    if supports.is_none() && imported.is_none() {
        return;
    }
    slot.set(Some(Anchor {
        revision,
        model: leaf,
        model_start,
        supports,
        imported,
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
            plane_motion(
                &parent.compose(&anchor.model_start),
                &query.world_transform(),
            )
        })
        .flatten();
    for (supports, start) in anchor.riders() {
        let target = match moved {
            Some(motion) => Transform3D::Affine(motion.affine()).compose(&start),
            None => start,
        };
        set_supports_transform(doc, supports, target);
    }
}

/// Ends a live move: the support leaf goes back where the history has it.
/// Once the scene has moved on (a commit, Undo, another scene) the anchor no
/// longer describes it and is only dropped.
pub(crate) fn settle(doc: &SceneDocument) {
    if let Some(anchor) = doc.support_follow().take()
        && anchor.revision == doc.scene_revision()
    {
        for (supports, start) in anchor.riders() {
            set_supports_transform(doc, supports, start);
        }
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

/// The set of `session`, made on one model and standing on it alone, taken
/// along by a move of that model `delta` (mm along X and Y) in the plate
/// plane and drawn there (#119): the set a copy of the model gets, standing
/// where the copy does (Fill bed, #356; paste, #113). Nothing is placed
/// again, so hand edits stay. `None` when it cannot be: a session of
/// several models, supports standing on another, or the engine failing.
pub(crate) fn moved_set(
    session: &DfSupportSession,
    delta: [f64; 2],
) -> Option<(Arc<DfSupportSession>, Arc<sc_mesh_core::IndexedMesh>)> {
    if session.model_count() != 1 || !session.supports_only_on(0) {
        return None;
    }
    // The port is DragonFruit's code: a panic in it leaves the copy without
    // a set drawn for it, not a failed copy.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let moved = session.moved_in_plane(0, delta);
        let mesh = moved.mesh().ok()??;
        Some((Arc::new(moved), Arc::new(mesh)))
    }))
    .ok()
    .flatten()
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
    /// The sets the moves in the plate plane redrew.
    pub(crate) moved: Vec<MovedSet>,
    /// The models whose supports a move in Z, a tilt or a scale took away:
    /// they get new ones (#153).
    pub(crate) regenerate: Vec<NodeId>,
    /// Whether the supports taken away were edited by hand.
    pub(crate) hand_edits_lost: bool,
}

/// An edit about to be recorded. Supports a live move left moved go back
/// first, whatever the edit. Each move of a model in the plate plane, by
/// itself or among others in one edit (Arrange, #180), comes back with the
/// model's supports redrawn where the model now is, applied and in the same
/// entry, and the set it redrew. Any other move of a model comes back with
/// the model's supports removed, applied and in the same entry, and the model
/// to regenerate. Any other edit comes back as it was.
pub(crate) fn commit(doc: &SceneDocument, edit: SceneEdit) -> Committed {
    settle(doc);
    let (mut carries, mut moved) = (Vec::new(), Vec::new());
    for (node_id, before, after) in transform_edits(&edit) {
        if let Some((carry, set)) = follow_commit(doc, node_id, before, after) {
            carries.push(carry);
            moved.push(set);
        }
    }
    // Every set carried in one edit: the scene tells its listeners once,
    // not once per set (#560: an arrangement of a hundred models with their
    // supports held the window for twenty seconds).
    let mut parts = Vec::new();
    if !carries.is_empty() {
        crate::breadcrumbs::note(&format!(
            "supports: {} set(s) moved with their models",
            carries.len()
        ));
        let carry = SceneEdit::Compound(carries);
        if doc.apply_edit_unfollowed(carry.clone()).is_ok() {
            parts.push(carry);
        } else {
            moved.clear();
        }
    }
    let (mut regenerate, hand_edits_lost) = match take_commit(doc, &edit) {
        Some((removal, regenerate, hand_edits_lost)) => {
            parts.push(removal);
            (regenerate, hand_edits_lost)
        }
        None => (Vec::new(), false),
    };
    if let Some((imported, wanting)) = imported_commit(doc, &edit) {
        parts.extend(imported);
        for model in wanting {
            if !regenerate.contains(&model) {
                regenerate.push(model);
            }
        }
    }
    let edit = if parts.is_empty() {
        edit
    } else {
        parts.insert(0, edit);
        SceneEdit::Compound(parts)
    };
    Committed {
        edit,
        moved,
        regenerate,
        hand_edits_lost,
    }
}

/// The transform edits `edit` makes: itself, or each part of a compound
/// that only moves leaves (a lift of several models clear of the raft, an
/// arrangement of the plate).
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

/// For each committed move of a model with imported supports (#259): in the
/// plate plane, the edits that take their leaves along; otherwise the edit
/// that removes them, and the model, which waits for supports made here.
/// All applied. `None` when no model moved has imported supports. A set the
/// edit moves itself (selected with its model) is left to the edit.
fn imported_commit(doc: &SceneDocument, edit: &SceneEdit) -> Option<(Vec<SceneEdit>, Vec<NodeId>)> {
    let moves = transform_edits(edit);
    let moved_by_edit: Vec<NodeId> = moves.iter().map(|m| m.0).collect();
    let (mut follow, mut removed, mut wanting) = (Vec::new(), Vec::new(), Vec::new());
    for (node_id, before, after) in moves {
        if before == after {
            continue;
        }
        let found = doc
            .with_scene(|spatial| {
                let query = spatial.leaf(node_id)?;
                if is_any_support_mesh(query.mesh()) {
                    return None;
                }
                let parent = query.parent_world();
                let from = parent.compose(before);
                // The edit is already live: the sets stood under the model
                // where it came from.
                let sets: Vec<(NodeId, Transform3D)> = imported_of(spatial, node_id, Some(&from))
                    .into_iter()
                    .filter(|set| !moved_by_edit.contains(set))
                    .filter_map(|set| Some((set, spatial.leaf(set)?.local_transform())))
                    .collect();
                Some((plane_motion(&from, &parent.compose(after)), sets))
            })
            .flatten();
        let Some((motion, sets)) = found else {
            continue;
        };
        if sets.is_empty() {
            continue;
        }
        match motion {
            Some(motion) => {
                follow.extend(
                    sets.into_iter()
                        .map(|(set, start)| SceneEdit::SetLocalTransform {
                            node_id: set,
                            before: start,
                            after: Transform3D::Affine(motion.affine()).compose(&start),
                        }),
                );
            }
            None => {
                removed.extend(sets.into_iter().map(|(set, _)| set));
                wanting.push(node_id);
            }
        }
    }
    if follow.is_empty() && removed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !follow.is_empty() {
        // In one edit, told once (#560).
        let follow = SceneEdit::Compound(follow);
        doc.apply_edit_unfollowed(follow.clone()).ok()?;
        parts.push(follow);
    }
    if !removed.is_empty() {
        let mut graph = doc.with_scene(|spatial| spatial.graph().clone())?;
        let mut removals = Vec::new();
        // Each removal is worked out on the scene the ones before it leave.
        for set in &removed {
            if let Some(remove) = SceneEdit::remove_leaf_with_gc(&graph, *set) {
                remove.apply_to_scene(&mut graph).ok()?;
                removals.push(remove);
            }
        }
        if !removals.is_empty() {
            let removal = SceneEdit::Compound(removals);
            doc.apply_edit_unfollowed(removal.clone()).ok()?;
            parts.push(removal);
        }
        crate::breadcrumbs::note(&format!(
            "supports: {} imported set(s) removed to generate supports here",
            removed.len()
        ));
    }
    Some((parts, wanting))
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
                if is_any_support_mesh(query.mesh()) {
                    return None;
                }
                let parent = query.parent_world();
                let from = parent.compose(before);
                // A move in the plate plane takes them along instead.
                plane_motion(&from, &parent.compose(after))
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

/// For a committed move of the leaf `node_id` in the plate plane: the edit
/// that carries its set's leaf along, still to apply, and the set taken
/// along. `None` for any other move, and for a model with no set in force.
fn follow_commit(
    doc: &SceneDocument,
    node_id: NodeId,
    before: &Transform3D,
    after: &Transform3D,
) -> Option<(SceneEdit, MovedSet)> {
    if doc.support_sessions().borrow().is_empty() {
        return None;
    }
    let (motion, from) = doc
        .with_scene(|spatial| {
            let query = spatial.leaf(node_id)?;
            if is_any_support_mesh(query.mesh()) {
                return None;
            }
            let parent = query.parent_world();
            let from = parent.compose(before);
            Some((plane_motion(&from, &parent.compose(after))?, from))
        })
        .flatten()?;
    // The edit is already live: the set was in force where the model came
    // from.
    let (supports, session, _) = model_set(doc, node_id, Some(&from))?;
    let moved = match motion {
        PlaneMotion::Move(d) => session.moved_in_plane(0, d),
        PlaneMotion::Turn { angle, d } => {
            // The session gets the turned model as a run makes it.
            let (_, now) = doc
                .with_scene(|spatial| {
                    scene_model_leaves(spatial.graph(), Some(&[node_id]))
                        .ok()?
                        .pop()
                })
                .flatten()?;
            session.turned_in_plane(0, now, angle, d)
        }
    };
    // The supports take the model's move as one rigid piece: the same mesh,
    // the leaf's transform moved along (#560). The leaf is a root, so its
    // transform is where it stands in the world.
    let local = doc
        .with_scene(|spatial| spatial.leaf(supports).map(|q| q.local_transform()))
        .flatten()?;
    let carry = SceneEdit::SetLocalTransform {
        node_id: supports,
        before: local,
        after: Transform3D::Affine(motion.affine().compose(&affine(&local))),
    };
    Some((
        carry,
        MovedSet {
            leaf: supports,
            model: node_id,
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

    /// #153: a turn about the vertical axis, about any point, is followed;
    /// a tilt, a scale or a turn that also moves in Z is not.
    #[test]
    fn a_turn_about_the_vertical_axis_counts() {
        let affine = |a: AffineTransform3D| Transform3D::Affine(a);
        let at = AffineTransform3D::from_translation([20.0, 20.0, 8.0]);
        let along = AffineTransform3D::from_translation([1.0, 0.0, 0.0]).compose(&at);
        assert_eq!(
            plane_motion(&affine(at), &affine(along)),
            Some(PlaneMotion::Move([1.0, 0.0]))
        );
        // A quarter turn about the model's own corner, as the gizmo turns it.
        let turned = at.compose(&AffineTransform3D::from_rotation_z(
            std::f64::consts::FRAC_PI_2,
        ));
        let Some(PlaneMotion::Turn { angle, d }) = plane_motion(&affine(at), &affine(turned))
        else {
            panic!("a turn about Z");
        };
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        // The motion takes the corner to itself.
        let corner = PlaneMotion::Turn { angle, d }
            .affine()
            .apply_to_vertex(sc_mesh_core::Vertex::new([20.0, 20.0, 8.0]));
        assert!((corner.as_array()[0] - 20.0).abs() < 1e-5);
        assert!((corner.as_array()[1] - 20.0).abs() < 1e-5);
        // Half a turn of a model that was already tilted still counts.
        let tilted = at.compose(&AffineTransform3D::from_rotation_x(0.4));
        let half = AffineTransform3D::from_rotation_z(std::f64::consts::PI).compose(&tilted);
        assert!(matches!(
            plane_motion(&affine(tilted), &affine(half)),
            Some(PlaneMotion::Turn { .. })
        ));
        // Not followed: a tilt, a turn about its own tilted Z, a scale, a
        // turn that also lifts.
        assert_eq!(plane_motion(&affine(at), &affine(tilted)), None);
        let own_z = tilted.compose(&AffineTransform3D::from_rotation_z(0.3));
        assert_eq!(plane_motion(&affine(tilted), &affine(own_z)), None);
        let scaled = at.compose(&AffineTransform3D::from_uniform_scale(1.5));
        assert_eq!(plane_motion(&affine(at), &affine(scaled)), None);
        let lifted = AffineTransform3D::from_translation([0.0, 0.0, 2.0]).compose(&turned);
        assert_eq!(plane_motion(&affine(at), &affine(lifted)), None);
        assert_eq!(plane_motion(&affine(at), &affine(at)), None);
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

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a prepared model's copy takes with it (#113): its hollow and its
//! supports, so a pasted copy prints as the model does.
//!
//! A copy shows the model's very mesh (one shared mesh resource, #353), so
//! a hollow model's copy is that hollow; the hollow's record (the original,
//! the settings, the drains, [`Hollowed`]) is taken at copy time and given
//! to the scene the copy lands in when that scene does not have it (another
//! window), so the copy can be hollowed again, made solid or drained there.
//! Its plugs are objects of their own and stay out.
//!
//! The model's support set in force is taken along to the copy as a move of
//! the model in the plate plane takes it ([`moved_set`], #119), as Fill bed
//! does (#356): drawn where the copy stands, with every hand edit, and the
//! copy's own, in force, editable and saved as data. Supports of the model
//! no session stands behind (an out-of-date set) go along as they are. A
//! copy with supports keeps the model's height, so they still hold it; one
//! without lands on the plate, as a plain paste does.
//!
//! Everything is taken at copy time: a paste after the model was deleted,
//! in another window, or after the model was edited again, pastes the model
//! as it was copied. The copy and its supports land as one undo entry.
//!
//! The supports a pre-supported file came with (#259) go along as they are,
//! at the model's height, as they follow it in the plate plane.

use std::sync::Arc;

use sc_geom::df_supports::DfSupportSession;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{Aabb, IndexedMesh, Vertex};
use sc_scene::node::{LeafPayload, SceneMetadata};
use sc_scene::{NodeId, SceneError};
use sc_scene_edit::SceneFragment;

use crate::hollow_run::Hollowed;
use crate::imported_supports::imported_of;
use crate::scene_document::SceneDocument;
use crate::support_follow::moved_set;
use crate::support_persist::{in_force, record, sets_of_model};

/// A support leaf of the model as copied: its mesh, where it stands, its
/// name.
#[derive(Clone, Debug)]
struct Piece {
    mesh: Arc<IndexedMesh>,
    world: AffineTransform3D,
    meta: Option<SceneMetadata>,
}

/// The model's support set in force: the session that drew it, and the
/// leaf that shows it.
#[derive(Clone, Debug)]
struct Set {
    session: Arc<DfSupportSession>,
    piece: Piece,
}

/// What the copied model's preparation adds to a copy. Empty for a model
/// with no hollow made here and no supports: its copy is a plain paste.
#[derive(Clone, Debug, Default)]
pub(crate) struct Prep {
    /// Where the model stood, the translation of its world transform: a
    /// copy with supports keeps its height, and the supports move by the
    /// copy's move from here.
    at: [f64; 3],
    /// The hollow the model is, without its plugs.
    hollow: Option<Hollowed>,
    set: Option<Set>,
    /// Supports of the model that go along as they are.
    rigid: Vec<Piece>,
    /// The world box of the model and its supports, less `at`: what the
    /// placement keeps clear of the plate's objects.
    reach: Option<Aabb>,
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => *a,
    }
}

impl Prep {
    /// The preparation of the model `model` of `doc`'s scene, as it stands.
    pub(crate) fn of(doc: &SceneDocument, model: NodeId) -> Self {
        let sessions = doc.support_sessions();
        let hollows = doc.hollows();
        doc.with_scene(|spatial| {
            let graph = spatial.graph();
            let Some(query) = spatial.leaf(model) else {
                return Self::default();
            };
            let world = affine(&query.world_transform());
            let Some(mesh) = graph.leaf_mesh_arc(model) else {
                return Self::default();
            };
            let hollow = hollows
                .borrow()
                .iter()
                .find(|h| Arc::ptr_eq(&h.hollow, mesh))
                .map(|h| Hollowed {
                    plugs: Vec::new(),
                    ..h.clone()
                });
            let mut prep = Self {
                at: *world.translation(),
                hollow,
                ..Self::default()
            };
            let supports = sets_of_model(spatial, &sessions, model);
            // The supports its file came with go along as they are (#259).
            let imported = imported_of(spatial, model, None);
            if supports.is_empty() && imported.is_empty() {
                return prep;
            }
            let mut reach = query.transformed_aabb(query.world_transform());
            for leaf in supports.into_iter().chain(imported) {
                let (Some(support), Some(support_mesh)) =
                    (spatial.leaf(leaf), graph.leaf_mesh_arc(leaf))
                else {
                    continue;
                };
                let piece = Piece {
                    mesh: support_mesh.clone(),
                    world: affine(&support.world_transform()),
                    meta: graph.node(leaf).and_then(|n| n.meta.clone()),
                };
                if let (Some(b), Some(r)) = (
                    support.transformed_aabb(support.world_transform()),
                    reach.as_mut(),
                ) {
                    r.union(&b);
                }
                match in_force(spatial, &sessions, leaf) {
                    Some(drawn) if drawn.model == model && prep.set.is_none() => {
                        prep.set = Some(Set {
                            session: drawn.session,
                            piece,
                        });
                    }
                    _ => prep.rigid.push(piece),
                }
            }
            let at = prep.at.map(|c| c as f32);
            prep.reach = reach.filter(|r| !r.is_empty()).map(|r| {
                let (lo, hi) = (r.min().coords(), r.max().coords());
                Aabb::from_min_max(
                    Vertex::new([lo[0] - at[0], lo[1] - at[1], lo[2] - at[2]]),
                    Vertex::new([hi[0] - at[0], hi[1] - at[1], hi[2] - at[2]]),
                )
            });
            prep
        })
        .unwrap_or_default()
    }

    /// Whether the copy is a plain paste: no hollow made here, no supports.
    /// Where the model stood when it was taken, its world translation.
    pub(crate) fn at(&self) -> [f64; 3] {
        self.at
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.hollow.is_none() && !self.has_supports()
    }

    fn has_supports(&self) -> bool {
        self.set.is_some() || !self.rigid.is_empty()
    }

    /// The box the placement keeps clear, in the fragment's frame, when the
    /// supports go along; `None` when the model's own box does.
    pub(crate) fn reach(&self) -> Option<Aabb> {
        self.reach.filter(|_| self.has_supports())
    }

    /// The translation of a copy the placement put at `placed`: the
    /// model's height when the supports go along, so they still hold it.
    pub(crate) fn translation(&self, placed: [f64; 3]) -> [f64; 3] {
        if self.has_supports() {
            [placed[0], placed[1], self.at[2]]
        } else {
            placed
        }
    }

    /// Pastes `fragment` (the model) into `doc` at `translation`, with its
    /// hollow and its supports, as one undo entry. Returns the copy's leaf.
    pub(crate) fn paste(
        &self,
        doc: &SceneDocument,
        fragment: &SceneFragment,
        translation: [f64; 3],
    ) -> Result<Option<NodeId>, SceneError> {
        // The copy shows the hollow's very mesh: with the hollow's record
        // in the scene it is that hollow as soon as it lands.
        if let Some(hollowed) = &self.hollow {
            let hollows = doc.hollows();
            let mut hollows = hollows.borrow_mut();
            if !hollows
                .iter()
                .any(|h| Arc::ptr_eq(&h.hollow, &hollowed.hollow))
            {
                hollows.push(hollowed.clone());
            }
        }
        let shift = [0, 1, 2].map(|k| translation[k] - self.at[k]);
        let motion = AffineTransform3D::from_translation(shift);
        let moved = |p: &Piece| Transform3D::Affine(motion.compose(&p.world));
        let mut extra = Vec::with_capacity(1 + self.rigid.len());
        let mut drawn = None;
        if let Some(set) = &self.set {
            match moved_set(&set.session, [shift[0], shift[1]]) {
                // Drawn where the copy stands, in world space, as every set
                // in force is.
                Some((session, mesh)) => {
                    extra.push((
                        LeafPayload::Mesh(mesh),
                        Transform3D::Affine(AffineTransform3D::identity()),
                        set.piece.meta.clone(),
                    ));
                    drawn = Some(session);
                }
                None => extra.push((
                    LeafPayload::Mesh(set.piece.mesh.clone()),
                    moved(&set.piece),
                    set.piece.meta.clone(),
                )),
            }
        }
        for piece in &self.rigid {
            extra.push((
                LeafPayload::Mesh(piece.mesh.clone()),
                moved(piece),
                piece.meta.clone(),
            ));
        }
        let Some((copy, leaves)) = doc.paste_fragment_with(fragment, translation, extra)? else {
            return Ok(None);
        };
        if let (Some(session), Some(&leaf)) = (drawn, leaves.first()) {
            record(doc, &doc.support_sessions(), leaf, copy, session);
        }
        crate::breadcrumbs::note(&format!(
            "paste: prepared copy, hollow {}, set {}, {} support leaf(s) as they are",
            self.hollow.is_some(),
            self.set.is_some(),
            self.rigid.len()
        ));
        Ok(Some(copy))
    }
}

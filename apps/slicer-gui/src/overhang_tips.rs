/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The support tips the overhang map counts as holding the faces around them
//! (#484): every place a support of a set the scene shows touches the model,
//! where the set is drawn now, with the reach of the options the set was made
//! with (the Supports tab's for a set that does not record them). Each end of
//! a twig or a stick holds the face it touches, as a tip does.
//!
//! Gathered again only when a set, its drawing or where its leaf stands
//! changes: a frame otherwise hands the renderer the tips it already has.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use sc_geom::df_supports::DfSupportSession;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;

use crate::mesh_render::SupportTip;
use crate::scene_document::SceneDocument;
use crate::support_persist::{shown_drawing, support_leaves};

/// One support set as the scene shows it: its leaf, the session that drew
/// it, and where the leaf stands.
pub(crate) struct ShownSet {
    pub(crate) leaf: NodeId,
    pub(crate) session: Arc<DfSupportSession>,
    pub(crate) world: Transform3D,
}

/// What the tips were gathered from.
#[derive(Clone, Debug, PartialEq)]
struct TipsKey {
    sets: Vec<(NodeId, usize, Transform3D)>,
    fallback_reach_bits: u64,
}

/// The tips gathered last, and from what.
#[derive(Default)]
pub(crate) struct TipCache {
    last: RefCell<Option<(TipsKey, Rc<[SupportTip]>)>>,
}

impl TipCache {
    /// The tips of the scene of `document`, sets that record no options
    /// reaching `fallback_reach_mm`.
    pub(crate) fn tips(
        &self,
        document: &SceneDocument,
        fallback_reach_mm: f64,
    ) -> Rc<[SupportTip]> {
        let sets = shown_sets(document);
        let key = TipsKey {
            sets: sets
                .iter()
                .map(|s| (s.leaf, Arc::as_ptr(&s.session) as usize, s.world))
                .collect(),
            fallback_reach_bits: fallback_reach_mm.to_bits(),
        };
        let mut last = self.last.borrow_mut();
        if let Some((kept, tips)) = last.as_ref()
            && *kept == key
        {
            return tips.clone();
        }
        let tips: Rc<[SupportTip]> = tips_of(&sets, fallback_reach_mm).into();
        *last = Some((key, tips.clone()));
        tips
    }
}

/// Every support set the scene of `document` shows, by the session that
/// drew it last: supports no session drew (a mesh saved as such) have no
/// tips to count.
pub(crate) fn shown_sets(document: &SceneDocument) -> Vec<ShownSet> {
    let sessions = document.support_sessions();
    document
        .with_scene(|spatial| {
            support_leaves(spatial)
                .filter_map(|(leaf, _)| {
                    let drawn = shown_drawing(&sessions, spatial, leaf)?;
                    // The session stands in the world where the leaf stood
                    // when it was recorded: only the leaf's move since then
                    // (a live move, or Undo past a carry) is still to apply.
                    let now = spatial.leaf(leaf)?.world_transform();
                    let world = now.compose(&drawn.leaf_world.inverse().ok()?);
                    Some(ShownSet {
                        leaf,
                        session: drawn.session,
                        world,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where the supports of `sets` touch the model, each where its leaf draws
/// it and with its set's reach.
fn tips_of(sets: &[ShownSet], fallback_reach_mm: f64) -> Vec<SupportTip> {
    sets.iter()
        .flat_map(|set| {
            let reach_mm = set
                .session
                .options()
                .map_or(fallback_reach_mm, |o| o.tip_reach_mm()) as f32;
            set.session
                .pieces()
                .into_iter()
                .flat_map(|piece| piece.contacts)
                .map(move |contact| {
                    let [x, y, z] = contact.surface;
                    SupportTip {
                        pos: set
                            .world
                            .apply_to_vertex(Vertex::new([x as f32, y as f32, z as f32]))
                            .coords(),
                        reach_mm,
                    }
                })
        })
        .collect()
}

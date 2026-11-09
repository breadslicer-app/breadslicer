/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The supports of the scene as lines, for the view from under the plate
//! (#493): the path of every piece of every set the scene shows (trunks,
//! branches, leaves, twigs, sticks, stumps and braces) and the ball at the
//! end of every tip, where the set is drawn now.
//!
//! Gathered again only when a set, its drawing or where its leaf stands
//! changes, as the overhang map's tips are.

use std::cell::RefCell;
use std::sync::Arc;

use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;

use crate::mesh_render::SupportLines;
use crate::overhang_tips::{ShownSet, shown_sets};
use crate::scene_document::SceneDocument;

/// Which sets lines were gathered from: each set's leaf, the session that
/// drew it and where the leaf stands.
type SetsKey = Vec<(NodeId, usize, Transform3D)>;

/// The lines gathered last, and from which sets.
#[derive(Default)]
pub(crate) struct LinesCache {
    last: RefCell<Option<(SetsKey, Arc<SupportLines>)>>,
}

impl LinesCache {
    /// The lines of the supports the scene of `document` shows.
    pub(crate) fn lines(&self, document: &SceneDocument) -> Arc<SupportLines> {
        let sets = shown_sets(document);
        let key: SetsKey = sets
            .iter()
            .map(|s| (s.leaf, Arc::as_ptr(&s.session) as usize, s.world))
            .collect();
        let mut last = self.last.borrow_mut();
        if let Some((kept, lines)) = last.as_ref()
            && *kept == key
        {
            return lines.clone();
        }
        let lines = Arc::new(lines_of(&sets));
        *last = Some((key, lines.clone()));
        lines
    }
}

/// The lines of `sets`, each where its leaf draws it.
fn lines_of(sets: &[ShownSet]) -> SupportLines {
    let mut paths: Vec<Vec<[f32; 3]>> = Vec::new();
    let mut balls: Vec<([f32; 3], f32)> = Vec::new();
    for set in sets {
        let place = |[x, y, z]: [f64; 3]| {
            set.world
                .apply_to_vertex(Vertex::new([x as f32, y as f32, z as f32]))
                .coords()
        };
        for piece in set.session.pieces() {
            let line = piece.line.iter().map(|joint| place(joint.at));
            // A piece with one tip runs from the model down its tip to its
            // shaft; a bar between two parts of the model draws its tips and
            // its shaft apart.
            match piece.contacts.as_slice() {
                [contact] => paths.push(
                    [place(contact.surface), place(contact.socket)]
                        .into_iter()
                        .chain(line)
                        .collect(),
                ),
                contacts => {
                    paths.extend(
                        contacts
                            .iter()
                            .map(|c| vec![place(c.surface), place(c.socket)]),
                    );
                    paths.push(line.collect());
                }
            }
            for contact in piece.contacts {
                // The round end of the tip, centred on the contact; a flat
                // tip shows its contact as large.
                let centre = contact.ball.unwrap_or(contact.surface);
                balls.push((place(centre), contact.tip_radius_mm as f32));
            }
        }
        for (_, ends) in set.session.brace_paths() {
            paths.push(ends.into_iter().map(place).collect());
        }
    }
    SupportLines::new(&paths, &balls)
}

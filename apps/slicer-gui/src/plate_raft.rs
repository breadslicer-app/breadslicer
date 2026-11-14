/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! One raft for all (#556): the support sets standing on one raft draw no
//! raft of their own; each plate shows one raft of lines under the roots of
//! all of them, a mesh leaf of its own ([`sc_geom::df_supports::PLATE_RAFT_NAME`]).
//!
//! The raft follows the sets: once the scene stands still after a change
//! (Generate, a move, an Arrange, a raft picked), it is drawn again in the
//! background and joins that change's undo step, so Undo takes the raft back
//! with the sets it was drawn under. A raft no change of the history can
//! carry (a project just opened) is taken as it stands.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::sync::{Arc, Weak};

use sc_geom::df_supports::{DfSupportSession, is_plate_raft, plate_raft};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;
use sc_scene::node::{LeafAttachment, LeafPayload, NodeKind};
use sc_scene_edit::SceneEdit;

use crate::build_volume::BuildVolume;
use crate::scene_document::{SceneDocument, SceneRevision};

/// What a plate's raft is drawn from: its sets' roots and their raft.
type Key = u64;

/// A plate's raft as the scene stands: what it is drawn from, and the raft
/// leaves it shows now.
struct PlateWork {
    key: Key,
    sets: Vec<(Arc<DfSupportSession>, AffineTransform3D)>,
    leaves: Vec<NodeId>,
}

/// A raft drawn in the background, for the scene at `revision`.
struct Job {
    revision: SceneRevision,
    done: mpsc::Receiver<Vec<(PlateWork, Option<Arc<IndexedMesh>>)>>,
}

#[derive(Default)]
pub(crate) struct PlateRafts {
    /// What each raft mesh shown was drawn from, by the mesh: Undo puts an
    /// older mesh back, and it is still known.
    keys: RefCell<HashMap<usize, (Weak<IndexedMesh>, Key)>>,
    /// The scene last found with its rafts as they should be.
    checked: Cell<Option<SceneRevision>>,
    job: RefCell<Option<Job>>,
}

impl PlateRafts {
    /// Looks at the scene once it has changed: draws the rafts that no
    /// longer match their sets, or lands the ones drawn.
    pub(crate) fn tick(&self, document: &SceneDocument, volume: &BuildVolume) {
        let revision = document.scene_revision();
        let landed = {
            let job = self.job.borrow();
            match job.as_ref() {
                Some(job) if job.revision != revision => None,
                Some(job) => match job.done.try_recv() {
                    Ok(drawn) => Some(Some(drawn)),
                    Err(mpsc::TryRecvError::Empty) => return,
                    Err(mpsc::TryRecvError::Disconnected) => Some(None),
                },
                None => None,
            }
        };
        match landed {
            Some(drawn) => {
                self.job.borrow_mut().take();
                if let Some(drawn) = drawn {
                    self.land(document, revision, drawn);
                }
                return;
            }
            None => {
                // A job for a scene moved on from is dropped.
                self.job.borrow_mut().take();
            }
        }
        if self.checked.get() == Some(revision) {
            return;
        }
        let work = self.work(document, volume);
        if work.is_empty() {
            self.checked.set(Some(revision));
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("plate-raft".into())
            .spawn(move || {
                let drawn = work
                    .into_iter()
                    .map(|w| {
                        let sets: Vec<(&DfSupportSession, AffineTransform3D)> =
                            w.sets.iter().map(|(s, m)| (s.as_ref(), *m)).collect();
                        let mesh = plate_raft(&sets).map(Arc::new);
                        (w, mesh)
                    })
                    .collect();
                let _ = tx.send(drawn);
            })
            .ok();
        *self.job.borrow_mut() = Some(Job { revision, done: rx });
    }

    /// Whether a raft is being drawn.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        self.job.borrow().is_some()
    }

    /// The plates whose raft does not match their sets.
    fn work(&self, document: &SceneDocument, volume: &BuildVolume) -> Vec<PlateWork> {
        let plates = document.plates();
        let sets = crate::overhang_tips::shown_sets(document);
        let keys = self.keys.borrow();
        document
            .with_scene(|spatial| {
                let mut by_plate: HashMap<u32, PlateWork> = HashMap::new();
                let entry = |by_plate: &mut HashMap<u32, PlateWork>, plate: u32| {
                    by_plate.entry(plate).or_insert_with(|| PlateWork {
                        key: 0,
                        sets: Vec::new(),
                        leaves: Vec::new(),
                    });
                };
                for set in sets.iter().filter(|s| s.session.stands_on_one_raft()) {
                    let Some(plate) =
                        crate::plate_layout::plate_of_leaf(spatial, volume, plates, set.leaf)
                    else {
                        continue;
                    };
                    let motion = match set.world {
                        Transform3D::Identity => AffineTransform3D::identity(),
                        Transform3D::Affine(a) => a,
                    };
                    entry(&mut by_plate, plate);
                    if let Some(w) = by_plate.get_mut(&plate) {
                        w.sets.push((set.session.clone(), motion));
                    }
                }
                for leaf in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
                    let Some(query) = spatial.leaf(leaf) else {
                        continue;
                    };
                    if !is_plate_raft(query.mesh()) {
                        continue;
                    }
                    let Some(plate) =
                        crate::plate_layout::plate_of_leaf(spatial, volume, plates, leaf)
                    else {
                        continue;
                    };
                    entry(&mut by_plate, plate);
                    if let Some(w) = by_plate.get_mut(&plate) {
                        w.leaves.push(leaf);
                    }
                }
                by_plate
                    .into_values()
                    .filter_map(|mut w| {
                        w.key = key_of(&w.sets);
                        let shown: Vec<Option<Key>> = w
                            .leaves
                            .iter()
                            .map(|&leaf| {
                                let mesh = spatial.graph().leaf_mesh_arc(leaf)?;
                                keys.get(&(Arc::as_ptr(mesh) as usize))
                                    .filter(|(weak, _)| weak.upgrade().is_some())
                                    .map(|(_, key)| *key)
                            })
                            .collect();
                        let fine = if w.sets.is_empty() {
                            w.leaves.is_empty()
                        } else {
                            shown.len() == 1 && shown[0] == Some(w.key)
                        };
                        (!fine).then_some(w)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Puts the rafts drawn in the scene, as part of the change that called
    /// for them.
    fn land(
        &self,
        document: &SceneDocument,
        revision: SceneRevision,
        drawn: Vec<(PlateWork, Option<Arc<IndexedMesh>>)>,
    ) {
        let edits = document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                let mut edits = Vec::new();
                let mut roots = scene.roots().len();
                let mut new_leaves = Vec::new();
                for (w, mesh) in &drawn {
                    let mut leaves = w.leaves.iter();
                    if let Some(mesh) = mesh {
                        match leaves.next() {
                            Some(&leaf) => {
                                if let Some(edit) =
                                    crate::scene_document::leaf_mesh_swap(scene, leaf, mesh.clone())
                                {
                                    edits.push(edit);
                                }
                            }
                            None => {
                                new_leaves.push((roots, mesh.clone()));
                                roots += 1;
                            }
                        }
                    }
                    // Extra rafts on the plate, or one with no set left on it.
                    for &leaf in leaves {
                        if let (Some(node), Some(attachment)) =
                            (scene.node(leaf), scene.attachment_of(leaf))
                            && let NodeKind::Leaf(payload) = &node.kind
                        {
                            edits.push(SceneEdit::RemoveLeaf {
                                node_id: leaf,
                                attachment,
                                payload: payload.clone(),
                                local_transform: node.local_transform,
                                meta: node.meta.clone(),
                            });
                        }
                    }
                }
                (edits, new_leaves)
            })
            .unwrap_or_default();
        let (mut edits, new_leaves) = edits;
        let ids = document.allocate_leaf_slots(new_leaves.len());
        for ((position, mesh), (node_id, _)) in new_leaves.into_iter().zip(ids) {
            edits.push(SceneEdit::AddLeaf {
                node_id,
                attachment: LeafAttachment::Root { position },
                payload: LeafPayload::Mesh(mesh),
                local_transform: Transform3D::Identity,
                meta: None,
            });
        }
        let joined = if edits.is_empty() {
            Ok(false)
        } else {
            document.complete_last_edit(revision, SceneEdit::Compound(edits), &[])
        };
        let mut keys = self.keys.borrow_mut();
        keys.retain(|_, (weak, _)| weak.strong_count() > 0);
        match joined {
            Ok(true) => {
                for (w, mesh) in &drawn {
                    if let Some(mesh) = mesh {
                        keys.insert(Arc::as_ptr(mesh) as usize, (Arc::downgrade(mesh), w.key));
                    }
                }
                crate::breadcrumbs::note(&format!(
                    "supports: one raft for all drawn on {} plate(s)",
                    drawn.len()
                ));
            }
            _ => {
                // Nothing of the history can carry it (a project just
                // opened, something to redo): the rafts shown stand.
                document.with_scene(|spatial| {
                    for (w, _) in &drawn {
                        if let [leaf] = w.leaves[..]
                            && let Some(mesh) = spatial.graph().leaf_mesh_arc(leaf)
                        {
                            keys.insert(Arc::as_ptr(mesh) as usize, (Arc::downgrade(mesh), w.key));
                        }
                    }
                });
                self.checked.set(Some(revision));
            }
        }
    }
}

/// What a plate's raft is drawn from: where its sets' roots stand, how wide,
/// and the raft they stand on.
fn key_of(sets: &[(Arc<DfSupportSession>, AffineTransform3D)]) -> Key {
    let mut circles: Vec<[i64; 3]> = sets
        .iter()
        .flat_map(|(session, motion)| session.raft_roots(motion))
        .map(|c| {
            [
                (c.x * 1e4).round() as i64,
                (c.y * 1e4).round() as i64,
                (c.r * 1e4).round() as i64,
            ]
        })
        .collect();
    circles.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    circles.hash(&mut hasher);
    for (session, _) in sets.iter().take(1) {
        if let Some(o) = session.options() {
            o.raft_thickness_mm.to_bits().hash(&mut hasher);
            o.raft_wall.hash(&mut hasher);
        }
        session.raft_top_mm().to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

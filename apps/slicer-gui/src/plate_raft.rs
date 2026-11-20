/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! One raft for all (#556): the support sets standing on one raft draw no
//! raft of their own; each plate shows one raft of lines under the roots of
//! all of them, a mesh leaf of its own ([`sc_geom::df_supports::PLATE_RAFT_NAME`]).
//!
//! A group of models (#475) stands on a raft of its own, apart from the
//! plate's: the sets of one group, and those of no group, are each drawn
//! under one raft.
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

/// One raft to draw: what it is drawn from, and the raft leaf it takes the
/// place of, if any.
struct RaftWork {
    key: Key,
    sets: Vec<(Arc<DfSupportSession>, AffineTransform3D)>,
    leaf: Option<NodeId>,
}

/// A plate's rafts as the scene stands: those to draw, and the raft leaves
/// no set stands on any more.
struct PlateWork {
    rafts: Vec<RaftWork>,
    extra: Vec<NodeId>,
}

/// A raft drawn in the background, for the scene at `revision`.
struct Job {
    revision: SceneRevision,
    done: mpsc::Receiver<Vec<(PlateWork, Vec<Option<Arc<IndexedMesh>>>)>>,
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
                        let meshes = w
                            .rafts
                            .iter()
                            .map(|r| {
                                let sets: Vec<(&DfSupportSession, AffineTransform3D)> =
                                    r.sets.iter().map(|(s, m)| (s.as_ref(), *m)).collect();
                                plate_raft(&sets).map(Arc::new)
                            })
                            .collect();
                        (w, meshes)
                    })
                    .collect();
                let _ = tx.send(drawn);
            })
            .ok();
        *self.job.borrow_mut() = Some(Job { revision, done: rx });
    }

    /// Whether every plate's raft matches its sets as the scene stands: none
    /// is being drawn, and the scene was looked at since its last change.
    pub(crate) fn settled(&self, document: &SceneDocument) -> bool {
        self.job.borrow().is_none() && self.checked.get() == Some(document.scene_revision())
    }

    /// Whether a raft still has to be drawn or land: one is being drawn, or
    /// a plate's raft does not match its sets. A slice made now would not
    /// see the raft the scene is about to have.
    pub(crate) fn pending(&self, document: &SceneDocument, volume: &BuildVolume) -> bool {
        self.job.borrow().is_some() || !self.work(document, volume).is_empty()
    }

    /// Whether a raft is being drawn.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        self.job.borrow().is_some()
    }

    /// The plates whose rafts do not match their sets.
    fn work(&self, document: &SceneDocument, volume: &BuildVolume) -> Vec<PlateWork> {
        let plates = document.plates();
        let groups = document.groups();
        let sets = crate::overhang_tips::shown_sets(document);
        let keys = self.keys.borrow();
        document
            .with_scene(|spatial| {
                // Each plate's rafts, by the group their sets' models are in
                // (`None`: no group), in a stable order.
                type Rafts = Vec<(
                    Option<usize>,
                    Vec<(Arc<DfSupportSession>, AffineTransform3D)>,
                )>;
                let mut by_plate: HashMap<u32, (Rafts, Vec<NodeId>)> = HashMap::new();
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
                    let group = groups.iter().position(|g| g.contains(&set.model));
                    let (rafts, _) = by_plate.entry(plate).or_default();
                    match rafts.iter_mut().find(|(g, _)| *g == group) {
                        Some((_, sets)) => sets.push((set.session.clone(), motion)),
                        None => rafts.push((group, vec![(set.session.clone(), motion)])),
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
                    by_plate.entry(plate).or_default().1.push(leaf);
                }
                let mut work: Vec<(u32, PlateWork)> = by_plate
                    .into_iter()
                    .filter_map(|(plate, (mut rafts, leaves))| {
                        rafts.sort_by_key(|(g, _)| g.map_or(0, |i| i + 1));
                        let shown = |leaf: NodeId| -> Option<Key> {
                            let mesh = spatial.graph().leaf_mesh_arc(leaf)?;
                            keys.get(&(Arc::as_ptr(mesh) as usize))
                                .filter(|(weak, _)| weak.upgrade().is_some())
                                .map(|(_, key)| *key)
                        };
                        let mut free: Vec<NodeId> = leaves.clone();
                        let mut to_draw: Vec<RaftWork> = Vec::new();
                        for (_, sets) in rafts {
                            let key = key_of(&sets);
                            // A raft shown drawn from these sets stands.
                            if let Some(i) = free.iter().position(|&l| shown(l) == Some(key)) {
                                free.remove(i);
                                continue;
                            }
                            to_draw.push(RaftWork {
                                key,
                                sets,
                                leaf: None,
                            });
                        }
                        // A raft to draw takes the place of one shown that
                        // stands for nothing now; the rest go.
                        for raft in &mut to_draw {
                            if !free.is_empty() {
                                raft.leaf = Some(free.remove(0));
                            }
                        }
                        (!to_draw.is_empty() || !free.is_empty()).then_some((
                            plate,
                            PlateWork {
                                rafts: to_draw,
                                extra: free,
                            },
                        ))
                    })
                    .collect();
                work.sort_by_key(|(plate, _)| *plate);
                work.into_iter().map(|(_, w)| w).collect()
            })
            .unwrap_or_default()
    }

    /// Puts the rafts drawn in the scene, as part of the change that called
    /// for them.
    fn land(
        &self,
        document: &SceneDocument,
        revision: SceneRevision,
        drawn: Vec<(PlateWork, Vec<Option<Arc<IndexedMesh>>>)>,
    ) {
        let edits = document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                let mut edits = Vec::new();
                let mut roots = scene.roots().len();
                let mut new_leaves = Vec::new();
                let remove = |edits: &mut Vec<SceneEdit>, leaf: NodeId| {
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
                };
                for (w, meshes) in &drawn {
                    for (raft, mesh) in w.rafts.iter().zip(meshes) {
                        match (mesh, raft.leaf) {
                            (Some(mesh), Some(leaf)) => {
                                if let Some(edit) =
                                    crate::scene_document::leaf_mesh_swap(scene, leaf, mesh.clone())
                                {
                                    edits.push(edit);
                                }
                            }
                            (Some(mesh), None) => {
                                new_leaves.push((roots, mesh.clone()));
                                roots += 1;
                            }
                            // No raft drawn: the one it would replace goes.
                            (None, Some(leaf)) => remove(&mut edits, leaf),
                            (None, None) => {}
                        }
                    }
                    // Rafts with no set left on them.
                    for &leaf in &w.extra {
                        remove(&mut edits, leaf);
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
                let mut count = 0;
                for (w, meshes) in &drawn {
                    for (raft, mesh) in w.rafts.iter().zip(meshes) {
                        if let Some(mesh) = mesh {
                            keys.insert(
                                Arc::as_ptr(mesh) as usize,
                                (Arc::downgrade(mesh), raft.key),
                            );
                            count += 1;
                        }
                    }
                }
                crate::breadcrumbs::note(&format!("supports: {count} shared raft(s) drawn"));
            }
            _ => {
                // Nothing of the history can carry it (a project just
                // opened, something to redo): the rafts shown stand.
                document.with_scene(|spatial| {
                    for (w, _) in &drawn {
                        for raft in &w.rafts {
                            if let Some(leaf) = raft.leaf
                                && let Some(mesh) = spatial.graph().leaf_mesh_arc(leaf)
                            {
                                keys.insert(
                                    Arc::as_ptr(mesh) as usize,
                                    (Arc::downgrade(mesh), raft.key),
                                );
                            }
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

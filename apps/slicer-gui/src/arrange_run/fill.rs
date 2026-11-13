/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Fill bed (#356): the room left on the plate filled with copies of the
//! selected models.
//!
//! Everything on the plate but the selected models stays where it is, and
//! the copies keep clear of it ([`sc_arrange::fill`]): with one model
//! selected, copies of it; with several, copies of each in turn, so they
//! get as many each, give or take one. The selected models stay too,
//! unless moving them lets more copies in (one in the plate's middle splits
//! it): then they move with the copies, their supports along, in the same
//! step. The Arrange card's settings hold: the gap, the edge margin, turns
//! about the vertical axis and Shadow only.
//!
//! A copy shows the very mesh of its model (the same `Arc`, hollowed or
//! not), and its support set in force comes along as if the model had
//! been pasted and moved there: its session taken along by the copy's move
//! in the plate plane ([`DfSupportSession::moved_in_plane`],
//! [`DfSupportSession::turned_in_plane`], #119, #153) and drawn for the
//! copy, which then has a set of its own, editable and saved as data
//! (#197). Supports no session stands behind, and those a pre-supported
//! file came with (#259), go along as they are.
//!
//! The search and the copies' supports run on a worker, with the count so
//! far on the card and Cancel. The copies land as one undoable edit; a
//! result worked out for a scene that has changed since is dropped.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sc_arrange::{Body, Filled, Placement, Plate, Settings};
use sc_geom::df_supports::{DfSupportSession, is_imported_supports, model_from_world_mesh};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::node::{LeafAttachment, LeafPayload, SceneMetadata};
use sc_scene::{MeshResourceId, NodeId, PaintKind, SurfaceTags};
use sc_scene_edit::SceneEdit;

use super::{ArrangeRun, affine, gather};
use crate::error_dialog::ErrorTexts;
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::scene_document::{EditNotApplied, SceneDocument, SceneRevision, leaf_mesh_swap};
use crate::support_follow::moved_set;
use crate::support_persist::{in_force, record};

/// A mesh that goes along with its model as it is: where it stands and
/// what its leaf is called.
#[derive(Clone, Debug)]
struct Piece {
    /// The leaf that shows it.
    leaf: NodeId,
    mesh: Arc<IndexedMesh>,
    world: AffineTransform3D,
    meta: Option<SceneMetadata>,
}

/// A selected model, what its copies are made from.
#[derive(Clone, Debug)]
struct Source {
    /// Its body among the plate's.
    body: usize,
    model: Piece,
    /// Its supports that go along as they are.
    rigid: Vec<Piece>,
    /// Its support set in force: the session, and the mesh it drew.
    set: Option<(Arc<DfSupportSession>, Piece)>,
}

/// What a fill made on the worker: the copies, and per copy, then per
/// moved model, the session and mesh of its support set where it goes, when
/// its model has one in force and it could be drawn.
struct Made {
    filled: Filled,
    sets: Vec<Option<CopiedSet>>,
    /// How long drawing the copies' sets took.
    sets_time: Duration,
}

/// A copy's support set: its session and the mesh it drew.
type CopiedSet = (Arc<DfSupportSession>, Arc<IndexedMesh>);

/// How far the worker is, for the card.
#[derive(Debug, Default)]
struct Progress {
    placed: AtomicUsize,
    sets_done: AtomicUsize,
    sets_total: AtomicUsize,
}

/// A fill in flight.
struct Pending {
    revision: SceneRevision,
    sources: Vec<Source>,
    started: Instant,
}

type FillSlot = Arc<Mutex<Option<Result<Option<Made>, String>>>>;

/// The fill's part of the Arrange driver.
#[derive(Default)]
pub(super) struct Fill {
    slot: FillSlot,
    pending: RefCell<Option<Pending>>,
    timer: slint::Timer,
}

impl Fill {
    /// Whether a fill is in flight: the worker's call lands on it.
    pub(super) fn is_running(&self) -> bool {
        self.pending.borrow().is_some()
    }
}

/// The card's line while a fill runs.
fn progress_text(progress: &Progress) -> String {
    let total = progress.sets_total.load(Ordering::Relaxed);
    if total > 0 {
        let done = progress.sets_done.load(Ordering::Relaxed);
        return ctr!("Inspector", "Copying the supports: {} of {}…", done, total).to_string();
    }
    let placed = progress.placed.load(Ordering::Relaxed);
    ctr!(
        "Inspector",
        "Filling the plate: {n} copy so far…",
        "Filling the plate: {n} copies so far…",
        placed
    )
    .to_string()
}

impl ArrangeRun {
    /// Fills the room left on the plate with copies of the selected models.
    pub(crate) fn start_fill(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading()
            || ui.get_slicing_in_progress()
            || ui.get_repairing_in_progress()
            || ui.get_arranging()
            || !self.ctrl.allows_layout_tools()
        {
            return;
        }
        // With several plates, the active one alone (#548).
        let (on, plate) = super::active_plate_job(&self.document, &self.ctrl.build_volume());
        let Some((bodies, sources)) = gather_fill(&self.document, on) else {
            ui.set_arrange_last_run(ctr!("Inspector", "Select the models to copy."));
            return;
        };
        let options = self.options.get();
        let settings = options.settings();
        let limit = (options.max_copies > 0).then_some(options.max_copies as usize);
        *self.fill.pending.borrow_mut() = Some(Pending {
            revision: self.document.scene_revision(),
            sources: sources.clone(),
            started: Instant::now(),
        });
        self.cancel.store(false, Ordering::Relaxed);
        let progress = Arc::new(Progress::default());
        let shown = progress.clone();
        let ui_weak = self.ui.clone();
        self.fill.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_arrange_fill_progress(progress_text(&shown).into());
                }
            },
        );
        ui.set_arrange_fill_progress(progress_text(&progress).into());
        self.stop_ahead();
        ui.set_arranging(true);
        ui.set_arrange_filling(true);
        crate::breadcrumbs::note(&format!(
            "fill: {} source(s) among {} object(s), limit {limit:?}",
            sources.len(),
            bodies.len()
        ));
        let slot = self.fill.slot.clone();
        let cancel = self.cancel.clone();
        let ui_weak = self.ui.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                make(
                    &bodies, &sources, &plate, &settings, limit, &progress, &cancel,
                )
            }))
            .map_err(|payload| panic_message(&payload));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            let _ = ui_weak.upgrade_in_event_loop(|ui| ui.invoke_arrange_finished());
        });
    }

    /// Takes the worker's result on the UI thread and adds the copies, when
    /// the scene is still the one they were worked out for.
    pub(super) fn finish_fill(&self) {
        self.fill.timer.stop();
        let taken = self
            .fill
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let pending = self.fill.pending.borrow_mut().take();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_arranging(false);
        ui.set_arrange_filling(false);
        let (Some(result), Some(pending)) = (taken, pending) else {
            return;
        };
        let made = match result {
            Ok(Some(made)) => made,
            // Cancelled.
            Ok(None) => return,
            Err(panic) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Fill bed failed"),
                    context: None,
                    message: ctr!("ArrangeDialog", "The fill stopped unexpectedly."),
                    details: panic.as_str().into(),
                });
                return;
            }
        };
        let seconds = format!("{:.1}", pending.started.elapsed().as_secs_f64());
        let text = match land(&self.document, &pending, made) {
            Landed::Stale => ctr!(
                "Inspector",
                "The scene changed while filling; nothing was added."
            ),
            Landed::NoneFit => ctr!("Inspector", "No copy fits on the plate."),
            Landed::Added(count) => ctr!(
                "Inspector",
                "Added {n} copy in {} s.",
                "Added {n} copies in {} s.",
                count,
                seconds
            ),
            Landed::Rejected(error) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Fill bed failed"),
                    context: None,
                    message: ctr!(
                        "ArrangeDialog",
                        "The scene rejected the copies; nothing was added."
                    ),
                    details: error.as_str().into(),
                });
                return;
            }
        };
        ui.set_arrange_last_run(text);
    }
}

/// The edits that move the models `moved` to let more copies in: each
/// model and its supports that go along as they are, and its set in force
/// redrawn where it goes (`sets`, one per move, as a move in the plate
/// plane takes it, #119, #153), which goes to `drawn`. A set that could not
/// be redrawn goes along as it is. `None` when a leaf is gone.
fn moved_models(
    document: &SceneDocument,
    sources: &[Source],
    moved: &[sc_arrange::FillCopy],
    sets: Vec<Option<CopiedSet>>,
    drawn: &mut Vec<(NodeId, NodeId, Arc<DfSupportSession>)>,
) -> Option<Vec<SceneEdit>> {
    document
        .with_scene(|spatial| {
            let mut edits = Vec::new();
            let shift = |leaf: NodeId, motion: &Transform3D| {
                let query = spatial.leaf(leaf)?;
                let up = query.parent_world().inverse().ok()?;
                Some(SceneEdit::SetLocalTransform {
                    node_id: leaf,
                    before: query.local_transform(),
                    after: up.compose(&motion.compose(&query.world_transform())),
                })
            };
            for (m, set) in moved.iter().zip(sets) {
                let source = sources.iter().find(|s| s.body == m.source)?;
                let motion = Transform3D::Affine(m.placement.motion());
                edits.push(shift(source.model.leaf, &motion)?);
                for piece in &source.rigid {
                    edits.push(shift(piece.leaf, &motion)?);
                }
                if let Some((_, piece)) = &source.set {
                    match set {
                        Some((session, mesh)) => {
                            edits.push(leaf_mesh_swap(spatial.graph(), piece.leaf, mesh)?);
                            drawn.push((piece.leaf, source.model.leaf, session));
                        }
                        None => edits.push(shift(piece.leaf, &motion)?),
                    }
                }
            }
            Some(edits)
        })
        .flatten()
}

/// What became of a fill's result.
#[derive(Debug, PartialEq)]
enum Landed {
    /// The scene changed since the fill started: nothing was added.
    Stale,
    /// No copy fit.
    NoneFit,
    /// This many copies were added, as one undo step.
    Added(usize),
    /// The scene refused the copies: nothing was added.
    Rejected(String),
}

/// Adds the copies of `made` to the scene of `document`, when it is still
/// the one `pending` was worked out for.
fn land(document: &SceneDocument, pending: &Pending, made: Made) -> Landed {
    if document.scene_revision() != pending.revision {
        return Landed::Stale;
    }
    crate::breadcrumbs::note(&format!(
        "fill: {} copies, footprints {:?}, search {:?}, supports {:?}",
        made.filled.copies.len(),
        made.filled.timings.footprints,
        made.filled.timings.search,
        made.sets_time
    ));
    let count = made.filled.copies.len();
    if count == 0 {
        return Landed::NoneFit;
    }
    match add_copies(document, &pending.sources, made) {
        Ok(()) => Landed::Added(count),
        Err(error) => Landed::Rejected(error),
    }
}

/// The plate's objects, everything staying where it is, and the selected
/// models as the sources of the copies. `None` when no model is selected.
fn gather_fill(
    document: &SceneDocument,
    on: Option<(crate::build_volume::BuildVolume, crate::plates::Plates, u32)>,
) -> Option<(Vec<Body>, Vec<Source>)> {
    let gathered = gather(document, false, on)?;
    let selected = document.selection_node_ids();
    let sessions = document.support_sessions();
    let sources: Vec<Source> = document
        .with_scene(|spatial| {
            let graph = spatial.graph();
            let piece = |leaf: NodeId| {
                let query = spatial.leaf(leaf)?;
                Some(Piece {
                    leaf,
                    mesh: graph.leaf_mesh_arc(leaf)?.clone(),
                    world: affine(&query.world_transform()),
                    meta: graph.node(leaf)?.meta.clone(),
                })
            };
            gathered
                .leaves
                .iter()
                .zip(&gathered.follows)
                .enumerate()
                .filter_map(|(body, (leaves, follows))| {
                    // A model's body moves the model first.
                    let &model = leaves.first()?;
                    if !selected.contains(&model) {
                        return None;
                    }
                    let set = follows.iter().find_map(|&leaf| {
                        let drawn = in_force(spatial, &sessions, leaf)?;
                        Some((drawn.session, piece(leaf)?))
                    });
                    // The supports its file came with go along as they are
                    // (#259).
                    let imported = follows.iter().filter(|&&leaf| {
                        spatial
                            .leaf(leaf)
                            .is_some_and(|q| is_imported_supports(q.mesh()))
                    });
                    Some(Source {
                        body,
                        model: piece(model)?,
                        rigid: leaves[1..]
                            .iter()
                            .chain(imported)
                            .filter_map(|&l| piece(l))
                            .collect(),
                        set,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (!sources.is_empty()).then_some((gathered.bodies, sources))
}

/// The worker's part: the copies, then each copy's support set. `None`
/// when cancelled.
fn make(
    bodies: &[Body],
    sources: &[Source],
    plate: &Plate,
    settings: &Settings,
    limit: Option<usize>,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Option<Made> {
    let of_body: Vec<usize> = sources.iter().map(|s| s.body).collect();
    let filled = sc_arrange::fill(
        bodies,
        &of_body,
        plate,
        settings,
        limit,
        &progress.placed,
        cancel,
    )
    .ok()?;
    let source_of = |body: usize| sources.iter().find(|s| s.body == body);
    // The copies, then the models that move to let more in.
    let todo: Vec<(usize, &Source, Placement)> = filled
        .copies
        .iter()
        .chain(&filled.moved)
        .enumerate()
        .filter_map(|(i, copy)| {
            let source = source_of(copy.source)?;
            source.set.is_some().then_some((i, source, copy.placement))
        })
        .collect();
    progress.sets_total.store(todo.len(), Ordering::Relaxed);
    let drawing = Instant::now();
    let mut sets = vec![None; filled.copies.len() + filled.moved.len()];
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .min(todo.len().max(1));
    let drawn: Vec<Vec<(usize, Option<CopiedSet>)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(i, source, placement)) = todo.get(k) else {
                            break;
                        };
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        out.push((i, copied_set(source, &placement)));
                        progress.sets_done.fetch_add(1, Ordering::Relaxed);
                    }
                    out
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().unwrap_or_default())
            .collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    for (i, set) in drawn.into_iter().flatten() {
        sets[i] = set;
    }
    Some(Made {
        filled,
        sets,
        sets_time: drawing.elapsed(),
    })
}

/// The support set of `source` taken along to a copy of it at
/// `placement`, as a move of the model would take it (#119, #153), and
/// drawn there. `None` when it cannot be: the copy then takes the drawn
/// mesh along as it is.
fn copied_set(source: &Source, placement: &Placement) -> Option<CopiedSet> {
    let (session, _) = source.set.as_ref()?;
    if placement.angle == 0.0 {
        return moved_set(session, placement.translation);
    }
    if session.model_count() != 1 || !session.supports_only_on(0) {
        return None;
    }
    // The port is DragonFruit's code: a panic in it leaves the copy with
    // its supports as they are drawn, not a failed fill.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The session gets the turned model as a run makes it.
        let world = Transform3D::Affine(placement.motion().compose(&source.model.world));
        let baked = sc_scene::bake_mesh_to_world(&source.model.mesh, &world).ok()?;
        let now = model_from_world_mesh(String::new(), &baked);
        let moved = session.turned_in_plane(0, now, placement.angle, placement.translation);
        let mesh = moved.mesh().ok()??;
        Some((Arc::new(moved), Arc::new(mesh)))
    }))
    .ok()
    .flatten()
}

/// Adds the copies `made` as one undoable edit, and makes each copied
/// support set the one in force for its copy. The copies of a model show
/// its mesh through one mesh resource, as pasted copies do: the mesh, its
/// spatial caches and its file entry are shared, never duplicated.
fn add_copies(document: &SceneDocument, sources: &[Source], made: Made) -> Result<(), String> {
    let source_of = |body: usize| sources.iter().position(|s| s.body == body);
    let copied: Vec<usize> = made
        .filled
        .copies
        .iter()
        .filter_map(|c| source_of(c.source))
        .collect();
    let leaves: usize = copied
        .iter()
        .map(|&k| 1 + sources[k].rigid.len() + usize::from(sources[k].set.is_some()))
        .sum();
    // The resource each source's copies show: one the scene already has
    // for its mesh (a pasted model's), or a new one.
    let (existing, position) = document
        .with_scene(|spatial| {
            let graph = spatial.graph();
            let existing: Vec<Option<MeshResourceId>> = sources
                .iter()
                .map(|s| graph.find_mesh_resource_by_arc(&s.model.mesh))
                .collect();
            (existing, graph.roots().len())
        })
        .ok_or("no scene")?;
    let fresh = existing
        .iter()
        .enumerate()
        .filter(|&(k, rid)| rid.is_none() && copied.contains(&k))
        .count();
    let (ids, mut rids) = document.allocate_ids(leaves, fresh);
    if ids.len() != leaves || rids.len() != fresh {
        return Err("no scene".to_owned());
    }
    // The models that move to let more copies in go first.
    let mut drawn: Vec<(NodeId, NodeId, Arc<DfSupportSession>)> = Vec::new();
    let mut sets = made.sets;
    let moved_sets = sets.split_off(made.filled.copies.len());
    let mut edits = moved_models(
        document,
        sources,
        &made.filled.moved,
        moved_sets,
        &mut drawn,
    )
    .ok_or("a moved model is gone")?;
    edits.reserve(leaves + fresh);
    let mut resource: Vec<Option<MeshResourceId>> = existing;
    for (k, rid) in resource.iter_mut().enumerate() {
        if rid.is_none() && copied.contains(&k) {
            let new = rids.pop().expect("one per source without one");
            edits.push(SceneEdit::AddMeshResource {
                rid: new,
                mesh: sources[k].model.mesh.clone(),
            });
            *rid = Some(new);
        }
    }
    let mut ids = ids.into_iter();
    let mut position = position;
    // Each copy keeps the paint of its model.
    let mut painted_copies: Vec<(usize, NodeId)> = Vec::new();
    let mut add = |payload: LeafPayload, world: AffineTransform3D, meta: Option<SceneMetadata>| {
        let node_id = ids.next().expect("one id per leaf");
        edits.push(SceneEdit::AddLeaf {
            node_id,
            attachment: LeafAttachment::Root { position },
            payload,
            local_transform: Transform3D::Affine(world),
            meta,
        });
        position += 1;
        node_id
    };
    let copies = made
        .filled
        .copies
        .iter()
        .zip(sets)
        .filter_map(|(copy, set)| Some((source_of(copy.source)?, copy, set)));
    for (k, copy, set) in copies {
        let source = &sources[k];
        let motion = copy.placement.motion();
        let moved = |p: &Piece| motion.compose(&p.world);
        let rid = resource[k].expect("every copied source has its resource");
        let model = add(
            LeafPayload::MeshResource(rid),
            moved(&source.model),
            source.model.meta.clone(),
        );
        painted_copies.push((k, model));
        for piece in &source.rigid {
            add(
                LeafPayload::Mesh(piece.mesh.clone()),
                moved(piece),
                piece.meta.clone(),
            );
        }
        if let Some((_, piece)) = &source.set {
            match set {
                // Drawn where the copy stands, in world space, as every set
                // in force is.
                Some((session, mesh)) => {
                    let leaf = add(
                        LeafPayload::Mesh(mesh),
                        AffineTransform3D::identity(),
                        piece.meta.clone(),
                    );
                    drawn.push((leaf, model, session));
                }
                None => {
                    add(
                        LeafPayload::Mesh(piece.mesh.clone()),
                        moved(piece),
                        piece.meta.clone(),
                    );
                }
            }
        }
    }
    for (k, copy) in painted_copies {
        let paint: Vec<(PaintKind, SurfaceTags)> = document
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .surface_tags_iter()
                    .filter(|(leaf, _, _)| *leaf == sources[k].model.leaf)
                    .map(|(_, kind, tags)| (kind, tags.clone()))
                    .collect()
            })
            .unwrap_or_default();
        edits.extend(
            paint
                .into_iter()
                .map(|(kind, tags)| SceneEdit::SetSurfaceTags {
                    node_id: copy,
                    kind,
                    before: None,
                    after: Some(tags),
                }),
        );
    }
    let edit = SceneEdit::Compound(edits);
    match document.apply_edit(edit.clone()) {
        Ok(()) => document.push_edit(edit),
        Err(EditNotApplied::Rejected(error)) => return Err(error.to_string()),
        Err(EditNotApplied::NoSceneLoaded) => return Err("no scene".to_owned()),
    }
    let sessions = document.support_sessions();
    for (leaf, model, session) in drawn {
        record(document, &sessions, leaf, model, session);
    }
    Ok(())
}

/// Fills on this thread and adds the copies, as a run does: for tests.
/// Also returns how long drawing the copies' support sets took.
#[cfg(test)]
pub(crate) fn fill_now(
    document: &SceneDocument,
    plate: &Plate,
    options: super::ArrangeOptions,
) -> Option<(Filled, Duration)> {
    fill_on_now(document, None, plate, options)
}

/// [`fill_now`], gathering the objects of the plate `on` names (#548).
#[cfg(test)]
pub(crate) fn fill_on_now(
    document: &SceneDocument,
    on: Option<(crate::build_volume::BuildVolume, crate::plates::Plates, u32)>,
    plate: &Plate,
    options: super::ArrangeOptions,
) -> Option<(Filled, Duration)> {
    let (bodies, sources) = gather_fill(document, on)?;
    let pending = Pending {
        revision: document.scene_revision(),
        sources,
        started: Instant::now(),
    };
    let limit = (options.max_copies > 0).then_some(options.max_copies as usize);
    let made = make(
        &bodies,
        &pending.sources,
        plate,
        &options.settings(),
        limit,
        &Progress::default(),
        &AtomicBool::new(false),
    )?;
    let (filled, sets_time) = (made.filled.clone(), made.sets_time);
    match land(document, &pending, made) {
        Landed::Added(_) | Landed::NoneFit => Some((filled, sets_time)),
        Landed::Stale | Landed::Rejected(_) => None,
    }
}

/// The support set the first selected model's copy at `placement` gets:
/// for tests.
#[cfg(test)]
pub(crate) fn copied_set_now(document: &SceneDocument, placement: &Placement) -> Option<CopiedSet> {
    let (_, sources) = gather_fill(document, None)?;
    copied_set(sources.first()?, placement)
}

#[cfg(test)]
mod tests;

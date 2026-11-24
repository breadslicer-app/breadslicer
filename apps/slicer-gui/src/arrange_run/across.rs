/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Arrange all over the plates (#554).
//!
//! Every model of every plate is packed for the fewest plates: the first
//! plate takes as many as fit, the next the rest, and plates are added at the
//! end as they are needed. Plates left empty, which held objects before, are
//! taken off, at the end or between others: the plates after one taken off
//! move up with their models and keep their names. Models the person placed
//! by hand may stay where they are, as obstacles, and move with their plate. The plate's own Arrange packs its plate, and what
//! does not fit goes to plates added at the end. One step of history: the
//! moves and the plates.

use bread_arrange::{Across, Body, Plate};
use sc_scene::NodeId;
use sc_scene_edit::SceneEdit;

use super::{ArrangeScope, gather_all, motion_edits, plate_of};
use crate::build_volume::BuildVolume;
use crate::scene_document::{EditNotApplied, SceneDocument};

/// What an Arrange all works on.
pub(crate) struct Job {
    pub(crate) bodies: Vec<Body>,
    /// Per body that stays, the plate it stands on: an index into `plates`.
    pub(crate) stands_on: Vec<Option<usize>>,
    /// The plates it may use, in the order they fill.
    pub(crate) plates: Vec<Plate>,
    /// Per plate of `plates`, the scene's plate it is.
    pub(crate) real: Vec<u32>,
    /// Per body, the leaves its move moves, and its name.
    pub(crate) leaves: Vec<Vec<NodeId>>,
    pub(crate) names: Vec<String>,
    /// Per body, its model: `None` for supports no model owns.
    pub(crate) models: Vec<Option<NodeId>>,
    /// Per body, whether it is a model with no supports yet (#556).
    pub(crate) bare: Vec<bool>,
    /// Per body, the scene's plate it stands on now.
    pub(crate) on: Vec<u32>,
    /// Per body, the leaf that says which plate it stands on: its model, or
    /// the supports no model owns.
    pub(crate) anchors: Vec<NodeId>,
    /// Per plate of the scene, whether objects stood on it.
    pub(crate) held: Vec<bool>,
    pub(crate) scope: ArrangeScope,
}

/// The area plate `index` of the scene covers, on `volume`.
pub(crate) fn plate_at(volume: &BuildVolume, index: u32) -> Plate {
    let mut plate = plate_of(volume);
    let at = crate::plates::offset(volume, index);
    let (dx, dy) = (f64::from(at.x), f64::from(at.y));
    plate.min = [plate.min[0] + dx, plate.min[1] + dy];
    plate.max = [plate.max[0] + dx, plate.max[1] + dy];
    plate.keep_out = crate::dead_zones::keep_out_at([dx, dy]);
    plate
}

/// The models placed by hand an Arrange of `scope` would move: on the
/// plates it works on.
pub(crate) fn by_hand(
    document: &SceneDocument,
    volume: &BuildVolume,
    scope: ArrangeScope,
) -> usize {
    let by_hand = document.hand_placed();
    if by_hand.is_empty() {
        return 0;
    }
    let plates = document.plates();
    document
        .with_scene(|spatial| {
            by_hand
                .iter()
                .filter(|&&leaf| match scope {
                    ArrangeScope::Plates => true,
                    ArrangeScope::ActivePlate => {
                        crate::plate_layout::plate_of_leaf(spatial, volume, plates, leaf)
                            == Some(plates.active())
                    }
                })
                .count()
        })
        .unwrap_or(0)
}

/// The job of an Arrange of `scope`; with `keep_by_hand`, the models placed
/// by hand stay where they are. `None` when nothing would move.
pub(crate) fn job(
    document: &SceneDocument,
    volume: &BuildVolume,
    scope: ArrangeScope,
    keep_by_hand: bool,
) -> Option<Job> {
    let gathered = gather_all(document, false)?;
    let plates = document.plates();
    let count = plates.count();
    let by_hand = if keep_by_hand {
        document.hand_placed()
    } else {
        Vec::new()
    };
    let on: Vec<u32> = document
        .with_scene(|spatial| {
            gathered
                .anchors
                .iter()
                .map(|&leaf| {
                    crate::plate_layout::plate_of_leaf(spatial, volume, plates, leaf).unwrap_or(0)
                })
                .collect()
        })
        .unwrap_or_default();
    let real: Vec<u32> = match scope {
        ArrangeScope::Plates => (0..crate::plates::MAX_PLATES).collect(),
        ArrangeScope::ActivePlate => std::iter::once(plates.active())
            .chain(count..crate::plates::MAX_PLATES)
            .collect(),
    };
    let mut held = vec![false; count as usize];
    let mut job = Job {
        bodies: Vec::new(),
        stands_on: Vec::new(),
        plates: real.iter().map(|&p| plate_at(volume, p)).collect(),
        real: real.clone(),
        leaves: Vec::new(),
        names: Vec::new(),
        models: Vec::new(),
        bare: Vec::new(),
        on: Vec::new(),
        anchors: Vec::new(),
        held: Vec::new(),
        scope,
    };
    for (k, mut body) in gathered.bodies.into_iter().enumerate() {
        let plate = on[k];
        if let Some(h) = held.get_mut(plate as usize) {
            *h = true;
        }
        if scope == ArrangeScope::ActivePlate && plate != plates.active() {
            continue;
        }
        let anchor = gathered.anchors[k];
        let is_model = !gathered.leaves[k].is_empty();
        if is_model && by_hand.contains(&anchor) {
            body.fixed = true;
        }
        job.stands_on.push(
            body.fixed
                .then(|| real.iter().position(|&p| p == plate))
                .flatten(),
        );
        job.bodies.push(body);
        job.leaves.push(gathered.leaves[k].clone());
        job.names.push(gathered.names[k].clone());
        job.models.push(is_model.then_some(anchor));
        job.bare.push(gathered.bare[k]);
        job.on.push(plate);
        job.anchors.push(anchor);
    }
    job.held = held;
    Some(job).filter(|j| j.bodies.iter().any(|b| !b.fixed))
}

/// What an applied Arrange all did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) moved: usize,
    /// How many plates the scene has after it, and before.
    pub(crate) plates: u32,
    pub(crate) plates_before: u32,
    /// The names of the objects on no plate.
    pub(crate) left_off: Vec<String>,
}

/// Applies `across`, worked out for `job`, as one step of history: the
/// moves, and the plates added or taken off. With `again`, an Arrange all
/// pressed again with nothing done to the scene since the last (#560), with
/// other settings, the step joins the last one: one Undo goes back to
/// before the first, and what stood in the middle is let go.
pub(crate) fn apply(
    document: &SceneDocument,
    job: &Job,
    across: &Across,
    again: bool,
) -> Result<Option<Applied>, EditNotApplied> {
    let mut placements: Vec<Option<bread_arrange::Placement>> = across
        .placements
        .iter()
        .map(|p| p.map(|(_, placement)| placement))
        .collect();
    let before = document.plate_names();
    let count = before.len() as u32;
    let needed = job.real[..across.plates_used.min(job.real.len())]
        .iter()
        .map(|&p| p + 1)
        .max()
        .unwrap_or(1);
    let plates = match job.scope {
        ArrangeScope::ActivePlate => count.max(needed),
        // Plates emptied at the end go; one empty before stays, with the
        // plates before it.
        ArrangeScope::Plates => {
            let kept_empty = (0..count)
                .filter(|&p| !job.held.get(p as usize).copied().unwrap_or(true))
                .map(|p| p + 1)
                .max()
                .unwrap_or(1);
            needed.max(kept_empty).max(1)
        }
    }
    .min(crate::plates::MAX_PLATES);
    // Plates left empty between others, which held objects before, go too:
    // the plates after them move up, each body with its plate.
    let gone = emptied_between(job, across, plates, count);
    if !gone.is_empty() {
        let up = |p: u32| p - gone.iter().filter(|&&g| g < p).count() as u32;
        for (k, placement) in placements.iter_mut().enumerate() {
            let p = landed_on(job, across, k);
            if up(p) == p {
                continue;
            }
            let (Some(from), Some(to)) = (job.plate_of(p), job.plate_of(up(p))) else {
                continue;
            };
            let by = [to.min[0] - from.min[0], to.min[1] - from.min[1]];
            let moved = placement.get_or_insert(bread_arrange::Placement {
                angle: 0.0,
                translation: [0.0, 0.0],
                on_plate: true,
            });
            moved.translation = [moved.translation[0] + by[0], moved.translation[1] + by[1]];
        }
    }
    // What moves with each body: its model and the supports that do not
    // follow it, or the supports no model owns.
    let leaves: Vec<Vec<NodeId>> = job
        .leaves
        .iter()
        .zip(&job.anchors)
        .map(|(leaves, &anchor)| {
            if leaves.is_empty() {
                vec![anchor]
            } else {
                leaves.clone()
            }
        })
        .collect();
    let edits = motion_edits(document, &placements, &leaves);
    let mut after = before.clone();
    after.truncate(plates as usize);
    while (after.len() as u32) < plates {
        let name = SceneDocument::new_plate_name(&after);
        after.push(name);
    }
    // The plates taken off between others, by their names: the others keep
    // theirs.
    for &g in gone.iter().rev() {
        after.remove(g as usize);
    }
    let plates = after.len() as u32;
    if edits.is_empty() && plates == count {
        return Ok(None);
    }
    let edit = SceneEdit::Compound(edits);
    document.apply_edit(edit.clone())?;
    if after == before {
        document.push_edit(edit);
    } else {
        // The active plate, or the one in its place.
        let active = document.plates().active();
        let active = active - gone.iter().filter(|&&g| g < active).count() as u32;
        document.push_plates_edit(edit, before, after, active.min(plates - 1));
    }
    if again {
        document.squash_last_two_steps();
        crate::support_persist::forget_unreachable(document, &document.support_sessions());
    }
    // Where the app put them, not by hand: models placed by hand that
    // stayed, and moved with their plate, are still placed by hand.
    let placed: Vec<NodeId> = job
        .models
        .iter()
        .zip(&job.bodies)
        .zip(&placements)
        .filter(|((_, body), p)| p.is_some() && !body.fixed)
        .filter_map(|((m, _), _)| *m)
        .collect();
    document.note_placed(&placed);
    Ok(Some(Applied {
        moved: across.placements.iter().flatten().count(),
        plates,
        plates_before: count,
        left_off: across
            .left_off()
            .into_iter()
            .filter_map(|i| job.names.get(i).cloned())
            .collect(),
    }))
}

impl Job {
    /// The area scene plate `plate` covers, when the job knows it.
    fn plate_of(&self, plate: u32) -> Option<&Plate> {
        self.real
            .iter()
            .position(|&p| p == plate)
            .and_then(|k| self.plates.get(k))
    }
}

/// The scene's plate body `k` stands on once `across` is applied: where it
/// went, or where it stays.
fn landed_on(job: &Job, across: &Across, k: usize) -> u32 {
    match across.placements.get(k).copied().flatten() {
        Some((p, _)) => job.real.get(p).copied().unwrap_or(job.on[k]),
        None => job.on[k],
    }
}

/// The scene's plates under `plates`, the count Arrange all leaves before
/// taking off any between others, that held objects before (`count` of
/// them stood) and hold none once `across` is applied, lowest first. Only
/// Arrange all takes plates off between others; a plate's own Arrange
/// leaves the plates as they are.
fn emptied_between(job: &Job, across: &Across, plates: u32, count: u32) -> Vec<u32> {
    if job.scope != ArrangeScope::Plates {
        return Vec::new();
    }
    let mut holds = vec![false; plates.max(count) as usize];
    for k in 0..job.bodies.len() {
        if let Some(h) = holds.get_mut(landed_on(job, across, k) as usize) {
            *h = true;
        }
    }
    (0..plates.min(count))
        .filter(|&p| job.held.get(p as usize).copied().unwrap_or(false) && !holds[p as usize])
        .collect()
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Footprints kept between runs (#560).
//!
//! Drawing a body's bands is most of an arrangement's time. Each part of a
//! body (a model, a set of supports) is drawn in a frame of its own: its
//! body's first part with no turn about the vertical axis and no move in
//! the plate plane. Moving a body or turning it about the vertical axis
//! leaves its parts where they were in that frame, so their bands are
//! taken from here. Scaling, tilting or new geometry draws a part again.
//!
//! A part is known by its mesh (the very one: a mesh is never changed in
//! place behind an `Arc`) and its pose in its body's frame. The entry holds
//! a weak handle on the mesh, so the address cannot name another mesh
//! while the entry lives; entries whose mesh is gone go first.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::footprint::{self, Base, Geometry, Placed};

/// What the kept bases may take, roughly.
const BUDGET_BYTES: usize = 256 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    mesh: usize,
    /// The part's pose in its body's frame: the linear part to 1e-9 and the
    /// move to 1e-7 mm.
    pose: [i64; 12],
    /// The sizes the bands are drawn at.
    geometry: [u64; 4],
}

impl Key {
    fn of(mesh: &Arc<IndexedMesh>, pose: &AffineTransform3D, geo: &Geometry) -> Key {
        let (l, t) = (pose.linear(), pose.translation());
        let mut q = [0i64; 12];
        for k in 0..9 {
            q[k] = (l[k / 3][k % 3] * 1e9).round() as i64;
        }
        for k in 0..3 {
            q[9 + k] = (t[k] * 1e7).round() as i64;
        }
        Key {
            mesh: Arc::as_ptr(mesh) as usize,
            pose: q,
            geometry: [
                geo.fine.to_bits(),
                geo.band.unwrap_or(-1.0).to_bits(),
                geo.gap.to_bits(),
                geo.floor.to_bits(),
            ],
        }
    }
}

struct Entry {
    mesh: Weak<IndexedMesh>,
    base: Option<Arc<Base>>,
    used: u64,
    bytes: usize,
}

#[derive(Default)]
struct Kept {
    entries: HashMap<Key, Entry>,
    tick: u64,
    bytes: usize,
}

impl Kept {
    fn insert(&mut self, key: Key, mesh: &Arc<IndexedMesh>, base: Option<Arc<Base>>) {
        self.tick += 1;
        let bytes = base.as_ref().map_or(64, |b| b.bytes());
        if let Some(old) = self.entries.insert(
            key,
            Entry {
                mesh: Arc::downgrade(mesh),
                base,
                used: self.tick,
                bytes,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        if self.bytes > BUDGET_BYTES {
            self.trim();
        }
    }

    /// Lets go of the parts whose mesh is gone, then of the least used,
    /// until the kept bases fit their budget.
    fn trim(&mut self) {
        let before = self.entries.len();
        self.entries.retain(|_, e| e.mesh.strong_count() > 0);
        if self.entries.len() != before {
            self.bytes = self.entries.values().map(|e| e.bytes).sum();
        }
        if self.bytes <= BUDGET_BYTES {
            return;
        }
        let mut by_use: Vec<(u64, Key)> = self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        by_use.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in by_use {
            if self.bytes <= BUDGET_BYTES / 2 {
                break;
            }
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.bytes;
            }
        }
    }
}

static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(|| Mutex::new(Kept::default()));

/// The bands of `mesh` at `pose`, its pose in its body's frame: kept from an
/// earlier run, or drawn now and kept. `None` for no triangles.
pub(crate) fn part_base(
    mesh: &Arc<IndexedMesh>,
    pose: &AffineTransform3D,
    geo: &Geometry,
) -> Option<Arc<Base>> {
    let key = Key::of(mesh, pose, geo);
    {
        let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        kept.tick += 1;
        let tick = kept.tick;
        if let Some(entry) = kept.entries.get_mut(&key) {
            entry.used = tick;
            return entry.base.clone();
        }
    }
    let base = footprint::part_base(&Placed { mesh, world: pose }, geo).map(Arc::new);
    KEPT.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, mesh, base.clone());
    base
}

/// How many parts are kept.
pub fn kept_parts() -> usize {
    KEPT.lock().unwrap_or_else(|e| e.into_inner()).entries.len()
}

/// The parts of `parts` not kept yet, each once.
pub(crate) fn wanted(
    parts: Vec<(Arc<IndexedMesh>, AffineTransform3D)>,
    geo: &Geometry,
) -> Vec<(Arc<IndexedMesh>, AffineTransform3D)> {
    let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let mut seen = std::collections::HashSet::new();
    parts
        .into_iter()
        .filter(|(mesh, pose)| {
            let key = Key::of(mesh, pose, geo);
            !kept.entries.contains_key(&key) && seen.insert(key)
        })
        .collect()
}

/// How many parts drawn from `mesh` are kept.
pub fn kept_parts_of(mesh: &Arc<IndexedMesh>) -> usize {
    let at = Arc::as_ptr(mesh) as usize;
    KEPT.lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .keys()
        .filter(|k| k.mesh == at)
        .count()
}

/// Roughly how many bytes the kept parts take.
pub fn kept_bytes() -> usize {
    KEPT.lock().unwrap_or_else(|e| e.into_inner()).bytes
}

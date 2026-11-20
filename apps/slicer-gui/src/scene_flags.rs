/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Hidden and locked models (#16), from the scene panel's eye and padlock.
//!
//! - **Hidden:** the model and its supports are not drawn, not picked and
//!   not sliced; its row in the scene panel says so, and the Slice tab says
//!   how many are left out of the print. Nothing hidden ever prints unseen.
//! - **Locked:** the model and its supports stay drawn and print as any
//!   other, but a click, a box or Select all never picks them, so they
//!   cannot be moved or edited by mistake.
//!
//! Each change is one step of history (Undo shows or unlocks again) and the
//! project keeps both lists, by the models' rows in the scene's list of
//! leaves, as it keeps groups (#475). A hidden or locked model leaves the
//! selection.

use sc_scene::NodeId;

use super::{HistoryEntry, SceneDocument, SceneEvent, UNDO_STACK_CAP};
use crate::selection::Selection;
use sc_scene_edit::SceneEdit;

/// The hidden and the locked models.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneFlags {
    pub(crate) hidden: Vec<NodeId>,
    pub(crate) locked: Vec<NodeId>,
}

impl SceneFlags {
    fn is_empty(&self) -> bool {
        self.hidden.is_empty() && self.locked.is_empty()
    }
}

impl SceneDocument {
    /// The leaves of the scene, in the list's order.
    fn leaf_rows(&self) -> Vec<NodeId> {
        self.with_scene(|spatial| (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect())
            .unwrap_or_default()
    }

    /// The flags with their models still in the scene.
    fn present_flags(&self) -> SceneFlags {
        let flags = self.flags.borrow();
        if flags.is_empty() {
            return SceneFlags::default();
        }
        let present = self.leaf_rows();
        SceneFlags {
            hidden: flags
                .hidden
                .iter()
                .copied()
                .filter(|id| present.contains(id))
                .collect(),
            locked: flags
                .locked
                .iter()
                .copied()
                .filter(|id| present.contains(id))
                .collect(),
        }
    }

    /// The hidden models (#16).
    pub(crate) fn hidden_models(&self) -> Vec<NodeId> {
        self.present_flags().hidden
    }

    /// Whether model `id` is hidden (#16).
    pub(crate) fn is_hidden(&self, id: NodeId) -> bool {
        self.flags.borrow().hidden.contains(&id)
    }

    /// Whether model `id` is locked (#16).
    pub(crate) fn is_locked(&self, id: NodeId) -> bool {
        self.flags.borrow().locked.contains(&id)
    }

    /// Every leaf left out of the view and the slice (#16): the hidden
    /// models and their supports.
    pub(crate) fn hidden_leaves(&self) -> Vec<NodeId> {
        let hidden = self.hidden_models();
        if hidden.is_empty() {
            return hidden;
        }
        self.with_their_supports(hidden)
    }

    /// Every leaf no click, box or Select all picks (#16): the hidden and
    /// the locked models, and their supports.
    pub(crate) fn unpickable_leaves(&self) -> Vec<NodeId> {
        let flags = self.present_flags();
        if flags.is_empty() {
            return Vec::new();
        }
        let mut models = flags.hidden;
        models.extend(flags.locked);
        self.with_their_supports(models)
    }

    /// `sel` without the leaves no pick may take (#16).
    pub(super) fn without_unpickable(&self, sel: Selection) -> Selection {
        if self.flags.borrow().is_empty() {
            return sel;
        }
        let blocked = self.unpickable_leaves();
        if blocked.is_empty() {
            return sel;
        }
        sel.retained(|id| !blocked.contains(&id))
    }

    /// Hides `models` (`on`) or shows them again (#16), as one step of
    /// history. Whether anything changed.
    pub(crate) fn set_hidden(&self, models: &[NodeId], on: bool) -> bool {
        let mut after = self.present_flags();
        if !toggle(&mut after.hidden, models, on) {
            return false;
        }
        self.push_flags(after);
        true
    }

    /// Locks `models` (`on`) or unlocks them (#16), as one step of history.
    /// Whether anything changed.
    pub(crate) fn set_locked(&self, models: &[NodeId], on: bool) -> bool {
        let mut after = self.present_flags();
        if !toggle(&mut after.locked, models, on) {
            return false;
        }
        self.push_flags(after);
        true
    }

    /// Sets the flags to `after` as one step of history: an empty edit, so
    /// the project is unsaved and a slice of the scene out of date.
    fn push_flags(&self, after: SceneFlags) {
        if !self.is_loaded() {
            return;
        }
        let before = self.flags.borrow().clone();
        let was_dirty = self.is_dirty();
        crate::breadcrumbs::note("edit: hide or lock");
        let wants = self
            .undo_stack
            .borrow()
            .back()
            .and_then(|entry| entry.wants.clone());
        let id = self.next_edit_id.get();
        self.next_edit_id.set(id + 1);
        {
            let mut undo = self.undo_stack.borrow_mut();
            if undo.len() == UNDO_STACK_CAP
                && let Some(gone) = undo.pop_front()
            {
                self.note_evicted(&gone);
            }
            undo.push_back(HistoryEntry {
                id,
                edit: SceneEdit::Compound(Vec::new()),
                wants,
                paint_only: false,
                marks: None,
                plates: None,
                groups: None,
                flags: Some((before, after.clone())),
            });
        }
        self.redo_stack.borrow_mut().clear();
        self.put_flags(after);
        self.notify(SceneEvent::UndoStackChanged);
        self.notify_dirty_change(was_dirty);
    }

    /// Sets the flags to `flags`, as Undo and Redo do; what is no longer
    /// pickable leaves the selection.
    pub(super) fn put_flags(&self, flags: SceneFlags) {
        *self.flags.borrow_mut() = flags;
        let kept = self.without_unpickable(self.selection());
        if !self.selection.is(&kept) {
            self.selection.set(kept);
        }
        self.notify(SceneEvent::SelectionChanged);
        // The scene panel's rows and the slice's state follow, as they do
        // the plates.
        self.notify(SceneEvent::PlatesChanged);
    }

    /// Forgets every flag, for a scene installed or cleared.
    pub(super) fn clear_flags(&self) {
        *self.flags.borrow_mut() = SceneFlags::default();
    }

    /// The flags as rows of the scene's list of leaves, for the project:
    /// hidden, then locked.
    pub(super) fn flag_rows(&self) -> (Vec<u32>, Vec<u32>) {
        let flags = self.present_flags();
        if flags.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let rows = self.leaf_rows();
        let of = |ids: &[NodeId]| -> Vec<u32> {
            let mut out: Vec<u32> = ids
                .iter()
                .filter_map(|id| rows.iter().position(|r| r == id))
                .filter_map(|r| u32::try_from(r).ok())
                .collect();
            out.sort_unstable();
            out
        };
        (of(&flags.hidden), of(&flags.locked))
    }

    /// The flags of a project, from its rows.
    pub(super) fn load_flag_rows(&self, hidden: &[u32], locked: &[u32]) {
        if hidden.is_empty() && locked.is_empty() {
            self.clear_flags();
            return;
        }
        let rows = self.leaf_rows();
        let of = |rs: &[u32]| -> Vec<NodeId> {
            rs.iter()
                .filter_map(|&r| rows.get(r as usize).copied())
                .collect()
        };
        *self.flags.borrow_mut() = SceneFlags {
            hidden: of(hidden),
            locked: of(locked),
        };
    }
}

/// Puts `models` in `list` (`on`) or takes them out; whether it changed.
fn toggle(list: &mut Vec<NodeId>, models: &[NodeId], on: bool) -> bool {
    let before = list.len();
    if on {
        for &m in models {
            if !list.contains(&m) {
                list.push(m);
            }
        }
        list.len() != before
    } else {
        list.retain(|m| !models.contains(m));
        list.len() != before
    }
}

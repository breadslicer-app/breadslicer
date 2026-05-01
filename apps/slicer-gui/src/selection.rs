/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Document-level selection model.
//!
//! `Selection` is the single-object-or-none shape exposed to the UI;
//! `ObjectIdx` is the typed index into `Scene::objects`; `SelectionState`
//! wraps a `Cell<Selection>` so callers can avoid repeating the
//! `.get().primary().map(ObjectIdx::as_usize)` chain at every read.

use std::cell::Cell;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct ObjectIdx(pub(crate) usize);

impl ObjectIdx {
    pub(crate) fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Selection {
    None,
    One(ObjectIdx),
}

impl Selection {
    pub(crate) fn primary(self) -> Option<ObjectIdx> {
        match self {
            Selection::None => None,
            Selection::One(i) => Some(i),
        }
    }
}

pub(crate) struct SelectionState {
    cell: Cell<Selection>,
}

impl SelectionState {
    pub(crate) fn new() -> Self {
        Self {
            cell: Cell::new(Selection::None),
        }
    }

    pub(crate) fn current(&self) -> Selection {
        self.cell.get()
    }

    pub(crate) fn set(&self, sel: Selection) {
        self.cell.set(sel);
    }

    /// Currently-selected object's index as a plain `usize`. Convenience
    /// for the (very common) call sites that only want to look up
    /// `scene.objects[idx]`.
    pub(crate) fn primary_idx(&self) -> Option<usize> {
        self.cell.get().primary().map(ObjectIdx::as_usize)
    }
}

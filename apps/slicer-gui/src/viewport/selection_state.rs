/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Owner of the controller's selection cell. Encapsulates the
//! `Cell<Selection>` so callers can avoid repeating the
//! `.get().primary().map(ObjectIdx::as_usize)` chain at every read.

use std::cell::Cell;

use super::types::{ObjectIdx, Selection};

pub(super) struct SelectionState {
    cell: Cell<Selection>,
}

impl SelectionState {
    pub(super) fn new() -> Self {
        Self {
            cell: Cell::new(Selection::None),
        }
    }

    pub(super) fn current(&self) -> Selection {
        self.cell.get()
    }

    pub(super) fn set(&self, sel: Selection) {
        self.cell.set(sel);
    }

    /// Currently-selected object's index, or `None` when nothing is
    /// selected. Equivalent to `current().primary()`.
    pub(super) fn primary(&self) -> Option<ObjectIdx> {
        self.cell.get().primary()
    }

    /// Currently-selected object's index as a plain `usize`. Convenience
    /// for the (very common) call sites that only want to look up
    /// `scene.objects[idx]`.
    pub(super) fn primary_idx(&self) -> Option<usize> {
        self.cell.get().primary().map(ObjectIdx::as_usize)
    }
}

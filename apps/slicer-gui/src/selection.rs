/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Document-level selection model.
//!
//! `Selection` is the single-leaf-or-none shape exposed to the UI;
//! `LeafId` wraps a stable [`NodeId`] from the scene graph so
//! selection survives any later graph mutation. `SelectionState`
//! wraps a `Cell<Selection>` so callers can avoid repeating the
//! `.get().primary()` chain at every read.

use std::cell::Cell;

use sc_mesh_core::NodeId;
use sc_mesh_spatial::SceneSpatial;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct LeafId(pub(crate) NodeId);

impl LeafId {
    pub(crate) fn node_id(self) -> NodeId {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Selection {
    None,
    One(LeafId),
}

impl Selection {
    pub(crate) fn primary(self) -> Option<LeafId> {
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

    /// Stable `NodeId` of the currently-selected leaf, or `None`. The
    /// permanent read API: selection survives any graph mutation that
    /// keeps the leaf alive, since `NodeId` is the arena's
    /// never-reused identifier.
    pub(crate) fn primary_node_id(&self) -> Option<NodeId> {
        self.cell.get().primary().map(LeafId::node_id)
    }

    /// BRIDGE: pre-order leaf index of the currently-selected leaf, or
    /// `None` when nothing is selected or the selected `NodeId` no
    /// longer points at a reachable leaf. Used by every legacy
    /// idx-keyed consumer (renderer's `selection: Option<usize>`,
    /// sidebar list highlight, gizmo / keyboard FSMs, document tests)
    /// while the migration to `NodeId` walks through `slicer-gui`.
    ///
    /// DELETE-ON-CLEANUP when those consumers carry `NodeId` directly.
    pub(crate) fn primary_legacy_idx(&self, spatial: &SceneSpatial) -> Option<usize> {
        let node_id = self.primary_node_id()?;
        spatial.leaf_index_of(node_id)
    }
}

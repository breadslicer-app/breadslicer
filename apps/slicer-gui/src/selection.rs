/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Document-level selection model.
//!
//! `Selection` is the none, one or several leaves exposed to the UI (#13);
//! `LeafId` wraps a stable [`NodeId`] from the scene graph so
//! selection survives any later graph mutation. The last leaf to join a
//! selection is its primary: the one the inspectors and the one-object tools
//! act on. `SelectionState` wraps a `RefCell<Selection>` so callers can avoid
//! repeating the `.current().primary()` chain at every read.

use std::cell::RefCell;

use sc_scene::NodeId;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct LeafId(pub(crate) NodeId);

impl LeafId {
    pub(crate) fn node_id(self) -> NodeId {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Selection {
    None,
    One(LeafId),
    /// Two or more leaves, each once, in the order they joined; the last is
    /// the primary. Built through [`Selection::of`], which keeps that shape.
    Many(Vec<LeafId>),
}

impl Selection {
    /// The selection holding `leaves`, each once, in their order: `None`
    /// for no leaf, `One` for a single one.
    pub(crate) fn of(leaves: impl IntoIterator<Item = LeafId>) -> Self {
        let mut kept: Vec<LeafId> = Vec::new();
        for leaf in leaves {
            if !kept.contains(&leaf) {
                kept.push(leaf);
            }
        }
        match kept.len() {
            0 => Selection::None,
            1 => Selection::One(kept[0]),
            _ => Selection::Many(kept),
        }
    }

    pub(crate) fn primary(&self) -> Option<LeafId> {
        self.leaves().last().copied()
    }

    /// Every selected leaf, in the order they joined.
    pub(crate) fn leaves(&self) -> &[LeafId] {
        match self {
            Selection::None => &[],
            Selection::One(leaf) => std::slice::from_ref(leaf),
            Selection::Many(leaves) => leaves,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.leaves().len()
    }

    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.leaves().iter().any(|leaf| leaf.0 == id)
    }

    /// Shift+click (#13): `leaf` leaves the selection when it is in it, and
    /// joins it as the primary when it is not.
    pub(crate) fn toggled(&self, leaf: LeafId) -> Self {
        if self.contains(leaf.0) {
            Self::of(self.leaves().iter().copied().filter(|&l| l != leaf))
        } else {
            Self::of(self.leaves().iter().copied().chain([leaf]))
        }
    }

    /// The selection with `leaves` added after what it holds; a leaf already
    /// in it keeps its place.
    pub(crate) fn with_added(&self, leaves: impl IntoIterator<Item = LeafId>) -> Self {
        Self::of(self.leaves().iter().copied().chain(leaves))
    }

    /// The selection without the leaves `keep` turns down.
    pub(crate) fn retained(&self, mut keep: impl FnMut(NodeId) -> bool) -> Self {
        Self::of(self.leaves().iter().copied().filter(|leaf| keep(leaf.0)))
    }

    /// The selected leaves' ids, in the order they joined.
    pub(crate) fn node_ids(&self) -> Vec<NodeId> {
        self.leaves().iter().map(|leaf| leaf.0).collect()
    }
}

pub(crate) struct SelectionState {
    cell: RefCell<Selection>,
}

impl SelectionState {
    pub(crate) fn new() -> Self {
        Self {
            cell: RefCell::new(Selection::None),
        }
    }

    pub(crate) fn current(&self) -> Selection {
        self.cell.borrow().clone()
    }

    pub(crate) fn set(&self, sel: Selection) {
        *self.cell.borrow_mut() = sel;
    }

    /// Whether the selection is `sel`, without copying it.
    pub(crate) fn is(&self, sel: &Selection) -> bool {
        *self.cell.borrow() == *sel
    }

    /// Stable `NodeId` of the primary selected leaf, or `None`.
    /// Selection survives any graph mutation that keeps the leaf
    /// alive, since `NodeId` is the arena's never-reused identifier.
    pub(crate) fn primary_node_id(&self) -> Option<NodeId> {
        self.cell.borrow().primary().map(LeafId::node_id)
    }

    /// Every selected leaf's id, in the order they joined.
    pub(crate) fn node_ids(&self) -> Vec<NodeId> {
        self.cell.borrow().node_ids()
    }

    pub(crate) fn len(&self) -> usize {
        self.cell.borrow().len()
    }

    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.cell.borrow().contains(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(n: usize) -> Vec<LeafId> {
        let mut scene = sc_scene::Scene::new();
        (0..n)
            .map(|_| LeafId(scene.add_group(None, sc_mesh_core::transform::Transform3D::Identity)))
            .collect()
    }

    #[test]
    fn of_keeps_each_leaf_once_and_picks_the_shape() {
        let l = leaves(3);
        assert_eq!(Selection::of([]), Selection::None);
        assert_eq!(Selection::of([l[0], l[0]]), Selection::One(l[0]));
        assert_eq!(
            Selection::of([l[0], l[1], l[0], l[2]]),
            Selection::Many(vec![l[0], l[1], l[2]])
        );
    }

    #[test]
    fn the_last_leaf_to_join_is_the_primary() {
        let l = leaves(3);
        let sel = Selection::One(l[0]).toggled(l[2]).toggled(l[1]);
        assert_eq!(sel.primary(), Some(l[1]));
        assert_eq!(sel.len(), 3);
        // Taking the primary out hands the role to the one before it.
        assert_eq!(sel.toggled(l[1]).primary(), Some(l[2]));
    }

    #[test]
    fn toggling_a_selected_leaf_takes_it_out() {
        let l = leaves(2);
        let sel = Selection::One(l[0]).toggled(l[1]);
        assert_eq!(sel, Selection::Many(vec![l[0], l[1]]));
        assert_eq!(sel.toggled(l[0]), Selection::One(l[1]));
        assert_eq!(Selection::One(l[1]).toggled(l[1]), Selection::None);
        assert_eq!(Selection::None.toggled(l[1]), Selection::One(l[1]));
    }

    #[test]
    fn adding_keeps_what_was_selected_in_its_place() {
        let l = leaves(3);
        let sel = Selection::Many(vec![l[1], l[0]]).with_added([l[0], l[2]]);
        assert_eq!(sel, Selection::Many(vec![l[1], l[0], l[2]]));
        assert_eq!(
            sel.retained(|id| id != l[0].0),
            Selection::Many(vec![l[1], l[2]])
        );
        assert_eq!(sel.retained(|_| false), Selection::None);
    }
}

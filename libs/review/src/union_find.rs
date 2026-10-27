/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Grouping per-layer regions into bodies that span layers.
//!
//! [`UnionFind`] is a plain disjoint-set forest over `0..n`. [`Groups`] is
//! its frozen, ordered result over the regions of a plate, built by
//! [`PlateRegions::group_solid`](crate::regions::PlateRegions::group_solid)
//! and [`PlateRegions::group_air`](crate::regions::PlateRegions::group_air):
//! for example every solid region that overlaps the one below it forms one
//! printed body, and every air region linked to the open air around the
//! plate is drained.

/// A disjoint-set forest over the integers `0..n`, with path halving.
///
/// The partition it describes does not depend on the order of the unions;
/// only the internal representatives do, so read groups through
/// [`Groups`] when the order matters.
#[derive(Clone, Debug, Default)]
pub struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    /// `n` singleton sets.
    ///
    /// # Panics
    ///
    /// When `n` exceeds `u32::MAX`, far beyond any plate.
    pub fn new(n: usize) -> Self {
        let n = u32::try_from(n).expect("at most u32::MAX elements");
        Self {
            parent: (0..n).collect(),
        }
    }

    /// The number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// Whether there are no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// The representative of the set holding `x`.
    pub fn find(&mut self, x: usize) -> usize {
        let mut x = x as u32;
        loop {
            let parent = self.parent[x as usize];
            if parent == x {
                return x as usize;
            }
            let grandparent = self.parent[parent as usize];
            self.parent[x as usize] = grandparent;
            x = grandparent;
        }
    }

    /// Joins the sets holding `a` and `b`. Returns whether they were apart.
    /// The smaller representative becomes the root, which keeps the forest
    /// shallow for the scan-order unions the review does.
    pub fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        let (root, child) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[child] = root as u32;
        true
    }

    /// Whether `a` and `b` are in the same set.
    pub fn same(&mut self, a: usize, b: usize) -> bool {
        self.find(a) == self.find(b)
    }
}

/// One region of one layer: `index` into
/// [`LayerRegions::solid`](crate::regions::LayerRegions::solid) or
/// [`LayerRegions::air`](crate::regions::LayerRegions::air) of layer `layer`,
/// depending on what was grouped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionRef {
    /// The layer index, 0 on the plate.
    pub layer: usize,
    /// The region's index within its layer.
    pub index: u32,
}

/// Regions of every layer partitioned into groups.
///
/// Group numbers follow the first member in `(layer, index)` order, and the
/// members of a group are listed in that order too, so the first member of a
/// group is its lowest region and the last its highest. The same plate and
/// the same links always give the same numbering.
#[derive(Clone, Debug, Default)]
pub struct Groups {
    /// First global id of each layer, plus the total at the end.
    offsets: Vec<usize>,
    /// Group of each global id.
    group_of: Vec<u32>,
    /// Members of all groups, group by group.
    members: Vec<RegionRef>,
    /// Start of each group in `members`, plus the total at the end.
    starts: Vec<usize>,
}

impl Groups {
    /// Freezes `sets`, whose elements are the global ids laid out by
    /// `offsets` (layer `k` owns ids `offsets[k]..offsets[k + 1]`).
    pub(crate) fn from_union_find(offsets: Vec<usize>, sets: &mut UnionFind) -> Self {
        let total = sets.len();
        let mut group_of = vec![u32::MAX; total];
        let mut group_of_root = vec![u32::MAX; total];
        let mut sizes: Vec<usize> = Vec::new();
        for (id, slot) in group_of.iter_mut().enumerate() {
            let root = sets.find(id);
            if group_of_root[root] == u32::MAX {
                group_of_root[root] = sizes.len() as u32;
                sizes.push(0);
            }
            let group = group_of_root[root];
            *slot = group;
            sizes[group as usize] += 1;
        }
        let mut starts = Vec::with_capacity(sizes.len() + 1);
        let mut acc = 0;
        starts.push(0);
        for size in &sizes {
            acc += size;
            starts.push(acc);
        }
        let mut fill: Vec<usize> = starts[..sizes.len()].to_vec();
        let mut members = vec![RegionRef { layer: 0, index: 0 }; total];
        let mut layer = 0;
        for (id, &group) in group_of.iter().enumerate() {
            while offsets[layer + 1] <= id {
                layer += 1;
            }
            let slot = &mut fill[group as usize];
            members[*slot] = RegionRef {
                layer,
                index: (id - offsets[layer]) as u32,
            };
            *slot += 1;
        }
        Self {
            offsets,
            group_of,
            members,
            starts,
        }
    }

    /// The number of groups.
    #[inline]
    pub fn len(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    /// Whether there is no group (the plate had no region of this kind).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The group `region` belongs to.
    ///
    /// # Panics
    ///
    /// When `region` is not a region of the grouped plate.
    pub fn group_of(&self, region: RegionRef) -> usize {
        let id = self.offsets[region.layer] + region.index as usize;
        assert!(id < self.offsets[region.layer + 1], "no such region");
        self.group_of[id] as usize
    }

    /// The members of group `group`, lowest first.
    pub fn members(&self, group: usize) -> &[RegionRef] {
        &self.members[self.starts[group]..self.starts[group + 1]]
    }

    /// Every group's members, in group order.
    pub fn iter(&self) -> impl Iterator<Item = &[RegionRef]> + '_ {
        (0..self.len()).map(|group| self.members(group))
    }
}

#[cfg(test)]
mod union_find_tests {
    use super::*;

    #[test]
    fn unions_join_sets_once() {
        let mut sets = UnionFind::new(5);
        assert!(sets.union(3, 1));
        assert!(sets.union(1, 4));
        assert!(!sets.union(4, 3));
        assert!(sets.same(3, 4));
        assert!(!sets.same(0, 3));
        assert_eq!(sets.find(4), 1);
    }

    #[test]
    fn groups_are_numbered_by_their_lowest_member() {
        // Layer 0 owns ids 0..2, layer 1 owns 2..5.
        let mut sets = UnionFind::new(5);
        sets.union(4, 1);
        sets.union(2, 0);
        let groups = Groups::from_union_find(vec![0, 2, 5], &mut sets);
        assert_eq!(groups.len(), 3);
        assert_eq!(
            groups.members(0),
            &[
                RegionRef { layer: 0, index: 0 },
                RegionRef { layer: 1, index: 0 },
            ]
        );
        assert_eq!(
            groups.members(1),
            &[
                RegionRef { layer: 0, index: 1 },
                RegionRef { layer: 1, index: 2 },
            ]
        );
        assert_eq!(groups.members(2), &[RegionRef { layer: 1, index: 1 }]);
        assert_eq!(groups.group_of(RegionRef { layer: 1, index: 2 }), 1);
        assert_eq!(groups.iter().count(), 3);
    }
}

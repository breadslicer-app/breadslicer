/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The walls of a build envelope, and sets of them.

use std::ops::BitOrAssign;

use sc_mesh_core::primitives::CoordAxis;

/// The end of an axis that a wall bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Min,
    Max,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Self::Min => 0,
            Self::Max => 1,
        }
    }
}

/// A wall of the build envelope: the plane that bounds `axis` at its `side`.
/// The `Min` wall of [`CoordAxis::Z`] is the plate's floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Wall {
    pub axis: CoordAxis,
    pub side: Side,
}

/// A set of walls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Walls([[bool; 2]; 3]);

impl Walls {
    pub fn contains(self, wall: Wall) -> bool {
        self.0[wall.axis.as_index()][wall.side.index()]
    }

    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    pub fn iter(self) -> impl Iterator<Item = Wall> {
        CoordAxis::ALL
            .into_iter()
            .flat_map(|axis| [Side::Min, Side::Max].map(|side| Wall { axis, side }))
            .filter(move |&wall| self.contains(wall))
    }
}

impl FromIterator<Wall> for Walls {
    fn from_iter<I: IntoIterator<Item = Wall>>(walls: I) -> Self {
        let mut set = Self::default();
        for wall in walls {
            set.0[wall.axis.as_index()][wall.side.index()] = true;
        }
        set
    }
}

impl BitOrAssign for Walls {
    fn bitor_assign(&mut self, other: Self) {
        let theirs = other.0.as_flattened();
        for (mine, theirs) in self.0.as_flattened_mut().iter_mut().zip(theirs) {
            *mine |= theirs;
        }
    }
}

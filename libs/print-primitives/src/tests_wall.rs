/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::primitives::CoordAxis;

use super::wall::{Side, Wall, Walls};

const X_MIN: Wall = Wall {
    axis: CoordAxis::X,
    side: Side::Min,
};
const Y_MAX: Wall = Wall {
    axis: CoordAxis::Y,
    side: Side::Max,
};
const Z_MIN: Wall = Wall {
    axis: CoordAxis::Z,
    side: Side::Min,
};

const ALL_WALLS: [Wall; 6] = [
    X_MIN,
    Wall {
        axis: CoordAxis::X,
        side: Side::Max,
    },
    Wall {
        axis: CoordAxis::Y,
        side: Side::Min,
    },
    Y_MAX,
    Z_MIN,
    Wall {
        axis: CoordAxis::Z,
        side: Side::Max,
    },
];

#[test]
fn the_default_set_is_empty() {
    let walls = Walls::default();
    assert!(walls.is_empty());
    assert_eq!(walls.iter().count(), 0);
    for wall in ALL_WALLS {
        assert!(!walls.contains(wall), "{wall:?}");
    }
}

#[test]
fn a_set_of_1_wall_holds_only_that_wall() {
    for wall in ALL_WALLS {
        let walls: Walls = [wall].into_iter().collect();
        assert!(!walls.is_empty());
        for other in ALL_WALLS {
            assert_eq!(
                walls.contains(other),
                other == wall,
                "{wall:?} and {other:?}"
            );
        }
        assert_eq!(walls.iter().collect::<Vec<_>>(), [wall]);
    }
}

#[test]
fn a_collected_set_holds_each_wall_once() {
    let walls: Walls = [Y_MAX, X_MIN, Y_MAX].into_iter().collect();
    for wall in ALL_WALLS {
        assert_eq!(
            walls.contains(wall),
            wall == X_MIN || wall == Y_MAX,
            "{wall:?}"
        );
    }
    let listed: Vec<Wall> = walls.iter().collect();
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&X_MIN) && listed.contains(&Y_MAX));
}

#[test]
fn bit_or_assign_gives_the_union() {
    let mut walls: Walls = [X_MIN, Y_MAX].into_iter().collect();
    walls |= [Y_MAX, Z_MIN].into_iter().collect();
    assert_eq!(walls, [X_MIN, Y_MAX, Z_MIN].into_iter().collect());
}

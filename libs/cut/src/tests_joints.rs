/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Joints across a cut (#109, #442): a pin and its socket, magnet pockets
//! on both sides, all closed, and left out where they do not fit.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::{
    CutModel, Joint, MIN_WALL_MM, PinShape, Plane, add_joints, is_closed, joint_points, volume,
};

/// A 20 × 20 × 20 mm block from z −10 to 10, cut level at z 0.
fn block() -> Arc<IndexedMesh> {
    let (lo, hi) = ([-10.0f32, -10.0, -10.0], [10.0f32, 10.0, 10.0]);
    let corner = |k: u32| {
        Vertex::new([
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ])
    };
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let faces = quads
        .iter()
        .flat_map(|&[a, b, c, d]| {
            [
                IndexedTriangle::from_raw_indices([a, b, c]),
                IndexedTriangle::from_raw_indices([a, c, d]),
            ]
        })
        .collect();
    Arc::new(IndexedMesh::new(None, (0..8).map(corner).collect(), faces))
}

fn cut_block() -> (Vec<IndexedMesh>, Plane, Vec<[f64; 3]>) {
    let plane = Plane::through([0.0; 3], [0.0, 0.0, 1.0]).unwrap();
    let section = CutModel::new(block()).unwrap().section(plane).unwrap();
    let chosen = vec![true; section.pieces().len()];
    let at = joint_points(&section, &chosen);
    let cut = section.cut(&chosen).unwrap();
    (cut.pieces, plane, at)
}

#[test]
fn the_joint_stands_in_the_middle_of_the_cut() {
    let (_, _, at) = cut_block();
    assert_eq!(at.len(), 1);
    for v in at[0] {
        assert!(v.abs() < 1e-6, "{at:?}");
    }
}

#[test]
fn a_pin_goes_on_the_part_below_and_its_socket_above() {
    let (pieces, plane, at) = cut_block();
    let before: Vec<f64> = pieces.iter().map(volume).collect();
    // Pieces come the one along the normal first: above, then below.
    let joined = add_joints(&pieces, plane, &at, Joint::default_pin()).unwrap();
    assert_eq!((joined.made, joined.skipped), (1, 0));
    for piece in &joined.pieces {
        assert!(is_closed(piece));
    }
    let after: Vec<f64> = joined.pieces.iter().map(volume).collect();
    let pin = std::f64::consts::PI * 1.5 * 1.5 * 3.0;
    let socket = std::f64::consts::PI * 1.65 * 1.65 * 3.15;
    // A 48-sided round loses a little to the circle.
    assert!(
        (before[1] + pin - after[1]).abs() < pin * 0.02,
        "{before:?} {after:?}"
    );
    assert!(
        (before[0] - socket - after[0]).abs() < socket * 0.02,
        "{before:?} {after:?}"
    );
    // The socket is wider than the pin: it slides in.
    assert!(socket > pin);
}

#[test]
fn a_square_pin_and_magnet_pockets_come_out_closed() {
    let (pieces, plane, at) = cut_block();
    let square = Joint::Pin {
        shape: PinShape::Square,
        size_mm: 4.0,
        depth_mm: 2.0,
        clearance_mm: 0.2,
    };
    let joined = add_joints(&pieces, plane, &at, square).unwrap();
    assert_eq!(joined.made, 1);
    let after: Vec<f64> = joined.pieces.iter().map(volume).collect();
    assert!((after[1] - (4000.0 + 32.0)).abs() < 1e-3, "{after:?}");
    assert!(
        (after[0] - (4000.0 - 4.4 * 4.4 * 2.2)).abs() < 1e-3,
        "{after:?}"
    );

    let joined = add_joints(&pieces, plane, &at, Joint::default_magnet()).unwrap();
    assert_eq!(joined.made, 1);
    let pocket = std::f64::consts::PI * 1.6 * 1.6 * 2.1;
    for piece in &joined.pieces {
        assert!(is_closed(piece));
        assert!((4000.0 - pocket - volume(piece)).abs() < pocket * 0.02);
    }
}

#[test]
fn a_joint_too_wide_or_too_deep_for_the_part_is_left_out() {
    let (pieces, plane, at) = cut_block();
    // Wider than the section can hold with its wall.
    let wide = Joint::Magnet {
        diameter_mm: 20.0 - 2.0 * MIN_WALL_MM,
        height_mm: 1.0,
        clearance_mm: 0.1,
    };
    let joined = add_joints(&pieces, plane, &at, wide).unwrap();
    assert_eq!((joined.made, joined.skipped), (0, 1));
    // Deeper than the 10 mm half of the block leaves a floor for.
    let deep = Joint::Magnet {
        diameter_mm: 3.0,
        height_mm: 9.5,
        clearance_mm: 0.1,
    };
    let joined = add_joints(&pieces, plane, &at, deep).unwrap();
    assert_eq!((joined.made, joined.skipped), (0, 1));
    assert_eq!(volume(&joined.pieces[0]), volume(&pieces[0]));
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cuts along a stroke (#440) and lasso cuts (#441).

use manifold_rust::linalg::Vec3;
use manifold_rust::manifold::Manifold;
use sc_mesh_core::IndexedMesh;

use crate::stroke::Stroke;
use crate::{is_closed, stroke_cut, volume};

fn solid(m: &Manifold) -> IndexedMesh {
    let gl = m.get_mesh_gl(-1);
    let stride = gl.num_prop as usize;
    IndexedMesh::new(
        None,
        gl.vert_properties
            .chunks_exact(stride)
            .map(|p| sc_mesh_core::primitives::Vertex::new([p[0], p[1], p[2]]))
            .collect(),
        gl.tri_verts
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&t| sc_mesh_core::primitives::IndexedTriangle::from_raw_indices(t))
            .collect(),
    )
}

/// A 20 mm block centred on the origin.
fn block() -> IndexedMesh {
    solid(&Manifold::cube(Vec3::new(20.0, 20.0, 20.0), true))
}

/// A stroke seen from straight above.
fn from_above(points: Vec<[f64; 2]>, surface: Vec<[f64; 3]>, closed: bool) -> Stroke {
    let along = [0.0, 0.0, -1.0];
    let (u, v) = Stroke::frame(along);
    Stroke {
        origin: [0.0; 3],
        u,
        v,
        along,
        points,
        surface,
        closed,
    }
}

#[test]
fn a_straight_stroke_cuts_the_block_in_two_halves() {
    let mesh = block();
    let stroke = from_above(vec![[-15.0, 0.0], [15.0, 0.0]], Vec::new(), false);
    let cut = stroke_cut(&mesh, &stroke).unwrap();
    assert_eq!(cut.pieces.len(), 2);
    for piece in &cut.pieces {
        assert!(is_closed(piece));
        assert!((volume(piece) - 4000.0).abs() < 1e-3, "{}", volume(piece));
    }
}

#[test]
fn a_bent_stroke_follows_its_bend() {
    let mesh = block();
    // A V through the middle: its point 5 mm off the straight line.
    let stroke = from_above(
        vec![[-15.0, -5.0], [0.0, 5.0], [15.0, -5.0]],
        Vec::new(),
        false,
    );
    let cut = stroke_cut(&mesh, &stroke).unwrap();
    let (a, b) = (volume(&cut.pieces[0]), volume(&cut.pieces[1]));
    assert!((a + b - 8000.0).abs() < 1e-3);
    assert!(
        (a - 4000.0).abs() > 100.0,
        "not a straight halving: {a} {b}"
    );
    for piece in &cut.pieces {
        assert!(is_closed(piece));
    }
}

#[test]
fn a_lasso_cuts_off_the_bump_it_encloses() {
    // A 20 x 20 x 10 base with a 6 mm bump on top.
    let base = Manifold::cube(Vec3::new(20.0, 20.0, 10.0), true);
    let bump = Manifold::cube(Vec3::new(6.0, 6.0, 6.0), true).translate(Vec3::new(0.0, 0.0, 7.5));
    let mesh = solid(&base.union(&bump));
    let ring = [[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
    // Drawn on the base's top, round the bump.
    let surface = ring.iter().map(|p| [p[0], p[1], 5.0]).collect();
    let stroke = from_above(ring.to_vec(), surface, true);
    let cut = stroke_cut(&mesh, &stroke).unwrap();
    let off = volume(&cut.pieces[0]);
    assert!(
        (off - 6.0 * 6.0 * 5.5).abs() < 1e-3,
        "the bump above the base: {off}"
    );
    assert!((volume(&cut.pieces[1]) - 4000.0).abs() < 1e-3);
}

#[test]
fn a_stroke_that_misses_or_crosses_itself_cuts_nothing() {
    let mesh = block();
    let away = from_above(vec![[30.0, -15.0], [30.0, 15.0]], Vec::new(), false);
    assert!(stroke_cut(&mesh, &away).is_err());
    let knot = from_above(
        vec![
            [-15.0, 0.0],
            [5.0, 5.0],
            [5.0, -5.0],
            [-5.0, 5.0],
            [15.0, 0.0],
        ],
        Vec::new(),
        false,
    );
    assert!(stroke_cut(&mesh, &knot).is_err());
}

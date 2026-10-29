/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::{Body, Part, Placement, Plate, Settings, arrange};

fn cube() -> Arc<IndexedMesh> {
    Arc::new(sc_mesh_fixtures::unit_cube_mesh())
}

/// A box `size` mm with its low corner at `at` (the unit cube is centred on
/// the origin).
fn boxed(mesh: &Arc<IndexedMesh>, size: [f64; 3], at: [f64; 3]) -> Part {
    Part {
        mesh: mesh.clone(),
        world: AffineTransform3D::from_linear_and_translation(
            [
                [size[0], 0.0, 0.0],
                [0.0, size[1], 0.0],
                [0.0, 0.0, size[2]],
            ],
            std::array::from_fn(|k| at[k] + size[k] * 0.5),
        ),
    }
}

fn body(parts: Vec<Part>) -> Body {
    Body {
        parts,
        fixed: false,
    }
}

fn plate(w: f64, h: f64) -> Plate {
    Plate {
        min: [0.0, 0.0],
        max: [w, h],
        floor: 0.0,
    }
}

/// The world corners of a body's parts once `placement` moves it.
fn corners(body: &Body, placement: Option<Placement>) -> Vec<[f64; 3]> {
    let motion = placement.map_or(AffineTransform3D::identity(), |p| p.motion());
    body.parts
        .iter()
        .flat_map(|part| {
            let world = motion.compose(&part.world);
            (0..8).map(move |k| {
                let v = sc_mesh_core::Vertex::new([
                    (k & 1) as f32 - 0.5,
                    (k >> 1 & 1) as f32 - 0.5,
                    (k >> 2 & 1) as f32 - 0.5,
                ]);
                let p = world.apply_to_vertex(v).coords();
                [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
            })
        })
        .collect()
}

/// Each moved part's box, `[lo, hi]`.
fn boxes(body: &Body, placement: Option<Placement>) -> Vec<([f64; 3], [f64; 3])> {
    corners(body, placement)
        .chunks(8)
        .map(|c| {
            c.iter()
                .fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), p| {
                    (
                        std::array::from_fn(|k| lo[k].min(p[k])),
                        std::array::from_fn(|k| hi[k].max(p[k])),
                    )
                })
        })
        .collect()
}

/// The distance between two boxes, 0 when they overlap.
fn apart(a: &([f64; 3], [f64; 3]), b: &([f64; 3], [f64; 3])) -> f64 {
    let d: Vec<f64> = (0..3)
        .map(|k| (a.0[k] - b.1[k]).max(b.0[k] - a.1[k]).max(0.0))
        .collect();
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn quarter_turns() -> Settings {
    Settings {
        turns: 4,
        ..Settings::default()
    }
}

/// Every part of every body at least `gap` from every part of every other
/// body, and inside the plate's margin.
fn assert_kept_apart(
    bodies: &[Body],
    placements: &[Option<Placement>],
    plate: &Plate,
    s: &Settings,
) {
    let moved: Vec<Vec<([f64; 3], [f64; 3])>> = bodies
        .iter()
        .zip(placements)
        .map(|(b, p)| boxes(b, *p))
        .collect();
    for (i, a) in moved.iter().enumerate() {
        if placements[i].is_some_and(|p| p.on_plate) {
            for part in a {
                for k in 0..2 {
                    assert!(part.0[k] >= plate.min[k] + s.margin - 1e-6, "{part:?}");
                    assert!(part.1[k] <= plate.max[k] - s.margin + 1e-6, "{part:?}");
                }
            }
        }
        for b in &moved[i + 1..] {
            for p in a {
                for q in b {
                    assert!(apart(p, q) >= s.gap - 1e-6, "{p:?} {q:?}");
                }
            }
        }
    }
}

#[test]
fn boxes_keep_the_gap_and_the_margin() {
    let mesh = cube();
    let bodies: Vec<Body> = (0..6)
        .map(|i| {
            body(vec![boxed(
                &mesh,
                [10.0, 6.0, 8.0],
                [f64::from(i) * 3.0, 0.0, 0.0],
            )])
        })
        .collect();
    let plate = plate(60.0, 40.0);
    let settings = quarter_turns();
    let out = arrange(&bodies, &plate, &settings, &AtomicBool::new(false)).unwrap();
    assert!(out.left_off().is_empty(), "{:?}", out.placements);
    assert_kept_apart(&bodies, &out.placements, &plate, &settings);
    // Packed: 6 boxes of 10 × 6 with a 1 mm gap take about 4 × 3 cm.
    let all: Vec<_> = bodies
        .iter()
        .zip(&out.placements)
        .flat_map(|(b, p)| boxes(b, *p))
        .collect();
    let span = |k: usize| {
        all.iter().map(|b| b.1[k]).fold(f64::MIN, f64::max)
            - all.iter().map(|b| b.0[k]).fold(f64::MAX, f64::min)
    };
    assert!(span(0) * span(1) < 34.0 * 22.0, "{} × {}", span(0), span(1));
}

/// A table: a leg in one corner and a top 18 mm up. In bands, a short box
/// fits under the top; by whole shadows it does not fit on a plate barely
/// larger than the table.
#[test]
fn a_short_body_may_stand_under_an_overhang() {
    let mesh = cube();
    let table = body(vec![
        boxed(&mesh, [4.0, 4.0, 18.0], [0.0, 0.0, 0.0]),
        boxed(&mesh, [20.0, 20.0, 2.0], [0.0, 0.0, 18.0]),
    ]);
    let short = body(vec![boxed(&mesh, [8.0, 8.0, 8.0], [40.0, 0.0, 0.0])]);
    let bodies = [table, short];
    let plate = plate(32.0, 32.0);
    let no_turns = Settings {
        turns: 1,
        ..Settings::default()
    };
    let banded = arrange(&bodies, &plate, &no_turns, &AtomicBool::new(false)).unwrap();
    assert!(banded.left_off().is_empty(), "{:?}", banded.placements);
    assert_kept_apart(&bodies, &banded.placements, &plate, &no_turns);
    // The short box's shadow is under the table's top.
    let top = boxes(&bodies[0], banded.placements[0])[1];
    let under = boxes(&bodies[1], banded.placements[1])[0];
    assert!(
        under.0[0] < top.1[0]
            && under.1[0] > top.0[0]
            && under.0[1] < top.1[1]
            && under.1[1] > top.0[1],
        "{top:?} {under:?}"
    );

    let shadows = Settings {
        shadow_only: true,
        ..no_turns
    };
    let whole = arrange(&bodies, &plate, &shadows, &AtomicBool::new(false)).unwrap();
    assert_eq!(whole.left_off().len(), 1, "{:?}", whole.placements);
}

#[test]
fn a_body_that_stays_is_kept_clear_of() {
    let mesh = cube();
    let mut kept = body(vec![boxed(&mesh, [10.0, 10.0, 5.0], [25.0, 15.0, 0.0])]);
    kept.fixed = true;
    let bodies = [
        kept,
        body(vec![boxed(&mesh, [10.0, 10.0, 5.0], [0.0, 0.0, 0.0])]),
        body(vec![boxed(&mesh, [10.0, 10.0, 5.0], [0.0, 0.0, 0.0])]),
    ];
    let plate = plate(60.0, 40.0);
    let settings = quarter_turns();
    let out = arrange(&bodies, &plate, &settings, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.placements[0], None, "it stays");
    assert!(out.left_off().is_empty());
    assert_kept_apart(&bodies, &out.placements, &plate, &settings);
}

#[test]
fn what_does_not_fit_goes_beside_the_plate() {
    let mesh = cube();
    let bodies: Vec<Body> = (0..5)
        .map(|_| body(vec![boxed(&mesh, [11.0, 11.0, 4.0], [0.0, 0.0, 0.0])]))
        .collect();
    let plate = plate(25.0, 25.0);
    let settings = Settings {
        margin: 0.0,
        ..quarter_turns()
    };
    let out = arrange(&bodies, &plate, &settings, &AtomicBool::new(false)).unwrap();
    let off = out.left_off();
    assert_eq!(off.len(), 1, "{:?}", out.placements);
    assert_kept_apart(&bodies, &out.placements, &plate, &settings);
    let parked = boxes(&bodies[off[0]], out.placements[off[0]])[0];
    assert!(
        parked.0[0] >= plate.max[0] + 1.0,
        "beside the plate: {parked:?}"
    );
}

#[test]
fn the_same_scene_arranges_the_same() {
    let mesh = cube();
    let bodies: Vec<Body> = (0..7)
        .map(|i| {
            let f = f64::from(i);
            body(vec![boxed(
                &mesh,
                [4.0 + f, 9.0 - f, 3.0 + f],
                [f * 12.0, 0.0, 0.0],
            )])
        })
        .collect();
    let plate = plate(70.0, 50.0);
    let settings = Settings {
        time_limit: std::time::Duration::from_secs(30),
        ..Settings::default()
    };
    let a = arrange(&bodies, &plate, &settings, &AtomicBool::new(false)).unwrap();
    let b = arrange(&bodies, &plate, &settings, &AtomicBool::new(false)).unwrap();
    assert_eq!(a.placements, b.placements);
    assert_kept_apart(&bodies, &a.placements, &plate, &settings);
}

#[test]
fn a_cancelled_arrangement_says_so() {
    let mesh = cube();
    let bodies = [body(vec![boxed(&mesh, [5.0, 5.0, 5.0], [0.0, 0.0, 0.0])])];
    let out = arrange(
        &bodies,
        &plate(40.0, 40.0),
        &Settings::default(),
        &AtomicBool::new(true),
    );
    assert_eq!(out, Err(crate::Cancelled));
}

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

/// #356: the bodies with the copies `fill` placed, each copy as its source
/// moved, and the placements to check them with.
fn filled_plate(bodies: &[Body], filled: &crate::Filled) -> (Vec<Body>, Vec<Option<Placement>>) {
    let mut all: Vec<Body> = bodies.to_vec();
    let mut placements: Vec<Option<Placement>> = vec![None; bodies.len()];
    for moved in &filled.moved {
        placements[moved.source] = Some(moved.placement);
    }
    for copy in &filled.copies {
        all.push(bodies[copy.source].clone());
        placements.push(Some(copy.placement));
    }
    (all, placements)
}

fn fill_now(
    bodies: &[Body],
    sources: &[usize],
    plate: &Plate,
    settings: &Settings,
    limit: Option<usize>,
) -> crate::Filled {
    let placed = std::sync::atomic::AtomicUsize::new(0);
    let filled = crate::fill(
        bodies,
        sources,
        plate,
        settings,
        limit,
        &placed,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        placed.load(std::sync::atomic::Ordering::Relaxed) >= filled.copies.len(),
        "the progress saw every copy"
    );
    filled
}

/// One box in the plate's corner: copies fill the rest of the plate in
/// rows, kept apart from it, from each other and from the edge.
#[test]
fn a_fill_puts_as_many_copies_as_fit() {
    let mesh = cube();
    // 8.5 mm boxes and a 1 mm gap: 4 × 4 fit in the 40 mm inside the margin.
    let bodies = [body(vec![boxed(&mesh, [8.5, 8.5, 6.0], [5.0, 5.0, 0.0])])];
    let plate = plate(50.0, 50.0);
    let settings = quarter_turns();
    let filled = fill_now(&bodies, &[0], &plate, &settings, None);
    assert_eq!(filled.copies.len(), 15, "{:?}", filled.copies);
    assert!(filled.copies.iter().all(|c| c.source == 0));
    let (all, placements) = filled_plate(&bodies, &filled);
    assert_kept_apart(&all, &placements, &plate, &settings);

    // At most as many as asked for.
    let some = fill_now(&bodies, &[0], &plate, &settings, Some(4));
    assert_eq!(some.copies.len(), 4);
    let (all, placements) = filled_plate(&bodies, &some);
    assert_kept_apart(&all, &placements, &plate, &settings);
}

/// What is not copied stays where it is, and the copies keep clear of it,
/// at every height: in bands, a short copy may stand under an overhang.
#[test]
fn a_fill_keeps_clear_of_what_stays() {
    let mesh = cube();
    let table = body(vec![
        boxed(&mesh, [4.0, 4.0, 18.0], [20.0, 12.0, 0.0]),
        boxed(&mesh, [20.0, 16.0, 2.0], [20.0, 12.0, 18.0]),
    ]);
    let other = body(vec![boxed(&mesh, [6.0, 20.0, 9.0], [45.0, 6.0, 0.0])]);
    let short = body(vec![boxed(&mesh, [5.0, 5.0, 5.0], [6.0, 6.0, 0.0])]);
    let bodies = [table, other, short];
    let plate = plate(60.0, 40.0);
    let settings = quarter_turns();
    let filled = fill_now(&bodies, &[2], &plate, &settings, None);
    assert!(filled.copies.len() > 10, "{}", filled.copies.len());
    assert!(filled.copies.iter().all(|c| c.source == 2));
    let (all, placements) = filled_plate(&bodies, &filled);
    assert_kept_apart(&all, &placements, &plate, &settings);
    // Some copy stands under the table's top.
    let top = boxes(&bodies[0], None)[1];
    let under = filled.copies.iter().any(|c| {
        let b = boxes(&bodies[2], Some(c.placement))[0];
        b.0[0] < top.1[0] && b.1[0] > top.0[0] && b.0[1] < top.1[1] && b.1[1] > top.0[1]
    });
    assert!(under, "a copy under the overhang");
}

/// Two sources: copies of both, mixed, as many of one as of the other,
/// give or take one.
#[test]
fn a_fill_of_two_bodies_mixes_them_evenly() {
    let mesh = cube();
    let bodies = [
        body(vec![boxed(&mesh, [12.0, 7.0, 6.0], [5.0, 5.0, 0.0])]),
        body(vec![boxed(&mesh, [5.0, 5.0, 12.0], [20.0, 5.0, 0.0])]),
    ];
    let plate = plate(80.0, 60.0);
    let settings = quarter_turns();
    let filled = fill_now(&bodies, &[0, 1], &plate, &settings, None);
    let count = |s: usize| filled.copies.iter().filter(|c| c.source == s).count();
    let (a, b) = (count(0), count(1));
    assert!(a > 5 && b > 5, "{a} and {b}");
    assert!(a.abs_diff(b) <= 1, "{a} and {b}");
    let (all, placements) = filled_plate(&bodies, &filled);
    assert_kept_apart(&all, &placements, &plate, &settings);
}

/// A source in the plate's middle splits it: no row fits above or below
/// it. Moved, it lets the copies in rows and columns, as on a tray; what
/// is not copied still stays.
#[test]
fn a_source_in_the_middle_moves_to_let_more_copies_in() {
    let mesh = cube();
    let plate = plate(70.0, 36.0);
    let bodies = [
        body(vec![boxed(&mesh, [12.0, 12.0, 6.0], [29.0, 12.0, 0.0])]),
        body(vec![boxed(&mesh, [3.0, 3.0, 3.0], [64.0, 2.0, 0.0])]),
    ];
    let settings = quarter_turns();
    let filled = fill_now(&bodies, &[0], &plate, &settings, None);
    assert_eq!(filled.moved.len(), 1, "the source moves");
    assert_eq!(filled.moved[0].source, 0);
    // 12 mm boxes and a 1 mm gap: 4 × 2 in the 60 × 26 mm inside the margin.
    assert_eq!(filled.copies.len(), 7, "{:?}", filled.copies);
    let (all, placements) = filled_plate(&bodies, &filled);
    assert_eq!(placements[1], None, "the other one stays");
    assert_kept_apart(&all, &placements, &plate, &settings);

    // Where staying fits as many, the source stays.
    let corner = [body(vec![boxed(&mesh, [12.0, 12.0, 6.0], [5.0, 5.0, 0.0])])];
    let filled = fill_now(&corner, &[0], &plate, &settings, None);
    assert!(filled.moved.is_empty());
    assert_eq!(filled.copies.len(), 7);
}

#[test]
fn a_full_plate_takes_no_copy() {
    let mesh = cube();
    let bodies = [body(vec![boxed(&mesh, [30.0, 30.0, 5.0], [5.0, 5.0, 0.0])])];
    let filled = fill_now(&bodies, &[0], &plate(45.0, 45.0), &quarter_turns(), None);
    assert!(filled.copies.is_empty(), "{:?}", filled.copies);
}

#[test]
fn a_cancelled_fill_says_so() {
    let mesh = cube();
    let bodies = [body(vec![boxed(&mesh, [5.0, 5.0, 5.0], [5.0, 5.0, 0.0])])];
    let out = crate::fill(
        &bodies,
        &[0],
        &plate(40.0, 40.0),
        &Settings::default(),
        None,
        &std::sync::atomic::AtomicUsize::new(0),
        &AtomicBool::new(true),
    );
    assert_eq!(out, Err(crate::Cancelled));
}

/// Plates `w` × `h` mm side by side, 40 mm apart.
fn plates_in_a_row(count: usize, w: f64, h: f64) -> Vec<Plate> {
    (0..count)
        .map(|k| {
            let x = k as f64 * (w + 40.0);
            Plate {
                min: [x, 0.0],
                max: [x + w, h],
                floor: 0.0,
            }
        })
        .collect()
}

/// Whether the box of every part of `body`, moved by `p`, lies on `plate`.
fn lies_on(body: &Body, p: Placement, plate: &Plate) -> bool {
    boxes(body, Some(p)).iter().all(|(lo, hi)| {
        lo[0] >= plate.min[0] - 1e-6
            && lo[1] >= plate.min[1] - 1e-6
            && hi[0] <= plate.max[0] + 1e-6
            && hi[1] <= plate.max[1] + 1e-6
    })
}

#[test]
fn what_does_not_fit_on_a_plate_goes_on_the_next() {
    let mesh = cube();
    // A 60 × 60 plate with a 5 mm margin takes 4 boxes of 20 mm.
    let bodies: Vec<Body> = (0..10)
        .map(|k| {
            body(vec![boxed(
                &mesh,
                [20.0, 20.0, 5.0],
                [k as f64 * 25.0, 0.0, 0.0],
            )])
        })
        .collect();
    let plates = plates_in_a_row(8, 60.0, 60.0);
    let settings = Settings {
        turns: 1,
        ..Settings::default()
    };
    let across = crate::arrange_across(
        &bodies,
        &vec![None; bodies.len()],
        &plates,
        &settings,
        &AtomicBool::new(false),
    )
    .expect("not cancelled");
    assert_eq!(across.plates_used, 3, "4 + 4 + 2");
    assert!(across.left_off().is_empty());
    let mut on = [0; 8];
    for (b, placed) in bodies.iter().zip(&across.placements) {
        let (p, placement) = placed.expect("placed");
        assert!(lies_on(b, placement, &plates[p]), "on its plate");
        on[p] += 1;
    }
    assert_eq!(&on[..3], &[4, 4, 2]);
}

#[test]
fn a_body_that_stays_keeps_its_plate_and_the_others_keep_clear() {
    let mesh = cube();
    let plates = plates_in_a_row(4, 60.0, 60.0);
    // A 40 mm box standing on the second plate, and 3 of 20 mm to place.
    let mut bodies = vec![Body {
        parts: vec![boxed(&mesh, [40.0, 40.0, 5.0], [110.0, 10.0, 0.0])],
        fixed: true,
    }];
    bodies.extend((0..6).map(|k| {
        body(vec![boxed(
            &mesh,
            [20.0, 20.0, 5.0],
            [k as f64 * 25.0, 0.0, 0.0],
        )])
    }));
    let mut stands_on = vec![None; bodies.len()];
    stands_on[0] = Some(1);
    let across = crate::arrange_across(
        &bodies,
        &stands_on,
        &plates,
        &Settings {
            turns: 1,
            ..Settings::default()
        },
        &AtomicBool::new(false),
    )
    .expect("not cancelled");
    assert!(across.placements[0].is_none(), "it stays");
    let fixed = boxes(&bodies[0], None)[0];
    let mut on = [0; 4];
    for (b, placed) in bodies.iter().zip(&across.placements).skip(1) {
        let (p, placement) = placed.expect("placed");
        assert!(lies_on(b, placement, &plates[p]));
        let (lo, hi) = boxes(b, Some(placement))[0];
        let apart = lo[0] >= fixed.1[0] + 1.0 - 1e-6
            || hi[0] <= fixed.0[0] - 1.0 + 1e-6
            || lo[1] >= fixed.1[1] + 1.0 - 1e-6
            || hi[1] <= fixed.0[1] - 1.0 + 1e-6;
        assert!(apart, "clear of the box that stays");
        on[p] += 1;
    }
    // The first plate takes 4; the second, with the 40 mm box, none.
    assert_eq!(on, [4, 0, 2, 0]);
    assert_eq!(across.plates_used, 3);
}

#[test]
fn a_body_too_big_for_any_plate_waits_beside_the_last_plate_used() {
    let mesh = cube();
    let bodies = vec![
        body(vec![boxed(&mesh, [20.0, 20.0, 5.0], [0.0, 0.0, 0.0])]),
        body(vec![boxed(&mesh, [90.0, 20.0, 5.0], [0.0, 40.0, 0.0])]),
    ];
    let plates = plates_in_a_row(4, 60.0, 60.0);
    let across = crate::arrange_across(
        &bodies,
        &[None; 2],
        &plates,
        &Settings::default(),
        &AtomicBool::new(false),
    )
    .expect("not cancelled");
    assert_eq!(across.left_off(), vec![1]);
    assert_eq!(across.plates_used, 1);
    assert_eq!(across.placements[1].map(|(p, _)| p), Some(0));
}

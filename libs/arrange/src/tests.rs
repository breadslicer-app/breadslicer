/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};

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
    let placed = AtomicUsize::new(0);
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
        &AtomicUsize::new(0),
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

/// A box `size` mm turned `turn` radians about the vertical, its middle at
/// `at` (on the plate).
fn turned_box(mesh: &Arc<IndexedMesh>, size: [f64; 3], turn: f64, at: [f64; 2]) -> Part {
    let (s, c) = turn.sin_cos();
    Part {
        mesh: mesh.clone(),
        world: AffineTransform3D::from_linear_and_translation(
            [
                [c * size[0], -s * size[1], 0.0],
                [s * size[0], c * size[1], 0.0],
                [0.0, 0.0, size[2]],
            ],
            [at[0], at[1], size[2] * 0.5],
        ),
    }
}

/// A body moved and turned about the vertical keeps the bands drawn for it
/// (#560); scaled, it is drawn again.
#[test]
fn a_body_moved_and_turned_keeps_its_bands() {
    let mesh = cube();
    let plate = plate(80.0, 80.0);
    let cancel = AtomicBool::new(false);
    let run = |part: Part| {
        arrange(&[body(vec![part])], &plate, &Settings::default(), &cancel).expect("arranged");
    };
    run(turned_box(&mesh, [10.0, 6.0, 4.0], 0.0, [20.0, 20.0]));
    assert_eq!(crate::cache::kept_parts_of(&mesh), 1);
    run(turned_box(&mesh, [10.0, 6.0, 4.0], 0.7, [53.3, 31.9]));
    assert_eq!(crate::cache::kept_parts_of(&mesh), 1, "the same bands");
    run(turned_box(&mesh, [10.0, 6.0, 5.0], 0.7, [53.3, 31.9]));
    assert_eq!(crate::cache::kept_parts_of(&mesh), 2, "taller: drawn again");
}

/// A body that stands turned is placed as it stands: kept bands are turned
/// the way the body is (#560). Bars of 20 × 4 standing upright, four wide
/// by twenty deep, fit five in a row of 27 mm; lying, they would fit four.
#[test]
fn a_body_standing_turned_is_placed_as_it_stands() {
    let mesh = cube();
    let plate = plate(37.0, 32.0);
    let s = Settings {
        turns: 1,
        ..Settings::default()
    };
    let bodies: Vec<Body> = (0..6)
        .map(|k| {
            body(vec![turned_box(
                &mesh,
                [20.0, 4.0, 3.0],
                std::f64::consts::FRAC_PI_2,
                [100.0 + 7.0 * k as f64, 100.0],
            )])
        })
        .collect();
    let outcome = arrange(&bodies, &plate, &s, &AtomicBool::new(false)).expect("arranged");
    let on = outcome
        .placements
        .iter()
        .filter(|p| p.is_some_and(|p| p.on_plate))
        .count();
    assert_eq!(on, 5);
    let placed: Vec<Option<Placement>> = outcome
        .placements
        .iter()
        .map(|p| p.filter(|p| p.on_plate))
        .collect();
    let (kept, at): (Vec<Body>, Vec<Option<Placement>>) = bodies
        .iter()
        .cloned()
        .zip(placed)
        .filter(|(_, p)| p.is_some())
        .unzip();
    assert_kept_apart(&kept, &at, &plate, &s);
}

/// Drawn ahead, the bands are there when the arrangement comes, once for
/// copies of a body (#560).
#[test]
fn bands_drawn_ahead_are_not_drawn_again() {
    let mesh = cube();
    let plate = plate(80.0, 80.0);
    let cancel = AtomicBool::new(false);
    let bodies: Vec<Body> = (0..4)
        .map(|k| {
            body(vec![turned_box(
                &mesh,
                [7.0, 5.0, 3.0],
                0.3 * k as f64,
                [10.0 + 15.0 * k as f64, 10.0],
            )])
        })
        .collect();
    assert_eq!(
        crate::warm(&bodies, &plate, &Settings::default(), &cancel),
        1
    );
    assert_eq!(
        crate::warm(&bodies, &plate, &Settings::default(), &cancel),
        0
    );
    arrange(&bodies, &plate, &Settings::default(), &cancel).expect("arranged");
    assert_eq!(crate::cache::kept_parts_of(&mesh), 1);
}

/// Copies of a source whose snapped reference lies off its middle keep the
/// gap at every turn (#560): turned, that offset turns with them. Cubes of
/// 4 mm keep 1 mm when their middles are 5 mm apart along an axis, or the
/// cubes' circles 1 mm apart.
#[test]
fn copies_turned_keep_the_gap() {
    let bodies: Vec<Body> = [[10.0, 10.0], [25.0, 20.0], [40.0, 30.0]]
        .iter()
        .map(|&[x, y]| {
            body(vec![boxed(
                &cube(),
                [4.0, 4.0, 4.0],
                [x - 2.0, y - 2.0, 0.0],
            )])
        })
        .collect();
    let plate = plate(50.0, 40.0);
    let filled = fill_now(&bodies, &[0], &plate, &Settings::default(), None);
    assert!(filled.copies.len() > 10, "{}", filled.copies.len());
    let (all, placements) = filled_plate(&bodies, &filled);
    let middles: Vec<[f64; 2]> = all
        .iter()
        .zip(&placements)
        .map(|(b, p)| {
            let c = corners(b, *p);
            let n = c.len() as f64;
            [
                c.iter().map(|q| q[0]).sum::<f64>() / n,
                c.iter().map(|q| q[1]).sum::<f64>() / n,
            ]
        })
        .collect();
    for (i, a) in middles.iter().enumerate() {
        for b in &middles[i + 1..] {
            let (dx, dy) = ((a[0] - b[0]).abs(), (a[1] - b[1]).abs());
            assert!(
                dx.max(dy) >= 5.0 - 1e-6
                    || dx.hypot(dy) >= 4.0 * std::f64::consts::SQRT_2 + 1.0 - 1e-6,
                "{a:?} {b:?}"
            );
        }
    }
}

/// How long Arrange all takes on copies of a real model the first time and
/// again after they moved (#560). `ARRANGE_BENCH_STL` names the model,
/// `ARRANGE_BENCH_COPIES` how many (12), each with a mesh of its own.
#[test]
#[ignore = "a measurement on a real model"]
fn kept_footprints_on_a_real_model() {
    let Some(path) = std::env::var_os("ARRANGE_BENCH_STL") else {
        return;
    };
    let copies: usize = std::env::var("ARRANGE_BENCH_COPIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let loaded = sc_mesh_formats::load_mesh_from_path(std::path::Path::new(&path), None, None)
        .expect("loads");
    let graph = loaded.graph;
    let mesh = graph
        .roots()
        .iter()
        .find_map(|&r| graph.leaf_mesh_arc(r))
        .expect("a mesh")
        .clone();
    let bodies: Vec<Body> = (0..copies)
        .map(|k| {
            let own = Arc::new((*mesh).clone());
            let (s, c) = (0.4 * k as f64).sin_cos();
            body(vec![Part {
                mesh: own,
                world: AffineTransform3D::from_linear_and_translation(
                    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
                    [400.0 + 60.0 * k as f64, 0.0, 0.0],
                ),
            }])
        })
        .collect();
    let plates = plates_in_a_row(6, 223.642, 126.976);
    let stands_on = vec![None; bodies.len()];
    let cancel = AtomicBool::new(false);
    let s = Settings::default();
    let first = crate::arrange_across(&bodies, &stands_on, &plates, &s, &cancel).unwrap();
    // The bodies where the first run put them, then arranged again.
    let moved: Vec<Body> = bodies
        .iter()
        .zip(&first.placements)
        .map(|(b, p)| {
            let motion = p.map_or(AffineTransform3D::identity(), |(_, p)| p.motion());
            Body {
                parts: b
                    .parts
                    .iter()
                    .map(|part| Part {
                        mesh: part.mesh.clone(),
                        world: motion.compose(&part.world),
                    })
                    .collect(),
                fixed: false,
            }
        })
        .collect();
    let again = crate::arrange_across(&moved, &stands_on, &plates, &s, &cancel).unwrap();
    println!(
        "{copies} copies: first {:.2} s footprints + {:.2} s search on {} plate(s); again {:.2} s + {:.2} s on {}",
        first.timings.footprints.as_secs_f64(),
        first.timings.search.as_secs_f64(),
        first.plates_used,
        again.timings.footprints.as_secs_f64(),
        again.timings.search.as_secs_f64(),
        again.plates_used,
    );
}

/// How many copies of a real model Fill bed and Arrange all put on an M7 Pro
/// plate, as the app asks (#555). `ARRANGE_BENCH_STL` names the model,
/// `ARRANGE_BENCH_COPIES` how many copies Arrange all gets (40),
/// `ARRANGE_BENCH_TIGHT=1` packs as under one raft for all.
#[test]
#[ignore = "a measurement on a real model"]
fn copies_of_a_real_model_on_a_plate() {
    let Some(path) = std::env::var_os("ARRANGE_BENCH_STL") else {
        return;
    };
    let copies: usize = std::env::var("ARRANGE_BENCH_COPIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let tight = std::env::var("ARRANGE_BENCH_TIGHT").is_ok_and(|v| v == "1");
    let loaded = sc_mesh_formats::load_mesh_from_path(std::path::Path::new(&path), None, None)
        .expect("loads");
    let mesh = Arc::new(loaded.graph.merge_meshes().expect("merge").expect("a mesh"));
    let floor = mesh
        .vertices()
        .iter()
        .map(|v| f64::from(v.coords()[2]))
        .fold(f64::INFINITY, f64::min);
    let at = |x: f64, y: f64| Part {
        mesh: mesh.clone(),
        world: AffineTransform3D::from_translation([x, y, -floor]),
    };
    let s = Settings {
        margin: 2.0,
        tight,
        ..Settings::default()
    };
    let (w, h) = (223.642, 126.976);
    let one = plate(w, h);
    let cancel = AtomicBool::new(false);
    let placed = AtomicUsize::new(0);
    let started = std::time::Instant::now();
    let filled = crate::fill(
        &[body(vec![at(w / 2.0, h / 2.0)])],
        &[0],
        &one,
        &s,
        None,
        &placed,
        &cancel,
    )
    .unwrap();
    if let Some(dump) = std::env::var_os("ARRANGE_BENCH_DUMP") {
        let mut out = format!(
            "{} {} {}
",
            w / 2.0,
            h / 2.0,
            -floor
        );
        let moved = filled.moved.first().map(|m| m.placement);
        for p in moved
            .iter()
            .chain(filled.copies.iter().map(|c| &c.placement))
        {
            out += &format!(
                "{} {} {}
",
                p.angle, p.translation[0], p.translation[1]
            );
        }
        std::fs::write(dump, out).unwrap();
    }
    println!(
        "fill: 1 + {} copies in {:.2} s (footprints {:.2} s)",
        filled.copies.len(),
        started.elapsed().as_secs_f64(),
        filled.timings.footprints.as_secs_f64(),
    );
    let bodies: Vec<Body> = (0..copies)
        .map(|k| body(vec![at(400.0 + 60.0 * k as f64, 0.0)]))
        .collect();
    // Or a mix of models, `ARRANGE_BENCH_LIST` naming one per line, each
    // `ARRANGE_BENCH_COPIES` times.
    let bodies: Vec<Body> = match std::env::var_os("ARRANGE_BENCH_LIST") {
        None => bodies,
        Some(list) => std::fs::read_to_string(list)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .flat_map(|l| {
                let loaded = sc_mesh_formats::load_mesh_from_path(
                    std::path::Path::new(l.trim()),
                    None,
                    None,
                )
                .expect("loads");
                let mesh = Arc::new(loaded.graph.merge_meshes().expect("merge").expect("a mesh"));
                let floor = mesh
                    .vertices()
                    .iter()
                    .map(|v| f64::from(v.coords()[2]))
                    .fold(f64::INFINITY, f64::min);
                (0..copies).map(move |_| {
                    body(vec![Part {
                        mesh: mesh.clone(),
                        world: AffineTransform3D::from_translation([-500.0, 0.0, -floor]),
                    }])
                })
            })
            .collect(),
    };
    let copies = bodies.len();
    let plates = plates_in_a_row(copies, w, h);
    let started = std::time::Instant::now();
    let across = crate::arrange_across(&bodies, &vec![None; copies], &plates, &s, &cancel).unwrap();
    let off = Settings {
        squeeze: false,
        ..s
    };
    let alone =
        crate::arrange_across(&bodies, &vec![None; copies], &plates, &off, &cancel).unwrap();
    let mut was = vec![0usize; alone.plates_used];
    for p in alone.placements.iter().flatten() {
        was[p.0] += 1;
    }
    println!("without squeezing: {} plates {was:?}", alone.plates_used);
    let mut per = vec![0usize; across.plates_used];
    for p in across.placements.iter().flatten() {
        per[p.0] += 1;
    }
    println!(
        "arrange all {copies}: {} plates {per:?} in {:.2} s (footprints {:.2} s)",
        across.plates_used,
        started.elapsed().as_secs_f64(),
        across.timings.footprints.as_secs_f64(),
    );
}

/// The spread an arrangement reports is the spread of its bodies once they
/// stand where it put them (#560), so the two can be weighed; scattered
/// bodies are more spread out than arranged.
#[test]
fn an_arrangement_s_spread_is_what_its_bodies_measure_as_they_stand() {
    let mesh = cube();
    let plate = plate(120.0, 80.0);
    let bodies: Vec<Body> = [[8.0, 8.0], [100.0, 60.0], [10.0, 62.0], [95.0, 9.0]]
        .iter()
        .map(|&[x, y]| body(vec![boxed(&mesh, [9.0, 6.0, 4.0], [x, y, 0.0])]))
        .collect();
    let s = Settings::default();
    let on = vec![Some(0); bodies.len()];
    let scattered = crate::spread_now(&bodies, &on, &[plate], &s);
    let across =
        crate::arrange_across(&bodies, &[None; 4], &[plate], &s, &AtomicBool::new(false)).unwrap();
    let moved: Vec<Body> = bodies
        .iter()
        .zip(&across.placements)
        .map(|(b, p)| {
            let motion = p.map_or(AffineTransform3D::identity(), |(_, p)| p.motion());
            Body {
                parts: b
                    .parts
                    .iter()
                    .map(|part| Part {
                        mesh: part.mesh.clone(),
                        world: motion.compose(&part.world),
                    })
                    .collect(),
                fixed: false,
            }
        })
        .collect();
    let arranged = crate::spread_now(&moved, &on, &[plate], &s);
    assert!(
        across.spread < scattered / 2.0,
        "{} {scattered}",
        across.spread
    );
    assert!(
        (arranged - across.spread).abs() <= across.spread * 0.05,
        "{arranged} {}",
        across.spread
    );
}

/// One raft for all (#556): what stands under `free_below` takes no room,
/// so bodies on wide rafts pack by what stands on them, their rafts lying
/// on each other, and what stands above keeps the gap.
#[test]
fn under_one_raft_for_all_the_rafts_take_no_room() {
    let mesh = cube();
    let on_raft = |i: u32| {
        let at = f64::from(i) * 12.0;
        body(vec![
            boxed(&mesh, [10.0, 10.0, 0.8], [at, 0.0, 0.0]),
            boxed(&mesh, [4.0, 4.0, 10.0], [at + 3.0, 3.0, 0.8]),
        ])
    };
    let bodies: Vec<Body> = (0..6).map(on_raft).collect();
    let plate = plate(40.0, 30.0);
    let apart = Settings {
        gap: 2.0,
        ..quarter_turns()
    };
    let raft_by_raft = arrange(&bodies, &plate, &apart, &AtomicBool::new(false)).unwrap();
    assert!(
        !raft_by_raft.left_off().is_empty(),
        "{:?}",
        raft_by_raft.placements
    );

    let one_raft = Settings {
        free_below: 0.85,
        ..apart
    };
    let out = arrange(&bodies, &plate, &one_raft, &AtomicBool::new(false)).unwrap();
    assert!(out.left_off().is_empty(), "{:?}", out.placements);
    let pillars: Vec<Body> = bodies
        .iter()
        .map(|b| body(vec![b.parts[1].clone()]))
        .collect();
    assert_kept_apart(&pillars, &out.placements, &plate, &one_raft);
}

/// An L of two bars, 12 × 4 mm each, turned by `turn` about its corner at
/// `at`.
fn l_body(mesh: &Arc<IndexedMesh>, turn: f64, at: [f64; 2]) -> Body {
    let (s, c) = turn.sin_cos();
    let bar = |size: [f64; 3], middle: [f64; 2]| {
        let p = [c * middle[0] - s * middle[1], s * middle[0] + c * middle[1]];
        turned_box(mesh, size, turn, [at[0] + p[0], at[1] + p[1]])
    };
    body(vec![
        bar([12.0, 4.0, 4.0], [6.0, 2.0]),
        bar([4.0, 12.0, 4.0], [2.0, 6.0]),
    ])
}

/// A copy standing a quarter turn from another takes, at the angle one
/// index short, the turn the other takes: Arrange all turns copies alike
/// by how they end up standing, not by the index of their angle.
#[test]
fn copies_standing_turned_apart_are_turned_alike_by_their_turn() {
    let mesh = cube();
    let plate = plate(90.0, 70.0);
    let frame = crate::Frame::new(&plate, &quarter_turns());
    let first = frame
        .footprint(&l_body(&mesh, 0.0, [20.0, 20.0]), false, &frame.angles)
        .expect("drawn");
    let turned = frame
        .footprint(
            &l_body(&mesh, std::f64::consts::FRAC_PI_2, [50.0, 20.0]),
            false,
            &frame.angles,
        )
        .expect("drawn");
    for a in 0..4 {
        let k = crate::search::turned_alike(&first, a, &turned).expect("a quarter turn");
        assert_eq!(k, (a + 3) % 4, "angle {a}");
        let (s, t) = (&first.shapes[a], &turned.shapes[k]);
        assert_eq!(
            [s.hi[0] - s.lo[0], s.hi[1] - s.lo[1]],
            [t.hi[0] - t.lo[0], t.hi[1] - t.lo[1]],
            "angle {a}: the same shape"
        );
    }
    assert_eq!(
        crate::search::turned_alike(
            &first,
            0,
            &frame
                .footprint(&l_body(&mesh, 0.3, [50.0, 50.0]), false, &frame.angles)
                .expect("drawn")
        ),
        None,
        "no angle stands that way"
    );
}

/// The bodies a pass leaves off are squeezed in (#555): Ls in rows leave
/// holes no L fits in, which the Ls around them close when they move. The
/// gap and the margin hold.
#[test]
fn the_bodies_left_off_are_squeezed_in_and_keep_the_gap() {
    let mesh = cube();
    let bodies: Vec<Body> = (0..30)
        .map(|k| l_body(&mesh, 0.0, [200.0 + 20.0 * f64::from(k), 0.0]))
        .collect();
    let plate = plate(50.0, 40.0);
    let squeezing = Settings {
        margin: 1.0,
        ..quarter_turns()
    };
    let passes_alone = Settings {
        squeeze: false,
        ..squeezing
    };
    let cancel = AtomicBool::new(false);
    let on = |o: &crate::Outcome| o.placements.iter().flatten().filter(|p| p.on_plate).count();
    let before = arrange(&bodies, &plate, &passes_alone, &cancel).unwrap();
    let after = arrange(&bodies, &plate, &squeezing, &cancel).unwrap();
    assert!(on(&after) > on(&before), "{} {}", on(&after), on(&before));
    assert_kept_apart(&bodies, &after.placements, &plate, &squeezing);
    // And again: the same scene, the same plate.
    let again = arrange(&bodies, &plate, &squeezing, &cancel).unwrap();
    assert_eq!(again.placements, after.placements);
}

/// Copies of a body share its footprint (#554), whether they share its
/// meshes or have meshes of their own, and however they stand turned by a
/// whole number of the angles' steps; turned otherwise, they do not.
#[test]
fn copies_share_a_footprint_however_they_stand_turned() {
    let mesh = cube();
    let own = || Arc::new((*mesh).clone());
    let eighth = std::f64::consts::FRAC_PI_4;
    let bodies = vec![
        body(vec![turned_box(&mesh, [10.0, 6.0, 4.0], 0.0, [20.0, 20.0])]),
        body(vec![turned_box(
            &own(),
            [10.0, 6.0, 4.0],
            0.0,
            [50.0, 20.0],
        )]),
        body(vec![turned_box(
            &own(),
            [10.0, 6.0, 4.0],
            eighth,
            [20.0, 50.0],
        )]),
        body(vec![turned_box(
            &mesh,
            [10.0, 6.0, 4.0],
            3.0 * eighth,
            [50.0, 50.0],
        )]),
        body(vec![turned_box(&mesh, [10.0, 6.0, 4.0], 0.3, [80.0, 50.0])]),
        body(vec![turned_box(&mesh, [10.0, 7.0, 4.0], 0.0, [80.0, 20.0])]),
    ];
    let frame = crate::Frame::new(&plate(100.0, 80.0), &Settings::default());
    let fps = crate::across::movable_footprints(&frame, &bodies);
    let shapes = |k: usize| Arc::as_ptr(&fps[k].as_ref().expect("drawn").shapes);
    for k in 1..4 {
        assert_eq!(shapes(k), shapes(0), "body {k} is a copy");
    }
    assert_ne!(shapes(4), shapes(0), "turned off the angles");
    assert_ne!(shapes(5), shapes(0), "another size");
}

/// Plates of copies take the same layout (#554): the search would find it
/// again, or one copy short, which leaves a plate at the end nearly empty.
/// Every plate keeps the gap and the margin.
#[test]
fn plates_of_copies_take_the_same_layout() {
    let mesh = cube();
    let bodies: Vec<Body> = (0..30)
        .map(|k| l_body(&mesh, 0.0, [-200.0 - 20.0 * f64::from(k), 0.0]))
        .collect();
    let plates = plates_in_a_row(8, 50.0, 40.0);
    let s = Settings {
        margin: 1.0,
        ..quarter_turns()
    };
    let cancel = AtomicBool::new(false);
    let across = crate::arrange_across(&bodies, &vec![None; bodies.len()], &plates, &s, &cancel)
        .expect("not cancelled");
    assert!(across.left_off().is_empty());
    // Each plate's bodies, where they stand on it.
    let on = |p: usize| -> Vec<(i64, i64, i64)> {
        let mut at: Vec<(i64, i64, i64)> = across
            .placements
            .iter()
            .map(|pl| pl.expect("placed"))
            .zip(&bodies)
            .filter(|((q, _), _)| *q == p)
            .map(|((_, pl), b)| {
                // Where the body's parts end up, from the plate's corner.
                let c = corners(b, Some(pl));
                let lo = c
                    .iter()
                    .fold([f64::INFINITY; 2], |m, q| [m[0].min(q[0]), m[1].min(q[1])]);
                (
                    ((lo[0] - plates[p].min[0]) * 1e3).round() as i64,
                    (lo[1] * 1e3).round() as i64,
                    (pl.angle.rem_euclid(std::f64::consts::TAU) * 1e6).round() as i64,
                )
            })
            .collect();
        at.sort_unstable();
        at
    };
    let first = on(0);
    assert!(first.len() > 4, "{}", first.len());
    let full = bodies.len() / first.len();
    for p in 1..full {
        assert_eq!(on(p), first, "plate {p} as the first");
    }
    for (p, plate) in plates.iter().enumerate().take(across.plates_used) {
        let (kept, at): (Vec<Body>, Vec<Option<Placement>>) = bodies
            .iter()
            .zip(&across.placements)
            .filter(|(_, pl)| pl.is_some_and(|(q, _)| q == p))
            .map(|(b, pl)| (b.clone(), pl.map(|(_, pl)| pl)))
            .unzip();
        assert_kept_apart(&kept, &at, plate, &s);
    }
    // And again: the same scene, the same plates.
    let again = crate::arrange_across(&bodies, &vec![None; bodies.len()], &plates, &s, &cancel)
        .expect("not cancelled");
    assert_eq!(again.placements, across.placements);
}

/// A plate a body stays on is filled first (#554): it is used anyway, and
/// the bodies it takes leave a plate fewer to fill.
#[test]
fn the_plate_a_body_stays_on_is_filled_first() {
    let mesh = cube();
    let plates = plates_in_a_row(4, 60.0, 60.0);
    // A 20 mm box staying on the third plate, and 6 of 20 mm to place: a
    // plate holds 4.
    let x = plates[2].min[0];
    let mut bodies = vec![Body {
        parts: vec![boxed(&mesh, [20.0, 20.0, 5.0], [x + 6.0, 6.0, 0.0])],
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
    stands_on[0] = Some(2);
    let s = Settings {
        turns: 1,
        ..Settings::default()
    };
    let across = crate::arrange_across(&bodies, &stands_on, &plates, &s, &AtomicBool::new(false))
        .expect("not cancelled");
    assert!(across.placements[0].is_none(), "it stays");
    let mut on = [0; 4];
    for placed in across.placements.iter().skip(1) {
        on[placed.expect("placed").0] += 1;
    }
    assert_eq!(on, [3, 0, 3, 0]);
    assert_eq!(across.plates_used, 3);
    let (kept, at): (Vec<Body>, Vec<Option<Placement>>) = bodies
        .iter()
        .zip(&across.placements)
        .filter(|(b, pl)| b.fixed || pl.is_some_and(|(q, _)| q == 2))
        .map(|(b, pl)| (b.clone(), pl.map(|(_, pl)| pl)))
        .unzip();
    let kept: Vec<Body> = kept
        .into_iter()
        .map(|b| Body { fixed: false, ..b })
        .collect();
    assert_kept_apart(&kept, &at, &plates[2], &s);
}

/// Copies that one plate holds are arranged as Arrange does (#554): rows
/// and lattices, which pack copies for fewer plates, would turn squares an
/// eighth into a lattice of diamonds only a little smaller, whose boxes
/// overlap in plan.
#[test]
fn copies_one_plate_holds_are_arranged_as_arrange_does() {
    let mesh = cube();
    let bodies: Vec<Body> = (0..6)
        .map(|_| body(vec![boxed(&mesh, [14.0, 14.0, 6.0], [-7.0, -7.0, 0.0])]))
        .collect();
    let plates = plates_in_a_row(4, 100.0, 60.0);
    let s = Settings {
        margin: 2.0,
        ..Settings::default()
    };
    let across = crate::arrange_across(&bodies, &[None; 6], &plates, &s, &AtomicBool::new(false))
        .expect("not cancelled");
    assert_eq!(across.plates_used, 1);
    let placements: Vec<Option<Placement>> = across
        .placements
        .iter()
        .map(|p| p.map(|(_, p)| p))
        .collect();
    // Each part's box, turned with it, keeps the gap.
    assert_kept_apart(&bodies, &placements, &plates[0], &s);
}

/// A table (a leg under a wide top) and a short box, at every quarter turn,
/// as a fill draws them on `plate`.
fn table_and_box(plate: &Plate) -> (crate::Frame, Vec<crate::footprint::Footprint>) {
    let mesh = cube();
    let table = body(vec![
        boxed(&mesh, [4.0, 4.0, 18.0], [20.0, 12.0, 0.0]),
        boxed(&mesh, [20.0, 16.0, 2.0], [12.0, 6.0, 18.0]),
    ]);
    let short = body(vec![boxed(&mesh, [5.0, 5.0, 5.0], [6.0, 6.0, 0.0])]);
    let frame = crate::Frame::new(plate, &quarter_turns());
    let fps = [table, short]
        .iter()
        .map(|b| frame.footprint(b, true, &frame.angles).expect("drawn"))
        .collect();
    (frame, fps)
}

/// A no-fit map says exactly where one shape collides with another, band
/// by band: a short box stands under a table's top, not in its leg.
#[test]
fn no_fit_maps_say_where_two_shapes_collide() {
    use crate::pattern::{Nofit, Runs};
    let (_, fps) = table_and_box(&plate(200.0, 200.0));
    let shapes: Vec<&crate::footprint::Shape> =
        fps.iter().flat_map(|fp| fp.shapes.iter()).collect();
    let mut under = false;
    for (i, a) in shapes.iter().enumerate() {
        for b in shapes.iter().skip(i) {
            let map = Nofit::of(&Runs::of(a), &Runs::of(b));
            let at = [300, 300];
            let mut scratch = crate::plate::Occupancy::new(600, 600, 40);
            scratch.stamp(a, at);
            let (lo, size) = (map.lo(), map.size());
            for dy in lo[1] - 3..lo[1] + size[1] as i32 + 3 {
                for dx in lo[0] - 3..lo[0] + size[0] as i32 + 3 {
                    let d = [dx, dy];
                    let collides = scratch.collides(b, [at[0] + d[0], at[1] + d[1]]);
                    assert_eq!(map.hit(d), collides, "{d:?}");
                    // The box's whole shadow inside the table's: under it.
                    under |= !collides
                        && b.hi[0] - b.lo[0] < 30
                        && a.hi[0] - a.lo[0] > 60
                        && at[0] + dx + b.lo[0] > at[0] + a.lo[0]
                        && at[0] + dx + b.hi[0] < at[0] + a.hi[0]
                        && at[1] + dy + b.lo[1] > at[1] + a.lo[1]
                        && at[1] + dy + b.hi[1] < at[1] + a.hi[1];
                }
            }
        }
    }
    assert!(under, "a box fits under the table");
}

/// The maps of where copies still fit say what the plate says, as copies
/// go in around a body that stays: every kind, every spot.
#[test]
fn the_free_maps_say_what_the_plate_says() {
    use crate::nest::{Corner, Free, Kinds};
    let plate = plate(60.0, 45.0);
    let (frame, fps) = table_and_box(&plate);
    // The table stays where it stands; copies of both go around it.
    let mut held = frame.plate_with(crate::bands_of(fps.iter()), [&fps[0]]);
    let shapes: Vec<&[crate::footprint::Shape]> = fps.iter().map(|f| &f.shapes[..]).collect();
    let kinds = Kinds::new(&shapes, &held);
    let mut free = Free::new(&kinds);
    let corners = [
        Corner {
            top: false,
            right: false,
        },
        Corner {
            top: true,
            right: true,
        },
    ];
    for round in 0..8 {
        for k in 0..kinds.len() {
            let room = kinds.room(k);
            let (b, a) = kinds.of[k];
            let shape = &fps[b].shapes[a];
            for y in 0..room[1] {
                for x in 0..room[0] {
                    let p = [x, y];
                    assert_eq!(
                        free.fits(&kinds, k, p),
                        held.fits(shape, kinds.at(k, p)),
                        "kind {k} at {p:?}, round {round}"
                    );
                }
            }
        }
        let k = (round * 3) % kinds.len();
        let Some(p) = free.first(&kinds, k, corners[round % 2]) else {
            continue;
        };
        let (b, a) = kinds.of[k];
        held.stamp(&fps[b].shapes[a], kinds.at(k, p));
        free.place(&kinds, k, p);
    }
}

/// Squares repeat on a lattice as on a tray: 8.5 mm boxes and a 1 mm gap,
/// 4 × 4 in the 40 mm inside the margin.
#[test]
fn squares_lie_on_a_lattice() {
    use crate::nest::{Free, Kinds};
    let mesh = cube();
    let plate = plate(50.0, 50.0);
    let frame = crate::Frame::new(&plate, &quarter_turns());
    let fp = frame
        .footprint(
            &body(vec![boxed(&mesh, [8.5, 8.5, 6.0], [5.0, 5.0, 0.0])]),
            true,
            &frame.angles,
        )
        .expect("drawn");
    let empty = crate::plate::Occupancy::new(frame.width, frame.height, 8);
    let kinds = Kinds::new(&[&fp.shapes[..]], &empty);
    let laid = crate::pattern::patterns(
        &kinds,
        &Free::new(&kinds),
        &[0, 1, 2, 3],
        &AtomicBool::new(false),
    )
    .expect("not cancelled");
    assert_eq!(laid[0].len(), 16);
}

/// A copy pressed in (Fill bed): three boxes spread over a row leave no
/// room for a fourth, which goes in once they move closer. Every box then
/// fits.
#[test]
fn a_copy_is_pressed_in_where_the_copies_make_room() {
    use crate::nest::{Free, Kinds};
    use crate::press::{Depths, press_in};
    let mesh = cube();
    let plate = plate(50.0, 20.0);
    let frame = crate::Frame::new(&plate, &quarter_turns());
    let fp = frame
        .footprint(
            &body(vec![boxed(&mesh, [8.5, 8.5, 6.0], [5.0, 5.0, 0.0])]),
            true,
            &frame.angles,
        )
        .expect("drawn");
    let empty = crate::plate::Occupancy::new(frame.width, frame.height, 8);
    let kinds = Kinds::new(&[&fp.shapes[..]], &empty);
    let free = Free::new(&kinds);
    // Corners at the left, middle and right of the row.
    let room = kinds.room(0);
    let spread: Vec<(usize, usize, [i32; 2])> = [0, room[0] / 2, room[0] - 1]
        .into_iter()
        .map(|x| (0, 0, kinds.at(0, [x, room[1] / 2])))
        .collect();
    let mut check = free.clone();
    for &(_, _, at) in &spread {
        let p = [at[0] + kinds.lo(0)[0], at[1] + kinds.lo(0)[1]];
        assert!(check.fits(&kinds, 0, p));
        check.place(&kinds, 0, p);
    }
    assert!(
        (0..kinds.len()).all(|k| check
            .first(
                &kinds,
                k,
                crate::nest::Corner {
                    top: false,
                    right: false
                }
            )
            .is_none()),
        "no room for a fourth"
    );
    let depths = Depths::of(&kinds);
    let pressed = press_in(
        &kinds,
        &depths,
        &free,
        spread,
        [0, 0],
        |_| true,
        &AtomicBool::new(false),
    )
    .expect("not cancelled");
    assert_eq!(pressed.len(), 4);
    let mut check = free.clone();
    for &(b, a, at) in &pressed {
        let k = kinds.of.iter().position(|&o| o == (b, a)).expect("a kind");
        let p = [at[0] + kinds.lo(k)[0], at[1] + kinds.lo(k)[1]];
        assert!(check.fits(&kinds, k, p), "{at:?}");
        check.place(&kinds, k, p);
    }
}

/// The same scene fills the same, copy for copy.
#[test]
fn the_same_scene_fills_the_same() {
    let mesh = cube();
    let table = body(vec![
        boxed(&mesh, [4.0, 4.0, 18.0], [20.0, 12.0, 0.0]),
        boxed(&mesh, [20.0, 16.0, 2.0], [12.0, 6.0, 18.0]),
    ]);
    let short = body(vec![boxed(&mesh, [5.0, 5.0, 5.0], [6.0, 6.0, 0.0])]);
    let l = l_body(&mesh, 0.0, [40.0, 20.0]);
    let bodies = [table, short, l];
    let plate = plate(70.0, 50.0);
    let s = quarter_turns();
    let a = fill_now(&bodies, &[1, 2], &plate, &s, None);
    let b = fill_now(&bodies, &[1, 2], &plate, &s, None);
    assert_eq!(a.copies, b.copies);
    assert_eq!(a.moved, b.moved);
    let (all, placements) = filled_plate(&bodies, &a);
    assert_kept_apart(&all, &placements, &plate, &s);
}

/// Without turns, or by whole shadows, a fill still puts as many copies
/// as fit: 8.5 mm boxes, 4 × 4 in the 40 mm inside the margin.
#[test]
fn a_fill_without_turns_or_by_shadows_puts_as_many_as_fit() {
    let mesh = cube();
    let bodies = [body(vec![boxed(&mesh, [8.5, 8.5, 6.0], [5.0, 5.0, 0.0])])];
    let plate = plate(50.0, 50.0);
    for s in [
        Settings {
            turns: 1,
            ..Settings::default()
        },
        Settings {
            shadow_only: true,
            ..quarter_turns()
        },
    ] {
        let filled = fill_now(&bodies, &[0], &plate, &s, None);
        assert_eq!(filled.copies.len(), 15, "{s:?}");
        let (all, placements) = filled_plate(&bodies, &filled);
        assert_kept_apart(&all, &placements, &plate, &s);
    }
}

/// Under one raft for all (#556), a fill packs copies by what stands on
/// their rafts: pillars on wide thin rafts go far closer than the rafts
/// would let them, and the pillars keep the gap.
#[test]
fn under_one_raft_for_all_a_fill_packs_by_what_stands_on_the_rafts() {
    let mesh = cube();
    let raft = boxed(&mesh, [10.0, 10.0, 1.0], [5.0, 5.0, 0.0]);
    let pillar = boxed(&mesh, [3.0, 3.0, 6.0], [8.5, 8.5, 1.0]);
    let bodies = [body(vec![raft, pillar])];
    let plate = plate(50.0, 50.0);
    let own_rafts = quarter_turns();
    let one_raft = Settings {
        free_below: 1.05,
        ..own_rafts
    };
    let apart = fill_now(&bodies, &[0], &plate, &own_rafts, None);
    let together = fill_now(&bodies, &[0], &plate, &one_raft, None);
    assert!(
        together.copies.len() > 2 * apart.copies.len(),
        "{} and {}",
        together.copies.len(),
        apart.copies.len()
    );
    let (all, placements) = filled_plate(&bodies, &together);
    let pillars: Vec<Body> = all.iter().map(|b| body(vec![b.parts[1].clone()])).collect();
    assert_kept_apart(&pillars, &placements, &plate, &one_raft);
}

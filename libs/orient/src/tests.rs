/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;

use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::{IndexedMesh, Vertex};

use crate::histogram::{self, overhang_weight};
use crate::pose::axis_angle;
use crate::tests_shapes::{Shape, lopsided, mini};
use crate::vec::{angle_deg, cross, dot, mul, norm, sub, unit};
use crate::*;

fn analyse(mesh: &IndexedMesh) -> OrientAnalysis {
    OrientAnalysis::new(
        mesh,
        &AffineTransform3D::identity(),
        None,
        &AtomicBool::new(false),
    )
    .expect("not cancelled")
}

fn search(mesh: &IndexedMesh, ctx: &OrientContext) -> SearchOutcome {
    analyse(mesh)
        .search(ctx, &AtomicBool::new(false), &mut |_, _| {})
        .expect("not cancelled")
}

fn rotated(mesh: &IndexedMesh, m: &[[f64; 3]; 3]) -> IndexedMesh {
    let mut out = mesh.clone();
    for v in &mut out.vertices {
        let p = mul(m, v.coords().map(f64::from));
        *v = Vertex::new(p.map(|x| x as f32));
    }
    out
}

fn transpose(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}

fn balanced() -> OrientContext {
    OrientContext::new(Preset::Balanced)
}

/// The recommended up direction after a search.
fn best_up(o: &SearchOutcome) -> [f64; 3] {
    o.recommended().up
}

#[test]
fn shapes_are_closed_and_outward() {
    for mesh in [
        Shape::default().sphere([0.0; 3], 10.0, 64, 32).mesh(),
        Shape::default().bowl(20.0, 17.0, 64, 16).mesh(),
        mini(),
        lopsided(),
    ] {
        let a = analyse(&mesh);
        assert!(a.is_closed());
        assert!(a.volume_mm3() > 0.0);
    }
}

#[test]
fn layer_area_of_a_sphere_follows_the_circle() {
    let r = 10.0;
    let a = analyse(&Shape::default().sphere([0.0; 3], r, 256, 128).mesh());
    let p = a.layer_profile([0.0, 0.0, 1.0], 0.5);
    assert_eq!(p.area.len(), 40);
    for (k, area) in p.area.iter().enumerate() {
        let z = p.z0 + (k as f64 + 0.5) * p.step;
        let exact = PI * (r * r - z * z);
        assert!(
            (area - exact).abs() <= 0.01 * exact + 0.5,
            "layer {k}: {area} vs {exact}"
        );
    }
}

#[test]
fn layer_area_of_a_cube_is_constant_and_a_tunnel_subtracts() {
    let cube = analyse(&Shape::default().boxed([0.0; 3], [20.0; 3], 3).mesh());
    // On any up direction through a face the section is 400 mm².
    for up in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]] {
        let p = cube.layer_profile(up, 0.5);
        for a in &p.area {
            assert!((a - 400.0).abs() < 1e-6, "{a}");
        }
        for l in &p.perimeter {
            assert!((l - 80.0).abs() < 1e-6, "{l}");
        }
    }
    // A 20 × 20 block with an 8 × 8 hole through it.
    let mut tunnel = sc_mesh_fixtures::cuboid_with_z_tunnel();
    for v in &mut tunnel.vertices {
        *v = Vertex::new(v.coords().map(|x| x * 10.0));
    }
    let t = analyse(&tunnel);
    assert!(t.is_closed());
    let p = t.layer_profile([0.0, 0.0, 1.0], 0.5);
    for a in &p.area {
        assert!((a - 336.0).abs() < 1e-6, "{a}");
    }
    for l in &p.perimeter {
        assert!((l - 112.0).abs() < 1e-6, "{l}");
    }
}

#[test]
fn layer_areas_add_up_to_the_volume() {
    for mesh in [
        Shape::default()
            .sphere([3.0, -2.0, 1.0], 12.0, 128, 64)
            .mesh(),
        mini(),
        lopsided(),
    ] {
        let a = analyse(&mesh);
        let volume = a.volume_mm3();
        for up in [[0.0, 0.0, 1.0], [0.3, -0.5, 0.81], [-1.0, 0.2, 0.1]] {
            let step = 0.05;
            let p = a.layer_profile(up, step);
            let integral: f64 = p.area.iter().sum::<f64>() * step;
            assert!(
                (integral - volume).abs() / volume < 0.003,
                "{integral} vs {volume} at {up:?}"
            );
        }
    }
}

#[test]
fn the_histogram_matches_the_exact_overhang() {
    let mesh = Shape::default().sphere([0.0; 3], 10.0, 256, 128).mesh();
    let a = analyse(&mesh);
    let c0 = 45f64.to_radians().cos();
    for up in [[0.0, 0.0, 1.0], [0.6, 0.0, 0.8], [0.2, -0.7, -0.68]] {
        let u = unit(up).unwrap();
        let binned = histogram::overhang(&a.bins, u, c0, 0.0).area;
        let exact: f64 = a
            .tri
            .iter()
            .map(|t| {
                let [p, q, r] = t.map(|i| a.pos[i as usize]);
                let n = cross(sub(q, p), sub(r, p));
                let area = norm(n) / 2.0;
                area * overhang_weight(-dot(unit(n).unwrap(), u), c0)
            })
            .sum();
        assert!(
            (binned - exact).abs() / exact < 0.005,
            "{binned} vs {exact}"
        );
    }
}

#[test]
fn the_search_is_deterministic() {
    let mesh = lopsided();
    let digest = |o: &SearchOutcome| {
        format!(
            "{:?} {:?} {}",
            o.options.iter().map(|c| (c.up, c.cost)).collect::<Vec<_>>(),
            o.current.cost,
            o.keep_current
        )
    };
    let a = digest(&search(&mesh, &balanced()));
    let b = digest(&search(&mesh, &balanced()));
    assert_eq!(a, b);
}

#[test]
fn three_diverse_options_never_worse_than_the_current_pose() {
    for mesh in [lopsided(), mini()] {
        let o = search(&mesh, &balanced());
        assert_eq!(o.options.len(), 3);
        for (i, a) in o.options.iter().enumerate() {
            for b in &o.options[i + 1..] {
                assert!(angle_deg(a.up, b.up) >= DIVERSE_DEG - 1e-9);
            }
        }
        let chosen = o.recommended();
        assert!(chosen.cost.total <= o.current.cost.total + 1e-9);
        if !o.keep_current {
            assert!(chosen.cost.beats(&o.current.cost, KEEP_MARGIN));
        }
    }
}

#[test]
fn re_orienting_the_result_keeps_it() {
    for mesh in [lopsided(), mini()] {
        let first = search(&mesh, &balanced());
        let turned = rotated(&mesh, &first.recommended().rotation);
        let again = search(&turned, &balanced());
        let up = best_up(&again);
        assert!(
            again.keep_current || angle_deg(up, [0.0, 0.0, 1.0]) <= 2.0,
            "moved again by {}°",
            angle_deg(up, [0.0, 0.0, 1.0])
        );
    }
}

/// The cost of a pose does not depend on the frame the model comes in, and
/// the search lands on the same pose: within 2°, or on a pose the cost
/// cannot tell apart (a flat valley, far below the keep margin).
#[test]
fn rotating_the_input_gives_the_same_pose_and_cost() {
    let mesh = lopsided();
    let a = analyse(&mesh);
    let base = search(&mesh, &balanced());
    let up = best_up(&base);
    let here = a.evaluate(up, &balanced()).total;
    for (axis, angle) in [
        ([0.3, 0.5, 0.8], 1.1),
        ([-0.9, 0.2, 0.1], 2.4),
        ([0.0, 1.0, -0.4], 0.7),
    ] {
        let r = axis_angle(unit(axis).unwrap(), angle);
        let turned = rotated(&mesh, &r);
        // The same pose costs the same in either frame.
        let same = analyse(&turned).evaluate(mul(&r, up), &balanced()).total;
        assert!((same - here).abs() / here < 0.001, "{same} vs {here}");
        let o = search(&turned, &balanced());
        let (c0, c1) = (base.recommended().cost.total, o.recommended().cost.total);
        assert!((c0 - c1).abs() / c0 < 0.01, "{c0} vs {c1}");
        // Back in the model's own frame.
        let back = mul(&transpose(&r), best_up(&o));
        let there = a.evaluate(back, &balanced()).total;
        assert!(
            angle_deg(back, up) <= 2.0 || (there - here).abs() / here < 0.005,
            "{}° apart, {here} vs {there}",
            angle_deg(back, up)
        );
    }
}

#[test]
fn a_bowl_never_ends_open_towards_the_film() {
    let bowl = Shape::default().bowl(20.0, 17.0, 64, 16).mesh();
    let opening = [0.0, 0.0, 1.0];
    for (axis, angle) in [
        ([1.0, 0.0, 0.0], 0.0),
        ([1.0, 0.0, 0.0], PI),
        ([0.2, 1.0, 0.0], 1.2),
    ] {
        let r = axis_angle(unit(axis).unwrap(), angle);
        let o = search(&rotated(&bowl, &r), &balanced());
        let open = mul(&r, opening);
        let up = best_up(&o);
        // How much the opening faces up once oriented.
        let facing = dot(open, up);
        assert!(facing <= 0.5, "opening {:.0}° from up", angle_deg(open, up));
    }
}

#[test]
fn a_thin_plate_stands_tilted_with_lower_force() {
    let plate = Shape::default()
        .boxed([0.0; 3], [60.0, 40.0, 2.0], 6)
        .mesh();
    let o = search(&plate, &OrientContext::new(Preset::LowerForce));
    let up = best_up(&o);
    let tilt = 90.0 - angle_deg(up, [0.0, 0.0, 1.0]).min(angle_deg(up, [0.0, 0.0, -1.0]));
    assert!(tilt >= 20.0, "lies within {tilt}° of flat");
    assert!(o.recommended().cost.peel.peak_force_n < o.current.cost.peel.peak_force_n / 3.0);
}

#[test]
fn a_long_part_gets_an_option_that_fits_diagonally() {
    // Standing, it is too tall; lying along X, too wide unless turned.
    let rod = Shape::default()
        .boxed([0.0; 3], [5.0, 5.0, 110.0], 8)
        .mesh();
    let ctx = OrientContext {
        build_volume_mm: Some([100.0, 60.0, 50.0]),
        ..balanced()
    };
    let o = search(&rod, &ctx);
    assert_eq!(o.current.cost.fit, Fit::TooTall);
    let lying = analyse(&rod).evaluate([1.0, 0.0, 0.0], &ctx);
    assert!(
        matches!(lying.fit, Fit::Fits { turn_deg } if turn_deg.abs() > 10.0),
        "{:?}",
        lying.fit
    );
    let fits: Vec<&Candidate> = o.options.iter().filter(|c| c.cost.fit.ok()).collect();
    assert!(!fits.is_empty());
    let best = o.recommended();
    assert!(
        matches!(best.cost.fit, Fit::Fits { .. }),
        "{:?}",
        best.cost.fit
    );
    assert!(!o.keep_current);
}

#[test]
fn a_large_flat_base_is_tilted_or_rests_on_the_plate() {
    let m = mini();
    // Lifted for supports: the base does not face straight down.
    let lifted = search(&m, &balanced());
    let up = best_up(&lifted);
    assert!(angle_deg(up, [0.0, 0.0, 1.0]) >= 20.0, "base flat, lifted");
    // Resting on the plate: the base sits on it.
    let resting = search(
        &m,
        &OrientContext {
            lift_mm: 0.0,
            ..balanced()
        },
    );
    let best = resting.recommended();
    assert!(angle_deg(best.up, [0.0, 0.0, 1.0]) <= 5.0, "{:?}", best.up);
    assert!(best.cost.plate_contact_mm2 > 400.0);
    assert_eq!(best.cost.islands, 0);
}

#[test]
fn an_open_mesh_is_searched_without_the_peel() {
    let mut m = lopsided();
    m.faces.pop();
    let o = search(&m, &balanced());
    assert_eq!(o.notes, vec![Note::OpenMesh]);
    assert!(!o.current.cost.peel_measured);
    assert_eq!(o.options.len(), 3);
}

#[test]
fn a_cancelled_search_stops() {
    let a = analyse(&lopsided());
    let cancel = AtomicBool::new(true);
    assert_eq!(
        a.search(&balanced(), &cancel, &mut |_, _| {}).unwrap_err(),
        Cancelled
    );
    assert!(
        OrientAnalysis::new(&lopsided(), &AffineTransform3D::identity(), None, &cancel).is_err()
    );
}

#[test]
fn partial_results_come_before_the_end() {
    let a = analyse(&mini());
    let mut stages = Vec::new();
    let o = a
        .search(&balanced(), &AtomicBool::new(false), &mut |s, c| {
            stages.push((s, c.len()))
        })
        .unwrap();
    assert_eq!(stages, vec![(Stage::Coarse, 3), (Stage::Refined, 3)]);
    assert_eq!(o.timings.verdict_poses, 4);
}

#[test]
fn painted_detail_gets_no_tips_when_a_pose_avoids_them() {
    let plate = Shape::default()
        .boxed([0.0; 3], [60.0, 40.0, 2.0], 6)
        .mesh();
    // Paint the bottom side: the faces facing down now.
    let paint: Vec<u8> = plate
        .faces
        .iter()
        .map(|f| {
            let z: Vec<f32> = f
                .vertices
                .iter()
                .map(|&v| plate.vertices[usize::from(v)].coords()[2])
                .collect();
            u8::from(z.iter().all(|&z| z == 0.0))
        })
        .collect();
    let a = OrientAnalysis::new(
        &plate,
        &AffineTransform3D::identity(),
        Some(&paint),
        &AtomicBool::new(false),
    )
    .unwrap();
    let o = a
        .search(&balanced(), &AtomicBool::new(false), &mut |_, _| {})
        .unwrap();
    let now = o.current.cost.detail.expect("painted");
    assert!(now.tips > 100.0);
    let best = o.recommended().cost.detail.expect("painted");
    assert!(best.total_tips() < 1.0, "{best:?}");
    assert!(best.facing_up > 0.5);
}

#[test]
fn turned_about_footprint_keeps_centre_and_lift() {
    let m = lopsided();
    let world = AffineTransform3D::from_translation([10.0, -5.0, 3.0]);
    let r = rotation_between([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let t = turned_about_footprint(&m, &world, &r);
    let bounds = |t: &AffineTransform3D| {
        m.vertices.iter().fold(
            ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
            |(mut lo, mut hi), v| {
                let p = t.apply_to_vertex(*v).coords().map(f64::from);
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
                (lo, hi)
            },
        )
    };
    let (lo0, hi0) = bounds(&world);
    let (lo1, hi1) = bounds(&t);
    assert!(((lo0[0] + hi0[0]) - (lo1[0] + hi1[0])).abs() < 1e-3);
    assert!(((lo0[1] + hi0[1]) - (lo1[1] + hi1[1])).abs() < 1e-3);
    assert!((lo0[2] - lo1[2]).abs() < 1e-3);
    // The +X side now points up: the model is 42 mm tall.
    assert!((hi1[2] - lo1[2] - 42.0).abs() < 1e-3);
}

/// Pruning by the minimum caps finds exactly the minima a full scan finds.
#[test]
fn pruned_minima_match_a_full_scan() {
    use crate::islands::{Neighbours, Ring, local_minima_many};
    let sphere = Shape::default().sphere([1.0, 2.0, 3.0], 9.0, 48, 24);
    for mesh in [
        mini(),
        lopsided(),
        sphere.mesh(),
        Shape::default().bowl(20.0, 17.0, 40, 10).mesh(),
    ] {
        let pos: Vec<[f64; 3]> = mesh
            .vertices
            .iter()
            .map(|v| v.coords().map(f64::from))
            .collect();
        let tri: Vec<[u32; 3]> = mesh
            .faces
            .iter()
            .map(|f| f.vertices.map(|v| v.get()))
            .collect();
        let ring = Ring::new(pos.len(), &tri);
        let pruned = Neighbours::new(&ring).with_caps(&pos);
        let dirs: Vec<[f64; 3]> = (0..500)
            .map(|i| {
                let z = 1.0 - 2.0 * (i as f64 + 0.5) / 500.0;
                let (s, c) = (i as f64 * 2.399963).sin_cos();
                let r = (1.0 - z * z).sqrt();
                [r * c, r * s, z]
            })
            .collect();
        let found = local_minima_many(&pos, &pruned, &dirs);
        for (u, got) in dirs.iter().zip(found) {
            let all: Vec<u32> = (0..pos.len() as u32)
                .filter(|&v| {
                    let r = ring.of(v as usize);
                    !r.is_empty()
                        && r.iter()
                            .flatten()
                            .all(|&j| dot(sub(pos[j as usize], pos[v as usize]), *u) > 0.0)
                })
                .collect();
            assert_eq!(got, all, "at {u:?}");
        }
    }
}

#[test]
fn a_full_search_judges_more_finalists_and_is_never_worse_than_the_current_pose() {
    for mesh in [lopsided(), mini()] {
        let quick = search(&mesh, &balanced());
        let full_ctx = OrientContext {
            mode: SearchMode::Full,
            ..balanced()
        };
        let full = search(&mesh, &full_ctx);
        assert_eq!(full.options.len(), 3);
        for (i, a) in full.options.iter().enumerate() {
            for b in &full.options[i + 1..] {
                assert!(angle_deg(a.up, b.up) >= DIVERSE_DEG - 1e-9);
            }
        }
        assert!(full.timings.stage1_poses > 3 * quick.timings.stage1_poses);
        assert_eq!(full.timings.verdict_poses, FULL_FINALISTS + 1);
        assert!(full.recommended().cost.total <= full.current.cost.total + 1e-9);
        // The same pose, scored the same way: what it finds is as good as
        // what Quick finds, give or take the margin within which poses tie.
        let (q, f) = (quick.options[0].cost.total, full.options[0].cost.total);
        assert!(f <= q * (1.0 + KEEP_MARGIN), "Full {f} against Quick {q}");
        // Deterministic too.
        let again = search(&mesh, &full_ctx);
        assert_eq!(
            again.options.iter().map(|c| c.up).collect::<Vec<_>>(),
            full.options.iter().map(|c| c.up).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_max_tilt_keeps_every_option_near_the_current_pose() {
    for mode in [SearchMode::Quick, SearchMode::Full] {
        let ctx = OrientContext {
            mode,
            max_tilt_deg: Some(30.0),
            ..balanced()
        };
        let o = search(&lopsided(), &ctx);
        assert!(!o.options.is_empty());
        for c in &o.options {
            assert!(c.tilt_deg <= 30.0 + 1e-6, "{mode:?}: {}°", c.tilt_deg);
        }
        // Without the limit the best pose of this shape tilts further.
        let free = search(&lopsided(), &OrientContext { mode, ..balanced() });
        assert!(free.options.iter().any(|c| c.tilt_deg > 30.0));
    }
}

#[test]
fn aligning_to_the_plate_lays_the_shadow_square() {
    // A flat 40 × 10 bar turned 30° about the vertical.
    let bar = Shape::default()
        .boxed([0.0; 3], [40.0, 10.0, 4.0], 4)
        .mesh();
    let bar = rotated(&bar, &axis_angle([0.0, 0.0, 1.0], 30f64.to_radians()));
    let a = analyse(&bar);
    let keep = a.evaluate(
        [0.0, 0.0, 1.0],
        &OrientContext {
            build_volume_mm: Some([120.0, 70.0, 150.0]),
            ..balanced()
        },
    );
    assert_eq!(keep.fit, Fit::Fits { turn_deg: 0.0 });
    let ctx = OrientContext {
        build_volume_mm: Some([120.0, 70.0, 150.0]),
        align_to_plate: true,
        ..balanced()
    };
    let square = a.evaluate([0.0, 0.0, 1.0], &ctx);
    let Fit::Fits { turn_deg } = square.fit else {
        panic!("{:?}", square.fit);
    };
    assert!((turn_deg + 30.0).abs() < 1e-4, "{turn_deg}");
    // The options turn the same way: however the bar tilts, its length
    // stays over the X axis, so its shadow lies along X.
    let o = search(&bar, &ctx);
    let best = o.recommended();
    let along = mul(
        &best.rotation,
        mul(
            &axis_angle([0.0, 0.0, 1.0], 30f64.to_radians()),
            [1.0, 0.0, 0.0],
        ),
    );
    assert!(
        along[1].abs() < 1e-3,
        "the bar's shadow lies along X: {along:?}"
    );
    // A plate deeper than wide takes the long side along Y.
    let tall = OrientContext {
        build_volume_mm: Some([70.0, 120.0, 150.0]),
        ..ctx
    };
    let Fit::Fits { turn_deg } = a.evaluate([0.0, 0.0, 1.0], &tall).fit else {
        panic!("fits");
    };
    assert!((turn_deg - 60.0).abs() < 1e-4, "{turn_deg}");
}

/// A model on the plate stays on it after a turn that would reach past an
/// edge, moved in the plate plane by the least amount; one that was off the
/// plate is left as turned.
#[test]
fn a_turn_near_the_edge_is_shifted_back_onto_the_plate() {
    let bar = Shape::default()
        .boxed([-30.0, -5.0, 0.0], [30.0, 5.0, 6.0], 4)
        .mesh();
    let (lo, hi) = ([-50.0, -40.0], [50.0, 40.0]);
    let bounds = |t: &AffineTransform3D| {
        bar.vertices.iter().fold(
            ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
            |(mut a, mut b), v| {
                let p = t.apply_to_vertex(*v).coords().map(f64::from);
                for k in 0..3 {
                    a[k] = a[k].min(p[k]);
                    b[k] = b[k].max(p[k]);
                }
                (a, b)
            },
        )
    };
    // Along X near the +Y edge; a quarter turn stands it along Y, 62 mm up.
    let world = AffineTransform3D::from_translation([10.0, 32.0, 0.0]);
    let quarter = axis_angle([0.0, 0.0, 1.0], PI / 2.0);
    let turned = turned_about_footprint(&bar, &world, &quarter);
    let (_, b) = bounds(&turned);
    assert!((b[1] - 62.0).abs() < 1e-3, "{b:?}");
    let kept = kept_on_plate(&bar, &world, &turned, lo, hi);
    let (a, b) = bounds(&kept);
    for k in 0..2 {
        assert!(a[k] >= lo[k] && b[k] <= hi[k], "axis {k}: {a:?} {b:?}");
    }
    assert!((b[1] - 40.0).abs() < 1e-2, "moved just inside: {b:?}");
    // Only moved along Y, and only in the plate plane.
    assert_eq!(kept.linear(), turned.linear());
    assert_eq!(kept.translation()[0], turned.translation()[0]);
    assert_eq!(kept.translation()[2], turned.translation()[2]);
    // Wider than the plate along Y: that axis is left alone.
    let narrow = kept_on_plate(&bar, &world, &turned, [-50.0, -20.0], [50.0, 37.1]);
    assert_eq!(narrow, turned);
    // Off the plate to begin with: left as turned.
    let off = AffineTransform3D::from_translation([200.0, 0.0, 0.0]).compose(&world);
    let turned_off = turned_about_footprint(&bar, &off, &quarter);
    assert_eq!(kept_on_plate(&bar, &off, &turned_off, lo, hi), turned_off);
}

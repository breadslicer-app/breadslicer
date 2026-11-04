/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The painted detail (#184): what the person painted is a constraint, never
//! one more weight.

use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;

use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};

use crate::tests_shapes::{Shape, lopsided, mini};
use crate::vec::{V3, angle_deg, cross, sub, unit};
use crate::*;

fn analyse(mesh: &IndexedMesh, paint: Option<&[u8]>) -> OrientAnalysis {
    OrientAnalysis::new(
        mesh,
        &AffineTransform3D::identity(),
        paint,
        &AtomicBool::new(false),
    )
    .expect("not cancelled")
}

fn search(analysis: &OrientAnalysis, ctx: &OrientContext) -> SearchOutcome {
    analysis
        .search(ctx, &AtomicBool::new(false), &mut |_, _| {})
        .expect("not cancelled")
}

fn balanced() -> OrientContext {
    OrientContext::new(Preset::Balanced)
}

fn on_detail(c: &Candidate) -> usize {
    c.cost.detail.expect("painted").whole_tips()
}

/// A mask of the faces whose centroid and outward normal pass `keep`.
fn paint_where(mesh: &IndexedMesh, keep: impl Fn(V3, V3) -> bool) -> Vec<u8> {
    mesh.faces
        .iter()
        .map(|f| {
            let [a, b, c] = f
                .vertices
                .map(|v| mesh.vertices[usize::from(v)].coords().map(f64::from));
            let centroid = std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0);
            let normal = unit(cross(sub(b, a), sub(c, a))).unwrap_or([0.0; 3]);
            if keep(centroid, normal) { 255 } else { 0 }
        })
        .collect()
}

/// A dome of radius `r` on a flat base at z = 0.
fn hemisphere(r: f64) -> IndexedMesh {
    let mut profile = vec![(0.0, 0.0), (r, 0.0)];
    for i in 1..24 {
        let a = PI / 2.0 * f64::from(i) / 24.0;
        profile.push((r * a.cos(), r * a.sin()));
    }
    profile.push((0.0, r));
    Shape::default().lathe([0.0, 0.0], &profile, 48).mesh()
}

/// `mesh` turned upside down.
fn flipped(mesh: &IndexedMesh) -> IndexedMesh {
    let mut out = mesh.clone();
    for v in &mut out.vertices {
        let p = v.coords();
        *v = Vertex::new([p[0], -p[1], -p[2]]);
    }
    out
}

/// Fibonacci directions over the whole sphere, to look for what a search
/// might have missed.
fn sphere_of_directions(n: usize) -> Vec<V3> {
    let ga = PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = (1.0 - z * z).sqrt();
            let th = ga * i as f64;
            [r * th.cos(), r * th.sin(), z]
        })
        .collect()
}

#[test]
fn a_hemisphere_with_its_dome_painted_ends_up_dome_up() {
    // Standing on its dome, the paint against the plate.
    let mesh = flipped(&hemisphere(20.0));
    let paint = paint_where(&mesh, |_, n| n[2] < 0.99);
    let a = analyse(&mesh, Some(&paint));
    let o = search(&a, &balanced());
    assert!(on_detail(&o.current) > 0);
    assert!(!o.keep_current);
    for option in &o.options {
        assert_eq!(on_detail(option), 0, "{option:?}");
        // The dome looks at -Z here, so that is what goes up. Past 45° the
        // dome starts to overhang.
        let tilt = angle_deg(option.up, [0.0, 0.0, -1.0]);
        assert!(tilt < 46.0, "{tilt}");
        assert!(option.cost.detail.unwrap().facing_up > 0.6);
    }
}

#[test]
fn a_painted_nose_that_is_a_low_point_stops_being_one() {
    // A head on a point: the point, the nose, is the lowest thing there is.
    let profile = [
        (0.0, 0.0),
        (2.0, 3.0),
        (12.0, 8.0),
        (15.0, 18.0),
        (12.0, 28.0),
        (0.0, 32.0),
    ];
    let mesh = Shape::default().lathe([0.0, 0.0], &profile, 32).mesh();
    let paint = paint_where(&mesh, |c, _| c[2] < 2.5);
    let a = analyse(&mesh, Some(&paint));
    let o = search(&a, &balanced());
    let now = o.current.cost.detail.unwrap();
    assert!(now.islands >= 1, "{now:?}");
    let best = o.recommended().cost.detail.unwrap();
    assert_eq!(best.islands, 0, "{best:?}");
    assert_eq!(best.whole_tips(), 0, "{best:?}");
    assert!(!o.notes.iter().any(|n| matches!(n, Note::DetailTips { .. })));
}

#[test]
fn the_cheapest_pose_gives_way_when_it_puts_the_paint_on_the_plate() {
    // Flat on the plate, the model needs nothing: that is what it does bare.
    let mesh = hemisphere(20.0);
    let ctx = OrientContext {
        lift_mm: 0.0,
        ..balanced()
    };
    let bare = search(&analyse(&mesh, None), &ctx);
    assert!(bare.keep_current, "{:?}", bare.recommended().up);
    let cheapest = bare.current.cost.total;
    // Painted, the flat side has to leave the plate, at whatever cost.
    let paint = paint_where(&mesh, |_, n| n[2] < -0.99);
    let o = search(&analyse(&mesh, Some(&paint)), &ctx);
    assert!(on_detail(&o.current) > 0);
    assert!(!o.keep_current);
    let best = o.recommended();
    assert_eq!(on_detail(best), 0, "{best:?}");
    assert!(best.cost.total > cheapest, "{} {cheapest}", best.cost.total);
    assert!(angle_deg(best.up, [0.0, 0.0, 1.0]) >= 44.0);
}

#[test]
fn no_option_puts_tips_on_paint_while_another_pose_avoids_them() {
    for (mesh, paint) in [
        {
            let m = mini();
            let p = paint_where(&m, |c, _| c[2] > 20.0);
            (m, p)
        },
        {
            let m = lopsided();
            let p = paint_where(&m, |_, n| n[1] < -0.9);
            (m, p)
        },
    ] {
        let a = analyse(&mesh, Some(&paint));
        let lowest = sphere_of_directions(300)
            .into_iter()
            .map(|u| a.evaluate(u, &balanced()).detail.unwrap().whole_tips())
            .min()
            .unwrap();
        for mode in [SearchMode::Quick, SearchMode::Full] {
            let o = search(&a, &OrientContext { mode, ..balanced() });
            assert!(!o.options.is_empty());
            for option in &o.options {
                assert!(
                    on_detail(option) <= lowest,
                    "{mode:?}: {} on detail, {lowest} is possible",
                    on_detail(option)
                );
            }
        }
    }
}

#[test]
fn a_cheaper_pose_with_a_tip_on_the_paint_loses_to_a_dearer_one_without() {
    let m = lopsided();
    // The large side the model would rather lie on.
    let paint = paint_where(&m, |c, n| n[2] < -0.9 && c[0] < 30.0);
    let a = analyse(&m, Some(&paint));
    let bare = search(&analyse(&m, None), &balanced());
    let o = search(&a, &balanced());
    let best = o.recommended();
    assert_eq!(on_detail(best), 0);
    let painted_cost_of_bare = a.evaluate(bare.recommended().up, &balanced());
    if painted_cost_of_bare.detail.unwrap().whole_tips() > 0 {
        assert!(best.cost.beats(&painted_cost_of_bare, 0.0));
    }
}

#[test]
fn when_no_pose_keeps_the_paint_clear_the_search_says_so() {
    // Painted all over: something always faces down.
    let mesh = Shape::default()
        .sphere([0.0, 0.0, 0.0], 15.0, 32, 24)
        .mesh();
    let paint = vec![255; mesh.faces.len()];
    let o = search(&analyse(&mesh, Some(&paint)), &balanced());
    let best = on_detail(&o.options[0]);
    assert!(best > 0);
    assert!(o.notes.contains(&Note::DetailTips { tips: best }));
    assert!(o.options.iter().all(|c| on_detail(c) == best));
}

#[test]
fn every_option_is_as_good_on_the_detail_as_the_best() {
    // Only tilts of the box around one axis keep this side clear.
    let m = mini();
    let paint = paint_where(&m, |_, n| n[2] < -0.5);
    let o = search(&analyse(&m, Some(&paint)), &balanced());
    let best = on_detail(&o.options[0]);
    assert!(o.options.iter().all(|c| on_detail(c) == best));
}

#[test]
fn without_paint_the_search_is_the_one_it_always_was() {
    let m = lopsided();
    let plain = analyse(&m, None);
    let blank = vec![0u8; m.faces.len()];
    let wrong_length = vec![255u8; m.faces.len() + 1];
    let same = search(&plain, &balanced());
    for other in [
        plain.with_detail(None),
        plain.with_detail(Some(&blank)),
        plain.with_detail(Some(&wrong_length)),
        analyse(&m, Some(&blank)),
    ] {
        assert!(!other.has_detail());
        let o = search(&other, &balanced());
        assert_eq!(o.options, same.options);
        assert_eq!(o.keep_current, same.keep_current);
    }
    assert!(same.options.iter().all(|c| c.cost.detail.is_none()));
}

#[test]
fn repainting_takes_the_analysis_it_has() {
    let m = lopsided();
    let plain = analyse(&m, None);
    let paint = paint_where(&m, |c, _| c[2] > 20.0);
    let painted = plain.with_detail(Some(&paint));
    assert!(painted.has_detail());
    assert!(!plain.has_detail());
    let direct = analyse(&m, Some(&paint));
    let a = search(&painted, &balanced());
    let b = search(&direct, &balanced());
    assert_eq!(a.options, b.options);
}

#[test]
fn a_degenerate_face_does_not_drop_the_paint() {
    let mut mesh = mini();
    // A face with a repeated corner, first in the list: the mask's indices
    // are the mesh's, so what follows must not shift.
    let v = mesh.faces[0].vertices.map(|v| v.get());
    let paint_clean = paint_where(&mesh, |c, _| c[2] > 20.0);
    mesh.faces
        .insert(0, IndexedTriangle::from_raw_indices([v[0], v[0], v[1]]));
    let mut paint = vec![0u8];
    paint.extend_from_slice(&paint_clean);
    let a = analyse(&mesh, Some(&paint));
    assert!(a.has_detail());
    let d = a.evaluate([0.0, 0.0, -1.0], &balanced()).detail.unwrap();
    assert!(d.total_tips() > 0.0);
}

#[test]
fn a_dimple_in_a_surface_that_looks_up_is_no_island() {
    let mut mesh = Shape::default()
        .boxed([0.0; 3], [40.0, 40.0, 10.0], 10)
        .mesh();
    // The middle of the top side sinks a little: lower than its ring, with
    // the model under it.
    let middle = mesh
        .vertices
        .iter()
        .position(|v| {
            let p = v.coords();
            (p[0] - 20.0).abs() < 1e-3 && (p[1] - 20.0).abs() < 1e-3 && p[2] > 9.0
        })
        .expect("a vertex in the middle");
    mesh.vertices[middle] = Vertex::new([20.0, 20.0, 9.5]);
    let paint = paint_where(&mesh, |c, _| c[2] > 9.0);
    let a = analyse(&mesh, Some(&paint));
    let d = a.evaluate([0.0, 0.0, 1.0], &balanced()).detail.unwrap();
    assert_eq!(d.islands, 0, "{d:?}");
    assert_eq!(d.whole_tips(), 0);
}

#[test]
fn a_sliver_past_the_overhang_angle_is_no_tip_but_anything_more_is() {
    let cost = |tips: f64, islands: usize| DetailCost {
        tips,
        islands,
        facing_up: 0.5,
    };
    assert_eq!(cost(0.0, 0).whole_tips(), 0);
    assert_eq!(cost(0.04, 0).whole_tips(), 0);
    assert_eq!(cost(0.2, 0).whole_tips(), 1);
    assert_eq!(cost(2.3, 1).whole_tips(), 4);
}

#[test]
fn facing_up_is_the_painted_mean_normal() {
    let mesh = Shape::default()
        .boxed([0.0; 3], [20.0, 20.0, 20.0], 4)
        .mesh();
    // All of it: the normals cancel, so no way up is better than another.
    let all = vec![255; mesh.faces.len()];
    let a = analyse(&mesh, Some(&all));
    for u in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]] {
        let d = a.evaluate(u, &balanced()).detail.unwrap();
        assert!((d.facing_up - 0.5).abs() < 1e-6, "{d:?}");
    }
    // The top alone: straight up is 1, straight down 0.
    let top = paint_where(&mesh, |_, n| n[2] > 0.9);
    let a = analyse(&mesh, Some(&top));
    let up = a.evaluate([0.0, 0.0, 1.0], &balanced()).detail.unwrap();
    let down = a.evaluate([0.0, 0.0, -1.0], &balanced()).detail.unwrap();
    assert!((up.facing_up - 1.0).abs() < 1e-6);
    assert!(down.facing_up.abs() < 1e-6);
}

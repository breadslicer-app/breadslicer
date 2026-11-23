/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The frame against closed forms.

use std::f64::consts::PI;

use crate::check::{buckling_runs, member_check};
use crate::frame::{Frame, MERGE_MM, Point};
use crate::solve::{Load, Material, Solved, Target, solve, world_forces};

const MAT: Material = Material {
    e_mpa: 1000.0,
    nu: 0.35,
    shear: true,
};

fn close(got: f64, want: f64, tol: f64) {
    let scale = want.abs().max(1e-12);
    assert!(
        (got - want).abs() / scale <= tol,
        "got {got}, want {want} (relative {:.2e})",
        (got - want).abs() / scale
    );
}

fn node_load(node: usize, f: [f64; 3]) -> Load {
    Load {
        target: Target::Node(node),
        at: [0.0; 3],
        force: f,
    }
}

fn area(d: f64) -> f64 {
    PI * d * d / 4.0
}

fn inertia(d: f64) -> f64 {
    PI * d.powi(4) / 64.0
}

/// A bar pulled along its axis: δ = P L / (E A), N = P.
#[test]
fn a_bar_pulled_stretches_by_p_l_over_e_a() {
    let (l, d, p) = (10.0, 1.0, 2.0);
    let mut fr = Frame::new();
    let a = fr.node([0.0, 0.0, 0.0]);
    fr.fix(a);
    let b = fr.node([0.0, 0.0, l]);
    fr.member(a, b, d, d, 1.0);
    let s = solve(&fr, &MAT, &[vec![node_load(b, [0.0, 0.0, p])]]).unwrap();
    close(
        s.displacement(0, b).unwrap()[2],
        p * l / (MAT.e_mpa * area(d)),
        1e-9,
    );
    close(s.forces[0][0].unwrap().axial_n(), p, 1e-9);
}

/// A slender cantilever in 8 spans, loaded across at its free end: its
/// deflection is P L³ / (3 E I) plus the shear's P L / (k G A), its moment
/// at the root P L.
#[test]
fn a_cantilever_bends_as_the_closed_form_says() {
    let (l, d, p, n) = (16.0, 1.0, 0.05, 8);
    let mut fr = Frame::new();
    let mut prev = fr.node([0.0, 0.0, 0.0]);
    fr.fix(prev);
    for i in 1..=n {
        let next = fr.node([0.0, 0.0, l * i as f64 / n as f64]);
        fr.member(prev, next, d, d, 1.0);
        prev = next;
    }
    let s = solve(&fr, &MAT, &[vec![node_load(prev, [p, 0.0, 0.0])]]).unwrap();
    let g = MAT.e_mpa / (2.0 * (1.0 + MAT.nu));
    let k = 6.0 * (1.0 + MAT.nu) / (7.0 + 6.0 * MAT.nu);
    let bend = p * l.powi(3) / (3.0 * MAT.e_mpa * inertia(d));
    let shear = p * l / (k * g * area(d));
    close(s.displacement(0, prev).unwrap()[0], bend + shear, 1e-9);
    close(s.forces[0][0].unwrap().bending_nmm(false), p * l, 1e-9);
    // The stress at the root: M c / I.
    let m = member_check(&fr, 0, &s.forces[0][0].unwrap(), 0.0);
    close(m.stress_mpa, p * l * (d / 2.0) / inertia(d), 1e-9);
}

/// A rigid body on two columns, pulled off centre: each column takes its
/// share by the lever rule, but for the ~1 % the columns' bending carries.
fn lever(fr_load_x: f64) -> (f64, f64) {
    let (a, l, p) = (5.0, 10.0, 1.0);
    let mut fr = Frame::new();
    let body = fr.body([0.0, 0.0, l], false);
    for x in [-a, a] {
        let root = fr.node([x, 0.0, 0.0]);
        fr.fix(root);
        let top = fr.node([x, 0.0, l]);
        fr.attach(top, body);
        fr.member(root, top, 1.0, 1.0, 1.0);
    }
    let load = Load {
        target: Target::Body(body),
        at: [fr_load_x, 0.0, l],
        force: [0.0, 0.0, p],
    };
    let s = solve(&fr, &MAT, &[vec![load]]).unwrap();
    (
        s.forces[0][0].unwrap().axial_n(),
        s.forces[0][1].unwrap().axial_n(),
    )
}

#[test]
fn a_body_on_two_columns_shares_its_pull_by_the_lever_rule() {
    let (a, b, p) = (5.0, 2.0, 1.0);
    let (far, near) = lever(b);
    close(far, p * (a - b) / (2.0 * a), 0.02);
    close(near, p * (a + b) / (2.0 * a), 0.02);
}

/// A small forest: tips under a tilted body, each on a trunk with a cone
/// and a neck, braces between neighbours.
fn forest() -> (Frame, Vec<Load>) {
    let mut fr = Frame::new();
    let body = fr.body([0.0, 0.0, 15.0], false);
    let mut tops = Vec::new();
    for iy in 0..3 {
        for ix in 0..4 {
            let (x, y) = (ix as f64 * 3.0 - 4.5, iy as f64 * 3.0 - 3.0);
            let z = 8.0 + 0.8 * x + 0.3 * y;
            let root = fr.node([x, y, 0.0]);
            fr.fix(root);
            let mid = fr.node([x, y, z * 0.5]);
            let socket = fr.node([x, y, z - 2.5]);
            let neck = fr.node([x, y, z - 0.25]);
            let surface = fr.node([x, y, z]);
            fr.attach(surface, body);
            fr.member(root, mid, 1.1, 1.1, 1.0);
            fr.member(mid, socket, 1.1, 1.1, 1.0);
            fr.member(socket, neck, 1.1, 0.3, 1.0);
            fr.member(neck, surface, 0.3, 0.3, 2.0);
            tops.push(mid);
        }
    }
    for k in 1..tops.len() {
        if k % 4 != 0 {
            fr.member(tops[k - 1], tops[k], 0.8, 0.8, 1.0);
        }
    }
    let loads = vec![
        Load {
            target: Target::Body(body),
            at: [1.0, 0.5, 15.0],
            force: [0.0, 0.0, 6.0],
        },
        Load {
            target: Target::Body(body),
            at: [1.0, 0.5, 15.0],
            force: [0.3, 0.1, 0.0],
        },
    ];
    (fr, loads)
}

fn reactions(fr: &Frame, s: &Solved, case: usize) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for (k, m) in fr.members().iter().enumerate() {
        let Some(forces) = s.forces[case][k] else {
            continue;
        };
        let world = world_forces(fr, k, &forces);
        for (end, node) in [m.a, m.b].into_iter().enumerate() {
            if fr.holds()[node] == crate::frame::Hold::Fixed {
                for c in 0..3 {
                    sum[c] += world[end * 6 + c];
                }
            }
        }
    }
    sum
}

#[test]
fn the_forest_holds_its_load_and_its_stresses_do_not_depend_on_e() {
    let (fr, loads) = forest();
    let cases = [loads.clone()];
    let s1 = solve(&fr, &MAT, &cases).unwrap();
    assert!(s1.residual < 1e-9, "residual {}", s1.residual);
    let r = reactions(&fr, &s1, 0);
    let load: [f64; 3] = std::array::from_fn(|c| loads.iter().map(|l| l.force[c]).sum());
    for c in 0..3 {
        assert!(
            (r[c] + load[c]).abs() < 1e-9,
            "axis {c}: {} + {}",
            r[c],
            load[c]
        );
    }
    let stiffer = Material {
        e_mpa: 2500.0,
        ..MAT
    };
    let s2 = solve(&fr, &stiffer, &cases).unwrap();
    let stress = |s: &Solved| -> Vec<f64> {
        (0..fr.members().len())
            .map(|k| member_check(&fr, k, &s.forces[0][k].unwrap(), 0.0).stress_mpa)
            .collect()
    };
    let (a, b) = (stress(&s1), stress(&s2));
    let peak = a.iter().copied().fold(0.0, f64::max);
    for (x, y) in a.iter().zip(&b) {
        assert!((x - y).abs() / peak < 1e-8);
    }
}

/// Turning the whole forest about Z changes no stress.
#[test]
fn turning_the_forest_changes_no_stress() {
    let (fr, loads) = forest();
    let angle: f64 = 0.645;
    let (s, c) = angle.sin_cos();
    let turn = |p: Point| [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
    let mut turned = Frame::new();
    let mut map = Vec::new();
    for (i, p) in fr.nodes().iter().enumerate() {
        let n = turned.node(turn(*p));
        if fr.holds()[i] == crate::frame::Hold::Fixed {
            turned.fix(n);
        }
        map.push(n);
    }
    let body = turned.body(turn(fr.bodies()[0].at), false);
    for (i, hold) in fr.holds().iter().enumerate() {
        if *hold == crate::frame::Hold::Body(0) {
            turned.attach(map[i], body);
        }
    }
    for m in fr.members() {
        turned.member(map[m.a], map[m.b], m.d_a, m.d_b, m.kt);
    }
    let turned_loads: Vec<Load> = loads
        .iter()
        .map(|l| Load {
            target: Target::Body(body),
            at: turn(l.at),
            force: turn(l.force),
        })
        .collect();
    let a = solve(&fr, &MAT, &[loads]).unwrap();
    let b = solve(&turned, &MAT, &[turned_loads]).unwrap();
    for k in 0..fr.members().len() {
        let x = member_check(&fr, k, &a.forces[0][k].unwrap(), 0.0).stress_mpa;
        let y = member_check(&turned, k, &b.forces[0][k].unwrap(), 0.0).stress_mpa;
        assert!(
            (x - y).abs() < 1e-8 * x.max(1e-3),
            "beam {k}: {x} against {y}"
        );
    }
}

/// A beam split in two at its middle carries the same.
#[test]
fn splitting_a_beam_changes_nothing() {
    let p = [0.07, -0.02, 1.5];
    let one = {
        let mut fr = Frame::new();
        let a = fr.node([0.0, 0.0, 0.0]);
        fr.fix(a);
        let b = fr.node([2.0, 1.0, 12.0]);
        fr.member(a, b, 1.0, 1.0, 1.0);
        let s = solve(&fr, &MAT, &[vec![node_load(b, p)]]).unwrap();
        (s.displacement(0, b).unwrap(), s.forces[0][0].unwrap())
    };
    let two = {
        let mut fr = Frame::new();
        let a = fr.node([0.0, 0.0, 0.0]);
        fr.fix(a);
        let m = fr.node([1.0, 0.5, 6.0]);
        let b = fr.node([2.0, 1.0, 12.0]);
        fr.member(a, m, 1.0, 1.0, 1.0);
        fr.member(m, b, 1.0, 1.0, 1.0);
        let s = solve(&fr, &MAT, &[vec![node_load(b, p)]]).unwrap();
        (s.displacement(0, b).unwrap(), s.forces[0][0].unwrap())
    };
    for i in 0..6 {
        close(two.0[i], one.0[i], 1e-9);
    }
    close(two.1.axial_n(), one.1.axial_n(), 1e-9);
    close(two.1.bending_nmm(false), one.1.bending_nmm(false), 1e-9);
}

/// Solving twice gives the same, bit for bit; several cases at once give
/// what each gives alone.
#[test]
fn a_solve_is_deterministic_and_cases_add_up() {
    let (fr, loads) = forest();
    let both = solve(&fr, &MAT, &[vec![loads[0]], vec![loads[1]]]).unwrap();
    let again = solve(&fr, &MAT, &[vec![loads[0]], vec![loads[1]]]).unwrap();
    let alone = solve(&fr, &MAT, &[vec![loads[1]]]).unwrap();
    let sum = solve(&fr, &MAT, std::slice::from_ref(&loads)).unwrap();
    for k in 0..fr.members().len() {
        assert_eq!(both.forces[0][k], again.forces[0][k]);
        let (x, y) = (both.forces[1][k].unwrap(), alone.forces[0][k].unwrap());
        for i in 0..12 {
            assert!((x.0[i] - y.0[i]).abs() < 1e-9);
        }
        let added = both.forces[0][k].unwrap().plus(&x, 1.0);
        for i in 0..12 {
            assert!((added.0[i] - sum.forces[0][k].unwrap().0[i]).abs() < 1e-9);
        }
    }
}

/// What nothing fixed holds is left out, not solved; its load is told.
#[test]
fn what_nothing_holds_is_left_out() {
    let mut fr = Frame::new();
    let a = fr.node([0.0, 0.0, 0.0]);
    fr.fix(a);
    let b = fr.node([0.0, 0.0, 5.0]);
    fr.member(a, b, 1.0, 1.0, 1.0);
    let c = fr.node([10.0, 0.0, 0.0]);
    let d = fr.node([10.0, 0.0, 5.0]);
    fr.member(c, d, 1.0, 1.0, 1.0);
    let s = solve(
        &fr,
        &MAT,
        &[vec![
            node_load(b, [0.0, 0.0, 1.0]),
            node_load(d, [0.0, 0.0, 1.0]),
        ]],
    )
    .unwrap();
    assert!(s.forces[0][0].is_some());
    assert!(s.forces[0][1].is_none());
    assert_eq!(s.left_out, 1);
    close(s.unheld_n[0], 1.0, 1e-12);
}

/// Nodes closer than the merge distance are one.
#[test]
fn close_nodes_are_one() {
    let mut fr = Frame::new();
    let a = fr.node([1.0, 1.0, 1.0]);
    let b = fr.node([1.0 + MERGE_MM * 0.6, 1.0, 1.0]);
    let c = fr.node([1.0 + MERGE_MM * 1.5, 1.0, 1.0]);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert!(fr.member(a, b, 1.0, 1.0, 1.0).is_none());
}

/// A column squeezed: its run between the root and the body buckles at
/// π² E I / L², whatever spans it is drawn in.
#[test]
fn a_column_buckles_over_its_whole_run() {
    let (l, d) = (12.0, 1.0);
    let mut fr = Frame::new();
    let mut prev = fr.node([0.0, 0.0, 0.0]);
    fr.fix(prev);
    for i in 1..=4 {
        let next = fr.node([0.0, 0.0, l * f64::from(i) / 4.0]);
        fr.member(prev, next, d, d, 1.0);
        prev = next;
    }
    let runs = buckling_runs(&fr, &MAT);
    let want = PI * PI * MAT.e_mpa * inertia(d) / (l * l);
    for r in runs {
        close(r, want, 1e-12);
    }
    let s = solve(&fr, &MAT, &[vec![node_load(prev, [0.0, 0.0, -want / 2.0])]]).unwrap();
    let m = member_check(&fr, 3, &s.forces[0][3].unwrap(), want);
    close(m.buckling, 0.5, 1e-9);
}

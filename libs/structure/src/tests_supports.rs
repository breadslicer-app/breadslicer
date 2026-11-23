/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A support graph checked while a layer peels.

use crate::solve::Material;
use crate::supports::{BodyLoad, CheckOptions, NECK_KT, SupportGraph, check_layer};

const OPTIONS: CheckOptions = CheckOptions {
    material: Material::GREEN_RESIN,
    strength_mpa: 8.0,
    lateral_share: 0.0,
    directions: 8,
};

/// A trunk from the raft at `(x, y)` up to a tip whose end is at `z`: its
/// tip ends 0.1 mm inside the model.
fn trunk(g: &mut SupportGraph, name: &str, x: f64, y: f64, z: f64) -> usize {
    let owner = g.owner(name);
    let nodes = g.line(
        &[
            ([x, y, 0.0], 1.1),
            ([x, y, z - 2.5], 1.1),
            ([x, y, z - 0.25], 0.3),
        ],
        owner,
    );
    g.fix(nodes[0]);
    let surface = g.node([x, y, z]);
    g.tip_member(nodes[2], surface, 0.3, 0.3, NECK_KT, owner);
    g.contact(surface, [x, y, z + 0.1], owner);
    nodes[1]
}

fn body(force_n: f64, at: [f64; 3], tips: Vec<[f64; 3]>) -> BodyLoad {
    BodyLoad {
        force_n,
        at,
        held: false,
        tips,
        feet: Vec::new(),
    }
}

/// Two equal trunks under a body pulled midway: each takes half, and the
/// neck is the most loaded part.
#[test]
fn two_trunks_under_a_body_share_its_pull() {
    let mut g = SupportGraph::new();
    trunk(&mut g, "a", -3.0, 0.0, 10.0);
    trunk(&mut g, "b", 3.0, 0.0, 10.0);
    let pulled = body(
        1.0,
        [0.0, 0.0, 10.0],
        vec![[-3.0, 0.0, 10.1], [3.0, 0.0, 10.1]],
    );
    let check = check_layer(&[g], 10.0, &[pulled], &OPTIONS).unwrap();
    assert_eq!((check.tips_found, check.tips_missed), (2, 0));
    assert_eq!(
        check.parts[0],
        vec![crate::Part::Tip; 2],
        "the neck is worst off"
    );
    let [a, b] = [check.owners[0][0], check.owners[0][1]];
    assert!((a - b).abs() < 1e-9 * a, "{a} against {b}");
    // Half a newton on a 0.3 mm neck, twice for its shoulder, over 8 MPa.
    let area = std::f64::consts::PI * 0.3 * 0.3 / 4.0;
    let want = NECK_KT * 0.5 / area / OPTIONS.strength_mpa;
    assert!((a - want).abs() < 0.02 * want, "{a} against {want}");
    assert!(check.residual < 1e-9);
}

/// A branch hung from a trunk loads that trunk with both tips' pull: the
/// trunk's own neck is no worse off, its shaft is.
#[test]
fn a_trunk_carrying_a_branch_takes_both_pulls() {
    let mut g = SupportGraph::new();
    let knot = trunk(&mut g, "trunk", 0.0, 0.0, 10.0);
    let lone = trunk(&mut g, "lone", 6.0, 0.0, 10.0);
    let _ = lone;
    let branch = g.owner("branch");
    let knot_at = g.frame().nodes()[knot];
    let tip_top = [2.0, 0.0, 10.0];
    let nodes = g.line(
        &[
            (knot_at, 0.8),
            ([2.0, 0.0, 7.5], 0.8),
            ([2.0, 0.0, 9.75], 0.3),
        ],
        branch,
    );
    let surface = g.node(tip_top);
    g.member(nodes[2], surface, 0.3, 0.3, NECK_KT, branch);
    g.contact(surface, [2.0, 0.0, 10.1], branch);
    let pulled = body(
        3.0,
        [2.0, 0.0, 10.0],
        vec![[0.0, 0.0, 10.1], [2.0, 0.0, 10.1], [6.0, 0.0, 10.1]],
    );
    let check = check_layer(&[g.clone()], 10.0, &[pulled.clone()], &OPTIONS).unwrap();
    assert_eq!(check.tips_found, 3);
    assert!(check.worst > 0.0);

    // Without the branch, the trunk's shaft carries less.
    let mut plain = SupportGraph::new();
    trunk(&mut plain, "trunk", 0.0, 0.0, 10.0);
    trunk(&mut plain, "lone", 6.0, 0.0, 10.0);
    let lone_body = body(
        3.0,
        [2.0, 0.0, 10.0],
        vec![[0.0, 0.0, 10.1], [6.0, 0.0, 10.1]],
    );
    let alone = check_layer(&[plain], 10.0, &[lone_body], &OPTIONS).unwrap();
    // The branch takes some of the pull, so the necks are less loaded.
    assert!(check.owners[0][0] < alone.owners[0][0]);
}

/// Below the body's tips, the layer reaches no support: nothing is pulled.
/// A body held by the plate pulls nothing either.
#[test]
fn a_layer_below_the_tips_or_a_held_body_pull_nothing() {
    let mut g = SupportGraph::new();
    trunk(&mut g, "a", 0.0, 0.0, 10.0);
    let tips = vec![[0.0, 0.0, 10.1]];
    let check = check_layer(
        &[g.clone()],
        4.0,
        &[body(1.0, [0.0, 0.0, 4.0], tips.clone())],
        &OPTIONS,
    )
    .unwrap();
    assert_eq!(check.tips_found, 0);
    assert_eq!(check.bodies_unheld, 1);
    assert_eq!(check.worst, 0.0);
    let mut held = body(1.0, [0.0, 0.0, 10.0], tips);
    held.held = true;
    let check = check_layer(&[g], 10.0, &[held], &OPTIONS).unwrap();
    assert_eq!(check.worst, 0.0);
}

/// A sideways share bends the necks: their stress grows, in every
/// direction alike for a lone round trunk.
#[test]
fn a_sideways_share_bends_the_necks() {
    let mut g = SupportGraph::new();
    trunk(&mut g, "a", 0.0, 0.0, 10.0);
    let pulled = body(1.0, [0.0, 0.0, 10.0], vec![[0.0, 0.0, 10.1]]);
    let straight = check_layer(&[g.clone()], 10.0, &[pulled.clone()], &OPTIONS).unwrap();
    let sideways = CheckOptions {
        lateral_share: 0.1,
        ..OPTIONS
    };
    let bent = check_layer(&[g], 10.0, &[pulled], &sideways).unwrap();
    assert!(
        bent.worst > straight.worst * 1.5,
        "{} against {}",
        bent.worst,
        straight.worst
    );
}

/// A graph moved as a whole, its bodies with it, checks the same.
#[test]
fn a_moved_graph_checks_the_same() {
    let mut g = SupportGraph::new();
    trunk(&mut g, "a", -3.0, 0.0, 10.0);
    trunk(&mut g, "b", 3.0, 1.0, 12.0);
    let pulled = body(
        1.0,
        [0.5, 0.0, 12.0],
        vec![[-3.0, 0.0, 10.1], [3.0, 1.0, 12.1]],
    );
    let shift = |p: [f64; 3]| [p[0] + 40.0, p[1] - 7.0, p[2]];
    let moved = g.mapped(shift);
    let mut moved_body = pulled.clone();
    moved_body.at = shift(moved_body.at);
    moved_body.tips = moved_body.tips.iter().map(|p| shift(*p)).collect();
    let options = CheckOptions {
        lateral_share: 0.05,
        ..OPTIONS
    };
    let a = check_layer(&[g], 12.0, &[pulled], &options).unwrap();
    let b = check_layer(&[moved], 12.0, &[moved_body], &options).unwrap();
    for (x, y) in a.owners[0].iter().zip(&b.owners[0]) {
        assert!((x - y).abs() < 1e-8 * x.max(1e-6), "{x} against {y}");
    }
}

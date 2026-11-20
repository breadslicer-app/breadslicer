/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use super::*;

fn group(tips: &[[f64; 2]]) -> TipGroup {
    let mut sums = TipSums::default();
    for &tip in tips {
        sums.add(tip, 0.0);
    }
    sums.group().expect("tips")
}

fn pull(force_n: f64, at_mm: [f64; 2]) -> TipLoadCase {
    TipLoadCase {
        force_n,
        at_mm,
        z_mm: 0.0,
        lateral_share: 0.0,
    }
}

fn loads(tips: &[[f64; 2]], case: &TipLoadCase) -> Vec<f64> {
    let group = group(tips);
    tips.iter().map(|&tip| group.load_n(tip, case)).collect()
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

const SQUARE: [[f64; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

#[test]
fn a_centred_pull_splits_evenly() {
    for load in loads(&SQUARE, &pull(4.0, [0.0, 0.0])) {
        near(load, 1.0);
    }
}

#[test]
fn an_eccentric_pull_follows_the_lever() {
    // On the right edge: the right tips take it all.
    let at_edge = loads(&SQUARE, &pull(4.0, [1.0, 0.0]));
    near(at_edge[0], 0.0);
    near(at_edge[1], 2.0);
    near(at_edge[2], 2.0);
    near(at_edge[3], 0.0);
    // Two tips 2 mm apart, the pull a quarter of the way: 3/4 and 1/4.
    let pair = loads(&[[-1.0, 0.0], [1.0, 0.0]], &pull(1.0, [0.5, 0.0]));
    near(pair[0], 0.25);
    near(pair[1], 0.75);
}

#[test]
fn the_shares_balance_the_pull_and_its_moment() {
    let tips = [
        [0.3, 2.0],
        [4.1, -1.2],
        [-2.5, 0.7],
        [1.9, 3.3],
        [-0.4, -2.8],
    ];
    let case = pull(7.5, [1.2, -0.6]);
    let shares = loads(&tips, &case);
    near(shares.iter().sum::<f64>(), 7.5);
    for axis in 0..2 {
        let moment: f64 = tips.iter().zip(&shares).map(|(t, p)| t[axis] * p).sum();
        near(moment, 7.5 * case.at_mm[axis]);
    }
}

#[test]
fn moving_and_turning_the_body_changes_nothing() {
    let tips = [[0.3, 2.0], [4.1, -1.2], [-2.5, 0.7], [1.9, 3.3]];
    let at = [0.9, 0.4];
    let before = loads(&tips, &pull(3.0, at));
    let (s, c) = 0.7f64.sin_cos();
    let moved = |p: [f64; 2]| [c * p[0] - s * p[1] + 41.0, s * p[0] + c * p[1] - 17.0];
    let turned: Vec<[f64; 2]> = tips.iter().map(|&t| moved(t)).collect();
    let after = loads(&turned, &pull(3.0, moved(at)));
    for (a, b) in before.iter().zip(&after) {
        near(*a, *b);
    }
}

#[test]
fn tips_in_a_line_take_no_moment_across_it() {
    let line = [[-2.0, 0.0], [0.0, 0.0], [2.0, 0.0]];
    for load in loads(&line, &pull(3.0, [0.0, 5.0])) {
        near(load, 1.0);
    }
    // One tip takes the whole pull wherever it is.
    near(loads(&[[3.0, 3.0]], &pull(2.0, [9.0, -4.0]))[0], 2.0);
    // Two tips on one spot share it.
    for load in loads(&[[1.0, 1.0], [1.0, 1.0]], &pull(2.0, [5.0, 5.0])) {
        near(load, 1.0);
    }
}

#[test]
fn a_sideways_share_loads_the_far_tips_more() {
    let case = TipLoadCase {
        force_n: 4.0,
        at_mm: [0.0, 0.0],
        z_mm: 10.0,
        lateral_share: 0.05,
    };
    // I = diag(4, 4); |I⁻¹ d| = √2/4 at each corner.
    let expected = 1.0 + 0.05 * 4.0 * 10.0 * 2f64.sqrt() / 4.0;
    for load in loads(&SQUARE, &case) {
        near(load, expected);
    }
    // Tips spread twice as far take half the extra load.
    let wide: Vec<[f64; 2]> = SQUARE.iter().map(|t| [t[0] * 2.0, t[1] * 2.0]).collect();
    let extra = loads(&wide, &case)[0] - 1.0;
    near(extra, (expected - 1.0) / 2.0);
}

#[test]
fn merged_sums_equal_one_group() {
    let mut a = TipSums::default();
    a.add([1.0, 2.0], 3.0);
    a.add([-1.0, 0.5], 2.0);
    let mut b = TipSums::default();
    b.add([4.0, -2.0], 1.0);
    let mut all = TipSums::default();
    all.add([1.0, 2.0], 3.0);
    all.add([-1.0, 0.5], 2.0);
    all.add([4.0, -2.0], 1.0);
    a.merge(&b);
    assert_eq!(a, all);
    assert_eq!(a.count(), 3);
    assert!(TipSums::default().group().is_none());
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! How close each beam comes to breaking, or to buckling.

use std::f64::consts::PI;

use crate::frame::{Frame, Hold};
use crate::solve::{Material, MemberForces};

/// The round sections at a beam's two ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sections {
    pub d_a: f64,
    pub d_b: f64,
}

/// What a beam's forces do to it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MemberCheck {
    /// The highest stress at either end, MPa: axial plus bending, with the
    /// beam's stress concentration, combined with its torsion (von Mises).
    pub stress_mpa: f64,
    /// The squeeze on it over what buckles its run of beams: 0 pulled.
    pub buckling: f64,
}

/// The stress of a round section `d` across, carrying axial force `n`,
/// bending moment `m` and torque `t`, a concentration `kt` on the normal
/// stress.
fn stress_at(d: f64, n: f64, m: f64, t: f64, kt: f64) -> f64 {
    let area = PI * d * d / 4.0;
    let inertia = PI * d.powi(4) / 64.0;
    let sigma = kt * (n.abs() / area + m * (d / 2.0) / inertia);
    let tau = t * (d / 2.0) / (2.0 * inertia);
    (sigma * sigma + 3.0 * tau * tau).sqrt()
}

/// What `forces` do to beam `member` of `frame`, whose run buckles under
/// `critical_n` (N).
pub fn member_check(
    frame: &Frame,
    member: usize,
    forces: &MemberForces,
    critical_n: f64,
) -> MemberCheck {
    let m = frame.members()[member];
    let n = forces.axial_n();
    let t = forces.torque_nmm();
    let at_a = stress_at(m.d_a, n, forces.bending_nmm(false), t, m.kt);
    let at_b = stress_at(m.d_b, n, forces.bending_nmm(true), t, m.kt);
    // The moment goes linearly along the beam while its section narrows:
    // at mid length too.
    let mid_m = 0.5 * (forces.bending_nmm(false) + forces.bending_nmm(true));
    let at_mid = stress_at(0.5 * (m.d_a + m.d_b), n, mid_m, t, m.kt);
    let squeeze = (-n).max(0.0);
    MemberCheck {
        stress_mpa: at_a.max(at_b).max(at_mid),
        buckling: if critical_n > 0.0 {
            squeeze / critical_n
        } else {
            0.0
        },
    }
}

/// The load that buckles each beam's run (N): the beams joined end to end
/// through free nodes where nothing else meets, a column pinned at its two
/// ends, as long as the run and as stiff as its narrowest section.
pub fn buckling_runs(frame: &Frame, material: &Material) -> Vec<f64> {
    let members = frame.members();
    let holds = frame.holds();
    let mut at_node: Vec<Vec<usize>> = vec![Vec::new(); frame.nodes().len()];
    for (k, m) in members.iter().enumerate() {
        at_node[m.a].push(k);
        at_node[m.b].push(k);
    }
    // A node passes a run through when it is free and two beams meet at it.
    let passes = |node: usize| holds[node] == Hold::Free && at_node[node].len() == 2;
    let mut run_of = vec![usize::MAX; members.len()];
    let mut critical = vec![0.0; members.len()];
    for start in 0..members.len() {
        if run_of[start] != usize::MAX {
            continue;
        }
        let mut run = vec![start];
        run_of[start] = start;
        for from in [members[start].a, members[start].b] {
            let (mut node, mut came) = (from, start);
            while passes(node) {
                let next = at_node[node].iter().copied().find(|&k| k != came);
                let Some(next) = next else { break };
                if run_of[next] != usize::MAX {
                    break;
                }
                run_of[next] = start;
                run.push(next);
                let m = members[next];
                node = if m.a == node { m.b } else { m.a };
                came = next;
            }
        }
        let length: f64 = run.iter().map(|&k| frame.length(k)).sum();
        let d = run
            .iter()
            .map(|&k| members[k].d_a.min(members[k].d_b))
            .fold(f64::INFINITY, f64::min);
        let inertia = PI * d.powi(4) / 64.0;
        let p_cr = PI * PI * material.e_mpa * inertia / (length * length);
        for &k in &run {
            critical[k] = p_cr;
        }
    }
    critical
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The frame's stiffness, assembled and solved.

use faer::Mat;
use faer::Side;
use faer::prelude::Solve;
use faer::sparse::linalg::solvers::{Llt, SymbolicLlt};
use faer::sparse::{SparseColMat, Triplet};

#[cfg(test)]
use crate::element::to_global;
use crate::element::{M12, R3, cross, even_section, global_k, local_k, rotation, to_local};
use crate::frame::{Frame, Hold, Point};

/// The material of the beams.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Young's modulus, MPa.
    pub e_mpa: f64,
    /// Poisson's ratio.
    pub nu: f64,
}

impl Material {
    /// Resin cured in the vat, not yet post-cured: about 1 GPa measured by
    /// nanoindentation on a green DLP part. Only buckling and how far the
    /// frame moves depend on it.
    pub const GREEN_RESIN: Self = Self {
        e_mpa: 1000.0,
        nu: 0.35,
    };
}

/// What a load pulls on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A node, at the node.
    Node(usize),
    /// A body, at [`Load::at`].
    Body(usize),
}

/// A force `force` (N) pulling on `target` at point `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Load {
    pub target: Target,
    pub at: Point,
    pub force: [f64; 3],
}

/// One load case: the loads pulling at once.
pub type Loads = Vec<Load>;

/// What a beam carries, in its own axes (x from node `a` to node `b`): the
/// forces and moments its two ends take from their nodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemberForces(pub [f64; 12]);

impl MemberForces {
    /// The axial force, N: positive pulls the beam, negative squeezes it.
    pub fn axial_n(&self) -> f64 {
        self.0[6]
    }

    /// The bending moment at end `a` (`at_b` false) or `b`, N·mm.
    pub fn bending_nmm(&self, at_b: bool) -> f64 {
        let o = if at_b { 6 } else { 0 };
        self.0[o + 4].hypot(self.0[o + 5])
    }

    /// The torque, N·mm.
    pub fn torque_nmm(&self) -> f64 {
        self.0[9].abs()
    }

    /// `self` plus `other` times `k`: load cases add up.
    pub fn plus(&self, other: &Self, k: f64) -> Self {
        Self(std::array::from_fn(|i| self.0[i] + other.0[i] * k))
    }
}

/// Why a frame could not be solved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolveError {
    /// The stiffness is not positive definite: something held moves freely.
    Singular,
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Singular => f.write_str("the frame's stiffness is singular"),
        }
    }
}

impl std::error::Error for SolveError {}

/// A frame solved for its load cases.
#[derive(Clone, Debug)]
pub struct Solved {
    /// What each beam carries, per case: `None` for one nothing fixed holds.
    pub forces: Vec<Vec<Option<MemberForces>>>,
    /// How many beams nothing fixed holds, left out.
    pub left_out: usize,
    /// The force of each case's loads that nothing fixed holds, N.
    pub unheld_n: Vec<f64>,
    /// The worst case's `|K u − f| / |f|` after refinement.
    pub residual: f64,
    /// Degrees of freedom solved.
    pub dofs: usize,
    node_dofs: Vec<Option<usize>>,
    node_body: Vec<Option<(usize, Point)>>,
    body_dofs: Vec<Option<usize>>,
    u: Vec<Vec<f64>>,
}

impl Solved {
    /// How node `node` moves in case `case`: translations (mm), then
    /// rotations (rad). `None` for a node that does not move or that nothing
    /// fixed holds.
    pub fn displacement(&self, case: usize, node: usize) -> Option<[f64; 6]> {
        let u = &self.u[case];
        if let Some(base) = self.node_dofs[node] {
            return Some(std::array::from_fn(|i| u[base + i]));
        }
        let (body, r) = self.node_body[node]?;
        let base = self.body_dofs[body]?;
        let c: [f64; 6] = std::array::from_fn(|i| u[base + i]);
        let turn = cross([c[3], c[4], c[5]], r);
        Some([
            c[0] + turn[0],
            c[1] + turn[1],
            c[2] + turn[2],
            c[3],
            c[4],
            c[5],
        ])
    }
}

type M6 = [[f64; 6]; 6];

/// How an end moves with the degrees of freedom of what holds it: none for a
/// fixed one; its own for a free one; for a point of a body,
/// `u = u_c + θ × r`, `θ = θ_c`.
#[derive(Clone, Copy)]
enum EndMap {
    Fixed,
    Own(usize),
    Body(usize, M6),
}

fn body_map(r: Point) -> M6 {
    let mut m = [[0.0; 6]; 6];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    // -skew(r)
    m[0][4] = r[2];
    m[0][5] = -r[1];
    m[1][3] = -r[2];
    m[1][5] = r[0];
    m[2][3] = r[1];
    m[2][4] = -r[0];
    m
}

impl EndMap {
    fn base(&self) -> Option<usize> {
        match self {
            Self::Fixed => None,
            Self::Own(base) | Self::Body(base, _) => Some(*base),
        }
    }

    /// Row `i` of the map times `v`: the end's motion from its owner's.
    fn apply(&self, v: &[f64; 6]) -> [f64; 6] {
        match self {
            Self::Fixed => [0.0; 6],
            Self::Own(_) => *v,
            Self::Body(_, m) => std::array::from_fn(|i| (0..6).map(|j| m[i][j] * v[j]).sum()),
        }
    }
}

struct Prepared {
    r: R3,
    k: M12,
    ends: [EndMap; 2],
}

/// Solves `frame` of `material` for every load case in `cases`, the
/// stiffness factorised once.
pub fn solve(frame: &Frame, material: &Material, cases: &[Loads]) -> Result<Solved, SolveError> {
    let (held_nodes, held_bodies) = frame.grounded();
    let nodes = frame.nodes();
    let holds = frame.holds();
    let bodies = frame.bodies();

    // Number the degrees of freedom: free held nodes, free held bodies.
    let mut n = 0;
    let mut node_dofs = vec![None; nodes.len()];
    let mut node_body = vec![None; nodes.len()];
    for i in 0..nodes.len() {
        match holds[i] {
            Hold::Free if held_nodes[i] => {
                node_dofs[i] = Some(n);
                n += 6;
            }
            Hold::Body(b) => {
                let at = bodies[b].at;
                let p = nodes[i];
                node_body[i] = Some((b, [p[0] - at[0], p[1] - at[1], p[2] - at[2]]));
            }
            _ => {}
        }
    }
    let mut body_dofs = vec![None; bodies.len()];
    for (b, body) in bodies.iter().enumerate() {
        if !body.fixed && held_bodies[b] {
            body_dofs[b] = Some(n);
            n += 6;
        }
    }
    let end_of = |i: usize| match holds[i] {
        Hold::Free => node_dofs[i].map_or(EndMap::Fixed, EndMap::Own),
        Hold::Fixed => EndMap::Fixed,
        Hold::Body(b) => match body_dofs[b] {
            Some(base) => EndMap::Body(base, body_map(node_body[i].expect("a body's node").1)),
            None => EndMap::Fixed,
        },
    };

    // Assemble.
    let mut left_out = 0;
    let mut prepared: Vec<Option<Prepared>> = Vec::with_capacity(frame.members().len());
    let mut trips: Vec<(usize, usize, f64)> = Vec::with_capacity(frame.members().len() * 144);
    for m in frame.members() {
        if !held_nodes[m.a] && !held_nodes[m.b] {
            left_out += 1;
            prepared.push(None);
            continue;
        }
        let (pa, pb) = (nodes[m.a], nodes[m.b]);
        let l = crate::frame::distance(pa, pb);
        let r = rotation(pa, pb);
        let (area, inertia) = even_section(m.d_a, m.d_b);
        let kl = local_k(l, area, inertia, material.e_mpa, material.nu);
        let kg = global_k(&r, &kl);
        let ends = [end_of(m.a), end_of(m.b)];
        for p in 0..2 {
            let Some(bp) = ends[p].base() else { continue };
            for q in 0..2 {
                let Some(bq) = ends[q].base() else { continue };
                // Mpᵀ · Kpq · Mq
                let mut block = [[0.0; 6]; 6];
                for i in 0..6 {
                    for j in 0..6 {
                        block[i][j] = kg[p * 6 + i][q * 6 + j];
                    }
                }
                if let EndMap::Body(_, mq) = ends[q] {
                    let mut right = [[0.0; 6]; 6];
                    for i in 0..6 {
                        for j in 0..6 {
                            right[i][j] = (0..6).map(|s| block[i][s] * mq[s][j]).sum();
                        }
                    }
                    block = right;
                }
                if let EndMap::Body(_, mp) = ends[p] {
                    let mut left = [[0.0; 6]; 6];
                    for i in 0..6 {
                        for j in 0..6 {
                            left[i][j] = (0..6).map(|s| mp[s][i] * block[s][j]).sum();
                        }
                    }
                    block = left;
                }
                for (i, row) in block.iter().enumerate() {
                    for (j, &v) in row.iter().enumerate() {
                        if v != 0.0 {
                            trips.push((bp + i, bq + j, v));
                        }
                    }
                }
            }
        }
        prepared.push(Some(Prepared { r, k: kl, ends }));
    }

    // The loads.
    let mut unheld_n = vec![0.0; cases.len()];
    let mut f = Mat::<f64>::zeros(n, cases.len());
    for (c, loads) in cases.iter().enumerate() {
        for load in loads {
            let (body, at) = match load.target {
                Target::Node(i) => match (node_dofs[i], holds[i]) {
                    (Some(base), _) => {
                        for k in 0..3 {
                            f[(base + k, c)] += load.force[k];
                        }
                        continue;
                    }
                    (None, Hold::Body(b)) => (b, nodes[i]),
                    _ => {
                        unheld_n[c] += norm(load.force);
                        continue;
                    }
                },
                Target::Body(b) => (b, load.at),
            };
            let Some(base) = body_dofs[body] else {
                if !bodies[body].fixed {
                    unheld_n[c] += norm(load.force);
                }
                continue;
            };
            let o = bodies[body].at;
            let moment = cross([at[0] - o[0], at[1] - o[1], at[2] - o[2]], load.force);
            for k in 0..3 {
                f[(base + k, c)] += load.force[k];
                f[(base + 3 + k, c)] += moment[k];
            }
        }
    }

    let mut u = Mat::<f64>::zeros(n, cases.len());
    let mut residual: f64 = 0.0;
    if n > 0 {
        trips.sort_unstable_by_key(|&(r, c, _)| (c, r));
        let mut merged: Vec<Triplet<usize, usize, f64>> = Vec::with_capacity(trips.len() / 2);
        for (r, c, v) in trips {
            match merged.last_mut() {
                Some(last) if last.row == r && last.col == c => last.val += v,
                _ => merged.push(Triplet::new(r, c, v)),
            }
        }
        // Jacobi scaling: a body turns on levers of tens of mm over necks of
        // tenths, and the matrix loses digits unscaled.
        let mut diag = vec![1.0; n];
        for t in &merged {
            if t.row == t.col {
                diag[t.row] = t.val;
            }
        }
        if diag.iter().any(|d| !(*d > 0.0)) {
            return Err(SolveError::Singular);
        }
        let scale: Vec<f64> = diag.iter().map(|d| 1.0 / d.sqrt()).collect();
        let scaled: Vec<Triplet<usize, usize, f64>> = merged
            .iter()
            .map(|t| Triplet::new(t.row, t.col, t.val * scale[t.row] * scale[t.col]))
            .collect();
        let k_raw = SparseColMat::<usize, f64>::try_new_from_triplets(n, n, &merged)
            .map_err(|_| SolveError::Singular)?;
        let k = SparseColMat::<usize, f64>::try_new_from_triplets(n, n, &scaled)
            .map_err(|_| SolveError::Singular)?;
        let symbolic =
            SymbolicLlt::try_new(k.symbolic(), Side::Lower).map_err(|_| SolveError::Singular)?;
        let llt = Llt::try_new_with_symbolic(symbolic, k.as_ref(), Side::Lower)
            .map_err(|_| SolveError::Singular)?;
        let cols = cases.len();
        let fs = Mat::<f64>::from_fn(n, cols, |i, c| f[(i, c)] * scale[i]);
        let y = llt.solve(&fs);
        u = Mat::<f64>::from_fn(n, cols, |i, c| y[(i, c)] * scale[i]);
        // One step of iterative refinement.
        let r = &f - &k_raw * &u;
        let rs = Mat::<f64>::from_fn(n, cols, |i, c| r[(i, c)] * scale[i]);
        let dy = llt.solve(&rs);
        for c in 0..cols {
            for i in 0..n {
                u[(i, c)] += dy[(i, c)] * scale[i];
            }
        }
        let r = &f - &k_raw * &u;
        for c in 0..cols {
            let f_norm = (0..n).map(|i| f[(i, c)].powi(2)).sum::<f64>().sqrt();
            if f_norm > 0.0 {
                let r_norm = (0..n).map(|i| r[(i, c)].powi(2)).sum::<f64>().sqrt();
                residual = residual.max(r_norm / f_norm);
            }
        }
        if !residual.is_finite() {
            return Err(SolveError::Singular);
        }
    }

    // What each beam carries.
    let u: Vec<Vec<f64>> = (0..cases.len())
        .map(|c| (0..n).map(|i| u[(i, c)]).collect())
        .collect();
    let forces = u
        .iter()
        .map(|u| {
            prepared
                .iter()
                .map(|p| {
                    let p = p.as_ref()?;
                    let mut ug = [0.0; 12];
                    for (e, end) in p.ends.iter().enumerate() {
                        let Some(base) = end.base() else { continue };
                        let own: [f64; 6] = std::array::from_fn(|i| u[base + i]);
                        ug[e * 6..e * 6 + 6].copy_from_slice(&end.apply(&own));
                    }
                    let ul = to_local(&p.r, &ug);
                    let fl: [f64; 12] =
                        std::array::from_fn(|i| (0..12).map(|j| p.k[i][j] * ul[j]).sum());
                    Some(MemberForces(fl))
                })
                .collect()
        })
        .collect();
    Ok(Solved {
        forces,
        left_out,
        unheld_n,
        residual,
        dofs: n,
        node_dofs,
        node_body,
        body_dofs,
        u,
    })
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[cfg(test)]
/// The forces of beam `member` in world axes, at each end, as its nodes
/// feel them pushed back: what the frame's other beams and holds take.
pub(crate) fn world_forces(frame: &Frame, member: usize, forces: &MemberForces) -> [f64; 12] {
    let m = frame.members()[member];
    let r = rotation(frame.nodes()[m.a], frame.nodes()[m.b]);
    to_global(&r, &forces.0)
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Nodes, beams and bodies.

use std::collections::HashMap;

/// A point, in mm.
pub type Point = [f64; 3];

/// Nodes closer than this are one node (mm): a beam this short would ruin
/// the matrix, and the slicer draws nothing that small.
pub const MERGE_MM: f64 = 0.05;

/// What holds a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// Nothing: it moves with the beams that meet at it.
    Free,
    /// The raft or the plate: it does not move.
    Fixed,
    /// It is a point of rigid body `.0`, and moves with it.
    Body(usize),
}

/// A round beam between two nodes, its diameter going linearly from `d_a`
/// at node `a` to `d_b` at node `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    pub a: usize,
    pub b: usize,
    pub d_a: f64,
    pub d_b: f64,
    /// The stress concentration where it meets what it joins: 1 for none.
    pub kt: f64,
}

/// A rigid body: the model printed so far, or part of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Its reference point: where its load is reported from.
    pub at: Point,
    /// Something else holds it (the plate): it does not move.
    pub fixed: bool,
}

/// Nodes, beams between them and rigid bodies. Built a node and a beam at a
/// time; nodes closer than [`MERGE_MM`] are merged as they come.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    nodes: Vec<Point>,
    holds: Vec<Hold>,
    members: Vec<Member>,
    bodies: Vec<Body>,
    cells: HashMap<[i64; 3], Vec<usize>>,
}

fn cell_of(p: Point) -> [i64; 3] {
    p.map(|v| (v / MERGE_MM).floor() as i64)
}

pub(crate) fn distance(a: Point, b: Point) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Frame {
    /// An empty frame.
    pub fn new() -> Self {
        Self::default()
    }

    /// The node at `p`: one already within [`MERGE_MM`] of it, the nearest,
    /// or a new free one.
    pub fn node(&mut self, p: Point) -> usize {
        if let Some(found) = self.node_near(p) {
            return found;
        }
        let index = self.nodes.len();
        self.nodes.push(p);
        self.holds.push(Hold::Free);
        self.cells.entry(cell_of(p)).or_default().push(index);
        index
    }

    /// The nearest node within [`MERGE_MM`] of `p`.
    pub fn node_near(&self, p: Point) -> Option<usize> {
        let [cx, cy, cz] = cell_of(p);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cells.get(&[cx + dx, cy + dy, cz + dz]) else {
                        continue;
                    };
                    for &index in list {
                        let d = distance(self.nodes[index], p);
                        if d <= MERGE_MM && best.is_none_or(|(b, _)| d < b) {
                            best = Some((d, index));
                        }
                    }
                }
            }
        }
        best.map(|(_, index)| index)
    }

    /// Holds node `node` fixed.
    pub fn fix(&mut self, node: usize) {
        self.holds[node] = Hold::Fixed;
    }

    /// A rigid body, its reference point at `at`.
    pub fn body(&mut self, at: Point, fixed: bool) -> usize {
        self.bodies.push(Body { at, fixed });
        self.bodies.len() - 1
    }

    /// Ties node `node` to body `body`. A fixed node stays fixed.
    pub fn attach(&mut self, node: usize, body: usize) {
        if self.holds[node] != Hold::Fixed {
            self.holds[node] = Hold::Body(body);
        }
    }

    /// A beam from node `a` to node `b`, `None` when they are one node.
    pub fn member(&mut self, a: usize, b: usize, d_a: f64, d_b: f64, kt: f64) -> Option<usize> {
        if a == b || !(d_a > 0.0 && d_b > 0.0) {
            return None;
        }
        self.members.push(Member {
            a,
            b,
            d_a,
            d_b,
            kt: kt.max(1.0),
        });
        Some(self.members.len() - 1)
    }

    pub fn nodes(&self) -> &[Point] {
        &self.nodes
    }

    pub fn holds(&self) -> &[Hold] {
        &self.holds
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    /// The length of beam `member`.
    pub fn length(&self, member: usize) -> f64 {
        let m = self.members[member];
        distance(self.nodes[m.a], self.nodes[m.b])
    }

    /// Which nodes and bodies something fixed holds through the beams:
    /// `(nodes, bodies)`. Whatever is not held carries nothing and is left
    /// out of a solve.
    pub(crate) fn grounded(&self) -> (Vec<bool>, Vec<bool>) {
        let (nodes, bodies) = (self.nodes.len(), self.bodies.len());
        let ground = nodes + bodies;
        let mut parent: Vec<usize> = (0..=ground).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let unit = |i: usize| match self.holds[i] {
            Hold::Free => i,
            Hold::Fixed => ground,
            Hold::Body(b) if self.bodies[b].fixed => ground,
            Hold::Body(b) => nodes + b,
        };
        for b in 0..bodies {
            if self.bodies[b].fixed {
                let (x, y) = (find(&mut parent, nodes + b), find(&mut parent, ground));
                parent[x] = y;
            }
        }
        for m in &self.members {
            let (x, y) = (find(&mut parent, unit(m.a)), find(&mut parent, unit(m.b)));
            if x != y {
                parent[x] = y;
            }
        }
        let root = find(&mut parent, ground);
        let held_nodes = (0..nodes)
            .map(|i| {
                let u = unit(i);
                find(&mut parent, u) == root
            })
            .collect();
        let held_bodies = (0..bodies)
            .map(|b| find(&mut parent, nodes + b) == root)
            .collect();
        (held_nodes, held_bodies)
    }
}

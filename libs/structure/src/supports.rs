/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A support set as a frame, checked while a layer peels (#243, #244).

use std::collections::HashMap;
use std::f64::consts::TAU;

use crate::check::{buckling_runs, member_check};
use crate::frame::{Frame, Hold, Point, distance};
use crate::solve::{Load, Material, SolveError, Target, solve};

/// The stress concentration where a tip's neck meets the model and its
/// cone: the section changes sharply there. Between 1.5 and 2.5 by the usual
/// shoulder tables; to measure.
pub const NECK_KT: f64 = 2.0;

/// How far above the layer that peels a support is still kept (mm): a tip
/// ends a little into the model, up to a layer or two over the layer whose
/// region it belongs to.
const CLIP_SLACK_MM: f64 = 0.5;

/// Two tip ends this close are the same tip (mm).
const SAME_TIP_MM: f64 = 0.01;

/// Where a support enters the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphContact {
    /// The node on the model's surface: tied to the body the tip holds.
    pub node: usize,
    /// Where the tip ends inside the model: how a body's tips find it.
    pub tip: Point,
    /// The support it belongs to.
    pub owner: usize,
}

/// Which part of a support a beam is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Part {
    /// Its shaft, a brace, a root.
    #[default]
    Shaft,
    /// Its tip: the cone and the neck to the model.
    Tip,
}

/// Where a support stands on the model instead of the plate: its root.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphFoot {
    /// The root's node: tied to the body it stands on.
    pub node: usize,
    /// Where it stands: how a body's feet find it.
    pub at: Point,
    /// The support it belongs to.
    pub owner: usize,
}

/// A set of supports as a frame: every beam named by the support it is part
/// of, roots fixed in the raft, tips ending at the model's surface.
#[derive(Clone, Debug, Default)]
pub struct SupportGraph {
    frame: Frame,
    owner_of: Vec<usize>,
    part_of: Vec<Part>,
    owners: Vec<String>,
    contacts: Vec<GraphContact>,
    feet: Vec<GraphFoot>,
}

impl SupportGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// The index of support `name`, added the first time.
    pub fn owner(&mut self, name: &str) -> usize {
        if let Some(at) = self.owners.iter().rposition(|o| o == name) {
            return at;
        }
        self.owners.push(name.to_owned());
        self.owners.len() - 1
    }

    /// The node at `p` (see [`Frame::node`]).
    pub fn node(&mut self, p: Point) -> usize {
        self.frame.node(p)
    }

    /// Holds node `node` in the raft.
    pub fn fix(&mut self, node: usize) {
        self.frame.fix(node);
    }

    /// A beam of support `owner`'s shaft (see [`Frame::member`]).
    pub fn member(&mut self, a: usize, b: usize, d_a: f64, d_b: f64, kt: f64, owner: usize) {
        self.part_member(a, b, d_a, d_b, kt, owner, Part::Shaft);
    }

    /// A beam of support `owner`'s tip.
    pub fn tip_member(&mut self, a: usize, b: usize, d_a: f64, d_b: f64, kt: f64, owner: usize) {
        self.part_member(a, b, d_a, d_b, kt, owner, Part::Tip);
    }

    #[allow(clippy::too_many_arguments)]
    fn part_member(
        &mut self,
        a: usize,
        b: usize,
        d_a: f64,
        d_b: f64,
        kt: f64,
        owner: usize,
        part: Part,
    ) {
        if self.frame.member(a, b, d_a, d_b, kt).is_some() {
            self.owner_of.push(owner);
            self.part_of.push(part);
        }
    }

    /// Beams of support `owner` along `line`, each point with the diameter
    /// of the span that ends there going down the line: the nodes it made.
    pub fn line(&mut self, line: &[(Point, f64)], owner: usize) -> Vec<usize> {
        let nodes: Vec<usize> = line.iter().map(|(p, _)| self.node(*p)).collect();
        for k in 1..nodes.len() {
            let d = line[k].1;
            self.member(nodes[k - 1], nodes[k], d, d, 1.0, owner);
        }
        nodes
    }

    /// The tip of support `owner` that enters the model at node `node` and
    /// ends at `tip`.
    pub fn contact(&mut self, node: usize, tip: Point, owner: usize) {
        self.contacts.push(GraphContact { node, tip, owner });
    }

    /// Support `owner` stands on the model at `at`, its root at node `node`.
    pub fn foot(&mut self, node: usize, at: Point, owner: usize) {
        self.feet.push(GraphFoot { node, at, owner });
    }

    pub fn feet(&self) -> &[GraphFoot] {
        &self.feet
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// The support each beam belongs to.
    pub fn owner_of(&self) -> &[usize] {
        &self.owner_of
    }

    /// The part of its support each beam is.
    pub fn part_of(&self) -> &[Part] {
        &self.part_of
    }

    pub fn owners(&self) -> &[String] {
        &self.owners
    }

    pub fn contacts(&self) -> &[GraphContact] {
        &self.contacts
    }

    /// The graph with every point moved by `f`.
    pub fn mapped(&self, f: impl Fn(Point) -> Point) -> Self {
        let mut out = Self {
            owners: self.owners.clone(),
            ..Self::default()
        };
        let nodes: Vec<usize> = self.frame.nodes().iter().map(|p| out.node(f(*p))).collect();
        for (i, hold) in self.frame.holds().iter().enumerate() {
            if *hold == Hold::Fixed {
                out.fix(nodes[i]);
            }
        }
        for ((m, owner), part) in self
            .frame
            .members()
            .iter()
            .zip(&self.owner_of)
            .zip(&self.part_of)
        {
            out.part_member(nodes[m.a], nodes[m.b], m.d_a, m.d_b, m.kt, *owner, *part);
        }
        out.contacts = self
            .contacts
            .iter()
            .map(|c| GraphContact {
                node: nodes[c.node],
                tip: f(c.tip),
                owner: c.owner,
            })
            .collect();
        out.feet = self
            .feet
            .iter()
            .map(|foot| GraphFoot {
                node: nodes[foot.node],
                at: f(foot.at),
                owner: foot.owner,
            })
            .collect();
        out
    }
}

/// A printed body while a layer peels.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyLoad {
    /// What the layer pulls it with, N: 0 for one this layer does not
    /// reach, held by its tips all the same.
    pub force_n: f64,
    /// Where the pull centres, on the layer.
    pub at: Point,
    /// The plate holds it: it does not move.
    pub held: bool,
    /// Where its tips end in it.
    pub tips: Vec<Point>,
    /// Where supports stand on it.
    pub feet: Vec<Point>,
}

/// How a layer is checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CheckOptions {
    pub material: Material,
    /// The stress that breaks a support in the vat, MPa.
    pub strength_mpa: f64,
    /// The pull's sideways share at the film, in any direction.
    pub lateral_share: f64,
    /// How many sideways directions are tried, evenly around.
    pub directions: usize,
}

/// What a layer's peel does to the supports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerCheck {
    /// Per graph, per support: the highest utilisation of its beams, the
    /// stress over the strength or the squeeze over what buckles it,
    /// whichever is worse; 0 for one the layer does not reach.
    pub owners: Vec<Vec<f64>>,
    /// Per graph, per support: the part of it that is worst off.
    pub parts: Vec<Vec<Part>>,
    /// The highest of them.
    pub worst: f64,
    /// Tips of the bodies found in the graphs, and not.
    pub tips_found: usize,
    pub tips_missed: usize,
    /// Bodies pulled that nothing fixed holds.
    pub bodies_unheld: usize,
    /// The solve's `|K u − f| / |f|`.
    pub residual: f64,
}

/// Checks `graphs` while the layer whose top is at `z_mm` peels, pulling
/// `bodies`: each graph cut just above the layer, the tips tied to the
/// bodies they end in, each body pulled straight up by its force plus a
/// sideways share, the worst of every direction kept.
pub fn check_layer(
    graphs: &[SupportGraph],
    z_mm: f64,
    bodies: &[BodyLoad],
    options: &CheckOptions,
) -> Result<LayerCheck, SolveError> {
    let top = z_mm + CLIP_SLACK_MM;
    let mut frame = Frame::new();
    // Each beam kept: its graph, support and part.
    let mut kept: Vec<(usize, usize, Part)> = Vec::new();
    // Each graph's node in the frame.
    let mut node_maps: Vec<Vec<Option<usize>>> = Vec::with_capacity(graphs.len());
    for (g, graph) in graphs.iter().enumerate() {
        let source = graph.frame();
        let map: Vec<Option<usize>> = source
            .nodes()
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (p[2] <= top).then(|| {
                    let node = frame.node(*p);
                    if source.holds()[i] == Hold::Fixed {
                        frame.fix(node);
                    }
                    node
                })
            })
            .collect();
        for ((m, &owner), &part) in source
            .members()
            .iter()
            .zip(graph.owner_of())
            .zip(graph.part_of())
        {
            let (a, b) = match (map[m.a], map[m.b]) {
                (Some(a), Some(b)) => (a, b),
                (None, None) => continue,
                // Cut where it crosses the top.
                (Some(kept_end), None) | (None, Some(kept_end)) => {
                    let (low, high) = if map[m.a].is_some() {
                        (m.a, m.b)
                    } else {
                        (m.b, m.a)
                    };
                    let (pl, ph) = (source.nodes()[low], source.nodes()[high]);
                    let t = ((top - pl[2]) / (ph[2] - pl[2])).clamp(0.0, 1.0);
                    let cut = std::array::from_fn(|k| pl[k] + (ph[k] - pl[k]) * t);
                    (kept_end, frame.node(cut))
                }
            };
            // A beam cut from its end b keeps that end's section first.
            let (d_a, d_b) = if map[m.a].is_none() {
                (m.d_b, m.d_a)
            } else {
                (m.d_a, m.d_b)
            };
            if frame.member(a, b, d_a, d_b, m.kt).is_some() {
                kept.push((g, owner, part));
            }
        }
        node_maps.push(map);
    }

    // The tips, found by where they end.
    let key = |p: Point| p.map(|v| (v / SAME_TIP_MM).floor() as i64);
    // Each graph's tips, then its feet: (graph, node, point).
    let mut ends: HashMap<[i64; 3], Vec<(usize, usize, Point)>> = HashMap::new();
    let mut feet: HashMap<[i64; 3], Vec<(usize, usize, Point)>> = HashMap::new();
    for (g, graph) in graphs.iter().enumerate() {
        for contact in graph.contacts() {
            ends.entry(key(contact.tip))
                .or_default()
                .push((g, contact.node, contact.tip));
        }
        for foot in graph.feet() {
            feet.entry(key(foot.at))
                .or_default()
                .push((g, foot.node, foot.at));
        }
    }
    let find = |within: &HashMap<[i64; 3], Vec<(usize, usize, Point)>>, p: Point| {
        let [x, y, z] = key(p);
        let mut best: Option<(f64, (usize, usize))> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &(g, node, q) in within.get(&[x + dx, y + dy, z + dz]).into_iter().flatten()
                    {
                        let d = distance(q, p);
                        if d <= SAME_TIP_MM && best.is_none_or(|(b, _)| d < b) {
                            best = Some((d, (g, node)));
                        }
                    }
                }
            }
        }
        best.map(|(_, found)| found)
    };
    let mut check = LayerCheck {
        owners: graphs.iter().map(|g| vec![0.0; g.owners().len()]).collect(),
        parts: graphs
            .iter()
            .map(|g| vec![Part::Shaft; g.owners().len()])
            .collect(),
        ..LayerCheck::default()
    };
    let mut pulled: Vec<(usize, f64, Point)> = Vec::new();
    for body in bodies {
        let index = frame.body(body.at, body.held);
        for &tip in &body.tips {
            match find(&ends, tip).and_then(|(g, node)| node_maps[g][node]) {
                Some(node) => {
                    check.tips_found += 1;
                    frame.attach(node, index);
                }
                None => check.tips_missed += 1,
            }
        }
        for &foot in &body.feet {
            if let Some(node) = find(&feet, foot).and_then(|(g, node)| node_maps[g][node]) {
                frame.attach(node, index);
            }
        }
        if !body.held && body.force_n > 0.0 {
            pulled.push((index, body.force_n, body.at));
        }
    }

    let case = |dir: [f64; 3], share: f64| -> Vec<Load> {
        pulled
            .iter()
            .map(|&(body, force, at)| Load {
                target: Target::Body(body),
                at,
                force: dir.map(|v| v * force * share),
            })
            .collect()
    };
    let cases = [
        case([0.0, 0.0, 1.0], 1.0),
        case([1.0, 0.0, 0.0], options.lateral_share),
        case([0.0, 1.0, 0.0], options.lateral_share),
    ];
    let solved = solve(&frame, &options.material, &cases)?;
    check.residual = solved.residual;
    let (_, held_bodies) = frame.grounded();
    check.bodies_unheld = pulled.iter().filter(|(b, _, _)| !held_bodies[*b]).count();
    let critical = buckling_runs(&frame, &options.material);
    let directions = options.directions.max(1);
    for (k, &(g, owner, part)) in kept.iter().enumerate() {
        let (Some(up), Some(x), Some(y)) = (
            solved.forces[0][k],
            solved.forces[1][k],
            solved.forces[2][k],
        ) else {
            continue;
        };
        let mut worst: f64 = 0.0;
        for d in 0..directions {
            let angle = TAU * d as f64 / directions as f64;
            let forces = up.plus(&x, angle.cos()).plus(&y, angle.sin());
            let m = member_check(&frame, k, &forces, critical[k]);
            worst = worst
                .max(m.stress_mpa / options.strength_mpa)
                .max(m.buckling);
        }
        let slot = &mut check.owners[g][owner];
        if worst > *slot {
            *slot = worst;
            check.parts[g][owner] = part;
        }
        check.worst = check.worst.max(worst);
    }
    Ok(check)
}

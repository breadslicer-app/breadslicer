//! Splits each model's shafts into bracing groups.
//!
//! Shafts are points in the plan view, linked to every other within brace
//! reach. Seeds are laid on a square grid over the model's shafts, each
//! nudged by a repeatable per-model jitter and snapped to the nearest shaft;
//! a linked cluster no seed landed in gets one at the shaft nearest its
//! centroid. Every seed then grows its group breadth first over the links,
//! all seeds at once, so each shaft joins the seed that reaches it first.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use bread_support_geom::geom::math_utils::{clamp, js_max, js_sin};
use bread_support_placement::volume_analysis::islands::voxel_footprint::number_key;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// A shaft as grouping sees it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GroupNode {
    pub support_id: String,
    pub model_id: String,
    pub point: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GroupingSettings {
    pub seed_spacing_mm: f64,
    pub seed_jitter_mm: f64,
    /// Shafts farther apart than this are not linked.
    pub max_neighbor_distance_mm: f64,
}

const SLACK: f64 = 0.000001;

/// Ids in UTF-16 code-unit order, the order every list of ids here is kept in.
fn id_order(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// 32-bit FNV-1a of `text`'s UTF-16 code units: a model's jitter seed.
fn fnv1a(text: &str) -> u32 {
    text.encode_utf16().fold(2166136261u32, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(16777619)
    })
}

/// A repeatable number in `[0, 1)` for grid cell (`a`, `b`) under `seed`.
fn cell_noise(a: f64, b: f64, seed: f64) -> f64 {
    let wave = js_sin(a * 12.9898 + b * 78.233 + seed * 0.0001) * 43758.5453123;
    wave - wave.floor()
}

fn dist_sq(a: Vec2, b: Vec2) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

/// Shafts bucketed by square plan cells, so a search only reads the cells
/// around a point. Cell coordinates are keyed by value, so -0 and 0 (and
/// every NaN) share a cell.
struct CellGrid<'n> {
    size: f64,
    cells: HashMap<(u64, u64), Vec<&'n GroupNode>>,
}

impl<'n> CellGrid<'n> {
    fn new(nodes: &'n [GroupNode], size: f64) -> Self {
        let mut grid = CellGrid {
            size,
            cells: HashMap::new(),
        };
        for node in nodes {
            let cell = grid.cell_of(node.point);
            grid.cells.entry(cell_key(cell)).or_default().push(node);
        }
        grid
    }

    fn cell_of(&self, p: Vec2) -> (f64, f64) {
        ((p.x / self.size).floor(), (p.y / self.size).floor())
    }

    /// The shafts of the cells up to `radius` cells from `p`'s, column by
    /// column, each cell's in the order they were given.
    fn around(&self, p: Vec2, radius: i64) -> impl Iterator<Item = &'n GroupNode> + '_ {
        let (cx, cy) = self.cell_of(p);
        (-radius..=radius).flat_map(move |ox| {
            (-radius..=radius).flat_map(move |oy| {
                self.cells
                    .get(&cell_key((cx + ox as f64, cy + oy as f64)))
                    .into_iter()
                    .flatten()
                    .copied()
            })
        })
    }
}

fn cell_key((x, y): (f64, f64)) -> (u64, u64) {
    (number_key(x), number_key(y))
}

/// Each shaft's links: the other ids within `reach_mm`, in id order.
fn link_shafts(nodes: &[GroupNode], reach_mm: f64) -> HashMap<String, Vec<String>> {
    // A NaN reach must not put every shaft in one cell.
    let reach = if reach_mm.is_finite() {
        js_max(reach_mm, 0.1)
    } else {
        0.1
    };
    let grid = CellGrid::new(nodes, reach);
    let limit = reach * reach + SLACK;
    let mut links: HashMap<String, Vec<String>> = HashMap::new();
    for node in nodes {
        let found = links.entry(node.support_id.clone()).or_default();
        for other in grid.around(node.point, 1) {
            if other.support_id != node.support_id && !(dist_sq(node.point, other.point) > limit) {
                found.push(other.support_id.clone());
            }
        }
    }
    for found in links.values_mut() {
        found.sort_by(|a, b| id_order(a, b));
        found.dedup();
    }
    links
}

fn links_of<'l>(links: &'l HashMap<String, Vec<String>>, id: &str) -> &'l [String] {
    links.get(id).map_or(&[], Vec::as_slice)
}

/// The linked clusters of shafts, each in breadth-first order from its
/// first shaft.
fn clusters(nodes: &[GroupNode], links: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut found = Vec::new();
    for node in nodes {
        if !visited.insert(node.support_id.as_str()) {
            continue;
        }
        let mut cluster = vec![node.support_id.clone()];
        let mut next = 0;
        while let Some(id) = cluster.get(next) {
            next += 1;
            let mut reached = Vec::new();
            for neighbour in links_of(links, id) {
                if visited.insert(neighbour.as_str()) {
                    reached.push(neighbour.clone());
                }
            }
            cluster.extend(reached);
        }
        found.push(cluster);
    }
    found
}

/// The first of `nodes` nearest `target`.
fn nearest_of(target: Vec2, nodes: &[&GroupNode]) -> Option<String> {
    let mut best: Option<&GroupNode> = None;
    let mut best_d = f64::INFINITY;
    for &node in nodes {
        let d = dist_sq(target, node.point);
        if d < best_d {
            best = Some(node);
            best_d = d;
        }
    }
    best.map(|node| node.support_id.clone())
}

/// The jittered grid seeds of one model's shafts, each snapped to the
/// nearest shaft within three quarters of the spacing.
fn grid_seeds(nodes: &[GroupNode], settings: &GroupingSettings) -> HashSet<String> {
    let mut seeds = HashSet::new();
    let Some(first) = nodes.first() else {
        return seeds;
    };
    let (mut lo, mut hi) = (first.point, first.point);
    for node in nodes {
        let p = node.point;
        if p.x < lo.x {
            lo.x = p.x;
        }
        if p.x > hi.x {
            hi.x = p.x;
        }
        if p.y < lo.y {
            lo.y = p.y;
        }
        if p.y > hi.y {
            hi.y = p.y;
        }
    }
    let spacing = js_max(settings.seed_spacing_mm, 0.25);
    let jitter = clamp(settings.seed_jitter_mm, 0.0, spacing * 0.49);
    let snap = js_max(spacing * 0.75, 0.25);
    let grid = CellGrid::new(nodes, js_max(snap, 0.1));
    let cells_to_search = js_max(1.0, (snap / grid.size).ceil()) as i64;
    let snap_limit = snap * snap + SLACK;
    let model_seed = f64::from(fnv1a(&first.model_id));

    let mut column = 0.0;
    let mut x = lo.x;
    while x <= hi.x + SLACK {
        let mut row = 0.0;
        let mut y = lo.y;
        while y <= hi.y + SLACK {
            let target = Vec2 {
                x: x + (cell_noise(column, row, model_seed) * 2.0 - 1.0) * jitter,
                y: y + (cell_noise(column, row, model_seed + 7919.0) * 2.0 - 1.0) * jitter,
            };
            let mut best: Option<&GroupNode> = None;
            let mut best_d = snap_limit;
            for node in grid.around(target, cells_to_search) {
                let d = dist_sq(target, node.point);
                if !(d > snap_limit || d >= best_d) {
                    best = Some(node);
                    best_d = d;
                }
            }
            if let Some(node) = best {
                seeds.insert(node.support_id.clone());
            }
            y += spacing;
            row += 1.0;
        }
        x += spacing;
        column += 1.0;
    }
    seeds
}

/// Grows every seed's group breadth first over the links, seeds in id
/// order; a shaft no seed reaches joins a linked shaft's group if it has
/// one, else stands alone. Returns each shaft's seed.
fn grow_groups(
    nodes: &[GroupNode],
    links: &HashMap<String, Vec<String>>,
    seeds: &HashSet<String>,
) -> HashMap<String, String> {
    let mut ordered: Vec<&String> = seeds.iter().collect();
    ordered.sort_by(|a, b| id_order(a, b));
    let mut seed_of: HashMap<String, String> = HashMap::new();
    let mut frontier: Vec<(String, String)> = Vec::new();
    for seed in ordered {
        seed_of.insert(seed.clone(), seed.clone());
        frontier.push((seed.clone(), seed.clone()));
    }
    let mut next = 0;
    while let Some((id, seed)) = frontier.get(next).cloned() {
        next += 1;
        for neighbour in links_of(links, &id) {
            if !seed_of.contains_key(neighbour) {
                seed_of.insert(neighbour.clone(), seed.clone());
                frontier.push((neighbour.clone(), seed.clone()));
            }
        }
    }
    if seed_of.len() == nodes.len() {
        return seed_of;
    }

    let mut unreached: Vec<String> = Vec::new();
    for node in nodes {
        if !seed_of.contains_key(&node.support_id) && !unreached.contains(&node.support_id) {
            unreached.push(node.support_id.clone());
        }
    }
    loop {
        let before = unreached.len();
        unreached.retain(|id| {
            let joined = links_of(links, id)
                .iter()
                .find_map(|n| seed_of.get(n).cloned());
            match joined {
                Some(seed) => {
                    seed_of.insert(id.clone(), seed);
                    false
                }
                None => true,
            }
        });
        if unreached.is_empty() || unreached.len() == before {
            break;
        }
    }
    for id in unreached {
        seed_of.insert(id.clone(), id);
    }
    seed_of
}

/// The bracing groups of `supports`: per model in order of first
/// appearance, its groups in order of their first shaft, each group's ids
/// in id order.
pub(super) fn group_shafts(
    supports: &[GroupNode],
    settings: &GroupingSettings,
) -> Vec<Vec<String>> {
    let mut by_model: IndexMap<&str, Vec<GroupNode>> = IndexMap::new();
    for support in supports {
        by_model
            .entry(support.model_id.as_str())
            .or_default()
            .push(support.clone());
    }

    let mut groups = Vec::new();
    for nodes in by_model.values() {
        let links = link_shafts(nodes, settings.max_neighbor_distance_mm);
        let node_by_id: HashMap<&str, &GroupNode> =
            nodes.iter().map(|n| (n.support_id.as_str(), n)).collect();
        let clusters = clusters(nodes, &links);

        let mut seeds = grid_seeds(nodes, settings);
        // A cluster no grid seed landed in is seeded at its centroid.
        for cluster in &clusters {
            if cluster.iter().any(|id| seeds.contains(id)) {
                continue;
            }
            let members: Vec<&GroupNode> = cluster
                .iter()
                .filter_map(|id| node_by_id.get(id.as_str()).copied())
                .collect();
            if members.is_empty() {
                continue;
            }
            let (mut cx, mut cy) = (0.0, 0.0);
            for m in &members {
                cx += m.point.x;
                cy += m.point.y;
            }
            cx /= members.len() as f64;
            cy /= members.len() as f64;
            if let Some(id) = nearest_of(Vec2 { x: cx, y: cy }, &members) {
                seeds.insert(id);
            }
        }
        for cluster in &clusters {
            if let Some(first) = cluster.first()
                && !cluster.iter().any(|id| seeds.contains(id))
            {
                seeds.insert(first.clone());
            }
        }
        if seeds.is_empty()
            && let Some(first) = nodes.first()
        {
            seeds.insert(first.support_id.clone());
        }

        let seed_of = grow_groups(nodes, &links, &seeds);
        let mut by_seed: IndexMap<&str, Vec<String>> = IndexMap::new();
        for node in nodes {
            if let Some(seed) = seed_of.get(&node.support_id) {
                by_seed
                    .entry(seed.as_str())
                    .or_default()
                    .push(node.support_id.clone());
            }
        }
        for (_, mut group) in by_seed {
            group.sort_by(|a, b| id_order(a, b));
            groups.push(group);
        }
    }
    groups
}

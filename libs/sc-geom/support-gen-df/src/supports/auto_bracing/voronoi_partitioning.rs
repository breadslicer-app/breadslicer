//! DragonFruit `src/supports/autoBracing/voronoiPartitioning.ts`: split each
//! model's supports into bracing groups — seeds on a jittered grid snapped to
//! the nearest support, a fallback seed per connected island, then a
//! breadth-first claim over the neighbour graph. The debug seed markers are
//! the GUI's and are not kept.

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};

use crate::three::math_utils::{clamp, js_max, js_sin};
use crate::volume_analysis::islands::voxel_footprint::number_key;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoronoiSupportNode {
    pub support_id: String,
    pub model_id: String,
    pub point: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoronoiPartitionSettings {
    pub seed_spacing_mm: f64,
    pub seed_jitter_mm: f64,
    pub max_neighbor_distance_mm: f64,
}

const EPS: f64 = 0.000001;

/// FNV-1a over UTF-16 code units, as `hashString`.
fn hash_string(input: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for unit in input.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

fn pseudo_random01(a: f64, b: f64, seed: f64) -> f64 {
    let n = js_sin(a * 12.9898 + b * 78.233 + seed * 0.0001) * 43758.5453123;
    n - n.floor()
}

fn squared_distance(a: Vec2, b: Vec2) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

/// `${ix}:${iy}`: the string key makes -0 and 0 (and every NaN) one bucket.
fn bucket_key(ix: f64, iy: f64) -> (u64, u64) {
    (number_key(ix), number_key(iy))
}

type Buckets<'n> = HashMap<(u64, u64), Vec<&'n VoronoiSupportNode>>;

fn bucket(nodes: &[VoronoiSupportNode], cell_size: f64) -> Buckets<'_> {
    let mut buckets: Buckets<'_> = HashMap::new();
    for node in nodes {
        let key = bucket_key(
            (node.point.x / cell_size).floor(),
            (node.point.y / cell_size).floor(),
        );
        buckets.entry(key).or_default().push(node);
    }
    buckets
}

fn build_adjacency(
    nodes: &[VoronoiSupportNode],
    max_neighbor_distance_mm: f64,
) -> HashMap<String, Vec<String>> {
    let mut adjacency: IndexMap<&str, IndexSet<&str>> = nodes
        .iter()
        .map(|n| (n.support_id.as_str(), IndexSet::new()))
        .collect();
    if nodes.is_empty() {
        return HashMap::new();
    }
    // A NaN distance must not collapse every node into one bucket.
    let safe_max = if max_neighbor_distance_mm.is_finite() {
        js_max(max_neighbor_distance_mm, 0.1)
    } else {
        0.1
    };
    let cell_size = safe_max;
    let buckets = bucket(nodes, cell_size);
    let max_dist_sq = safe_max * safe_max;
    for node in nodes {
        let ix = (node.point.x / cell_size).floor();
        let iy = (node.point.y / cell_size).floor();
        for ox in -1..=1 {
            for oy in -1..=1 {
                let Some(neighbor_bucket) =
                    buckets.get(&bucket_key(ix + f64::from(ox), iy + f64::from(oy)))
                else {
                    continue;
                };
                for other in neighbor_bucket {
                    // `other.supportId <= node.supportId`: code-unit order.
                    if other
                        .support_id
                        .encode_utf16()
                        .cmp(node.support_id.encode_utf16())
                        .is_le()
                    {
                        continue;
                    }
                    if squared_distance(node.point, other.point) > max_dist_sq + EPS {
                        continue;
                    }
                    if let Some(set) = adjacency.get_mut(node.support_id.as_str()) {
                        set.insert(other.support_id.as_str());
                    }
                    if let Some(set) = adjacency.get_mut(other.support_id.as_str()) {
                        set.insert(node.support_id.as_str());
                    }
                }
            }
        }
    }
    adjacency
        .into_iter()
        .map(|(id, neighbors)| {
            let mut sorted: Vec<String> = neighbors.into_iter().map(str::to_owned).collect();
            sorted.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            (id.to_owned(), sorted)
        })
        .collect()
}

fn find_connected_islands(
    nodes: &[VoronoiSupportNode],
    adjacency: &HashMap<String, Vec<String>>,
) -> Vec<Vec<String>> {
    let mut islands = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    for node in nodes {
        if visited.contains(&node.support_id) {
            continue;
        }
        let mut island = Vec::new();
        let mut queue = vec![node.support_id.clone()];
        visited.insert(node.support_id.clone());
        let mut cursor = 0;
        while cursor < queue.len() {
            let current = queue[cursor].clone();
            cursor += 1;
            island.push(current.clone());
            for neighbor in adjacency.get(&current).map_or(&[][..], Vec::as_slice) {
                if visited.insert(neighbor.clone()) {
                    queue.push(neighbor.clone());
                }
            }
        }
        islands.push(island);
    }
    islands
}

fn nearest_node_id<'n>(target: Vec2, nodes: &[&'n VoronoiSupportNode]) -> Option<&'n str> {
    let mut best: Option<&VoronoiSupportNode> = None;
    let mut best_dist_sq = f64::INFINITY;
    for node in nodes {
        let dist_sq = squared_distance(target, node.point);
        if dist_sq < best_dist_sq {
            best = Some(node);
            best_dist_sq = dist_sq;
        }
    }
    best.map(|n| n.support_id.as_str())
}

fn nearest_node_id_from_buckets<'n>(
    target: Vec2,
    buckets: &Buckets<'n>,
    cell_size: f64,
    max_distance_mm: f64,
) -> Option<&'n str> {
    let max_dist_sq = max_distance_mm * max_distance_mm;
    let ix = (target.x / cell_size).floor();
    let iy = (target.y / cell_size).floor();
    let bucket_radius = js_max(1.0, (max_distance_mm / cell_size).ceil()) as i64;
    let mut best: Option<&VoronoiSupportNode> = None;
    let mut best_dist_sq = max_dist_sq + EPS;
    for ox in -bucket_radius..=bucket_radius {
        for oy in -bucket_radius..=bucket_radius {
            let Some(bucket) = buckets.get(&bucket_key(ix + ox as f64, iy + oy as f64)) else {
                continue;
            };
            for node in bucket {
                let dist_sq = squared_distance(target, node.point);
                if dist_sq > max_dist_sq + EPS || dist_sq >= best_dist_sq {
                    continue;
                }
                best = Some(node);
                best_dist_sq = dist_sq;
            }
        }
    }
    best.map(|n| n.support_id.as_str())
}

fn build_seeds(
    nodes: &[VoronoiSupportNode],
    settings: &VoronoiPartitionSettings,
) -> IndexSet<String> {
    let mut seeds = IndexSet::new();
    let Some(first) = nodes.first() else {
        return seeds;
    };
    let (mut min_x, mut max_x, mut min_y, mut max_y) =
        (first.point.x, first.point.x, first.point.y, first.point.y);
    for node in nodes {
        if node.point.x < min_x {
            min_x = node.point.x;
        }
        if node.point.x > max_x {
            max_x = node.point.x;
        }
        if node.point.y < min_y {
            min_y = node.point.y;
        }
        if node.point.y > max_y {
            max_y = node.point.y;
        }
    }
    let spacing = js_max(settings.seed_spacing_mm, 0.25);
    let jitter = clamp(settings.seed_jitter_mm, 0.0, spacing * 0.49);
    let snap_radius = js_max(spacing * 0.75, 0.25);
    let cell_size = js_max(snap_radius, 0.1);
    let buckets = bucket(nodes, cell_size);
    let model_seed = f64::from(hash_string(&first.model_id));

    let mut ix = 0.0;
    let mut gx = min_x;
    while gx <= max_x + EPS {
        let mut iy = 0.0;
        let mut gy = min_y;
        while gy <= max_y + EPS {
            let jx = (pseudo_random01(ix, iy, model_seed) * 2.0 - 1.0) * jitter;
            let jy = (pseudo_random01(ix, iy, model_seed + 7919.0) * 2.0 - 1.0) * jitter;
            let target = Vec2 {
                x: gx + jx,
                y: gy + jy,
            };
            if let Some(snapped) =
                nearest_node_id_from_buckets(target, &buckets, cell_size, snap_radius)
            {
                seeds.insert(snapped.to_owned());
            }
            gy += spacing;
            iy += 1.0;
        }
        gx += spacing;
        ix += 1.0;
    }
    seeds
}

fn apply_island_fallback_seeds(
    islands: &[Vec<String>],
    node_by_id: &HashMap<&str, &VoronoiSupportNode>,
    seeds: &mut IndexSet<String>,
) {
    for island in islands {
        if island.iter().any(|id| seeds.contains(id)) {
            continue;
        }
        let (mut cx, mut cy) = (0.0, 0.0);
        let mut island_nodes: Vec<&VoronoiSupportNode> = Vec::new();
        for id in island {
            let Some(node) = node_by_id.get(id.as_str()) else {
                continue;
            };
            island_nodes.push(node);
            cx += node.point.x;
            cy += node.point.y;
        }
        if island_nodes.is_empty() {
            continue;
        }
        cx /= island_nodes.len() as f64;
        cy /= island_nodes.len() as f64;
        if let Some(snapped) = nearest_node_id(Vec2 { x: cx, y: cy }, &island_nodes) {
            seeds.insert(snapped.to_owned());
        }
    }
}

fn multi_source_claim(
    nodes: &[VoronoiSupportNode],
    adjacency: &HashMap<String, Vec<String>>,
    seeds: &IndexSet<String>,
) -> HashMap<String, String> {
    let mut claimed: HashMap<String, String> = HashMap::new();
    let mut queue: Vec<(String, String)> = Vec::new();
    let mut sorted_seeds: Vec<&String> = seeds.iter().collect();
    sorted_seeds.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    for center in sorted_seeds {
        claimed.insert(center.clone(), center.clone());
        queue.push((center.clone(), center.clone()));
    }
    let mut cursor = 0;
    while cursor < queue.len() {
        let (support_id, center_id) = queue[cursor].clone();
        cursor += 1;
        for neighbor in adjacency.get(&support_id).map_or(&[][..], Vec::as_slice) {
            if claimed.contains_key(neighbor) {
                continue;
            }
            claimed.insert(neighbor.clone(), center_id.clone());
            queue.push((neighbor.clone(), center_id.clone()));
        }
    }
    if claimed.len() == nodes.len() {
        return claimed;
    }

    let mut stranded: IndexSet<String> = nodes
        .iter()
        .filter(|n| !claimed.contains_key(&n.support_id))
        .map(|n| n.support_id.clone())
        .collect();
    let mut progress = true;
    while progress && !stranded.is_empty() {
        progress = false;
        for support_id in stranded.clone() {
            let attached = adjacency
                .get(&support_id)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .find_map(|n| claimed.get(n).cloned());
            let Some(center) = attached else {
                continue;
            };
            claimed.insert(support_id.clone(), center);
            stranded.shift_remove(&support_id);
            progress = true;
        }
    }
    for support_id in stranded {
        claimed.insert(support_id.clone(), support_id);
    }
    claimed
}

/// `partitionSupportsWithVoronoi`: groups of support ids, per model in first
/// appearance order, each group sorted.
pub fn partition_supports_with_voronoi(
    supports: &[VoronoiSupportNode],
    settings: &VoronoiPartitionSettings,
) -> Vec<Vec<String>> {
    let mut by_model: IndexMap<&str, Vec<VoronoiSupportNode>> = IndexMap::new();
    for support in supports {
        by_model
            .entry(support.model_id.as_str())
            .or_default()
            .push(support.clone());
    }

    let mut groups_across_models = Vec::new();
    for model_supports in by_model.values() {
        let adjacency = build_adjacency(model_supports, settings.max_neighbor_distance_mm);
        let node_by_id: HashMap<&str, &VoronoiSupportNode> = model_supports
            .iter()
            .map(|n| (n.support_id.as_str(), n))
            .collect();
        let islands = find_connected_islands(model_supports, &adjacency);

        let mut seeds = build_seeds(model_supports, settings);
        apply_island_fallback_seeds(&islands, &node_by_id, &mut seeds);
        for island in &islands {
            if island.is_empty() || island.iter().any(|id| seeds.contains(id)) {
                continue;
            }
            seeds.insert(island[0].clone());
        }
        if seeds.is_empty()
            && let Some(first) = model_supports.first()
        {
            seeds.insert(first.support_id.clone());
        }

        let claimed = multi_source_claim(model_supports, &adjacency, &seeds);
        let mut groups_by_center: IndexMap<&str, Vec<String>> = IndexMap::new();
        for support in model_supports {
            let Some(center) = claimed.get(&support.support_id) else {
                continue;
            };
            groups_by_center
                .entry(center.as_str())
                .or_default()
                .push(support.support_id.clone());
        }
        for (_, mut group) in groups_by_center {
            group.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            groups_across_models.push(group);
        }
    }
    groups_across_models
}

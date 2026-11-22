//! The nodes around a node, nearest rings first.

use super::grid_math::{key_indices, node_key};

/// The keys of the nodes in square rings round `preferred_key`, up to
/// `max_rings` out: the node itself first, then each ring by `dx`, then
/// `dy`.
pub fn build_nearest_candidate_node_keys(preferred_key: &str, max_rings: u32) -> Vec<String> {
    let (cx, cy) = key_indices(preferred_key);
    let max_rings = i64::from(max_rings);
    let mut keys = Vec::new();
    for ring in 0..=max_rings {
        for dx in -ring..=ring {
            for dy in -ring..=ring {
                if dx.abs().max(dy.abs()) == ring {
                    keys.push(node_key(cx + dx as f64, cy + dy as f64));
                }
            }
        }
    }
    keys
}

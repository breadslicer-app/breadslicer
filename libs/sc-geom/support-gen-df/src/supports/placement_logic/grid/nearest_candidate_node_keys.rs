//! DragonFruit `src/supports/PlacementLogic/Grid/nearestCandidateNodeKeys.ts`.

use super::grid_math::js_number_to_string;

/// `Number(part)` for a key part: a missing part is NaN, an empty one 0.
fn key_part(part: Option<&str>) -> f64 {
    match part.map(str::trim) {
        None => f64::NAN,
        Some("") => 0.0,
        Some(p) => p.parse::<f64>().unwrap_or(f64::NAN),
    }
}

/// The node keys in square rings around `preferred_key`, ring 0 (the key
/// itself) first, each ring in `dx`-major then `dy` order.
pub fn build_nearest_candidate_node_keys(preferred_key: &str, max_rings: u32) -> Vec<String> {
    let mut parts = preferred_key.split(',');
    let center_x = key_part(parts.next());
    let center_y = key_part(parts.next());
    let mut keys = Vec::new();

    let max_rings = i64::from(max_rings);
    for ring in 0..=max_rings {
        for dx in -ring..=ring {
            for dy in -ring..=ring {
                if dx.abs().max(dy.abs()) != ring {
                    continue;
                }
                keys.push(format!(
                    "{},{}",
                    js_number_to_string(center_x + dx as f64),
                    js_number_to_string(center_y + dy as f64)
                ));
            }
        }
    }

    keys
}

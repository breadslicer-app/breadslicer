//! DragonFruit `src/supports/autoBracing/{initialPattern,repeatingPattern,zigzagChain}.ts`:
//! how a tier of pairs is braced, and the zigzag chains that replace tiers.

use super::settings::AutoBracingPattern;
use crate::supports::types::BraceDebugSection as BraceSection;
use crate::three::math_utils::js_max;

/// A pair to brace: two sample handles and their horizontal distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BracePair<T> {
    pub a: T,
    pub b: T,
    pub h_dist: f64,
}

/// `applyInitialPattern` / `applyRepeatingPattern`: one diagonal per pair, two
/// for the cross pattern.
pub fn apply_tier_pattern<T: Copy>(
    pairs: &[BracePair<T>],
    pattern: AutoBracingPattern,
    section: BraceSection,
    place: &mut dyn FnMut(T, T, BraceSection),
) {
    for edge in pairs {
        place(edge.a, edge.b, section);
        if pattern == AutoBracingPattern::CrossDiagonal {
            place(edge.b, edge.a, section);
        }
    }
}

const MAX_CHAIN_LINKS: usize = 200;

/// `runZigZagChain`: per pair, a continuous chain climbing by its own rise
/// (never less than `min_rise_mm`), alternating ends, mirrored per pair.
pub fn run_zig_zag_chain<T: Copy>(
    edges: &[BracePair<T>],
    start_z: f64,
    max_z: f64,
    first_section: BraceSection,
    place: &mut dyn FnMut(T, T, BraceSection, f64, f64),
    min_rise_mm: f64,
) {
    for (edge_index, edge) in edges.iter().enumerate() {
        if !(edge.h_dist > 0.0) {
            continue;
        }
        let rise = js_max(edge.h_dist, min_rise_mm);
        let (mut low, mut high) = if edge_index % 2 == 0 {
            (edge.a, edge.b)
        } else {
            (edge.b, edge.a)
        };
        let mut z = start_z;
        let mut section = first_section;
        let mut link = 0;
        while link < MAX_CHAIN_LINKS && z < max_z {
            place(low, high, section, z, rise);
            z += rise;
            std::mem::swap(&mut low, &mut high);
            section = BraceSection::Repeating;
            link += 1;
        }
    }
}

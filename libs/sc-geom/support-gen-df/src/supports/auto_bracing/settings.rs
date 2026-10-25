//! DragonFruit `src/supports/autoBracing/settings.ts`.

use serde::{Deserialize, Serialize};

use crate::supports::numeric_constraint::{NumericConstraint, clamp_numeric};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoBracingPattern {
    SingleDiagonal,
    CrossDiagonal,
    ZigZag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoBracingSettings {
    pub brace_diameter_mm: f64,
    pub initial_pattern: AutoBracingPattern,
    pub initial_distance_mm: f64,
    pub repeating_pattern: AutoBracingPattern,
    pub pattern_interval_mm: f64,
    pub seed_spacing_mm: f64,
    pub seed_jitter_mm: f64,
    pub max_brace_length_mm: f64,
    pub debug_section_colors_enabled: bool,
    pub debug_voronoi_seeds_enabled: bool,
    /// Remove existing bracing before generating. Only braces and kickstands
    /// this tool generated are removed; `generatedBy` distinguishes them.
    pub remove_existing_bracing: bool,
}

impl Default for AutoBracingSettings {
    fn default() -> Self {
        create_default_auto_bracing_settings()
    }
}

pub const AUTO_BRACING_PATTERN_OPTIONS: [AutoBracingPattern; 3] = [
    AutoBracingPattern::CrossDiagonal,
    AutoBracingPattern::SingleDiagonal,
    AutoBracingPattern::ZigZag,
];

pub mod auto_bracing_constraints {
    use crate::supports::numeric_constraint::NumericConstraint;

    pub const BRACE_DIAMETER_MM: NumericConstraint = NumericConstraint::new(0.5, 2.0, 0.05, 1.0);
    pub const INITIAL_DISTANCE_MM: NumericConstraint = NumericConstraint::new(0.1, 25.0, 0.1, 2.0);
    pub const PATTERN_INTERVAL_MM: NumericConstraint = NumericConstraint::new(1.0, 50.0, 0.1, 8.0);
    pub const SEED_SPACING_MM: NumericConstraint = NumericConstraint::new(2.0, 60.0, 0.5, 10.0);
    pub const SEED_JITTER_MM: NumericConstraint = NumericConstraint::new(0.0, 20.0, 0.25, 1.0);
    pub const MAX_BRACE_LENGTH_MM: NumericConstraint = NumericConstraint::new(1.0, 50.0, 0.1, 10.0);
}

/// `DEBUG_SECTION_COLORS`.
pub const DEBUG_SECTION_COLOR_INITIAL: &str = "#00ff00"; // Green
pub const DEBUG_SECTION_COLOR_REPEATING: &str = "#00e5ff"; // Light Blue

/// `AUTO_BRACING_HARD_RULES`.
pub mod auto_bracing_hard_rules {
    pub const BRACE_ANGLE_DEG: f64 = 45.0;
    pub const MIN_GROUP_SIZE: usize = 3;
    /// A second axis must be separated by at least the ladder's own brace
    /// angle: the 45° X-pattern diagonals qualify as a second axis, so a
    /// braced trunk reads as 2-axis and does NOT get a kickstand. (Was 50° —
    /// stricter than the braces the ladder actually places, so dense forests
    /// of X-braced trunks still spawned kickstands.)
    pub const MIN_AXIS_SEPARATION_DEG: f64 = 45.0;
    pub const TARGET_AXIS_SEPARATION_DEG: f64 = 90.0;
    pub const KICKSTAND_MESH_CLEARANCE_MM: f64 = 0.5;
    /// A zig-zag link rises by its own horizontal span, so a pair of nearly
    /// coincident trunks used to stack one 45° stub per fraction of a mm —
    /// hundreds of near-parallel links filling the gap. Two floors keep that
    /// from happening without starving close-but-real pairs of bracing:
    ///  * the chain never climbs less than minZigZagRiseMm per link — a pitch
    ///    below one brace thickness is unprintable mush; the link simply
    ///    steepens and still starts where the previous one ended, and
    ///  * supports closer than minPairSpanMm are one post (raised to the brace
    ///    diameter, since two surfaces that overlap cannot be bridged).
    ///
    /// Both are deliberately small: they only reject degenerate pairs.
    pub const MIN_ZIG_ZAG_RISE_MM: f64 = 1.0;
    pub const MIN_PAIR_SPAN_MM: f64 = 0.7;
}

pub fn create_default_auto_bracing_settings() -> AutoBracingSettings {
    use auto_bracing_constraints as c;
    AutoBracingSettings {
        brace_diameter_mm: c::BRACE_DIAMETER_MM.default_value,
        initial_pattern: AutoBracingPattern::CrossDiagonal,
        initial_distance_mm: c::INITIAL_DISTANCE_MM.default_value,
        repeating_pattern: AutoBracingPattern::CrossDiagonal,
        pattern_interval_mm: c::PATTERN_INTERVAL_MM.default_value,
        seed_spacing_mm: c::SEED_SPACING_MM.default_value,
        seed_jitter_mm: c::SEED_JITTER_MM.default_value,
        max_brace_length_mm: c::MAX_BRACE_LENGTH_MM.default_value,
        debug_section_colors_enabled: false,
        debug_voronoi_seeds_enabled: false,
        remove_existing_bracing: true,
    }
}

/// `normalizeAutoBracingSettings`. Every field of a Rust value is present, so
/// the type coercions of the TypeScript reduce to the numeric clamps.
pub fn normalize_auto_bracing_settings(source: &AutoBracingSettings) -> AutoBracingSettings {
    use auto_bracing_constraints as c;
    let clamp = |value: f64, constraint: NumericConstraint| clamp_numeric(Some(value), constraint);
    AutoBracingSettings {
        brace_diameter_mm: clamp(source.brace_diameter_mm, c::BRACE_DIAMETER_MM),
        initial_pattern: source.initial_pattern,
        initial_distance_mm: clamp(source.initial_distance_mm, c::INITIAL_DISTANCE_MM),
        repeating_pattern: source.repeating_pattern,
        pattern_interval_mm: clamp(source.pattern_interval_mm, c::PATTERN_INTERVAL_MM),
        seed_spacing_mm: clamp(source.seed_spacing_mm, c::SEED_SPACING_MM),
        seed_jitter_mm: clamp(source.seed_jitter_mm, c::SEED_JITTER_MM),
        max_brace_length_mm: clamp(source.max_brace_length_mm, c::MAX_BRACE_LENGTH_MM),
        debug_section_colors_enabled: source.debug_section_colors_enabled,
        debug_voronoi_seeds_enabled: source.debug_voronoi_seeds_enabled,
        remove_existing_bracing: source.remove_existing_bracing,
    }
}

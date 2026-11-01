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
    /// SoulCrafted (#360): a bracing preset. When set, braces climb every
    /// pair in a chain from `initial_distance_mm`, its links `gap_mm` apart,
    /// with patterns for the bottom, middle and top links and rings at the
    /// reinforcement heights; every brace is drawn at
    /// `brace_diameter_mm` (not at its hosts' diameter). The tier fields
    /// above are not read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stacked: Option<StackedBracing>,
}

/// How the links of a stacked chain are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GridDiagonal {
    None,
    Single,
    Cross,
}

/// A bracing preset (SoulCrafted, #360); see `AutoBracingSettings::stacked`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StackedBracing {
    /// Vertical gap between one link's top and the next link's bottom.
    pub gap_mm: f64,
    pub bottom_pattern: AutoBracingPattern,
    /// How many links from the bottom take the bottom pattern.
    pub bottom_repeat: u32,
    pub middle_pattern: AutoBracingPattern,
    pub top_pattern: AutoBracingPattern,
    /// How many links from the top take the top pattern.
    pub top_repeat: u32,
    /// Heights (mm above the plate) of reinforcement rings between every
    /// pair, each drawn as a cross at that height (a level brace would start
    /// in mid-air).
    pub reinforcement_heights_mm: Vec<f64>,
    /// The diagonal each ring adds in the link above it.
    pub reinforcement_diagonal: GridDiagonal,
}

impl Default for StackedBracing {
    fn default() -> Self {
        StackedBracing {
            gap_mm: 0.0,
            bottom_pattern: AutoBracingPattern::SingleDiagonal,
            bottom_repeat: 0,
            middle_pattern: AutoBracingPattern::SingleDiagonal,
            top_pattern: AutoBracingPattern::SingleDiagonal,
            top_repeat: 0,
            reinforcement_heights_mm: Vec::new(),
            reinforcement_diagonal: GridDiagonal::None,
        }
    }
}

impl StackedBracing {
    /// The pattern link `index` of a chain of `links` takes.
    pub fn pattern_of(&self, index: u32, links: u32) -> AutoBracingPattern {
        if index < self.bottom_repeat {
            self.bottom_pattern
        } else if index + self.top_repeat >= links {
            self.top_pattern
        } else {
            self.middle_pattern
        }
    }
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
        stacked: None,
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
        stacked: source.stacked.as_ref().map(|st| StackedBracing {
            gap_mm: if st.gap_mm.is_finite() {
                st.gap_mm.clamp(0.0, 50.0)
            } else {
                0.0
            },
            bottom_repeat: st.bottom_repeat.min(100),
            top_repeat: st.top_repeat.min(100),
            reinforcement_heights_mm: st
                .reinforcement_heights_mm
                .iter()
                .copied()
                .filter(|h| h.is_finite() && *h > 0.0)
                .collect(),
            ..st.clone()
        }),
    }
}

//! What a bracing run is told: the brace size, how pairs of shafts are
//! braced up their height, how shafts are grouped, and the fixed rules no
//! setting changes.

use serde::{Deserialize, Serialize};

use crate::supports::numeric_constraint::{NumericConstraint, clamp_numeric};

/// How a pair of shafts is braced at one height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoBracingPattern {
    /// One diagonal from the first shaft up to the second.
    SingleDiagonal,
    /// Two diagonals, one each way: an X.
    CrossDiagonal,
    /// A continuous chain climbing the pair, each link leaning the other way.
    ZigZag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoBracingSettings {
    pub brace_diameter_mm: f64,
    /// The pattern of the lowest tier.
    pub initial_pattern: AutoBracingPattern,
    /// Height of the lowest tier above the plate.
    pub initial_distance_mm: f64,
    /// The pattern of every tier above the lowest.
    pub repeating_pattern: AutoBracingPattern,
    /// Vertical distance between tiers.
    pub pattern_interval_mm: f64,
    /// Spacing of the seeds that split a model's shafts into groups.
    pub seed_spacing_mm: f64,
    /// How far a seed may stray from its grid point.
    pub seed_jitter_mm: f64,
    pub max_brace_length_mm: f64,
    pub debug_section_colors_enabled: bool,
    pub debug_voronoi_seeds_enabled: bool,
    /// Drop the braces and kickstands an earlier run made (`generatedBy`
    /// tells them apart) before bracing; what the person placed stays.
    pub remove_existing_bracing: bool,
    /// A bracing preset (#360). When set, every pair climbs in one chain
    /// from `initial_distance_mm`, its links `gap_mm` apart, with one
    /// pattern for the bottom links, one for the middle and one for the top,
    /// and a ring at each reinforcement height; braces are drawn at
    /// `brace_diameter_mm`, not at the width of the shafts they join. The
    /// tier fields are then unused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stacked: Option<StackedBracing>,
    /// Models whose supports are braced as one structure (#475), by id. A
    /// brace between two of them belongs to the first model of the group and
    /// must keep clear of every model in it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brace_together: Vec<Vec<String>>,
    /// Size every brace from the thinner of the two shafts it joins (#471),
    /// as [`BRACE_SHARE_OF_TRUNK`](crate::supports::auto_support::zone_rules::BRACE_SHARE_OF_TRUNK)
    /// of its width, instead of `brace_diameter_mm`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub follow_trunks: bool,
}

impl AutoBracingSettings {
    /// The id model `model_id` is braced under (#475): the first model of
    /// its group, or its own.
    pub fn bracing_key<'a>(&'a self, model_id: &'a str) -> &'a str {
        self.brace_together
            .iter()
            .find(|group| group.iter().any(|m| m == model_id))
            .and_then(|group| group.first())
            .map_or(model_id, String::as_str)
    }

    /// Every model braced under `key` (#475): its group, or `key` alone.
    pub fn braced_with<'a>(&'a self, key: &'a str) -> Vec<&'a str> {
        match self
            .brace_together
            .iter()
            .find(|group| group.first().is_some_and(|first| first == key))
        {
            Some(group) => group.iter().map(String::as_str).collect(),
            None => vec![key],
        }
    }
}

/// Which diagonal a reinforcement ring adds above itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GridDiagonal {
    None,
    Single,
    Cross,
}

/// A bracing preset (#360); see [`AutoBracingSettings::stacked`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StackedBracing {
    /// Vertical gap from the top of one link to the bottom of the next.
    pub gap_mm: f64,
    pub bottom_pattern: AutoBracingPattern,
    /// How many links, counted from the bottom, use `bottom_pattern`.
    pub bottom_repeat: u32,
    pub middle_pattern: AutoBracingPattern,
    pub top_pattern: AutoBracingPattern,
    /// How many links, counted from the top, use `top_pattern`.
    pub top_repeat: u32,
    /// Heights above the plate where every pair gets a ring: a cross at that
    /// height (a level brace would begin in mid-air).
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
    /// The pattern of link `index` (from 0 at the bottom) in a chain of
    /// `links`. The bottom links win over the top ones in a short chain.
    pub fn pattern_of(&self, index: u32, links: u32) -> AutoBracingPattern {
        if index < self.bottom_repeat {
            return self.bottom_pattern;
        }
        if index + self.top_repeat >= links {
            return self.top_pattern;
        }
        self.middle_pattern
    }
}

impl Default for AutoBracingSettings {
    fn default() -> Self {
        create_default_auto_bracing_settings()
    }
}

/// The range, step and default of each numeric setting.
pub mod auto_bracing_constraints {
    use crate::supports::numeric_constraint::NumericConstraint;

    pub const BRACE_DIAMETER_MM: NumericConstraint = NumericConstraint::new(0.5, 2.0, 0.05, 1.0);
    pub const INITIAL_DISTANCE_MM: NumericConstraint = NumericConstraint::new(0.1, 25.0, 0.1, 2.0);
    pub const PATTERN_INTERVAL_MM: NumericConstraint = NumericConstraint::new(1.0, 50.0, 0.1, 8.0);
    pub const SEED_SPACING_MM: NumericConstraint = NumericConstraint::new(2.0, 60.0, 0.5, 10.0);
    pub const SEED_JITTER_MM: NumericConstraint = NumericConstraint::new(0.0, 20.0, 0.25, 1.0);
    pub const MAX_BRACE_LENGTH_MM: NumericConstraint = NumericConstraint::new(1.0, 50.0, 0.1, 10.0);
}

/// Rules of every bracing run that no setting changes.
pub mod auto_bracing_hard_rules {
    /// Fewer shafts than this on the plate are not braced at all.
    pub const MIN_GROUP_SIZE: usize = 3;
    /// Two brace axes count as two directions from this angle on. It equals
    /// the 45° of the braces themselves, so the diagonals of an X already
    /// hold a shaft both ways and it needs no kickstand.
    pub const MIN_AXIS_SEPARATION_DEG: f64 = 45.0;
    /// The least a zigzag link climbs. A link climbs by the pair's own
    /// horizontal span, so two shafts almost touching would stack dozens of
    /// flat stubs into the gap; below one brace thickness the pitch cannot
    /// print, so the link steepens instead, still starting where the
    /// previous one ended.
    pub const MIN_ZIG_ZAG_RISE_MM: f64 = 1.0;
    /// Shafts closer than this (or than the brace diameter, if larger) are
    /// one post: there is no gap to bridge.
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
        brace_together: Vec::new(),
        follow_trunks: false,
    }
}

/// `source` with every number brought into its range; a preset's gap is
/// kept within 0–50 mm, its repeats within 100, and only its positive
/// finite ring heights are kept.
pub fn normalize_auto_bracing_settings(source: &AutoBracingSettings) -> AutoBracingSettings {
    use auto_bracing_constraints as c;
    let within = |value: f64, range: NumericConstraint| clamp_numeric(Some(value), range);
    let stacked = source.stacked.as_ref().map(|preset| StackedBracing {
        gap_mm: if preset.gap_mm.is_finite() {
            preset.gap_mm.clamp(0.0, 50.0)
        } else {
            0.0
        },
        bottom_repeat: preset.bottom_repeat.min(100),
        top_repeat: preset.top_repeat.min(100),
        reinforcement_heights_mm: preset
            .reinforcement_heights_mm
            .iter()
            .copied()
            .filter(|h| h.is_finite() && *h > 0.0)
            .collect(),
        ..preset.clone()
    });
    AutoBracingSettings {
        brace_diameter_mm: within(source.brace_diameter_mm, c::BRACE_DIAMETER_MM),
        initial_distance_mm: within(source.initial_distance_mm, c::INITIAL_DISTANCE_MM),
        pattern_interval_mm: within(source.pattern_interval_mm, c::PATTERN_INTERVAL_MM),
        seed_spacing_mm: within(source.seed_spacing_mm, c::SEED_SPACING_MM),
        seed_jitter_mm: within(source.seed_jitter_mm, c::SEED_JITTER_MM),
        max_brace_length_mm: within(source.max_brace_length_mm, c::MAX_BRACE_LENGTH_MM),
        stacked,
        ..source.clone()
    }
}

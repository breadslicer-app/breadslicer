//! Zone presets (#360): support setups that size every support by the zone
//! it lands in (see `support_gen_df::supports::auto_support::zone_rules`),
//! with their bracing, and the two that ship: "Minis 30 mm+" and
//! "Figures 70 mm+", Damian's setups tuned over many prints, value for value.

use serde::{Deserialize, Serialize};
use support_gen_df::supports::auto_bracing::settings::{
    AutoBracingPattern, AutoBracingSettings, GridDiagonal, StackedBracing,
};
use support_gen_df::supports::auto_support::zone_rules::{
    AreaBand, BaseTip, IgnoreSmallIslands, OverhangRule, StabilizationRule, SupportSize,
    VolumeBand, ZoneRules,
};

/// A bracing preset: the brace diameter, the height the bracing starts at
/// above the plate, and its chain (gap, patterns, reinforcement rings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BracingPreset {
    pub diameter_mm: f64,
    pub start_mm: f64,
    pub chain: StackedBracing,
}

impl Default for BracingPreset {
    fn default() -> Self {
        auto_support_bracing()
    }
}

impl BracingPreset {
    /// Writes the preset into `settings`, keeping everything it does not set.
    pub fn apply(&self, settings: &mut AutoBracingSettings) {
        settings.brace_diameter_mm = self.diameter_mm;
        settings.initial_distance_mm = self.start_mm;
        settings.stacked = Some(self.chain.clone());
    }
}

/// A whole zone preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ZonePreset {
    pub rules: ZoneRules,
    pub bracing: BracingPreset,
    /// How high the model stands above the plate for its supports, mm.
    pub auto_lift_mm: f64,
}

impl Default for ZonePreset {
    fn default() -> Self {
        minis_30()
    }
}

/// The trunk, base and base tip every J3D size shares; `tip` sets the rest.
fn j3d(id: &str, tip: f64) -> SupportSize {
    SupportSize {
        id: id.to_owned(),
        tip_diameter_mm: tip,
        tip_length_mm: 2.3,
        penetration_mm: 0.0,
        break_point_mm: 0.0,
        trunk_diameter_mm: 1.1,
        base_diameter_mm: 5.0,
        base_thickness_mm: 0.25,
        join_cone_mm: 0.4,
        base_tip: Some(BaseTip {
            length_mm: 2.0,
            diameter_mm: tip,
        }),
    }
}

/// A factory size: no join cone, no base tip of its own.
fn factory(
    id: &str,
    tip: f64,
    tip_length: f64,
    trunk: f64,
    base: f64,
    thickness: f64,
) -> SupportSize {
    SupportSize {
        id: id.to_owned(),
        tip_diameter_mm: tip,
        tip_length_mm: tip_length,
        penetration_mm: 0.0,
        break_point_mm: 0.0,
        trunk_diameter_mm: trunk,
        base_diameter_mm: base,
        base_thickness_mm: thickness,
        join_cone_mm: 0.0,
        base_tip: None,
    }
}

/// The sizes both setups pick from: the four J3D sizes and the three
/// factory ones.
pub fn support_sizes() -> Vec<SupportSize> {
    vec![
        j3d("extrasmall", 0.18),
        j3d("small", 0.24),
        j3d("medium", 0.30),
        j3d("large", 0.40),
        factory("light", 0.30, 2.0, 1.0, 6.0, 0.25),
        factory("factory-medium", 0.60, 3.0, 1.3, 9.0, 0.50),
        factory("heavy", 1.00, 4.0, 3.0, 12.0, 0.50),
    ]
}

/// The smallest tip there is: what goes on an island too narrow for the
/// tips of its preset, before it is given up as too small for any.
pub fn smallest_size() -> SupportSize {
    support_sizes()
        .into_iter()
        .min_by(|a, b| a.tip_diameter_mm.total_cmp(&b.tip_diameter_mm))
        .expect("the sizes that ship")
}

fn chain(
    gap_mm: f64,
    (bottom_pattern, bottom_repeat): (AutoBracingPattern, u32),
    middle_pattern: AutoBracingPattern,
    (top_pattern, top_repeat): (AutoBracingPattern, u32),
    reinforcement_heights_mm: [f64; 2],
    reinforcement_diagonal: GridDiagonal,
) -> StackedBracing {
    StackedBracing {
        gap_mm,
        bottom_pattern,
        bottom_repeat,
        middle_pattern,
        top_pattern,
        top_repeat,
        reinforcement_heights_mm: reinforcement_heights_mm.to_vec(),
        reinforcement_diagonal,
    }
}

const SIMPLE: AutoBracingPattern = AutoBracingPattern::SingleDiagonal;
const DOUBLE: AutoBracingPattern = AutoBracingPattern::CrossDiagonal;

/// "Auto support" bracing, the one both setups use: double diagonal at
/// the top, simple below.
pub fn auto_support_bracing() -> BracingPreset {
    BracingPreset {
        diameter_mm: 0.7,
        start_mm: 5.0,
        chain: chain(
            1.0,
            (SIMPLE, 1),
            SIMPLE,
            (DOUBLE, 1),
            [30.0, 60.0],
            GridDiagonal::None,
        ),
    }
}

/// The other bracing presets, by name, for a preset of the person's own.
pub fn bracing_presets() -> Vec<(&'static str, BracingPreset)> {
    let uniform = |p: AutoBracingPattern| (p, 0);
    vec![
        ("auto support", auto_support_bracing()),
        (
            "default",
            BracingPreset {
                diameter_mm: 0.9,
                start_mm: 1.0,
                chain: chain(
                    0.0,
                    (SIMPLE, 1),
                    SIMPLE,
                    (DOUBLE, 1),
                    [30.0, 60.0],
                    GridDiagonal::None,
                ),
            },
        ),
        (
            "small object",
            BracingPreset {
                diameter_mm: 0.8,
                start_mm: 1.0,
                chain: chain(
                    0.0,
                    uniform(DOUBLE),
                    DOUBLE,
                    uniform(DOUBLE),
                    [10.0, 30.0],
                    GridDiagonal::None,
                ),
            },
        ),
        (
            "tall object",
            BracingPreset {
                diameter_mm: 2.0,
                start_mm: 1.0,
                chain: chain(
                    0.0,
                    (DOUBLE, 1),
                    SIMPLE,
                    (DOUBLE, 2),
                    [20.0, 50.0],
                    GridDiagonal::Single,
                ),
            },
        ),
        (
            "strong",
            BracingPreset {
                diameter_mm: 1.0,
                // 0 in the setup; the bracing engine starts at 0.1 mm at least.
                start_mm: 0.1,
                chain: chain(
                    0.0,
                    uniform(DOUBLE),
                    DOUBLE,
                    uniform(DOUBLE),
                    [30.0, 50.0],
                    GridDiagonal::Cross,
                ),
            },
        ),
    ]
}

fn area(min_area_mm2: f64, spacing_mm: f64, size: &str) -> AreaBand {
    AreaBand {
        enabled: true,
        min_area_mm2,
        spacing_mm,
        size: size.to_owned(),
    }
}

fn volume(min_volume_mm3: f64, height_mm: f64, spacing_mm: f64, size: &str) -> VolumeBand {
    VolumeBand {
        enabled: true,
        min_volume_mm3,
        height_mm,
        spacing_mm,
        size: size.to_owned(),
    }
}

/// "Minis 30 mm+".
pub fn minis_30() -> ZonePreset {
    ZonePreset {
        rules: ZoneRules {
            sizes: support_sizes(),
            first_layer: vec![
                area(0.0, 0.8, "small"),
                area(1.05, 2.0, "medium"),
                area(50.0, 2.0, "large"),
            ],
            ignore_small_islands: IgnoreSmallIslands {
                enabled: false,
                volume_mm3: 0.02,
                height_mm: 0.1,
            },
            reinforcement: vec![
                volume(0.1, 1.5, 1.0, "small"),
                volume(10.0, 1.5, 1.8, "medium"),
                volume(95.0, 1.5, 2.5, "large"),
            ],
            overhang: OverhangRule {
                enabled: true,
                angle_deg: 38.0,
                spacing_mm: 1.8,
                size: "small".to_owned(),
                min_area_mm2: 0.8,
            },
            stabilization: StabilizationRule {
                enabled: true,
                spacing_mm: 3.8,
                size: "extrasmall".to_owned(),
                min_area_mm2: 4.0,
            },
            search_ground_mm: 30.0,
        },
        bracing: auto_support_bracing(),
        auto_lift_mm: 5.0,
    }
}

/// "Figures 70 mm+".
pub fn figures_70() -> ZonePreset {
    ZonePreset {
        rules: ZoneRules {
            sizes: support_sizes(),
            first_layer: vec![
                area(0.0, 1.4, "small"),
                area(5.0, 1.4, "medium"),
                area(10.0, 3.0, "large"),
            ],
            ignore_small_islands: IgnoreSmallIslands {
                enabled: true,
                volume_mm3: 0.02,
                height_mm: 0.1,
            },
            reinforcement: vec![
                volume(0.1, 1.8, 1.8, "small"),
                volume(10.0, 3.0, 1.8, "medium"),
                volume(200.0, 3.0, 1.8, "large"),
            ],
            overhang: OverhangRule {
                enabled: true,
                angle_deg: 35.0,
                spacing_mm: 2.8,
                size: "factory-medium".to_owned(),
                min_area_mm2: 8.84,
            },
            stabilization: StabilizationRule {
                enabled: true,
                spacing_mm: 10.0,
                size: "extrasmall".to_owned(),
                min_area_mm2: 10.0,
            },
            search_ground_mm: 30.0,
        },
        bracing: auto_support_bracing(),
        auto_lift_mm: 5.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_band_names_a_size_that_exists() {
        for preset in [minis_30(), figures_70()] {
            let r = &preset.rules;
            let names = r
                .first_layer
                .iter()
                .map(|b| &b.size)
                .chain(r.reinforcement.iter().map(|b| &b.size))
                .chain([&r.overhang.size, &r.stabilization.size]);
            for name in names {
                assert!(r.size_index(name).is_some(), "{name}");
            }
            assert_eq!(r.clamped(), *r, "the setups are inside every bound");
        }
    }

    #[test]
    fn the_sizes_are_the_setup_values() {
        let sizes = support_sizes();
        let tips: Vec<f64> = sizes.iter().map(|s| s.tip_diameter_mm).collect();
        assert_eq!(tips, [0.18, 0.24, 0.3, 0.4, 0.3, 0.6, 1.0]);
        let small = &sizes[1];
        assert_eq!(
            (
                small.tip_length_mm,
                small.trunk_diameter_mm,
                small.join_cone_mm
            ),
            (2.3, 1.1, 0.4)
        );
        assert_eq!(
            (small.base_diameter_mm, small.base_thickness_mm),
            (5.0, 0.25)
        );
        assert_eq!(small.base_tip.unwrap().length_mm, 2.0);
        assert_eq!(small.base_tip.unwrap().diameter_mm, 0.24);
        let medium = &sizes[5];
        assert_eq!(
            (
                medium.tip_diameter_mm,
                medium.tip_length_mm,
                medium.trunk_diameter_mm
            ),
            (0.6, 3.0, 1.3)
        );
        assert_eq!(
            (medium.base_diameter_mm, medium.base_thickness_mm),
            (9.0, 0.5)
        );
        assert!(medium.base_tip.is_none());
        assert!(
            sizes
                .iter()
                .all(|s| s.penetration_mm == 0.0 && s.break_point_mm == 0.0)
        );
    }
}
